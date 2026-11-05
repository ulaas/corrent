//! Casting: `GET /cast/devices`, `POST /cast/{device}/play`, `GET /cast/{device}`,
//! `POST /cast/{device}/command`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use corrent_proto::library::{CastCommand, CastConfig, CastDevice, CastStatus, PlaybackRequest};
use corrent_transcode::decide::ClientProfile;
use serde::Deserialize;

use super::library::lib_err;
use super::{ApiResult, Auth, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cast/devices", get(devices))
        .route("/cast/active", get(active))
        .route("/cast/config", get(get_config).put(put_config))
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
    let cfg = enabled(&st)?;
    let list =
        st.casts.devices(q.refresh, &cfg.devices).await.map_err(|e| err(ErrorCode::Internal, format!("{e:#}")))?;
    Ok(Json(list.into_iter().map(|f| f.device).collect()))
}

/// The casting settings, unless casting is off.
fn enabled(st: &AppState) -> Result<CastConfig, super::ApiErr> {
    let cfg = st.cfg.load().cast.clone();
    if cfg.enabled { Ok(cfg) } else { Err(err(ErrorCode::Conflict, "casting is turned off in the settings")) }
}

/// What a device is asked to play as it is: the Default Media Receiver's formats, as far as the
/// settings go.
fn profile(c: &CastConfig) -> ClientProfile {
    let mut p = ClientProfile::chromecast();
    p.max_height = (c.max_height > 0).then_some(i64::from(c.max_height));
    p.max_bitrate = (c.max_bitrate_kbps > 0).then_some(i64::from(c.max_bitrate_kbps) * 1000);
    if c.hevc {
        p.video_codecs.push("hevc".into());
    }
    p
}

/// The casts the caller could control: the ones to attach to.
async fn active(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<CastStatus>> {
    let mut list = st.casts.active();
    list.retain(|s| s.by == auth.name() || auth.0.can(Perms::USERS_MANAGE));
    Ok(Json(list))
}

async fn get_config(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<CastConfig> {
    auth.require(Perms::SETTINGS_WRITE)?;
    Ok(Json(st.cfg.load().cast.clone()))
}

async fn put_config(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(mut c): Json<CastConfig>,
) -> ApiResult<CastConfig> {
    auth.require(Perms::SETTINGS_WRITE)?;
    c.devices = c.devices.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if let Some(bad) = c.devices.iter().find(|l| crate::cast::manual_device(l).is_none()) {
        return Err(err(ErrorCode::BadRequest, format!("not a device on this network: {bad}")));
    }
    let own = st.cfg.load();
    if c.port != 0 && [own.https_port, own.http_port, own.jellyfin_port].contains(&c.port) {
        return Err(err(ErrorCode::BadRequest, format!("port {} is used by the server itself", c.port)));
    }
    drop(own);
    {
        let _w = st.settings_lock.lock().await;
        // From the file: what a hosted server forces (its loopback address) isn't its setting.
        let mut file =
            crate::ServerConfig::load(&st.paths.data_dir).map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
        file.cast = c.clone();
        file.save(&st.paths.data_dir).map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
        let mut cfg = (*st.cfg.load_full()).clone();
        cfg.cast = c.clone();
        st.cfg.store(Arc::new(cfg));
    }
    // A new port applies from the next cast, unless something is casting now.
    crate::remote::close_cast_listener(&st, std::time::Duration::ZERO).await;
    Ok(Json(c))
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

async fn status(State(st): State<Arc<AppState>>, auth: Auth, Path(device): Path<String>) -> ApiResult<CastStatus> {
    // What someone else is watching is theirs.
    st.casts
        .status(&device)
        .filter(|s| s.by == auth.name() || auth.0.can(Perms::USERS_MANAGE))
        .map(Json)
        .ok_or_else(|| err(ErrorCode::NotFound, "nothing is casting there"))
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
    /// The subtitle to show (a text one from a file goes to the device, any other is burned in).
    #[serde(default)]
    subtitle: Option<i64>,
}

async fn play(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(device): Path<String>,
    Json(b): Json<PlayBody>,
) -> ApiResult<CastStatus> {
    let cfg = enabled(&st)?;
    may_control(&st, &auth, &device)?;
    let found = st.casts.device(&device, &cfg.devices).await.map_err(|e| err(ErrorCode::NotFound, format!("{e:#}")))?;
    let host =
        crate::cast::local_ip_towards(found.addr).ok_or_else(|| err(ErrorCode::Internal, "no route to the device"))?;
    let links = crate::remote::lan_links(&st).await.map_err(|e| err(ErrorCode::EngineUnavailable, format!("{e:#}")))?;
    let user = auth.0.user.id;
    let detail = st.library.item(user, b.item_id).await.map_err(lib_err)?;
    let file = match b.file_id {
        Some(id) => detail.files.iter().find(|f| f.id == id),
        None => detail.files.iter().find(|f| f.role == "main" && !f.missing),
    };
    let chosen =
        b.subtitle.filter(|i| *i > 0).and_then(|i| file?.streams.iter().find(|s| s.kind == "subtitle" && s.index == i));
    // The device shows text from a file by itself; anything else has to be in the picture.
    let side =
        chosen.filter(|s| s.external && s.codec.as_deref().is_some_and(|c| super::playback::TEXT_SUBS.contains(&c)));
    let mut req = PlaybackRequest {
        profile: Some("chromecast".into()),
        file_id: b.file_id,
        audio_track: b.audio_track,
        burn_subtitle: chosen.filter(|_| side.is_none()).map(|s| s.index),
        ..Default::default()
    };
    let shape = profile(&cfg);
    let mut info = super::playback::prepare(&st, &auth, b.item_id, req.clone(), Some(shape.clone()), false).await?;
    // Unfinished downloads only play through the engine: transcode (which reads through it).
    if info.method == "direct" && info.downloading {
        req.force_transcode = true;
        info = super::playback::prepare(&st, &auth, b.item_id, req, Some(shape), false).await?;
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
    let subtitle_track = side.map(|s| crate::cast::SideTrack {
        url: links.subtitle(host, user, info.file_id, s.index),
        name: s.title.clone().or_else(|| s.lang.clone()).unwrap_or_else(|| s.index.to_string()),
        lang: s.lang.clone(),
    });
    st.casts.touch();
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
            subtitle_track,
            hits: links.hits(),
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
