//! The engine: a tokio task that owns the librqbit session. The UI talks to it only through
//! [`EngineCommand`]s and reads immutable [`Snapshot`]s, so the UI thread never blocks on I/O.

mod actor;
mod automation;
pub mod creator;
#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod fault_tests;
#[cfg(any(test, feature = "testkit"))]
pub mod faultserver;
#[cfg(any(test, feature = "testkit"))]
pub(crate) mod faulty_storage;
pub mod ipfilter;
pub mod logic;
mod moves;
#[cfg(any(test, feature = "testkit"))]
pub mod netproxy;
pub mod paths;
mod restore;
pub(crate) mod session;
mod session_db;
pub mod stream;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod types;
#[cfg(test)]
mod untrusted_tests;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

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
    /// Removed from the list for good, its files left alone and no Undo offered: the files have
    /// a new home (moved into a library), so re-adding them would download again.
    Forget(Vec<InfoHash>),
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
    /// Run the health self-checks now (they also run every 5 minutes).
    CheckHealth,
    #[cfg(debug_assertions)]
    DevPanic,
    /// Debug builds only: the engine is busy this long (hang-report testing).
    #[cfg(debug_assertions)]
    DevHang(std::time::Duration),
    /// Debug builds only: a task next to the engine panics, like a librqbit peer task (E7).
    #[cfg(debug_assertions)]
    DevTaskPanic,
}

/// Free space on the volume holding `path` (or its nearest existing ancestor).
pub fn free_space_for(path: &std::path::Path) -> Option<u64> {
    actor::free_space(path)
}

/// Commands queued while the engine is down, replayed after a restart.
const BACKLOG_CAP: usize = 256;

/// Commands waiting for the engine, at most (STABILITY.md F11). The actor takes one per loop
/// turn, so a full queue means it is stuck or flooded: callers are refused instead of growing
/// memory without end (the GUI shows a "busy" toast, the APIs answer 503).
pub const COMMAND_QUEUE: usize = 1024;

/// Adds from the APIs are refused once this many adds wait (each can carry a 32 MiB .torrent).
pub const API_ADD_QUEUE: usize = 16;

/// The engine's end of the command queue. It counts the adds that leave it, for
/// [`EngineHandle::queued_adds`].
pub struct CommandReceiver {
    rx: mpsc::Receiver<EngineCommand>,
    adds: Arc<std::sync::atomic::AtomicUsize>,
}

impl CommandReceiver {
    pub async fn recv(&mut self) -> Option<EngineCommand> {
        let cmd = self.rx.recv().await;
        if matches!(cmd, Some(EngineCommand::Add(_))) {
            // Saturating: the actor also queues adds for itself, which aren't counted.
            let mut n = self.adds.load(Ordering::Relaxed);
            while n > 0 {
                match self.adds.compare_exchange_weak(n, n - 1, Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(now) => n = now,
                }
            }
        }
        cmd
    }
}

/// A fresh command queue: the sender, the engine's end and the shared count of queued adds.
fn command_queue() -> (mpsc::Sender<EngineCommand>, CommandReceiver, Arc<std::sync::atomic::AtomicUsize>) {
    let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
    let adds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    (tx, CommandReceiver { rx, adds: adds.clone() }, adds)
}

struct Inner {
    tx: mpsc::Sender<EngineCommand>,
    /// Adds waiting in `tx`'s queue.
    adds: Arc<std::sync::atomic::AtomicUsize>,
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
    pub commands: CommandReceiver,
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
    /// Commands refused because the queue was full (see [`Self::refused`]).
    refused: Arc<std::sync::atomic::AtomicU64>,
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
        let (tx, adds, task) = Self::spawn(rt, &paths, settings, &wake, &events, &snapshot);
        EngineHandle {
            inner: Arc::new(std::sync::Mutex::new(Inner {
                tx,
                adds,
                task,
                backlog: Default::default(),
                stopping: false,
            })),
            snapshot,
            rt: rt.clone(),
            paths,
            wake,
            events,
            backend: None,
            refused: Default::default(),
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
        let (tx, adds, task) = Self::spawn_backend(rt, &backend, &wake, &events, &snapshot);
        EngineHandle {
            inner: Arc::new(std::sync::Mutex::new(Inner {
                tx,
                adds,
                task,
                backlog: Default::default(),
                stopping: false,
            })),
            snapshot,
            rt: rt.clone(),
            paths,
            wake,
            events,
            backend: Some(backend),
            refused: Default::default(),
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
    ) -> (mpsc::Sender<EngineCommand>, Arc<std::sync::atomic::AtomicUsize>, tokio::task::JoinHandle<()>) {
        let (tx, rx, adds) = command_queue();
        let io = BackendIo { commands: rx, snapshot: snapshot.clone(), wake: wake.clone(), events: events.clone() };
        (tx, adds, rt.spawn(crate::crash::engine_scope(backend(io))))
    }

    fn spawn(
        rt: &tokio::runtime::Handle,
        paths: &Paths,
        settings: Settings,
        wake: &Wake,
        events: &std::sync::mpsc::Sender<EngineEvent>,
        snapshot: &Arc<ArcSwap<Snapshot>>,
    ) -> (mpsc::Sender<EngineCommand>, Arc<std::sync::atomic::AtomicUsize>, tokio::task::JoinHandle<()>) {
        let (tx, rx, adds) = command_queue();
        let (paths, wake, events, snapshot, self_tx) =
            (paths.clone(), wake.clone(), events.clone(), snapshot.clone(), tx.clone());
        // Loading the store and feed history does file I/O (with retries): not on the UI thread.
        let task = rt.spawn(async move {
            // A panic in the engine actor is fatal (E7), also while it loads.
            let actor = tokio::task::spawn_blocking(move || {
                crate::crash::engine_thread(|| actor::Actor::new(paths, settings, wake, events, snapshot, self_tx))
            })
            .await;
            match actor {
                Ok(actor) => crate::crash::engine_scope(actor.run(rx)).await,
                Err(e) => std::panic::resume_unwind(e.into_panic()),
            }
        });
        (tx, adds, task)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// False if the engine task ended unexpectedly (e.g. a panic).
    pub fn is_alive(&self) -> bool {
        let inner = self.lock();
        inner.stopping || !inner.task.is_finished()
    }

    /// Queue `cmd` for the engine. False if it was dropped: the queue is full
    /// ([`COMMAND_QUEUE`], counted in [`Self::refused`]) or the engine is down with a full
    /// backlog. Shutdown and Flush are never refused: they wait for room.
    pub fn send(&self, cmd: EngineCommand) -> bool {
        self.send_counted(cmd, true)
    }

    /// [`Self::send`] for commands from the APIs: a refusal is answered there (503), so it isn't
    /// counted in [`Self::refused`] (which the GUI reports as its own).
    pub fn send_from_api(&self, cmd: EngineCommand) -> bool {
        self.send_counted(cmd, false)
    }

    fn send_counted(&self, cmd: EngineCommand, count: bool) -> bool {
        use mpsc::error::TrySendError;
        let add = matches!(cmd, EngineCommand::Add(_));
        let mut inner = self.lock();
        if matches!(cmd, EngineCommand::Shutdown(_)) {
            inner.stopping = true;
        }
        let dead = inner.task.is_finished();
        let cmd = match if dead { Err(TrySendError::Closed(cmd)) } else { inner.tx.try_send(cmd) } {
            Ok(()) => {
                if add {
                    inner.adds.fetch_add(1, Ordering::Relaxed);
                }
                return true;
            }
            Err(TrySendError::Full(cmd)) => {
                if matches!(cmd, EngineCommand::Shutdown(_) | EngineCommand::Flush(_)) {
                    let tx = inner.tx.clone();
                    self.rt.spawn(async move {
                        let _ = tx.send(cmd).await;
                    });
                    return true;
                }
                if count {
                    let n = self.refused.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_power_of_two() {
                        tracing::warn!("the engine's command queue is full; {n} command(s) refused so far");
                    }
                } else {
                    tracing::warn!("the engine's command queue is full; an API command was refused");
                }
                return false;
            }
            Err(TrySendError::Closed(cmd)) => cmd,
        };
        if matches!(cmd, EngineCommand::Shutdown(_)) {
            return true; // nothing to stop; dropping the reply sender ends the wait
        }
        if inner.backlog.len() < BACKLOG_CAP {
            inner.backlog.push_back(cmd);
            true
        } else {
            tracing::warn!("engine is down and its command backlog is full; command dropped");
            false
        }
    }

    /// How many commands were refused because the engine's queue was full, since the start.
    pub fn refused(&self) -> u64 {
        self.refused.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Commands waiting for the engine now.
    pub fn queued(&self) -> usize {
        let inner = self.lock();
        inner.tx.max_capacity() - inner.tx.capacity()
    }

    /// Adds waiting for the engine now (the APIs refuse more at [`API_ADD_QUEUE`]).
    pub fn queued_adds(&self) -> usize {
        self.lock().adds.load(Ordering::Relaxed)
    }

    /// Start a fresh engine (after a crash) and replay commands queued meanwhile.
    pub fn restart(&self, settings: Settings) {
        let mut inner = self.lock();
        if !inner.task.is_finished() {
            return;
        }
        let (tx, adds, task) = match &self.backend {
            Some(b) => Self::spawn_backend(&self.rt, b, &self.wake, &self.events, &self.snapshot),
            None => Self::spawn(&self.rt, &self.paths, settings, &self.wake, &self.events, &self.snapshot),
        };
        // The backlog (at most BACKLOG_CAP) fits the fresh queue.
        for cmd in inner.backlog.drain(..) {
            let add = matches!(cmd, EngineCommand::Add(_));
            if tx.try_send(cmd).is_ok() && add {
                adds.fetch_add(1, Ordering::Relaxed);
            }
        }
        inner.tx = tx;
        inner.adds = adds;
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

#[cfg(test)]
mod queue_tests {
    use super::*;

    /// A backend that holds its commands but never reads them (a stuck engine).
    fn stuck() -> Backend {
        Arc::new(|io: BackendIo| {
            Box::pin(async move {
                let _commands = io.commands;
                std::future::pending::<()>().await
            })
        })
    }

    #[test]
    fn a_full_command_queue_refuses_and_counts() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = std::env::temp_dir().join(format!("corrent-queue-{}", std::process::id()));
        let paths = Paths { data_dir: dir.clone(), portable: true };
        let (events, _rx) = std::sync::mpsc::channel();
        let e = EngineHandle::with_backend(rt.handle(), paths, Wake::noop(), events, stuck());
        for _ in 0..COMMAND_QUEUE {
            assert!(e.send(EngineCommand::PauseAll));
        }
        assert_eq!(e.queued(), COMMAND_QUEUE);
        assert!(!e.send(EngineCommand::PauseAll), "refused when full");
        assert!(!e.send(EngineCommand::ResumeAll));
        assert_eq!(e.refused(), 2);
        // Refusals of API commands are answered by the API, not counted for the GUI.
        assert!(!e.send_from_api(EngineCommand::PauseAll));
        assert_eq!(e.refused(), 2);
        // Shutdown and Flush wait for room instead of being lost.
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(e.send(EngineCommand::Shutdown(tx)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
