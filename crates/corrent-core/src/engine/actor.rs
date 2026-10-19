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
use super::{AddRequest, EngineCommand, EngineEvent, Preview, PreviewFile, ToastKind, Wake};
use crate::config::{Paths, SeedAction, Settings};
use crate::store::{PauseReason, Store};
use crate::util::now_unix;

const HISTORY_LEN: usize = 600;
const STALL_AFTER: Duration = Duration::from_secs(30);

enum Internal {
    AddFinished {
        pending: u64,
        req: Box<AddRequest>,
        result: Result<(InfoHash, String), String>,
    },
    PreviewFinished {
        req: u64,
    },
    OpFinished {
        hash: InfoHash,
        what: &'static str,
        result: Result<(), String>,
    },
    Deleted {
        hash: InfoHash,
        result: Result<(), String>,
    },
    RestoreReport(Vec<super::restore::FailedRestore>),
    /// `Ok(true)` = re-added; `Ok(false)` = the torrent was already back in the list.
    UndoFinished {
        hash: InfoHash,
        entry: Box<UndoEntry>,
        result: Result<bool, String>,
    },
    RestoreRetried {
        hash: InfoHash,
        result: Result<(), String>,
        quiet: bool,
    },
    RssFetched {
        url: String,
        result: Result<crate::rss::ParsedFeed, String>,
    },
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

/// Downloaded/uploaded byte counters per peer address (for rates).
type PeerCounters = HashMap<String, (u64, u64)>;

pub struct Actor {
    paths: Paths,
    settings: Settings,
    wake: Wake,
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
    row_history: Arc<HashMap<InfoHash, Vec<(u32, u32)>>>,
    prev_peer: HashMap<String, (u64, u64)>,
    /// Peer counters per torrent for on-demand details (remote clients), with the time taken.
    remote_prev: HashMap<InfoHash, (Instant, PeerCounters)>,
    pieces_cache: Option<(Instant, InfoHash, (Vec<u8>, u32))>,
    static_info: HashMap<InfoHash, StaticInfo>,
    previews: HashMap<u64, JoinHandle<()>>,
    pending: HashMap<u64, PendingAdd>,
    next_pending: u64,
    busy: HashMap<InfoHash, String>,
    seen_unfinished: HashSet<InfoHash>,
    /// Torrents first seen in the engine while an add was pending, until their add is recorded.
    unrecorded: HashSet<InfoHash>,
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
    /// Bumped on every resume; spawned pause tasks stop when it changes, so a pause queued just
    /// before a Resume can't undo it.
    resume_epoch: Arc<std::sync::atomic::AtomicU64>,
    deferred_settings: Option<Box<Settings>>,
    failed: HashMap<InfoHash, super::restore::FailedRestore>,
    undo_pending: HashMap<InfoHash, UndoEntry>,
    undo: HashMap<InfoHash, UndoEntry>,
    /// Pruning the store waits until failed restores have been identified.
    restore_checked: bool,
    /// Size of the last refused prune (see `logic::prune_allowed`), so it is reported once.
    prune_refused: Option<usize>,
    /// The engine's list was restored from an older copy this run: torrents added since are
    /// missing from it, so nothing is pruned (their records and metainfo are kept).
    prune_suspended: bool,
    /// The first session start of this process (D14 runs only then).
    first_start: bool,
    last_heartbeat: Option<Instant>,
    low_space_warned: bool,
    stream: Option<Arc<super::stream::StreamServer>>,
    /// Per stream session: last sample (time, bytes served) and smoothed rate.
    stream_rates: HashMap<u64, (Instant, u64, f64)>,
    /// Piece bitfields of torrents being streamed, refreshed about once a second.
    stream_haves: HashMap<InfoHash, (Instant, Vec<u8>, u32)>,
    rss: crate::rss::Rss,
    /// Adds started by feed rules (pending id → article), so a failed add can be retried.
    rss_adds: HashMap<u64, crate::rss::Download>,
    /// Cancels the session's tasks when the actor goes away, even by unwinding after a panic
    /// (`stop_session` only runs on a clean stop).
    session_cancel: tokio_util::sync::DropGuard,
    /// Watch-folder files behind pending adds, with their folder's after-add action.
    watch_adds: HashMap<u64, (PathBuf, crate::config::AfterAdd)>,
    rss_failures: HashMap<String, u32>,
    /// Shared limiters for categories with speed limits.
    category_limits: HashMap<String, Arc<librqbit::limits::Limits>>,
    rss_client: Option<(Option<String>, reqwest::Client)>,
}

impl Actor {
    pub fn new(
        paths: Paths,
        settings: Settings,
        wake: Wake,
        events: std::sync::mpsc::Sender<EngineEvent>,
        snapshot: Arc<ArcSwap<Snapshot>>,
        self_tx: mpsc::UnboundedSender<EngineCommand>,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let store = Store::load(paths.store_file());
        let mut rss = crate::rss::Rss::load(paths.rss_file());
        rss.rebuild(&settings.rss);
        let alt = settings.speed.alt_enabled;
        Actor {
            paths,
            settings,
            wake,
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
            row_history: Default::default(),
            prev_peer: HashMap::new(),
            remote_prev: HashMap::new(),
            pieces_cache: None,
            static_info: HashMap::new(),
            previews: HashMap::new(),
            pending: HashMap::new(),
            next_pending: 1,
            busy: HashMap::new(),
            seen_unfinished: HashSet::new(),
            unrecorded: HashSet::new(),
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
            resume_epoch: Default::default(),
            deferred_settings: None,
            failed: HashMap::new(),
            undo_pending: HashMap::new(),
            undo: HashMap::new(),
            restore_checked: false,
            prune_refused: None,
            prune_suspended: false,
            first_start: true,
            last_heartbeat: None,
            low_space_warned: false,
            stream: None,
            stream_rates: HashMap::new(),
            stream_haves: HashMap::new(),
            rss,
            rss_adds: HashMap::new(),
            session_cancel: tokio_util::sync::CancellationToken::new().drop_guard(),
            watch_adds: HashMap::new(),
            rss_failures: HashMap::new(),
            category_limits: HashMap::new(),
            rss_client: None,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<EngineCommand>) {
        let mut internal_rx = self.internal_rx.take().expect("run called once");
        if let Some(w) = self.store.warning.take() {
            self.emit(EngineEvent::DataWarning(w));
        }
        if let Some(w) = self.rss.warning.take() {
            self.emit(EngineEvent::DataWarning(w));
        }
        match super::stream::StreamServer::start().await {
            Ok(s) => self.stream = Some(s),
            Err(e) => tracing::warn!("streaming server unavailable: {e:#}"),
        }
        self.start_session().await;
        let mut tick =
            tokio::time::interval(Duration::from_millis(self.settings.advanced.refresh_interval_ms.clamp(200, 5000)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    None => { self.shutdown(&mut internal_rx).await; break; }
                    Some(EngineCommand::Shutdown(done)) => {
                        self.shutdown(&mut internal_rx).await;
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
        self.recover_session_db();
        if std::mem::take(&mut self.first_start) {
            self.check_after_power_loss();
        }
        super::session_db::snapshot(&self.paths.session_dir());
        self.purge_pending();
        let mut last_err = String::new();
        for attempt in 0..3 {
            let cancel = tokio_util::sync::CancellationToken::new();
            match session::create(&self.paths, &self.settings, self.alt, &cancel).await {
                Ok((sess, warning)) => {
                    if let Some(w) = &warning {
                        self.toast(ToastKind::Warning, w.clone());
                    }
                    self.warning = warning;
                    self.api = Some(Api::new(sess.clone(), None));
                    if let Some(st) = &self.stream {
                        st.set_session(Some(sess.clone()));
                    }
                    self.session = Some(sess);
                    // The old guard (a stopped or never-started session) fires harmlessly.
                    self.session_cancel = cancel.drop_guard();
                    self.engine_state = EngineState::Running;
                    self.apply_rate_limits();
                    tracing::info!("engine session started");
                    self.detect_failed_restores();
                    self.recover_interrupted_readds();
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
        self.toast(ToastKind::Error, t!("Could not start the torrent engine: {last_err}", last_err = last_err));
        self.publish();
    }

    async fn stop_session(&mut self) {
        self.api = None;
        if let Some(st) = &self.stream {
            st.set_session(None);
        }
        if let Some(sess) = self.session.take() {
            if tokio::time::timeout(Duration::from_secs(6), sess.stop()).await.is_err() {
                tracing::warn!("session stop timed out");
            }
            // Give listeners a moment to release their sockets before a restart binds again.
            drop(sess);
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    async fn shutdown(&mut self, internal: &mut mpsc::UnboundedReceiver<Internal>) {
        tracing::info!("engine shutting down");
        // Let moves/rechecks finish (bounded): a torrent is out of the session while it is
        // re-added. Anything still running is recovered from its backup at the next start.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.busy.is_empty() && Instant::now() < deadline {
            if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(200), internal.recv()).await {
                self.handle_internal(msg);
            }
        }
        // Results that already arrived, e.g. an add that finished just now: without this its
        // category and tags would never be recorded.
        while let Ok(msg) = internal.try_recv() {
            self.handle_internal(msg);
        }
        for (_, h) in self.previews.drain() {
            h.abort();
        }
        self.store.flush();
        self.stop_session().await;
        self.purge_pending();
        // The freshest good copy of the engine's list (the start-time copy covers a crash).
        super::session_db::snapshot(&self.paths.session_dir());
        self.store.flush();
        self.rss.flush();
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
        self.wake.wake();
    }

    fn emit(&self, ev: EngineEvent) {
        let _ = self.events.send(ev);
        self.wake.wake();
    }

    fn handle_for(&self, hash: &InfoHash) -> Option<Arc<ManagedTorrent>> {
        self.session.as_ref()?.get(TorrentIdOrHash::Hash(Id20::new(hash.0)))
    }

    // ---------------------------------------------------------------- commands

    /// Drop hashes that have an operation (move/recheck/delete) in flight.
    fn not_busy(&self, hashes: Vec<InfoHash>) -> Vec<InfoHash> {
        let (ok, busy): (Vec<_>, Vec<_>) = hashes.into_iter().partition(|h| !self.busy.contains_key(h));
        if !busy.is_empty() {
            self.toast(ToastKind::Info, t!("Some torrents are busy with another operation; try again in a moment."));
        }
        ok
    }

    async fn handle(&mut self, cmd: EngineCommand) {
        let cmd = match cmd {
            EngineCommand::Pause(h) => EngineCommand::Pause(self.not_busy(h)),
            EngineCommand::Resume(h) => EngineCommand::Resume(self.not_busy(h)),
            EngineCommand::ForceStart(h) => EngineCommand::ForceStart(self.not_busy(h)),
            EngineCommand::Delete { hashes, with_data } => {
                EngineCommand::Delete { hashes: self.not_busy(hashes), with_data }
            }
            EngineCommand::MoveStorage { hashes, to } => {
                EngineCommand::MoveStorage { hashes: self.not_busy(hashes), to }
            }
            EngineCommand::Recheck(h) => EngineCommand::Recheck(self.not_busy(h)),
            EngineCommand::SetFiles { hash, included } if self.busy.contains_key(&hash) => {
                let _ = included;
                self.toast(ToastKind::Info, t!("This torrent is busy with another operation."));
                return;
            }
            other => other,
        };
        match cmd {
            EngineCommand::CreateTorrent(req) => {
                let sess = self.session.clone();
                let id = req.id;
                let events = self.events.clone();
                let wake = self.wake.clone();
                let self_tx = self.self_tx.clone();
                tokio::spawn(async move {
                    let result = async {
                        let created = super::creator::create(&req).await?;
                        if req.seed {
                            // Seed from where the data already is: the folder itself for a folder
                            // torrent, its parent for a single file.
                            let (save_path, subfolder) = if req.source.is_dir() {
                                (req.source.clone(), false)
                            } else {
                                (super::creator::seed_folder(&req.source), false)
                            };
                            let add = AddRequest {
                                source: AddSource::Bytes { name: created.name.clone(), bytes: created.bytes.clone() },
                                torrent_bytes: Some(created.bytes.clone()),
                                save_path,
                                create_subfolder: subfolder,
                                category: None,
                                tags: Vec::new(),
                                paused: false,
                                only_files: None,
                                add_to_top: false,
                                initial_peers: None,
                                added_by: None,
                            };
                            if sess.is_none() {
                                anyhow::bail!(t!(
                                    "the torrent file was saved, but seeding needs the engine, which isn't running"
                                ));
                            }
                            // The normal add path records added date, name and queue position.
                            let _ = self_tx.send(EngineCommand::Add(Box::new(add)));
                        }
                        anyhow::Ok((
                            created.name,
                            req.save_to.clone(),
                            created.magnet,
                            req.seed.then_some(created.hash),
                        ))
                    }
                    .await
                    .map_err(|e| format!("{e:#}"));
                    let _ = events.send(EngineEvent::Created { id, result });
                    wake.wake();
                });
            }
            EngineCommand::UndoRemove(hashes) => {
                for hash in hashes {
                    let Some(e) = self.undo.remove(&hash) else { continue };
                    if e.at.elapsed() > UNDO_WINDOW {
                        continue;
                    }
                    let Some(sess) = self.session.clone() else {
                        self.undo.insert(hash, e);
                        continue;
                    };
                    let tx = self.internal_tx.clone();
                    let peer_limit = Some(self.settings.connection.max_peers_per_torrent).filter(|n| *n > 0);
                    self.busy.insert(hash, t!("Restoring…").into());
                    tokio::spawn(async move {
                        let opts = AddTorrentOptions {
                            peer_limit,
                            paused: e.paused,
                            only_files: e.only_files.clone(),
                            output_folder: Some(e.output.to_string_lossy().into_owned()),
                            overwrite: true,
                            trackers: (!e.trackers.is_empty()).then(|| e.trackers.clone()),
                            ..Default::default()
                        };
                        let result = match sess.add_torrent(AddTorrent::from_bytes(e.bytes.clone()), Some(opts)).await {
                            Ok(AddTorrentResponse::Added(..)) => Ok(true),
                            Ok(AddTorrentResponse::AlreadyManaged(..)) => Ok(false),
                            Ok(AddTorrentResponse::ListOnly(_)) => Err("unexpected response from engine".to_string()),
                            Err(err) => Err(format!("{err:#}")),
                        };
                        let _ = tx.send(Internal::UndoFinished { hash, entry: Box::new(e), result });
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
                    // Cancelled by the user: keep the file, renamed so it isn't added again.
                    if let Some((path, after)) = self.watch_adds.remove(&id) {
                        self.watch.finish(&path, after, false);
                    }
                }
            }
            EngineCommand::Add(req) => {
                self.start_add(req);
            }
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
                        self.store.mark_dirty();
                        self.emit(EngineEvent::Removed { hash, name: String::new(), undoable: false });
                        if with_data {
                            self.toast(ToastKind::Info, t!("The torrent was removed; its files were left in place because they couldn't be opened."));
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
                        self.toast(ToastKind::Warning, t!("At least one file must stay selected."));
                    } else if let Err(e) = sess.update_only_files(&h, &set).await {
                        self.toast(ToastKind::Error, t!("Could not update file selection: {e}", e = format!("{e:#}")));
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
            EngineCommand::QueuePlace { hashes, before } => self.store.queue_place(&hashes, before),
            EngineCommand::Detail { hash, reply } => {
                let _ = reply.send(self.remote_detail(hash));
            }
            EngineCommand::FileList { hash, reply } => {
                let files = self.handle_for(&hash).map(|h| self.file_views(&h, hash));
                let _ = reply.send(files);
            }
            EngineCommand::SetFilePriority { hash, files, priority } => {
                let m = self.store.meta_mut(hash);
                for f in files {
                    if priority == 0 {
                        m.file_priority.remove(&f);
                    } else {
                        m.file_priority.insert(f, priority.clamp(-1, 1));
                    }
                }
                let pri = m.file_priority.clone();
                if let Some(h) = self.handle_for(&hash) {
                    h.set_file_priorities(file_order(&h, &pri));
                }
                self.pieces_cache = None;
            }
            EngineCommand::SetTorrentLimits { hashes, down_kib, up_kib } => {
                for h in &hashes {
                    let m = self.store.meta_mut(*h);
                    m.down_limit_kib = down_kib;
                    m.up_limit_kib = up_kib;
                    if let Some(t) = self.handle_for(h) {
                        t.set_ratelimits(session::limits(down_kib, up_kib));
                    }
                }
            }
            EngineCommand::StopStream(id) => {
                if let Some(st) = &self.stream
                    && st.stop(id)
                {
                    tracing::info!("stream {id} stopped by the user");
                }
            }
            EngineCommand::AllowStream { hash, file } => {
                if let Some(st) = &self.stream {
                    st.unblock(hash, file);
                }
            }
            EngineCommand::ReloadIpFilter => {
                if self.busy.is_empty() {
                    self.stop_session().await;
                    self.start_session().await;
                    if self.engine_state == EngineState::Running {
                        self.toast(ToastKind::Success, t!("IP filter reloaded."));
                    }
                } else {
                    self.toast(ToastKind::Info, t!("Try again once running operations finish."));
                }
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
                if new.rss != self.settings.rss {
                    self.rss.settings_changed(&new.rss);
                    let rules_changed = new.rss.rules != self.settings.rss.rules
                        || new.rss.auto_download != self.settings.rss.auto_download;
                    if rules_changed {
                        for d in self.rss.apply_rules(&new.rss) {
                            self.rss_add(d, &new);
                        }
                    }
                }
                let restart = self.settings.requires_restart(&new);
                if restart && !self.busy.is_empty() {
                    // Tasks hold the current session; restarting under them could corrupt its state.
                    self.toast(ToastKind::Info, t!("Network settings will apply once running operations finish."));
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
                        self.toast(ToastKind::Success, t!("Settings applied — reconnected."));
                    }
                } else {
                    self.apply_rate_limits();
                }
            }
            EngineCommand::RssRefresh(url) => self.rss.refresh_now(url.as_deref()),
            EngineCommand::RssMarkDownloaded { feed, guid } => {
                self.rss.mark_downloaded(&feed, &guid, &self.settings.rss)
            }
            EngineCommand::SetAltSpeed(on) => {
                self.alt = on;
                self.apply_rate_limits();
            }
            EngineCommand::Shutdown(_) => unreachable!("handled in run loop"),
            EngineCommand::Flush(done) => {
                self.store.flush();
                self.rss.flush();
                let _ = done.send(());
            }
            #[cfg(debug_assertions)]
            EngineCommand::DevPanic => panic!("CORRENT_DEV_PANIC=engine"),
        }
    }

    /// Start fetching feeds that are due.
    fn poll_feeds(&mut self) {
        // Without a session, matches couldn't be added (and would be marked as downloaded).
        if self.settings.rss.feeds.is_empty() || self.session.is_none() {
            return;
        }
        let due = self.rss.due(&self.settings.rss);
        if due.is_empty() {
            return;
        }
        let proxy = self.settings.connection.proxy_url();
        if self.rss_client.as_ref().is_none_or(|(p, _)| *p != proxy) {
            self.rss_client = Some((proxy.clone(), crate::rss::feed::client(proxy)));
        }
        let client = self.rss_client.as_ref().map(|(_, c)| c.clone()).expect("client was just set");
        for url in due {
            let (tx, client, wake) = (self.internal_tx.clone(), client.clone(), self.wake.clone());
            tokio::spawn(async move {
                let result = crate::rss::feed::fetch(&client, &url).await;
                let _ = tx.send(Internal::RssFetched { url, result });
                wake.wake();
            });
        }
        self.publish();
    }

    /// Add an article chosen by a download rule.
    fn rss_add(&mut self, d: crate::rss::Download, settings: &Settings) {
        let link = d.link.clone();
        let source = if link.starts_with("magnet:") { AddSource::Magnet(link) } else { AddSource::Url(link) };
        let category = d.rule.category.clone().filter(|c| !c.is_empty());
        let save_path = d
            .rule
            .save_path
            .clone()
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| {
                category.as_ref().and_then(|c| self.store.data.categories.get(c)).and_then(|c| c.save_path.clone())
            })
            .unwrap_or_else(|| settings.downloads.save_path.clone());
        tracing::info!("rss rule {:?}: adding {:?}", d.rule.name, d.title);
        self.toast(ToastKind::Info, t!("Feed rule “{rule}” added “{title}”", rule = d.rule.name, title = d.title));
        let pending = self.start_add(Box::new(AddRequest {
            source,
            torrent_bytes: None,
            save_path,
            create_subfolder: settings.downloads.create_subfolder,
            category,
            tags: vec!["rss".into()],
            paused: d.rule.start_paused,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
        }));
        match pending {
            Some(id) => {
                self.rss_adds.insert(id, d);
            }
            None => self.rss.forget(&d, &settings.rss),
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
        // Don't trust `is_paused()`: librqbit sets it when a pause is requested during
        // initialization even if the torrent then goes live anyway.
        if !matches!(h.stats().state, TorrentStatsState::Paused)
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
        self.resume_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if matches!(h.stats().state, TorrentStatsState::Paused)
            && let Err(e) = sess.unpause(&h).await
        {
            self.toast(ToastKind::Error, t!("Could not resume: {e}", e = format!("{e:#}")));
        }
        // Going live resets the limiter to the add-time limits.
        let m = self.store.get(&hash).map(|m| (m.down_limit_kib, m.up_limit_kib)).unwrap_or((0, 0));
        h.set_ratelimits(session::limits(m.0, m.1));
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
                    paused: matches!(h.stats().state, TorrentStatsState::Paused),
                    trackers: h.shared().trackers.iter().map(|u| u.to_string()).collect(),
                    meta: self.store.get(&hash).cloned().unwrap_or_default(),
                    at: Instant::now(),
                },
            );
        }
        self.busy.insert(hash, if with_data { t!("Deleting files…").into() } else { t!("Removing…").into() });
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let result =
                sess.delete(TorrentIdOrHash::Hash(Id20::new(hash.0)), with_data).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Internal::Deleted { hash, result });
        });
    }

    // ---------------------------------------------------------------- add / preview

    fn start_preview(&mut self, req: u64, source: AddSource) {
        let Some(sess) = self.session.clone() else {
            self.emit(EngineEvent::PreviewReady { req, result: Err(t!("The engine is not running.").into()) });
            return;
        };
        let events = self.events.clone();
        let wake = self.wake.clone();
        let tx = self.internal_tx.clone();
        let handle = tokio::spawn(async move {
            let result = preview(&sess, &source).await.map(Box::new).map_err(|e| format!("{e:#}"));
            let _ = events.send(EngineEvent::PreviewReady { req, result });
            let _ = tx.send(Internal::PreviewFinished { req });
            wake.wake();
        });
        self.previews.insert(req, handle);
    }

    /// Returns the pending-add id, or None if nothing was started.
    fn start_add(&mut self, req: Box<AddRequest>) -> Option<u64> {
        let Some(sess) = self.session.clone() else {
            self.toast(ToastKind::Error, t!("The engine is not running; cannot add torrents."));
            return None;
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
            PendingAdd {
                name: req.source.describe(),
                hash,
                started: now_unix(),
                save_path: req.save_path.clone(),
                task: None,
            },
        );
        let tx = self.internal_tx.clone();
        let peer_limit = Some(self.settings.connection.max_peers_per_torrent).filter(|n| *n > 0);
        let task = tokio::spawn(async move {
            let result = add_torrent(&sess, &req, peer_limit).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Internal::AddFinished { pending, req, result });
        });
        if let Some(p) = self.pending.get_mut(&pending) {
            p.task = Some(task);
        }
        Some(pending)
    }

    fn start_readd(
        &mut self,
        hash: InfoHash,
        move_to: Option<PathBuf>,
        trackers: Option<Vec<String>>,
        what: &'static str,
    ) {
        let (Some(sess), Some(h)) = (self.session.clone(), self.handle_for(&hash)) else { return };
        if self.busy.contains_key(&hash) {
            return;
        }
        let label = match what {
            "move" => t!("Moving files…"),
            "recheck" => t!("Preparing check…"),
            _ => t!("Updating trackers…"),
        };
        self.busy.insert(hash, label.into());
        let multi = self.static_info.get(&hash).map(|s| s.multi_file).unwrap_or(false);
        let backup_dir = self.paths.torrents_dir();
        let tx = self.internal_tx.clone();
        let peer_limit = Some(self.settings.connection.max_peers_per_torrent).filter(|n| *n > 0);
        tokio::spawn(async move {
            let result =
                readd(&sess, h, multi, move_to, trackers, backup_dir, peer_limit).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Internal::OpFinished { hash, what, result });
        });
    }

    /// Drop queued entries from the engine database. Only call while no session is running.
    /// Torrents whose move/recheck was cut off (quit, crash) after they left the session: their
    /// metainfo backup and a small note are in `torrents/`; add them back where their data is.
    fn recover_interrupted_readds(&mut self) {
        let Some(sess) = self.session.clone() else { return };
        let dir = self.paths.torrents_dir();
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "torrent") {
                continue;
            }
            let Some(hash) = p.file_stem().and_then(|s| s.to_str()).and_then(InfoHash::from_hex) else { continue };
            let note = p.with_extension("json");
            if self.handle_for(&hash).is_some() {
                // Finished after all; the leftovers are stale.
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_file(&note);
                continue;
            }
            let (Ok(bytes), Some(info)) = (
                std::fs::read(&p),
                std::fs::read(&note).ok().and_then(|b| serde_json::from_slice::<ReaddNote>(&b).ok()),
            ) else {
                continue;
            };
            tracing::warn!("recovering {} after an interrupted re-add", hash.to_hex());
            self.busy.insert(hash, t!("Recovering…").into());
            let tx = self.internal_tx.clone();
            let sess = sess.clone();
            tokio::spawn(async move {
                // Bring the files into one folder: finish the move, or undo it.
                let (name, files) =
                    match librqbit::torrent_from_bytes(&bytes).map_err(anyhow::Error::from).and_then(|t| {
                        let name = t.info.data.name.as_ref().map(|n| String::from_utf8_lossy(n.as_ref()).into_owned());
                        let v = t.info.data.validate()?;
                        let files: Vec<PathBuf> = v.iter_file_details().map(|f| f.filename.to_pathbuf()).collect();
                        Ok((name.unwrap_or_else(|| hash.to_hex()), files))
                    }) {
                        Ok(x) => x,
                        Err(e) => {
                            let result = Err(format!("{e:#}"));
                            let _ = tx.send(Internal::OpFinished { hash, what: "recover", result });
                            return;
                        }
                    };
                let (old, new, moved) = (info.old_output.clone(), info.new_output.clone(), info.moved.clone());
                let settled =
                    tokio::task::spawn_blocking(move || super::moves::settle(&old, &new, &files, &moved, false)).await;
                let output = match settled {
                    Ok(Ok(dir)) => dir,
                    Ok(Err(e)) => {
                        // Leave the files, the backup and the note alone: nothing is re-added
                        // until the files are in one folder again.
                        tracing::error!("{} is split between two folders: {e:#}", hash.to_hex());
                        let result = Err(t!(
                            "The files of {name} are split between {old} and {new} ({e}). Put them in one folder \
                             and restart Corrent; the torrent will be added back.",
                            name = name,
                            old = info.old_output.display(),
                            new = info.new_output.display(),
                            e = format!("{e:#}")
                        ));
                        let _ = tx.send(Internal::OpFinished { hash, what: "recover", result });
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Internal::OpFinished { hash, what: "recover", result: Err(e.to_string()) });
                        return;
                    }
                };
                let opts = AddTorrentOptions {
                    paused: info.paused,
                    only_files: info.only_files.clone(),
                    output_folder: Some(output.to_string_lossy().into_owned()),
                    overwrite: true,
                    trackers: Some(info.trackers.clone()),
                    ..Default::default()
                };
                let result = sess
                    .add_torrent(AddTorrent::from_bytes(bytes), Some(opts))
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"));
                if result.is_ok() {
                    let _ = std::fs::remove_file(&p);
                    let _ = std::fs::remove_file(&note);
                }
                let _ = tx.send(Internal::OpFinished { hash, what: "recover", result });
            });
        }
    }

    /// D6a: a damaged (or lost) `session.json` comes back from its newest good copy, before
    /// librqbit reads it.
    fn recover_session_db(&mut self) {
        let known: HashSet<InfoHash> = self.store.data.torrents.keys().copied().collect();
        match super::session_db::recover(&self.paths.session_dir(), &known) {
            None => {}
            Some(Ok(r)) => {
                self.prune_suspended = true;
                self.emit(EngineEvent::DataWarning(t!(
                    "The engine's torrent list was damaged and was restored from a copy saved {date}. Its {n} \
                     torrents are paused, because their folders may have changed since; resume them after a look. \
                     Torrents added after that copy are missing, but their categories and tags were kept. The \
                     damaged file was kept next to it.",
                    date = crate::util::fmt_date(r.saved_at),
                    n = r.torrents
                )));
            }
            Some(Err(path)) => {
                self.emit(EngineEvent::DataWarning(t!(
                    "The engine's torrent list ({path}) is damaged and no usable copy was found. The torrent \
                     engine can't start until the file is repaired or removed.",
                    path = path
                )));
            }
        }
    }

    /// D14: after a power loss, torrents that were writing data shortly before it are checked in
    /// full; the rest keep fast resume.
    fn check_after_power_loss(&mut self) {
        let Some(alive_at) = crate::crash::take_power_loss(&self.paths.log_dir()) else { return };
        let n = super::session_db::clear_unfinished_bitfields(&self.paths.session_dir(), &self.store.data, alive_at);
        tracing::warn!("the computer restarted without a clean exit; {n} torrent(s) will be checked in full");
        match n {
            0 => {}
            1 => self.toast(
                ToastKind::Info,
                t!("Checking 1 download that was in progress when the computer stopped unexpectedly."),
            ),
            n => self.toast(
                ToastKind::Info,
                t!("Checking {n} downloads that were in progress when the computer stopped unexpectedly.", n = n),
            ),
        }
    }

    fn purge_pending(&mut self) {
        if self.store.data.purge.is_empty() {
            return;
        }
        let set: HashSet<InfoHash> = self.store.data.purge.drain(..).collect();
        super::restore::purge(&self.paths.session_dir(), &set);
        self.store.mark_dirty();
    }

    fn detect_failed_restores(&mut self) {
        self.restore_checked = false;
        let Some(sess) = self.session.clone() else { return };
        let dir = self.paths.session_dir();
        let tx = self.internal_tx.clone();
        let skip: HashSet<InfoHash> = self.store.data.purge.iter().copied().collect();
        let peer_limit = Some(self.settings.connection.max_peers_per_torrent).filter(|n| *n > 0);
        tokio::spawn(async move {
            let mut failed = Vec::new();
            for mut f in super::restore::persisted_entries(&dir) {
                if skip.contains(&f.hash) || sess.get(TorrentIdOrHash::Hash(Id20::new(f.hash.0))).is_some() {
                    continue;
                }
                // One retry captures the actual error (and fixes transient failures).
                match super::restore::retry(&sess, &dir, &f, peer_limit).await {
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
        self.busy.insert(hash, t!("Retrying…").into());
        let peer_limit = Some(self.settings.connection.max_peers_per_torrent).filter(|n| *n > 0);
        tokio::spawn(async move {
            let result = super::restore::retry(&sess, &dir, &f, peer_limit).await;
            let _ = tx.send(Internal::RestoreRetried { hash, result, quiet });
        });
    }

    /// Keep the engine in line with Corrent's records: a torrent that should be paused (by the
    /// user, the queue or a seeding limit) but is running gets paused again. librqbit ignores a
    /// pause sent while a torrent is still initializing, so this makes pauses stick.
    /// librqbit resets a torrent's limiter and file order whenever it goes live again: keep
    /// Corrent's per-torrent limits, category limits and file priorities applied.
    fn apply_torrent_limits(&mut self, handles: &[(usize, Arc<ManagedTorrent>)]) {
        // Category limiters: one shared limiter per limited category, updated in place.
        let cats = &self.store.data.categories;
        self.category_limits
            .retain(|name, _| cats.get(name).is_some_and(|c| c.down_limit_kib > 0 || c.up_limit_kib > 0));
        for (name, c) in cats {
            if c.down_limit_kib == 0 && c.up_limit_kib == 0 {
                continue;
            }
            let want = session::limits(c.down_limit_kib, c.up_limit_kib);
            let l = self
                .category_limits
                .entry(name.clone())
                .or_insert_with(|| Arc::new(librqbit::limits::Limits::new(want)));
            if l.get_config() != want {
                l.set_download_bps(want.download_bps);
                l.set_upload_bps(want.upload_bps);
            }
        }
        for (_, h) in handles {
            let hash = InfoHash(h.info_hash().0);
            let Some(m) = self.store.get(&hash) else { continue };
            let want = session::limits(m.down_limit_kib, m.up_limit_kib);
            if h.ratelimits().is_some_and(|cur| cur != want) {
                h.set_ratelimits(want);
            }
            let group = m.category.as_ref().and_then(|c| self.category_limits.get(c)).cloned();
            let current = h.ratelimit_group();
            let same = match (&group, &current) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                h.set_ratelimit_group(group);
            }
            if !m.file_priority.is_empty()
                && let Some(cur) = h.file_priorities()
            {
                let want = file_order(h, &m.file_priority);
                if cur != want {
                    h.set_file_priorities(want);
                }
            }
        }
    }

    fn enforce_pauses(&mut self, views: &[TorrentView], handles: &[(usize, Arc<ManagedTorrent>)]) {
        let Some(sess) = self.session.clone() else { return };
        let running = |s: TorrentState| {
            matches!(
                s,
                TorrentState::Downloading | TorrentState::Stalled | TorrentState::Seeding | TorrentState::Metadata
            )
        };
        let to_pause: Vec<Arc<ManagedTorrent>> = views
            .iter()
            .filter(|v| running(v.state) && v.busy.is_none())
            .filter(|v| self.resumed_at.get(&v.hash).is_none_or(|t| t.elapsed() > Duration::from_secs(3)))
            .filter(|v| self.store.get(&v.hash).is_some_and(|m| m.pause_reason.is_some() && !m.force_started))
            .filter_map(|v| handles.iter().find(|(_, h)| h.info_hash().0 == v.hash.0).map(|(_, h)| h.clone()))
            .collect();
        if to_pause.is_empty() {
            return;
        }
        let epoch = self.resume_epoch.clone();
        let started = epoch.load(std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            for h in to_pause {
                if epoch.load(std::sync::atomic::Ordering::SeqCst) != started {
                    return; // something was resumed meanwhile; the next tick re-evaluates
                }
                if matches!(h.stats().state, TorrentStatsState::Live) {
                    let _ = sess.pause(&h).await;
                }
            }
        });
    }

    /// Warn once when the default download folder runs low while something is downloading.
    fn check_free_space(&mut self) {
        const LOW: u64 = 1024 * 1024 * 1024;
        let Some(free) = self.free_space else { return };
        let downloading = self.snapshot.load().torrents.iter().any(|t| t.state == TorrentState::Downloading);
        if free < LOW && downloading && !self.low_space_warned {
            self.low_space_warned = true;
            self.toast(
                ToastKind::Warning,
                t!(
                    "Only {free} free in the download folder. Downloads will fail when the disk is full.",
                    free = crate::util::fmt_bytes(free)
                ),
            );
        } else if free >= 2 * LOW {
            self.low_space_warned = false;
        }
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
            Internal::RssFetched { url, result } => {
                let settings = self.settings.clone();
                for d in self.rss.fetched(&url, result, &settings.rss) {
                    self.rss_add(d, &settings);
                }
            }
            Internal::PreviewFinished { req } => {
                self.previews.remove(&req);
            }
            Internal::AddFinished { pending, req, result } => {
                self.pending.remove(&pending);
                if let Some(d) = self.rss_adds.remove(&pending)
                    && let Err(e) = &result
                    && !is_duplicate_error(e)
                {
                    // Retry on a later refresh, but give up on links that keep failing.
                    let n = self.rss_failures.entry(d.key.clone()).or_default();
                    *n += 1;
                    if *n < 3 {
                        let rss = self.settings.rss.clone();
                        self.rss.forget(&d, &rss);
                    }
                }
                match result {
                    Ok((hash, name)) => {
                        self.unrecorded.remove(&hash);
                        let top = req.add_to_top;
                        if let Some((path, after)) = self.watch_adds.remove(&pending) {
                            self.watch.finish(&path, after, true);
                        }
                        let m = self.store.meta_mut(hash);
                        m.name = name.clone();
                        m.category = req.category.clone();
                        m.tags = req.tags.clone();
                        m.added_by = req.added_by.clone();
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
                        self.store.mark_dirty();
                        self.emit(EngineEvent::Added { hash, name });
                    }
                    Err(e) => {
                        // Logged too: a headless server has no one reading the toasts.
                        tracing::warn!("could not add {}: {e}", req.source.describe());
                        let duplicate = is_duplicate_error(&e);
                        let mut msg = t!("Could not add {name}: {e}", name = req.source.describe(), e = e);
                        if let Some((path, after)) = self.watch_adds.remove(&pending)
                            && let Some(failed) = self.watch.finish(&path, after, duplicate)
                        {
                            let name = failed.file_name().unwrap_or_default().to_string_lossy().into_owned();
                            msg = format!(
                                "{msg}\n{}",
                                t!("The file was renamed to {name} so it isn't added again.", name = name)
                            );
                        }
                        self.toast(ToastKind::Error, msg);
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
                            "move" => t!("Files moved. Verifying data…"),
                            "recheck" => t!("Re-checking data…"),
                            "recover" => t!("A torrent interrupted while being moved or re-checked was restored."),
                            _ => t!("Trackers added. Verifying data…"),
                        };
                        self.toast(ToastKind::Info, msg);
                    }
                    Err(e) => self.toast(ToastKind::Error, t!("Operation failed: {e}", e = e)),
                }
            }
            Internal::UndoFinished { hash, entry, result } => {
                self.busy.remove(&hash);
                match result {
                    Ok(false) => {
                        // Added again in the meantime: keep what the user set up then.
                        self.toast(ToastKind::Info, t!("That torrent is already back in your list."));
                    }
                    Ok(true) => {
                        let meta = entry.meta;
                        let name = meta.display_name.clone().unwrap_or(meta.name.clone());
                        // Bring back category, tags, dates, queue position and totals.
                        let mut meta = meta;
                        if meta.pause_reason == Some(PauseReason::Queue) && !self.settings.queue.enabled {
                            meta.pause_reason = None;
                        }
                        *self.store.meta_mut(hash) = meta;
                        self.store.normalize_queue();
                        self.store.mark_dirty();
                        self.emit(EngineEvent::Restored { hash, name });
                    }
                    Err(e) => {
                        // Keep the undo available (within its window) for another try.
                        self.undo.insert(hash, *entry);
                        self.toast(ToastKind::Error, t!("Undo failed: {e}", e = e));
                    }
                }
            }
            Internal::RestoreReport(list) => {
                if !list.is_empty() {
                    let n = list.len();
                    self.toast(
                        ToastKind::Error,
                        if n == 1 {
                            t!("A torrent could not be loaded — see its error in the list.").to_string()
                        } else {
                            t!("{n} torrents could not be loaded — see their errors in the list.", n = n)
                        },
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
                        self.toast(ToastKind::Success, t!("Torrent loaded."));
                    }
                    Err(e) => {
                        if let Some(f) = self.failed.get_mut(&hash) {
                            f.error = e;
                        }
                        if !quiet {
                            self.toast(
                                ToastKind::Error,
                                t!("Still can't load the torrent — see the error in the list."),
                            );
                        }
                    }
                }
            }
            Internal::Deleted { hash, result } => {
                self.busy.remove(&hash);
                let gone = self.handle_for(&hash).is_none();
                if let Err(e) = &result {
                    self.toast(ToastKind::Error, t!("Delete failed: {e}", e = e));
                }
                // librqbit forgets the torrent before touching files, so it may be gone despite an error.
                if result.is_ok() || gone {
                    let name = self
                        .store
                        .get(&hash)
                        .map(|m| m.display_name.clone().unwrap_or(m.name.clone()))
                        .unwrap_or_default();
                    self.forget_hash(&hash);
                    self.store.mark_dirty();
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
        self.poll_feeds();
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
                    .or_else(|| {
                        category
                            .as_ref()
                            .and_then(|c| self.store.data.categories.get(c))
                            .and_then(|c| c.save_path.clone())
                    })
                    .unwrap_or_else(|| self.settings.downloads.save_path.clone());
                tracing::info!("watch folder: adding {}", hit.source.describe());
                let (path, after) = (hit.path, hit.folder.after_add);
                let id = self.start_add(Box::new(AddRequest {
                    source: hit.source,
                    torrent_bytes: None,
                    save_path,
                    create_subfolder: self.settings.downloads.create_subfolder,
                    category,
                    tags: Vec::new(),
                    paused: hit.folder.start_paused,
                    only_files: None,
                    add_to_top: false,
                    initial_peers: None,
                    added_by: None,
                }));
                match id {
                    Some(id) => {
                        self.watch_adds.insert(id, (path, after));
                    }
                    None => self.watch.release(&path),
                }
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
            let total = self.store.data.torrents.len();
            if orphans.is_empty() {
                self.prune_refused = None;
            } else if self.prune_suspended || !super::logic::prune_allowed(total, orphans.len()) {
                if self.prune_refused.replace(orphans.len()) != Some(orphans.len()) && !self.prune_suspended {
                    tracing::warn!(
                        "{} of {total} store entries have no torrent in the engine; not pruning",
                        orphans.len()
                    );
                    self.emit(EngineEvent::DataWarning(t!(
                        "{n} of {total} torrents are missing from the engine's database, so Corrent kept their \
                         categories, tags and history instead of deleting them. If this is unexpected, restore a \
                         backup of the session folder.",
                        n = orphans.len(),
                        total = total
                    )));
                }
            } else {
                tracing::info!("pruning {} stale store entries", orphans.len());
                self.prune_refused = None;
                // A dated copy of the complete data first (rare: only after outside changes).
                match serde_json::to_vec_pretty(&self.store.data) {
                    Ok(bytes) => {
                        if let Err(e) = crate::util::keep_generations(&self.paths.store_file(), "pre-prune", &bytes, 3)
                        {
                            tracing::warn!("could not keep a copy of store.json before pruning: {e:#}");
                        }
                    }
                    Err(e) => tracing::warn!("could not serialize the store: {e}"),
                }
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

        if self.tick_count.is_multiple_of(2) {
            self.enforce_pauses(&views, &handles);
            self.apply_torrent_limits(&handles);
        }
        if self.settings.queue.enabled && self.tick_count.is_multiple_of(4) {
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
            stream: self.stream.as_ref().map(|s| (s.port, s.token.clone())),
            streams: self.stream_views(&handles),
            stream_slots: self.stream.as_ref().map(|s| s.slots()).unwrap_or_default(),
            blocked_ranges: sess.blocklist.len(),
            allowed_ranges: sess.allowlist.as_ref().map(|a| a.len()),
        };

        // Activity sparklines: the last minute or so, refreshed every other tick.
        if self.tick_count.is_multiple_of(2) {
            const ROW_SAMPLES: usize = 40;
            let kib = |b: u64| (b / 1024).min(u32::MAX as u64) as u32;
            self.row_history = Arc::new(
                self.torrent_history
                    .iter()
                    .map(|(h, v)| {
                        let skip = v.len().saturating_sub(ROW_SAMPLES);
                        (*h, v.iter().skip(skip).map(|(d, u)| (kib(*d), kib(*u))).collect())
                    })
                    .collect(),
            );
        }

        self.generation += 1;
        let snap = Snapshot {
            row_history: self.row_history.clone(),
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
            rss: self.rss.view(),
            busy_ops: self.busy.len(),
            undoable: self
                .undo
                .iter()
                .map(|(h, e)| (*h, e.meta.display_name.clone().unwrap_or(e.meta.name.clone())))
                .collect(),
        };
        self.snapshot.store(Arc::new(snap));
        self.store.maybe_save();
        if let Some(e) = self.store.take_save_error() {
            self.emit(EngineEvent::DataWarning(t!(
                "Corrent couldn't save its data: {why}",
                why = crate::util::explain_io(&e)
            )));
        }
        self.check_free_space();
        if self.last_heartbeat.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
            // For D14: when this run was last alive, should the computer lose power.
            crate::crash::heartbeat(&self.paths.log_dir());
            self.last_heartbeat = Some(Instant::now());
        }
        self.wake.wake();
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
        if self.pending.is_empty() {
            self.unrecorded.clear();
        } else if self.store.get(&hash).is_none() {
            self.unrecorded.insert(hash);
        }
        // A torrent the engine already has while its add is still being recorded (category,
        // tags…): announce its completion on a later tick, once the add has finished.
        let add_unrecorded = self.unrecorded.contains(&hash);
        let m = self.store.meta_quiet(hash);
        let mut structural = false;
        let mut counters = false;
        if let Some(f) = fetched {
            let (du, dd) = m.absorb_counters(stats.uploaded_bytes, f);
            *total_du += du;
            *total_dd += dd;
            counters = du > 0 || dd > 0;
            if dd > 0 {
                // Data arrived this session: a completion right after is a real download.
                self.seen_unfinished.insert(hash);
            }
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
        if stats.finished && has_metadata && !add_unrecorded {
            if m.completed_at.is_none() {
                m.completed_at = Some(now);
                // Only torrents seen unfinished (or receiving data) this session: fast downloads
                // finishing between two ticks still count, restarts and re-adds stay silent.
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
        let eta = (down_bps > 0 && remaining > 0 && !stats.finished)
            .then(|| Duration::from_secs(remaining.div_ceil(down_bps)));
        let full_size = si_total(h).unwrap_or(size);
        let ratio = if full_size > 0 { m.uploaded_total as f32 / full_size as f32 } else { 0.0 };

        let output = h.output_folder().to_path_buf();
        let (save_path, content_path) = if si.multi_file {
            (output.parent().map(Path::to_path_buf).unwrap_or_else(|| output.clone()), output.clone())
        } else {
            let first = h.with_metadata(|md| md.file_infos.first().map(|f| f.relative_filename.clone())).ok().flatten();
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
            error: stats.error.as_deref().map(crate::util::explain_io),
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
            added_by: m.added_by.clone(),
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
            down_limit_kib: m.down_limit_kib,
            up_limit_kib: m.up_limit_kib,
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
            let (save, category) = (view.save_path.clone(), view.category.clone());
            self.emit(EngineEvent::Completed { hash, name, category, content_path, save_path: save, size });
        }
        view
    }

    fn file_views(&self, h: &Arc<ManagedTorrent>, hash: InfoHash) -> Vec<FileView> {
        let stats = h.stats();
        let only: Option<HashSet<usize>> = h.only_files().map(|v| v.into_iter().collect());
        let file_priority = self.store.get(&hash).map(|m| m.file_priority.clone()).unwrap_or_default();
        h.with_metadata(|md| {
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
                        components,
                        len: f.len,
                        done: stats.file_progress.get(i).copied().unwrap_or(0),
                        included: only.as_ref().map(|s| s.contains(&i)).unwrap_or(true),
                        priority: file_priority.get(&i).copied().unwrap_or(0),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
    }

    /// Peers with rates from the counters seen `interval` seconds ago (`prev` is updated).
    fn peer_views(h: &Arc<ManagedTorrent>, prev: &mut PeerCounters, interval: f64) -> Vec<PeerView> {
        let mut peers = Vec::new();
        if let Some(live) = h.live() {
            let snap = live.per_peer_stats_snapshot(Default::default());
            let mut next_prev = HashMap::with_capacity(snap.peers.len());
            for (addr, p) in snap.peers {
                let (pf, pu) =
                    prev.get(&addr).copied().unwrap_or((p.counters.fetched_bytes, p.counters.uploaded_bytes));
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
            *prev = next_prev;
        }
        peers
    }

    /// Files, peers and pieces of any torrent, for a remote client (the GUI's detail target is
    /// left alone). Peer rates are measured since the previous request for the same torrent.
    fn remote_detail(&mut self, hash: InfoHash) -> Option<TorrentDetail> {
        let h = self.handle_for(&hash)?;
        let files = self.file_views(&h, hash);
        let now = Instant::now();
        self.remote_prev.retain(|_, (t, _)| now.duration_since(*t) < Duration::from_secs(60));
        let (then, mut prev) = self.remote_prev.remove(&hash).unwrap_or((now, HashMap::new()));
        let interval = now.duration_since(then).as_secs_f64().max(0.2);
        let peers = Self::peer_views(&h, &mut prev, interval);
        self.remote_prev.insert(hash, (now, prev));
        let pieces = self
            .api
            .as_ref()
            .and_then(|api| api.api_dump_haves(TorrentIdOrHash::Hash(Id20::new(hash.0))).ok())
            .map(|(bf, n)| (bf.as_raw_slice().to_vec(), n));
        Some(TorrentDetail {
            hash,
            files,
            peers,
            pieces,
            history: self.torrent_history.get(&hash).cloned().unwrap_or_default(),
        })
    }

    fn build_detail(&mut self, id: usize, h: &Arc<ManagedTorrent>, hash: InfoHash) -> TorrentDetail {
        let files = self.file_views(h, hash);

        let interval = self.settings.advanced.refresh_interval_ms.max(200) as f64 / 1000.0;
        let peers = Self::peer_views(h, &mut self.prev_peer, interval);

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

    /// Stream sessions with torrent names, smoothed rates and file availability.
    fn stream_views(&mut self, handles: &[(usize, Arc<ManagedTorrent>)]) -> Vec<StreamView> {
        let Some(server) = &self.stream else { return Vec::new() };
        let infos = server.sessions();
        let now = Instant::now();
        self.stream_rates.retain(|id, _| infos.iter().any(|i| i.id == *id));
        self.stream_haves.retain(|h, _| infos.iter().any(|i| i.hash == *h));
        let mut out = Vec::with_capacity(infos.len());
        for i in infos {
            let handle = handles.iter().find(|(_, h)| h.info_hash().0 == i.hash.0);
            let rate = match self.stream_rates.get(&i.id) {
                Some(&(t, served, ema)) => {
                    let dt = now.duration_since(t).as_secs_f64();
                    if dt < 0.2 {
                        ema
                    } else {
                        let inst = i.served.saturating_sub(served) as f64 / dt;
                        let ema = if i.state.is_active() { ema * 0.6 + inst * 0.4 } else { 0.0 };
                        self.stream_rates.insert(i.id, (now, i.served, ema));
                        ema
                    }
                }
                None => {
                    self.stream_rates.insert(i.id, (now, i.served, 0.0));
                    0.0
                }
            };
            let mut buffered_ahead = None;
            let mut availability = Vec::new();
            if let Some((id, h)) = handle {
                let stale = self.stream_haves.get(&i.hash).is_none_or(|(t, _, _)| t.elapsed() > Duration::from_secs(1));
                if stale
                    && let Some(api) = &self.api
                    && let Ok((bf, n)) = api.api_dump_haves(TorrentIdOrHash::Id(*id))
                {
                    self.stream_haves.insert(i.hash, (now, bf.as_raw_slice().to_vec(), n));
                }
                let layout = h
                    .with_metadata(|m| {
                        m.file_infos
                            .get(i.file)
                            .map(|f| (f.offset_in_torrent, m.info.lengths().default_piece_length() as u64))
                    })
                    .ok()
                    .flatten();
                if let (Some((bits, n)), Some((offset, piece_len))) =
                    (self.stream_haves.get(&i.hash).map(|(_, b, n)| (b, *n)), layout)
                {
                    let (ahead, strip) =
                        super::stream::file_availability(bits, n, piece_len, offset, i.file_len, i.position, 120);
                    buffered_ahead = Some(ahead);
                    availability = strip;
                }
            }
            out.push(StreamView {
                id: i.id,
                hash: i.hash,
                torrent_name: handle.map(|(_, h)| h.name().unwrap_or_default()).unwrap_or_default(),
                file: i.file,
                player: super::stream::player_name(&i.user_agent),
                file_name: i.file_name,
                file_len: i.file_len,
                client: i.client.to_string(),
                user_agent: i.user_agent,
                state: i.state,
                connections: i.connections,
                requests: i.requests,
                refused: i.refused,
                elapsed_secs: i.elapsed.as_secs(),
                served: i.served,
                rate_bps: rate as u64,
                position: i.position,
                buffered_ahead,
                availability,
            });
        }
        out
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
        snap.rss = self.rss.view();
        snap.busy_ops = self.busy.len();
        for t in &mut snap.torrents {
            if let Some(m) = self.store.get(&t.hash) {
                t.category = m.category.clone();
                t.added_by = m.added_by.clone();
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
        self.wake.wake();
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
                    if want {
                        t!("Scheduled alternative speed limits enabled.")
                    } else {
                        t!("Scheduled alternative speed limits ended.")
                    },
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
        if !to_resume.is_empty() {
            self.resume_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let find = |hash: &InfoHash| handles.iter().find(|(_, h)| h.info_hash().0 == hash.0).map(|(_, h)| h.clone());
        let pause: Vec<_> = to_pause.iter().filter_map(find).collect();
        let resume: Vec<_> = to_resume
            .iter()
            .filter_map(|hash| {
                let m = self.store.get(hash).map(|m| (m.down_limit_kib, m.up_limit_kib)).unwrap_or((0, 0));
                find(hash).map(|h| (h, session::limits(m.0, m.1)))
            })
            .collect();
        tokio::spawn(async move {
            for h in pause {
                let _ = sess.pause(&h).await;
            }
            for (h, limits) in resume {
                let _ = sess.unpause(&h).await;
                h.set_ratelimits(limits);
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
                    self.toast(ToastKind::Info, t!("Stopped seeding “{name}”: {why}.", name = name, why = why));
                }
                SeedAction::Remove | SeedAction::RemoveWithData => {
                    self.delete(hash, s.action == SeedAction::RemoveWithData);
                    self.toast(ToastKind::Info, t!("Removed “{name}”: {why}.", name = name, why = why));
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
        added_by: None,
        trackers: Vec::new(),
        has_metadata: false,
        file_count: 0,
        piece_count: 0,
        piece_size: 0,
        comment: None,
        created_by: None,
        creation_date: None,
        busy: Some(t!("Fetching metadata…").into()),
        last_activity: 0,
        seeding_secs: 0,
        down_limit_kib: 0,
        up_limit_kib: 0,
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
        v.added_by = m.added_by.clone();
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
        .add_torrent(AddTorrent::from_bytes(bytes), Some(AddTorrentOptions { list_only: true, ..Default::default() }))
        .await?
    {
        AddTorrentResponse::ListOnly(l) => {
            // Every add and preview comes through here (files, magnets, URLs, RSS, watch folders,
            // the Web API, imports), before anything is written.
            // BEP 47 padding files (`.pad/<size>`) may repeat and aren't the user's files.
            let files = l
                .info
                .iter_file_details()
                .filter(|f| !f.attrs().padding)
                .map(|f| f.filename.to_vec().into_iter().map(|c| c.to_string()).collect::<Vec<String>>());
            super::paths::check_torrent_paths(files, super::paths::Rules::HOST).map_err(anyhow::Error::msg)?;
            Ok(l)
        }
        _ => anyhow::bail!("unexpected response from engine"),
    }
}

async fn preview(sess: &Arc<Session>, source: &AddSource) -> anyhow::Result<Preview> {
    let bytes = fetch_torrent_bytes(sess, source).await?;
    // Not a .torrent file: say so now (handed to the session, broken bytes can stall instead).
    librqbit::torrent_from_bytes(&bytes).map_err(|e| anyhow::anyhow!("not a valid .torrent file: {e}"))?;
    let l = list_only(sess, bytes).await?;
    let info_hash = InfoHash(l.info_hash.0);
    let name = l.info.name().map(|n| n.into_owned()).unwrap_or_else(|| info_hash.to_hex());
    let files: Vec<PreviewFile> = l
        .info
        .iter_file_details()
        .map(|f| {
            let components = f.filename.to_vec();
            PreviewFile { components, len: f.len }
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
        already_added: sess.get(TorrentIdOrHash::Hash(l.info_hash)).is_some(),
        comment,
    })
}

pub fn sanitize_name(name: &str) -> String {
    let cleaned: String =
        name.chars()
            .map(|c| {
                if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    let mut trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if super::paths::is_reserved(&trimmed) {
        trimmed.insert(0, '_');
    }
    // At most 255 UTF-16 units (the Windows limit for one component).
    let mut units = 0;
    let end = trimmed
        .char_indices()
        .find(|(_, c)| {
            units += c.len_utf16();
            units > 255
        })
        .map_or(trimmed.len(), |(i, _)| i);
    trimmed.truncate(end);
    let trimmed = trimmed.trim_end_matches(['.', ' ']).to_string();
    if trimmed.is_empty() { "torrent".into() } else { trimmed }
}

/// The end of the error an add returns for a torrent that is already in the list.
const DUPLICATE: &str = "is already in your list";

/// True for the error of an add that found its torrent already in the list.
fn is_duplicate_error(e: &str) -> bool {
    e.ends_with(DUPLICATE)
}

async fn add_torrent(
    sess: &Arc<Session>,
    req: &AddRequest,
    peer_limit: Option<usize>,
) -> anyhow::Result<(InfoHash, String)> {
    let bytes = match &req.torrent_bytes {
        Some(b) => b.clone(),
        None => fetch_torrent_bytes(sess, &req.source).await?,
    };
    let l = list_only(sess, bytes.clone()).await?;
    let hash = InfoHash(l.info_hash.0);
    let name = l.info.name().map(|n| n.into_owned()).unwrap_or_else(|| hash.to_hex());
    if sess.get(TorrentIdOrHash::Hash(l.info_hash)).is_some() {
        anyhow::bail!("“{name}” {DUPLICATE}");
    }
    let multi = l.info.info().files.is_some();
    // Trackers from the magnet link (tr=…) must reach the engine even when we add from bytes.
    let magnet_trackers: Vec<String> = match &req.source {
        AddSource::Magnet(m) => librqbit::Magnet::parse(m).map(|m| m.trackers).unwrap_or_default(),
        _ => Vec::new(),
    };
    let output =
        if multi && req.create_subfolder { req.save_path.join(sanitize_name(&name)) } else { req.save_path.clone() };
    tokio::fs::create_dir_all(&output).await.with_context(|| format!("creating {}", output.display()))?;
    let opts = AddTorrentOptions {
        paused: req.paused,
        only_files: req.only_files.clone(),
        output_folder: Some(output.to_string_lossy().into_owned()),
        overwrite: true,
        peer_limit,
        trackers: (!magnet_trackers.is_empty()).then_some(magnet_trackers),
        initial_peers: req.initial_peers.clone(),
        ..Default::default()
    };
    match sess.add_torrent(AddTorrent::from_bytes(bytes), Some(opts)).await? {
        AddTorrentResponse::Added(_, _) => Ok((hash, name)),
        AddTorrentResponse::AlreadyManaged(_, _) => anyhow::bail!("“{name}” {DUPLICATE}"),
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
    peer_limit: Option<usize>,
) -> anyhow::Result<()> {
    let id = h.id();
    let hash = InfoHash(h.info_hash().0);
    let bytes = h.with_metadata(|m| m.torrent_bytes.to_vec())?;
    let only_files = h.only_files();
    let paused = matches!(h.stats().state, TorrentStatsState::Paused);
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
    let file_list: Vec<PathBuf> =
        h.with_metadata(|m| m.file_infos.iter().map(|f| f.relative_filename.clone()).collect())?;
    drop(h);
    // Keep a copy of the metainfo until the torrent is safely back in the session.
    let backup = backup_dir.join(format!("{}.torrent", hash.to_hex()));
    std::fs::create_dir_all(&backup_dir).with_context(|| format!("creating {}", backup_dir.display()))?;
    crate::util::write_atomic(&backup, &bytes).context("saving a copy of the torrent before re-adding it")?;
    let note = backup.with_extension("json");
    let info = ReaddNote {
        old_output: old_output.clone(),
        new_output: new_output.clone(),
        first_file: file_list.first().cloned(),
        only_files: only_files.clone(),
        paused,
        trackers: trackers.clone(),
        moved: Vec::new(),
    };
    crate::util::write_replace(&note, &serde_json::to_vec(&info)?).context("saving re-add details")?;
    sess.delete(TorrentIdOrHash::Id(id), false).await?;

    let mut move_err = None;
    if move_to.is_some() {
        let (from, to, note) = (old_output.clone(), new_output.clone(), note.clone());
        let mut info = info.clone();
        let r = tokio::task::spawn_blocking(move || {
            super::moves::move_files(
                &from,
                &to,
                &file_list,
                &mut |rel| {
                    // So a crash from here on knows which files are already in the new folder.
                    info.moved.push(rel.to_path_buf());
                    crate::util::write_replace(&note, &serde_json::to_vec(&info)?)
                },
                false,
            )
        })
        .await?;
        match r {
            Ok(()) => {}
            Err(super::moves::MoveError::Back(e)) => move_err = Some(e),
            Err(super::moves::MoveError::Split(e)) => {
                // Don't add it back at either folder (that would download the missing files
                // again): the backup and note stay, and the next start brings the files together.
                return Err(e.context(t!(
                    "the files are now split between {old} and {new}; Corrent will try to bring them together \
                     the next time it starts",
                    old = old_output.display(),
                    new = new_output.display()
                )));
            }
        }
    }
    let target = if move_err.is_some() { old_output.clone() } else { new_output };
    let add_at = |output: &Path| {
        let opts = AddTorrentOptions {
            paused,
            only_files: only_files.clone(),
            output_folder: Some(output.to_string_lossy().into_owned()),
            overwrite: true,
            // No preferred id: another add during the gap may have been given the old one, and
            // librqbit would then report *that* torrent as "already managed".
            trackers: Some(trackers.clone()),
            peer_limit,
            ..Default::default()
        };
        let (sess, bytes) = (sess.clone(), bytes.clone());
        async move {
            match sess.add_torrent(AddTorrent::from_bytes(bytes), Some(opts)).await? {
                AddTorrentResponse::Added(..) => Ok(()),
                AddTorrentResponse::AlreadyManaged(_, other) if InfoHash(other.info_hash().0) == hash => Ok(()),
                _ => anyhow::bail!("the engine didn't take the torrent back"),
            }
        }
    };
    if let Err(e) = add_at(&target).await {
        // Never lose the torrent: fall back to its previous location.
        if target != old_output && add_at(&old_output).await.is_ok() {
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::remove_file(&note);
            return Err(e.context(t!("could not use the new location; the torrent was kept where it was")));
        }
        return Err(e.context(t!(
            "the torrent could not be re-added; its metainfo is saved at {path}",
            path = backup.display()
        )));
    }
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_file(&note);
    if let Some(e) = move_err {
        return Err(e.context(t!("moving files failed; the torrent was kept at its old location")));
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
        assert_eq!(sanitize_name("CON"), "_CON");
        assert_eq!(sanitize_name("nul.tar"), "_nul.tar");
        let long = sanitize_name(&"é".repeat(300));
        assert_eq!(long.encode_utf16().count(), 255);
        assert_eq!(sanitize_name(&format!("{}. x", "a".repeat(254))), "a".repeat(254), "no trailing dot after the cut");
    }

    #[test]
    fn kib_limits() {
        assert_eq!(super::super::session::kib_limit(0), None);
        assert_eq!(super::super::session::kib_limit(2).map(|n| n.get()), Some(2048));
        let l = super::super::session::limits(1, 0);
        assert!(l.download_bps.is_some() && l.upload_bps.is_none());
    }
}

/// Download order of a torrent's files: librqbit's default (by file name), then by priority.
fn file_order(h: &ManagedTorrent, priority: &std::collections::BTreeMap<usize, i8>) -> Vec<usize> {
    let names: Vec<PathBuf> =
        h.with_metadata(|m| m.file_infos.iter().map(|f| f.relative_filename.clone()).collect()).unwrap_or_default();
    super::logic::file_order(&names, priority)
}

/// Written next to a re-add backup so an interrupted move/recheck can be recovered.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ReaddNote {
    old_output: PathBuf,
    new_output: PathBuf,
    first_file: Option<PathBuf>,
    only_files: Option<Vec<usize>>,
    paused: bool,
    trackers: Vec<String>,
    /// Files already moved to `new_output`, recorded (and synced) as each one finishes.
    #[serde(default)]
    moved: Vec<PathBuf>,
}
