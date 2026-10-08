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
}

type Dict = HashMap<ByteBufOwned, BencodeValue<ByteBufOwned>>;

fn dict(bytes: &[u8]) -> Option<Dict> {
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

fn parse_torrent(bytes: Vec<u8>) -> Option<(InfoHash, String, Vec<u8>)> {
    let t = librqbit::torrent_from_bytes(&bytes).ok()?;
    let hash = InfoHash(t.info_hash.0);
    let name = t.info.data.name.as_ref().map(|n| String::from_utf8_lossy(n.as_ref()).into_owned());
    Some((hash, name.unwrap_or_else(|| hash.to_hex()), bytes))
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
        return Err(format!("{} doesn't exist", dir.display()));
    }
    let mut out = Vec::new();
    match client {
        Client::QBittorrent => {
            for f in torrent_files(dir) {
                let Ok(bytes) = std::fs::read(&f) else { continue };
                let Some((hash, name, bytes)) = parse_torrent(bytes) else { continue };
                let resume = std::fs::read(f.with_extension("fastresume")).ok().and_then(|b| dict(&b));
                let r = resume.as_ref();
                // qBittorrent 4.x keeps its own save path; libtorrent's is the fallback.
                let save_path =
                    r.and_then(|d| get_str(d, "qBt-savePath").or_else(|| get_str(d, "save_path"))).map(PathBuf::from);
                let category = r.and_then(|d| get_str(d, "qBt-category"));
                let tags = r.map(|d| get_str_list(d, "qBt-tags")).unwrap_or_default();
                let paused = r.is_some_and(|d| get_int(d, "paused") == Some(1) || get_int(d, "stopped") == Some(1));
                out.push(ImportItem { name, hash, bytes, save_path, category, tags, paused });
            }
        }
        Client::Transmission => {
            let tdir = ["torrents", "Torrents"].iter().map(|n| dir.join(n)).find(|p| p.is_dir());
            let rdir = ["resume", "Resume"].iter().map(|n| dir.join(n)).find(|p| p.is_dir());
            let Some(tdir) = tdir else { return Err("no torrents folder found".into()) };
            for f in torrent_files(&tdir) {
                let Ok(bytes) = std::fs::read(&f) else { continue };
                let Some((hash, name, bytes)) = parse_torrent(bytes) else { continue };
                let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let resume = rdir.as_ref().and_then(|r| {
                    [format!("{stem}.resume"), format!("{}.resume", hash.to_hex())]
                        .iter()
                        .find_map(|n| std::fs::read(r.join(n)).ok())
                        .and_then(|b| dict(&b))
                });
                let save_path = resume.as_ref().and_then(|d| get_str(d, "destination")).map(PathBuf::from);
                let paused = resume.as_ref().is_some_and(|d| get_int(d, "paused") == Some(1));
                out.push(ImportItem { name, hash, bytes, save_path, category: None, tags: Vec::new(), paused });
            }
        }
        Client::Deluge => {
            for f in torrent_files(dir) {
                let Ok(bytes) = std::fs::read(&f) else { continue };
                let Some((hash, name, bytes)) = parse_torrent(bytes) else { continue };
                out.push(ImportItem {
                    name,
                    hash,
                    bytes,
                    save_path: None,
                    category: None,
                    tags: Vec::new(),
                    paused: false,
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
