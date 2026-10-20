//! One frame of a video as a PNG: the thumbnail of a video without artwork (the artwork store
//! makes its JPEG sizes).

use std::path::{Path, PathBuf};
use std::time::Duration;

use corrent_mpv::encode::EncodeJob;

/// Where in a video its thumbnail is taken: 15 % in (20 % for an episode, past the intro), at
/// least 5 s in and at least 1 s before the end; 30 s when the length is unknown.
pub fn position_secs(runtime_ms: Option<i64>, episode: bool) -> f64 {
    let Some(len) = runtime_ms.filter(|ms| *ms > 0).map(|ms| ms as f64 / 1000.0) else { return 30.0 };
    let at = len * if episode { 0.20 } else { 0.15 };
    at.max(5.0).min((len - 1.0).max(0.0))
}

/// The frame at `at_secs` of `input`, scaled to `width` pixels wide, as PNG bytes. `tmp` is a
/// folder of its own for the job (emptied afterwards).
pub async fn extract(
    input: &str,
    at_secs: f64,
    width: u32,
    worker: Option<PathBuf>,
    tmp: &Path,
) -> Result<Vec<u8>, String> {
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let out = tmp.join("frame.png");
    let _ = std::fs::remove_file(&out);
    let job = EncodeJob {
        input: input.to_string(),
        output: out.display().to_string(),
        options: vec![
            ("start".into(), format!("{at_secs:.3}")),
            ("frames".into(), "1".into()),
            // PNG: some libmpv builds have no MJPEG encoder (the Windows one we bundle).
            ("of".into(), "image2".into()),
            ("ovc".into(), "png".into()),
            ("vf".into(), format!("scale={width}:-2,format=rgb24")),
            ("aid".into(), "no".into()),
            ("sid".into(), "no".into()),
        ],
        audio_file: None,
    };
    let ok = crate::trickplay::run_job(job, worker.as_deref(), &tmp.join("job.json"), Duration::from_secs(60)).await;
    let bytes = std::fs::read(&out).ok().filter(|b| !b.is_empty());
    let _ = std::fs::remove_dir_all(tmp);
    match bytes {
        Some(b) if ok => Ok(b),
        _ => Err("no frame".into()),
    }
}
