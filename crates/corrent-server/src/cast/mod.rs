//! Casting to Chromecasts and other Google Cast receivers on the server's network. The server
//! finds them (mDNS `_googlecast._tcp`), talks Cast v2 to them (TLS on port 8009) and has the
//! Default Media Receiver play a signed link on the media listener (`crate::dlna`): the file
//! itself when the device plays it, else an HLS transcode. Clients only pick a device and send
//! play/pause/seek; the device fetches the media from the server directly.

pub mod wire;

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
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
/// The same, when the device never fetched anything: most likely a firewall on this computer.
pub const CANT_REACH: &str = "cant_reach";
/// How long a device may take to start playing what it was told to load.
const LOAD_DEADLINE: Duration = Duration::from_secs(60);
/// How long a device stays ours with nothing on it (for the next item) before it is let go, which
/// also lets the media listener close.
#[cfg(not(test))]
const IDLE_EXIT: Duration = Duration::from_secs(30);
#[cfg(test)]
const IDLE_EXIT: Duration = Duration::from_secs(1);
/// The port receivers listen on.
const DEVICE_PORT: u16 = 8009;
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
    /// The subtitles the device can show itself.
    pub tracks: Vec<SideTrack>,
    /// The one of `tracks` to show.
    pub active: Option<i64>,
    /// `url` is HLS with fMP4 segments: streams copied from the file.
    pub copied: bool,
    /// What to do when the device reached us and can't play this.
    pub fallback: Option<Fallback>,
    /// What changing only the subtitle mustn't change.
    pub loaded: Loaded,
    /// Which request this is for ([`Casts::next_serial`]): a fallback is for this one alone.
    pub serial: u64,
    /// What the media listener served so far (see [`CANT_REACH`]).
    pub hits: Arc<AtomicU64>,
}

/// The choices that are in the media itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    pub item_id: i64,
    pub audio_track: Option<i64>,
    pub file_id: Option<i64>,
    pub burned: bool,
}

type FallbackRun = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Plays the item another way (see [`Media::fallback`]), from the given position in seconds.
#[derive(Clone)]
pub struct Fallback(pub Arc<dyn Fn(f64) -> FallbackRun + Send + Sync>);

impl std::fmt::Debug for Fallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fallback")
    }
}

/// A WebVTT subtitle for the receiver.
#[derive(Clone, Debug)]
pub struct SideTrack {
    /// The file's stream index.
    pub id: i64,
    pub url: String,
    pub name: String,
    pub lang: Option<String>,
}

enum Cmd {
    Load(Box<Media>),
    Control(CastCommand),
    /// Another of the media's own subtitles (or none).
    Tracks(Option<i64>),
    /// The fallback under way couldn't be started.
    Failed(String),
}

struct Session {
    tx: mpsc::Sender<Cmd>,
    status: Arc<Mutex<CastStatus>>,
    /// Told to stop: it won't read anything more, though its channel is still open.
    stopping: bool,
    /// What was last asked of it.
    loaded: Loaded,
    serial: u64,
}

#[derive(Default)]
pub struct Casts {
    found: Mutex<Option<(Instant, Vec<Found>)>>,
    sessions: Mutex<HashMap<String, Session>>,
    /// When links on the media listener were last asked for.
    last_use: Mutex<Option<Instant>>,
    serial: AtomicU64,
}

/// A device typed into the settings: `address`, `address:port` or `name=address[:port]`. Local
/// IPv4 addresses only: this is where the server opens connections.
pub fn manual_device(line: &str) -> Option<Found> {
    let line = line.trim();
    let (name, at) = match line.split_once('=') {
        Some((n, a)) => (n.trim(), a.trim()),
        None => ("", line),
    };
    let addr = match at.parse::<IpAddr>() {
        Ok(ip) => SocketAddr::new(ip, DEVICE_PORT),
        Err(_) => at.parse::<SocketAddr>().ok()?,
    };
    // IPv4 like what the search finds, and like the media listener.
    if !addr.is_ipv4() || !crate::net::is_local(addr.ip(), &[]) {
        return None;
    }
    let id = format!("manual-{}-{}", addr.ip(), addr.port());
    let name = if name.is_empty() { addr.ip().to_string() } else { name.to_string() };
    Some(Found { device: CastDevice { id, name, model: String::new() }, addr })
}

/// What was found, then the typed devices that aren't among them.
fn merged(mut found: Vec<Found>, manual: &[String]) -> Vec<Found> {
    for m in manual.iter().filter_map(|l| manual_device(l)) {
        if !found.iter().any(|f| f.addr.ip() == m.addr.ip()) {
            found.push(m);
        }
    }
    found
}

fn is_live(state: &str) -> bool {
    matches!(state, "connecting" | "loading" | "buffering" | "playing" | "paused")
}

/// A new item on `status`, at once: whoever asks next gets the new item, never the one before.
fn mark_loading(status: &Mutex<CastStatus>, m: &Media) {
    let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
    if s.state != "connecting" {
        s.state = "loading".into();
    }
    s.item_id = Some(m.item_id);
    s.title = m.title.clone();
    s.by = m.by.clone();
    s.position_ms = (m.start_s * 1000.0) as i64;
    s.duration_ms = m.duration_ms;
    s.error = None;
    s.finished = false;
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
    /// The devices on the network (looked for at most once a minute unless `refresh`) and the
    /// typed ones. Typed ones are offered even when the search can't run.
    pub async fn devices(&self, refresh: bool, manual: &[String]) -> anyhow::Result<Vec<Found>> {
        if !refresh
            && let Some((at, list)) = self.found.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && at.elapsed() < FRESH
        {
            return Ok(merged(list.clone(), manual));
        }
        let list = match tokio::task::spawn_blocking(|| browse(Duration::from_secs(3))).await? {
            Ok(list) => list,
            Err(e) if manual.iter().any(|l| manual_device(l).is_some()) => {
                tracing::info!("casting: can't look for devices: {e:#}");
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), list.clone()));
        Ok(merged(list, manual))
    }

    pub async fn device(&self, id: &str, manual: &[String]) -> anyhow::Result<Found> {
        if let Some(f) = self.devices(false, manual).await?.into_iter().find(|f| f.device.id == id) {
            return Ok(f);
        }
        self.devices(true, manual)
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

    /// What is casting right now.
    pub fn active(&self) -> Vec<CastStatus> {
        let s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let mut list: Vec<CastStatus> = s
            .values()
            .filter(|s| !s.tx.is_closed())
            .map(|s| s.status.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .filter(|s| is_live(&s.state))
            .collect();
        list.sort_by_key(|s| s.device.name.to_lowercase());
        list
    }

    /// Links on the media listener were just asked for.
    pub fn touch(&self) {
        *self.last_use.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    /// Whether the media listener may still be needed: a connection is open, or it was asked for
    /// links within `idle` (the cast they are for may not have started yet).
    pub fn in_use(&self, idle: Duration) -> bool {
        let recent = self.last_use.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < idle);
        recent || self.sessions.lock().unwrap_or_else(|e| e.into_inner()).values().any(|s| !s.tx.is_closed())
    }

    /// Plays `media` on `found`, reusing the connection when there is one.
    pub fn play(&self, st: Arc<crate::state::AppState>, found: Found, media: Media) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let id = found.device.id.clone();
        if let Some(s) = sessions.get(&id).filter(|s| !s.stopping)
            && s.tx.try_send(Cmd::Load(Box::new(media.clone()))).is_ok()
        {
            mark_loading(&s.status, &media);
            if let Some(s) = sessions.get_mut(&id) {
                s.loaded = media.loaded.clone();
                s.serial = media.serial;
            }
            return;
        }
        let status = Arc::new(Mutex::new(CastStatus {
            device: found.device.clone(),
            state: "connecting".into(),
            ..Default::default()
        }));
        mark_loading(&status, &media);
        let (loaded, serial) = (media.loaded.clone(), media.serial);
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
        sessions.insert(id, Session { tx, status, stopping: false, loaded, serial });
    }

    /// Shows another of the subtitles the device already has, if `loaded` is what it is playing:
    /// nothing has to load again.
    pub fn switch_subtitle(&self, id: &str, loaded: &Loaded, active: Option<i64>) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = sessions.get(id).filter(|s| !s.stopping && s.loaded == *loaded && !loaded.burned) else {
            return false;
        };
        let state = s.status.lock().unwrap_or_else(|e| e.into_inner()).state.clone();
        matches!(state.as_str(), "playing" | "paused" | "buffering") && s.tx.try_send(Cmd::Tracks(active)).is_ok()
    }

    /// A number for the next play request.
    pub fn next_serial(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Whether the device is still on the request `serial`: not stopped, and nothing newer asked.
    pub fn is_current(&self, id: &str, serial: u64) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(id).is_some_and(|s| !s.stopping && s.serial == serial)
    }

    /// Plays `media` instead of what the request `serial` started, if the device is still on it.
    /// Never opens a connection: someone who stopped the cast meanwhile meant it.
    pub fn replace(&self, id: &str, serial: u64, media: Media) -> bool {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = sessions.get_mut(id).filter(|s| !s.stopping && s.serial == serial) else { return false };
        let loaded = media.loaded.clone();
        let boxed = Box::new(media);
        mark_loading(&s.status, &boxed);
        if s.tx.try_send(Cmd::Load(boxed)).is_err() {
            return false;
        }
        s.loaded = loaded;
        true
    }

    /// The other way of playing what `serial` asked for couldn't be started.
    pub fn fail(&self, id: &str, serial: u64, why: String) {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = sessions.get(id).filter(|s| s.serial == serial) {
            let _ = s.tx.try_send(Cmd::Failed(why));
        }
    }

    pub fn command(&self, id: &str, c: CastCommand) -> bool {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let Some(s) = sessions.get_mut(id) else { return false };
        s.stopping |= c == CastCommand::Stop;
        s.tx.try_send(Cmd::Control(c)).is_ok()
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

/// The media-status fields a sender shows. Returns the media session.
pub fn apply_media_status(s: &mut CastStatus, v: &Value) -> Option<i64> {
    let m = v.get("status")?.as_array()?.first()?;
    let session = m.get("mediaSessionId").and_then(Value::as_i64);
    let idle_reason = (m.get("playerState").and_then(Value::as_str) == Some("IDLE"))
        .then(|| m.get("idleReason").and_then(Value::as_str).unwrap_or(""));
    // The item being replaced says it ended: that isn't about the one on its way.
    if s.state == "loading" && idle_reason.is_some_and(|r| r != "ERROR") {
        return None;
    }
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
            let reason = idle_reason.unwrap_or("");
            s.state = if reason == "ERROR" { "error".into() } else { "idle".into() };
            s.finished = reason == "FINISHED";
            if reason == "ERROR" {
                s.error = Some(CANT_PLAY.into());
            }
        }
        _ => {}
    }
    session
}

/// The media session being replaced saying it ended, while its replacement loads: even a failure
/// there says nothing about the new item. Only that: a device reusing the number for the new item
/// is heard as soon as it plays.
fn about_replaced(v: &Value, replaced: Option<i64>, state: &str) -> bool {
    state == "loading"
        && replaced.is_some()
        && v.pointer("/status/0/mediaSessionId").and_then(Value::as_i64) == replaced
        && v.pointer("/status/0/playerState").and_then(Value::as_str) == Some("IDLE")
}

/// The volume fields of a receiver status.
pub(crate) fn apply_receiver_status(s: &mut CastStatus, v: &Value) {
    if let Some(level) = v.pointer("/status/volume/level").and_then(Value::as_f64) {
        s.volume = Some(level);
    }
    if let Some(muted) = v.pointer("/status/volume/muted").and_then(Value::as_bool) {
        s.muted = Some(muted);
    }
    if let Some(control) = v.pointer("/status/volume/controlType").and_then(Value::as_str) {
        s.volume_fixed = control == "fixed";
    }
}

/// The LOAD message's body for `media`.
fn load_message(media: &Media, request: i64) -> Value {
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
    let mut load = json!({
        "type": "LOAD", "requestId": request, "media": info, "autoplay": true, "currentTime": media.start_s,
    });
    if media.copied {
        load["media"]["hlsSegmentFormat"] = json!("fmp4");
        load["media"]["hlsVideoSegmentFormat"] = json!("fmp4");
    }
    if !media.tracks.is_empty() {
        let tracks: Vec<Value> = media
            .tracks
            .iter()
            .map(|t| {
                json!({
                    "trackId": t.id, "type": "TEXT", "subtype": "SUBTITLES", "trackContentId": t.url,
                    "trackContentType": "text/vtt", "name": t.name,
                    // Required for subtitles, also when the file doesn't say: "undetermined".
                    "language": t.lang.as_deref().unwrap_or("und"),
                })
            })
            .collect();
        load["media"]["tracks"] = json!(tracks);
    }
    if let Some(a) = media.active {
        load["activeTrackIds"] = json!([a]);
    }
    load
}

/// Records how far `m` got, as the players do.
fn report_progress(st: &Arc<crate::state::AppState>, m: &Media, pos: i64, dur: Option<i64>, finished: bool) {
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
    // For its volume.
    w.write_all(&msg("receiver-0", NS_RECEIVER, json!({ "type": "GET_STATUS", "requestId": 1 }))).await?;
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
    // Told to load and not playing yet; what the media listener had served when it was told.
    let mut loading_since: Option<Instant> = None;
    let mut hits_at_load: Option<(Arc<AtomicU64>, u64)> = None;
    let mut last_report = Instant::now();
    // Where the current item really was (a status while loading says where it starts).
    let mut played_at = 0i64;
    // The media session the item now loading replaces.
    let mut replaced: Option<i64> = None;
    // Nothing on the device since then.
    let mut idle_since: Option<Instant> = None;
    // The request that changed the subtitle, in case the device refuses that.
    let mut tracks_reqs: Vec<i64> = Vec::new();
    // The request that loaded what's on the device: only its failure is about that.
    let mut load_req: Option<i64> = None;
    // Another way of playing it is being prepared: what the device says of this one no longer counts.
    let mut falling_back = false;
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
    // Why a load failed: the device reached us and couldn't play it, or never came.
    let failure = |hits: &Option<(Arc<AtomicU64>, u64)>| match hits {
        Some((now, then)) if now.load(Ordering::Relaxed) == *then => CANT_REACH,
        _ => CANT_PLAY,
    };
    // A device that came and can't play this may play it another way: under way, if so.
    let fall_back = |m: &Option<Box<Media>>, why: &str, played_ms: i64| {
        let f = m.as_ref().and_then(|m| m.fallback.clone()).filter(|_| why == CANT_PLAY);
        if let Some(f) = &f {
            tokio::spawn((f.0)(played_ms as f64 / 1000.0));
        }
        f.is_some()
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
                    if loading_since.is_some_and(|t: Instant| t.elapsed() > LOAD_DEADLINE) {
                        loading_since = None;
                        let why = failure(&hits_at_load);
                        falling_back = fall_back(&current, why, played_at);
                        if falling_back {
                            loading_since = Some(Instant::now());
                        } else {
                            let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                            s.state = "error".into();
                            s.error = Some(why.into());
                            idle_since = Some(Instant::now());
                        }
                        stop_hls(&current);
                        current = None;
                    }
                    if idle_since.is_some_and(|t: Instant| t.elapsed() > IDLE_EXIT) && pending.is_none() {
                        if let Some(sid) = &session_id {
                            req += 1;
                            w.write_all(&msg("receiver-0", NS_RECEIVER, json!({ "type": "STOP", "sessionId": sid, "requestId": req }))).await?;
                        }
                        return Ok(());
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
                            status.lock().unwrap_or_else(|e| e.into_inner()).state = "loading".into();
                            if let Some(c) = &current {
                                report_progress(&st, c, played_at, c.duration_ms, false);
                            }
                            stop_hls(&current);
                            current = None;
                            replaced = media_session.take();
                            idle_since = None;
                            falling_back = false;
                            tracks_reqs.clear();
                            // 4: one still waiting for the player is never going to be played.
                            stop_hls(&pending);
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
                        Cmd::Failed(why) => {
                            falling_back = false;
                            loading_since = None;
                            let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                            s.state = "error".into();
                            s.error = Some(why);
                            drop(s);
                            idle_since = Some(Instant::now());
                        }
                        Cmd::Tracks(active) => {
                            let (Some(t), Some(ms), Some(c)) = (transport.clone(), media_session, current.as_mut()) else {
                                continue;
                            };
                            c.active = active;
                            req += 1;
                            tracks_reqs.push(req);
                            let ids: Vec<i64> = active.into_iter().collect();
                            w.write_all(&msg(&t, NS_MEDIA, json!({
                                "type": "EDIT_TRACKS_INFO", "mediaSessionId": ms, "requestId": req, "activeTrackIds": ids,
                            }))).await?;
                        }
                        // The device's, whatever it is playing.
                        Cmd::Control(CastCommand::Volume { level }) => {
                            req += 1;
                            w.write_all(&msg("receiver-0", NS_RECEIVER, json!({
                                "type": "SET_VOLUME", "requestId": req, "volume": { "level": level.clamp(0.0, 1.0) },
                            }))).await?;
                        }
                        Cmd::Control(CastCommand::Mute { muted }) => {
                            req += 1;
                            w.write_all(&msg("receiver-0", NS_RECEIVER, json!({
                                "type": "SET_VOLUME", "requestId": req, "volume": { "muted": muted },
                            }))).await?;
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
                                CastCommand::Volume { .. } | CastCommand::Mute { .. } => continue,
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
                    let request = v.get("requestId").and_then(Value::as_i64);
                    match (m.namespace.as_str(), ty) {
                        (NS_HEARTBEAT, "PING") => {
                            w.write_all(&CastMessage { source: SENDER.into(), destination: m.source.clone(), namespace: NS_HEARTBEAT.into(), payload: r#"{"type":"PONG"}"#.into() }.encode()).await?;
                        }
                        (NS_RECEIVER, "RECEIVER_STATUS") => {
                            apply_receiver_status(&mut status.lock().unwrap_or_else(|e| e.into_inner()), &v);
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
                            let (state, pos, dur, finished) = {
                                let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                                let stale = falling_back || about_replaced(&v, replaced, &s.state);
                                let v = if stale { Value::Null } else { v };
                                if let Some(ms) = apply_media_status(&mut s, &v) {
                                    media_session = Some(ms);
                                }
                                // No media session any more: it ended without saying how (or we
                                // missed that), which right at the end means it played through.
                                let gone = v.get("status").and_then(Value::as_array).is_some_and(Vec::is_empty);
                                if gone && media_session.is_some() && matches!(s.state.as_str(), "playing" | "paused" | "buffering") {
                                    s.finished = s.duration_ms.is_some_and(|d| d > 0 && s.position_ms + 5000 >= d);
                                    s.state = "idle".into();
                                }
                                if s.state == "error" && s.error.as_deref() == Some(CANT_PLAY) {
                                    let why = failure(&hits_at_load);
                                    if fall_back(&current, why, played_at) {
                                        s.state = "loading".into();
                                        s.error = None;
                                        current = None;
                                        replaced = media_session;
                                        falling_back = true;
                                        loading_since = Some(Instant::now());
                                    } else {
                                        s.error = Some(why.into());
                                    }
                                }
                                (s.state.clone(), s.position_ms, s.duration_ms, s.finished)
                            };
                            if matches!(state.as_str(), "playing" | "paused" | "buffering") {
                                loading_since = None;
                                played_at = pos;
                            }
                            // Watch progress, as the players record it (every 10 s, and at the end).
                            if let Some(m) = &current
                                && (finished || (state == "playing" && last_report.elapsed() > Duration::from_secs(10)))
                            {
                                last_report = Instant::now();
                                report_progress(&st, m, pos, dur, finished);
                            }
                            // Over: nothing more will be asked of its transcode, and nothing more
                            // to ask the device. The connection stays for what comes next.
                            if matches!(state.as_str(), "idle" | "error") && (current.is_some() || media_session.is_some()) {
                                if !finished && let Some(m) = &current {
                                    report_progress(&st, m, played_at, dur, false);
                                }
                                stop_hls(&current);
                                current = None;
                                media_session = None;
                                loading_since = None;
                                idle_since = Some(Instant::now());
                            }
                        }
                        // It won't switch subtitles in place: load the same thing again, from here,
                        // with the new one.
                        (NS_MEDIA, "INVALID_REQUEST") if request.is_some_and(|r| tracks_reqs.contains(&r)) => {
                            tracks_reqs.clear();
                            if let Some(mut c) = current.take() {
                                c.start_s = played_at as f64 / 1000.0;
                                status.lock().unwrap_or_else(|e| e.into_inner()).state = "loading".into();
                                replaced = media_session.take();
                                pending = Some(c);
                            }
                        }
                        // About the load on the device: not a refused pause or seek, nor a load since
                        // replaced. Some failures don't say which request they answer.
                        (NS_MEDIA, "LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST")
                            if request == load_req || (ty != "INVALID_REQUEST" && request.unwrap_or(0) == 0) =>
                        {
                            let why = failure(&hits_at_load);
                            falling_back = fall_back(&current, why, played_at);
                            if falling_back {
                                loading_since = Some(Instant::now());
                            } else {
                                loading_since = None;
                                let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                                s.state = "error".into();
                                s.error = Some(why.into());
                                idle_since = Some(Instant::now());
                            }
                            stop_hls(&current);
                            current = None;
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
                w.write_all(&msg(&t, NS_MEDIA, load_message(&media, req))).await?;
                load_req = Some(req);
                loading_since = Some(Instant::now());
                hits_at_load = Some((media.hits.clone(), media.hits.load(Ordering::Relaxed)));
                played_at = (media.start_s * 1000.0) as i64;
                current = Some(media);
            }
        }
    }
    .await;
    if let Some(m) = &current {
        report_progress(&st, m, played_at, m.duration_ms, false);
    }
    stop_hls(&current);
    stop_hls(&pending);
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
    fn only_finished_counts_as_played_through() {
        let idle = |r: &str| json!({ "status": [{ "mediaSessionId": 3, "playerState": "IDLE", "idleReason": r }] });
        let mut s = CastStatus { state: "playing".into(), ..Default::default() };
        apply_media_status(&mut s, &idle("FINISHED"));
        assert_eq!((s.state.as_str(), s.finished), ("idle", true));
        let mut s = CastStatus { state: "playing".into(), ..Default::default() };
        apply_media_status(&mut s, &idle("CANCELLED"));
        assert_eq!((s.state.as_str(), s.finished), ("idle", false));
    }

    #[test]
    fn the_item_being_replaced_ending_says_nothing_about_the_next() {
        let mut s = CastStatus { state: "loading".into(), position_ms: 1000, ..Default::default() };
        let old = json!({ "status": [{ "mediaSessionId": 3, "playerState": "IDLE", "idleReason": "INTERRUPTED", "currentTime": 900 }] });
        assert_eq!(apply_media_status(&mut s, &old), None);
        assert_eq!((s.state.as_str(), s.position_ms, s.finished), ("loading", 1000, false));
        // A failure is about the new one.
        let bad = json!({ "status": [{ "mediaSessionId": 4, "playerState": "IDLE", "idleReason": "ERROR" }] });
        apply_media_status(&mut s, &bad);
        assert_eq!(s.state, "error");
    }

    #[test]
    fn a_late_failure_of_the_replaced_session_isnt_the_new_items() {
        let late = json!({ "status": [{ "mediaSessionId": 3, "playerState": "IDLE", "idleReason": "ERROR" }] });
        assert!(about_replaced(&late, Some(3), "loading"));
        assert!(!about_replaced(&late, Some(4), "loading"), "another session's is news");
        assert!(!about_replaced(&late, Some(3), "playing"), "once under way the number may come back");
        assert!(!about_replaced(&late, None, "loading"));
        let reused = json!({ "status": [{ "mediaSessionId": 3, "playerState": "PLAYING" }] });
        assert!(!about_replaced(&reused, Some(3), "loading"), "the number used again, for the new item");
    }

    fn media(serial: u64) -> Media {
        Media {
            url: String::new(),
            content_type: String::new(),
            title: String::new(),
            subtitle: String::new(),
            image: None,
            start_s: 0.0,
            item_id: 1,
            by: String::new(),
            user_id: 1,
            hls_session: None,
            duration_ms: None,
            tracks: Vec::new(),
            active: None,
            copied: false,
            fallback: None,
            loaded: Loaded::default(),
            serial,
            hits: Arc::default(),
        }
    }

    /// What a fallback may do depends on what happened while it was being prepared.
    #[test]
    fn a_fallback_only_replaces_the_request_it_was_for() {
        let casts = Casts::default();
        // No session: nothing to replace, and no connection opened.
        assert!(!casts.replace("tv", 1, media(1)));
        let (tx, mut rx) = mpsc::channel(4);
        let status = Arc::new(Mutex::new(CastStatus::default()));
        let session = Session { tx, status: status.clone(), stopping: false, loaded: Loaded::default(), serial: 5 };
        casts.sessions.lock().unwrap().insert("tv".into(), session);
        // Something newer was asked for.
        assert!(!casts.is_current("tv", 4));
        assert!(!casts.replace("tv", 4, media(4)));
        assert!(rx.try_recv().is_err());
        assert_eq!(status.lock().unwrap().item_id, None);
        // Still what the device is on.
        assert!(casts.is_current("tv", 5));
        assert!(casts.replace("tv", 5, media(5)));
        assert!(matches!(rx.try_recv(), Ok(Cmd::Load(_))));
        // Stopped meanwhile.
        assert!(casts.command("tv", CastCommand::Stop));
        assert!(!casts.is_current("tv", 5));
        assert!(!casts.replace("tv", 5, media(5)));
        // A failure is reported only for its own request.
        let _ = rx.try_recv();
        casts.fail("tv", 4, "no".into());
        assert!(rx.try_recv().is_err());
        casts.fail("tv", 5, "no".into());
        assert!(matches!(rx.try_recv(), Ok(Cmd::Failed(_))));
    }

    #[test]
    fn typed_devices() {
        let f = manual_device("Bedroom = 192.168.1.20").unwrap();
        assert_eq!((f.device.name.as_str(), f.addr), ("Bedroom", "192.168.1.20:8009".parse().unwrap()));
        let f = manual_device(" 10.0.0.5:9000 ").unwrap();
        assert_eq!((f.device.name.as_str(), f.addr.port()), ("10.0.0.5", 9000));
        assert_eq!(manual_device("10.0.0.5:9000").unwrap().device.id, "manual-10.0.0.5-9000");
        for bad in ["", "tv.local", "8.8.8.8", "Far=8.8.8.8:8009", "192.168.1.20:port", "[fd00::1]:8009", "fd00::1"] {
            assert!(manual_device(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_typed_device_gives_way_to_the_same_one_found() {
        let found = manual_device("Found=192.168.1.20").unwrap();
        let list = merged(vec![found], &["192.168.1.20".into(), "192.168.1.21".into(), "nonsense".into()]);
        let names: Vec<_> = list.iter().map(|f| f.device.name.as_str()).collect();
        assert_eq!(names, ["Found", "192.168.1.21"]);
    }

    #[test]
    fn load_carries_a_subtitle_the_device_shows() {
        let mut m = Media {
            url: "http://h/a.mp4".into(),
            content_type: "video/mp4".into(),
            title: "T".into(),
            subtitle: String::new(),
            image: None,
            start_s: 5.0,
            item_id: 1,
            by: String::new(),
            user_id: 1,
            hls_session: None,
            duration_ms: Some(60_000),
            tracks: Vec::new(),
            active: None,
            copied: false,
            fallback: None,
            loaded: Loaded::default(),
            serial: 0,
            hits: Arc::default(),
        };
        let v = load_message(&m, 7);
        assert_eq!(v["currentTime"], 5.0);
        assert!(v.get("activeTrackIds").is_none() && v["media"].get("tracks").is_none());
        let track = |id, lang: Option<&str>| SideTrack {
            id,
            url: format!("http://h/{id}.vtt"),
            name: "Sub".into(),
            lang: lang.map(str::to_string),
        };
        // All of them go along, so that switching needs no new load; one is showing, or none.
        m.tracks = vec![track(1001, Some("en")), track(1002, None)];
        let v = load_message(&m, 8);
        assert!(v.get("activeTrackIds").is_none());
        m.active = Some(1002);
        let v = load_message(&m, 9);
        assert_eq!(v["activeTrackIds"], json!([1002]));
        let t = &v["media"]["tracks"];
        assert_eq!((&t[0]["language"], &t[1]["language"]), (&json!("en"), &json!("und")));
        assert_eq!(
            (&t[1]["trackContentId"], &t[1]["trackContentType"]),
            (&json!("http://h/1002.vtt"), &json!("text/vtt"))
        );
        // Copied streams are fMP4, which the device has to be told.
        assert!(v["media"].get("hlsVideoSegmentFormat").is_none());
        m.copied = true;
        assert_eq!(load_message(&m, 10)["media"]["hlsVideoSegmentFormat"], "fmp4");
    }

    #[test]
    fn route_to_loopback() {
        assert_eq!(local_ip_towards("127.0.0.1:8009".parse().unwrap()), Some("127.0.0.1".parse().unwrap()));
    }
}
