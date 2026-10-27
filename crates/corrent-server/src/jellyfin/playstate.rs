//! Watch state: played/favorite marks, user data and playback progress reports.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use corrent_proto::library::{ItemQuery, StateUpdate};
use serde_json::Value;

use super::{JfAuth, Q, dto, ids, json, no_content, not_found};
use crate::state::AppState;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/userplayeditems/{id}", post(played).delete(unplayed))
        .route("/users/{u}/playeditems/{id}", post(played_of).delete(unplayed_of))
        .route("/userfavoriteitems/{id}", post(favorite).delete(unfavorite))
        .route("/users/{u}/favoriteitems/{id}", post(favorite_of).delete(unfavorite_of))
        .route("/useritems/{id}/userdata", get(data).post(set_data))
        .route("/users/{u}/items/{id}/userdata", get(data_of).post(set_data_of))
        .route("/useritems/{id}/rating", post(rating).delete(rating))
        .route("/users/{u}/items/{id}/rating", post(rating_of).delete(rating_of))
        .route("/sessions/playing", post(started))
        .route("/sessions/playing/progress", post(progress))
        .route("/sessions/playing/stopped", post(stopped))
        .route("/sessions/playing/ping", post(ping))
        .route("/playingitems/{id}", post(legacy_progress).delete(legacy_progress))
        .route("/playingitems/{id}/progress", post(legacy_progress))
        .route("/users/{u}/playingitems/{id}", post(legacy_progress_of).delete(legacy_progress_of))
        .route("/users/{u}/playingitems/{id}/progress", post(legacy_progress_of))
}

/// The item's user data now (after a change).
async fn user_data(st: &AppState, user: i64, item: i64) -> Response {
    match st.library.items(user, ItemQuery { ids: Some(vec![item]), ..Default::default() }).await {
        Ok(p) => match p.items.first() {
            Some(it) => json(dto::user_data(it)),
            None => not_found(),
        },
        Err(_) => not_found(),
    }
}

async fn change(st: &AppState, user: i64, id: &str, u: StateUpdate) -> Response {
    let Some(item) = ids::dec_kind(id, ids::ITEM) else { return not_found() };
    if st.library.set_state(user, item, u).await.is_err() {
        return not_found();
    }
    user_data(st, user, item).await
}

fn mark(played: Option<bool>, favorite: Option<bool>) -> StateUpdate {
    StateUpdate { played, favorite, ..Default::default() }
}

async fn played(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => change(&st, u, &id, mark(Some(true), None)).await,
        Err(r) => r,
    }
}

async fn unplayed(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => change(&st, u, &id, mark(Some(false), None)).await,
        Err(r) => r,
    }
}

async fn played_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => change(&st, u, &id, mark(Some(true), None)).await,
        Err(r) => r,
    }
}

async fn unplayed_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => change(&st, u, &id, mark(Some(false), None)).await,
        Err(r) => r,
    }
}

async fn favorite(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => change(&st, u, &id, mark(None, Some(true))).await,
        Err(r) => r,
    }
}

async fn unfavorite(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => change(&st, u, &id, mark(None, Some(false))).await,
        Err(r) => r,
    }
}

async fn favorite_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => change(&st, u, &id, mark(None, Some(true))).await,
        Err(r) => r,
    }
}

async fn unfavorite_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => change(&st, u, &id, mark(None, Some(false))).await,
        Err(r) => r,
    }
}

async fn data(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    match a.check_user(q.get("userid")) {
        Ok(u) => user_data(&st, u, item).await,
        Err(r) => r,
    }
}

async fn data_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    match a.check_user(Some(&u)) {
        Ok(u) => user_data(&st, u, item).await,
        Err(r) => r,
    }
}

fn field<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    let lower = format!("{}{}", name[..1].to_ascii_lowercase(), &name[1..]);
    v.get(name).or_else(|| v.get(&lower)).filter(|v| !v.is_null())
}

fn update_of(b: &Value) -> StateUpdate {
    StateUpdate {
        played: field(b, "Played").and_then(Value::as_bool),
        favorite: field(b, "IsFavorite").and_then(Value::as_bool),
        position_ms: field(b, "PlaybackPositionTicks").and_then(Value::as_i64).map(ids::ms),
        rating: field(b, "Rating").and_then(Value::as_f64).map(|r| r as f32),
        ..Default::default()
    }
}

async fn set_data(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q, body: Bytes) -> Response {
    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match a.check_user(q.get("userid")) {
        Ok(u) => change(&st, u, &id, update_of(&b)).await,
        Err(r) => r,
    }
}

async fn set_data_of(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((u, id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match a.check_user(Some(&u)) {
        Ok(u) => change(&st, u, &id, update_of(&b)).await,
        Err(r) => r,
    }
}

/// No "likes" here: the current data.
async fn rating(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    user_data(&st, a.user(), item).await
}

async fn rating_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    match a.check_user(Some(&u)) {
        Ok(u) => user_data(&st, u, item).await,
        Err(r) => r,
    }
}

/// `Sessions/Playing`: playback began (the resume point stays as it is, as in Jellyfin).
async fn started(State(st): State<Arc<AppState>>, a: JfAuth, body: Bytes) -> Response {
    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    touch(&st, a.user(), &b);
    no_content()
}

/// `Sessions/Playing/Progress`: the position (the played threshold applies as usual).
async fn progress(State(st): State<Arc<AppState>>, a: JfAuth, body: Bytes) -> Response {
    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    touch(&st, a.user(), &b);
    report(&st, a.user(), &b).await;
    no_content()
}

async fn ping(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    if let Some(p) = q.get("playsessionid") {
        super::playback::touch_play(&st, a.user(), p);
    }
    no_content()
}

/// A paused player still holds its transcode.
fn touch(st: &AppState, user: i64, b: &Value) {
    if let Some(p) = field(b, "PlaySessionId").and_then(Value::as_str) {
        super::playback::touch_play(st, user, p);
    }
}

async fn report(st: &AppState, user: i64, b: &Value) {
    let item = field(b, "ItemId").and_then(Value::as_str).and_then(|i| ids::dec_kind(i, ids::ITEM));
    let pos = field(b, "PositionTicks").and_then(Value::as_i64);
    if let (Some(item), Some(pos)) = (item, pos) {
        let _ = st
            .library
            .set_state(user, item, StateUpdate { position_ms: Some(ids::ms(pos).max(0)), ..Default::default() })
            .await;
    }
}

async fn stopped(State(st): State<Arc<AppState>>, a: JfAuth, body: Bytes) -> Response {
    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    report(&st, a.user(), &b).await;
    if let Some(ps) = field(&b, "PlaySessionId").and_then(Value::as_str) {
        super::playback::end_play(&st, a.user(), ps).await;
    }
    no_content()
}

async fn legacy(st: &AppState, user: i64, id: &str, q: &Q) {
    if let (Some(item), Some(pos)) = (ids::dec_kind(id, ids::ITEM), q.i64("positionticks")) {
        let _ = st
            .library
            .set_state(user, item, StateUpdate { position_ms: Some(ids::ms(pos).max(0)), ..Default::default() })
            .await;
    }
}

async fn legacy_progress(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    legacy(&st, a.user(), &id, &q).await;
    no_content()
}

async fn legacy_progress_of(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((u, id)): Path<(String, String)>,
    q: Q,
) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => {
            legacy(&st, u, &id, &q).await;
            no_content()
        }
        Err(r) => r,
    }
}
