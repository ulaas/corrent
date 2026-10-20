//! CORRENT PATCH (D8, test-only): named points in the persistence writes, so Corrent's
//! durability tests can end the process exactly there. Nothing happens unless a hook is set,
//! which only Corrent's test builds do (`corrent_core::faults`).

use std::sync::OnceLock;

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
