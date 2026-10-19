use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use corrent_proto::{InfoHash, TorrentState};

#[derive(Clone, Debug, Default)]
pub struct TorrentView {
    pub id: usize,
    pub hash: InfoHash,
    pub name: String,
    pub state: TorrentState,
    pub error: Option<String>,
    pub size: u64,
    pub done: u64,
    pub progress: f32,
    pub uploaded: u64,
    pub downloaded: u64,
    pub down_bps: u64,
    pub up_bps: u64,
    pub eta: Option<Duration>,
    pub ratio: f32,
    pub peers_live: u32,
    pub peers_seen: u32,
    pub peers_connecting: u32,
    /// Folder the user chose (parent of the content folder for multi-file torrents).
    pub save_path: PathBuf,
    /// The file (single-file torrent) or folder (multi-file torrent) holding the data.
    pub content_path: PathBuf,
    pub category: Option<String>,
    pub tags: Vec<String>,
    /// The server user who added it (None for the desktop app, RSS and watch folders).
    pub added_by: Option<String>,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub queue_pos: u32,
    pub trackers: Vec<String>,
    pub has_metadata: bool,
    pub file_count: usize,
    pub piece_count: u32,
    pub piece_size: u64,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
    pub busy: Option<String>,
    pub last_activity: i64,
    pub seeding_secs: u64,
    /// Per-torrent limits in KiB/s (0 = none).
    pub down_limit_kib: u32,
    pub up_limit_kib: u32,
    pub pending: bool,
    /// The engine could not load this torrent at startup (see `error`).
    pub restore_failed: bool,
}

impl TorrentView {
    pub fn magnet(&self) -> String {
        let mut s = format!("magnet:?xt=urn:btih:{}", self.hash.to_hex());
        s.push_str("&dn=");
        s.push_str(&url_encode(&self.name));
        for t in &self.trackers {
            s.push_str("&tr=");
            s.push_str(&url_encode(t));
        }
        s
    }
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct FileView {
    pub index: usize,
    pub components: Vec<String>,
    pub len: u64,
    pub done: u64,
    pub included: bool,
    pub priority: i8,
}

#[derive(Clone, Debug)]
pub struct PeerView {
    pub addr: String,
    pub client: Option<String>,
    pub kind: String,
    pub state: String,
    pub downloaded: u64,
    pub uploaded: u64,
    pub down_bps: u64,
    pub up_bps: u64,
}

#[derive(Clone, Debug, Default)]
pub struct TorrentDetail {
    pub hash: InfoHash,
    pub files: Vec<FileView>,
    pub peers: Vec<PeerView>,
    /// MSB-first bitfield of pieces we have, plus total piece count.
    pub pieces: Option<(Vec<u8>, u32)>,
    pub history: VecDeque<(u64, u64)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamState {
    /// Data is flowing to the player.
    #[default]
    Playing,
    /// The player waits for pieces that aren't downloaded yet.
    Buffering,
    /// The player stopped reading (paused, or its buffer is full).
    Paused,
    /// All connections closed (the player stopped or finished).
    Ended,
    /// Stopped from Corrent.
    Stopped,
}

impl StreamState {
    pub fn is_active(self) -> bool {
        matches!(self, StreamState::Playing | StreamState::Buffering | StreamState::Paused)
    }
}

/// A media player reading a file through the streaming server.
#[derive(Clone, Debug, Default)]
pub struct StreamView {
    pub id: u64,
    pub hash: InfoHash,
    pub torrent_name: String,
    pub file: usize,
    pub file_name: String,
    pub file_len: u64,
    /// Client address (loopback) and friendly player name from its User-Agent.
    pub client: String,
    pub player: String,
    pub user_agent: String,
    pub state: StreamState,
    pub connections: usize,
    pub requests: u32,
    /// Requests refused because every streaming slot was busy.
    pub refused: u32,
    pub elapsed_secs: u64,
    pub served: u64,
    pub rate_bps: u64,
    /// Byte offset being read in the file.
    pub position: u64,
    /// Downloaded bytes contiguous from `position` (None while metadata is unknown).
    pub buffered_ahead: Option<u64>,
    /// Availability of the file in equal buckets, 0 (missing) to 255 (downloaded).
    pub availability: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionView {
    pub down_bps: u64,
    pub up_bps: u64,
    pub dht_nodes: Option<usize>,
    pub listen_port: Option<u16>,
    pub peers_live: u32,
    pub alt_speed: bool,
    pub uptime_secs: u64,
    pub session_downloaded: u64,
    pub session_uploaded: u64,
    /// Local streaming server (port, URL token) for playing files while they download.
    pub stream: Option<(u16, String)>,
    /// Players reading from the streaming server, newest last.
    pub streams: Vec<StreamView>,
    /// Streaming bodies being sent and the maximum at once.
    pub stream_slots: (usize, usize),
    /// The slowest of the engine's last 60 ticks, in ms (budget: 20).
    pub tick_max_ms: u32,
    /// Address ranges loaded from the IP filter (blocked, allowed).
    pub blocked_ranges: usize,
    pub allowed_ranges: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum EngineState {
    #[default]
    Starting,
    Running,
    Restarting,
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub torrents: Vec<TorrentView>,
    pub detail: Option<TorrentDetail>,
    pub session: SessionView,
    pub engine_state: EngineState,
    /// Global (down, up) bytes/s samples, one per second, newest last.
    pub history: VecDeque<(u64, u64)>,
    pub generation: u64,
    pub categories: std::collections::BTreeMap<String, crate::store::CategoryInfo>,
    pub tags: Vec<String>,
    pub all_time_downloaded: u64,
    pub all_time_uploaded: u64,
    pub free_space: Option<u64>,
    pub warning: Option<String>,
    /// Recently removed torrents that can still be restored (hash, name).
    pub undoable: Vec<(InfoHash, String)>,
    /// Torrents in the middle of a move/recheck/re-add (briefly not in the list).
    pub busy_ops: usize,
    pub rss: Arc<crate::rss::RssView>,
    /// Recent (down, up) KiB/s per torrent, oldest first, for the Activity column.
    pub row_history: Arc<std::collections::HashMap<InfoHash, Vec<(u32, u32)>>>,
}

/// Where a torrent to be added comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum AddSource {
    File(PathBuf),
    Bytes { name: String, bytes: Vec<u8> },
    Magnet(String),
    Url(String),
}

impl AddSource {
    pub fn describe(&self) -> String {
        match self {
            AddSource::File(p) => {
                p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
            }
            AddSource::Bytes { name, .. } => name.clone(),
            AddSource::Magnet(m) => magnet_display_name(m).unwrap_or_else(|| shorten(m, 60)),
            // The query may carry a tracker passkey or an indexer API key.
            AddSource::Url(u) => shorten(&crate::util::redact_url(u), 60),
        }
    }

    /// Classify user input (CLI argument, pasted text, forwarded launch item).
    /// The info hash, when it can be known before adding (magnets with a v1 hash, .torrent
    /// bytes); URLs and files on disk are only known once fetched/read.
    pub fn info_hash(&self) -> Option<InfoHash> {
        match self {
            AddSource::Magnet(m) => librqbit_core::magnet::Magnet::parse(m).ok()?.as_id20().map(|h| InfoHash(h.0)),
            AddSource::Bytes { bytes, .. } => librqbit::torrent_from_bytes(bytes).ok().map(|t| InfoHash(t.info_hash.0)),
            _ => None,
        }
    }

    pub fn parse(input: &str) -> Option<AddSource> {
        let s = input.trim().trim_matches('"');
        if s.is_empty() {
            return None;
        }
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("magnet:") {
            return Some(AddSource::Magnet(s.to_string()));
        }
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Some(AddSource::Url(s.to_string()));
        }
        if (s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()))
            || (s.len() == 32 && s.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Some(AddSource::Magnet(format!("magnet:?xt=urn:btih:{s}")));
        }
        let path = PathBuf::from(s);
        if lower.ends_with(".torrent") || path.is_file() {
            return Some(AddSource::File(path));
        }
        None
    }
}

pub fn magnet_display_name(m: &str) -> Option<String> {
    let q = m.split_once('?')?.1;
    for kv in q.split('&') {
        if let Some(v) = kv.strip_prefix("dn=") {
            return Some(url_decode(v));
        }
    }
    None
}

pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_roundtrip() {
        let h = InfoHash([0xab; 20]);
        assert_eq!(InfoHash::from_hex(&h.to_hex()), Some(h));
    }

    #[test]
    fn parse_sources() {
        assert!(matches!(AddSource::parse("magnet:?xt=urn:btih:abc"), Some(AddSource::Magnet(_))));
        assert!(matches!(AddSource::parse("https://x/y.torrent"), Some(AddSource::Url(_))));
        assert!(matches!(AddSource::parse("0123456789abcdef0123456789abcdef01234567"), Some(AddSource::Magnet(_))));
        assert!(matches!(AddSource::parse("C:\\a b\\c.torrent"), Some(AddSource::File(_))));
        assert_eq!(AddSource::parse("   "), None);
    }

    #[test]
    fn magnet_name() {
        assert_eq!(magnet_display_name("magnet:?xt=urn:btih:x&dn=Hello%20World+2").as_deref(), Some("Hello World 2"));
        assert_eq!(url_decode("a%2"), "a%2");
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn info_hash_rejects_bad_hex() {
        assert!(InfoHash::from_hex("abc").is_none());
        assert!(InfoHash::from_hex(&"zz".repeat(20)).is_none());
        assert!(InfoHash::from_hex(&format!("  {}  ", "0a".repeat(20))).is_some(), "whitespace is trimmed");
    }

    #[test]
    fn add_source_hashes() {
        let h = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            AddSource::Magnet(format!("magnet:?xt=urn:btih:{h}&dn=x")).info_hash().map(|x| x.to_hex()).as_deref(),
            Some(h)
        );
        assert_eq!(AddSource::Url("http://x/y.torrent".into()).info_hash(), None);
    }

    #[test]
    fn info_hash_serde() {
        let h = InfoHash([7; 20]);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, format!("\"{}\"", "07".repeat(20)));
        assert_eq!(serde_json::from_str::<InfoHash>(&json).unwrap(), h);
        assert!(serde_json::from_str::<InfoHash>("\"nope\"").is_err());
    }

    #[test]
    fn url_coding_roundtrip() {
        for s in ["a b/c?d=e&f", "Ünïcødé 名前", "100%"] {
            assert_eq!(url_decode(&url_encode(s)), s);
        }
    }

    #[test]
    fn magnet_link_contains_name_and_trackers() {
        let t = TorrentView {
            hash: InfoHash([1; 20]),
            name: "My File".into(),
            trackers: vec!["udp://t.example:80".into()],
            ..Default::default()
        };
        let m = t.magnet();
        assert!(m.starts_with("magnet:?xt=urn:btih:0101"));
        assert!(m.contains("&dn=My%20File"));
        assert!(m.contains("&tr=udp%3A%2F%2Ft.example%3A80"));
    }

    #[test]
    fn parse_more_sources() {
        assert!(matches!(AddSource::parse("  \"magnet:?xt=urn:btih:x\"  "), Some(AddSource::Magnet(_))));
        assert!(matches!(AddSource::parse("HTTP://X/Y"), Some(AddSource::Url(_))));
        let b32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        assert!(matches!(AddSource::parse(b32), Some(AddSource::Magnet(m)) if m.ends_with(b32)));
        assert_eq!(AddSource::parse("hello world"), None);
    }

    #[test]
    fn describe_sources() {
        assert_eq!(AddSource::File("C:/x/y.torrent".into()).describe(), "y.torrent");
        assert_eq!(AddSource::Magnet("magnet:?xt=urn:btih:x&dn=Big%20Buck".into()).describe(), "Big Buck");
        assert!(AddSource::Url("https://example.com/".repeat(10)).describe().ends_with('…'));
    }

    #[test]
    fn state_flags() {
        assert!(TorrentState::Downloading.is_active());
        assert!(!TorrentState::Paused.is_active());
        assert!(TorrentState::Queued.is_paused_like());
        assert!(!TorrentState::Seeding.is_paused_like());
        assert_eq!(TorrentState::Metadata.label(), "Fetching metadata");
    }
}

/// Wire forms of the engine's views (`corrent-proto`), for the server and remote clients.
pub mod wire {
    use corrent_proto::torrent as p;

    use super::{EngineState, Snapshot, TorrentDetail, TorrentView};

    pub fn torrent(t: &TorrentView) -> p::Torrent {
        p::Torrent {
            hash: t.hash,
            name: t.name.clone(),
            state: t.state,
            error: t.error.clone(),
            size: t.size,
            done: t.done,
            progress: t.progress,
            uploaded: t.uploaded,
            downloaded: t.downloaded,
            down_bps: t.down_bps,
            up_bps: t.up_bps,
            eta_secs: t.eta.map(|d| d.as_secs()),
            ratio: t.ratio,
            peers_live: t.peers_live,
            peers_seen: t.peers_seen,
            peers_connecting: t.peers_connecting,
            save_path: t.save_path.display().to_string(),
            content_path: t.content_path.display().to_string(),
            category: t.category.clone(),
            tags: t.tags.clone(),
            added_by: t.added_by.clone(),
            added_at: t.added_at,
            completed_at: t.completed_at,
            queue_pos: t.queue_pos,
            trackers: t.trackers.clone(),
            has_metadata: t.has_metadata,
            file_count: t.file_count,
            piece_count: t.piece_count,
            piece_size: t.piece_size,
            comment: t.comment.clone(),
            created_by: t.created_by.clone(),
            creation_date: t.creation_date,
            busy: t.busy.clone(),
            last_activity: t.last_activity,
            seeding_secs: t.seeding_secs,
            down_limit_kib: t.down_limit_kib,
            up_limit_kib: t.up_limit_kib,
            pending: t.pending,
            restore_failed: t.restore_failed,
        }
    }

    pub fn torrents(s: &Snapshot) -> Vec<p::Torrent> {
        s.torrents.iter().map(torrent).collect()
    }

    pub fn labels(s: &Snapshot) -> p::Labels {
        p::Labels {
            categories: s
                .categories
                .iter()
                .map(|(k, c)| {
                    (
                        k.clone(),
                        p::Category {
                            save_path: c.save_path.as_ref().map(|p| p.display().to_string()),
                            down_limit_kib: c.down_limit_kib,
                            up_limit_kib: c.up_limit_kib,
                        },
                    )
                })
                .collect(),
            tags: s.tags.clone(),
        }
    }

    pub fn session(s: &Snapshot) -> p::SessionStats {
        let v = &s.session;
        p::SessionStats {
            engine: match &s.engine_state {
                EngineState::Starting => p::EngineStatus::Starting,
                EngineState::Running => p::EngineStatus::Running,
                EngineState::Restarting => p::EngineStatus::Restarting,
                EngineState::Failed(e) => p::EngineStatus::Failed(e.clone()),
            },
            down_bps: v.down_bps,
            up_bps: v.up_bps,
            dht_nodes: v.dht_nodes,
            listen_port: v.listen_port,
            peers_live: v.peers_live,
            alt_speed: v.alt_speed,
            uptime_secs: v.uptime_secs,
            session_downloaded: v.session_downloaded,
            session_uploaded: v.session_uploaded,
            all_time_downloaded: s.all_time_downloaded,
            all_time_uploaded: s.all_time_uploaded,
            free_space: s.free_space,
            warning: s.warning.clone(),
            busy_ops: s.busy_ops,
            torrent_count: s.torrents.len(),
        }
    }

    pub fn detail(d: &TorrentDetail) -> p::TorrentDetail {
        p::TorrentDetail {
            hash: d.hash,
            files: d
                .files
                .iter()
                .map(|f| p::FileEntry {
                    index: f.index,
                    path: f.components.clone(),
                    len: f.len,
                    done: f.done,
                    included: f.included,
                    priority: f.priority,
                })
                .collect(),
            peers: d
                .peers
                .iter()
                .map(|x| p::Peer {
                    addr: x.addr.clone(),
                    client: x.client.clone(),
                    kind: x.kind.clone(),
                    state: x.state.clone(),
                    downloaded: x.downloaded,
                    uploaded: x.uploaded,
                    down_bps: x.down_bps,
                    up_bps: x.up_bps,
                })
                .collect(),
            pieces: d.pieces.as_ref().map(|(b, _)| b.clone()),
            piece_count: d.pieces.as_ref().map(|(_, n)| *n).unwrap_or(0),
        }
    }

    /// The view of a torrent received from a server (`id` is the row's position there).
    pub fn torrent_view(t: &p::Torrent, id: usize) -> TorrentView {
        TorrentView {
            id,
            hash: t.hash,
            name: t.name.clone(),
            state: t.state,
            error: t.error.clone(),
            size: t.size,
            done: t.done,
            progress: t.progress,
            uploaded: t.uploaded,
            downloaded: t.downloaded,
            down_bps: t.down_bps,
            up_bps: t.up_bps,
            eta: t.eta_secs.map(std::time::Duration::from_secs),
            ratio: t.ratio,
            peers_live: t.peers_live,
            peers_seen: t.peers_seen,
            peers_connecting: t.peers_connecting,
            save_path: t.save_path.clone().into(),
            content_path: t.content_path.clone().into(),
            category: t.category.clone(),
            tags: t.tags.clone(),
            added_by: t.added_by.clone(),
            added_at: t.added_at,
            completed_at: t.completed_at,
            queue_pos: t.queue_pos,
            trackers: t.trackers.clone(),
            has_metadata: t.has_metadata,
            file_count: t.file_count,
            piece_count: t.piece_count,
            piece_size: t.piece_size,
            comment: t.comment.clone(),
            created_by: t.created_by.clone(),
            creation_date: t.creation_date,
            busy: t.busy.clone(),
            last_activity: t.last_activity,
            seeding_secs: t.seeding_secs,
            down_limit_kib: t.down_limit_kib,
            up_limit_kib: t.up_limit_kib,
            pending: t.pending,
            restore_failed: t.restore_failed,
        }
    }

    pub fn categories(l: &p::Labels) -> std::collections::BTreeMap<String, crate::store::CategoryInfo> {
        l.categories
            .iter()
            .map(|(k, c)| {
                (
                    k.clone(),
                    crate::store::CategoryInfo {
                        save_path: c.save_path.as_ref().map(Into::into),
                        down_limit_kib: c.down_limit_kib,
                        up_limit_kib: c.up_limit_kib,
                    },
                )
            })
            .collect()
    }

    /// Fills the session part of a snapshot from a server's statistics.
    pub fn apply_session(s: &mut Snapshot, v: &p::SessionStats) {
        s.engine_state = match &v.engine {
            p::EngineStatus::Starting => EngineState::Starting,
            p::EngineStatus::Running => EngineState::Running,
            p::EngineStatus::Restarting => EngineState::Restarting,
            p::EngineStatus::Failed(e) => EngineState::Failed(e.clone()),
        };
        let x = &mut s.session;
        x.down_bps = v.down_bps;
        x.up_bps = v.up_bps;
        x.dht_nodes = v.dht_nodes;
        x.listen_port = v.listen_port;
        x.peers_live = v.peers_live;
        x.alt_speed = v.alt_speed;
        x.uptime_secs = v.uptime_secs;
        x.session_downloaded = v.session_downloaded;
        x.session_uploaded = v.session_uploaded;
        s.all_time_downloaded = v.all_time_downloaded;
        s.all_time_uploaded = v.all_time_uploaded;
        s.free_space = v.free_space;
        s.warning = v.warning.clone();
        s.busy_ops = v.busy_ops;
    }

    pub fn detail_view(d: &p::TorrentDetail) -> TorrentDetail {
        TorrentDetail {
            hash: d.hash,
            files: d
                .files
                .iter()
                .map(|f| super::FileView {
                    index: f.index,
                    components: f.path.clone(),
                    len: f.len,
                    done: f.done,
                    included: f.included,
                    priority: f.priority,
                })
                .collect(),
            peers: d
                .peers
                .iter()
                .map(|x| super::PeerView {
                    addr: x.addr.clone(),
                    client: x.client.clone(),
                    kind: x.kind.clone(),
                    state: x.state.clone(),
                    downloaded: x.downloaded,
                    uploaded: x.uploaded,
                    down_bps: x.down_bps,
                    up_bps: x.up_bps,
                })
                .collect(),
            pieces: d.pieces.as_ref().map(|b| (b.clone(), d.piece_count)),
            history: Default::default(),
        }
    }
}
