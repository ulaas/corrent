//! The engine: a tokio task that owns the librqbit session. The UI talks to it only through
//! [`EngineCommand`]s and reads immutable [`Snapshot`]s, so the UI thread never blocks on I/O.

mod actor;
mod automation;
pub mod creator;
#[cfg(test)]
mod e2e_tests;
pub mod ipfilter;
pub mod logic;
mod moves;
pub mod paths;
mod restore;
mod session;
pub mod stream;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
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
    /// The server user who added it (None: the desktop app, RSS, watch folders).
    pub added_by: Option<String>,
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
        hash: InfoHash,
        name: String,
        /// The torrent's category (libraries import completed torrents by category).
        category: Option<String>,
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
        /// The request id, so a late answer for a closed dialog is ignored.
        id: u64,
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
    RssMarkDownloaded {
        feed: String,
        guid: String,
    },
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
    /// Per-torrent speed limits in KiB/s (0 = no limit).
    SetTorrentLimits {
        hashes: Vec<InfoHash>,
        down_kib: u32,
        up_kib: u32,
    },
    /// Files, peers and pieces of one torrent for a remote client (the GUI's details target is
    /// unaffected; several clients can watch different torrents).
    Detail {
        hash: InfoHash,
        reply: tokio::sync::oneshot::Sender<Option<types::TorrentDetail>>,
    },
    /// A torrent's files, answered directly (Web API), without changing the details target.
    FileList {
        hash: InfoHash,
        reply: tokio::sync::oneshot::Sender<Option<Vec<types::FileView>>>,
    },
    /// Download priority for files of a torrent (1 high, 0 normal, -1 low).
    SetFilePriority {
        hash: InfoHash,
        files: Vec<usize>,
        priority: i8,
    },
    /// Re-read the IP filter lists (restarts the session).
    ReloadIpFilter,
    /// Close a streaming session (its player gets an error).
    StopStream(u64),
    /// The user plays this file again: lift a Stop block.
    AllowStream {
        hash: InfoHash,
        file: usize,
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
    /// Save the app data (store, feeds) now and answer; the engine keeps running (the OS is
    /// asking whether the session may end, and the answer may still be no).
    Flush(std::sync::mpsc::Sender<()>),
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

/// Called by the engine whenever a new snapshot or event is ready (the GUI passes
/// `egui::Context::request_repaint`; the server wakes its event fan-out).
#[derive(Clone)]
pub struct Wake(Arc<dyn Fn() + Send + Sync>);

impl Wake {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Wake {
        Wake(Arc::new(f))
    }
    pub fn noop() -> Wake {
        Wake::new(|| {})
    }
    pub fn wake(&self) {
        (self.0)()
    }
}

/// What an engine backend gets: its commands, where to publish snapshots, how to wake the
/// reader, and where to send events.
pub struct BackendIo {
    pub commands: mpsc::UnboundedReceiver<EngineCommand>,
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub wake: Wake,
    pub events: std::sync::mpsc::Sender<EngineEvent>,
}

/// Something other than the local librqbit session answering engine commands (a remote
/// server). Called again on every restart.
pub type Backend =
    Arc<dyn Fn(BackendIo) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Cheap, cloneable handle used by the UI. Survives engine restarts.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<std::sync::Mutex<Inner>>,
    snapshot: Arc<ArcSwap<Snapshot>>,
    rt: tokio::runtime::Handle,
    paths: Paths,
    wake: Wake,
    events: std::sync::mpsc::Sender<EngineEvent>,
    backend: Option<Backend>,
}

impl EngineHandle {
    pub fn start(
        rt: &tokio::runtime::Handle,
        paths: Paths,
        settings: Settings,
        wake: Wake,
        events: std::sync::mpsc::Sender<EngineEvent>,
    ) -> EngineHandle {
        let snapshot = Arc::new(ArcSwap::from_pointee(Snapshot::default()));
        let (tx, task) = Self::spawn(rt, &paths, settings, &wake, &events, &snapshot);
        EngineHandle {
            inner: Arc::new(std::sync::Mutex::new(Inner { tx, task, backlog: Default::default(), stopping: false })),
            snapshot,
            rt: rt.clone(),
            paths,
            wake,
            events,
            backend: None,
        }
    }

    /// A handle whose commands go to `backend` instead of a local session.
    pub fn with_backend(
        rt: &tokio::runtime::Handle,
        paths: Paths,
        wake: Wake,
        events: std::sync::mpsc::Sender<EngineEvent>,
        backend: Backend,
    ) -> EngineHandle {
        let snapshot = Arc::new(ArcSwap::from_pointee(Snapshot::default()));
        let (tx, task) = Self::spawn_backend(rt, &backend, &wake, &events, &snapshot);
        EngineHandle {
            inner: Arc::new(std::sync::Mutex::new(Inner { tx, task, backlog: Default::default(), stopping: false })),
            snapshot,
            rt: rt.clone(),
            paths,
            wake,
            events,
            backend: Some(backend),
        }
    }

    /// True when a backend other than the local session answers (see [`Self::with_backend`]).
    pub fn is_remote(&self) -> bool {
        self.backend.is_some()
    }

    fn spawn_backend(
        rt: &tokio::runtime::Handle,
        backend: &Backend,
        wake: &Wake,
        events: &std::sync::mpsc::Sender<EngineEvent>,
        snapshot: &Arc<ArcSwap<Snapshot>>,
    ) -> (mpsc::UnboundedSender<EngineCommand>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let io = BackendIo { commands: rx, snapshot: snapshot.clone(), wake: wake.clone(), events: events.clone() };
        (tx, rt.spawn(backend(io)))
    }

    fn spawn(
        rt: &tokio::runtime::Handle,
        paths: &Paths,
        settings: Settings,
        wake: &Wake,
        events: &std::sync::mpsc::Sender<EngineEvent>,
        snapshot: &Arc<ArcSwap<Snapshot>>,
    ) -> (mpsc::UnboundedSender<EngineCommand>, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (paths, wake, events, snapshot, self_tx) =
            (paths.clone(), wake.clone(), events.clone(), snapshot.clone(), tx.clone());
        // Loading the store and feed history does file I/O (with retries): not on the UI thread.
        let task = rt.spawn(async move {
            let actor = tokio::task::spawn_blocking(move || {
                actor::Actor::new(paths, settings, wake, events, snapshot, self_tx)
            })
            .await;
            match actor {
                Ok(actor) => actor.run(rx).await,
                Err(e) => std::panic::resume_unwind(e.into_panic()),
            }
        });
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
        let (tx, task) = match &self.backend {
            Some(b) => Self::spawn_backend(&self.rt, b, &self.wake, &self.events, &self.snapshot),
            None => Self::spawn(&self.rt, &self.paths, settings, &self.wake, &self.events, &self.snapshot),
        };
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
    /// Save the app data and wait (at most `timeout`). False if the engine didn't answer.
    pub fn flush_blocking(&self, timeout: std::time::Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(EngineCommand::Flush(tx));
        rx.recv_timeout(timeout).is_ok()
    }

    pub fn shutdown_blocking(&self, timeout: std::time::Duration) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(EngineCommand::Shutdown(tx));
        if rx.recv_timeout(timeout).is_err() {
            tracing::warn!("engine did not stop within {timeout:?}");
        }
    }
}
