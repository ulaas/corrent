//! Server backups: a consistent copy of `corrent.db` (`VACUUM INTO`) plus the configuration,
//! certificates and keys, zipped into `<data>/backups/` (the newest seven are kept).
//!
//! Restore: stop the server, unzip a backup into an empty data folder (or over the old one),
//! start it again. The zip holds private keys: on Unix it is readable by the owner only; on
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
