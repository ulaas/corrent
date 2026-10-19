//! Data backup (zip export) and restore (staged, applied at the next start).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::config::Paths;

const RESTORE_DIR: &str = ".restore";
const RESTORE_TMP: &str = ".restore.tmp";
/// Written last when staging; a staged restore without it is ignored.
const COMPLETE: &str = ".complete";
const COMPONENTS: [&str; 5] = ["settings.toml", "store.json", "rss.json", "session", "torrents"];

/// Files and folders that make up a profile, relative to the data dir.
fn entries(paths: &Paths) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for f in ["settings.toml", "store.json", "rss.json"] {
        let p = paths.data_dir.join(f);
        if p.is_file() {
            out.push(PathBuf::from(f));
        }
    }
    for dir in ["session", "torrents"] {
        let d = paths.data_dir.join(dir);
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                // Skip temp files the engine may be writing right now, and the engine list's
                // good and damaged copies (session_db).
                let is_tmp = e.path().extension().is_some_and(|x| x == "tmp");
                let name = e.file_name().to_string_lossy().into_owned();
                let is_copy = name.starts_with("session.json.good-") || name.starts_with("session.json.bad-");
                if e.path().is_file() && !is_tmp && !is_copy {
                    out.push(PathBuf::from(dir).join(e.file_name()));
                }
            }
        }
    }
    out
}

/// Write a zip with settings, app data and the engine's session to `dest`.
pub fn export(paths: &Paths, dest: &Path) -> anyhow::Result<usize> {
    let tmp = dest.with_extension("zip.tmp");
    let file = std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let list = entries(paths);
    for rel in &list {
        let mut buf = Vec::new();
        std::fs::File::open(paths.data_dir.join(rel))?.read_to_end(&mut buf)?;
        let name = rel.to_string_lossy().replace('\\', "/");
        zip.start_file(name, opts)?;
        zip.write_all(&buf)?;
    }
    zip.start_file("corrent-backup.txt", opts)?;
    writeln!(zip, "Corrent {} backup, {}", env!("CARGO_PKG_VERSION"), crate::util::fmt_date(crate::util::now_unix()))?;
    zip.finish()?;
    std::fs::rename(&tmp, dest)?;
    Ok(list.len())
}

/// Validate and unpack a backup; it is applied at the next start. Returns the components found
/// (e.g. "settings", "torrent list"). Nothing is staged unless the whole archive extracts cleanly.
pub fn stage_import(paths: &Paths, src: &Path) -> anyhow::Result<Vec<&'static str>> {
    let file = std::fs::File::open(src).with_context(|| format!("opening {}", src.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid backup file")?;
    let names: Vec<String> =
        zip.file_names().map(|n| n.map(String::from)).collect::<Result<_, _>>().context("not a valid backup file")?;
    anyhow::ensure!(
        names.iter().any(|n| n == "settings.toml" || n == "store.json"),
        "this zip is not a Corrent backup"
    );
    let tmp = paths.data_dir.join(RESTORE_TMP);
    let _ = std::fs::remove_dir_all(&tmp);
    let result = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(&tmp)?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            // enclosed_name rejects absolute paths and `..` components.
            let Some(rel) = f.enclosed_name() else { continue };
            let comps = rel.components().count();
            let allowed = rel == Path::new("settings.toml")
                || rel == Path::new("store.json")
                || rel == Path::new("rss.json")
                || ((rel.starts_with("session") || rel.starts_with("torrents")) && comps == 2);
            if !allowed || f.is_dir() {
                continue;
            }
            let out = tmp.join(&rel);
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            // Corrent's own files are small; refuse absurd entries (zip bombs).
            const MAX_ENTRY: u64 = 512 * 1024 * 1024;
            if f.size() > MAX_ENTRY {
                anyhow::bail!(t!("the backup contains an unexpectedly large file"));
            }
            let mut buf = Vec::new();
            std::io::Read::by_ref(&mut f).take(MAX_ENTRY).read_to_end(&mut buf)?;
            std::fs::write(out, buf)?;
        }
        std::fs::write(tmp.join(COMPLETE), b"ok")?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let stage = paths.data_dir.join(RESTORE_DIR);
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::rename(&tmp, &stage)?;
    let mut found = Vec::new();
    for (name, label) in [
        ("settings.toml", "settings"),
        ("store.json", "categories, tags and totals"),
        ("session", "torrent list"),
        ("torrents", "metainfo backups"),
    ] {
        if stage.join(name).exists() {
            found.push(label);
        }
    }
    Ok(found)
}

/// Apply a staged restore (call at startup before anything reads the data dir).
///
/// Only components present in the backup are replaced; each replaced component's current data
/// is moved to `pre-restore-<time>` first. Every applied component is removed from the stage,
/// so an interrupted restore resumes where it stopped instead of wiping data.
pub fn apply_pending(paths: &Paths) -> Option<String> {
    let _ = std::fs::remove_dir_all(paths.data_dir.join(RESTORE_TMP));
    let stage = paths.data_dir.join(RESTORE_DIR);
    if !stage.is_dir() {
        return None;
    }
    if !stage.join(COMPLETE).exists() {
        let _ = std::fs::remove_dir_all(&stage);
        return None;
    }
    let keep = paths.data_dir.join(format!("pre-restore-{}", crate::util::now_unix()));
    let mut applied = 0;
    let result = (|| -> std::io::Result<()> {
        for name in COMPONENTS {
            let staged = stage.join(name);
            if !staged.exists() {
                continue;
            }
            let cur = paths.data_dir.join(name);
            if cur.exists() {
                std::fs::create_dir_all(&keep)?;
                std::fs::rename(&cur, keep.join(name))?;
            }
            std::fs::rename(&staged, &cur)?;
            applied += 1;
        }
        std::fs::remove_dir_all(&stage)
    })();
    let kept = if keep.exists() {
        format!(" {}", t!("The previous data was kept in {path}.", path = keep.display()))
    } else {
        String::new()
    };
    Some(match result {
        Ok(()) => t!("Restored {n} parts of your data from a backup.", n = applied) + &kept,
        Err(e) => t!("Restoring from the backup was interrupted ({e}); it will continue next start.", e = e) + &kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_stage_apply_roundtrip() {
        let base = std::env::temp_dir().join(format!("corrent-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths { data_dir: base.join("data"), portable: true };
        std::fs::create_dir_all(paths.session_dir()).unwrap();
        std::fs::write(paths.settings_file(), "[speed]\ndownload_limit = 5\n").unwrap();
        std::fs::write(paths.store_file(), "{}").unwrap();
        std::fs::write(paths.session_dir().join("session.json"), "{\"torrents\":{}}").unwrap();
        let zip = base.join("b.zip");
        assert_eq!(export(&paths, &zip).unwrap(), 3);

        std::fs::write(paths.settings_file(), "changed").unwrap();
        assert_eq!(stage_import(&paths, &zip).unwrap().len(), 3);
        let msg = apply_pending(&paths).unwrap();
        assert!(msg.starts_with("Restored"));
        assert_eq!(std::fs::read_to_string(paths.settings_file()).unwrap(), "[speed]\ndownload_limit = 5\n");
        assert!(apply_pending(&paths).is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn partial_backup_keeps_other_components() {
        let base = std::env::temp_dir().join(format!("corrent-backup-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths { data_dir: base.join("data"), portable: true };
        std::fs::create_dir_all(paths.session_dir()).unwrap();
        std::fs::write(paths.session_dir().join("session.json"), "live").unwrap();
        std::fs::write(paths.store_file(), "{\"live\":1}").unwrap();
        // A backup with settings only.
        let zip_path = base.join("s.zip");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        z.start_file("settings.toml", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"[speed]\n").unwrap();
        z.finish().unwrap();
        assert_eq!(stage_import(&paths, &zip_path).unwrap(), vec!["settings"]);
        apply_pending(&paths).unwrap();
        assert_eq!(std::fs::read_to_string(paths.session_dir().join("session.json")).unwrap(), "live");
        assert_eq!(std::fs::read_to_string(paths.store_file()).unwrap(), "{\"live\":1}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn incomplete_stage_is_ignored() {
        let base = std::env::temp_dir().join(format!("corrent-backup-inc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths { data_dir: base.join("data"), portable: true };
        std::fs::create_dir_all(paths.data_dir.join(RESTORE_DIR)).unwrap();
        std::fs::write(paths.store_file(), "keep").unwrap();
        assert!(apply_pending(&paths).is_none());
        assert_eq!(std::fs::read_to_string(paths.store_file()).unwrap(), "keep");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn rejects_non_backups() {
        let base = std::env::temp_dir().join(format!("corrent-backup-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = Paths { data_dir: base.join("data"), portable: true };
        let zip_path = base.join("x.zip");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        z.start_file("evil.txt", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
        assert!(stage_import(&paths, &zip_path).is_err());
        let _ = std::fs::remove_dir_all(base);
    }
}
