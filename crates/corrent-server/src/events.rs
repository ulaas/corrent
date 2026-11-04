//! The event hub: engine wake-ups (new snapshot) and discrete events fan out to every
//! connected client.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use corrent_core::engine::types::{Snapshot, wire};
use corrent_core::engine::{EngineEvent, ToastKind};
use corrent_proto::event::{Event, Severity};
use corrent_proto::torrent::{Labels, Torrent};

/// A snapshot's torrents and labels in wire form, shared by every client.
#[derive(Clone)]
pub struct WireList {
    generation: u64,
    pub torrents: Arc<Vec<Torrent>>,
    pub labels: Arc<Labels>,
}

pub struct Hub {
    snapshot: tokio::sync::watch::Sender<u64>,
    events: tokio::sync::broadcast::Sender<Event>,
    wire: Mutex<Option<WireList>>,
    wire_builds: AtomicU64,
}

impl Default for Hub {
    fn default() -> Self {
        Hub {
            snapshot: tokio::sync::watch::channel(0).0,
            events: tokio::sync::broadcast::channel(1024).0,
            wire: Mutex::new(None),
            wire_builds: AtomicU64::new(0),
        }
    }
}

impl Hub {
    /// The engine published a new snapshot (called from its `Wake`; never blocks).
    pub fn wake(&self) {
        self.snapshot.send_modify(|g| *g = g.wrapping_add(1));
    }

    pub fn snapshots(&self) -> tokio::sync::watch::Receiver<u64> {
        self.snapshot.subscribe()
    }

    pub fn publish(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// A snapshot in wire form: the first client to see a generation converts it, the others
    /// share the result.
    pub fn wire_list(&self, snap: &Snapshot) -> WireList {
        let mut cached = self.wire.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = cached.as_ref().filter(|w| w.generation == snap.generation) {
            return w.clone();
        }
        self.wire_builds.fetch_add(1, Ordering::Relaxed);
        let w = WireList {
            generation: snap.generation,
            torrents: Arc::new(wire::torrents(snap)),
            labels: Arc::new(wire::labels(snap)),
        };
        *cached = Some(w.clone());
        w
    }

    /// How many times [`Hub::wire_list`] had to convert a snapshot.
    pub fn wire_builds(&self) -> u64 {
        self.wire_builds.load(Ordering::Relaxed)
    }

    /// Connected realtime clients (each subscribes to events).
    pub fn clients(&self) -> usize {
        self.events.receiver_count()
    }

    /// Turns an engine event into client events (ones that only make sense in the GUI that
    /// hosts the engine, like preview answers, are dropped here).
    pub fn engine_event(&self, ev: EngineEvent) {
        let e = match ev {
            EngineEvent::Toast(kind, text) => Event::Notice {
                severity: match kind {
                    ToastKind::Info => Severity::Info,
                    ToastKind::Success => Severity::Success,
                    ToastKind::Warning => Severity::Warning,
                    ToastKind::Error => Severity::Error,
                },
                text,
                code: None,
            },
            EngineEvent::Added { hash, name } => Event::Added { hash, name },
            EngineEvent::Completed { hash, name, category, .. } => Event::Completed { hash, name, category },
            EngineEvent::Removed { hash, name, undoable } => Event::Removed { hash, name, undoable },
            _ => return,
        };
        self.publish(e);
    }
}
