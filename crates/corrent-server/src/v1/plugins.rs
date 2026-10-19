//! Plugins (list, settings, enable/disable, install, remove) and identifying library items
//! with metadata providers.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{FromRequest, Multipart, Path, Query, Request, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use corrent_plugins::{Manifest, PluginInfo};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use serde::{Deserialize, Serialize};

use super::library::lib_err;
use super::{ApiErr, ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/plugins", get(list).post(install).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)))
        .route("/plugins/{id}", axum::routing::delete(remove))
        .route("/plugins/{id}/settings", put(settings))
        .route("/plugins/{id}/enabled", put(enabled))
        .route("/items/{id}/identify", get(identify_candidates).post(identify))
        .route("/items/{id}/refresh-metadata", post(refresh_item))
        .route("/libraries/{id}/refresh-metadata", post(refresh_library))
}

fn host(st: &AppState) -> Result<&Arc<corrent_plugins::PluginHost>, ApiErr> {
    st.plugins.as_ref().ok_or_else(|| err(ErrorCode::EngineUnavailable, "plugins are unavailable on this server"))
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<PluginInfo>> {
    auth.require(Perms::SETTINGS_READ)?;
    Ok(Json(host(&st)?.list()))
}

async fn settings(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(s): Json<HashMap<String, String>>,
) -> ApiResult<PluginInfo> {
    auth.require(Perms::PLUGINS_MANAGE)?;
    let h = host(&st)?;
    h.set_settings(&id, s).map_err(|_| err(ErrorCode::NotFound, "no such plugin"))?;
    crate::plugins::persist(st.identity.db(), h, &id).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "plugin_settings", &id, true, "").await;
    h.list()
        .into_iter()
        .find(|p| p.manifest.id == id)
        .map(Json)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such plugin"))
}

#[derive(Deserialize)]
struct Enabled {
    enabled: bool,
}

async fn enabled(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<String>,
    Json(e): Json<Enabled>,
) -> ApiResult<PluginInfo> {
    auth.require(Perms::PLUGINS_MANAGE)?;
    let h = host(&st)?;
    h.set_enabled(&id, e.enabled).map_err(|_| err(ErrorCode::NotFound, "no such plugin"))?;
    crate::plugins::persist(st.identity.db(), h, &id).await;
    st.identity
        .audit(
            Some(auth.name()),
            &ip.0.to_string(),
            if e.enabled { "plugin_enable" } else { "plugin_disable" },
            &id,
            true,
            "",
        )
        .await;
    h.list()
        .into_iter()
        .find(|p| p.manifest.id == id)
        .map(Json)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such plugin"))
}

/// Installs a plugin from a multipart upload with `manifest` (plugin.toml) and `wasm` parts.
/// Unsigned plugins are allowed for admins; they run in the same sandbox as built-in ones.
async fn install(State(st): State<Arc<AppState>>, auth: Auth, ip: ClientIp, req: Request) -> ApiResult<PluginInfo> {
    auth.require(Perms::PLUGINS_MANAGE)?;
    let h = host(&st)?;
    let mut mp = Multipart::from_request(req, &()).await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    let (mut manifest, mut wasm) = (None, None);
    while let Some(f) = mp.next_field().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))? {
        match f.name() {
            Some("manifest") => manifest = Some(f.text().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?),
            Some("wasm") => wasm = Some(f.bytes().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?),
            _ => {}
        }
    }
    let (Some(text), Some(wasm)) = (manifest, wasm) else {
        return Err(err(ErrorCode::BadRequest, "manifest and wasm parts required"));
    };
    let m = Manifest::parse(&text).map_err(|e| err(ErrorCode::BadRequest, e))?;
    if corrent_plugins::builtins().iter().any(|(b, _)| b.id == m.id) {
        return Err(err(ErrorCode::Conflict, "a built-in plugin has that id"));
    }
    let id = m.id.clone();
    let (h2, m2, w2) = (h.clone(), m.clone(), wasm.to_vec());
    tokio::task::spawn_blocking(move || h2.load(m2, &w2, HashMap::new(), true, false))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?
        .map_err(|e| err(ErrorCode::BadRequest, e))?;
    let dir = crate::plugins::plugins_dir(&st.paths.data_dir).join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    corrent_core::util::write_atomic(&dir.join("plugin.toml"), text.as_bytes())
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    corrent_core::util::write_atomic(&dir.join("plugin.wasm"), &wasm)
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    crate::plugins::persist(st.identity.db(), h, &id).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "plugin_install", &id, true, &m.version).await;
    h.list()
        .into_iter()
        .find(|p| p.manifest.id == id)
        .map(Json)
        .ok_or_else(|| err(ErrorCode::Internal, "plugin vanished"))
}

async fn remove(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::PLUGINS_MANAGE)?;
    let h = host(&st)?;
    if h.list().iter().any(|p| p.manifest.id == id && p.builtin) {
        return Err(err(ErrorCode::Conflict, "built-in plugins can be disabled, not removed"));
    }
    if !h.unload(&id) {
        return Err(err(ErrorCode::NotFound, "no such plugin"));
    }
    let _ = std::fs::remove_dir_all(crate::plugins::plugins_dir(&st.paths.data_dir).join(&id));
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "plugin_remove", &id, true, "").await;
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IdentifyQuery {
    title: Option<String>,
    year: Option<i32>,
}

#[derive(Serialize)]
struct IdentifyCandidate {
    provider: String,
    id: String,
    title: String,
    year: Option<u32>,
    overview: Option<String>,
    poster_url: Option<String>,
}

async fn language_of(st: &AppState, library_id: i64) -> String {
    st.library
        .get(library_id)
        .await
        .ok()
        .map(|l| l.settings.language)
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en".into())
}

async fn identify_candidates(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Query(q): Query<IdentifyQuery>,
) -> ApiResult<Vec<IdentifyCandidate>> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let h = host(&st)?.clone();
    let p = st.library.pending_item(id).await.map_err(lib_err)?;
    let lang = language_of(&st, p.library_id).await;
    let found = crate::metadata::candidates(&h, &p, q.title, q.year, &lang).await;
    // Listing candidates must not count as a check.
    Ok(Json(
        found
            .into_iter()
            .map(|(provider, c)| IdentifyCandidate {
                provider,
                id: c.id,
                title: c.title,
                year: c.year,
                overview: c.overview,
                poster_url: c.poster_url,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct IdentifyBody {
    provider: String,
    id: String,
}

async fn identify(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(b): Json<IdentifyBody>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let h = host(&st)?.clone();
    let p = st.library.pending_item(id).await.map_err(lib_err)?;
    let lang = language_of(&st, p.library_id).await;
    crate::metadata::apply(&st, &h, id, p.kind, &b.provider, &b.id, &lang)
        .await
        .map_err(|e| err(ErrorCode::BadRequest, e))?;
    st.library.emit_changed(p.library_id);
    Ok(Json(serde_json::json!({})))
}

async fn refresh_item(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let h = host(&st)?.clone();
    let p = st.library.pending_item(id).await.map_err(lib_err)?;
    let lang = language_of(&st, p.library_id).await;
    let found = crate::metadata::identify(&st, &h, &p, &lang).await;
    st.library.emit_changed(p.library_id);
    Ok(Json(serde_json::json!({ "identified": found })))
}

async fn refresh_library(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    st.library.get(id).await.map_err(lib_err)?;
    tokio::spawn(crate::metadata::refresh_library(st.clone(), id));
    Ok(Json(serde_json::json!({})))
}
