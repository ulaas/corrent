//! Artwork for the media UI: fetched in the background, decoded off the UI thread, uploaded as
//! textures a few per frame, evicted least-recently-used. Blurhash placeholders show at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{ColorImage, TextureHandle, TextureOptions};

use super::client::MediaClient;

/// Texture memory kept before evicting (bytes, RGBA).
const BUDGET: usize = 256 * 1024 * 1024;
/// Textures created per frame (uploads cost frame time).
const UPLOADS_PER_FRAME: usize = 6;
const PARALLEL: usize = 6;

enum Entry {
    Loading,
    Decoded(ColorImage),
    Ready(TextureHandle, usize),
    Failed,
}

struct Slot {
    entry: Entry,
    used: Instant,
}

pub struct ImageCache {
    entries: Arc<Mutex<HashMap<(String, u32), Slot>>>,
    blur: HashMap<String, TextureHandle>,
    inflight: Arc<tokio::sync::Semaphore>,
    bytes: usize,
}

impl Default for ImageCache {
    fn default() -> Self {
        ImageCache {
            entries: Arc::default(),
            blur: HashMap::new(),
            inflight: Arc::new(tokio::sync::Semaphore::new(PARALLEL)),
            bytes: 0,
        }
    }
}

impl ImageCache {
    /// The texture for `sha` at about `width` px, starting a download when needed.
    pub fn get(
        &mut self,
        client: &MediaClient,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        sha: &str,
        width: u32,
    ) -> Option<TextureHandle> {
        let w = [200, 400, 800].into_iter().find(|v| *v >= width).unwrap_or(800);
        let key = (sha.to_string(), w);
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = entries.get_mut(&key) {
            s.used = Instant::now();
            if let Entry::Ready(t, _) = &s.entry {
                return Some(t.clone());
            }
            return None;
        }
        entries.insert(key.clone(), Slot { entry: Entry::Loading, used: Instant::now() });
        drop(entries);
        let (client, map, sem, ctx) = (client.clone(), self.entries.clone(), self.inflight.clone(), ctx.clone());
        rt.spawn(async move {
            let _p = sem.acquire().await;
            let decoded = match client.bytes(&format!("/api/v1/images/{}?w={}", key.0, key.1)).await {
                Ok(bytes) => tokio::task::spawn_blocking(move || {
                    image::load_from_memory(&bytes).ok().map(|img| {
                        let rgba = img.to_rgba8();
                        ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        )
                    })
                })
                .await
                .ok()
                .flatten(),
                Err(_) => None,
            };
            if let Some(s) = map.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&key) {
                s.entry = match decoded {
                    Some(img) => Entry::Decoded(img),
                    None => Entry::Failed,
                };
            }
            ctx.request_repaint();
        });
        None
    }

    /// Uploads decoded images (a few per frame) and evicts over budget. Call once per frame.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut uploaded = 0;
        let mut more = false;
        for (key, s) in entries.iter_mut() {
            if let Entry::Decoded(_) = s.entry {
                if uploaded >= UPLOADS_PER_FRAME {
                    more = true;
                    break;
                }
                let Entry::Decoded(img) = std::mem::replace(&mut s.entry, Entry::Loading) else { continue };
                let size = img.width() * img.height() * 4;
                let tex = ctx.load_texture(format!("art-{}-{}", key.0, key.1), img, TextureOptions::LINEAR);
                s.entry = Entry::Ready(tex, size);
                self.bytes += size;
                uploaded += 1;
            }
        }
        if more {
            ctx.request_repaint();
        }
        if self.bytes > BUDGET {
            let mut ready: Vec<((String, u32), Instant, usize)> = entries
                .iter()
                .filter_map(|(k, s)| match s.entry {
                    Entry::Ready(_, n) => Some((k.clone(), s.used, n)),
                    _ => None,
                })
                .collect();
            ready.sort_by_key(|r| r.1);
            for (k, _, n) in ready {
                if self.bytes <= BUDGET * 3 / 4 {
                    break;
                }
                entries.remove(&k);
                self.bytes -= n;
            }
        }
        // Failed entries are retried after a while.
        entries.retain(|_, s| !matches!(s.entry, Entry::Failed) || s.used.elapsed().as_secs() < 300);
    }

    /// A tiny texture from a blurhash (placeholder while the image loads).
    pub fn blurhash(&mut self, ctx: &egui::Context, hash: &str) -> Option<TextureHandle> {
        if hash.is_empty() {
            return None;
        }
        if let Some(t) = self.blur.get(hash) {
            return Some(t.clone());
        }
        let img = decode_blurhash(hash, 16, 24)?;
        let t = ctx.load_texture(format!("blur-{hash}"), img, TextureOptions::LINEAR);
        if self.blur.len() > 2000 {
            self.blur.clear();
        }
        self.blur.insert(hash.to_string(), t.clone());
        Some(t)
    }
}

fn base83(s: &str) -> Option<u32> {
    const CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";
    s.chars().try_fold(0u32, |acc, c| CHARS.find(c).map(|i| acc * 83 + i as u32))
}

fn srgb_to_linear(v: u32) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Decodes a blurhash into a `w`×`h` image (https://blurha.sh).
pub fn decode_blurhash(hash: &str, w: usize, h: usize) -> Option<ColorImage> {
    if hash.len() < 6 || !hash.is_ascii() {
        return None;
    }
    let size = base83(&hash[0..1])?;
    let (nx, ny) = ((size % 9 + 1) as usize, (size / 9 + 1) as usize);
    if hash.len() != 4 + 2 * nx * ny {
        return None;
    }
    let max = (base83(&hash[1..2])? as f32 + 1.0) / 166.0;
    let mut colors = Vec::with_capacity(nx * ny);
    let dc = base83(&hash[2..6])?;
    colors.push([srgb_to_linear(dc >> 16), srgb_to_linear((dc >> 8) & 255), srgb_to_linear(dc & 255)]);
    for i in 1..nx * ny {
        let v = base83(&hash[4 + i * 2..6 + i * 2])?;
        let q = |x: u32| {
            let f = (x as f32 - 9.0) / 9.0;
            f.signum() * f * f * max
        };
        colors.push([q(v / (19 * 19)), q((v / 19) % 19), q(v % 19)]);
    }
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut c = [0.0f32; 3];
            for j in 0..ny {
                for i in 0..nx {
                    let basis = (std::f32::consts::PI * x as f32 * i as f32 / w as f32).cos()
                        * (std::f32::consts::PI * y as f32 * j as f32 / h as f32).cos();
                    let col = colors[i + j * nx];
                    c[0] += col[0] * basis;
                    c[1] += col[1] * basis;
                    c[2] += col[2] * basis;
                }
            }
            px.push(egui::Color32::from_rgb(linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2])));
        }
    }
    Some(ColorImage { size: [w, h], pixels: px, source_size: egui::vec2(w as f32, h as f32) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_reference_hash() {
        // The example from blurha.sh.
        let img = decode_blurhash("LEHV6nWB2yk8pyo0adR*.7kCMdnj", 8, 8).unwrap();
        assert_eq!(img.size, [8, 8]);
        assert!(decode_blurhash("short", 8, 8).is_none());
        assert!(decode_blurhash("LEHV6nWB2yk8pyo0adR*.7kCMdn", 8, 8).is_none(), "wrong length");
    }
}
