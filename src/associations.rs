use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
use winreg::RegKey;

use crate::i18n::Language;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtCategory {
    StandardImage,
    AnimatedOrVector,
    RawOrExtended,
    Video,
}

impl ExtCategory {
    pub const ALL: [Self; 4] = [
        Self::StandardImage,
        Self::AnimatedOrVector,
        Self::RawOrExtended,
        Self::Video,
    ];

    pub fn label(self, lang: Language) -> &'static str {
        let t = lang.tr();
        match self {
            Self::StandardImage => t.ext_cat_standard,
            Self::AnimatedOrVector => t.ext_cat_anim_vector,
            Self::RawOrExtended => t.ext_cat_raw_ext,
            Self::Video => t.ext_cat_video,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SupportedExt {
    pub ext: &'static str,
    pub category: ExtCategory,
    pub description: &'static str,
}

impl SupportedExt {
    pub const fn is_image(self) -> bool {
        !matches!(self.category, ExtCategory::Video)
    }

    pub const fn is_video(self) -> bool {
        matches!(self.category, ExtCategory::Video)
    }

    pub fn localized_description(self, lang: Language) -> String {
        if lang == Language::English {
            return self.description.to_string();
        }
        let upper = self.ext.to_ascii_uppercase();
        match (lang, self.category) {
            (Language::Spanish, ExtCategory::Video) => format!("Video {upper}"),
            (Language::Spanish, ExtCategory::AnimatedOrVector) if self.ext == "gif" => {
                "Imagen GIF animada".to_string()
            }
            (Language::Spanish, ExtCategory::AnimatedOrVector) => {
                format!("Gráfico vectorial {upper}")
            }
            (Language::Spanish, _) => format!("Imagen {upper}"),

            (Language::Portuguese, ExtCategory::Video) => format!("Vídeo {upper}"),
            (Language::Portuguese, ExtCategory::AnimatedOrVector) if self.ext == "gif" => {
                "Imagem GIF animada".to_string()
            }
            (Language::Portuguese, ExtCategory::AnimatedOrVector) => {
                format!("Gráfico vetorial {upper}")
            }
            (Language::Portuguese, _) => format!("Imagem {upper}"),

            (Language::French, ExtCategory::Video) => format!("Vidéo {upper}"),
            (Language::French, ExtCategory::AnimatedOrVector) if self.ext == "gif" => {
                "Image GIF animée".to_string()
            }
            (Language::French, ExtCategory::AnimatedOrVector) => {
                format!("Graphique vectoriel {upper}")
            }
            (Language::French, _) => format!("Image {upper}"),

            (Language::German, ExtCategory::Video) => format!("{upper}-Video"),
            (Language::German, ExtCategory::AnimatedOrVector) if self.ext == "gif" => {
                "Animiertes GIF-Bild".to_string()
            }
            (Language::German, ExtCategory::AnimatedOrVector) => {
                format!("{upper}-Vektorgrafik")
            }
            (Language::German, _) => format!("{upper}-Bild"),

            (Language::Italian, ExtCategory::Video) => format!("Video {upper}"),
            (Language::Italian, ExtCategory::AnimatedOrVector) if self.ext == "gif" => {
                "Immagine GIF animata".to_string()
            }
            (Language::Italian, ExtCategory::AnimatedOrVector) => {
                format!("Grafica vettoriale {upper}")
            }
            (Language::Italian, _) => format!("Immagine {upper}"),

            _ => self.description.to_string(),
        }
    }
}

pub const ALL_SUPPORTED_EXTS: &[SupportedExt] = &[
    // Standard images
    SupportedExt { ext: "jpg", category: ExtCategory::StandardImage, description: "JPEG Image" },
    SupportedExt { ext: "jpeg", category: ExtCategory::StandardImage, description: "JPEG Image" },
    SupportedExt { ext: "jfif", category: ExtCategory::StandardImage, description: "JFIF Image" },
    SupportedExt { ext: "png", category: ExtCategory::StandardImage, description: "PNG Image" },
    SupportedExt { ext: "webp", category: ExtCategory::StandardImage, description: "WebP Image" },
    SupportedExt { ext: "bmp", category: ExtCategory::StandardImage, description: "BMP Bitmap" },
    SupportedExt { ext: "ico", category: ExtCategory::StandardImage, description: "ICO Icon" },
    SupportedExt { ext: "cur", category: ExtCategory::StandardImage, description: "CUR Cursor" },
    SupportedExt { ext: "tiff", category: ExtCategory::StandardImage, description: "TIFF Image" },
    SupportedExt { ext: "tif", category: ExtCategory::StandardImage, description: "TIFF Image" },
    SupportedExt { ext: "tga", category: ExtCategory::StandardImage, description: "TGA Image" },
    SupportedExt { ext: "dds", category: ExtCategory::StandardImage, description: "DDS Texture" },
    SupportedExt { ext: "hdr", category: ExtCategory::StandardImage, description: "HDR Image" },
    SupportedExt { ext: "exr", category: ExtCategory::StandardImage, description: "OpenEXR Image" },
    SupportedExt { ext: "qoi", category: ExtCategory::StandardImage, description: "QOI Image" },
    SupportedExt { ext: "pnm", category: ExtCategory::StandardImage, description: "PNM Image" },
    SupportedExt { ext: "pbm", category: ExtCategory::StandardImage, description: "PBM Image" },
    SupportedExt { ext: "pgm", category: ExtCategory::StandardImage, description: "PGM Image" },
    SupportedExt { ext: "ppm", category: ExtCategory::StandardImage, description: "PPM Image" },
    SupportedExt { ext: "ff", category: ExtCategory::StandardImage, description: "Farbfeld Image" },
    // Animated & Vector
    SupportedExt { ext: "gif", category: ExtCategory::AnimatedOrVector, description: "Animated GIF Image" },
    SupportedExt { ext: "svg", category: ExtCategory::AnimatedOrVector, description: "SVG Vector Graphic" },
    SupportedExt { ext: "svgz", category: ExtCategory::AnimatedOrVector, description: "SVGZ Vector Graphic" },
    // RAW & Extended
    SupportedExt { ext: "heic", category: ExtCategory::RawOrExtended, description: "HEIC Image" },
    SupportedExt { ext: "heif", category: ExtCategory::RawOrExtended, description: "HEIF Image" },
    SupportedExt { ext: "avif", category: ExtCategory::RawOrExtended, description: "AVIF Image" },
    SupportedExt { ext: "jxl", category: ExtCategory::RawOrExtended, description: "JPEG XL Image" },
    SupportedExt { ext: "psd", category: ExtCategory::RawOrExtended, description: "Photoshop PSD Document" },
    SupportedExt { ext: "jp2", category: ExtCategory::RawOrExtended, description: "JPEG 2000 Image" },
    SupportedExt { ext: "cr2", category: ExtCategory::RawOrExtended, description: "Canon RAW CR2" },
    SupportedExt { ext: "cr3", category: ExtCategory::RawOrExtended, description: "Canon RAW CR3" },
    SupportedExt { ext: "nef", category: ExtCategory::RawOrExtended, description: "Nikon RAW NEF" },
    SupportedExt { ext: "arw", category: ExtCategory::RawOrExtended, description: "Sony RAW ARW" },
    SupportedExt { ext: "dng", category: ExtCategory::RawOrExtended, description: "Digital Negative DNG" },
    SupportedExt { ext: "orf", category: ExtCategory::RawOrExtended, description: "Olympus RAW ORF" },
    SupportedExt { ext: "rw2", category: ExtCategory::RawOrExtended, description: "Panasonic RAW RW2" },
    SupportedExt { ext: "pef", category: ExtCategory::RawOrExtended, description: "Pentax RAW PEF" },
    SupportedExt { ext: "raf", category: ExtCategory::RawOrExtended, description: "Fujifilm RAW RAF" },
    SupportedExt { ext: "srw", category: ExtCategory::RawOrExtended, description: "Samsung RAW SRW" },
    // Videos
    SupportedExt { ext: "mp4", category: ExtCategory::Video, description: "MP4 Video" },
    SupportedExt { ext: "mkv", category: ExtCategory::Video, description: "Matroska MKV Video" },
    SupportedExt { ext: "webm", category: ExtCategory::Video, description: "WebM Video" },
    SupportedExt { ext: "mov", category: ExtCategory::Video, description: "QuickTime MOV Video" },
    SupportedExt { ext: "avi", category: ExtCategory::Video, description: "AVI Video" },
    SupportedExt { ext: "wmv", category: ExtCategory::Video, description: "WMV Video" },
    SupportedExt { ext: "flv", category: ExtCategory::Video, description: "Flash FLV Video" },
    SupportedExt { ext: "m4v", category: ExtCategory::Video, description: "M4V Video" },
    SupportedExt { ext: "mpg", category: ExtCategory::Video, description: "MPEG Video" },
    SupportedExt { ext: "mpeg", category: ExtCategory::Video, description: "MPEG Video" },
    SupportedExt { ext: "3gp", category: ExtCategory::Video, description: "3GP Video" },
    SupportedExt { ext: "ts", category: ExtCategory::Video, description: "MPEG-TS Video" },
];

#[derive(Debug, Clone)]
pub struct ExtAssociationStatus {
    pub is_associated_with_us: bool,
    pub has_other_association: bool,
    pub has_user_choice_lock: bool,
    pub current_handler_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationPreset {
    All,
    None,
    UnassociatedOnly,
    ImagesOnly,
    VideosOnly,
}

impl AssociationPreset {
    pub fn label(self, lang: Language) -> &'static str {
        let t = lang.tr();
        match self {
            Self::All => t.preset_all,
            Self::None => t.preset_none,
            Self::UnassociatedOnly => t.preset_unassociated,
            Self::ImagesOnly => t.preset_images,
            Self::VideosOnly => t.preset_videos,
        }
    }
}

pub fn prog_id_for(ext: &str) -> String {
    format!("ImageViewer.AssocFile.{ext}")
}

pub fn query_all_statuses() -> HashMap<String, ExtAssociationStatus> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let mut map = HashMap::with_capacity(ALL_SUPPORTED_EXTS.len());

    for info in ALL_SUPPORTED_EXTS {
        let dot_ext = format!(".{}", info.ext);
        let user_choice_key = format!(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{dot_ext}\UserChoice"
        );
        let user_choice_progid = hkcu
            .open_subkey(&user_choice_key)
            .ok()
            .and_then(|k| k.get_value::<String, _>("ProgId").ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let class_progid = hkcu
            .open_subkey(format!(r"Software\Classes\{dot_ext}"))
            .ok()
            .and_then(|k| k.get_value::<String, _>("").ok())
            .or_else(|| {
                hkcr.open_subkey(&dot_ext)
                    .ok()
                    .and_then(|k| k.get_value::<String, _>("").ok())
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let active_progid = user_choice_progid.clone().or(class_progid);

        let status = match active_progid {
            Some(pid) if pid.starts_with("ImageViewer.AssocFile.") => ExtAssociationStatus {
                is_associated_with_us: true,
                has_other_association: false,
                has_user_choice_lock: false,
                current_handler_label: "ImageViewer".to_string(),
            },
            Some(pid) => {
                let handler_exists = hkcr.open_subkey(&pid).is_ok() || pid.starts_with("AppX");
                if handler_exists {
                    let friendly = resolve_progid_label(&hkcr, &pid);
                    ExtAssociationStatus {
                        is_associated_with_us: false,
                        has_other_association: true,
                        has_user_choice_lock: user_choice_progid.is_some(),
                        current_handler_label: friendly,
                    }
                } else {
                    ExtAssociationStatus {
                        is_associated_with_us: false,
                        has_other_association: false,
                        has_user_choice_lock: false,
                        current_handler_label: "Unassociated".to_string(),
                    }
                }
            }
            None => ExtAssociationStatus {
                is_associated_with_us: false,
                has_other_association: false,
                has_user_choice_lock: false,
                current_handler_label: "Unassociated".to_string(),
            },
        };

        map.insert(info.ext.to_string(), status);
    }

    map
}

fn resolve_progid_label(hkcr: &RegKey, progid: &str) -> String {
    if progid.starts_with("AppX") {
        if let Ok(k) = hkcr.open_subkey(progid) {
            if let Ok(app) = k.open_subkey("Application") {
                if let Ok(name) = app.get_value::<String, _>("ApplicationName") {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('@') && !trimmed.starts_with("ms-resource:") {
                        return trimmed.to_string();
                    }
                }
            }
        }
        return "Windows App".to_string();
    }

    if let Ok(k) = hkcr.open_subkey(progid) {
        if let Ok(val) = k.get_value::<String, _>("") {
            let trimmed = val.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('@') {
                return trimmed.to_string();
            }
        }
    }
    progid.to_string()
}

pub fn apply_preset(
    preset: AssociationPreset,
    statuses: &HashMap<String, ExtAssociationStatus>,
) -> HashSet<String> {
    let mut selected = HashSet::new();
    match preset {
        AssociationPreset::All => {
            for info in ALL_SUPPORTED_EXTS {
                selected.insert(info.ext.to_string());
            }
        }
        AssociationPreset::None => {}
        AssociationPreset::UnassociatedOnly => {
            for info in ALL_SUPPORTED_EXTS {
                let has_other = statuses
                    .get(info.ext)
                    .map(|s| s.has_other_association)
                    .unwrap_or(false);
                if !has_other {
                    selected.insert(info.ext.to_string());
                }
            }
        }
        AssociationPreset::ImagesOnly => {
            for info in ALL_SUPPORTED_EXTS {
                if info.is_image() {
                    selected.insert(info.ext.to_string());
                }
            }
        }
        AssociationPreset::VideosOnly => {
            for info in ALL_SUPPORTED_EXTS {
                if info.is_video() {
                    selected.insert(info.ext.to_string());
                }
            }
        }
    }
    selected
}

#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub associated_count: usize,
    pub user_choice_locked_count: usize,
}

pub fn apply_file_associations(
    exe_path: &Path,
    selected_exts: &HashSet<String>,
) -> Result<ApplyResult, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let exe_str = exe_path.to_string_lossy().into_owned();
    let icon_val = format!("{exe_str},0");
    let open_cmd = format!("\"{exe_str}\" \"%1\"");

    // 1. Register under HKCU\Software\Classes\Applications\ImageViewer.exe
    let (app_key, _) = hkcu
        .create_subkey(r"Software\Classes\Applications\ImageViewer.exe")
        .map_err(|e| format!("Error writing Applications registry key: {e}"))?;
    let _ = app_key.set_value("FriendlyAppName", &"ImageViewer");
    if let Ok((ico_k, _)) = app_key.create_subkey("DefaultIcon") {
        let _ = ico_k.set_value("", &icon_val);
    }
    if let Ok((cmd_k, _)) = app_key.create_subkey(r"shell\open\command") {
        let _ = cmd_k.set_value("", &open_cmd);
    }
    if let Ok((supp_k, _)) = app_key.create_subkey("SupportedTypes") {
        for info in ALL_SUPPORTED_EXTS {
            let _ = supp_k.set_value(format!(".{}", info.ext), &"");
        }
    }

    // 2. Register Default Programs Capabilities
    let (cap_key, _) = hkcu
        .create_subkey(r"Software\ImageViewer\Capabilities")
        .map_err(|e| format!("Error creating Capabilities key: {e}"))?;
    let _ = cap_key.set_value("ApplicationName", &"ImageViewer");
    let _ = cap_key.set_value(
        "ApplicationDescription",
        &"Fast image and video viewer and editor",
    );
    let (assoc_cap_key, _) = cap_key
        .create_subkey("FileAssociations")
        .map_err(|e| format!("Error creating FileAssociations key: {e}"))?;

    if let Ok((reg_apps, _)) = hkcu.create_subkey(r"Software\RegisteredApplications") {
        let _ = reg_apps.set_value("ImageViewer", &r"Software\ImageViewer\Capabilities");
    }

    // 3. Apply or remove per-extension ProgIDs & associations
    let statuses = query_all_statuses();
    let mut associated_count = 0usize;
    let mut user_choice_locked_count = 0usize;

    for info in ALL_SUPPORTED_EXTS {
        let dot_ext = format!(".{}", info.ext);
        let prog_id = prog_id_for(info.ext);

        if selected_exts.contains(info.ext) {
            associated_count += 1;
            if statuses
                .get(info.ext)
                .map(|s| s.has_user_choice_lock)
                .unwrap_or(false)
            {
                user_choice_locked_count += 1;
            }

            // Create ProgID
            if let Ok((pid_k, _)) = hkcu.create_subkey(format!(r"Software\Classes\{prog_id}")) {
                let _ = pid_k.set_value("", &info.description);
                if let Ok((ik, _)) = pid_k.create_subkey("DefaultIcon") {
                    let _ = ik.set_value("", &icon_val);
                }
                if let Ok((ck, _)) = pid_k.create_subkey(r"shell\open\command") {
                    let _ = ck.set_value("", &open_cmd);
                }
            }

            // Set HKCU\Software\Classes\.ext default & OpenWithProgids
            if let Ok((ext_k, _)) = hkcu.create_subkey(format!(r"Software\Classes\{dot_ext}")) {
                let _ = ext_k.set_value("", &prog_id);
                if let Ok((owp, _)) = ext_k.create_subkey("OpenWithProgids") {
                    let _ = owp.set_value(&prog_id, &"");
                }
            }

            // Add to Explorer FileExts OpenWithProgids
            if let Ok((fe_owp, _)) = hkcu.create_subkey(format!(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{dot_ext}\OpenWithProgids"
            )) {
                let _ = fe_owp.set_value(&prog_id, &"");
            }

            let _ = assoc_cap_key.set_value(&dot_ext, &prog_id);
        } else {
            // Remove our association if unselected
            if let Ok(ext_k) = hkcu.open_subkey_with_flags(
                format!(r"Software\Classes\{dot_ext}"),
                winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
            ) {
                if let Ok(cur) = ext_k.get_value::<String, _>("") {
                    if cur == prog_id {
                        let _ = ext_k.delete_value("");
                    }
                }
                if let Ok(owp) = ext_k.open_subkey_with_flags(
                    "OpenWithProgids",
                    winreg::enums::KEY_WRITE,
                ) {
                    let _ = owp.delete_value(&prog_id);
                }
            }
            let _ = assoc_cap_key.delete_value(&dot_ext);
            let _ = hkcu.delete_subkey_all(format!(r"Software\Classes\{prog_id}"));
        }
    }

    notify_shell_associations_changed();

    Ok(ApplyResult {
        associated_count,
        user_choice_locked_count,
    })
}

pub fn default_install_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local).join("Programs").join("ImageViewer")
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        PathBuf::from(profile)
            .join("AppData")
            .join("Local")
            .join("Programs")
            .join("ImageViewer")
    } else {
        PathBuf::from(r"C:\Programs\ImageViewer")
    }
}

pub fn is_running_from_install_dir() -> bool {
    let Ok(cur) = std::env::current_exe() else {
        return false;
    };
    let expected = default_install_dir().join("ImageViewer.exe");
    cur.to_string_lossy()
        .eq_ignore_ascii_case(&expected.to_string_lossy())
}

pub fn install_application(
    install_dir: &Path,
    create_desktop_shortcut: bool,
    create_start_menu_shortcut: bool,
    selected_exts: &HashSet<String>,
) -> Result<(PathBuf, ApplyResult), String> {
    fs::create_dir_all(install_dir)
        .map_err(|e| format!("Could not create installation folder: {e}"))?;

    let current_exe =
        std::env::current_exe().map_err(|e| format!("Could not locate executable: {e}"))?;
    let target_exe = install_dir.join("ImageViewer.exe");

    let same_path = current_exe
        .to_string_lossy()
        .eq_ignore_ascii_case(&target_exe.to_string_lossy());
    if !same_path {
        fs::copy(&current_exe, &target_exe)
            .map_err(|e| format!("Could not copy executable to {}: {e}", target_exe.display()))?;
    }

    if create_start_menu_shortcut {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let programs_dir = PathBuf::from(appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            let _ = fs::create_dir_all(&programs_dir);
            let lnk = programs_dir.join("ImageViewer.lnk");
            create_windows_shortcut(&lnk, &target_exe, "")?;
        }
    }

    if create_desktop_shortcut {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            let desktop = PathBuf::from(profile).join("Desktop");
            if desktop.exists() {
                let lnk = desktop.join("ImageViewer.lnk");
                create_windows_shortcut(&lnk, &target_exe, "")?;
            }
        }
    }

    // Register Windows Uninstall entry (user-level, no UAC needed)
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok((uninst, _)) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ImageViewer") {
        let exe_str = target_exe.to_string_lossy().into_owned();
        let _ = uninst.set_value("DisplayName", &"ImageViewer");
        let _ = uninst.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"));
        let _ = uninst.set_value("Publisher", &"ImageViewer");
        let _ = uninst.set_value("DisplayIcon", &exe_str);
        let _ = uninst.set_value("InstallLocation", &install_dir.to_string_lossy().into_owned());
        let _ = uninst.set_value("UninstallString", &format!("\"{exe_str}\" --uninstall"));
        let _ = uninst.set_value("NoModify", &1u32);
        let _ = uninst.set_value("NoRepair", &1u32);
    }

    let apply_res = apply_file_associations(&target_exe, selected_exts)?;
    Ok((target_exe, apply_res))
}

pub fn uninstall_application() -> Result<(), String> {
    let exe_fallback = default_install_dir().join("ImageViewer.exe");
    let _ = apply_file_associations(&exe_fallback, &HashSet::new());

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let _ = hkcu.delete_subkey_all(r"Software\ImageViewer");
    let _ = hkcu.delete_subkey_all(r"Software\Classes\Applications\ImageViewer.exe");
    let _ = hkcu.delete_subkey_all(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ImageViewer");
    if let Ok(reg_apps) = hkcu.open_subkey_with_flags(
        r"Software\RegisteredApplications",
        winreg::enums::KEY_WRITE,
    ) {
        let _ = reg_apps.delete_value("ImageViewer");
    }

    if let Ok(appdata) = std::env::var("APPDATA") {
        let lnk = PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("ImageViewer.lnk");
        let _ = fs::remove_file(lnk);
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let lnk = PathBuf::from(profile).join("Desktop").join("ImageViewer.lnk");
        let _ = fs::remove_file(lnk);
    }

    notify_shell_associations_changed();
    Ok(())
}

fn create_windows_shortcut(lnk_path: &Path, target: &Path, args: &str) -> Result<(), String> {
    let esc = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let esc_str = |s: &str| s.replace('\'', "''");
    let work_dir = target.parent().unwrap_or_else(|| Path::new("."));

    let script = format!(
        "$ws = New-Object -ComObject WScript.Shell; \
         $s = $ws.CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.Arguments = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.IconLocation = '{},0'; \
         $s.Save()",
        esc(lnk_path),
        esc(target),
        esc_str(args),
        esc(work_dir),
        esc(target)
    );

    let mut cmd = std::process::Command::new("powershell");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output()
        .map_err(|e| format!("Error creating shortcut: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "Could not create shortcut: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

fn notify_shell_associations_changed() {
    #[cfg(windows)]
    {
        extern "system" {
            fn SHChangeNotify(wEventId: i32, uFlags: u32, dwItem1: *const u8, dwItem2: *const u8);
        }
        const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
        const SHCNF_IDLIST: u32 = 0x0000;
        unsafe {
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, std::ptr::null(), std::ptr::null());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_statuses() -> HashMap<String, ExtAssociationStatus> {
        let mut map = HashMap::new();
        for info in ALL_SUPPORTED_EXTS {
            let has_other = matches!(info.ext, "jpg" | "png" | "mp4");
            map.insert(
                info.ext.to_string(),
                ExtAssociationStatus {
                    is_associated_with_us: false,
                    has_other_association: has_other,
                    has_user_choice_lock: has_other,
                    current_handler_label: if has_other {
                        "Other app".to_string()
                    } else {
                        "Unassociated".to_string()
                    },
                },
            );
        }
        map
    }

    #[test]
    fn supported_exts_cover_images_and_videos() {
        assert!(ALL_SUPPORTED_EXTS.iter().any(|e| e.ext == "jpg" && e.is_image()));
        assert!(ALL_SUPPORTED_EXTS.iter().any(|e| e.ext == "svg" && e.is_image()));
        assert!(ALL_SUPPORTED_EXTS.iter().any(|e| e.ext == "cr3" && e.is_image()));
        assert!(ALL_SUPPORTED_EXTS.iter().any(|e| e.ext == "mp4" && e.is_video()));
    }

    #[test]
    fn presets_select_expected_extensions() {
        let statuses = mock_statuses();

        let all = apply_preset(AssociationPreset::All, &statuses);
        assert_eq!(all.len(), ALL_SUPPORTED_EXTS.len());

        let none = apply_preset(AssociationPreset::None, &statuses);
        assert!(none.is_empty());

        let unassoc = apply_preset(AssociationPreset::UnassociatedOnly, &statuses);
        assert!(!unassoc.contains("jpg"));
        assert!(!unassoc.contains("png"));
        assert!(!unassoc.contains("mp4"));
        assert!(unassoc.contains("webp"));
        assert!(unassoc.contains("mkv"));

        let imgs = apply_preset(AssociationPreset::ImagesOnly, &statuses);
        assert!(imgs.contains("jpg"));
        assert!(imgs.contains("svg"));
        assert!(imgs.contains("heic"));
        assert!(!imgs.contains("mp4"));
        assert!(!imgs.contains("mkv"));

        let vids = apply_preset(AssociationPreset::VideosOnly, &statuses);
        assert!(vids.contains("mp4"));
        assert!(vids.contains("mkv"));
        assert!(!vids.contains("jpg"));
        assert!(!vids.contains("png"));
    }
}
