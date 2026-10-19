//! Intro detection by audio fingerprint: episodes of a season share their opening theme. The
//! first minutes of each are decoded to 8 kHz mono, fingerprinted (16 bits per 128 ms from band
//! energy changes, after Haitsma & Kalker), and two neighbours are compared at every alignment;
//! the longest stretch they share (15 s to 2.5 min) is the intro.

use std::path::{Path, PathBuf};
use std::time::Duration;

use corrent_mpv::encode::EncodeJob;

pub const RATE: u32 = 8000;
const FRAME: usize = 2048;
const HOP: usize = 1024;
const BANDS: usize = 17;
/// Shortest and longest intro accepted (seconds).
const MIN_S: f64 = 15.0;
const MAX_S: f64 = 150.0;

/// Seconds per fingerprint frame.
pub fn frame_secs() -> f64 {
    HOP as f64 / RATE as f64
}

/// Decodes the first `secs` of `input`'s audio to 8 kHz mono samples (through the worker when
/// there is one).
pub async fn decode(input: &str, secs: u32, tmp: &Path, worker: Option<PathBuf>) -> Result<Vec<i16>, String> {
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let wav = tmp.join(format!("intro-{}.wav", std::process::id() as u64 * 1_000_000 + rand_u32() as u64));
    let job = EncodeJob {
        input: input.to_string(),
        output: wav.display().to_string(),
        options: vec![
            ("of".into(), "wav".into()),
            ("oac".into(), "pcm_s16le".into()),
            ("vid".into(), "no".into()),
            ("sid".into(), "no".into()),
            ("length".into(), secs.to_string()),
            ("audio-channels".into(), "mono".into()),
            ("audio-samplerate".into(), RATE.to_string()),
            ("audio-format".into(), "s16".into()),
        ],
        audio_file: None,
    };
    let ok = match &worker {
        Some(w) => {
            let file = wav.with_extension("json");
            std::fs::write(&file, serde_json::to_vec(&job).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let status = tokio::process::Command::new(w)
                .arg("encode")
                .arg(&file)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status();
            let r = tokio::time::timeout(Duration::from_secs(600), status).await;
            let _ = std::fs::remove_file(&file);
            matches!(r, Ok(Ok(s)) if s.success())
        }
        None => tokio::task::spawn_blocking(move || {
            corrent_mpv::encode::run(&job, Duration::from_secs(600), &mut |_| true).is_ok()
        })
        .await
        .unwrap_or(false),
    };
    let bytes = std::fs::read(&wav);
    let _ = std::fs::remove_file(&wav);
    if !ok {
        return Err("audio decoding failed".into());
    }
    wav_samples(&bytes.map_err(|e| e.to_string())?).ok_or_else(|| "not a 16-bit WAV".to_string())
}

/// A number for a temporary file name (unique within the process).
fn rand_u32() -> u32 {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The samples of a 16-bit PCM WAV (mono, or the first channel).
pub fn wav_samples(b: &[u8]) -> Option<Vec<i16>> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut pos, mut channels, mut bits) = (12, 1u16, 16u16);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            channels = u16::from_le_bytes(b[body + 2..body + 4].try_into().ok()?).max(1);
            bits = u16::from_le_bytes(b[body + 14..body + 16].try_into().ok()?);
        } else if id == b"data" {
            if bits != 16 {
                return None;
            }
            let end = (body + len).min(b.len());
            let step = 2 * channels as usize;
            return Some(b[body..end].chunks_exact(step).map(|c| i16::from_le_bytes([c[0], c[1]])).collect());
        }
        pos = body + len + (len & 1);
    }
    None
}

/// In-place radix-2 FFT of (re, im).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// 16-bit fingerprints, one per 128 ms.
pub fn fingerprint(samples: &[i16]) -> Vec<u16> {
    if samples.len() < FRAME {
        return Vec::new();
    }
    // Band edges, log-spaced 300–2000 Hz (as FFT bins).
    let edges: Vec<usize> = (0..=BANDS)
        .map(|i| {
            let f = 300.0 * (2000.0f64 / 300.0).powf(i as f64 / BANDS as f64);
            (f * FRAME as f64 / RATE as f64) as usize
        })
        .collect();
    let window: Vec<f32> =
        (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME - 1) as f32).cos()).collect();
    let mut prev = [0f32; BANDS];
    let mut out = Vec::with_capacity(samples.len() / HOP);
    let (mut re, mut im) = (vec![0f32; FRAME], vec![0f32; FRAME]);
    for (n, start) in (0..=samples.len() - FRAME).step_by(HOP).enumerate() {
        for i in 0..FRAME {
            re[i] = samples[start + i] as f32 * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut e = [0f32; BANDS];
        for (b, eb) in e.iter_mut().enumerate() {
            let s: f32 = (edges[b]..edges[b + 1].max(edges[b] + 1)).map(|k| re[k] * re[k] + im[k] * im[k]).sum();
            *eb = (s + 1.0).ln();
        }
        if n > 0 {
            let mut bits = 0u16;
            for m in 0..16 {
                if (e[m] - e[m + 1]) - (prev[m] - prev[m + 1]) > 0.0 {
                    bits |= 1 << m;
                }
            }
            out.push(bits);
        }
        prev = e;
    }
    out
}

/// The stretch two fingerprints share: frame ranges `(a_start, a_end, b_start, b_end)`.
// Two arrays at an offset: indexing reads clearer than zipped iterators.
#[allow(clippy::needless_range_loop)]
pub fn common(a: &[u16], b: &[u16]) -> Option<(usize, usize, usize, usize)> {
    let min = (MIN_S / frame_secs()) as usize;
    let max = (MAX_S / frame_secs()) as usize;
    if a.len() < min || b.len() < min {
        return None;
    }
    let mut best: Option<(usize, usize, usize)> = None; // (len, a_start, offset+)
    for d in -(b.len() as isize) + 1..a.len() as isize {
        // a[i] lines up with b[i - d].
        let (lo, hi) = ((d.max(0)) as usize, (a.len() as isize).min(b.len() as isize + d) as usize);
        let (mut run_start, mut gap, mut last_match) = (None::<usize>, 0usize, 0usize);
        for i in lo..hi {
            let j = (i as isize - d) as usize;
            let ok = (a[i] ^ b[j]).count_ones() <= 4;
            if ok {
                if run_start.is_none() {
                    run_start = Some(i);
                }
                last_match = i;
                gap = 0;
            } else if run_start.is_some() {
                gap += 1;
                if gap > 6 {
                    let s = run_start.take().unwrap_or(i);
                    let len = last_match + 1 - s;
                    if len >= min && best.is_none_or(|b| len > b.0) {
                        best = Some((len, s, (d + b.len() as isize) as usize));
                    }
                    gap = 0;
                }
            }
        }
        if let Some(s) = run_start {
            let len = last_match + 1 - s;
            if len >= min && best.is_none_or(|b| len > b.0) {
                best = Some((len, s, (d + b.len() as isize) as usize));
            }
        }
    }
    let (len, s, dd) = best?;
    let len = len.min(max);
    let d = dd as isize - b.len() as isize;
    let bs = (s as isize - d) as usize;
    Some((s, s + len, bs, bs + len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: u64, secs: f64) -> Vec<i16> {
        let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (0..(secs * RATE as f64) as usize)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                // Noise shaped by a slow beat so bands change over time.
                let env = 0.5 + 0.5 * ((i as f64 / RATE as f64) * 1.7 + seed as f64).sin();
                ((x % 20000) as f64 - 10000.0) as f32 as f64 * env
            })
            .map(|v| v as i16)
            .collect()
    }

    #[test]
    fn finds_the_shared_theme() {
        let theme = noise(42, 30.0);
        let a: Vec<i16> = [noise(1, 10.0), theme.clone(), noise(2, 40.0)].concat();
        let b: Vec<i16> = [noise(3, 25.0), theme, noise(4, 40.0)].concat();
        let (fa, fb) = (fingerprint(&a), fingerprint(&b));
        let (as_, ae, bs, be) = common(&fa, &fb).expect("found");
        let t = frame_secs();
        let (sa, ea, sb, eb) = (as_ as f64 * t, ae as f64 * t, bs as f64 * t, be as f64 * t);
        assert!((sa - 10.0).abs() < 1.5 && (ea - 40.0).abs() < 1.5, "a: {sa:.1}–{ea:.1}");
        assert!((sb - 25.0).abs() < 1.5 && (eb - 55.0).abs() < 1.5, "b: {sb:.1}–{eb:.1}");
        // Nothing shared: nothing found.
        let c: Vec<i16> = noise(9, 60.0);
        assert!(common(&fa, &fingerprint(&c)).is_none());
    }

    #[test]
    fn wav_reading() {
        let mut w = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&[1, 0, 1, 0]);
        w.extend_from_slice(&8000u32.to_le_bytes());
        w.extend_from_slice(&16000u32.to_le_bytes());
        w.extend_from_slice(&[2, 0, 16, 0]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&[1, 0, 0xff, 0xff]);
        assert_eq!(wav_samples(&w), Some(vec![1, -1]));
        assert_eq!(wav_samples(b"nope"), None);
    }
}
