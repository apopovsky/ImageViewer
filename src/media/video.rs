use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, TryRecvError};
use image::DynamicImage;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use super::decoder::{probe_video, CREATE_NO_WINDOW};

struct ActiveStream {
    rx: Receiver<egui::ColorImage>,
    stop_flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    reader_thread: Option<JoinHandle<()>>,
    eof_reached: Arc<AtomicBool>,
}

impl ActiveStream {
    fn shutdown(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        // Drain any remaining items so reader thread isn't blocked on bounded send
        while self.rx.try_recv().is_ok() {}
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct VideoPlayer {
    pub path: PathBuf,
    pub duration_secs: f64,
    pub fps: f64,
    pub stream_size: [usize; 2],
    pub original_size: (u32, u32),
    pub position_secs: f64,
    pub playing: bool,
    pub looping: bool,
    pub texture: Option<egui::TextureHandle>,
    pub last_frame: Option<egui::ColorImage>,
    pub error: Option<String>,
    stream: Option<ActiveStream>,
    last_tick: Instant,
    frame_accumulator: f64,
}

impl VideoPlayer {
    pub fn open(path: &Path, autoplay: bool) -> Result<Self, String> {
        let info = probe_video(path)?;
        let (sw, sh) = compute_stream_dims(info.width, info.height, 1920, 1080);

        let mut player = Self {
            path: path.to_path_buf(),
            duration_secs: info.duration_secs.max(0.0),
            fps: info.fps.clamp(1.0, 120.0),
            stream_size: [sw as usize, sh as usize],
            original_size: (info.width, info.height),
            position_secs: 0.0,
            playing: autoplay,
            looping: true,
            texture: None,
            last_frame: None,
            error: None,
            stream: None,
            last_tick: Instant::now(),
            frame_accumulator: 0.0,
        };
        player.start_stream_at(0.0)?;
        Ok(player)
    }

    pub fn seek(&mut self, target_secs: f64) {
        let clamped = if self.duration_secs > 0.0 {
            target_secs.clamp(0.0, (self.duration_secs - 0.05).max(0.0))
        } else {
            target_secs.max(0.0)
        };
        self.position_secs = clamped;
        self.frame_accumulator = 0.0;
        self.last_tick = Instant::now();
        if let Err(e) = self.start_stream_at(clamped) {
            self.error = Some(e);
        }
    }

    pub fn toggle_play(&mut self) {
        if !self.playing {
            let at_end = self.duration_secs > 0.0 && self.position_secs >= self.duration_secs - 0.1;
            if at_end {
                self.seek(0.0);
            }
        }
        self.playing = !self.playing;
        self.last_tick = Instant::now();
    }

    pub fn step_one_frame(&mut self, ctx: &egui::Context) {
        self.playing = false;
        self.pull_next_frame(ctx);
    }

    pub fn update(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.25);
        self.last_tick = now;

        // Ensure first frame appears even if started paused
        if self.texture.is_none() {
            self.pull_next_frame(ctx);
        }

        if !self.playing {
            return;
        }

        let frame_dur = 1.0 / self.fps;
        self.frame_accumulator += dt;

        let mut pulled = false;
        // Cap catch-up frames per UI tick to avoid spiraling
        let mut steps = 0;
        while self.frame_accumulator >= frame_dur && steps < 3 {
            self.frame_accumulator -= frame_dur;
            steps += 1;
            if self.pull_next_frame(ctx) {
                pulled = true;
            } else if self.is_eof() {
                if self.looping {
                    self.seek(0.0);
                    self.pull_next_frame(ctx);
                } else {
                    self.playing = false;
                }
                break;
            }
        }

        if pulled || self.playing {
            ctx.request_repaint_after(Duration::from_millis((frame_dur * 500.0).clamp(4.0, 33.0) as u64));
        }
    }

    pub fn current_dynamic_image(&self) -> Option<DynamicImage> {
        let img = self.last_frame.as_ref()?;
        let [w, h] = img.size;
        let mut raw = Vec::with_capacity(w * h * 4);
        for px in &img.pixels {
            raw.push(px.r());
            raw.push(px.g());
            raw.push(px.b());
            raw.push(px.a());
        }
        image::RgbaImage::from_raw(w as u32, h as u32, raw).map(DynamicImage::ImageRgba8)
    }

    fn is_eof(&self) -> bool {
        self.stream
            .as_ref()
            .map(|s| s.eof_reached.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    fn pull_next_frame(&mut self, ctx: &egui::Context) -> bool {
        let Some(stream) = self.stream.as_ref() else {
            return false;
        };
        match stream.rx.try_recv() {
            Ok(color_img) => {
                self.position_secs += 1.0 / self.fps;
                if self.duration_secs > 0.0 {
                    self.position_secs = self.position_secs.min(self.duration_secs);
                }
                match &mut self.texture {
                    Some(tex) => tex.set(color_img.clone(), egui::TextureOptions::LINEAR),
                    None => {
                        self.texture = Some(ctx.load_texture(
                            format!("video_frame_{}", self.path.display()),
                            color_img.clone(),
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                self.last_frame = Some(color_img);
                true
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => false,
        }
    }

    fn start_stream_at(&mut self, start_secs: f64) -> Result<(), String> {
        if let Some(mut prev) = self.stream.take() {
            prev.shutdown();
        }

        let [sw, sh] = self.stream_size;
        let vf = format!("scale={sw}:{sh}");
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        if start_secs > 0.01 {
            cmd.args(["-ss", &format!("{start_secs:.3}")]);
        }
        cmd.arg("-i")
            .arg(&self.path)
            .args([
                "-an",
                "-sn",
                "-vf",
                &vf,
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "-v",
                "error",
                "-",
            ]);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start video playback with ffmpeg: {e}"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Could not open ffmpeg stdout".to_string())?;

        let (tx, rx) = bounded::<egui::ColorImage>(3);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let eof_reached = Arc::new(AtomicBool::new(false));
        let child_arc = Arc::new(Mutex::new(Some(child)));

        let stop_clone = Arc::clone(&stop_flag);
        let eof_clone = Arc::clone(&eof_reached);
        let child_clone = Arc::clone(&child_arc);

        let frame_bytes = sw * sh * 4;
        let reader_thread = thread::spawn(move || {
            let mut buf = vec![0u8; frame_bytes];
            while !stop_clone.load(Ordering::SeqCst) {
                if stdout.read_exact(&mut buf).is_err() {
                    eof_clone.store(true, Ordering::SeqCst);
                    break;
                }
                let color_img = egui::ColorImage::from_rgba_unmultiplied([sw, sh], &buf);
                if tx.send(color_img).is_err() {
                    break;
                }
            }
            if let Ok(mut guard) = child_clone.lock() {
                if let Some(mut c) = guard.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        });

        self.stream = Some(ActiveStream {
            rx,
            stop_flag,
            child: child_arc,
            reader_thread: Some(reader_thread),
            eof_reached,
        });
        Ok(())
    }
}

fn compute_stream_dims(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let w = w.max(2);
    let h = h.max(2);
    let scale = (max_w as f64 / w as f64)
        .min(max_h as f64 / h as f64)
        .min(1.0);
    let mut sw = ((w as f64 * scale).round() as u32).max(2);
    let mut sh = ((h as f64 * scale).round() as u32).max(2);
    if sw % 2 == 1 {
        sw += 1;
    }
    if sh % 2 == 1 {
        sh += 1;
    }
    (sw, sh)
}

pub fn format_duration(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
