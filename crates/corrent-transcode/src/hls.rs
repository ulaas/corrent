//! HLS transcoding sessions.
//!
//! The server owns the playlist: for a file of duration D it publishes a VOD playlist of
//! ⌈D/S⌉ segments of S seconds before anything is encoded. A segment that exists is served; one
//! inside the encoder's reach is waited for; one outside (a seek) stops the encoder and starts a
//! new one at that segment, with `start_number`/`output_ts_offset` keeping the original
//! timeline (verified in docs/spikes/mpv-hls.md). Encoding runs in the `corrent-mpvw` worker
//! process (in-process libmpv when the worker isn't installed). Idle sessions are stopped and
//! their files deleted.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_mpv::encode::EncodeJob;
use serde::{Deserialize, Serialize};

use crate::decide::{AudioTarget, VideoTarget};

/// Segments this far ahead of what the encoder produced are waited for, not restarted.
const AHEAD_SEGMENTS: u64 = 8;
const IDLE: Duration = Duration::from_secs(90);
const SEGMENT_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct HlsConfig {
    /// Sessions live in `<root>/<session id>/`.
    pub root: PathBuf,
    pub segment_secs: u32,
    /// H.264 encoder (`libx264`, `h264_nvenc`, …), see [`crate::hw::detect`].
    pub encoder: String,
    /// libx264 preset.
    pub x264_preset: String,
    pub max_sessions: usize,
    pub worker: Option<PathBuf>,
}

impl Default for HlsConfig {
    fn default() -> Self {
        HlsConfig {
            root: std::env::temp_dir().join("corrent-transcode"),
            segment_secs: 4,
            encoder: "libx264".into(),
            x264_preset: "veryfast".into(),
            max_sessions: 3,
            worker: corrent_mpv::worker_path(),
        }
    }
}

/// What to encode.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSpec {
    /// A path or URL mpv can open.
    pub input: String,
    pub duration_ms: i64,
    pub fps: Option<f64>,
    pub video: Option<VideoTarget>,
    pub audio: Option<AudioTarget>,
    pub burn_subtitle: Option<i64>,
    /// An external subtitle file to burn in (instead of an embedded track).
    pub burn_subtitle_file: Option<String>,
    /// Who plays it (Sessions page) and what.
    pub user: String,
    pub title: String,
    pub item_id: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub user: String,
    pub title: String,
    pub item_id: Option<i64>,
    pub encoder: String,
    pub video: bool,
    pub segments: u64,
    pub produced: u64,
    pub encoding_from: Option<u64>,
    pub idle_secs: u64,
}

enum Encoder {
    /// The task owning the worker process kills it when this fires (or is dropped).
    Process(tokio::sync::oneshot::Sender<()>),
    InProcess(Arc<AtomicBool>),
}

impl Encoder {
    fn stop(self) {
        match self {
            Encoder::Process(kill) => {
                let _ = kill.send(());
            }
            Encoder::InProcess(cancel) => cancel.store(true, Ordering::Relaxed),
        }
    }
}

struct Running {
    start: u64,
    encoder: Encoder,
    /// Set when the job ended (Some(true) = it failed).
    ended: Arc<Mutex<Option<bool>>>,
}

struct Session {
    id: String,
    spec: SessionSpec,
    dir: PathBuf,
    segments: u64,
    done: Mutex<BTreeSet<u64>>,
    running: tokio::sync::Mutex<Option<Running>>,
    last_access: Mutex<Instant>,
}

impl Session {
    fn touch(&self) {
        *self.last_access.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn idle(&self) -> Duration {
        self.last_access.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }

    /// Reads the muxer's own playlist: every segment listed there is complete.
    fn refresh_done(&self) {
        let Ok(text) = std::fs::read_to_string(self.dir.join("index.m3u8")) else { return };
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            if let Some(n) = segment_number(line) {
                done.insert(n);
            }
        }
    }
}

/// `seg00012.ts` → 12.
fn segment_number(name: &str) -> Option<u64> {
    let file = Path::new(name.trim()).file_name()?.to_str()?;
    file.strip_prefix("seg")?.strip_suffix(".ts")?.parse().ok()
}

#[derive(Debug, Clone, PartialEq)]
pub enum HlsError {
    NotFound,
    TooMany,
    Failed(String),
}

impl std::fmt::Display for HlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HlsError::NotFound => write!(f, "no such session or segment"),
            HlsError::TooMany => write!(f, "too many transcodes at once"),
            HlsError::Failed(m) => write!(f, "transcode failed: {m}"),
        }
    }
}

impl std::error::Error for HlsError {}

pub struct HlsManager {
    cfg: Mutex<HlsConfig>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

fn random_id() -> String {
    let mut b = [0u8; 12];
    // Session ids are capabilities only together with the user's authentication; uniqueness
    // is what matters here.
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut x = t as u64 ^ (std::process::id() as u64) << 32;
    for v in b.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *v = x as u8;
    }
    b.iter().map(|v| format!("{v:02x}")).collect()
}

impl HlsManager {
    /// Creates the manager and its reaper (idle sessions are stopped and deleted).
    pub fn new(cfg: HlsConfig) -> Arc<HlsManager> {
        let _ = std::fs::remove_dir_all(&cfg.root);
        let m = Arc::new(HlsManager { cfg: Mutex::new(cfg), sessions: Mutex::new(HashMap::new()) });
        let weak = Arc::downgrade(&m);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let Some(m) = weak.upgrade() else { break };
                let idle: Vec<String> = m
                    .sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .values()
                    .filter(|s| s.idle() > IDLE)
                    .map(|s| s.id.clone())
                    .collect();
                for id in idle {
                    m.stop(&id).await;
                }
            }
        });
        m
    }

    pub fn set_encoder(&self, encoder: &str) {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).encoder = encoder.to_string();
    }

    pub fn config(&self) -> HlsConfig {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn create(&self, spec: SessionSpec) -> Result<String, HlsError> {
        let cfg = self.config();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if sessions.len() >= cfg.max_sessions {
            // Make room by dropping the longest-idle session that has been idle a while.
            let victim = sessions
                .values()
                .filter(|s| s.idle() > Duration::from_secs(20))
                .max_by_key(|s| s.idle())
                .map(|s| s.id.clone());
            match victim {
                Some(v) => {
                    if let Some(s) = sessions.remove(&v) {
                        tokio::spawn(async move { stop_session(s).await });
                    }
                }
                None => return Err(HlsError::TooMany),
            }
        }
        let id = random_id();
        let dir = cfg.root.join(&id);
        std::fs::create_dir_all(&dir).map_err(|e| HlsError::Failed(e.to_string()))?;
        let seg_ms = cfg.segment_secs as i64 * 1000;
        let segments = ((spec.duration_ms.max(1) + seg_ms - 1) / seg_ms) as u64;
        sessions.insert(
            id.clone(),
            Arc::new(Session {
                id: id.clone(),
                spec,
                dir,
                segments,
                done: Mutex::new(BTreeSet::new()),
                running: tokio::sync::Mutex::new(None),
                last_access: Mutex::new(Instant::now()),
            }),
        );
        Ok(id)
    }

    fn get(&self, id: &str) -> Result<Arc<Session>, HlsError> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned().ok_or(HlsError::NotFound)
    }

    /// The owner of a session (the server checks it against the requesting user).
    pub fn owner(&self, id: &str) -> Option<String> {
        self.get(id).ok().map(|s| s.spec.user.clone())
    }

    /// The whole VOD playlist (segments are encoded on demand).
    pub fn playlist(&self, id: &str) -> Result<String, HlsError> {
        let s = self.get(id)?;
        s.touch();
        let seg = self.config().segment_secs as f64;
        let total = s.spec.duration_ms as f64 / 1000.0;
        let mut out = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-INDEPENDENT-SEGMENTS\n",
            seg.ceil() as u64 + 1
        );
        for n in 0..s.segments {
            let len = (total - n as f64 * seg).clamp(0.1, seg);
            out.push_str(&format!("#EXTINF:{len:.3},\n{n}.ts\n"));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        Ok(out)
    }

    /// The file of segment `n`, encoding (or re-positioning the encoder) as needed.
    pub async fn segment(&self, id: &str, n: u64) -> Result<PathBuf, HlsError> {
        let s = self.get(id)?;
        if n >= s.segments {
            return Err(HlsError::NotFound);
        }
        s.touch();
        let path = s.dir.join(format!("seg{n:05}.ts"));
        let cfg = self.config();
        let deadline = Instant::now() + SEGMENT_WAIT;
        loop {
            s.refresh_done();
            if s.done.lock().unwrap_or_else(|e| e.into_inner()).contains(&n) && path.exists() {
                return Ok(path);
            }
            {
                let mut running = s.running.lock().await;
                let produced = s.done.lock().unwrap_or_else(|e| e.into_inner()).range(..=n).next_back().copied();
                let reachable = match running.as_ref() {
                    Some(r) => {
                        let ended = *r.ended.lock().unwrap_or_else(|e| e.into_inner());
                        if ended == Some(true) && n >= r.start {
                            let failed_at = r.start;
                            *running = None;
                            return Err(HlsError::Failed(format!(
                                "the encoder stopped (started at segment {failed_at})"
                            )));
                        }
                        ended.is_none()
                            && n >= r.start
                            && n <= produced.unwrap_or(r.start).max(r.start) + AHEAD_SEGMENTS
                    }
                    None => false,
                };
                if !reachable {
                    if let Some(old) = running.take() {
                        old.encoder.stop();
                    }
                    *running = Some(start_encoder(&cfg, &s, n).map_err(HlsError::Failed)?);
                }
            }
            if Instant::now() >= deadline {
                return Err(HlsError::Failed("segment not ready in time".into()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn stop(&self, id: &str) {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        if let Some(s) = s {
            stop_session(s).await;
        }
    }

    pub fn sessions(&self) -> Vec<SessionInfo> {
        let encoder = self.config().encoder;
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                user: s.spec.user.clone(),
                title: s.spec.title.clone(),
                item_id: s.spec.item_id,
                encoder: encoder.clone(),
                video: s.spec.video.is_some(),
                segments: s.segments,
                produced: s.done.lock().unwrap_or_else(|e| e.into_inner()).len() as u64,
                encoding_from: s.running.try_lock().ok().and_then(|r| r.as_ref().map(|r| r.start)),
                idle_secs: s.idle().as_secs(),
            })
            .collect()
    }
}

async fn stop_session(s: Arc<Session>) {
    if let Some(r) = s.running.lock().await.take() {
        r.encoder.stop();
    }
    let dir = s.dir.clone();
    // Give a killed encoder a moment to close its files.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = tokio::fs::remove_dir_all(dir).await;
}

/// mpv options for encoding from segment `start`.
pub fn job_options(cfg: &HlsConfig, spec: &SessionSpec, dir: &Path, start: u64) -> Vec<(String, String)> {
    let seg = cfg.segment_secs as u64;
    let seg_pattern = dir.join("seg%05d.ts").display().to_string();
    let mut o: Vec<(String, String)> = vec![
        ("start".into(), (start * seg).to_string()),
        ("hr-seek".into(), "yes".into()),
        ("of".into(), "hls".into()),
        (
            "ofopts".into(),
            format!(
                "hls_time={seg},hls_list_size=0,start_number={start},output_ts_offset={},hls_segment_filename=%{}%{seg_pattern}",
                start * seg,
                seg_pattern.len()
            ),
        ),
    ];
    match &spec.video {
        Some(v) => {
            let gop = ((spec.fps.unwrap_or(25.0).clamp(1.0, 240.0)) * seg as f64).round() as i64;
            let br = v.bitrate;
            let common = format!("g={gop},keyint_min={gop},b={br},maxrate={},bufsize={}", br * 3 / 2, br * 2);
            let opts = if cfg.encoder.contains("nvenc") {
                format!("{common},preset=p4,forced-idr=1,no-scenecut=1,profile=high")
            } else if cfg.encoder == "libx264" {
                format!("{common},preset={},profile=high,level=4.1,sc_threshold=0", cfg.x264_preset)
            } else {
                common
            };
            o.push(("ovc".into(), cfg.encoder.clone()));
            o.push(("ovcopts".into(), opts));
            let scale = v.height.map(|h| format!("scale=-2:{h},")).unwrap_or_default();
            o.push(("vf".into(), format!("{scale}format=yuv420p")));
        }
        None => o.push(("vid".into(), "no".into())),
    }
    match &spec.audio {
        Some(a) => {
            o.push(("oac".into(), "aac".into()));
            o.push(("oacopts".into(), format!("b={}", a.bitrate)));
            o.push(("audio-channels".into(), if a.channels <= 2 { "stereo".into() } else { "5.1".into() }));
            if let Some(t) = a.track {
                o.push(("aid".into(), t.to_string()));
            }
        }
        None => o.push(("aid".into(), "no".into())),
    }
    match (&spec.burn_subtitle_file, spec.burn_subtitle) {
        (Some(_), _) => o.push(("sid".into(), "1".into())),
        (None, Some(sid)) => o.push(("sid".into(), sid.to_string())),
        (None, None) => o.push(("sid".into(), "no".into())),
    }
    if let Some(f) = &spec.burn_subtitle_file {
        o.push(("sub-files".into(), f.clone()));
    }
    o
}

fn start_encoder(cfg: &HlsConfig, s: &Session, start: u64) -> Result<Running, String> {
    // A new run rewrites the muxer playlist from `start`; earlier complete segments stay.
    let _ = std::fs::remove_file(s.dir.join("index.m3u8"));
    let job = EncodeJob {
        input: s.spec.input.clone(),
        output: s.dir.join("index.m3u8").display().to_string(),
        options: job_options(cfg, &s.spec, &s.dir, start),
        audio_file: None,
    };
    tracing::debug!("transcode {} from segment {start} with {}", s.id, cfg.encoder);
    let ended: Arc<Mutex<Option<bool>>> = Arc::default();
    let encoder = match &cfg.worker {
        Some(w) => {
            let job_file = s.dir.join(format!("job-{start}.json"));
            std::fs::write(&job_file, serde_json::to_vec(&job).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let mut child = tokio::process::Command::new(w)
                .arg("encode")
                .arg(&job_file)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| e.to_string())?;
            let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
            let e2 = ended.clone();
            tokio::spawn(async move {
                let failed = tokio::select! {
                    status = child.wait() => !status.is_ok_and(|s| s.success()),
                    _ = kill_rx => {
                        let _ = child.kill().await;
                        false
                    }
                };
                *e2.lock().unwrap_or_else(|e| e.into_inner()) = Some(failed);
            });
            Encoder::Process(kill_tx)
        }
        None => {
            let cancel = Arc::new(AtomicBool::new(false));
            let (c2, e2) = (cancel.clone(), ended.clone());
            tokio::task::spawn_blocking(move || {
                let r = corrent_mpv::encode::run(&job, Duration::from_secs(12 * 3600), &mut |_| {
                    !c2.load(Ordering::Relaxed)
                });
                *e2.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.is_err() && !c2.load(Ordering::Relaxed));
            });
            Encoder::InProcess(cancel)
        }
    };
    Ok(Running { start, encoder, ended })
}
