//! Probing media files: through the `corrent-mpvw` worker process when it is installed next to
//! the server (a crashing demuxer can't take the server down), else libmpv in-process, else
//! `ffprobe` when that is installed (`CORRENT_FFPROBE` or on the `PATH`).

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
            None if !corrent_mpv::Library::available() && ffprobe_path().is_some() => self.ffprobe(path).await,
            None => {
                let p = path.display().to_string();
                let t = self.timeout;
                tokio::task::spawn_blocking(move || corrent_mpv::probe(&p, t).map_err(|e| e.to_string()))
                    .await
                    .map_err(|e| e.to_string())?
            }
        }
    }

    async fn ffprobe(&self, path: &Path) -> Result<ProbeResult, String> {
        let bin = ffprobe_path().ok_or("ffprobe isn't installed")?;
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", "-show_chapters"])
            .arg(path)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        let out = tokio::time::timeout(self.timeout, cmd.output())
            .await
            .map_err(|_| "probe timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("ffprobe failed").to_string());
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        Ok(from_ffprobe(&v))
    }
}

/// `ffprobe` from `CORRENT_FFPROBE` or the `PATH` (looked up once).
pub fn ffprobe_path() -> Option<PathBuf> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(find_ffprobe).clone()
}

fn find_ffprobe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CORRENT_FFPROBE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let exe = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// ffprobe's JSON as the probe result mpv would give (track ids counted per kind, as mpv does).
pub fn from_ffprobe(v: &serde_json::Value) -> ProbeResult {
    use corrent_mpv::probe::{Chapter, Track, TrackKind};
    let num = |x: &serde_json::Value| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok()));
    let float = |x: &serde_json::Value| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()));
    let fmt = &v["format"];
    let mut tags = std::collections::BTreeMap::new();
    for (k, val) in fmt["tags"].as_object().into_iter().flatten() {
        if let Some(s) = val.as_str() {
            tags.insert(k.to_lowercase(), s.to_string());
        }
    }
    let mut counts = [0i64; 3];
    let mut tracks = Vec::new();
    for s in v["streams"].as_array().into_iter().flatten() {
        let (kind, slot) = match s["codec_type"].as_str() {
            Some("video") => (TrackKind::Video, 0),
            Some("audio") => (TrackKind::Audio, 1),
            Some("subtitle") => (TrackKind::Sub, 2),
            _ => continue,
        };
        counts[slot] += 1;
        let t = &s["tags"];
        let disp = &s["disposition"];
        let fps = s["avg_frame_rate"].as_str().and_then(|r| {
            let (a, b) = r.split_once('/')?;
            let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
            (b > 0.0 && a > 0.0).then(|| a / b)
        });
        tracks.push(Track {
            id: counts[slot],
            kind: Some(kind),
            codec: s["codec_name"].as_str().map(str::to_string),
            codec_profile: s["profile"].as_str().map(str::to_string),
            title: t["title"].as_str().map(str::to_string),
            lang: t["language"].as_str().map(str::to_string).filter(|l| l != "und"),
            default: num(&disp["default"]) == Some(1),
            forced: num(&disp["forced"]) == Some(1),
            external: false,
            albumart: num(&disp["attached_pic"]) == Some(1),
            width: num(&s["width"]),
            height: num(&s["height"]),
            fps,
            channels: num(&s["channels"]),
            channel_layout: s["channel_layout"].as_str().map(str::to_string),
            sample_rate: num(&s["sample_rate"]),
            bitrate: num(&s["bit_rate"]),
            ff_index: num(&s["index"]),
        });
    }
    let chapters = v["chapters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| Chapter {
            title: c["tags"]["title"].as_str().map(str::to_string),
            start: float(&c["start_time"]).unwrap_or(0.0),
        })
        .collect();
    ProbeResult {
        format: fmt["format_name"].as_str().map(str::to_string),
        duration: float(&fmt["duration"]),
        size: num(&fmt["size"]),
        tracks,
        chapters,
        tags,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_ffprobe_json() {
        let v = serde_json::json!({
            "streams": [
                { "index": 0, "codec_name": "h264", "codec_type": "video", "profile": "High", "width": 1920, "height": 1080,
                  "avg_frame_rate": "24000/1001", "disposition": { "default": 1, "attached_pic": 0 } },
                { "index": 1, "codec_name": "aac", "codec_type": "audio", "channels": 6, "channel_layout": "5.1", "sample_rate": "48000",
                  "disposition": { "default": 1 }, "tags": { "language": "eng" } },
                { "index": 2, "codec_name": "aac", "codec_type": "audio", "channels": 2, "tags": { "language": "tur", "title": "Türkçe" } },
                { "index": 3, "codec_name": "subrip", "codec_type": "subtitle", "disposition": { "forced": 1 }, "tags": { "language": "und" } },
                { "index": 4, "codec_name": "ttf", "codec_type": "attachment" }
            ],
            "chapters": [{ "start_time": "0.000000", "tags": { "title": "Intro" } }, { "start_time": "90.5", "tags": { "title": "Part 1" } }],
            "format": { "format_name": "matroska,webm", "duration": "5400.25", "size": "123456", "tags": { "TITLE": "Sintel" } }
        });
        let r = super::from_ffprobe(&v);
        assert_eq!(r.format.as_deref(), Some("matroska,webm"));
        assert_eq!(r.duration, Some(5400.25));
        assert_eq!(r.tags.get("title").map(String::as_str), Some("Sintel"));
        let audio: Vec<_> = r.audio().collect();
        assert_eq!((audio[0].id, audio[1].id, audio[1].lang.as_deref()), (1, 2, Some("tur")));
        let v0 = r.video().next().unwrap();
        assert!((v0.fps.unwrap() - 23.976).abs() < 0.01 && v0.default);
        let sub = r.subtitles().next().unwrap();
        assert!(sub.forced && sub.lang.is_none());
        assert_eq!(r.chapters[1].start, 90.5);
        assert_eq!(r.tracks.len(), 4);
    }
}
