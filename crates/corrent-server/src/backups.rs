//! Server backups: a consistent copy of `corrent.db` (`VACUUM INTO`) plus the configuration,
//! certificates and keys, zipped into `<data>/backups/` (the newest seven are kept).
//!
//! Restore: from the web client (Settings → Tasks and backups → Restore, or upload a backup),
//! which stages it; the next start applies it before opening the database, keeping the files it
//! replaces in `<data>/before-restore-<stamp>/` (put back if the restore fails). By hand: stop
//! the server, unzip a backup into its data folder, start it again. The zip holds private keys: on Unix it is readable by the owner only; on
//! Windows the data folder's ACL applies (`correntd service install` limits it to SYSTEM,
//! Administrators and the service account).

use std::io::Write;
use std::path::{Path, PathBuf};

use corrent_proto::tasks::BackupFile;

use crate::state::AppState;

const KEEP: usize = 7;

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backups")
}

fn stamp(ts: i64) -> String {
    let t = ts + corrent_core::util::local_offset_secs();
    let (y, mo, d) = corrent_core::util::civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{y:04}{mo:02}{d:02}-{:02}{:02}{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Backups, newest first.
pub fn list(data_dir: &Path) -> Vec<BackupFile> {
    let mut out: Vec<BackupFile> = std::fs::read_dir(dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !(name.starts_with("corrent-server-") && name.ends_with(".zip")) {
                return None;
            }
            let m = e.metadata().ok()?;
            let created = m
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default();
            Some(BackupFile { name, size: m.len(), created })
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// A backup's path, when `name` is one of ours (no path tricks).
pub fn path_of(data_dir: &Path, name: &str) -> Option<PathBuf> {
    list(data_dir).into_iter().find(|b| b.name == name).map(|b| dir(data_dir).join(b.name))
}

fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

pub async fn create(st: &AppState) -> Result<BackupFile, String> {
    let data = st.paths.data_dir.clone();
    let out_dir = dir(&data);
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&out_dir, std::fs::Permissions::from_mode(0o700));
    }
    let name = format!("corrent-server-{}.zip", stamp(corrent_core::util::now_unix()));
    let db_copy = out_dir.join(format!(".{name}.db"));
    let _ = std::fs::remove_file(&db_copy);
    let target = db_copy.display().to_string();
    st.identity
        .db()
        .read(move |c| c.execute("VACUUM INTO ?1", [target]).map(drop))
        .await
        .map_err(|e| format!("database copy: {e}"))?;
    let (out, name2) = (out_dir.clone(), name.clone());
    let result = tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
        let partial = out.join(format!(".{name2}.partial"));
        let _ = std::fs::remove_file(&partial);
        let mut z = zip::ZipWriter::new(private_file(&partial)?);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600);
        let mut add = |zip_name: &str, path: &Path| -> std::io::Result<()> {
            if let Ok(bytes) = std::fs::read(path) {
                z.start_file(zip_name, opts).map_err(std::io::Error::other)?;
                z.write_all(&bytes)?;
            }
            Ok(())
        };
        add("corrent.db", &out.join(format!(".{name2}.db")))?;
        for f in ["server.toml", "settings.toml", "store.json", "rss.json", "connect.key"] {
            add(f, &data.join(f))?;
        }
        for e in std::fs::read_dir(data.join("tls")).into_iter().flatten().flatten() {
            if e.path().is_file() {
                add(&format!("tls/{}", e.file_name().to_string_lossy()), &e.path())?;
            }
        }
        z.finish().map_err(std::io::Error::other)?;
        let final_path = out.join(&name2);
        std::fs::rename(&partial, &final_path)?;
        Ok(std::fs::metadata(&final_path)?.len())
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&db_copy);
    let size = result.map_err(|e| e.to_string())?;
    for old in list(&st.paths.data_dir).into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(out_dir.join(old.name));
    }
    Ok(BackupFile { name, size, created: corrent_core::util::now_unix() })
}

/// Files a backup may contain (anything else in a zip is refused).
const FILES: [&str; 6] = ["corrent.db", "server.toml", "settings.toml", "store.json", "rss.json", "connect.key"];

fn pending_marker(data_dir: &Path) -> PathBuf {
    data_dir.join("restore-pending")
}

/// Checks that a zip is a server backup: a database and only known files.
pub fn validate(path: &Path) -> Result<(), String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|_| "not a zip file".to_string())?;
    let mut has_db = false;
    for i in 0..z.len() {
        let e = z.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().map_err(|e| e.to_string())?.to_string();
        let ok = FILES.contains(&name.as_str())
            || name
                .strip_prefix("tls/")
                .is_some_and(|f| !f.is_empty() && !f.contains(['/', '\\']) && !f.starts_with('.'));
        if !ok {
            return Err(format!("unexpected file in the backup: {name}"));
        }
        has_db |= name == "corrent.db";
    }
    if !has_db {
        return Err("the backup has no database".into());
    }
    Ok(())
}

/// Where an upload is written while it arrives: (final name, the private partial file).
pub fn start_upload(data_dir: &Path) -> Result<(String, PathBuf, std::fs::File), String> {
    let out = dir(data_dir);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    // Unique even for two uploads in the same second.
    let name = format!(
        "corrent-server-{}-uploaded-{}.zip",
        stamp(corrent_core::util::now_unix()),
        corrent_core::util::random_hex(3)
    );
    let partial = out.join(format!(".{name}.partial"));
    let f = private_file(&partial).map_err(|e| e.to_string())?;
    Ok((name, partial, f))
}

/// Validates a finished upload and puts it next to the other backups.
pub fn finish_upload(data_dir: &Path, name: String, partial: &Path) -> Result<BackupFile, String> {
    if let Err(e) = validate(partial) {
        let _ = std::fs::remove_file(partial);
        return Err(e);
    }
    let size = std::fs::metadata(partial).map(|m| m.len()).unwrap_or(0);
    std::fs::rename(partial, dir(data_dir).join(&name)).map_err(|e| e.to_string())?;
    Ok(BackupFile { name, size, created: corrent_core::util::now_unix() })
}

/// Saves an uploaded backup next to the others (validated; tests).
#[cfg(test)]
pub fn store_upload(data_dir: &Path, bytes: &[u8]) -> Result<BackupFile, String> {
    let (name, partial, mut f) = start_upload(data_dir)?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    drop(f);
    finish_upload(data_dir, name, &partial)
}

/// Stages a restore for the next start.
pub fn stage_restore(data_dir: &Path, name: &str) -> Result<(), String> {
    let path = path_of(data_dir, name).ok_or("no such backup")?;
    validate(&path)?;
    corrent_core::util::write_atomic(&pending_marker(data_dir), name.as_bytes()).map_err(|e| e.to_string())
}

/// The staged restore, if any.
pub fn pending(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(pending_marker(data_dir)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn cancel_restore(data_dir: &Path) {
    let _ = std::fs::remove_file(pending_marker(data_dir));
}

/// Applies a staged restore (at startup, before the database is opened). Returns what happened
/// for the log; the marker is removed either way so a broken backup can't block every start.
pub fn apply_pending(data_dir: &Path) -> Option<Result<String, String>> {
    let name = pending(data_dir)?;
    cancel_restore(data_dir);
    let Some(zip_path) = path_of(data_dir, &name) else { return Some(Err(format!("backup {name} is gone"))) };
    if let Err(e) = validate(&zip_path) {
        return Some(Err(e));
    }
    let aside = data_dir.join(format!("before-restore-{}", stamp(corrent_core::util::now_unix())));
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut created: Vec<PathBuf> = Vec::new();
    let r = (|| -> Result<(), String> {
        std::fs::create_dir_all(aside.join("tls")).map_err(|e| e.to_string())?;
        let mut names: Vec<String> = FILES.iter().map(|s| s.to_string()).collect();
        names.extend(["corrent.db-wal".into(), "corrent.db-shm".into()]);
        for e in std::fs::read_dir(data_dir.join("tls")).into_iter().flatten().flatten() {
            names.push(format!("tls/{}", e.file_name().to_string_lossy()));
        }
        for n in names {
            let from = data_dir.join(&n);
            if from.is_file() {
                let to = aside.join(&n);
                std::fs::rename(&from, &to).map_err(|e| format!("{n}: {e}"))?;
                moved.push((from, to));
            }
        }
        let f = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(data_dir.join("tls")).map_err(|e| e.to_string())?;
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| e.to_string())?;
            let target = data_dir.join(e.name().map_err(|e| e.to_string())?.as_ref());
            created.push(target.clone());
            let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
            std::io::copy(&mut e, &mut out).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    Some(match r {
        Ok(()) => Ok(format!("restored {name}; the previous files are in {}", aside.display())),
        Err(e) => {
            // Remove what the backup wrote, put back what was moved.
            for f in &created {
                let _ = std::fs::remove_file(f);
            }
            for (from, to) in moved.iter().rev() {
                let _ = std::fs::rename(to, from);
            }
            // The aside folder is empty now (remove_dir refuses otherwise).
            let _ = std::fs::remove_dir(aside.join("tls"));
            let _ = std::fs::remove_dir(&aside);
            Err(format!("restoring {name} failed ({e}); nothing was changed"))
        }
    })
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, b) in files {
            z.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn stages_applies_and_refuses_strangers() {
        let d = std::env::temp_dir().join(format!("corrent-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(dir(&d)).unwrap();
        std::fs::write(d.join("corrent.db"), b"current").unwrap();
        std::fs::write(d.join("server.toml"), b"name = 'now'").unwrap();
        let good = dir(&d).join("corrent-server-20260101-000000.zip");
        zip_with(&good, &[("corrent.db", b"old db"), ("server.toml", b"name = 'then'"), ("tls/ca.pem", b"ca")]);
        let bad = dir(&d).join("corrent-server-20260102-000000.zip");
        zip_with(&bad, &[("corrent.db", b"x"), ("../../evil", b"x")]);
        assert!(stage_restore(&d, "corrent-server-20260102-000000.zip").unwrap_err().contains("unexpected"));
        assert!(stage_restore(&d, "../corrent.db").is_err());
        assert!(store_upload(&d, b"not a zip").is_err());

        stage_restore(&d, "corrent-server-20260101-000000.zip").unwrap();
        assert!(pending(&d).is_some());
        let r = apply_pending(&d).unwrap().unwrap();
        assert!(r.contains("restored"), "{r}");
        assert_eq!(std::fs::read(d.join("corrent.db")).unwrap(), b"old db");
        assert_eq!(std::fs::read(d.join("tls/ca.pem")).unwrap(), b"ca");
        assert!(pending(&d).is_none());
        let aside = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("before-restore-"))
            .unwrap();
        assert_eq!(std::fs::read(aside.path().join("corrent.db")).unwrap(), b"current");
        assert!(apply_pending(&d).is_none());
        let _ = std::fs::remove_dir_all(d);
    }
}
