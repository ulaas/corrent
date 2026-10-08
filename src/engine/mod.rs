//! The engine: a tokio task that owns the librqbit session. The UI talks to it only through
//! [`EngineCommand`]s and reads immutable [`Snapshot`]s, so the UI thread never blocks on I/O.

mod actor;
mod automation;
pub mod creator;
#[cfg(test)]
mod e2e_tests;
pub mod logic;
mod restore;
mod session;
pub mod stream;
pub mod types;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::mpsc;

use crate::config::{Paths, Settings};
use crate::store::{CategoryInfo, QueueMove};
use types::{AddSource, InfoHash, Snapshot};

#[derive(Clone, Debug)]
pub struct AddRequest {
    pub source: AddSource,
    /// Pre-fetched .torrent bytes from a preview, so magnets are not resolved twice.
    pub torrent_bytes: Option<Vec<u8>>,
    pub save_path: PathBuf,
    pub create_subfolder: bool,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub paused: bool,
    pub only_files: Option<Vec<usize>>,
    pub add_to_top: bool,
    /// Peers to connect to right away (besides trackers/DHT).
    pub initial_peers: Option<Vec<std::net::SocketAddr>>,
}

#[derive(Clone, Debug)]
pub struct PreviewFile {
    pub components: Vec<String>,
    pub len: u64,
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub info_hash: InfoHash,
    pub name: String,
    pub files: Vec<PreviewFile>,
    pub total_size: u64,
    pub piece_count: u32,
    pub piece_length: u32,
    pub torrent_bytes: Vec<u8>,
    pub already_added: bool,
    pub comment: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub enum EngineEvent {
    Toast(ToastKind, String),
    PreviewReady {
        req: u64,
        result: Result<Box<Preview>, String>,
    },
    Added {
        hash: InfoHash,
        name: String,
    },
    Completed {
        name: String,
        content_path: PathBuf,
        save_path: PathBuf,
        size: u64,
    },
    Removed {
        hash: InfoHash,
        name: String,
        undoable: bool,
    },
    /// A torrent file was created (or creating failed).
    Created {
        result: Result<(String, PathBuf, String, Option<InfoHash>), String>,
    },
    /// An undo re-added a torrent.
    Restored {
        hash: InfoHash,
        name: String,
    },
    /// Persistent problem with the user's data (shown as a banner until dismissed).
    DataWarning(String),
}

#[derive(Debug)]
pub enum EngineCommand {
    Add(Box<AddRequest>),
    Preview {
        req: u64,
        source: AddSource,
    },
    CancelPreview(u64),
    /// Create a .torrent from a file/folder (and optionally seed it).
    CreateTorrent(Box<creator::CreateRequest>),
    /// Fetch one feed (or all) now.
    RssRefresh(Option<String>),
    /// An article was added by hand: rules skip it from now on.
    RssMarkDownloaded(String),
    /// Re-add torrents removed (without data) in the last two minutes.
    UndoRemove(Vec<InfoHash>),
    /// Abort an add that is still resolving metadata (identified by its placeholder row hash).
    CancelAdd(InfoHash),
    Pause(Vec<InfoHash>),
    Resume(Vec<InfoHash>),
    ForceStart(Vec<InfoHash>),
    PauseAll,
    ResumeAll,
    Delete {
        hashes: Vec<InfoHash>,
        with_data: bool,
    },
    SetFiles {
        hash: InfoHash,
        included: Vec<usize>,
    },
    SetCategory {
        hashes: Vec<InfoHash>,
        category: Option<String>,
    },
    SetTags {
        hashes: Vec<InfoHash>,
        tags: Vec<String>,
    },
    ToggleTag {
        hashes: Vec<InfoHash>,
        tag: String,
        on: bool,
    },
    Rename {
        hash: InfoHash,
        name: Option<String>,
    },
    /// Drag & drop in the queue: put these before `before` (None = at the end).
    QueuePlace {
        hashes: Vec<InfoHash>,
        before: Option<InfoHash>,
    },
    QueueMove {
        hashes: Vec<InfoHash>,
        mv: QueueMove,
    },
    SetSeedLimits {
        hashes: Vec<InfoHash>,
        ratio: Option<f32>,
        minutes: Option<u32>,
    },
    MoveStorage {
        hashes: Vec<InfoHash>,
        to: PathBuf,
    },
    Recheck(Vec<InfoHash>),
    /// Add extra trackers (re-adds the torrent, which re-verifies its data).
    AddTrackers {
        hash: InfoHash,
        trackers: Vec<String>,
    },
    SetCategories(BTreeMap<String, CategoryInfo>),
    SetTagList(Vec<String>),
    SetDetailTarget(Option<InfoHash>),
    ApplySettings(Box<Settings>),
    SetAltSpeed(bool),
    Shutdown(std::sync::mpsc::Sender<()>),
    /// Debug builds only: panic inside the engine task (crash-report testing).
    #[cfg(debug_assertions)]
    DevPanic,
}

/// Free space on the volume holding `path` (or its nearest existing ancestor).
pub fn free_space_for(path: &std::path::Path) -> Option<u64> {
    actor::free_space(path)
}

/// Commands queued while the engine is down, replayed after a restart.
const BACKLOG_CAP: usize = 256;

struct Inner {
    tx: mpsc::UnboundedSender<EngineCommand>,
    task: tokio::task::JoinHandle<()>,
    backlog: std::collections::VecDeque<EngineCommand>,
    stopping: bool,
}

/// Cheap, cloneable handle used by the UI. Survives engine restarts.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<std::sync::Mutex<Inner>>,
    snapshot: Arc<ArcSwap<Snapshot>>,
    rt: tokio::runtime::Handle,
    paths: Paths,
    ctx: egui::Context,
    events: std::sync::mpsc::Sender<EngineEvent>,
}

impl EngineHandle {
    pub fn start(
        rt: &tokio::runtime::Handle,
        paths: Paths,
        settings: Settings,
        ctx: egui::Context,
        events: std::sync::mpsc::Sender<EngineEvent>,
    ) -> EngineHandle {
        let snapshot = Arc::new(ArcSwap::from_pointee(Snapshot::default()));
        let (tx, task) = Self::spawn(rt, &paths, settings, &ctx, &events, &snapshot);
        EngineHandle {
            inner: Arc::new(std::sync::Mutex::new(Inner { tx, task, backlog: Default::default(), stopping: false })),
            snapshot,
            rt: rt.clone(),
            paths,
            ctx,
            events,
        }
    }

    fn spawn(
        rt: &tokio::runtime::Handle,
        paths: &Paths,
        settings: Settings,
        ctx: &egui::Context,
        events: &std::sync::mpsc::Sender<EngineEvent>,
        snapshot: &Arc<ArcSwap<Snapshot>>,
    ) -> (mpsc::UnboundedSender<EngineCommand>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let actor =
            actor::Actor::new(paths.clone(), settings, ctx.clone(), events.clone(), snapshot.clone(), tx.clone());
        let task = rt.spawn(actor.run(rx));
        (tx, task)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// False if the engine task ended unexpectedly (e.g. a panic).
    pub fn is_alive(&self) -> bool {
        let inner = self.lock();
        inner.stopping || !inner.task.is_finished()
    }

    pub fn send(&self, cmd: EngineCommand) {
        let mut inner = self.lock();
        if matches!(cmd, EngineCommand::Shutdown(_)) {
            inner.stopping = true;
        }
        let dead = inner.task.is_finished();
        let cmd = if dead { Err(cmd) } else { inner.tx.send(cmd).map_err(|e| e.0) };
        if let Err(cmd) = cmd {
            if matches!(cmd, EngineCommand::Shutdown(_)) {
                return; // nothing to stop; dropping the reply sender ends the wait
            }
            if inner.backlog.len() < BACKLOG_CAP {
                inner.backlog.push_back(cmd);
            } else {
                tracing::warn!("engine is down and its command backlog is full; command dropped");
            }
        }
    }

    /// Start a fresh engine (after a crash) and replay commands queued meanwhile.
    pub fn restart(&self, settings: Settings) {
        let mut inner = self.lock();
        if !inner.task.is_finished() {
            return;
        }
        let (tx, task) = Self::spawn(&self.rt, &self.paths, settings, &self.ctx, &self.events, &self.snapshot);
        for cmd in inner.backlog.drain(..) {
            let _ = tx.send(cmd);
        }
        inner.tx = tx;
        inner.task = task;
        tracing::info!("engine restarted");
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.load_full()
    }

    /// Ask the engine to stop and wait (bounded) for it to flush state.
    pub fn shutdown_blocking(&self, timeout: std::time::Duration) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(EngineCommand::Shutdown(tx));
        if rx.recv_timeout(timeout).is_err() {
            tracing::warn!("engine did not stop within {timeout:?}");
        }
    }
}
