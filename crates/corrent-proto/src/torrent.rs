//! Torrent wire types: identity, state, list rows, per-torrent detail, and the delta format
//! that keeps a client's torrent list in sync with the server.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// One row of the torrent list as sent to clients (`TorrentView` without engine internals;
/// paths are strings so every client platform can show them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Torrent {
    pub hash: InfoHash,
    pub name: String,
    pub state: TorrentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub size: u64,
    pub done: u64,
    pub progress: f32,
    pub uploaded: u64,
    pub downloaded: u64,
    pub down_bps: u64,
    pub up_bps: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    pub ratio: f32,
    pub peers_live: u32,
    pub peers_seen: u32,
    pub peers_connecting: u32,
    pub save_path: String,
    pub content_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
    pub added_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    pub queue_pos: u32,
    pub trackers: Vec<String>,
    pub has_metadata: bool,
    pub file_count: usize,
    pub piece_count: u32,
    pub piece_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_date: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<String>,
    pub last_activity: i64,
    pub seeding_secs: u64,
    pub down_limit_kib: u32,
    pub up_limit_kib: u32,
    pub pending: bool,
    pub restore_failed: bool,
}

/// Changed fields of one torrent. Fast-changing fields only; anything else changing sends the
/// whole row again (in `TorrentsDelta::upserted`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TorrentPatch {
    pub hash: InfoHash,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<TorrentState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up_bps: Option<u64>,
    /// `Some(None)` clears the ETA.
    #[serde(skip_serializing_if = "Option::is_none", with = "double_option")]
    pub eta_secs: Option<Option<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ratio: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers_live: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers_seen: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers_connecting: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeding_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_pos: Option<u32>,
}

mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Option<u64>>, s: S) -> Result<S::Ok, S::Error> {
        v.as_ref().and_then(|x| *x).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<u64>>, D::Error> {
        Ok(Some(Option::<u64>::deserialize(d)?))
    }
}

/// Changes since the last list a client received.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TorrentsDelta {
    /// New torrents, or rows whose slow-changing fields changed (sent whole).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub upserted: Vec<Torrent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub patched: Vec<TorrentPatch>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<InfoHash>,
}

impl TorrentsDelta {
    pub fn is_empty(&self) -> bool {
        self.upserted.is_empty() && self.patched.is_empty() && self.removed.is_empty()
    }
}

/// The patch turning `old` into `new`, or `None` when a slow field changed (send the row).
fn patch(old: &Torrent, new: &Torrent) -> Option<TorrentPatch> {
    let mut a = old.clone();
    let mut p = TorrentPatch { hash: new.hash, ..Default::default() };
    macro_rules! fast {
        ($($f:ident),*) => {$(
            if old.$f != new.$f {
                p.$f = Some(new.$f.clone());
            }
            a.$f = new.$f.clone();
        )*};
    }
    fast!(
        state,
        done,
        progress,
        uploaded,
        downloaded,
        down_bps,
        up_bps,
        ratio,
        peers_live,
        peers_seen,
        peers_connecting,
        last_activity,
        seeding_secs,
        queue_pos
    );
    if old.eta_secs != new.eta_secs {
        p.eta_secs = Some(new.eta_secs);
    }
    a.eta_secs = new.eta_secs;
    // Everything else must be equal for a patch to describe the change.
    (a == *new).then_some(p)
}

/// The delta from `old` to `new` (both in list order; order is by `queue_pos`/client sort).
pub fn diff(old: &[Torrent], new: &[Torrent]) -> TorrentsDelta {
    let before: HashMap<InfoHash, &Torrent> = old.iter().map(|t| (t.hash, t)).collect();
    let mut d = TorrentsDelta::default();
    for t in new {
        match before.get(&t.hash) {
            None => d.upserted.push(t.clone()),
            Some(o) if *o == t => {}
            Some(o) => match patch(o, t) {
                Some(p) => d.patched.push(p),
                None => d.upserted.push(t.clone()),
            },
        }
    }
    let now: std::collections::HashSet<InfoHash> = new.iter().map(|t| t.hash).collect();
    d.removed = old.iter().filter(|t| !now.contains(&t.hash)).map(|t| t.hash).collect();
    d
}

/// Applies a delta to a client's list (new rows are appended; clients sort for display).
pub fn apply(list: &mut Vec<Torrent>, d: &TorrentsDelta) {
    if !d.removed.is_empty() {
        let gone: std::collections::HashSet<&InfoHash> = d.removed.iter().collect();
        list.retain(|t| !gone.contains(&t.hash));
    }
    let mut index: HashMap<InfoHash, usize> = list.iter().enumerate().map(|(i, t)| (t.hash, i)).collect();
    for t in &d.upserted {
        match index.get(&t.hash) {
            Some(&i) => list[i] = t.clone(),
            None => {
                index.insert(t.hash, list.len());
                list.push(t.clone());
            }
        }
    }
    for p in &d.patched {
        let Some(&i) = index.get(&p.hash) else { continue };
        let t = &mut list[i];
        macro_rules! set {
            ($($f:ident),*) => {$(
                if let Some(v) = &p.$f {
                    t.$f = v.clone();
                }
            )*};
        }
        set!(
            state,
            done,
            progress,
            uploaded,
            downloaded,
            down_bps,
            up_bps,
            ratio,
            peers_live,
            peers_seen,
            peers_connecting,
            last_activity,
            seeding_secs,
            queue_pos
        );
        if let Some(e) = p.eta_secs {
            t.eta_secs = e;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub index: usize,
    /// Path components inside the torrent.
    pub path: Vec<String>,
    pub len: u64,
    pub done: u64,
    pub included: bool,
    pub priority: i8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    pub addr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub kind: String,
    pub state: String,
    pub downloaded: u64,
    pub uploaded: u64,
    pub down_bps: u64,
    pub up_bps: u64,
}

/// Files, peers and pieces of one torrent (fetched on demand).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TorrentDetail {
    pub hash: InfoHash,
    pub files: Vec<FileEntry>,
    pub peers: Vec<Peer>,
    /// MSB-first bitfield of pieces we have (base64 on the wire is not needed: JSON array).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pieces: Option<Vec<u8>>,
    pub piece_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Category {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    pub down_limit_kib: u32,
    pub up_limit_kib: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "error")]
pub enum EngineStatus {
    #[default]
    Starting,
    Running,
    Restarting,
    Failed(String),
}

/// Session-wide numbers, sent on every engine tick.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionStats {
    pub engine: EngineStatus,
    pub down_bps: u64,
    pub up_bps: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dht_nodes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    pub peers_live: u32,
    pub alt_speed: bool,
    pub uptime_secs: u64,
    pub session_downloaded: u64,
    pub session_uploaded: u64,
    pub all_time_downloaded: u64,
    pub all_time_uploaded: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_space: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub busy_ops: usize,
    pub torrent_count: usize,
}

/// What a magnet link or .torrent URL holds, before adding it (`POST /torrents/preview`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TorrentPreview {
    pub info_hash: InfoHash,
    pub name: String,
    pub files: Vec<PreviewFile>,
    pub total_size: u64,
    pub piece_count: u32,
    pub piece_length: u32,
    pub already_added: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The .torrent file, base64 (upload it when adding so the magnet isn't resolved again).
    pub torrent: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreviewFile {
    pub path: Vec<String>,
    pub len: u64,
}

/// Categories and tags (sent on change).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Labels {
    pub categories: BTreeMap<String, Category>,
    pub tags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: u8) -> Torrent {
        Torrent { hash: InfoHash([i; 20]), name: format!("t{i}"), size: 100, ..Default::default() }
    }

    #[test]
    fn info_hash_hex_round_trip() {
        let h = InfoHash([0xab; 20]);
        assert_eq!(InfoHash::from_hex(&h.to_hex()), Some(h));
        assert_eq!(serde_json::to_string(&h).unwrap(), format!("\"{}\"", h.to_hex()));
        assert!(InfoHash::from_hex("xyz").is_none());
    }

    #[test]
    fn state_uses_snake_case() {
        assert_eq!(serde_json::to_string(&TorrentState::Downloading).unwrap(), "\"downloading\"");
    }

    #[test]
    fn diff_and_apply_reproduce_the_new_list() {
        let old: Vec<Torrent> = (0..5).map(row).collect();
        let mut new = old.clone();
        new[0].down_bps = 1234;
        new[0].eta_secs = Some(60);
        new[1].name = "renamed".into(); // slow field -> whole row
        new.remove(2);
        new.push(row(9));
        let d = diff(&old, &new);
        assert_eq!(d.patched.len(), 1);
        assert_eq!(d.upserted.len(), 2);
        assert_eq!(d.removed, vec![InfoHash([2; 20])]);
        let mut client = old.clone();
        apply(&mut client, &d);
        let sort = |v: &mut Vec<Torrent>| v.sort_by_key(|t| t.hash);
        let (mut a, mut b) = (client, new);
        sort(&mut a);
        sort(&mut b);
        assert_eq!(a, b);
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn eta_can_be_cleared_by_a_patch() {
        let mut a = row(1);
        a.eta_secs = Some(5);
        let mut b = a.clone();
        b.eta_secs = None;
        let d = diff(std::slice::from_ref(&a), std::slice::from_ref(&b));
        let json = serde_json::to_string(&d).unwrap();
        let back: TorrentsDelta = serde_json::from_str(&json).unwrap();
        let mut l = vec![a];
        apply(&mut l, &back);
        assert_eq!(l[0].eta_secs, None, "{json}");
    }

    #[test]
    fn five_thousand_torrents_delta_is_small() {
        let old: Vec<Torrent> = (0..5000u32)
            .map(|i| {
                let mut h = [0u8; 20];
                h[..4].copy_from_slice(&i.to_be_bytes());
                Torrent {
                    hash: InfoHash(h),
                    name: format!("torrent number {i}"),
                    trackers: vec!["udp://tracker.example:1337".into()],
                    ..Default::default()
                }
            })
            .collect();
        let mut new = old.clone();
        for t in new.iter_mut().step_by(25) {
            t.down_bps += 50_000;
            t.done += 16_384;
            t.progress += 0.001;
        }
        let start = std::time::Instant::now();
        let d = diff(&old, &new);
        let took = start.elapsed();
        let bytes = serde_json::to_vec(&d).unwrap().len();
        let full = serde_json::to_vec(&new).unwrap().len();
        assert_eq!(d.patched.len(), 200);
        assert!(bytes * 20 < full, "delta {bytes} B vs full {full} B");
        assert!(took < std::time::Duration::from_millis(200), "diff took {took:?}");
    }
}
