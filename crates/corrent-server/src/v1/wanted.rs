//! The wanted list, quality profiles, "find releases"/"get this" for library items, and
//! streaming files that are still downloading.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use corrent_proto::auth::Perms;
use corrent_proto::library::{ItemKind, NewWanted, QualityProfile, QualityRules, Release, Wanted, WantedKind};
use corrent_proto::{ErrorCode, InfoHash};
use serde::Deserialize;

use super::library::lib_err;
use super::{ApiResult, Auth, ClientIp, err};
use crate::integrate;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/wanted", get(list).post(add))
        .route("/wanted/{id}", axum::routing::delete(remove))
        .route("/wanted/{id}/search", post(search))
        .route("/wanted/{id}/grab", post(grab_wanted))
        .route("/quality-profiles", get(profiles).post(create_profile))
        .route("/quality-profiles/{id}", put(update_profile).delete(delete_profile))
        .route("/items/{id}/releases", get(item_releases))
        .route("/items/{id}/monitor", put(monitor).delete(unmonitor))
        .route("/monitored", get(monitored))
        .route("/items/{id}/grab", post(grab_item))
        .route("/torrents/{hash}/files/{file}/stream", get(stream))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListQuery {
    status: Option<String>,
}

async fn list(State(st): State<Arc<AppState>>, _auth: Auth, Query(q): Query<ListQuery>) -> ApiResult<Vec<Wanted>> {
    Ok(Json(st.library.wanted(q.status).await.map_err(lib_err)?))
}

async fn add(State(st): State<Arc<AppState>>, auth: Auth, Json(w): Json<NewWanted>) -> ApiResult<Wanted> {
    auth.require(Perms::TORRENTS_ADD)?;
    Ok(Json(st.library.add_wanted(w, auth.name()).await.map_err(lib_err)?))
}

async fn remove(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<serde_json::Value> {
    let w = st.library.wanted_one(id).await.map_err(lib_err)?;
    if w.added_by.as_deref() != Some(auth.name()) {
        auth.require(Perms::TORRENTS_MANAGE_ALL)?;
    }
    st.library.delete_wanted(id).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({})))
}

async fn search(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Vec<Release>> {
    auth.require(Perms::TORRENTS_ADD)?;
    let w = st.library.wanted_one(id).await.map_err(lib_err)?;
    let _ = st.library.mark_searched(id).await;
    Ok(Json(integrate::releases_for(&st, &w).await))
}

#[derive(Deserialize)]
struct GrabRequest {
    link: String,
}

async fn grab_wanted(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(g): Json<GrabRequest>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_ADD)?;
    let w = st.library.wanted_one(id).await.map_err(lib_err)?;
    let lib = match w.library_id {
        Some(l) => st.library.get(l).await.ok(),
        None => None,
    };
    let hash = integrate::grab(&st, &g.link, lib.as_ref(), auth.name()).await?;
    let h = hash.map(|h| h.to_hex());
    st.library.set_wanted_status(id, "grabbed", h.clone()).await.map_err(lib_err)?;
    let _ = st.library.record_grab(Some(id), None, &h.clone().unwrap_or_default(), &w.title, 0).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "grab", &w.title, true, "").await;
    Ok(Json(serde_json::json!({ "hash": h })))
}

async fn profiles(State(st): State<Arc<AppState>>, _auth: Auth) -> ApiResult<Vec<QualityProfile>> {
    Ok(Json(st.library.profiles().await.map_err(lib_err)?))
}

#[derive(Deserialize)]
struct ProfileBody {
    name: String,
    #[serde(default)]
    rules: QualityRules,
}

async fn create_profile(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(p): Json<ProfileBody>,
) -> ApiResult<QualityProfile> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    Ok(Json(st.library.save_profile(None, p.name, p.rules).await.map_err(lib_err)?))
}

async fn update_profile(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(p): Json<ProfileBody>,
) -> ApiResult<QualityProfile> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    Ok(Json(st.library.save_profile(Some(id), p.name, p.rules).await.map_err(lib_err)?))
}

async fn delete_profile(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    st.library.delete_profile(id).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({})))
}

/// A wanted entry describing a library item (for searching releases of it).
async fn wanted_for_item(
    st: &AppState,
    user: i64,
    id: i64,
) -> Result<(Wanted, Option<corrent_proto::library::Library>), super::ApiErr> {
    let d = st.library.item(user, id).await.map_err(lib_err)?;
    let lib = st.library.get(d.item.library_id).await.ok();
    let i = &d.item;
    // Episodes and seasons: the show's title comes first in `context`.
    let show = i.context.first().cloned();
    let (kind, title, season, episode) = match i.kind {
        ItemKind::Movie => (WantedKind::Movie, i.title.clone(), None, None),
        ItemKind::Show => (WantedKind::Show, i.title.clone(), None, None),
        ItemKind::Season => {
            (WantedKind::Season, i.context.last().cloned().unwrap_or_else(|| i.title.clone()), i.index, None)
        }
        ItemKind::Episode => {
            let season = st.library.item(user, i.parent_id.unwrap_or(0)).await.ok().and_then(|s| s.item.index);
            (WantedKind::Episode, show.unwrap_or_else(|| i.title.clone()), season, i.index)
        }
        ItemKind::Album => (
            WantedKind::Album,
            format!("{} {}", i.context.first().cloned().unwrap_or_default(), i.title).trim().to_string(),
            None,
            None,
        ),
        _ => (WantedKind::Other, i.title.clone(), None, None),
    };
    let w = Wanted {
        id: 0,
        library_id: Some(i.library_id),
        kind,
        title,
        year: i.year,
        season,
        episode,
        query: None,
        profile_id: lib.as_ref().and_then(|l| l.settings.profile_id),
        status: "wanted".into(),
        torrent: None,
        added_by: None,
        created_at: 0,
        searched_at: None,
    };
    Ok((w, lib))
}

async fn item_releases(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Vec<Release>> {
    auth.require(Perms::TORRENTS_ADD)?;
    let (w, _) = wanted_for_item(&st, auth.0.user.id, id).await?;
    Ok(Json(integrate::releases_for(&st, &w).await))
}

async fn grab_item(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(g): Json<GrabRequest>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_ADD)?;
    let (w, lib) = wanted_for_item(&st, auth.0.user.id, id).await?;
    let hash = integrate::grab(&st, &g.link, lib.as_ref(), auth.name()).await?;
    let h = hash.map(|h| h.to_hex());
    let _ = st.library.record_grab(None, Some(id), &h.clone().unwrap_or_default(), &w.title, 0).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "grab", &w.title, true, "").await;
    Ok(Json(serde_json::json!({ "hash": h })))
}

/// A torrent file as it downloads (Range requests wait for the pieces they need): proxied
/// from the engine's loopback stream server.
async fn stream(
    State(st): State<Arc<AppState>>,
    _auth: Auth,
    Path((hash, file)): Path<(String, usize)>,
    req: Request,
) -> Result<Response, super::ApiErr> {
    let h = InfoHash::from_hex(&hash).ok_or_else(|| err(ErrorCode::BadRequest, "not an info hash"))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.engine.send(corrent_core::engine::EngineCommand::FileList { hash: h, reply: tx });
    let files = rx.await.ok().flatten().ok_or_else(|| err(ErrorCode::NotFound, "no such torrent"))?;
    let f = files.iter().find(|f| f.index == file).ok_or_else(|| err(ErrorCode::NotFound, "no such file"))?;
    let name = f.components.last().cloned().unwrap_or_default();
    let url = integrate::loopback_stream_url(&st, h, file, &name)
        .ok_or_else(|| err(ErrorCode::EngineUnavailable, "streaming is not running"))?;
    let mut upstream = reqwest::Client::new().get(&url);
    if let Some(r) = req.headers().get(header::RANGE) {
        upstream = upstream.header(header::RANGE, r);
    }
    let res = upstream.send().await.map_err(|e| err(ErrorCode::EngineUnavailable, e.to_string()))?;
    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = Response::builder().status(status);
    for k in [header::CONTENT_TYPE, header::CONTENT_LENGTH, header::CONTENT_RANGE, header::ACCEPT_RANGES] {
        if let Some(v) = res.headers().get(k.as_str()) {
            out = out.header(k, v.as_bytes());
        }
    }
    out.body(Body::from_stream(res.bytes_stream())).map_err(|e| err(ErrorCode::Internal, e.to_string()))
}

/// Follows a show; its missing aired episodes are added to the wanted list right away.
async fn monitor(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(r): Json<corrent_proto::library::MonitorRequest>,
) -> ApiResult<Vec<corrent_proto::library::MonitoredShow>> {
    auth.require(Perms::TORRENTS_ADD)?;
    st.library.monitor_show(id, r.profile_id, r.from_season.unwrap_or(1), auth.name()).await.map_err(lib_err)?;
    let st2 = st.clone();
    tokio::spawn(async move {
        if let Some(show) = st2.library.monitored().await.ok().and_then(|l| l.into_iter().find(|m| m.item_id == id))
            && let Err(e) = crate::monitor::check_show(&st2, &show).await
        {
            tracing::info!("monitor {}: {e}", show.title);
        }
    });
    Ok(Json(st.library.monitored().await.map_err(lib_err)?))
}

async fn unmonitor(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_ADD)?;
    st.library.unmonitor_show(id).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({})))
}

async fn monitored(
    State(st): State<Arc<AppState>>,
    _auth: Auth,
) -> ApiResult<Vec<corrent_proto::library::MonitoredShow>> {
    Ok(Json(st.library.monitored().await.map_err(lib_err)?))
}
