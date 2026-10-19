//! Server information, settings, audit log, log lines, folder browser and alternative speeds.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_core::config::Settings;
use corrent_core::engine::EngineCommand;
use corrent_proto::ErrorCode;
use corrent_proto::auth::{AuditEntry, Perms};
use corrent_proto::server::{FsEntry, FsListing, ServerInfo};
use serde::{Deserialize, Serialize};

use super::{ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

/// Stands in for a stored secret in settings sent to clients; sent back unchanged it means
/// "keep the stored value".
pub const SECRET_MASK: &str = "••••••••";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/server/info", get(info))
        .route("/server/settings", get(get_settings).put(put_settings))
        .route("/server/audit", get(audit))
        .route("/server/log", get(log))
        .route("/server/fs", get(fs))
        .route("/server/fs/mkdir", post(mkdir))
        .route("/server/alt-speed", post(alt_speed))
}

/// Public: lets a client recognise the server (and whether it still needs setting up).
async fn info(State(st): State<Arc<AppState>>) -> ApiResult<ServerInfo> {
    Ok(Json(ServerInfo {
        id: st.server_id.clone(),
        name: st.cfg.load().display_name(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: corrent_proto::API_VERSION,
        tls_fingerprint: st.tls_fingerprint.clone(),
        setup_required: st.identity.user_count().await? == 0,
        features: vec!["torrents".into(), "rss".into(), "search".into(), "events".into()],
    }))
}

fn mask(s: &mut String) {
    if !s.is_empty() {
        *s = SECRET_MASK.to_string();
    }
}

pub fn redact(mut s: Settings) -> Settings {
    mask(&mut s.connection.proxy_password);
    mask(&mut s.web_api.password_hash);
    for p in &mut s.search.providers {
        mask(&mut p.api_key);
    }
    s
}

/// Puts stored secrets back where a client sent the mask.
pub fn unredact(mut new: Settings, old: &Settings) -> Settings {
    if new.connection.proxy_password == SECRET_MASK {
        new.connection.proxy_password = old.connection.proxy_password.clone();
    }
    if new.web_api.password_hash == SECRET_MASK {
        new.web_api.password_hash = old.web_api.password_hash.clone();
    }
    for p in &mut new.search.providers {
        if p.api_key == SECRET_MASK {
            p.api_key =
                old.search.providers.iter().find(|o| o.url == p.url).map(|o| o.api_key.clone()).unwrap_or_default();
        }
    }
    new
}

async fn get_settings(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Settings> {
    auth.require(Perms::SETTINGS_READ)?;
    Ok(Json(redact((**st.settings.load()).clone())))
}

/// Replaces the settings (whole document, as returned by GET with any changes).
pub async fn apply_settings(st: &AppState, new: Settings) -> Result<Settings, super::ApiErr> {
    let _g = st.settings_lock.lock().await;
    let old = (**st.settings.load()).clone();
    let new = unredact(new, &old);
    if new == old {
        return Ok(new);
    }
    let paths = st.paths.clone();
    let save = new.clone();
    tokio::task::spawn_blocking(move || save.save(&paths))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?
        .map_err(|e| err(ErrorCode::Internal, format!("saving settings: {e:#}")))?;
    st.settings.store(Arc::new(new.clone()));
    let speed_changed = old.speed != new.speed;
    let restart = old.requires_restart(&new);
    st.engine.send(EngineCommand::ApplySettings(Box::new(new.clone())));
    if speed_changed && !restart {
        st.engine.send(EngineCommand::SetAltSpeed(st.engine.snapshot().session.alt_speed));
    }
    Ok(new)
}

async fn put_settings(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(new): Json<Settings>,
) -> ApiResult<Settings> {
    auth.require(Perms::SETTINGS_WRITE)?;
    let r = apply_settings(&st, new).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "settings", "", r.is_ok(), "").await;
    Ok(Json(redact(r?)))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuditQuery {
    limit: Option<u32>,
    before: Option<i64>,
}

async fn audit(State(st): State<Arc<AppState>>, auth: Auth, Query(q): Query<AuditQuery>) -> ApiResult<Vec<AuditEntry>> {
    auth.require(Perms::USERS_MANAGE)?;
    Ok(Json(st.identity.audit_log(q.limit.unwrap_or(200), q.before).await?))
}

#[derive(Serialize)]
struct LogLine {
    time: i64,
    level: String,
    target: String,
    message: String,
}

async fn log(auth: Auth) -> ApiResult<Vec<LogLine>> {
    auth.require(Perms::SETTINGS_WRITE)?;
    let lines = corrent_core::logbuf::LINES.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(
        lines
            .iter()
            .rev()
            .take(1000)
            .map(|l| LogLine {
                time: l.time,
                level: l.level.to_string(),
                target: l.target.clone(),
                message: l.message.clone(),
            })
            .collect(),
    ))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FsQuery {
    path: Option<String>,
    /// Include files (library management); default folders only.
    files: bool,
}

fn may_browse(auth: &Auth, files: bool) -> Result<(), super::ApiErr> {
    if files {
        return auth.require(Perms::LIBRARY_MANAGE);
    }
    if auth.0.can(Perms::TORRENTS_ADD) || auth.0.can(Perms::LIBRARY_MANAGE) || auth.0.can(Perms::SETTINGS_WRITE) {
        Ok(())
    } else {
        Err(err(ErrorCode::Forbidden, "permission denied"))
    }
}

/// Roots shown when no path is given: drives on Windows, `/` and home elsewhere.
fn roots() -> Vec<FsEntry> {
    let mut v = Vec::new();
    if cfg!(windows) {
        for d in b'A'..=b'Z' {
            let p = format!("{}:\\", d as char);
            if std::path::Path::new(&p).exists() {
                v.push(FsEntry { name: p.clone(), path: p, dir: true });
            }
        }
    } else {
        v.push(FsEntry { name: "/".into(), path: "/".into(), dir: true });
    }
    if let Some(h) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
        v.push(FsEntry { name: h.display().to_string(), path: h.display().to_string(), dir: true });
    }
    v.push(FsEntry {
        name: corrent_core::config::default_download_dir().display().to_string(),
        path: corrent_core::config::default_download_dir().display().to_string(),
        dir: true,
    });
    v
}

fn list_dir(path: PathBuf, files: bool) -> Result<FsListing, super::ApiErr> {
    let rd = std::fs::read_dir(&path).map_err(|e| err(ErrorCode::NotFound, format!("{}: {e}", path.display())))?;
    let mut entries: Vec<FsEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let dir = e.file_type().ok()?.is_dir() || e.path().is_dir();
            (dir || files).then(|| FsEntry { name, path: e.path().display().to_string(), dir })
        })
        .collect();
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries.truncate(5000);
    Ok(FsListing {
        parent: path.parent().map(|p| p.display().to_string()),
        free_space: corrent_core::engine::free_space_for(&path),
        path: path.display().to_string(),
        entries,
    })
}

async fn fs(auth: Auth, Query(q): Query<FsQuery>) -> ApiResult<FsListing> {
    may_browse(&auth, q.files)?;
    match q.path.filter(|p| !p.trim().is_empty()) {
        None => Ok(Json(FsListing { path: String::new(), parent: None, entries: roots(), free_space: None })),
        Some(p) => {
            let path = PathBuf::from(p);
            if !path.is_absolute() {
                return Err(err(ErrorCode::BadRequest, "absolute path required"));
            }
            let files = q.files;
            Ok(Json(
                tokio::task::spawn_blocking(move || list_dir(path, files))
                    .await
                    .map_err(|e| err(ErrorCode::Internal, e.to_string()))??,
            ))
        }
    }
}

#[derive(Deserialize)]
struct MkdirRequest {
    path: String,
}

async fn mkdir(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(r): Json<MkdirRequest>,
) -> ApiResult<FsEntry> {
    may_browse(&auth, false)?;
    let path = PathBuf::from(&r.path);
    if !path.is_absolute() {
        return Err(err(ErrorCode::BadRequest, "absolute path required"));
    }
    let p2 = path.clone();
    let res = tokio::task::spawn_blocking(move || std::fs::create_dir_all(&p2))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "mkdir", &r.path, res.is_ok(), "").await;
    res.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    Ok(Json(FsEntry {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.display().to_string(),
        dir: true,
    }))
}

#[derive(Deserialize)]
struct AltSpeed {
    on: bool,
}

async fn alt_speed(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(a): Json<AltSpeed>,
) -> ApiResult<serde_json::Value> {
    if !(auth.0.can(Perms::TORRENTS_MANAGE_ALL) || auth.0.can(Perms::SETTINGS_WRITE)) {
        return Err(err(ErrorCode::Forbidden, "permission denied"));
    }
    st.engine.send(EngineCommand::SetAltSpeed(a.on));
    Ok(Json(serde_json::json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked_and_kept() {
        let mut s = Settings::default();
        s.connection.proxy_password = "pw".into();
        s.search.providers.push(corrent_core::search::TorznabProvider {
            url: "http://j".into(),
            api_key: "k".into(),
            ..Default::default()
        });
        let r = redact(s.clone());
        assert_eq!(r.connection.proxy_password, SECRET_MASK);
        assert_eq!(r.search.providers[0].api_key, SECRET_MASK);
        assert_eq!(r.web_api.password_hash, "", "empty stays empty");
        assert_eq!(unredact(r, &s), s);
    }
}
