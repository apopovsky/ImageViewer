#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod associations;
mod cache;
mod editor;
mod i18n;
mod icon;
mod media;
mod windows_shell;

use std::path::PathBuf;
use std::sync::Arc;

use app::ImageViewerApp;
use eframe::egui::{self, Color32, FontId, TextStyle};

fn normalize_cli_path(arg: std::ffi::OsString) -> PathBuf {
    let p = PathBuf::from(arg);
    if p.is_absolute() {
        p
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        p
    }
}

fn configure_fonts_and_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Load native Windows UI & symbol/emoji fallback fonts if available
    for (name, path) in [
        ("segoe_ui", r"C:\Windows\Fonts\segoeui.ttf"),
        ("segoe_sym", r"C:\Windows\Fonts\seguisym.ttf"),
        ("segoe_emoji", r"C:\Windows\Fonts\seguiemj.ttf"),
    ] {
        if let Ok(data) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(data).into());
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                if name == "segoe_ui" {
                    family.insert(0, name.to_owned());
                } else {
                    family.push(name.to_owned());
                }
            }
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.push(name.to_owned());
            }
        }
    }

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
    style.text_styles.insert(TextStyle::Button, FontId::proportional(14.5));
    style.text_styles.insert(TextStyle::Heading, FontId::proportional(19.0));
    style.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));

    style.spacing.button_padding = egui::Vec2::new(10.0, 6.0);
    style.spacing.item_spacing = egui::Vec2::new(8.0, 6.0);

    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(28, 30, 36);
    style.visuals.window_fill = Color32::from_rgb(34, 37, 45);
    style.visuals.extreme_bg_color = Color32::from_rgb(18, 20, 25);
    style.visuals.window_rounding = egui::Rounding::same(10.0);
    style.visuals.window_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(60, 65, 78));
    style.visuals.selection.bg_fill = Color32::from_rgb(37, 99, 235);
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 30, 36);
    style.visuals.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(218, 224, 234));
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(43, 47, 58);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(43, 47, 58);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(56, 61, 74));
    style.visuals.widgets.inactive.fg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(235, 240, 248));
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(7.0);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(56, 62, 76);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(56, 62, 76);
    style.visuals.widgets.hovered.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(96, 130, 196));
    style.visuals.widgets.hovered.rounding = egui::Rounding::same(7.0);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(66, 74, 92);
    style.visuals.widgets.active.rounding = egui::Rounding::same(7.0);
    ctx.set_style(style);
}

fn show_windows_info_box(title: &str, text: &str) {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        extern "system" {
            fn MessageBoxW(hwnd: usize, lpText: *const u16, lpCaption: *const u16, uType: u32) -> i32;
        }
        let wide = |s: &str| -> Vec<u16> {
            OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
        };
        let t = wide(text);
        let c = wide(title);
        unsafe {
            MessageBoxW(0, t.as_ptr(), c.as_ptr(), 0x0000_0040);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title, text);
    }
}

fn main() -> eframe::Result<()> {
    let mut initial_target: Option<PathBuf> = None;
    let mut start_installer = false;
    let mut start_assoc = false;
    let mut do_uninstall = false;
    let mut cli_lang: Option<i18n::Language> = None;

    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let s = arg.to_string_lossy();
        if s.eq_ignore_ascii_case("--setup") || s.eq_ignore_ascii_case("--install") {
            start_installer = true;
        } else if s.eq_ignore_ascii_case("--associate") {
            start_assoc = true;
        } else if s.eq_ignore_ascii_case("--uninstall") {
            do_uninstall = true;
        } else if s.eq_ignore_ascii_case("--lang") {
            if let Some(next_val) = args.next() {
                if let Some(parsed) = i18n::Language::from_code(&next_val.to_string_lossy()) {
                    cli_lang = Some(parsed);
                }
            }
        } else if let Some(rest) = s.strip_prefix("--lang=").or_else(|| s.strip_prefix("--LANG=")) {
            if let Some(parsed) = i18n::Language::from_code(rest) {
                cli_lang = Some(parsed);
            }
        } else if initial_target.is_none() {
            initial_target = Some(normalize_cli_path(arg));
        }
    }

    let initial_lang = cli_lang.unwrap_or_else(i18n::load_saved_language);

    if do_uninstall {
        let t = initial_lang.tr();
        match associations::uninstall_application() {
            Ok(()) => show_windows_info_box("ImageViewer", t.uninstall_success_msg),
            Err(e) => show_windows_info_box(t.uninstall_error_title, &e),
        }
        return Ok(());
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ImageViewer")
            .with_icon(Arc::new(icon::build_egui_icon()))
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([680.0, 500.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "ImageViewer",
        native_options,
        Box::new(move |cc| {
            configure_fonts_and_style(&cc.egui_ctx);
            let mut app = ImageViewerApp::new(initial_target, initial_lang);
            if start_installer {
                app.open_associations_modal(true);
            } else if start_assoc {
                app.open_associations_modal(false);
            }
            Ok(Box::new(app))
        }),
    )
}
