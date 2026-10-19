//! Sign-in, sign-out, first-run setup and device pairing.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::{LoginRequest, LoginResponse, NewUser, PairRequest, PairingCode, Role, User};
use serde::Deserialize;

use super::{ApiErr, ApiResult, Auth, COOKIE, ClientIp, err};
use crate::identity::{Identity, Issued};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/setup", post(setup))
        .route("/auth/pair", post(pair))
        .route("/auth/pair/code", post(pair_code))
}

fn user_agent(h: &HeaderMap) -> String {
    h.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

fn limited(st: &AppState, ip: ClientIp) -> Result<(), ApiErr> {
    if st.limiter.lock().unwrap_or_else(|e| e.into_inner()).allow(ip.0) {
        Ok(())
    } else {
        Err(err(ErrorCode::TooManyRequests, "too many attempts"))
    }
}

/// The response for a new session: browsers get an HttpOnly cookie, apps the token itself.
fn session_response(st: &AppState, user: User, issued: Issued, device: bool) -> Response {
    let body = LoginResponse {
        user,
        token: device.then(|| issued.token.clone()),
        csrf: (!device).then(|| issued.csrf.clone()),
        expires_at: issued.expires_at,
    };
    if device {
        return Json(body).into_response();
    }
    let secure = if st.tls { "; Secure" } else { "" };
    let cookie = format!(
        "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
        issued.token,
        // The server slides the session; the browser keeps the cookie well beyond it.
        30 * 86_400
    );
    ([(header::SET_COOKIE, cookie)], Json(body)).into_response()
}

async fn login(
    State(st): State<Arc<AppState>>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiErr> {
    limited(&st, ip)?;
    match st.identity.authenticate(&req.username, &req.password).await {
        Ok(user) => {
            st.limiter.lock().unwrap_or_else(|e| e.into_inner()).succeeded(ip.0);
            let device = req.device.is_some();
            let issued = st.identity.issue(user.id, req.device, ip.0.to_string(), user_agent(&headers)).await?;
            st.identity
                .audit(Some(&user.name), &ip.0.to_string(), "login", if device { "device" } else { "web" }, true, "")
                .await;
            Ok(session_response(&st, user, issued, device))
        }
        Err(e) => {
            if e.code == ErrorCode::InvalidCredentials {
                st.limiter.lock().unwrap_or_else(|e| e.into_inner()).failed(ip.0);
            }
            st.identity
                .audit(Some(req.username.trim()), &ip.0.to_string(), "login", "", false, &format!("{:?}", e.code))
                .await;
            Err(e.into())
        }
    }
}

async fn logout(State(st): State<Arc<AppState>>, auth: Auth) -> Result<Response, ApiErr> {
    st.identity.revoke_session(auth.0.session_id).await?;
    let clear = format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    Ok(([(header::SET_COOKIE, clear)], Json(serde_json::json!({}))).into_response())
}

/// The signed-in user, plus the CSRF token for cookie sessions (so a reloaded page recovers it).
async fn me(auth: Auth) -> ApiResult<LoginResponse> {
    let cookie = auth.0.kind == crate::identity::SessionKind::Web;
    Ok(Json(LoginResponse {
        user: auth.0.user.clone(),
        token: None,
        csrf: cookie.then(|| auth.0.csrf.clone()),
        expires_at: 0,
    }))
}

/// First run: create the administrator. Only while no user exists, and only from this computer.
async fn setup(
    State(st): State<Arc<AppState>>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(mut req): Json<NewUser>,
) -> Result<Response, ApiErr> {
    limited(&st, ip)?;
    if !ip.0.is_loopback() {
        return Err(err(ErrorCode::Forbidden, "setup is only possible on the server itself"));
    }
    req.role = Role::Admin;
    req.perms = None;
    let user = st.identity.create_first_admin(req).await?;
    st.identity.audit(Some(&user.name), &ip.0.to_string(), "setup", "", true, "").await;
    let issued = st.identity.issue(user.id, None, ip.0.to_string(), user_agent(&headers)).await?;
    Ok(session_response(&st, user, issued, false))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CodeRequest {
    /// Admins may create a code for another user.
    user_id: Option<i64>,
    /// The address clients should use (shown in the pairing link).
    host: Option<String>,
}

async fn pair_code(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    axum::Extension(host): axum::Extension<super::RequestHost>,
    body: Option<Json<CodeRequest>>,
) -> ApiResult<PairingCode> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let uid = match req.user_id {
        Some(id) if id != auth.0.user.id => {
            auth.require(corrent_proto::auth::Perms::USERS_MANAGE)?;
            id
        }
        _ => auth.0.user.id,
    };
    let (code, expires) = st.identity.pairing_code(uid).await?;
    let host = req.host.unwrap_or(host.0);
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "pairing_code", &uid.to_string(), true, "").await;
    Ok(Json(Identity::pairing_link(
        &code,
        expires,
        &host,
        &st.server_id,
        st.tls_fingerprint.as_deref(),
        st.remote_status.load().connect_id.as_deref(),
    )))
}

async fn pair(
    State(st): State<Arc<AppState>>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(req): Json<PairRequest>,
) -> Result<Response, ApiErr> {
    limited(&st, ip)?;
    if !st.limiter.lock().unwrap_or_else(|e| e.into_inner()).may_pair(ip.0) {
        return Err(err(ErrorCode::TooManyRequests, "too many wrong pairing codes"));
    }
    let user = match st.identity.redeem_pairing(&req.code).await {
        Ok(u) => u,
        Err(e) => {
            {
                let mut l = st.limiter.lock().unwrap_or_else(|e| e.into_inner());
                l.failed(ip.0);
                l.pair_failed(ip.0);
            }
            st.identity.audit(None, &ip.0.to_string(), "pair", "", false, "").await;
            return Err(e.into());
        }
    };
    let name = req.device.name.clone();
    let issued = st.identity.issue(user.id, Some(req.device), ip.0.to_string(), user_agent(&headers)).await?;
    st.identity.audit(Some(&user.name), &ip.0.to_string(), "pair", &name, true, "").await;
    Ok(session_response(&st, user, issued, true))
}
