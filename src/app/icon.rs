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

/// A Windows .ico containing PNG-compressed images at the given sizes.
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| crate::util::encode_png(&icon_rgba(s), s, s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// Development helper: `corrent --write-icons <dir>` renders the icon set used for packaging.
pub fn write_icon_set(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for s in [32u32, 64, 128, 256, 512] {
        std::fs::write(dir.join(format!("corrent-{s}.png")), crate::util::encode_png(&icon_rgba(s), s, s))?;
    }
    std::fs::write(dir.join("corrent.ico"), ico(&[16, 24, 32, 48, 64, 128, 256]))
}
