use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};

use crate::i18n::Language;
use crate::media::decoder::{dynamic_to_color_image, new_hidden_command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropAspect {
    Free,
    Original,
    Square1x1,
    Ratio16x9,
    Ratio4x3,
    Ratio3x2,
    Ratio9x16,
}

impl CropAspect {
    pub const ALL: [CropAspect; 7] = [
        Self::Free,
        Self::Original,
        Self::Square1x1,
        Self::Ratio16x9,
        Self::Ratio4x3,
        Self::Ratio3x2,
        Self::Ratio9x16,
    ];

    pub fn label(self, lang: Language) -> &'static str {
        let t = lang.tr();
        match self {
            Self::Free => t.aspect_free,
            Self::Original => t.aspect_original,
            Self::Square1x1 => "1:1",
            Self::Ratio16x9 => "16:9",
            Self::Ratio4x3 => "4:3",
            Self::Ratio3x2 => "3:2",
            Self::Ratio9x16 => "9:16",
        }
    }

    pub fn ratio(self, orig_w: u32, orig_h: u32) -> Option<f32> {
        match self {
            Self::Free => None,
            Self::Original => Some((orig_w.max(1) as f32) / (orig_h.max(1) as f32)),
            Self::Square1x1 => Some(1.0),
            Self::Ratio16x9 => Some(16.0 / 9.0),
            Self::Ratio4x3 => Some(4.0 / 3.0),
            Self::Ratio3x2 => Some(3.0 / 2.0),
            Self::Ratio9x16 => Some(9.0 / 16.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropDragMode {
    Move,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct CropOverlayState {
    pub active: bool,
    /// Normalized [x0, y0, x1, y1] within 0.0..=1.0
    pub rect_norm: [f32; 4],
    pub aspect: CropAspect,
    pub drag_mode: Option<CropDragMode>,
    pub drag_start_norm: [f32; 4],
    pub drag_start_pointer: [f32; 2],
}

impl Default for CropOverlayState {
    fn default() -> Self {
        Self {
            active: false,
            rect_norm: [0.1, 0.1, 0.9, 0.9],
            aspect: CropAspect::Free,
            drag_mode: None,
            drag_start_norm: [0.1, 0.1, 0.9, 0.9],
            drag_start_pointer: [0.0, 0.0],
        }
    }
}

impl CropOverlayState {
    pub fn reset(&mut self) {
        self.rect_norm = [0.08, 0.08, 0.92, 0.92];
        self.aspect = CropAspect::Free;
        self.drag_mode = None;
    }

    pub fn enforce_aspect(&mut self, img_w: u32, img_h: u32) {
        let Some(target_ratio) = self.aspect.ratio(img_w, img_h) else {
            return;
        };
        let iw = img_w.max(1) as f32;
        let ih = img_h.max(1) as f32;

        let cx = (self.rect_norm[0] + self.rect_norm[2]) * 0.5;
        let cy = (self.rect_norm[1] + self.rect_norm[3]) * 0.5;
        let cur_w_px = (self.rect_norm[2] - self.rect_norm[0]).max(0.02) * iw;
        let cur_h_px = (self.rect_norm[3] - self.rect_norm[1]).max(0.02) * ih;

        let mut new_w_px = cur_w_px;
        let mut new_h_px = new_w_px / target_ratio;
        if new_h_px > ih * 0.95 {
            new_h_px = ih * 0.95;
            new_w_px = new_h_px * target_ratio;
        }
        if new_w_px > iw * 0.95 {
            new_w_px = iw * 0.95;
            new_h_px = new_w_px / target_ratio;
        }
        if cur_h_px > cur_w_px && new_h_px < 10.0 {
            new_h_px = cur_h_px;
            new_w_px = new_h_px * target_ratio;
        }

        let half_nw = (new_w_px / iw) * 0.5;
        let half_nh = (new_h_px / ih) * 0.5;
        let clamped_cx = cx.clamp(half_nw, 1.0 - half_nw);
        let clamped_cy = cy.clamp(half_nh, 1.0 - half_nh);
        self.rect_norm = [
            (clamped_cx - half_nw).clamp(0.0, 1.0),
            (clamped_cy - half_nh).clamp(0.0, 1.0),
            (clamped_cx + half_nw).clamp(0.0, 1.0),
            (clamped_cy + half_nh).clamp(0.0, 1.0),
        ];
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeFilter {
    Lanczos3,
    CatmullRom,
    Nearest,
}

impl ResizeFilter {
    pub const ALL: [ResizeFilter; 3] = [Self::Lanczos3, Self::CatmullRom, Self::Nearest];

    pub fn label(self, lang: Language) -> &'static str {
        let t = lang.tr();
        match self {
            Self::Lanczos3 => t.filter_lanczos,
            Self::CatmullRom => t.filter_bicubic,
            Self::Nearest => t.filter_nearest,
        }
    }

    pub fn to_image_filter(self) -> FilterType {
        match self {
            Self::Lanczos3 => FilterType::Lanczos3,
            Self::CatmullRom => FilterType::CatmullRom,
            Self::Nearest => FilterType::Nearest,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResizeDialogState {
    pub open: bool,
    pub width_text: String,
    pub height_text: String,
    pub lock_aspect: bool,
    pub orig_w: u32,
    pub orig_h: u32,
    pub filter: ResizeFilter,
}

impl Default for ResizeDialogState {
    fn default() -> Self {
        Self {
            open: false,
            width_text: String::new(),
            height_text: String::new(),
            lock_aspect: true,
            orig_w: 1,
            orig_h: 1,
            filter: ResizeFilter::Lanczos3,
        }
    }
}

impl ResizeDialogState {
    pub fn open_with_dims(&mut self, w: u32, h: u32) {
        self.open = true;
        self.orig_w = w.max(1);
        self.orig_h = h.max(1);
        self.width_text = self.orig_w.to_string();
        self.height_text = self.orig_h.to_string();
    }

    pub fn set_percent(&mut self, pct: f32) {
        let w = ((self.orig_w as f32 * pct / 100.0).round() as u32).max(1);
        let h = ((self.orig_h as f32 * pct / 100.0).round() as u32).max(1);
        self.width_text = w.to_string();
        self.height_text = h.to_string();
    }

    pub fn sync_height_from_width(&mut self) {
        if !self.lock_aspect {
            return;
        }
        if let Ok(w) = self.width_text.trim().parse::<u32>() {
            if w > 0 {
                let h = ((w as f64 * self.orig_h as f64 / self.orig_w as f64).round() as u32).max(1);
                self.height_text = h.to_string();
            }
        }
    }

    pub fn sync_width_from_height(&mut self) {
        if !self.lock_aspect {
            return;
        }
        if let Ok(h) = self.height_text.trim().parse::<u32>() {
            if h > 0 {
                let w = ((h as f64 * self.orig_w as f64 / self.orig_h as f64).round() as u32).max(1);
                self.width_text = w.to_string();
            }
        }
    }

    pub fn parsed_dims(&self) -> Option<(u32, u32)> {
        let w = self.width_text.trim().parse::<u32>().ok()?;
        let h = self.height_text.trim().parse::<u32>().ok()?;
        if w == 0 || h == 0 || w > 32768 || h > 32768 {
            return None;
        }
        Some((w, h))
    }
}

pub struct ImageEditorState {
    pub working_image: Option<Arc<DynamicImage>>,
    pub working_texture: Option<egui::TextureHandle>,
    pub undo_stack: Vec<Arc<DynamicImage>>,
    pub is_dirty: bool,
    pub crop: CropOverlayState,
    pub resize: ResizeDialogState,
}

impl Default for ImageEditorState {
    fn default() -> Self {
        Self {
            working_image: None,
            working_texture: None,
            undo_stack: Vec::new(),
            is_dirty: false,
            crop: CropOverlayState::default(),
            resize: ResizeDialogState::default(),
        }
    }
}

impl ImageEditorState {
    pub fn reset(&mut self) {
        self.working_image = None;
        self.working_texture = None;
        self.undo_stack.clear();
        self.is_dirty = false;
        self.crop.active = false;
        self.crop.reset();
        self.resize.open = false;
    }

    pub fn active_image<'a>(&'a self, base: &'a Arc<DynamicImage>) -> &'a Arc<DynamicImage> {
        self.working_image.as_ref().unwrap_or(base)
    }

    fn commit_edit(&mut self, ctx: Option<&egui::Context>, base: &Arc<DynamicImage>, new_img: DynamicImage) {
        let prev = self.working_image.take().unwrap_or_else(|| Arc::clone(base));
        self.undo_stack.push(prev);
        if self.undo_stack.len() > 20 {
            self.undo_stack.remove(0);
        }
        let arc = Arc::new(new_img);
        if let Some(c) = ctx {
            let color = dynamic_to_color_image(&arc);
            self.working_texture = Some(c.load_texture(
                "editor_working_tex",
                color,
                egui::TextureOptions::LINEAR,
            ));
        } else {
            self.working_texture = None;
        }
        self.working_image = Some(arc);
        self.is_dirty = true;
    }

    pub fn rotate_90_cw(&mut self, ctx: Option<&egui::Context>, base: &Arc<DynamicImage>) {
        let src = self.active_image(base);
        let next = src.rotate90();
        self.commit_edit(ctx, base, next);
    }

    pub fn rotate_90_ccw(&mut self, ctx: Option<&egui::Context>, base: &Arc<DynamicImage>) {
        let src = self.active_image(base);
        let next = src.rotate270();
        self.commit_edit(ctx, base, next);
    }

    pub fn flip_horizontal(&mut self, ctx: Option<&egui::Context>, base: &Arc<DynamicImage>) {
        let src = self.active_image(base);
        let next = src.fliph();
        self.commit_edit(ctx, base, next);
    }

    pub fn flip_vertical(&mut self, ctx: Option<&egui::Context>, base: &Arc<DynamicImage>) {
        let src = self.active_image(base);
        let next = src.flipv();
        self.commit_edit(ctx, base, next);
    }

    pub fn apply_crop(&mut self, ctx: Option<&egui::Context>, base: &Arc<DynamicImage>) -> bool {
        let src = self.active_image(base);
        let (iw, ih) = src.dimensions();
        let (x, y, w, h) = crop_rect_pixels(iw, ih, self.crop.rect_norm);
        if w == 0 || h == 0 {
            return false;
        }
        let next = src.crop_imm(x, y, w, h);
        self.commit_edit(ctx, base, next);
        self.crop.active = false;
        self.crop.reset();
        true
    }

    pub fn apply_resize(
        &mut self,
        ctx: Option<&egui::Context>,
        base: &Arc<DynamicImage>,
        new_w: u32,
        new_h: u32,
        filter: ResizeFilter,
    ) -> bool {
        if new_w == 0 || new_h == 0 {
            return false;
        }
        let src = self.active_image(base);
        let next = src.resize_exact(new_w, new_h, filter.to_image_filter());
        self.commit_edit(ctx, base, next);
        self.resize.open = false;
        true
    }

    pub fn undo(&mut self, ctx: Option<&egui::Context>) {
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        if self.undo_stack.is_empty() {
            // Back to original base image
            self.working_image = None;
            self.working_texture = None;
            self.is_dirty = false;
        } else {
            if let Some(c) = ctx {
                let color = dynamic_to_color_image(&prev);
                self.working_texture = Some(c.load_texture(
                    "editor_working_tex",
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            } else {
                self.working_texture = None;
            }
            self.working_image = Some(prev);
            self.is_dirty = true;
        }
    }
}

pub fn crop_rect_pixels(img_w: u32, img_h: u32, rect_norm: [f32; 4]) -> (u32, u32, u32, u32) {
    let x0 = (rect_norm[0].clamp(0.0, 1.0) * img_w as f32).round() as u32;
    let y0 = (rect_norm[1].clamp(0.0, 1.0) * img_h as f32).round() as u32;
    let x1 = (rect_norm[2].clamp(0.0, 1.0) * img_w as f32).round() as u32;
    let y1 = (rect_norm[3].clamp(0.0, 1.0) * img_h as f32).round() as u32;

    let x = x0.min(img_w.saturating_sub(1));
    let y = y0.min(img_h.saturating_sub(1));
    let w = x1.saturating_sub(x).max(1).min(img_w.saturating_sub(x));
    let h = y1.saturating_sub(y).max(1).min(img_h.saturating_sub(y));
    (x, y, w, h)
}

pub fn save_image_to_path(img: &DynamicImage, path: &Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" | "jfif" => {
            let file = File::create(path).map_err(|e| format!("Could not create file: {e}"))?;
            let mut writer = BufWriter::new(file);
            let rgb = img.to_rgb8();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 92);
            encoder
                .encode_image(&rgb)
                .map_err(|e| format!("Error saving JPEG: {e}"))?;
            Ok(())
        }
        "png" => img
            .save_with_format(path, ImageFormat::Png)
            .map_err(|e| format!("Error saving PNG: {e}")),
        "webp" => img
            .save_with_format(path, ImageFormat::WebP)
            .map_err(|e| format!("Error saving WebP: {e}")),
        "bmp" => img
            .save_with_format(path, ImageFormat::Bmp)
            .map_err(|e| format!("Error saving BMP: {e}")),
        "tif" | "tiff" => img
            .save_with_format(path, ImageFormat::Tiff)
            .map_err(|e| format!("Error saving TIFF: {e}")),
        "tga" => img
            .save_with_format(path, ImageFormat::Tga)
            .map_err(|e| format!("Error saving TGA: {e}")),
        "qoi" => img
            .save_with_format(path, ImageFormat::Qoi)
            .map_err(|e| format!("Error saving QOI: {e}")),
        "ico" => img
            .save_with_format(path, ImageFormat::Ico)
            .map_err(|e| format!("Error saving ICO: {e}")),
        _ => {
            // Fallback: encode PNG to temp file and let ImageMagick convert to requested extension (e.g. AVIF/HEIC)
            let tmp_png = std::env::temp_dir().join(format!(
                "imgv_conv_{}_{}.png",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            img.save_with_format(&tmp_png, ImageFormat::Png)
                .map_err(|e| format!("Error preparing temporary image: {e}"))?;
            let out = new_hidden_command("magick")
                .arg(&tmp_png)
                .arg(path)
                .output();
            let _ = fs::remove_file(&tmp_png);
            match out {
                Ok(res) if res.status.success() => Ok(()),
                Ok(res) => Err(format!(
                    "Could not save in .{ext} format: {}",
                    String::from_utf8_lossy(&res.stderr)
                )),
                Err(e) => Err(format!("Format .{ext} not supported for writing: {e}")),
            }
        }
    }
}

pub fn save_overwrite(img: &DynamicImage, original_path: &Path) -> Result<(), String> {
    let parent = original_path
        .parent()
        .ok_or_else(|| "Invalid file path".to_string())?;
    let ext = original_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let stem = original_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");

    let tmp_path = parent.join(format!(".{stem}_tmp_{}.{ext}", std::process::id()));
    save_image_to_path(img, &tmp_path)?;

    if let Err(err) = fs::rename(&tmp_path, original_path) {
        // Fallback replace on Windows if rename fails
        if let Err(copy_err) = fs::copy(&tmp_path, original_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!(
                "Error replacing original file ({err} / {copy_err})"
            ));
        }
        let _ = fs::remove_file(&tmp_path);
    }
    Ok(())
}

pub fn save_copy_dialog(
    img: &DynamicImage,
    original_path: &Path,
    lang: Language,
) -> Result<Option<PathBuf>, String> {
    let t = lang.tr();
    let parent = original_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = original_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    let orig_ext = original_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "png".to_string());

    let default_ext = match orig_ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "tif" | "tiff" => orig_ext.as_str(),
        _ => "png",
    };
    let suggested = format!("{stem}{}.{default_ext}", lang.edited_file_suffix());

    let chosen = rfd::FileDialog::new()
        .set_title(t.save_edited_copy_title)
        .set_directory(parent)
        .set_file_name(&suggested)
        .add_filter("PNG", &["png"])
        .add_filter("JPEG", &["jpg", "jpeg"])
        .add_filter("WebP", &["webp"])
        .add_filter("BMP", &["bmp"])
        .add_filter("TIFF", &["tiff", "tif"])
        .save_file();

    let Some(target) = chosen else {
        return Ok(None);
    };

    save_image_to_path(img, &target)?;
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn make_test_image() -> Arc<DynamicImage> {
        let mut rgba = RgbaImage::new(20, 10);
        for y in 0..10 {
            for x in 0..20 {
                rgba.put_pixel(x, y, Rgba([x as u8 * 10, y as u8 * 20, 100, 255]));
            }
        }
        Arc::new(DynamicImage::ImageRgba8(rgba))
    }

    #[test]
    fn rotate_and_flip_and_undo() {
        let base = make_test_image();
        let mut editor = ImageEditorState::default();

        editor.rotate_90_cw(None, &base);
        assert!(editor.is_dirty);
        assert_eq!(editor.active_image(&base).dimensions(), (10, 20));

        editor.flip_horizontal(None, &base);
        assert_eq!(editor.active_image(&base).dimensions(), (10, 20));

        editor.undo(None);
        assert!(editor.is_dirty);
        assert_eq!(editor.active_image(&base).dimensions(), (10, 20));

        editor.undo(None);
        assert!(!editor.is_dirty);
        assert_eq!(editor.active_image(&base).dimensions(), (20, 10));
    }

    #[test]
    fn crop_and_resize_exact() {
        let base = make_test_image();
        let mut editor = ImageEditorState::default();
        editor.crop.rect_norm = [0.25, 0.2, 0.75, 0.8];
        assert!(editor.apply_crop(None, &base));
        assert_eq!(editor.active_image(&base).dimensions(), (10, 6));

        assert!(editor.apply_resize(None, &base, 40, 24, ResizeFilter::Lanczos3));
        assert_eq!(editor.active_image(&base).dimensions(), (40, 24));
    }

    #[test]
    fn save_overwrite_and_copy_work() {
        let tmp_dir = std::env::temp_dir().join(format!("imgv_editor_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);
        fs::create_dir_all(&tmp_dir).unwrap();

        let orig = tmp_dir.join("sample.png");
        let base = make_test_image();
        save_image_to_path(&base, &orig).unwrap();
        assert!(orig.exists());

        let rotated = base.rotate90();
        save_overwrite(&rotated, &orig).unwrap();
        let reloaded = image::open(&orig).unwrap();
        assert_eq!(reloaded.dimensions(), (10, 20));

        let jpg_copy = tmp_dir.join("copy.jpg");
        save_image_to_path(&rotated, &jpg_copy).unwrap();
        assert!(jpg_copy.exists());
        let reloaded_jpg = image::open(&jpg_copy).unwrap();
        assert_eq!(reloaded_jpg.dimensions(), (10, 20));

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
