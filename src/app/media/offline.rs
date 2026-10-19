//! Offline copies (remote servers): items downloaded to this computer to play without the
//! server — the original file (resumable Range download) or a smaller copy (the server's
//! 720p HLS transcode, its segments joined into one MPEG-TS file, resumable by segment).
//! Watch progress made while the server is unreachable is kept and sent when it is back,
//! unless the server saw newer progress for that item in the meantime.
//!
//! Layout: `<data>/offline/index.json` and `<data>/offline/<n>/<file>`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corrent_proto::library::{Item, PlaybackInfo};
use serde::{Deserialize, Serialize};

use super::client::MediaClient;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Original,
    /// The 720p transcode.
    Smaller,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "error")]
pub enum State {
    Queued,
    Running,
    Paused,
    Done,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Download {
    /// Folder name under `offline/`.
    pub key: u64,
    pub server: String,
    pub item: Item,
    pub quality: Quality,
    pub file: String,
    /// Bytes (original) or segments (smaller) done, of `total` (0: unknown yet).
    pub done: u64,
    pub total: u64,
    pub state: State,
    pub added_at: i64,
}

/// Progress made offline, to send later.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingState {
    pub server: String,
    pub item_id: i64,
    pub body: serde_json::Value,
    /// When it happened (Unix seconds).
    pub at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    downloads: Vec<Download>,
    #[serde(default)]
    pending: Vec<PendingState>,
    #[serde(default)]
    next_key: u64,
}

pub struct Store {
    dir: PathBuf,
    index: Mutex<Index>,
    /// Keys of downloads with a running task.
    running: Mutex<std::collections::HashSet<u64>>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A file name from an item (no path separators or reserved characters).
pub fn file_name(title: &str, ext: &str) -> String {
    let clean: String = title
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(120)
        .collect();
    let stem = if clean.is_empty() { "item".to_string() } else { clean };
    format!("{stem}.{ext}")
}

/// Segment names of an HLS playlist, in order.
pub fn segments(playlist: &str) -> Vec<String> {
    playlist.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect()
}

impl Store {
    pub fn open(data_dir: &Path) -> Arc<Store> {
        let dir = data_dir.join("offline");
        let index = std::fs::read(dir.join("index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
            .unwrap_or_default();
        let s = Store { dir, index: Mutex::new(index), running: Default::default() };
        // Downloads that were running when the app closed continue when asked to.
        {
            let mut i = s.index.lock().unwrap_or_else(|e| e.into_inner());
            for d in &mut i.downloads {
                if d.state == State::Running {
                    d.state = State::Queued;
                }
            }
        }
        Arc::new(s)
    }

    fn save(&self, i: &Index) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(b) = serde_json::to_vec_pretty(i)
            && let Err(e) = corrent_core::util::write_atomic(&self.dir.join("index.json"), &b)
        {
            tracing::warn!("offline index: {e}");
        }
    }

    fn update(&self, key: u64, f: impl FnOnce(&mut Download)) {
        let mut i = self.index.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = i.downloads.iter_mut().find(|d| d.key == key) {
            f(d);
        }
        self.save(&i);
    }

    pub fn downloads(&self, server: &str) -> Vec<Download> {
        let i = self.index.lock().unwrap_or_else(|e| e.into_inner());
        i.downloads.iter().filter(|d| d.server == server).cloned().collect()
    }

    pub fn find(&self, server: &str, item: i64) -> Option<Download> {
        let i = self.index.lock().unwrap_or_else(|e| e.into_inner());
        i.downloads.iter().find(|d| d.server == server && d.item.id == item).cloned()
    }

    /// The local file of a finished download.
    pub fn path(&self, d: &Download) -> PathBuf {
        self.dir.join(d.key.to_string()).join(&d.file)
    }

    /// Queues `item` (no-op when it's already there) and starts it.
    pub fn add(
        self: &Arc<Self>,
        client: &MediaClient,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        item: Item,
        quality: Quality,
    ) {
        let key = {
            let mut i = self.index.lock().unwrap_or_else(|e| e.into_inner());
            if i.downloads.iter().any(|d| d.server == client.base() && d.item.id == item.id) {
                return;
            }
            i.next_key += 1;
            let key = i.next_key;
            i.downloads.push(Download {
                key,
                server: client.base().to_string(),
                item,
                quality,
                file: String::new(),
                done: 0,
                total: 0,
                state: State::Queued,
                added_at: now(),
            });
            self.save(&i);
            key
        };
        self.start(client, rt, ctx, key);
    }

    pub fn pause(&self, key: u64) {
        self.update(key, |d| {
            if matches!(d.state, State::Queued | State::Running) {
                d.state = State::Paused;
            }
        });
    }

    pub fn remove(&self, key: u64) {
        let mut i = self.index.lock().unwrap_or_else(|e| e.into_inner());
        i.downloads.retain(|d| d.key != key);
        self.save(&i);
        drop(i);
        let _ = std::fs::remove_dir_all(self.dir.join(key.to_string()));
    }

    /// Runs (or resumes) a download.
    pub fn start(self: &Arc<Self>, client: &MediaClient, rt: &tokio::runtime::Handle, ctx: &egui::Context, key: u64) {
        if !self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(key) {
            return;
        }
        self.update(key, |d| d.state = State::Running);
        let (me, c, ctx) = (self.clone(), client.clone(), ctx.clone());
        rt.spawn(async move {
            let r = me.run(&c, key, &ctx).await;
            me.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
            me.update(key, |d| match r {
                Ok(true) => d.state = State::Done,
                Ok(false) => {}
                Err(e) => d.state = State::Failed(e),
            });
            ctx.request_repaint();
        });
    }

    fn paused(&self, key: u64) -> bool {
        let i = self.index.lock().unwrap_or_else(|e| e.into_inner());
        i.downloads.iter().find(|d| d.key == key).is_none_or(|d| d.state == State::Paused)
    }

    /// `Ok(true)`: finished; `Ok(false)`: paused or removed.
    async fn run(&self, c: &MediaClient, key: u64, ctx: &egui::Context) -> Result<bool, String> {
        let Some(d) =
            self.index.lock().unwrap_or_else(|e| e.into_inner()).downloads.iter().find(|d| d.key == key).cloned()
        else {
            return Ok(false);
        };
        let folder = self.dir.join(key.to_string());
        tokio::fs::create_dir_all(&folder).await.map_err(|e| e.to_string())?;
        match d.quality {
            Quality::Original => {
                let info: PlaybackInfo = c
                    .send(
                        reqwest::Method::POST,
                        &format!("/api/v1/playback/{}", d.item.id),
                        &serde_json::json!({ "profile": "native" }),
                    )
                    .await?;
                if info.downloading {
                    return Err(t!("The file is still downloading on the server.").to_string());
                }
                let file = if d.file.is_empty() {
                    let ext = info.url.rsplit_once('.').map(|(_, e)| e).filter(|e| e.len() <= 5 && !e.contains('/'));
                    let ext = match ext {
                        Some(e) => e.to_string(),
                        None => self.extension(c, d.item.id).await.unwrap_or_else(|| "mkv".into()),
                    };
                    let f = file_name(&d.item.title, &ext);
                    self.update(key, |x| x.file = f.clone());
                    f
                } else {
                    d.file.clone()
                };
                let path = folder.join(&file);
                let have = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
                let mut res = c.download(&info.url, have).await?;
                // A server that ignored the range sends everything again.
                let resumed = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
                let start = if resumed { have } else { 0 };
                let total = res.content_length().map(|l| l + start).unwrap_or(0);
                self.update(key, |x| (x.total, x.done) = (total, start));
                let mut out = tokio::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .append(resumed)
                    .truncate(!resumed)
                    .open(&path)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut done = start;
                let mut last = std::time::Instant::now();
                use tokio::io::AsyncWriteExt;
                while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
                    out.write_all(&chunk).await.map_err(|e| e.to_string())?;
                    done += chunk.len() as u64;
                    if last.elapsed() > Duration::from_millis(500) {
                        last = std::time::Instant::now();
                        self.update(key, |x| x.done = done);
                        ctx.request_repaint();
                        if self.paused(key) {
                            out.flush().await.map_err(|e| e.to_string())?;
                            return Ok(false);
                        }
                    }
                }
                out.flush().await.map_err(|e| e.to_string())?;
                self.update(key, |x| (x.done, x.total) = (done, done));
                Ok(true)
            }
            Quality::Smaller => {
                let info: PlaybackInfo = c
                    .send(
                        reqwest::Method::POST,
                        &format!("/api/v1/playback/{}", d.item.id),
                        &serde_json::json!({ "profile": "web", "force_transcode": true, "max_height": 720, "max_bitrate": 3_000_000 }),
                    )
                    .await?;
                let session = info.session_id.clone();
                let r = self.segments(c, key, &d, &folder, &info, ctx).await;
                if let Some(s) = session {
                    let _ = c
                        .send::<serde_json::Value, _>(
                            reqwest::Method::DELETE,
                            &format!("/api/v1/playback/sessions/{s}"),
                            &serde_json::json!({}),
                        )
                        .await;
                }
                r
            }
        }
    }

    async fn extension(&self, c: &MediaClient, item: i64) -> Option<String> {
        let d: corrent_proto::library::ItemDetail = c.get(&format!("/api/v1/items/{item}")).await.ok()?;
        let f = d.files.iter().find(|f| f.role == "main")?;
        Path::new(&f.path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
    }

    async fn segments(
        &self,
        c: &MediaClient,
        key: u64,
        d: &Download,
        folder: &Path,
        info: &PlaybackInfo,
        ctx: &egui::Context,
    ) -> Result<bool, String> {
        if info.method != "hls" {
            return Err(t!("The server can't make a smaller copy of this.").to_string());
        }
        let playlist = c.text(&info.url).await?;
        let names = segments(&playlist);
        let base = info.url.rsplit_once('/').map(|(b, _)| b).unwrap_or("");
        let file = file_name(&d.item.title, "ts");
        let path = folder.join(&file);
        // Resume at the segment after the last complete one (the file is cut back to it).
        let mut done = d.done.min(names.len() as u64);
        let sizes = folder.join("segments.json");
        let mut ends: Vec<u64> =
            std::fs::read(&sizes).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        ends.truncate(done as usize);
        done = ends.len() as u64;
        let keep = ends.last().copied().unwrap_or(0);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| e.to_string())?;
        out.set_len(keep).map_err(|e| e.to_string())?;
        drop(out);
        self.update(key, |x| (x.file, x.total, x.done) = (file.clone(), names.len() as u64, done));
        let mut out = tokio::fs::OpenOptions::new().append(true).open(&path).await.map_err(|e| e.to_string())?;
        use tokio::io::AsyncWriteExt;
        for (n, name) in names.iter().enumerate().skip(done as usize) {
            if self.paused(key) {
                return Ok(false);
            }
            let bytes = c.bytes(&format!("{base}/{name}")).await?;
            out.write_all(&bytes).await.map_err(|e| e.to_string())?;
            out.flush().await.map_err(|e| e.to_string())?;
            ends.push(ends.last().copied().unwrap_or(0) + bytes.len() as u64);
            let _ = std::fs::write(&sizes, serde_json::to_vec(&ends).unwrap_or_default());
            self.update(key, |x| x.done = n as u64 + 1);
            ctx.request_repaint();
        }
        let _ = std::fs::remove_file(&sizes);
        Ok(true)
    }

    /// Remembers progress the server didn't get.
    pub fn queue_state(&self, server: &str, item_id: i64, body: serde_json::Value) {
        let mut i = self.index.lock().unwrap_or_else(|e| e.into_inner());
        // Only the latest progress per item matters.
        i.pending.retain(|p| !(p.server == server && p.item_id == item_id));
        i.pending.push(PendingState { server: server.to_string(), item_id, body, at: now() });
        self.save(&i);
    }

    pub fn has_pending(&self, server: &str) -> bool {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).pending.iter().any(|p| p.server == server)
    }

    /// Sends queued progress; entries the server has newer progress for are dropped.
    pub async fn flush(&self, c: &MediaClient) {
        let list: Vec<PendingState> = {
            let i = self.index.lock().unwrap_or_else(|e| e.into_inner());
            i.pending.iter().filter(|p| p.server == c.base()).cloned().collect()
        };
        for p in list {
            let current: Result<corrent_proto::library::ItemDetail, String> =
                c.get(&format!("/api/v1/items/{}", p.item_id)).await;
            let send = match &current {
                Ok(d) => keep_local(p.at, d.item.state.last_played_at),
                // Gone from the server: nothing to send it to.
                Err(e) if e.contains("404") || e.to_lowercase().contains("not found") => false,
                // Still unreachable: try later.
                Err(_) => return,
            };
            if send
                && c.send::<serde_json::Value, _>(
                    reqwest::Method::PUT,
                    &format!("/api/v1/items/{}/state", p.item_id),
                    &p.body,
                )
                .await
                .is_err()
            {
                return;
            }
            let mut i = self.index.lock().unwrap_or_else(|e| e.into_inner());
            i.pending.retain(|x| x != &p);
            self.save(&i);
        }
    }
}

/// Offline progress wins unless the server saw the item played later.
pub fn keep_local(local_at: i64, server_last_played: Option<i64>) -> bool {
    server_last_played.is_none_or(|s| s <= local_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_playlists() {
        assert_eq!(file_name("Sintel: Director's Cut?", "mkv"), "Sintel_ Director's Cut_.mkv");
        assert_eq!(file_name("  ..", "mp4"), "item.mp4");
        assert_eq!(file_name("a/b\\c", "ts"), "a_b_c.ts");
        let pl = "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:4.000,\n0.ts\n#EXTINF:4.000,\n1.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(segments(pl), vec!["0.ts", "1.ts"]);
    }

    #[test]
    fn newer_server_progress_wins() {
        assert!(keep_local(100, None));
        assert!(keep_local(100, Some(90)));
        assert!(!keep_local(100, Some(150)));
    }

    #[test]
    fn index_survives_restart_and_queues_running() {
        let dir = std::env::temp_dir().join(format!("corrent-offline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Store::open(&dir);
        {
            let mut i = s.index.lock().unwrap();
            i.next_key = 1;
            i.downloads.push(Download {
                key: 1,
                server: "https://s".into(),
                item: Item {
                    id: 5,
                    library_id: 1,
                    kind: corrent_proto::library::ItemKind::Movie,
                    parent_id: None,
                    title: "Sintel".into(),
                    year: None,
                    index: None,
                    runtime_ms: None,
                    context: vec![],
                    images: vec![],
                    state: Default::default(),
                    added_at: 0,
                    child_count: None,
                    partial: false,
                },
                quality: Quality::Original,
                file: "Sintel.mkv".into(),
                done: 10,
                total: 20,
                state: State::Running,
                added_at: 0,
            });
            s.save(&i);
        }
        s.queue_state("https://s", 5, serde_json::json!({ "position_ms": 1000 }));
        s.queue_state("https://s", 5, serde_json::json!({ "position_ms": 2000 }));
        let s2 = Store::open(&dir);
        let d = s2.find("https://s", 5).unwrap();
        assert_eq!(d.state, State::Queued);
        assert!(s2.has_pending("https://s"));
        assert_eq!(s2.index.lock().unwrap().pending.len(), 1);
        s2.remove(1);
        assert!(s2.find("https://s", 5).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Downloads an item from a real server, resumes a cut-off copy, and sends offline progress.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn downloads_resumes_and_syncs() {
        use arc_swap::ArcSwap;
        use corrent_core::engine::testkit;
        let dir = testkit::temp_dir("offline");
        let eng = testkit::start(&dir);
        let settings = Arc::new(ArcSwap::from_pointee(testkit::settings(&dir)));
        let local = corrent_server::local::start(eng.engine.clone(), settings, eng.paths.clone()).await.unwrap();
        let c = MediaClient::new(&local.base_url, &local.token, None).unwrap();
        let pics = dir.join("Pics");
        std::fs::create_dir_all(&pics).unwrap();
        image::RgbImage::from_fn(300, 200, |x, y| image::Rgb([x as u8, y as u8, 7])).save(pics.join("a.png")).unwrap();
        let original = std::fs::read(pics.join("a.png")).unwrap();
        let _: serde_json::Value = c
            .send(
                reqwest::Method::POST,
                "/api/v1/libraries",
                &serde_json::json!({ "name": "Pics", "kind": "photos", "folders": [pics] }),
            )
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let item = loop {
            let p: corrent_proto::library::Page<Item> = c.get("/api/v1/items?kind=photo").await.unwrap();
            if let Some(i) = p.items.into_iter().next() {
                break i;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let store = Store::open(&dir.join("client"));
        let ctx = egui::Context::default();
        let rt = tokio::runtime::Handle::current();
        let wait_done = async |store: &Store| loop {
            let d = store.find(c.base(), item.id).unwrap();
            match &d.state {
                State::Done => return d,
                State::Failed(e) => panic!("{e}"),
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        store.add(&c, &rt, &ctx, item.clone(), Quality::Original);
        let d = wait_done(&store).await;
        assert_eq!(d.file, "a.png");
        assert_eq!(std::fs::read(store.path(&d)).unwrap(), original);

        // Cut off half way: the rest comes with a Range request.
        let path = store.path(&d);
        std::fs::write(&path, &original[..original.len() / 2]).unwrap();
        store.update(d.key, |x| x.state = State::Paused);
        store.start(&c, &rt, &ctx, d.key);
        let d = wait_done(&store).await;
        assert_eq!(std::fs::read(store.path(&d)).unwrap(), original);
        assert_eq!(d.done, original.len() as u64);

        // Progress made offline reaches the server; an older entry doesn't overwrite newer.
        store.queue_state(c.base(), item.id, serde_json::json!({ "favorite": true }));
        store.flush(&c).await;
        assert!(!store.has_pending(c.base()));
        let it: corrent_proto::library::ItemDetail = c.get(&format!("/api/v1/items/{}", item.id)).await.unwrap();
        assert!(it.item.state.favorite);
        drop(local);
        testkit::stop(eng).await;
        let _ = std::fs::remove_dir_all(dir);
    }
}
