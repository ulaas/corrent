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
#[derive(Debug, Serialize, Deserialize)]
struct Db {
    torrents: HashMap<usize, Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
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

/// Torrents in the engine's list (0 if it can't be read).
pub fn torrent_count(session_dir: &Path) -> usize {
    std::fs::read(db_file(session_dir)).ok().and_then(|b| parse(&b)).map_or(0, |db| db.torrents.len())
}

/// How `plan` brings the list back.
#[derive(Debug, PartialEq, Eq)]
pub enum How {
    /// From the good copy taken then (unix time).
    Restored { saved_at: i64 },
    /// From the metainfo in the session folder and the store's copy of each torrent's folder
    /// (D6b), because no good copy was usable.
    Rebuilt,
}

/// A torrent the store knows (with its folder) whose metainfo is gone: the engine can't load it,
/// so it is not in a rebuilt list; it shows as a failed load whose Retry looks for the metainfo.
#[derive(Debug, PartialEq, Eq)]
pub struct Missing {
    pub hash: InfoHash,
    pub output_folder: PathBuf,
    pub only_files: Option<Vec<usize>>,
}

/// What `plan` would put back; `commit` writes it.
#[derive(Debug)]
pub struct Plan {
    pub how: How,
    /// Torrents in the new list (all paused).
    pub torrents: Vec<InfoHash>,
    /// Rebuilt into a guessed folder: the store had no record of theirs.
    pub guessed: Vec<InfoHash>,
    pub missing: Vec<Missing>,
    db: Db,
}

/// Where a torrent goes whose folder the store doesn't know: (category, name, multi-file) → folder.
pub type Fallback<'a> = &'a dyn Fn(Option<&str>, &str, bool) -> PathBuf;

/// Before the engine starts: if `session.json` is damaged, or missing while a good copy exists,
/// plan to put the newest good copy back. Entries of the copy that neither the store nor the
/// session folder knows any more (removed since) are dropped. Without a usable copy, the list is
/// rebuilt from `session/<hash>.torrent` and the store (`rebuild`). Every torrent comes back
/// paused, because its folder may have changed since. The damaged file is kept as
/// `session.json.bad-<ts>`.
///
/// `skip` are torrents being removed or re-added (never brought back from here, even if their
/// metainfo is still there).
///
/// `None`: nothing to do. `Err`: the plan couldn't be made (the file is left as it is).
/// The caller records in the store that these torrents are paused *before* `commit`, so a crash in
/// between leaves either the old state (planned again at the next start) or a paused record.
pub fn plan(
    session_dir: &Path,
    store: &crate::store::StoreData,
    skip: &HashSet<InfoHash>,
    fallback: Fallback<'_>,
) -> Option<Result<Plan, String>> {
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
            goods()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let good = goods();
            if good.is_some() {
                tracing::error!("session.json is missing; a good copy exists");
            }
            good
        }
        // Unreadable (locked, permissions): librqbit reports it as itself.
        Err(_) => return None,
    };
    let Some((saved_at, mut db)) = good else {
        let p = rebuild(session_dir, store, skip, fallback);
        // A new profile has no list and nothing to rebuild one from.
        if p.torrents.is_empty() && p.missing.is_empty() && !path.exists() {
            return None;
        }
        tracing::error!(
            "no usable copy of session.json: rebuilding it ({} torrents, {} in a guessed folder, {} without metainfo)",
            p.torrents.len(),
            p.guessed.len(),
            p.missing.len()
        );
        return Some(Ok(p));
    };
    db.torrents.retain(|_, e| {
        let Some(h) = InfoHash::from_hex(&e.info_hash) else { return false };
        !skip.contains(&h)
            && (store.torrents.contains_key(&h) || session_dir.join(format!("{}.torrent", e.info_hash)).exists())
    });
    if store.torrents.is_empty() {
        tracing::warn!("restoring session.json without a store: only torrents with metainfo are kept");
    }
    for e in db.torrents.values_mut() {
        e.is_paused = true;
    }
    let torrents = db.torrents.values().filter_map(|e| InfoHash::from_hex(&e.info_hash)).collect();
    Some(Ok(Plan { how: How::Restored { saved_at }, torrents, guessed: Vec::new(), missing: Vec::new(), db }))
}

/// Metainfo in the session folder that really is the torrent its name says (a file cut off by a
/// crash before librqbit wrote it atomically, or renamed, is not): hash → (name, multi-file).
fn metainfo(session_dir: &Path) -> HashMap<InfoHash, (String, bool)> {
    let Ok(rd) = std::fs::read_dir(session_dir) else { return HashMap::new() };
    rd.flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "torrent") {
                return None;
            }
            let hash = p.file_stem().and_then(|s| s.to_str()).and_then(InfoHash::from_hex)?;
            let bytes = crate::util::read_capped(&p, crate::util::MAX_TORRENT_BYTES).ok()?;
            let Ok(t) = librqbit::torrent_from_bytes(&bytes) else {
                tracing::warn!("{} doesn't parse; not rebuilt from it", p.display());
                return None;
            };
            if t.info_hash.0 != hash.0 {
                tracing::warn!("{} holds another torrent; not rebuilt from it", p.display());
                return None;
            }
            let name = t.info.data.name.as_ref().map(|n| String::from_utf8_lossy(n.as_ref()).into_owned());
            Some((hash, (name.unwrap_or_else(|| hash.to_hex()), t.info.data.files.is_some())))
        })
        .collect()
}

/// D6b: a new list from the metainfo in the session folder, with the folder and file selection
/// the store copied from the engine (`TorrentMeta::output_folder`), in queue order. Metainfo the
/// store has no folder for goes to `fallback`'s folder (`guessed`); store records with a folder
/// but no metainfo can't be loaded by the engine and are returned as `missing`.
fn rebuild(
    session_dir: &Path,
    store: &crate::store::StoreData,
    skip: &HashSet<InfoHash>,
    fallback: Fallback<'_>,
) -> Plan {
    let files = metainfo(session_dir);
    let mut hashes: Vec<InfoHash> =
        files.keys().chain(store.torrents.keys()).filter(|h| !skip.contains(h)).copied().collect();
    hashes.sort_by_key(|h| (store.torrents.get(h).map_or(u32::MAX, |m| m.queue_pos), *h));
    hashes.dedup();
    let mut plan = Plan {
        how: How::Rebuilt,
        torrents: Vec::new(),
        guessed: Vec::new(),
        missing: Vec::new(),
        db: Db { torrents: HashMap::new() },
    };
    for hash in hashes {
        let meta = store.torrents.get(&hash);
        let folder = meta.and_then(|m| m.output_folder.clone());
        let Some((name, multi)) = files.get(&hash) else {
            match folder {
                Some(output_folder) => plan.missing.push(Missing {
                    hash,
                    output_folder,
                    only_files: meta.and_then(|m| m.only_files.clone()),
                }),
                None => tracing::warn!("{}: no metainfo and no folder; not rebuilt", hash.to_hex()),
            }
            continue;
        };
        let output_folder = folder.unwrap_or_else(|| {
            plan.guessed.push(hash);
            fallback(meta.and_then(|m| m.category.as_deref()), name, *multi)
        });
        let entry = Entry {
            info_hash: hash.to_hex(),
            trackers: HashSet::new(),
            output_folder,
            only_files: meta.and_then(|m| m.only_files.clone()),
            is_paused: true,
        };
        plan.db.torrents.insert(plan.torrents.len(), entry);
        plan.torrents.push(hash);
    }
    plan
}

/// Write the planned list. The piece state of torrents in a guessed folder is removed: it was
/// saved for their old folder, so they are checked in full.
pub fn commit(session_dir: &Path, plan: &Plan) -> Result<(), String> {
    let path = db_file(session_dir);
    for h in &plan.guessed {
        let _ = std::fs::remove_file(session_dir.join(format!("{}.bitv", h.to_hex())));
    }
    let bytes = serde_json::to_vec(&plan.db).map_err(|e| format!("{}: {e}", path.display()))?;
    crate::util::write_replace(&path, &bytes).map_err(|e| format!("{}: {e:#}", path.display()))?;
    match plan.how {
        How::Restored { saved_at } => tracing::warn!(
            "session.json restored from the copy of {saved_at} ({} torrents, all paused)",
            plan.torrents.len()
        ),
        How::Rebuilt => tracing::warn!("session.json rebuilt ({} torrents, all paused)", plan.torrents.len()),
    }
    Ok(())
}

/// Data written within this long before a power loss may not have reached the disk.
const UNSAFE_WINDOW: i64 = 300;

/// After a power loss: remove the saved piece state (`<hash>.bitv`) of every torrent that was
/// downloading shortly before `alive_at`, so the engine checks its data in full (as a manual
/// Recheck would). Torrents that were finished or idle by then keep their fast resume. Returns
/// how many will be checked.
pub fn clear_unfinished_bitfields(session_dir: &Path, store: &crate::store::StoreData, alive_at: i64) -> usize {
    let Ok(rd) = std::fs::read_dir(session_dir) else { return 0 };
    let mut cleared = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "bitv") {
            continue;
        }
        let Some(hash) = p.file_stem().and_then(|s| s.to_str()).and_then(InfoHash::from_hex) else { continue };
        // When data was last written: a finished torrent when it finished (seeding only reads),
        // an unfinished one when it last transferred, or was added (its first transfers reach
        // the store's file only with the next save).
        let safe = store.torrents.get(&hash).is_some_and(|m| {
            let last_write = m.completed_at.unwrap_or(m.last_activity.max(m.added_at));
            last_write > 0 && last_write + UNSAFE_WINDOW < alive_at
        });
        if !safe && std::fs::remove_file(&p).is_ok() {
            tracing::info!("after a power loss: {} will be checked in full", hash.to_hex());
            cleared += 1;
        }
    }
    cleared
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

    fn store_of(hashes: &[InfoHash]) -> crate::store::StoreData {
        let mut s = crate::store::StoreData::default();
        for h in hashes {
            s.torrents.insert(*h, Default::default());
        }
        s
    }

    fn no_fallback(_: Option<&str>, _: &str, _: bool) -> PathBuf {
        PathBuf::from("fallback")
    }

    /// Plan and write, like the actor (which saves the store in between).
    fn recover(
        dir: &Path,
        store: &crate::store::StoreData,
        skip: &HashSet<InfoHash>,
        fallback: Fallback<'_>,
    ) -> Option<Plan> {
        let p = plan(dir, store, skip, fallback)?.unwrap();
        commit(dir, &p).unwrap();
        Some(p)
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
        let store = store_of(&[hash(A)]);
        let r = recover(&dir, &store, &HashSet::new(), &no_fallback).unwrap();
        assert_eq!((r.how, r.torrents), (How::Restored { saved_at: 200 }, vec![hash(A)]));
        let back = parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back.torrents.len(), 1);
        assert!(back.torrents.values().all(|e| e.info_hash == A && e.is_paused), "paused");
        let bad = crate::util::generations(&path, "bad");
        assert_eq!(std::fs::read(&bad[0].1).unwrap(), b"{ cut off");
        // Now good: nothing more to do.
        assert!(plan(&dir, &store, &HashSet::new(), &no_fallback).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_torrent_file_in_the_session_folder_keeps_its_entry() {
        let dir = scratch("keep");
        good(&dir, 100, &db(&[(0, A), (1, B)]));
        std::fs::write(dir.join(format!("{B}.torrent")), b"d4:infoe").unwrap();
        let empty = crate::store::StoreData::default();
        let r = recover(&dir, &empty, &HashSet::new(), &no_fallback).unwrap();
        assert_eq!(r.torrents, vec![hash(B)], "B (its metainfo is there), not A");
        // Being removed: not brought back even with its metainfo.
        std::fs::write(dir.join("session.json"), "{").unwrap();
        assert!(recover(&dir, &empty, &[hash(B)].into(), &no_fallback).unwrap().torrents.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_list_comes_back_only_if_there_was_one() {
        let dir = scratch("missing");
        let empty = crate::store::StoreData::default();
        assert!(plan(&dir, &empty, &HashSet::new(), &no_fallback).is_none(), "a new profile");
        good(&dir, 100, &db(&[(0, A)]));
        let r = recover(&dir, &store_of(&[hash(A)]), &HashSet::new(), &no_fallback).unwrap();
        assert_eq!(r.torrents, vec![hash(A)]);
        assert!(dir.join("session.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A tiny torrent's metainfo (made-up piece hashes), saved in `dir` as librqbit does.
    fn metainfo_file(dir: &Path, i: u8, multi: bool) -> InfoHash {
        let name = format!("t{i}");
        let mut b = b"d4:infod".to_vec();
        if multi {
            b.extend_from_slice(b"5:filesld6:lengthi16384e4:pathl1:aeed6:lengthi16384e4:pathl1:beee");
        } else {
            b.extend_from_slice(b"6:lengthi16384e");
        }
        b.extend_from_slice(format!("4:name{}:{name}12:piece lengthi16384e6:pieces", name.len()).as_bytes());
        let pieces = if multi { 40 } else { 20 };
        b.extend_from_slice(format!("{pieces}:").as_bytes());
        b.extend_from_slice(&vec![i; pieces]);
        b.extend_from_slice(b"ee");
        let h = InfoHash(librqbit::torrent_from_bytes(&b).unwrap().info_hash.0);
        std::fs::write(dir.join(format!("{}.torrent", h.to_hex())), &b).unwrap();
        std::fs::write(dir.join(format!("{}.bitv", h.to_hex())), b"x").unwrap();
        h
    }

    #[test]
    fn without_a_usable_copy_the_list_is_rebuilt_from_the_metainfo_and_the_store() {
        let dir = scratch("rebuild");
        let path = dir.join("session.json");
        std::fs::write(&path, "garbage").unwrap();
        good(&dir, 100, "also garbage");
        let a = metainfo_file(&dir, 1, true);
        let b = metainfo_file(&dir, 2, false);
        let e = metainfo_file(&dir, 3, true);
        let (c, d) = (InfoHash([0xcc; 20]), InfoHash([0xdd; 20]));
        let mut store = store_of(&[a, b, c, d]);
        let set = |m: &mut crate::store::TorrentMeta, folder: &str, queue: u32| {
            m.output_folder = Some(PathBuf::from(folder));
            m.queue_pos = queue;
        };
        set(store.torrents.get_mut(&a).unwrap(), "X/t1", 1);
        store.torrents.get_mut(&a).unwrap().only_files = Some(vec![1]);
        set(store.torrents.get_mut(&b).unwrap(), "Y", 0);
        // C: a folder but no metainfo. D: neither. E: metainfo but no record.
        set(store.torrents.get_mut(&c).unwrap(), "Z", 2);
        let fallback = |cat: Option<&str>, name: &str, multi: bool| {
            assert_eq!((cat, name, multi), (None, "t3", true));
            PathBuf::from("dl").join(name)
        };
        let r = recover(&dir, &store, &HashSet::new(), &fallback).unwrap();
        assert_eq!(r.how, How::Rebuilt);
        assert_eq!(r.torrents, vec![b, a, e], "queue order, then the unrecorded one");
        assert_eq!(r.guessed, vec![e]);
        assert_eq!(r.missing, vec![Missing { hash: c, output_folder: PathBuf::from("Z"), only_files: None }]);

        let back = parse(&std::fs::read(&path).unwrap()).unwrap();
        let entry = |id: usize| back.torrents.get(&id).unwrap();
        assert_eq!(
            (entry(0).info_hash.as_str(), entry(0).output_folder.as_path()),
            (b.to_hex().as_str(), Path::new("Y"))
        );
        assert_eq!(entry(1).output_folder, PathBuf::from("X/t1"));
        assert_eq!(entry(1).only_files, Some(vec![1]));
        assert_eq!(entry(2).output_folder, PathBuf::from("dl").join("t3"));
        assert!(back.torrents.values().all(|e| e.is_paused), "all paused");
        // The guessed one is checked in full; the others keep fast resume.
        assert!(!dir.join(format!("{}.bitv", e.to_hex())).exists());
        assert!(dir.join(format!("{}.bitv", a.to_hex())).exists());
        assert_eq!(std::fs::read(&crate::util::generations(&path, "bad")[0].1).unwrap(), b"garbage");
        // What librqbit reads back (through the same view as failed restores).
        let entries = super::super::restore::persisted_entries(&dir);
        assert_eq!(entries.len(), 3);
        assert!(plan(&dir, &store, &HashSet::new(), &fallback).is_none(), "now good");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_rebuild_leaves_out_torrents_being_removed_or_re_added() {
        let dir = scratch("rebuild-skip");
        let a = metainfo_file(&dir, 1, false);
        let b = metainfo_file(&dir, 2, false);
        let r = recover(&dir, &store_of(&[a, b]), &[a].into(), &no_fallback).unwrap();
        assert_eq!(r.torrents, vec![b]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn metainfo_that_is_not_the_torrent_it_names_is_not_trusted() {
        let dir = scratch("rebuild-trust");
        let a = metainfo_file(&dir, 1, false);
        let b = metainfo_file(&dir, 2, false);
        // A cut off; B's file holds A's metainfo.
        let a_file = dir.join(format!("{}.torrent", a.to_hex()));
        let bytes = std::fs::read(&a_file).unwrap();
        std::fs::write(dir.join(format!("{}.torrent", b.to_hex())), &bytes).unwrap();
        std::fs::write(&a_file, &bytes[..bytes.len() / 2]).unwrap();
        let mut store = store_of(&[a, b]);
        for m in store.torrents.values_mut() {
            m.output_folder = Some(PathBuf::from("dl"));
        }
        let r = recover(&dir, &store, &HashSet::new(), &no_fallback).unwrap();
        assert!(r.torrents.is_empty());
        assert_eq!(r.missing.len(), 2, "both shown as missing their metainfo");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_list_with_nothing_to_rebuild_becomes_an_empty_one() {
        let dir = scratch("rebuild-empty");
        let path = dir.join("session.json");
        std::fs::write(&path, "garbage").unwrap();
        let r = recover(&dir, &crate::store::StoreData::default(), &HashSet::new(), &no_fallback).unwrap();
        assert!(r.torrents.is_empty());
        assert_eq!(parse(&std::fs::read(&path).unwrap()).unwrap().torrents.len(), 0, "the engine can start");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_torrents_written_shortly_before_the_power_loss_lose_their_bitfield() {
        let dir = scratch("bitv");
        let now = 1_000_000;
        let mut store = crate::store::StoreData::default();
        let cases = [
            ("1", Some(now - 3600), now - 5, true), // finished an hour before, seeding now
            ("2", Some(now - 60), now - 60, false), // finished a minute before
            ("3", None, now - 10, false),           // downloading
            ("4", None, now - 3600, true),          // paused for an hour
            ("5", None, 0, true),                   // added an hour ago, never transferred
            ("7", None, 0, false),                  // added a minute ago
        ];
        for (c, completed, active, _) in cases {
            let h = c.repeat(40);
            std::fs::write(dir.join(format!("{h}.bitv")), b"x").unwrap();
            let m = store.torrents.entry(hash(&h)).or_default();
            m.completed_at = completed;
            m.last_activity = active;
            m.added_at = if c == "7" { now - 60 } else { now - 7200 };
        }
        // Not in the store: checked.
        std::fs::write(dir.join(format!("{}.bitv", "6".repeat(40))), b"x").unwrap();
        std::fs::write(dir.join("other.bitv"), b"x").unwrap();
        assert_eq!(clear_unfinished_bitfields(&dir, &store, now), 4);
        for (c, _, _, kept) in cases {
            assert_eq!(dir.join(format!("{}.bitv", c.repeat(40))).exists(), kept, "{c}");
        }
        assert!(!dir.join(format!("{}.bitv", "6".repeat(40))).exists());
        assert!(dir.join("other.bitv").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
