//! Engine integration tests over loopback (helpers in `testkit`).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use librqbit::{ManagedTorrent, Session, SessionOptions};

use super::testkit::*;
use super::types::{AddSource, InfoHash, StreamState, TorrentState};
use super::{AddRequest, EngineCommand, EngineEvent, EngineHandle};
use crate::config::Paths;
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
    // A completion seen before the add was recorded is announced a tick later, with its category.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match l.events.try_recv() {
            Ok(EngineEvent::Completed { hash, category, .. }) => {
                assert_eq!(hash, s.hash);
                assert_eq!(category.as_deref(), Some("Tests"));
                break;
            }
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "a Completed event was emitted");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    // On-demand details for a remote client.
    let (tx, rx) = tokio::sync::oneshot::channel();
    l.engine.send(EngineCommand::Detail { hash: s.hash, reply: tx });
    let d = rx.await.unwrap().expect("detail of a known torrent");
    assert_eq!(d.files.len(), 1);
    let (bits, n) = d.pieces.expect("piece bitfield");
    assert!(n > 0 && bits.iter().any(|b| *b != 0));
    let (tx, rx) = tokio::sync::oneshot::channel();
    l.engine.send(EngineCommand::Detail { hash: InfoHash([9; 20]), reply: tx });
    assert!(rx.await.unwrap().is_none());
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

#[cfg(debug_assertions)]
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
        id: 1,
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
        added_by: None,
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

    // The requests show up as one stream session with the client's details.
    let vlc = |range: &'static str| format!("{range}\r\nUser-Agent: VLC/3.0.20 LibVLC/3.0.20");
    let (head, _) = get(port, &path, Some(vlc("bytes=0-999").as_str())).await;
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    let (head, _) = get(port, &path, Some(vlc("bytes=1000-1999").as_str())).await;
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    let snap = wait_for(&l, "stream session listed", 10, |sn| {
        sn.session.streams.iter().any(|v| v.player == "VLC 3.0.20" && v.served == 2000)
    })
    .await;
    let v = snap.session.streams.iter().find(|v| v.player == "VLC 3.0.20").unwrap();
    assert_eq!((v.hash, v.file, v.file_len), (s.hash, 0, SIZE as u64));
    assert_eq!(v.client, "127.0.0.1");
    assert_eq!(v.requests, 2);
    assert_eq!(v.position, 2000);
    assert_eq!(v.buffered_ahead, Some(SIZE as u64 - 2000), "the whole file is downloaded");
    assert!(v.availability.iter().all(|&a| a == 255));
    assert!(!v.torrent_name.is_empty());
    assert_eq!(snap.session.streams.len(), 2, "requests without a User-Agent are another client");

    // Stop: the same player is refused for a while.
    l.engine.send(EngineCommand::StopStream(v.id));
    wait_for(&l, "stream stopped", 10, |sn| {
        sn.session.streams.iter().any(|x| x.id == v.id && x.state == StreamState::Stopped)
    })
    .await;
    let (head, _) = get(port, &path, Some(vlc("bytes=0-9").as_str())).await;
    assert!(head.starts_with("HTTP/1.1 410"), "{head}");
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
    let engine = EngineHandle::start(&tokio::runtime::Handle::current(), paths.clone(), cfg, super::Wake::noop(), tx);
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

/// A plain librqbit client that only knows Corrent (at `port`) as a peer.
async fn client_of(dir: &Path, s: &Seeder, port: u16) -> (Arc<Session>, Arc<ManagedTorrent>) {
    let other = dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let opts = SessionOptions {
        dht: None,
        persistence: None,
        disable_trackers: true,
        disable_local_service_discovery: true,
        listen: None,
        ..Default::default()
    };
    let sess = Session::new_with_opts(other, opts).await.unwrap();
    let resp = sess
        .add_torrent(
            librqbit::AddTorrent::from_bytes(s.bytes.clone()),
            Some(librqbit::AddTorrentOptions {
                initial_peers: Some(vec![SocketAddr::from(([127, 0, 0, 1], port))]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    (sess, resp.into_handle().unwrap())
}

/// The client receives data, but no faster than `kib`/s (plus a burst): an unlimited loopback
/// transfer would finish the 3 MiB file in well under a second. Measured against elapsed time, so
/// a slow machine can't fail it.
async fn assert_rate_limited(h: &Arc<ManagedTorrent>, kib: u64, what: &str) {
    let start = Instant::now();
    while h.stats().progress_bytes == 0 {
        assert!(start.elapsed() < Duration::from_secs(30), "{what}: nothing arrived");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let got = h.stats().progress_bytes;
    let allowed = kib * 1024 * start.elapsed().as_secs().max(1) + 512 * 1024;
    assert!(got <= allowed, "{what}: {got} bytes in {:?} (allowed {allowed})", start.elapsed());
    assert!(!h.stats().finished, "{what}: finished far too fast");
}

/// Corrent seeds with a per-torrent upload limit (set while live, then kept across a pause and
/// resume); a plain librqbit client downloads from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_torrent_speed_limit_applies_and_survives_resume() {
    let dir = temp_dir("limits");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    let port = l.engine.snapshot().session.listen_port.expect("Corrent listens");
    l.engine.send(EngineCommand::SetTorrentLimits { hashes: vec![s.hash], down_kib: 0, up_kib: 64 });
    wait_for(&l, "limit stored", 10, |sn| find(sn, s.hash).is_some_and(|t| t.up_limit_kib == 64)).await;
    l.engine.send(EngineCommand::Pause(vec![s.hash]));
    wait_for(&l, "paused", 20, |sn| find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Completed)).await;
    l.engine.send(EngineCommand::Resume(vec![s.hash]));
    wait_for(&l, "seeding again", 20, |sn| complete(sn, s.hash)).await;

    let (_sess, h) = client_of(&dir, &s, port).await;
    assert_rate_limited(&h, 64, "per-torrent upload limit after a resume").await;

    // Lifting the limit lets it finish.
    l.engine.send(EngineCommand::SetTorrentLimits { hashes: vec![s.hash], down_kib: 0, up_kib: 0 });
    let deadline = Instant::now() + Duration::from_secs(30);
    while !h.stats().finished {
        assert!(Instant::now() < deadline, "download didn't finish once unlimited");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(h);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A category speed limit is shared by the category's torrents.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn category_speed_limit_applies() {
    let dir = temp_dir("catlimit");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await; // category "Tests"
    let port = l.engine.snapshot().session.listen_port.expect("Corrent listens");
    let mut cats = l.engine.snapshot().categories.clone();
    cats.insert("Tests".into(), crate::store::CategoryInfo { save_path: None, down_limit_kib: 0, up_limit_kib: 64 });
    l.engine.send(EngineCommand::SetCategories(cats));
    wait_for(&l, "category limit stored", 10, |sn| sn.categories.get("Tests").is_some_and(|c| c.up_limit_kib == 64))
        .await;
    tokio::time::sleep(Duration::from_millis(1500)).await; // applied within two ticks (200 ms each), with slack
    let (_sess, h) = client_of(&dir, &s, port).await;
    assert_rate_limited(&h, 64, "category upload limit").await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_priority_is_stored_and_shown() {
    let dir = temp_dir("filepri");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    l.engine.send(EngineCommand::SetDetailTarget(Some(s.hash)));
    l.engine.send(EngineCommand::SetFilePriority { hash: s.hash, files: vec![0], priority: 1 });
    wait_for(&l, "priority in details", 10, |sn| {
        sn.detail.as_ref().is_some_and(|d| d.hash == s.hash && d.files.first().is_some_and(|f| f.priority == 1))
    })
    .await;
    l.engine.send(EngineCommand::SetFilePriority { hash: s.hash, files: vec![0], priority: 0 });
    wait_for(&l, "back to normal", 10, |sn| {
        sn.detail.as_ref().is_some_and(|d| d.files.first().is_some_and(|f| f.priority == 0))
    })
    .await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A move/recheck cut off after the torrent left the session is recovered at the next start.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_readd_is_recovered_at_start() {
    let dir = temp_dir("recover");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let t = download(&l, &s, &dir).await;
    l.engine.send(EngineCommand::Delete { hashes: vec![s.hash], with_data: false });
    wait_for(&l, "removed", 20, |sn| find(sn, s.hash).is_none()).await;
    let paths = stop(l).await;
    // What readd leaves behind when it is interrupted.
    let tdir = paths.torrents_dir();
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join(format!("{}.torrent", s.hash.to_hex())), &s.bytes).unwrap();
    let note = serde_json::json!({
        "old_output": t.save_path, "new_output": dir.join("elsewhere"), "first_file": null,
        "only_files": null, "paused": false, "trackers": []
    });
    std::fs::write(tdir.join(format!("{}.json", s.hash.to_hex())), note.to_string()).unwrap();
    let l = start(&dir);
    wait_for(&l, "recovered and complete", 30, |sn| complete(sn, s.hash)).await;
    assert!(!tdir.join(format!("{}.torrent", s.hash.to_hex())).exists(), "backup removed after recovery");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_adds_at_once_all_arrive() {
    // librqbit's JSON persistence used to give concurrent adds the same id, refusing all but one.
    let dir = temp_dir("batch-add");
    let l = start(&dir);
    let n = 40;
    for i in 0..n {
        let name = format!("batch-{i:03}.bin");
        let mut info =
            format!("d6:lengthi16384e4:name{}:{name}12:piece lengthi16384e6:pieces20:", name.len()).into_bytes();
        info.extend_from_slice(&[i as u8; 20]);
        info.push(b'e');
        let mut bytes = b"d4:info".to_vec();
        bytes.extend_from_slice(&info);
        bytes.push(b'e');
        l.engine.send(EngineCommand::Add(Box::new(AddRequest {
            source: AddSource::Bytes { name: format!("{i}.torrent"), bytes },
            torrent_bytes: None,
            save_path: dir.join("dl"),
            create_subfolder: false,
            category: None,
            tags: Vec::new(),
            paused: true,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
        })));
    }
    wait_for(&l, "every torrent", 60, |sn| sn.torrents.len() == n && sn.torrents.iter().all(|t| !t.pending)).await;
    // The store is saved on the writer thread with a 2 s debounce: a batch costs a write or two,
    // not one (plus an fsync) per torrent on the engine's thread.
    // The writer thread is shared with the other tests (some of which slow their own disk down):
    // wait for the first save instead of a fixed time, then let the debounce settle.
    let deadline = Instant::now() + Duration::from_secs(20);
    while crate::writer::global().writes(&l.paths.store_file()) == 0 {
        assert!(Instant::now() < deadline, "store.json was never saved");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let writes = crate::writer::global().writes(&l.paths.store_file());
    assert!((1..=5).contains(&writes), "{writes} store writes for {n} adds");
    let paths = stop(l).await;
    let store: crate::store::StoreData = serde_json::from_slice(&std::fs::read(paths.store_file()).unwrap()).unwrap();
    assert_eq!(store.torrents.len(), n, "all of them saved");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watch_folder_keeps_files_until_their_add_finishes() {
    let dir = temp_dir("watch");
    let s = seeder(&dir).await;
    let watch = dir.join("watch");
    std::fs::create_dir_all(&watch).unwrap();
    std::fs::write(watch.join("good.torrent"), &s.bytes).unwrap();
    std::fs::write(watch.join("bad.torrent"), b"this is not a torrent").unwrap();
    // Too large to be a torrent (65 MiB, sparse): refused unread, kept as .failed.
    std::fs::File::create(watch.join("huge.torrent")).unwrap().set_len(65 << 20).unwrap();
    let mut cfg = settings(&dir);
    cfg.watch.push(crate::config::WatchFolder { path: watch.clone(), start_paused: true, ..Default::default() });
    let l = start(&dir);
    l.engine.send(EngineCommand::ApplySettings(Box::new(cfg)));
    wait_for(&l, "the watched torrent", 30, |sn| find(sn, s.hash).is_some_and(|t| !t.pending)).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(watch.join("good.torrent.added").exists()
        && watch.join("bad.torrent.failed").exists()
        && watch.join("huge.torrent.failed").exists())
    {
        assert!(
            Instant::now() < deadline,
            "files after their adds: {:?}",
            std::fs::read_dir(&watch).unwrap().collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(std::fs::read(watch.join("bad.torrent.failed")).unwrap(), b"this is not a torrent");
    assert!(!watch.join("good.torrent").exists() && !watch.join("bad.torrent").exists());
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A multi-file .torrent whose files have the given path components (16 KiB each).
#[cfg(any(windows, target_os = "macos"))]
fn multi_file_torrent(name: &str, files: &[&[&str]]) -> Vec<u8> {
    fn s(b: &mut Vec<u8>, x: &str) {
        b.extend_from_slice(format!("{}:{x}", x.len()).as_bytes());
    }
    let mut b = b"d4:infod5:filesl".to_vec();
    for f in files {
        b.extend_from_slice(b"d6:lengthi16384e4:pathl");
        for c in *f {
            s(&mut b, c);
        }
        b.extend_from_slice(b"ee");
    }
    b.extend_from_slice(b"e4:name");
    s(&mut b, name);
    b.extend_from_slice(b"12:piece lengthi16384e6:pieces");
    let pieces = vec![7u8; 20 * files.len()];
    b.extend_from_slice(format!("{}:", pieces.len()).as_bytes());
    b.extend_from_slice(&pieces);
    b.extend_from_slice(b"ee");
    b
}

/// Add `bytes` and return the error toast it ends with (None if it was added).
#[cfg(any(windows, target_os = "macos"))]
async fn add_error(l: &Leecher, dir: &Path, bytes: Vec<u8>) -> Option<String> {
    l.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "crafted.torrent".into(), bytes },
        torrent_bytes: None,
        save_path: dir.join("dl"),
        create_subfolder: true,
        category: None,
        tags: Vec::new(),
        paused: true,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: None,
    })));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match l.events.try_recv() {
            Ok(EngineEvent::Added { .. }) => return None,
            Ok(EngineEvent::Toast(super::ToastKind::Error, text)) => return Some(text),
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "the add neither finished nor failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_differing_only_in_case_are_refused() {
    let dir = temp_dir("case-dup");
    let l = start(&dir);
    let bytes = multi_file_torrent("pair", &[&["A.txt"], &["a.txt"]]);
    let err = add_error(&l, &dir, bytes).await.expect("refused");
    assert!(err.contains("letter case"), "{err}");
    // Windows names that would escape the save folder, open a device or merge two files. Only
    // run once the check exists: unchecked, `C:x` is written relative to the C: drive's folder.
    #[cfg(windows)]
    for (name, file) in [("drive", "C:corrent-e1-probe"), ("device", "CON.txt"), ("dot", "name.")] {
        let bytes = multi_file_torrent(name, &[&[file], &["ok.txt"]]);
        let err = add_error(&l, &dir, bytes).await.unwrap_or_else(|| panic!("{file} was added"));
        assert!(err.contains("can't store safely"), "{err}");
    }
    assert!(l.engine.snapshot().torrents.iter().all(|t| t.pending), "nothing was added");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_panic_stops_the_old_session() {
    let dir = temp_dir("panic-session");
    let l = start(&dir);
    let snap = wait_for(&l, "a listening session", 30, |sn| sn.session.listen_port.is_some()).await;
    let port = snap.session.listen_port.unwrap();
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(), "listening before the panic");
    l.engine.send(EngineCommand::DevPanic);
    let deadline = Instant::now() + Duration::from_secs(10);
    while l.engine.is_alive() {
        assert!(Instant::now() < deadline, "engine should have died");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Without a restart nothing may keep running on the old session (and its files).
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)).is_ok() {
        assert!(Instant::now() < deadline, "the dead engine's session still listens on {port}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The new engine continues the generation (STABILITY.md D12): the UI caches rows by it.
    let before = l.engine.snapshot().generation;
    l.engine.restart(settings(&dir));
    let deadline = Instant::now() + Duration::from_secs(30);
    let first = loop {
        let g = l.engine.snapshot().generation;
        if g != before {
            break g;
        }
        assert!(Instant::now() < deadline, "no snapshot from the new engine");
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    assert!(first > before, "generation went from {before} to {first}");
    wait_for(&l, "a new session", 30, |sn| sn.session.listen_port.is_some()).await;
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// Add `n` tiny paused torrents (made-up piece hashes; nothing to download) and wait for them.
async fn add_tiny_torrents(l: &Leecher, dir: &Path, n: u8) -> Vec<InfoHash> {
    for i in 0..n {
        let name = format!("tiny-{i:03}.bin");
        let mut info =
            format!("d6:lengthi16384e4:name{}:{name}12:piece lengthi16384e6:pieces20:", name.len()).into_bytes();
        info.extend_from_slice(&[i.wrapping_add(100); 20]);
        info.push(b'e');
        let mut bytes = b"d4:info".to_vec();
        bytes.extend_from_slice(&info);
        bytes.push(b'e');
        l.engine.send(EngineCommand::Add(Box::new(AddRequest {
            source: AddSource::Bytes { name: format!("{i}.torrent"), bytes },
            torrent_bytes: None,
            save_path: dir.join("dl"),
            create_subfolder: false,
            category: Some("Kept".into()),
            tags: Vec::new(),
            paused: true,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
        })));
    }
    // Recorded too (category set), not only visible in the engine.
    let snap = wait_for(l, "the tiny torrents", 30, |sn| {
        sn.torrents.len() == n as usize && sn.torrents.iter().all(|t| t.category.as_deref() == Some("Kept"))
    })
    .await;
    snap.torrents.iter().filter(|t| !t.pending).map(|t| t.hash).collect()
}

fn read_store(paths: &Paths) -> crate::store::StoreData {
    serde_json::from_slice(&std::fs::read(paths.store_file()).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emptied_engine_database_does_not_wipe_the_store() {
    let dir = temp_dir("prune-refused");
    let l = start(&dir);
    let hashes = add_tiny_torrents(&l, &dir, 3).await;
    let paths = stop(l).await;
    assert_eq!(read_store(&paths).torrents.len(), 3);
    // The engine's database comes back empty (restored from an old backup, rewritten, …).
    std::fs::write(paths.session_dir().join("session.json"), br#"{"torrents":{}}"#).unwrap();
    let l = start(&dir);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match l.events.try_recv() {
            Ok(EngineEvent::DataWarning(w)) if w.contains("missing from the engine") => break,
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "no warning about the refused prune");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    assert!(l.engine.snapshot().torrents.is_empty(), "the engine really lost them");
    let paths = stop(l).await;
    let store = read_store(&paths);
    for h in &hashes {
        assert_eq!(store.torrents.get(h).and_then(|m| m.category.as_deref()), Some("Kept"), "kept {h:?}");
    }
    assert!(crate::util::generations(&paths.store_file(), "pre-prune").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stray_store_entry_is_pruned_after_a_snapshot() {
    let dir = temp_dir("prune-one");
    let l = start(&dir);
    let hashes = add_tiny_torrents(&l, &dir, 2).await;
    let paths = stop(l).await;
    let stray = InfoHash([0xee; 20]);
    let mut store = read_store(&paths);
    store.torrents.insert(stray, Default::default());
    std::fs::write(paths.store_file(), serde_json::to_vec(&store).unwrap()).unwrap();
    let l = start(&dir);
    let store_file = paths.store_file();
    let deadline = Instant::now() + Duration::from_secs(20);
    while crate::util::generations(&store_file, "pre-prune").is_empty() {
        assert!(Instant::now() < deadline, "no snapshot before the prune");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let paths = stop(l).await;
    let store = read_store(&paths);
    assert!(!store.torrents.contains_key(&stray), "pruned");
    assert!(hashes.iter().all(|h| store.torrents.contains_key(h)));
    let (_, snap) = &crate::util::generations(&paths.store_file(), "pre-prune")[0];
    let snap: crate::store::StoreData = serde_json::from_slice(&std::fs::read(snap).unwrap()).unwrap();
    assert!(snap.torrents.contains_key(&stray), "the snapshot has everything");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn flush_saves_now_and_keeps_the_engine_running() {
    let dir = temp_dir("flush");
    let l = start(&dir);
    let hashes = add_tiny_torrents(&l, &dir, 1).await;
    // Wait out the debounce so the next change is the only unsaved one.
    tokio::time::sleep(Duration::from_secs(3)).await;
    l.engine.send(EngineCommand::SetCategory { hashes: hashes.clone(), category: Some("Flushed".into()) });
    wait_for(&l, "the new category", 10, |sn| {
        find(sn, hashes[0]).is_some_and(|t| t.category.as_deref() == Some("Flushed"))
    })
    .await;
    let engine = l.engine.clone();
    assert!(tokio::task::spawn_blocking(move || engine.flush_blocking(Duration::from_secs(5))).await.unwrap());
    let store = read_store(&l.paths);
    assert_eq!(store.torrents[&hashes[0]].category.as_deref(), Some("Flushed"), "on disk right away");
    assert!(l.engine.is_alive(), "still running");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// No `<name>.tmp` left in the engine's session folder.
fn no_temp_files(paths: &Paths) {
    let left: Vec<_> = std::fs::read_dir(paths.session_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persisted_metainfo_is_written_whole_and_atomically() {
    let dir = temp_dir("persist");
    let s = seeder(&dir).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    let torrent = l.paths.session_dir().join(format!("{}.torrent", s.hash.to_hex()));
    assert_eq!(std::fs::read(&torrent).unwrap(), s.bytes);
    // A recheck forgets and re-adds the torrent, which writes the metainfo again.
    l.engine.send(EngineCommand::Recheck(vec![s.hash]));
    tokio::time::sleep(Duration::from_millis(500)).await;
    wait_for(&l, "rechecked", 30, |sn| complete(sn, s.hash)).await;
    let paths = stop(l).await;
    assert_eq!(std::fs::read(&torrent).unwrap(), s.bytes);
    no_temp_files(&paths);
    let _ = std::fs::remove_dir_all(dir);
}

/// librqbit built its temp file names with `to_str().unwrap()`, which panicked in the engine
/// task for a data folder whose name isn't valid Unicode (possible on NTFS and on Unix).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_folder_with_a_non_unicode_name_works() {
    use std::os::windows::ffi::OsStringExt;
    let base = temp_dir("non-unicode");
    // "s" followed by an unpaired surrogate: a valid NTFS name that `to_str()` can't represent.
    let dir = base.join(std::ffi::OsString::from_wide(&[b's' as u16, 0xD800]));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(dir.to_str().is_none());
    let l = start(&dir);
    // Downloads go to a Unicode folder: this is about the data folder only (a non-Unicode save
    // path is converted lossily when the torrent is added, a separate P4 item).
    let hashes = add_tiny_torrents(&l, &base, 1).await;
    assert!(l.engine.is_alive());
    let paths = stop(l).await;
    let db = std::fs::read_to_string(paths.session_dir().join("session.json")).unwrap();
    assert!(db.contains(&hashes[0].to_hex()), "{db}");
    assert!(paths.session_dir().join(format!("{}.bitv", hashes[0].to_hex())).exists());
    no_temp_files(&paths);
    let _ = std::fs::remove_dir_all(base);
}

/// What syncing session.json costs (STABILITY.md risk 4): pause and resume 250 torrents, each of
/// which rewrites (and now syncs) the engine's database. Numbers go to docs/PERFORMANCE.md.
#[ignore = "timing, run by hand: cargo test -p corrent-core --lib perf_session_flush -- --ignored --nocapture"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perf_session_flush_cost() {
    let dir = temp_dir("perf-flush");
    let l = start(&dir);
    let n = 250;
    let t = Instant::now();
    add_tiny_torrents(&l, &dir, n).await;
    println!("add {n}: {:?}", t.elapsed());
    // A Flush queued behind the command is answered once the actor has worked through it.
    for (what, cmd) in [("resume all", EngineCommand::ResumeAll), ("pause all", EngineCommand::PauseAll)] {
        let t = Instant::now();
        l.engine.send(cmd);
        let engine = l.engine.clone();
        assert!(tokio::task::spawn_blocking(move || engine.flush_blocking(Duration::from_secs(300))).await.unwrap());
        println!("{what} {n}: {:?}", t.elapsed());
    }
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// Error toasts and data warnings seen until `until` returns true or `secs` pass.
async fn notices_until(l: &Leecher, secs: u64, until: impl Fn(&super::types::Snapshot) -> bool) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seen = Vec::new();
    while Instant::now() < deadline && !until(&l.engine.snapshot()) {
        while let Ok(ev) = l.events.try_recv() {
            match ev {
                EngineEvent::Toast(_, t) | EngineEvent::DataWarning(t) => seen.push(t),
                _ => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    while let Ok(ev) = l.events.try_recv() {
        if let EngineEvent::Toast(_, t) | EngineEvent::DataWarning(t) = ev {
            seen.push(t);
        }
    }
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_occupied_port_falls_back_to_a_random_one() {
    let dir = temp_dir("port-taken");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut cfg = settings(&dir);
    cfg.connection.random_port = false;
    cfg.connection.listen_port = port;
    let l = start_with(&dir, cfg);
    let notices = notices_until(&l, 30, |sn| {
        sn.engine_state == super::types::EngineState::Running && sn.session.listen_port.is_some()
    })
    .await;
    let snap = l.engine.snapshot();
    assert_eq!(snap.engine_state, super::types::EngineState::Running, "{notices:?}");
    assert_ne!(snap.session.listen_port, Some(port));
    assert!(notices.iter().any(|n| n.contains(&format!("Port {port} is unavailable"))), "{notices:?}");
    stop(l).await;
    drop(taken);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_damaged_session_database_is_not_blamed_on_the_port() {
    let dir = temp_dir("db-not-port");
    let mut cfg = settings(&dir);
    cfg.connection.random_port = false;
    cfg.connection.listen_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let session = dir.join("data").join("session");
    std::fs::create_dir_all(&session).unwrap();
    // One that can't be read (a damaged one is rebuilt): librqbit fails to open it.
    std::fs::create_dir_all(session.join("session.json").join("x")).unwrap();
    let l = start_with(&dir, cfg);
    let notices = notices_until(&l, 30, |sn| matches!(sn.engine_state, super::types::EngineState::Failed(_))).await;
    let super::types::EngineState::Failed(why) = l.engine.snapshot().engine_state.clone() else {
        panic!("the engine should not start: {notices:?}")
    };
    assert!(why.contains("session"), "{why}");
    assert!(!notices.iter().any(|n| n.contains("Port ")), "{notices:?}");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_damaged_session_database_comes_back_from_its_last_good_copy() {
    let dir = temp_dir("db-restore");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let mut hashes = add_tiny_torrents(&l, &dir, 1).await;
    download(&l, &s, &dir).await;
    hashes.push(s.hash);
    let paths = stop(l).await;
    let db = paths.session_dir().join("session.json");
    let good = std::fs::read(&db).unwrap();
    let damage: [(&str, Option<Vec<u8>>); 5] = [
        ("cut off", Some(good[..good.len() / 2].to_vec())),
        ("empty", Some(Vec::new())),
        ("garbage", Some(b"not json".to_vec())),
        ("wrong types", Some(br#"{"torrents":{"0":{"info_hash":5}}}"#.to_vec())),
        ("missing", None),
    ];
    for (what, bytes) in damage {
        match bytes {
            Some(b) => std::fs::write(&db, b).unwrap(),
            None => std::fs::remove_file(&db).unwrap(),
        }
        let l = start(&dir);
        // Until each is back and done checking its data.
        let notices = notices_until(&l, 30, |sn| {
            hashes.iter().all(|h| find(sn, *h).is_some_and(|t| !t.pending && t.state != TorrentState::Checking))
        })
        .await;
        assert!(notices.iter().any(|n| n.contains("restored from a copy")), "{what}: {notices:?}");
        let snap = l.engine.snapshot();
        for h in &hashes {
            // Paused (a finished torrent that is paused shows as Completed, not Seeding).
            let state = find(&snap, *h).map(|t| t.state);
            assert!(matches!(state, Some(TorrentState::Paused | TorrentState::Completed)), "{what}: {state:?}");
        }
        stop(l).await;
    }
    assert!(!crate::util::generations(&db, "bad").is_empty());
    assert!(crate::util::generations(&db, "good").len() <= 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torrents_added_after_the_good_copy_keep_their_records() {
    let dir = temp_dir("db-newer");
    let l = start(&dir);
    let first = add_tiny_torrents(&l, &dir, 1).await;
    let paths = stop(l).await;
    let db = paths.session_dir().join("session.json");
    let older = crate::util::generations(&db, "good");
    assert_eq!(older.len(), 1);
    // A second torrent is added in a later run, and that run's copies are lost.
    let l = start(&dir);
    l.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "later.torrent".into(), bytes: multi_file_torrent_any("later") },
        torrent_bytes: None,
        save_path: dir.join("dl"),
        create_subfolder: true,
        category: Some("Later".into()),
        tags: Vec::new(),
        paused: true,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: None,
    })));
    let snap =
        wait_for(&l, "the later torrent", 30, |sn| sn.torrents.iter().any(|t| t.category.as_deref() == Some("Later")))
            .await;
    let later = snap.torrents.iter().find(|t| t.category.as_deref() == Some("Later")).unwrap().hash;
    let paths = stop(l).await;
    for (_, p) in crate::util::generations(&db, "good") {
        if !older.iter().any(|(_, o)| *o == p) {
            std::fs::remove_file(p).unwrap();
        }
    }
    std::fs::remove_file(paths.session_dir().join(format!("{}.torrent", later.to_hex()))).unwrap();
    std::fs::write(&db, b"{").unwrap();
    let l = start(&dir);
    wait_for(&l, "the first torrent", 30, |sn| find(sn, first[0]).is_some_and(|t| !t.pending)).await;
    // Pruning runs every 20 ticks (4 s here): wait it out.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(find(&l.engine.snapshot(), later).is_none(), "not in the older list");
    let paths = stop(l).await;
    let store = read_store(&paths);
    assert_eq!(store.torrents.get(&later).and_then(|m| m.category.as_deref()), Some("Later"), "record kept");
    let _ = std::fs::remove_dir_all(dir);
}

/// A small valid torrent with one 16 KiB file (any platform).
fn multi_file_torrent_any(name: &str) -> Vec<u8> {
    let mut info = format!("d6:lengthi16384e4:name{}:{name}12:piece lengthi16384e6:pieces20:", name.len()).into_bytes();
    info.extend_from_slice(&[9; 20]);
    info.push(b'e');
    let mut bytes = b"d4:info".to_vec();
    bytes.extend_from_slice(&info);
    bytes.push(b'e');
    bytes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_a_power_loss_only_recently_written_torrents_are_checked() {
    let dir = temp_dir("power-loss");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let tiny = add_tiny_torrents(&l, &dir, 1).await[0];
    download(&l, &s, &dir).await;
    let paths = stop(l).await;
    let now = crate::util::now_unix();
    // The download finished an hour ago; the tiny one was transferring just before power went.
    let mut store = read_store(&paths);
    store.torrents.get_mut(&s.hash).unwrap().completed_at = Some(now - 3600);
    let t = store.torrents.get_mut(&tiny).unwrap();
    t.completed_at = None;
    t.last_activity = now - 10;
    std::fs::write(paths.store_file(), serde_json::to_vec(&store).unwrap()).unwrap();
    let bitv = |h: InfoHash| paths.session_dir().join(format!("{}.bitv", h.to_hex()));
    assert!(bitv(tiny).exists() && bitv(s.hash).exists());
    std::fs::create_dir_all(paths.log_dir()).unwrap();
    std::fs::write(paths.log_dir().join("power-loss.pending"), now.to_string()).unwrap();

    let l = start(&dir);
    let notices = notices_until(&l, 30, |sn| find(sn, tiny).is_some_and(|t| !t.pending) && complete(sn, s.hash)).await;
    assert!(notices.iter().any(|n| n.contains("Checking 1 download")), "{notices:?}");
    assert!(!paths.log_dir().join("power-loss.pending").exists(), "taken once");
    stop(l).await;
    assert!(bitv(tiny).exists(), "rebuilt by the full check");
    assert_eq!(
        std::fs::read(find_file(&dir.join("dl"), "data.bin")).unwrap(),
        s.content,
        "the finished download is untouched"
    );

    let l = start(&dir);
    let notices = notices_until(&l, 5, |_| false).await;
    assert!(!notices.iter().any(|n| n.contains("Checking")), "{notices:?}");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// The first file named `name` under `dir`.
fn find_file(dir: &Path, name: &str) -> std::path::PathBuf {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == name) {
                return p;
            }
        }
    }
    panic!("{name} not found under {}", dir.display())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_player_that_stops_reading_frees_its_slot() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = temp_dir("stream-stall");
    // Bigger than the loopback socket buffers, so a player that doesn't read blocks the writes.
    let data: Vec<u8> = (0..48 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let s = seeder_with(&dir, "big.bin", data).await;
    let l = start(&dir);
    download(&l, &s, &dir).await;
    super::stream::TEST_WRITE_TIMEOUT_MS.store(2000, std::sync::atomic::Ordering::Relaxed);
    let (port, token) = l.engine.snapshot().session.stream.clone().expect("stream server running");
    let path = format!("/{token}/{}/0/big.bin", s.hash.to_hex());
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n");
    // Three players that ask for the whole file and then never read.
    let mut stalled = Vec::new();
    for _ in 0..3 {
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(request.as_bytes()).await.unwrap();
        stalled.push(c);
    }
    wait_for(&l, "all slots taken", 10, |sn| sn.session.stream_slots.0 == 3).await;
    async fn head(port: u16, request: &str) -> String {
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(request.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 256];
        let n = c.read(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
    let range = request.replace("\r\n\r\n", "\r\nRange: bytes=0-99\r\n\r\n");
    assert!(head(port, &range).await.starts_with("HTTP/1.1 503"), "no slot left");
    // The stalled players time out and give their slots back.
    wait_for(&l, "slots freed", 15, |sn| sn.session.stream_slots.0 == 0).await;
    assert!(head(port, &range).await.starts_with("HTTP/1.1 206"));
    super::stream::TEST_WRITE_TIMEOUT_MS.store(0, std::sync::atomic::Ordering::Relaxed);
    drop(stalled);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_disk_does_not_stall_the_engine() {
    let dir = temp_dir("slow-disk");
    let s = seeder(&dir).await;
    let watch = dir.join("watch");
    std::fs::create_dir_all(&watch).unwrap();
    std::fs::write(watch.join("slow.torrent"), &s.bytes).unwrap();
    let mut cfg = settings(&dir);
    cfg.watch.push(crate::config::WatchFolder { path: watch.clone(), start_paused: true, ..Default::default() });
    let l = start(&dir);
    wait_for(&l, "a running engine", 30, |sn| sn.engine_state == super::types::EngineState::Running).await;
    // Every free-space check and watch scan under this folder now takes 2 s.
    crate::faults::set_fs_delay(&dir, Duration::from_secs(2));
    l.engine.send(EngineCommand::ApplySettings(Box::new(cfg)));
    let (mut last, mut gap, start_at) = (l.engine.snapshot().generation, Duration::ZERO, Instant::now());
    let mut seen_at = Instant::now();
    while start_at.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let g = l.engine.snapshot().generation;
        if g != last {
            gap = gap.max(seen_at.elapsed());
            (last, seen_at) = (g, Instant::now());
        }
    }
    gap = gap.max(seen_at.elapsed());
    let snap = l.engine.snapshot();
    assert!(gap < Duration::from_secs(1), "snapshots stopped for {gap:?}");
    assert!(snap.session.tick_max_ms < 500, "slowest tick {} ms", snap.session.tick_max_ms);
    // The scan finishes late, but the torrent is still added.
    wait_for(&l, "the watched torrent", 30, |sn| find(sn, s.hash).is_some_and(|t| !t.pending)).await;
    crate::faults::clear(&dir);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_engine_call_leaves_the_engine_responsive() {
    let dir = temp_dir("stalled-call");
    let l = start(&dir);
    let h = add_tiny_torrents(&l, &dir, 1).await[0];
    // librqbit hangs on this torrent: the resume waits 5 s.
    crate::faults::set_call_delay(h.0, Duration::from_secs(5));
    l.engine.send(EngineCommand::Resume(vec![h]));
    let asked = Instant::now();
    let (tx, rx) = tokio::sync::oneshot::channel();
    l.engine.send(EngineCommand::Detail { hash: h, reply: tx });
    assert!(rx.await.unwrap().is_some());
    assert!(asked.elapsed() < Duration::from_secs(1), "the engine answered after {:?}", asked.elapsed());
    assert!(l.engine.snapshot().session.tick_max_ms < 500);
    // The call still happens, late.
    wait_for(&l, "resumed", 20, |sn| find(sn, h).is_some_and(|t| t.state != TorrentState::Paused)).await;
    crate::faults::clear_call_delay(h.0);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_and_resume_in_a_burst_end_as_last_asked() {
    let dir = temp_dir("burst");
    let l = start(&dir);
    let h = add_tiny_torrents(&l, &dir, 1).await[0];
    for _ in 0..3 {
        l.engine.send(EngineCommand::Resume(vec![h]));
        l.engine.send(EngineCommand::Pause(vec![h]));
    }
    // Let every call run, then the torrent must be paused in the engine too.
    tokio::time::sleep(Duration::from_secs(2)).await;
    wait_for(&l, "paused", 10, |sn| find(sn, h).is_some_and(|t| t.state == TorrentState::Paused)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(find(&l.engine.snapshot(), h).map(|t| t.state), Some(TorrentState::Paused));
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hanging_blocklist_server_does_not_hold_up_the_start() {
    let dir = temp_dir("blackhole");
    // Accepts connections and never answers.
    let hole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = hole.local_addr().unwrap().port();
    let keep = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((c, _)) = hole.accept().await {
            open.push(c);
        }
    });
    // A copy from an earlier start, two days old.
    let cache = dir.join("data").join("ipfilter").join("blocklist-download.txt");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, "bad:10.0.0.1-10.0.0.9\n").unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(2 * 86400);
    std::fs::File::options().write(true).open(&cache).unwrap().set_modified(old).unwrap();
    let mut cfg = settings(&dir);
    cfg.connection.blocklist_url = format!("http://127.0.0.1:{port}/list.p2p");
    std::fs::write(cache.with_extension("url"), &cfg.connection.blocklist_url).unwrap();
    let started = Instant::now();
    let l = start_with(&dir, cfg);
    wait_for(&l, "a running engine", 5, |sn| sn.engine_state == super::types::EngineState::Running).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let snap = l.engine.snapshot();
    assert_eq!(snap.start_phase, None);
    assert!(snap.session.blocked_ranges > 0, "the cached list is in use");
    stop(l).await;
    keep.abort();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_engine_that_doesnt_start_in_time_fails_with_a_reason() {
    let dir = temp_dir("start-deadline");
    crate::faults::set_start_delay(&dir, Duration::from_secs(10));
    crate::faults::set_start_deadline(&dir, Duration::from_secs(1));
    let l = start(&dir);
    let notices = notices_until(&l, 20, |sn| matches!(sn.engine_state, super::types::EngineState::Failed(_))).await;
    let super::types::EngineState::Failed(why) = l.engine.snapshot().engine_state.clone() else {
        panic!("the engine should have given up: {notices:?}")
    };
    assert!(why.contains("didn't start within 1 seconds"), "{why}");
    crate::faults::clear(&dir);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A loopback HTTP server that answers every request with `head`, then sends `body` in `chunk`
/// pieces with `pause` between them.
async fn http_server(head: String, body: Vec<u8>, chunk: usize, pause: Duration) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            let (head, body) = (head.clone(), body.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = c.read(&mut buf).await;
                if c.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for piece in body.chunks(chunk) {
                    if c.write_all(piece).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(pause).await;
                }
            });
        }
    });
    port
}

async fn add_url_error(l: &Leecher, dir: &Path, url: String) -> String {
    l.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Url(url),
        torrent_bytes: None,
        save_path: dir.join("dl"),
        create_subfolder: true,
        category: None,
        tags: Vec::new(),
        paused: true,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: None,
    })));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match l.events.try_recv() {
            Ok(EngineEvent::Toast(super::ToastKind::Error, text)) => return text,
            Ok(EngineEvent::Added { .. }) => panic!("the add went through"),
            Ok(_) => {}
            Err(_) => {
                assert!(Instant::now() < deadline, "the add neither failed nor finished");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_torrent_url_that_trickles_fails_in_time() {
    let dir = temp_dir("trickle");
    crate::faults::set_http_timeout(&dir, Duration::from_secs(3));
    // Promises a megabyte, then sends a byte a second.
    let head =
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-bittorrent\r\nContent-Length: 1000000\r\n\r\n".to_string();
    let port = http_server(head, vec![b'd'; 1000], 1, Duration::from_secs(1)).await;
    let l = start(&dir);
    let started = Instant::now();
    let err = add_url_error(&l, &dir, format!("http://127.0.0.1:{port}/slow.torrent")).await;
    assert!(started.elapsed() < Duration::from_secs(10), "failed after {:?}: {err}", started.elapsed());
    crate::faults::clear(&dir);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversized_torrent_url_is_refused() {
    let dir = temp_dir("oversized");
    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/x-bittorrent\r\n\r\n".to_string();
    let port = http_server(head, vec![b'x'; 40 << 20], 1 << 20, Duration::ZERO).await;
    let l = start(&dir);
    let err = add_url_error(&l, &dir, format!("http://127.0.0.1:{port}/huge.torrent")).await;
    assert!(err.contains("larger than 32 MiB"), "{err}");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pause_right_before_quitting_is_saved_by_the_engine() {
    let dir = temp_dir("pause-quit");
    let l = start(&dir);
    let h = add_tiny_torrents(&l, &dir, 1).await[0];
    l.engine.send(EngineCommand::Resume(vec![h]));
    wait_for(&l, "running", 20, |sn| find(sn, h).is_some_and(|t| t.state != TorrentState::Paused)).await;
    // Pause and quit at once: the queued engine call must run before the session stops, so
    // librqbit itself records the pause (not only Corrent's store).
    l.engine.send(EngineCommand::Pause(vec![h]));
    let paths = stop(l).await;
    let db: serde_json::Value =
        serde_json::from_slice(&std::fs::read(paths.session_dir().join("session.json")).unwrap()).unwrap();
    let entry = db["torrents"].as_object().unwrap().values().find(|e| e["info_hash"] == h.to_hex()).unwrap().clone();
    assert_eq!(entry["is_paused"], true, "{entry}");
    let _ = std::fs::remove_dir_all(dir);
}

/// STABILITY.md D10: a backup restored into a profile at another path (a new machine, a moved
/// portable folder) keeps its torrents; their data is found where it was, nothing downloads again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_backup_restored_elsewhere_keeps_its_torrents() {
    let dir = temp_dir("restore-elsewhere");
    let s = seeder(&dir).await;
    let a = start(&dir.join("a"));
    let t = download(&a, &s, &dir).await;
    let a_paths = stop(a).await;
    let zip = dir.join("backup.zip");
    crate::backup::export(&a_paths, &zip).unwrap();
    drop(s); // no peer from here on

    let b_dir = dir.join("b");
    let b_paths = crate::config::Paths { data_dir: b_dir.join("data"), portable: true };
    crate::backup::stage_import(&b_paths, &zip).unwrap();
    let note = crate::backup::apply_pending(&b_paths).unwrap();
    assert!(note.starts_with("Restored"), "{note}");
    // Offline test settings (the backup has no settings.toml: the engine doesn't write one).
    let b = start_with(&b_dir, settings(&b_dir));
    let snap = wait_for(&b, "the restored torrent, complete", 60, |sn| complete(sn, t.hash)).await;
    let got = find(&snap, t.hash).unwrap();
    assert_eq!(got.save_path, t.save_path, "its data stays where it was");
    assert_eq!(got.category, t.category);
    stop(b).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// `flush_blocking` off the runtime's threads.
async fn flush(l: &Leecher) {
    let engine = l.engine.clone();
    assert!(tokio::task::spawn_blocking(move || engine.flush_blocking(Duration::from_secs(5))).await.unwrap());
}

/// D7: the store keeps a copy of where each torrent's data is, its file selection and whether it
/// is paused, and follows a selection change and a move.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_store_mirrors_where_the_engine_keeps_data() {
    let dir = temp_dir("mirror");
    let data = content();
    let (a, b) = (data[..65536].to_vec(), data[65536..131072].to_vec());
    let s = seeder_files(&dir, "pack", &[("a.bin", a), ("b.bin", b)]).await;
    let l = start(&dir);
    let tiny = add_tiny_torrents(&l, &dir, 1).await[0];
    l.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "pack.torrent".into(), bytes: s.bytes.clone() },
        torrent_bytes: None,
        save_path: dir.join("dl"),
        create_subfolder: true,
        category: None,
        tags: Vec::new(),
        paused: false,
        only_files: Some(vec![0]),
        add_to_top: false,
        initial_peers: Some(vec![s.addr]),
        added_by: None,
    })));
    wait_for(&l, "the first file", 60, |sn| complete(sn, s.hash) && find(sn, s.hash).is_some_and(|t| t.size == 65536))
        .await;
    flush(&l).await;
    let store = read_store(&l.paths);
    let (m, t) = (&store.torrents[&s.hash], &store.torrents[&tiny]);
    assert_eq!(m.output_folder.as_deref(), Some(dir.join("dl").join("pack").as_path()), "a folder torrent");
    assert_eq!((m.only_files.clone(), m.paused), (Some(vec![0]), false));
    assert_eq!(t.output_folder.as_deref(), Some(dir.join("dl").as_path()), "a single file");
    assert_eq!((t.only_files.clone(), t.paused), (None, true));

    l.engine.send(EngineCommand::SetFiles { hash: s.hash, included: vec![0, 1] });
    wait_for(&l, "both files", 60, |sn| complete(sn, s.hash) && find(sn, s.hash).is_some_and(|t| t.size == 131072))
        .await;
    flush(&l).await;
    let only = read_store(&l.paths).torrents[&s.hash].only_files.clone();
    assert!(only.as_ref().is_none_or(|o| *o == vec![0, 1]), "{only:?}");

    let dest = dir.join("dl2");
    l.engine.send(EngineCommand::MoveStorage { hashes: vec![s.hash], to: dest.clone() });
    wait_for(&l, "the move", 60, |sn| {
        find(sn, s.hash).is_some_and(|t| t.save_path == dest && t.busy.is_none() && t.state == TorrentState::Seeding)
    })
    .await;
    flush(&l).await;
    let m = &read_store(&l.paths).torrents[&s.hash];
    assert_eq!(m.output_folder.as_deref(), Some(dest.join("pack").as_path()), "follows the move");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// Remove the engine's list and every good copy of it (`damaged`: garbage in its place).
fn lose_session_db(paths: &Paths, damaged: bool) {
    let db = paths.session_dir().join("session.json");
    for (_, p) in crate::util::generations(&db, "good") {
        std::fs::remove_file(p).unwrap();
    }
    if damaged {
        std::fs::write(&db, b"\x00\x00garbage").unwrap();
    } else {
        std::fs::remove_file(&db).unwrap();
    }
}

/// D6b: with the list and every copy of it gone, it is rebuilt from the metainfo and the store:
/// every torrent comes back paused, in its folder, with its file selection and records, and its
/// data is verified, never downloaded again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_session_database_is_rebuilt_from_the_torrent_files_and_the_store() {
    let dir = temp_dir("db-rebuild");
    let s = seeder(&dir).await;
    let data = content();
    let files = [("a.bin", data[..65536].to_vec()), ("b.bin", data[65536..131072].to_vec())];
    let pack = seeder_files(&dir, "pack", &files).await;
    let l = start(&dir);
    let tiny = add_tiny_torrents(&l, &dir, 1).await[0];
    download(&l, &s, &dir).await;
    l.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "pack.torrent".into(), bytes: pack.bytes.clone() },
        torrent_bytes: None,
        save_path: dir.join("dl"),
        create_subfolder: true,
        category: Some("Packs".into()),
        tags: vec!["two".into()],
        paused: false,
        only_files: Some(vec![0]),
        add_to_top: false,
        initial_peers: Some(vec![pack.addr]),
        added_by: None,
    })));
    wait_for(&l, "the first file", 60, |sn| {
        complete(sn, pack.hash) && find(sn, pack.hash).is_some_and(|t| t.size == 65536)
    })
    .await;
    let dl2 = dir.join("dl2");
    l.engine.send(EngineCommand::MoveStorage { hashes: vec![pack.hash], to: dl2.clone() });
    wait_for(&l, "the move", 60, |sn| {
        find(sn, pack.hash).is_some_and(|t| t.save_path == dl2 && t.busy.is_none() && t.state == TorrentState::Seeding)
    })
    .await;
    let paths = stop(l).await;
    // Nothing to download from from now on.
    s._session.stop().await;
    pack._session.stop().await;
    let before = read_store(&paths);
    let hashes = [tiny, s.hash, pack.hash];
    for damaged in [true, false] {
        lose_session_db(&paths, damaged);
        let l = start(&dir);
        let notices = notices_until(&l, 30, |sn| {
            hashes.iter().all(|h| find(sn, *h).is_some_and(|t| t.busy.is_none() && t.state != TorrentState::Checking))
        })
        .await;
        assert!(notices.iter().any(|n| n.contains("rebuilt from the torrent files")), "{damaged}: {notices:?}");
        let snap = l.engine.snapshot();
        for h in &hashes {
            let t = find(&snap, *h).unwrap();
            assert!(matches!(t.state, TorrentState::Paused | TorrentState::Completed), "{damaged}: {:?}", t.state);
            assert_eq!(t.category, before.torrents[h].category, "{damaged}: records kept");
            assert_eq!(t.tags, before.torrents[h].tags);
        }
        l.engine.send(EngineCommand::ResumeAll);
        let snap = wait_for(&l, "seeding again", 60, |sn| complete(sn, s.hash) && complete(sn, pack.hash)).await;
        assert_eq!(find(&snap, s.hash).unwrap().save_path, dir.join("dl"));
        assert_eq!(find(&snap, pack.hash).unwrap().save_path, dl2, "where it was moved to");
        let (tx, rx) = tokio::sync::oneshot::channel();
        l.engine.send(EngineCommand::FileList { hash: pack.hash, reply: tx });
        let included: Vec<bool> = rx.await.unwrap().unwrap().iter().map(|f| f.included).collect();
        assert_eq!(included, vec![true, false], "{damaged}: the file selection");
        let paths = stop(l).await;
        let after = read_store(&paths);
        for h in [s.hash, pack.hash] {
            assert_eq!(after.torrents[&h].downloaded_total, before.torrents[&h].downloaded_total, "nothing downloaded");
        }
    }
    assert!(!crate::util::generations(&paths.session_dir().join("session.json"), "bad").is_empty());
    // The next start is an ordinary one.
    let l = start(&dir);
    let notices =
        notices_until(&l, 10, |sn| hashes.iter().all(|h| find(sn, *h).is_some_and(|t| t.busy.is_none()))).await;
    assert!(!notices.iter().any(|n| n.contains("torrent list")), "{notices:?}");
    let paths = stop(l).await;
    assert!(!crate::util::generations(&paths.session_dir().join("session.json"), "good").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// D6b without Corrent's records: metainfo alone comes back paused in the download folder; a
/// record whose metainfo is gone shows as a failed load and keeps its record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebuild_without_records_or_metainfo_loses_nothing_it_can_keep() {
    let dir = temp_dir("db-rebuild-partial");
    let l = start(&dir);
    let hashes = add_tiny_torrents(&l, &dir, 2).await;
    let paths = stop(l).await;
    let (kept, gone) = (hashes[0], hashes[1]);
    std::fs::remove_file(paths.session_dir().join(format!("{}.torrent", gone.to_hex()))).unwrap();
    lose_session_db(&paths, false);
    let l = start(&dir);
    let notices = notices_until(&l, 30, |sn| {
        find(sn, kept).is_some_and(|t| t.busy.is_none()) && find(sn, gone).is_some_and(|t| t.restore_failed)
    })
    .await;
    assert!(notices.iter().any(|n| n.contains("file is missing")), "{notices:?}");
    let snap = l.engine.snapshot();
    let row = find(&snap, gone).unwrap();
    assert_eq!(row.state, TorrentState::Error);
    assert!(row.error.as_deref().is_some_and(|e| e.contains("torrent file is missing")), "{:?}", row.error);
    assert_eq!(row.category.as_deref(), Some("Kept"));
    // Pruning runs every 20 ticks (4 s here): the record of the failed one stays.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let paths = stop(l).await;
    assert!(read_store(&paths).torrents.contains_key(&gone), "record kept");

    // Now without the store as well.
    for e in std::fs::read_dir(&paths.data_dir).unwrap().flatten() {
        if e.file_name().to_string_lossy().starts_with("store.json") {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
    lose_session_db(&paths, true);
    let l = start(&dir);
    // Checked in full: its saved piece state was for a folder that is only a guess.
    let notices = notices_until(&l, 30, |sn| {
        find(sn, kept).is_some_and(|t| t.busy.is_none() && t.state != TorrentState::Checking)
    })
    .await;
    assert!(notices.iter().any(|n| n.contains("no recorded folder")), "{notices:?}");
    let t = find(&l.engine.snapshot(), kept).unwrap().clone();
    assert_eq!(t.state, TorrentState::Paused);
    assert_eq!(t.save_path, dir.join("dl"), "the download folder");
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}
