//! The event hub: engine wake-ups (new snapshot) and discrete events fan out to every
//! connected client.

use corrent_core::engine::{EngineEvent, ToastKind};
use corrent_proto::event::{Event, Severity};

pub struct Hub {
    snapshot: tokio::sync::watch::Sender<u64>,
    events: tokio::sync::broadcast::Sender<Event>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub { snapshot: tokio::sync::watch::channel(0).0, events: tokio::sync::broadcast::channel(1024).0 }
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
