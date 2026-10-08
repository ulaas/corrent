use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use arc_swap::ArcSwap;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::Id20;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Api, ManagedTorrent, Session, TorrentStatsState};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::session::{self, kib_limit};
use super::types::*;
use super::{AddRequest, EngineCommand, EngineEvent, Preview, PreviewFile, ToastKind};
use crate::config::{Paths, SeedAction, Settings};
use crate::store::{PauseReason, Store};
use crate::util::now_unix;

const HISTORY_LEN: usize = 600;
const STALL_AFTER: Duration = Duration::from_secs(30);

enum Internal {
    AddFinished { pending: u64, req: Box<AddRequest>, result: Result<(InfoHash, String), String> },
    PreviewFinished { req: u64 },
    OpFinished { hash: InfoHash, what: &'static str, result: Result<(), String> },
    Deleted { hash: InfoHash, result: Result<(), String> },
    RestoreReport(Vec<super::restore::FailedRestore>),
    UndoFinished { hash: InfoHash, meta: Box<crate::store::TorrentMeta>, result: Result<(), String> },
    RestoreRetried { hash: InfoHash, result: Result<(), String>, quiet: bool },
}

/// What is needed to re-add a torrent that was just removed (Undo).
struct UndoEntry {
    bytes: Vec<u8>,
    output: PathBuf,
    only_files: Option<Vec<usize>>,
    paused: bool,
    trackers: Vec<String>,
    meta: crate::store::TorrentMeta,
    at: Instant,
}

const UNDO_WINDOW: Duration = Duration::from_secs(120);

struct PendingAdd {
    name: String,
    hash: InfoHash,
    started: i64,
    save_path: PathBuf,
    task: Option<JoinHandle<()>>,
}

/// Static per-torrent info parsed once from metadata.
#[derive(Clone, Default)]
struct StaticInfo {
    file_count: usize,
    piece_count: u32,
    piece_size: u64,
    multi_file: bool,
    comment: Option<String>,
    created_by: Option<String>,
    creation_date: Option<i64>,
}

pub struct Actor {
    paths: Paths,
    settings: Settings,
    ctx: egui::Context,
    events: std::sync::mpsc::Sender<EngineEvent>,
    snapshot: Arc<ArcSwap<Snapshot>>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    internal_rx: Option<mpsc::UnboundedReceiver<Internal>>,
    self_tx: mpsc::UnboundedSender<EngineCommand>,

    session: Option<Arc<Session>>,
    api: Option<Api>,
    engine_state: EngineState,
    warning: Option<String>,
    store: Store,

    alt: bool,
    last_schedule_state: Option<bool>,
    detail_target: Option<InfoHash>,
    history: VecDeque<(u64, u64)>,
    torrent_history: HashMap<InfoHash, VecDeque<(u64, u64)>>,
    prev_peer: HashMap<String, (u64, u64)>,
    pieces_cache: Option<(Instant, InfoHash, (Vec<u8>, u32))>,
    static_info: HashMap<InfoHash, StaticInfo>,
    previews: HashMap<u64, JoinHandle<()>>,
    pending: HashMap<u64, PendingAdd>,
    next_pending: u64,
    busy: HashMap<InfoHash, String>,
    seen_unfinished: HashSet<InfoHash>,
    last_down_active: HashMap<InfoHash, Instant>,
    seed_carry_ms: HashMap<InfoHash, u64>,
    generation: u64,
    tick_count: u64,
    last_tick: Instant,
    free_space: Option<u64>,
    session_down_total: u64,
    session_up_total: u64,
    watch: super::automation::WatchState,
    slow_since: HashMap<InfoHash, Instant>,
    resumed_at: HashMap<InfoHash, Instant>,
    deferred_settings: Option<Box<Settings>>,
    failed: HashMap<InfoHash, super::restore::FailedRestore>,
    undo_pending: HashMap<InfoHash, UndoEntry>,
    undo: HashMap<InfoHash, UndoEntry>,
    /// Pruning the store waits until failed restores have been identified.
    restore_checked: bool,
}

impl Actor {
    pub fn new(
        paths: Paths,
        settings: Settings,
        ctx: egui::Context,
        events: std::sync::mpsc::Sender<EngineEvent>,
        snapshot: Arc<ArcSwap<Snapshot>>,
        self_tx: mpsc::UnboundedSender<EngineCommand>,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let store = Store::load(paths.store_file());
        let alt = settings.speed.alt_enabled;
        Actor {
            paths,
            settings,
            ctx,
            events,
            snapshot,
            internal_tx,
            internal_rx: Some(internal_rx),
            self_tx,
            session: None,
            api: None,
            engine_state: EngineState::Starting,
            warning: None,
            store,
            alt,
            last_schedule_state: None,
            detail_target: None,
            history: VecDeque::with_capacity(HISTORY_LEN),
            torrent_history: HashMap::new(),
            prev_peer: HashMap::new(),
            pieces_cache: None,
            static_info: HashMap::new(),
            previews: HashMap::new(),
            pending: HashMap::new(),
            next_pending: 1,
            busy: HashMap::new(),
            seen_unfinished: HashSet::new(),
            last_down_active: HashMap::new(),
            seed_carry_ms: HashMap::new(),
            generation: 0,
            tick_count: 0,
            last_tick: Instant::now(),
            free_space: None,
            session_down_total: 0,
            session_up_total: 0,
            watch: Default::default(),
            slow_since: HashMap::new(),
            resumed_at: HashMap::new(),
            deferred_settings: None,
            failed: HashMap::new(),
            undo_pending: HashMap::new(),
            undo: HashMap::new(),
            restore_checked: false,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<EngineCommand>) {
        let mut internal_rx = self.internal_rx.take().expect("run called once");
        if let Some(w) = self.store.warning.take() {
            self.emit(EngineEvent::DataWarning(w));
        }
        self.start_session().await;
        let mut tick = tokio::time::interval(Duration::from_millis(self.settings.advanced.refresh_interval_ms.clamp(200, 5000)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None => { self.shutdown().await; break; }
                    Some(EngineCommand::Shutdown(done)) => {
                        self.shutdown().await;
                        let _ = done.send(());
                        break;
                    }
                    Some(cmd) => {
                        self.handle(cmd).await;
                        self.publish();
                    }
                },
                Some(msg) = internal_rx.recv() => {
                    self.handle_internal(msg);
                    self.publish();
                }
                _ = tick.tick() => {
                    self.tick();
                }
            }
        }
    }

    // ---------------------------------------------------------------- lifecycle

    async fn start_session(&mut self) {
        self.engine_state = if self.session.is_some() { EngineState::Restarting } else { EngineState::Starting };
        self.publish();
        self.purge_pending();
        let mut last_err = String::new();
        for attempt in 0..3 {
            match session::create(&self.paths, &self.settings, self.alt).await {
                Ok((sess, warning)) => {
                    if let Some(w) = &warning {
                        self.toast(ToastKind::Warning, w.clone());
                    }
                    self.warning = warning;
                    self.api = Some(Api::new(sess.clone(), None));
                    self.session = Some(sess);
                    self.engine_state = EngineState::Running;
                    self.apply_rate_limits();
                    tracing::info!("engine session started");
                    self.detect_failed_restores();
                    self.publish();
                    return;
                }
                Err(e) => {
                    last_err = format!("{e:#}");
                    tracing::error!("failed to start session (attempt {}): {last_err}", attempt + 1);
                    tokio::time::sleep(Duration::from_millis(700 * (attempt + 1))).await;
                }
            }
        }
        self.engine_state = EngineState::Failed(last_err.clone());
        self.toast(ToastKind::Error, format!("Could not start the torrent engine: {last_err}"));
        self.publish();
    }

    async fn stop_session(&mut self) {
        self.api = None;
        if let Some(sess) = self.session.take() {
            if tokio::time::timeout(Duration::from_secs(6), sess.stop()).await.is_err() {
                tracing::warn!("session stop timed out");
            }
            // Give listeners a moment to release their sockets before a restart binds again.
            drop(sess);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn shutdown(&mut self) {
        tracing::info!("engine shutting down");
        for (_, h) in self.previews.drain() {
            h.abort();
        }
        self.store.maybe_save(true);
        self.stop_session().await;
        self.purge_pending();
        self.store.maybe_save(true);
    }

    fn apply_rate_limits(&self) {
        if let Some(s) = &self.session {
            let (down, up) = self.settings.speed.effective(self.alt);
            s.ratelimits.set_download_bps(kib_limit(down));
            s.ratelimits.set_upload_bps(kib_limit(up));
        }
    }

    fn toast(&self, kind: ToastKind, text: impl Into<String>) {
        let _ = self.events.send(EngineEvent::Toast(kind, text.into()));
        self.ctx.request_repaint();
    }

    fn emit(&self, ev: EngineEvent) {
        let _ = self.events.send(ev);
        self.ctx.request_repaint();
    }

    fn handle_for(&self, hash: &InfoHash) -> Option<Arc<ManagedTorrent>> {
        self.session.as_ref()?.get(TorrentIdOrHash::Hash(Id20::new(hash.0)))
    }

    // ---------------------------------------------------------------- commands

    /// Drop hashes that have an operation (move/recheck/delete) in flight.
    fn not_busy(&self, hashes: Vec<InfoHash>) -> Vec<InfoHash> {
        let (ok, busy): (Vec<_>, Vec<_>) = hashes.into_iter().partition(|h| !self.busy.contains_key(h));
        if !busy.is_empty() {
            self.toast(ToastKind::Info, "Some torrents are busy with another operation; try again in a moment.");
        }
        ok
    }

    async fn handle(&mut self, cmd: EngineCommand) {
        let cmd = match cmd {
            EngineCommand::Pause(h) => EngineCommand::Pause(self.not_busy(h)),
            EngineCommand::Resume(h) => EngineCommand::Resume(self.not_busy(h)),
            EngineCommand::ForceStart(h) => EngineCommand::ForceStart(self.not_busy(h)),
            EngineCommand::Delete { hashes, with_data } => EngineCommand::Delete { hashes: self.not_busy(hashes), with_data },
            EngineCommand::MoveStorage { hashes, to } => EngineCommand::MoveStorage { hashes: self.not_busy(hashes), to },
            EngineCommand::Recheck(h) => EngineCommand::Recheck(self.not_busy(h)),
            EngineCommand::SetFiles { hash, included } if self.busy.contains_key(&hash) => {
                let _ = included;
                self.toast(ToastKind::Info, "This torrent is busy with another operation.");
                return;
            }
            other => other,
        };
        match cmd {
            EngineCommand::UndoRemove(hashes) => {
                for hash in hashes {
                    let Some(e) = self.undo.remove(&hash) else { continue };
                    if e.at.elapsed() > UNDO_WINDOW {
                        continue;
                    }
                    let Some(sess) = self.session.clone() else { continue };
                    let tx = self.internal_tx.clone();
                    self.busy.insert(hash, "Restoring…".into());
                    tokio::spawn(async move {
                        let opts = AddTorrentOptions {
                            paused: e.paused,
                            only_files: e.only_files.clone(),
                            output_folder: Some(e.output.to_string_lossy().into_owned()),
                            overwrite: true,
                            trackers: (!e.trackers.is_empty()).then(|| e.trackers.clone()),
                            ..Default::default()
                        };
                        let result = sess
                            .add_torrent(AddTorrent::from_bytes(e.bytes), Some(opts))
                            .await
                            .map(|_| ())
                            .map_err(|err| format!("{err:#}"));
                        let _ = tx.send(Internal::UndoFinished { hash, meta: Box::new(e.meta), result });
                    });
                }
            }
            EngineCommand::CancelAdd(hash) => {
                let ids: Vec<u64> = self.pending.iter().filter(|(_, p)| p.hash == hash).map(|(id, _)| *id).collect();
                for id in ids {
                    if let Some(p) = self.pending.remove(&id)
                        && let Some(t) = p.task
                    {
                        t.abort();
                    }
                }
            }
            EngineCommand::Add(req) => self.start_add(req),
            EngineCommand::Preview { req, source } => self.start_preview(req, source),
            EngineCommand::CancelPreview(req) => {
                if let Some(h) = self.previews.remove(&req) {
                    h.abort();
                }
            }
            EngineCommand::Pause(hashes) => {
                for h in hashes {
                    self.pause(h, PauseReason::User).await;
                }
            }
            EngineCommand::Resume(hashes) => {
                for h in hashes {
                    if self.failed.contains_key(&h) {
                        self.retry_failed(h);
                    } else {
                        self.resume(h, false).await;
                    }
                }
            }
            EngineCommand::ForceStart(hashes) => {
                for h in hashes {
                    if self.failed.contains_key(&h) {
                        self.retry_failed(h);
                    } else {
                        self.resume(h, true).await;
                    }
                }
            }
            EngineCommand::PauseAll => {
                let all: Vec<InfoHash> = self.store.data.torrents.keys().copied().collect();
                for h in all {
                    self.pause(h, PauseReason::User).await;
                }
            }
            EngineCommand::ResumeAll => {
                let all: Vec<InfoHash> = self.store.data.torrents.keys().copied().collect();
                for h in all {
                    self.resume(h, false).await;
                }
            }
            EngineCommand::Delete { hashes, with_data } => {
                for hash in hashes {
                    if self.failed.remove(&hash).is_some() {
                        // The engine never loaded it; drop it from its database on the next stop.
                        self.store.data.purge.push(hash);
                        self.forget_hash(&hash);
                        self.store.maybe_save(true);
                        self.emit(EngineEvent::Removed { hash, name: String::new(), undoable: false });
                        if with_data {
                            self.toast(ToastKind::Info, "The torrent was removed; its files were left in place because they couldn't be opened.");
                        }
                    } else {
                        self.delete(hash, with_data);
                    }
                }
            }
            EngineCommand::SetFiles { hash, included } => {
                if let (Some(sess), Some(h)) = (self.session.clone(), self.handle_for(&hash)) {
                    let set: HashSet<usize> = included.into_iter().collect();
                    if set.is_empty() {
                        self.toast(ToastKind::Warning, "At least one file must stay selected.");
                    } else if let Err(e) = sess.update_only_files(&h, &set).await {
                        self.toast(ToastKind::Error, format!("Could not update file selection: {e:#}"));
                    }
                }
            }
            EngineCommand::SetCategory { hashes, category } => {
                for h in hashes {
                    self.store.meta_mut(h).category = category.clone();
                }
                if let Some(c) = category {
                    self.store.data.categories.entry(c).or_default();
                }
            }
            EngineCommand::SetTags { hashes, tags } => {
                for h in hashes {
                    self.store.meta_mut(h).tags = tags.clone();
                }
                self.register_tags(&tags);
            }
            EngineCommand::ToggleTag { hashes, tag, on } => {
                for h in hashes {
                    let m = self.store.meta_mut(h);
                    m.tags.retain(|t| t != &tag);
                    if on {
                        m.tags.push(tag.clone());
                        m.tags.sort();
                    }
                }
                self.register_tags(std::slice::from_ref(&tag));
            }
            EngineCommand::Rename { hash, name } => {
                self.store.meta_mut(hash).display_name = name.filter(|n| !n.trim().is_empty());
            }
            EngineCommand::QueueMove { hashes, mv } => {
                // Move in an order that keeps the relative order of a multi-selection.
                let mut hs = hashes;
                hs.sort_by_key(|h| self.store.get(h).map(|m| m.queue_pos).unwrap_or(0));
                if matches!(mv, crate::store::QueueMove::Down | crate::store::QueueMove::Top) {
                    hs.reverse();
                }
                for h in hs {
                    self.store.queue_move(h, mv);
                }
            }
            EngineCommand::SetSeedLimits { hashes, ratio, minutes } => {
                for h in hashes {
                    let m = self.store.meta_mut(h);
                    m.seed_ratio_override = ratio;
                    m.seed_time_override_min = minutes;
                }
            }
            EngineCommand::MoveStorage { hashes, to } => {
                for h in hashes {
                    self.start_readd(h, Some(to.clone()), None, "move");
                }
            }
            EngineCommand::Recheck(hashes) => {
                for h in hashes {
                    self.start_readd(h, None, None, "recheck");
                }
            }
            EngineCommand::AddTrackers { hash, trackers } => {
                self.start_readd(hash, None, Some(trackers), "trackers");
            }
            EngineCommand::SetCategories(cats) => {
                // Torrents in removed categories become uncategorized.
                for m in self.store.data.torrents.values_mut() {
                    if let Some(c) = &m.category
                        && !cats.contains_key(c)
                    {
                        m.category = None;
                    }
                }
                self.store.data.categories = cats;
                self.store.mark_dirty();
            }
            EngineCommand::SetTagList(tags) => {
                for m in self.store.data.torrents.values_mut() {
                    m.tags.retain(|t| tags.contains(t));
                }
                self.store.data.tags = tags;
                self.store.mark_dirty();
            }
            EngineCommand::SetDetailTarget(t) => {
                if self.detail_target != t {
                    self.detail_target = t;
                    self.prev_peer.clear();
                    self.pieces_cache = None;
                }
            }
            EngineCommand::ApplySettings(new) => {
                let restart = self.settings.requires_restart(&new);
                if restart && !self.busy.is_empty() {
                    // Tasks hold the current session; restarting under them could corrupt its state.
                    self.toast(ToastKind::Info, "Network settings will apply once running operations finish.");
                    self.deferred_settings = Some(new);
                    return;
                }
                if self.settings.queue.enabled && !new.queue.enabled {
                    let queued: Vec<InfoHash> = self
                        .store
                        .data
                        .torrents
                        .iter()
                        .filter(|(_, m)| m.pause_reason == Some(PauseReason::Queue))
                        .map(|(h, _)| *h)
                        .collect();
                    for h in queued {
                        self.resume(h, false).await;
                    }
                }
                self.settings = *new;
                if restart {
                    tracing::info!("settings changed; restarting session");
                    self.stop_session().await;
                    self.start_session().await;
                    if self.engine_state == EngineState::Running {
                        self.toast(ToastKind::Success, "Settings applied — reconnected.");
                    }
                } else {
                    self.apply_rate_limits();
                }
            }
            EngineCommand::SetAltSpeed(on) => {
                self.alt = on;
                self.apply_rate_limits();
            }
            EngineCommand::Shutdown(_) => unreachable!("handled in run loop"),
            #[cfg(debug_assertions)]
            EngineCommand::DevPanic => panic!("CORRENT_DEV_PANIC=engine"),
        }
    }

    fn register_tags(&mut self, tags: &[String]) {
        for t in tags {
            if !self.store.data.tags.contains(t) {
                self.store.data.tags.push(t.clone());
            }
        }
        self.store.data.tags.sort();
        self.store.mark_dirty();
    }

    async fn pause(&mut self, hash: InfoHash, reason: PauseReason) {
        let (Some(sess), Some(h)) = (self.session.clone(), self.handle_for(&hash)) else { return };
        let m = self.store.meta_mut(hash);
        // A user pause overrides queue/limit pauses; automatic pauses never override a user pause.
        if reason == PauseReason::User || m.pause_reason != Some(PauseReason::User) {
            m.pause_reason = Some(reason);
        }
        if reason == PauseReason::User {
            m.force_started = false;
        }
        if !h.is_paused()
            && let Err(e) = sess.pause(&h).await
        {
            tracing::debug!("pause failed: {e:#}");
        }
    }

    async fn resume(&mut self, hash: InfoHash, force: bool) {
        let (Some(sess), Some(h)) = (self.session.clone(), self.handle_for(&hash)) else { return };
        let m = self.store.meta_mut(hash);
        if m.pause_reason == Some(PauseReason::SeedLimit) {
            m.seed_limit_exempt = true;
        }
        m.pause_reason = None;
        m.force_started = force;
        m.inactive_seeding_secs = 0;
        self.resumed_at.insert(hash, Instant::now());
        if h.is_paused()
            && let Err(e) = sess.unpause(&h).await
        {
            self.toast(ToastKind::Error, format!("Could not resume: {e:#}"));
        }
    }

    fn delete(&mut self, hash: InfoHash, with_data: bool) {
        let Some(sess) = self.session.clone() else { return };
        if !with_data
            && let Some(h) = self.handle_for(&hash)
            && let Ok(bytes) = h.with_metadata(|m| m.torrent_bytes.to_vec())
        {
            self.undo_pending.insert(
                hash,
                UndoEntry {
                    bytes,
                    output: h.output_folder().to_path_buf(),
                    only_files: h.only_files(),
                    paused: h.is_paused(),
                    trackers: h.shared().trackers.iter().map(|u| u.to_string()).collect(),
                    meta: self.store.get(&hash).cloned().unwrap_or_default(),
                    at: Instant::now(),
                },
            );
        }
        self.busy.insert(hash, if with_data { "Deleting files…".into() } else { "Removing…".into() });
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let result = sess
                .delete(TorrentIdOrHash::Hash(Id20::new(hash.0)), with_data)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Internal::Deleted { hash, result });
        });
    }

    // ---------------------------------------------------------------- add / preview

    fn start_preview(&mut self, req: u64, source: AddSource) {
        let Some(sess) = self.session.clone() else {
            self.emit(EngineEvent::PreviewReady { req, result: Err("The engine is not running.".into()) });
            return;
        };
        let events = self.events.clone();
        let ctx = self.ctx.clone();
        let tx = self.internal_tx.clone();
        let handle = tokio::spawn(async move {
            let result = preview(&sess, &source).await.map(Box::new).map_err(|e| format!("{e:#}"));
            let _ = events.send(EngineEvent::PreviewReady { req, result });
            let _ = tx.send(Internal::PreviewFinished { req });
            ctx.request_repaint();
        });
        self.previews.insert(req, handle);
    }

    fn start_add(&mut self, req: Box<AddRequest>) {
        let Some(sess) = self.session.clone() else {
            self.toast(ToastKind::Error, "The engine is not running; cannot add torrents.");
            return;
        };
        let pending = self.next_pending;
        self.next_pending += 1;
        let hash = match &req.source {
            AddSource::Magnet(m) => librqbit::Magnet::parse(m)
                .ok()
                .and_then(|m| m.as_id20())
                .map(|id| InfoHash(id.0))
                .unwrap_or_else(|| pending_hash(pending)),
            _ => pending_hash(pending),
        };
        self.pending.insert(
            pending,
            PendingAdd { name: req.source.describe(), hash, started: now_unix(), save_path: req.save_path.clone(), task: None },
        );
        let tx = self.internal_tx.clone();
        let peer_limit = Some(self.settings.connection.max_peers_per_torrent).filter(|n| *n > 0);
        let task = tokio::spawn(async move {
            let result = add(&sess, &req, peer_limit).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Internal::AddFinished { pending, req, result });
        });
        if let Some(p) = self.pending.get_mut(&pending) {
            p.task = Some(task);
        }
    }

    fn start_readd(&mut self, hash: InfoHash, move_to: Option<PathBuf>, trackers: Option<Vec<String>>, what: &'static str) {
        let (Some(sess), Some(h)) = (self.session.clone(), self.handle_for(&hash)) else { return };
        if self.busy.contains_key(&hash) {
            return;
        }
        let label = match what {
            "move" => "Moving files…",
            "recheck" => "Preparing check…",
            _ => "Updating trackers…",
        };
        self.busy.insert(hash, label.into());
        let multi = self.static_info.get(&hash).map(|s| s.multi_file).unwrap_or(false);
        let backup_dir = self.paths.torrents_dir();
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let result = readd(&sess, h, multi, move_to, trackers, backup_dir).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Internal::OpFinished { hash, what, result });
        });
    }

    /// Drop queued entries from the engine database. Only call while no session is running.
    fn purge_pending(&mut self) {
        if self.store.data.purge.is_empty() {
            return;
        }
        let set: HashSet<InfoHash> = self.store.data.purge.drain(..).collect();
        super::restore::purge(&self.paths.session_dir(), &set);
        self.store.mark_dirty();
        self.store.maybe_save(true);
    }

    fn detect_failed_restores(&mut self) {
        self.restore_checked = false;
        let Some(sess) = self.session.clone() else { return };
        let dir = self.paths.session_dir();
        let tx = self.internal_tx.clone();
        let skip: HashSet<InfoHash> = self.store.data.purge.iter().copied().collect();
        tokio::spawn(async move {
            let mut failed = Vec::new();
            for mut f in super::restore::persisted_entries(&dir) {
                if skip.contains(&f.hash) || sess.get(TorrentIdOrHash::Hash(Id20::new(f.hash.0))).is_some() {
                    continue;
                }
                // One retry captures the actual error (and fixes transient failures).
                match super::restore::retry(&sess, &dir, &f).await {
                    Ok(()) => tracing::info!("restored {} on retry", f.hash.to_hex()),
                    Err(e) => {
                        tracing::warn!("torrent {} could not be loaded: {e}", f.hash.to_hex());
                        f.error = e;
                        failed.push(f);
                    }
                }
            }
            let _ = tx.send(Internal::RestoreReport(failed));
        });
    }

    fn retry_failed(&mut self, hash: InfoHash) {
        self.retry_failed_inner(hash, false);
    }

    fn retry_failed_inner(&mut self, hash: InfoHash, quiet: bool) {
        let (Some(sess), Some(f)) = (self.session.clone(), self.failed.get(&hash).cloned()) else { return };
        let dir = self.paths.session_dir();
        let tx = self.internal_tx.clone();
        self.busy.insert(hash, "Retrying…".into());
        tokio::spawn(async move {
            let result = super::restore::retry(&sess, &dir, &f).await;
            let _ = tx.send(Internal::RestoreRetried { hash, result, quiet });
        });
    }

    fn forget_hash(&mut self, hash: &InfoHash) {
        self.store.remove(hash);
        self.static_info.remove(hash);
        self.torrent_history.remove(hash);
        self.seen_unfinished.remove(hash);
        self.last_down_active.remove(hash);
        self.seed_carry_ms.remove(hash);
        self.slow_since.remove(hash);
        self.resumed_at.remove(hash);
    }

    fn maybe_apply_deferred(&mut self) {
        if self.busy.is_empty()
            && let Some(s) = self.deferred_settings.take()
        {
            let _ = self.self_tx.send(EngineCommand::ApplySettings(s));
        }
    }

    fn handle_internal(&mut self, msg: Internal) {
        match msg {
            Internal::PreviewFinished { req } => {
                self.previews.remove(&req);
            }
            Internal::AddFinished { pending, req, result } => {
                self.pending.remove(&pending);
                match result {
                    Ok((hash, name)) => {
                        let top = req.add_to_top;
                        let m = self.store.meta_mut(hash);
                        m.name = name.clone();
                        m.category = req.category.clone();
                        m.tags = req.tags.clone();
                        m.added_at = now_unix();
                        m.pause_reason = req.paused.then_some(PauseReason::User);
                        if let AddSource::Magnet(s) = &req.source {
                            m.source_magnet = Some(s.clone());
                        }
                        if let Some(c) = &req.category {
                            self.store.data.categories.entry(c.clone()).or_default();
                        }
                        let tags = req.tags.clone();
                        self.register_tags(&tags);
                        if top {
                            self.store.queue_move(hash, crate::store::QueueMove::Top);
                        } else {
                            self.store.queue_move(hash, crate::store::QueueMove::Bottom);
                        }
                        if let AddSource::File(p) = &req.source
                            && self.settings.downloads.delete_torrent_file_after_add
                        {
                            let _ = std::fs::remove_file(p);
                        }
                        self.store.maybe_save(true);
                        self.emit(EngineEvent::Added { hash, name });
                    }
                    Err(e) => {
                        self.toast(ToastKind::Error, format!("Could not add {}: {e}", req.source.describe()));
                    }
                }
            }
            Internal::OpFinished { hash, what, result } => {
                self.busy.remove(&hash);
                self.static_info.remove(&hash);
                self.maybe_apply_deferred();
                match result {
                    Ok(()) => {
                        let msg = match what {
                            "move" => "Files moved. Verifying data…",
                            "recheck" => "Re-checking data…",
                            _ => "Trackers added. Verifying data…",
                        };
                        self.toast(ToastKind::Info, msg);
                    }
                    Err(e) => self.toast(ToastKind::Error, format!("Operation failed: {e}")),
                }
            }
            Internal::UndoFinished { hash, meta, result } => {
                self.busy.remove(&hash);
                match result {
                    Ok(()) => {
                        let name = meta.display_name.clone().unwrap_or(meta.name.clone());
                        // Bring back category, tags, dates, queue position and totals.
                        *self.store.meta_mut(hash) = *meta;
                        self.store.normalize_queue();
                        self.store.maybe_save(true);
                        self.emit(EngineEvent::Restored { hash, name });
                    }
                    Err(e) => self.toast(ToastKind::Error, format!("Undo failed: {e}")),
                }
            }
            Internal::RestoreReport(list) => {
                if !list.is_empty() {
                    let n = list.len();
                    self.toast(
                        ToastKind::Error,
                        if n == 1 { "A torrent could not be loaded — see its error in the list.".to_string() } else { format!("{n} torrents could not be loaded — see their errors in the list.") },
                    );
                }
                self.failed = list.into_iter().map(|f| (f.hash, f)).collect();
                self.restore_checked = true;
            }
            Internal::RestoreRetried { hash, result, quiet } => {
                self.busy.remove(&hash);
                match result {
                    Ok(()) => {
                        self.failed.remove(&hash);
                        self.toast(ToastKind::Success, "Torrent loaded.");
                    }
                    Err(e) => {
                        if let Some(f) = self.failed.get_mut(&hash) {
                            f.error = e;
                        }
                        if !quiet {
                            self.toast(ToastKind::Error, "Still can't load the torrent — see the error in the list.");
                        }
                    }
                }
            }
            Internal::Deleted { hash, result } => {
                self.busy.remove(&hash);
                let gone = self.handle_for(&hash).is_none();
                if let Err(e) = &result {
                    self.toast(ToastKind::Error, format!("Delete failed: {e}"));
                }
                // librqbit forgets the torrent before touching files, so it may be gone despite an error.
                if result.is_ok() || gone {
                    let name = self.store.get(&hash).map(|m| m.display_name.clone().unwrap_or(m.name.clone())).unwrap_or_default();
                    self.forget_hash(&hash);
                    self.store.maybe_save(true);
                    let undoable = match self.undo_pending.remove(&hash) {
                        Some(mut e) => {
                            e.at = Instant::now();
                            self.undo.insert(hash, e);
                            true
                        }
                        None => false,
                    };
                    self.emit(EngineEvent::Removed { hash, name, undoable });
                } else {
                    self.undo_pending.remove(&hash);
                }
                self.maybe_apply_deferred();
            }
        }
    }

    // ---------------------------------------------------------------- periodic work

    fn tick(&mut self) {
        let elapsed = self.last_tick.elapsed();
        self.last_tick = Instant::now();
        self.tick_count += 1;

        self.run_schedule();
        if self.tick_count % 20 == 5 {
            self.undo.retain(|_, e| e.at.elapsed() <= UNDO_WINDOW);
        }
        // Files blocked by antivirus or another program are often released later.
        if self.tick_count % 120 == 60 && !self.failed.is_empty() {
            let hashes: Vec<InfoHash> = self.failed.keys().filter(|h| !self.busy.contains_key(h)).copied().collect();
            for h in hashes {
                self.retry_failed_inner(h, true);
            }
        }

        if self.tick_count % 20 == 1 {
            self.free_space = free_space(&self.settings.downloads.save_path);
        }
        if self.tick_count % 6 == 3 && !self.settings.watch.is_empty() && self.session.is_some() {
            for hit in self.watch.poll(&self.settings.watch) {
                let category = hit.folder.category.clone();
                let save_path = hit
                    .folder
                    .save_path
                    .clone()
                    .or_else(|| category.as_ref().and_then(|c| self.store.data.categories.get(c)).and_then(|c| c.save_path.clone()))
                    .unwrap_or_else(|| self.settings.downloads.save_path.clone());
                tracing::info!("watch folder: adding {}", hit.source.describe());
                self.start_add(Box::new(AddRequest {
                    source: hit.source,
                    torrent_bytes: None,
                    save_path,
                    create_subfolder: self.settings.downloads.create_subfolder,
                    category,
                    tags: Vec::new(),
                    paused: hit.folder.start_paused,
                    only_files: None,
                    add_to_top: false,
                }));
            }
        }

        let Some(sess) = self.session.clone() else {
            self.publish();
            return;
        };

        let handles: Vec<(usize, Arc<ManagedTorrent>)> =
            sess.with_torrents(|it| it.map(|(id, h)| (id, h.clone())).collect());

        let now = now_unix();
        let mut views = Vec::with_capacity(handles.len() + self.pending.len());
        let mut total_du = 0u64;
        let mut total_dd = 0u64;
        let mut present = HashSet::with_capacity(handles.len());

        for (id, h) in &handles {
            let hash = InfoHash(h.info_hash().0);
            present.insert(hash);
            let v = self.build_view(*id, h, hash, now, elapsed, &mut total_du, &mut total_dd);
            views.push(v);
        }

        for f in self.failed.values() {
            if !present.contains(&f.hash) {
                present.insert(f.hash);
                views.push(failed_view(f, self.store.get(&f.hash)));
            }
        }

        // Torrents being resolved (magnets) are shown as placeholder rows.
        for p in self.pending.values() {
            if present.contains(&p.hash) {
                continue;
            }
            views.push(pending_view(p));
        }

        if self.tick_count % 20 == 10 && self.engine_state == EngineState::Running && self.restore_checked {
            let pending: HashSet<InfoHash> = self.pending.values().map(|p| p.hash).collect();
            let orphans: Vec<InfoHash> = self
                .store
                .data
                .torrents
                .keys()
                .filter(|h| !present.contains(h) && !pending.contains(h) && !self.busy.contains_key(h))
                .copied()
                .collect();
            if !orphans.is_empty() {
                tracing::info!("pruning {} stale store entries", orphans.len());
                for h in orphans {
                    self.forget_hash(&h);
                }
                self.store.normalize_queue();
            }
        }

        self.store.data.stats.all_time_downloaded += total_dd;
        self.store.data.stats.all_time_uploaded += total_du;
        self.session_down_total += total_dd;
        self.session_up_total += total_du;

        if self.settings.queue.enabled && self.tick_count % 4 == 0 {
            self.run_queue(&views, &handles);
        }
        if self.tick_count % 4 == 2 {
            self.run_seed_limits(&views);
        }

        // Session-wide stats.
        let st = sess.stats_snapshot();
        let down = st.download_speed.as_bytes();
        let up = st.upload_speed.as_bytes();
        if self.history.len() >= HISTORY_LEN {
            self.history.pop_front();
        }
        self.history.push_back((down, up));

        let detail = self.detail_target.and_then(|t| {
            let (id, h) = handles.iter().find(|(_, h)| h.info_hash().0 == t.0)?;
            Some(self.build_detail(*id, h, t))
        });

        let session_view = SessionView {
            down_bps: down,
            up_bps: up,
            dht_nodes: sess.get_dht().map(|d| d.stats().routing_table_size),
            listen_port: sess.announce_port().or_else(|| sess.listen_addr().map(|a| a.port())),
            peers_live: st.peers.live,
            alt_speed: self.alt,
            uptime_secs: st.uptime_seconds,
            session_downloaded: self.session_down_total,
            session_uploaded: self.session_up_total,
        };

        self.generation += 1;
        let snap = Snapshot {
            torrents: views,
            detail,
            session: session_view,
            engine_state: self.engine_state.clone(),
            history: self.history.clone(),
            generation: self.generation,
            categories: self.store.data.categories.clone(),
            tags: self.store.data.tags.clone(),
            all_time_downloaded: self.store.data.stats.all_time_downloaded,
            all_time_uploaded: self.store.data.stats.all_time_uploaded,
            free_space: self.free_space,
            warning: self.warning.clone(),
        };
        self.snapshot.store(Arc::new(snap));
        self.store.maybe_save(false);
        self.ctx.request_repaint();
    }

    #[allow(clippy::too_many_arguments)]
    fn build_view(
        &mut self,
        id: usize,
        h: &Arc<ManagedTorrent>,
        hash: InfoHash,
        now: i64,
        elapsed: Duration,
        total_du: &mut u64,
        total_dd: &mut u64,
    ) -> TorrentView {
        let stats = h.stats();
        let has_metadata = h.with_metadata(|_| ()).is_ok();
        if has_metadata && !self.static_info.contains_key(&hash) {
            self.static_info.insert(hash, static_info(h));
        }
        let si = self.static_info.get(&hash).cloned().unwrap_or_default();

        let (down_bps, up_bps, peers_live, peers_seen, peers_connecting, fetched) = match &stats.live {
            Some(l) => (
                l.download_speed.as_bytes(),
                l.upload_speed.as_bytes(),
                l.snapshot.peer_stats.live,
                l.snapshot.peer_stats.seen,
                l.snapshot.peer_stats.connecting + l.snapshot.peer_stats.queued,
                Some(l.snapshot.fetched_bytes),
            ),
            None => (0, 0, 0, 0, 0, None),
        };

        let busy = self.busy.get(&hash).cloned();
        let is_live = matches!(stats.state, TorrentStatsState::Live);
        if is_live && !stats.finished {
            self.seen_unfinished.insert(hash);
        }
        if down_bps > 0 || !is_live {
            self.last_down_active.insert(hash, Instant::now());
        }
        let stalled = is_live
            && !stats.finished
            && down_bps == 0
            && self.last_down_active.get(&hash).map(|t| t.elapsed() > STALL_AFTER).unwrap_or(false);

        let name_engine = h.name();
        let m = self.store.meta_quiet(hash);
        let mut structural = false;
        let mut counters = false;
        if let Some(f) = fetched {
            let (du, dd) = m.absorb_counters(stats.uploaded_bytes, f);
            *total_du += du;
            *total_dd += dd;
            counters = du > 0 || dd > 0;
        }
        if let Some(n) = &name_engine
            && m.name != *n
        {
            m.name = n.clone();
            structural = true;
        }
        if down_bps > 0 || up_bps > 0 {
            m.last_activity = now;
        }
        if up_bps > 0 {
            m.last_upload_at = now;
        }
        let mut completed_event = false;
        if stats.finished && has_metadata {
            if m.completed_at.is_none() {
                m.completed_at = Some(now);
                completed_event = self.seen_unfinished.contains(&hash);
                structural = true;
            }
        } else if is_live && !stats.finished && m.completed_at.is_some() {
            // More files were selected after completion.
            m.completed_at = None;
            structural = true;
        }

        let state = super::logic::map_state(&super::logic::StateInput {
            status: match &stats.state {
                TorrentStatsState::Error => super::logic::EngineStatus::Error,
                TorrentStatsState::Initializing { .. } => super::logic::EngineStatus::Initializing,
                TorrentStatsState::Paused => super::logic::EngineStatus::Paused,
                TorrentStatsState::Live => super::logic::EngineStatus::Live,
            },
            finished: stats.finished,
            busy: busy.is_some(),
            stalled,
            pause_reason: m.pause_reason,
        });

        if state == TorrentState::Seeding {
            let carry = self.seed_carry_ms.entry(hash).or_insert(0);
            *carry += elapsed.as_millis() as u64;
            if *carry >= 1000 {
                let secs = *carry / 1000;
                m.seeding_seconds += secs;
                // Inactivity only accrues while actually seeding, never while Corrent is closed.
                m.inactive_seeding_secs = if up_bps > 0 { 0 } else { m.inactive_seeding_secs + secs };
                *carry %= 1000;
                counters = true;
            }
        }
        if structural {
            self.store.mark_dirty();
        } else if counters {
            self.store.mark_soft_dirty();
        }
        let m = self.store.meta_quiet(hash);

        let size = stats.total_bytes;
        let done = stats.progress_bytes.min(size);
        let progress = if size > 0 { done as f32 / size as f32 } else { 0.0 };
        let remaining = size.saturating_sub(done);
        let eta = (down_bps > 0 && remaining > 0 && !stats.finished).then(|| Duration::from_secs(remaining.div_ceil(down_bps)));
        let full_size = si_total(h).unwrap_or(size);
        let ratio = if full_size > 0 { m.uploaded_total as f32 / full_size as f32 } else { 0.0 };

        let output = h.output_folder().to_path_buf();
        let (save_path, content_path) = if si.multi_file {
            (output.parent().map(Path::to_path_buf).unwrap_or_else(|| output.clone()), output.clone())
        } else {
            let first = h
                .with_metadata(|md| md.file_infos.first().map(|f| f.relative_filename.clone()))
                .ok()
                .flatten();
            (output.clone(), first.map(|f| output.join(f)).unwrap_or_else(|| output.clone()))
        };

        let name = m
            .display_name
            .clone()
            .or(name_engine)
            .filter(|n| !n.is_empty())
            .or_else(|| (!m.name.is_empty()).then(|| m.name.clone()))
            .unwrap_or_else(|| hash.to_hex());

        let view = TorrentView {
            id,
            hash,
            name: name.clone(),
            state,
            error: stats.error.clone(),
            size,
            done,
            progress,
            uploaded: m.uploaded_total,
            downloaded: m.downloaded_total,
            down_bps,
            up_bps,
            eta,
            ratio,
            peers_live,
            peers_seen,
            peers_connecting,
            save_path,
            content_path: content_path.clone(),
            category: m.category.clone(),
            tags: m.tags.clone(),
            added_at: m.added_at,
            completed_at: m.completed_at,
            queue_pos: m.queue_pos,
            trackers: h.shared().trackers.iter().map(|u| u.to_string()).collect(),
            has_metadata,
            file_count: si.file_count,
            piece_count: si.piece_count,
            piece_size: si.piece_size,
            comment: si.comment.clone(),
            created_by: si.created_by.clone(),
            creation_date: si.creation_date,
            busy,
            last_activity: m.last_activity,
            seeding_secs: m.seeding_seconds,
            pending: false,
            restore_failed: false,
        };

        let hist = self.torrent_history.entry(hash).or_default();
        if hist.len() >= HISTORY_LEN {
            hist.pop_front();
        }
        hist.push_back((down_bps, up_bps));

        if completed_event {
            let b = &self.settings.behavior;
            if b.run_on_complete_enabled && !b.run_on_complete.trim().is_empty() {
                super::automation::run_on_complete(&b.run_on_complete, &view);
            }
            let save = view.save_path.clone();
            self.emit(EngineEvent::Completed { hash, name, content_path, save_path: save, size });
        }
        view
    }

    fn build_detail(&mut self, id: usize, h: &Arc<ManagedTorrent>, hash: InfoHash) -> TorrentDetail {
        let stats = h.stats();
        let only: Option<HashSet<usize>> = h.only_files().map(|v| v.into_iter().collect());
        let files = h
            .with_metadata(|md| {
                md.file_infos
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !f.attrs.padding)
                    .map(|(i, f)| {
                        let components: Vec<String> = f
                            .relative_filename
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .collect();
                        FileView {
                            index: i,
                            path: f.relative_filename.to_string_lossy().into_owned(),
                            components,
                            len: f.len,
                            done: stats.file_progress.get(i).copied().unwrap_or(0),
                            included: only.as_ref().map(|s| s.contains(&i)).unwrap_or(true),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let interval = self.settings.advanced.refresh_interval_ms.max(200) as f64 / 1000.0;
        let mut peers = Vec::new();
        if let Some(live) = h.live() {
            let snap = live.per_peer_stats_snapshot(Default::default());
            let mut next_prev = HashMap::with_capacity(snap.peers.len());
            for (addr, p) in snap.peers {
                let (pf, pu) = self.prev_peer.get(&addr).copied().unwrap_or((p.counters.fetched_bytes, p.counters.uploaded_bytes));
                let down_bps = (p.counters.fetched_bytes.saturating_sub(pf) as f64 / interval) as u64;
                let up_bps = (p.counters.uploaded_bytes.saturating_sub(pu) as f64 / interval) as u64;
                next_prev.insert(addr.clone(), (p.counters.fetched_bytes, p.counters.uploaded_bytes));
                peers.push(PeerView {
                    addr,
                    client: p.client_name.clone(),
                    kind: p.conn_kind.map(|k| format!("{k:?}")).unwrap_or_default(),
                    state: p.state.to_string(),
                    downloaded: p.counters.fetched_bytes,
                    uploaded: p.counters.uploaded_bytes,
                    down_bps,
                    up_bps,
                });
            }
            self.prev_peer = next_prev;
        }

        let refresh_pieces = match &self.pieces_cache {
            Some((t, ph, _)) => *ph != hash || t.elapsed() > Duration::from_secs(2),
            None => true,
        };
        if refresh_pieces
            && let Some(api) = &self.api
            && let Ok((bf, n)) = api.api_dump_haves(TorrentIdOrHash::Id(id))
        {
            self.pieces_cache = Some((Instant::now(), hash, (bf.as_raw_slice().to_vec(), n)));
        }
        let pieces = self.pieces_cache.as_ref().filter(|(_, ph, _)| *ph == hash).map(|(_, _, p)| p.clone());

        TorrentDetail {
            hash,
            files,
            peers,
            pieces,
            history: self.torrent_history.get(&hash).cloned().unwrap_or_default(),
        }
    }

    fn publish(&mut self) {
        // Re-publish the last snapshot with fresh engine state / categories after a command.
        let old = self.snapshot.load_full();
        let mut snap = (*old).clone();
        snap.engine_state = self.engine_state.clone();
        snap.categories = self.store.data.categories.clone();
        snap.tags = self.store.data.tags.clone();
        snap.session.alt_speed = self.alt;
        snap.warning = self.warning.clone();
        for t in &mut snap.torrents {
            if let Some(m) = self.store.get(&t.hash) {
                t.category = m.category.clone();
                t.tags = m.tags.clone();
                t.queue_pos = m.queue_pos;
                if let Some(n) = &m.display_name {
                    t.name = n.clone();
                }
            }
            t.busy = self.busy.get(&t.hash).cloned();
        }
        let present: HashSet<InfoHash> = snap.torrents.iter().map(|t| t.hash).collect();
        for p in self.pending.values() {
            if !present.contains(&p.hash) {
                snap.torrents.push(pending_view(p));
            }
        }
        snap.torrents.retain(|t| !t.pending || self.pending.values().any(|p| p.hash == t.hash));
        self.generation += 1;
        snap.generation = self.generation;
        self.snapshot.store(Arc::new(snap));
        self.ctx.request_repaint();
    }

    // ---------------------------------------------------------------- automation

    fn run_schedule(&mut self) {
        let sch = &self.settings.speed.schedule;
        if !sch.enabled {
            self.last_schedule_state = None;
            return;
        }
        let (minutes, weekday) = crate::util::local_minutes_and_weekday();
        let want = super::logic::in_schedule(sch, minutes, weekday);
        // Only act on transitions so a manual toggle holds until the next boundary.
        if self.last_schedule_state != Some(want) {
            self.last_schedule_state = Some(want);
            if self.alt != want {
                self.alt = want;
                self.apply_rate_limits();
                self.toast(
                    ToastKind::Info,
                    if want { "Scheduled alternative speed limits enabled." } else { "Scheduled alternative speed limits ended." },
                );
            }
        }
    }

    fn run_queue(&mut self, views: &[TorrentView], handles: &[(usize, Arc<ManagedTorrent>)]) {
        let q = self.settings.queue.clone();
        let Some(sess) = self.session.clone() else { return };
        // Track how long each running torrent has been slow (time-dependent input to the planner).
        for v in views {
            if super::logic::is_slow_now(v, &q) {
                self.slow_since.entry(v.hash).or_insert_with(Instant::now);
            } else {
                self.slow_since.remove(&v.hash);
            }
        }
        let items: Vec<super::logic::QueueItem<'_>> = views
            .iter()
            .filter_map(|v| {
                let meta = self.store.get(&v.hash)?;
                Some(super::logic::QueueItem {
                    view: v,
                    meta,
                    slow_for: self.slow_since.get(&v.hash).map(|t| t.elapsed()).unwrap_or_default(),
                    since_resume: self.resumed_at.get(&v.hash).map(|t| t.elapsed()),
                })
            })
            .collect();
        let plan = super::logic::plan_queue(&q, items);
        let (to_pause, to_resume) = (plan.pause, plan.resume);
        if to_pause.is_empty() && to_resume.is_empty() {
            return;
        }
        for hash in &to_pause {
            self.store.meta_mut(*hash).pause_reason = Some(PauseReason::Queue);
        }
        for hash in &to_resume {
            self.store.meta_mut(*hash).pause_reason = None;
            self.resumed_at.insert(*hash, Instant::now());
        }
        let find = |hash: &InfoHash| handles.iter().find(|(_, h)| h.info_hash().0 == hash.0).map(|(_, h)| h.clone());
        let pause: Vec<_> = to_pause.iter().filter_map(find).collect();
        let resume: Vec<_> = to_resume.iter().filter_map(find).collect();
        tokio::spawn(async move {
            for h in pause {
                let _ = sess.pause(&h).await;
            }
            for h in resume {
                let _ = sess.unpause(&h).await;
            }
        });
    }

    fn run_seed_limits(&mut self, views: &[TorrentView]) {
        let s = self.settings.seeding.clone();
        let actions: Vec<(InfoHash, String, String)> = views
            .iter()
            .filter_map(|v| {
                let m = self.store.get(&v.hash)?;
                super::logic::seed_limit_reason(v, m, &s).map(|r| (v.hash, v.name.clone(), r))
            })
            .collect();
        for (hash, name, why) in actions {
            match s.action {
                SeedAction::Pause => {
                    self.store.meta_mut(hash).pause_reason = Some(PauseReason::SeedLimit);
                    if let (Some(sess), Some(h)) = (self.session.clone(), self.handle_for(&hash)) {
                        tokio::spawn(async move {
                            let _ = sess.pause(&h).await;
                        });
                    }
                    self.toast(ToastKind::Info, format!("Stopped seeding “{name}”: {why}."));
                }
                SeedAction::Remove | SeedAction::RemoveWithData => {
                    self.delete(hash, s.action == SeedAction::RemoveWithData);
                    self.toast(ToastKind::Info, format!("Removed “{name}”: {why}."));
                }
            }
        }
    }
}

fn pending_hash(n: u64) -> InfoHash {
    let mut h = [0xffu8; 20];
    h[12..20].copy_from_slice(&n.to_be_bytes());
    InfoHash(h)
}

fn pending_view(p: &PendingAdd) -> TorrentView {
    TorrentView {
        id: usize::MAX,
        hash: p.hash,
        name: p.name.clone(),
        state: TorrentState::Metadata,
        error: None,
        size: 0,
        done: 0,
        progress: 0.0,
        uploaded: 0,
        downloaded: 0,
        down_bps: 0,
        up_bps: 0,
        eta: None,
        ratio: 0.0,
        peers_live: 0,
        peers_seen: 0,
        peers_connecting: 0,
        save_path: p.save_path.clone(),
        content_path: p.save_path.clone(),
        category: None,
        tags: Vec::new(),
        added_at: p.started,
        completed_at: None,
        queue_pos: u32::MAX,
        trackers: Vec::new(),
        has_metadata: false,
        file_count: 0,
        piece_count: 0,
        piece_size: 0,
        comment: None,
        created_by: None,
        creation_date: None,
        busy: Some("Fetching metadata…".into()),
        last_activity: 0,
        seeding_secs: 0,
        pending: true,
        restore_failed: false,
    }
}

fn failed_view(f: &super::restore::FailedRestore, m: Option<&crate::store::TorrentMeta>) -> TorrentView {
    let name = m
        .and_then(|m| m.display_name.clone().or_else(|| (!m.name.is_empty()).then(|| m.name.clone())))
        .unwrap_or_else(|| f.hash.to_hex());
    let mut v = pending_view(&PendingAdd {
        name,
        hash: f.hash,
        started: m.map(|m| m.added_at).unwrap_or(0),
        save_path: f.output_folder.clone(),
        task: None,
    });
    v.state = TorrentState::Error;
    v.error = Some(f.error.clone());
    v.busy = None;
    v.pending = false;
    v.restore_failed = true;
    v.id = f.id;
    if let Some(m) = m {
        v.category = m.category.clone();
        v.tags = m.tags.clone();
        v.queue_pos = m.queue_pos;
        v.uploaded = m.uploaded_total;
        v.downloaded = m.downloaded_total;
        v.completed_at = m.completed_at;
    }
    v
}

fn si_total(h: &ManagedTorrent) -> Option<u64> {
    h.with_metadata(|m| m.info.lengths().total_length()).ok()
}

fn static_info(h: &ManagedTorrent) -> StaticInfo {
    h.with_metadata(|m| {
        let parsed = librqbit::torrent_from_bytes(&m.torrent_bytes).ok();
        let text = |b: &Option<librqbit::ByteBuf<'_>>| {
            b.as_ref().map(|b| String::from_utf8_lossy(b.as_ref()).into_owned()).filter(|s| !s.trim().is_empty())
        };
        StaticInfo {
            file_count: m.file_infos.iter().filter(|f| !f.attrs.padding).count(),
            piece_count: m.info.lengths().total_pieces(),
            piece_size: m.info.lengths().default_piece_length() as u64,
            multi_file: m.info.info().files.is_some(),
            comment: parsed.as_ref().and_then(|p| text(&p.comment)),
            created_by: parsed.as_ref().and_then(|p| text(&p.created_by)),
            creation_date: parsed.as_ref().and_then(|p| p.creation_date.map(|d| d as i64)),
        }
    })
    .unwrap_or_default()
}

/// Turn any add source into .torrent bytes, resolving magnets and URLs via the session.
async fn fetch_torrent_bytes(sess: &Arc<Session>, source: &AddSource) -> anyhow::Result<Vec<u8>> {
    match source {
        AddSource::File(p) => tokio::fs::read(p).await.with_context(|| format!("reading {}", p.display())),
        AddSource::Bytes { bytes, .. } => Ok(bytes.clone()),
        AddSource::Magnet(u) | AddSource::Url(u) => {
            let resp = sess
                .add_torrent(
                    AddTorrent::from_url(u.as_str()),
                    Some(AddTorrentOptions { list_only: true, ..Default::default() }),
                )
                .await?;
            match resp {
                AddTorrentResponse::ListOnly(l) => Ok(l.torrent_bytes.to_vec()),
                AddTorrentResponse::AlreadyManaged(_, h) | AddTorrentResponse::Added(_, h) => {
                    Ok(h.with_metadata(|m| m.torrent_bytes.to_vec())?)
                }
            }
        }
    }
}

async fn list_only(sess: &Arc<Session>, bytes: Vec<u8>) -> anyhow::Result<librqbit::ListOnlyResponse> {
    match sess
        .add_torrent(
            AddTorrent::from_bytes(bytes),
            Some(AddTorrentOptions { list_only: true, ..Default::default() }),
        )
        .await?
    {
        AddTorrentResponse::ListOnly(l) => Ok(l),
        _ => anyhow::bail!("unexpected response from engine"),
    }
}

async fn preview(sess: &Arc<Session>, source: &AddSource) -> anyhow::Result<Preview> {
    let bytes = fetch_torrent_bytes(sess, source).await?;
    let l = list_only(sess, bytes).await?;
    let info_hash = InfoHash(l.info_hash.0);
    let name = l.info.name().map(|n| n.into_owned()).unwrap_or_else(|| info_hash.to_hex());
    let multi = l.info.info().files.is_some();
    let files: Vec<PreviewFile> = l
        .info
        .iter_file_details()
        .map(|f| {
            let components = f.filename.to_vec();
            PreviewFile { path: components.join("/"), components, len: f.len }
        })
        .collect();
    let parsed = librqbit::torrent_from_bytes(&l.torrent_bytes).ok();
    let comment = parsed
        .as_ref()
        .and_then(|p| p.comment.as_ref().map(|c| String::from_utf8_lossy(c.as_ref()).into_owned()))
        .filter(|s| !s.trim().is_empty());
    Ok(Preview {
        info_hash,
        name,
        total_size: l.info.lengths().total_length(),
        piece_count: l.info.lengths().total_pieces(),
        piece_length: l.info.lengths().default_piece_length(),
        files,
        torrent_bytes: l.torrent_bytes.to_vec(),
        multi_file: multi,
        already_added: sess.get(TorrentIdOrHash::Hash(l.info_hash)).is_some(),
        comment,
    })
}

pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() { "torrent".into() } else { trimmed }
}

async fn add(sess: &Arc<Session>, req: &AddRequest, peer_limit: Option<usize>) -> anyhow::Result<(InfoHash, String)> {
    let bytes = match &req.torrent_bytes {
        Some(b) => b.clone(),
        None => fetch_torrent_bytes(sess, &req.source).await?,
    };
    let l = list_only(sess, bytes.clone()).await?;
    let hash = InfoHash(l.info_hash.0);
    let name = l.info.name().map(|n| n.into_owned()).unwrap_or_else(|| hash.to_hex());
    if sess.get(TorrentIdOrHash::Hash(l.info_hash)).is_some() {
        anyhow::bail!("“{name}” is already in your list");
    }
    let multi = l.info.info().files.is_some();
    // Trackers from the magnet link (tr=…) must reach the engine even when we add from bytes.
    let magnet_trackers: Vec<String> = match &req.source {
        AddSource::Magnet(m) => librqbit::Magnet::parse(m).map(|m| m.trackers).unwrap_or_default(),
        _ => Vec::new(),
    };
    let output = if multi && req.create_subfolder { req.save_path.join(sanitize_name(&name)) } else { req.save_path.clone() };
    tokio::fs::create_dir_all(&output).await.with_context(|| format!("creating {}", output.display()))?;
    let opts = AddTorrentOptions {
        paused: req.paused,
        only_files: req.only_files.clone(),
        output_folder: Some(output.to_string_lossy().into_owned()),
        overwrite: true,
        peer_limit,
        trackers: (!magnet_trackers.is_empty()).then_some(magnet_trackers),
        ..Default::default()
    };
    match sess.add_torrent(AddTorrent::from_bytes(bytes), Some(opts)).await? {
        AddTorrentResponse::Added(_, _) => Ok((hash, name)),
        AddTorrentResponse::AlreadyManaged(_, _) => anyhow::bail!("“{name}” is already in your list"),
        AddTorrentResponse::ListOnly(_) => anyhow::bail!("unexpected response from engine"),
    }
}

/// Forget a torrent and add it back with changed options. librqbit has no runtime API for
/// these changes; the re-add re-verifies existing data but never re-downloads it.
async fn readd(
    sess: &Arc<Session>,
    h: Arc<ManagedTorrent>,
    multi: bool,
    move_to: Option<PathBuf>,
    extra_trackers: Option<Vec<String>>,
    backup_dir: PathBuf,
) -> anyhow::Result<()> {
    let id = h.id();
    let hash = InfoHash(h.info_hash().0);
    let bytes = h.with_metadata(|m| m.torrent_bytes.to_vec())?;
    let only_files = h.only_files();
    let paused = h.is_paused();
    let old_output = h.output_folder().to_path_buf();
    let mut trackers: Vec<String> = h.shared().trackers.iter().map(|u| u.to_string()).collect();
    if let Some(extra) = extra_trackers {
        for t in extra {
            let t = t.trim().to_string();
            if !t.is_empty() && !trackers.contains(&t) {
                trackers.push(t);
            }
        }
    }
    let new_output = match &move_to {
        Some(dest) if multi => dest.join(old_output.file_name().unwrap_or_default()),
        Some(dest) => dest.clone(),
        None => old_output.clone(),
    };
    if move_to.is_some() && new_output == old_output {
        anyhow::bail!("the torrent is already in that folder");
    }
    let file_list: Vec<PathBuf> = h.with_metadata(|m| m.file_infos.iter().map(|f| f.relative_filename.clone()).collect())?;
    drop(h);
    // Keep a copy of the metainfo until the torrent is safely back in the session.
    let backup = backup_dir.join(format!("{}.torrent", hash.to_hex()));
    let _ = std::fs::create_dir_all(&backup_dir);
    let _ = std::fs::write(&backup, &bytes);
    sess.delete(TorrentIdOrHash::Id(id), false).await?;

    let mut move_err = None;
    if move_to.is_some() {
        let (from, to) = (old_output.clone(), new_output.clone());
        let r = tokio::task::spawn_blocking(move || move_files(&from, &to, &file_list)).await?;
        if let Err(e) = r {
            move_err = Some(e);
        }
    }
    let target = if move_err.is_some() { old_output.clone() } else { new_output };
    let add_at = |output: &Path| {
        let opts = AddTorrentOptions {
            paused,
            only_files: only_files.clone(),
            output_folder: Some(output.to_string_lossy().into_owned()),
            overwrite: true,
            preferred_id: Some(id),
            trackers: Some(trackers.clone()),
            ..Default::default()
        };
        sess.add_torrent(AddTorrent::from_bytes(bytes.clone()), Some(opts))
    };
    if let Err(e) = add_at(&target).await {
        // Never lose the torrent: fall back to its previous location.
        if target != old_output && add_at(&old_output).await.is_ok() {
            let _ = std::fs::remove_file(&backup);
            return Err(e.context("could not use the new location; the torrent was kept where it was"));
        }
        return Err(e.context(format!("the torrent could not be re-added; its metainfo is saved at {}", backup.display())));
    }
    let _ = std::fs::remove_file(&backup);
    if let Some(e) = move_err {
        return Err(e.context("moving files failed; the torrent was kept at its old location"));
    }
    Ok(())
}

/// Move a torrent's files. On failure, files already moved are moved back so the data is never split.
fn move_files(from: &Path, to: &Path, files: &[PathBuf]) -> anyhow::Result<()> {
    fn move_one(src: &Path, dst: &Path) -> anyhow::Result<()> {
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        if std::fs::rename(src, dst).is_err() {
            // Different volume: copy then remove.
            std::fs::copy(src, dst).with_context(|| format!("copying {}", src.display()))?;
            std::fs::remove_file(src)?;
        }
        Ok(())
    }
    std::fs::create_dir_all(to)?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for rel in files {
        let src = from.join(rel);
        if !src.exists() {
            continue;
        }
        let dst = to.join(rel);
        if let Err(e) = move_one(&src, &dst) {
            // A partial copy is discarded; the original is still in place.
            if src.exists() {
                let _ = std::fs::remove_file(&dst);
            }
            for (s, d) in moved.iter().rev() {
                if let Err(e2) = move_one(d, s) {
                    tracing::error!("could not move {} back: {e2:#}", d.display());
                }
            }
            return Err(e);
        }
        moved.push((src, dst));
    }
    // Remove now-empty directories left behind (best effort).
    let mut dirs: Vec<PathBuf> = files.iter().filter_map(|f| f.parent().map(|p| from.join(p))).collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
    if from != to {
        let _ = std::fs::remove_dir(from);
    }
    Ok(())
}

pub(super) fn free_space(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    loop {
        if p.exists() {
            return fs4::available_space(&p).ok();
        }
        if !p.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_name("a/b:c?"), "a_b_c_");
        assert_eq!(sanitize_name("  ..  "), "torrent");
        assert_eq!(sanitize_name("Movie (2020)."), "Movie (2020)");
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn pending_hashes_are_unique_and_marked() {
        assert_ne!(pending_hash(1), pending_hash(2));
        assert_eq!(pending_hash(7).0[0], 0xff);
    }

    #[test]
    fn sanitize_strips_reserved_characters() {
        assert_eq!(sanitize_name("a<b>c|d*e"), "a_b_c_d_e");
        assert_eq!(sanitize_name("tab\there"), "tab_here");
        assert_eq!(sanitize_name("ok name"), "ok name");
    }

    #[test]
    fn kib_limits() {
        assert_eq!(super::super::session::kib_limit(0), None);
        assert_eq!(super::super::session::kib_limit(2).map(|n| n.get()), Some(2048));
        let l = super::super::session::limits(1, 0);
        assert!(l.download_bps.is_some() && l.upload_bps.is_none());
    }
}
