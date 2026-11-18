use eframe::egui;

pub fn build_app_icon_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let sub = 4; // 4x4 supersampling for crisp anti-aliased edges

    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..sub {
                for sx in 0..sub {
                    let u = (px as f32 + (sx as f32 + 0.5) / sub as f32) / s;
                    let v = (py as f32 + (sy as f32 + 0.5) / sub as f32) / s;
                    let c = sample_icon_pixel(u, v);
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

fn sample_icon_pixel(u: f32, v: f32) -> [f32; 4] {
    // Rounded square SDF in normalized [0,1]x[0,1] space
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

    // Background gradient: deep cobalt -> sky cyan
    let t = (u * 0.55 + v * 0.65).clamp(0.0, 1.0);
    let mut r = lerp(0.10, 0.06, t);
    let mut g = lerp(0.22, 0.56, t);
    let mut b = lerp(0.58, 0.90, t);

    // Subtle inner rim highlight
    if dist > -0.022 {
        r = lerp(r, 0.65, 0.35);
        g = lerp(g, 0.85, 0.35);
        b = lerp(b, 1.00, 0.35);
    }

    // Golden sun circle at (0.69, 0.31)
    let dx = u - 0.69;
    let dy = v - 0.31;
    let sun_d = (dx * dx + dy * dy).sqrt();
    if sun_d <= 0.105 {
        let st = (sun_d / 0.105).clamp(0.0, 1.0);
        r = lerp(1.0, 0.98, st);
        g = lerp(0.90, 0.72, st);
        b = lerp(0.45, 0.16, st);
    }

    // Back mountain (lighter sky-ice silhouette)
    let back_peak = 0.80 - 0.36 * (1.0 - ((u - 0.62) / 0.34).abs()).max(0.0);
    if v >= back_peak && v <= 0.86 && u >= 0.12 && u <= 0.88 {
        r = lerp(0.70, 0.52, ((v - 0.44) / 0.42).clamp(0.0, 1.0));
        g = lerp(0.89, 0.78, ((v - 0.44) / 0.42).clamp(0.0, 1.0));
        b = 0.99;
    }

    // Front mountain (crisp bright white-slate)
    let front_peak = 0.84 - 0.44 * (1.0 - ((u - 0.37) / 0.34).abs()).max(0.0);
    if v >= front_peak && v <= 0.86 && u >= 0.10 && u <= 0.88 {
        let grad = ((v - 0.40) / 0.46).clamp(0.0, 1.0);
        r = lerp(0.98, 0.86, grad);
        g = lerp(0.99, 0.93, grad);
        b = 1.0;
    }

    [r, g, b, 1.0]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn build_egui_icon() -> egui::IconData {
    let size = 64u32;
    egui::IconData {
        rgba: build_app_icon_rgba(size),
        width: size,
        height: size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_valid_64x64_rgba_icon_with_transparent_corners_and_opaque_center() {
        let rgba = build_app_icon_rgba(64);
        assert_eq!(rgba.len(), 64 * 64 * 4);
        // Top-left corner pixel (0,0) should be transparent
        assert_eq!(rgba[3], 0);
        // Center pixel (32,32) should be opaque
        let center_alpha = rgba[(32 * 64 + 32) * 4 + 3];
        assert_eq!(center_alpha, 255);
    }
}
