//! Playlists (`/playlists…`, each person's own; public ones readable by everyone) and
//! collections (`/collections…`, read-only: metadata providers create them).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_proto::library::{Collection, Item, Playlist};
use serde::Deserialize;

use super::library::lib_err;
use super::{ApiResult, Auth};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/playlists", get(list).post(create))
        .route("/playlists/{id}", get(one).patch(update).delete(remove))
        .route("/playlists/{id}/items", get(items).post(add))
        .route("/playlists/{id}/items/{index}", axum::routing::delete(remove_entry))
        .route("/playlists/{id}/move", post(move_entry))
        .route("/collections", get(collections))
        .route("/collections/{id}/items", get(collection_items))
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<Playlist>> {
    Ok(Json(st.library.playlists(auth.0.user.id).await.map_err(lib_err)?))
}

#[derive(Deserialize)]
struct NewPlaylist {
    name: String,
    #[serde(default)]
    public: bool,
    /// Items to start with.
    #[serde(default)]
    items: Vec<i64>,
}

async fn create(State(st): State<Arc<AppState>>, auth: Auth, Json(b): Json<NewPlaylist>) -> ApiResult<Playlist> {
    let user = auth.0.user.id;
    let id = st.library.create_playlist(user, b.name, b.public).await.map_err(lib_err)?;
    if !b.items.is_empty() {
        st.library.add_to_playlist(user, id, b.items).await.map_err(lib_err)?;
    }
    found(&st, user, id).await
}

async fn found(st: &AppState, user: i64, id: i64) -> ApiResult<Playlist> {
    let all = st.library.playlists(user).await.map_err(lib_err)?;
    all.into_iter().find(|p| p.id == id).map(Json).ok_or_else(|| lib_err(corrent_library::LibError::NotFound))
}

async fn one(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Playlist> {
    found(&st, auth.0.user.id, id).await
}

#[derive(Deserialize)]
struct Update {
    name: Option<String>,
    public: Option<bool>,
}

async fn update(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(b): Json<Update>,
) -> ApiResult<Playlist> {
    let user = auth.0.user.id;
    st.library.update_playlist(user, id, b.name, b.public).await.map_err(lib_err)?;
    found(&st, user, id).await
}

async fn remove(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<serde_json::Value> {
    st.library.delete_playlist(auth.0.user.id, id).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({})))
}

async fn items(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Vec<Item>> {
    Ok(Json(st.library.playlist_items(auth.0.user.id, id).await.map_err(lib_err)?))
}

#[derive(Deserialize)]
struct Add {
    items: Vec<i64>,
}

async fn add(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(b): Json<Add>,
) -> ApiResult<serde_json::Value> {
    let n = st.library.add_to_playlist(auth.0.user.id, id, b.items).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({ "added": n })))
}

async fn remove_entry(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path((id, index)): Path<(i64, usize)>,
) -> ApiResult<serde_json::Value> {
    st.library.edit_playlist_entry(auth.0.user.id, id, index, None).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
struct Move {
    from: usize,
    to: usize,
}

async fn move_entry(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(b): Json<Move>,
) -> ApiResult<serde_json::Value> {
    st.library.edit_playlist_entry(auth.0.user.id, id, b.from, Some(b.to)).await.map_err(lib_err)?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CollectionQuery {
    library_id: Option<i64>,
}

async fn collections(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<CollectionQuery>,
) -> ApiResult<Vec<Collection>> {
    Ok(Json(st.library.collections(auth.0.user.id, q.library_id).await.map_err(lib_err)?))
}

async fn collection_items(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Vec<Item>> {
    Ok(Json(st.library.collection_items(auth.0.user.id, id).await.map_err(lib_err)?))
}
