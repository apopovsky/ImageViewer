use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use image::{AnimationDecoder, DynamicImage, GenericImageView, ImageFormat, ImageReader};

use super::{read_exif, ExifSummary, MediaKind};

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone)]
pub struct DecodedFrame {
    pub image: Arc<DynamicImage>,
    pub color_image: Arc<egui::ColorImage>,
    pub rgba_size: [usize; 2],
    pub delay_ms: u32,
}

#[derive(Clone)]
pub struct DecodedMedia {
    pub frames: Vec<DecodedFrame>,
    pub exif: ExifSummary,
    pub original_dimensions: (u32, u32),
    pub video_duration_secs: Option<f64>,
    pub format_label: String,
    pub color_depth_label: String,
    pub aspect_ratio_label: String,
    pub megapixels: f64,
}

pub fn format_color_depth(color: image::ColorType) -> String {
    match color {
        image::ColorType::L8 => "8-bit Grayscale".to_string(),
        image::ColorType::La8 => "16-bit Grayscale + Alpha".to_string(),
        image::ColorType::Rgb8 => "24-bit RGB".to_string(),
        image::ColorType::Rgba8 => "32-bit RGBA".to_string(),
        image::ColorType::L16 => "16-bit Grayscale".to_string(),
        image::ColorType::La16 => "32-bit Grayscale + Alpha".to_string(),
        image::ColorType::Rgb16 => "48-bit RGB16".to_string(),
        image::ColorType::Rgba16 => "64-bit RGBA16".to_string(),
        image::ColorType::Rgb32F => "96-bit RGB32F".to_string(),
        image::ColorType::Rgba32F => "128-bit RGBA32F".to_string(),
        other => format!("{other:?}"),
    }
}

pub fn format_aspect_ratio(w: u32, h: u32) -> String {
    if w == 0 || h == 0 {
        return "—".to_string();
    }
    let mut a = w;
    let mut b = h;
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    let rw = w / a.max(1);
    let rh = h / a.max(1);
    if rw <= 32 && rh <= 32 {
        format!("{rw}:{rh}")
    } else {
        let ratio = w as f64 / h as f64;
        format!("{ratio:.2}:1")
    }
}

fn build_decoded_media(
    frames: Vec<DecodedFrame>,
    exif: ExifSummary,
    dims: (u32, u32),
    video_duration_secs: Option<f64>,
    format_label: String,
    color_depth_label: String,
) -> DecodedMedia {
    let megapixels = (dims.0 as f64 * dims.1 as f64) / 1_000_000.0;
    let aspect_ratio_label = format_aspect_ratio(dims.0, dims.1);
    DecodedMedia {
        frames,
        exif,
        original_dimensions: dims,
        video_duration_secs,
        format_label,
        color_depth_label,
        aspect_ratio_label,
        megapixels,
    }
}

pub fn dynamic_to_color_image(img: &DynamicImage) -> egui::ColorImage {
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

pub fn apply_exif_orientation(img: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

fn make_frame(img: DynamicImage, delay_ms: u32) -> DecodedFrame {
    let (w, h) = img.dimensions();
    let color = dynamic_to_color_image(&img);
    DecodedFrame {
        image: Arc::new(img),
        color_image: Arc::new(color),
        rgba_size: [w as usize, h as usize],
        delay_ms,
    }
}

pub fn new_hidden_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn decode_media(path: &Path) -> Result<DecodedMedia, String> {
    let kind = MediaKind::from_path(path).unwrap_or(MediaKind::StaticImage);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_uppercase();

    match kind {
        MediaKind::Video => decode_video_preview(path, &ext),
        MediaKind::VectorSvg => decode_svg_media(path, &ext),
        MediaKind::AnimatedImage => decode_animated_gif(path, &ext),
        MediaKind::ExtendedImage => decode_extended_or_fallback(path, &ext),
        MediaKind::StaticImage => {
            if ext == "WEBP" {
                if let Ok(anim) = decode_webp_maybe_animated(path) {
                    return Ok(anim);
                }
            }
            decode_static_native(path, &ext)
                .or_else(|_| decode_extended_or_fallback(path, &ext))
        }
    }
}

fn decode_static_native(path: &Path, ext: &str) -> Result<DecodedMedia, String> {
    let exif = read_exif(path);
    let reader = ImageReader::open(path)
        .map_err(|e| format!("Could not open file: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("Unrecognized format: {e}"))?;
    let detected = reader
        .format()
        .map(|f| format!("{f:?}").to_ascii_uppercase())
        .unwrap_or_else(|| ext.to_string());

    let raw_img = reader
        .decode()
        .map_err(|e| format!("Error decoding image: {e}"))?;
    let color_depth = format_color_depth(raw_img.color());
    let img = apply_exif_orientation(raw_img, exif.orientation);
    let dims = img.dimensions();

    Ok(build_decoded_media(
        vec![make_frame(img, 0)],
        exif,
        dims,
        None,
        detected,
        color_depth,
    ))
}

fn decode_animated_gif(path: &Path, ext: &str) -> Result<DecodedMedia, String> {
    let file = File::open(path).map_err(|e| format!("Error opening GIF: {e}"))?;
    let decoder = image::codecs::gif::GifDecoder::new(BufReader::new(file))
        .map_err(|e| format!("Error decoding GIF: {e}"))?;

    let mut frames = Vec::new();
    for frame_res in decoder.into_frames() {
        let frame = frame_res.map_err(|e| format!("Error in GIF frame: {e}"))?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay_ms = (numer / denom.max(1)).max(20);
        let rgba = frame.into_buffer();
        let dyn_img = DynamicImage::ImageRgba8(rgba);
        frames.push(make_frame(dyn_img, delay_ms));
    }

    if frames.is_empty() {
        return decode_static_native(path, ext);
    }

    let dims = frames[0].image.dimensions();
    let depth = format_color_depth(frames[0].image.color());
    Ok(build_decoded_media(
        frames,
        ExifSummary::default(),
        dims,
        None,
        "GIF".to_string(),
        depth,
    ))
}

fn decode_webp_maybe_animated(path: &Path) -> Result<DecodedMedia, String> {
    let file = File::open(path).map_err(|e| format!("Error opening WebP: {e}"))?;
    let decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(file))
        .map_err(|e| format!("Error decoding WebP: {e}"))?;

    if !decoder.has_animation() {
        return Err("Not animated".to_string());
    }

    let mut frames = Vec::new();
    for frame_res in decoder.into_frames() {
        let frame = frame_res.map_err(|e| format!("Error in WebP frame: {e}"))?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay_ms = (numer / denom.max(1)).max(20);
        let dyn_img = DynamicImage::ImageRgba8(frame.into_buffer());
        frames.push(make_frame(dyn_img, delay_ms));
    }

    if frames.is_empty() {
        return Err("Empty WebP".to_string());
    }
    let dims = frames[0].image.dimensions();
    let depth = format_color_depth(frames[0].image.color());
    Ok(build_decoded_media(
        frames,
        ExifSummary::default(),
        dims,
        None,
        "WEBP (Animated)".to_string(),
        depth,
    ))
}

fn decode_svg_media(path: &Path, _ext: &str) -> Result<DecodedMedia, String> {
    let img = render_svg_to_image(path, 1920, 4096)?;
    let dims = img.dimensions();
    Ok(build_decoded_media(
        vec![make_frame(img, 0)],
        ExifSummary::default(),
        dims,
        None,
        "SVG".to_string(),
        "32-bit RGBA (Vector)".to_string(),
    ))
}

fn render_svg_to_image(path: &Path, min_edge: u32, max_edge: u32) -> Result<DynamicImage, String> {
    let data = fs::read(path).map_err(|e| format!("Could not read SVG: {e}"))?;
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&data, &opt)
        .map_err(|e| format!("Invalid SVG: {e}"))?;

    let size = tree.size();
    let base_w = size.width().max(1.0);
    let base_h = size.height().max(1.0);
    let long_edge = base_w.max(base_h);

    let scale = if long_edge < min_edge as f32 {
        min_edge as f32 / long_edge
    } else if long_edge > max_edge as f32 {
        max_edge as f32 / long_edge
    } else {
        1.0
    };

    let target_w = ((base_w * scale).round() as u32).clamp(1, max_edge);
    let target_h = ((base_h * scale).round() as u32).clamp(1, max_edge);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(target_w, target_h)
        .ok_or_else(|| "Could not allocate buffer for SVG".to_string())?;
    let transform = resvg::usvg::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Convert premultiplied RGBA from tiny_skia into straight RGBA
    let mut raw = pixmap.take();
    for px in raw.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            let alpha = a as f32 / 255.0;
            px[0] = ((px[0] as f32 / alpha).min(255.0)) as u8;
            px[1] = ((px[1] as f32 / alpha).min(255.0)) as u8;
            px[2] = ((px[2] as f32 / alpha).min(255.0)) as u8;
        }
    }

    let rgba = image::RgbaImage::from_raw(target_w, target_h, raw)
        .ok_or_else(|| "Error building SVG image".to_string())?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

fn decode_extended_or_fallback(path: &Path, ext: &str) -> Result<DecodedMedia, String> {
    let exif = read_exif(path);

    // 1. Try ImageMagick (excellent for HEIC, AVIF, RAW CR2/CR3/NEF/ARW/DNG, PSD, JXL)
    let input_spec = format!("{}[0]", path.display());
    if let Ok(out) = new_hidden_command("magick")
        .arg(&input_spec)
        .arg("-auto-orient")
        .arg("png:-")
        .output()
    {
        if out.status.success() && !out.stdout.is_empty() {
            if let Ok(img) = image::load_from_memory_with_format(&out.stdout, ImageFormat::Png) {
                let dims = img.dimensions();
                let depth = format_color_depth(img.color());
                return Ok(build_decoded_media(
                    vec![make_frame(img, 0)],
                    exif,
                    dims,
                    None,
                    if ext.is_empty() {
                        "EXT".to_string()
                    } else {
                        ext.to_string()
                    },
                    depth,
                ));
            }
        }
    }

    // 2. Fallback to FFmpeg single-frame extraction
    if let Ok(out) = new_hidden_command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
        .output()
    {
        if out.status.success() && !out.stdout.is_empty() {
            if let Ok(img) = image::load_from_memory_with_format(&out.stdout, ImageFormat::Png) {
                let depth = format_color_depth(img.color());
                let img = apply_exif_orientation(img, exif.orientation);
                let dims = img.dimensions();
                return Ok(build_decoded_media(
                    vec![make_frame(img, 0)],
                    exif,
                    dims,
                    None,
                    ext.to_string(),
                    depth,
                ));
            }
        }
    }

    Err(format!(
        "Could not decode '{}'. Make sure the format is valid or that ImageMagick/FFmpeg are installed.",
        path.display()
    ))
}

#[derive(Debug, Clone)]
pub struct VideoProbeInfo {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    pub fps: f64,
    pub metadata_rows: Vec<(String, String)>,
}

pub fn probe_video(path: &Path) -> Result<VideoProbeInfo, String> {
    let out = new_hidden_command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,codec_name,codec_long_name,pix_fmt,bit_rate:format=duration,bit_rate,format_long_name",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("Could not run ffprobe: {e}"))?;

    if !out.status.success() {
        return Err("ffprobe failed to analyze the video".to_string());
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut width = 0u32;
    let mut height = 0u32;
    let mut duration_secs = 0.0f64;
    let mut fps = 30.0f64;
    let mut metadata_rows = Vec::new();

    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            if v.is_empty() || v == "N/A" {
                continue;
            }
            match k.trim() {
                "width" => width = v.parse().unwrap_or(0),
                "height" => height = v.parse().unwrap_or(0),
                "duration" => duration_secs = v.parse().unwrap_or(0.0),
                "r_frame_rate" => {
                    if let Some((num, den)) = v.split_once('/') {
                        let n: f64 = num.parse().unwrap_or(30.0);
                        let d: f64 = den.parse().unwrap_or(1.0);
                        if d > 0.0 && n > 0.0 {
                            fps = (n / d).clamp(1.0, 120.0);
                        }
                    } else if let Ok(f) = v.parse::<f64>() {
                        if f > 0.0 {
                            fps = f.clamp(1.0, 120.0);
                        }
                    }
                    metadata_rows.push(("Framerate".to_string(), format!("{fps:.2} fps")));
                }
                "codec_name" => metadata_rows.push(("Video Codec".to_string(), v.to_ascii_uppercase())),
                "codec_long_name" => metadata_rows.push(("Codec Description".to_string(), v.to_string())),
                "pix_fmt" => metadata_rows.push(("Pixel Format".to_string(), v.to_string())),
                "bit_rate" => {
                    if let Ok(bps) = v.parse::<u64>() {
                        metadata_rows.push(("Bitrate".to_string(), format!("{:.0} kbps", bps as f64 / 1000.0)));
                    }
                }
                "format_long_name" => metadata_rows.push(("Container".to_string(), v.to_string())),
                _ => {}
            }
        }
    }

    if width == 0 || height == 0 {
        return Err("File does not contain a valid video stream".to_string());
    }

    Ok(VideoProbeInfo {
        width,
        height,
        duration_secs,
        fps,
        metadata_rows,
    })
}

fn decode_video_preview(path: &Path, ext: &str) -> Result<DecodedMedia, String> {
    let info = probe_video(path)?;
    let out = new_hidden_command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "-"])
        .output()
        .map_err(|e| format!("Error running ffmpeg: {e}"))?;

    let img = if out.status.success() && !out.stdout.is_empty() {
        image::load_from_memory_with_format(&out.stdout, ImageFormat::Png)
            .map_err(|e| format!("Error reading video frame: {e}"))?
    } else {
        DynamicImage::new_rgba8(info.width.min(640), info.height.min(360))
    };

    let mut exif = ExifSummary::default();
    if !info.metadata_rows.is_empty() {
        exif.categories
            .push(("Video & Container".to_string(), info.metadata_rows));
    }

    Ok(build_decoded_media(
        vec![make_frame(img, 0)],
        exif,
        (info.width, info.height),
        Some(info.duration_secs),
        format!("VIDEO ({ext})"),
        "Video Stream".to_string(),
    ))
}

pub fn decode_thumbnail(path: &Path, max_edge: u32) -> Result<egui::ColorImage, String> {
    let kind = MediaKind::from_path(path).unwrap_or(MediaKind::StaticImage);

    let img = match kind {
        MediaKind::VectorSvg => render_svg_to_image(path, max_edge, max_edge)?,
        MediaKind::Video => {
            let vf = format!("scale={max_edge}:-1");
            let attempt = new_hidden_command("ffmpeg")
                .args(["-v", "error", "-ss", "00:00:00.500", "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-vf", &vf, "-f", "image2pipe", "-vcodec", "png", "-"])
                .output();
            let png_bytes = match attempt {
                Ok(out) if out.status.success() && !out.stdout.is_empty() => out.stdout,
                _ => {
                    let fallback = new_hidden_command("ffmpeg")
                        .args(["-v", "error", "-i"])
                        .arg(path)
                        .args(["-frames:v", "1", "-vf", &vf, "-f", "image2pipe", "-vcodec", "png", "-"])
                        .output()
                        .map_err(|e| format!("Video thumbnail error: {e}"))?;
                    if !fallback.status.success() || fallback.stdout.is_empty() {
                        return Err("No video thumbnail".to_string());
                    }
                    fallback.stdout
                }
            };
            image::load_from_memory_with_format(&png_bytes, ImageFormat::Png)
                .map_err(|e| format!("Video thumbnail PNG error: {e}"))?
        }
        MediaKind::ExtendedImage => {
            let input_spec = format!("{}[0]", path.display());
            let geom = format!("{max_edge}x{max_edge}");
            let out = new_hidden_command("magick")
                .arg(&input_spec)
                .args(["-auto-orient", "-thumbnail", &geom, "png:-"])
                .output()
                .map_err(|e| format!("Magick thumbnail error: {e}"))?;
            if !out.status.success() || out.stdout.is_empty() {
                return Err("Could not generate extended thumbnail".to_string());
            }
            image::load_from_memory_with_format(&out.stdout, ImageFormat::Png)
                .map_err(|e| format!("Error decoding extended thumbnail: {e}"))?
        }
        MediaKind::StaticImage | MediaKind::AnimatedImage => {
            let exif = read_exif(path);
            let native = ImageReader::open(path)
                .ok()
                .and_then(|r| r.with_guessed_format().ok())
                .and_then(|r| r.decode().ok());
            if let Some(decoded) = native {
                let oriented = apply_exif_orientation(decoded, exif.orientation);
                oriented.thumbnail(max_edge, max_edge)
            } else {
                let input_spec = format!("{}[0]", path.display());
                let geom = format!("{max_edge}x{max_edge}");
                let out = new_hidden_command("magick")
                    .arg(&input_spec)
                    .args(["-auto-orient", "-thumbnail", &geom, "png:-"])
                    .output()
                    .map_err(|e| format!("Fallback thumbnail error: {e}"))?;
                image::load_from_memory_with_format(&out.stdout, ImageFormat::Png)
                    .map_err(|e| format!("Fallback thumbnail PNG error: {e}"))?
            }
        }
    };

    Ok(dynamic_to_color_image(&img))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn decodes_png_and_svg_and_generates_thumbnails() {
        let tmp_dir = std::env::temp_dir().join(format!("imgv_decoder_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let png_path = tmp_dir.join("test.png");
        let img = RgbaImage::from_pixel(100, 50, Rgba([20, 120, 220, 255]));
        img.save(&png_path).unwrap();

        let decoded_png = decode_media(&png_path).unwrap();
        assert_eq!(decoded_png.original_dimensions, (100, 50));
        assert_eq!(decoded_png.frames.len(), 1);
        assert_eq!(decoded_png.aspect_ratio_label, "2:1");
        assert!((decoded_png.megapixels - 0.005).abs() < 1e-6);
        assert_eq!(format_aspect_ratio(1920, 1080), "16:9");
        assert_eq!(format_aspect_ratio(2252, 4000), "0.56:1");

        let svg_path = tmp_dir.join("shape.svg");
        fs::write(
            &svg_path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><rect width="120" height="60" fill="#22aa66"/></svg>"##,
        )
        .unwrap();
        let decoded_svg = decode_media(&svg_path).unwrap();
        assert_eq!(decoded_svg.format_label, "SVG");
        assert!(decoded_svg.original_dimensions.0 >= 1920);

        let thumb = decode_thumbnail(&svg_path, 128).unwrap();
        assert_eq!(thumb.size[0], 128);
        assert_eq!(thumb.size[1], 64);

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
