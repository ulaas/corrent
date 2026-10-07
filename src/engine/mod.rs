//! The engine: a tokio task that owns the librqbit session. The UI talks to it only through
//! [`EngineCommand`]s and reads immutable [`Snapshot`]s, so the UI thread never blocks on I/O.

mod actor;
mod automation;
mod session;
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
}

#[derive(Clone, Debug)]
pub struct PreviewFile {
    pub path: String,
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
    pub multi_file: bool,
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
    PreviewReady { req: u64, result: Result<Box<Preview>, String> },
    Added { hash: InfoHash, name: String },
    Completed { hash: InfoHash, name: String, content_path: PathBuf, save_path: PathBuf, size: u64 },
    Removed { hash: InfoHash },
}

#[derive(Debug)]
pub enum EngineCommand {
    Add(Box<AddRequest>),
    Preview { req: u64, source: AddSource },
    CancelPreview(u64),
    Pause(Vec<InfoHash>),
    Resume(Vec<InfoHash>),
    ForceStart(Vec<InfoHash>),
    PauseAll,
    ResumeAll,
    Delete { hashes: Vec<InfoHash>, with_data: bool },
    SetFiles { hash: InfoHash, included: Vec<usize> },
    SetCategory { hashes: Vec<InfoHash>, category: Option<String> },
    SetTags { hashes: Vec<InfoHash>, tags: Vec<String> },
    ToggleTag { hashes: Vec<InfoHash>, tag: String, on: bool },
    Rename { hash: InfoHash, name: Option<String> },
    QueueMove { hashes: Vec<InfoHash>, mv: QueueMove },
    SetSeedLimits { hashes: Vec<InfoHash>, ratio: Option<f32>, minutes: Option<u32> },
    MoveStorage { hashes: Vec<InfoHash>, to: PathBuf },
    Recheck(Vec<InfoHash>),
    /// Add extra trackers (re-adds the torrent, which re-verifies its data).
    AddTrackers { hash: InfoHash, trackers: Vec<String> },
    SetCategories(BTreeMap<String, CategoryInfo>),
    SetTagList(Vec<String>),
    SetDetailTarget(Option<InfoHash>),
    ApplySettings(Box<Settings>),
    SetAltSpeed(bool),
    Shutdown(std::sync::mpsc::Sender<()>),
}

/// Free space on the volume holding `path` (or its nearest existing ancestor).
pub fn free_space_for(path: &std::path::Path) -> Option<u64> {
    actor::free_space(path)
}

/// Cheap, cloneable handle used by the UI.
#[derive(Clone)]
pub struct EngineHandle {
    tx: mpsc::UnboundedSender<EngineCommand>,
    snapshot: Arc<ArcSwap<Snapshot>>,
}

impl EngineHandle {
    pub fn start(
        rt: &tokio::runtime::Handle,
        paths: Paths,
        settings: Settings,
        ctx: egui::Context,
        events: std::sync::mpsc::Sender<EngineEvent>,
    ) -> EngineHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let snapshot = Arc::new(ArcSwap::from_pointee(Snapshot::default()));
        let actor = actor::Actor::new(paths, settings, ctx, events, snapshot.clone(), tx.clone());
        rt.spawn(actor.run(rx));
        EngineHandle { tx, snapshot }
    }

    pub fn send(&self, cmd: EngineCommand) {
        if self.tx.send(cmd).is_err() {
            tracing::warn!("engine is not running; command dropped");
        }
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
