//! Internet radio: `GET/POST /radio`, `DELETE /radio/{id}` (adding needs settings rights) and
//! `GET /radio/{id}/stream` (the stream, relayed).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use serde::Deserialize;

use super::{ApiErr, ApiResult, Auth, err};
use crate::radio::Station;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/radio", get(list).post(add))
        .route("/radio/{id}", delete(remove))
        .route("/radio/{id}/stream", get(stream))
}

async fn list(State(st): State<Arc<AppState>>, _auth: Auth) -> ApiResult<Vec<Station>> {
    Ok(Json(crate::radio::load(&st.paths.data_dir)))
}

#[derive(Deserialize)]
struct NewStation {
    name: String,
    url: String,
    #[serde(default)]
    image: Option<String>,
}

fn web_url(u: &str) -> bool {
    (u.starts_with("http://") || u.starts_with("https://")) && u.len() <= 2000
}

async fn add(State(st): State<Arc<AppState>>, auth: Auth, Json(b): Json<NewStation>) -> ApiResult<Station> {
    auth.require(Perms::SETTINGS_WRITE)?;
    let url = b.url.trim().to_string();
    let name = b.name.trim().chars().take(120).collect::<String>();
    if !web_url(&url) || name.is_empty() {
        return Err(err(ErrorCode::BadRequest, "a station needs a name and an http(s) stream address"));
    }
    let s = Station {
        id: corrent_core::util::random_hex(6),
        name,
        url,
        image: b.image.map(|i| i.trim().to_string()).filter(|i| web_url(i)),
        added_at: corrent_core::util::now_unix(),
    };
    let s2 = s.clone();
    crate::radio::edit(&st, move |l| l.push(s2)).await.map_err(|e| err(ErrorCode::Internal, e))?;
    Ok(Json(s))
}

async fn remove(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    auth.require(Perms::SETTINGS_WRITE)?;
    crate::radio::edit(&st, |l| l.retain(|s| s.id != id)).await.map_err(|e| err(ErrorCode::Internal, e))?;
    Ok(Json(serde_json::json!({})))
}

/// Relays the station's stream (following `.pls`/`.m3u` playlists once).
async fn stream(State(st): State<Arc<AppState>>, _auth: Auth, Path(id): Path<String>) -> Result<Response, ApiErr> {
    let s = crate::radio::load(&st.paths.data_dir)
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such station"))?;
    let http = reqwest::Client::builder()
        .user_agent(concat!("Corrent/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    let open = |u: String| {
        let http = http.clone();
        async move { http.get(u).send().await.and_then(|r| r.error_for_status()) }
    };
    let mut res = open(s.url.clone()).await.map_err(|e| err(ErrorCode::EngineUnavailable, e.to_string()))?;
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if ct.contains("mpegurl")
        || ct.contains("scpls")
        || ct.contains("x-pls")
        || s.url.ends_with(".pls")
        || s.url.ends_with(".m3u")
    {
        let body = res.text().await.map_err(|e| err(ErrorCode::EngineUnavailable, e.to_string()))?;
        let target = crate::radio::playlist_target(&body)
            .ok_or_else(|| err(ErrorCode::EngineUnavailable, "the playlist names no stream"))?;
        res = open(target).await.map_err(|e| err(ErrorCode::EngineUnavailable, e.to_string()))?;
    }
    let ct = res.headers().get(header::CONTENT_TYPE).cloned().unwrap_or(HeaderValue::from_static("audio/mpeg"));
    Ok((
        [(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Body::from_stream(res.bytes_stream()),
    )
        .into_response())
}
