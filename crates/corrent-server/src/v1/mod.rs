//! API v1 (`/api/v1`): JSON over HTTP(S), typed by `corrent-proto`.
//!
//! Authentication: `Authorization: Bearer <token>` (apps, device tokens) or the
//! `corrent_session` cookie (the web client) — cookie requests that change something must echo
//! the session's CSRF token in `X-Corrent-CSRF`. Errors are `ApiError` JSON with a stable code.

mod admin;
mod auth;
mod devices;
mod events;
pub mod library;
mod me;
pub mod playback;
mod plugins;
mod rss;
pub mod search;
mod server;
mod subtitles;
pub(crate) mod torrents;
mod users;
mod wanted;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use corrent_proto::auth::Perms;
use corrent_proto::{ApiError, ErrorCode};

use crate::identity::Principal;
use crate::state::AppState;

pub const COOKIE: &str = "corrent_session";
pub const CSRF_HEADER: &str = "x-corrent-csrf";

/// An `ApiError` as an HTTP response.
#[derive(Debug)]
pub struct ApiErr(pub ApiError);

impl From<ApiError> for ApiErr {
    fn from(e: ApiError) -> Self {
        ApiErr(e)
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self.0)).into_response()
    }
}

pub fn err(code: ErrorCode, msg: impl Into<String>) -> ApiErr {
    ApiErr(ApiError::new(code, msg))
}

pub type ApiResult<T> = Result<Json<T>, ApiErr>;

/// The client's address (after trusted proxies), put into request extensions by the guard.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = ApiErr;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiErr> {
        parts.extensions.get::<ClientIp>().copied().ok_or_else(|| err(ErrorCode::Internal, "no client address"))
    }
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|c| {
        let (k, v) = c.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// The host name the request was addressed to (from the guard).
#[derive(Clone, Debug)]
pub struct RequestHost(pub String);

/// How the caller authenticated (browsers: the cookie).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSource {
    Bearer,
    Cookie,
}

/// A signed-in caller.
pub struct Auth(pub Principal);

impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = ApiErr;
    async fn from_request_parts(parts: &mut Parts, st: &Arc<AppState>) -> Result<Self, ApiErr> {
        let ip = parts.extensions.get::<ClientIp>().map(|c| c.0.to_string()).unwrap_or_default();
        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|s| s.trim().to_string());
        let (token, from_cookie) = match bearer {
            Some(t) => (t, false),
            None => match cookie(&parts.headers, COOKIE) {
                Some(t) => (t, true),
                None => return Err(err(ErrorCode::Unauthorized, "not signed in")),
            },
        };
        let p = st
            .identity
            .verify_token(&token, &ip)
            .await?
            .ok_or_else(|| err(ErrorCode::Unauthorized, "session expired"))?;
        parts.extensions.insert(if from_cookie { AuthSource::Cookie } else { AuthSource::Bearer });
        if from_cookie && !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            let sent = parts.headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()).unwrap_or("");
            if sent.is_empty() || sent != p.csrf {
                return Err(err(ErrorCode::Forbidden, "missing or wrong CSRF token"));
            }
        }
        Ok(Auth(p))
    }
}

impl Auth {
    pub fn require(&self, p: Perms) -> Result<(), ApiErr> {
        if self.0.can(p) { Ok(()) } else { Err(err(ErrorCode::Forbidden, "permission denied").with_param("needs", p)) }
    }

    pub fn name(&self) -> &str {
        &self.0.user.name
    }
}

/// Another user's torrent without what is private to its owner: tracker URLs (private
/// trackers put the account's passkey in them), folders and the comment.
pub fn redact_torrent(p: &Principal, mut t: corrent_proto::Torrent) -> corrent_proto::Torrent {
    let own = t.added_by.as_deref() == Some(p.user.name.as_str());
    if !own && !p.can(Perms::TORRENTS_MANAGE_ALL) {
        t.trackers.clear();
        t.save_path.clear();
        t.content_path.clear();
        t.comment = None;
    }
    t
}

trait WithParam {
    fn with_param(self, k: &str, p: Perms) -> Self;
}

impl WithParam for ApiErr {
    fn with_param(self, k: &str, p: Perms) -> Self {
        ApiErr(self.0.with(k, p.names().join(",")))
    }
}

/// The host the client asked for: `Host` (HTTP/1.1) or the `:authority` that HTTP/2 puts in
/// the URI.
pub fn request_host<B>(req: &axum::http::Request<B>) -> String {
    req.headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default()
}

/// Every request: work out the client address, refuse non-local clients unless remote access
/// is on, refuse unknown Host names (DNS rebinding), and add protective headers.
pub async fn guard(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    let Some(peer) = peer else { return err(ErrorCode::Internal, "no peer address").into_response() };
    let nets = st.nets.load();
    let xff = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok());
    let ip = crate::net::client_ip(peer, xff, &nets.proxies);
    if !st.cfg.load().remote_access && !crate::net::is_local(ip, &nets.allowed) {
        tracing::info!("refused {ip}: remote access is off");
        return err(ErrorCode::RemoteAccessDisabled, "remote access is off").into_response();
    }
    let host = request_host(&req);
    if !crate::net::host_allowed(&host, &nets.hostnames) {
        return err(ErrorCode::Forbidden, "unknown host name").into_response();
    }
    req.extensions_mut().insert(ClientIp(ip));
    req.extensions_mut().insert(RequestHost(host));
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().expect("static"));
    h.insert(header::REFERRER_POLICY, "no-referrer".parse().expect("static"));
    h.insert(header::X_FRAME_OPTIONS, "DENY".parse().expect("static"));
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    }
    res
}

pub use plugins::signed_by_trusted as plugins_signed_by_trusted;

#[cfg(test)]
pub fn server_mask() -> &'static str {
    server::SECRET_MASK
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(auth::routes())
        .merge(users::routes())
        .merge(devices::routes())
        .merge(server::routes())
        .merge(torrents::routes())
        .merge(rss::routes())
        .merge(search::routes())
        .merge(library::routes())
        .merge(playback::routes())
        .merge(wanted::routes())
        .merge(plugins::routes())
        .merge(admin::routes())
        .merge(subtitles::routes())
        .merge(me::routes())
        .route("/events", axum::routing::get(events::events))
        .fallback(|| async { err(ErrorCode::NotFound, "no such endpoint") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_from_http2_authority() {
        let r = axum::http::Request::builder().uri("https://localhost:8443/api/v1/server/info").body(()).unwrap();
        assert_eq!(request_host(&r), "localhost:8443");
        let r = axum::http::Request::builder().uri("/x").header(header::HOST, "10.0.0.1:8443").body(()).unwrap();
        assert_eq!(request_host(&r), "10.0.0.1:8443");
    }
}
