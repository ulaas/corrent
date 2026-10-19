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
        .route("/plugins/catalog", get(catalog))
        .route("/plugins/catalog/install", post(install_from_catalog))
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
    host(&st)?;
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
    install_files(&st, auth.name(), &ip.0.to_string(), &text, &wasm, None).await
}

/// Installs a plugin from its manifest and component (`expect_id`: what a catalog promised).
async fn install_files(
    st: &Arc<AppState>,
    by: &str,
    ip: &str,
    text: &str,
    wasm: &[u8],
    expect_id: Option<&str>,
) -> ApiResult<PluginInfo> {
    let h = host(st)?;
    let m = Manifest::parse(text).map_err(|e| err(ErrorCode::BadRequest, e))?;
    if expect_id.is_some_and(|id| id != m.id) {
        return Err(err(ErrorCode::BadRequest, "the archive holds another plugin"));
    }
    if m.runtime != corrent_plugins::Runtime::Wasm {
        return Err(err(ErrorCode::BadRequest, "process plugins are installed by putting their folder on the server"));
    }
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
    corrent_core::util::write_atomic(&dir.join("plugin.wasm"), wasm)
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    crate::plugins::persist(st.identity.db(), h, &id).await;
    st.identity.audit(Some(by), ip, "plugin_install", &id, true, &m.version).await;
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

/// One plugin in a catalog index (`{"plugins": [...]}` at `plugin_catalog`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kinds: Vec<String>,
    /// A zip with `plugin.toml` and `plugin.wasm`.
    pub url: String,
    /// Hex SHA-256 of the zip.
    pub sha256: String,
    /// Base64 Ed25519 signature over the zip's SHA-256 (the 32 raw bytes).
    pub signature: String,
}

#[derive(Serialize, Deserialize)]
struct CatalogIndex {
    plugins: Vec<CatalogEntry>,
}

#[derive(Serialize)]
struct CatalogView {
    /// No catalog configured.
    configured: bool,
    entries: Vec<CatalogItem>,
}

#[derive(Serialize)]
struct CatalogItem {
    #[serde(flatten)]
    entry: CatalogEntry,
    /// The installed version, if any.
    installed: Option<String>,
}

async fn fetch_index(st: &AppState) -> Result<Vec<CatalogEntry>, super::ApiErr> {
    let url = st.cfg.load().plugin_catalog.clone();
    let res = st
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| err(ErrorCode::BadRequest, format!("catalog: {e}")))?;
    let bytes = res.bytes().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(err(ErrorCode::PayloadTooLarge, "catalog too large"));
    }
    let idx: CatalogIndex =
        serde_json::from_slice(&bytes).map_err(|e| err(ErrorCode::BadRequest, format!("catalog: {e}")))?;
    Ok(idx.plugins)
}

async fn catalog(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<CatalogView> {
    auth.require(Perms::PLUGINS_MANAGE)?;
    if st.cfg.load().plugin_catalog.is_empty() {
        return Ok(Json(CatalogView { configured: false, entries: Vec::new() }));
    }
    let installed: HashMap<String, String> =
        host(&st)?.list().into_iter().map(|p| (p.manifest.id, p.manifest.version)).collect();
    let entries = fetch_index(&st)
        .await?
        .into_iter()
        .map(|e| CatalogItem { installed: installed.get(&e.id).cloned(), entry: e })
        .collect();
    Ok(Json(CatalogView { configured: true, entries }))
}

/// Whether `sig` (base64) over `digest` verifies with one of the trusted keys (hex).
pub fn signed_by_trusted(digest: &[u8], sig: &str, keys: &[String]) -> bool {
    use base64::Engine as _;
    let Ok(sig) = base64::engine::general_purpose::STANDARD.decode(sig.trim()) else { return false };
    let Ok(sig) = <[u8; 64]>::try_from(sig.as_slice()) else { return false };
    let sig = ed25519_dalek::Signature::from_bytes(&sig);
    keys.iter().any(|k| {
        let bytes: Vec<u8> = (0..k.trim().len() / 2)
            .filter_map(|i| u8::from_str_radix(k.trim().get(i * 2..i * 2 + 2)?, 16).ok())
            .collect();
        <[u8; 32]>::try_from(bytes.as_slice())
            .ok()
            .and_then(|b| ed25519_dalek::VerifyingKey::from_bytes(&b).ok())
            .is_some_and(|vk| vk.verify_strict(digest, &sig).is_ok())
    })
}

#[derive(Deserialize)]
struct CatalogInstall {
    id: String,
}

/// Installs (or upgrades) a catalog plugin: the archive must match its hash and carry a
/// signature by a trusted key.
async fn install_from_catalog(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(r): Json<CatalogInstall>,
) -> ApiResult<PluginInfo> {
    auth.require(Perms::PLUGINS_MANAGE)?;
    let entry = fetch_index(&st)
        .await?
        .into_iter()
        .find(|e| e.id == r.id)
        .ok_or_else(|| err(ErrorCode::NotFound, "not in the catalog"))?;
    let res = st
        .http
        .get(&entry.url)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    if res.content_length().is_some_and(|l| l > 32 * 1024 * 1024) {
        return Err(err(ErrorCode::PayloadTooLarge, "plugin too large"));
    }
    let zip = res.bytes().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    if zip.len() > 32 * 1024 * 1024 {
        return Err(err(ErrorCode::PayloadTooLarge, "plugin too large"));
    }
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &zip);
    let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if !hex.eq_ignore_ascii_case(entry.sha256.trim()) {
        return Err(err(ErrorCode::BadRequest, "the download doesn't match the catalog"));
    }
    if !signed_by_trusted(digest.as_ref(), &entry.signature, &st.cfg.load().plugin_keys) {
        return Err(err(ErrorCode::Forbidden, "not signed by a trusted key"));
    }
    let (text, wasm) = tokio::task::spawn_blocking(move || -> Result<(String, Vec<u8>), String> {
        use std::io::Read;
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip)).map_err(|e| e.to_string())?;
        let mut read = |name: &str, max: u64| -> Result<Vec<u8>, String> {
            let f = z.by_name(name).map_err(|e| format!("{name}: {e}"))?;
            let mut out = Vec::new();
            f.take(max).read_to_end(&mut out).map_err(|e| e.to_string())?;
            Ok(out)
        };
        let text = String::from_utf8(read("plugin.toml", 256 * 1024)?).map_err(|e| e.to_string())?;
        Ok((text, read("plugin.wasm", 32 * 1024 * 1024)?))
    })
    .await
    .map_err(|e| err(ErrorCode::Internal, e.to_string()))?
    .map_err(|e| err(ErrorCode::BadRequest, e))?;
    install_files(&st, auth.name(), &ip.0.to_string(), &text, &wasm, Some(&entry.id)).await
}
