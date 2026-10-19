//! Fault injection for tests (STABILITY.md workstream I): slow disks and similar, scoped to one
//! test's data folder because the tests of a crate run in one process. Compiled out of normal
//! builds; the `testkit` feature (only enabled by dev-dependencies) keeps it for other crates'
//! tests.

use std::path::Path;
use std::time::Duration;

#[cfg(any(test, feature = "testkit"))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    static FS_DELAY: Mutex<Vec<(PathBuf, Duration)>> = Mutex::new(Vec::new());

    pub fn set_fs_delay(prefix: &Path, delay: Duration) {
        let mut v = FS_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|(p, _)| p != prefix);
        v.push((prefix.to_path_buf(), delay));
    }

    pub fn clear(prefix: &Path) {
        FS_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(p, _)| !p.starts_with(prefix));
    }

    pub fn fs_delay(path: &Path) -> Option<Duration> {
        let v = FS_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.iter().find(|(p, _)| path.starts_with(p)).map(|(_, d)| *d)
    }
}

#[cfg(any(test, feature = "testkit"))]
pub use imp::{clear, set_fs_delay};

/// A test's slow disk: file system work on `path` waits this long first.
pub fn fs_delay(path: &Path) {
    #[cfg(any(test, feature = "testkit"))]
    if let Some(d) = imp::fs_delay(path) {
        std::thread::sleep(d);
    }
    #[cfg(not(any(test, feature = "testkit")))]
    let _ = path;
}

/// Unused outside tests; keeps `Duration` imported in every configuration.
#[allow(dead_code)]
const _UNIT: Duration = Duration::ZERO;
