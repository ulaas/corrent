//! Podcast subscriptions: `GET/POST /podcasts`, `DELETE /podcasts/{id}`,
//! `POST /podcasts/{id}/refresh` (people who may change settings).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use corrent_proto::library::LibraryKind;
use serde::Deserialize;

use super::library::lib_err;
use super::{ApiResult, Auth, err};
use crate::podcasts::Podcast;
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/podcasts", get(list).post(add))
        .route("/podcasts/{id}", delete(remove))
        .route("/podcasts/{id}/refresh", post(refresh))
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<Podcast>> {
    auth.require(Perms::SETTINGS_WRITE)?;
    Ok(Json(crate::podcasts::load(&st.paths.data_dir)))
}

#[derive(Deserialize)]
struct NewPodcast {
    url: String,
    library_id: i64,
    #[serde(default = "five")]
    keep: u32,
}

fn five() -> u32 {
    5
}

async fn add(State(st): State<Arc<AppState>>, auth: Auth, Json(b): Json<NewPodcast>) -> ApiResult<Podcast> {
    auth.require(Perms::SETTINGS_WRITE)?;
    let url = b.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.len() > 2000 {
        return Err(err(ErrorCode::BadRequest, "a feed address starts with http:// or https://"));
    }
    let lib = st.library.get(b.library_id).await.map_err(lib_err)?;
    if lib.kind != LibraryKind::Podcasts {
        return Err(err(ErrorCode::BadRequest, "choose a podcast library"));
    }
    let feed = crate::podcasts::fetch(&url).await.map_err(|e| err(ErrorCode::BadRequest, e))?;
    let p = Podcast {
        id: corrent_core::util::random_hex(6),
        url: url.clone(),
        title: feed.title,
        library_id: b.library_id,
        keep: b.keep.min(1000),
        added_at: corrent_core::util::now_unix(),
        ..Default::default()
    };
    let p2 = p.clone();
    let dup = crate::podcasts::edit(&st, move |l| {
        if l.iter().any(|x| x.url == url) {
            return true;
        }
        l.push(p2);
        false
    })
    .await
    .map_err(|e| err(ErrorCode::Internal, e))?;
    if dup {
        return Err(err(ErrorCode::Conflict, "already subscribed"));
    }
    spawn_check(&st, p.clone());
    Ok(Json(p))
}

/// Downloads in the background (episodes can be large).
fn spawn_check(st: &Arc<AppState>, p: Podcast) {
    let st = st.clone();
    tokio::spawn(async move {
        let r = crate::podcasts::check(&st, &p).await;
        let _ = crate::podcasts::edit(&st, |l| {
            if let Some(x) = l.iter_mut().find(|x| x.id == p.id) {
                x.checked_at = Some(corrent_core::util::now_unix());
                match r {
                    Ok((title, _, n)) => {
                        x.title = title;
                        x.episodes = n;
                        x.error = None;
                    }
                    Err(e) => x.error = Some(e),
                }
            }
        })
        .await;
    });
}

async fn remove(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    auth.require(Perms::SETTINGS_WRITE)?;
    // The downloaded episodes stay in the library.
    crate::podcasts::edit(&st, |l| l.retain(|x| x.id != id)).await.map_err(|e| err(ErrorCode::Internal, e))?;
    Ok(Json(serde_json::json!({})))
}

async fn refresh(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    auth.require(Perms::SETTINGS_WRITE)?;
    let p = crate::podcasts::load(&st.paths.data_dir)
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such podcast"))?;
    spawn_check(&st, p);
    Ok(Json(serde_json::json!({})))
}
