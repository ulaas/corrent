//! Administration: scheduled tasks, notification targets and backups.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use corrent_proto::tasks::{BackupFile, NotifyTarget, TaskInfo, TaskRun, TaskUpdate};

use super::{ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(tasks))
        .route("/tasks/{id}", put(update_task))
        .route("/tasks/{id}/run", post(run_task))
        .route("/tasks/{id}/history", get(history))
        .route("/notifications", get(targets).post(add_target))
        .route("/notifications/{id}", put(edit_target).delete(delete_target))
        .route("/notifications/{id}/test", post(test_target))
        .route("/backups", get(backups).post(backup_now))
        .route("/backups/{name}", get(download_backup))
        .route("/backups/{name}/restore", post(restore_backup))
        .route("/backups/upload", post(upload_backup).layer(axum::extract::DefaultBodyLimit::max(2 << 30)))
        .route("/backups/restore", get(restore_status).delete(cancel_restore))
}

async fn tasks(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<TaskInfo>> {
    auth.require(Perms::SETTINGS_READ)?;
    Ok(Json(crate::tasks::list(&st).await))
}

async fn update_task(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Json(u): Json<TaskUpdate>,
) -> ApiResult<Vec<TaskInfo>> {
    auth.require(Perms::SETTINGS_WRITE)?;
    crate::tasks::update(&st, &id, u).await.map_err(|e| err(ErrorCode::NotFound, e))?;
    Ok(Json(crate::tasks::list(&st).await))
}

/// Starts a task and answers right away with its run (the page polls for the outcome).
async fn run_task(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<String>,
) -> ApiResult<TaskRun> {
    auth.require(Perms::SETTINGS_WRITE)?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "task_run", &id, true, "").await;
    match crate::tasks::start_run(st.clone(), &id, auth.name()).await {
        Ok(r) => Ok(Json(r)),
        Err(e) if e == "already running" => Err(err(ErrorCode::Conflict, e)),
        Err(e) => Err(err(ErrorCode::NotFound, e)),
    }
}

async fn history(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<Vec<TaskRun>> {
    auth.require(Perms::SETTINGS_READ)?;
    Ok(Json(crate::tasks::history(&st, &id, 50).await))
}

async fn targets(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<NotifyTarget>> {
    admin(&auth)?;
    crate::notify::list(st.identity.db()).await.map(Json).map_err(|e| err(ErrorCode::Internal, e))
}

async fn save(st: &AppState, t: NotifyTarget) -> ApiResult<NotifyTarget> {
    crate::notify::validate(&t).map_err(|e| err(ErrorCode::BadRequest, e))?;
    if let Some(bad) = t.events.iter().find(|e| !corrent_proto::tasks::NOTIFY_EVENTS.contains(&e.as_str())) {
        return Err(err(ErrorCode::BadRequest, format!("unknown event {bad}")));
    }
    crate::notify::save(st.identity.db(), t)
        .await
        .map_err(|e| err(ErrorCode::Internal, e))?
        .map(Json)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such target"))
}

async fn add_target(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(mut t): Json<NotifyTarget>,
) -> ApiResult<NotifyTarget> {
    admin(&auth)?;
    t.id = 0;
    save(&st, t).await
}

async fn edit_target(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(mut t): Json<NotifyTarget>,
) -> ApiResult<NotifyTarget> {
    admin(&auth)?;
    t.id = id;
    save(&st, t).await
}

async fn delete_target(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    admin(&auth)?;
    match crate::notify::delete(st.identity.db(), id).await {
        Ok(true) => Ok(Json(serde_json::json!({}))),
        Ok(false) => Err(err(ErrorCode::NotFound, "no such target")),
        Err(e) => Err(err(ErrorCode::Internal, e)),
    }
}

/// Sends a test message; the error (if any) comes back for the person to see.
async fn test_target(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<serde_json::Value> {
    admin(&auth)?;
    let t = crate::notify::list(st.identity.db())
        .await
        .map_err(|e| err(ErrorCode::Internal, e))?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such target"))?;
    crate::notify::deliver(&st, &t, "test", t!("Test notification"), t!("Notifications from Corrent reach you here."))
        .await
        .map_err(|e| err(ErrorCode::BadRequest, e))?;
    Ok(Json(serde_json::json!({})))
}

fn admin(auth: &Auth) -> Result<(), super::ApiErr> {
    auth.require(Perms::SETTINGS_WRITE)?;
    auth.require(Perms::USERS_MANAGE)
}

async fn backups(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<BackupFile>> {
    admin(&auth)?;
    Ok(Json(crate::backups::list(&st.paths.data_dir)))
}

async fn backup_now(State(st): State<Arc<AppState>>, auth: Auth, ip: ClientIp) -> ApiResult<BackupFile> {
    admin(&auth)?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "backup", "", true, "").await;
    crate::backups::create(&st).await.map(Json).map_err(|e| err(ErrorCode::Internal, e))
}

/// The backup holds private keys: admins only, never cached.
async fn download_backup(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(name): Path<String>,
) -> Result<Response, super::ApiErr> {
    admin(&auth)?;
    let path =
        crate::backups::path_of(&st.paths.data_dir, &name).ok_or_else(|| err(ErrorCode::NotFound, "no such backup"))?;
    let bytes = tokio::fs::read(&path).await.map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "backup_download", &name, true, "").await;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        bytes,
    )
        .into_response())
}

#[derive(serde::Serialize)]
struct RestoreStatus {
    /// The backup a restart will restore.
    pending: Option<String>,
}

async fn restore_status(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<RestoreStatus> {
    admin(&auth)?;
    Ok(Json(RestoreStatus { pending: crate::backups::pending(&st.paths.data_dir) }))
}

/// Stages a restore: the server applies it at its next start.
async fn restore_backup(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(name): Path<String>,
) -> ApiResult<RestoreStatus> {
    admin(&auth)?;
    let data = st.paths.data_dir.clone();
    let n = name.clone();
    let r = tokio::task::spawn_blocking(move || crate::backups::stage_restore(&data, &n))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "backup_restore", &name, r.is_ok(), "").await;
    r.map_err(|e| err(ErrorCode::BadRequest, e))?;
    Ok(Json(RestoreStatus { pending: Some(name) }))
}

async fn cancel_restore(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<RestoreStatus> {
    admin(&auth)?;
    crate::backups::cancel_restore(&st.paths.data_dir);
    Ok(Json(RestoreStatus { pending: None }))
}

async fn upload_backup(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    body: axum::body::Bytes,
) -> ApiResult<BackupFile> {
    admin(&auth)?;
    let data = st.paths.data_dir.clone();
    let r = tokio::task::spawn_blocking(move || crate::backups::store_upload(&data, &body))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "backup_upload", "", r.is_ok(), "").await;
    r.map(Json).map_err(|e| err(ErrorCode::BadRequest, e))
}
