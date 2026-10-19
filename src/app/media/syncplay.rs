//! Watching together from the native player: the same rooms as the web client
//! (`/api/v1/syncplay`). The player sends what the person does and follows the room, which it
//! asks for once a second (positions are extrapolated from when an answer arrived, so the two
//! computers' clocks don't matter).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_proto::library::{RoomCommand, RoomState};

use super::client::MediaClient;

/// Positions this far apart (seconds) are corrected with a seek.
const DRIFT_S: f64 = 1.0;
const POLL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Shared {
    /// The room and when its state arrived here.
    state: Option<(RoomState, Instant)>,
    error: Option<String>,
    polling: bool,
}

pub struct Room {
    shared: Arc<Mutex<Shared>>,
    client: MediaClient,
    rt: tokio::runtime::Handle,
    last_poll: Instant,
    /// When this side last corrected its position (gives the seek time to land).
    last_seek: Instant,
}

/// What the player should do to match the room.
#[derive(Debug, PartialEq)]
pub struct Correction {
    pub seek_to: Option<f64>,
    pub pause: Option<bool>,
}

impl Room {
    fn spawn(
        client: MediaClient,
        rt: tokio::runtime::Handle,
        ctx: egui::Context,
        path: String,
        body: serde_json::Value,
    ) -> Room {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let s = shared.clone();
        let c = client.clone();
        rt.spawn(async move {
            let r = c.send::<RoomState, _>(reqwest::Method::POST, &path, &body).await;
            if let Ok(mut s) = s.lock() {
                match r {
                    Ok(st) => s.state = Some((st, Instant::now())),
                    Err(e) => s.error = Some(e),
                }
            }
            ctx.request_repaint();
        });
        let long_ago = Instant::now() - Duration::from_secs(10);
        Room { shared, client, rt, last_poll: Instant::now(), last_seek: long_ago }
    }

    pub fn create(
        client: MediaClient,
        rt: tokio::runtime::Handle,
        ctx: egui::Context,
        item_id: i64,
        pos: f64,
        paused: bool,
    ) -> Room {
        let body = serde_json::json!({ "item_id": item_id, "position_ms": (pos * 1000.0) as i64, "paused": paused });
        Room::spawn(client, rt, ctx, "/api/v1/syncplay".into(), body)
    }

    pub fn join(client: MediaClient, rt: tokio::runtime::Handle, ctx: egui::Context, code: &str) -> Room {
        let code: String = code.trim().chars().filter(char::is_ascii_alphanumeric).collect();
        Room::spawn(client, rt, ctx, format!("/api/v1/syncplay/{code}/join"), serde_json::json!({}))
    }

    pub fn state(&self) -> Option<RoomState> {
        self.shared.lock().ok()?.state.as_ref().map(|(s, _)| s.clone())
    }

    pub fn error(&self) -> Option<String> {
        self.shared.lock().ok()?.error.clone()
    }

    /// The web client's link to this room.
    pub fn link(&self) -> Option<String> {
        let s = self.state()?;
        Some(self.client.url(&format!("/play/{}?room={}", s.item_id, s.id)))
    }

    /// Asks the server for the room now and then.
    pub fn poll(&mut self, ctx: &egui::Context) {
        ctx.request_repaint_after(POLL);
        if self.last_poll.elapsed() < POLL {
            return;
        }
        let Some(id) = self.state().map(|s| s.id) else { return };
        {
            let Ok(mut s) = self.shared.lock() else { return };
            if s.polling {
                return;
            }
            s.polling = true;
        }
        self.last_poll = Instant::now();
        let (c, shared) = (self.client.clone(), self.shared.clone());
        self.rt.spawn(async move {
            let r = c.get::<RoomState>(&format!("/api/v1/syncplay/{id}")).await;
            if let Ok(mut s) = shared.lock() {
                s.polling = false;
                match r {
                    Ok(st) => {
                        s.state = Some((st, Instant::now()));
                        s.error = None;
                    }
                    Err(e) => s.error = Some(e),
                }
            }
        });
    }

    /// Where the room is now (seconds), and whether it's paused.
    fn target(&self) -> Option<(i64, f64, bool)> {
        let s = self.shared.lock().ok()?;
        let (st, at) = s.state.as_ref()?;
        let since = at.elapsed().as_millis() as i64;
        let pos = st.position_at(st.server_now_ms + since);
        Some((st.item_id, pos as f64 / 1000.0, st.paused))
    }

    /// Compares the player with the room. Nothing while another item plays or mpv buffers.
    pub fn correction(&mut self, item_id: i64, pos: f64, paused: bool, buffering: bool) -> Correction {
        let none = Correction { seek_to: None, pause: None };
        let Some((room_item, target, room_paused)) = self.target() else { return none };
        if room_item != item_id || buffering {
            return none;
        }
        let seek_to =
            ((pos - target).abs() > DRIFT_S && self.last_seek.elapsed() > Duration::from_secs(2)).then(|| {
                self.last_seek = Instant::now();
                target
            });
        Correction { seek_to, pause: (paused != room_paused).then_some(room_paused) }
    }

    /// The person played, paused or seeked here. The local copy changes at once (so the next
    /// correction doesn't undo it before the server answers).
    pub fn send(&mut self, c: RoomCommand) {
        let (paused, pos) = match c {
            RoomCommand::Play { position_ms } => (Some(false), position_ms),
            RoomCommand::Pause { position_ms } => (Some(true), position_ms),
            RoomCommand::Seek { position_ms } => (None, position_ms),
        };
        let Some(id) = (match self.shared.lock() {
            Ok(mut s) => s.state.as_mut().map(|(st, at)| {
                if let Some(p) = paused {
                    st.paused = p;
                }
                st.position_ms = pos;
                st.at_ms = st.server_now_ms;
                *at = Instant::now();
                st.id.clone()
            }),
            Err(_) => None,
        }) else {
            return;
        };
        self.last_seek = Instant::now();
        let (cl, shared) = (self.client.clone(), self.shared.clone());
        self.rt.spawn(async move {
            match cl.send::<RoomState, _>(reqwest::Method::POST, &format!("/api/v1/syncplay/{id}/command"), &c).await {
                Ok(st) => {
                    if let Ok(mut s) = shared.lock() {
                        s.state = Some((st, Instant::now()));
                    }
                }
                Err(e) => tracing::warn!("syncplay: {e}"),
            }
        });
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        let Some(id) = self.state().map(|s| s.id) else { return };
        let c = self.client.clone();
        self.rt.spawn(async move {
            let _ = c
                .send::<serde_json::Value, _>(
                    reqwest::Method::POST,
                    &format!("/api/v1/syncplay/{id}/leave"),
                    &serde_json::json!({}),
                )
                .await;
        });
    }
}
