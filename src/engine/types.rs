use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 20-byte BitTorrent v1 info hash, the stable identity of a torrent across restarts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct InfoHash(pub [u8; 20]);

impl InfoHash {
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(40);
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 40 {
            return None;
        }
        let mut out = [0u8; 20];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hex = std::str::from_utf8(chunk).ok()?;
            out[i] = u8::from_str_radix(hex, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl serde::Serialize for InfoHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for InfoHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        InfoHash::from_hex(&s).ok_or_else(|| serde::de::Error::custom("invalid info hash"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum TorrentState {
    /// Waiting for metadata from peers (magnet links).
    Metadata,
    Checking,
    Downloading,
    Stalled,
    Seeding,
    Completed,
    #[default]
    Paused,
    Queued,
    Error,
}

impl TorrentState {
    pub fn label(self) -> &'static str {
        match self {
            TorrentState::Metadata => t!("Fetching metadata"),
            TorrentState::Checking => t!("Checking"),
            TorrentState::Downloading => t!("Downloading"),
            TorrentState::Stalled => t!("Stalled"),
            TorrentState::Seeding => t!("Seeding"),
            TorrentState::Completed => t!("Completed"),
            TorrentState::Paused => t!("Paused"),
            TorrentState::Queued => t!("Queued"),
            TorrentState::Error => t!("Error"),
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            TorrentState::Downloading | TorrentState::Seeding | TorrentState::Metadata | TorrentState::Checking
        )
    }

    pub fn is_paused_like(self) -> bool {
        matches!(self, TorrentState::Paused | TorrentState::Queued | TorrentState::Completed)
    }
}

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
