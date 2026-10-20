//! Startup clean-up of the copies Corrent keeps when something goes wrong (STABILITY.md F15):
//! damaged files moved aside, migration backups and the data folder of a restore. Each kind
//! keeps its 3 newest copies plus every copy younger than 30 days, so a problem still leaves
//! the evidence for a while; anything else in the data folder is never touched.
//!
//! The other generations bound themselves when they are written (`util::keep_generations`:
//! `store.json.daily-*`, `store.json.pre-prune-*`, `session/session.json.good-*`/`.bad-*`;
//! crash reports; `logfile` rotation).

use std::path::PathBuf;
use std::time::Duration;

use crate::config::Paths;

/// Copies of each kind kept whatever their age.
pub const KEEP: usize = 3;
/// Copies younger than this are kept too.
pub const KEEP_FOR: Duration = Duration::from_secs(30 * 86_400);

#[derive(Debug, Default)]
pub struct Pruned {
    pub removed: Vec<PathBuf>,
    pub failed: usize,
}

/// The kind and Unix time of a copy Corrent made in the data folder, from its name.
fn kind_of(name: &str, is_dir: bool) -> Option<(String, u64)> {
    if is_dir {
        let ts = name.strip_prefix("pre-restore-")?;
        return Some(("pre-restore".into(), digits(ts)?));
    }
    for file in ["settings.toml", "store.json", "rss.json"] {
        let Some(rest) = name.strip_prefix(file).and_then(|r| r.strip_prefix('.')) else { continue };
        if let Some(ts) = rest.strip_prefix("bad-") {
            return Some((format!("{file}.bad"), digits(ts)?));
        }
        // Migration backups: `<file>.v<N>-<time>` (one per schema version).
        if let Some((v, ts)) = rest.strip_prefix('v').and_then(|r| r.split_once('-'))
            && digits(v).is_some()
        {
            return Some((format!("{file}.v"), digits(ts)?));
        }
    }
    None
}

fn digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Remove the old copies in `paths.data_dir`, given the current Unix time `now`.
pub fn prune_at(paths: &Paths, now: u64) -> Pruned {
    let mut kinds: std::collections::HashMap<String, Vec<(u64, PathBuf, bool)>> = Default::default();
    for e in std::fs::read_dir(&paths.data_dir).into_iter().flatten().flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some((kind, ts)) = kind_of(&name, ft.is_dir()) {
            kinds.entry(kind).or_default().push((ts, e.path(), ft.is_dir()));
        }
    }
    let mut out = Pruned::default();
    for (_, mut copies) in kinds {
        copies.sort_by_key(|c| std::cmp::Reverse(c.0));
        for (ts, path, dir) in copies.into_iter().skip(KEEP) {
            if now.saturating_sub(ts) < KEEP_FOR.as_secs() {
                continue;
            }
            let r = if dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            match r {
                Ok(()) => out.removed.push(path),
                Err(e) => {
                    tracing::warn!("couldn't remove the old copy {}: {e}", path.display());
                    out.failed += 1;
                }
            }
        }
    }
    out
}

/// Remove the temp files a crash left behind (`<name>.tmp`, written before a rename), in the
/// data folder and the folders Corrent writes atomically into. Call it at start, before the
/// engine or the writer runs: then no `.tmp` file belongs to a write in progress
/// (STABILITY.md D8).
pub fn sweep_temp_files(paths: &Paths) -> usize {
    let dirs = [paths.data_dir.clone(), paths.session_dir(), paths.torrents_dir(), paths.data_dir.join("ipfilter")];
    let mut n = 0;
    for dir in dirs {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let is_file = e.file_type().is_ok_and(|t| t.is_file());
            if is_file && e.file_name().to_string_lossy().ends_with(".tmp") && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    if n > 0 {
        tracing::info!("removed {n} temp files left by an interrupted save");
    }
    n
}

/// [`prune_at`] now, logging what was removed.
pub fn prune(paths: &Paths) -> Pruned {
    let out = prune_at(paths, u64::try_from(crate::util::now_unix()).unwrap_or(0));
    if !out.removed.is_empty() {
        tracing::info!("removed {} old backup copies from {}", out.removed.len(), paths.data_dir.display());
    }
    out
}

/// [`prune`] on a background thread: nothing reads these copies, and removing a big
/// `pre-restore-*` folder can take a while.
pub fn prune_in_background(paths: &Paths) {
    let paths = paths.clone();
    let _ = std::thread::Builder::new().name("corrent-retention".into()).spawn(move || prune(&paths));
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const DAY: u64 = 86_400;
    const NOW: u64 = 2_000_000_000;

    fn dir(name: &str) -> (PathBuf, Paths) {
        let d = std::env::temp_dir().join(format!("corrent-retention-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (d.clone(), Paths { data_dir: d, portable: true })
    }

    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn keeps_three_newest_and_anything_recent() {
        let (d, paths) = dir("old");
        // 20 of each kind, all older than 30 days (one a day, from 40 days back).
        for i in 0..20u64 {
            let ts = NOW - (40 + i) * DAY;
            for f in ["settings.toml.bad", "store.json.bad", "rss.json.bad"] {
                std::fs::write(d.join(format!("{f}-{ts}")), "x").unwrap();
            }
            std::fs::write(d.join(format!("store.json.v{}-{ts}", i % 3)), "x").unwrap();
            std::fs::create_dir_all(d.join(format!("pre-restore-{ts}/session"))).unwrap();
            std::fs::write(d.join(format!("pre-restore-{ts}/session/session.json")), "{}").unwrap();
        }
        let out = prune_at(&paths, NOW);
        assert_eq!(out.failed, 0);
        assert_eq!(out.removed.len(), 5 * 17);
        let left = names(&d);
        let newest = [NOW - 40 * DAY, NOW - 41 * DAY, NOW - 42 * DAY];
        for f in ["settings.toml.bad", "store.json.bad", "rss.json.bad"] {
            for ts in newest {
                assert!(left.contains(&format!("{f}-{ts}")), "{f}-{ts} kept: {left:?}");
            }
        }
        assert_eq!(left.iter().filter(|n| n.starts_with("pre-restore-")).count(), 3);
        assert_eq!(left.iter().filter(|n| n.starts_with("store.json.v")).count(), 3);
        assert_eq!(left.len(), 15);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn recent_copies_survive() {
        let (d, paths) = dir("recent");
        for i in 0..3u64 {
            std::fs::write(d.join(format!("store.json.bad-{}", NOW - (60 + i) * DAY)), "x").unwrap();
        }
        for i in 0..5u64 {
            std::fs::write(d.join(format!("store.json.bad-{}", NOW - i * DAY)), "x").unwrap();
        }
        let out = prune_at(&paths, NOW);
        assert_eq!(out.removed.len(), 3, "only the old ones beyond the newest 3");
        assert_eq!(names(&d).len(), 5);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn temp_files_from_interrupted_saves_are_swept() {
        let (d, paths) = dir("tmp");
        for sub in ["session", "torrents", "ipfilter", "logs"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        for f in [
            "store.json.tmp",
            "settings.toml.bak.tmp",
            "session/session.json.tmp",
            "session/ab.bitv.tmp",
            "torrents/ab.torrent.tmp",
            "ipfilter/blocklist-download.txt.tmp",
        ] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        // Not Corrent's temp files, or not in its folders.
        for f in ["store.json", "notes.tmp.txt", "logs/x.tmp"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        std::fs::create_dir_all(d.join("folder.tmp")).unwrap();
        assert_eq!(sweep_temp_files(&paths), 6);
        assert!(d.join("store.json").exists() && d.join("notes.tmp.txt").exists() && d.join("logs/x.tmp").exists());
        assert!(d.join("folder.tmp").is_dir());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn other_files_are_never_touched() {
        let (d, paths) = dir("other");
        let old = NOW - 400 * DAY;
        let keep = [
            "settings.toml".to_string(),
            "settings.toml.bak".into(),
            "store.json".into(),
            "store.json.bak".into(),
            "rss.json".into(),
            format!("store.json.pre-prune-{old}"),
            format!("store.json.daily-{old}"),
            "store.json.bad-".into(),
            "store.json.bad-12x".into(),
            "notes.txt".into(),
        ];
        for n in &keep {
            std::fs::write(d.join(n), "x").unwrap();
        }
        std::fs::create_dir_all(d.join("session")).unwrap();
        std::fs::create_dir_all(d.join("pre-restore-notanumber")).unwrap();
        // A file named like a restore folder isn't one.
        std::fs::write(d.join(format!("pre-restore-{old}")), "x").unwrap();
        for i in 0..5u64 {
            std::fs::write(d.join(format!("settings.toml.bad-{}", old - i)), "x").unwrap();
        }
        let out = prune_at(&paths, NOW);
        assert_eq!(out.removed.len(), 2, "{:?}", out.removed);
        let left = names(&d);
        for n in keep.iter().map(String::as_str).chain(["session", "pre-restore-notanumber"]) {
            assert!(left.iter().any(|l| l == n), "{n} kept");
        }
        assert!(left.contains(&format!("pre-restore-{old}")));
        let _ = std::fs::remove_dir_all(d);
    }
}
