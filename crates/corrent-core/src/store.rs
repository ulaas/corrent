//! App-side per-torrent metadata that librqbit does not persist: categories, tags, dates,
//! queue positions, pause reasons and cumulative transfer counters, plus a copy of where the engine
//! keeps each torrent's data (to rebuild its list if that is lost).

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
    /// The server user who added the torrent (see `AddRequest::added_by`).
    pub added_by: Option<String>,
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
    /// Per-torrent speed limits in KiB/s (0 = no limit).
    pub down_limit_kib: u32,
    pub up_limit_kib: u32,
    /// Per-file download priority (1 high, -1 low); files not listed are normal.
    pub file_priority: BTreeMap<usize, i8>,
    /// Copies of what the engine persists in `session.json` (schema v2), refreshed by the actor:
    /// its output folder (the content folder of a multi-file torrent, the file's folder for one
    /// file; None until first seen), its file selection (None = every file) and its paused flag
    /// (librqbit's `is_paused`, the intent, not the live state). Only used to rebuild a lost list.
    pub output_folder: Option<PathBuf>,
    pub only_files: Option<Vec<usize>>,
    pub paused: bool,
    /// A rebuild of the engine's list found no metainfo for it: shown as a failed load with
    /// Retry (on every start) until it is found or removed.
    pub metainfo_missing: bool,
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
    /// Shared by all torrents in the category, in KiB/s (0 = no limit).
    pub down_limit_kib: u32,
    pub up_limit_kib: u32,
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
    /// File format version (see `migrate.rs`); 0 = written before versioning.
    pub schema_version: u32,
    pub torrents: HashMap<InfoHash, TorrentMeta>,
    pub categories: BTreeMap<String, CategoryInfo>,
    pub tags: Vec<String>,
    pub stats: GlobalStats,
    /// Torrents to drop from the engine's database the next time it is stopped/started
    /// (used for torrents the engine failed to restore and can't delete itself).
    pub purge: Vec<InfoHash>,
}

pub struct Store {
    pub data: StoreData,
    path: PathBuf,
    /// Structural change (category, tags, queue, …): saved within a couple of seconds.
    dirty: bool,
    /// Only transfer counters changed: saved at a relaxed pace (and always on shutdown).
    soft_dirty: bool,
    last_save: std::time::Instant,
    /// Set when loading had to recover from a damaged file.
    pub warning: Option<String>,
    /// The file belongs to a newer Corrent: never overwrite it.
    read_only: bool,
    /// The file was damaged beyond its backups: the records start over.
    reset: bool,
    last_error: Option<String>,
    pending_error: Option<String>,
    /// UTC day of the newest daily generation (`store.json.daily-<ts>`), read on the first save.
    daily_day: Option<i64>,
    /// After a failed save: how long to wait before the next try (doubles up to 5 minutes).
    retry_after: std::time::Duration,
}

const FIRST_RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// Daily generations of `store.json` kept besides `.bak`.
const DAILY_GENERATIONS: usize = 3;

impl Drop for Store {
    /// Flush pending changes, also when the engine task unwinds after a panic.
    fn drop(&mut self) {
        self.flush();
    }
}

/// How long a flush waits for the writer thread (a hung disk must not hang shutdown forever).
const FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Store {
    /// Tests: drop without flushing to disk.
    #[cfg(test)]
    pub(crate) fn discard(mut self) {
        self.dirty = false;
        self.soft_dirty = false;
        crate::writer::global().cancel(&self.path);
    }

    pub fn load(path: PathBuf) -> Store {
        // A transient read failure (e.g. a lock) must not be mistaken for "no data".
        for _ in 0..5 {
            match std::fs::read(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    std::thread::sleep(std::time::Duration::from_millis(200))
                }
                _ => break,
            }
        }
        let mut warning = None;
        let mut newer_unreadable = false;
        let mut reset = false;
        // Exists but can't be read (lock, permissions): run read-only so it is never overwritten.
        let mut unreadable = false;
        let data = match std::fs::read(&path) {
            // A file in place of a parent folder (ENOTDIR on Unix) also means "no store yet".
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => None,
            Err(e) => {
                tracing::error!("store.json can't be read ({e}); running without saving it");
                unreadable = true;
                warning = Some(t!(
                    "Corrent couldn't read its data file ({e}). Categories, tags and totals are hidden and nothing is \
                     saved this session, so the file stays intact. Your torrents are unaffected.",
                    e = e
                ));
                None
            }
            Ok(b) => match serde_json::from_slice::<StoreData>(&b) {
                Ok(d) => Some(d),
                Err(e) if crate::migrate::json_version(&b) > crate::migrate::STORE_VERSION => {
                    // Newer format, not damage: start empty in memory and never overwrite the file.
                    tracing::warn!("store.json from a newer version can't be read ({e})");
                    newer_unreadable = true;
                    None
                }
                Err(e) => {
                    tracing::error!("store.json is corrupt ({e}); trying backup");
                    let bad = path.with_extension(format!("json.bad-{}", crate::util::now_unix()));
                    let kept = std::fs::copy(&path, &bad).is_ok();
                    // The hourly `.bak` first, then the daily generations, newest first.
                    let candidates = std::iter::once(path.with_extension("json.bak"))
                        .chain(crate::util::generations(&path, "daily").into_iter().map(|g| g.1));
                    let good = candidates.into_iter().find_map(|p| {
                        let b = std::fs::read(&p).ok()?;
                        serde_json::from_slice::<StoreData>(&b).ok().map(|d| (p, d))
                    });
                    match good {
                        Some((bak, d)) if std::fs::copy(&bak, &path).is_err() => {
                            // The good copy couldn't be put back (the file is locked): show it,
                            // but never save, or the damaged file would be rotated over `.bak`.
                            tracing::error!("could not put {} back in place", bak.display());
                            unreadable = true;
                            warning = Some(t!(
                                "Corrent's data file was damaged. Its backup is shown, but it couldn't be put in place, \
                                 so nothing is saved this session; restart Corrent to try again."
                            ).into());
                            Some(d)
                        }
                        Some((_, d)) => {
                            // The good copy is back in place (above), so the next save never
                            // rotates the corrupt file over the only good backup.
                            warning = Some(t!(
                                "Corrent's data file was damaged and was restored from its backup; the most recent changes \
                                 (categories, tags, totals) may be missing. The damaged file was kept as {path}.",
                                path = bad.display()
                            ));
                            Some(d)
                        }
                        None if !kept => {
                            // Couldn't keep a copy of the damaged file: leave it alone, save nothing.
                            unreadable = true;
                            warning = Some(t!(
                                "Corrent's data file is damaged and couldn't be backed up. Categories, tags and totals \
                                 are hidden and nothing is saved this session; the file is left as it is."
                            ).into());
                            None
                        }
                        None => {
                            let _ = std::fs::remove_file(&path);
                            reset = true;
                            warning = Some(t!(
                                "Corrent's data file was damaged and no usable backup exists: categories, tags and totals were \
                                 reset. Your torrents are unaffected. The damaged file was kept as {path}.",
                                path = bad.display()
                            ));
                            None
                        }
                    }
                }
            },
        }
        .unwrap_or_default();
        let mut data = data;
        let mut read_only = newer_unreadable || unreadable;
        if newer_unreadable {
            warning = Some(
                t!("Corrent's data was saved by a newer version and can't be read by this one. Categories and tags are hidden and nothing is saved until you update Corrent; your torrents are unaffected.")
                    .into(),
            );
        }
        let mut dirty = warning.is_some();
        use crate::migrate::{STORE_VERSION, migrate_store};
        let from = data.schema_version;
        if from > STORE_VERSION {
            read_only = true;
            warning.get_or_insert(t!(
                "Corrent's data was saved by a newer version (format {from}). Changes to categories, tags and \
                 totals won't be saved until you update Corrent.",
                from = from
            ));
        } else if from < STORE_VERSION {
            if path.exists() && !newer_unreadable {
                let _ = crate::migrate::backup_before_migration(&path, from);
            }
            migrate_store(&mut data, from);
            dirty = true;
        }
        Store {
            data,
            path,
            dirty,
            soft_dirty: false,
            last_save: std::time::Instant::now(),
            warning,
            read_only,
            reset,
            last_error: None,
            pending_error: None,
            daily_day: None,
            retry_after: FIRST_RETRY,
        }
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

    /// A new save failure to report (each distinct failure is reported once).
    pub fn take_save_error(&mut self) -> Option<String> {
        self.pending_error.take()
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

    /// Move `hashes` (keeping their relative order) right before `before`, or to the end.
    pub fn queue_place(&mut self, hashes: &[InfoHash], before: Option<InfoHash>) {
        self.normalize_queue();
        let mut order: Vec<(InfoHash, u32)> = self.data.torrents.iter().map(|(h, m)| (*h, m.queue_pos)).collect();
        order.sort_by_key(|(_, p)| *p);
        let mut order: Vec<InfoHash> = order.into_iter().map(|(h, _)| h).collect();
        let moving: Vec<InfoHash> = order.iter().filter(|h| hashes.contains(h)).copied().collect();
        if moving.is_empty() || before.is_some_and(|b| moving.contains(&b)) {
            return;
        }
        order.retain(|h| !moving.contains(h));
        let at = before.and_then(|b| order.iter().position(|h| *h == b)).unwrap_or(order.len());
        order.splice(at..at, moving);
        for (i, h) in order.iter().enumerate() {
            if let Some(m) = self.data.torrents.get_mut(h) {
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

    /// Debounced save on the writer thread: structural changes within ~2 s, counters within 30 s.
    /// A batch of changes (500 adds) costs a write or two.
    pub fn maybe_save(&mut self) {
        self.collect_result();
        if !self.dirty && !self.soft_dirty || self.read_only {
            return;
        }
        let mut wait = if self.dirty { std::time::Duration::from_secs(2) } else { std::time::Duration::from_secs(30) };
        if self.last_error.is_some() {
            // A disk that keeps refusing isn't hammered every 2 s.
            wait = wait.max(self.retry_after);
        }
        if self.last_save.elapsed() >= wait {
            self.submit();
        }
    }

    /// Save now and wait until it is on disk (shutdown; a store dropped while its task unwinds).
    /// True when everything is on disk.
    pub fn flush(&mut self) -> bool {
        if (self.dirty || self.soft_dirty) && !self.read_only {
            self.submit();
        }
        let done = crate::writer::global().flush(&self.path, FLUSH_TIMEOUT);
        if !done {
            tracing::warn!("store.json is still being saved after {FLUSH_TIMEOUT:?}");
        }
        self.collect_result();
        done && !self.read_only && self.last_error.is_none()
    }

    /// The records are what was saved: not hidden (an unreadable or newer file, saved by nobody
    /// this session) and not started over after damage. A rebuild of the engine's list relies
    /// on them (D6b).
    pub fn records_trusted(&self) -> bool {
        !self.read_only && !self.reset
    }

    fn submit(&mut self) {
        match serde_json::to_vec_pretty(&self.data) {
            Ok(bytes) => {
                let generation =
                    self.daily_due().then_some(crate::writer::Generation { tag: "daily", keep: DAILY_GENERATIONS });
                crate::writer::global().submit(&self.path, bytes, generation);
                self.dirty = false;
                self.soft_dirty = false;
                self.last_save = std::time::Instant::now();
            }
            Err(e) => tracing::error!("failed to serialize store: {e:#}"),
        }
    }

    /// Pick up the outcome of the last write: a failure is reported once and retried later.
    fn collect_result(&mut self) {
        match crate::writer::global().take_result(&self.path) {
            Some(Ok(())) => {
                self.last_error = None;
                self.retry_after = FIRST_RETRY;
            }
            Some(Err(msg)) => {
                if self.last_error.is_some() {
                    self.retry_after = (self.retry_after * 2).min(std::time::Duration::from_secs(300));
                }
                // The day's dated copy may not have been taken: ask again with the next save.
                self.daily_day = None;
                if self.last_error.as_deref() != Some(msg.as_str()) {
                    self.pending_error = Some(msg.clone());
                }
                self.last_error = Some(msg);
                self.dirty = true;
                self.last_save = std::time::Instant::now();
            }
            None => {}
        }
    }

    /// True on the first save of a UTC day (then the save also keeps a dated generation).
    fn daily_due(&mut self) -> bool {
        let today = crate::util::now_unix().div_euclid(86400);
        let path = &self.path;
        let newest = *self.daily_day.get_or_insert_with(|| {
            crate::util::generations(path, "daily").first().map_or(i64::MIN, |g| g.0.div_euclid(86400))
        });
        self.daily_day = Some(today);
        newest != today
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
        let mut s = Store::load(std::env::temp_dir().join(format!("corrent-store-moves-{}.json", std::process::id())));
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
        // Drag & drop: [3,1,2,0] → move {1,0} before 3 → [1,0,3,2]
        s.queue_place(&[hs[0], hs[1]], Some(hs[3]));
        let pos = |s: &Store, i: usize| s.get(&hs[i]).unwrap().queue_pos;
        assert_eq!((pos(&s, 1), pos(&s, 0), pos(&s, 3), pos(&s, 2)), (0, 1, 2, 3));
        s.queue_place(&[hs[1]], None);
        assert_eq!(pos(&s, 1), 3, "to the end");
        s.queue_place(&[hs[0]], Some(hs[0]));
        assert_eq!(pos(&s, 0), 0, "dropping onto itself is a no-op");
        s.discard();
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    /// `store.json` in a fresh folder of its own (saves also write `.bak` and daily generations).
    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("store.json")
    }

    #[test]
    fn quiet_access_does_not_dirty_existing() {
        let mut s = Store::load(tmp("q"));
        s.meta_mut(InfoHash([1; 20]));
        s.dirty = false;
        s.meta_quiet(InfoHash([1; 20])).last_activity = 5;
        assert!(!s.dirty);
        s.meta_quiet(InfoHash([2; 20]));
        assert!(s.dirty, "new entries are always saved");
        s.discard();
    }

    #[test]
    fn remove_renumbers_queue() {
        let mut s = Store::load(tmp("r"));
        for i in 0..3u8 {
            s.meta_mut(InfoHash([i; 20]));
        }
        s.remove(&InfoHash([0; 20]));
        let mut ps: Vec<u32> = s.data.torrents.values().map(|m| m.queue_pos).collect();
        ps.sort();
        assert_eq!(ps, vec![0, 1]);
        s.discard();
    }

    #[test]
    fn save_and_reload() {
        let p = tmp("s");
        let mut s = Store::load(p.clone());
        let m = s.meta_mut(InfoHash([9; 20]));
        m.category = Some("Movies".into());
        m.output_folder = Some(std::path::Path::new("D:").join("Media").join("Show"));
        m.only_files = Some(vec![0, 2]);
        m.paused = true;
        s.flush();
        let back = Store::load(p.clone());
        let m = back.get(&InfoHash([9; 20])).unwrap();
        assert_eq!(m.category.as_deref(), Some("Movies"));
        assert_eq!(m.output_folder, Some(std::path::Path::new("D:").join("Media").join("Show")));
        assert_eq!((m.only_files.clone(), m.paused), (Some(vec![0, 2]), true));
        assert!(back.warning.is_none());
        drop((s, back));
        assert_eq!(crate::util::generations(&p, "daily").len(), 1, "the day's first save is kept");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn corrupt_store_falls_back_to_backup() {
        let p = tmp("c");
        let mut s = Store::load(p.clone());
        s.meta_mut(InfoHash([3; 20])).tags = vec!["x".into()];
        s.flush();
        s.meta_mut(InfoHash([4; 20]));
        s.flush(); // second save creates .bak with the first version
        std::fs::write(&p, b"{ broken").unwrap();
        let back = Store::load(p.clone());
        assert!(back.warning.as_deref().unwrap_or("").contains("restored from its backup"));
        assert!(back.get(&InfoHash([3; 20])).is_some());
        s.discard();
        back.discard();
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn damaged_store_and_backup_fall_back_to_a_daily_generation() {
        let p = tmp("d");
        let mut s = Store::load(p.clone());
        s.meta_mut(InfoHash([5; 20])).category = Some("Daily".into());
        s.flush();
        s.discard();
        std::fs::write(&p, b"{ broken").unwrap();
        std::fs::write(p.with_extension("json.bak"), b"also broken").unwrap();
        let back = Store::load(p.clone());
        assert!(back.warning.as_deref().unwrap_or("").contains("restored from its backup"));
        assert_eq!(back.get(&InfoHash([5; 20])).unwrap().category.as_deref(), Some("Daily"));
        back.discard();
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn counters_first_sample_counts_fully() {
        let mut m = TorrentMeta::default();
        assert_eq!(m.absorb_counters(10, 20), (10, 20));
        assert_eq!(m.absorb_counters(10, 20), (0, 0));
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn newer_unreadable_store_is_never_overwritten() {
        let p = std::env::temp_dir().join(format!("corrent-store-newer-{}.json", std::process::id()));
        let future = r#"{"schema_version": 99, "torrents": "a new shape"}"#;
        std::fs::write(&p, future).unwrap();
        let mut s = Store::load(p.clone());
        assert!(s.warning.is_some());
        s.meta_mut(InfoHash([1; 20]));
        s.flush();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), future);
        drop(s);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), future, "not even on drop");
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(test)]
mod save_error_tests {
    use super::*;

    #[test]
    fn unreadable_store_is_never_overwritten() {
        // A directory where the file should be: exists, but can't be read as a file.
        let base = std::env::temp_dir().join(format!("corrent-store-unreadable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = base.join("store.json");
        std::fs::create_dir_all(&path).unwrap();
        let mut s = Store::load(path.clone());
        assert!(s.warning.is_some());
        s.meta_mut(InfoHash([1; 20]));
        s.flush();
        assert!(path.is_dir(), "left as it was");
        assert!(!base.join("store.json.bak").exists());
        s.discard();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn save_failures_are_reported_once() {
        let base = std::env::temp_dir().join(format!("corrent-store-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A *file* where the store's folder should be makes every save fail.
        std::fs::write(base.join("blocked"), b"x").unwrap();
        let mut s = Store::load(base.join("blocked").join("store.json"));
        s.meta_mut(InfoHash([1; 20]));
        s.flush();
        assert!(s.take_save_error().is_some(), "first failure is reported");
        s.mark_dirty();
        s.flush();
        assert!(s.take_save_error().is_none(), "the same failure isn't repeated");
        assert!(s.dirty, "data stays marked for a later retry");
        s.discard();
        let _ = std::fs::remove_dir_all(base);
    }
}
