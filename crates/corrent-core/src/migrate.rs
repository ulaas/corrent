//! Schema versions and migrations for `settings.toml` and `store.json`.
//!
//! Each file carries `schema_version` (missing = 0, i.e. written before versioning). Older files are
//! migrated step by step after a dated backup copy is taken; files written by a *newer* Corrent are
//! loaded best-effort but never overwritten, so downgrading can't destroy data.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{Settings, default_download_dir, os_download_dir};
use crate::store::StoreData;

pub const SETTINGS_VERSION: u32 = 1;
pub const STORE_VERSION: u32 = 2;
/// Names the profile fixtures (`tests/fixtures/profile-v<n>`): bumped whenever any file format
/// changes (settings, store, feed history), so each release that changes one adds a fixture.
pub const PROFILE_VERSION: u32 = 2;

/// Set when settings.toml comes from a newer version: saving is disabled.
pub static SETTINGS_READ_ONLY: AtomicBool = AtomicBool::new(false);

pub fn settings_read_only() -> bool {
    SETTINGS_READ_ONLY.load(Ordering::Relaxed)
}

/// Keep a copy of `file` before migrating it from `from`.
pub fn backup_before_migration(file: &Path, from: u32) -> Option<PathBuf> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    // One backup per source version is enough: if saving the migrated file keeps failing, don't
    // pile up a copy on every start.
    let prefix = format!("{}.{ext}.v{from}-", file.file_stem()?.to_string_lossy());
    if let Some(dir) = file.parent()
        && let Ok(rd) = std::fs::read_dir(dir)
        && let Some(existing) = rd.flatten().find(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            // Not a temp file a crash left: that one may be cut off.
            n.starts_with(&prefix) && !n.ends_with(".tmp")
        })
    {
        return Some(existing.path());
    }
    let copy = file.with_extension(format!("{ext}.v{from}-{}", crate::util::now_unix()));
    // Through a temp file: a copy cut off by a crash would count as the backup on the next start.
    let bytes = std::fs::read(file).ok()?;
    crate::util::write_replace(&copy, &bytes).ok().map(|()| copy)
}

/// Bring settings from schema `from` to the current version. Returns true if anything changed.
pub fn migrate_settings(s: &mut Settings, from: u32) -> bool {
    let mut changed = false;
    if from < 1 {
        // v0 → v1: the default save folder moved from Downloads to Downloads/Corrent.
        if s.downloads.save_path == os_download_dir() {
            s.downloads.save_path = default_download_dir();
        }
        changed = true;
    }
    s.schema_version = SETTINGS_VERSION;
    changed
}

/// Bring the store from schema `from` to the current version.
pub fn migrate_store(d: &mut StoreData, from: u32) -> bool {
    let mut changed = false;
    if from < 1 {
        // v0 → v1: no shape change; queue positions are normalized by the store after loading.
        changed = true;
    }
    if from < 2 {
        // v1 → v2: where the engine keeps the data (output folder, file selection, paused); new
        // optional fields, filled in by the engine on its first ticks.
        changed = true;
    }
    d.schema_version = STORE_VERSION;
    changed
}

/// Schema version recorded in a TOML document (0 when absent or unreadable).
pub fn toml_version(text: &str) -> u32 {
    text.parse::<toml::Table>()
        .ok()
        .and_then(|t| t.get("schema_version").and_then(|v| v.as_integer()))
        .map(|v| v.clamp(0, u32::MAX as i64) as u32)
        .unwrap_or(0)
}

/// Schema version recorded in a JSON document (0 when absent).
pub fn json_version(bytes: &[u8]) -> u32 {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|v| v.get("schema_version").and_then(|n| n.as_u64()))
        .map(|v| v.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures: files as written by earlier releases.
    const SETTINGS_V0: &str = r#"
[behavior]
skip_add_dialog = true

[speed]
download_limit = 512
"#;

    const STORE_V0: &str = r#"{"torrents":{"0101010101010101010101010101010101010101":{"name":"x","category":"Movies","queue_pos":3}},"categories":{"Movies":{"save_path":null}},"tags":["a"],"stats":{"all_time_uploaded":5}}"#;

    #[test]
    fn v0_settings_load_losslessly_and_migrate() {
        assert_eq!(toml_version(SETTINGS_V0), 0);
        let mut s: Settings = toml::from_str(SETTINGS_V0).unwrap();
        assert!(migrate_settings(&mut s, 0));
        assert_eq!(s.schema_version, SETTINGS_VERSION);
        assert!(s.behavior.skip_add_dialog);
        assert_eq!(s.speed.download_limit, 512);
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml_version(&text), SETTINGS_VERSION);
    }

    #[test]
    fn v0_downloads_folder_moves_to_corrent_subfolder() {
        let mut s = Settings::default();
        s.downloads.save_path = os_download_dir();
        migrate_settings(&mut s, 0);
        assert_eq!(s.downloads.save_path, default_download_dir());
        // A folder the user chose is left alone.
        let mut s = Settings::default();
        s.downloads.save_path = "D:/Media".into();
        migrate_settings(&mut s, 0);
        assert_eq!(s.downloads.save_path, PathBuf::from("D:/Media"));
    }

    #[test]
    fn v0_store_loads_losslessly() {
        assert_eq!(json_version(STORE_V0.as_bytes()), 0);
        let mut d: StoreData = serde_json::from_str(STORE_V0).unwrap();
        assert!(migrate_store(&mut d, 0));
        assert_eq!(d.schema_version, STORE_VERSION);
        assert_eq!(d.torrents.len(), 1);
        assert_eq!(d.tags, vec!["a".to_string()]);
        assert_eq!(d.stats.all_time_uploaded, 5);
        assert_eq!(json_version(&serde_json::to_vec(&d).unwrap()), STORE_VERSION);
    }

    #[test]
    fn v1_store_migrates_to_v2_without_losing_fields() {
        let v1 = r#"{"schema_version":1,"torrents":{"0101010101010101010101010101010101010101":{"name":"x","category":"Movies","tags":["t"],"queue_pos":2,"pause_reason":"User","downloaded_total":7,"file_priority":{"1":1}}}}"#;
        assert_eq!(json_version(v1.as_bytes()), 1);
        let mut d: StoreData = serde_json::from_str(v1).unwrap();
        assert!(migrate_store(&mut d, 1));
        assert_eq!(d.schema_version, 2);
        let m = d.torrents.values().next().unwrap();
        assert_eq!((m.category.as_deref(), m.queue_pos, m.downloaded_total), (Some("Movies"), 2, 7));
        assert_eq!(m.pause_reason, Some(crate::store::PauseReason::User));
        assert_eq!(m.file_priority.get(&1), Some(&1));
        // Not known yet: the engine fills them in.
        assert_eq!((m.output_folder.as_ref(), m.only_files.as_ref(), m.paused), (None, None, false));
        let out = serde_json::to_vec(&d).unwrap();
        assert_eq!(json_version(&out), 2);
        let back: StoreData = serde_json::from_slice(&out).unwrap();
        assert_eq!(back.torrents.values().next().unwrap().tags, vec!["t".to_string()]);
    }

    #[test]
    fn profile_version_covers_every_file_format() {
        let newest = SETTINGS_VERSION.max(STORE_VERSION).max(crate::rss::STATE_VERSION);
        assert!(PROFILE_VERSION >= newest, "bump PROFILE_VERSION and add a fixture");
    }

    #[test]
    fn current_version_is_not_migrated() {
        let mut s = Settings { schema_version: SETTINGS_VERSION, ..Default::default() };
        assert!(!migrate_settings(&mut s, SETTINGS_VERSION));
    }
}
