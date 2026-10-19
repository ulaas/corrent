//! The engine's own database, `session/session.json` (librqbit's list of torrents with their
//! folders and trackers). librqbit refuses to start with a damaged one, so Corrent keeps the
//! last good copies (`session.json.good-<ts>`) and puts one back when it has to.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::InfoHash;

/// Good copies kept.
const KEEP: usize = 3;

/// librqbit's `SerializedSessionDatabase` (a private type), with the same field types so that
/// whatever this accepts, librqbit accepts too.
#[derive(Serialize, Deserialize)]
struct Db {
    torrents: HashMap<usize, Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    /// 40 hex digits (checked in `parse`).
    info_hash: String,
    trackers: HashSet<String>,
    output_folder: PathBuf,
    only_files: Option<Vec<usize>>,
    is_paused: bool,
}

fn parse(bytes: &[u8]) -> Option<Db> {
    let db: Db = serde_json::from_slice(bytes).ok()?;
    db.torrents.values().all(|e| e.info_hash.len() == 40 && InfoHash::from_hex(&e.info_hash).is_some()).then_some(db)
}

fn db_file(session_dir: &Path) -> PathBuf {
    session_dir.join("session.json")
}

/// Keep the current `session.json` as a good copy if it parses and differs from the newest one
/// (compared as data: librqbit writes its entries in a different order every run).
pub fn snapshot(session_dir: &Path) {
    let path = db_file(session_dir);
    let Ok(bytes) = std::fs::read(&path) else { return };
    if parse(&bytes).is_none() {
        return;
    }
    let value = |b: &[u8]| serde_json::from_slice::<serde_json::Value>(b).ok();
    let newest = crate::util::generations(&path, "good").into_iter().next();
    if let Some((_, good)) = newest
        && std::fs::read(&good).ok().and_then(|b| value(&b)) == value(&bytes)
    {
        return;
    }
    if let Err(e) = crate::util::keep_generations(&path, "good", &bytes, KEEP) {
        tracing::warn!("could not keep a copy of session.json: {e:#}");
    }
}

/// What `recover` put back.
#[derive(Debug, PartialEq, Eq)]
pub struct Recovered {
    /// When the copy was taken (unix time).
    pub saved_at: i64,
    /// Torrents in the restored list (all paused).
    pub torrents: usize,
}

/// Before the engine starts: if `session.json` is damaged, or missing while a good copy exists,
/// put the newest good copy back. `known` are the torrents in Corrent's store; entries of the
/// copy that neither the store nor the session folder knows any more (removed since) are dropped,
/// and every torrent comes back paused, because its folder may have changed since the copy.
/// The damaged file is kept as `session.json.bad-<ts>`.
///
/// `None`: nothing to do. `Err`: damaged and no usable copy (the file is left as it is).
pub fn recover(session_dir: &Path, known: &HashSet<InfoHash>) -> Option<Result<Recovered, String>> {
    let path = db_file(session_dir);
    let goods = || {
        crate::util::generations(&path, "good")
            .into_iter()
            .find_map(|(ts, p)| std::fs::read(&p).ok().and_then(|b| parse(&b)).map(|db| (ts, db)))
    };
    let good = match std::fs::read(&path) {
        Ok(bytes) if parse(&bytes).is_some() => return None,
        Ok(bytes) => {
            tracing::error!("session.json is damaged");
            if let Err(e) = crate::util::keep_generations(&path, "bad", &bytes, KEEP) {
                tracing::warn!("could not keep a copy of the damaged session.json: {e:#}");
            }
            match goods() {
                Some(good) => good,
                None => return Some(Err(path.display().to_string())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A new profile has no copies; one that had torrents lost its list.
            let good = goods()?;
            tracing::error!("session.json is missing; a good copy exists");
            good
        }
        // Unreadable (locked, permissions): librqbit reports it as itself.
        Err(_) => return None,
    };
    let (saved_at, mut db) = good;
    db.torrents.retain(|_, e| {
        let hash = InfoHash::from_hex(&e.info_hash);
        hash.is_some_and(|h| known.contains(&h)) || session_dir.join(format!("{}.torrent", e.info_hash)).exists()
    });
    for e in db.torrents.values_mut() {
        e.is_paused = true;
    }
    let bytes = match serde_json::to_vec(&db) {
        Ok(b) => b,
        Err(e) => return Some(Err(format!("{}: {e}", path.display()))),
    };
    if let Err(e) = crate::util::write_replace(&path, &bytes) {
        return Some(Err(format!("{}: {e:#}", path.display())));
    }
    tracing::warn!("session.json restored from the copy of {saved_at} ({} torrents, all paused)", db.torrents.len());
    Some(Ok(Recovered { saved_at, torrents: db.torrents.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn entry(hash: &str, paused: bool) -> String {
        format!(
            r#"{{"info_hash":"{hash}","trackers":[],"output_folder":"C:/dl","only_files":null,"is_paused":{paused}}}"#
        )
    }

    fn db(entries: &[(usize, &str)]) -> String {
        let body: Vec<String> = entries.iter().map(|(id, h)| format!(r#""{id}":{}"#, entry(h, false))).collect();
        format!(r#"{{"torrents":{{{}}}}}"#, body.join(","))
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-sessiondb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hash(h: &str) -> InfoHash {
        InfoHash::from_hex(h).unwrap()
    }

    #[test]
    fn parses_exactly_what_librqbit_accepts() {
        assert!(parse(db(&[(0, A), (1, B)]).as_bytes()).is_some());
        assert!(parse(br#"{"torrents":{}}"#).is_some());
        // Unknown fields are ignored, like librqbit does.
        assert!(parse(br#"{"torrents":{},"new":1}"#).is_some());
        for bad in [
            "".to_string(),
            "not json".into(),
            db(&[(0, A)])[..30].to_string(),
            r#"{"torrents":{"0":{"info_hash":5}}}"#.into(),
            db(&[(0, "abc")]),
            db(&[(0, &"z".repeat(40))]),
            db(&[(0, A)]).replace(r#""is_paused":false"#, r#""is_paused":"no""#),
            db(&[(0, A)]).replace(r#","is_paused":false"#, ""),
            r#"{"torrents":{"x":{}}}"#.into(),
        ] {
            assert!(parse(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[test]
    fn snapshots_only_when_the_list_changes() {
        let dir = scratch("snap");
        let path = dir.join("session.json");
        std::fs::write(&path, db(&[(0, A), (1, B)])).unwrap();
        snapshot(&dir);
        // The same list in another order (librqbit's map) is not a new copy.
        std::fs::write(&path, db(&[(1, B), (0, A)])).unwrap();
        snapshot(&dir);
        assert_eq!(crate::util::generations(&path, "good").len(), 1);
        // Damage is never kept as good.
        std::fs::write(&path, "{ cut off").unwrap();
        snapshot(&dir);
        assert_eq!(crate::util::generations(&path, "good").len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn good(dir: &Path, ts: i64, body: &str) {
        std::fs::write(dir.join(format!("session.json.good-{ts}")), body).unwrap();
    }

    #[test]
    fn a_damaged_list_comes_back_from_the_newest_good_copy() {
        let dir = scratch("recover");
        let path = dir.join("session.json");
        good(&dir, 100, &db(&[(0, A)]));
        good(&dir, 200, &db(&[(0, A), (1, B)]));
        good(&dir, 300, "damaged too");
        std::fs::write(&path, "{ cut off").unwrap();
        // B was removed since the copy: neither the store nor the session folder knows it.
        let known: HashSet<InfoHash> = [hash(A)].into();
        let r = recover(&dir, &known).unwrap().unwrap();
        assert_eq!(r, Recovered { saved_at: 200, torrents: 1 });
        let back = parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.torrents.len(), 1);
        assert!(back.torrents.values().all(|e| e.info_hash == A && e.is_paused), "paused");
        let bad = crate::util::generations(&path, "bad");
        assert_eq!(std::fs::read(&bad[0].1).unwrap(), b"{ cut off");
        // Now good: nothing more to do.
        assert_eq!(recover(&dir, &known), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_torrent_file_in_the_session_folder_keeps_its_entry() {
        let dir = scratch("keep");
        good(&dir, 100, &db(&[(0, A), (1, B)]));
        std::fs::write(dir.join(format!("{B}.torrent")), b"d4:infoe").unwrap();
        let r = recover(&dir, &HashSet::new()).unwrap().unwrap();
        assert_eq!(r.torrents, 1, "B (its metainfo is there), not A");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_list_comes_back_only_if_there_was_one() {
        let dir = scratch("missing");
        assert_eq!(recover(&dir, &HashSet::new()), None, "a new profile");
        good(&dir, 100, &db(&[(0, A)]));
        let r = recover(&dir, &[hash(A)].into()).unwrap().unwrap();
        assert_eq!(r.torrents, 1);
        assert!(dir.join("session.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_a_usable_copy_the_file_is_left_alone() {
        let dir = scratch("none");
        let path = dir.join("session.json");
        std::fs::write(&path, "garbage").unwrap();
        good(&dir, 100, "also garbage");
        assert!(matches!(recover(&dir, &HashSet::new()), Some(Err(_))));
        assert_eq!(std::fs::read(&path).unwrap(), b"garbage");
        let _ = std::fs::remove_dir_all(dir);
    }
}
