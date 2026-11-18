pub mod decoder;
pub mod video;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    StaticImage,
    AnimatedImage,
    VectorSvg,
    ExtendedImage,
    Video,
}

impl MediaKind {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" | "jfif" | "png" | "webp" | "bmp" | "ico" | "cur" | "tiff" | "tif"
            | "tga" | "dds" | "hdr" | "exr" | "qoi" | "pnm" | "pbm" | "pgm" | "ppm" | "ff" => {
                Some(Self::StaticImage)
            }
            "gif" => Some(Self::AnimatedImage),
            "svg" | "svgz" => Some(Self::VectorSvg),
            "heic" | "heif" | "avif" | "jxl" | "psd" | "jp2" | "cr2" | "cr3" | "nef" | "arw"
            | "dng" | "orf" | "rw2" | "pef" | "raf" | "srw" => Some(Self::ExtendedImage),
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg"
            | "3gp" | "ts" => Some(Self::Video),
            _ => None,
        }
    }

    pub fn is_video(self) -> bool {
        matches!(self, Self::Video)
    }

    pub fn badge(self) -> Option<&'static str> {
        match self {
            Self::Video => Some("VIDEO"),
            Self::AnimatedImage => Some("GIF"),
            Self::VectorSvg => Some("SVG"),
            Self::ExtendedImage => Some("RAW/EXT"),
            Self::StaticImage => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MediaItem {
    pub path: PathBuf,
    pub file_name: String,
    pub kind: MediaKind,
    pub file_size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    NameAsc,
    NameDesc,
    DateNewest,
    DateOldest,
    SizeDesc,
    SizeAsc,
}

impl SortOrder {
    pub fn label(self, lang: Language) -> &'static str {
        let t = lang.tr();
        match self {
            Self::NameAsc => t.sort_name_asc,
            Self::NameDesc => t.sort_name_desc,
            Self::DateNewest => t.sort_date_newest,
            Self::DateOldest => t.sort_date_oldest,
            Self::SizeDesc => t.sort_size_desc,
            Self::SizeAsc => t.sort_size_asc,
        }
    }

    pub const ALL: [SortOrder; 6] = [
        SortOrder::NameAsc,
        SortOrder::NameDesc,
        SortOrder::DateNewest,
        SortOrder::DateOldest,
        SortOrder::SizeDesc,
        SortOrder::SizeAsc,
    ];
}

/// Natural case-insensitive string comparison (e.g. `img_2.jpg` < `img_10.jpg`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ac), Some(bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let mut a_digits = String::new();
                    while let Some(c) = ai.peek().copied() {
                        if c.is_ascii_digit() {
                            a_digits.push(c);
                            ai.next();
                        } else {
                            break;
                        }
                    }
                    let mut b_digits = String::new();
                    while let Some(c) = bi.peek().copied() {
                        if c.is_ascii_digit() {
                            b_digits.push(c);
                            bi.next();
                        } else {
                            break;
                        }
                    }

                    let a_trim = a_digits.trim_start_matches('0');
                    let b_trim = b_digits.trim_start_matches('0');
                    let ord = a_trim.len().cmp(&b_trim.len()).then_with(|| a_trim.cmp(b_trim));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    // Tie-break on leading zero count (shorter literal first)
                    let len_ord = a_digits.len().cmp(&b_digits.len());
                    if len_ord != Ordering::Equal {
                        return len_ord;
                    }
                } else {
                    ai.next();
                    bi.next();
                    let al = ac.to_lowercase();
                    let bl = bc.to_lowercase();
                    let ord = al.cmp(bl);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
}

pub fn scan_directory(dir: &Path) -> Vec<MediaItem> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(kind) = MediaKind::from_path(&path) else {
            continue;
        };
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meta = entry.metadata().ok();
        let file_size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let created = meta.and_then(|m| m.created().ok());
        items.push(MediaItem {
            path,
            file_name,
            kind,
            file_size,
            modified,
            created,
        });
    }

    sort_items(&mut items, SortOrder::NameAsc);
    items
}

pub fn sort_items(items: &mut [MediaItem], order: SortOrder) {
    items.sort_by(|a, b| match order {
        SortOrder::NameAsc => natural_cmp(&a.file_name, &b.file_name),
        SortOrder::NameDesc => natural_cmp(&b.file_name, &a.file_name),
        SortOrder::DateNewest => b
            .modified
            .cmp(&a.modified)
            .then_with(|| natural_cmp(&a.file_name, &b.file_name)),
        SortOrder::DateOldest => a
            .modified
            .cmp(&b.modified)
            .then_with(|| natural_cmp(&a.file_name, &b.file_name)),
        SortOrder::SizeDesc => b
            .file_size
            .cmp(&a.file_size)
            .then_with(|| natural_cmp(&a.file_name, &b.file_name)),
        SortOrder::SizeAsc => a
            .file_size
            .cmp(&b.file_size)
            .then_with(|| natural_cmp(&a.file_name, &b.file_name)),
    });
}

pub fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_system_time(t: Option<SystemTime>) -> Option<String> {
    let t = t?;
    let dur = t.duration_since(UNIX_EPOCH).ok()?;
    let unix_secs = dur.as_secs();

    #[cfg(windows)]
    {
        if let Some(s) = format_windows_local_time(unix_secs) {
            return Some(s);
        }
    }

    Some(format_utc_secs(unix_secs))
}

#[cfg(windows)]
fn format_windows_local_time(unix_secs: u64) -> Option<String> {
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeWin {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    extern "system" {
        fn FileTimeToLocalFileTime(lpFileTime: *const FileTime, lpLocalFileTime: *mut FileTime) -> i32;
        fn FileTimeToSystemTime(lpFileTime: *const FileTime, lpSystemTime: *mut SystemTimeWin) -> i32;
    }

    // 11644473600 seconds between 1601-01-01 and 1970-01-01
    let ticks = (unix_secs + 11_644_473_600).checked_mul(10_000_000)?;
    let ft = FileTime {
        low: (ticks & 0xFFFF_FFFF) as u32,
        high: (ticks >> 32) as u32,
    };
    let mut local_ft = FileTime { low: 0, high: 0 };
    let mut st = SystemTimeWin::default();
    unsafe {
        if FileTimeToLocalFileTime(&ft, &mut local_ft) == 0 {
            return None;
        }
        if FileTimeToSystemTime(&local_ft, &mut st) == 0 {
            return None;
        }
    }
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.year, st.month, st.day, st.hour, st.minute, st.second
    ))
}

fn format_utc_secs(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02} UTC")
}

#[derive(Debug, Clone)]
pub struct ExifSummary {
    pub orientation: u32,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub software: Option<String>,
    pub date_taken: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub iso: Option<String>,
    pub focal_length: Option<String>,
    pub focal_length_35mm: Option<String>,
    pub exposure_bias: Option<String>,
    pub flash: Option<String>,
    pub metering_mode: Option<String>,
    pub exposure_program: Option<String>,
    pub white_balance: Option<String>,
    pub brightness: Option<String>,
    pub max_aperture: Option<String>,
    pub digital_zoom: Option<String>,
    pub lens_model: Option<String>,
    pub color_space: Option<String>,
    pub dpi_x: Option<String>,
    pub dpi_y: Option<String>,
    pub bits_per_sample: Option<String>,
    pub exif_version: Option<String>,
    pub gps_lat_lon: Option<(f64, f64)>,
    pub gps_display: Option<String>,
    pub gps_altitude: Option<String>,
    pub categories: Vec<(String, Vec<(String, String)>)>,
}

impl Default for ExifSummary {
    fn default() -> Self {
        Self {
            orientation: 1,
            camera_make: None,
            camera_model: None,
            software: None,
            date_taken: None,
            exposure_time: None,
            f_number: None,
            iso: None,
            focal_length: None,
            focal_length_35mm: None,
            exposure_bias: None,
            flash: None,
            metering_mode: None,
            exposure_program: None,
            white_balance: None,
            brightness: None,
            max_aperture: None,
            digital_zoom: None,
            lens_model: None,
            color_space: None,
            dpi_x: None,
            dpi_y: None,
            bits_per_sample: None,
            exif_version: None,
            gps_lat_lon: None,
            gps_display: None,
            gps_altitude: None,
            categories: Vec::new(),
        }
    }
}

pub fn read_exif(path: &Path) -> ExifSummary {
    let mut summary = ExifSummary::default();
    let Ok(file) = fs::File::open(path) else {
        return summary;
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return summary;
    };

    if let Some(field) = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
        if let Some(val) = field.value.get_uint(0) {
            summary.orientation = val;
        }
    }

    let get_str = |tag: exif::Tag| -> Option<String> {
        let field = exif.get_field(tag, exif::In::PRIMARY)?;
        let s = field.display_value().with_unit(&exif).to_string();
        let trimmed = s.trim().trim_matches('"').trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    };

    summary.camera_make = get_str(exif::Tag::Make);
    summary.camera_model = get_str(exif::Tag::Model);
    summary.software = get_str(exif::Tag::Software);
    summary.date_taken = get_str(exif::Tag::DateTimeOriginal);
    summary.exposure_time = get_str(exif::Tag::ExposureTime);
    summary.f_number = get_str(exif::Tag::FNumber);
    summary.iso = get_str(exif::Tag::PhotographicSensitivity)
        .or_else(|| get_str(exif::Tag::ISOSpeed));
    summary.focal_length = get_str(exif::Tag::FocalLength);
    summary.focal_length_35mm = get_str(exif::Tag::FocalLengthIn35mmFilm);
    summary.exposure_bias = get_str(exif::Tag::ExposureBiasValue);
    summary.flash = get_str(exif::Tag::Flash);
    summary.metering_mode = get_str(exif::Tag::MeteringMode);
    summary.exposure_program = get_str(exif::Tag::ExposureProgram);
    summary.white_balance = get_str(exif::Tag::WhiteBalance);
    summary.brightness = get_str(exif::Tag::BrightnessValue);
    summary.max_aperture = get_str(exif::Tag::MaxApertureValue);
    summary.digital_zoom = get_str(exif::Tag::DigitalZoomRatio);
    summary.lens_model = get_str(exif::Tag::LensModel);
    summary.color_space = get_str(exif::Tag::ColorSpace);
    summary.dpi_x = get_str(exif::Tag::XResolution);
    summary.dpi_y = get_str(exif::Tag::YResolution);
    summary.bits_per_sample = get_str(exif::Tag::BitsPerSample);
    summary.exif_version = get_str(exif::Tag::ExifVersion);
    summary.gps_altitude = get_str(exif::Tag::GPSAltitude);

    if let (Some(lat), Some(lon)) = (
        parse_gps_coord(&exif, exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef, 'S'),
        parse_gps_coord(&exif, exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef, 'W'),
    ) {
        summary.gps_lat_lon = Some((lat, lon));
        summary.gps_display = Some(format!("{lat:.6}, {lon:.6}"));
    }

    summary.categories = build_categorized_exif(&exif);
    summary
}

fn parse_gps_coord(
    exif: &exif::Exif,
    coord_tag: exif::Tag,
    ref_tag: exif::Tag,
    neg_ref: char,
) -> Option<f64> {
    let field = exif.get_field(coord_tag, exif::In::PRIMARY)?;
    let exif::Value::Rational(rats) = &field.value else {
        return None;
    };
    if rats.len() < 3 {
        return None;
    }
    let deg = rats[0].to_f64();
    let min = rats[1].to_f64();
    let sec = rats[2].to_f64();
    if deg.is_nan() || min.is_nan() || sec.is_nan() {
        return None;
    }
    let mut val = deg + min / 60.0 + sec / 3600.0;
    if let Some(ref_field) = exif.get_field(ref_tag, exif::In::PRIMARY) {
        let s = ref_field.display_value().to_string();
        if s.trim().to_ascii_uppercase().starts_with(neg_ref) {
            val = -val;
        }
    }
    Some(val)
}

fn build_categorized_exif(exif: &exif::Exif) -> Vec<(String, Vec<(String, String)>)> {
    let mut buckets: BTreeMap<&'static str, Vec<(String, String)>> = BTreeMap::new();
    let mut seen: HashSet<(u16, u16)> = HashSet::new();

    for field in exif.fields() {
        if field.ifd_num != exif::In::PRIMARY {
            continue;
        }
        let ctx_id = match field.tag.context() {
            exif::Context::Tiff => 0u16,
            exif::Context::Exif => 1u16,
            exif::Context::Gps => 2u16,
            exif::Context::Interop => 3u16,
            _ => 4u16,
        };
        if !seen.insert((ctx_id, field.tag.number())) {
            continue;
        }

        let value_str = format_exif_value(field, exif);
        if value_str.is_empty() {
            continue;
        }

        let (cat, label) = classify_exif_tag(field.tag);
        buckets.entry(cat).or_default().push((label, value_str));
    }

    let order = [
        "Camera",
        "Exposure",
        "Aperture",
        "Lens & Zoom",
        "Image & Color",
        "GPS",
        "Other EXIF Data",
    ];

    let mut out = Vec::new();
    for cat in order {
        if let Some(mut entries) = buckets.remove(cat) {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            out.push((cat.to_string(), entries));
        }
    }
    out
}

fn format_exif_value(field: &exif::Field, exif: &exif::Exif) -> String {
    if field.tag == exif::Tag::MakerNote {
        let bytes_len = match &field.value {
            exif::Value::Undefined(v, _) | exif::Value::Byte(v) => v.len(),
            _ => 0,
        };
        if bytes_len > 0 {
            return format!("<{bytes_len} bytes>");
        }
    }

    match &field.value {
        exif::Value::Undefined(v, _) | exif::Value::Byte(v) if v.len() > 48 => {
            format!("<{} bytes>", v.len())
        }
        _ => {
            let s = field.display_value().with_unit(exif).to_string();
            let trimmed = s.trim().trim_matches('"').trim().to_string();
            if trimmed.len() > 140 {
                format!("{}...", &trimmed[..137])
            } else {
                trimmed
            }
        }
    }
}

fn classify_exif_tag(tag: exif::Tag) -> (&'static str, String) {
    use exif::Tag;
    match tag {
        Tag::Make => ("Camera", "Make".to_string()),
        Tag::Model => ("Camera", "Model".to_string()),
        Tag::Software => ("Camera", "Software".to_string()),
        Tag::ExifVersion => ("Camera", "EXIF Version".to_string()),
        Tag::DateTimeOriginal => ("Camera", "Date Taken".to_string()),
        Tag::DateTimeDigitized => ("Camera", "Date Digitized".to_string()),
        Tag::DateTime => ("Camera", "Date Modified".to_string()),
        Tag::OffsetTime | Tag::OffsetTimeOriginal => ("Camera", "Time Zone".to_string()),
        Tag::Artist => ("Camera", "Artist".to_string()),
        Tag::Copyright => ("Camera", "Copyright".to_string()),
        Tag::BodySerialNumber => ("Camera", "Body Serial Number".to_string()),

        Tag::ExposureTime => ("Exposure", "Exposure Time".to_string()),
        Tag::ShutterSpeedValue => ("Exposure", "Shutter Speed".to_string()),
        Tag::ExposureProgram => ("Exposure", "Exposure Program".to_string()),
        Tag::ExposureMode => ("Exposure", "Exposure Mode".to_string()),
        Tag::ExposureBiasValue => ("Exposure", "Exposure Bias".to_string()),
        Tag::PhotographicSensitivity | Tag::ISOSpeed => ("Exposure", "ISO".to_string()),
        Tag::MeteringMode => ("Exposure", "Metering Mode".to_string()),
        Tag::BrightnessValue => ("Exposure", "Brightness".to_string()),
        Tag::Flash => ("Exposure", "Flash".to_string()),
        Tag::LightSource => ("Exposure", "Light Source".to_string()),
        Tag::WhiteBalance => ("Exposure", "White Balance".to_string()),
        Tag::SceneCaptureType => ("Exposure", "Scene Capture Type".to_string()),

        Tag::FNumber => ("Aperture", "Aperture (f/)".to_string()),
        Tag::ApertureValue => ("Aperture", "Aperture Value".to_string()),
        Tag::MaxApertureValue => ("Aperture", "Max Aperture".to_string()),

        Tag::FocalLength => ("Lens & Zoom", "Focal Length".to_string()),
        Tag::FocalLengthIn35mmFilm => ("Lens & Zoom", "35mm Equiv. Focal".to_string()),
        Tag::DigitalZoomRatio => ("Lens & Zoom", "Digital Zoom".to_string()),
        Tag::LensMake => ("Lens & Zoom", "Lens Make".to_string()),
        Tag::LensModel => ("Lens & Zoom", "Lens Model".to_string()),
        Tag::LensSpecification => ("Lens & Zoom", "Lens Specification".to_string()),
        Tag::SubjectDistance => ("Lens & Zoom", "Subject Distance".to_string()),

        Tag::PixelXDimension | Tag::ImageWidth => ("Image & Color", "EXIF Width".to_string()),
        Tag::PixelYDimension | Tag::ImageLength => ("Image & Color", "EXIF Height".to_string()),
        Tag::Orientation => ("Image & Color", "Orientation".to_string()),
        Tag::XResolution => ("Image & Color", "X Resolution".to_string()),
        Tag::YResolution => ("Image & Color", "Y Resolution".to_string()),
        Tag::ResolutionUnit => ("Image & Color", "Resolution Unit".to_string()),
        Tag::ColorSpace => ("Image & Color", "Color Space".to_string()),
        Tag::BitsPerSample => ("Image & Color", "Bits per Sample".to_string()),
        Tag::Compression => ("Image & Color", "Compression".to_string()),
        Tag::Contrast => ("Image & Color", "Contrast".to_string()),
        Tag::Saturation => ("Image & Color", "Saturation".to_string()),
        Tag::Sharpness => ("Image & Color", "Sharpness".to_string()),

        t if t.context() == exif::Context::Gps => ("GPS", t.to_string()),
        _ => ("Other EXIF Data", tag.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_orders_numbers_correctly() {
        let mut names = vec!["img_10.jpg", "img_2.jpg", "IMG_1.jpg", "img_20.png", "alpha.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec!["alpha.jpg", "IMG_1.jpg", "img_2.jpg", "img_10.jpg", "img_20.png"]
        );
    }

    #[test]
    fn media_kind_detection_covers_images_raw_svg_and_video() {
        assert_eq!(MediaKind::from_path(Path::new("photo.JPG")), Some(MediaKind::StaticImage));
        assert_eq!(MediaKind::from_path(Path::new("anim.gif")), Some(MediaKind::AnimatedImage));
        assert_eq!(MediaKind::from_path(Path::new("icon.svg")), Some(MediaKind::VectorSvg));
        assert_eq!(MediaKind::from_path(Path::new("pic.heic")), Some(MediaKind::ExtendedImage));
        assert_eq!(MediaKind::from_path(Path::new("raw.CR3")), Some(MediaKind::ExtendedImage));
        assert_eq!(MediaKind::from_path(Path::new("clip.mp4")), Some(MediaKind::Video));
        assert_eq!(MediaKind::from_path(Path::new("readme.txt")), None);
    }

    #[test]
    fn scan_directory_sorts_naturally() {
        let tmp = std::env::temp_dir().join(format!("imgv_scan_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        for name in ["shot_12.png", "shot_2.png", "shot_1.jpg", "ignore.txt", "clip.mkv"] {
            fs::write(tmp.join(name), b"dummy").unwrap();
        }
        let scanned = scan_directory(&tmp);
        let names: Vec<_> = scanned.iter().map(|i| i.file_name.as_str()).collect();
        assert_eq!(names, vec!["clip.mkv", "shot_1.jpg", "shot_2.png", "shot_12.png"]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn formats_timestamps_accurately() {
        assert_eq!(format_utc_secs(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc_secs(1_700_000_000), "2023-11-14 22:13:20 UTC");
        assert!(format_system_time(Some(SystemTime::now())).is_some());
    }
}
