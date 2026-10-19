//! Watching together: rooms where play, pause and seek are shared by everyone in them.
//! Commands come over HTTP; the state goes out on the `room` WebSocket topic (members only)
//! with the server's clock, so each player can line itself up.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::event::Event;
use corrent_proto::library::{RoomCommand, RoomState};
use serde::Deserialize;

use super::library::lib_err;
use super::{ApiResult, Auth, err};
use crate::state::AppState;

/// Rooms nobody touched for this long are forgotten.
const IDLE: Duration = Duration::from_secs(6 * 3600);
const MAX_ROOMS: usize = 200;

pub struct Room {
    state: RoomState,
    /// (account, device or session): one account on two devices is in twice.
    members: BTreeSet<(String, i64)>,
    touched: Instant,
}

#[derive(Default)]
pub struct Rooms(std::sync::Mutex<HashMap<String, Room>>);

impl Rooms {
    pub fn is_member(&self, room: &str, user: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(room)
            .is_some_and(|r| r.members.iter().any(|(n, _)| n == user))
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Who is in a room: the account and its device (or browser session).
fn key(auth: &Auth) -> (String, i64) {
    (auth.name().to_string(), auth.0.device_id.unwrap_or(-auth.0.session_id))
}

fn snapshot(r: &Room) -> RoomState {
    let mut names: Vec<String> = r.members.iter().map(|(n, _)| n.clone()).collect();
    names.dedup();
    RoomState { members: names, server_now_ms: now_ms(), ..r.state.clone() }
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/syncplay", post(create))
        .route("/syncplay/{id}", get(state))
        .route("/syncplay/{id}/join", post(join))
        .route("/syncplay/{id}/leave", post(leave))
        .route("/syncplay/{id}/command", post(command))
}

fn publish(st: &AppState, r: &Room) -> RoomState {
    let s = snapshot(r);
    st.hub.publish(Event::SyncPlay { room: s.clone() });
    s
}

#[derive(Deserialize)]
struct NewRoom {
    item_id: i64,
    #[serde(default)]
    position_ms: i64,
    /// Whether the room starts paused (it does unless the creator is already playing).
    #[serde(default = "yes")]
    paused: bool,
}

fn yes() -> bool {
    true
}

async fn create(State(st): State<Arc<AppState>>, auth: Auth, Json(r): Json<NewRoom>) -> ApiResult<RoomState> {
    // Only something this user may watch.
    st.library.item(auth.0.user.id, r.item_id).await.map_err(lib_err)?;
    let mut rooms = st.rooms.0.lock().unwrap_or_else(|e| e.into_inner());
    rooms.retain(|_, room| room.touched.elapsed() < IDLE);
    if rooms.len() >= MAX_ROOMS {
        return Err(err(ErrorCode::TooManyRequests, "too many rooms"));
    }
    let id = corrent_core::util::random_hex(4);
    let room = Room {
        state: RoomState {
            id: id.clone(),
            item_id: r.item_id,
            paused: r.paused,
            position_ms: r.position_ms.max(0),
            at_ms: now_ms(),
            by: auth.name().to_string(),
            ..Default::default()
        },
        members: BTreeSet::from([key(&auth)]),
        touched: Instant::now(),
    };
    let s = snapshot(&room);
    rooms.insert(id, room);
    Ok(Json(s))
}

async fn state(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<RoomState> {
    let rooms = st.rooms.0.lock().unwrap_or_else(|e| e.into_inner());
    let r = rooms
        .get(&id)
        .filter(|r| r.members.iter().any(|(n, _)| n == auth.name()))
        .ok_or_else(|| err(ErrorCode::NotFound, "no such room"))?;
    Ok(Json(snapshot(r)))
}

async fn join(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<RoomState> {
    let item = st.rooms.0.lock().unwrap_or_else(|e| e.into_inner()).get(&id).map(|r| r.state.item_id);
    let item = item.ok_or_else(|| err(ErrorCode::NotFound, "no such room"))?;
    st.library.item(auth.0.user.id, item).await.map_err(lib_err)?;
    let mut rooms = st.rooms.0.lock().unwrap_or_else(|e| e.into_inner());
    rooms.retain(|_, room| room.touched.elapsed() < IDLE);
    let r = rooms.get_mut(&id).ok_or_else(|| err(ErrorCode::NotFound, "no such room"))?;
    if r.members.len() >= 20 && !r.members.contains(&key(&auth)) {
        return Err(err(ErrorCode::Conflict, "the room is full"));
    }
    r.members.insert(key(&auth));
    r.touched = Instant::now();
    Ok(Json(publish(&st, r)))
}

async fn leave(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let mut rooms = st.rooms.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = rooms.get_mut(&id) {
        r.members.remove(&key(&auth));
        if r.members.is_empty() {
            rooms.remove(&id);
        } else {
            publish(&st, r);
        }
    }
    Ok(Json(serde_json::json!({})))
}

async fn command(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<String>,
    Json(c): Json<RoomCommand>,
) -> ApiResult<RoomState> {
    let mut rooms = st.rooms.0.lock().unwrap_or_else(|e| e.into_inner());
    let r = rooms
        .get_mut(&id)
        .filter(|r| r.members.iter().any(|(n, _)| n == auth.name()))
        .ok_or_else(|| err(ErrorCode::NotFound, "no such room"))?;
    let now = now_ms();
    let (paused, pos) = match c {
        RoomCommand::Play { position_ms } => (false, position_ms),
        RoomCommand::Pause { position_ms } => (true, position_ms),
        RoomCommand::Seek { position_ms } => (r.state.paused, position_ms),
    };
    r.state.paused = paused;
    r.state.position_ms = pos.max(0);
    r.state.at_ms = now;
    r.state.by = auth.name().to_string();
    r.touched = Instant::now();
    Ok(Json(publish(&st, r)))
}
