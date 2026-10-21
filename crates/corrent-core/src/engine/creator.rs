//! Create .torrent files from a file or folder (optionally private, with a comment), and
//! optionally start seeding them.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{ByteBufOwned, CreateTorrentOptions};

use super::types::InfoHash;

#[derive(Clone, Debug)]
pub struct CreateRequest {
    pub id: u64,
    pub source: PathBuf,
    pub name: Option<String>,
    pub trackers: Vec<String>,
    /// Bytes per piece; `None` picks automatically.
    pub piece_length: Option<u32>,
    pub private: bool,
    pub comment: Option<String>,
    pub save_to: PathBuf,
    pub seed: bool,
}

pub struct Created {
    pub bytes: Vec<u8>,
    pub hash: InfoHash,
    pub name: String,
    pub magnet: String,
}

/// Hash the source and write the .torrent file. Runs the hashing on blocking threads.
pub async fn create(req: &CreateRequest) -> anyhow::Result<Created> {
    anyhow::ensure!(req.source.exists(), "{} does not exist", req.source.display());
    // A single-file torrent's file is named after the torrent, so a custom name would make the
    // data unfindable when seeding; only folders can be renamed.
    let name = if req.source.is_file() { None } else { req.name.as_deref() };
    let spawner = BlockingSpawner::new(4);
    let res = librqbit::create_torrent(
        &req.source,
        CreateTorrentOptions { name, trackers: req.trackers.clone(), piece_length: req.piece_length },
        &spawner,
    )
    .await
    .context("hashing failed")?;
    let mut meta = res.meta;
    // librqbit can't set these; the info dict is re-encoded from its fields, so editing them
    // here produces a correct (re-hashed) torrent.
    meta.info.data.private = req.private;
    meta.comment = req.comment.as_ref().filter(|c| !c.trim().is_empty()).map(|c| ByteBufOwned::from(c.as_bytes()));
    meta.created_by = Some(ByteBufOwned::from(format!("Corrent {}", env!("CARGO_PKG_VERSION")).as_bytes()));
    meta.creation_date = Some(crate::util::now_unix().max(0) as usize);
    let mut bytes = Vec::new();
    librqbit_bencode::bencode_serialize_to_writer(&meta, &mut bytes).context("encoding the torrent")?;
    // Parse back: computes the info hash over the final info dict and validates the result.
    let parsed = librqbit::torrent_from_bytes(&bytes).context("the created torrent is invalid")?;
    let hash = InfoHash(parsed.info_hash.0);
    let name = parsed
        .info
        .data
        .name
        .as_ref()
        .map(|n| String::from_utf8_lossy(n.as_ref()).into_owned())
        .unwrap_or_else(|| hash.to_hex());
    let mut magnet = format!("magnet:?xt=urn:btih:{}&dn={}", hash.to_hex(), super::types::url_encode(&name));
    for t in &req.trackers {
        magnet.push_str("&tr=");
        magnet.push_str(&super::types::url_encode(t));
    }
    let had_bak = req.save_to.with_extension("torrent.bak").exists();
    crate::util::write_atomic(&req.save_to, &bytes).with_context(|| format!("writing {}", req.save_to.display()))?;
    if !had_bak {
        let _ = std::fs::remove_file(req.save_to.with_extension("torrent.bak"));
    }
    Ok(Created { bytes, hash, name, magnet })
}

/// Where to seed from: the folder that contains the source.
pub fn seed_folder(source: &Path) -> PathBuf {
    source.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// Suggested piece size for a total size: ~1000–2000 pieces, 16 KiB … 16 MiB.
pub fn auto_piece_length(total: u64) -> u32 {
    let mut p: u64 = 16 * 1024;
    while total / p > 2000 && p < 16 * 1024 * 1024 {
        p *= 2;
    }
    p as u32
}

/// Total size of a file or folder (for the piece-size suggestion).
pub fn total_size(path: &Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    let mut sum = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    stack.push(e.path());
                } else if let Ok(m) = e.metadata() {
                    sum += m.len();
                }
            }
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_lengths() {
        assert_eq!(auto_piece_length(1024), 16 * 1024);
        assert_eq!(auto_piece_length(4 * 1024 * 1024 * 1024), 4 * 1024 * 1024);
        assert_eq!(auto_piece_length(u64::MAX), 16 * 1024 * 1024);
    }

    #[tokio::test]
    async fn creates_private_torrent_with_comment() {
        let dir = std::env::temp_dir().join(format!("corrent-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/sub")).unwrap();
        std::fs::write(dir.join("src/a.txt"), vec![7u8; 100_000]).unwrap();
        std::fs::write(dir.join("src/sub/b.bin"), vec![9u8; 50_000]).unwrap();
        let req = CreateRequest {
            id: 1,
            source: dir.join("src"),
            name: Some("My Share".into()),
            trackers: vec!["udp://tracker.example:1337/announce".into()],
            piece_length: Some(32 * 1024),
            private: true,
            comment: Some("hello".into()),
            save_to: dir.join("out.torrent"),
            seed: false,
        };
        let c = create(&req).await.unwrap();
        assert_eq!(c.name, "My Share");
        let bytes = std::fs::read(dir.join("out.torrent")).unwrap();
        assert_eq!(bytes, c.bytes);
        let parsed = librqbit::torrent_from_bytes(&bytes).unwrap();
        assert!(parsed.info.data.private, "private flag kept");
        assert_eq!(parsed.comment.as_ref().map(|c| c.as_ref().to_vec()), Some(b"hello".to_vec()));
        assert_eq!(InfoHash(parsed.info_hash.0), c.hash);
        assert!(c.magnet.contains(&c.hash.to_hex()));
        assert_eq!(total_size(&dir.join("src")), 150_000);
        // A public torrent of the same data has a different info hash.
        let public = create(&CreateRequest { private: false, save_to: dir.join("pub.torrent"), ..req }).await.unwrap();
        assert_ne!(public.hash, c.hash);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// One hash per piece, also when the size is an exact multiple of the piece length (librqbit
    /// wrote an extra one there, which libtorrent-based clients refuse).
    #[tokio::test]
    async fn one_hash_per_piece() {
        let dir = std::env::temp_dir().join(format!("corrent-create-pieces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, size) in [("exact.bin", 4 * 32 * 1024), ("partial.bin", 4 * 32 * 1024 + 1)] {
            std::fs::write(dir.join(name), vec![5u8; size]).unwrap();
            let req = CreateRequest {
                id: 1,
                source: dir.join(name),
                name: None,
                trackers: Vec::new(),
                piece_length: Some(32 * 1024),
                private: false,
                comment: None,
                save_to: dir.join(format!("{name}.torrent")),
                seed: false,
            };
            let c = create(&req).await.unwrap();
            let t = librqbit::torrent_from_bytes(&c.bytes).unwrap();
            assert_eq!(t.info.data.pieces.as_ref().len(), 20 * size.div_ceil(32 * 1024), "{name}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
