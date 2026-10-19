//! A Jellyfin-compatible API subset, so Jellyfin apps (Jellyfin Web, Android TV, Swiftfin,
//! Findroid, Infuse, the Kodi add-on) can sign in with a Corrent account, browse the libraries
//! and play them. See `docs/JELLYFIN-PLAN.md` for the surface and its sources.
//!
//! Served at `/jellyfin` on the main listener and at the root of a plain-HTTP, local-network
//! listener (`jellyfin_port`, 8096 — the apps refuse our private CA). Paths are matched without
//! case (as ASP.NET does), query keys too. Auth: Jellyfin's `Authorization: MediaBrowser …`
//! header, `X-Emby-*` headers or an `api_key`/`ApiKey` query — never the browser cookie. Corrent
//! reports itself as "Jellyfin Server" with a current version because the apps accept nothing
//! else; its real name is in `ServerName`.

mod dto;
pub mod ids;
mod items;
mod playback;
mod playstate;
mod system;
mod users;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::identity::Principal;
use crate::state::AppState;
use crate::v1::ClientIp;

/// Per-server state of the Jellyfin surface.
#[derive(Default)]
pub struct JfState {
    /// Play sessions from `PlaybackInfo` (id → session); an HLS session starts on first use.
    pub(crate) plays: Mutex<HashMap<String, playback::PlaySession>>,
    /// Device profiles from `Sessions/Capabilities/Full`, per (user, device id).
    pub(crate) profiles: Mutex<HashMap<(i64, String), Value>>,
}

/// A response with Jellyfin's habits: plain text errors.
pub(crate) fn fail(status: StatusCode, text: &str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text.to_string()).into_response()
}

pub(crate) fn not_found() -> Response {
    fail(StatusCode::NOT_FOUND, "Not found")
}

pub(crate) fn json(v: Value) -> Response {
    axum::Json(v).into_response()
}

pub(crate) fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// An empty `QueryResult`.
pub(crate) fn empty_result() -> Response {
    json(serde_json::json!({ "Items": [], "TotalRecordCount": 0, "StartIndex": 0 }))
}

/// Query parameters with lower-case keys (clients send `ParentId`, `parentId` and `parentid`).
#[derive(Default, Debug)]
pub(crate) struct Q(HashMap<String, String>);

impl Q {
    pub fn parse(query: Option<&str>) -> Q {
        let mut m = HashMap::new();
        for (k, v) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
            m.entry(k.to_ascii_lowercase()).or_insert_with(|| v.into_owned());
        }
        Q(m)
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(String::as_str).filter(|v| !v.is_empty())
    }

    pub fn i64(&self, k: &str) -> Option<i64> {
        self.get(k).and_then(|v| v.trim().parse().ok())
    }

    pub fn bool(&self, k: &str) -> Option<bool> {
        self.get(k).map(|v| v.eq_ignore_ascii_case("true") || v == "1")
    }

    /// A comma (or `|`) separated list.
    pub fn list(&self, k: &str) -> Vec<String> {
        self.get(k)
            .map(|v| v.split([',', '|']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Q {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Q::parse(parts.uri.query()))
    }
}

/// What a Jellyfin client says about itself (`Authorization: MediaBrowser Client=…`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DeviceCtx {
    pub client: String,
    pub device: String,
    pub device_id: String,
    pub version: String,
    pub token: Option<String>,
}

fn pct_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

/// `MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="…", Version="10.10.7", Token="…"`.
pub(crate) fn parse_authorization(v: &str) -> Option<DeviceCtx> {
    let v = v.trim();
    let (scheme, rest) = v.split_once(char::is_whitespace).unwrap_or((v, ""));
    if !(scheme.eq_ignore_ascii_case("mediabrowser") || scheme.eq_ignore_ascii_case("emby")) {
        return None;
    }
    let mut d = DeviceCtx::default();
    let mut parts = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    for p in parts {
        let Some((k, val)) = p.split_once('=') else { continue };
        let val = pct_decode(val.trim().trim_matches('"'));
        match k.trim().to_ascii_lowercase().as_str() {
            "client" => d.client = val,
            "device" => d.device = val,
            "deviceid" => d.device_id = val,
            "version" => d.version = val,
            "token" => d.token = Some(val).filter(|t| !t.is_empty()),
            _ => {}
        }
    }
    Some(d)
}

/// The client's description, from the headers (before or after signing in).
pub(crate) fn device_ctx(headers: &HeaderMap, q: &Q) -> DeviceCtx {
    let auth = ["x-emby-authorization", "authorization"]
        .iter()
        .filter_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()))
        .find_map(parse_authorization);
    let mut d = auth.unwrap_or_default();
    if d.token.is_none() {
        d.token = ["x-emby-token", "x-mediabrowser-token"]
            .iter()
            .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()).map(str::trim).map(str::to_string))
            .or_else(|| q.get("apikey").or_else(|| q.get("api_key")).map(str::to_string))
            .filter(|t| !t.is_empty());
    }
    if d.device_id.is_empty() {
        d.device_id = q.get("deviceid").unwrap_or("").to_string();
    }
    d
}

/// A signed-in Jellyfin client.
pub(crate) struct JfAuth {
    pub p: Principal,
    pub dev: DeviceCtx,
}

impl FromRequestParts<Arc<AppState>> for JfAuth {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, st: &Arc<AppState>) -> Result<Self, Response> {
        let q = Q::parse(parts.uri.query());
        let dev = device_ctx(&parts.headers, &q);
        let ip = parts.extensions.get::<ClientIp>().map(|c| c.0.to_string()).unwrap_or_default();
        let Some(token) = dev.token.clone() else { return Err(fail(StatusCode::UNAUTHORIZED, "Unauthorized")) };
        match st.identity.verify_token(&token, &ip).await {
            Ok(Some(p)) => Ok(JfAuth { p, dev }),
            Ok(None) => Err(fail(StatusCode::UNAUTHORIZED, "Unauthorized")),
            Err(_) => Err(fail(StatusCode::SERVICE_UNAVAILABLE, "Try again")),
        }
    }
}

impl JfAuth {
    pub fn user(&self) -> i64 {
        self.p.user.id
    }

    /// A `userId` in the path or query must be the caller (administrators may read others).
    // A response is what the handlers return anyway.
    #[allow(clippy::result_large_err)]
    pub fn check_user(&self, id: Option<&str>) -> Result<i64, Response> {
        match id.filter(|s| !s.is_empty()) {
            None => Ok(self.user()),
            Some(s) => match ids::dec_kind(s, ids::USER) {
                Some(u) if u == self.user() => Ok(u),
                Some(u) if self.p.can(corrent_proto::auth::Perms::USERS_MANAGE) => Ok(u),
                Some(_) => Err(fail(StatusCode::FORBIDDEN, "Forbidden")),
                None => Err(not_found()),
            },
        }
    }
}

/// Lower-cases the path (ASP.NET matches routes without case) and drops a `/jellyfin` prefix
/// (the plain-HTTP listener serves both).
fn normalize(mut req: Request) -> Request {
    let path = req.uri().path().to_ascii_lowercase();
    let path = match path.strip_prefix("/jellyfin") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.to_string(),
        _ => path,
    };
    let path = if path.is_empty() { "/".to_string() } else { path };
    let pq = match req.uri().query() {
        Some(q) => format!("{path}?{q}"),
        None => path,
    };
    if let Ok(uri) = pq.parse::<Uri>() {
        *req.uri_mut() = uri;
    }
    req
}

/// Off unless `jellyfin_api`; CORS when asked for; the cookie never counts here.
async fn gate(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let cfg = st.cfg.load();
    if !cfg.jellyfin_api {
        return not_found();
    }
    let cors = cfg.jellyfin_cors;
    drop(cfg);
    req.headers_mut().remove(header::COOKIE);
    if cors && req.method() == Method::OPTIONS {
        let mut r = StatusCode::NO_CONTENT.into_response();
        add_cors(r.headers_mut());
        return r;
    }
    let mut res = next.run(req).await;
    if cors {
        add_cors(res.headers_mut());
    }
    res
}

fn add_cors(h: &mut HeaderMap) {
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, HEAD, POST, DELETE, OPTIONS"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "Authorization, X-Emby-Authorization, X-Emby-Token, X-MediaBrowser-Token, Content-Type, Range, X-Requested-With",
        ),
    );
    h.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("Content-Range, Content-Length"));
}

/// The Jellyfin surface as a router to nest (`/jellyfin`) or serve at a root.
pub fn service(st: Arc<AppState>) -> Router {
    let inner: Router = Router::new()
        .merge(system::routes())
        .merge(users::routes())
        .merge(items::routes())
        .merge(playback::routes())
        .merge(playstate::routes())
        .fallback(|| async { not_found() })
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(st.clone());
    let svc = tower::ServiceBuilder::new().map_request(normalize).service(inner);
    Router::new().fallback_service(svc).layer(axum::middleware::from_fn_with_state(st, gate))
}

/// The plain-HTTP listener (`jellyfin_port`): local network only, whatever `remote_access` says.
pub async fn listen(st: Arc<AppState>) -> anyhow::Result<crate::Listener> {
    let cfg = st.cfg.load_full();
    let addr = std::net::SocketAddr::new(cfg.bind.parse().unwrap_or([0, 0, 0, 0].into()), cfg.jellyfin_port);
    let st2 = st.clone();
    let app = service(st.clone()).layer(axum::middleware::from_fn_with_state(st.clone(), crate::v1::guard)).layer(
        axum::middleware::from_fn(
            move |axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
                  req: Request,
                  next: Next| {
                let ok = crate::net::is_local(peer.ip(), &st2.nets.load().allowed);
                async move { if ok { next.run(req).await } else { fail(StatusCode::FORBIDDEN, "Local network only") } }
            },
        ),
    );
    crate::serve_http(app, addr).await
}
