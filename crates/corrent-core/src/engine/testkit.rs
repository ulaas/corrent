//! Loopback test kit: a plain librqbit seeder and a Corrent engine leecher.
//! Offline, no DHT/trackers/UPnP; sockets bind to 127.0.0.1 only. Used by the engine's own
//! integration tests and, through the `testkit` feature, by other crates' tests.
// Test helpers (feature `testkit`, used only by tests): failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::{CreateTorrentOptions, ListenerMode, ListenerOptions, Session, SessionOptions};

use super::types::{AddSource, InfoHash, Snapshot, TorrentState, TorrentView};
use super::{AddRequest, EngineCommand, EngineEvent, EngineHandle};
use crate::config::{Paths, Settings};

pub const SIZE: usize = 3 * 1024 * 1024 + 12_345;

pub struct Seeder {
    pub _session: Arc<Session>,
    pub addr: SocketAddr,
    pub bytes: Vec<u8>,
    pub hash: InfoHash,
    pub content: Vec<u8>,
}

pub fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("corrent-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn content() -> Vec<u8> {
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

pub async fn seeder(dir: &Path) -> Seeder {
    seeder_with(dir, "data.bin", content()).await
}

/// A seeder for a single file `name` with `data`.
pub async fn seeder_with(dir: &Path, name: &str, data: Vec<u8>) -> Seeder {
    let seed_dir = dir.join("seed");
    std::fs::create_dir_all(&seed_dir).unwrap();
    std::fs::write(seed_dir.join(name), &data).unwrap();
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
            &seed_dir.join(name),
            CreateTorrentOptions { name: Some(name), trackers: vec![], piece_length: Some(64 * 1024) },
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

pub fn settings(dir: &Path) -> Settings {
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

pub struct Leecher {
    pub engine: EngineHandle,
    pub events: std::sync::mpsc::Receiver<EngineEvent>,
    pub paths: Paths,
}

pub fn start(dir: &Path) -> Leecher {
    start_with(dir, settings(dir))
}

/// Like `start`, with other settings (start from `settings(dir)`).
pub fn start_with(dir: &Path, settings: Settings) -> Leecher {
    let paths = Paths { data_dir: dir.join("data"), portable: true };
    let (tx, events) = std::sync::mpsc::channel();
    let engine =
        EngineHandle::start(&tokio::runtime::Handle::current(), paths.clone(), settings, super::Wake::noop(), tx);
    Leecher { engine, events, paths }
}

pub async fn stop(l: Leecher) -> Paths {
    let engine = l.engine.clone();
    tokio::task::spawn_blocking(move || engine.shutdown_blocking(Duration::from_secs(10))).await.unwrap();
    l.paths
}

pub fn add(l: &Leecher, s: &Seeder, dir: &Path) {
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
        added_by: None,
    })));
}

pub async fn wait_for(l: &Leecher, what: &str, secs: u64, pred: impl Fn(&Snapshot) -> bool) -> Arc<Snapshot> {
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

pub fn find(snap: &Snapshot, h: InfoHash) -> Option<&TorrentView> {
    snap.torrents.iter().find(|t| t.hash == h && !t.pending)
}

pub fn complete(snap: &Snapshot, h: InfoHash) -> bool {
    find(snap, h).is_some_and(|t| t.state == TorrentState::Seeding && t.busy.is_none())
}

pub async fn download(l: &Leecher, s: &Seeder, dir: &Path) -> TorrentView {
    add(l, s, dir);
    // A small loopback download can finish before the add is recorded (category, tags).
    let snap = wait_for(l, "download to complete", 60, |sn| {
        complete(sn, s.hash) && find(sn, s.hash).is_some_and(|t| t.category.is_some())
    })
    .await;
    find(&snap, s.hash).unwrap().clone()
}
