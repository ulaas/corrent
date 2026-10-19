//! `GET /api/v1/events`: the realtime channel (WebSocket).
//!
//! The client subscribes to topics. `torrents` sends the whole list once (`torrents_full`) and
//! then only changes (`torrents_delta`), at most every `rate_ms` (default 1000, minimum 250);
//! `session` sends the session numbers; `torrent` sends one torrent's files/peers/pieces every
//! second; `log` (admins) sends new log lines. Discrete events (added, completed, removed,
//! notices) always flow. Messages over 32 KiB go out as zlib-compressed binary frames.

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use corrent_core::engine::EngineCommand;
use corrent_core::engine::types::wire;
use corrent_proto::auth::Perms;
use corrent_proto::event::{ClientMessage, Event, Topic};
use corrent_proto::torrent::{self, Labels, Torrent};
use corrent_proto::{ErrorCode, InfoHash};
use serde::Deserialize;

use super::{ApiErr, Auth, err};
use crate::state::AppState;

const COMPRESS_OVER: usize = 32 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct EventsQuery {
    rate_ms: Option<u64>,
}

/// Cookie-authenticated sockets must come from our own pages: browsers send `Origin` on
/// WebSocket handshakes and do not apply SameSite to them reliably.
fn same_origin(headers: &HeaderMap, host: &str) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else { return false };
    url::Url::parse(origin).ok().is_some_and(|u| {
        let o = match u.port() {
            Some(p) => format!("{}:{p}", u.host_str().unwrap_or("")),
            None => u.host_str().unwrap_or("").to_string(),
        };
        o.eq_ignore_ascii_case(host)
    })
}

pub async fn events(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    headers: HeaderMap,
    axum::Extension(host): axum::Extension<super::RequestHost>,
    axum::Extension(source): axum::Extension<super::AuthSource>,
    Query(q): Query<EventsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiErr> {
    // Browsers (cookie sessions) must come from our own pages.
    if source == super::AuthSource::Cookie && !same_origin(&headers, &host.0) {
        return Err(err(ErrorCode::Forbidden, "cross-origin WebSocket refused"));
    }
    let rate = Duration::from_millis(q.rate_ms.unwrap_or(1000).clamp(250, 10_000));
    Ok(ws.max_message_size(64 * 1024).on_upgrade(move |socket| async move {
        if let Err(e) = Client::new(st, auth, rate).run(socket).await {
            tracing::debug!("events connection ended: {e}");
        }
    }))
}

fn encode(e: &Event) -> Message {
    let json = serde_json::to_vec(e).unwrap_or_default();
    if json.len() > COMPRESS_OVER {
        let mut z = flate2::write::ZlibEncoder::new(Vec::with_capacity(json.len() / 4), flate2::Compression::fast());
        if z.write_all(&json).is_ok()
            && let Ok(bytes) = z.finish()
        {
            return Message::Binary(bytes.into());
        }
    }
    Message::Text(String::from_utf8(json).unwrap_or_default().into())
}

struct Client {
    st: Arc<AppState>,
    auth: Auth,
    rate: Duration,
    topics: HashSet<Topic>,
    sent: Option<Vec<Torrent>>,
    labels: Option<Labels>,
    generation: u64,
    last_tick: Instant,
    log_seen: u64,
}

impl Client {
    fn new(st: Arc<AppState>, auth: Auth, rate: Duration) -> Client {
        Client {
            st,
            auth,
            rate,
            topics: HashSet::new(),
            sent: None,
            labels: None,
            generation: u64::MAX,
            last_tick: Instant::now() - Duration::from_secs(60),
            log_seen: 0,
        }
    }

    async fn send(&self, ws: &mut WebSocket, e: &Event) -> anyhow::Result<()> {
        tokio::time::timeout(SEND_TIMEOUT, ws.send(encode(e)))
            .await
            .map_err(|_| anyhow::anyhow!("client too slow"))??;
        Ok(())
    }

    async fn run(mut self, mut ws: WebSocket) -> anyhow::Result<()> {
        let mut snapshots = self.st.hub.snapshots();
        let mut events = self.st.hub.subscribe();
        let mut fallback = tokio::time::interval(self.rate);
        let mut detail_tick = tokio::time::interval(Duration::from_secs(1));
        self.send(&mut ws, &Event::Hello { server_id: self.st.server_id.clone(), user: self.auth.name().to_string() })
            .await?;
        loop {
            tokio::select! {
                msg = ws.recv() => {
                    let Some(msg) = msg else { return Ok(()) };
                    match msg? {
                        Message::Text(t) => self.on_message(&mut ws, &t).await?,
                        Message::Close(_) => return Ok(()),
                        _ => {}
                    }
                }
                changed = snapshots.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    self.tick(&mut ws, false).await?;
                }
                _ = fallback.tick() => self.tick(&mut ws, false).await?,
                ev = events.recv() => match ev {
                    Ok(e @ (Event::LibraryChanged { .. } | Event::ScanStatus { .. })) => {
                        if self.topics.contains(&Topic::Library) {
                            self.send(&mut ws, &e).await?;
                        }
                    }
                    Ok(e) => self.send(&mut ws, &e).await?,
                    // Fell behind on discrete events: they are notifications, skipping is fine.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => tracing::debug!("events client lagged {n}"),
                    Err(_) => return Ok(()),
                },
                _ = detail_tick.tick() => self.details(&mut ws).await?,
            }
        }
    }

    async fn on_message(&mut self, ws: &mut WebSocket, text: &str) -> anyhow::Result<()> {
        let msg: ClientMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                let notice = Event::Notice {
                    severity: corrent_proto::event::Severity::Error,
                    text: format!("bad message: {e}"),
                    code: Some("bad_request".into()),
                };
                return self.send(ws, &notice).await;
            }
        };
        match msg {
            ClientMessage::Ping { id } => self.send(ws, &Event::Pong { id }).await?,
            ClientMessage::Subscribe { topics } => {
                for t in topics {
                    if t == Topic::Log && !self.auth.0.can(Perms::SETTINGS_WRITE) {
                        continue;
                    }
                    if t == Topic::Torrents {
                        // A (re)subscription always starts from the whole list.
                        self.sent = None;
                        self.labels = None;
                    }
                    if let Topic::Torrent(_) = t {
                        // One detail topic at a time.
                        self.topics.retain(|x| !matches!(x, Topic::Torrent(_)));
                    }
                    self.topics.insert(t);
                }
                self.tick(ws, true).await?;
                self.details(ws).await?;
            }
            ClientMessage::Unsubscribe { topics } => {
                for t in topics {
                    if t == Topic::Torrents {
                        self.sent = None;
                    }
                    self.topics.remove(&t);
                }
            }
        }
        Ok(())
    }

    /// Sends what changed since the last tick (rate-limited unless `force`).
    async fn tick(&mut self, ws: &mut WebSocket, force: bool) -> anyhow::Result<()> {
        if !force && self.last_tick.elapsed() < self.rate {
            return Ok(());
        }
        let snap = self.st.engine.snapshot();
        if !force && snap.generation == self.generation && !self.topics.contains(&Topic::Log) {
            return Ok(());
        }
        self.last_tick = Instant::now();
        self.generation = snap.generation;
        if self.topics.contains(&Topic::Torrents) {
            let now: Vec<Torrent> =
                wire::torrents(&snap).into_iter().map(|t| super::redact_torrent(&self.auth.0, t)).collect();
            let labels = wire::labels(&snap);
            match &self.sent {
                None => self.send(ws, &Event::TorrentsFull { torrents: now.clone(), labels: labels.clone() }).await?,
                Some(before) => {
                    let delta = torrent::diff(before, &now);
                    if !delta.is_empty() {
                        self.send(ws, &Event::TorrentsDelta { delta }).await?;
                    }
                    if self.labels.as_ref() != Some(&labels) {
                        self.send(ws, &Event::Labels { labels: labels.clone() }).await?;
                    }
                }
            }
            self.sent = Some(now);
            self.labels = Some(labels);
        }
        if self.topics.contains(&Topic::Session) {
            self.send(ws, &Event::Session { stats: wire::session(&snap) }).await?;
        }
        if self.topics.contains(&Topic::Log) {
            let lines: Vec<Event> = {
                let buf = corrent_core::logbuf::LINES.lock().unwrap_or_else(|e| e.into_inner());
                let new: Vec<&corrent_core::logbuf::LogLine> = buf.iter().filter(|l| l.seq > self.log_seen).collect();
                if let Some(last) = new.last() {
                    self.log_seen = last.seq;
                }
                new.into_iter()
                    .map(|l| Event::Log {
                        level: l.level.to_string(),
                        target: l.target.clone(),
                        message: l.message.clone(),
                        time: l.time,
                    })
                    .collect()
            };
            for l in lines.iter().rev().take(200).rev() {
                self.send(ws, l).await?;
            }
        }
        Ok(())
    }

    async fn details(&mut self, ws: &mut WebSocket) -> anyhow::Result<()> {
        let Some(hash) = self.topics.iter().find_map(|t| match t {
            Topic::Torrent(h) => Some(*h),
            _ => None,
        }) else {
            return Ok(());
        };
        if let Some(d) = detail(&self.st, hash).await {
            self.send(ws, &Event::TorrentDetail { detail: wire::detail(&d) }).await?;
        }
        Ok(())
    }
}

async fn detail(st: &AppState, hash: InfoHash) -> Option<corrent_core::engine::types::TorrentDetail> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.engine.send(EngineCommand::Detail { hash, reply: tx });
    tokio::time::timeout(Duration::from_secs(5), rx).await.ok()?.ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        let mut h = HeaderMap::new();
        let host = "192.168.1.5:8443";
        assert!(!same_origin(&h, host), "no Origin");
        h.insert(header::ORIGIN, "https://192.168.1.5:8443".parse().unwrap());
        assert!(same_origin(&h, host));
        h.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!same_origin(&h, host));
    }

    #[test]
    fn big_messages_are_compressed() {
        let big =
            Event::Notice { severity: corrent_proto::event::Severity::Info, text: "x".repeat(100_000), code: None };
        let Message::Binary(b) = encode(&big) else { panic!("binary frame") };
        assert!(b.len() < 2_000);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&b[..]), &mut out).unwrap();
        assert_eq!(serde_json::from_slice::<Event>(&out).unwrap(), big);
        assert!(matches!(encode(&Event::Pong { id: 1 }), Message::Text(_)));
    }
}
