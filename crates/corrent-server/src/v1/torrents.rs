//! Torrents: list, details, add, actions, edits, files, trackers, categories and tags.
//!
//! `torrents.manage_all` may act on any torrent; `torrents.manage_own` only on torrents the
//! same user added through the server.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::header;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use corrent_core::engine::types::{AddSource, wire};
use corrent_core::engine::{AddRequest, EngineCommand};
use corrent_core::store::{CategoryInfo, QueueMove};
use corrent_proto::auth::Perms;
use corrent_proto::torrent::{Labels, Torrent, TorrentDetail};
use corrent_proto::{ErrorCode, InfoHash};
use serde::{Deserialize, Serialize};

use super::{ApiErr, ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/torrents", get(list).post(add).layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)))
        .route("/torrents/preview", post(preview).layer(axum::extract::DefaultBodyLimit::max(24 * 1024 * 1024)))
        .route("/torrents/queue", post(queue_place))
        .route("/torrents/actions", post(action))
        .route("/torrents/move", post(move_storage))
        .route("/torrents/categories", get(labels).put(set_categories))
        .route("/torrents/tags", put(set_tags))
        .route("/torrents/{hash}", get(one).patch(edit))
        .route("/torrents/{hash}/detail", get(detail))
        .route("/torrents/{hash}/files", post(files))
        .route("/torrents/{hash}/trackers", post(trackers))
}

#[derive(Serialize, Deserialize)]
pub struct TorrentList {
    pub torrents: Vec<Torrent>,
    pub labels: Labels,
}

fn ok() -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::json!({})))
}

fn parse_hash(s: &str) -> Result<InfoHash, ApiErr> {
    InfoHash::from_hex(s).ok_or_else(|| err(ErrorCode::BadRequest, "not an info hash"))
}

/// Users who may only manage their own torrents see every torrent but act only on theirs.
fn may_manage(st: &AppState, auth: &Auth, hashes: &[InfoHash]) -> Result<(), ApiErr> {
    if auth.0.can(Perms::TORRENTS_MANAGE_ALL) {
        return Ok(());
    }
    if !auth.0.can(Perms::TORRENTS_MANAGE_OWN) {
        return Err(err(ErrorCode::Forbidden, "permission denied"));
    }
    let snap = st.engine.snapshot();
    let me = auth.name();
    for h in hashes {
        let owned = snap.torrents.iter().find(|t| t.hash == *h).is_some_and(|t| t.added_by.as_deref() == Some(me));
        if !owned {
            return Err(err(ErrorCode::Forbidden, "not your torrent").with_param_hash(*h));
        }
    }
    Ok(())
}

trait WithHash {
    fn with_param_hash(self, h: InfoHash) -> Self;
}

impl WithHash for ApiErr {
    fn with_param_hash(self, h: InfoHash) -> Self {
        ApiErr(self.0.with("hash", h.to_hex()))
    }
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<TorrentList> {
    let snap = st.engine.snapshot();
    let torrents = wire::torrents(&snap).into_iter().map(|t| super::redact_torrent(&auth.0, t)).collect();
    Ok(Json(TorrentList { torrents, labels: wire::labels(&snap) }))
}

async fn labels(State(st): State<Arc<AppState>>, _auth: Auth) -> ApiResult<Labels> {
    Ok(Json(wire::labels(&st.engine.snapshot())))
}

async fn one(State(st): State<Arc<AppState>>, auth: Auth, Path(hash): Path<String>) -> ApiResult<Torrent> {
    let h = parse_hash(&hash)?;
    let snap = st.engine.snapshot();
    let t = snap.torrents.iter().find(|t| t.hash == h).ok_or_else(|| err(ErrorCode::NotFound, "no such torrent"))?;
    Ok(Json(super::redact_torrent(&auth.0, wire::torrent(t))))
}

async fn detail(State(st): State<Arc<AppState>>, _auth: Auth, Path(hash): Path<String>) -> ApiResult<TorrentDetail> {
    let h = parse_hash(&hash)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.engine.send(EngineCommand::Detail { hash: h, reply: tx });
    let d = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
        .await
        .map_err(|_| err(ErrorCode::EngineUnavailable, "engine did not answer"))?
        .map_err(|_| err(ErrorCode::EngineUnavailable, "engine stopped"))?
        .ok_or_else(|| err(ErrorCode::NotFound, "no such torrent"))?;
    Ok(Json(wire::detail(&d)))
}

/// Options for adding torrents (JSON body, or the `options` field of a multipart upload whose
/// file parts are `.torrent` files).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct AddOptions {
    /// Magnet links, http(s) URLs of .torrent files, or bare info hashes.
    pub urls: Vec<String>,
    pub save_path: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub paused: Option<bool>,
    pub create_subfolder: Option<bool>,
    pub add_to_top: bool,
    /// Download only these files (indexes in the torrent).
    pub only_files: Option<Vec<usize>>,
}

#[derive(Serialize, Deserialize)]
pub struct Added {
    pub count: usize,
}

const MAX_TORRENT_FILE: usize = 16 * 1024 * 1024;

async fn add(State(st): State<Arc<AppState>>, auth: Auth, ip: ClientIp, req: Request) -> ApiResult<Added> {
    auth.require(Perms::TORRENTS_ADD)?;
    let ctype = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut opts = AddOptions::default();
    let mut sources: Vec<AddSource> = Vec::new();
    if ctype.starts_with("multipart/form-data") {
        let mut mp = Multipart::from_request(req, &()).await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
        while let Some(f) = mp.next_field().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))? {
            match (f.name().unwrap_or("").to_string(), f.file_name().map(str::to_string)) {
                (_, Some(name)) => {
                    let bytes = f.bytes().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
                    if bytes.len() > MAX_TORRENT_FILE {
                        return Err(err(ErrorCode::PayloadTooLarge, "torrent file too large"));
                    }
                    sources.push(AddSource::Bytes { name, bytes: bytes.to_vec() });
                }
                (n, None) if n == "options" => {
                    let text = f.text().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
                    opts = serde_json::from_str(&text).map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
                }
                _ => {}
            }
        }
    } else {
        let Json(o) =
            Json::<AddOptions>::from_request(req, &()).await.map_err(|e| err(ErrorCode::BadRequest, e.body_text()))?;
        opts = o;
    }
    for u in opts.urls.iter().map(|u| u.trim()).filter(|u| !u.is_empty()) {
        if u.starts_with("magnet:") {
            sources.push(AddSource::Magnet(u.to_string()));
        } else if u.starts_with("http://") || u.starts_with("https://") {
            sources.push(AddSource::Url(u.to_string()));
        } else if let Some(h) = InfoHash::from_hex(u) {
            sources.push(AddSource::Magnet(format!("magnet:?xt=urn:btih:{}", h.to_hex())));
        } else {
            return Err(err(ErrorCode::BadRequest, "not a magnet link, URL or info hash").with_text("url", u));
        }
    }
    if sources.is_empty() {
        return Err(err(ErrorCode::BadRequest, "nothing to add"));
    }
    if !crate::paths::unconfined(&auth) {
        for s in &sources {
            if let AddSource::Url(u) = s {
                crate::net::check_public_url(u).await.map_err(|e| err(ErrorCode::Forbidden, e).with_text("url", u))?;
            }
        }
    }
    let settings = st.settings.load_full();
    let snap = st.engine.snapshot();
    let category = opts.category.filter(|c| !c.trim().is_empty());
    let save_path = match opts.save_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => crate::paths::check(&st, &auth, &PathBuf::from(p))?,
        None => category
            .as_ref()
            .and_then(|c| snap.categories.get(c))
            .and_then(|c| c.save_path.clone())
            .unwrap_or_else(|| settings.downloads.save_path.clone()),
    };
    let count = sources.len();
    for source in sources {
        tracing::info!("API: {} adds {}", auth.name(), source.describe());
        st.engine.send(EngineCommand::Add(Box::new(AddRequest {
            source,
            torrent_bytes: None,
            save_path: save_path.clone(),
            create_subfolder: opts.create_subfolder.unwrap_or(settings.downloads.create_subfolder),
            category: category.clone(),
            tags: opts.tags.clone(),
            paused: opts.paused.unwrap_or(settings.downloads.start_paused),
            only_files: opts.only_files.clone(),
            add_to_top: opts.add_to_top,
            initial_peers: None,
            added_by: Some(auth.name().to_string()),
        })));
    }
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "torrent_add", &format!("{count}"), true, "").await;
    Ok(Json(Added { count }))
}

/// Engine request ids of API previews (the desktop app's own previews count up from 0).
pub const PREVIEW_IDS: u64 = 1 << 62;
static NEXT_PREVIEW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(PREVIEW_IDS);

#[derive(Deserialize, Default)]
#[serde(default)]
struct PreviewRequest {
    /// A magnet link, http(s) URL of a .torrent file, or bare info hash…
    url: String,
    /// …or a .torrent file (base64).
    torrent: Option<String>,
}

/// Resolves a magnet link or .torrent URL into its name and files (magnets can take a while:
/// up to two minutes).
async fn preview(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(r): Json<PreviewRequest>,
) -> ApiResult<corrent_proto::torrent::TorrentPreview> {
    use base64::Engine as _;
    auth.require(Perms::TORRENTS_ADD)?;
    let u = r.url.trim();
    let source = if let Some(b64) = &r.torrent {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|_| err(ErrorCode::BadRequest, "not base64"))?;
        if bytes.len() > MAX_TORRENT_FILE {
            return Err(err(ErrorCode::PayloadTooLarge, "torrent file too large"));
        }
        AddSource::Bytes { name: "preview.torrent".into(), bytes }
    } else if u.starts_with("magnet:") {
        AddSource::Magnet(u.to_string())
    } else if u.starts_with("http://") || u.starts_with("https://") {
        if !crate::paths::unconfined(&auth) {
            crate::net::check_public_url(u).await.map_err(|e| err(ErrorCode::Forbidden, e).with_text("url", u))?;
        }
        AddSource::Url(u.to_string())
    } else if let Some(h) = InfoHash::from_hex(u) {
        AddSource::Magnet(format!("magnet:?xt=urn:btih:{}", h.to_hex()))
    } else {
        return Err(err(ErrorCode::BadRequest, "not a magnet link, URL or info hash").with_text("url", u));
    };
    let req = NEXT_PREVIEW.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.previews.lock().unwrap_or_else(|e| e.into_inner()).insert(req, tx);
    st.engine.send(EngineCommand::Preview { req, source });
    let res = tokio::time::timeout(std::time::Duration::from_secs(120), rx).await;
    st.previews.lock().unwrap_or_else(|e| e.into_inner()).remove(&req);
    let p = match res {
        Ok(Ok(Ok(p))) => p,
        Ok(Ok(Err(e))) => return Err(err(ErrorCode::BadRequest, e)),
        Ok(Err(_)) => return Err(err(ErrorCode::EngineUnavailable, "engine stopped")),
        Err(_) => {
            st.engine.send(EngineCommand::CancelPreview(req));
            return Err(err(ErrorCode::Timeout, "no metadata yet"));
        }
    };
    Ok(Json(corrent_proto::torrent::TorrentPreview {
        info_hash: p.info_hash,
        name: p.name,
        files: p
            .files
            .into_iter()
            .map(|f| corrent_proto::torrent::PreviewFile { path: f.components, len: f.len })
            .collect(),
        total_size: p.total_size,
        piece_count: p.piece_count,
        piece_length: p.piece_length,
        already_added: p.already_added,
        comment: p.comment,
        torrent: base64::engine::general_purpose::STANDARD.encode(&p.torrent_bytes),
    }))
}

trait WithText {
    fn with_text(self, k: &str, v: &str) -> Self;
}

impl WithText for ApiErr {
    fn with_text(self, k: &str, v: &str) -> Self {
        ApiErr(self.0.with(k, v))
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Pause,
    Resume,
    ForceStart,
    Recheck,
    Remove,
    RemoveWithData,
    QueueTop,
    QueueUp,
    QueueDown,
    QueueBottom,
    PauseAll,
    ResumeAll,
    UndoRemove,
}

#[derive(Deserialize)]
struct ActionRequest {
    action: Action,
    #[serde(default)]
    hashes: Vec<InfoHash>,
    /// Remove with data even though library files are inside.
    #[serde(default)]
    force: bool,
}

async fn action(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(r): Json<ActionRequest>,
) -> ApiResult<serde_json::Value> {
    let hashes = r.hashes;
    match r.action {
        Action::PauseAll | Action::ResumeAll | Action::UndoRemove => auth.require(Perms::TORRENTS_MANAGE_ALL)?,
        _ => {
            if hashes.is_empty() {
                return Err(err(ErrorCode::BadRequest, "no torrents given"));
            }
            may_manage(&st, &auth, &hashes)?;
        }
    }
    // Library files are never deleted by accident with a torrent's data.
    if r.action == Action::RemoveWithData && !r.force {
        let snap = st.engine.snapshot();
        let mut in_library = 0;
        for t in snap.torrents.iter().filter(|t| hashes.contains(&t.hash)) {
            in_library += st.library.files_under(&t.content_path).await.unwrap_or(0);
        }
        if in_library > 0 {
            return Err(ApiErr(
                corrent_proto::ApiError::new(ErrorCode::Conflict, "library files would be deleted")
                    .with("files", in_library),
            ));
        }
    }
    let queue = |mv| EngineCommand::QueueMove { hashes: hashes.clone(), mv };
    let cmd = match r.action {
        Action::Pause => EngineCommand::Pause(hashes.clone()),
        Action::Resume => EngineCommand::Resume(hashes.clone()),
        Action::ForceStart => EngineCommand::ForceStart(hashes.clone()),
        Action::Recheck => EngineCommand::Recheck(hashes.clone()),
        Action::Remove => EngineCommand::Delete { hashes: hashes.clone(), with_data: false },
        Action::RemoveWithData => EngineCommand::Delete { hashes: hashes.clone(), with_data: true },
        Action::QueueTop => queue(QueueMove::Top),
        Action::QueueUp => queue(QueueMove::Up),
        Action::QueueDown => queue(QueueMove::Down),
        Action::QueueBottom => queue(QueueMove::Bottom),
        Action::PauseAll => EngineCommand::PauseAll,
        Action::ResumeAll => EngineCommand::ResumeAll,
        Action::UndoRemove => {
            let snap = st.engine.snapshot();
            EngineCommand::UndoRemove(snap.undoable.iter().map(|(h, _)| *h).collect())
        }
    };
    st.engine.send(cmd);
    if matches!(r.action, Action::Remove | Action::RemoveWithData) {
        let what = hashes.iter().map(|h| h.to_hex()).collect::<Vec<_>>().join(",");
        let details = if r.action == Action::RemoveWithData { "with data" } else { "" };
        st.identity.audit(Some(auth.name()), &ip.0.to_string(), "torrent_remove", &what, true, details).await;
    }
    ok()
}

/// Changes to one torrent; absent fields stay as they are.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Edit {
    /// Empty string = back to the torrent's own name.
    name: Option<String>,
    /// Empty string = no category.
    category: Option<String>,
    tags: Option<Vec<String>>,
    down_limit_kib: Option<u32>,
    up_limit_kib: Option<u32>,
    /// Seeding limits: absent = unchanged; `null` inside `seed` = use the global setting.
    seed: Option<SeedLimits>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SeedLimits {
    ratio: Option<f32>,
    minutes: Option<u32>,
}

async fn edit(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(hash): Path<String>,
    Json(e): Json<Edit>,
) -> ApiResult<serde_json::Value> {
    let h = parse_hash(&hash)?;
    may_manage(&st, &auth, &[h])?;
    if let Some(n) = e.name {
        st.engine.send(EngineCommand::Rename { hash: h, name: Some(n).filter(|n| !n.trim().is_empty()) });
    }
    if let Some(c) = e.category {
        st.engine
            .send(EngineCommand::SetCategory { hashes: vec![h], category: Some(c).filter(|c| !c.trim().is_empty()) });
    }
    if let Some(t) = e.tags {
        st.engine.send(EngineCommand::SetTags { hashes: vec![h], tags: t });
    }
    if e.down_limit_kib.is_some() || e.up_limit_kib.is_some() {
        let snap = st.engine.snapshot();
        let cur = snap.torrents.iter().find(|t| t.hash == h);
        st.engine.send(EngineCommand::SetTorrentLimits {
            hashes: vec![h],
            down_kib: e.down_limit_kib.or(cur.map(|t| t.down_limit_kib)).unwrap_or(0),
            up_kib: e.up_limit_kib.or(cur.map(|t| t.up_limit_kib)).unwrap_or(0),
        });
    }
    if let Some(s) = e.seed {
        st.engine.send(EngineCommand::SetSeedLimits { hashes: vec![h], ratio: s.ratio, minutes: s.minutes });
    }
    ok()
}

#[derive(Deserialize)]
struct FilesRequest {
    files: Vec<usize>,
    /// Download only these files (the others are skipped)…
    #[serde(default)]
    only: bool,
    /// …or set their priority (1 high, 0 normal, -1 low).
    #[serde(default)]
    priority: Option<i8>,
}

async fn files(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(hash): Path<String>,
    Json(r): Json<FilesRequest>,
) -> ApiResult<serde_json::Value> {
    let h = parse_hash(&hash)?;
    may_manage(&st, &auth, &[h])?;
    if r.only {
        st.engine.send(EngineCommand::SetFiles { hash: h, included: r.files.clone() });
    }
    if let Some(p) = r.priority {
        st.engine.send(EngineCommand::SetFilePriority { hash: h, files: r.files, priority: p.clamp(-1, 1) });
    }
    ok()
}

#[derive(Deserialize)]
struct TrackersRequest {
    trackers: Vec<String>,
}

async fn trackers(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(hash): Path<String>,
    Json(r): Json<TrackersRequest>,
) -> ApiResult<serde_json::Value> {
    let h = parse_hash(&hash)?;
    may_manage(&st, &auth, &[h])?;
    let list: Vec<String> = r.trackers.into_iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    if list.iter().any(|t| url::Url::parse(t).is_err()) {
        return Err(err(ErrorCode::BadRequest, "not a tracker URL"));
    }
    st.engine.send(EngineCommand::AddTrackers { hash: h, trackers: list });
    ok()
}

#[derive(Deserialize)]
struct QueuePlace {
    hashes: Vec<InfoHash>,
    /// Put them before this torrent (absent: at the end of the queue).
    #[serde(default)]
    before: Option<InfoHash>,
}

/// Drag and drop in the queue.
async fn queue_place(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(r): Json<QueuePlace>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_MANAGE_ALL)?;
    st.engine.send(EngineCommand::QueuePlace { hashes: r.hashes, before: r.before });
    ok()
}

#[derive(Deserialize)]
struct MoveRequest {
    hashes: Vec<InfoHash>,
    to: String,
}

async fn move_storage(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(r): Json<MoveRequest>,
) -> ApiResult<serde_json::Value> {
    may_manage(&st, &auth, &r.hashes)?;
    let to = crate::paths::check(&st, &auth, &PathBuf::from(&r.to))?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "torrent_move", &r.to, true, "").await;
    st.engine.send(EngineCommand::MoveStorage { hashes: r.hashes, to });
    ok()
}

async fn set_categories(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(cats): Json<BTreeMap<String, corrent_proto::torrent::Category>>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_MANAGE_ALL)?;
    // A category's folder is a download destination: same confinement as save paths.
    for c in cats.values() {
        if let Some(p) = c.save_path.as_ref().filter(|p| !p.trim().is_empty()) {
            crate::paths::check(&st, &auth, &PathBuf::from(p))?;
        }
    }
    let map = cats
        .into_iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, c)| {
            (
                k,
                CategoryInfo {
                    save_path: c.save_path.filter(|p| !p.trim().is_empty()).map(PathBuf::from),
                    down_limit_kib: c.down_limit_kib,
                    up_limit_kib: c.up_limit_kib,
                },
            )
        })
        .collect();
    st.engine.send(EngineCommand::SetCategories(map));
    ok()
}

async fn set_tags(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(tags): Json<Vec<String>>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_MANAGE_ALL)?;
    st.engine.send(EngineCommand::SetTagList(tags.into_iter().filter(|t| !t.trim().is_empty()).collect()));
    ok()
}
