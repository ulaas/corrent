//! `/socket`: Jellyfin's WebSocket. Apps open it after signing in and expect keep-alives; Corrent
//! sends none of Jellyfin's remote-control messages.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde_json::{Value, json};

use super::JfAuth;
use crate::state::AppState;

/// Idle connections close after this long (Jellyfin's `WebSocketLostTimeout`).
const LOST: Duration = Duration::from_secs(60);

pub(super) async fn socket(State(_st): State<Arc<AppState>>, _a: JfAuth, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(64 * 1024).on_upgrade(run)
}

fn msg(kind: &str, data: Value) -> Message {
    let id = corrent_core::util::random_hex(16);
    Message::Text(json!({ "MessageType": kind, "Data": data, "MessageId": id }).to_string().into())
}

async fn run(mut ws: WebSocket) {
    if ws.send(msg("ForceKeepAlive", json!(LOST.as_secs()))).await.is_err() {
        return;
    }
    let mut last = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if last.elapsed() > LOST {
                    let _ = ws.send(Message::Close(None)).await;
                    return;
                }
                if last.elapsed() > LOST * 3 / 4 && ws.send(msg("ForceKeepAlive", json!(LOST.as_secs()))).await.is_err() {
                    return;
                }
            }
            m = ws.recv() => {
                let Some(Ok(m)) = m else { return };
                last = Instant::now();
                if let Message::Text(t) = m {
                    let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                    if v.get("MessageType").and_then(Value::as_str) == Some("KeepAlive")
                        && ws.send(msg("KeepAlive", Value::Null)).await.is_err()
                    {
                        return;
                    }
                }
            }
        }
    }
}
