//! Trickplay: small frames every few seconds, tiled into JPEG sheets with a WebVTT index
//! (`sheet-N.jpg#xywh=x,y,w,h`), for seek-bar previews in both clients.

use std::path::{Path, PathBuf};
use std::time::Duration;

use corrent_mpv::encode::EncodeJob;
use image::{GenericImage, RgbImage};

pub const COLUMNS: u32 = 10;
pub const ROWS: u32 = 10;

fn fmt_time(ms: u64) -> String {
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Extracts frames with mpv's encoder (`--of=image2`), then tiles them. Blocking parts run on
/// the pool. Returns the number of frames.
pub async fn generate(
    input: &str,
    out: &Path,
    interval_secs: u32,
    width: u32,
    worker: Option<PathBuf>,
) -> Result<usize, String> {
    let frames = out.join("frames");
    let _ = std::fs::remove_dir_all(&frames);
    std::fs::create_dir_all(&frames).map_err(|e| e.to_string())?;
    let job = EncodeJob {
        input: input.to_string(),
        output: frames.join("f%05d.jpg").display().to_string(),
        options: vec![
            ("of".into(), "image2".into()),
            ("ovc".into(), "mjpeg".into()),
            ("ovcopts".into(), "q=6".into()),
            ("vf".into(), format!("fps=1/{interval_secs},scale={width}:-2,format=yuvj420p")),
            ("aid".into(), "no".into()),
            ("sid".into(), "no".into()),
        ],
        audio_file: None,
    };
    let ok = match &worker {
        Some(w) => {
            let file = out.join("job.json");
            std::fs::write(&file, serde_json::to_vec(&job).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let mut cmd = tokio::process::Command::new(w);
            // No console window flashing up (the worker is a console program).
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let status = cmd
                .arg("encode")
                .arg(&file)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status();
            let r = tokio::time::timeout(Duration::from_secs(3 * 3600), status).await;
            let _ = std::fs::remove_file(&file);
            matches!(r, Ok(Ok(s)) if s.success())
        }
        None => tokio::task::spawn_blocking(move || {
            corrent_mpv::encode::run(&job, Duration::from_secs(3 * 3600), &mut |_| true).is_ok()
        })
        .await
        .unwrap_or(false),
    };
    if !ok {
        let _ = std::fs::remove_dir_all(&frames);
        return Err("frame extraction failed".into());
    }
    let out2 = out.to_path_buf();
    tokio::task::spawn_blocking(move || tile(&frames, &out2, interval_secs)).await.map_err(|e| e.to_string())?
}

fn tile(frames: &Path, out: &Path, interval_secs: u32) -> Result<usize, String> {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(frames).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    files.sort();
    if files.is_empty() {
        return Err("no frames".into());
    }
    let first = image::open(&files[0]).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = first.dimensions();
    let per_sheet = (COLUMNS * ROWS) as usize;
    let mut vtt = String::from("WEBVTT\n\n");
    for (sheet_no, chunk) in files.chunks(per_sheet).enumerate() {
        let rows = (chunk.len() as u32).div_ceil(COLUMNS);
        let mut sheet = RgbImage::new(w * COLUMNS.min(chunk.len() as u32), h * rows);
        for (i, f) in chunk.iter().enumerate() {
            let img = image::open(f).map_err(|e| e.to_string())?.to_rgb8();
            let (x, y) = ((i as u32 % COLUMNS) * w, (i as u32 / COLUMNS) * h);
            if img.dimensions() == (w, h) {
                sheet.copy_from(&img, x, y).map_err(|e| e.to_string())?;
            }
            let idx = (sheet_no * per_sheet + i) as u64;
            let start = idx * interval_secs as u64 * 1000;
            vtt.push_str(&format!(
                "{} --> {}\nsheet-{sheet_no}.jpg#xywh={x},{y},{w},{h}\n\n",
                fmt_time(start),
                fmt_time(start + interval_secs as u64 * 1000)
            ));
        }
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 75)
            .encode_image(&sheet)
            .map_err(|e| e.to_string())?;
        std::fs::write(out.join(format!("sheet-{sheet_no}.jpg")), buf).map_err(|e| e.to_string())?;
    }
    std::fs::write(out.join("index.vtt"), vtt).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(frames);
    Ok(files.len())
}
