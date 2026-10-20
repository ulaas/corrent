//! A Corrent server as the app's engine: the realtime channel (WebSocket) keeps a mirror of the
//! server's torrents that is published as ordinary [`Snapshot`]s, and engine commands become API
//! calls. Features that only make sense next to the files (creating torrents, the IP filter,
//! local streaming) answer that they aren't available.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::{ArcSwap, ArcSwapOption};
use base64::Engine as _;
use corrent_core::engine::types::{AddSource, EngineState, InfoHash, Snapshot, wire};
use corrent_core::engine::{
    AddRequest, Backend, BackendIo, EngineCommand, EngineEvent, Preview, PreviewFile, ToastKind, Wake,
};
use corrent_core::store::QueueMove;
use corrent_proto::event::{ClientMessage, Event, Severity, Topic};
use corrent_proto::torrent as p;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::ServerEntry;
use crate::app::media::client::MediaClient;
use crate::config::Settings;

const HISTORY_LEN: usize = 600;
const ROW_HISTORY_LEN: usize = 60;
const UNDO_FOR: Duration = Duration::from_secs(120);

/// What the UI can read about the connection besides the snapshot.
#[derive(Default)]
pub struct RemoteStatus {
    /// The server's settings, when this user may read them (defaults for the add dialog).
    pub settings: ArcSwapOption<Settings>,
    /// Why the server can't be reached right now.
    pub problem: ArcSwapOption<String>,
    pub connected: std::sync::atomic::AtomicBool,
    /// How the server is reached right now.
    pub route: ArcSwapOption<Route>,
}

/// Use the public relays for Connect (tests turn them off to stay offline).
pub static CONNECT_RELAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// One way to the server: its own address, or Corrent Connect through a loopback bridge.
pub struct Route {
    pub url: String,
    pub ca: Option<String>,
    pub api: MediaClient,
    pub via_connect: bool,
    #[cfg(feature = "p2p")]
    _bridge: Option<corrent_server::p2p::Bridge>,
}

/// Finds a way to the server: its address first (the same server must answer), then Corrent
/// Connect when the server has it.
pub async fn resolve(server: &ServerEntry) -> Result<Route, String> {
    let lan = MediaClient::new(&server.url, &server.token, server.ca())?;
    let lan_err = match tokio::time::timeout(
        Duration::from_secs(5),
        lan.get::<corrent_proto::server::ServerInfo>("/api/v1/server/info"),
    )
    .await
    {
        Ok(Ok(info)) if server.id.is_empty() || info.id == server.id => {
            return Ok(Route {
                url: server.url.clone(),
                ca: server.ca().map(str::to_string),
                api: lan,
                via_connect: false,
                #[cfg(feature = "p2p")]
                _bridge: None,
            });
        }
        Ok(Ok(_)) => t!("A different server answers at this address.").to_string(),
        Ok(Err(e)) => t!("Can't reach the server: {e}", e = e),
        Err(_) => t!("The server didn't answer.").to_string(),
    };
    #[cfg(feature = "p2p")]
    if !server.connect_id.is_empty() {
        let id: corrent_server::p2p::EndpointId =
            server.connect_id.parse().map_err(|_| t!("The Corrent Connect address isn't valid.").to_string())?;
        let direct = server.connect_addrs.iter().filter_map(|a| a.parse().ok()).collect();
        let relay = CONNECT_RELAY.load(std::sync::atomic::Ordering::Relaxed);
        let b = corrent_server::p2p::bridge(id, relay, direct)
            .await
            .map_err(|e| t!("Corrent Connect: {e}", e = format!("{e:#}")))?;
        let url = b.url();
        let api = MediaClient::new(&url, &server.token, None)?;
        return Ok(Route { url, ca: None, api, via_connect: true, _bridge: Some(b) });
    }
    Err(lan_err)
}

/// The backend for `server`, and the status the UI reads.
pub fn backend(server: ServerEntry) -> (Backend, Arc<RemoteStatus>) {
    let status: Arc<RemoteStatus> = Arc::default();
    let s2 = status.clone();
    (Arc::new(move |io| Box::pin(run(server.clone(), s2.clone(), io))), status)
}

#[derive(Default)]
struct Mirror {
    torrents: Vec<p::Torrent>,
    labels: p::Labels,
    stats: Option<p::SessionStats>,
    detail: Option<p::TorrentDetail>,
    detail_target: Option<InfoHash>,
    detail_history: VecDeque<(u64, u64)>,
    history: VecDeque<(u64, u64)>,
    row_history: HashMap<InfoHash, Vec<(u32, u32)>>,
    undoable: Vec<(InfoHash, String, Instant)>,
    rss: Arc<corrent_core::rss::RssView>,
    ever_connected: bool,
    generation: u64,
}

struct Shared {
    mirror: Mutex<Mirror>,
    /// Signalled whenever the connection comes up (queued commands are sent then).
    up: tokio::sync::Notify,
    snapshot: Arc<ArcSwap<Snapshot>>,
    wake: Wake,
    status: Arc<RemoteStatus>,
}

impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut Mirror) -> R) -> R {
        f(&mut self.mirror.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn publish(&self) {
        let connected = self.status.connected.load(std::sync::atomic::Ordering::Relaxed);
        let problem = self.status.problem.load_full();
        let snap = self.with(|m| {
            m.generation += 1;
            m.undoable.retain(|u| u.2.elapsed() < UNDO_FOR);
            let mut s = Snapshot {
                torrents: m.torrents.iter().enumerate().map(|(i, t)| wire::torrent_view(t, i)).collect(),
                detail: m.detail.as_ref().filter(|d| Some(d.hash) == m.detail_target).map(|d| {
                    let mut v = wire::detail_view(d);
                    v.history = m.detail_history.clone();
                    v
                }),
                history: m.history.clone(),
                generation: m.generation,
                categories: wire::categories(&m.labels),
                tags: m.labels.tags.clone(),
                undoable: m.undoable.iter().map(|u| (u.0, u.1.clone())).collect(),
                rss: m.rss.clone(),
                row_history: Arc::new(m.row_history.clone()),
                ..Default::default()
            };
            if let Some(st) = &m.stats {
                wire::apply_session(&mut s, st);
            }
            if !connected {
                s.engine_state = if m.ever_connected { EngineState::Restarting } else { EngineState::Starting };
                s.warning = problem.as_deref().cloned();
            }
            s
        });
        self.snapshot.store(Arc::new(snap));
        self.wake.wake();
    }

    fn set_connected(&self, on: bool, problem: Option<String>) {
        self.status.connected.store(on, std::sync::atomic::Ordering::Relaxed);
        self.status.problem.store(problem.map(Arc::new));
        if on {
            self.with(|m| m.ever_connected = true);
            self.up.notify_one();
        }
        self.publish();
    }
}

async fn run(server: ServerEntry, status: Arc<RemoteStatus>, mut io: BackendIo) {
    // Continue the generation after a restart: the UI caches rows per generation.
    let mirror = Mirror { generation: io.snapshot.load().generation, ..Default::default() };
    let shared = Arc::new(Shared {
        mirror: Mutex::new(mirror),
        up: tokio::sync::Notify::new(),
        snapshot: io.snapshot.clone(),
        wake: io.wake.clone(),
        status: status.clone(),
    });
    // After a restart the status still describes the previous task.
    shared.set_connected(false, None);
    let fallback = match MediaClient::new(&server.url, &server.token, server.ca()) {
        Ok(c) => c,
        Err(e) => {
            shared.set_connected(false, Some(e));
            while let Some(cmd) = io.commands.recv().await {
                if let EngineCommand::Shutdown(tx) = cmd {
                    let _ = tx.send(());
                    return;
                }
            }
            return;
        }
    };
    let (target_tx, target_rx) = tokio::sync::watch::channel(None);
    let ws = tokio::spawn(ws_loop(server.clone(), shared.clone(), io.events.clone(), target_rx));
    let ctx = Ctx { fallback, events: io.events.clone(), wake: io.wake.clone(), shared: shared.clone() };
    // Changes made while the server is unreachable wait here and go out after reconnecting
    // (in order); queries are answered (or fail) right away.
    let mut queued: std::collections::VecDeque<EngineCommand> = Default::default();
    loop {
        let cmd = tokio::select! {
            c = io.commands.recv() => match c {
                Some(c) => c,
                None => break,
            },
            _ = shared.up.notified() => {
                if !queued.is_empty() {
                    let n = queued.len();
                    tracing::info!("server {}: sending {n} queued changes", server.name);
                    for c in queued.drain(..) {
                        ctx.command(c);
                    }
                }
                continue;
            }
        };
        let offline = !status.connected.load(std::sync::atomic::Ordering::Relaxed);
        if offline && queueable(&cmd) {
            if queued.len() < QUEUE_MAX {
                if queued.is_empty() {
                    ctx.toast(
                        ToastKind::Info,
                        t!("The server can't be reached: changes are sent when it's back.").to_string(),
                    );
                }
                queued.push_back(cmd);
            } else {
                ctx.toast(ToastKind::Error, t!("The server can't be reached.").to_string());
            }
            continue;
        }
        match cmd {
            EngineCommand::Shutdown(tx) => {
                ws.abort();
                let _ = tx.send(());
                return;
            }
            EngineCommand::SetDetailTarget(h) => {
                shared.with(|m| {
                    if m.detail_target != h {
                        m.detail = None;
                        m.detail_history.clear();
                    }
                    m.detail_target = h;
                });
                let _ = target_tx.send(h);
            }
            cmd => ctx.command(cmd),
        }
    }
    ws.abort();
}

const QUEUE_MAX: usize = 200;

/// Changes that make sense to send later (not queries with a reply, previews or local-only
/// things).
fn queueable(c: &EngineCommand) -> bool {
    matches!(
        c,
        EngineCommand::Add(_)
            | EngineCommand::Pause(_)
            | EngineCommand::Resume(_)
            | EngineCommand::ForceStart(_)
            | EngineCommand::PauseAll
            | EngineCommand::ResumeAll
            | EngineCommand::Delete { .. }
            | EngineCommand::SetFiles { .. }
            | EngineCommand::SetCategory { .. }
            | EngineCommand::SetTags { .. }
            | EngineCommand::ToggleTag { .. }
            | EngineCommand::Rename { .. }
            | EngineCommand::SetTorrentLimits { .. }
            | EngineCommand::SetFilePriority { .. }
            | EngineCommand::QueuePlace { .. }
            | EngineCommand::QueueMove { .. }
            | EngineCommand::SetSeedLimits { .. }
            | EngineCommand::MoveStorage { .. }
            | EngineCommand::Recheck(_)
            | EngineCommand::AddTrackers { .. }
            | EngineCommand::SetCategories(_)
            | EngineCommand::SetTagList(_)
            | EngineCommand::SetAltSpeed(_)
    )
}

#[derive(Clone)]
struct Ctx {
    /// Used while no route is known (calls then fail with a readable error).
    fallback: MediaClient,
    events: std::sync::mpsc::Sender<EngineEvent>,
    wake: Wake,
    shared: Arc<Shared>,
}

impl Ctx {
    fn api(&self) -> MediaClient {
        match self.shared.status.route.load_full() {
            Some(r) => r.api.clone(),
            None => self.fallback.clone(),
        }
    }

    fn toast(&self, kind: ToastKind, text: String) {
        let _ = self.events.send(EngineEvent::Toast(kind, text));
        self.wake.wake();
    }

    /// Runs an API call in the background; failures become error toasts.
    fn call<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let me = self.clone();
        tokio::spawn(async move {
            if let Err(e) = fut.await {
                me.toast(ToastKind::Error, e);
            }
        });
    }

    fn post(&self, path: String, body: serde_json::Value) {
        let api = self.api();
        self.call(async move { api.send::<serde_json::Value, _>(reqwest::Method::POST, &path, &body).await.map(drop) });
    }

    fn patch(&self, hash: InfoHash, body: serde_json::Value) {
        let api = self.api();
        self.call(async move {
            api.send::<serde_json::Value, _>(
                reqwest::Method::PATCH,
                &format!("/api/v1/torrents/{}", hash.to_hex()),
                &body,
            )
            .await
            .map(drop)
        });
    }

    fn action(&self, action: &str, hashes: &[InfoHash]) {
        self.post("/api/v1/torrents/actions".into(), json!({ "action": action, "hashes": hashes }));
    }

    fn unavailable(&self) {
        self.toast(ToastKind::Warning, t!("That isn't available while connected to a server.").to_string());
    }

    fn command(&self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Add(r) => {
                let api = self.api();
                self.call(add(api, *r));
            }
            EngineCommand::Preview { req, source } => {
                let me = self.clone();
                tokio::spawn(async move {
                    let result = preview(&me.api(), source).await.map(Box::new);
                    let _ = me.events.send(EngineEvent::PreviewReady { req, result });
                    me.wake.wake();
                });
            }
            // A late preview answer is ignored by the dialog that closed.
            EngineCommand::CancelPreview(_) => {}
            EngineCommand::CreateTorrent(r) => {
                let _ = self.events.send(EngineEvent::Created {
                    id: r.id,
                    result: Err(t!("That isn't available while connected to a server.").to_string()),
                });
                self.wake.wake();
            }
            EngineCommand::RssRefresh(url) => {
                let me = self.clone();
                self.call(async move {
                    me.api()
                        .send::<serde_json::Value, _>(
                            reqwest::Method::POST,
                            "/api/v1/rss/refresh",
                            &json!({ "url": url }),
                        )
                        .await?;
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    refresh_rss(&me.api(), &me.shared).await;
                    Ok(())
                });
            }
            EngineCommand::RssMarkDownloaded { .. } => {}
            EngineCommand::UndoRemove(h) => self.action("undo_remove", &h),
            EngineCommand::CancelAdd(h) => self.action("remove", &[h]),
            EngineCommand::Pause(h) => self.action("pause", &h),
            EngineCommand::Resume(h) => self.action("resume", &h),
            EngineCommand::ForceStart(h) => self.action("force_start", &h),
            EngineCommand::Recheck(h) => self.action("recheck", &h),
            EngineCommand::PauseAll => self.action("pause_all", &[]),
            EngineCommand::ResumeAll => self.action("resume_all", &[]),
            EngineCommand::Delete { hashes, with_data } => {
                self.action(if with_data { "remove_with_data" } else { "remove" }, &hashes)
            }
            EngineCommand::QueueMove { hashes, mv } => {
                let a = match mv {
                    QueueMove::Top => "queue_top",
                    QueueMove::Up => "queue_up",
                    QueueMove::Down => "queue_down",
                    QueueMove::Bottom => "queue_bottom",
                };
                self.action(a, &hashes)
            }
            EngineCommand::QueuePlace { hashes, before } => {
                self.post("/api/v1/torrents/queue".into(), json!({ "hashes": hashes, "before": before }))
            }
            EngineCommand::SetFiles { hash, included } => self
                .post(format!("/api/v1/torrents/{}/files", hash.to_hex()), json!({ "files": included, "only": true })),
            EngineCommand::SetFilePriority { hash, files, priority } => self.post(
                format!("/api/v1/torrents/{}/files", hash.to_hex()),
                json!({ "files": files, "priority": priority }),
            ),
            EngineCommand::SetCategory { hashes, category } => {
                for h in hashes {
                    self.patch(h, json!({ "category": category.clone().unwrap_or_default() }));
                }
            }
            EngineCommand::SetTags { hashes, tags } => {
                for h in hashes {
                    self.patch(h, json!({ "tags": tags }));
                }
            }
            EngineCommand::ToggleTag { hashes, tag, on } => {
                let current: Vec<(InfoHash, Vec<String>)> = self.shared.with(|m| {
                    m.torrents.iter().filter(|t| hashes.contains(&t.hash)).map(|t| (t.hash, t.tags.clone())).collect()
                });
                for (h, mut tags) in current {
                    let had = tags.contains(&tag);
                    if on && !had {
                        tags.push(tag.clone());
                    } else if !on && had {
                        tags.retain(|t| t != &tag);
                    } else {
                        continue;
                    }
                    self.patch(h, json!({ "tags": tags }));
                }
            }
            EngineCommand::Rename { hash, name } => self.patch(hash, json!({ "name": name.unwrap_or_default() })),
            EngineCommand::SetTorrentLimits { hashes, down_kib, up_kib } => {
                for h in hashes {
                    self.patch(h, json!({ "down_limit_kib": down_kib, "up_limit_kib": up_kib }));
                }
            }
            EngineCommand::SetSeedLimits { hashes, ratio, minutes } => {
                for h in hashes {
                    self.patch(h, json!({ "seed": { "ratio": ratio, "minutes": minutes } }));
                }
            }
            EngineCommand::MoveStorage { hashes, to } => {
                self.post("/api/v1/torrents/move".into(), json!({ "hashes": hashes, "to": to.display().to_string() }))
            }
            EngineCommand::AddTrackers { hash, trackers } => {
                self.post(format!("/api/v1/torrents/{}/trackers", hash.to_hex()), json!({ "trackers": trackers }))
            }
            EngineCommand::SetCategories(cats) => {
                let labels = wire::labels(&Snapshot { categories: cats, ..Default::default() });
                let api = self.api();
                self.call(async move {
                    api.send::<serde_json::Value, _>(
                        reqwest::Method::PUT,
                        "/api/v1/torrents/categories",
                        &labels.categories,
                    )
                    .await
                    .map(drop)
                });
            }
            EngineCommand::SetTagList(tags) => {
                let api = self.api();
                self.call(async move {
                    api.send::<serde_json::Value, _>(reqwest::Method::PUT, "/api/v1/torrents/tags", &tags)
                        .await
                        .map(drop)
                });
            }
            EngineCommand::SetAltSpeed(on) => self.post("/api/v1/server/alt-speed".into(), json!({ "on": on })),
            EngineCommand::Detail { hash, reply } => {
                let api = self.api();
                tokio::spawn(async move {
                    let d = api.get::<p::TorrentDetail>(&format!("/api/v1/torrents/{}/detail", hash.to_hex())).await;
                    let _ = reply.send(d.ok().map(|d| wire::detail_view(&d)));
                });
            }
            EngineCommand::FileList { hash, reply } => {
                let api = self.api();
                tokio::spawn(async move {
                    let d = api.get::<p::TorrentDetail>(&format!("/api/v1/torrents/{}/detail", hash.to_hex())).await;
                    let _ = reply.send(d.ok().map(|d| wire::detail_view(&d).files));
                });
            }
            // The server's own settings are edited in its web interface; this app's settings
            // (appearance, behaviour) stay local.
            EngineCommand::ApplySettings(_) => {}
            EngineCommand::ReloadIpFilter | EngineCommand::StopStream(_) | EngineCommand::AllowStream { .. } => {
                self.unavailable()
            }
            EngineCommand::SetDetailTarget(_) | EngineCommand::Shutdown(_) => {}
            // The server keeps its own data; nothing to save here.
            EngineCommand::Flush(done) => {
                let _ = done.send(());
            }
            #[cfg(debug_assertions)]
            #[expect(clippy::panic, reason = "debug-only hook that tests crash reports")]
            EngineCommand::DevPanic => panic!("CORRENT_DEV_PANIC=engine (remote)"),
        }
    }
}

/// Uploads a .torrent (from a file, bytes or a preview) or sends the link.
async fn add(api: MediaClient, r: AddRequest) -> Result<(), String> {
    let save_path = r.save_path.display().to_string();
    let mut opts = json!({
        "category": r.category,
        "tags": r.tags,
        "paused": r.paused,
        "create_subfolder": r.create_subfolder,
        "add_to_top": r.add_to_top,
        "only_files": r.only_files,
    });
    if !save_path.is_empty()
        && let Some(o) = opts.as_object_mut()
    {
        o.insert("save_path".into(), json!(save_path));
    }
    let file = match (r.torrent_bytes, r.source) {
        (Some(b), s) => Some((s.describe(), b)),
        (None, AddSource::Bytes { name, bytes }) => Some((name, bytes)),
        (None, AddSource::File(p)) => {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file.torrent".into());
            Some((
                name,
                corrent_core::util::read_capped_async(&p, corrent_core::util::MAX_TORRENT_BYTES)
                    .await
                    .map_err(|e| format!("{}: {e}", p.display()))?,
            ))
        }
        (None, AddSource::Magnet(u) | AddSource::Url(u)) => {
            if let Some(o) = opts.as_object_mut() {
                o.insert("urls".into(), json!([u]));
            }
            None
        }
    };
    match file {
        Some((name, bytes)) => {
            let name = if name.ends_with(".torrent") { name } else { format!("{name}.torrent") };
            let form = reqwest::multipart::Form::new()
                .part("file", reqwest::multipart::Part::bytes(bytes).file_name(name))
                .text("options", opts.to_string());
            api.multipart::<serde_json::Value>("/api/v1/torrents", form).await.map(drop)
        }
        None => api.send::<serde_json::Value, _>(reqwest::Method::POST, "/api/v1/torrents", &opts).await.map(drop),
    }
}

async fn preview(api: &MediaClient, source: AddSource) -> Result<Preview, String> {
    let body = match &source {
        AddSource::Magnet(u) | AddSource::Url(u) => json!({ "url": u }),
        AddSource::Bytes { bytes, .. } => json!({ "torrent": base64::engine::general_purpose::STANDARD.encode(bytes) }),
        AddSource::File(p) => {
            let bytes = corrent_core::util::read_capped_async(p, corrent_core::util::MAX_TORRENT_BYTES)
                .await
                .map_err(|e| format!("{}: {e}", p.display()))?;
            json!({ "torrent": base64::engine::general_purpose::STANDARD.encode(bytes) })
        }
    };
    let p: p::TorrentPreview = api.send(reqwest::Method::POST, "/api/v1/torrents/preview", &body).await?;
    Ok(Preview {
        info_hash: p.info_hash,
        name: p.name,
        files: p.files.into_iter().map(|f| PreviewFile { components: f.path, len: f.len }).collect(),
        total_size: p.total_size,
        piece_count: p.piece_count,
        piece_length: p.piece_length,
        torrent_bytes: base64::engine::general_purpose::STANDARD.decode(p.torrent).map_err(|e| e.to_string())?,
        already_added: p.already_added,
        comment: p.comment,
    })
}

async fn refresh_rss(api: &MediaClient, shared: &Arc<Shared>) {
    if let Ok(v) = api.get::<corrent_core::rss::RssView>("/api/v1/rss").await {
        shared.with(|m| m.rss = Arc::new(v));
        shared.publish();
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Ws = tokio_tungstenite::WebSocketStream<Box<dyn Io>>;

fn tls_config(ca: Option<&str>) -> Result<rustls::ClientConfig, String> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;
    use rustls_platform_verifier::BuilderVerifierExt;
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let mut config = match ca {
        Some(pem) => {
            let mut roots = rustls::RootCertStore::empty();
            for cert in CertificateDer::pem_slice_iter(pem.as_bytes()) {
                roots.add(cert.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        None => builder.with_platform_verifier().map_err(|e| e.to_string())?.with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// Opens the realtime channel (the error is shown to people).
async fn connect_ws(base: &str, ca: Option<&str>, token: &str) -> Result<Ws, String> {
    let u = url::Url::parse(base).map_err(|e| e.to_string())?;
    let host = match u.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        Some(url::Host::Ipv4(a)) => a.to_string(),
        Some(url::Host::Ipv6(a)) => a.to_string(),
        None => return Err(t!("The server address isn't valid.").to_string()),
    };
    let port = u.port_or_known_default().unwrap_or(8443);
    let tcp = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| t!("The server didn't answer.").to_string())?
        .map_err(|e| t!("Can't reach the server: {e}", e = e))?;
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Io> = if u.scheme() == "https" {
        let config = tls_config(ca)?;
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .map_err(|e| t!("Secure connection failed: {e}", e = e))?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let ws_url = format!("{}/api/v1/events", base.replacen("http", "ws", 1));
    let mut req = ws_url.into_client_request().map_err(|e| e.to_string())?;
    req.headers_mut().insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().map_err(|_| "bad token".to_string())?,
    );
    match tokio_tungstenite::client_async(req, stream).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) if res.status() == 401 => {
            Err(t!("The server no longer accepts this device. Pair it again.").to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn decode(msg: Message) -> Option<Result<Event, ()>> {
    match msg {
        Message::Text(t) => Some(serde_json::from_str(&t).map_err(|_| ())),
        Message::Binary(b) => {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&b[..]), &mut out).ok()?;
            Some(serde_json::from_slice(&out).map_err(|_| ()))
        }
        Message::Close(_) => None,
        _ => Some(Err(())),
    }
}

async fn send(ws: &mut Ws, m: &ClientMessage) -> bool {
    let Ok(text) = serde_json::to_string(m) else { return false };
    ws.send(Message::Text(text.into())).await.is_ok()
}

/// Keeps the realtime channel open (reconnecting with backoff) and the mirror current.
async fn ws_loop(
    server: ServerEntry,
    shared: Arc<Shared>,
    events: std::sync::mpsc::Sender<EngineEvent>,
    mut target: tokio::sync::watch::Receiver<Option<InfoHash>>,
) {
    let mut delay = Duration::from_secs(1);
    loop {
        // A fresh route after every failure (the LAN may be back, or gone).
        let route = match resolve(&server).await {
            Ok(r) => {
                let r = Arc::new(r);
                shared.status.route.store(Some(r.clone()));
                r
            }
            Err(e) => {
                shared.status.route.store(None);
                tracing::info!("server {}: {e}", server.name);
                shared.set_connected(false, Some(e));
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        let api = route.api.clone();
        let problem = match connect_ws(&route.url, route.ca.as_deref(), &server.token).await {
            Ok(mut ws) => {
                delay = Duration::from_secs(1);
                let mut subscribed = *target.borrow_and_update();
                let mut topics = vec![Topic::Torrents, Topic::Session];
                topics.extend(subscribed.map(Topic::Torrent));
                if send(&mut ws, &ClientMessage::Subscribe { topics }).await {
                    shared.set_connected(true, None);
                    refresh_rss(&api, &shared).await;
                    if let Ok(s) = api.get::<Settings>("/api/v1/server/settings").await {
                        shared.status.settings.store(Some(Arc::new(s)));
                    }
                }
                let mut ping = tokio::time::interval(Duration::from_secs(30));
                let mut rss = tokio::time::interval(Duration::from_secs(60));
                // The server sends session stats every second and answers pings: silence means
                // a dead connection (sleep, network change) that TCP hasn't noticed yet.
                let mut last_heard = std::time::Instant::now();
                loop {
                    if last_heard.elapsed() > Duration::from_secs(75) {
                        break t!("The connection to the server was lost.").to_string();
                    }
                    tokio::select! {
                        m = ws.next() => match m {
                            Some(Ok(m)) => {
                                last_heard = std::time::Instant::now();
                                match decode(m) {
                                    Some(Ok(Event::Bye)) => break t!("The server is restarting.").to_string(),
                                    Some(Ok(ev)) => on_event(&shared, &events, ev),
                                    Some(Err(())) => {}
                                    None => break t!("The server closed the connection.").to_string(),
                                }
                            }
                            Some(Err(e)) => break e.to_string(),
                            None => break t!("The server closed the connection.").to_string(),
                        },
                        Ok(()) = target.changed() => {
                            let now = *target.borrow_and_update();
                            if let Some(old) = subscribed.filter(|o| Some(*o) != now) {
                                send(&mut ws, &ClientMessage::Unsubscribe { topics: vec![Topic::Torrent(old)] }).await;
                            }
                            if let Some(new) = now.filter(|n| Some(*n) != subscribed) {
                                send(&mut ws, &ClientMessage::Subscribe { topics: vec![Topic::Torrent(new)] }).await;
                            }
                            subscribed = now;
                        }
                        _ = ping.tick() => {
                            if !send(&mut ws, &ClientMessage::Ping { id: 0 }).await {
                                break t!("The connection to the server was lost.").to_string();
                            }
                        }
                        _ = rss.tick() => refresh_rss(&api, &shared).await,
                    }
                }
            }
            Err(e) => e,
        };
        tracing::info!("server {}: {problem}", server.name);
        shared.set_connected(false, Some(problem));
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

fn on_event(shared: &Arc<Shared>, events: &std::sync::mpsc::Sender<EngineEvent>, ev: Event) {
    let mut changed = true;
    match ev {
        Event::TorrentsFull { torrents, labels } => shared.with(|m| {
            m.torrents = torrents;
            m.labels = labels;
        }),
        Event::TorrentsDelta { delta } => shared.with(|m| p::apply(&mut m.torrents, &delta)),
        Event::Labels { labels } => shared.with(|m| m.labels = labels),
        Event::Session { stats } => shared.with(|m| {
            push_capped(&mut m.history, (stats.down_bps, stats.up_bps), HISTORY_LEN);
            let live: std::collections::HashSet<InfoHash> = m.torrents.iter().map(|t| t.hash).collect();
            m.row_history.retain(|h, _| live.contains(h));
            for t in &m.torrents {
                let row = m.row_history.entry(t.hash).or_default();
                if row.len() >= ROW_HISTORY_LEN {
                    row.remove(0);
                }
                row.push(((t.down_bps / 1024) as u32, (t.up_bps / 1024) as u32));
            }
            if let Some(t) = m.detail_target.and_then(|h| m.torrents.iter().find(|t| t.hash == h)) {
                let sample = (t.down_bps, t.up_bps);
                push_capped(&mut m.detail_history, sample, HISTORY_LEN);
            }
            m.stats = Some(stats);
        }),
        Event::TorrentDetail { detail } => shared.with(|m| {
            if Some(detail.hash) == m.detail_target {
                m.detail = Some(detail);
            }
        }),
        Event::Notice { severity, text, .. } => {
            let kind = match severity {
                Severity::Info => ToastKind::Info,
                Severity::Success => ToastKind::Success,
                Severity::Warning => ToastKind::Warning,
                Severity::Error => ToastKind::Error,
            };
            let _ = events.send(EngineEvent::Toast(kind, text));
            changed = false;
        }
        Event::Added { hash, name } => {
            let _ = events.send(EngineEvent::Added { hash, name });
        }
        Event::Completed { hash, name, category } => {
            let (content_path, save_path, size) = shared.with(|m| {
                m.torrents
                    .iter()
                    .find(|t| t.hash == hash)
                    .map(|t| (t.content_path.clone().into(), t.save_path.clone().into(), t.size))
                    .unwrap_or_default()
            });
            let _ = events.send(EngineEvent::Completed { hash, name, category, content_path, save_path, size });
        }
        Event::Removed { hash, name, undoable } => {
            if undoable {
                shared.with(|m| m.undoable.push((hash, name.clone(), Instant::now())));
            }
            let _ = events.send(EngineEvent::Removed { hash, name, undoable });
        }
        _ => changed = false,
    }
    if changed {
        shared.publish();
    } else {
        shared.wake.wake();
    }
}

fn push_capped<T>(q: &mut VecDeque<T>, v: T, cap: usize) {
    if q.len() >= cap {
        q.pop_front();
    }
    q.push_back(v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use corrent_core::engine::testkit;
    use corrent_core::engine::{EngineHandle, Wake};

    async fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The app drives a server's engine through the API and mirrors it over the WebSocket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mirrors_and_drives_a_server() {
        let dir = testkit::temp_dir("remote-link");
        let seeder = testkit::seeder(&dir).await;
        let server_engine = testkit::start(&dir);
        let settings = Arc::new(ArcSwap::from_pointee(testkit::settings(&dir)));
        let local = corrent_server::local::start(server_engine.engine.clone(), settings, server_engine.paths.clone())
            .await
            .unwrap();
        corrent_server::forward_engine_events(server_engine.events, local.state.clone());

        let entry = ServerEntry {
            id: local.state.server_id.clone(),
            name: "Test".into(),
            url: local.base_url.clone(),
            token: local.token.clone(),
            ..Default::default()
        };
        let (backend, status) = backend(entry);
        let (tx, events) = std::sync::mpsc::channel();
        let paths = corrent_core::config::Paths { data_dir: dir.join("client"), portable: true };
        let client = EngineHandle::with_backend(&tokio::runtime::Handle::current(), paths, Wake::noop(), tx, backend);
        assert!(client.is_remote());
        until("connection", || status.connected.load(std::sync::atomic::Ordering::Relaxed).then_some(())).await;

        // A preview goes through the server's engine.
        client.send(EngineCommand::Preview {
            req: 7,
            source: AddSource::Bytes { name: "data.torrent".into(), bytes: seeder.bytes.clone() },
        });
        let preview = until("preview", || match events.try_recv() {
            Ok(EngineEvent::PreviewReady { req: 7, result }) => Some(result),
            _ => None,
        })
        .await
        .unwrap();
        assert_eq!(preview.info_hash, seeder.hash);
        assert_eq!(preview.files.len(), 1);

        // Added paused, with the preview's bytes; the mirror shows it.
        client.send(EngineCommand::Add(Box::new(AddRequest {
            source: AddSource::Magnet(format!("magnet:?xt=urn:btih:{}", seeder.hash.to_hex())),
            torrent_bytes: Some(preview.torrent_bytes.clone()),
            save_path: dir.join("dl"),
            create_subfolder: true,
            category: None,
            tags: vec!["remote".into()],
            paused: true,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
        })));
        let t = until("mirrored torrent", || {
            client.snapshot().torrents.iter().find(|t| t.hash == seeder.hash && !t.pending).cloned()
        })
        .await;
        assert_eq!(t.tags, vec!["remote".to_string()]);
        assert_eq!(t.added_by.as_deref(), Some(local.user.as_str()));

        // Commands become API calls.
        client.send(EngineCommand::ToggleTag { hashes: vec![seeder.hash], tag: "more".into(), on: true });
        until("tag", || {
            client.snapshot().torrents.iter().find(|t| t.hash == seeder.hash).filter(|t| t.tags.len() == 2).map(drop)
        })
        .await;
        client.send(EngineCommand::SetDetailTarget(Some(seeder.hash)));
        until("detail", || client.snapshot().detail.as_ref().filter(|d| d.files.len() == 1).map(drop)).await;
        client.send(EngineCommand::Delete { hashes: vec![seeder.hash], with_data: false });
        until("removed", || client.snapshot().torrents.iter().all(|t| t.hash != seeder.hash).then_some(())).await;
        assert!(client.snapshot().session.uptime_secs < 3600, "session stats arrive");

        // Unavailable features answer instead of hanging.
        client.send(EngineCommand::ReloadIpFilter);
        until("unavailable toast", || match events.try_recv() {
            Ok(EngineEvent::Toast(ToastKind::Warning, _)) => Some(()),
            _ => None,
        })
        .await;

        let c2 = client.clone();
        tokio::task::spawn_blocking(move || c2.shutdown_blocking(Duration::from_secs(5))).await.unwrap();
        let e2 = server_engine.engine.clone();
        tokio::task::spawn_blocking(move || e2.shutdown_blocking(Duration::from_secs(10))).await.unwrap();
        drop(local);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The server's address is unreachable: the link falls back to Corrent Connect.
    #[cfg(feature = "p2p")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn falls_back_to_corrent_connect() {
        CONNECT_RELAY.store(false, std::sync::atomic::Ordering::Relaxed);
        let dir = testkit::temp_dir("remote-connect");
        let server_engine = testkit::start(&dir);
        let settings = Arc::new(ArcSwap::from_pointee(testkit::settings(&dir)));
        let local = corrent_server::local::start(server_engine.engine.clone(), settings, server_engine.paths.clone())
            .await
            .unwrap();
        let p2p = corrent_server::p2p::serve(
            corrent_server::router(local.state.clone()),
            corrent_server::p2p::load_or_create_key(&dir).unwrap(),
            false,
            None,
            Some("127.0.0.1:0".parse().unwrap()),
        )
        .await
        .unwrap();
        let entry = ServerEntry {
            id: local.state.server_id.clone(),
            name: "Far away".into(),
            // Nothing listens here.
            url: "http://127.0.0.1:9".into(),
            token: local.token.clone(),
            connect_id: p2p.id().to_string(),
            connect_addrs: p2p.endpoint.bound_sockets().iter().map(|a| a.to_string()).collect(),
            ..Default::default()
        };
        let (backend, status) = backend(entry);
        let (tx, _events) = std::sync::mpsc::channel();
        let paths = corrent_core::config::Paths { data_dir: dir.join("client"), portable: true };
        let client = EngineHandle::with_backend(&tokio::runtime::Handle::current(), paths, Wake::noop(), tx, backend);
        until("connection over Connect", || status.connected.load(std::sync::atomic::Ordering::Relaxed).then_some(()))
            .await;
        let route = status.route.load_full().unwrap();
        assert!(route.via_connect && route.url.starts_with("http://127.0.0.1:"));
        // Plain API calls go the same way.
        let info: corrent_proto::server::ServerInfo = route.api.get("/api/v1/server/info").await.unwrap();
        assert_eq!(info.id, local.state.server_id);
        let c2 = client.clone();
        tokio::task::spawn_blocking(move || c2.shutdown_blocking(Duration::from_secs(5))).await.unwrap();
        drop(p2p);
        let e2 = server_engine.engine.clone();
        tokio::task::spawn_blocking(move || e2.shutdown_blocking(Duration::from_secs(10))).await.unwrap();
        drop(local);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Changes made while the server is down are sent once it is reachable.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queues_changes_while_offline() {
        let dir = testkit::temp_dir("remote-offline");
        let seeder = testkit::seeder(&dir).await;
        let server_engine = testkit::start(&dir);
        let settings = Arc::new(ArcSwap::from_pointee(testkit::settings(&dir)));
        let local = corrent_server::local::start(server_engine.engine.clone(), settings, server_engine.paths.clone())
            .await
            .unwrap();
        corrent_server::forward_engine_events(server_engine.events, local.state.clone());
        // A port with nothing behind it yet.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let entry = ServerEntry {
            id: local.state.server_id.clone(),
            name: "Later".into(),
            url: format!("http://127.0.0.1:{port}"),
            token: local.token.clone(),
            ..Default::default()
        };
        let (backend, status) = backend(entry);
        let (tx, events) = std::sync::mpsc::channel();
        let paths = corrent_core::config::Paths { data_dir: dir.join("client"), portable: true };
        let client = EngineHandle::with_backend(&tokio::runtime::Handle::current(), paths, Wake::noop(), tx, backend);
        until("a failed first attempt", || status.problem.load_full().map(drop)).await;
        client.send(EngineCommand::Add(Box::new(AddRequest {
            source: AddSource::Bytes { name: "data.torrent".into(), bytes: seeder.bytes.clone() },
            torrent_bytes: None,
            save_path: dir.join("dl"),
            create_subfolder: true,
            category: None,
            tags: vec![],
            paused: true,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
        })));
        until("the queued notice", || match events.try_recv() {
            Ok(EngineEvent::Toast(ToastKind::Info, _)) => Some(()),
            _ => None,
        })
        .await;
        // The server comes up on that port.
        let _listener = corrent_server::serve_http(
            corrent_server::router(local.state.clone()),
            std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        )
        .await
        .unwrap();
        until("the queued add arrived", || {
            client.snapshot().torrents.iter().any(|t| t.hash == seeder.hash).then_some(())
        })
        .await;
        let c2 = client.clone();
        tokio::task::spawn_blocking(move || c2.shutdown_blocking(Duration::from_secs(5))).await.unwrap();
        let e2 = server_engine.engine.clone();
        tokio::task::spawn_blocking(move || e2.shutdown_blocking(Duration::from_secs(10))).await.unwrap();
        drop(local);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
