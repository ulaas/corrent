//! Playback: the direct-play/transcode decision for a client, HLS sessions, WebVTT subtitles
//! and trickplay thumbnails.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use corrent_proto::library::{MediaFile, PlaybackInfo, PlaybackRequest, PlaybackSession, SubtitleTrack};
use corrent_transcode::decide::{
    self, AudioFacts, ClientProfile, Decision, MediaFacts, PlayRequest, SubFacts, VideoFacts,
};
use corrent_transcode::hls::{HlsError, SessionSpec};

use super::library::lib_err;
use super::{ApiErr, ApiResult, Auth, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/playback/{item}", post(start))
        .route("/playback/sessions", get(sessions))
        .route("/playback/sessions/{id}", delete(stop))
        .route("/hls/{id}/index.m3u8", get(playlist))
        .route("/hls/{id}/{segment}", get(segment))
        .route("/files/{id}/subtitles/{name}", get(subtitle))
        .route("/files/{id}/trickplay/{name}", get(trickplay))
}

const TEXT_SUBS: &[&str] = &["srt", "subrip", "ass", "ssa", "vtt", "webvtt"];

fn facts(f: &MediaFile) -> MediaFacts {
    let video = f.streams.iter().find(|s| s.kind == "video").map(|s| VideoFacts {
        codec: s.codec.clone().unwrap_or_default(),
        profile: s.profile.clone(),
        width: s.width,
        height: s.height,
        fps: s.fps,
    });
    MediaFacts {
        container: f.container.clone(),
        duration_ms: f.duration_ms,
        video,
        audio: f
            .streams
            .iter()
            .filter(|s| s.kind == "audio")
            .map(|s| AudioFacts {
                index: s.index,
                codec: s.codec.clone().unwrap_or_default(),
                channels: s.channels,
                default: s.default,
                lang: s.lang.clone(),
            })
            .collect(),
        subtitles: f
            .streams
            .iter()
            .filter(|s| s.kind == "subtitle")
            .map(|s| SubFacts { index: s.index, codec: s.codec.clone(), external: s.external, lang: s.lang.clone() })
            .collect(),
        bitrate: f.duration_ms.filter(|d| *d > 0).map(|d| f.size * 8 * 1000 / d),
    }
}

fn hls_err(e: HlsError) -> ApiErr {
    match e {
        HlsError::NotFound => err(ErrorCode::NotFound, "no such playback session"),
        HlsError::TooMany => err(ErrorCode::TooManyRequests, "too many transcodes at once"),
        HlsError::Failed(m) => err(ErrorCode::Internal, m),
    }
}

/// `<data>/trickplay/<file id>/`
fn trickplay_dir(st: &AppState, file_id: i64) -> PathBuf {
    st.paths.data_dir.join("trickplay").join(file_id.to_string())
}

/// Starts making trickplay sheets for a file (once; one at a time server-wide).
pub fn ensure_trickplay(st: &Arc<AppState>, file_id: i64, input: String) {
    {
        let mut busy = st.trickplay_busy.lock().unwrap_or_else(|e| e.into_inner());
        if !busy.insert(file_id) {
            return;
        }
    }
    let st = st.clone();
    tokio::spawn(async move {
        let _one = st.trickplay_slot.lock().await;
        let dir = trickplay_dir(&st, file_id);
        if !dir.join("index.vtt").exists() {
            let _ = std::fs::create_dir_all(&dir);
            match corrent_transcode::trickplay::generate(&input, &dir, 10, 240, corrent_mpv::worker_path()).await {
                Ok(n) => tracing::info!("trickplay for file {file_id}: {n} frames"),
                Err(e) => {
                    tracing::debug!("trickplay for file {file_id}: {e}");
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
        st.trickplay_busy.lock().unwrap_or_else(|e| e.into_inner()).remove(&file_id);
    });
}

async fn start(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(item): Path<i64>,
    body: Option<Json<PlaybackRequest>>,
) -> ApiResult<PlaybackInfo> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let user = auth.0.user.id;
    let detail = st.library.item(user, item).await.map_err(lib_err)?;
    let file = match req.file_id {
        Some(id) => detail.files.iter().find(|f| f.id == id),
        None => detail.files.iter().find(|f| f.role == "main" && !f.missing),
    }
    .ok_or_else(|| err(ErrorCode::NotFound, "nothing to play"))?
    .clone();
    let profile = ClientProfile::named(req.profile.as_deref().unwrap_or("web"));
    let facts = facts(&file);
    let decision = decide::decide(
        &facts,
        &profile,
        &PlayRequest {
            audio_track: req.audio_track,
            burn_subtitle: req.burn_subtitle,
            max_bitrate: req.max_bitrate,
            max_height: req.max_height,
            force_transcode: req.force_transcode,
        },
    );
    let subtitles = file
        .streams
        .iter()
        .filter(|s| s.kind == "subtitle")
        .map(|s| {
            let text = s.codec.as_deref().is_some_and(|c| TEXT_SUBS.contains(&c));
            SubtitleTrack {
                index: s.index,
                lang: s.lang.clone(),
                title: s.title.clone(),
                forced: s.forced,
                external: s.external,
                url: (s.external && text).then(|| format!("/api/v1/files/{}/subtitles/{}.vtt", file.id, s.index)),
            }
        })
        .collect();
    let has_video = facts.video.is_some();
    let tp = trickplay_dir(&st, file.id);
    let trickplay = if tp.join("index.vtt").exists() {
        Some(format!("/api/v1/files/{}/trickplay/index.vtt", file.id))
    } else {
        if has_video {
            ensure_trickplay(&st, file.id, file.path.clone());
        }
        None
    };
    let (method, url, session_id, reasons) = match decision {
        Decision::Direct { reasons } => ("direct".to_string(), format!("/api/v1/files/{}", file.id), None, reasons),
        Decision::Transcode { video, audio, burn_subtitle, reasons } => {
            auth.require(Perms::PLAYBACK_TRANSCODE)?;
            if video.is_some() && st.hls_encoder.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                return Err(err(
                    ErrorCode::EngineUnavailable,
                    "video transcoding is unavailable on this server (no working H.264 encoder or libmpv)",
                ));
            }
            // External subtitles are burned from their file; embedded ones by track id.
            let ext = burn_subtitle.filter(|i| *i >= 1000);
            let burn_file = match ext {
                Some(i) => Some(st.library.subtitle_path(user, file.id, i).await.map_err(lib_err)?),
                None => None,
            };
            let spec = SessionSpec {
                input: file.path.clone(),
                duration_ms: file.duration_ms.unwrap_or(0).max(1000),
                fps: facts.video.as_ref().and_then(|v| v.fps),
                video,
                audio,
                burn_subtitle: if ext.is_some() { None } else { burn_subtitle },
                burn_subtitle_file: burn_file,
                user: auth.name().to_string(),
                title: detail.item.title.clone(),
                item_id: Some(item),
            };
            let id = st.hls.create(spec).map_err(hls_err)?;
            ("hls".to_string(), format!("/api/v1/hls/{id}/index.m3u8"), Some(id), reasons)
        }
    };
    Ok(Json(PlaybackInfo {
        method,
        url,
        session_id,
        file_id: file.id,
        duration_ms: file.duration_ms,
        subtitles,
        trickplay,
        reasons,
    }))
}

fn owns(st: &AppState, auth: &Auth, id: &str) -> Result<(), ApiErr> {
    match st.hls.owner(id) {
        Some(u) if u == auth.name() || auth.0.can(Perms::USERS_MANAGE) => Ok(()),
        _ => Err(err(ErrorCode::NotFound, "no such playback session")),
    }
}

async fn playlist(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> Result<Response, ApiErr> {
    owns(&st, &auth, &id)?;
    let body = st.hls.playlist(&id).map_err(hls_err)?;
    Ok(([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-store")], body)
        .into_response())
}

async fn segment(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path((id, seg)): Path<(String, String)>,
    req: Request,
) -> Result<Response, ApiErr> {
    owns(&st, &auth, &id)?;
    let n: u64 = seg
        .strip_suffix(".ts")
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| err(ErrorCode::NotFound, "no such segment"))?;
    let path = st.hls.segment(&id, n).await.map_err(hls_err)?;
    let mut res = tower::ServiceExt::oneshot(
        tower_http::services::ServeFile::new_with_mime(&path, &"video/mp2t".parse().expect("static")),
        req,
    )
    .await
    .map_err(|e| err(ErrorCode::Internal, e.to_string()))?
    .map(axum::body::Body::new);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(res)
}

async fn stop(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    owns(&st, &auth, &id)?;
    st.hls.stop(&id).await;
    Ok(Json(serde_json::json!({})))
}

async fn sessions(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<PlaybackSession>> {
    let all = auth.0.can(Perms::USERS_MANAGE);
    Ok(Json(
        st.hls
            .sessions()
            .into_iter()
            .filter(|s| all || s.user == auth.name())
            .map(|s| PlaybackSession {
                id: s.id,
                user: s.user,
                title: s.title,
                item_id: s.item_id,
                encoder: s.encoder,
                video: s.video,
                segments: s.segments,
                produced: s.produced,
                idle_secs: s.idle_secs,
            })
            .collect(),
    ))
}

async fn subtitle(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path((id, name)): Path<(i64, String)>,
) -> Result<Response, ApiErr> {
    let idx: i64 = name
        .strip_suffix(".vtt")
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| err(ErrorCode::NotFound, "no such subtitle"))?;
    let path = st.library.subtitle_path(auth.0.user.id, id, idx).await.map_err(lib_err)?;
    let ext = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
    let bytes = tokio::fs::read(&path).await.map_err(|e| err(ErrorCode::NotFound, e.to_string()))?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(err(ErrorCode::PayloadTooLarge, "subtitle file too large"));
    }
    let vtt = corrent_transcode::vtt::to_vtt(&ext, &bytes)
        .ok_or_else(|| err(ErrorCode::BadRequest, "not a text subtitle"))?;
    Ok(([(header::CONTENT_TYPE, "text/vtt; charset=utf-8"), (header::CACHE_CONTROL, "private, max-age=600")], vtt)
        .into_response())
}

async fn trickplay(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path((id, name)): Path<(i64, String)>,
    req: Request,
) -> Result<Response, ApiErr> {
    // The user must be allowed to see the file.
    st.library.file_path(auth.0.user.id, id).await.map_err(lib_err)?;
    let ok = name == "index.vtt"
        || (name.starts_with("sheet-")
            && name.ends_with(".jpg")
            && name[6..name.len() - 4].chars().all(|c| c.is_ascii_digit()));
    if !ok {
        return Err(err(ErrorCode::NotFound, "no such file"));
    }
    let path = trickplay_dir(&st, id).join(&name);
    if !path.exists() {
        return Err(err(ErrorCode::NotFound, "not generated yet"));
    }
    let mime = if name.ends_with(".vtt") { "text/vtt" } else { "image/jpeg" };
    Ok(tower::ServiceExt::oneshot(
        tower_http::services::ServeFile::new_with_mime(&path, &mime.parse().expect("static")),
        req,
    )
    .await
    .map_err(|e| err(ErrorCode::Internal, e.to_string()))?
    .map(axum::body::Body::new))
}
