//! DLNA / UPnP-AV: a MediaServer that TVs, receivers and players on the local network (smart
//! TVs, VLC, Kodi, BubbleUPnP) find on their own and browse; they play the files as they are.
//!
//! Off by default (`dlna = true` in `server.toml`, then restart). DLNA has no accounts, so it
//! shows what one account (`dlna_user`, else the first administrator) may see, answers only
//! local addresses, and serves media only through signed URLs it handed out while browsing
//! (the key changes with every start). Plain HTTP on `dlna_port`: TVs don't trust our
//! certificates. The service descriptions (`*.xml`) come from librqbit-upnp-serve (Apache-2.0).
//!
//! The same listener serves Chromecasts (`crate::cast`) their signed media and HLS links; when
//! only casting needs it, it runs without announcing and without browsing.

mod didl;
mod ssdp;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use corrent_proto::auth::Role;
use corrent_proto::library::{ItemKind, ItemQuery, SortBy};

use crate::state::AppState;

pub const DEVICE: &str = "urn:schemas-upnp-org:device:MediaServer:1";
const XML: &str = "text/xml; charset=\"utf-8\"";
/// Most objects one Browse answer carries (clients page through the rest).
const PAGE_MAX: usize = 500;

/// The running server (dropping it stops announcing; the HTTP listener stops with it).
pub struct Running {
    _http: crate::Listener,
    ssdp: Option<(Arc<ssdp::Ssdp>, tokio::task::JoinHandle<()>)>,
    d: Arc<Dlna>,
    pub port: u16,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some((s, t)) = self.ssdp.take() {
            t.abort();
            // TVs forget us now rather than when the announcement expires.
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move { s.announce("ssdp:byebye").await });
            }
        }
    }
}

impl Running {
    /// Announced to TVs and browsable (not just serving casts).
    pub fn announcing(&self) -> bool {
        self.ssdp.is_some()
    }

    /// Links for other devices on the network (casting).
    pub fn links(&self) -> Links {
        Links { d: self.d.clone(), port: self.port }
    }
}

/// Signed URLs on the media listener.
#[derive(Clone)]
pub struct Links {
    d: Arc<Dlna>,
    port: u16,
}

impl Links {
    /// A library file as `user` may see it, at `host` (our address as the device sees it).
    pub fn media(&self, host: std::net::IpAddr, user: i64, file: i64, ext: &str) -> String {
        format!("http://{}{}", host_port(host, self.port), media_path(&self.d, user, file, ext))
    }

    /// Artwork by its hash.
    pub fn art(&self, host: std::net::IpAddr, sha: &str) -> String {
        let sig = sign(&self.d, &format!("art:{sha}"));
        format!("http://{}/dlna/art/{sha}/{sig}", host_port(host, self.port))
    }

    /// An HLS session's playlist.
    pub fn hls(&self, host: std::net::IpAddr, session: &str) -> String {
        let sig = sign(&self.d, &format!("hls:{session}"));
        format!("http://{}/dlna/hls/{session}/{sig}/index.m3u8", host_port(host, self.port))
    }
}

fn host_port(ip: std::net::IpAddr, port: u16) -> String {
    match ip {
        std::net::IpAddr::V6(v) => format!("[{v}]:{port}"),
        v4 => format!("{v4}:{port}"),
    }
}

fn media_path(d: &Dlna, user: i64, file: i64, ext: &str) -> String {
    format!("/dlna/media/{user}/{file}/{}.{ext}", sign(d, &format!("media:{user}:{file}")))
}

struct Dlna {
    st: Arc<AppState>,
    /// The account DLNA browses as (None: only casting).
    user: Option<i64>,
    key: aws_lc_rs::hmac::Key,
    udn: String,
    name: String,
    /// Changes when the server starts (clients drop cached listings).
    update_id: u32,
}

/// Starts the HTTP side, and with `announce` browsing and the SSDP announcer (DLNA).
pub async fn start(st: Arc<AppState>, announce: bool) -> anyhow::Result<Running> {
    let d = prepare(st, announce).await?;
    let cfg = d.st.cfg.load_full();
    let addr = SocketAddr::new(cfg.bind.parse().unwrap_or([0, 0, 0, 0].into()), cfg.dlna_port);
    let http = crate::serve_http(router(d.clone()), addr).await?;
    let port = http.addr.port();
    let ssdp = if announce {
        let runner = Arc::new(ssdp::Ssdp::new(d.udn.clone(), port, d.st.clone()).await?);
        tracing::info!("DLNA: announcing “{}” as {} (port {port})", d.name, d.udn);
        let r2 = runner.clone();
        Some((runner, tokio::spawn(async move { r2.run().await })))
    } else {
        None
    };
    Ok(Running { _http: http, ssdp, d, port })
}

/// The HTTP side alone (tests).
#[cfg(test)]
pub(crate) async fn test_router(st: Arc<AppState>, announce: bool) -> anyhow::Result<(Router, Links)> {
    let d = prepare(st, announce).await?;
    Ok((router(d.clone()), Links { d, port: 8201 }))
}

async fn prepare(st: Arc<AppState>, announce: bool) -> anyhow::Result<Arc<Dlna>> {
    let cfg = st.cfg.load_full();
    let user = if announce { Some(dlna_user(&st, &cfg).await?) } else { None };
    let rng = aws_lc_rs::rand::SystemRandom::new();
    let key = aws_lc_rs::hmac::Key::generate(aws_lc_rs::hmac::HMAC_SHA256, &rng)
        .map_err(|_| anyhow::anyhow!("no random numbers"))?;
    let d = Arc::new(Dlna {
        user,
        key,
        udn: udn(&st.server_id),
        name: cfg.display_name(),
        update_id: (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1)
            & 0x7fff_ffff) as u32,
        st: st.clone(),
    });
    Ok(d)
}

async fn dlna_user(st: &AppState, cfg: &crate::ServerConfig) -> anyhow::Result<i64> {
    let users = st.identity.users().await.map_err(|e| anyhow::anyhow!("{}", e.message))?;
    let user = if cfg.dlna_user.trim().is_empty() {
        users.iter().filter(|u| u.role == Role::Admin && !u.disabled).min_by_key(|u| u.id)
    } else {
        users.iter().find(|u| u.name.eq_ignore_ascii_case(cfg.dlna_user.trim()) && !u.disabled)
    };
    let Some(user) = user else {
        anyhow::bail!("no account for DLNA (set dlna_user to an existing, enabled account)");
    };
    Ok(user.id)
}

/// A stable UPnP device id from the server id.
fn udn(server_id: &str) -> String {
    let h = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, format!("corrent-dlna:{server_id}").as_bytes());
    let b = h.as_ref();
    let hex: String = b[..16].iter().map(|x| format!("{x:02x}")).collect();
    format!("uuid:{}-{}-4{}-a{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

fn router(d: Arc<Dlna>) -> Router {
    let st = d.st.clone();
    let mut r = Router::new()
        .route("/dlna/media/{user}/{file}/{sig}", get(media))
        .route("/dlna/hls/{session}/{sig}/{name}", get(hls))
        .route("/dlna/art/{sha}/{sig}", get(art));
    if d.user.is_some() {
        r = r
            .route("/dlna/description.xml", get(description))
            .route(
                "/dlna/ContentDirectory.xml",
                get(|| async { xml(include_str!("ContentDirectory.xml").to_string()) }),
            )
            .route(
                "/dlna/ConnectionManager.xml",
                get(|| async { xml(include_str!("ConnectionManager.xml").to_string()) }),
            )
            .route("/dlna/control/ContentDirectory", post(content_directory))
            .route("/dlna/control/ConnectionManager", post(connection_manager))
            .route("/dlna/events/{service}", any(subscribe));
    }
    r.layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn(
            move |ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next| {
                // Read per request: changes to the allowed networks apply at once.
                let ok = crate::net::is_local(peer.ip(), &crate::net::parse_cidrs(&st.cfg.load().allowed_networks));
                async move { if ok { next.run(req).await } else { StatusCode::FORBIDDEN.into_response() } }
            },
        ))
        .with_state(d)
}

fn xml(body: String) -> Response {
    ([(header::CONTENT_TYPE, XML)], body).into_response()
}

fn sign(d: &Dlna, what: &str) -> String {
    let t = aws_lc_rs::hmac::sign(&d.key, what.as_bytes());
    t.as_ref()[..12].iter().map(|x| format!("{x:02x}")).collect()
}

fn signed(d: &Dlna, what: &str, sig: &str) -> bool {
    let want = sign(d, what);
    // Constant time: the comparison doesn't stop at the first difference.
    want.len() == sig.len() && want.bytes().zip(sig.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

async fn description(State(d): State<Arc<Dlna>>) -> Response {
    xml(format!(
        r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0" xmlns:dlna="urn:schemas-dlna-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>{DEVICE}</deviceType>
    <friendlyName>{name}</friendlyName>
    <manufacturer>Corrent</manufacturer>
    <manufacturerURL>https://github.com/corrent</manufacturerURL>
    <modelName>Corrent media server</modelName>
    <modelNumber>{version}</modelNumber>
    <UDN>{udn}</UDN>
    <dlna:X_DLNADOC>DMS-1.50</dlna:X_DLNADOC>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
        <SCPDURL>/dlna/ContentDirectory.xml</SCPDURL>
        <controlURL>/dlna/control/ContentDirectory</controlURL>
        <eventSubURL>/dlna/events/ContentDirectory</eventSubURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>
        <SCPDURL>/dlna/ConnectionManager.xml</SCPDURL>
        <controlURL>/dlna/control/ConnectionManager</controlURL>
        <eventSubURL>/dlna/events/ConnectionManager</eventSubURL>
      </service>
    </serviceList>
  </device>
</root>"#,
        name = didl::esc(&d.name),
        version = env!("CARGO_PKG_VERSION"),
        udn = d.udn,
    ))
}

/// The SOAP action a request names (`"urn:…:ContentDirectory:1#Browse"` → `Browse`).
fn action(h: &HeaderMap) -> String {
    h.get("soapaction")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim_matches('"').rsplit_once('#').map(|(_, a)| a.to_string()))
        .unwrap_or_default()
}

fn soap(service: &str, action: &str, inner: &str) -> Response {
    xml(format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action}Response xmlns:u="urn:schemas-upnp-org:service:{service}:1">{inner}</u:{action}Response></s:Body></s:Envelope>"#
    ))
}

fn soap_fault(code: u16, text: &str) -> Response {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>{}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#,
        didl::esc(text)
    );
    (StatusCode::INTERNAL_SERVER_ERROR, [(header::CONTENT_TYPE, XML)], body).into_response()
}

async fn connection_manager(headers: HeaderMap) -> Response {
    const S: &str = "ConnectionManager";
    match action(&headers).as_str() {
        "GetProtocolInfo" => soap(S, "GetProtocolInfo", "<Source>http-get:*:*:*</Source><Sink></Sink>"),
        "GetCurrentConnectionIDs" => soap(S, "GetCurrentConnectionIDs", "<ConnectionIDs>0</ConnectionIDs>"),
        "GetCurrentConnectionInfo" => soap(
            S,
            "GetCurrentConnectionInfo",
            "<RcsID>-1</RcsID><AVTransportID>-1</AVTransportID><ProtocolInfo></ProtocolInfo>\
             <PeerConnectionManager></PeerConnectionManager><PeerConnectionID>-1</PeerConnectionID>\
             <Direction>Output</Direction><Status>OK</Status>",
        ),
        _ => soap_fault(401, "Invalid Action"),
    }
}

/// Event subscriptions are accepted (some TVs insist) but nothing is ever sent: listings change
/// with `SystemUpdateID` on the next browse.
async fn subscribe(method: Method) -> Response {
    match method.as_str() {
        "SUBSCRIBE" => {
            let sid = format!("uuid:{}", corrent_core::util::random_hex(16));
            let mut r = StatusCode::OK.into_response();
            if let Ok(v) = HeaderValue::from_str(&sid) {
                r.headers_mut().insert("sid", v);
            }
            r.headers_mut().insert("timeout", HeaderValue::from_static("Second-1800"));
            r
        }
        "UNSUBSCRIBE" => StatusCode::OK.into_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn content_directory(State(d): State<Arc<Dlna>>, headers: HeaderMap, body: String) -> Response {
    const S: &str = "ContentDirectory";
    let Some(user) = d.user else { return soap_fault(401, "Invalid Action") };
    match action(&headers).as_str() {
        "Browse" => {
            let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
            let object = didl::tag(&body, "ObjectID").unwrap_or_else(|| "0".into());
            let flag = didl::tag(&body, "BrowseFlag").unwrap_or_default();
            let start: usize = didl::tag(&body, "StartingIndex").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let count: usize = didl::tag(&body, "RequestedCount").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            let count = if count == 0 { PAGE_MAX } else { count.min(PAGE_MAX) };
            let r = if flag == "BrowseMetadata" {
                metadata(&d, user, &host, &object).await.map(|o| (o.into_iter().collect::<Vec<_>>(), 1))
            } else {
                children(&d, user, &host, &object, start, count).await
            };
            match r {
                Ok((objects, total)) => {
                    let result = didl::document(&objects);
                    soap(
                        S,
                        "Browse",
                        &format!(
                            "<Result>{}</Result><NumberReturned>{}</NumberReturned><TotalMatches>{total}</TotalMatches><UpdateID>{}</UpdateID>",
                            didl::esc(&result),
                            objects.len(),
                            d.update_id
                        ),
                    )
                }
                Err(()) => soap_fault(701, "No such object"),
            }
        }
        "GetSystemUpdateID" => soap(S, "GetSystemUpdateID", &format!("<Id>{}</Id>", d.update_id)),
        "GetSearchCapabilities" => soap(S, "GetSearchCapabilities", "<SearchCaps></SearchCaps>"),
        "GetSortCapabilities" => soap(S, "GetSortCapabilities", "<SortCaps></SortCaps>"),
        _ => soap_fault(401, "Invalid Action"),
    }
}

fn is_container(k: ItemKind) -> bool {
    matches!(k, ItemKind::Show | ItemKind::Season | ItemKind::Artist | ItemKind::Album | ItemKind::Folder)
}

async fn libraries(d: &Dlna, user: i64) -> Result<Vec<corrent_proto::library::Library>, ()> {
    let all = d.st.library.list().await.map_err(|_| ())?;
    let allowed = d.st.library.user_libraries(user).await.map_err(|_| ())?;
    Ok(if allowed.is_empty() { all } else { all.into_iter().filter(|l| allowed.contains(&l.id)).collect() })
}

fn root() -> didl::Object {
    didl::Object::Container { id: "0".into(), parent: "-1".into(), title: "Corrent".into(), children: None }
}

fn library_object(l: &corrent_proto::library::Library) -> didl::Object {
    didl::Object::Container {
        id: format!("l{}", l.id),
        parent: "0".into(),
        title: l.name.clone(),
        children: Some(l.item_count),
    }
}

/// An item as a DIDL object: containers for shows, seasons, artists, albums and folders; files
/// (with a signed URL) for the rest. `None`: nothing to play (the file is missing).
async fn item_object(d: &Dlna, user: i64, host: &str, it: &corrent_proto::library::Item) -> Option<didl::Object> {
    let parent = it.parent_id.map(|p| format!("i{p}")).unwrap_or_else(|| format!("l{}", it.library_id));
    let art = it
        .images
        .iter()
        .find(|i| matches!(i.kind.as_str(), "poster" | "primary" | "cover" | "thumb"))
        .or(it.images.first());
    let art = art.map(|i| format!("http://{host}/dlna/art/{}/{}", i.sha, sign(d, &format!("art:{}", i.sha))));
    // Unfinished downloads: the file on disk has holes (TVs would play zeros).
    if it.partial && !is_container(it.kind) {
        return None;
    }
    if is_container(it.kind) {
        return Some(didl::Object::Container {
            id: format!("i{}", it.id),
            parent,
            title: it.title.clone(),
            children: it.child_count,
        });
    }
    let (file, path) = d.st.library.main_file(user, it.id).await.ok()?;
    let path = std::path::PathBuf::from(path);
    let size = tokio::fs::metadata(&path).await.ok()?.len();
    let mime = crate::v1::library::mime_for(&path).to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_ascii_lowercase();
    let url = format!("http://{host}{}", media_path(d, user, file, &ext));
    let class = match it.kind {
        ItemKind::Movie => "object.item.videoItem.movie",
        ItemKind::Episode | ItemKind::Video => "object.item.videoItem",
        ItemKind::Track => "object.item.audioItem.musicTrack",
        ItemKind::Book => "object.item.audioItem.audioBook",
        ItemKind::Photo => "object.item.imageItem.photo",
        _ => "object.item",
    };
    Some(didl::Object::Item(didl::Item {
        id: format!("i{}", it.id),
        parent,
        title: match (it.kind, it.index) {
            (ItemKind::Episode | ItemKind::Track, Some(n)) => format!("{n:02}. {}", it.title),
            _ => it.title.clone(),
        },
        class,
        year: it.year,
        // Episode: show, season; track: album artist, album.
        artist: (it.kind == ItemKind::Track).then(|| it.context.first().cloned()).flatten(),
        album: (it.kind == ItemKind::Track).then(|| it.context.get(1).cloned()).flatten(),
        track: (it.kind == ItemKind::Track).then_some(it.index).flatten(),
        duration_ms: it.runtime_ms,
        art,
        url,
        mime,
        size,
    }))
}

async fn metadata(d: &Dlna, user: i64, host: &str, object: &str) -> Result<Option<didl::Object>, ()> {
    if object == "0" {
        return Ok(Some(root()));
    }
    if let Some(id) = object.strip_prefix('l').and_then(|s| s.parse::<i64>().ok()) {
        return Ok(libraries(d, user).await?.iter().find(|l| l.id == id).map(library_object));
    }
    let id = object.strip_prefix('i').and_then(|s| s.parse::<i64>().ok()).ok_or(())?;
    let it = d.st.library.item(user, id).await.map_err(|_| ())?;
    Ok(item_object(d, user, host, &it.item).await)
}

async fn children(
    d: &Dlna,
    user: i64,
    host: &str,
    object: &str,
    start: usize,
    count: usize,
) -> Result<(Vec<didl::Object>, i64), ()> {
    if object == "0" {
        let libs = libraries(d, user).await?;
        let total = libs.len() as i64;
        return Ok((libs.iter().skip(start).take(count).map(library_object).collect(), total));
    }
    let mut q = ItemQuery { offset: start as i64, limit: Some(count as i64), ..Default::default() };
    if let Some(id) = object.strip_prefix('l').and_then(|s| s.parse::<i64>().ok()) {
        if !libraries(d, user).await?.iter().any(|l| l.id == id) {
            return Err(());
        }
        q.library_id = Some(id);
    } else {
        let id = object.strip_prefix('i').and_then(|s| s.parse::<i64>().ok()).ok_or(())?;
        let parent = d.st.library.item(user, id).await.map_err(|_| ())?;
        // Seasons, episodes and tracks in their order; the rest by title.
        if matches!(parent.item.kind, ItemKind::Show | ItemKind::Season | ItemKind::Album) {
            q.sort = SortBy::Index;
        }
        q.parent_id = Some(id);
    }
    let page = d.st.library.items(user, q).await.map_err(|_| ())?;
    let mut out = Vec::with_capacity(page.items.len());
    for it in &page.items {
        if let Some(o) = item_object(d, user, host, it).await {
            out.push(o);
        }
    }
    Ok((out, page.total))
}

/// Headers DLNA renderers look for before they play.
fn dlna_headers(res: &mut Response, image: bool) {
    let h = res.headers_mut();
    h.insert("transfermode.dlna.org", HeaderValue::from_static(if image { "Interactive" } else { "Streaming" }));
    h.insert(
        "contentfeatures.dlna.org",
        HeaderValue::from_static("DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000"),
    );
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
}

async fn media(
    State(d): State<Arc<Dlna>>,
    Path((user, file, sig)): Path<(i64, i64, String)>,
    req: Request,
) -> Response {
    let sig = sig.split('.').next().unwrap_or("");
    if !signed(&d, &format!("media:{user}:{file}"), sig) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Still checked: the account may have lost access since the link was made.
    let Ok(path) = d.st.library.file_path(user, file).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match crate::v1::library::serve_path(path.into(), req).await {
        Ok(mut r) => {
            let image = r
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|t| t.starts_with("image/"));
            dlna_headers(&mut r, image);
            r
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// HLS for casting: the session's playlist and segments (the receiver fetches them with CORS).
async fn hls(
    State(d): State<Arc<Dlna>>,
    Path((session, sig, name)): Path<(String, String, String)>,
    req: Request,
) -> Response {
    if !signed(&d, &format!("hls:{session}"), &sig) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut res = if name == "index.m3u8" {
        match d.st.hls.playlist(&session) {
            Ok(body) => {
                ([(header::CONTENT_TYPE, "application/vnd.apple.mpegurl"), (header::CACHE_CONTROL, "no-store")], body)
                    .into_response()
            }
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        let Some(n) = name.strip_suffix(".ts").and_then(|n| n.parse::<u64>().ok()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(path) = d.st.hls.segment(&session, n).await else { return StatusCode::NOT_FOUND.into_response() };
        match tower::ServiceExt::oneshot(
            tower_http::services::ServeFile::new_with_mime(&path, &"video/mp2t".parse().expect("static")),
            req,
        )
        .await
        {
            Ok(r) => r.map(Body::new),
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        }
    };
    res.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    res
}

async fn art(State(d): State<Arc<Dlna>>, Path((sha, sig)): Path<(String, String)>, req: Request) -> Response {
    if !signed(&d, &format!("art:{sha}"), &sig) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(path) = corrent_library::artwork::existing(d.st.library.artwork_root(), &sha, Some(480)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match crate::v1::library::serve_path(path, req).await {
        Ok(mut r) => {
            dlna_headers(&mut r, true);
            r
        }
        Err(_) => (StatusCode::NOT_FOUND, Body::empty()).into_response(),
    }
}

#[cfg(test)]
mod tests;
