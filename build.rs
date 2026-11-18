use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ico_path = out_dir.join("app_icon.ico");
    let rc_path = out_dir.join("app.rc");
    let res_path = out_dir.join("app.res");

    let rgba = render_icon_rgba(64);
    let png = encode_stored_png(64, 64, &rgba);
    let ico = wrap_png_as_ico(64, 64, &png);

    if fs::write(&ico_path, ico).is_err() {
        return;
    }
    if fs::write(&rc_path, "1 ICON \"app_icon.ico\"\r\n").is_err() {
        return;
    }

    if let Some(rc_exe) = find_rc_exe() {
        let status = Command::new(rc_exe)
            .args(["/nologo", "/fo"])
            .arg(&res_path)
            .arg(&rc_path)
            .status();
        if matches!(status, Ok(s) if s.success()) && res_path.exists() {
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
    }
}

fn find_rc_exe() -> Option<PathBuf> {
    let preferred = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\rc.exe");
    if preferred.exists() {
        return Some(preferred.to_path_buf());
    }
    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let entries = fs::read_dir(kits_bin).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path().join("x64").join("rc.exe"))
        .filter(|p| p.exists())
        .collect();
    candidates.sort();
    candidates.pop()
}

fn wrap_png_as_ico(width: u8, height: u8, png: &[u8]) -> Vec<u8> {
    let mut ico = Vec::with_capacity(22 + png.len());
    // ICONDIR
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    // ICONDIRENTRY
    ico.push(width);
    ico.push(height);
    ico.push(0);
    ico.push(0);
    ico.extend_from_slice(&1u16.to_le_bytes()); // planes
    ico.extend_from_slice(&32u16.to_le_bytes()); // bpp
    ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes()); // offset
    ico.extend_from_slice(png);
    ico
}

fn encode_stored_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut png = Vec::new();
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    write_png_chunk(&mut png, *b"IHDR", &ihdr);

    let stride = (width * 4) as usize;
    let mut raw = Vec::with_capacity(height as usize * (1 + stride));
    for y in 0..height as usize {
        raw.push(0); // filter: None
        let start = y * stride;
        raw.extend_from_slice(&rgba[start..start + stride]);
    }

    let len = raw.len() as u16;
    let nlen = !len;
    let mut zlib = Vec::with_capacity(2 + 5 + raw.len() + 4);
    zlib.extend_from_slice(&[0x78, 0x01]);
    zlib.push(0x01); // BFINAL=1, BTYPE=00 (stored)
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&nlen.to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    write_png_chunk(&mut png, *b"IDAT", &zlib);

    write_png_chunk(&mut png, *b"IEND", &[]);
    png
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&chunk_type);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(&chunk_type);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

fn render_icon_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let sub = 4;

    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..sub {
                for sx in 0..sub {
                    let u = (px as f32 + (sx as f32 + 0.5) / sub as f32) / s;
                    let v = (py as f32 + (sy as f32 + 0.5) / sub as f32) / s;
                    let c = sample_pixel(u, v);
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let n = (sub * sub) as f32;
            let a = (acc[3] / n).clamp(0.0, 1.0);
            let idx = ((py * size + px) * 4) as usize;
            if a > 0.001 {
                out[idx] = ((acc[0] / n / a) * 255.0).round().clamp(0.0, 255.0) as u8;
                out[idx + 1] = ((acc[1] / n / a) * 255.0).round().clamp(0.0, 255.0) as u8;
                out[idx + 2] = ((acc[2] / n / a) * 255.0).round().clamp(0.0, 255.0) as u8;
                out[idx + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn sample_pixel(u: f32, v: f32) -> [f32; 4] {
    let cx = (u - 0.5).abs();
    let cy = (v - 0.5).abs();
    let half = 0.45;
    let radius = 0.14;
    let qx = (cx - (half - radius)).max(0.0);
    let qy = (cy - (half - radius)).max(0.0);
    let dist = (qx * qx + qy * qy).sqrt() - radius;
    if dist > 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }

    let t = (u * 0.55 + v * 0.65).clamp(0.0, 1.0);
    let mut r = 0.10 + (0.06 - 0.10) * t;
    let mut g = 0.22 + (0.56 - 0.22) * t;
    let mut b = 0.58 + (0.90 - 0.58) * t;

    if dist > -0.022 {
        r += (0.65 - r) * 0.35;
        g += (0.85 - g) * 0.35;
        b += (1.00 - b) * 0.35;
    }

    let dx = u - 0.69;
    let dy = v - 0.31;
    let sun_d = (dx * dx + dy * dy).sqrt();
    if sun_d <= 0.105 {
        let st = (sun_d / 0.105).clamp(0.0, 1.0);
        r = 1.0 + (0.98 - 1.0) * st;
        g = 0.90 + (0.72 - 0.90) * st;
        b = 0.45 + (0.16 - 0.45) * st;
    }

    let back_peak = 0.80 - 0.36 * (1.0 - ((u - 0.62) / 0.34).abs()).max(0.0);
    if v >= back_peak && v <= 0.86 && u >= 0.12 && u <= 0.88 {
        let grad = ((v - 0.44) / 0.42).clamp(0.0, 1.0);
        r = 0.70 + (0.52 - 0.70) * grad;
        g = 0.89 + (0.78 - 0.89) * grad;
        b = 0.99;
    }

    let front_peak = 0.84 - 0.44 * (1.0 - ((u - 0.37) / 0.34).abs()).max(0.0);
    if v >= front_peak && v <= 0.86 && u >= 0.10 && u <= 0.88 {
        let grad = ((v - 0.40) / 0.46).clamp(0.0, 1.0);
        r = 0.98 + (0.86 - 0.98) * grad;
        g = 0.99 + (0.93 - 0.99) * grad;
        b = 1.0;
    }

    [r, g, b, 1.0]
}
