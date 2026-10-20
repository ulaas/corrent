//! Torrents that librqbit failed to restore at startup (e.g. a file locked or blocked by
//! antivirus). librqbit logs the error and keeps them in its session database without loading
//! them, so they would silently vanish. We detect them, keep showing them as errors and offer
//! retry/remove.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librqbit::api::TorrentIdOrHash;
use librqbit::dht::Id20;
use librqbit::{AddTorrent, AddTorrentOptions, Session};

use super::types::InfoHash;

/// `FailedRestore::id` of a torrent that is not in the engine's list (D6b: its metainfo is gone).
pub const NOT_LISTED: usize = usize::MAX;

#[derive(Clone, Debug)]
pub struct FailedRestore {
    pub id: usize,
    pub hash: InfoHash,
    pub output_folder: PathBuf,
    pub only_files: Option<Vec<usize>>,
    pub paused: bool,
    pub trackers: Vec<String>,
    pub error: String,
}

/// Entries in librqbit's `session.json` (id → persisted torrent).
pub fn persisted_entries(session_dir: &Path) -> Vec<FailedRestore> {
    let Ok(bytes) = std::fs::read(session_dir.join("session.json")) else { return Vec::new() };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return Vec::new() };
    let Some(map) = v.get("torrents").and_then(|t| t.as_object()) else { return Vec::new() };
    map.iter()
        .filter_map(|(id, t)| {
            let hash = InfoHash::from_hex(t.get("info_hash")?.as_str()?)?;
            Some(FailedRestore {
                id: id.parse().ok()?,
                hash,
                output_folder: PathBuf::from(t.get("output_folder")?.as_str()?),
                only_files: t
                    .get("only_files")
                    .and_then(|o| o.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect()),
                paused: t.get("is_paused").and_then(|p| p.as_bool()).unwrap_or(false),
                trackers: t
                    .get("trackers")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                error: String::new(),
            })
        })
        .collect()
}

/// Try to add a persisted torrent back into the session; returns the error text on failure.
pub async fn retry(
    sess: &Arc<Session>,
    session_dir: &Path,
    f: &FailedRestore,
    peer_limit: Option<usize>,
) -> Result<(), String> {
    if sess.get(TorrentIdOrHash::Hash(Id20::new(f.hash.0))).is_some() {
        return Ok(());
    }
    let torrent_file = session_dir.join(format!("{}.torrent", f.hash.to_hex()));
    let add = match tokio::fs::read(&torrent_file).await {
        Ok(bytes) => AddTorrent::from_bytes(bytes),
        Err(_) => AddTorrent::from_url(format!("magnet:?xt=urn:btih:{}", f.hash.to_hex())),
    };
    let opts = AddTorrentOptions {
        paused: f.paused,
        only_files: f.only_files.clone(),
        output_folder: Some(f.output_folder.to_string_lossy().into_owned()),
        overwrite: true,
        // No preferred id: a torrent added since may have been given this one, and librqbit would
        // then report that torrent as "already managed".
        trackers: (!f.trackers.is_empty()).then(|| f.trackers.clone()),
        peer_limit,
        ..Default::default()
    };
    match sess.add_torrent(add, Some(opts)).await {
        Ok(librqbit::AddTorrentResponse::Added(..)) => Ok(()),
        Ok(librqbit::AddTorrentResponse::AlreadyManaged(_, h)) if h.info_hash().0 == f.hash.0 => Ok(()),
        Ok(_) => Err(t!("the engine didn't take the torrent back").into()),
        Err(e) => Err(friendly(&format!("{e:#}"))),
    }
}

/// Make common OS errors understandable.
pub fn friendly(e: &str) -> String {
    crate::util::explain_io(e)
}

/// Remove entries from librqbit's database while the session is stopped.
pub fn purge(session_dir: &Path, hashes: &HashSet<InfoHash>) {
    if hashes.is_empty() {
        return;
    }
    let path = session_dir.join("session.json");
    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(map) = v.get_mut("torrents").and_then(|t| t.as_object_mut())
    {
        map.retain(|_, t| {
            t.get("info_hash")
                .and_then(|h| h.as_str())
                .and_then(InfoHash::from_hex)
                .is_none_or(|h| !hashes.contains(&h))
        });
        if let Ok(out) = serde_json::to_vec(&v)
            && let Err(e) = crate::util::write_atomic(&path, &out)
        {
            tracing::error!("could not update session database: {e:#}");
        }
    }
    for h in hashes {
        for ext in ["torrent", "bitv"] {
            let _ = std::fs::remove_file(session_dir.join(format!("{}.{ext}", h.to_hex())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_purge() {
        let dir = std::env::temp_dir().join(format!("corrent-restore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = "aa".repeat(20);
        let b = "bb".repeat(20);
        let json = format!(
            r#"{{"torrents":{{"0":{{"info_hash":"{a}","trackers":["udp://t:1"],"output_folder":"C:/x","only_files":[0,2],"is_paused":true}},"3":{{"info_hash":"{b}","trackers":[],"output_folder":"C:/y","only_files":null,"is_paused":false}}}}}}"#
        );
        std::fs::write(dir.join("session.json"), json).unwrap();
        let mut e = persisted_entries(&dir);
        e.sort_by_key(|x| x.id);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].only_files, Some(vec![0, 2]));
        assert!(e[0].paused);
        assert_eq!(e[1].id, 3);
        purge(&dir, &HashSet::from([InfoHash::from_hex(&a).unwrap()]));
        let e = persisted_entries(&dir);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].hash.to_hex(), b);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn friendly_messages() {
        assert!(friendly("error opening x: Access is denied. (os error 5)").contains("antivirus"));
        assert!(friendly("(os error 32)").contains("in use"));
        assert!(friendly("No space left on device").contains("disk is full"));
        assert_eq!(friendly("something else"), "something else");
    }

    #[test]
    fn missing_session_file_is_empty() {
        assert!(persisted_entries(std::path::Path::new("Z:/definitely/not/here")).is_empty());
    }
}
