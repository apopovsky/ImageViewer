use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, Pos2, Rect, Rounding, Sense, Stroke, Vec2};
use image::GenericImageView;

use crate::associations::{
    apply_file_associations, apply_preset, default_install_dir, install_application,
    is_running_from_install_dir, query_all_statuses, AssociationPreset, ExtAssociationStatus,
    ExtCategory, ALL_SUPPORTED_EXTS,
};

use crate::cache::{MediaCache, ThumbStatus};
use crate::editor::{
    crop_rect_pixels, save_copy_dialog, save_overwrite, CropAspect, CropDragMode,
    ImageEditorState, ResizeFilter,
};
use crate::i18n::{self, Language};
use crate::media::decoder::DecodedMedia;
use crate::media::video::{format_duration, VideoPlayer};
use crate::media::{
    format_file_size, format_system_time, scan_directory, sort_items, MediaItem, MediaKind,
    SortOrder,
};
use crate::windows_shell::{
    copy_image_to_clipboard, copy_text_to_clipboard, find_registered_editors, launch_with_app,
    move_to_trash, open_url, open_with_system_dialog, reveal_in_explorer, RegisteredApp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Single,
    Gallery,
}

#[derive(Debug, Clone)]
enum PendingAction {
    NavigateTo(usize),
    SwitchToGallery,
    OpenPath(PathBuf),
}

struct ToastMessage {
    text: String,
    is_error: bool,
    expires_at: Instant,
}

pub struct AssocModalState {
    pub open: bool,
    pub installer_mode: bool,
    pub install_dir_text: String,
    pub create_desktop_shortcut: bool,
    pub create_start_menu_shortcut: bool,
    pub selected_exts: HashSet<String>,
    pub statuses: HashMap<String, ExtAssociationStatus>,
    pub active_preset: Option<AssociationPreset>,
    pub last_locked_notice: Option<usize>,
}

impl Default for AssocModalState {
    fn default() -> Self {
        Self {
            open: false,
            installer_mode: false,
            install_dir_text: default_install_dir().to_string_lossy().into_owned(),
            create_desktop_shortcut: true,
            create_start_menu_shortcut: true,
            selected_exts: HashSet::new(),
            statuses: HashMap::new(),
            active_preset: None,
            last_locked_notice: None,
        }
    }
}

pub struct ImageViewerApp {
    current_dir: Option<PathBuf>,
    items: Vec<MediaItem>,
    current_index: Option<usize>,
    last_nav_dir: isize,

    view_mode: ViewMode,
    sort_order: SortOrder,
    gallery_search: String,
    thumb_size: f32,
    gallery_scroll_to_selected: bool,

    // Zoom & Pan & Wheel Nav
    fit_mode: bool,
    last_fit_scale: f32,
    zoom: f32,
    pan: Vec2,
    wheel_nav_accum: f32,
    last_wheel_nav: Instant,
    show_checkerboard: bool,
    show_info_panel: bool,
    is_fullscreen: bool,
    metadata_search: String,

    // Animation (GIF / animated WebP)
    anim_frame_idx: usize,
    anim_last_advance: Instant,
    anim_playing: bool,

    // Video player
    video_player: Option<VideoPlayer>,
    video_target_path: Option<PathBuf>,

    // Cache & Editor & i18n
    lang: Language,
    cache: MediaCache,
    editor: ImageEditorState,

    // Windows "Open with" cache per extension
    editors_cache: HashMap<String, Vec<RegisteredApp>>,

    // Modals & Notifications
    pending_unsaved_action: Option<PendingAction>,
    confirm_delete_path: Option<PathBuf>,
    delete_modal_selected_btn: usize,
    assoc_modal: AssocModalState,
    toast: Option<ToastMessage>,
}

impl ImageViewerApp {
    pub fn new(initial_target: Option<PathBuf>, lang: Language) -> Self {
        let mut app = Self {
            current_dir: None,
            items: Vec::new(),
            current_index: None,
            last_nav_dir: 1,

            view_mode: ViewMode::Single,
            sort_order: SortOrder::NameAsc,
            gallery_search: String::new(),
            thumb_size: 185.0,
            gallery_scroll_to_selected: false,

            fit_mode: true,
            last_fit_scale: 1.0,
            zoom: 1.0,
            pan: Vec2::ZERO,
            wheel_nav_accum: 0.0,
            last_wheel_nav: Instant::now(),
            show_checkerboard: false,
            show_info_panel: false,
            is_fullscreen: false,
            metadata_search: String::new(),

            anim_frame_idx: 0,
            anim_last_advance: Instant::now(),
            anim_playing: true,

            video_player: None,
            video_target_path: None,

            lang,
            cache: MediaCache::new(),
            editor: ImageEditorState::default(),
            editors_cache: HashMap::new(),

            pending_unsaved_action: None,
            confirm_delete_path: None,
            delete_modal_selected_btn: 0,
            assoc_modal: AssocModalState::default(),
            toast: None,
        };

        if let Some(target) = initial_target {
            app.open_path(&target);
        }
        app
    }

    pub fn open_associations_modal(&mut self, installer_mode: bool) {
        let statuses = query_all_statuses();
        let already_ours: HashSet<String> = statuses
            .iter()
            .filter(|(_, s)| s.is_associated_with_us)
            .map(|(k, _)| k.clone())
            .collect();

        let (selected_exts, active_preset) = if !already_ours.is_empty() && !installer_mode {
            (already_ours, None)
        } else {
            (
                apply_preset(AssociationPreset::UnassociatedOnly, &statuses),
                Some(AssociationPreset::UnassociatedOnly),
            )
        };

        self.assoc_modal.statuses = statuses;
        self.assoc_modal.selected_exts = selected_exts;
        self.assoc_modal.active_preset = active_preset;
        self.assoc_modal.installer_mode = installer_mode || !is_running_from_install_dir();
        self.assoc_modal.install_dir_text = default_install_dir().to_string_lossy().into_owned();
        self.assoc_modal.last_locked_notice = None;
        self.assoc_modal.open = true;
    }

    fn set_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(ToastMessage {
            text: msg.into(),
            is_error,
            expires_at: Instant::now() + Duration::from_secs(if is_error { 5 } else { 3 }),
        });
    }

    pub fn open_path(&mut self, target: &Path) {
        if self.editor.is_dirty {
            self.pending_unsaved_action = Some(PendingAction::OpenPath(target.to_path_buf()));
            return;
        }
        self.open_path_immediate(target);
    }

    fn open_path_immediate(&mut self, target: &Path) {
        if target.is_dir() {
            self.load_directory(target, None);
            self.view_mode = ViewMode::Gallery;
            return;
        }

        if let Some(parent) = target.parent() {
            let need_rescan = self.current_dir.as_deref() != Some(parent) || self.items.is_empty();
            if need_rescan {
                self.load_directory(parent, Some(target));
            } else if let Some(idx) = self.items.iter().position(|i| i.path == target) {
                self.select_index_immediate(idx);
            } else {
                self.load_directory(parent, Some(target));
            }
            self.view_mode = ViewMode::Single;
        }
    }

    fn load_directory(&mut self, dir: &Path, select_file: Option<&Path>) {
        self.cache.clear_all();
        self.video_player = None;
        self.video_target_path = None;
        self.editor.reset();

        self.current_dir = Some(dir.to_path_buf());
        let mut items = scan_directory(dir);
        sort_items(&mut items, self.sort_order);
        self.items = items;

        if self.items.is_empty() {
            self.current_index = None;
            return;
        }

        let idx = select_file
            .and_then(|target| self.items.iter().position(|i| i.path == target))
            .unwrap_or(0);
        self.select_index_immediate(idx);
    }

    fn current_item(&self) -> Option<&MediaItem> {
        self.current_index.and_then(|idx| self.items.get(idx))
    }

    fn navigate_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        let cur = self.current_index.unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(len) as usize;
        self.last_nav_dir = if delta >= 0 { 1 } else { -1 };
        self.request_select_index(next);
    }

    fn request_select_index(&mut self, idx: usize) {
        if idx >= self.items.len() {
            return;
        }
        if self.current_index == Some(idx) {
            return;
        }
        if self.editor.is_dirty {
            self.pending_unsaved_action = Some(PendingAction::NavigateTo(idx));
            return;
        }
        self.select_index_immediate(idx);
    }

    fn select_index_immediate(&mut self, idx: usize) {
        if idx >= self.items.len() {
            return;
        }
        self.current_index = Some(idx);
        self.editor.reset();
        self.fit_mode = true;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.anim_frame_idx = 0;
        self.anim_last_advance = Instant::now();
        self.anim_playing = true;

        let item = &self.items[idx];
        if item.kind.is_video() {
            self.video_target_path = Some(item.path.clone());
            match VideoPlayer::open(&item.path, true) {
                Ok(player) => {
                    self.video_player = Some(player);
                }
                Err(e) => {
                    self.video_player = None;
                    self.set_toast(e, true);
                }
            }
        } else {
            self.video_player = None;
            self.video_target_path = None;
        }

        self.cache
            .request_with_preload(&self.items, idx, self.last_nav_dir);
    }

    fn switch_to_gallery(&mut self) {
        if self.editor.is_dirty {
            self.pending_unsaved_action = Some(PendingAction::SwitchToGallery);
            return;
        }
        if let Some(vp) = &mut self.video_player {
            vp.playing = false;
        }
        self.view_mode = ViewMode::Gallery;
        self.scroll_gallery_to_current();
    }

    fn scroll_gallery_to_current(&mut self) {
        if self.current_index.is_some() {
            self.gallery_scroll_to_selected = true;
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.gallery_search.trim().to_ascii_lowercase();
        if query.is_empty() {
            (0..self.items.len()).collect()
        } else {
            self.items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.file_name.to_ascii_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect()
        }
    }

    fn get_editors_for(&mut self, path: &Path) -> Vec<RegisteredApp> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if let Some(cached) = self.editors_cache.get(&ext) {
            return cached.clone();
        }
        let discovered = find_registered_editors(path);
        self.editors_cache.insert(ext, discovered.clone());
        discovered
    }

    fn current_display_image(&mut self) -> Option<std::sync::Arc<image::DynamicImage>> {
        let path = self.current_item()?.path.clone();
        if let Some(working) = &self.editor.working_image {
            return Some(std::sync::Arc::clone(working));
        }
        if let Some(entry) = self.cache.get_full_mut(&path) {
            let frame_idx = self.anim_frame_idx.min(entry.media.frames.len().saturating_sub(1));
            return entry.media.frames.get(frame_idx).map(|f| std::sync::Arc::clone(&f.image));
        }
        None
    }

    fn perform_save_overwrite(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current_item().map(|i| i.path.clone()) else {
            return;
        };
        let Some(img) = self.current_display_image() else {
            return;
        };
        match save_overwrite(&img, &path) {
            Ok(()) => {
                self.editor.reset();
                self.cache.invalidate(&path);
                if let Some(idx) = self.current_index {
                    self.cache
                        .request_with_preload(&self.items, idx, self.last_nav_dir);
                }
                self.set_toast(self.lang.tr().toast_saved_overwrite, false);
                ctx.request_repaint();
            }
            Err(e) => self.set_toast(e, true),
        }
    }

    fn perform_save_copy(&mut self) {
        let Some(path) = self.current_item().map(|i| i.path.clone()) else {
            return;
        };
        let Some(img) = self.current_display_image() else {
            return;
        };
        match save_copy_dialog(&img, &path, self.lang) {
            Ok(Some(saved_path)) => {
                self.editor.reset();
                if let Some(parent) = saved_path.parent() {
                    if self.current_dir.as_deref() == Some(parent) {
                        self.load_directory(parent, Some(&saved_path));
                    }
                }
                let t = self.lang.tr();
                self.set_toast(
                    format!(
                        "{} {}",
                        t.toast_saved_copy_prefix,
                        saved_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(t.default_file_word)
                    ),
                    false,
                );
            }
            Ok(None) => {}
            Err(e) => self.set_toast(e, true),
        }
    }

    fn execute_pending_action(&mut self) {
        let Some(action) = self.pending_unsaved_action.take() else {
            return;
        };
        self.editor.reset();
        match action {
            PendingAction::NavigateTo(idx) => self.select_index_immediate(idx),
            PendingAction::SwitchToGallery => {
                self.view_mode = ViewMode::Gallery;
                self.scroll_gallery_to_current();
            }
            PendingAction::OpenPath(p) => self.open_path_immediate(&p),
        }
    }

    fn open_delete_confirm(&mut self, path: PathBuf) {
        self.confirm_delete_path = Some(path);
        self.delete_modal_selected_btn = 0;
    }

    fn delete_current_item(&mut self) {
        let Some(idx) = self.current_index else {
            return;
        };
        let path = self.items[idx].path.clone();
        self.video_player = None;
        match move_to_trash(&path) {
            Ok(()) => {
                self.cache.invalidate(&path);
                self.items.remove(idx);
                self.editor.reset();
                if self.items.is_empty() {
                    self.current_index = None;
                } else {
                    let next_idx = idx.min(self.items.len() - 1);
                    self.select_index_immediate(next_idx);
                }
                self.set_toast(self.lang.tr().toast_moved_to_trash, false);
            }
            Err(e) => self.set_toast(e, true),
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if let Some(first) = dropped.first() {
            self.open_path(first);
        }
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        let wants_text = ctx.wants_keyboard_input();

        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(Key::O) {
                // Handled outside closure
            }
        });

        let open_file_req = ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::O));
        let open_folder_req = ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(Key::O));
        if open_file_req {
            if let Some(p) = rfd::FileDialog::new().pick_file() {
                self.open_path(&p);
            }
        } else if open_folder_req {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                self.open_path(&dir);
            }
        }

        if ctx.input(|i| i.key_pressed(Key::F11)) {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
        }

        if self.confirm_delete_path.is_some() {
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft)
                    || i.consume_key(egui::Modifiers::NONE, Key::ArrowUp)
                {
                    self.delete_modal_selected_btn = 0;
                } else if i.consume_key(egui::Modifiers::NONE, Key::ArrowRight)
                    || i.consume_key(egui::Modifiers::NONE, Key::ArrowDown)
                {
                    self.delete_modal_selected_btn = 1;
                } else if i.consume_key(egui::Modifiers::NONE, Key::Tab) {
                    self.delete_modal_selected_btn = 1 - self.delete_modal_selected_btn;
                } else if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                    self.confirm_delete_path = None;
                } else if i.consume_key(egui::Modifiers::NONE, Key::Enter)
                    || i.consume_key(egui::Modifiers::NONE, Key::Space)
                {
                    if self.delete_modal_selected_btn == 0 {
                        self.confirm_delete_path = None;
                        self.delete_current_item();
                    } else {
                        self.confirm_delete_path = None;
                    }
                }
            });
            return;
        }

        if self.assoc_modal.open {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.assoc_modal.open = false;
            }
            return;
        }

        if wants_text || self.pending_unsaved_action.is_some() {
            return;
        }

        // Toggle Gallery / Single with Enter (unless cropping or resizing)
        if !self.editor.crop.active && !self.editor.resize.open && ctx.input(|i| i.key_pressed(Key::Enter)) {
            match self.view_mode {
                ViewMode::Single => {
                    if !self.items.is_empty() {
                        self.switch_to_gallery();
                    }
                }
                ViewMode::Gallery => {
                    if let Some(idx) = self.current_index {
                        self.select_index_immediate(idx);
                        self.view_mode = ViewMode::Single;
                    }
                }
            }
            return;
        }

        if self.view_mode == ViewMode::Gallery {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.view_mode = ViewMode::Single;
            }
            return;
        }

        // Single view shortcuts
        let is_video = self.current_item().map(|i| i.kind.is_video()).unwrap_or(false);

        if self.editor.crop.active {
            if ctx.input(|i| i.key_pressed(Key::Enter)) {
                if let Some(base) = self.current_display_image_base() {
                    self.editor.apply_crop(Some(ctx), &base);
                    self.fit_mode = true;
                    self.pan = Vec2::ZERO;
                }
            } else if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.editor.crop.active = false;
                self.editor.crop.reset();
            }
            return;
        }

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.editor.resize.open {
                self.editor.resize.open = false;
            } else if self.is_fullscreen {
                self.is_fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }

        if ctx.input(|i| i.key_pressed(Key::ArrowRight) || i.pointer.button_pressed(egui::PointerButton::Extra2)) {
            self.navigate_by(1);
        } else if ctx.input(|i| i.key_pressed(Key::ArrowLeft) || i.pointer.button_pressed(egui::PointerButton::Extra1)) {
            self.navigate_by(-1);
        }

        if ctx.input(|i| i.key_pressed(Key::Space)) {
            if let Some(vp) = &mut self.video_player {
                vp.toggle_play();
            } else {
                self.anim_playing = !self.anim_playing;
            }
        }

        if ctx.input(|i| i.key_pressed(Key::I)) {
            self.show_info_panel = !self.show_info_panel;
        }
        if ctx.input(|i| i.key_pressed(Key::T)) {
            self.show_checkerboard = !self.show_checkerboard;
        }
        if ctx.input(|i| i.key_pressed(Key::Delete)) {
            if let Some(item) = self.current_item() {
                self.open_delete_confirm(item.path.clone());
            }
        }

        // Zoom shortcuts
        if ctx.input(|i| i.key_pressed(Key::Num0) || (i.modifiers.ctrl && i.key_pressed(Key::Num0))) {
            self.fit_mode = true;
            self.pan = Vec2::ZERO;
        } else if ctx.input(|i| i.key_pressed(Key::Num1)) {
            self.fit_mode = false;
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        } else if ctx.input(|i| i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)) {
            self.step_zoom(1.25);
        } else if ctx.input(|i| i.key_pressed(Key::Minus)) {
            self.step_zoom(0.8);
        }

        if !is_video {
            if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::R)) {
                if let Some(base) = self.current_display_image_base() {
                    self.editor.rotate_90_cw(Some(ctx), &base);
                }
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::L)) {
                if let Some(base) = self.current_display_image_base() {
                    self.editor.rotate_90_ccw(Some(ctx), &base);
                }
            } else if ctx.input(|i| i.key_pressed(Key::C) && !i.modifiers.ctrl) {
                self.editor.crop.active = true;
                self.editor.crop.reset();
                self.fit_mode = true;
                self.pan = Vec2::ZERO;
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::E)) {
                if let Some(img) = self.current_display_image() {
                    let (w, h) = img.dimensions();
                    self.editor.resize.open_with_dims(w, h);
                }
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::Z)) {
                self.editor.undo(Some(ctx));
            } else if ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(Key::S)) {
                if self.editor.is_dirty {
                    self.perform_save_copy();
                }
            } else if ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::S)) {
                if self.editor.is_dirty {
                    self.perform_save_overwrite(ctx);
                }
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::C)) {
                if let Some(img) = self.current_display_image() {
                    match copy_image_to_clipboard(&img) {
                        Ok(()) => self.set_toast(self.lang.tr().toast_image_copied, false),
                        Err(e) => self.set_toast(e, true),
                    }
                }
            }
        }
    }

    fn current_display_image_base(&mut self) -> Option<std::sync::Arc<image::DynamicImage>> {
        let path = self.current_item()?.path.clone();
        let entry = self.cache.get_full_mut(&path)?;
        let idx = self.anim_frame_idx.min(entry.media.frames.len().saturating_sub(1));
        entry.media.frames.get(idx).map(|f| std::sync::Arc::clone(&f.image))
    }

    fn step_zoom(&mut self, factor: f32) {
        if self.fit_mode {
            self.zoom = self.last_fit_scale;
            self.fit_mode = false;
        }
        self.zoom = (self.zoom * factor).clamp(0.05, 32.0);
    }

    fn update_window_title(&self, ctx: &egui::Context) {
        let title = match (self.current_item(), self.current_index) {
            (Some(item), Some(idx)) => {
                let dirty = if self.editor.is_dirty { "* " } else { "" };
                format!(
                    "{dirty}{} ({}/{}) — ImageViewer",
                    item.file_name,
                    idx + 1,
                    self.items.len()
                )
            }
            _ => "ImageViewer".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn render_language_menu(&mut self, ui: &mut egui::Ui) {
        let t = self.lang.tr();
        let label = format!("🌐 {}", self.lang.short_label());
        let mut chosen: Option<Language> = None;
        ui.menu_button(label, |ui| {
            ui.set_min_width(140.0);
            for lang in Language::ALL {
                if ui.selectable_label(lang == self.lang, lang.display_name()).clicked() {
                    chosen = Some(lang);
                    ui.close_menu();
                }
            }
        })
        .response
        .on_hover_text(t.tip_language);

        if let Some(new_lang) = chosen {
            if new_lang != self.lang {
                self.lang = new_lang;
                i18n::save_language(new_lang);
            }
        }
    }

    fn render_top_bar(&mut self, ctx: &egui::Context) {
        let t = self.lang.tr();
        egui::TopBottomPanel::top("top_toolbar")
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(27, 29, 36))
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 49, 58))),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(7.0, 6.0);
                ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
                let btn_h = 32.0;

                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(t.btn_open).min_size(Vec2::new(0.0, btn_h)))
                        .on_hover_text(t.tip_open)
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            self.open_path(&p);
                        }
                    }

                    if !self.items.is_empty() {
                        let is_gallery = self.view_mode == ViewMode::Gallery;
                        let label = if is_gallery {
                            t.btn_view_image
                        } else {
                            t.btn_view_gallery
                        };
                        let btn = egui::Button::new(label)
                            .min_size(Vec2::new(0.0, btn_h))
                            .fill(if is_gallery {
                                Color32::from_rgb(37, 99, 235)
                            } else {
                                Color32::from_rgb(43, 47, 58)
                            });
                        if ui
                            .add(btn)
                            .on_hover_text(t.tip_toggle_gallery)
                            .clicked()
                        {
                            if is_gallery {
                                self.view_mode = ViewMode::Single;
                            } else {
                                self.switch_to_gallery();
                            }
                        }
                    }

                    if self.view_mode == ViewMode::Single && self.current_item().is_some() {
                        let is_video = self.current_item().map(|i| i.kind.is_video()).unwrap_or(false);
                        if !is_video {
                            ui.separator();
                            if ui
                                .add(egui::Button::new("↺").min_size(Vec2::new(34.0, btn_h)))
                                .on_hover_text(t.tip_rotate_ccw)
                                .clicked()
                            {
                                if let Some(base) = self.current_display_image_base() {
                                    self.editor.rotate_90_ccw(Some(ctx), &base);
                                }
                            }
                            if ui
                                .add(egui::Button::new("↻").min_size(Vec2::new(34.0, btn_h)))
                                .on_hover_text(t.tip_rotate_cw)
                                .clicked()
                            {
                                if let Some(base) = self.current_display_image_base() {
                                    self.editor.rotate_90_cw(Some(ctx), &base);
                                }
                            }
                            if ui
                                .add(egui::Button::new("⇄").min_size(Vec2::new(34.0, btn_h)))
                                .on_hover_text(t.tip_flip_h)
                                .clicked()
                            {
                                if let Some(base) = self.current_display_image_base() {
                                    self.editor.flip_horizontal(Some(ctx), &base);
                                }
                            }
                            if ui
                                .add(egui::Button::new("⇅").min_size(Vec2::new(34.0, btn_h)))
                                .on_hover_text(t.tip_flip_v)
                                .clicked()
                            {
                                if let Some(base) = self.current_display_image_base() {
                                    self.editor.flip_vertical(Some(ctx), &base);
                                }
                            }

                            let crop_btn = egui::Button::new(t.btn_crop)
                                .min_size(Vec2::new(0.0, btn_h))
                                .fill(if self.editor.crop.active {
                                    Color32::from_rgb(37, 99, 235)
                                } else {
                                    Color32::from_rgb(43, 47, 58)
                                });
                            if ui.add(crop_btn).on_hover_text(t.tip_crop).clicked() {
                                self.editor.crop.active = !self.editor.crop.active;
                                if self.editor.crop.active {
                                    self.editor.crop.reset();
                                    self.fit_mode = true;
                                    self.pan = Vec2::ZERO;
                                }
                            }

                            if ui
                                .add(egui::Button::new(t.btn_resize).min_size(Vec2::new(0.0, btn_h)))
                                .on_hover_text(t.tip_resize)
                                .clicked()
                            {
                                if let Some(img) = self.current_display_image() {
                                    let (w, h) = img.dimensions();
                                    self.editor.resize.open_with_dims(w, h);
                                }
                            }
                        }

                        if self.editor.is_dirty {
                            ui.separator();
                            if ui
                                .add(
                                    egui::Button::new(t.btn_save_copy)
                                        .min_size(Vec2::new(0.0, btn_h))
                                        .fill(Color32::from_rgb(28, 125, 76)),
                                )
                                .on_hover_text(t.tip_save_copy)
                                .clicked()
                            {
                                self.perform_save_copy();
                            }
                            if ui
                                .add(
                                    egui::Button::new(t.btn_overwrite)
                                        .min_size(Vec2::new(0.0, btn_h))
                                        .fill(Color32::from_rgb(37, 99, 235)),
                                )
                                .on_hover_text(t.tip_overwrite)
                                .clicked()
                            {
                                self.perform_save_overwrite(ctx);
                            }
                            if ui
                                .add(egui::Button::new("↩").min_size(Vec2::new(34.0, btn_h)))
                                .on_hover_text(t.tip_undo)
                                .clicked()
                            {
                                self.editor.undo(Some(ctx));
                            }
                            if ui
                                .add(egui::Button::new("✖").min_size(Vec2::new(34.0, btn_h)))
                                .on_hover_text(t.tip_discard)
                                .clicked()
                            {
                                self.editor.reset();
                            }
                        }

                        // Right side actions: Language, Assoc, Delete, Info, Copy, Open With
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.render_language_menu(ui);

                            if ui
                                .add(egui::Button::new(t.btn_associate).min_size(Vec2::new(0.0, btn_h)))
                                .on_hover_text(t.tip_associate)
                                .clicked()
                            {
                                self.open_associations_modal(false);
                            }

                            if let Some(item_path) = self.current_item().map(|i| i.path.clone()) {
                                let editors = self.get_editors_for(&item_path);
                                if ui
                                    .add(egui::Button::new("🗑").min_size(Vec2::new(36.0, btn_h)))
                                    .on_hover_text(t.tip_delete)
                                    .clicked()
                                {
                                    self.open_delete_confirm(item_path.clone());
                                }

                                let info_btn = egui::Button::new(t.btn_info)
                                    .min_size(Vec2::new(0.0, btn_h))
                                    .fill(if self.show_info_panel {
                                        Color32::from_rgb(37, 99, 235)
                                    } else {
                                        Color32::from_rgb(43, 47, 58)
                                    });
                                if ui
                                    .add(info_btn)
                                    .on_hover_text(t.tip_info)
                                    .clicked()
                                {
                                    self.show_info_panel = !self.show_info_panel;
                                }

                                if !is_video
                                    && ui
                                        .add(egui::Button::new("📋").min_size(Vec2::new(36.0, btn_h)))
                                        .on_hover_text(t.tip_copy_image)
                                        .clicked()
                                {
                                    if let Some(img) = self.current_display_image() {
                                        match copy_image_to_clipboard(&img) {
                                            Ok(()) => {
                                                self.set_toast(t.toast_image_copied, false)
                                            }
                                            Err(e) => self.set_toast(e, true),
                                        }
                                    }
                                }

                                ui.menu_button(t.btn_open_with, |ui| {
                                    ui.set_min_width(220.0);
                                    if editors.is_empty() {
                                        ui.label(t.no_direct_editors);
                                    } else {
                                        for app in &editors {
                                            if ui.button(format!("↗ {}", app.name)).clicked() {
                                                if let Err(e) = launch_with_app(app, &item_path) {
                                                    self.set_toast(e, true);
                                                }
                                                ui.close_menu();
                                            }
                                        }
                                    }
                                    ui.separator();
                                    if ui.button(t.choose_other_app_win).clicked() {
                                        if let Err(e) = open_with_system_dialog(&item_path) {
                                            self.set_toast(e, true);
                                        }
                                        ui.close_menu();
                                    }
                                    if ui.button(t.btn_show_in_explorer).clicked() {
                                        if let Err(e) = reveal_in_explorer(&item_path) {
                                            self.set_toast(e, true);
                                        }
                                        ui.close_menu();
                                    }
                                });
                            }
                        });
                    } else {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.render_language_menu(ui);

                            if ui
                                .add(egui::Button::new(t.btn_associate_install).min_size(Vec2::new(0.0, btn_h)))
                                .on_hover_text(t.tip_associate)
                                .clicked()
                            {
                                self.open_associations_modal(false);
                            }
                        });
                    }
                });
            });
    }

    fn render_bottom_bar(&mut self, ctx: &egui::Context) {
        if self.view_mode != ViewMode::Single {
            return;
        }
        let Some(item) = self.current_item().cloned() else {
            return;
        };
        let t = self.lang.tr();
        let idx = self.current_index.unwrap_or(0);
        let total = self.items.len();

        let (dims_text, format_text) = if let Some(working) = &self.editor.working_image {
            let (w, h) = working.dimensions();
            (
                format!("{w} × {h} px ({})", t.status_edited_suffix),
                item.kind.badge().unwrap_or("IMG").to_string(),
            )
        } else if let Some(vp) = &self.video_player {
            (
                format!("{} × {} px", vp.original_size.0, vp.original_size.1),
                format!("VIDEO {:.0} fps", vp.fps),
            )
        } else if let Some(entry) = self.cache.get_full_mut(&item.path) {
            let (w, h) = entry.media.original_dimensions;
            (
                format!("{w} × {h} px"),
                self.lang.translate_format_label(&entry.media.format_label),
            )
        } else {
            (t.loading.to_string(), String::new())
        };

        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(24, 26, 32))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(42, 46, 56))),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} / {}", idx + 1, total))
                            .strong()
                            .color(Color32::from_rgb(147, 197, 253)),
                    );
                    ui.separator();
                    if !format_text.is_empty() {
                        ui.label(
                            egui::RichText::new(&format_text)
                                .small()
                                .color(Color32::from_rgb(165, 175, 195)),
                        );
                        ui.separator();
                    }
                    ui.label(&dims_text);
                    ui.separator();
                    ui.label(format_file_size(item.file_size));
                    ui.separator();
                    ui.label(
                        egui::RichText::new(truncate_middle(&item.file_name, 42))
                            .color(Color32::from_rgb(230, 235, 244)),
                    );

                    // Bottom-right modern zoom bar (matching Windows Photos style)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let fs_btn = egui::Button::new("⛶")
                            .min_size(Vec2::new(28.0, 26.0))
                            .fill(if self.is_fullscreen {
                                Color32::from_rgb(37, 99, 235)
                            } else {
                                Color32::from_rgb(38, 42, 52)
                            });
                        if ui.add(fs_btn).on_hover_text(t.tip_fullscreen).clicked() {
                            self.is_fullscreen = !self.is_fullscreen;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                        }

                        if ui
                            .add(egui::Button::new("➕").min_size(Vec2::new(28.0, 26.0)))
                            .on_hover_text(t.tip_zoom_in)
                            .clicked()
                        {
                            self.step_zoom(1.25);
                        }

                        let mut slider_zoom = if self.fit_mode {
                            self.last_fit_scale.clamp(0.10, 5.0)
                        } else {
                            self.zoom.clamp(0.10, 5.0)
                        };
                        ui.spacing_mut().slider_width = 110.0;
                        let slider_resp = ui.add(
                            egui::Slider::new(&mut slider_zoom, 0.10..=5.0)
                                .logarithmic(true)
                                .show_value(false),
                        );
                        if slider_resp.changed() {
                            self.fit_mode = false;
                            self.zoom = slider_zoom;
                        }

                        if ui
                            .add(egui::Button::new("➖").min_size(Vec2::new(28.0, 26.0)))
                            .on_hover_text(t.tip_zoom_out)
                            .clicked()
                        {
                            self.step_zoom(0.8);
                        }

                        let zoom_label = if self.fit_mode {
                            format!("{} ({:.0}%)", t.zoom_fit_prefix, self.last_fit_scale * 100.0)
                        } else {
                            format!("{:.0}%", self.zoom * 100.0)
                        };
                        if ui
                            .add(egui::Button::new(&zoom_label).min_size(Vec2::new(0.0, 26.0)))
                            .on_hover_text(t.tip_zoom_toggle)
                            .clicked()
                        {
                            if self.fit_mode {
                                self.fit_mode = false;
                                self.zoom = 1.0;
                            } else {
                                self.fit_mode = true;
                            }
                            self.pan = Vec2::ZERO;
                        }

                        let checker_btn = egui::Button::new("🏁")
                            .min_size(Vec2::new(28.0, 26.0))
                            .fill(if self.show_checkerboard {
                                Color32::from_rgb(37, 99, 235)
                            } else {
                                Color32::from_rgb(38, 42, 52)
                            });
                        if ui
                            .add(checker_btn)
                            .on_hover_text(t.tip_checkerboard)
                            .clicked()
                        {
                            self.show_checkerboard = !self.show_checkerboard;
                        }
                    });
                });
            });
    }

    fn render_info_panel(&mut self, ctx: &egui::Context) {
        if !self.show_info_panel || self.view_mode != ViewMode::Single {
            return;
        }
        let Some(item) = self.current_item().cloned() else {
            return;
        };

        let lang = self.lang;
        let t = lang.tr();

        let media_info = self
            .cache
            .get_full_mut(&item.path)
            .map(|e| e.media.clone());

        egui::SidePanel::right("info_panel")
            .resizable(true)
            .default_width(340.0)
            .min_width(280.0)
            .max_width(480.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(26, 28, 35))
                    .inner_margin(egui::Margin::same(14.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 49, 58))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(t.info_heading);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new("✖").min_size(Vec2::new(28.0, 28.0)))
                            .clicked()
                        {
                            self.show_info_panel = false;
                        }
                    });
                });
                ui.add_space(6.0);

                // Quick action tiles row (matching modern Windows Photos style)
                ui.columns(4, |cols| {
                    let tile_size = Vec2::new(cols[0].available_width(), 48.0);
                    let is_video = item.kind.is_video();
                    if cols[0]
                        .add_enabled(
                            !is_video,
                            egui::Button::new(t.tile_resize).min_size(tile_size),
                        )
                        .on_hover_text(t.tile_resize_tip)
                        .clicked()
                    {
                        if let Some(img) = self.current_display_image() {
                            let (w, h) = img.dimensions();
                            self.editor.resize.open_with_dims(w, h);
                        }
                    }

                    if cols[1]
                        .add(egui::Button::new(t.tile_path).min_size(tile_size))
                        .on_hover_text(t.tile_path_tip)
                        .clicked()
                    {
                        match copy_text_to_clipboard(&item.path.display().to_string()) {
                            Ok(()) => self.set_toast(t.toast_path_copied, false),
                            Err(e) => self.set_toast(e, true),
                        }
                    }

                    if cols[2]
                        .add(egui::Button::new(t.tile_metadata).min_size(tile_size))
                        .on_hover_text(t.tile_metadata_tip)
                        .clicked()
                    {
                        let report = build_metadata_report(&item, media_info.as_ref(), lang);
                        match copy_text_to_clipboard(&report) {
                            Ok(()) => self.set_toast(t.toast_metadata_copied, false),
                            Err(e) => self.set_toast(e, true),
                        }
                    }

                    if cols[3]
                        .add(egui::Button::new(t.tile_folder).min_size(tile_size))
                        .on_hover_text(t.tile_folder_tip)
                        .clicked()
                    {
                        if let Err(e) = reveal_in_explorer(&item.path) {
                            self.set_toast(e, true);
                        }
                    }
                });

                ui.add_space(8.0);
                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);

                        // File name
                        info_section_title(ui, t.sec_file_name);
                        ui.label(egui::RichText::new(&item.file_name).strong().color(Color32::WHITE));
                        ui.add_space(6.0);

                        // Date and time
                        info_section_title(ui, t.sec_date_time);
                        if let Some(m) = &media_info {
                            if let Some(taken) = &m.exif.date_taken {
                                ui.label(format!("{}: {taken}", t.lbl_taken_exif));
                            }
                        }
                        if let Some(mod_str) = format_system_time(item.modified) {
                            ui.label(
                                egui::RichText::new(format!("{}: {mod_str}", t.lbl_modified))
                                    .color(Color32::from_rgb(195, 202, 214)),
                            );
                        }
                        if let Some(cre_str) = format_system_time(item.created) {
                            ui.label(
                                egui::RichText::new(format!("{}: {cre_str}", t.lbl_created))
                                    .small()
                                    .color(Color32::GRAY),
                            );
                        }
                        ui.add_space(6.0);

                        // Size & Resolution summary
                        info_section_title(ui, t.sec_size_image);
                        if let Some(m) = &media_info {
                            ui.label(format!(
                                "{} × {} px   •   {:.1} MP   ({})",
                                m.original_dimensions.0,
                                m.original_dimensions.1,
                                m.megapixels,
                                m.aspect_ratio_label
                            ));
                            let mut sub_parts = vec![
                                format_file_size(item.file_size),
                                lang.translate_format_label(&m.format_label),
                                lang.translate_color_depth(&m.color_depth_label),
                            ];
                            if let Some(dpi) = &m.exif.dpi_x {
                                sub_parts.push(dpi.clone());
                            }
                            if let Some(dur) = m.video_duration_secs {
                                sub_parts.push(format!("{} {}", t.lbl_duration, format_duration(dur)));
                            }
                            ui.label(
                                egui::RichText::new(sub_parts.join("  •  "))
                                    .small()
                                    .color(Color32::from_rgb(175, 185, 202)),
                            );
                        } else {
                            ui.label(format_file_size(item.file_size));
                        }
                        ui.add_space(6.0);

                        // Device / Camera summary
                        if let Some(m) = &media_info {
                            let has_cam = m.exif.camera_make.is_some()
                                || m.exif.camera_model.is_some()
                                || m.exif.exposure_time.is_some()
                                || m.exif.f_number.is_some()
                                || m.exif.iso.is_some();
                            if has_cam {
                                info_section_title(ui, t.sec_device_capture);
                                let dev = match (&m.exif.camera_make, &m.exif.camera_model) {
                                    (Some(mk), Some(md)) => {
                                        if md.to_ascii_lowercase().starts_with(&mk.to_ascii_lowercase()) {
                                            md.clone()
                                        } else {
                                            format!("{mk} {md}")
                                        }
                                    }
                                    (None, Some(md)) => md.clone(),
                                    (Some(mk), None) => mk.clone(),
                                    (None, None) => t.lbl_camera.to_string(),
                                };
                                ui.label(egui::RichText::new(dev).strong().color(Color32::WHITE));

                                let mut shot_line = Vec::new();
                                if let Some(v) = &m.exif.focal_length {
                                    shot_line.push(v.clone());
                                }
                                if let Some(v) = &m.exif.f_number {
                                    shot_line.push(v.clone());
                                }
                                if let Some(v) = &m.exif.exposure_time {
                                    shot_line.push(v.clone());
                                }
                                if let Some(v) = &m.exif.iso {
                                    shot_line.push(format!("ISO {v}"));
                                }
                                if let Some(v) = &m.exif.exposure_bias {
                                    shot_line.push(v.clone());
                                }
                                if !shot_line.is_empty() {
                                    ui.label(
                                        egui::RichText::new(shot_line.join("   •   "))
                                            .small()
                                            .color(Color32::from_rgb(180, 200, 230)),
                                    );
                                }
                                if let Some(lens) = &m.exif.lens_model {
                                    ui.label(
                                        egui::RichText::new(format!("{}: {lens}", t.lbl_lens))
                                            .small()
                                            .color(Color32::GRAY),
                                    );
                                }
                                ui.add_space(6.0);
                            }

                            // GPS Location card
                            if let Some((lat, lon)) = m.exif.gps_lat_lon {
                                info_section_title(ui, t.sec_gps);
                                egui::Frame::none()
                                    .fill(Color32::from_rgb(33, 37, 47))
                                    .rounding(Rounding::same(8.0))
                                    .inner_margin(egui::Margin::same(10.0))
                                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(54, 62, 80)))
                                    .show(ui, |ui| {
                                        ui.label(format!("Lat: {lat:.6}   Lon: {lon:.6}"));
                                        if let Some(alt) = &m.exif.gps_altitude {
                                            ui.label(
                                                egui::RichText::new(format!("{}: {alt}", t.lbl_altitude))
                                                    .small()
                                                    .color(Color32::LIGHT_GRAY),
                                            );
                                        }
                                        ui.add_space(4.0);
                                        if ui.button(t.btn_google_maps).clicked() {
                                            let url = format!("https://www.google.com/maps?q={lat:.6},{lon:.6}");
                                            if let Err(e) = open_url(&url) {
                                                self.set_toast(e, true);
                                            }
                                        }
                                    });
                                ui.add_space(6.0);
                            }
                        }

                        // Source & Folder Path
                        info_section_title(ui, t.sec_source_folder);
                        if ui
                            .link(item.path.display().to_string())
                            .on_hover_text(t.tip_show_win_explorer)
                            .clicked()
                        {
                            if let Err(e) = reveal_in_explorer(&item.path) {
                                self.set_toast(e, true);
                            }
                        }

                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(4.0);

                        // Full Additional Metadata section with search
                        ui.label(
                            egui::RichText::new(t.lbl_additional_meta)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.metadata_search)
                                    .hint_text(t.hint_search_meta)
                                    .desired_width(ui.available_width() - 32.0),
                            );
                            if !self.metadata_search.is_empty() && ui.button("✖").clicked() {
                                self.metadata_search.clear();
                            }
                        });

                        let query = self.metadata_search.trim().to_ascii_lowercase();
                        let mut all_sections = build_file_metadata_section(&item, media_info.as_ref(), lang);
                        if let Some(m) = &media_info {
                            for (cat, rows) in &m.exif.categories {
                                let loc_cat = lang.translate_meta_category(cat);
                                let loc_rows = rows
                                    .iter()
                                    .map(|(k, v)| (lang.translate_meta_key(k), v.clone()))
                                    .collect();
                                all_sections.push((loc_cat, loc_rows));
                            }
                        }

                        let mut any_shown = false;
                        for (cat, rows) in all_sections {
                            let cat_matches = query.is_empty() || cat.to_ascii_lowercase().contains(&query);
                            let filtered_rows: Vec<(String, String)> = rows
                                .into_iter()
                                .filter(|(k, v)| {
                                    cat_matches
                                        || k.to_ascii_lowercase().contains(&query)
                                        || v.to_ascii_lowercase().contains(&query)
                                })
                                .collect();

                            if filtered_rows.is_empty() {
                                continue;
                            }
                            any_shown = true;
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(&cat)
                                    .strong()
                                    .color(Color32::from_rgb(125, 178, 255)),
                            );
                            egui::Grid::new(format!("meta_grid_{cat}"))
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    for (k, v) in filtered_rows {
                                        ui.label(
                                            egui::RichText::new(k)
                                                .small()
                                                .color(Color32::from_rgb(168, 178, 195)),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(v)
                                                    .small()
                                                    .color(Color32::from_rgb(235, 240, 248)),
                                            )
                                            .wrap(),
                                        );
                                        ui.end_row();
                                    }
                                });
                        }

                        if !any_shown {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(t.no_matching_props)
                                    .small()
                                    .color(Color32::GRAY),
                            );
                        }
                    });
            });
    }

    fn render_single_view(&mut self, ctx: &egui::Context) {
        if self.current_item().is_none() {
            self.render_welcome_screen(ctx);
            return;
        }

        let item = self.current_item().unwrap().clone();

        // Update animated GIF/WebP frame timer
        let mut multi_frame_info: Option<(usize, u32)> = None;
        if !item.kind.is_video() && !self.editor.is_dirty {
            if let Some(entry) = self.cache.get_full_mut(&item.path) {
                let count = entry.media.frames.len();
                if count > 1 {
                    let cur = self.anim_frame_idx % count;
                    let delay = entry.media.frames[cur].delay_ms.max(16);
                    multi_frame_info = Some((count, delay));
                }
            }
        }
        if let Some((count, delay)) = multi_frame_info {
            if self.anim_playing && self.anim_last_advance.elapsed() >= Duration::from_millis(delay as u64) {
                self.anim_frame_idx = (self.anim_frame_idx + 1) % count;
                self.anim_last_advance = Instant::now();
            }
            if self.anim_playing {
                ctx.request_repaint_after(Duration::from_millis((delay / 2).max(8) as u64));
            }
        }

        // Update video stream if active
        if let Some(vp) = &mut self.video_player {
            vp.update(ctx);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(16, 17, 21)))
            .show(ctx, |ui| {
                let canvas_rect = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(canvas_rect, Sense::click_and_drag());

                // Obtain texture to render
                let texture_opt: Option<(egui::TextureId, [usize; 2])> = if let Some(vp) = &self.video_player {
                    vp.texture.as_ref().map(|t| (t.id(), vp.stream_size))
                } else if self.editor.is_dirty {
                    if self.editor.working_texture.is_none() {
                        if let Some(img) = &self.editor.working_image {
                            let color = crate::media::decoder::dynamic_to_color_image(img);
                            self.editor.working_texture = Some(ctx.load_texture(
                                "editor_working_tex",
                                color,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    self.editor.working_texture.as_ref().map(|t| (t.id(), t.size()))
                } else if let Some(entry) = self.cache.get_full_mut(&item.path) {
                    let frame_idx = if entry.media.frames.len() > 1 {
                        self.anim_frame_idx % entry.media.frames.len()
                    } else {
                        0
                    };
                    entry
                        .ensure_texture(ctx, frame_idx, &item.path)
                        .map(|t| (t.id(), t.size()))
                } else {
                    None
                };

                let mut drawn_image_rect: Option<Rect> = None;

                if let Some((tex_id, tex_size)) = texture_opt {
                    let img_w = tex_size[0].max(1) as f32;
                    let img_h = tex_size[1].max(1) as f32;

                    // Reserve margin for video controls at bottom if video
                    let view_rect = if item.kind.is_video() {
                        Rect::from_min_max(
                            canvas_rect.min,
                            Pos2::new(canvas_rect.max.x, (canvas_rect.max.y - 60.0).max(canvas_rect.min.y + 40.0)),
                        )
                    } else {
                        canvas_rect
                    };

                    let pad = if self.editor.crop.active { 32.0 } else { 8.0 };
                    let avail_w = (view_rect.width() - pad).max(16.0);
                    let avail_h = (view_rect.height() - pad).max(16.0);
                    let fit_scale = (avail_w / img_w).min(avail_h / img_h);
                    self.last_fit_scale = fit_scale;

                    // Mouse wheel: Wheel alone = navigate prev/next; Ctrl + Wheel = zoom anchored at cursor
                    if !self.editor.crop.active && response.hovered() {
                        let (scroll_y, ctrl_down) = ctx.input(|i| {
                            (i.raw_scroll_delta.y, i.modifiers.ctrl || i.modifiers.command)
                        });
                        if scroll_y.abs() > 0.1 {
                            if ctrl_down {
                                let old_scale = if self.fit_mode { fit_scale } else { self.zoom };
                                let factor = (1.0 + scroll_y * 0.0018).clamp(0.75, 1.35);
                                let new_zoom = (old_scale * factor).clamp(0.05, 32.0);
                                if let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) {
                                    let center = view_rect.center();
                                    let cursor_rel = pointer - (center + self.pan);
                                    self.pan -= cursor_rel * (new_zoom / old_scale - 1.0);
                                }
                                self.fit_mode = false;
                                self.zoom = new_zoom;
                            } else if self.items.len() > 1 {
                                self.wheel_nav_accum += scroll_y;
                                let threshold = 20.0;
                                if self.last_wheel_nav.elapsed() >= Duration::from_millis(60) {
                                    if self.wheel_nav_accum <= -threshold {
                                        self.wheel_nav_accum = 0.0;
                                        self.last_wheel_nav = Instant::now();
                                        self.navigate_by(1);
                                    } else if self.wheel_nav_accum >= threshold {
                                        self.wheel_nav_accum = 0.0;
                                        self.last_wheel_nav = Instant::now();
                                        self.navigate_by(-1);
                                    }
                                }
                            }
                        }
                    }

                    // Double click toggles Fit <-> 100%
                    if !self.editor.crop.active && response.double_clicked() {
                        if self.fit_mode {
                            let old_scale = fit_scale;
                            let new_zoom = 1.0f32;
                            if let Some(pointer) = response.interact_pointer_pos() {
                                let center = view_rect.center();
                                let cursor_rel = pointer - (center + self.pan);
                                self.pan -= cursor_rel * (new_zoom / old_scale - 1.0);
                            }
                            self.fit_mode = false;
                            self.zoom = new_zoom;
                        } else {
                            self.fit_mode = true;
                            self.pan = Vec2::ZERO;
                        }
                    }

                    // Drag to pan when zoomed
                    if !self.editor.crop.active && response.dragged_by(egui::PointerButton::Primary) {
                        if self.fit_mode {
                            self.fit_mode = false;
                            self.zoom = fit_scale;
                        }
                        self.pan += response.drag_delta();
                    }

                    let scale = if self.fit_mode { fit_scale } else { self.zoom };
                    let disp_size = Vec2::new(img_w * scale, img_h * scale);
                    let img_center = view_rect.center() + if self.fit_mode { Vec2::ZERO } else { self.pan };
                    let img_rect = Rect::from_center_size(img_center, disp_size);
                    drawn_image_rect = Some(img_rect);

                    let painter = ui.painter_at(view_rect);

                    if self.show_checkerboard {
                        draw_checkerboard(&painter, img_rect);
                    }

                    painter.image(
                        tex_id,
                        img_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );

                    // Crop overlay interaction
                    if self.editor.crop.active {
                        self.handle_and_draw_crop_overlay(ui, &painter, img_rect, img_w as u32, img_h as u32);
                    }
                } else if let Some(err) = self.cache.get_full_error(&item.path) {
                    ui.painter().text(
                        canvas_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("⚠ {err}"),
                        egui::FontId::proportional(16.0),
                        Color32::from_rgb(240, 120, 120),
                    );
                } else {
                    ui.painter().text(
                        canvas_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        self.lang.tr().loading,
                        egui::FontId::proportional(16.0),
                        Color32::LIGHT_GRAY,
                    );
                }

                // Right-click context menu on canvas
                if !self.editor.crop.active {
                    let t = self.lang.tr();
                    let item_path = item.path.clone();
                    let editors = self.get_editors_for(&item_path);
                    response.context_menu(|ui| {
                        if ui.button(t.ctx_view_gallery).clicked() {
                            self.switch_to_gallery();
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.menu_button(t.ctx_open_with, |ui| {
                            for app in &editors {
                                if ui.button(&app.name).clicked() {
                                    if let Err(e) = launch_with_app(app, &item_path) {
                                        self.set_toast(e, true);
                                    }
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui.button(t.ctx_choose_other_app).clicked() {
                                if let Err(e) = open_with_system_dialog(&item_path) {
                                    self.set_toast(e, true);
                                }
                                ui.close_menu();
                            }
                        });
                        if ui.button(t.btn_show_in_explorer).clicked() {
                            if let Err(e) = reveal_in_explorer(&item_path) {
                                self.set_toast(e, true);
                            }
                            ui.close_menu();
                        }
                        if !item.kind.is_video() && ui.button(t.ctx_copy_image).clicked() {
                            if let Some(img) = self.current_display_image() {
                                if let Err(e) = copy_image_to_clipboard(&img) {
                                    self.set_toast(e, true);
                                } else {
                                    self.set_toast(t.toast_image_copied, false);
                                }
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button(t.ctx_move_to_trash).clicked() {
                            self.open_delete_confirm(item_path);
                            ui.close_menu();
                        }
                    });
                }

                // Floating Left / Right navigation buttons
                if self.items.len() > 1 && !self.editor.crop.active {
                    let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
                    let near_left = pointer_pos
                        .map(|p| canvas_rect.contains(p) && p.x - canvas_rect.min.x < 120.0)
                        .unwrap_or(false);
                    let near_right = pointer_pos
                        .map(|p| canvas_rect.contains(p) && canvas_rect.max.x - p.x < 120.0)
                        .unwrap_or(false);

                    let left_rect = Rect::from_center_size(
                        Pos2::new(canvas_rect.min.x + 28.0, canvas_rect.center().y),
                        Vec2::new(40.0, 64.0),
                    );
                    let right_rect = Rect::from_center_size(
                        Pos2::new(canvas_rect.max.x - 28.0, canvas_rect.center().y),
                        Vec2::new(40.0, 64.0),
                    );

                    if self.draw_nav_arrow(ui, left_rect, "◀", near_left) {
                        self.navigate_by(-1);
                    }
                    if self.draw_nav_arrow(ui, right_rect, "▶", near_right) {
                        self.navigate_by(1);
                    }
                }

                // Crop action bar overlay
                if self.editor.crop.active {
                    if let Some(img_rect) = drawn_image_rect {
                        let _ = img_rect;
                    }
                    self.render_crop_toolbar(ui, canvas_rect, &item.path);
                }

                // Video playback bar overlay
                if item.kind.is_video() {
                    self.render_video_controls(ui, canvas_rect);
                }
            });
    }

    fn draw_nav_arrow(&self, ui: &mut egui::Ui, rect: Rect, icon: &str, highlight: bool) -> bool {
        let resp = ui.allocate_rect(rect, Sense::click());
        let hovered = resp.hovered();
        let alpha = if hovered {
            210
        } else if highlight {
            150
        } else {
            45
        };
        let bg = Color32::from_rgba_unmultiplied(30, 34, 42, alpha);
        let fg = Color32::from_rgba_unmultiplied(240, 244, 250, if hovered { 255 } else if highlight { 220 } else { 110 });
        ui.painter()
            .rect_filled(rect, Rounding::same(10.0), bg);
        if hovered {
            ui.painter()
                .rect_stroke(rect, Rounding::same(10.0), Stroke::new(1.0_f32, Color32::from_rgb(100, 140, 220)));
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(20.0),
            fg,
        );
        resp.clicked()
    }

    fn handle_and_draw_crop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        img_rect: Rect,
        img_w: u32,
        img_h: u32,
    ) {
        let to_screen = |norm: [f32; 4]| -> Rect {
            Rect::from_min_max(
                Pos2::new(
                    img_rect.min.x + norm[0] * img_rect.width(),
                    img_rect.min.y + norm[1] * img_rect.height(),
                ),
                Pos2::new(
                    img_rect.min.x + norm[2] * img_rect.width(),
                    img_rect.min.y + norm[3] * img_rect.height(),
                ),
            )
        };

        let crop_rect = to_screen(self.editor.crop.rect_norm);
        let resp = ui.interact(img_rect.expand(16.0), ui.id().with("crop_interact"), Sense::drag());

        let pointer_pos = ui.input(|i| i.pointer.hover_pos());
        let hit_radius = 12.0f32;

        let detect_mode = |pos: Pos2, r: Rect| -> Option<CropDragMode> {
            let near_l = (pos.x - r.min.x).abs() <= hit_radius;
            let near_r = (pos.x - r.max.x).abs() <= hit_radius;
            let near_t = (pos.y - r.min.y).abs() <= hit_radius;
            let near_b = (pos.y - r.max.y).abs() <= hit_radius;
            if near_l && near_t {
                Some(CropDragMode::TopLeft)
            } else if near_r && near_t {
                Some(CropDragMode::TopRight)
            } else if near_l && near_b {
                Some(CropDragMode::BottomLeft)
            } else if near_r && near_b {
                Some(CropDragMode::BottomRight)
            } else if near_t && pos.x >= r.min.x && pos.x <= r.max.x {
                Some(CropDragMode::Top)
            } else if near_b && pos.x >= r.min.x && pos.x <= r.max.x {
                Some(CropDragMode::Bottom)
            } else if near_l && pos.y >= r.min.y && pos.y <= r.max.y {
                Some(CropDragMode::Left)
            } else if near_r && pos.y >= r.min.y && pos.y <= r.max.y {
                Some(CropDragMode::Right)
            } else if r.contains(pos) {
                Some(CropDragMode::Move)
            } else {
                None
            }
        };

        if let Some(pos) = pointer_pos {
            if let Some(mode) = self.editor.crop.drag_mode.or_else(|| detect_mode(pos, crop_rect)) {
                let icon = match mode {
                    CropDragMode::TopLeft | CropDragMode::BottomRight => egui::CursorIcon::ResizeNwSe,
                    CropDragMode::TopRight | CropDragMode::BottomLeft => egui::CursorIcon::ResizeNeSw,
                    CropDragMode::Top | CropDragMode::Bottom => egui::CursorIcon::ResizeVertical,
                    CropDragMode::Left | CropDragMode::Right => egui::CursorIcon::ResizeHorizontal,
                    CropDragMode::Move => egui::CursorIcon::Grab,
                };
                ui.ctx().set_cursor_icon(icon);
            }
        }

        if resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                self.editor.crop.drag_mode = detect_mode(pos, crop_rect);
                self.editor.crop.drag_start_norm = self.editor.crop.rect_norm;
                self.editor.crop.drag_start_pointer = [
                    (pos.x - img_rect.min.x) / img_rect.width().max(1.0),
                    (pos.y - img_rect.min.y) / img_rect.height().max(1.0),
                ];
            }
        }

        if resp.dragged() {
            if let (Some(mode), Some(pos)) = (self.editor.crop.drag_mode, resp.interact_pointer_pos()) {
                let cur_nx = (pos.x - img_rect.min.x) / img_rect.width().max(1.0);
                let cur_ny = (pos.y - img_rect.min.y) / img_rect.height().max(1.0);
                let dx = cur_nx - self.editor.crop.drag_start_pointer[0];
                let dy = cur_ny - self.editor.crop.drag_start_pointer[1];
                let s = self.editor.crop.drag_start_norm;
                let min_span = 0.02f32;

                let mut n = s;
                match mode {
                    CropDragMode::Move => {
                        let w = s[2] - s[0];
                        let h = s[3] - s[1];
                        let x0 = (s[0] + dx).clamp(0.0, 1.0 - w);
                        let y0 = (s[1] + dy).clamp(0.0, 1.0 - h);
                        n = [x0, y0, x0 + w, y0 + h];
                    }
                    CropDragMode::TopLeft => {
                        n[0] = (s[0] + dx).clamp(0.0, s[2] - min_span);
                        n[1] = (s[1] + dy).clamp(0.0, s[3] - min_span);
                    }
                    CropDragMode::TopRight => {
                        n[2] = (s[2] + dx).clamp(s[0] + min_span, 1.0);
                        n[1] = (s[1] + dy).clamp(0.0, s[3] - min_span);
                    }
                    CropDragMode::BottomLeft => {
                        n[0] = (s[0] + dx).clamp(0.0, s[2] - min_span);
                        n[3] = (s[3] + dy).clamp(s[1] + min_span, 1.0);
                    }
                    CropDragMode::BottomRight => {
                        n[2] = (s[2] + dx).clamp(s[0] + min_span, 1.0);
                        n[3] = (s[3] + dy).clamp(s[1] + min_span, 1.0);
                    }
                    CropDragMode::Top => {
                        n[1] = (s[1] + dy).clamp(0.0, s[3] - min_span);
                    }
                    CropDragMode::Bottom => {
                        n[3] = (s[3] + dy).clamp(s[1] + min_span, 1.0);
                    }
                    CropDragMode::Left => {
                        n[0] = (s[0] + dx).clamp(0.0, s[2] - min_span);
                    }
                    CropDragMode::Right => {
                        n[2] = (s[2] + dx).clamp(s[0] + min_span, 1.0);
                    }
                }

                // Apply aspect ratio lock when dragging corners/edges
                if mode != CropDragMode::Move {
                    if let Some(ratio) = self.editor.crop.aspect.ratio(img_w, img_h) {
                        let iw = img_w.max(1) as f32;
                        let ih = img_h.max(1) as f32;
                        let w_px = (n[2] - n[0]) * iw;
                        let h_px = w_px / ratio;
                        let h_norm = h_px / ih;
                        match mode {
                            CropDragMode::TopLeft | CropDragMode::TopRight | CropDragMode::Top => {
                                n[1] = (n[3] - h_norm).clamp(0.0, n[3] - min_span);
                                let actual_w = ((n[3] - n[1]) * ih * ratio) / iw;
                                if mode == CropDragMode::TopLeft {
                                    n[0] = (n[2] - actual_w).clamp(0.0, n[2] - min_span);
                                } else {
                                    n[2] = (n[0] + actual_w).clamp(n[0] + min_span, 1.0);
                                }
                            }
                            _ => {
                                n[3] = (n[1] + h_norm).clamp(n[1] + min_span, 1.0);
                                let actual_w = ((n[3] - n[1]) * ih * ratio) / iw;
                                if mode == CropDragMode::BottomLeft || mode == CropDragMode::Left {
                                    n[0] = (n[2] - actual_w).clamp(0.0, n[2] - min_span);
                                } else {
                                    n[2] = (n[0] + actual_w).clamp(n[0] + min_span, 1.0);
                                }
                            }
                        }
                    }
                }

                self.editor.crop.rect_norm = n;
            }
        }

        if resp.drag_stopped() {
            self.editor.crop.drag_mode = None;
        }

        let cr = to_screen(self.editor.crop.rect_norm);
        let shade = Color32::from_black_alpha(155);

        // Darken outside areas (4 strips)
        painter.rect_filled(
            Rect::from_min_max(img_rect.min, Pos2::new(img_rect.max.x, cr.min.y)),
            0.0,
            shade,
        );
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(img_rect.min.x, cr.max.y), img_rect.max),
            0.0,
            shade,
        );
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(img_rect.min.x, cr.min.y), Pos2::new(cr.min.x, cr.max.y)),
            0.0,
            shade,
        );
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(cr.max.x, cr.min.y), Pos2::new(img_rect.max.x, cr.max.y)),
            0.0,
            shade,
        );

        // Rule of thirds lines
        let third_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(80));
        for k in [1.0 / 3.0, 2.0 / 3.0] {
            let x = cr.min.x + cr.width() * k;
            let y = cr.min.y + cr.height() * k;
            painter.line_segment([Pos2::new(x, cr.min.y), Pos2::new(x, cr.max.y)], third_stroke);
            painter.line_segment([Pos2::new(cr.min.x, y), Pos2::new(cr.max.x, y)], third_stroke);
        }

        // Crop border & corner handles
        painter.rect_stroke(cr, 0.0, Stroke::new(2.0_f32, Color32::WHITE));
        for corner in [cr.left_top(), cr.right_top(), cr.left_bottom(), cr.right_bottom()] {
            painter.rect_filled(
                Rect::from_center_size(corner, Vec2::splat(10.0)),
                2.0,
                Color32::from_rgb(76, 150, 255),
            );
            painter.rect_stroke(
                Rect::from_center_size(corner, Vec2::splat(10.0)),
                2.0,
                Stroke::new(1.5_f32, Color32::WHITE),
            );
        }
    }

    fn render_crop_toolbar(&mut self, ui: &mut egui::Ui, canvas_rect: Rect, _item_path: &Path) {
        let Some(img) = self.current_display_image() else {
            return;
        };
        let lang = self.lang;
        let t = lang.tr();
        let (iw, ih) = img.dimensions();
        let (_, _, cw, ch) = crop_rect_pixels(iw, ih, self.editor.crop.rect_norm);

        egui::Area::new(egui::Id::new("crop_toolbar_area"))
            .fixed_pos(Pos2::new(canvas_rect.center().x - 270.0, canvas_rect.max.y - 54.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::none()
                    .fill(Color32::from_rgba_unmultiplied(24, 27, 34, 240))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(70, 78, 95)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("{cw} × {ch} px")).strong());
                            ui.separator();
                            for aspect in CropAspect::ALL {
                                let selected = self.editor.crop.aspect == aspect;
                                if ui.selectable_label(selected, aspect.label(lang)).clicked() {
                                    self.editor.crop.aspect = aspect;
                                    self.editor.crop.enforce_aspect(iw, ih);
                                }
                            }
                            ui.separator();
                            if ui
                                .add(egui::Button::new(t.crop_apply).fill(Color32::from_rgb(35, 118, 72)))
                                .clicked()
                            {
                                let ctx_clone = ui.ctx().clone();
                                if let Some(base) = self.current_display_image_base() {
                                    self.editor.apply_crop(Some(&ctx_clone), &base);
                                    self.fit_mode = true;
                                    self.pan = Vec2::ZERO;
                                }
                            }
                            if ui.button(t.cancel).clicked() {
                                self.editor.crop.active = false;
                                self.editor.crop.reset();
                            }
                        });
                    });
            });
    }

    fn render_video_controls(&mut self, ui: &mut egui::Ui, canvas_rect: Rect) {
        let t = self.lang.tr();
        let mut capture_img: Option<image::DynamicImage> = None;
        let bar_rect = Rect::from_min_max(
            Pos2::new(canvas_rect.min.x + 16.0, canvas_rect.max.y - 52.0),
            Pos2::new(canvas_rect.max.x - 16.0, canvas_rect.max.y - 8.0),
        );

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
            egui::Frame::none()
                .fill(Color32::from_rgba_unmultiplied(22, 24, 30, 235))
                .rounding(Rounding::same(10.0))
                .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(58, 63, 76)))
                .show(ui, |ui| {
                    if let Some(vp) = &mut self.video_player {
                        ui.horizontal(|ui| {
                            let play_icon = if vp.playing { t.video_pause } else { t.video_play };
                            if ui.button(play_icon).on_hover_text(t.key_space).clicked() {
                                vp.toggle_play();
                            }
                            if ui.button("⏭ +1").on_hover_text(t.video_step_tip).clicked() {
                                vp.step_one_frame(ui.ctx());
                            }

                            let dur = vp.duration_secs.max(0.1);
                            let mut pos = vp.position_secs;
                            ui.spacing_mut().slider_width = (ui.available_width() - 260.0).max(120.0);
                            let slider_resp = ui.add(
                                egui::Slider::new(&mut pos, 0.0..=dur)
                                    .show_value(false)
                                    .trailing_fill(true),
                            );
                            if slider_resp.changed() || slider_resp.drag_stopped() {
                                vp.seek(pos);
                            }

                            ui.label(format!(
                                "{} / {}",
                                format_duration(vp.position_secs),
                                format_duration(vp.duration_secs)
                            ));

                            let loop_btn = egui::Button::new("🔁").fill(if vp.looping {
                                Color32::from_rgb(56, 102, 180)
                            } else {
                                Color32::from_rgb(42, 46, 56)
                            });
                            if ui.add(loop_btn).on_hover_text(t.video_loop_tip).clicked() {
                                vp.looping = !vp.looping;
                            }

                            if ui
                                .button(t.video_capture)
                                .on_hover_text(t.video_capture_tip)
                                .clicked()
                            {
                                capture_img = vp.current_dynamic_image();
                            }
                        });
                    }
                });
        });

        if let Some(frame_img) = capture_img {
            if let Some(path) = self.current_item().map(|i| i.path.clone()) {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                if let Some(target) = rfd::FileDialog::new()
                    .set_title(t.video_save_frame_title)
                    .set_directory(parent)
                    .set_file_name(&format!("{stem}_frame.png"))
                    .add_filter("PNG", &["png"])
                    .add_filter("JPEG", &["jpg", "jpeg"])
                    .save_file()
                {
                    match crate::editor::save_image_to_path(&frame_img, &target) {
                        Ok(()) => {
                            self.set_toast(t.toast_frame_saved, false);
                            if target.parent() == self.current_dir.as_deref() {
                                self.load_directory(parent, Some(path.as_path()));
                            }
                        }
                        Err(e) => self.set_toast(e, true),
                    }
                }
            }
        }
    }

    fn render_gallery_view(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let t = lang.tr();
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(18, 20, 25)))
            .show(ctx, |ui| {
                // Gallery controls bar
                ui.horizontal(|ui| {
                    ui.label("🔍");
                    let search_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.gallery_search)
                            .hint_text(t.gallery_filter_hint)
                            .desired_width(220.0),
                    );
                    if !self.gallery_search.is_empty() && ui.button("✖").clicked() {
                        self.gallery_search.clear();
                    }
                    let _ = search_resp;

                    ui.separator();
                    ui.label(t.gallery_sort);
                    let prev_sort = self.sort_order;
                    egui::ComboBox::from_id_salt("gallery_sort")
                        .selected_text(self.sort_order.label(lang))
                        .show_ui(ui, |ui| {
                            for ord in SortOrder::ALL {
                                ui.selectable_value(&mut self.sort_order, ord, ord.label(lang));
                            }
                        });
                    if self.sort_order != prev_sort {
                        let cur_path = self.current_item().map(|i| i.path.clone());
                        sort_items(&mut self.items, self.sort_order);
                        if let Some(p) = cur_path {
                            self.current_index = self.items.iter().position(|i| i.path == p);
                        }
                    }

                    ui.separator();
                    ui.label(t.gallery_size);
                    ui.add(egui::Slider::new(&mut self.thumb_size, 110.0..=300.0).show_value(false));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(dir) = &self.current_dir {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} ({} {})",
                                    dir.display(),
                                    self.items.len(),
                                    t.gallery_items_word
                                ))
                                .small()
                                .color(Color32::GRAY),
                            );
                        }
                    });
                });

                ui.separator();

                let filtered = self.filtered_indices();
                if filtered.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(t.gallery_empty_filter);
                    });
                    return;
                }

                let spacing = 12.0f32;
                let tile_w = self.thumb_size;
                let tile_h = self.thumb_size + 28.0;
                let avail_w = ui.available_width().max(tile_w + spacing);
                let cols = ((avail_w + spacing) / (tile_w + spacing)).floor().max(1.0) as usize;
                let total_rows = filtered.len().div_ceil(cols);
                let row_height = tile_h + spacing;

                // Keyboard grid navigation inside gallery when search box isn't focused
                if !ctx.wants_keyboard_input() {
                    let cur_pos = self
                        .current_index
                        .and_then(|c| filtered.iter().position(|&idx| idx == c))
                        .unwrap_or(0);
                    let mut next_pos: Option<usize> = None;
                    if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
                        next_pos = Some((cur_pos + 1).min(filtered.len() - 1));
                    } else if ctx.input(|i| i.key_pressed(Key::ArrowLeft)) {
                        next_pos = Some(cur_pos.saturating_sub(1));
                    } else if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
                        next_pos = Some((cur_pos + cols).min(filtered.len() - 1));
                    } else if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
                        next_pos = Some(cur_pos.saturating_sub(cols));
                    }
                    if let Some(np) = next_pos {
                        let sel = filtered[np];
                        self.current_index = Some(sel);
                        self.gallery_scroll_to_selected = true;
                        self.cache.request_with_preload(&self.items, sel, 1);
                    }
                }

                let mut scroll_area = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if self.gallery_scroll_to_selected {
                    self.gallery_scroll_to_selected = false;
                    if let Some(cur_pos) = self
                        .current_index
                        .and_then(|c| filtered.iter().position(|&idx| idx == c))
                    {
                        let row = cur_pos / cols;
                        let offset_y = (row as f32 * row_height - 100.0).max(0.0);
                        scroll_area = scroll_area.vertical_scroll_offset(offset_y);
                    }
                }

                let mut selected_idx: Option<usize> = None;
                let mut opened_idx: Option<usize> = None;

                scroll_area.show_rows(ui, row_height, total_rows, |ui, row_range| {
                    // Prioritized background thumbnail requests for visible + buffer rows
                    let buf_start = row_range.start.saturating_sub(2);
                    let buf_end = (row_range.end + 3).min(total_rows);

                    let mut visible_paths = Vec::new();
                    for r in row_range.clone() {
                        let start = r * cols;
                        let end = (start + cols).min(filtered.len());
                        for &item_idx in &filtered[start..end] {
                            visible_paths.push(self.items[item_idx].path.clone());
                        }
                    }
                    self.cache.request_thumbnails(&visible_paths, true);

                    let mut buffer_paths = Vec::new();
                    for r in buf_start..buf_end {
                        if !row_range.contains(&r) {
                            let start = r * cols;
                            let end = (start + cols).min(filtered.len());
                            for &item_idx in &filtered[start..end] {
                                buffer_paths.push(self.items[item_idx].path.clone());
                            }
                        }
                    }
                    if !buffer_paths.is_empty() {
                        self.cache.request_thumbnails(&buffer_paths, false);
                    }

                    for r in row_range {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(spacing, spacing);
                            let start = r * cols;
                            let end = (start + cols).min(filtered.len());
                            for &item_idx in &filtered[start..end] {
                                let item = &self.items[item_idx];
                                let is_selected = self.current_index == Some(item_idx);
                                let (tile_rect, resp) =
                                    ui.allocate_exact_size(Vec2::new(tile_w, tile_h), Sense::click());

                                let hovered = resp.hovered();
                                let bg = if is_selected {
                                    Color32::from_rgb(38, 56, 86)
                                } else if hovered {
                                    Color32::from_rgb(35, 39, 48)
                                } else {
                                    Color32::from_rgb(26, 29, 36)
                                };
                                let border = if is_selected {
                                    Stroke::new(2.0_f32, Color32::from_rgb(86, 156, 255))
                                } else if hovered {
                                    Stroke::new(1.5_f32, Color32::from_rgb(95, 110, 140))
                                } else {
                                    Stroke::new(1.0_f32, Color32::from_rgb(42, 46, 56))
                                };

                                let painter = ui.painter_at(tile_rect);
                                painter.rect_filled(tile_rect, Rounding::same(8.0), bg);
                                painter.rect_stroke(tile_rect, Rounding::same(8.0), border);

                                let thumb_box = Rect::from_min_max(
                                    tile_rect.min + Vec2::new(6.0, 6.0),
                                    Pos2::new(tile_rect.max.x - 6.0, tile_rect.max.y - 26.0),
                                );

                                match self.cache.thumbnail_status(&item.path) {
                                    ThumbStatus::Ready(tex) => {
                                        let ts = tex.size();
                                        let tw = ts[0].max(1) as f32;
                                        let th = ts[1].max(1) as f32;
                                        let s = (thumb_box.width() / tw).min(thumb_box.height() / th);
                                        let draw_rect = Rect::from_center_size(
                                            thumb_box.center(),
                                            Vec2::new(tw * s, th * s),
                                        );
                                        painter.image(
                                            tex.id(),
                                            draw_rect,
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    }
                                    ThumbStatus::Failed => {
                                        painter.text(
                                            thumb_box.center(),
                                            egui::Align2::CENTER_CENTER,
                                            if item.kind.is_video() { "🎬" } else { "🖼" },
                                            egui::FontId::proportional(28.0),
                                            Color32::GRAY,
                                        );
                                    }
                                    ThumbStatus::Loading => {
                                        painter.text(
                                            thumb_box.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "...",
                                            egui::FontId::proportional(16.0),
                                            Color32::DARK_GRAY,
                                        );
                                    }
                                }

                                // Media type badge
                                if let Some(badge) = item.kind.badge() {
                                    let badge_color = match item.kind {
                                        MediaKind::Video => Color32::from_rgba_unmultiplied(180, 45, 55, 220),
                                        MediaKind::AnimatedImage => Color32::from_rgba_unmultiplied(45, 130, 180, 220),
                                        _ => Color32::from_rgba_unmultiplied(90, 90, 110, 210),
                                    };
                                    let b_rect = Rect::from_min_size(
                                        Pos2::new(thumb_box.max.x - 54.0, thumb_box.min.y + 4.0),
                                        Vec2::new(50.0, 18.0),
                                    );
                                    painter.rect_filled(b_rect, Rounding::same(4.0), badge_color);
                                    painter.text(
                                        b_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        badge,
                                        egui::FontId::proportional(10.0),
                                        Color32::WHITE,
                                    );
                                }

                                // File name label
                                let label_pos = Pos2::new(tile_rect.center().x, tile_rect.max.y - 13.0);
                                let display_name = truncate_middle(&item.file_name, (tile_w / 7.5) as usize);
                                painter.text(
                                    label_pos,
                                    egui::Align2::CENTER_CENTER,
                                    display_name,
                                    egui::FontId::proportional(12.0),
                                    Color32::from_rgb(225, 230, 240),
                                );

                                if resp.double_clicked() {
                                    opened_idx = Some(item_idx);
                                } else if resp.clicked() {
                                    selected_idx = Some(item_idx);
                                }
                                resp.on_hover_text(format!(
                                    "{}\n{}\n{}",
                                    item.file_name,
                                    format_file_size(item.file_size),
                                    t.gallery_tile_tip
                                ));
                            }
                        });
                        ui.add_space(spacing);
                    }
                });

                if let Some(idx) = opened_idx {
                    self.select_index_immediate(idx);
                    self.view_mode = ViewMode::Single;
                } else if let Some(idx) = selected_idx {
                    self.current_index = Some(idx);
                    self.cache.request_with_preload(&self.items, idx, 1);
                }
            });
    }

    fn render_welcome_screen(&mut self, ctx: &egui::Context) {
        let t = self.lang.tr();
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(18, 20, 25)))
            .show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.28);
                        ui.heading(
                            egui::RichText::new(t.welcome_title)
                                .size(26.0)
                                .color(Color32::WHITE),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(t.welcome_subtitle)
                                .size(15.0)
                                .color(Color32::LIGHT_GRAY),
                        );
                        ui.add_space(22.0);
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 500.0).max(0.0) * 0.5);
                            if ui
                                .add_sized(
                                    [160.0, 38.0],
                                    egui::Button::new(t.welcome_open_file),
                                )
                                .clicked()
                            {
                                if let Some(p) = rfd::FileDialog::new().pick_file() {
                                    self.open_path(&p);
                                }
                            }
                            ui.add_space(10.0);
                            if ui
                                .add_sized(
                                    [150.0, 38.0],
                                    egui::Button::new(t.welcome_open_folder),
                                )
                                .clicked()
                            {
                                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                    self.open_path(&dir);
                                }
                            }
                            ui.add_space(10.0);
                            if ui
                                .add_sized(
                                    [168.0, 38.0],
                                    egui::Button::new(t.welcome_install_assoc),
                                )
                                .clicked()
                            {
                                self.open_associations_modal(true);
                            }
                        });
                    });
                });
            });
    }

    fn render_modals_and_toasts(&mut self, ctx: &egui::Context) {
        let lang = self.lang;
        let t = lang.tr();

        // Resize modal
        if self.editor.resize.open {
            let mut close = false;
            let mut apply = false;
            egui::Window::new(t.resize_title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{}: {} × {} px",
                        t.resize_current_size,
                        self.editor.resize.orig_w,
                        self.editor.resize.orig_h
                    ));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(t.resize_width);
                        let w_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.editor.resize.width_text)
                                .desired_width(75.0),
                        );
                        if w_resp.changed() {
                            self.editor.resize.sync_height_from_width();
                        }

                        ui.label(t.resize_height);
                        let h_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.editor.resize.height_text)
                                .desired_width(75.0),
                        );
                        if h_resp.changed() {
                            self.editor.resize.sync_width_from_height();
                        }
                    });

                    ui.checkbox(&mut self.editor.resize.lock_aspect, t.resize_keep_aspect);

                    ui.horizontal(|ui| {
                        ui.label(t.resize_quick_scale);
                        for pct in [25.0, 50.0, 75.0, 150.0, 200.0] {
                            if ui.button(format!("{pct:.0}%")).clicked() {
                                self.editor.resize.set_percent(pct);
                            }
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label(t.resize_filter);
                        egui::ComboBox::from_id_salt("resize_filter")
                            .selected_text(self.editor.resize.filter.label(lang))
                            .show_ui(ui, |ui| {
                                for f in ResizeFilter::ALL {
                                    ui.selectable_value(&mut self.editor.resize.filter, f, f.label(lang));
                                }
                            });
                    });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let valid = self.editor.resize.parsed_dims().is_some();
                        if ui
                            .add_enabled(valid, egui::Button::new(t.crop_apply).fill(Color32::from_rgb(35, 118, 72)))
                            .clicked()
                        {
                            apply = true;
                        }
                        if ui.button(t.cancel).clicked() {
                            close = true;
                        }
                    });
                });

            if apply {
                if let (Some((nw, nh)), Some(base)) = (
                    self.editor.resize.parsed_dims(),
                    self.current_display_image_base(),
                ) {
                    let filter = self.editor.resize.filter;
                    self.editor.apply_resize(Some(ctx), &base, nw, nh, filter);
                }
            } else if close {
                self.editor.resize.open = false;
            }
        }

        // Unsaved changes prompt
        if self.pending_unsaved_action.is_some() {
            egui::Window::new(t.unsaved_title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(t.unsaved_body);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(t.btn_save_copy).fill(Color32::from_rgb(35, 118, 72)))
                            .clicked()
                        {
                            self.perform_save_copy();
                            if !self.editor.is_dirty {
                                self.execute_pending_action();
                            }
                        }
                        if ui
                            .add(egui::Button::new(t.btn_overwrite).fill(Color32::from_rgb(44, 96, 168)))
                            .clicked()
                        {
                            self.perform_save_overwrite(ctx);
                            if !self.editor.is_dirty {
                                self.execute_pending_action();
                            }
                        }
                        if ui.button(t.btn_discard).clicked() {
                            self.execute_pending_action();
                        }
                        if ui.button(t.cancel).clicked() {
                            self.pending_unsaved_action = None;
                        }
                    });
                });
        }

        // Confirm delete modal (keyboard navigable with Enter, Arrow keys, Tab, Esc)
        if let Some(target_path) = self.confirm_delete_path.clone() {
            egui::Window::new(t.delete_title)
                .collapsible(false)
                .resizable(false)
                .min_width(360.0)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    let name = target_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(t.delete_default_name);
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} \"{name}\" {}",
                            t.delete_prompt_prefix, t.delete_prompt_suffix
                        ))
                        .size(15.0)
                        .color(Color32::WHITE),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(t.delete_keyboard_hint)
                            .small()
                            .color(Color32::GRAY),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let del_focused = self.delete_modal_selected_btn == 0;
                        let cancel_focused = self.delete_modal_selected_btn == 1;

                        let del_btn = egui::Button::new(
                            egui::RichText::new(t.btn_delete).strong().color(Color32::WHITE),
                        )
                        .min_size(Vec2::new(145.0, 36.0))
                        .fill(if del_focused {
                            Color32::from_rgb(198, 52, 52)
                        } else {
                            Color32::from_rgb(120, 42, 42)
                        })
                        .stroke(if del_focused {
                            Stroke::new(2.0_f32, Color32::WHITE)
                        } else {
                            Stroke::new(1.0_f32, Color32::from_rgb(140, 65, 65))
                        });

                        let cancel_btn = egui::Button::new(t.cancel)
                            .min_size(Vec2::new(130.0, 36.0))
                            .fill(if cancel_focused {
                                Color32::from_rgb(56, 92, 160)
                            } else {
                                Color32::from_rgb(48, 52, 64)
                            })
                            .stroke(if cancel_focused {
                                Stroke::new(2.0_f32, Color32::WHITE)
                            } else {
                                Stroke::new(1.0_f32, Color32::from_rgb(72, 78, 94))
                            });

                        let del_resp = ui.add(del_btn);
                        if del_resp.hovered() {
                            self.delete_modal_selected_btn = 0;
                        }
                        if del_resp.clicked() {
                            self.confirm_delete_path = None;
                            self.delete_current_item();
                        }

                        ui.add_space(8.0);
                        let cancel_resp = ui.add(cancel_btn);
                        if cancel_resp.hovered() {
                            self.delete_modal_selected_btn = 1;
                        }
                        if cancel_resp.clicked() {
                            self.confirm_delete_path = None;
                        }
                    });
                });
        }

        // Associations & Simple Installer modal
        self.render_associations_modal(ctx);

        // Toast notification
        if let Some(toast) = &self.toast {
            if Instant::now() < toast.expires_at {
                let bg = if toast.is_error {
                    Color32::from_rgba_unmultiplied(150, 38, 42, 235)
                } else {
                    Color32::from_rgba_unmultiplied(32, 82, 56, 235)
                };
                egui::Area::new(egui::Id::new("toast_area"))
                    .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 52.0))
                    .show(ctx, |ui| {
                        egui::Frame::none()
                            .fill(bg)
                            .rounding(Rounding::same(8.0))
                            .inner_margin(egui::Margin::symmetric(14.0, 8.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(&toast.text).color(Color32::WHITE));
                            });
                    });
                ctx.request_repaint_after(Duration::from_millis(200));
            } else {
                self.toast = None;
            }
        }
    }

    fn render_associations_modal(&mut self, ctx: &egui::Context) {
        if !self.assoc_modal.open {
            return;
        }

        let lang = self.lang;
        let t = lang.tr();
        let mut close_modal = false;
        let mut do_apply = false;
        let mut toast_msg: Option<(String, bool)> = None;

        egui::Window::new(t.assoc_title)
            .collapsible(false)
            .resizable(false)
            .min_width(680.0)
            .max_width(760.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);

                // 1. Optional Windows user-level install section
                egui::Frame::none()
                    .fill(Color32::from_rgb(33, 37, 47))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(54, 60, 74)))
                    .show(ui, |ui| {
                        ui.checkbox(
                            &mut self.assoc_modal.installer_mode,
                            egui::RichText::new(t.assoc_install_check)
                                .strong()
                                .color(Color32::WHITE),
                        );

                        if self.assoc_modal.installer_mode {
                            ui.horizontal(|ui| {
                                ui.label(t.assoc_folder);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.assoc_modal.install_dir_text)
                                        .desired_width(430.0),
                                );
                                if ui.button(t.assoc_browse).clicked() {
                                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                        self.assoc_modal.install_dir_text =
                                            dir.to_string_lossy().into_owned();
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.checkbox(
                                    &mut self.assoc_modal.create_desktop_shortcut,
                                    t.assoc_desktop_shortcut,
                                );
                                ui.add_space(12.0);
                                ui.checkbox(
                                    &mut self.assoc_modal.create_start_menu_shortcut,
                                    t.assoc_start_menu,
                                );
                            });
                        }
                    });

                // 2. Quick selection presets
                ui.label(
                    egui::RichText::new(t.assoc_choose_types)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.horizontal_wrapped(|ui| {
                    let presets = [
                        (AssociationPreset::All, "☑"),
                        (AssociationPreset::None, "☐"),
                        (AssociationPreset::UnassociatedOnly, "✨"),
                        (AssociationPreset::ImagesOnly, "🖼"),
                        (AssociationPreset::VideosOnly, "🎬"),
                    ];
                    for (preset, icon) in presets {
                        let active = self.assoc_modal.active_preset == Some(preset);
                        let label = format!("{icon} {}", preset.label(lang));
                        let btn = egui::Button::new(label)
                            .min_size(Vec2::new(0.0, 30.0))
                            .fill(if active {
                                Color32::from_rgb(37, 99, 235)
                            } else {
                                Color32::from_rgb(43, 47, 58)
                            });
                        if ui.add(btn).clicked() {
                            self.assoc_modal.selected_exts =
                                apply_preset(preset, &self.assoc_modal.statuses);
                            self.assoc_modal.active_preset = Some(preset);
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} {} {} {}",
                            t.assoc_selected_prefix,
                            self.assoc_modal.selected_exts.len(),
                            t.assoc_selected_mid,
                            ALL_SUPPORTED_EXTS.len(),
                            t.assoc_selected_suffix
                        ))
                        .color(Color32::from_rgb(147, 197, 253)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(t.assoc_refresh).clicked() {
                            self.assoc_modal.statuses = query_all_statuses();
                        }
                        ui.label(
                            egui::RichText::new(t.assoc_legend)
                                .small()
                                .color(Color32::GRAY),
                        );
                    });
                });

                // 3. Per-extension scrollable grid
                egui::Frame::none()
                    .fill(Color32::from_rgb(24, 26, 32))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 49, 58)))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(250.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for cat in ExtCategory::ALL {
                                    let exts: Vec<_> = ALL_SUPPORTED_EXTS
                                        .iter()
                                        .filter(|e| e.category == cat)
                                        .collect();
                                    if exts.is_empty() {
                                        continue;
                                    }

                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(cat.label(lang))
                                            .strong()
                                            .color(Color32::from_rgb(125, 178, 255)),
                                    );

                                    let cols = 5usize;
                                    for chunk in exts.chunks(cols) {
                                        ui.columns(cols, |col_uis| {
                                            for (i, info) in chunk.iter().enumerate() {
                                                let col_ui = &mut col_uis[i];
                                                let mut checked =
                                                    self.assoc_modal.selected_exts.contains(info.ext);
                                                let status = self.assoc_modal.statuses.get(info.ext);
                                                let badge = match status {
                                                    Some(s) if s.is_associated_with_us => "✔",
                                                    Some(s) if s.has_other_association => "🔒",
                                                    _ => "⚪",
                                                };
                                                let label = format!(".{} {badge}", info.ext);
                                                let desc = info.localized_description(lang);
                                                let tip = match status {
                                                    Some(s) => format!(
                                                        "{}\n{}: {}",
                                                        desc,
                                                        t.assoc_current_status,
                                                        lang.translate_assoc_status(&s.current_handler_label)
                                                    ),
                                                    None => desc,
                                                };
                                                if col_ui
                                                    .checkbox(&mut checked, label)
                                                    .on_hover_text(tip)
                                                    .changed()
                                                {
                                                    self.assoc_modal.active_preset = None;
                                                    if checked {
                                                        self.assoc_modal
                                                            .selected_exts
                                                            .insert(info.ext.to_string());
                                                    } else {
                                                        self.assoc_modal
                                                            .selected_exts
                                                            .remove(info.ext);
                                                    }
                                                }
                                            }
                                        });
                                    }
                                    ui.add_space(4.0);
                                }
                            });
                    });

                if let Some(locked) = self.assoc_modal.last_locked_notice {
                    if locked > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} {locked} {}",
                                t.assoc_locked_note_a, t.assoc_locked_note_b
                            ))
                            .small()
                            .color(Color32::from_rgb(251, 191, 36)),
                        );
                    }
                }

                // 4. Footer action buttons
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let primary_label = if self.assoc_modal.installer_mode {
                        t.assoc_btn_install_apply
                    } else {
                        t.assoc_btn_apply
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(primary_label).strong().color(Color32::WHITE),
                            )
                            .min_size(Vec2::new(0.0, 34.0))
                            .fill(Color32::from_rgb(28, 125, 76)),
                        )
                        .clicked()
                    {
                        do_apply = true;
                    }

                    if ui
                        .add(
                            egui::Button::new(t.assoc_btn_default_apps)
                                .min_size(Vec2::new(0.0, 34.0)),
                        )
                        .on_hover_text(t.assoc_tip_default_apps)
                        .clicked()
                    {
                        if let Err(e) = open_url("ms-settings:defaultapps") {
                            toast_msg = Some((e, true));
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(t.btn_close).min_size(Vec2::new(90.0, 34.0)))
                            .clicked()
                        {
                            close_modal = true;
                        }
                    });
                });
            });

        if do_apply {
            if self.assoc_modal.installer_mode {
                let target_dir = PathBuf::from(self.assoc_modal.install_dir_text.trim());
                match install_application(
                    &target_dir,
                    self.assoc_modal.create_desktop_shortcut,
                    self.assoc_modal.create_start_menu_shortcut,
                    &self.assoc_modal.selected_exts,
                ) {
                    Ok((exe, res)) => {
                        self.assoc_modal.statuses = query_all_statuses();
                        self.assoc_modal.last_locked_notice =
                            if res.user_choice_locked_count > 0 {
                                Some(res.user_choice_locked_count)
                            } else {
                                None
                            };
                        toast_msg = Some((
                            format!(
                                "{} {} ({} {})",
                                t.toast_installed_in,
                                exe.display(),
                                res.associated_count,
                                t.toast_formats_applied
                            ),
                            false,
                        ));
                    }
                    Err(e) => toast_msg = Some((e, true)),
                }
            } else {
                match std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                    apply_file_associations(&exe, &self.assoc_modal.selected_exts)
                }) {
                    Ok(res) => {
                        self.assoc_modal.statuses = query_all_statuses();
                        self.assoc_modal.last_locked_notice =
                            if res.user_choice_locked_count > 0 {
                                Some(res.user_choice_locked_count)
                            } else {
                                None
                            };
                        toast_msg = Some((
                            format!(
                                "{} ({} {})",
                                t.toast_assoc_updated,
                                res.associated_count,
                                t.toast_formats_word
                            ),
                            false,
                        ));
                    }
                    Err(e) => toast_msg = Some((e, true)),
                }
            }
        }

        if let Some((msg, is_err)) = toast_msg {
            self.set_toast(msg, is_err);
        }
        if close_modal {
            self.assoc_modal.open = false;
        }
    }
}

impl eframe::App for ImageViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pinned = self.current_item().map(|i| i.path.clone());
        self.cache.poll_results(ctx, pinned.as_deref());

        self.handle_dropped_files(ctx);
        self.handle_global_shortcuts(ctx);
        self.update_window_title(ctx);

        self.render_top_bar(ctx);
        self.render_bottom_bar(ctx);
        self.render_info_panel(ctx);

        match self.view_mode {
            ViewMode::Single => self.render_single_view(ctx),
            ViewMode::Gallery => self.render_gallery_view(ctx),
        }

        self.render_modals_and_toasts(ctx);
    }
}

fn draw_checkerboard(painter: &egui::Painter, rect: Rect) {
    let cell = 14.0f32;
    let c1 = Color32::from_rgb(56, 58, 64);
    let c2 = Color32::from_rgb(38, 40, 46);
    painter.rect_filled(rect, 0.0, c1);

    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    // Cap checkerboard cells if deeply zoomed in to keep rendering O(1)
    if cols * rows > 12_000 {
        return;
    }

    for r in 0..rows {
        for c in 0..cols {
            if (r + c) % 2 == 1 {
                let min = Pos2::new(rect.min.x + c as f32 * cell, rect.min.y + r as f32 * cell);
                let max = Pos2::new(
                    (min.x + cell).min(rect.max.x),
                    (min.y + cell).min(rect.max.y),
                );
                painter.rect_filled(Rect::from_min_max(min, max), 0.0, c2);
            }
        }
    }
}

fn info_section_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .small()
            .strong()
            .color(Color32::from_rgb(150, 165, 188)),
    );
}

fn build_file_metadata_section(
    item: &MediaItem,
    media: Option<&DecodedMedia>,
    lang: Language,
) -> Vec<(String, Vec<(String, String)>)> {
    let t = lang.tr();
    let mut file_rows = vec![
        (t.meta_name.to_string(), item.file_name.clone()),
        (
            t.meta_folder.to_string(),
            item.path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ),
        (
            t.meta_disk_size.to_string(),
            format!("{} ({} bytes)", format_file_size(item.file_size), item.file_size),
        ),
    ];
    if let Some(created) = format_system_time(item.created) {
        file_rows.push((t.meta_created.to_string(), created));
    }
    if let Some(modified) = format_system_time(item.modified) {
        file_rows.push((t.meta_last_modified.to_string(), modified));
    }

    if let Some(m) = media {
        file_rows.push((
            t.meta_detected_format.to_string(),
            lang.translate_format_label(&m.format_label),
        ));
        file_rows.push((
            t.meta_dimensions.to_string(),
            format!("{} × {} px", m.original_dimensions.0, m.original_dimensions.1),
        ));
        file_rows.push((t.meta_megapixels.to_string(), format!("{:.2} MP", m.megapixels)));
        file_rows.push((t.meta_aspect_ratio.to_string(), m.aspect_ratio_label.clone()));
        file_rows.push((
            t.meta_color_depth.to_string(),
            lang.translate_color_depth(&m.color_depth_label),
        ));
        if m.frames.len() > 1 {
            file_rows.push((t.meta_anim_frames.to_string(), m.frames.len().to_string()));
        }
        if let Some(dur) = m.video_duration_secs {
            file_rows.push((t.meta_video_duration.to_string(), format_duration(dur)));
        }
        if let Some(gps) = &m.exif.gps_display {
            file_rows.push((t.meta_gps_coords.to_string(), gps.clone()));
        }
    }

    vec![(t.meta_sec_file_props.to_string(), file_rows)]
}

fn build_metadata_report(item: &MediaItem, media: Option<&DecodedMedia>, lang: Language) -> String {
    let mut lines = Vec::new();
    let mut sections = build_file_metadata_section(item, media, lang);
    if let Some(m) = media {
        for (cat, rows) in &m.exif.categories {
            let loc_cat = lang.translate_meta_category(cat);
            let loc_rows = rows
                .iter()
                .map(|(k, v)| (lang.translate_meta_key(k), v.clone()))
                .collect();
            sections.push((loc_cat, loc_rows));
        }
    }
    for (cat, rows) in sections {
        lines.push(format!("[{cat}]"));
        for (k, v) in rows {
            lines.push(format!("{k}: {v}"));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

fn truncate_middle(s: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(8);
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    let keep = (max_chars - 1) / 2;
    let start: String = chars[..keep].iter().collect();
    let end: String = chars[chars.len() - keep..].iter().collect();
    format!("{start}…{end}")
}
