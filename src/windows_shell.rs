use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use image::DynamicImage;
use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};
use winreg::RegKey;

#[derive(Debug, Clone)]
pub struct RegisteredApp {
    pub name: String,
    pub command_template: String,
}

pub fn find_registered_editors(file_path: &Path) -> Vec<RegisteredApp> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if ext.is_empty() {
        return Vec::new();
    }

    let mut apps = Vec::new();
    let mut seen_exes: HashSet<String> = HashSet::new();

    let mut push_app = |name: String, cmd: String| {
        if cmd.trim().is_empty() {
            return;
        }
        let exe_key = extract_exe_basename(&cmd).unwrap_or_else(|| name.to_ascii_lowercase());
        // Skip ourselves or rundll32/cmd wrappers without concrete target
        if exe_key == "imageviewer.exe" || exe_key == "rust-photo-viewer.exe" || exe_key == "openwith.exe" {
            return;
        }
        if seen_exes.insert(exe_key) {
            apps.push(RegisteredApp {
                name,
                command_template: cmd,
            });
        }
    };

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    let dot_ext = format!(".{ext}");

    // 1. User's FileExts OpenWithList
    let user_owl = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{dot_ext}\OpenWithList");
    if let Ok(key) = hkcu.open_subkey(&user_owl) {
        for (val_name, val) in key.enum_values().flatten() {
            if val_name.eq_ignore_ascii_case("MRUList") {
                continue;
            }
            let exe = val.to_string();
            if let Some((friendly, cmd)) = resolve_application_exe(&hkcr, &exe) {
                push_app(friendly, cmd);
            }
        }
    }

    // 2. User's OpenWithProgids
    let user_owp = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{dot_ext}\OpenWithProgids");
    if let Ok(key) = hkcu.open_subkey(&user_owp) {
        for (progid, _) in key.enum_values().flatten() {
            if let Some((friendly, cmd)) = resolve_progid(&hkcr, &progid) {
                push_app(friendly, cmd);
            }
        }
    }

    // 3. HKCR\.<ext> default ProgID, OpenWithProgids & OpenWithList
    if let Ok(ext_key) = hkcr.open_subkey(&dot_ext) {
        if let Ok(default_progid) = ext_key.get_value::<String, _>("") {
            if let Some((friendly, cmd)) = resolve_progid(&hkcr, &default_progid) {
                push_app(friendly, cmd);
            }
        }
        if let Ok(owp) = ext_key.open_subkey("OpenWithProgids") {
            for (progid, _) in owp.enum_values().flatten() {
                if let Some((friendly, cmd)) = resolve_progid(&hkcr, &progid) {
                    push_app(friendly, cmd);
                }
            }
        }
        if let Ok(owl) = ext_key.open_subkey("OpenWithList") {
            for sub in owl.enum_keys().flatten() {
                if let Some((friendly, cmd)) = resolve_application_exe(&hkcr, &sub) {
                    push_app(friendly, cmd);
                }
            }
        }
    }

    // 4. HKCR\SystemFileAssociations\<kind>\OpenWithList
    let sys_kind = if crate::media::MediaKind::from_path(file_path)
        .map(|k| k.is_video())
        .unwrap_or(false)
    {
        "video"
    } else {
        "image"
    };
    let sys_owl = format!(r"SystemFileAssociations\{sys_kind}\OpenWithList");
    if let Ok(key) = hkcr.open_subkey(&sys_owl) {
        for sub in key.enum_keys().flatten() {
            if let Some((friendly, cmd)) = resolve_application_exe(&hkcr, &sub) {
                push_app(friendly, cmd);
            }
        }
    }

    // 5. Ensure classic built-in / common editors if present on system
    if sys_kind == "image" {
        let win_dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
        let mspaint = Path::new(&win_dir).join("System32").join("mspaint.exe");
        if mspaint.exists() {
            push_app(
                "Paint".to_string(),
                format!("\"{}\" \"%1\"", mspaint.display()),
            );
        }
        for (label, candidate) in [
            ("Paint.NET", r"C:\Program Files\paint.net\paintdotnet.exe"),
            ("GIMP", r"C:\Program Files\GIMP 2\bin\gimp-2.10.exe"),
            ("IrfanView", r"C:\Program Files\IrfanView\i_view64.exe"),
            ("Krita", r"C:\Program Files\Krita (x64)\bin\krita.exe"),
            ("Inkscape", r"C:\Program Files\Inkscape\bin\inkscape.exe"),
        ] {
            if Path::new(candidate).exists() {
                push_app(label.to_string(), format!("\"{candidate}\" \"%1\""));
            }
        }
    } else {
        for (label, candidate) in [
            ("VLC Media Player", r"C:\Program Files\VideoLAN\VLC\vlc.exe"),
            ("VLC Media Player", r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe"),
            ("MPC-HC", r"C:\Program Files\MPC-HC\mpc-hc64.exe"),
            ("mpv", r"C:\ProgramData\chocolatey\bin\mpv.exe"),
        ] {
            if Path::new(candidate).exists() {
                push_app(label.to_string(), format!("\"{candidate}\" \"%1\""));
            }
        }
    }

    apps
}

fn resolve_application_exe(hkcr: &RegKey, exe: &str) -> Option<(String, String)> {
    let exe = exe.trim();
    if exe.is_empty() {
        return None;
    }
    let app_key_path = format!(r"Applications\{exe}");
    let app_key = hkcr.open_subkey(&app_key_path).ok()?;

    let cmd = app_key
        .open_subkey(r"shell\edit\command")
        .or_else(|_| app_key.open_subkey(r"shell\open\command"))
        .ok()
        .and_then(|k| k.get_value::<String, _>("").ok())?;

    if !command_target_exists(&cmd) {
        return None;
    }

    let friendly = app_key
        .get_value::<String, _>("FriendlyAppName")
        .ok()
        .filter(|s| !s.trim().is_empty() && !s.starts_with('@'))
        .unwrap_or_else(|| pretty_exe_name(exe));

    Some((friendly, cmd))
}

fn resolve_progid(hkcr: &RegKey, progid: &str) -> Option<(String, String)> {
    let progid = progid.trim();
    if progid.is_empty() {
        return None;
    }
    let key = hkcr.open_subkey(progid).ok()?;
    let cmd = key
        .open_subkey(r"shell\edit\command")
        .or_else(|_| key.open_subkey(r"shell\open\command"))
        .ok()
        .and_then(|k| k.get_value::<String, _>("").ok())?;

    if !command_target_exists(&cmd) {
        return None;
    }

    let friendly = key
        .get_value::<String, _>("FriendlyTypeName")
        .ok()
        .or_else(|| key.get_value::<String, _>("").ok())
        .filter(|s| !s.trim().is_empty() && !s.starts_with('@'))
        .unwrap_or_else(|| {
            extract_exe_basename(&cmd)
                .map(|e| pretty_exe_name(&e))
                .unwrap_or_else(|| progid.to_string())
        });

    Some((friendly, cmd))
}

fn pretty_exe_name(exe: &str) -> String {
    let stem = Path::new(exe)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(exe);
    match stem.to_ascii_lowercase().as_str() {
        "mspaint" => "Paint".to_string(),
        "photoshop" => "Adobe Photoshop".to_string(),
        "gimp-2.10" | "gimp-3.0" | "gimp" => "GIMP".to_string(),
        "paintdotnet" => "Paint.NET".to_string(),
        "vlc" => "VLC Media Player".to_string(),
        "i_view64" | "i_view32" => "IrfanView".to_string(),
        "xnview" | "xnviewmp" => "XnView MP".to_string(),
        _ => {
            let mut chars = stem.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => stem.to_string(),
            }
        }
    }
}

fn expand_env_vars(input: &str) -> String {
    let mut out = input.to_string();
    for var in ["SystemRoot", "WINDIR", "ProgramFiles", "ProgramFiles(x86)", "LocalAppData", "AppData"] {
        if let Ok(val) = std::env::var(var) {
            let pattern_upper = format!("%{}%", var.to_ascii_uppercase());
            let pattern_exact = format!("%{var}%");
            out = out.replace(&pattern_exact, &val).replace(&pattern_upper, &val);
        }
    }
    out
}

fn split_exe_and_rest(cmd: &str) -> Option<(String, String)> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(after_quote) = trimmed.strip_prefix('"') {
        let end = after_quote.find('"')?;
        let exe = after_quote[..end].to_string();
        let rest = after_quote[end + 1..].trim().to_string();
        Some((exe, rest))
    } else {
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let exe = parts.next()?.to_string();
        let rest = parts.next().unwrap_or("").trim().to_string();
        Some((exe, rest))
    }
}

fn command_target_exists(cmd: &str) -> bool {
    let expanded = expand_env_vars(cmd);
    let Some((exe, _)) = split_exe_and_rest(&expanded) else {
        return false;
    };
    let lower = exe.to_ascii_lowercase();
    if lower.contains("rundll32") || lower.contains("dllhost") {
        return false;
    }
    let p = Path::new(&exe);
    if p.is_absolute() {
        p.exists()
    } else {
        // Bare exe name like mspaint.exe; let registry association stand if valid
        lower.ends_with(".exe")
    }
}

fn extract_exe_basename(cmd: &str) -> Option<String> {
    let expanded = expand_env_vars(cmd);
    let (exe, _) = split_exe_and_rest(&expanded)?;
    Path::new(&exe)
        .file_name()
        .map(|f| f.to_string_lossy().to_ascii_lowercase())
}

pub fn launch_with_app(app: &RegisteredApp, file_path: &Path) -> Result<(), String> {
    let expanded = expand_env_vars(&app.command_template);
    let (exe, rest) = split_exe_and_rest(&expanded)
        .ok_or_else(|| format!("Invalid command for {}", app.name))?;

    let path_str = file_path.to_string_lossy();
    let mut cmd = Command::new(&exe);

    #[cfg(windows)]
    {
        let args_str = if rest.contains("%1")
            || rest.contains("%L")
            || rest.contains("%l")
            || rest.contains("%V")
        {
            rest.replace("\"%1\"", &format!("\"{path_str}\""))
                .replace("%1", &format!("\"{path_str}\""))
                .replace("\"%L\"", &format!("\"{path_str}\""))
                .replace("%L", &format!("\"{path_str}\""))
                .replace("%l", &format!("\"{path_str}\""))
                .replace("%V", &format!("\"{path_str}\""))
        } else if rest.is_empty() {
            format!("\"{path_str}\"")
        } else {
            format!("{rest} \"{path_str}\"")
        };
        cmd.raw_arg(args_str);
    }

    #[cfg(not(windows))]
    {
        let _ = rest;
        cmd.arg(file_path);
    }

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open with {}: {e}", app.name))
}

pub fn open_with_system_dialog(file_path: &Path) -> Result<(), String> {
    let path_str = file_path.to_string_lossy();
    let mut cmd = Command::new("rundll32.exe");
    #[cfg(windows)]
    cmd.raw_arg(format!("shell32.dll,OpenAs_RunDLL {path_str}"));
    #[cfg(not(windows))]
    cmd.args(["shell32.dll,OpenAs_RunDLL", &path_str]);

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Windows app chooser: {e}"))
}

pub fn reveal_in_explorer(file_path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("explorer.exe");
    #[cfg(windows)]
    cmd.raw_arg(format!("/select,\"{}\"", file_path.display()));
    #[cfg(not(windows))]
    cmd.arg(file_path);

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Explorer: {e}"))
}

pub fn copy_image_to_clipboard(img: &DynamicImage) -> Result<(), String> {
    let rgba = img.to_rgba8();
    let width = rgba.width() as usize;
    let height = rgba.height() as usize;
    let bytes = rgba.into_raw();

    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Error opening clipboard: {e}"))?;
    clipboard
        .set_image(arboard::ImageData {
            width,
            height,
            bytes: Cow::Owned(bytes),
        })
        .map_err(|e| format!("Error copying image to clipboard: {e}"))
}

pub fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Error opening clipboard: {e}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|e| format!("Error copying text to clipboard: {e}"))
}

pub fn open_url(url: &str) -> Result<(), String> {
    let mut cmd = Command::new("cmd");
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd.args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open browser: {e}"))
}

pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| format!("Could not move to Recycle Bin: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_windows_registered_editors_for_png() {
        let apps = find_registered_editors(Path::new("sample.png"));
        assert!(!apps.is_empty(), "Expected at least Paint or another registered editor on Windows");
    }

    #[test]
    fn splits_quoted_and_unquoted_commands() {
        let (exe, rest) = split_exe_and_rest(r#""C:\Program Files\App\app.exe" /edit "%1""#).unwrap();
        assert_eq!(exe, r"C:\Program Files\App\app.exe");
        assert_eq!(rest, r#"/edit "%1""#);

        let (exe2, rest2) = split_exe_and_rest("mspaint.exe %1").unwrap();
        assert_eq!(exe2, "mspaint.exe");
        assert_eq!(rest2, "%1");
    }
}
