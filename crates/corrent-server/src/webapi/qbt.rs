//! qBittorrent Web API v2 endpoints (the subset download managers use).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::ApiState;
use crate::engine::types::{AddSource, InfoHash, TorrentState, TorrentView};
use crate::engine::{AddRequest, EngineCommand};

type St = State<Arc<ApiState>>;

/// The qBittorrent version we present ourselves as (clients gate features on it).
const APP_VERSION: &str = "v4.6.7";
const API_VERSION: &str = "2.9.3";
/// qBittorrent's "infinite" ETA.
const ETA_INF: u64 = 8_640_000;

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v2/auth/logout", post(logout))
        .route("/api/v2/app/version", get(|| async { APP_VERSION }))
        .route("/api/v2/app/webapiVersion", get(|| async { API_VERSION }))
        .route("/api/v2/app/buildInfo", get(build_info))
        .route("/api/v2/app/preferences", get(preferences))
        .route("/api/v2/app/defaultSavePath", get(default_save_path))
        .route("/api/v2/transfer/info", get(transfer_info))
        .route("/api/v2/sync/maindata", get(maindata))
        .route("/api/v2/torrents/info", get(info).post(info))
        .route("/api/v2/torrents/properties", get(properties).post(properties))
        .route("/api/v2/torrents/files", get(files).post(files))
        .route("/api/v2/torrents/categories", get(categories))
        .route("/api/v2/torrents/tags", get(tags))
        .route("/api/v2/torrents/add", post(add))
        .route("/api/v2/torrents/pause", post(pause))
        .route("/api/v2/torrents/stop", post(pause))
        .route("/api/v2/torrents/resume", post(resume))
        .route("/api/v2/torrents/start", post(resume))
        .route("/api/v2/torrents/delete", post(delete))
        .route("/api/v2/torrents/recheck", post(recheck))
        .route("/api/v2/torrents/setCategory", post(set_category))
        .route("/api/v2/torrents/createCategory", post(create_category))
        .route("/api/v2/torrents/editCategory", post(create_category))
        .route("/api/v2/torrents/removeCategories", post(remove_categories))
        .route("/api/v2/torrents/addTags", post(add_tags))
        .route("/api/v2/torrents/removeTags", post(remove_tags))
        .route("/api/v2/torrents/setShareLimits", post(share_limits))
        .route("/api/v2/torrents/setForceStart", post(force_start))
        .route("/api/v2/torrents/setDownloadLimit", post(|s: St, r: Request| set_limit(s, r, true)))
        .route("/api/v2/torrents/setUploadLimit", post(|s: St, r: Request| set_limit(s, r, false)))
        .route("/api/v2/torrents/topPrio", post(|s: St, r: Request| queue(s, r, crate::store::QueueMove::Top)))
        .route("/api/v2/torrents/bottomPrio", post(|s: St, r: Request| queue(s, r, crate::store::QueueMove::Bottom)))
        .route("/api/v2/torrents/increasePrio", post(|s: St, r: Request| queue(s, r, crate::store::QueueMove::Up)))
        .route("/api/v2/torrents/decreasePrio", post(|s: St, r: Request| queue(s, r, crate::store::QueueMove::Down)))
}

/// Parameters from the query string and an urlencoded or multipart body (text fields only).
pub(super) async fn read_form(req: Request) -> HashMap<String, String> {
    let mut out: HashMap<String, String> =
        req.uri().query().map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default();
    let ctype = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if ctype.starts_with("multipart/form-data") {
        if let Ok(mut mp) = Multipart::from_request(req, &()).await {
            while let Ok(Some(f)) = mp.next_field().await {
                let name = f.name().unwrap_or("").to_string();
                if f.file_name().is_none()
                    && let Ok(t) = f.text().await
                {
                    out.insert(name, t);
                }
            }
        }
    } else if let Ok(body) = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024).await {
        out.extend(url::form_urlencoded::parse(&body).into_owned());
    }
    out
}

fn ok() -> Response {
    "Ok.".into_response()
}

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, msg.to_string()).into_response()
}

/// `hashes=a|b|c` or `all`.
fn hashes(st: &ApiState, form: &HashMap<String, String>) -> Vec<InfoHash> {
    let Some(h) = form.get("hashes").or_else(|| form.get("hash")) else { return Vec::new() };
    if h == "all" {
        return st.engine.snapshot().torrents.iter().filter(|t| !t.pending).map(|t| t.hash).collect();
    }
    h.split('|').filter_map(|x| InfoHash::from_hex(x.trim())).collect()
}

fn truthy(v: Option<&String>) -> bool {
    v.is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

fn state_name(t: &TorrentView) -> &'static str {
    let done = t.progress >= 1.0;
    if t.restore_failed {
        return "missingFiles";
    }
    match t.state {
        TorrentState::Metadata => "metaDL",
        TorrentState::Checking => {
            if done {
                "checkingUP"
            } else {
                "checkingDL"
            }
        }
        TorrentState::Downloading => "downloading",
        TorrentState::Stalled => "stalledDL",
        TorrentState::Seeding => {
            if t.up_bps > 0 {
                "uploading"
            } else {
                "stalledUP"
            }
        }
        TorrentState::Completed => "pausedUP",
        TorrentState::Paused => {
            if done {
                "pausedUP"
            } else {
                "pausedDL"
            }
        }
        TorrentState::Queued => {
            if done {
                "queuedUP"
            } else {
                "queuedDL"
            }
        }
        TorrentState::Error => "error",
    }
}

fn path_str(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn torrent_json(t: &TorrentView) -> Value {
    let left = t.size.saturating_sub(t.done);
    let eta = t.eta.map(|d| d.as_secs()).unwrap_or(ETA_INF);
    json!({
        "hash": t.hash.to_hex(),
        "infohash_v1": t.hash.to_hex(),
        "name": t.name,
        "size": t.size,
        "total_size": t.size,
        "progress": t.progress,
        "dlspeed": t.down_bps,
        "upspeed": t.up_bps,
        "priority": if t.progress >= 1.0 { 0 } else { t.queue_pos as i64 + 1 },
        "num_seeds": t.peers_live,
        "num_complete": t.peers_seen,
        "num_leechs": 0,
        "num_incomplete": 0,
        "ratio": t.ratio,
        "eta": eta,
        "state": state_name(t),
        "category": t.category.clone().unwrap_or_default(),
        "tags": t.tags.join(", "),
        "save_path": path_str(&t.save_path),
        "content_path": path_str(&t.content_path),
        "download_path": "",
        "added_on": t.added_at,
        "completion_on": t.completed_at.unwrap_or(-1),
        "last_activity": t.last_activity,
        "amount_left": left,
        "completed": t.done,
        "downloaded": t.downloaded,
        "uploaded": t.uploaded,
        "downloaded_session": t.downloaded,
        "uploaded_session": t.uploaded,
        "seeding_time": t.seeding_secs,
        "time_active": t.seeding_secs,
        "ratio_limit": -2,
        "seeding_time_limit": -2,
        "inactive_seeding_time_limit": -2,
        "max_ratio": -1,
        "max_seeding_time": -1,
        "dl_limit": if t.down_limit_kib == 0 { -1 } else { t.down_limit_kib as i64 * 1024 },
        "up_limit": if t.up_limit_kib == 0 { -1 } else { t.up_limit_kib as i64 * 1024 },
        "force_start": false,
        "seq_dl": false,
        "f_l_piece_prio": false,
        "auto_tmm": false,
        "super_seeding": false,
        "magnet_uri": t.magnet(),
        "tracker": t.trackers.first().cloned().unwrap_or_default(),
        "trackers_count": t.trackers.len(),
        "availability": -1,
    })
}

fn matches_filter(t: &TorrentView, filter: &str) -> bool {
    let s = t.state;
    let done = t.progress >= 1.0;
    match filter {
        "downloading" => {
            matches!(s, TorrentState::Downloading | TorrentState::Stalled | TorrentState::Metadata)
                || (!done && s == TorrentState::Queued)
        }
        "seeding" => s == TorrentState::Seeding,
        "completed" => done,
        "paused" | "stopped" => matches!(s, TorrentState::Paused | TorrentState::Completed),
        "resumed" | "running" => !matches!(s, TorrentState::Paused | TorrentState::Completed),
        "active" => t.down_bps > 0 || t.up_bps > 0,
        "inactive" => t.down_bps == 0 && t.up_bps == 0,
        "stalled" => s == TorrentState::Stalled || (s == TorrentState::Seeding && t.up_bps == 0),
        "stalled_downloading" => s == TorrentState::Stalled,
        "stalled_uploading" => s == TorrentState::Seeding && t.up_bps == 0,
        "checking" => s == TorrentState::Checking,
        "errored" => s == TorrentState::Error,
        _ => true,
    }
}

async fn logout(State(st): St, req: Request) -> Response {
    if let Some(sid) = super::sid_from(req.headers()) {
        st.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
    }
    ok()
}

async fn build_info() -> Response {
    axum::Json(json!({"qt": "", "libtorrent": "librqbit 9", "boost": "", "openssl": "", "bitness": 64})).into_response()
}

async fn preferences(State(st): St) -> Response {
    let s = st.settings.load();
    axum::Json(json!({
        "save_path": path_str(&s.downloads.save_path),
        "temp_path_enabled": false,
        "max_ratio_enabled": s.seeding.ratio_enabled,
        "max_ratio": if s.seeding.ratio_enabled { s.seeding.ratio } else { -1.0 },
        "max_seeding_time_enabled": s.seeding.time_enabled,
        "max_seeding_time": if s.seeding.time_enabled { s.seeding.time_minutes as i64 } else { -1 },
        "max_inactive_seeding_time_enabled": false,
        "max_inactive_seeding_time": -1,
        "max_ratio_act": match s.seeding.action {
            crate::config::SeedAction::Pause => 0,
            _ => 1,
        },
        "queueing_enabled": s.queue.enabled,
        "max_active_downloads": s.queue.max_active_downloads,
        "max_active_uploads": s.queue.max_active_uploads,
        "max_active_torrents": s.queue.max_active_total,
        "dht": s.bittorrent.dht,
        "pex": true,
        "lsd": s.bittorrent.lsd,
        "listen_port": s.connection.listen_port,
        "dl_limit": s.speed.download_limit as u64 * 1024,
        "up_limit": s.speed.upload_limit as u64 * 1024,
        "web_ui_port": st.cfg.port,
        "web_ui_username": st.cfg.username,
        "auto_tmm_enabled": false,
        "start_paused_enabled": s.downloads.start_paused,
        "create_subfolder_enabled": s.downloads.create_subfolder,
    }))
    .into_response()
}

async fn default_save_path(State(st): St) -> Response {
    path_str(&st.settings.load().downloads.save_path).into_response()
}

fn server_state(st: &ApiState) -> Value {
    let snap = st.engine.snapshot();
    let s = st.settings.load();
    let (down, up) = s.speed.effective(snap.session.alt_speed);
    json!({
        "dl_info_speed": snap.session.down_bps,
        "dl_info_data": snap.session.session_downloaded,
        "up_info_speed": snap.session.up_bps,
        "up_info_data": snap.session.session_uploaded,
        "dl_rate_limit": down as u64 * 1024,
        "up_rate_limit": up as u64 * 1024,
        "dht_nodes": snap.session.dht_nodes.unwrap_or(0),
        "connection_status": if snap.session.listen_port.is_some() { "connected" } else { "disconnected" },
        "free_space_on_disk": snap.free_space.unwrap_or(0),
        "alltime_dl": snap.all_time_downloaded,
        "alltime_ul": snap.all_time_uploaded,
        "use_alt_speed_limits": snap.session.alt_speed,
        "queueing": s.queue.enabled,
    })
}

async fn transfer_info(State(st): St) -> Response {
    axum::Json(server_state(&st)).into_response()
}

fn categories_json(st: &ApiState) -> Value {
    let snap = st.engine.snapshot();
    let map: serde_json::Map<String, Value> = snap
        .categories
        .iter()
        .map(|(name, c)| {
            let path = c.save_path.as_deref().map(path_str).unwrap_or_default();
            (name.clone(), json!({"name": name, "savePath": path}))
        })
        .collect();
    Value::Object(map)
}

async fn maindata(State(st): St) -> Response {
    let rid = st.rid.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let snap = st.engine.snapshot();
    let torrents: serde_json::Map<String, Value> =
        snap.torrents.iter().filter(|t| !t.pending).map(|t| (t.hash.to_hex(), torrent_json(t))).collect();
    axum::Json(json!({
        "rid": rid,
        "full_update": true,
        "torrents": torrents,
        "categories": categories_json(&st),
        "tags": snap.tags,
        "server_state": server_state(&st),
    }))
    .into_response()
}

async fn info(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let snap = st.engine.snapshot();
    let wanted: Option<Vec<InfoHash>> = f.get("hashes").map(|_| hashes(&st, &f));
    let mut list: Vec<&TorrentView> = snap
        .torrents
        .iter()
        .filter(|t| !t.pending)
        .filter(|t| f.get("filter").is_none_or(|x| matches_filter(t, x)))
        .filter(|t| match f.get("category") {
            None => true,
            Some(c) if c.is_empty() => t.category.is_none(),
            Some(c) => t.category.as_deref() == Some(c.as_str()),
        })
        .filter(|t| match f.get("tag") {
            None => true,
            Some(x) if x.is_empty() => t.tags.is_empty(),
            Some(x) => t.tags.iter().any(|g| g == x),
        })
        .filter(|t| wanted.as_ref().is_none_or(|w| w.contains(&t.hash)))
        .collect();
    if let Some(key) = f.get("sort") {
        static NULL: Value = Value::Null;
        list.sort_by(|a, b| {
            let (x, y) = (torrent_json(a), torrent_json(b));
            let (x, y) = (x.get(key.as_str()).unwrap_or(&NULL), y.get(key.as_str()).unwrap_or(&NULL));
            match (x.as_f64(), y.as_f64()) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                _ => x.as_str().unwrap_or("").cmp(y.as_str().unwrap_or("")),
            }
        });
        if truthy(f.get("reverse")) {
            list.reverse();
        }
    }
    let offset: usize = f.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
    let limit: usize = f.get("limit").and_then(|o| o.parse().ok()).filter(|l| *l > 0).unwrap_or(usize::MAX);
    let out: Vec<Value> = list.into_iter().skip(offset).take(limit).map(torrent_json).collect();
    axum::Json(out).into_response()
}

fn find(st: &ApiState, f: &HashMap<String, String>) -> Option<TorrentView> {
    let h = InfoHash::from_hex(f.get("hash")?)?;
    st.engine.snapshot().torrents.iter().find(|t| t.hash == h && !t.pending).cloned()
}

async fn properties(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let Some(t) = find(&st, &f) else { return (StatusCode::NOT_FOUND, "Torrent hash was not found").into_response() };
    axum::Json(json!({
        "save_path": path_str(&t.save_path),
        "creation_date": t.creation_date.unwrap_or(-1),
        "piece_size": t.piece_size,
        "comment": t.comment.clone().unwrap_or_default(),
        "total_wasted": 0,
        "total_uploaded": t.uploaded,
        "total_downloaded": t.downloaded,
        "up_limit": -1,
        "dl_limit": -1,
        "time_elapsed": t.seeding_secs,
        "seeding_time": t.seeding_secs,
        "nb_connections": t.peers_live,
        "share_ratio": t.ratio,
        "addition_date": t.added_at,
        "completion_date": t.completed_at.unwrap_or(-1),
        "created_by": t.created_by.clone().unwrap_or_default(),
        "dl_speed": t.down_bps,
        "up_speed": t.up_bps,
        "eta": t.eta.map(|d| d.as_secs()).unwrap_or(ETA_INF),
        "seeds": t.peers_live,
        "pieces_num": t.piece_count,
        "total_size": t.size,
        "is_private": false,
    }))
    .into_response()
}

async fn files(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let Some(t) = find(&st, &f) else { return (StatusCode::NOT_FOUND, "Torrent hash was not found").into_response() };
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.engine.send(EngineCommand::FileList { hash: t.hash, reply: tx });
    let files = tokio::time::timeout(std::time::Duration::from_secs(5), rx).await.ok().and_then(Result::ok).flatten();
    let Some(files) = files else { return axum::Json(Vec::<Value>::new()).into_response() };
    let out: Vec<Value> = files
        .iter()
        .map(|file| {
            json!({
                "index": file.index,
                "name": file.components.join("/"),
                "size": file.len,
                "progress": if file.len == 0 { 1.0 } else { file.done as f64 / file.len as f64 },
                "priority": if file.included { 1 } else { 0 },
                "is_seed": file.done >= file.len,
                "availability": -1,
            })
        })
        .collect();
    axum::Json(out).into_response()
}

async fn categories(State(st): St) -> Response {
    axum::Json(categories_json(&st)).into_response()
}

async fn tags(State(st): St) -> Response {
    axum::Json(st.engine.snapshot().tags.clone()).into_response()
}

async fn add(State(st): St, req: Request) -> Response {
    let ctype = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if ctype.starts_with("multipart/form-data") {
        let Ok(mut mp) = Multipart::from_request(req, &()).await else { return bad("invalid form") };
        loop {
            match mp.next_field().await {
                Ok(Some(f)) => {
                    let name = f.name().unwrap_or("").to_string();
                    match f.file_name().map(String::from) {
                        Some(file) => match f.bytes().await {
                            Ok(b) => files.push((file, b.to_vec())),
                            Err(_) => return bad("invalid upload"),
                        },
                        None => {
                            if let Ok(t) = f.text().await {
                                fields.insert(name, t);
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(_) => return bad("invalid form"),
            }
        }
    } else {
        fields = read_form(req).await;
    }
    let settings = st.settings.load_full();
    let category = fields.get("category").filter(|c| !c.is_empty()).cloned();
    let snap = st.engine.snapshot();
    let save_path = fields
        .get("savepath")
        .filter(|p| !p.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| category.as_ref().and_then(|c| snap.categories.get(c)).and_then(|c| c.save_path.clone()))
        .unwrap_or_else(|| settings.downloads.save_path.clone());
    let paused = match fields.get("paused").or_else(|| fields.get("stopped")) {
        Some(v) => truthy(Some(v)),
        None => settings.downloads.start_paused,
    };
    let tags: Vec<String> = fields
        .get("tags")
        .map(|t| t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let create_subfolder = match fields.get("contentLayout").map(String::as_str) {
        Some("NoSubfolder") => false,
        Some("Subfolder") => true,
        _ => settings.downloads.create_subfolder,
    };
    let mut sources: Vec<AddSource> = files.into_iter().map(|(name, bytes)| AddSource::Bytes { name, bytes }).collect();
    for u in fields.get("urls").map(|u| u.lines().map(str::trim).filter(|l| !l.is_empty())).into_iter().flatten() {
        if u.starts_with("magnet:") {
            sources.push(AddSource::Magnet(u.to_string()));
        } else if u.starts_with("http://") || u.starts_with("https://") {
            sources.push(AddSource::Url(u.to_string()));
        } else if let Some(h) = InfoHash::from_hex(u) {
            sources.push(AddSource::Magnet(format!("magnet:?xt=urn:btih:{}", h.to_hex())));
        }
    }
    if sources.is_empty() {
        return "Fails.".into_response();
    }
    // A stuck or flooded engine: refuse instead of queueing more (STABILITY.md F11).
    let busy = || (StatusCode::SERVICE_UNAVAILABLE, "Corrent is busy, try again later").into_response();
    if st.engine.queued() >= crate::engine::API_ADD_QUEUE {
        return busy();
    }
    for source in sources {
        tracing::info!("web API: adding {}", source.describe());
        let queued = st.engine.send(EngineCommand::Add(Box::new(AddRequest {
            source,
            torrent_bytes: None,
            save_path: save_path.clone(),
            create_subfolder,
            category: category.clone(),
            tags: tags.clone(),
            paused,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
        })));
        if !queued {
            return busy();
        }
    }
    ok()
}

async fn pause(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    st.engine.send(EngineCommand::Pause(hashes(&st, &f)));
    ok()
}

async fn resume(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    st.engine.send(EngineCommand::Resume(hashes(&st, &f)));
    ok()
}

async fn force_start(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let h = hashes(&st, &f);
    if truthy(f.get("value")) {
        st.engine.send(EngineCommand::ForceStart(h));
    } else {
        st.engine.send(EngineCommand::Resume(h));
    }
    ok()
}

async fn delete(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    st.engine.send(EngineCommand::Delete { hashes: hashes(&st, &f), with_data: truthy(f.get("deleteFiles")) });
    ok()
}

async fn recheck(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    st.engine.send(EngineCommand::Recheck(hashes(&st, &f)));
    ok()
}

async fn set_category(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let category = f.get("category").filter(|c| !c.is_empty()).cloned();
    if let Some(c) = &category
        && !st.engine.snapshot().categories.contains_key(c)
    {
        return (StatusCode::CONFLICT, "Category does not exist").into_response();
    }
    st.engine.send(EngineCommand::SetCategory { hashes: hashes(&st, &f), category });
    ok()
}

async fn create_category(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let Some(name) = f.get("category").map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) else {
        return bad("Category name is empty");
    };
    let mut cats = st.engine.snapshot().categories.clone();
    let path = f.get("savePath").filter(|p| !p.trim().is_empty()).map(std::path::PathBuf::from);
    // Keep the category's speed limits when its path is edited.
    let mut info = cats.get(&name).cloned().unwrap_or_default();
    info.save_path = path;
    cats.insert(name.clone(), info);
    st.engine.send(EngineCommand::SetCategories(cats));
    // Sonarr calls setCategory right after: answer once the category exists.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !st.engine.snapshot().categories.contains_key(&name) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    ok()
}

async fn remove_categories(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let names: Vec<String> = f.get("categories").map(|c| c.lines().map(String::from).collect()).unwrap_or_default();
    let mut cats = st.engine.snapshot().categories.clone();
    cats.retain(|k, _| !names.contains(k));
    st.engine.send(EngineCommand::SetCategories(cats));
    ok()
}

fn tag_list(f: &HashMap<String, String>) -> Vec<String> {
    f.get("tags")
        .map(|t| t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default()
}

async fn add_tags(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let h = hashes(&st, &f);
    for tag in tag_list(&f) {
        st.engine.send(EngineCommand::ToggleTag { hashes: h.clone(), tag, on: true });
    }
    ok()
}

async fn remove_tags(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let h = hashes(&st, &f);
    for tag in tag_list(&f) {
        st.engine.send(EngineCommand::ToggleTag { hashes: h.clone(), tag, on: false });
    }
    ok()
}

/// qBittorrent: -2 = use the global limit, -1 = no limit, otherwise the limit.
fn share_limit<T: std::str::FromStr + PartialOrd + Default>(v: Option<&String>) -> Option<T> {
    let v = v?.trim();
    if v == "-2" {
        return None;
    }
    if v == "-1" {
        return Some(T::default()); // 0 = no limit
    }
    v.parse().ok().filter(|x| *x > T::default())
}

async fn share_limits(State(st): St, req: Request) -> Response {
    let f = read_form(req).await;
    let ratio = share_limit::<f32>(f.get("ratioLimit"));
    let minutes = share_limit::<u32>(f.get("seedingTimeLimit"));
    st.engine.send(EngineCommand::SetSeedLimits { hashes: hashes(&st, &f), ratio, minutes });
    ok()
}

/// `limit` in bytes/s; 0 or -1 = none. The other direction keeps its current value.
async fn set_limit(State(st): St, req: Request, download: bool) -> Response {
    let f = read_form(req).await;
    let kib = f
        .get("limit")
        .and_then(|l| l.trim().parse::<i64>().ok())
        .filter(|l| *l > 0)
        .map(|l| (l.saturating_add(1023) / 1024).min(u32::MAX as i64) as u32);
    let snap = st.engine.snapshot();
    for h in hashes(&st, &f) {
        let Some(t) = snap.torrents.iter().find(|t| t.hash == h) else { continue };
        let (down_kib, up_kib) =
            if download { (kib.unwrap_or(0), t.up_limit_kib) } else { (t.down_limit_kib, kib.unwrap_or(0)) };
        st.engine.send(EngineCommand::SetTorrentLimits { hashes: vec![h], down_kib, up_kib });
    }
    ok()
}

async fn queue(State(st): St, req: Request, mv: crate::store::QueueMove) -> Response {
    let f = read_form(req).await;
    st.engine.send(EngineCommand::QueueMove { hashes: hashes(&st, &f), mv });
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_states() {
        assert_eq!(share_limit::<f32>(Some(&"-2".into())), None);
        assert_eq!(share_limit::<f32>(Some(&"-1".into())), Some(0.0));
        assert_eq!(share_limit::<f32>(Some(&"1.5".into())), Some(1.5));
        assert_eq!(share_limit::<u32>(Some(&"60".into())), Some(60));
        let t = TorrentView { state: TorrentState::Paused, progress: 1.0, ..Default::default() };
        assert_eq!(state_name(&t), "pausedUP");
        let t = TorrentView { state: TorrentState::Queued, progress: 0.2, ..Default::default() };
        assert_eq!(state_name(&t), "queuedDL");
        assert!(matches_filter(&t, "downloading"));
    }
}
