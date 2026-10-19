//! Artwork: verified, decoded, resized into 200/400/800 px JPEG variants plus a blurhash, and
//! stored by content hash (`<data>/artwork/<sha[..2]>/<sha>.jpg`, `….w400.jpg`) so URLs are
//! immutable and identical images are stored once.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader};

pub const WIDTHS: [u32; 3] = [200, 400, 800];
/// Larger inputs are refused before decoding (decompression bombs).
const MAX_BYTES: usize = 40 * 1024 * 1024;
const MAX_PIXELS: u64 = 80_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub sha: String,
    pub width: u32,
    pub height: u32,
    pub blurhash: String,
}

pub fn sha256_hex(data: &[u8]) -> String {
    aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, data).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The file for `sha` at the variant closest to `width` (None = the full-size copy).
pub fn path_for(root: &Path, sha: &str, width: Option<u32>) -> PathBuf {
    let dir = root.join(sha.get(..2).unwrap_or("00"));
    match width {
        None => dir.join(format!("{sha}.jpg")),
        Some(w) => {
            let pick = WIDTHS.iter().copied().find(|v| *v >= w).unwrap_or(WIDTHS[WIDTHS.len() - 1]);
            dir.join(format!("{sha}.w{pick}.jpg"))
        }
    }
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Path of an existing variant (falls back to the original); `None` for unknown or bad hashes.
pub fn existing(root: &Path, sha: &str, width: Option<u32>) -> Option<PathBuf> {
    if !valid_sha(sha) {
        return None;
    }
    let p = path_for(root, sha, width);
    if p.exists() {
        return Some(p);
    }
    let full = path_for(root, sha, None);
    full.exists().then_some(full)
}

fn decode(data: &[u8]) -> Result<DynamicImage, String> {
    if data.len() > MAX_BYTES {
        return Err("image too large".into());
    }
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format().is_none() {
        return Err("not an image".into());
    }
    let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if w as u64 * h as u64 > MAX_PIXELS || w == 0 || h == 0 {
        return Err("image dimensions out of range".into());
    }
    ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())
}

fn save_jpeg(img: &DynamicImage, path: &Path) -> Result<(), String> {
    let mut out = Vec::new();
    let rgb = DynamicImage::ImageRgb8(img.to_rgb8());
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    enc.encode_image(&rgb).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Stores an image (blocking; run it on a worker). Existing hashes are not processed again.
pub fn store(root: &Path, data: &[u8]) -> Result<Stored, String> {
    let sha = sha256_hex(data);
    let full = path_for(root, &sha, None);
    let img = decode(data)?;
    let (w, h) = img.dimensions();
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if !full.exists() {
        // Keep the original bytes when they are already JPEG; re-encode anything else.
        if image::guess_format(data).ok() == Some(ImageFormat::Jpeg) {
            let tmp = full.with_extension("tmp");
            std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &full).map_err(|e| e.to_string())?;
        } else {
            save_jpeg(&img, &full)?;
        }
    }
    for vw in WIDTHS {
        let p = path_for(root, &sha, Some(vw));
        if p.exists() {
            continue;
        }
        let v = if w > vw { img.resize(vw, u32::MAX, image::imageops::FilterType::CatmullRom) } else { img.clone() };
        save_jpeg(&v, &p)?;
    }
    let small = img.thumbnail(32, 32).to_rgb8();
    let blurhash = blurhash(small.width(), small.height(), small.as_raw(), 4, 3);
    Ok(Stored { sha, width: w, height: h, blurhash })
}

// ---------------------------------------------------------------- blurhash

fn srgb_to_linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> u32 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u32
}

fn base83(mut v: u32, len: usize, out: &mut String) {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";
    let mut buf = vec![0u8; len];
    for i in (0..len).rev() {
        buf[i] = CHARS[(v % 83) as usize];
        v /= 83;
    }
    out.push_str(std::str::from_utf8(&buf).unwrap_or(""));
}

fn sign_pow(v: f32, e: f32) -> f32 {
    v.abs().powf(e).copysign(v)
}

/// Blurhash of an RGB8 image (the standard algorithm, https://blurha.sh).
pub fn blurhash(w: u32, h: u32, rgb: &[u8], cx: u32, cy: u32) -> String {
    if w == 0 || h == 0 || rgb.len() < (w * h * 3) as usize {
        return String::new();
    }
    let mut factors = Vec::with_capacity((cx * cy) as usize);
    for j in 0..cy {
        for i in 0..cx {
            let norm = if i == 0 && j == 0 { 1.0 } else { 2.0 };
            let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
            for y in 0..h {
                for x in 0..w {
                    let basis = norm
                        * (std::f32::consts::PI * i as f32 * x as f32 / w as f32).cos()
                        * (std::f32::consts::PI * j as f32 * y as f32 / h as f32).cos();
                    let p = ((y * w + x) * 3) as usize;
                    r += basis * srgb_to_linear(rgb[p]);
                    g += basis * srgb_to_linear(rgb[p + 1]);
                    b += basis * srgb_to_linear(rgb[p + 2]);
                }
            }
            let scale = 1.0 / (w * h) as f32;
            factors.push((r * scale, g * scale, b * scale));
        }
    }
    let mut out = String::new();
    base83((cx - 1) + (cy - 1) * 9, 1, &mut out);
    let ac = &factors[1..];
    let max_ac = ac.iter().flat_map(|(r, g, b)| [r.abs(), g.abs(), b.abs()]).fold(0.0f32, f32::max);
    let (quant_max, max_value) = if ac.is_empty() {
        (0, 1.0)
    } else {
        let q = ((max_ac * 166.0 - 0.5).floor() as i32).clamp(0, 82) as u32;
        (q, (q as f32 + 1.0) / 166.0)
    };
    base83(quant_max, 1, &mut out);
    let (r, g, b) = factors[0];
    base83((linear_to_srgb(r) << 16) + (linear_to_srgb(g) << 8) + linear_to_srgb(b), 4, &mut out);
    for (r, g, b) in ac {
        let q = |v: f32| ((sign_pow(v / max_value, 0.5) * 9.0 + 9.5).floor() as i32).clamp(0, 18) as u32;
        base83(q(*r) * 19 * 19 + q(*g) * 19 + q(*b), 2, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 128]));
        let mut out = Vec::new();
        DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn stores_variants_once() {
        let root = std::env::temp_dir().join(format!("corrent-art-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let data = png(1000, 1500);
        let s = store(&root, &data).unwrap();
        assert_eq!((s.width, s.height), (1000, 1500));
        assert_eq!(s.blurhash.len(), 1 + 1 + 4 + 2 * 11);
        for w in WIDTHS {
            let p = path_for(&root, &s.sha, Some(w));
            let (vw, _) = image::image_dimensions(&p).unwrap();
            assert_eq!(vw, w);
        }
        assert_eq!(existing(&root, &s.sha, Some(300)), Some(path_for(&root, &s.sha, Some(400))));
        assert_eq!(existing(&root, "../../etc/passwd", None), None);
        assert_eq!(store(&root, &data).unwrap(), s, "same content, same result");
        assert!(store(&root, b"not an image").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn blurhash_reference() {
        // Size flag for 4×3 components is 21 ("L"); white's DC (0xFFFFFF) is "TSUA" in base 83.
        let rgb = vec![255u8; 4 * 4 * 3];
        let h = blurhash(4, 4, &rgb, 4, 3);
        assert!(h.starts_with('L'), "{h}");
        assert_eq!(&h[2..6], "TSUA", "white DC: {h}");
        assert_eq!(h.len(), 28);
    }
}
