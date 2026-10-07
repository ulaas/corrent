//! Procedurally drawn application icon (no binary assets needed).

/// RGBA pixels of the Corrent icon at `size`×`size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let radius = s * 0.22;
    let (top, bottom) = ([0x6A, 0x9C, 0xFF], [0x8B, 0x5C, 0xF6]);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded-square coverage with 1px anti-aliasing.
            let dx = (fx - s / 2.0).abs() - (s / 2.0 - radius);
            let dy = (fy - s / 2.0).abs() - (s / 2.0 - radius);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - radius;
            let cover = (0.5 - outside).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let t = (fx + fy) / (2.0 * s);
            let mut rgb = [0f32; 3];
            for i in 0..3 {
                rgb[i] = top[i] as f32 * (1.0 - t) + bottom[i] as f32 * t;
            }
            // White down-arrow glyph: a shaft and a chevron, plus a base bar.
            let u = fx / s;
            let v = fy / s;
            let shaft = (u - 0.5).abs() < 0.065 && v > 0.2 && v < 0.6;
            let chevron = {
                let d = (v - 0.62) + (u - 0.5).abs();
                d.abs() < 0.075 && v < 0.66 && v > 0.36 && (u - 0.5).abs() < 0.24
            };
            let base = v > 0.74 && v < 0.82 && (u - 0.5).abs() < 0.24;
            let i = ((y * size + x) * 4) as usize;
            let white = shaft || chevron || base;
            for c in 0..3 {
                px[i + c] = if white { 255 } else { rgb[c] as u8 };
            }
            px[i + 3] = (cover * 255.0) as u8;
        }
    }
    px
}

pub fn app_icon() -> egui::IconData {
    let size = 128;
    egui::IconData { rgba: icon_rgba(size), width: size, height: size }
}
