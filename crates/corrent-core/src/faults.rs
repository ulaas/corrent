//! Fault injection for tests (STABILITY.md workstream I): slow disks, a stalled engine and
//! similar, scoped to one test's data folder (or torrent) because the tests of a crate run in one
//! process. Compiled out of normal builds; the `testkit` feature (only enabled by
//! dev-dependencies) keeps it for other crates' tests.

use std::path::Path;
use std::time::Duration;

#[cfg(any(test, feature = "testkit"))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    /// Found by `scripts/release-hygiene.ps1` if this module ever reaches a shipped binary.
    pub const MARKER: &str = "CORRENT_FAULTS_COMPILED_IN";

    /// Rules by path prefix.
    pub struct Rules(Mutex<Vec<(PathBuf, Duration)>>);

    impl Rules {
        pub const fn new() -> Rules {
            Rules(Mutex::new(Vec::new()))
        }
        pub fn set(&self, prefix: &Path, d: Duration) {
            tracing::debug!("{MARKER}: {d:?} under {}", prefix.display());
            let mut v = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            v.retain(|(p, _)| p != prefix);
            v.push((prefix.to_path_buf(), d));
        }
        pub fn clear(&self, prefix: &Path) {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(p, _)| !p.starts_with(prefix));
        }
        pub fn get(&self, path: &Path) -> Option<Duration> {
            let v = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            v.iter().find(|(p, _)| path.starts_with(p)).map(|(_, d)| *d)
        }
    }

    pub static FS_DELAY: Rules = Rules::new();
    pub static START_DELAY: Rules = Rules::new();
    pub static START_DEADLINE: Rules = Rules::new();
    pub static HTTP_TIMEOUT: Rules = Rules::new();
    static CALL_DELAY: Mutex<Vec<([u8; 20], Duration)>> = Mutex::new(Vec::new());

    pub fn set_call_delay(hash: [u8; 20], delay: Duration) {
        let mut v = CALL_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|(h, _)| *h != hash);
        v.push((hash, delay));
    }

    pub fn clear_call_delay(hash: [u8; 20]) {
        CALL_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(h, _)| *h != hash);
    }

    pub fn call_delay(hash: [u8; 20]) -> Option<Duration> {
        let v = CALL_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.iter().find(|(h, _)| *h == hash).map(|(_, d)| *d)
    }
}

#[cfg(any(test, feature = "testkit"))]
pub use imp::{clear_call_delay, set_call_delay};

/// Tests: file system work under `prefix` waits `d` first.
#[cfg(any(test, feature = "testkit"))]
pub fn set_fs_delay(prefix: &Path, d: Duration) {
    imp::FS_DELAY.set(prefix, d);
}

/// Tests: starting an engine whose data folder is under `prefix` takes `d` longer.
#[cfg(any(test, feature = "testkit"))]
pub fn set_start_delay(prefix: &Path, d: Duration) {
    imp::START_DELAY.set(prefix, d);
}

/// Tests: an engine whose data folder is under `prefix` must start within `d`.
#[cfg(any(test, feature = "testkit"))]
pub fn set_start_deadline(prefix: &Path, d: Duration) {
    imp::START_DEADLINE.set(prefix, d);
}

/// Tests: the engine HTTP requests of a data folder under `prefix` time out after `d`.
#[cfg(any(test, feature = "testkit"))]
pub fn set_http_timeout(prefix: &Path, d: Duration) {
    imp::HTTP_TIMEOUT.set(prefix, d);
}

/// Tests: drop every rule for paths under `prefix`.
#[cfg(any(test, feature = "testkit"))]
pub fn clear(prefix: &Path) {
    imp::FS_DELAY.clear(prefix);
    imp::START_DELAY.clear(prefix);
    imp::START_DEADLINE.clear(prefix);
    imp::HTTP_TIMEOUT.clear(prefix);
}

/// A test's slow disk: file system work on `path` waits this long first (blocking).
pub fn fs_delay(path: &Path) {
    if let Some(d) = lookup(Kind::Fs, path) {
        std::thread::sleep(d);
    }
}

/// A test's slow engine start for the data folder `path`.
pub fn start_delay(path: &Path) -> Option<Duration> {
    lookup(Kind::StartDelay, path)
}

/// A test's start deadline for the data folder `path`.
pub fn start_deadline(path: &Path) -> Option<Duration> {
    lookup(Kind::StartDeadline, path)
}

/// A test's HTTP timeout for the engine of the data folder `path`.
pub fn http_timeout(path: &Path) -> Option<Duration> {
    lookup(Kind::HttpTimeout, path)
}

/// A test's stalled engine: librqbit calls for the torrent `hash` hang this long first.
pub fn call_delay(hash: [u8; 20]) -> Option<Duration> {
    #[cfg(any(test, feature = "testkit"))]
    {
        imp::call_delay(hash)
    }
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = hash;
        None
    }
}

enum Kind {
    Fs,
    StartDelay,
    StartDeadline,
    HttpTimeout,
}

fn lookup(kind: Kind, path: &Path) -> Option<Duration> {
    #[cfg(any(test, feature = "testkit"))]
    {
        match kind {
            Kind::Fs => imp::FS_DELAY.get(path),
            Kind::StartDelay => imp::START_DELAY.get(path),
            Kind::StartDeadline => imp::START_DEADLINE.get(path),
            Kind::HttpTimeout => imp::HTTP_TIMEOUT.get(path),
        }
    }
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = (kind, path);
        None
    }
}
