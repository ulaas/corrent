//! qBittorrent-compatible Web API (v2 subset) so Sonarr, Radarr and similar tools can use Corrent
//! as their download client.
//!
//! Off by default. Listens on 127.0.0.1 unless LAN access is enabled; always requires a password
//! (stored as a PBKDF2 hash). Cookie sessions as in qBittorrent, failed-login throttling, and
//! Host/Origin checks against DNS rebinding and cross-site requests.

mod auth;
mod qbt;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use axum::Router;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

pub use auth::{hash_password, verify_password};

use crate::config::Settings;
use crate::engine::EngineHandle;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WebApiSettings {
    pub enabled: bool,
    pub port: u16,
    /// Listen on all interfaces instead of only this computer.
    pub allow_lan: bool,
    pub username: String,
    /// PBKDF2 hash (see `auth::hash_password`); empty = not set, the API stays off.
    pub password_hash: String,
}

impl Default for WebApiSettings {
    fn default() -> Self {
        WebApiSettings {
            enabled: false,
            port: 8080,
            allow_lan: false,
            username: "admin".into(),
            password_hash: String::new(),
        }
    }
}

pub(crate) struct ApiState {
    engine: EngineHandle,
    settings: Arc<ArcSwap<Settings>>,
    cfg: WebApiSettings,
    sessions: Mutex<auth::Sessions>,
    rid: std::sync::atomic::AtomicU64,
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

fn router(state: Arc<ApiState>) -> Router {
    let api = qbt::routes()
        .layer(axum::middleware::from_fn_with_state(state.clone(), require_login))
        .route("/api/v2/auth/login", axum::routing::post(login).get(login));
    api.layer(axum::middleware::from_fn_with_state(state.clone(), check_origin))
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
        .with_state(state)
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
    if !host_allowed(host, st.cfg.allow_lan) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    for h in [header::ORIGIN, header::REFERER] {
        let Some(v) = headers.get(&h) else { continue };
        // A present but opaque/garbled origin ("null" from sandboxed frames) is not ours.
        let same = v.to_str().ok().and_then(|v| url::Url::parse(v).ok()).is_some_and(|u| {
            let origin_host = match u.port() {
                Some(p) => format!("{}:{p}", u.host_str().unwrap_or("")),
                None => u.host_str().unwrap_or("").to_string(),
            };
            origin_host.eq_ignore_ascii_case(host)
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
    // Verify outside the lock: PBKDF2 takes a while.
    let cfg = st.cfg.clone();
    let user_ok = user == cfg.username;
    let pass = pass.to_string();
    let pass_ok =
        tokio::task::spawn_blocking(move || verify_password(&cfg.password_hash, &pass)).await.unwrap_or(false);
    let mut sessions = st.sessions.lock().unwrap_or_else(|e| e.into_inner());
    if !(user_ok && pass_ok) {
        sessions.failed(ip);
        tracing::info!("web API: failed login from {ip}");
        return (StatusCode::OK, "Fails.").into_response();
    }
    let sid = sessions.create(ip);
    let cookie = format!("SID={sid}; HttpOnly; SameSite=Strict; Path=/");
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
