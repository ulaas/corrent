//! Probing media files: through the `corrent-mpvw` worker process when it is installed next to
//! the server (a crashing demuxer can't take the server down), else libmpv in-process.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use corrent_mpv::ProbeResult;

#[derive(Clone)]
pub struct Prober {
    worker: Option<PathBuf>,
    permits: Arc<tokio::sync::Semaphore>,
    timeout: Duration,
}

impl Prober {
    pub fn new(worker: Option<PathBuf>, parallel: usize) -> Prober {
        Prober {
            worker,
            permits: Arc::new(tokio::sync::Semaphore::new(parallel.max(1))),
            timeout: Duration::from_secs(45),
        }
    }

    /// Finds `corrent-mpvw` (see [`corrent_mpv::worker_path`]).
    pub fn find_worker() -> Option<PathBuf> {
        corrent_mpv::worker_path()
    }

    pub async fn probe(&self, path: &Path) -> Result<ProbeResult, String> {
        let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
        match &self.worker {
            Some(w) => {
                let mut cmd = tokio::process::Command::new(w);
                cmd.arg("probe").arg(path).kill_on_drop(true).stdin(std::process::Stdio::null());
                let out = tokio::time::timeout(self.timeout, cmd.output())
                    .await
                    .map_err(|_| "probe timed out".to_string())?
                    .map_err(|e| e.to_string())?;
                let line = String::from_utf8_lossy(&out.stdout);
                let line = line.lines().last().unwrap_or("");
                if let Ok(r) = serde_json::from_str::<ProbeResult>(line)
                    && out.status.success()
                {
                    return Ok(r);
                }
                let err = serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("probe worker failed ({})", out.status));
                Err(err)
            }
            None => {
                let p = path.display().to_string();
                let t = self.timeout;
                tokio::task::spawn_blocking(move || corrent_mpv::probe(&p, t).map_err(|e| e.to_string()))
                    .await
                    .map_err(|e| e.to_string())?
            }
        }
    }
}
