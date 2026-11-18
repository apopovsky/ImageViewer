use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crossbeam_channel::{unbounded, Receiver, Sender};
use rayon::prelude::*;

use crate::media::decoder::{decode_media, decode_thumbnail, DecodedMedia};
use crate::media::MediaItem;

const MAX_FULL_ENTRIES: usize = 14;
const MAX_FULL_BYTES: usize = 768 * 1024 * 1024;
const THUMB_MAX_EDGE: u32 = 256;

#[derive(Clone)]
pub struct CachedFullEntry {
    pub media: DecodedMedia,
    pub textures: Vec<egui::TextureHandle>,
    pub last_used: u64,
    pub approx_bytes: usize,
}

impl CachedFullEntry {
    pub fn ensure_texture(&mut self, ctx: &egui::Context, frame_idx: usize, key: &Path) -> Option<&egui::TextureHandle> {
        if frame_idx >= self.media.frames.len() {
            return None;
        }
        while self.textures.len() <= frame_idx {
            let idx = self.textures.len();
            let frame = &self.media.frames[idx];
            let tex = ctx.load_texture(
                format!("full_{}_{}", key.display(), idx),
                (*frame.color_image).clone(),
                egui::TextureOptions::LINEAR,
            );
            self.textures.push(tex);
        }
        self.textures.get(frame_idx)
    }
}

struct FullQueueState {
    pending: VecDeque<PathBuf>,
    in_flight: HashSet<PathBuf>,
    shutdown: bool,
}

struct ThumbQueueState {
    pending: VecDeque<PathBuf>,
    in_flight: HashSet<PathBuf>,
    shutdown: bool,
}

pub enum ThumbStatus {
    Ready(egui::TextureHandle),
    Failed,
    Loading,
}

pub struct MediaCache {
    full_entries: HashMap<PathBuf, CachedFullEntry>,
    full_errors: HashMap<PathBuf, String>,
    lru_counter: u64,

    thumb_entries: HashMap<PathBuf, egui::TextureHandle>,
    thumb_failed: HashSet<PathBuf>,

    full_queue: Arc<(Mutex<FullQueueState>, Condvar)>,
    full_rx: Receiver<(PathBuf, Result<DecodedMedia, String>)>,

    thumb_queue: Arc<(Mutex<ThumbQueueState>, Condvar)>,
    thumb_rx: Receiver<(PathBuf, Result<egui::ColorImage, String>)>,

    egui_ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl MediaCache {
    pub fn new() -> Self {
        let egui_ctx: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));

        let full_queue = Arc::new((
            Mutex::new(FullQueueState {
                pending: VecDeque::new(),
                in_flight: HashSet::new(),
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let (full_tx, full_rx) = unbounded::<(PathBuf, Result<DecodedMedia, String>)>();

        // Spawn 2 high-priority full media decoder threads (current + next preload in parallel)
        for worker_id in 0..2 {
            let q = Arc::clone(&full_queue);
            let tx = full_tx.clone();
            let ctx_ref = Arc::clone(&egui_ctx);
            thread::Builder::new()
                .name(format!("full-decoder-{worker_id}"))
                .spawn(move || Self::full_worker_loop(q, tx, ctx_ref))
                .ok();
        }

        let thumb_queue = Arc::new((
            Mutex::new(ThumbQueueState {
                pending: VecDeque::new(),
                in_flight: HashSet::new(),
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let (thumb_tx, thumb_rx) = unbounded::<(PathBuf, Result<egui::ColorImage, String>)>();

        {
            let q = Arc::clone(&thumb_queue);
            let tx = thumb_tx;
            let ctx_ref = Arc::clone(&egui_ctx);
            thread::Builder::new()
                .name("thumb-dispatcher".to_string())
                .spawn(move || Self::thumb_dispatcher_loop(q, tx, ctx_ref))
                .ok();
        }

        Self {
            full_entries: HashMap::new(),
            full_errors: HashMap::new(),
            lru_counter: 0,
            thumb_entries: HashMap::new(),
            thumb_failed: HashSet::new(),
            full_queue,
            full_rx,
            thumb_queue,
            thumb_rx,
            egui_ctx,
        }
    }

    pub fn bind_context(&self, ctx: &egui::Context) {
        if let Ok(mut guard) = self.egui_ctx.lock() {
            if guard.is_none() {
                *guard = Some(ctx.clone());
            }
        }
    }

    fn notify_repaint(ctx_ref: &Arc<Mutex<Option<egui::Context>>>) {
        if let Ok(guard) = ctx_ref.lock() {
            if let Some(ctx) = guard.as_ref() {
                ctx.request_repaint();
            }
        }
    }

    fn full_worker_loop(
        queue: Arc<(Mutex<FullQueueState>, Condvar)>,
        tx: Sender<(PathBuf, Result<DecodedMedia, String>)>,
        ctx_ref: Arc<Mutex<Option<egui::Context>>>,
    ) {
        let (lock, cvar) = &*queue;
        loop {
            let path = {
                let mut state = lock.lock().unwrap();
                while state.pending.is_empty() && !state.shutdown {
                    state = cvar.wait(state).unwrap();
                }
                if state.shutdown {
                    return;
                }
                let Some(p) = state.pending.pop_front() else {
                    continue;
                };
                state.in_flight.insert(p.clone());
                p
            };

            let result = decode_media(&path);

            {
                let mut state = lock.lock().unwrap();
                state.in_flight.remove(&path);
            }

            let _ = tx.send((path, result));
            Self::notify_repaint(&ctx_ref);
        }
    }

    fn thumb_dispatcher_loop(
        queue: Arc<(Mutex<ThumbQueueState>, Condvar)>,
        tx: Sender<(PathBuf, Result<egui::ColorImage, String>)>,
        ctx_ref: Arc<Mutex<Option<egui::Context>>>,
    ) {
        let (lock, cvar) = &*queue;
        loop {
            let batch: Vec<PathBuf> = {
                let mut state = lock.lock().unwrap();
                while state.pending.is_empty() && !state.shutdown {
                    state = cvar.wait(state).unwrap();
                }
                if state.shutdown {
                    return;
                }
                let mut items = Vec::with_capacity(12);
                while items.len() < 12 {
                    if let Some(p) = state.pending.pop_front() {
                        state.in_flight.insert(p.clone());
                        items.push(p);
                    } else {
                        break;
                    }
                }
                items
            };

            if batch.is_empty() {
                continue;
            }

            batch.into_par_iter().for_each(|path| {
                let res = decode_thumbnail(&path, THUMB_MAX_EDGE);
                {
                    let mut state = lock.lock().unwrap();
                    state.in_flight.remove(&path);
                }
                let _ = tx.send((path, res));
                Self::notify_repaint(&ctx_ref);
            });
        }
    }

    pub fn poll_results(&mut self, ctx: &egui::Context, pinned_path: Option<&Path>) {
        self.bind_context(ctx);

        while let Ok((path, result)) = self.full_rx.try_recv() {
            match result {
                Ok(media) => {
                    self.full_errors.remove(&path);
                    let approx_bytes: usize = media
                        .frames
                        .iter()
                        .map(|f| f.rgba_size[0] * f.rgba_size[1] * 4)
                        .sum();
                    self.lru_counter += 1;
                    let mut entry = CachedFullEntry {
                        media,
                        textures: Vec::new(),
                        last_used: self.lru_counter,
                        approx_bytes,
                    };
                    // Pre-upload frame 0 if it's the pinned item for zero-latency display
                    if pinned_path == Some(path.as_path()) {
                        entry.ensure_texture(ctx, 0, &path);
                    }
                    self.full_entries.insert(path, entry);
                    self.evict_if_needed(pinned_path);
                }
                Err(err) => {
                    self.full_errors.insert(path, err);
                }
            }
        }

        // Upload up to 16 thumbnails per frame so gallery scrolling remains 60 FPS
        let mut uploaded = 0;
        while uploaded < 16 {
            match self.thumb_rx.try_recv() {
                Ok((path, Ok(color_img))) => {
                    let tex = ctx.load_texture(
                        format!("thumb_{}", path.display()),
                        color_img,
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumb_entries.insert(path, tex);
                    uploaded += 1;
                }
                Ok((path, Err(_))) => {
                    self.thumb_failed.insert(path);
                }
                Err(_) => break,
            }
        }
        if uploaded >= 16 {
            ctx.request_repaint();
        }
    }

    pub fn request_with_preload(&mut self, items: &[MediaItem], current_idx: usize, direction: isize) {
        if items.is_empty() || current_idx >= items.len() {
            return;
        }

        let offsets: [isize; 6] = if direction < 0 {
            [0, -1, 1, -2, 2, -3]
        } else {
            [0, 1, -1, 2, -2, 3]
        };

        let mut desired = Vec::new();
        for off in offsets {
            let target = current_idx as isize + off;
            if target >= 0 && (target as usize) < items.len() {
                let item = &items[target as usize];
                // Preload images (videos only need preview/player on active item to avoid spawning extra ffmpeg processes)
                if off == 0 || !item.kind.is_video() {
                    let p = &item.path;
                    if !self.full_entries.contains_key(p) && !self.full_errors.contains_key(p) {
                        desired.push(p.clone());
                    }
                }
            }
        }

        if desired.is_empty() {
            return;
        }

        let (lock, cvar) = &*self.full_queue;
        let mut state = lock.lock().unwrap();
        state.pending.clear();
        for p in desired {
            if !state.in_flight.contains(&p) {
                state.pending.push_back(p);
            }
        }
        cvar.notify_all();
    }

    pub fn get_full_mut(&mut self, path: &Path) -> Option<&mut CachedFullEntry> {
        self.lru_counter += 1;
        let seq = self.lru_counter;
        let entry = self.full_entries.get_mut(path)?;
        entry.last_used = seq;
        Some(entry)
    }

    pub fn get_full_error(&self, path: &Path) -> Option<&str> {
        self.full_errors.get(path).map(|s| s.as_str())
    }

    pub fn request_thumbnails(&self, paths: &[PathBuf], high_priority: bool) {
        let (lock, cvar) = &*self.thumb_queue;
        let mut state = lock.lock().unwrap();
        let existing_pending: HashSet<PathBuf> = state.pending.iter().cloned().collect();
        let mut added = false;

        if high_priority {
            for p in paths.iter().rev() {
                if !self.thumb_entries.contains_key(p)
                    && !self.thumb_failed.contains(p)
                    && !state.in_flight.contains(p)
                    && !existing_pending.contains(p)
                {
                    state.pending.push_front(p.clone());
                    added = true;
                }
            }
        } else {
            for p in paths {
                if !self.thumb_entries.contains_key(p)
                    && !self.thumb_failed.contains(p)
                    && !state.in_flight.contains(p)
                    && !existing_pending.contains(p)
                {
                    state.pending.push_back(p.clone());
                    added = true;
                }
            }
        }

        if added {
            cvar.notify_all();
        }
    }

    pub fn thumbnail_status(&self, path: &Path) -> ThumbStatus {
        if let Some(tex) = self.thumb_entries.get(path) {
            ThumbStatus::Ready(tex.clone())
        } else if self.thumb_failed.contains(path) {
            ThumbStatus::Failed
        } else {
            ThumbStatus::Loading
        }
    }

    pub fn invalidate(&mut self, path: &Path) {
        self.full_entries.remove(path);
        self.full_errors.remove(path);
        self.thumb_entries.remove(path);
        self.thumb_failed.remove(path);
    }

    pub fn clear_all(&mut self) {
        self.full_entries.clear();
        self.full_errors.clear();
        self.thumb_entries.clear();
        self.thumb_failed.clear();
        if let Ok(mut state) = self.full_queue.0.lock() {
            state.pending.clear();
        }
        if let Ok(mut state) = self.thumb_queue.0.lock() {
            state.pending.clear();
        }
    }

    fn evict_if_needed(&mut self, pinned_path: Option<&Path>) {
        loop {
            let total_bytes: usize = self.full_entries.values().map(|e| e.approx_bytes).sum();
            let over_count = self.full_entries.len() > MAX_FULL_ENTRIES;
            let over_bytes = total_bytes > MAX_FULL_BYTES && self.full_entries.len() > 3;

            if !over_count && !over_bytes {
                break;
            }

            let victim = self
                .full_entries
                .iter()
                .filter(|(k, _)| pinned_path != Some(k.as_path()))
                .min_by_key(|(_, v)| v.last_used)
                .map(|(k, _)| k.clone());

            match victim {
                Some(key) => {
                    self.full_entries.remove(&key);
                }
                None => break,
            }
        }
    }
}

impl Drop for MediaCache {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.full_queue;
            if let Ok(mut s) = lock.lock() {
                s.shutdown = true;
            }
            cvar.notify_all();
        }
        {
            let (lock, cvar) = &*self.thumb_queue;
            if let Ok(mut s) = lock.lock() {
                s.shutdown = true;
            }
            cvar.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::decoder::dynamic_to_color_image;
    use crate::media::ExifSummary;

    #[test]
    fn lru_eviction_respects_pinned_and_max_entries() {
        let mut cache = MediaCache::new();
        let img = image::DynamicImage::new_rgba8(10, 10);
        let color = Arc::new(dynamic_to_color_image(&img));
        let dyn_arc = Arc::new(img);

        let pinned = PathBuf::from("pinned.png");
        for i in 0..20 {
            let p = if i == 0 {
                pinned.clone()
            } else {
                PathBuf::from(format!("img_{i}.png"))
            };
            cache.lru_counter += 1;
            cache.full_entries.insert(
                p,
                CachedFullEntry {
                    media: DecodedMedia {
                        frames: vec![crate::media::decoder::DecodedFrame {
                            image: Arc::clone(&dyn_arc),
                            color_image: Arc::clone(&color),
                            rgba_size: [10, 10],
                            delay_ms: 0,
                        }],
                        exif: ExifSummary::default(),
                        original_dimensions: (10, 10),
                        video_duration_secs: None,
                        format_label: "PNG".to_string(),
                        color_depth_label: "32-bit RGBA".to_string(),
                        aspect_ratio_label: "1:1".to_string(),
                        megapixels: 0.0001,
                    },
                    textures: Vec::new(),
                    last_used: cache.lru_counter,
                    approx_bytes: 400,
                },
            );
            cache.evict_if_needed(Some(&pinned));
        }

        assert!(cache.full_entries.len() <= MAX_FULL_ENTRIES);
        assert!(cache.full_entries.contains_key(&pinned));
        assert!(cache.full_entries.contains_key(Path::new("img_19.png")));
    }
}
