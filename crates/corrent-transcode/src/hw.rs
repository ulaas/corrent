//! Picks the H.264 encoder: a hardware one when it actually works on this machine (a one-second
//! trial encode), else libx264, else OpenH264.

use std::path::{Path, PathBuf};
use std::time::Duration;

use corrent_mpv::encode::EncodeJob;

/// Tried in this order (VA-API needs a device and upload filters; it is not tried yet).
pub const CANDIDATES: &[&str] = &["h264_nvenc", "h264_qsv", "libx264", "libopenh264"];

async fn run_job(worker: Option<&Path>, job: EncodeJob, dir: &Path) -> bool {
    match worker {
        Some(w) => {
            let file = dir.join("job.json");
            if std::fs::write(&file, serde_json::to_vec(&job).unwrap_or_default()).is_err() {
                return false;
            }
            let cmd = tokio::process::Command::new(w)
                .arg("encode")
                .arg(&file)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .status();
            matches!(tokio::time::timeout(Duration::from_secs(30), cmd).await, Ok(Ok(s)) if s.success())
        }
        None => tokio::task::spawn_blocking(move || {
            corrent_mpv::encode::run(&job, Duration::from_secs(30), &mut |_| true).is_ok()
        })
        .await
        .unwrap_or(false),
    }
}

/// Whether `encoder` can encode here.
pub async fn works(encoder: &str, worker: Option<&Path>, scratch: &Path) -> bool {
    let dir = scratch.join(format!("probe-{encoder}"));
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join("t.mkv");
    let job = EncodeJob {
        input: "av://lavfi:testsrc=duration=1:size=256x144:rate=25".into(),
        output: out.display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), encoder.into()),
            ("vf".into(), "format=yuv420p".into()),
        ],
        audio_file: None,
    };
    let ok = run_job(worker, job, &dir).await && std::fs::metadata(&out).is_ok_and(|m| m.len() > 0);
    let _ = std::fs::remove_dir_all(&dir);
    ok
}

/// The first working encoder of [`CANDIDATES`] (or `preferred` when it works).
pub async fn detect(preferred: Option<&str>, worker: Option<PathBuf>, scratch: &Path) -> Option<String> {
    if !corrent_mpv::Library::available() && worker.is_none() {
        return None;
    }
    for e in preferred.into_iter().chain(CANDIDATES.iter().copied()) {
        if works(e, worker.as_deref(), scratch).await {
            tracing::info!("video transcoding uses {e}");
            return Some(e.to_string());
        }
    }
    tracing::warn!("no working H.264 encoder: video transcoding is unavailable");
    None
}
