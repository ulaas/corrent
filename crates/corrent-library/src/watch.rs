//! Filesystem watching: changes under a library folder schedule a rescan of that library
//! (debounced), so new downloads and deletions show up within seconds.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};

use crate::Library;

/// Keeps the watchers alive; drop it to stop watching.
pub struct Watching {
    _watchers: Vec<notify::RecommendedWatcher>,
    _task: tokio::task::JoinHandle<()>,
}

/// Watches every folder of every library whose settings ask for it. Call again after
/// libraries change (the old `Watching` is dropped).
pub async fn start(lib: &Library, debounce: Duration) -> crate::Result<Watching> {
    let dirty: Arc<Mutex<HashSet<i64>>> = Arc::default();
    let mut watchers = Vec::new();
    for l in lib.list().await? {
        if !l.settings.watch {
            continue;
        }
        for f in &l.folders {
            let (id, dirty) = (l.id, dirty.clone());
            let w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
                if let Ok(ev) = ev
                    && !matches!(ev.kind, notify::EventKind::Access(_))
                    && ev.paths.iter().any(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                {
                    dirty.lock().unwrap_or_else(|e| e.into_inner()).insert(id);
                }
            });
            match w {
                Ok(mut w) => match w.watch(&PathBuf::from(f), RecursiveMode::Recursive) {
                    Ok(()) => watchers.push(w),
                    Err(e) => tracing::warn!("can't watch {f}: {e} (rescans still run on schedule)"),
                },
                Err(e) => tracing::warn!("file watching unavailable: {e}"),
            }
        }
    }
    let lib2 = lib.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(debounce).await;
            let ids: Vec<i64> = dirty.lock().unwrap_or_else(|e| e.into_inner()).drain().collect();
            for id in ids {
                // Changes during a running scan are picked up by scheduling another later.
                if !lib2.start_scan(id) {
                    dirty.lock().unwrap_or_else(|e| e.into_inner()).insert(id);
                }
            }
        }
    });
    Ok(Watching { _watchers: watchers, _task: task })
}

impl Drop for Watching {
    fn drop(&mut self) {
        self._task.abort();
    }
}
