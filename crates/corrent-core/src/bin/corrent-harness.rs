//! Durability harness (STABILITY.md D8): drives a Corrent engine with no UI through a scripted
//! workload, so a test can end it at any write (`faults::kill_point`, `CORRENT_DEV_KILL_AT`) and
//! check after a restart that nothing was lost. Built only with the `testkit` feature; never
//! shipped.
//!
//! ```text
//! corrent-harness run <scratch>      the workload; exits 0, or 75 when killed
//! corrent-harness verify <scratch>   starts like the app and checks <scratch>/expect.json
//! ```
//!
//! `<scratch>/data` is the profile (portable layout), `<scratch>/expect.json` the journal: what the
//! workload saw durably written, so a restart must find it.
// Test tooling: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use corrent_core::config::{Paths, Settings};
use corrent_core::engine::testkit::{self, Seeder};
use corrent_core::engine::types::{AddSource, InfoHash, Snapshot, TorrentState};
use corrent_core::engine::{AddRequest, EngineCommand, EngineHandle, Wake};
use serde::{Deserialize, Serialize};

/// What a restart must find.
#[derive(Default, Serialize, Deserialize)]
struct Journal {
    /// `queue.max_active_downloads` of the last settings save that returned.
    settings_marker: Option<u32>,
    /// The one being saved (a kill can land after its rename, before the journal is updated).
    settings_saving: Option<u32>,
    /// Torrents in the engine's database (librqbit flushes it before an add answers).
    engine: Vec<String>,
    /// Categories saved by a flush of the store.
    category: BTreeMap<String, String>,
    /// The download whose `.bitv` was seen complete on disk, and where its files are.
    finished: Option<(String, PathBuf)>,
    /// Removed for good.
    deleted: Vec<String>,
    /// Being removed: may or may not come back.
    deleting: Option<String>,
    /// The finished download is being moved here: it may be in either folder.
    moving_to: Option<PathBuf>,
}

fn journal_path(scratch: &Path) -> PathBuf {
    scratch.join("expect.json")
}

fn load_journal(scratch: &Path) -> Journal {
    std::fs::read(journal_path(scratch)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Atomically, without kill points (the harness's own bookkeeping isn't under test).
fn save_journal(scratch: &Path, j: &Journal) {
    let tmp = scratch.join("expect.json.part");
    std::fs::write(&tmp, serde_json::to_vec_pretty(j).unwrap()).unwrap();
    std::fs::rename(&tmp, journal_path(scratch)).unwrap();
}

fn paths(scratch: &Path) -> Paths {
    Paths { data_dir: scratch.join("data"), portable: true }
}

/// The app's start order (src/main.rs): lock, temp sweep, restore, settings.
fn open_profile(scratch: &Path) -> (corrent_core::lock::DataLock, Paths, Settings, bool, Option<String>) {
    let paths = paths(scratch);
    let lock = corrent_core::lock::lock_data_dir(&paths.data_dir).expect("lock the profile");
    corrent_core::retention::sweep_temp_files(&paths);
    if let Some(note) = corrent_core::backup::apply_pending(&paths) {
        eprintln!("restore: {note}");
    }
    let (settings, first_run, warning) = Settings::load(&paths);
    (lock, paths, settings, first_run, warning)
}

fn start(
    paths: &Paths,
    settings: Settings,
) -> (EngineHandle, std::sync::mpsc::Receiver<corrent_core::engine::EngineEvent>) {
    let (tx, events) = std::sync::mpsc::channel();
    let engine = EngineHandle::start(&tokio::runtime::Handle::current(), paths.clone(), settings, Wake::noop(), tx);
    (engine, events)
}

async fn wait_for(
    engine: &EngineHandle,
    what: &str,
    secs: u64,
    pred: impl Fn(&Snapshot) -> bool,
) -> std::sync::Arc<Snapshot> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let snap = engine.snapshot();
        if pred(&snap) {
            return snap;
        }
        if Instant::now() >= deadline {
            let rows: Vec<String> =
                snap.torrents.iter().map(|t| format!("{}={:?}/{:?}/{:?}", t.name, t.state, t.busy, t.error)).collect();
            eprintln!("timed out waiting for: {what}; engine {:?}; torrents {rows:?}", snap.engine_state);
            std::process::exit(1);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn present(snap: &Snapshot, hex: &str) -> bool {
    snap.torrents.iter().any(|t| !t.pending && t.hash.to_hex() == hex)
}

async fn flush(engine: &EngineHandle) {
    let e = engine.clone();
    let ok = tokio::task::spawn_blocking(move || e.flush_blocking(Duration::from_secs(20))).await.unwrap();
    assert!(ok, "the engine didn't flush");
}

async fn shutdown(engine: EngineHandle) {
    tokio::task::spawn_blocking(move || engine.shutdown_blocking(Duration::from_secs(20))).await.unwrap();
}

/// A tiny paused single-file torrent (made-up piece hash: nothing to download).
fn tiny(i: u8) -> Vec<u8> {
    let name = format!("tiny-{i:03}.bin");
    let mut info = b"d6:lengthi16384e4:name".to_vec();
    info.extend_from_slice(format!("{}:{name}", name.len()).as_bytes());
    info.extend_from_slice(b"12:piece lengthi16384e6:pieces20:");
    info.extend_from_slice(&[i.wrapping_add(100); 20]);
    info.push(b'e');
    let mut bytes = b"d4:info".to_vec();
    bytes.extend_from_slice(&info);
    bytes.push(b'e');
    bytes
}

fn add(engine: &EngineHandle, bytes: Vec<u8>, save: &Path, paused: bool, peers: Option<Vec<std::net::SocketAddr>>) {
    engine.send(EngineCommand::Add(Box::new(AddRequest {
        source: AddSource::Bytes { name: "harness.torrent".into(), bytes },
        torrent_bytes: None,
        save_path: save.to_path_buf(),
        create_subfolder: false,
        category: None,
        tags: Vec::new(),
        paused,
        only_files: None,
        add_to_top: false,
        initial_peers: peers,
        added_by: None,
    })));
}

/// The `.bitv` of `hash` on disk has every piece of a `pieces`-piece torrent.
fn bitv_complete(paths: &Paths, hash: &InfoHash, pieces: u32) -> bool {
    let hex = hash.to_hex();
    std::fs::read_dir(paths.session_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.ends_with(".bitv") && n.contains(&hex)
        })
        .any(|e| std::fs::read(e.path()).is_ok_and(|b| b.iter().map(|x| x.count_ones()).sum::<u32>() >= pieces))
}

fn settings_with_marker(base: Settings, marker: u32) -> Settings {
    let mut s = base;
    s.queue.max_active_downloads = marker;
    s
}

/// Make `settings.toml.bak` look two hours old, so the next save refreshes it.
fn age_bak(paths: &Paths) {
    let bak = paths.settings_file().with_extension("toml.bak");
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&bak) {
        let _ = f.set_modified(std::time::SystemTime::now() - Duration::from_secs(7200));
    }
}

/// Copy `from` into `to`, with the fixtures' `{{DL}}` replaced by `dl`.
fn copy_fixture(from: &Path, to: &Path, dl: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let ext = src.extension().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        if src.is_dir() {
            copy_fixture(&src, &dst, dl);
        } else if ext == "json" || ext == "toml" {
            let text = std::fs::read_to_string(&src).unwrap();
            std::fs::write(&dst, text.replace("{{DL}}", &dl.display().to_string().replace('\\', "/"))).unwrap();
        } else if ext != "md" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// The profile starts as the v1 fixture with a backup of the v0 fixture staged over it: the
/// run first restores (renames into place) and migrates (dated copies) before anything else.
fn stage_old_profile(scratch: &Path, j: &mut Journal) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let dl = scratch.join("fixture-dl");
    let live = paths(scratch);
    copy_fixture(&fixtures.join("profile-v1"), &live.data_dir, &dl);
    let old = Paths { data_dir: scratch.join("v0/data"), portable: true };
    copy_fixture(&fixtures.join("profile-v0"), &old.data_dir, &dl);
    let zip = scratch.join("v0.zip");
    corrent_core::backup::export(&old, &zip).unwrap();
    corrent_core::backup::stage_import(&live, &zip).unwrap();
    let store: serde_json::Value = serde_json::from_slice(&std::fs::read(old.store_file()).unwrap()).unwrap();
    j.engine.extend(store["torrents"].as_object().unwrap().keys().cloned());
}

/// No network: the fixtures' feeds point at a made-up host.
fn offline(mut s: Settings) -> Settings {
    s.rss.feeds.clear();
    s
}

async fn run(scratch: &Path) {
    let seeder: Seeder = testkit::seeder(&scratch.join("seed")).await;
    let mut j = Journal::default();
    if !scratch.join("data").exists() {
        stage_old_profile(scratch, &mut j);
        save_journal(scratch, &j);
    }
    let (_lock, paths, settings, _, _) = open_profile(scratch);
    let settings = offline(settings);
    // Settings: first save, then one that refreshes the .bak.
    let s = settings_with_marker(settings.clone(), 3);
    j.settings_saving = Some(3);
    save_journal(scratch, &j);
    s.save(&paths).unwrap();
    j.settings_marker = Some(3);
    save_journal(scratch, &j);
    age_bak(&paths);
    let s = settings_with_marker(s, 4);
    j.settings_saving = Some(4);
    save_journal(scratch, &j);
    s.save(&paths).unwrap();
    j.settings_marker = Some(4);
    save_journal(scratch, &j);

    let (engine, _events) = start(&paths, s.clone());
    wait_for(&engine, "the engine", 60, |sn| sn.session.listen_port.is_some()).await;
    let dl = scratch.join("dl");

    // Ten tiny paused torrents, each journaled once the engine lists it.
    for i in 0..10u8 {
        add(&engine, tiny(i), &dl, true, None);
        let name = format!("tiny-{i:03}.bin");
        let snap =
            wait_for(&engine, "a tiny torrent", 30, |sn| sn.torrents.iter().any(|t| !t.pending && t.name == name))
                .await;
        let hex = snap.torrents.iter().find(|t| !t.pending && t.name == name).map(|t| t.hash.to_hex()).unwrap();
        j.engine.push(hex);
        save_journal(scratch, &j);
    }

    // Categories, saved by a flush.
    let hashes: Vec<InfoHash> = j.engine.iter().filter_map(|h| InfoHash::from_hex(h)).collect();
    let (even, odd): (Vec<InfoHash>, Vec<InfoHash>) =
        hashes.iter().enumerate().fold((vec![], vec![]), |mut acc, (i, h)| {
            if i % 2 == 0 {
                acc.0.push(*h)
            } else {
                acc.1.push(*h)
            }
            acc
        });
    engine.send(EngineCommand::SetCategory { hashes: even.clone(), category: Some("Even".into()) });
    engine.send(EngineCommand::SetCategory { hashes: odd.clone(), category: Some("Odd".into()) });
    wait_for(&engine, "categories", 30, |sn| {
        sn.torrents.iter().filter(|t| t.category.is_some()).count() == hashes.len()
    })
    .await;
    flush(&engine).await;
    for h in &even {
        j.category.insert(h.to_hex(), "Even".into());
    }
    for h in &odd {
        j.category.insert(h.to_hex(), "Odd".into());
    }
    save_journal(scratch, &j);

    // A real download from the loopback seeder.
    add(&engine, seeder.bytes.clone(), &dl, false, Some(vec![seeder.addr]));
    let hex = seeder.hash.to_hex();
    wait_for(&engine, "the download", 120, |sn| testkit::complete(sn, seeder.hash)).await;
    j.engine.push(hex.clone());
    save_journal(scratch, &j);
    let pieces = testkit::SIZE.div_ceil(64 * 1024) as u32;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !bitv_complete(&paths, &seeder.hash, pieces) {
        assert!(Instant::now() < deadline, "the .bitv never showed the finished download");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    j.finished = Some((hex.clone(), dl.clone()));
    save_journal(scratch, &j);

    // Churn: pause and resume, a speed limit (re-adds the torrent), a delete, a move.
    engine.send(EngineCommand::Resume(vec![hashes[1]]));
    engine.send(EngineCommand::Pause(vec![hashes[1]]));
    engine.send(EngineCommand::SetTorrentLimits { hashes: vec![hashes[2]], down_kib: 100, up_kib: 50 });
    wait_for(&engine, "the re-add", 60, |sn| {
        sn.torrents.iter().any(|t| t.hash == hashes[2] && !t.pending && t.busy.is_none())
    })
    .await;
    j.deleting = Some(hashes[3].to_hex());
    save_journal(scratch, &j);
    engine.send(EngineCommand::Delete { hashes: vec![hashes[3]], with_data: false });
    wait_for(&engine, "the delete", 30, |sn| !sn.torrents.iter().any(|t| t.hash == hashes[3])).await;
    j.engine.retain(|h| *h != hashes[3].to_hex());
    j.category.remove(&hashes[3].to_hex());
    j.deleted.push(hashes[3].to_hex());
    save_journal(scratch, &j);
    let dl2 = scratch.join("dl2");
    j.moving_to = Some(dl2.clone());
    save_journal(scratch, &j);
    engine.send(EngineCommand::MoveStorage { hashes: vec![seeder.hash], to: dl2.clone() });
    wait_for(&engine, "the move", 120, |sn| {
        sn.torrents.iter().any(|t| {
            t.hash == seeder.hash && t.save_path == dl2 && t.busy.is_none() && t.state == TorrentState::Seeding
        })
    })
    .await;
    j.finished = Some((hex, dl2));
    save_journal(scratch, &j);
    flush(&engine).await;
    shutdown(engine).await;
}

/// Start like the app and check the journal. Prints every problem; exit code 1 if any.
async fn verify(scratch: &Path) {
    let j = load_journal(scratch);
    let (_lock, paths, settings, _, warning) = open_profile(scratch);
    let mut bad = false;
    macro_rules! problem {
        ($($arg:tt)*) => {{
            eprintln!("problem: {}", format_args!($($arg)*));
            bad = true;
        }};
    }
    if let Some(w) = &warning {
        eprintln!("settings: {w}");
    }
    let got = settings.queue.max_active_downloads;
    if let Some(m) = j.settings_marker
        && got != m
        && Some(got) != j.settings_saving
    {
        problem!("settings marker {got}, expected {m} or {:?}", j.settings_saving);
    }
    let (engine, events) = start(&paths, offline(settings));
    // Every journaled torrent comes back (re-adds interrupted by the kill are recovered from their
    // backup in the background, so this waits).
    let wanted: Vec<String> = j.engine.iter().filter(|h| Some(*h) != j.deleting.as_ref()).cloned().collect();
    let snap = wait_for(&engine, "every journaled torrent", 120, |sn| {
        wanted.iter().all(|h| present(sn, h))
            && sn.torrents.iter().all(|t| t.busy.is_none() || t.state == TorrentState::Error)
    })
    .await;
    for t in &snap.torrents {
        if t.state == TorrentState::Error {
            problem!("{} errored: {:?}", t.name, t.error);
        }
    }
    for h in &j.deleted {
        if present(&snap, h) {
            problem!("deleted {h} came back");
        }
    }
    for (h, cat) in &j.category {
        if Some(h) == j.deleting.as_ref() {
            continue;
        }
        let got = snap.torrents.iter().find(|t| t.hash.to_hex() == *h).and_then(|t| t.category.clone());
        if got.as_deref() != Some(cat.as_str()) {
            problem!("{h}: category {got:?} != {cat}");
        }
    }
    if let Some((hex, dir)) = &j.finished {
        // Complete again without any peer: nothing is downloaded again.
        let h = InfoHash::from_hex(hex).unwrap();
        let snap = wait_for(&engine, "the finished download", 120, |sn| {
            sn.torrents.iter().any(|t| t.hash == h && t.busy.is_none() && t.state != TorrentState::Checking)
        })
        .await;
        let t = snap.torrents.iter().find(|t| t.hash == h).unwrap();
        if t.state != TorrentState::Seeding {
            problem!("the finished download is {:?} ({:?})", t.state, t.error);
        }
        if t.save_path != *dir && Some(&t.save_path) != j.moving_to.as_ref() {
            problem!("the finished download is in {} instead of {}", t.save_path.display(), dir.display());
        }
        let data = std::fs::read(t.save_path.join("data.bin")).unwrap_or_default();
        if data != testkit::content() {
            problem!("the data in {} differs ({} bytes)", t.save_path.display(), data.len());
        }
    }
    // Files of a move are in one place only.
    for d in ["dl", "dl2"] {
        for part in std::fs::read_dir(scratch.join(d)).into_iter().flatten().flatten() {
            if part.file_name().to_string_lossy().ends_with(".part") {
                problem!("left over: {}", part.path().display());
            }
        }
    }
    let in_dl = scratch.join("dl/data.bin").exists();
    let in_dl2 = scratch.join("dl2/data.bin").exists();
    if in_dl && in_dl2 {
        problem!("data.bin is in dl and dl2");
    }
    while let Ok(e) = events.try_recv() {
        if let corrent_core::engine::EngineEvent::DataWarning(w) = e {
            eprintln!("warning: {w}");
        }
    }
    shutdown(engine).await;
    if bad {
        std::process::exit(1);
    }
    println!("verify: ok");
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    if let Some(note) = corrent_core::faults::compiled_in() {
        eprintln!("{note}");
    }
    corrent_core::faults::init_kill_points();
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(scratch)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: corrent-harness run|verify <scratch>");
        std::process::exit(2);
    };
    let scratch = PathBuf::from(scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    match mode.as_str() {
        "run" => run(&scratch).await,
        "verify" => verify(&scratch).await,
        _ => {
            eprintln!("unknown mode {mode}");
            std::process::exit(2);
        }
    }
}
