//! qBittorrent-compatible Web API (v2 subset) so Sonarr, Radarr and similar tools can use Corrent
//! as their download client.
//!
//! Off by default. Listens on 127.0.0.1 unless LAN access is enabled; always requires a password
//! (stored as a PBKDF2 hash). Cookie sessions as in qBittorrent, failed-login throttling, and
//! Host/Origin checks against DNS rebinding and cross-site requests.

// STABILITY.md F17: request handling doesn't index (tests may; clippy.toml).
#![deny(clippy::indexing_slicing)]

pub(crate) mod auth;
mod qbt;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub use auth::{hash_password, verify_password};

use crate::config::Settings;
use crate::engine::EngineHandle;

pub use corrent_core::config::WebApiSettings;

pub(crate) struct ApiState {
    engine: EngineHandle,
    settings: Arc<ArcSwap<Settings>>,
    cfg: WebApiSettings,
    sessions: Mutex<auth::Sessions>,
    rid: std::sync::atomic::AtomicU64,
    /// Inside the media server: sign-in checks server users, and the server's own guard has
    /// already checked the Host header.
    identity: Option<crate::identity::Identity>,
    /// Served over HTTPS: cookies get `Secure`.
    secure_cookie: bool,
}

/// A running server; stops when dropped.
pub struct WebApiServer {
    pub addr: SocketAddr,
    pub cfg: WebApiSettings,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for WebApiServer {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
    }
}

impl WebApiServer {
    pub async fn start(
        cfg: WebApiSettings,
        engine: EngineHandle,
        settings: Arc<ArcSwap<Settings>>,
    ) -> anyhow::Result<WebApiServer> {
        anyhow::ensure!(!cfg.password_hash.is_empty(), "set a password first");
        let ip = if cfg.allow_lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
        // A server being restarted may still hold the port for a moment.
        let mut tries = 0;
        let listener = loop {
            match tokio::net::TcpListener::bind((ip, cfg.port)).await {
                Ok(l) => break l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tries < 10 => {
                    tries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
                Err(e) => return Err(e.into()),
            }
        };
        let addr = listener.local_addr()?;
        let state = Arc::new(ApiState {
            engine,
            settings,
            cfg: cfg.clone(),
            sessions: Default::default(),
            rid: Default::default(),
            identity: None,
            secure_cookie: false,
        });
        let app = router(state);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let serve = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                });
            if let Err(e) = serve.await {
                tracing::warn!("web API stopped: {e}");
            }
        });
        tracing::info!("web API listening on {addr}");
        Ok(WebApiServer { addr, cfg, stop: Some(tx) })
    }
}

/// The qBittorrent-compatible API and the classic web UI as part of the media server: users sign
/// in with their server account (it needs the `downloader` or `torrents.manage_all` permission).
pub fn media_server_router(
    engine: EngineHandle,
    settings: Arc<ArcSwap<Settings>>,
    identity: crate::identity::Identity,
    tls: bool,
) -> Router {
    let cfg = WebApiSettings { enabled: true, allow_lan: true, ..Default::default() };
    router(Arc::new(ApiState {
        engine,
        settings,
        cfg,
        sessions: Default::default(),
        rid: Default::default(),
        identity: Some(identity),
        secure_cookie: tls,
    }))
}

fn router(state: Arc<ApiState>) -> Router {
    let api = qbt::routes()
        .layer(axum::middleware::from_fn_with_state(state.clone(), require_login))
        // POST only: a GET login would put the password into URLs and logs.
        .route("/api/v2/auth/login", axum::routing::post(login))
        // The web UI's static files are public; everything it does goes through the API.
        .route(
            if state.identity.is_some() { "/classic" } else { "/" },
            axum::routing::get(|| async { webui("text/html; charset=utf-8", WEBUI_HTML) }),
        )
        .route("/webui/app.js", axum::routing::get(|| async { webui("text/javascript; charset=utf-8", WEBUI_JS) }))
        .route("/webui/style.css", axum::routing::get(|| async { webui("text/css; charset=utf-8", WEBUI_CSS) }));
    api.layer(axum::middleware::from_fn_with_state(state.clone(), check_origin))
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
        .with_state(state)
}

const WEBUI_HTML: &str = include_str!("../../../../assets/webui/index.html");
const WEBUI_JS: &str = include_str!("../../../../assets/webui/app.js");
const WEBUI_CSS: &str = include_str!("../../../../assets/webui/style.css");

/// A web UI file with headers that keep it from being framed, sniffed or extended with
/// scripts from elsewhere.
fn webui(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self';                  form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
            ),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "same-origin"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

fn host_of(v: &str) -> &str {
    // "host:port", "[v6]:port" → host
    if let Some(rest) = v.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    v.rsplit_once(':').map(|(h, _)| h).unwrap_or(v)
}

/// Reject cross-site requests and (when local-only) foreign Host names (DNS rebinding).
async fn check_origin(State(st): State<Arc<ApiState>>, headers: HeaderMap, req: Request, next: Next) -> Response {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if st.identity.is_none() && !host_allowed(host, st.cfg.allow_lan) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    for h in [header::ORIGIN, header::REFERER] {
        let Some(v) = headers.get(&h) else { continue };
        // A present but opaque/garbled origin ("null" from sandboxed frames) is not ours.
        let same = v.to_str().ok().and_then(|v| url::Url::parse(v).ok()).is_some_and(|u| {
            // Compare host and port, filling in default ports (":80" may or may not be written).
            let origin = (u.host_str().unwrap_or("").to_ascii_lowercase(), u.port_or_known_default());
            origin == host_and_port(host)
        });
        if !same {
            return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
    }
    next.run(req).await
}

/// Host header check against DNS rebinding: names other than `localhost` could point anywhere,
/// so only `localhost` and IP literals are accepted (loopback ones unless LAN access is on).
fn host_allowed(host: &str, allow_lan: bool) -> bool {
    let h = host_of(host).to_ascii_lowercase();
    if h == "localhost" {
        return true;
    }
    match h.parse::<IpAddr>() {
        Ok(ip) => allow_lan || ip.is_loopback(),
        Err(_) => false,
    }
}

/// `Host` header → (host, port); no port means 80. IPv6 brackets kept as `url` writes them.
fn host_and_port(host: &str) -> (String, Option<u16>) {
    let (h, port) = match host.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.contains(']') && (!h.contains(':') || h.ends_with(']')) => {
            (h, p.parse().ok())
        }
        _ => (host, Some(80)),
    };
    (h.to_ascii_lowercase(), port)
}

fn sid_from(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|c| {
        let (k, v) = c.trim().split_once('=')?;
        (k == "SID").then(|| v.to_string())
    })
}

async fn require_login(State(st): State<Arc<ApiState>>, headers: HeaderMap, req: Request, next: Next) -> Response {
    let ok = sid_from(&headers).is_some_and(|sid| st.sessions.lock().unwrap_or_else(|e| e.into_inner()).check(&sid));
    if !ok {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    next.run(req).await
}

async fn login(State(st): State<Arc<ApiState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let form = qbt::read_form(req).await;
    let user = form.get("username").map(String::as_str).unwrap_or("");
    let pass = form.get("password").map(String::as_str).unwrap_or("");
    let ip = peer.ip();
    if st.sessions.lock().unwrap_or_else(|e| e.into_inner()).banned(ip) {
        return (
            StatusCode::FORBIDDEN,
            "Your IP address has been banned after too many failed authentication attempts.",
        )
            .into_response();
    }
    let pass = pass.to_string();
    let ok = match &st.identity {
        Some(identity) => match identity.authenticate(user, &pass).await {
            Ok(u) => {
                let allowed = u.perms.contains(corrent_proto::auth::Perms::DOWNLOADER)
                    || u.perms.contains(corrent_proto::auth::Perms::TORRENTS_MANAGE_ALL);
                identity.audit(Some(&u.name), &ip.to_string(), "login", "qbittorrent api", allowed, "").await;
                allowed
            }
            Err(_) => false,
        },
        None => {
            // Verify outside the lock: PBKDF2 takes a while.
            let cfg = st.cfg.clone();
            let user_ok = user == cfg.username;
            // PBKDF2 is deliberately slow: at most two checks at a time, so a flood of attempts
            // from the network can't keep every core busy.
            static VERIFY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
            let Ok(_permit) = VERIFY.acquire().await else {
                return (StatusCode::SERVICE_UNAVAILABLE, "").into_response();
            };
            let pass_ok =
                tokio::task::spawn_blocking(move || verify_password(&cfg.password_hash, &pass)).await.unwrap_or(false);
            user_ok && pass_ok
        }
    };
    let mut sessions = st.sessions.lock().unwrap_or_else(|e| e.into_inner());
    if !ok {
        sessions.failed(ip);
        tracing::info!("web API: failed login from {ip}");
        return (StatusCode::OK, "Fails.").into_response();
    }
    let sid = sessions.create(ip);
    let secure = if st.secure_cookie { "; Secure" } else { "" };
    let cookie = format!("SID={sid}; HttpOnly; SameSite=Strict; Path=/{secure}");
    ([(header::SET_COOKIE, cookie)], "Ok.").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_cookies() {
        assert_eq!(host_of("localhost:8080"), "localhost");
        assert_eq!(host_of("[::1]:8080"), "::1");
        assert_eq!(host_of("127.0.0.1"), "127.0.0.1");
        assert!(host_allowed("localhost:8080", false));
        assert_eq!(host_and_port("127.0.0.1"), ("127.0.0.1".into(), Some(80)));
        assert_eq!(host_and_port("LOCALHOST:8080"), ("localhost".into(), Some(8080)));
        assert_eq!(host_and_port("[::1]:8080"), ("[::1]".into(), Some(8080)));
        assert!(host_allowed("[::1]:8080", false));
        assert!(!host_allowed("192.168.1.5:8080", false));
        assert!(host_allowed("192.168.1.5:8080", true));
        assert!(!host_allowed("evil.example:8080", true), "names can be rebound");
        assert!(!host_allowed("", true));
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "a=b; SID=abc; c=d".parse().unwrap());
        assert_eq!(sid_from(&h).as_deref(), Some("abc"));
    }
}

#[cfg(test)]
mod e2e_tests;
