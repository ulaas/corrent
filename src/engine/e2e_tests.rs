//! Engine integration tests over loopback: a plain librqbit seeder and a Corrent engine leecher.
//! Offline, no DHT/trackers/UPnP; sockets bind to 127.0.0.1 only.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::{CreateTorrentOptions, ListenerMode, ListenerOptions, Session, SessionOptions};

use super::types::{AddSource, InfoHash, Snapshot, TorrentState, TorrentView};
use super::{AddRequest, EngineCommand, EngineEvent, EngineHandle};
use crate::config::{Paths, Settings};

const SIZE: usize = 3 * 1024 * 1024 + 12_345;

struct Seeder {
    _session: Arc<Session>,
    addr: SocketAddr,
    bytes: Vec<u8>,
    hash: InfoHash,
    content: Vec<u8>,
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("corrent-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn content() -> Vec<u8> {
    // Deterministic, incompressible-enough data.
    let mut x: u32 = 0x1234_5678;
    (0..SIZE)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

async fn seeder(dir: &Path) -> Seeder {
    let seed_dir = dir.join("seed");
    std::fs::create_dir_all(&seed_dir).unwrap();
    let data = content();
    std::fs::write(seed_dir.join("data.bin"), &data).unwrap();
    let opts = SessionOptions {
        dht: None,
        persistence: None,
        disable_trackers: true,
        disable_local_service_discovery: true,
        listen: Some(ListenerOptions {
            mode: ListenerMode::TcpOnly,
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            enable_upnp_port_forwarding: false,
            ipv4_only: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let session = Session::new_with_opts(seed_dir.clone(), opts).await.unwrap();
    let (res, _h) = session
        .create_and_serve_torrent(
            &seed_dir.join("data.bin"),
            CreateTorrentOptions { name: Some("data.bin"), trackers: vec![], piece_length: Some(64 * 1024) },
        )
        .await
        .unwrap();
    let addr = session.listen_addr().expect("seeder listens");
    Seeder {
        hash: InfoHash(res.info_hash().0),
        bytes: res.as_bytes().unwrap().to_vec(),
        _session: session,
        addr,
        content: data,
    }
}

fn settings(dir: &Path) -> Settings {
    let mut s = Settings::default();
    s.downloads.save_path = dir.join("dl");
    s.connection.listen_address = "127.0.0.1".into();
    s.connection.random_port = true;
    s.connection.upnp = false;
    s.connection.ipv4_only = true;
    s.bittorrent.dht = false;
    s.bittorrent.lsd = false;
    s.bittorrent.trackers_enabled = false;
    s.queue.enabled = false;
    s.advanced.refresh_interval_ms = 200;
    s
}

struct Leecher {
    engine: EngineHandle,
    events: std::sync::mpsc::Receiver<EngineEvent>,
    paths: Paths,
}

fn start(dir: &Path) -> Leecher {
    let paths = Paths { data_dir: dir.join("data"), portable: true };
    let (tx, events) = std::sync::mpsc::channel();
    let engine = EngineHandle::start(
        &tokio::runtime::Handle::current(),
        paths.clone(),
        settings(dir),
        egui::Context::default(),
        tx,
    );
    Leecher { engine, events, paths }
}

async fn stop(l: Leecher) -> Paths {
    let engine = l.engine.clone();
    tokio::task::spawn_blocking(move || engine.shutdown_blocking(Duration::from_secs(10))).await.unwrap();
    l.paths
}

fn add(l: &Leecher, s: &Seeder, dir: &Path) {
    l.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "data.torrent".into(), bytes: s.bytes.clone() },
        torrent_bytes: None,
        save_path: dir.join("dl"),
        create_subfolder: true,
        category: Some("Tests".into()),
        tags: vec!["e2e".into()],
        paused: false,
        only_files: None,
        add_to_top: false,
        initial_peers: Some(vec![s.addr]),
    })));
}

async fn wait_for(l: &Leecher, what: &str, secs: u64, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let snap = l.engine.snapshot();
        if pred(&snap) {
            return snap;
        }
        if Instant::now() >= deadline {
            let rows: Vec<String> = snap
                .torrents
                .iter()
                .map(|t| format!("{} {:?} busy={:?} err={:?} pending={}", t.name, t.state, t.busy, t.error, t.pending))
                .collect();
            panic!("timed out waiting for: {what}; engine={:?}; torrents={rows:?}", snap.engine_state);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn find(snap: &Snapshot, h: InfoHash) -> Option<&TorrentView> {
    snap.torrents.iter().find(|t| t.hash == h && !t.pending)
}

fn complete(snap: &Snapshot, h: InfoHash) -> bool {
    find(snap, h).is_some_and(|t| t.state == TorrentState::Seeding && t.busy.is_none())
}

async fn download(l: &Leecher, s: &Seeder, dir: &Path) -> TorrentView {
    add(l, s, dir);
    let snap = wait_for(l, "download to complete", 60, |sn| complete(sn, s.hash)).await;
    find(&snap, s.hash).unwrap().clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn download_completes_with_metadata_and_event() {
    let dir = temp_dir("download");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let t = download(&l, &s, &dir).await;
    assert_eq!(t.size, SIZE as u64);
    assert_eq!(t.category.as_deref(), Some("Tests"));
    assert_eq!(t.tags, vec!["e2e".to_string()]);
    assert_eq!(std::fs::read(&t.content_path).unwrap(), s.content, "downloaded bytes match");
    let mut completed = false;
    while let Ok(ev) = l.events.try_recv() {
        if matches!(ev, EngineEvent::Completed { .. }) {
            completed = true;
        }
    }
    assert!(completed, "a Completed event was emitted");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume() {
    let dir = temp_dir("pause");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    l.engine.send(EngineCommand::Pause(vec![s.hash]));
    wait_for(&l, "paused", 20, |sn| find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Completed)).await;
    l.engine.send(EngineCommand::Resume(vec![s.hash]));
    wait_for(&l, "resumed", 20, |sn| complete(sn, s.hash)).await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recheck_reverifies_without_downloading() {
    let dir = temp_dir("recheck");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let before = download(&l, &s, &dir).await.downloaded;
    while l.events.try_recv().is_ok() {}
    l.engine.send(EngineCommand::Recheck(vec![s.hash]));
    // The re-add reports back when done (a small torrent re-verifies faster than we can poll).
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(EngineEvent::Toast(_, t)) = l.events.try_recv()
            && t.contains("Re-checking")
        {
            break;
        }
        assert!(Instant::now() < deadline, "recheck did not report back");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let snap = wait_for(&l, "recheck to finish", 60, |sn| complete(sn, s.hash)).await;
    assert_eq!(find(&snap, s.hash).unwrap().downloaded, before, "nothing was downloaded again");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn move_storage_relocates_files() {
    let dir = temp_dir("move");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let old = download(&l, &s, &dir).await.content_path;
    let dest = dir.join("moved");
    l.engine.send(EngineCommand::MoveStorage { hashes: vec![s.hash], to: dest.clone() });
    let snap = wait_for(&l, "move to finish", 60, |sn| {
        find(sn, s.hash).is_some_and(|t| t.save_path == dest && t.state == TorrentState::Seeding && t.busy.is_none())
    })
    .await;
    let t = find(&snap, s.hash).unwrap();
    assert!(!old.exists(), "old file is gone");
    assert_eq!(std::fs::read(&t.content_path).unwrap(), s.content);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_keeps_or_removes_files() {
    let dir = temp_dir("delete");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let path = download(&l, &s, &dir).await.content_path;
    l.engine.send(EngineCommand::Delete { hashes: vec![s.hash], with_data: false });
    wait_for(&l, "removed", 20, |sn| find(sn, s.hash).is_none()).await;
    assert!(path.exists(), "files kept");

    // Undo brings it back, data intact.
    l.engine.send(EngineCommand::UndoRemove(vec![s.hash]));
    let snap = wait_for(&l, "undo", 30, |sn| complete(sn, s.hash)).await;
    assert_eq!(find(&snap, s.hash).unwrap().category.as_deref(), Some("Tests"), "app data restored");

    l.engine.send(EngineCommand::Delete { hashes: vec![s.hash], with_data: true });
    wait_for(&l, "removed with data", 20, |sn| find(sn, s.hash).is_none()).await;
    let gone = Instant::now() + Duration::from_secs(10);
    while path.exists() && Instant::now() < gone {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!path.exists(), "files deleted");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_restores_without_redownload() {
    let dir = temp_dir("restart");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let before = download(&l, &s, &dir).await;
    stop(l).await;

    let l = start(&dir);
    let snap = wait_for(&l, "restored after restart", 30, |sn| complete(sn, s.hash)).await;
    let after = find(&snap, s.hash).unwrap();
    assert_eq!(after.downloaded, before.downloaded, "totals survive and nothing is re-downloaded");
    assert_eq!(after.category, before.category);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locked_file_shows_as_failed_restore_and_recovers() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = temp_dir("locked");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let path = download(&l, &s, &dir).await.content_path;
    stop(l).await;

    // Hold the file open with no sharing so the engine cannot open it on restore.
    let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
    let l = start(&dir);
    let snap = wait_for(&l, "failed restore row", 30, |sn| {
        sn.torrents.iter().any(|t| t.hash == s.hash && t.restore_failed && t.state == TorrentState::Error)
    })
    .await;
    assert!(snap.torrents.iter().find(|t| t.hash == s.hash).unwrap().error.is_some());
    drop(lock);
    l.engine.send(EngineCommand::Resume(vec![s.hash]));
    wait_for(&l, "recovered after unlock", 30, |sn| complete(sn, s.hash)).await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_restart_after_panic_replays_commands() {
    let dir = temp_dir("panic");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    l.engine.send(EngineCommand::DevPanic);
    let deadline = Instant::now() + Duration::from_secs(10);
    while l.engine.is_alive() {
        assert!(Instant::now() < deadline, "engine should have died");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Sent while the engine is down: queued and replayed.
    l.engine.send(EngineCommand::Pause(vec![s.hash]));
    l.engine.restart(settings(&dir));
    assert!(l.engine.is_alive());
    wait_for(&l, "restored and paused after restart", 30, |sn| {
        find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Completed)
    })
    .await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn created_torrent_seeds_and_downloads() {
    let dir = temp_dir("create");
    let src = dir.join("share");
    std::fs::create_dir_all(src.join("inner")).unwrap();
    std::fs::write(src.join("one.bin"), content()).unwrap();
    std::fs::write(src.join("inner/two.txt"), b"hello from corrent").unwrap();

    // Engine A creates the torrent from the folder and seeds it in place.
    let a_dir = dir.join("a");
    let a = start(&a_dir);
    wait_for(&a, "engine A running", 20, |sn| sn.session.listen_port.is_some()).await;
    a.engine.send(EngineCommand::CreateTorrent(Box::new(super::creator::CreateRequest {
        source: src.clone(),
        name: Some("Renamed share".into()),
        trackers: vec![],
        piece_length: Some(64 * 1024),
        private: false,
        comment: Some("e2e".into()),
        save_to: dir.join("share.torrent"),
        seed: true,
    })));
    let snap =
        wait_for(&a, "creator seeding", 60, |sn| sn.torrents.iter().any(|t| t.state == TorrentState::Seeding)).await;
    let seeded = snap.torrents.iter().find(|t| t.state == TorrentState::Seeding).unwrap().clone();
    assert_eq!(seeded.name, "Renamed share");
    assert_eq!(seeded.downloaded, 0, "seeding from existing data, nothing downloaded");
    let port = snap.session.listen_port.unwrap();

    // Engine B downloads it from A.
    let b_dir = dir.join("b");
    let b = start(&b_dir);
    let bytes = std::fs::read(dir.join("share.torrent")).unwrap();
    b.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "share.torrent".into(), bytes },
        torrent_bytes: None,
        save_path: b_dir.join("dl"),
        create_subfolder: true,
        category: None,
        tags: vec![],
        paused: false,
        only_files: None,
        add_to_top: false,
        initial_peers: Some(vec![format!("127.0.0.1:{port}").parse().unwrap()]),
    })));
    let snap = wait_for(&b, "download from creator", 60, |sn| complete(sn, seeded.hash)).await;
    let got = find(&snap, seeded.hash).unwrap();
    assert_eq!(std::fs::read(got.content_path.join("inner/two.txt")).unwrap(), b"hello from corrent");
    assert_eq!(std::fs::read(got.content_path.join("one.bin")).unwrap(), content());
    stop(b).await;
    stop(a).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streaming_server_serves_ranges() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = temp_dir("stream");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    let (port, token) = l.engine.snapshot().session.stream.clone().expect("stream server running");

    async fn get(port: u16, path: &str, range: Option<&str>) -> (String, Vec<u8>) {
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n");
        if let Some(r) = range {
            req.push_str(&format!("Range: {r}\r\n"));
        }
        req.push_str("\r\n");
        c.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        c.read_to_end(&mut buf).await.unwrap();
        let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (String::from_utf8_lossy(&buf[..split]).into_owned(), buf[split + 4..].to_vec())
    }

    let path = format!("/{token}/{}/0/data.bin", s.hash.to_hex());
    let (head, body) = get(port, &path, Some("bytes=100-199")).await;
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    assert!(head.contains(&format!("Content-Range: bytes 100-199/{SIZE}")));
    assert_eq!(body, s.content[100..200]);

    let (head, body) = get(port, &path, None).await;
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(body.len(), SIZE);

    let (head, _) = get(port, &format!("/wrong-token/{}/0/data.bin", s.hash.to_hex()), None).await;
    assert!(head.starts_with("HTTP/1.1 404"), "a wrong token is rejected");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_does_not_repeat_completion() {
    let dir = temp_dir("norepeat");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    let paths = stop(l).await;
    // Simulate a crash before `completed_at` was saved: lifetime counters say data was fetched.
    let file = paths.store_file();
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    for (_, m) in v["torrents"].as_object_mut().unwrap() {
        m["completed_at"] = serde_json::Value::Null;
        assert!(m["downloaded_total"].as_u64().unwrap() > 0);
    }
    std::fs::write(&file, serde_json::to_vec(&v).unwrap()).unwrap();
    let l = start(&dir);
    wait_for(&l, "restored as complete", 30, |sn| complete(sn, s.hash)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(ev) = l.events.try_recv() {
        assert!(!matches!(ev, EngineEvent::Completed { .. }), "no second Completed event after a restart");
    }
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// Serve an RSS feed and the .torrent it links to; counts feed requests.
async fn feed_server(torrent: Vec<u8>) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (b, h) = (base.clone(), hits.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = listener.accept().await else { continue };
            let (b, h, torrent) = (b.clone(), h.clone(), torrent.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = c.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let (ctype, body) = if req.starts_with("GET /feed") {
                    h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let xml = format!(
                        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Test feed</title>
<item><title>Data S01E01 1080p</title><guid>data-1</guid><enclosure url="{b}/data.torrent" type="application/x-bittorrent"/></item>
<item><title>Something else</title><guid>other</guid><enclosure url="{b}/other.torrent" type="application/x-bittorrent"/></item>
</channel></rss>"#
                    );
                    ("application/rss+xml", xml.into_bytes())
                } else {
                    ("application/x-bittorrent", torrent)
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = c.write_all(head.as_bytes()).await;
                let _ = c.write_all(&body).await;
            });
        }
    });
    (base, hits)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rss_rule_adds_matching_item_exactly_once() {
    use std::sync::atomic::Ordering;
    let dir = temp_dir("rss");
    let s = seeder(&dir).await;
    let (base, hits) = feed_server(s.bytes.clone()).await;
    let mut cfg = settings(&dir);
    cfg.rss.feeds.push(crate::rss::FeedConfig { url: format!("{base}/feed"), ..Default::default() });
    cfg.rss.rules.push(crate::rss::RssRule {
        name: "Data".into(),
        must_contain: "data 1080p".into(),
        category: Some("Shows".into()),
        ..Default::default()
    });
    let l = start(&dir);
    l.engine.send(EngineCommand::ApplySettings(Box::new(cfg.clone())));
    let snap = wait_for(&l, "rule to add the torrent", 30, |sn| find(sn, s.hash).is_some()).await;
    let t = find(&snap, s.hash).unwrap();
    assert_eq!(t.category.as_deref(), Some("Shows"));
    assert!(t.tags.contains(&"rss".to_string()));
    assert_eq!(snap.torrents.iter().filter(|t| !t.pending).count(), 1, "only the matching item");
    let feed = &snap.rss.feeds[0];
    assert_eq!(feed.title, "Test feed");
    assert!(feed.articles.iter().any(|a| a.downloaded && a.article.guid == "data-1"));

    // Removed and refreshed: not added again.
    l.engine.send(EngineCommand::Delete { hashes: vec![s.hash], with_data: false });
    wait_for(&l, "removed", 20, |sn| find(sn, s.hash).is_none()).await;
    let before = hits.load(Ordering::SeqCst);
    l.engine.send(EngineCommand::RssRefresh(None));
    let deadline = Instant::now() + Duration::from_secs(20);
    while hits.load(Ordering::SeqCst) == before {
        assert!(Instant::now() < deadline, "refresh did not fetch");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(find(&l.engine.snapshot(), s.hash).is_none(), "not re-added after a refresh");
    stop(l).await;

    // Restarted with the same settings: fetched again, still not re-added.
    let (tx, events) = std::sync::mpsc::channel();
    let paths = Paths { data_dir: dir.join("data"), portable: true };
    let engine =
        EngineHandle::start(&tokio::runtime::Handle::current(), paths.clone(), cfg, egui::Context::default(), tx);
    let l = Leecher { engine, events, paths };
    let before = hits.load(Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(20);
    while hits.load(Ordering::SeqCst) == before {
        assert!(Instant::now() < deadline, "feed not fetched after restart");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(find(&l.engine.snapshot(), s.hash).is_none(), "not re-added after a restart");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// What Sonarr/Radarr do with a qBittorrent client, against Corrent's web API.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webapi_behaves_like_qbittorrent_for_sonarr() {
    use crate::webapi::{WebApiServer, WebApiSettings, hash_password};
    let dir = temp_dir("webapi");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let cfg = WebApiSettings {
        enabled: true,
        port: 0,
        allow_lan: false,
        username: "sonarr".into(),
        password_hash: hash_password("correct horse"),
    };
    let shared = Arc::new(arc_swap::ArcSwap::from_pointee(settings(&dir)));
    let server = WebApiServer::start(cfg, l.engine.clone(), shared).await.unwrap();
    let base = format!("http://127.0.0.1:{}/api/v2", server.addr.port());
    let http = reqwest::Client::new();

    // Wrong password, then no session.
    let r = http
        .post(format!("{base}/auth/login"))
        .form(&[("username", "sonarr"), ("password", "nope")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "Fails.");
    let r = http.get(format!("{base}/app/version")).send().await.unwrap();
    assert_eq!(r.status(), 403);

    let r = http
        .post(format!("{base}/auth/login"))
        .form(&[("username", "sonarr"), ("password", "correct horse")])
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    assert!(cookie.starts_with("SID="));
    assert_eq!(r.text().await.unwrap(), "Ok.");
    let get = |path: &str| http.get(format!("{base}/{path}")).header("Cookie", &cookie).send();
    let post = |path: &str, form: Vec<(&str, String)>| {
        http.post(format!("{base}/{path}")).header("Cookie", &cookie).form(&form).send()
    };

    assert_eq!(get("app/webapiVersion").await.unwrap().text().await.unwrap(), "2.9.3");
    assert!(get("app/version").await.unwrap().text().await.unwrap().starts_with("v4."));
    let prefs: serde_json::Value = get("app/preferences").await.unwrap().json().await.unwrap();
    assert!(prefs["save_path"].as_str().unwrap().ends_with("dl"));
    assert_eq!(prefs["queueing_enabled"], false);

    post("torrents/createCategory", vec![("category", "tv-sonarr".into()), ("savePath", "".into())]).await.unwrap();
    wait_for(&l, "category", 10, |sn| sn.categories.contains_key("tv-sonarr")).await;
    let cats: serde_json::Value = get("torrents/categories").await.unwrap().json().await.unwrap();
    assert_eq!(cats["tv-sonarr"]["name"], "tv-sonarr");

    // Upload a .torrent as multipart, like Sonarr does.
    let boundary = "corrent-test-boundary";
    let mut body = Vec::new();
    for (k, v) in [("category", "tv-sonarr"), ("paused", "false")] {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"data.torrent\"\r\nContent-Type: application/x-bittorrent\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend(&s.bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    let r = http
        .post(format!("{base}/torrents/add"))
        .header("Cookie", &cookie)
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "Ok.");
    wait_for(&l, "added via API", 20, |sn| find(sn, s.hash).is_some()).await;

    let list: Vec<serde_json::Value> = get("torrents/info?category=tv-sonarr").await.unwrap().json().await.unwrap();
    assert_eq!(list.len(), 1);
    let t = &list[0];
    assert_eq!(t["hash"], s.hash.to_hex());
    assert_eq!(t["name"], "data.bin");
    assert_eq!(t["category"], "tv-sonarr");
    assert_eq!(t["size"], SIZE as u64);
    assert!(t["content_path"].as_str().unwrap().contains("data.bin"));
    assert!(
        ["downloading", "stalledDL", "metaDL", "checkingDL", "queuedDL"].contains(&t["state"].as_str().unwrap()),
        "{t}"
    );
    let empty: Vec<serde_json::Value> = get("torrents/info?category=movies").await.unwrap().json().await.unwrap();
    assert!(empty.is_empty());
    let props: serde_json::Value =
        get(&format!("torrents/properties?hash={}", s.hash.to_hex())).await.unwrap().json().await.unwrap();
    assert!(props["save_path"].as_str().unwrap().ends_with("dl"));
    let main: serde_json::Value = get("sync/maindata").await.unwrap().json().await.unwrap();
    assert!(main["torrents"][s.hash.to_hex()].is_object());

    let h = s.hash.to_hex();
    let r = post(
        "torrents/setShareLimits",
        vec![("hashes", h.clone()), ("ratioLimit", "2".into()), ("seedingTimeLimit", "-2".into())],
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    post("torrents/pause", vec![("hashes", h.clone())]).await.unwrap();
    wait_for(&l, "paused via API", 20, |sn| find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Paused)).await;
    let list: Vec<serde_json::Value> = get("torrents/info?filter=paused").await.unwrap().json().await.unwrap();
    assert_eq!(list[0]["state"], "pausedDL");
    post("torrents/delete", vec![("hashes", h.clone()), ("deleteFiles", "true".into())]).await.unwrap();
    wait_for(&l, "deleted via API", 20, |sn| find(sn, s.hash).is_none()).await;

    // Cross-site requests and foreign Host names are refused even with a session.
    let r = http
        .get(format!("{base}/app/version"))
        .header("Cookie", &cookie)
        .header("Origin", "http://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .get(format!("{base}/app/version"))
        .header("Cookie", &cookie)
        .header("Host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    drop(server);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}
