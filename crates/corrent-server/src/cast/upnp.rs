//! Playing to DLNA / UPnP-AV renderers: smart TVs (Samsung, LG, Sony), receivers and players
//! such as Kodi. The server finds them (SSDP search for `MediaRenderer`), reads their
//! description, and drives their AVTransport and RenderingControl services with SOAP: set the
//! address of a signed link on the media listener, play, pause, seek, volume. Renderers don't
//! tell: their state is asked for every second.
//!
//! They play files as they are (no HLS), so only what [`ClientProfile::dlna_renderer`] takes is
//! offered; everything else is refused with a reason (`v1/cast.rs`).
//!
//! Every address used here was announced by the device itself: it must be a plain-HTTP local
//! IPv4 address, and a description may only name control addresses on the device it came from.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_proto::library::{CastCommand, CastDevice, CastStatus};
use tokio::sync::mpsc;

use super::{CANT_PLAY, CANT_REACH, Cmd, Found, IDLE_EXIT, LOAD_DEADLINE, Media, report_progress};
use crate::dlna::didl::{esc, tag};

pub const KIND: &str = "dlna";
const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
const AV_TRANSPORT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const RENDERING: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
/// Descriptions and SOAP answers are small; anything larger isn't one.
const MAX_BODY: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
/// A renderer that fails this many requests in a row is gone.
const MAX_FAILURES: u32 = 8;
/// After Play, a renderer that says "stopped" for this long couldn't play it.
const START_GRACE: Duration = Duration::from_secs(12);

/// A renderer's control addresses, from its description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renderer {
    pub udn: String,
    pub name: String,
    pub model: String,
    pub av: String,
    /// Volume and mute; some renderers have none.
    pub rc: Option<String>,
}

#[derive(Debug)]
pub struct Fault {
    /// The UPnP error code, when the renderer answered with one.
    pub code: Option<u32>,
    pub message: String,
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "{} (UPnP error {c})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Fault {}

fn fault(message: impl Into<String>) -> Fault {
    Fault { code: None, message: message.into() }
}

/// `url` when it is plain HTTP to a local IPv4 address (and to `only`, when given): the only
/// kind of address the server opens for a renderer.
pub fn local_url(url: &str, only: Option<IpAddr>) -> Option<(url::Url, SocketAddr)> {
    let u = url::Url::parse(url).ok()?;
    if u.scheme() != "http" || !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    let ip: Ipv4Addr = match u.host()? {
        url::Host::Ipv4(ip) => ip,
        _ => return None,
    };
    let ip = IpAddr::V4(ip);
    if !crate::net::is_local(ip, &[]) || only.is_some_and(|o| o != ip) {
        return None;
    }
    let addr = SocketAddr::new(ip, u.port_or_known_default()?);
    Some((u, addr))
}

fn http() -> Result<reqwest::Client, Fault> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| fault(e.to_string()))
}

/// Reads a renderer's description at `location` (an address it announced or the user typed).
pub async fn describe(location: &str) -> Result<(Renderer, SocketAddr), Fault> {
    let (u, addr) = local_url(location, None).ok_or_else(|| fault("not a device on this network"))?;
    let res = http()?.get(u.clone()).send().await.map_err(|e| fault(e.to_string()))?;
    if !res.status().is_success() {
        return Err(fault(format!("the device answered {}", res.status())));
    }
    let body = crate::net::read_capped(res, MAX_BODY).await.map_err(fault)?;
    let xml = String::from_utf8_lossy(&body);
    parse_description(&xml, &u).map(|r| (r, addr)).ok_or_else(|| fault("not a media renderer"))
}

/// The renderer in a device description fetched from `from`. Control addresses elsewhere than
/// on the device itself are not taken.
pub fn parse_description(xml: &str, from: &url::Url) -> Option<Renderer> {
    if !xml.contains(RENDERER) {
        return None;
    }
    let host = match from.host()? {
        url::Host::Ipv4(ip) => IpAddr::V4(ip),
        _ => return None,
    };
    let base = tag(xml, "URLBase").and_then(|b| url::Url::parse(b.trim()).ok()).unwrap_or_else(|| from.clone());
    let control = |service: &str| {
        xml.split("<service>").skip(1).find_map(|block| {
            let block = block.split("</service>").next()?;
            if tag(block, "serviceType")?.trim() != service {
                return None;
            }
            let joined = base.join(tag(block, "controlURL")?.trim()).ok()?;
            local_url(joined.as_str(), Some(host)).map(|(u, _)| u.to_string())
        })
    };
    let text = |name: &str| tag(xml, name).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let udn = text("UDN")?;
    Some(Renderer {
        name: text("friendlyName").unwrap_or_else(|| host.to_string()),
        model: text("modelName").unwrap_or_default(),
        av: control(AV_TRANSPORT)?,
        rc: control(RENDERING),
        udn,
    })
}

/// The renderer described at `location`, as a device to play on.
pub async fn device(location: &str) -> Result<Found, Fault> {
    describe(location).await.map(|(r, addr)| found(r, addr))
}

fn found(r: Renderer, addr: SocketAddr) -> Found {
    let id = format!("dlna-{}", r.udn.trim_start_matches("uuid:"));
    // Ids go into URLs: keep them to what a UDN is made of.
    let id: String =
        id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(80).collect();
    Found {
        device: CastDevice { id, name: r.name.clone(), model: r.model.clone(), kind: KIND.into() },
        addr,
        dlna: Some(Arc::new(r)),
    }
}

/// The header `name` of an SSDP answer.
fn header<'a>(answer: &'a str, name: &str) -> Option<&'a str> {
    answer.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// The description addresses of the renderers that answer a search sent from `ip` within `wait`.
async fn answers(ip: Ipv4Addr, wait: Duration) -> Vec<String> {
    let sock = match tokio::net::UdpSocket::bind((ip, 0)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::info!("casting: can't look for DLNA renderers from {ip}: {e}");
            return Vec::new();
        }
    };
    let ask = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {RENDERER}\r\n\r\n"
    );
    // Twice: UDP may lose one.
    for _ in 0..2 {
        let _ = sock.send_to(ask.as_bytes(), (Ipv4Addr::new(239, 255, 255, 250), 1900)).await;
    }
    let deadline = tokio::time::Instant::now() + wait;
    let mut buf = vec![0u8; 2048];
    let mut out: Vec<String> = Vec::new();
    while let Ok(Ok((n, from))) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await {
        let answer = String::from_utf8_lossy(buf.get(..n).unwrap_or_default()).into_owned();
        let Some(location) = header(&answer, "location") else { continue };
        // A device speaks for itself only: its description is at its own address.
        if local_url(location, Some(from.ip())).is_some() && !out.iter().any(|l| l == location) {
            out.push(location.to_string());
        }
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// Renderers that answer a search within `wait`, and the typed ones (`extra`: description
/// addresses).
pub async fn search(wait: Duration, extra: &[String]) -> Vec<Found> {
    let mut locations: Vec<String> = extra.to_vec();
    // From every network this computer is on: the default route alone misses a TV on another
    // interface (a second card, a VPN up at the same time).
    let mut from: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && crate::net::is_local(IpAddr::V4(ip), &[]) => Some(ip),
            _ => None,
        })
        .collect();
    if from.is_empty() {
        from.push(Ipv4Addr::UNSPECIFIED);
    }
    let mut asking = tokio::task::JoinSet::new();
    for ip in from {
        asking.spawn(answers(ip, wait));
    }
    while let Some(found) = asking.join_next().await {
        for location in found.unwrap_or_default() {
            if !locations.contains(&location) && locations.len() < 64 {
                locations.push(location);
            }
        }
    }
    // All at once: a device that doesn't answer mustn't hold up the others.
    let mut asking = tokio::task::JoinSet::new();
    for location in locations {
        asking.spawn(async move {
            let r = describe(&location).await;
            (location, r)
        });
    }
    let mut out: Vec<Found> = Vec::new();
    while let Some(Ok((location, r))) = asking.join_next().await {
        match r {
            Ok((r, addr)) => {
                let f = found(r, addr);
                if !out.iter().any(|o| o.device.id == f.device.id) {
                    out.push(f);
                }
            }
            Err(e) => tracing::debug!("casting: {location}: {e}"),
        }
    }
    out.sort_by_key(|f| f.device.name.to_lowercase());
    out
}

/// One SOAP action; the answer's body.
pub async fn soap(control: &str, service: &str, action: &str, args: &[(&str, &str)]) -> Result<String, Fault> {
    let (u, _) = local_url(control, None).ok_or_else(|| fault("not a device on this network"))?;
    let mut body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><u:{action} xmlns:u=\"{service}\">"
    );
    for (k, v) in args {
        body.push_str(&format!("<{k}>{}</{k}>", esc(v)));
    }
    body.push_str(&format!("</u:{action}></s:Body></s:Envelope>"));
    let res = http()?
        .post(u)
        .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", format!("\"{service}#{action}\""))
        .body(body)
        .send()
        .await
        .map_err(|e| fault(e.to_string()))?;
    let ok = res.status().is_success();
    let status = res.status();
    let text = String::from_utf8_lossy(&crate::net::read_capped(res, MAX_BODY).await.map_err(fault)?).into_owned();
    if ok {
        return Ok(text);
    }
    Err(Fault {
        code: tag(&text, "errorCode").and_then(|c| c.trim().parse().ok()),
        message: tag(&text, "errorDescription")
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| format!("{action}: the device answered {status}")),
    })
}

/// `H:MM:SS` as renderers take positions.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Milliseconds of `H:MM:SS[.fff]` (renderers also say `NOT_IMPLEMENTED`: `None`).
pub fn parse_clock(t: &str) -> Option<i64> {
    let mut parts = t.trim().split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    let s: f64 = s.parse().ok()?;
    (h >= 0 && (0..60).contains(&m) && (0.0..60.0).contains(&s))
        .then(|| ((h * 3600 + m * 60) * 1000) + (s * 1000.0) as i64)
}

/// The DLNA flags of something played from our listener: seekable by bytes (files), or not at
/// all (a stream made as it goes).
pub fn features(seekable: bool) -> &'static str {
    if seekable {
        "DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000"
    } else {
        "DLNA.ORG_OP=00;DLNA.ORG_CI=1;DLNA.ORG_FLAGS=01700000000000000000000000000000"
    }
}

/// The DIDL-Lite description sent along with the address: TVs refuse an address without one.
pub fn metadata(m: &Media) -> String {
    let class = if m.content_type.starts_with("audio/") {
        "object.item.audioItem.musicTrack"
    } else if m.content_type.starts_with("image/") {
        "object.item.imageItem.photo"
    } else {
        "object.item.videoItem"
    };
    let duration = m.duration_ms.filter(|d| *d > 0).map(|d| format!(" duration=\"{}.000\"", clock(d as f64 / 1000.0)));
    let art = m.image.as_ref().map(|i| format!("<upnp:albumArtURI>{}</upnp:albumArtURI>", esc(i))).unwrap_or_default();
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\" xmlns:dlna=\"urn:schemas-dlna-org:metadata-1-0/\">\
         <item id=\"0\" parentID=\"-1\" restricted=\"1\"><dc:title>{}</dc:title><upnp:class>{class}</upnp:class>{art}\
         <res protocolInfo=\"http-get:*:{}:{}\"{}>{}</res></item></DIDL-Lite>",
        esc(&m.title),
        esc(&m.content_type),
        features(m.hls_session.is_none()),
        duration.unwrap_or_default(),
        esc(&m.url),
    )
}

struct Ctl<'a> {
    r: &'a Renderer,
}

impl Ctl<'_> {
    async fn av(&self, action: &str, args: &[(&str, &str)]) -> Result<String, Fault> {
        let mut all = vec![("InstanceID", "0")];
        all.extend_from_slice(args);
        soap(&self.r.av, AV_TRANSPORT, action, &all).await
    }

    async fn rc(&self, action: &str, args: &[(&str, &str)]) -> Result<String, Fault> {
        let rc = self.r.rc.as_deref().ok_or_else(|| fault("the device has no volume control"))?;
        let mut all = vec![("InstanceID", "0"), ("Channel", "Master")];
        all.extend_from_slice(args);
        soap(rc, RENDERING, action, &all).await
    }

    /// Stops what is on, gives the renderer the address (`url`: `m`'s, or a later start of its
    /// stream) and starts it.
    async fn load(&self, m: &Media, url: &str) -> Result<(), Fault> {
        // Renderers refuse a new address while one plays; one with nothing on may refuse Stop.
        let _ = self.av("Stop", &[]).await;
        let m = Media { url: url.to_string(), ..m.clone() };
        self.av("SetAVTransportURI", &[("CurrentURI", &m.url), ("CurrentURIMetaData", &metadata(&m))]).await?;
        self.av("Play", &[("Speed", "1")]).await.map(|_| ())
    }
}

/// The actor of one renderer: like [`super::connection`], for a device that is asked instead of
/// telling.
pub(super) async fn session(
    r: Arc<Renderer>,
    status: Arc<Mutex<CastStatus>>,
    mut rx: mpsc::Receiver<Cmd>,
    st: Arc<crate::state::AppState>,
) -> anyhow::Result<()> {
    let ctl = Ctl { r: &r };
    let set = |f: &mut dyn FnMut(&mut CastStatus)| f(&mut status.lock().unwrap_or_else(|e| e.into_inner()));
    set(&mut |s| s.volume_fixed = r.rc.is_none());
    let stop_hls = |m: &Option<Box<Media>>| {
        if let Some(id) = m.as_ref().and_then(|m| m.hls_session.clone()) {
            let hls = st.hls.clone();
            tokio::spawn(async move { hls.stop(&id).await });
        }
    };
    let mut current: Option<Box<Media>> = None;
    // Told to play and not playing yet; what the media listener had served when it was told.
    let mut loading_since: Option<Instant> = None;
    let mut hits_at_load: Option<(Arc<AtomicU64>, u64)> = None;
    // The renderer has played the current item (so "stopped" means it ended).
    let mut played = false;
    // Where to go once it plays (renderers refuse a seek before that).
    let mut seek_to: Option<f64> = None;
    let mut played_at = 0i64;
    // A converted stream is made as it goes: the renderer can't seek in it. It is started again
    // at the segment wanted, and the renderer's clock then counts from there.
    let segment_ms = i64::from(st.hls.config().segment_secs.max(1)) * 1000;
    let mut offset_ms = 0i64;
    let from_segment = |m: &Media, at_ms: i64| {
        let n = at_ms.max(0) / segment_ms;
        (format!("{}?from={n}", m.url), n * segment_ms)
    };
    let mut last_report = Instant::now();
    let mut idle_since: Option<Instant> = None;
    let mut failures = 0u32;
    let mut ticks = 0u64;
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                match cmd {
                    Cmd::Load(m) => {
                        if let Some(c) = &current {
                            report_progress(&st, c, played_at, c.duration_ms, false);
                        }
                        stop_hls(&current);
                        set(&mut |s| s.state = "loading".into());
                        idle_since = None;
                        played = false;
                        played_at = (m.start_s * 1000.0) as i64;
                        let url = if m.hls_session.is_some() {
                            seek_to = None;
                            let (url, offset) = from_segment(&m, played_at);
                            offset_ms = offset;
                            url
                        } else {
                            seek_to = (m.start_s > 1.0).then_some(m.start_s);
                            offset_ms = 0;
                            m.url.clone()
                        };
                        hits_at_load = Some((m.hits.clone(), m.hits.load(Ordering::Relaxed)));
                        match ctl.load(&m, &url).await {
                            Ok(()) => {
                                loading_since = Some(Instant::now());
                                current = Some(m);
                            }
                            Err(e) => {
                                tracing::info!("cast to {}: {e}", r.name);
                                stop_hls(&Some(m));
                                current = None;
                                loading_since = None;
                                // 714 illegal MIME type, 716 resource not found: it won't play this.
                                let why = if e.code.is_some() { CANT_PLAY.to_string() } else { e.to_string() };
                                set(&mut |s| {
                                    s.state = "error".into();
                                    s.error = Some(why.clone());
                                });
                                idle_since = Some(Instant::now());
                            }
                        }
                    }
                    Cmd::Failed(why) => {
                        loading_since = None;
                        set(&mut |s| {
                            s.state = "error".into();
                            s.error = Some(why.clone());
                        });
                        idle_since = Some(Instant::now());
                    }
                    // A renderer shows no subtitles of ours.
                    Cmd::Tracks(_) => {}
                    Cmd::Control(c) => {
                        let done = match c {
                            CastCommand::Play => ctl.av("Play", &[("Speed", "1")]).await,
                            CastCommand::Pause => ctl.av("Pause", &[]).await,
                            CastCommand::Seek { position_ms } => match current.as_deref() {
                                // A converted stream starts again where it is wanted.
                                Some(m) if m.hls_session.is_some() => {
                                    let (url, offset) = from_segment(m, position_ms);
                                    offset_ms = offset;
                                    played_at = offset;
                                    played = false;
                                    loading_since = Some(Instant::now());
                                    hits_at_load = Some((m.hits.clone(), m.hits.load(Ordering::Relaxed)));
                                    set(&mut |s| {
                                        s.state = "buffering".into();
                                        s.position_ms = offset;
                                    });
                                    ctl.load(m, &url).await.map(|_| String::new())
                                }
                                _ => {
                                    let at = clock(position_ms.max(0) as f64 / 1000.0);
                                    ctl.av("Seek", &[("Unit", "REL_TIME"), ("Target", &at)]).await
                                }
                            },
                            CastCommand::Volume { level } => {
                                let v = ((level.clamp(0.0, 1.0) * 100.0).round() as i64).to_string();
                                ctl.rc("SetVolume", &[("DesiredVolume", &v)]).await
                            }
                            CastCommand::Mute { muted } => {
                                ctl.rc("SetMute", &[("DesiredMute", if muted { "1" } else { "0" })]).await
                            }
                            CastCommand::Stop => {
                                let _ = ctl.av("Stop", &[]).await;
                                if let Some(c) = &current {
                                    report_progress(&st, c, played_at, c.duration_ms, false);
                                }
                                stop_hls(&current);
                                return Ok(());
                            }
                        };
                        if let Err(e) = done {
                            tracing::debug!("cast to {}: {e}", r.name);
                        }
                    }
                }
            }
            _ = poll.tick() => {
                ticks += 1;
                if idle_since.is_some_and(|t| t.elapsed() > IDLE_EXIT) {
                    return Ok(());
                }
                let Some(m) = current.as_ref() else { continue };
                if ticks.is_multiple_of(5) {
                    // A paused renderer fetches nothing: keep its transcode alive.
                    if let Some(id) = m.hls_session.as_deref() {
                        st.hls.keep_alive(id);
                    }
                    if r.rc.is_some() {
                        let volume = ctl.rc("GetVolume", &[]).await.ok().and_then(|a| tag(&a, "CurrentVolume"));
                        let muted = ctl.rc("GetMute", &[]).await.ok().and_then(|a| tag(&a, "CurrentMute"));
                        set(&mut |s| {
                            if let Some(v) = volume.as_ref().and_then(|v| v.trim().parse::<f64>().ok()) {
                                s.volume = Some((v / 100.0).clamp(0.0, 1.0));
                            }
                            if let Some(m) = &muted {
                                s.muted = Some(matches!(m.trim(), "1" | "true" | "True"));
                            }
                        });
                    }
                }
                let state = match ctl.av("GetTransportInfo", &[]).await {
                    Ok(a) => {
                        failures = 0;
                        tag(&a, "CurrentTransportState").unwrap_or_default().trim().to_string()
                    }
                    Err(e) => {
                        failures += 1;
                        if failures >= MAX_FAILURES {
                            report_progress(&st, m, played_at, m.duration_ms, false);
                            stop_hls(&current);
                            anyhow::bail!("the device stopped answering ({e})");
                        }
                        continue;
                    }
                };
                let position = match ctl.av("GetPositionInfo", &[]).await {
                    Ok(a) => (tag(&a, "RelTime").and_then(|t| parse_clock(&t)), tag(&a, "TrackDuration").and_then(|t| parse_clock(&t))),
                    Err(_) => (None, None),
                };
                match state.as_str() {
                    "PLAYING" | "PAUSED_PLAYBACK" => {
                        loading_since = None;
                        // Where it was asked to start, once (it starts at the beginning).
                        if let Some(at) = seek_to.take().filter(|_| state == "PLAYING") {
                            let _ = ctl.av("Seek", &[("Unit", "REL_TIME"), ("Target", &clock(at))]).await;
                            played = true;
                            continue;
                        }
                        played = true;
                        if let Some(p) = position.0 {
                            played_at = p + offset_ms;
                        }
                        // The length of a stream still being made is the file's, not what the
                        // renderer has seen of it.
                        let length = position.1.filter(|d| *d > 0 && m.hls_session.is_none());
                        set(&mut |s| {
                            s.state = if state == "PLAYING" { "playing".into() } else { "paused".into() };
                            s.position_ms = played_at;
                            if let Some(d) = length {
                                s.duration_ms = Some(d);
                            }
                            s.error = None;
                        });
                        if state == "PLAYING" && last_report.elapsed() > Duration::from_secs(10) {
                            last_report = Instant::now();
                            report_progress(&st, m, played_at, m.duration_ms, false);
                        }
                    }
                    "TRANSITIONING" => {
                        if played {
                            set(&mut |s| s.state = "buffering".into());
                        }
                    }
                    // STOPPED, NO_MEDIA_PRESENT: over when it had played, a failure when it never
                    // did (after a moment: renderers say "stopped" while they fetch the start).
                    _ => {
                        let failed = !played
                            && loading_since.is_some_and(|t| t.elapsed() > START_GRACE);
                        let timed_out = loading_since.is_some_and(|t| t.elapsed() > LOAD_DEADLINE);
                        if played {
                            let duration = m.duration_ms.or(position.1.filter(|_| m.hls_session.is_none())).unwrap_or(0);
                            let finished = duration > 0 && played_at + 5000 >= duration;
                            report_progress(&st, m, played_at, m.duration_ms, finished);
                            set(&mut |s| {
                                s.state = "idle".into();
                                s.finished = finished;
                            });
                        } else if failed || timed_out {
                            let why = match &hits_at_load {
                                Some((now, then)) if now.load(Ordering::Relaxed) == *then => CANT_REACH,
                                _ => CANT_PLAY,
                            };
                            set(&mut |s| {
                                s.state = "error".into();
                                s.error = Some(why.into());
                            });
                        } else {
                            continue;
                        }
                        stop_hls(&current);
                        current = None;
                        loading_since = None;
                        idle_since = Some(Instant::now());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
<friendlyName>[TV] Living &amp; Room</friendlyName><modelName>QE55</modelName>
<UDN>uuid:0ad1-55/..x</UDN>
<serviceList>
<service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/upnp/control/RenderingControl1</controlURL></service>
<service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>upnp/control/AVTransport1</controlURL></service>
</serviceList></device></root>"#;

    #[test]
    fn a_description_gives_the_control_addresses_on_the_device_itself() {
        let from = url::Url::parse("http://192.168.1.20:9197/dmr").unwrap();
        let r = parse_description(DESCRIPTION, &from).unwrap();
        assert_eq!(r.name, "[TV] Living & Room");
        assert_eq!(r.av, "http://192.168.1.20:9197/upnp/control/AVTransport1");
        assert_eq!(r.rc.as_deref(), Some("http://192.168.1.20:9197/upnp/control/RenderingControl1"));
        let f = found(r, "192.168.1.20:9197".parse().unwrap());
        assert_eq!((f.device.id.as_str(), f.device.kind.as_str()), ("dlna-0ad1-55..x", "dlna"));
        // Control somewhere else than on the device isn't taken: here that leaves no volume.
        let away = DESCRIPTION.replace("/upnp/control/RenderingControl1", "http://203.0.113.9/rc");
        assert_eq!(parse_description(&away, &from).unwrap().rc, None);
        // Something that isn't a renderer, or has no AVTransport.
        assert!(parse_description("<root><UDN>uuid:1</UDN></root>", &from).is_none());
        assert!(parse_description(&DESCRIPTION.replace("AVTransport:1", "AVTransport:9"), &from).is_none());
    }

    #[test]
    fn only_plain_local_addresses_are_opened() {
        assert!(local_url("http://192.168.1.20:9197/dmr", None).is_some());
        for bad in [
            "https://192.168.1.20/dmr",
            "http://8.8.8.8/dmr",
            "http://tv.local/dmr",
            "http://[fe80::1]/dmr",
            "http://user:pw@192.168.1.20/dmr",
            "file:///etc/passwd",
        ] {
            assert!(local_url(bad, None).is_none(), "{bad}");
        }
        let other: IpAddr = "192.168.1.21".parse().unwrap();
        assert!(local_url("http://192.168.1.20/dmr", Some(other)).is_none(), "a device speaks for itself");
    }

    #[test]
    fn positions_as_renderers_write_them() {
        assert_eq!(clock(3725.9), "1:02:05");
        assert_eq!(parse_clock("1:02:05"), Some(3_725_000));
        assert_eq!(parse_clock("0:00:01.500"), Some(1500));
        assert_eq!(parse_clock("NOT_IMPLEMENTED"), None);
        assert_eq!(parse_clock("0:99:00"), None);
    }

    #[test]
    fn the_description_sent_along_is_escaped() {
        let m = Media {
            url: "http://192.168.1.2:8200/dlna/media/1/2/sig.mkv?a=1&b=2".into(),
            content_type: "video/x-matroska".into(),
            title: "Tom & <Jerry>".into(),
            duration_ms: Some(3_725_000),
            ..super::super::tests::media(1)
        };
        let d = metadata(&m);
        assert!(d.contains("<dc:title>Tom &amp; &lt;Jerry&gt;</dc:title>"), "{d}");
        assert!(d.contains("sig.mkv?a=1&amp;b=2</res>"), "{d}");
        assert!(d.contains("protocolInfo=\"http-get:*:video/x-matroska:DLNA.ORG_OP=01"), "{d}");
        assert!(d.contains("duration=\"1:02:05.000\""), "{d}");
    }

    /// What is on this network (`cargo test -p corrent-server on_this_network -- --ignored --nocapture`):
    /// only searches and reads descriptions, plays nothing.
    #[tokio::test]
    #[ignore = "looks at the real network"]
    async fn on_this_network() {
        for f in search(Duration::from_secs(4), &[]).await {
            let r = f.dlna.as_deref().unwrap();
            println!("{} · {} · {} · av {} · rc {:?}", f.device.name, f.device.model, f.addr, r.av, r.rc);
        }
    }

    #[test]
    fn search_answers() {
        let a =
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nLocation: http://192.168.1.20:9197/dmr\r\nST: x\r\n\r\n";
        assert_eq!(header(a, "LOCATION"), Some("http://192.168.1.20:9197/dmr"));
        assert_eq!(header(a, "usn"), None);
    }
}
