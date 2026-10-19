//! Casting to Chromecasts and other Google Cast receivers on the server's network. The server
//! finds them (mDNS `_googlecast._tcp`), talks Cast v2 to them (TLS on port 8009) and has the
//! Default Media Receiver play a signed link on the media listener (`crate::dlna`): the file
//! itself when the device plays it, else an HLS transcode. Clients only pick a device and send
//! play/pause/seek; the device fetches the media from the server directly.

pub mod wire;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_proto::library::{CastCommand, CastDevice, CastStatus};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use self::wire::CastMessage;

const SERVICE: &str = "_googlecast._tcp.local.";
/// The Default Media Receiver.
const APP_ID: &str = "CC1AD845";
const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
const SENDER: &str = "sender-corrent";
/// The error clients translate ("The device couldn't play this").
pub const CANT_PLAY: &str = "cant_play";
/// Devices are looked for again after this long.
const FRESH: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct Found {
    pub device: CastDevice,
    pub addr: SocketAddr,
}

/// What to play.
#[derive(Clone, Debug)]
pub struct Media {
    pub url: String,
    pub content_type: String,
    pub title: String,
    pub subtitle: String,
    pub image: Option<String>,
    pub start_s: f64,
    pub item_id: i64,
    pub by: String,
    /// Whose watch progress the cast records.
    pub user_id: i64,
    /// The HLS session to stop when the cast ends.
    pub hls_session: Option<String>,
    pub duration_ms: Option<i64>,
}

enum Cmd {
    Load(Box<Media>),
    Control(CastCommand),
}

struct Session {
    tx: mpsc::Sender<Cmd>,
    status: Arc<Mutex<CastStatus>>,
}

#[derive(Default)]
pub struct Casts {
    found: Mutex<Option<(Instant, Vec<Found>)>>,
    sessions: Mutex<HashMap<String, Session>>,
}

/// Our address as `peer` sees it (the interface the route to it leaves through).
pub fn local_ip_towards(peer: SocketAddr) -> Option<IpAddr> {
    let any: SocketAddr = if peer.is_ipv4() { "0.0.0.0:0".parse().ok()? } else { "[::]:0".parse().ok()? };
    let s = std::net::UdpSocket::bind(any).ok()?;
    s.connect(peer).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

fn browse(wait: Duration) -> anyhow::Result<Vec<Found>> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE)?;
    let deadline = Instant::now() + wait;
    let mut out: Vec<Found> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(s)) => {
                let p = s.get_properties();
                let get = |k: &str| p.get_property_val_str(k).unwrap_or_default().to_string();
                let id = get("id");
                let Some(ip) = s.get_addresses().iter().map(|a| a.to_ip_addr()).find(|a| a.is_ipv4()) else { continue };
                if id.is_empty() || out.iter().any(|f| f.device.id == id) {
                    continue;
                }
                let name = Some(get("fn"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| s.get_hostname().trim_end_matches('.').to_string());
                out.push(Found {
                    device: CastDevice { id, name, model: get("md") },
                    addr: SocketAddr::new(ip, s.get_port()),
                });
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    out.sort_by_key(|f| f.device.name.to_lowercase());
    Ok(out)
}

impl Casts {
    /// The devices on the network (looked for at most once a minute unless `refresh`).
    pub async fn devices(&self, refresh: bool) -> anyhow::Result<Vec<Found>> {
        if !refresh
            && let Some((at, list)) = self.found.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && at.elapsed() < FRESH
        {
            return Ok(list.clone());
        }
        let list = tokio::task::spawn_blocking(|| browse(Duration::from_secs(3))).await??;
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
        Ok(list)
    }

    pub async fn device(&self, id: &str) -> anyhow::Result<Found> {
        if let Some(f) = self.devices(false).await?.into_iter().find(|f| f.device.id == id) {
            return Ok(f);
        }
        self.devices(true)
            .await?
            .into_iter()
            .find(|f| f.device.id == id)
            .ok_or_else(|| anyhow::anyhow!("no such device"))
    }

    /// A device as if found on the network (tests).
    #[cfg(test)]
    pub fn inject(&self, f: Found) {
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), vec![f]));
    }

    pub fn status(&self, id: &str) -> Option<CastStatus> {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        s.get(id).map(|s| s.status.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Plays `media` on `found`, reusing the connection when there is one.
    pub fn play(&self, st: Arc<crate::state::AppState>, found: Found, media: Media) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let id = found.device.id.clone();
        if let Some(s) = sessions.get(&id)
            && s.tx.try_send(Cmd::Load(Box::new(media.clone()))).is_ok()
        {
            return;
        }
        let status = Arc::new(Mutex::new(CastStatus {
            device: found.device.clone(),
            state: "connecting".into(),
            ..Default::default()
        }));
        let (tx, rx) = mpsc::channel(16);
        let _ = tx.try_send(Cmd::Load(Box::new(media)));
        let s2 = status.clone();
        tokio::spawn(async move {
            let r = connection(found.addr, s2.clone(), rx, st.clone()).await;
            let mut s = s2.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = r {
                tracing::info!("cast to {}: {e:#}", s.device.name);
                s.state = "error".into();
                s.error = Some(format!("{e:#}"));
            } else if s.state != "error" {
                s.state = "idle".into();
            }
        });
        sessions.insert(id, Session { tx, status });
    }

    pub fn command(&self, id: &str, c: CastCommand) -> bool {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        s.get(id).is_some_and(|s| s.tx.try_send(Cmd::Control(c)).is_ok())
    }
}

struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl std::fmt::Debug for NoVerify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NoVerify")
    }
}

/// Cast devices present self-signed certificates (their identity is checked by Google's
/// device auth, which a media sender doesn't need): the channel is encrypted, not authenticated.
impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn msg(destination: &str, namespace: &str, payload: Value) -> Vec<u8> {
    CastMessage {
        source: SENDER.into(),
        destination: destination.into(),
        namespace: namespace.into(),
        payload: payload.to_string(),
    }
    .encode()
}

/// The media-status fields a sender shows.
pub fn apply_media_status(s: &mut CastStatus, v: &Value) -> Option<i64> {
    let m = v.get("status")?.as_array()?.first()?;
    let session = m.get("mediaSessionId").and_then(Value::as_i64);
    if let Some(t) = m.get("currentTime").and_then(Value::as_f64) {
        s.position_ms = (t * 1000.0) as i64;
    }
    if let Some(d) = m.get("media").and_then(|m| m.get("duration")).and_then(Value::as_f64) {
        s.duration_ms = Some((d * 1000.0) as i64);
    }
    match m.get("playerState").and_then(Value::as_str) {
        Some("PLAYING") => s.state = "playing".into(),
        Some("PAUSED") => s.state = "paused".into(),
        Some("BUFFERING") => s.state = "buffering".into(),
        Some("IDLE") => {
            let reason = m.get("idleReason").and_then(Value::as_str).unwrap_or("");
            s.state = if reason == "ERROR" { "error".into() } else { "idle".into() };
            if reason == "ERROR" {
                s.error = Some(CANT_PLAY.into());
            }
        }
        _ => {}
    }
    session
}

async fn connection(
    addr: SocketAddr,
    status: Arc<Mutex<CastStatus>>,
    mut rx: mpsc::Receiver<Cmd>,
    st: Arc<crate::state::AppState>,
) -> anyhow::Result<()> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
        .with_no_client_auth();
    let tcp = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(addr)).await??;
    let name = rustls::pki_types::ServerName::IpAddress(addr.ip().into());
    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_rustls::TlsConnector::from(Arc::new(tls)).connect(name, tcp),
    )
    .await
    .map_err(|_| anyhow::anyhow!("the device didn't finish the TLS handshake"))??;
    let (mut r, mut w) = tokio::io::split(stream);

    w.write_all(&msg("receiver-0", NS_CONNECTION, json!({ "type": "CONNECT" }))).await?;
    let mut req = 1i64;
    let mut transport: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut media_session: Option<i64> = None;
    let mut pending: Option<Box<Media>> = None;
    let mut current: Option<Box<Media>> = None;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    let mut last_seen = Instant::now();
    let mut launched: Option<Instant> = None;
    let mut last_report = Instant::now();
    let (frames_tx, mut frames) = mpsc::channel::<anyhow::Result<CastMessage>>(32);
    // Reading runs on its own so a half-read frame is never dropped by `select!`.
    let reader = tokio::spawn(async move {
        loop {
            let mut len = [0u8; 4];
            if let Err(e) = r.read_exact(&mut len).await {
                let _ = frames_tx.send(Err(e.into())).await;
                return;
            }
            let n = u32::from_be_bytes(len) as usize;
            if n > wire::MAX_FRAME {
                let _ = frames_tx.send(Err(anyhow::anyhow!("frame too large"))).await;
                return;
            }
            let mut body = vec![0u8; n];
            if let Err(e) = r.read_exact(&mut body).await {
                let _ = frames_tx.send(Err(e.into())).await;
                return;
            }
            if let Some(m) = CastMessage::decode(&body)
                && frames_tx.send(Ok(m)).await.is_err()
            {
                return;
            }
        }
    });
    let _reader = AbortOnDrop(reader);
    let stop_hls = |m: &Option<Box<Media>>| {
        if let Some(id) = m.as_ref().and_then(|m| m.hls_session.clone()) {
            let hls = st.hls.clone();
            tokio::spawn(async move { hls.stop(&id).await });
        }
    };

    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    w.write_all(&msg("receiver-0", NS_HEARTBEAT, json!({ "type": "PING" }))).await?;
                    // A paused cast fetches nothing: keep its transcode alive.
                    if let Some(id) = current.as_ref().and_then(|m| m.hls_session.as_deref()) {
                        st.hls.keep_alive(id);
                    }
                    if launched.is_some_and(|t: Instant| t.elapsed() > Duration::from_secs(20)) {
                        anyhow::bail!("the device didn't start the player");
                    }
                    if last_seen.elapsed() > Duration::from_secs(30) {
                        anyhow::bail!("the device stopped answering");
                    }
                }
                _ = poll.tick(), if transport.is_some() && media_session.is_some() => {
                    let t = transport.clone().unwrap_or_default();
                    req += 1;
                    w.write_all(&msg(&t, NS_MEDIA, json!({ "type": "GET_STATUS", "requestId": req }))).await?;
                }
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { return Ok(()) };
                    match cmd {
                        Cmd::Load(m) => {
                            {
                                let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                                s.state = "loading".into();
                                s.item_id = Some(m.item_id);
                                s.title = m.title.clone();
                                s.by = m.by.clone();
                                s.position_ms = (m.start_s * 1000.0) as i64;
                                s.duration_ms = m.duration_ms;
                                s.error = None;
                            }
                            stop_hls(&current);
                            current = None;
                            media_session = None;
                            match &transport {
                                Some(_) => pending = Some(m),
                                None => {
                                    pending = Some(m);
                                    req += 1;
                                    w.write_all(&msg("receiver-0", NS_RECEIVER, json!({ "type": "LAUNCH", "appId": APP_ID, "requestId": req }))).await?;
                                    launched = Some(Instant::now());
                                }
                            }
                        }
                        Cmd::Control(c) => {
                            let (Some(t), Some(ms)) = (transport.clone(), media_session) else {
                                if c == CastCommand::Stop { return Ok(()) }
                                continue;
                            };
                            req += 1;
                            let p = match c {
                                CastCommand::Play => json!({ "type": "PLAY", "mediaSessionId": ms, "requestId": req }),
                                CastCommand::Pause => json!({ "type": "PAUSE", "mediaSessionId": ms, "requestId": req }),
                                CastCommand::Seek { position_ms } => json!({
                                    "type": "SEEK", "mediaSessionId": ms, "requestId": req, "currentTime": position_ms.max(0) as f64 / 1000.0,
                                }),
                                CastCommand::Volume { level } => {
                                    w.write_all(&msg("receiver-0", NS_RECEIVER, json!({
                                        "type": "SET_VOLUME", "requestId": req, "volume": { "level": level.clamp(0.0, 1.0) },
                                    }))).await?;
                                    continue;
                                }
                                CastCommand::Stop => {
                                    if let Some(sid) = &session_id {
                                        w.write_all(&msg("receiver-0", NS_RECEIVER, json!({ "type": "STOP", "sessionId": sid, "requestId": req }))).await?;
                                    }
                                    return Ok(());
                                }
                            };
                            w.write_all(&msg(&t, NS_MEDIA, p)).await?;
                        }
                    }
                }
                frame = frames.recv() => {
                    let m = frame.ok_or_else(|| anyhow::anyhow!("connection closed"))??;
                    last_seen = Instant::now();
                    let v: Value = serde_json::from_str(&m.payload).unwrap_or(Value::Null);
                    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
                    match (m.namespace.as_str(), ty) {
                        (NS_HEARTBEAT, "PING") => {
                            w.write_all(&CastMessage { source: SENDER.into(), destination: m.source.clone(), namespace: NS_HEARTBEAT.into(), payload: r#"{"type":"PONG"}"#.into() }.encode()).await?;
                        }
                        (NS_RECEIVER, "RECEIVER_STATUS") => {
                            let app = v.pointer("/status/applications").and_then(Value::as_array).and_then(|a| {
                                a.iter().find(|x| x.get("appId").and_then(Value::as_str) == Some(APP_ID))
                            });
                            match app {
                                Some(app) => {
                                    let t = app.get("transportId").and_then(Value::as_str).unwrap_or("").to_string();
                                    session_id = app.get("sessionId").and_then(Value::as_str).map(str::to_string);
                                    if transport.as_deref() != Some(t.as_str()) && !t.is_empty() {
                                        w.write_all(&msg(&t, NS_CONNECTION, json!({ "type": "CONNECT" }))).await?;
                                        transport = Some(t);
                                        launched = None;
                                    }
                                }
                                // Another app took over the device (or ours was closed).
                                None if transport.is_some() => return Ok(()),
                                None => {}
                            }
                        }
                        (NS_RECEIVER, "LAUNCH_ERROR") => anyhow::bail!("the device refused to start the player"),
                        (NS_MEDIA, "MEDIA_STATUS") => {
                            let (state, pos, dur) = {
                                let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(ms) = apply_media_status(&mut s, &v) {
                                    media_session = Some(ms);
                                }
                                (s.state.clone(), s.position_ms, s.duration_ms)
                            };
                            // Watch progress, as the players record it (every 10 s, and at the end).
                            if let Some(m) = &current {
                                let finished = state == "idle"
                                    && v.pointer("/status/0/idleReason").and_then(Value::as_str) == Some("FINISHED");
                                if finished || (state == "playing" && last_report.elapsed() > Duration::from_secs(10)) {
                                    last_report = Instant::now();
                                    let near_end = dur.is_some_and(|d| d > 0 && pos * 10 > d * 9);
                                    let u = if finished || near_end {
                                        corrent_proto::library::StateUpdate { played: Some(true), ..Default::default() }
                                    } else {
                                        corrent_proto::library::StateUpdate { position_ms: Some(pos), ..Default::default() }
                                    };
                                    let (lib, user, item) = (st.library.clone(), m.user_id, m.item_id);
                                    tokio::spawn(async move {
                                        let _ = lib.set_state(user, item, u).await;
                                    });
                                }
                            }
                        }
                        (NS_MEDIA, "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST") => {
                            let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                            s.state = "error".into();
                            s.error = Some(CANT_PLAY.into());
                        }
                        (NS_CONNECTION, "CLOSE") if Some(&m.source) == transport.as_ref() => return Ok(()),
                        _ => {}
                    }
                }
            }
            // The player is ready: load what is waiting.
            if let Some(t) = transport.clone()
                && let Some(media) = pending.take()
            {
                req += 1;
                let mut meta = json!({ "metadataType": 0, "title": media.title, "subtitle": media.subtitle });
                if let Some(img) = &media.image {
                    meta["images"] = json!([{ "url": img }]);
                }
                let mut info = json!({
                    "contentId": media.url, "contentType": media.content_type, "streamType": "BUFFERED", "metadata": meta,
                });
                if let Some(d) = media.duration_ms {
                    info["duration"] = json!(d as f64 / 1000.0);
                }
                w.write_all(&msg(&t, NS_MEDIA, json!({
                    "type": "LOAD", "requestId": req, "media": info, "autoplay": true, "currentTime": media.start_s,
                }))).await?;
                current = Some(media);
            }
        }
    }
    .await;
    stop_hls(&current);
    result
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_status() {
        let mut s = CastStatus::default();
        let v: Value = serde_json::from_str(
            r#"{"type":"MEDIA_STATUS","status":[{"mediaSessionId":3,"playerState":"PLAYING","currentTime":12.5,"media":{"duration":60}}]}"#,
        )
        .unwrap();
        assert_eq!(apply_media_status(&mut s, &v), Some(3));
        assert_eq!((s.state.as_str(), s.position_ms, s.duration_ms), ("playing", 12500, Some(60000)));
        let v: Value =
            serde_json::from_str(r#"{"status":[{"mediaSessionId":3,"playerState":"IDLE","idleReason":"ERROR"}]}"#)
                .unwrap();
        apply_media_status(&mut s, &v);
        assert_eq!(s.state, "error");
        assert_eq!(apply_media_status(&mut s, &json!({ "status": [] })), None);
    }

    #[test]
    fn route_to_loopback() {
        assert_eq!(local_ip_towards("127.0.0.1:8009".parse().unwrap()), Some("127.0.0.1".parse().unwrap()));
    }
}
