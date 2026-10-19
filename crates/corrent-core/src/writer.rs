//! Saves app data files (`store.json`, `rss.json`) on a background thread, so the engine never
//! waits on a slow or sleeping disk.
//!
//! Jobs are per file and the newest wins: while one save runs, later ones for the same file
//! collapse into one. Contents equal to what is on disk are not written again. `flush` waits for a file's
//! queued save (shutdown, and a store dropped while its task unwinds).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A dated copy to keep before writing (see `util::keep_generations`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    pub tag: &'static str,
    pub keep: usize,
}

struct Job {
    bytes: Vec<u8>,
    generation: Option<Generation>,
}

#[derive(Default)]
struct State {
    pending: HashMap<PathBuf, Job>,
    /// The file being written right now.
    writing: Option<PathBuf>,
    /// Outcome of the last write per file, until someone takes it.
    results: HashMap<PathBuf, Result<(), String>>,
    writes: HashMap<PathBuf, u64>,
    /// The last error logged per file (a failure that repeats is logged once).
    logged: HashMap<PathBuf, String>,
    started: bool,
}

pub struct Writer {
    state: Mutex<State>,
    changed: Condvar,
}

/// The process-wide writer.
pub fn global() -> &'static Writer {
    static W: OnceLock<Writer> = OnceLock::new();
    W.get_or_init(|| Writer { state: Mutex::new(State::default()), changed: Condvar::new() })
}

impl Writer {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Also used from `Drop` during unwinding: a poisoned lock still holds valid data.
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue `bytes` to be written to `path`, replacing a queued older version.
    pub fn submit(&'static self, path: &Path, bytes: Vec<u8>, generation: Option<Generation>) {
        let mut st = self.lock();
        let generation = match st.pending.remove(path) {
            // A generation asked for by a replaced job is still owed.
            Some(old) => generation.or(old.generation),
            None => generation,
        };
        st.pending.insert(path.to_path_buf(), Job { bytes, generation });
        if !st.started {
            st.started = true;
            let spawned = std::thread::Builder::new().name("corrent-writer".into()).spawn(move || self.run());
            if let Err(e) = spawned {
                // Report it like a failed write instead of leaving the job queued forever.
                st.started = false;
                tracing::error!("could not start the writer thread: {e}");
                st.pending.remove(path);
                st.results.insert(path.to_path_buf(), Err(t!("could not start the writer thread: {e}", e = e)));
            }
        }
        self.changed.notify_all();
    }

    /// Wait until `path` has nothing queued or being written (at most `timeout`). False on timeout.
    pub fn flush(&self, path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.lock();
        while st.pending.contains_key(path) || st.writing.as_deref() == Some(path) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self.changed.wait_timeout(st, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
        true
    }

    /// The outcome of the last write of `path` since the previous call.
    pub fn take_result(&self, path: &Path) -> Option<Result<(), String>> {
        self.lock().results.remove(path)
    }

    /// Drop a queued (not yet started) write.
    pub fn cancel(&self, path: &Path) {
        self.lock().pending.remove(path);
        self.changed.notify_all();
    }

    /// Writes that reached the disk for `path` (skipped identical contents don't count).
    pub fn writes(&self, path: &Path) -> u64 {
        self.lock().writes.get(path).copied().unwrap_or(0)
    }

    fn run(&self) {
        loop {
            let (path, job) = {
                let mut st = self.lock();
                loop {
                    if let Some(path) = st.pending.keys().next().cloned() {
                        let job = st.pending.remove(&path).expect("present");
                        st.writing = Some(path.clone());
                        break (path, job);
                    }
                    st = self.changed.wait(st).unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };
            // Compared with the file itself, not with our last write: it may have been replaced
            // since (a restore, another tool).
            let same = std::fs::read(&path).is_ok_and(|on_disk| on_disk == job.bytes);
            let result = if same {
                // Unchanged, but a dated copy that was asked for is still owed.
                if let Some(g) = &job.generation
                    && let Err(e) = crate::util::keep_generations(&path, g.tag, &job.bytes, g.keep)
                {
                    tracing::warn!("could not keep a {} copy of {}: {e:#}", g.tag, path.display());
                }
                Ok(())
            } else {
                std::panic::catch_unwind(|| write(&path, &job)).unwrap_or_else(|_| Err("the save panicked".to_string()))
            };
            let mut st = self.lock();
            match &result {
                Err(e) if st.logged.get(&path) != Some(e) => {
                    tracing::error!("failed to save {}: {e}", path.display());
                    st.logged.insert(path.clone(), e.clone());
                }
                Err(_) => {}
                Ok(()) => {
                    st.logged.remove(&path);
                }
            }
            if result.is_ok() && !same {
                *st.writes.entry(path.clone()).or_default() += 1;
            }
            st.results.insert(path, result);
            st.writing = None;
            self.changed.notify_all();
        }
    }
}

fn write(path: &Path, job: &Job) -> Result<(), String> {
    crate::faults::fs_delay(path);
    if let Some(g) = &job.generation
        && let Err(e) = crate::util::keep_generations(path, g.tag, &job.bytes, g.keep)
    {
        tracing::warn!("could not keep a {} copy of {}: {e:#}", g.tag, path.display());
    }
    crate::util::write_atomic(path, &job.bytes).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-writer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn newest_wins_and_identical_contents_are_skipped() {
        let dir = scratch("coalesce");
        let f = dir.join("store.json");
        let w = global();
        for i in 0..200 {
            w.submit(&f, format!("{i}").into_bytes(), None);
        }
        assert!(w.flush(&f, Duration::from_secs(10)));
        assert_eq!(std::fs::read(&f).unwrap(), b"199");
        let writes = w.writes(&f);
        assert!(writes < 200, "collapsed while writing ({writes})");
        assert_eq!(w.take_result(&f), Some(Ok(())));
        w.submit(&f, b"199".to_vec(), None);
        assert!(w.flush(&f, Duration::from_secs(10)));
        assert_eq!(w.writes(&f), writes, "same contents again: no write");
        // Replaced behind our back: written again even though we wrote these bytes last.
        std::fs::write(&f, b"restored").unwrap();
        w.submit(&f, b"199".to_vec(), None);
        assert!(w.flush(&f, Duration::from_secs(10)));
        assert_eq!(std::fs::read(&f).unwrap(), b"199");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failures_are_reported_and_generations_kept() {
        let dir = scratch("errors");
        std::fs::write(dir.join("blocked"), b"a file where a folder should be").unwrap();
        let bad = dir.join("blocked").join("rss.json");
        let w = global();
        w.submit(&bad, b"x".to_vec(), None);
        assert!(w.flush(&bad, Duration::from_secs(10)));
        assert!(matches!(w.take_result(&bad), Some(Err(_))));
        assert_eq!(w.take_result(&bad), None, "taken once");

        let f = dir.join("store.json");
        w.submit(&f, b"one".to_vec(), Some(Generation { tag: "daily", keep: 3 }));
        w.submit(&f, b"two".to_vec(), None);
        assert!(w.flush(&f, Duration::from_secs(10)));
        let gens = crate::util::generations(&f, "daily");
        assert_eq!(gens.len(), 1, "the replaced job's generation is still taken");
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        // Unchanged contents aren't written again, but an owed generation is still taken.
        let g = dir.join("rss.json");
        std::fs::write(&g, b"same").unwrap();
        w.submit(&g, b"same".to_vec(), Some(Generation { tag: "daily", keep: 3 }));
        assert!(w.flush(&g, Duration::from_secs(10)));
        assert_eq!(w.writes(&g), 0);
        assert_eq!(crate::util::generations(&g, "daily").len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
