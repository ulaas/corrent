//! Health self-checks (STABILITY.md O5): every 5 minutes, off the engine's thread, the save
//! folder must be writable, and the engine combines that with the free space it already
//! watches and the store's last save. Results go into the snapshot; the GUI shows a status-bar
//! chip only when something is wrong (correntd logs changes).

use std::path::Path;

/// Less free space than this in the save folder is a problem (the engine also warns once).
pub const LOW_SPACE: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    /// The save folder can't be created or written (why, explained).
    SaveFolder(String),
    /// Free bytes in the save folder.
    LowSpace(u64),
    /// Corrent's own data (store.json) couldn't be saved (why).
    StoreSave(String),
}

impl Issue {
    pub fn label(&self) -> String {
        match self {
            Issue::SaveFolder(why) => t!("Downloads can't be saved to the download folder: {why}", why = why),
            Issue::LowSpace(free) => {
                t!("Only {free} free in the download folder", free = crate::util::fmt_bytes(*free))
            }
            Issue::StoreSave(why) => t!("Corrent couldn't save its data: {why}", why = why),
        }
    }
}

/// A file is written to and removed from `save_path`. A missing folder is created only when
/// `create` (a fresh profile, as its first download would): otherwise it is a problem, since
/// making it again would put downloads on another disk when a drive is unplugged.
pub fn probe(save_path: &Path, create: bool) -> std::io::Result<()> {
    crate::faults::fs_delay(save_path);
    let file = save_path.join(format!(".corrent-probe-{}", std::process::id()));
    if let Some(e) = crate::faults::file_error(&file) {
        return Err(e);
    }
    if create {
        std::fs::create_dir_all(save_path)?;
    } else if !save_path.is_dir() {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    std::fs::write(&file, b"corrent")?;
    std::fs::remove_file(&file)
}

/// The problems, from the probe's result, the free space and the store's last save error.
pub fn evaluate(probe: &Result<(), String>, free: Option<u64>, store_error: Option<&str>) -> Vec<Issue> {
    let mut v = Vec::new();
    if let Err(why) = probe {
        v.push(Issue::SaveFolder(why.clone()));
    }
    if let Some(free) = free
        && free < LOW_SPACE
    {
        v.push(Issue::LowSpace(free));
    }
    if let Some(why) = store_error {
        v.push(Issue::StoreSave(why.to_string()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_check_flips_on_its_own() {
        let ok = Ok(());
        assert!(evaluate(&ok, Some(LOW_SPACE), None).is_empty());
        assert!(evaluate(&ok, None, None).is_empty(), "unknown free space isn't a problem");
        assert_eq!(evaluate(&Err("denied".into()), Some(LOW_SPACE), None), vec![Issue::SaveFolder("denied".into())]);
        assert_eq!(evaluate(&ok, Some(5), None), vec![Issue::LowSpace(5)]);
        assert_eq!(evaluate(&ok, Some(LOW_SPACE), Some("full")), vec![Issue::StoreSave("full".into())]);
        assert_eq!(evaluate(&Err("x".into()), Some(1), Some("y")).len(), 3);
    }

    #[test]
    fn the_probe_creates_the_folder_only_when_asked_and_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!("corrent-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let save = dir.join("new").join("folder");
        // A folder that went away (an unplugged drive) isn't made again.
        assert_eq!(probe(&save, false).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert!(!save.exists());
        probe(&save, true).unwrap();
        assert!(save.is_dir());
        assert_eq!(std::fs::read_dir(&save).unwrap().count(), 0);
        probe(&save, false).unwrap();
        // Under a file: can't be created.
        std::fs::write(dir.join("file"), "x").unwrap();
        assert!(probe(&dir.join("file").join("sub"), true).is_err());
        crate::faults::set_access_denied(&save, None);
        assert!(probe(&save, false).is_err());
        crate::faults::clear_file_errors(&save);
        probe(&save, false).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
