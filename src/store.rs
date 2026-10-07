//! App-side per-torrent metadata that librqbit does not persist: categories, tags, dates,
//! queue positions, pause reasons and cumulative transfer counters.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::types::InfoHash;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PauseReason {
    User,
    Queue,
    SeedLimit,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TorrentMeta {
    pub name: String,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub display_name: Option<String>,
    pub queue_pos: u32,
    pub pause_reason: Option<PauseReason>,
    pub force_started: bool,
    /// Cumulative bytes over the torrent's whole life (across restarts and re-adds).
    pub uploaded_total: u64,
    pub downloaded_total: u64,
    /// Last raw engine counters seen, used to compute deltas.
    pub last_raw_uploaded: u64,
    pub last_raw_downloaded: u64,
    pub last_activity: i64,
    pub last_upload_at: i64,
    pub seeding_seconds: u64,
    /// Seconds spent seeding without uploading (reset by any upload).
    pub inactive_seeding_secs: u64,
    /// User resumed a torrent that hit its seeding limit: don't stop it again.
    pub seed_limit_exempt: bool,
    pub seed_ratio_override: Option<f32>,
    pub seed_time_override_min: Option<u32>,
    pub source_magnet: Option<String>,
}

impl TorrentMeta {
    /// Fold the engine's (possibly reset) session counters into the cumulative totals.
    pub fn absorb_counters(&mut self, raw_up: u64, raw_down: u64) -> (u64, u64) {
        let du = if raw_up >= self.last_raw_uploaded { raw_up - self.last_raw_uploaded } else { raw_up };
        let dd = if raw_down >= self.last_raw_downloaded { raw_down - self.last_raw_downloaded } else { raw_down };
        self.uploaded_total += du;
        self.downloaded_total += dd;
        self.last_raw_uploaded = raw_up;
        self.last_raw_downloaded = raw_down;
        (du, dd)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct CategoryInfo {
    pub save_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GlobalStats {
    pub all_time_uploaded: u64,
    pub all_time_downloaded: u64,
    pub total_uptime_secs: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StoreData {
    pub torrents: HashMap<InfoHash, TorrentMeta>,
    pub categories: BTreeMap<String, CategoryInfo>,
    pub tags: Vec<String>,
    pub stats: GlobalStats,
}

pub struct Store {
    pub data: StoreData,
    path: PathBuf,
    /// Structural change (category, tags, queue, …): saved within a couple of seconds.
    dirty: bool,
    /// Only transfer counters changed: saved at a relaxed pace (and always on shutdown).
    soft_dirty: bool,
    last_save: std::time::Instant,
}

impl Store {
    pub fn load(path: PathBuf) -> Store {
        // A transient read failure (e.g. a lock) must not be mistaken for "no data".
        for _ in 0..5 {
            match std::fs::read(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => std::thread::sleep(std::time::Duration::from_millis(200)),
                _ => break,
            }
        }
        let data = std::fs::read(&path)
            .ok()
            .and_then(|b| match serde_json::from_slice::<StoreData>(&b) {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::error!("store.json is corrupt ({e}); trying backup");
                    None
                }
            })
            .or_else(|| {
                let bak = path.with_extension("json.bak");
                std::fs::read(bak).ok().and_then(|b| serde_json::from_slice(&b).ok())
            })
            .unwrap_or_default();
        Store { data, path, dirty: false, soft_dirty: false, last_save: std::time::Instant::now() }
    }

    /// Mutable access that marks the store for a prompt save.
    pub fn meta_mut(&mut self, hash: InfoHash) -> &mut TorrentMeta {
        self.dirty = true;
        self.meta_quiet(hash)
    }

    /// Mutable access without marking anything dirty (the caller marks what changed).
    pub fn meta_quiet(&mut self, hash: InfoHash) -> &mut TorrentMeta {
        let next_pos = self.data.torrents.len() as u32;
        if !self.data.torrents.contains_key(&hash) {
            self.dirty = true;
        }
        self.data.torrents.entry(hash).or_insert_with(|| TorrentMeta {
            added_at: crate::util::now_unix(),
            queue_pos: next_pos,
            ..Default::default()
        })
    }

    pub fn mark_soft_dirty(&mut self) {
        self.soft_dirty = true;
    }

    pub fn get(&self, hash: &InfoHash) -> Option<&TorrentMeta> {
        self.data.torrents.get(hash)
    }

    pub fn remove(&mut self, hash: &InfoHash) {
        if self.data.torrents.remove(hash).is_some() {
            self.dirty = true;
            self.normalize_queue();
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Re-number queue positions 0..n keeping relative order.
    pub fn normalize_queue(&mut self) {
        let mut v: Vec<(InfoHash, u32)> = self.data.torrents.iter().map(|(h, m)| (*h, m.queue_pos)).collect();
        v.sort_by_key(|(h, p)| (*p, *h));
        for (i, (h, _)) in v.into_iter().enumerate() {
            if let Some(m) = self.data.torrents.get_mut(&h) {
                m.queue_pos = i as u32;
            }
        }
        self.dirty = true;
    }

    pub fn queue_move(&mut self, hash: InfoHash, mv: QueueMove) {
        self.normalize_queue();
        let n = self.data.torrents.len() as i64;
        let Some(cur) = self.data.torrents.get(&hash).map(|m| m.queue_pos as i64) else { return };
        let target = match mv {
            QueueMove::Top => 0,
            QueueMove::Bottom => n - 1,
            QueueMove::Up => (cur - 1).max(0),
            QueueMove::Down => (cur + 1).min(n - 1),
        };
        if target == cur {
            return;
        }
        for m in self.data.torrents.values_mut() {
            let p = m.queue_pos as i64;
            if target < cur && p >= target && p < cur {
                m.queue_pos += 1;
            } else if target > cur && p <= target && p > cur {
                m.queue_pos -= 1;
            }
        }
        if let Some(m) = self.data.torrents.get_mut(&hash) {
            m.queue_pos = target as u32;
        }
        self.dirty = true;
    }

    /// Debounced save; `force` writes immediately (shutdown).
    pub fn maybe_save(&mut self, force: bool) {
        if !self.dirty && !self.soft_dirty {
            return;
        }
        let wait = if self.dirty { std::time::Duration::from_secs(2) } else { std::time::Duration::from_secs(30) };
        if !force && self.last_save.elapsed() < wait {
            return;
        }
        match serde_json::to_vec_pretty(&self.data) {
            Ok(bytes) => match crate::util::write_atomic(&self.path, &bytes) {
                Ok(()) => {
                    self.dirty = false;
                    self.soft_dirty = false;
                    self.last_save = std::time::Instant::now();
                }
                Err(e) => tracing::error!("failed to save store: {e:#}"),
            },
            Err(e) => tracing::error!("failed to serialize store: {e:#}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueMove {
    Top,
    Up,
    Down,
    Bottom,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_survive_resets() {
        let mut m = TorrentMeta::default();
        m.absorb_counters(100, 1000);
        m.absorb_counters(150, 1500);
        // engine restarted: counters dropped to small values
        m.absorb_counters(10, 20);
        assert_eq!(m.uploaded_total, 160);
        assert_eq!(m.downloaded_total, 1520);
    }

    #[test]
    fn queue_moves() {
        let mut s = Store::load(std::env::temp_dir().join("corrent-nonexistent-store.json"));
        let hs: Vec<InfoHash> = (0..4u8).map(|i| InfoHash([i; 20])).collect();
        for h in &hs {
            s.meta_mut(*h);
        }
        s.queue_move(hs[3], QueueMove::Top);
        assert_eq!(s.get(&hs[3]).unwrap().queue_pos, 0);
        assert_eq!(s.get(&hs[0]).unwrap().queue_pos, 1);
        s.queue_move(hs[0], QueueMove::Bottom);
        assert_eq!(s.get(&hs[0]).unwrap().queue_pos, 3);
        let mut ps: Vec<u32> = s.data.torrents.values().map(|m| m.queue_pos).collect();
        ps.sort();
        assert_eq!(ps, vec![0, 1, 2, 3]);
    }
}
