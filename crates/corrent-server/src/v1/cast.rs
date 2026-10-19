//! Casting: `GET /cast/devices`, `POST /cast/{device}/play`, `GET /cast/{device}`,
//! `POST /cast/{device}/command`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::library::{CastCommand, CastDevice, CastStatus, PlaybackRequest};
use serde::Deserialize;

use super::library::lib_err;
use super::{ApiResult, Auth, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cast/devices", get(devices))
        .route("/cast/{device}", get(status))
        .route("/cast/{device}/play", post(play))
        .route("/cast/{device}/command", post(command))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DevicesQuery {
    refresh: bool,
}

async fn devices(
    State(st): State<Arc<AppState>>,
    _auth: Auth,
    Query(q): Query<DevicesQuery>,
) -> ApiResult<Vec<CastDevice>> {
    let list = st.casts.devices(q.refresh).await.map_err(|e| err(ErrorCode::Internal, format!("{e:#}")))?;
    Ok(Json(list.into_iter().map(|f| f.device).collect()))
}

/// Someone else's running cast is theirs (administrators may take over).
fn may_control(st: &AppState, auth: &Auth, device: &str) -> Result<(), super::ApiErr> {
    match st.casts.status(device) {
        Some(s)
            if s.by != auth.name()
                && !matches!(s.state.as_str(), "idle" | "error")
                && !auth.0.can(corrent_proto::auth::Perms::USERS_MANAGE) =>
        {
            Err(err(ErrorCode::Conflict, format!("{} is casting there", s.by)))
        }
        _ => Ok(()),
    }
}

async fn status(State(st): State<Arc<AppState>>, _auth: Auth, Path(device): Path<String>) -> ApiResult<CastStatus> {
    st.casts.status(&device).map(Json).ok_or_else(|| err(ErrorCode::NotFound, "nothing is casting there"))
}

#[derive(Deserialize)]
struct PlayBody {
    item_id: i64,
    #[serde(default)]
    position_ms: i64,
    #[serde(default)]
    audio_track: Option<i64>,
    #[serde(default)]
    file_id: Option<i64>,
}

async fn play(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(device): Path<String>,
    Json(b): Json<PlayBody>,
) -> ApiResult<CastStatus> {
    may_control(&st, &auth, &device)?;
    let found = st.casts.device(&device).await.map_err(|e| err(ErrorCode::NotFound, format!("{e:#}")))?;
    let host =
        crate::cast::local_ip_towards(found.addr).ok_or_else(|| err(ErrorCode::Internal, "no route to the device"))?;
    let links = crate::remote::lan_links(&st).await.map_err(|e| err(ErrorCode::EngineUnavailable, format!("{e:#}")))?;
    let user = auth.0.user.id;
    let detail = st.library.item(user, b.item_id).await.map_err(lib_err)?;
    let mut req = PlaybackRequest {
        profile: Some("chromecast".into()),
        file_id: b.file_id,
        audio_track: b.audio_track,
        ..Default::default()
    };
    let mut info = super::playback::prepare(&st, &auth, b.item_id, req.clone()).await?;
    // Unfinished downloads only play through the engine: transcode (which reads through it).
    if info.method == "direct" && info.downloading {
        req.force_transcode = true;
        info = super::playback::prepare(&st, &auth, b.item_id, req).await?;
    }
    let (url, content_type) = match info.session_id.clone() {
        Some(session) => (links.hls(host, &session), "application/x-mpegurl".to_string()),
        None => {
            let path = st.library.file_path(user, info.file_id).await.map_err(lib_err)?;
            let path = std::path::PathBuf::from(path);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_ascii_lowercase();
            (links.media(host, user, info.file_id, &ext), super::library::mime_for(&path).to_string())
        }
    };
    let image = detail
        .item
        .images
        .iter()
        .find(|i| matches!(i.kind.as_str(), "backdrop" | "poster" | "primary"))
        .or(detail.item.images.first())
        .map(|i| links.art(host, &i.sha));
    st.casts.play(
        st.clone(),
        found.clone(),
        crate::cast::Media {
            url,
            content_type,
            title: detail.item.title.clone(),
            subtitle: detail.item.context.join(" · "),
            image,
            start_s: b.position_ms.max(0) as f64 / 1000.0,
            item_id: b.item_id,
            by: auth.name().to_string(),
            user_id: user,
            hls_session: info.session_id,
            duration_ms: info.duration_ms,
        },
    );
    Ok(Json(st.casts.status(&found.device.id).unwrap_or_default()))
}

async fn command(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(device): Path<String>,
    Json(c): Json<CastCommand>,
) -> ApiResult<serde_json::Value> {
    may_control(&st, &auth, &device)?;
    if !st.casts.command(&device, c) {
        return Err(err(ErrorCode::NotFound, "nothing is casting there"));
    }
    Ok(Json(serde_json::json!({})))
}
