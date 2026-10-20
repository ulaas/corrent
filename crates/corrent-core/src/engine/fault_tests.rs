//! STABILITY.md S8: the engine under injected faults (storage, network, time). Fast ones run in
//! the gate; heavy ones are `#[ignore]` and run by `scripts/fault-matrix.ps1` (names start with
//! `fm_`).

use std::time::Duration;

use super::EngineCommand;
use super::testkit::*;
use super::types::TorrentState;
use crate::faults::{StorageFault, StorageFaultKind, StorageOp, clear_storage_faults, set_storage_fault};
use crate::util::{IoKind, classify_io};

/// A full disk stops the torrent with a "disk is full" error (not a crash, not a silent stall);
/// once there is room, Resume finishes it from what is on disk: the data matches and what was
/// written before isn't downloaded again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fm_disk_full_stops_the_torrent_and_resume_finishes_it() {
    let dir = temp_dir("fm-disk-full");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let scope = l.paths.data_dir.clone();
    set_storage_fault(
        &scope,
        StorageFault { op: StorageOp::Write, kind: StorageFaultKind::DiskFull { after_bytes: 1 << 20 } },
    );
    add(&l, &s, &dir);
    let snap =
        wait_for(&l, "the disk-full error", 60, |sn| find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Error))
            .await;
    let t = find(&snap, s.hash).unwrap();
    let why = t.error.clone().unwrap_or_default();
    assert_eq!(classify_io(&why), IoKind::DiskFull, "{why}");
    let before = t.done;
    assert!(before <= 1 << 20, "stopped at the limit: {before}");
    // Room again: Resume (the GUI's Retry) opens the files, checks them and finishes.
    clear_storage_faults(&scope);
    l.engine.send(EngineCommand::Resume(vec![s.hash]));
    let snap = wait_for(&l, "the finished download", 90, |sn| complete(sn, s.hash)).await;
    let t = find(&snap, s.hash).unwrap();
    assert_eq!(std::fs::read(&t.content_path).unwrap(), s.content, "the data is whole");
    assert!(t.downloaded <= SIZE as u64 + (512 << 10), "nothing much downloaded twice: {} of {SIZE}", t.downloaded);
    tokio::time::sleep(Duration::from_millis(200)).await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// Files the engine may not open at start (antivirus, permissions): the torrent shows as one
/// that couldn't be loaded, says why, and comes back with Retry once access is back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fm_access_denied_at_start_is_a_failed_load_with_retry() {
    let dir = temp_dir("fm-denied");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    let paths = stop(l).await;
    set_storage_fault(&paths.data_dir, StorageFault { op: StorageOp::Init, kind: StorageFaultKind::Permission });
    let l = start(&dir);
    let snap =
        wait_for(&l, "the failed load", 30, |sn| find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Error))
            .await;
    let t = find(&snap, s.hash).unwrap();
    assert_eq!(classify_io(t.error.as_deref().unwrap_or_default()), IoKind::Permission, "{:?}", t.error);
    clear_storage_faults(&paths.data_dir);
    l.engine.send(EngineCommand::Resume(vec![s.hash]));
    wait_for(&l, "seeding again", 60, |sn| complete(sn, s.hash)).await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A slow disk under the downloads (20 ms per write) slows the download, not the engine:
/// snapshots keep coming and the download completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fm_slow_data_writes_keep_the_engine_responsive() {
    let dir = temp_dir("fm-slow-writes");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let scope = l.paths.data_dir.clone();
    set_storage_fault(
        &scope,
        StorageFault { op: StorageOp::Write, kind: StorageFaultKind::Delay(Duration::from_millis(20)) },
    );
    add(&l, &s, &dir);
    let (mut last, mut gap, mut seen_at) = (l.engine.snapshot().generation, Duration::ZERO, std::time::Instant::now());
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    while !complete(&l.engine.snapshot(), s.hash) {
        assert!(std::time::Instant::now() < deadline, "the download finished");
        tokio::time::sleep(Duration::from_millis(25)).await;
        let g = l.engine.snapshot().generation;
        if g != last {
            gap = gap.max(seen_at.elapsed());
            (last, seen_at) = (g, std::time::Instant::now());
        }
    }
    assert!(gap < Duration::from_secs(1), "snapshots stopped for {gap:?}");
    assert!(
        l.engine.snapshot().session.tick_max_ms < 500,
        "slowest tick {} ms",
        l.engine.snapshot().session.tick_max_ms
    );
    clear_storage_faults(&scope);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A read error under a stream (a failing disk): the player gets an error, never short or
/// made-up data, the engine keeps running, and streaming works again once the disk reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fm_read_errors_never_serve_short_data() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = temp_dir("fm-eio");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    let (port, token) = l.engine.snapshot().session.stream.clone().expect("stream server running");
    let request = format!(
        "GET /{token}/{}/0/data.bin HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-65535\r\nConnection: close\r\n\r\n",
        s.hash.to_hex()
    );
    async fn fetch(port: u16, request: &str) -> Vec<u8> {
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(15), c.read_to_end(&mut out)).await;
        out
    }
    let scope = l.paths.data_dir.clone();
    set_storage_fault(&scope, StorageFault { op: StorageOp::Read, kind: StorageFaultKind::Eio });
    let bad = fetch(port, &request).await;
    let body_at = bad.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(bad.len());
    let body = bad.get(body_at..).unwrap_or_default();
    // Whatever arrived is a prefix of the real data, and never the whole range.
    assert!(body.len() < 65536, "a range served whole from a failing disk ({} bytes)", body.len());
    assert_eq!(body, s.content.get(..body.len()).unwrap(), "no made-up bytes");
    assert!(l.engine.is_alive(), "the engine kept running");
    clear_storage_faults(&scope);
    let good = fetch(port, &request).await;
    assert!(good.starts_with(b"HTTP/1.1 206"), "{}", String::from_utf8_lossy(good.get(..40).unwrap_or_default()));
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A full disk under the profile: saving Corrent's records fails with a warning that says the
/// disk is full, the file on disk stays the last good one, and the next save after room comes
/// back writes everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fm_disk_full_under_the_profile_keeps_the_saved_records() {
    let dir = temp_dir("fm-profile-full");
    let l = start(&dir);
    // A tiny paused torrent with a category, saved.
    let hashes = super::e2e_tests::add_tiny_torrents_pub(&l, &dir, 1).await;
    let engine = l.engine.clone();
    assert!(tokio::task::spawn_blocking(move || engine.flush_blocking(Duration::from_secs(5))).await.unwrap());
    let store_file = l.paths.store_file();
    let before = std::fs::read(&store_file).unwrap();
    crate::faults::set_disk_full(&store_file, None);
    l.engine.send(EngineCommand::SetCategory { hashes: hashes.clone(), category: Some("Changed".into()) });
    let mut warned = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while warned.is_none() {
        assert!(std::time::Instant::now() < deadline, "no warning about the failed save");
        match l.events.try_recv() {
            Ok(super::EngineEvent::DataWarning(w)) => warned = Some(w),
            Ok(_) => {}
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let w = warned.unwrap();
    assert_eq!(classify_io(&w), IoKind::DiskFull, "{w}");
    assert_eq!(std::fs::read(&store_file).unwrap(), before, "the last good file stays");
    crate::faults::clear_file_errors(&store_file);
    let engine = l.engine.clone();
    assert!(tokio::task::spawn_blocking(move || engine.flush_blocking(Duration::from_secs(5))).await.unwrap());
    let saved: crate::store::StoreData = serde_json::from_slice(&std::fs::read(&store_file).unwrap()).unwrap();
    assert_eq!(saved.torrents[&hashes[0]].category.as_deref(), Some("Changed"), "saved once there is room");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}
