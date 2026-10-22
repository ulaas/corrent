//! `CORRENT_DEV_DEMO=1` (debug builds only): a made-up snapshot for the website's screenshots.
//! Freely licensed torrents, invented folders and peers from the documentation address ranges
//! (RFC 5737), so a screenshot shows nothing about the machine it was taken on.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::types::{
    EngineState, FileView, InfoHash, PeerView, SessionView, Snapshot, TorrentDetail, TorrentState, TorrentView,
};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// A smooth, made-up speed curve: `n` samples around `base` bytes/s.
fn wave(n: usize, base: f64, seed: f64) -> impl Iterator<Item = u64> {
    (0..n).map(move |i| {
        let x = i as f64 + seed * 17.0;
        let v = base * (1.0 + 0.35 * (x / 7.0).sin() + 0.2 * (x / 2.3 + seed).sin() + 0.1 * (x / 1.1).cos());
        v.max(0.0) as u64
    })
}

struct Row {
    name: &'static str,
    state: TorrentState,
    size: u64,
    progress: f32,
    down: u64,
    up: u64,
    peers: u32,
    category: Option<&'static str>,
    tags: &'static [&'static str],
    folder: &'static str,
    tracker: &'static str,
    files: usize,
}

const ROWS: &[Row] = &[
    Row {
        name: "Sintel (2010) 4K",
        state: TorrentState::Downloading,
        size: 9 * GIB + 412 * MIB,
        progress: 0.64,
        down: 18_400_000,
        up: 1_250_000,
        peers: 48,
        category: Some("Movies/Open movies"),
        tags: &["blender"],
        folder: "D:\\Media\\Movies",
        tracker: "udp://tracker.opentrackr.org:1337/announce",
        files: 6,
    },
    Row {
        name: "debian-13.1.0-amd64-DVD-1.iso",
        state: TorrentState::Downloading,
        size: 3 * GIB + 980 * MIB,
        progress: 0.31,
        down: 9_800_000,
        up: 640_000,
        peers: 112,
        category: Some("Linux"),
        tags: &["iso"],
        folder: "D:\\Downloads\\ISO",
        tracker: "http://bttracker.debian.org:6969/announce",
        files: 1,
    },
    Row {
        name: "Big Buck Bunny (2008) 1080p",
        state: TorrentState::Seeding,
        size: 885 * MIB,
        progress: 1.0,
        down: 0,
        up: 2_100_000,
        peers: 23,
        category: Some("Movies/Open movies"),
        tags: &["blender"],
        folder: "D:\\Media\\Movies",
        tracker: "udp://tracker.opentrackr.org:1337/announce",
        files: 3,
    },
    Row {
        name: "ubuntu-24.04.3-desktop-amd64.iso",
        state: TorrentState::Seeding,
        size: 5 * GIB + 900 * MIB,
        progress: 1.0,
        down: 0,
        up: 3_400_000,
        peers: 61,
        category: Some("Linux"),
        tags: &["iso"],
        folder: "D:\\Downloads\\ISO",
        tracker: "https://torrent.ubuntu.com/announce",
        files: 1,
    },
    Row {
        name: "Tears of Steel (2012) 4K",
        state: TorrentState::Downloading,
        size: 6 * GIB + 210 * MIB,
        progress: 0.87,
        down: 6_300_000,
        up: 420_000,
        peers: 19,
        category: Some("Movies/Open movies"),
        tags: &["blender"],
        folder: "D:\\Media\\Movies",
        tracker: "udp://tracker.opentrackr.org:1337/announce",
        files: 4,
    },
    Row {
        name: "Cosmos Laundromat (2015)",
        state: TorrentState::Queued,
        size: 2 * GIB + 330 * MIB,
        progress: 0.0,
        down: 0,
        up: 0,
        peers: 0,
        category: Some("Movies/Open movies"),
        tags: &["blender"],
        folder: "D:\\Media\\Movies",
        tracker: "udp://tracker.opentrackr.org:1337/announce",
        files: 2,
    },
    Row {
        name: "Fedora-Workstation-Live-x86_64-43.iso",
        state: TorrentState::Completed,
        size: 2 * GIB + 280 * MIB,
        progress: 1.0,
        down: 0,
        up: 0,
        peers: 0,
        category: Some("Linux"),
        tags: &["iso"],
        folder: "D:\\Downloads\\ISO",
        tracker: "http://torrent.fedoraproject.org:6969/announce",
        files: 1,
    },
    Row {
        name: "Night of the Living Dead (1968)",
        state: TorrentState::Paused,
        size: GIB + 470 * MIB,
        progress: 0.42,
        down: 0,
        up: 0,
        peers: 0,
        category: Some("Movies/Classics"),
        tags: &["public domain"],
        folder: "D:\\Media\\Movies",
        tracker: "https://bt1.archive.org/announce",
        files: 2,
    },
    Row {
        name: "Elephants Dream (2006)",
        state: TorrentState::Seeding,
        size: 815 * MIB,
        progress: 1.0,
        down: 0,
        up: 380_000,
        peers: 7,
        category: Some("Movies/Open movies"),
        tags: &["blender"],
        folder: "D:\\Media\\Movies",
        tracker: "udp://tracker.opentrackr.org:1337/announce",
        files: 3,
    },
    Row {
        name: "archlinux-2026.10.01-x86_64.iso",
        state: TorrentState::Metadata,
        size: 0,
        progress: 0.0,
        down: 0,
        up: 0,
        peers: 4,
        category: Some("Linux"),
        tags: &[],
        folder: "D:\\Downloads\\ISO",
        tracker: "udp://tracker.archlinux.org:6969/announce",
        files: 0,
    },
];

/// When the demo started (the clock the made-up dates count from).
const NOW: i64 = 1_792_656_000;

/// The demo's snapshot; the details panel shows `selected` (or the first torrent).
pub fn snapshot(selected: Option<InfoHash>) -> Snapshot {
    let mut torrents = Vec::new();
    let mut row_history = HashMap::new();
    for (i, r) in ROWS.iter().enumerate() {
        let hash = InfoHash([i as u8 * 23 + 7; 20]);
        let done = (r.size as f64 * f64::from(r.progress)) as u64;
        let eta = (r.down > 0).then(|| Duration::from_secs((r.size - done) / r.down.max(1)));
        let downloaded = done;
        let uploaded = match r.state {
            TorrentState::Seeding => r.size * (2 + i as u64 % 3),
            _ => done / 6,
        };
        let content = PathBuf::from(r.folder).join(r.name);
        torrents.push(TorrentView {
            id: i,
            hash,
            name: r.name.into(),
            state: r.state,
            size: r.size,
            done,
            progress: r.progress,
            uploaded,
            downloaded,
            down_bps: r.down,
            up_bps: r.up,
            eta,
            ratio: if downloaded > 0 { uploaded as f32 / downloaded as f32 } else { 0.0 },
            peers_live: r.peers,
            peers_seen: r.peers * 3 + 11,
            save_path: r.folder.into(),
            content_path: content,
            category: r.category.map(Into::into),
            tags: r.tags.iter().map(|t| t.to_string()).collect(),
            added_at: NOW - 3600 * (i as i64 * 7 + 2),
            completed_at: (r.progress >= 1.0).then_some(NOW - 3600 * (i as i64 * 5 + 1)),
            queue_pos: i as u32,
            trackers: vec![r.tracker.into()],
            has_metadata: r.state != TorrentState::Metadata,
            file_count: r.files,
            piece_count: (r.size / (4 * MIB)).max(1) as u32,
            piece_size: 4 * MIB,
            comment: r
                .name
                .contains("Sintel")
                .then(|| "Sintel, an open movie by the Blender Foundation (CC BY 3.0)".into()),
            created_by: Some("mktorrent 1.1".into()),
            creation_date: Some(NOW - 86_400 * 30),
            last_activity: NOW - 60,
            seeding_secs: if r.state == TorrentState::Seeding { 86_400 * (i as u64 + 1) } else { 0 },
            ..Default::default()
        });
        let hist: Vec<(u32, u32)> = wave(40, r.down as f64 / 1024.0, i as f64)
            .zip(wave(40, r.up as f64 / 1024.0, i as f64 + 3.0))
            .map(|(d, u)| (d as u32, u as u32))
            .collect();
        row_history.insert(hash, hist);
    }
    let down: u64 = ROWS.iter().map(|r| r.down).sum();
    let up: u64 = ROWS.iter().map(|r| r.up).sum();
    let history: VecDeque<(u64, u64)> =
        wave(300, down as f64, 1.0).zip(wave(300, up as f64, 5.0)).collect::<VecDeque<_>>();
    let mut categories = BTreeMap::new();
    for c in ["Linux", "Movies", "Movies/Classics", "Movies/Open movies", "Music"] {
        categories.insert(c.to_string(), Default::default());
    }
    let shown = selected.or(torrents.first().map(|t| t.hash)).unwrap_or_default();
    Snapshot {
        torrents,
        detail: Some(detail(shown)),
        session: SessionView {
            down_bps: down,
            up_bps: up,
            dht_nodes: Some(412),
            listen_port: Some(51413),
            peers_live: ROWS.iter().map(|r| r.peers).sum(),
            uptime_secs: 3 * 3600 + 1240,
            session_downloaded: 38 * GIB,
            session_uploaded: 21 * GIB,
            ..Default::default()
        },
        engine_state: EngineState::Running,
        history,
        // Far from the engine's own count, which the list's caches compare against.
        generation: 1 << 40,
        categories,
        tags: vec!["blender".into(), "iso".into(), "public domain".into()],
        all_time_downloaded: 1_840 * GIB,
        all_time_uploaded: 2_310 * GIB,
        free_space: Some(1_204 * GIB),
        row_history: Arc::new(row_history),
        ..Default::default()
    }
}

/// Files, peers and pieces (Sintel's, whichever torrent is selected).
fn detail(hash: InfoHash) -> TorrentDetail {
    let files = [
        ("sintel-2160p.mkv", 8 * GIB + 900 * MIB, 0.62),
        ("sintel-1080p.mp4", 480 * MIB, 1.0),
        ("Subtitles/sintel_en.srt", 12_000, 1.0),
        ("Subtitles/sintel_de.srt", 13_000, 1.0),
        ("Subtitles/sintel_tr.srt", 12_500, 0.0),
        ("poster.jpg", 2 * MIB, 1.0),
    ];
    let files = files
        .iter()
        .enumerate()
        .map(|(i, (path, len, p))| FileView {
            index: i,
            components: path.split('/').map(Into::into).collect(),
            len: *len,
            done: (*len as f64 * p) as u64,
            included: true,
            priority: if i == 0 { 1 } else { 0 },
        })
        .collect();
    let clients = [
        "qBittorrent 5.1.2",
        "Transmission 4.0.6",
        "Deluge 2.2.0",
        "libtorrent 2.0.11",
        "Corrent 1.0.0",
        "rqbit 9.0.1",
    ];
    let peers = (0..18u64)
        .map(|i| {
            let net = ["192.0.2", "198.51.100", "203.0.113"].get(i as usize % 3).copied().unwrap_or_default();
            PeerView {
                addr: format!("{net}.{}:{}", 11 + i * 13 % 230, 50_000 + i * 977 % 15_000),
                client: clients.get(i as usize % clients.len()).map(|c| c.to_string()),
                kind: if i % 4 == 0 { "uTP".into() } else { "TCP".into() },
                state: "live".into(),
                downloaded: (40 + i * 37 % 300) * MIB,
                uploaded: (3 + i * 11 % 60) * MIB,
                down_bps: 1_800_000 / (i + 1) + 40_000,
                up_bps: if i % 3 == 0 { 120_000 / (i + 1) } else { 0 },
            }
        })
        .collect();
    let count = 2_354u32;
    let mut bits = vec![0u8; count.div_ceil(8) as usize];
    for i in 0..count {
        // Mostly done from the start (sequential for streaming), scattered after that.
        let have = i < 1_300 || i.wrapping_mul(2_654_435_761) % 7 < 2;
        if have && let Some(b) = bits.get_mut((i / 8) as usize) {
            *b |= 0x80 >> (i % 8);
        }
    }
    TorrentDetail {
        hash,
        files,
        peers,
        pieces: Some((bits, count)),
        history: wave(300, 18_400_000.0, 2.0).zip(wave(300, 1_250_000.0, 4.0)).collect(),
    }
}
