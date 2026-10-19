//! One process per data folder. Two engines on one profile would both write `session.json`,
//! `store.json` and `rss.json`, and both would apply a staged restore. The desktop app and
//! `correntd` take the same lock, before anything in the folder is read or restored.

use std::path::Path;

/// Held for the life of the process; the OS releases it when the process ends, also on a crash.
#[derive(Debug)]
pub struct DataLock {
    _file: std::fs::File,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process uses this data folder.
    Held,
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held => f.write_str("another Corrent process is using this data folder"),
            LockError::Io(e) => write!(f, "could not lock the data folder: {e}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Lock `data_dir/corrent.lock` (created if needed) without waiting.
pub fn lock_data_dir(data_dir: &Path) -> Result<DataLock, LockError> {
    std::fs::create_dir_all(data_dir).map_err(LockError::Io)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("corrent.lock"))
        .map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(DataLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(LockError::Held),
        Err(std::fs::TryLockError::Error(e)) => Err(LockError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn second_lock_on_one_folder_is_refused() {
        let dir = scratch("two");
        let first = lock_data_dir(&dir).unwrap();
        assert!(matches!(lock_data_dir(&dir), Err(LockError::Held)));
        assert!(lock_data_dir(&dir.join("other")).is_ok(), "other folders are independent");
        drop(first);
        assert!(lock_data_dir(&dir).is_ok(), "released with its holder");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_staged_restore_is_applied_by_exactly_one_of_two_starts() {
        let base = scratch("race");
        let paths = Paths { data_dir: base.join("data"), portable: true };
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::write(paths.settings_file(), "[speed]\ndownload_limit = 5\n").unwrap();
        let zip = base.join("b.zip");
        crate::backup::export(&paths, &zip).unwrap();
        crate::backup::stage_import(&paths, &zip).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let starts: Vec<_> = (0..2)
            .map(|_| {
                let (paths, barrier) = (paths.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    // Like main(): lock first; a start that can't lock touches nothing.
                    let lock = lock_data_dir(&paths.data_dir).ok()?;
                    let applied = crate::backup::apply_pending(&paths);
                    // Hold the lock until both have tried.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    drop(lock);
                    applied
                })
            })
            .collect();
        let applied: Vec<_> = starts.into_iter().filter_map(|t| t.join().unwrap()).collect();
        assert_eq!(applied.len(), 1, "{applied:?}");
        assert!(applied[0].starts_with("Restored"));
        let _ = std::fs::remove_dir_all(base);
    }
}
