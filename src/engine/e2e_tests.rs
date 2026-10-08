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
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
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
