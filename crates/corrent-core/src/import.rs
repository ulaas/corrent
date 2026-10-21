//! Import torrents from other clients' data folders (read-only).
//!
//! - qBittorrent: `BT_backup/<hash>.torrent` + `<hash>.fastresume` (save path, category, tags).
//! - Transmission: `torrents/*.torrent` + `resume/*.resume` (download folder).
//! - Deluge: `state/*.torrent` only (its other state is a Python pickle).
//!
//! Imported torrents are added with the data where the other client left it; the engine
//! verifies existing files and never downloads them again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resume data of other clients (a few bytes per piece; bigger is not resume data).
const MAX_RESUME_BYTES: u64 = 16 << 20;

use librqbit_bencode::{BencodeValue, ByteBufOwned};

use crate::engine::types::InfoHash;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Client {
    QBittorrent,
    Transmission,
    Deluge,
}

impl Client {
    pub const ALL: [Client; 3] = [Client::QBittorrent, Client::Transmission, Client::Deluge];

    pub fn name(self) -> &'static str {
        match self {
            Client::QBittorrent => "qBittorrent",
            Client::Transmission => "Transmission",
            Client::Deluge => "Deluge",
        }
    }

    /// Where the client keeps its torrents by default on this OS.
    pub fn default_dir(self) -> Option<PathBuf> {
        let home = directories::BaseDirs::new()?;
        let (local, config) = (home.data_local_dir().to_path_buf(), home.config_dir().to_path_buf());
        Some(match self {
            Client::QBittorrent if cfg!(target_os = "linux") => home.data_dir().join("qBittorrent/BT_backup"),
            Client::QBittorrent => local.join("qBittorrent").join("BT_backup"),
            Client::Transmission if cfg!(windows) => local.join("transmission"),
            Client::Transmission if cfg!(target_os = "macos") => config.join("Transmission"),
            Client::Transmission => config.join("transmission"),
            Client::Deluge if cfg!(windows) => home.config_dir().join("deluge").join("state"),
            Client::Deluge => config.join("deluge/state"),
        })
    }
}

#[derive(Clone, Debug)]
pub struct ImportItem {
    pub name: String,
    pub hash: InfoHash,
    pub bytes: Vec<u8>,
    /// Folder the other client saved into (None = unknown, use the default).
    pub save_path: Option<PathBuf>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub paused: bool,
    /// Multi-file content sits in a folder named after the torrent inside `save_path`.
    pub create_subfolder: bool,
    /// The data was found where the other client left it (otherwise adding it downloads again).
    pub data_found: bool,
}

pub(crate) type Dict = HashMap<ByteBufOwned, BencodeValue<ByteBufOwned>>;

pub(crate) fn dict(bytes: &[u8]) -> Option<Dict> {
    match librqbit_bencode::dyn_from_bytes::<ByteBufOwned>(bytes).ok()? {
        BencodeValue::Dict(d) => Some(d),
        _ => None,
    }
}

fn get<'a>(d: &'a Dict, key: &str) -> Option<&'a BencodeValue<ByteBufOwned>> {
    d.get(&ByteBufOwned::from(key.as_bytes()))
}

fn get_str(d: &Dict, key: &str) -> Option<String> {
    match get(d, key)? {
        BencodeValue::Bytes(b) => Some(String::from_utf8_lossy(b.as_ref()).into_owned()).filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn get_int(d: &Dict, key: &str) -> Option<i64> {
    match get(d, key)? {
        BencodeValue::Integer(i) => Some(*i),
        _ => None,
    }
}

fn get_str_list(d: &Dict, key: &str) -> Vec<String> {
    match get(d, key) {
        Some(BencodeValue::List(l)) => l
            .iter()
            .filter_map(|v| match v {
                BencodeValue::Bytes(b) => Some(String::from_utf8_lossy(b.as_ref()).into_owned()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// What the importer needs from a .torrent.
pub(crate) struct Parsed {
    hash: InfoHash,
    name: String,
    bytes: Vec<u8>,
    /// First file's path inside the torrent (None for single-file torrents).
    first_file: Option<PathBuf>,
}

pub(crate) fn parse_torrent(bytes: Vec<u8>) -> Option<Parsed> {
    let t = librqbit::torrent_from_bytes(&bytes).ok()?;
    let hash = InfoHash(t.info_hash.0);
    let name = t.info.data.name.as_ref().map(|n| String::from_utf8_lossy(n.as_ref()).into_owned());
    let first_file = t
        .info
        .data
        .files
        .as_ref()
        .and_then(|f| f.first())
        .map(|f| f.path.iter().map(|c| String::from_utf8_lossy(c.as_ref()).into_owned()).collect::<PathBuf>());
    Some(Parsed { hash, name: name.unwrap_or_else(|| hash.to_hex()), bytes, first_file })
}

/// Whether the content is where `base` + layout say it is.
fn data_at(t: &Parsed, base: &Path, subfolder: bool) -> bool {
    match &t.first_file {
        None => base.join(&t.name).is_file(),
        Some(first) if subfolder => base.join(&t.name).join(first).exists(),
        Some(first) => base.join(first).exists(),
    }
}

/// What a client's resume data says about a torrent (no file system access: the folders are
/// checked by `locate`).
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Folders the data may be in, most likely first.
    pub candidates: Vec<PathBuf>,
    pub subfolder: bool,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub paused: bool,
}

pub(crate) fn qbt_plan(t: &Parsed, r: Option<&Dict>) -> Plan {
    // qBittorrent keeps unfinished torrents in its "incomplete" folder when one is set.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for key in ["qBt-savePath", "save_path", "qBt-downloadPath"] {
        if let Some(p) = r.and_then(|d| get_str(d, key)).map(PathBuf::from)
            && !candidates.contains(&p)
        {
            candidates.push(p);
        }
    }
    // Content layout: Original/Subfolder keep the torrent's folder, NoSubfolder drops it;
    // "Subfolder" wraps a single file in a folder named after it (without extension).
    let layout = r.and_then(|d| get_str(d, "qBt-contentLayout")).unwrap_or_default();
    let mut subfolder = layout != "NoSubfolder";
    if layout == "Subfolder" && t.first_file.is_none() {
        let stem = Path::new(&t.name).file_stem().map(|x| x.to_os_string()).unwrap_or_default();
        candidates = candidates.into_iter().map(|c| c.join(&stem)).collect();
        subfolder = false;
    }
    Plan {
        candidates,
        subfolder,
        category: r.and_then(|d| get_str(d, "qBt-category")),
        tags: r.map(|d| get_str_list(d, "qBt-tags")).unwrap_or_default(),
        paused: r.is_some_and(|d| get_int(d, "paused") == Some(1) || get_int(d, "stopped") == Some(1)),
    }
}

pub(crate) fn transmission_plan(r: Option<&Dict>) -> Plan {
    // Unfinished torrents may sit in the "incomplete" folder.
    let candidates = ["destination", "incomplete-dir"]
        .iter()
        .filter_map(|k| r.and_then(|d| get_str(d, k)))
        .map(PathBuf::from)
        .collect();
    Plan { candidates, subfolder: true, paused: r.is_some_and(|d| get_int(d, "paused") == Some(1)), ..Plan::default() }
}

/// The first candidate folder that holds the data, else the first candidate.
fn locate(t: &Parsed, candidates: &[PathBuf], subfolder: bool) -> (Option<PathBuf>, bool) {
    match candidates.iter().find(|c| data_at(t, c, subfolder)) {
        Some(c) => (Some(c.clone()), true),
        None => (candidates.first().cloned(), false),
    }
}

fn torrent_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent")))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Scan a client's folder. Blocking (file I/O); call off the UI thread.
pub fn scan(client: Client, dir: &Path) -> Result<Vec<ImportItem>, String> {
    if !dir.is_dir() {
        return Err(t!("{path} doesn't exist", path = dir.display()));
    }
    let mut out = Vec::new();
    match client {
        Client::QBittorrent => {
            for f in torrent_files(dir) {
                let Ok(bytes) = crate::util::read_capped(&f, crate::util::MAX_TORRENT_BYTES) else { continue };
                let Some(t) = parse_torrent(bytes) else { continue };
                let resume = crate::util::read_capped(&f.with_extension("fastresume"), MAX_RESUME_BYTES)
                    .ok()
                    .and_then(|b| dict(&b));
                let plan = qbt_plan(&t, resume.as_ref());
                let (save_path, data_found) = locate(&t, &plan.candidates, plan.subfolder);
                out.push(ImportItem {
                    name: t.name,
                    hash: t.hash,
                    bytes: t.bytes,
                    save_path,
                    category: plan.category,
                    tags: plan.tags,
                    paused: plan.paused,
                    create_subfolder: plan.subfolder,
                    data_found,
                });
            }
        }
        Client::Transmission => {
            let tdir = ["torrents", "Torrents"].iter().map(|n| dir.join(n)).find(|p| p.is_dir());
            let rdir = ["resume", "Resume"].iter().map(|n| dir.join(n)).find(|p| p.is_dir());
            let Some(tdir) = tdir else { return Err(t!("no “torrents” folder found").into()) };
            for f in torrent_files(&tdir) {
                let Ok(bytes) = crate::util::read_capped(&f, crate::util::MAX_TORRENT_BYTES) else { continue };
                let Some(t) = parse_torrent(bytes) else { continue };
                let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let resume = rdir.as_ref().and_then(|r| {
                    [format!("{stem}.resume"), format!("{}.resume", t.hash.to_hex())]
                        .iter()
                        .find_map(|n| crate::util::read_capped(&r.join(n), MAX_RESUME_BYTES).ok())
                        .and_then(|b| dict(&b))
                });
                let plan = transmission_plan(resume.as_ref());
                // Files Transmission is still downloading are named "*.part": not found → re-download.
                let (save_path, data_found) = locate(&t, &plan.candidates, plan.subfolder);
                out.push(ImportItem {
                    name: t.name,
                    hash: t.hash,
                    bytes: t.bytes,
                    save_path,
                    category: plan.category,
                    tags: plan.tags,
                    paused: plan.paused,
                    create_subfolder: true,
                    data_found,
                });
            }
        }
        Client::Deluge => {
            // Deluge keeps download folders in a Python pickle Corrent doesn't read: the data is
            // looked for in Corrent's default folder.
            for f in torrent_files(dir) {
                let Ok(bytes) = crate::util::read_capped(&f, crate::util::MAX_TORRENT_BYTES) else { continue };
                let Some(t) = parse_torrent(bytes) else { continue };
                out.push(ImportItem {
                    name: t.name,
                    hash: t.hash,
                    bytes: t.bytes,
                    save_path: None,
                    category: None,
                    tags: Vec::new(),
                    paused: false,
                    create_subfolder: true,
                    data_found: false,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bencode(v: &BencodeValue<ByteBufOwned>) -> Vec<u8> {
        let mut out = Vec::new();
        librqbit_bencode::bencode_serialize_to_writer(v, &mut out).unwrap();
        out
    }

    fn s(v: &str) -> BencodeValue<ByteBufOwned> {
        BencodeValue::Bytes(ByteBufOwned::from(v.as_bytes()))
    }

    fn d(items: Vec<(&str, BencodeValue<ByteBufOwned>)>) -> BencodeValue<ByteBufOwned> {
        BencodeValue::Dict(items.into_iter().map(|(k, v)| (ByteBufOwned::from(k.as_bytes()), v)).collect())
    }

    /// A tiny valid single-file torrent.
    fn torrent(name: &str) -> Vec<u8> {
        let info = d(vec![
            ("length", BencodeValue::Integer(5)),
            ("name", s(name)),
            ("piece length", BencodeValue::Integer(16384)),
            ("pieces", BencodeValue::Bytes(ByteBufOwned::from(&[7u8; 20][..]))),
        ]);
        bencode(&d(vec![("info", info)]))
    }

    #[test]
    fn imports_qbittorrent_and_transmission_and_deluge() {
        let base = std::env::temp_dir().join(format!("corrent-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let qbt = base.join("BT_backup");
        std::fs::create_dir_all(&qbt).unwrap();
        std::fs::write(qbt.join("aaa.torrent"), torrent("one.iso")).unwrap();
        let resume = d(vec![
            ("qBt-savePath", s("D:/Downloads/Linux")),
            ("qBt-category", s("Linux")),
            ("qBt-tags", BencodeValue::List(vec![s("iso"), s("debian")])),
            ("paused", BencodeValue::Integer(1)),
        ]);
        std::fs::write(qbt.join("aaa.fastresume"), bencode(&resume)).unwrap();
        std::fs::write(qbt.join("broken.torrent"), b"not bencode").unwrap();
        let items = scan(Client::QBittorrent, &qbt).unwrap();
        assert_eq!(items.len(), 1, "broken files are skipped");
        let it = &items[0];
        assert_eq!(it.name, "one.iso");
        assert_eq!(it.save_path.as_deref(), Some(Path::new("D:/Downloads/Linux")));
        assert_eq!(it.category.as_deref(), Some("Linux"));
        assert_eq!(it.tags, vec!["iso".to_string(), "debian".to_string()]);
        assert!(it.paused);
        assert!(!it.data_found, "nothing at D:/Downloads/Linux");

        // Data where the fastresume says (incomplete folder), and the single-file "Subfolder" layout.
        let dl = base.join("incomplete");
        std::fs::create_dir_all(dl.join("one")).unwrap();
        std::fs::write(dl.join("one").join("one.iso"), b"12345").unwrap();
        let resume = d(vec![
            ("qBt-savePath", s(base.join("done").to_str().unwrap())),
            ("qBt-downloadPath", s(dl.to_str().unwrap())),
            ("qBt-contentLayout", s("Subfolder")),
        ]);
        std::fs::write(qbt.join("aaa.fastresume"), bencode(&resume)).unwrap();
        let it = &scan(Client::QBittorrent, &qbt).unwrap()[0];
        assert!(it.data_found, "found in the incomplete folder");
        assert_eq!(it.save_path.as_deref(), Some(dl.join("one").as_path()));
        assert!(!it.create_subfolder);

        let tr = base.join("transmission");
        std::fs::create_dir_all(tr.join("torrents")).unwrap();
        std::fs::create_dir_all(tr.join("resume")).unwrap();
        std::fs::write(tr.join("torrents/two.iso.0123456789abcdef.torrent"), torrent("two.iso")).unwrap();
        std::fs::write(
            tr.join("resume/two.iso.0123456789abcdef.resume"),
            bencode(&d(vec![("destination", s("/srv/dl"))])),
        )
        .unwrap();
        let items = scan(Client::Transmission, &tr).unwrap();
        assert_eq!(items[0].save_path.as_deref(), Some(Path::new("/srv/dl")));
        assert!(!items[0].paused);

        let de = base.join("state");
        std::fs::create_dir_all(&de).unwrap();
        std::fs::write(de.join("x.torrent"), torrent("three.iso")).unwrap();
        let items = scan(Client::Deluge, &de).unwrap();
        assert_eq!(items[0].name, "three.iso");
        assert_eq!(items[0].save_path, None);

        assert!(scan(Client::Deluge, &base.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(base);
    }
}
