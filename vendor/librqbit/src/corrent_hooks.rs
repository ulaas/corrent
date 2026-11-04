//! CORRENT PATCH (D8, test-only): named points in the persistence writes, so Corrent's
//! durability tests can end the process exactly there. Nothing happens unless a hook is set,
//! which only Corrent's test builds do (`corrent_core::faults`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static KILL: OnceLock<fn(&str, &str)> = OnceLock::new();

/// Called with (what is written, how far) at each point. Set once per process.
pub fn set_kill_hook(f: fn(&str, &str)) {
    let _ = KILL.set(f);
}

pub(crate) fn kill_point(op: &str, phase: &str) {
    if let Some(f) = KILL.get() {
        f(op, phase);
    }
}

static SESSION_WRITES: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);

pub(crate) fn count_session_write(path: &Path) {
    let mut writes = SESSION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    *writes.get_or_insert_default().entry(path.to_path_buf()).or_default() += 1;
}

/// How often this `session.json` has been written in this process (for Corrent's tests).
pub fn session_writes(path: &Path) -> u64 {
    let writes = SESSION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    writes.as_ref().and_then(|w| w.get(path)).copied().unwrap_or(0)
}
