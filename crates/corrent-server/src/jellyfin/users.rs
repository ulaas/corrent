//! Signing in, users, the client's session and stored preferences.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::{delete, get, post};
use corrent_proto::auth::{DeviceInfo, Perms};
use serde_json::{Value, json};

use super::system::ctx;
use super::{JfAuth, Q, device_ctx, dto, fail, json, no_content, not_found};
use crate::state::AppState;
use crate::v1::ClientIp;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users/authenticatebyname", post(authenticate))
        .route("/users/public", get(public))
        .route("/users/me", get(me))
        .route("/users", get(list))
        .route("/users/{id}", get(one))
        .route("/users/configuration", post(set_configuration))
        .route("/users/{id}/configuration", post(set_configuration_of))
        .route("/sessions/logout", post(logout))
        .route("/sessions/capabilities", post(capabilities))
        .route("/sessions/capabilities/full", post(capabilities))
        .route("/sessions", get(sessions))
        .route("/sessions/{id}/playing", post(|_a: JfAuth| async { no_content() }))
        .route(
            "/sessions/{id}/user/{user}",
            post(|_a: JfAuth| async { no_content() }).delete(|_a: JfAuth| async { no_content() }),
        )
        .route("/displaypreferences/{id}", get(display_get).post(display_set))
        .route("/devices", delete(|_a: JfAuth| async { no_content() }))
}

fn body_json(b: &Bytes) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

/// A field in PascalCase or camelCase.
fn field<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    let lower = format!("{}{}", name[..1].to_ascii_lowercase(), &name[1..]);
    v.get(name).or_else(|| v.get(&lower))
}

async fn authenticate(State(st): State<Arc<AppState>>, ip: ClientIp, headers: HeaderMap, body: Bytes) -> Response {
    let q = Q::default();
    let mut dev = device_ctx(&headers, &q);
    let b = body_json(&body);
    let name = field(&b, "Username").and_then(Value::as_str).unwrap_or("").to_string();
    let pw = field(&b, "Pw").or_else(|| field(&b, "Password")).and_then(Value::as_str).unwrap_or("").to_string();
    if !st.limiter.lock().unwrap_or_else(|e| e.into_inner()).allow() {
        return fail(StatusCode::TOO_MANY_REQUESTS, "Too many attempts");
    }
    let user = match st.identity.authenticate(&name, &pw).await {
        Ok(u) => u,
        Err(e) => {
            st.identity
                .audit(Some(name.trim()), &ip.0.to_string(), "login", "jellyfin", false, &format!("{:?}", e.code))
                .await;
            return fail(StatusCode::UNAUTHORIZED, "Invalid user or password entered.");
        }
    };
    // Jellyfin refuses logins without device information; we make one up instead.
    if dev.device_id.is_empty() {
        dev.device_id = format!("corrent-{}", corrent_core::util::random_hex(8));
    }
    let client = if dev.client.is_empty() {
        "Jellyfin app".to_string()
    } else {
        format!("{} {}", dev.client, dev.version).trim().to_string()
    };
    let device = DeviceInfo {
        name: if dev.device.is_empty() { client.clone() } else { dev.device.clone() },
        platform: "jellyfin".into(),
        client,
    };
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let issued = match st.identity.issue_external(user.id, device, dev.device_id.clone(), ip.0.to_string(), ua).await {
        Ok(i) => i,
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "Try again"),
    };
    st.identity.audit(Some(&user.name), &ip.0.to_string(), "login", "jellyfin", true, &dev.client).await;
    let c = ctx(&st);
    let config = stored(&st, user.id, "configuration").await;
    // The session id (Kodi keeps it) is the device's.
    let sid = issued.device_id.unwrap_or(0);
    json(json!({
        "User": dto::user_dto(&c, &user, config),
        "SessionInfo": dto::session_dto(&c, &user, sid, &dev, &ip.0.to_string()),
        "AccessToken": issued.token,
        "ServerId": c.server_id,
    }))
}

async fn stored(st: &AppState, user: i64, key: &str) -> Option<Value> {
    st.identity.jf_pref(user, key).await.ok().flatten().and_then(|s| serde_json::from_str(&s).ok())
}

async fn public(State(st): State<Arc<AppState>>) -> Response {
    let c = ctx(&st);
    let users = st.identity.signin_users().await.unwrap_or_default();
    json(Value::Array(users.iter().map(|u| dto::user_dto(&c, u, None)).collect()))
}

async fn me(State(st): State<Arc<AppState>>, a: JfAuth) -> Response {
    let config = stored(&st, a.user(), "configuration").await;
    json(dto::user_dto(&ctx(&st), &a.p.user, config))
}

async fn one(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>) -> Response {
    let uid = match a.check_user(Some(&id)) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Ok(user) = st.identity.user(uid).await else { return not_found() };
    let config = stored(&st, uid, "configuration").await;
    json(dto::user_dto(&ctx(&st), &user, config))
}

async fn list(State(st): State<Arc<AppState>>, a: JfAuth) -> Response {
    let c = ctx(&st);
    if a.p.can(Perms::USERS_MANAGE) {
        let users = st.identity.signin_users().await.unwrap_or_default();
        return json(Value::Array(users.iter().map(|u| dto::user_dto(&c, u, None)).collect()));
    }
    json(json!([dto::user_dto(&c, &a.p.user, None)]))
}

async fn save_config(st: &AppState, user: i64, body: &Bytes) -> Response {
    if body.len() > 64 * 1024 || serde_json::from_slice::<Value>(body).map(|v| !v.is_object()).unwrap_or(true) {
        return fail(StatusCode::BAD_REQUEST, "Bad configuration");
    }
    match st.identity.set_jf_pref(user, "configuration", String::from_utf8_lossy(body).into_owned()).await {
        Ok(()) => no_content(),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE, "Try again"),
    }
}

async fn set_configuration(State(st): State<Arc<AppState>>, a: JfAuth, q: Q, body: Bytes) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => save_config(&st, u, &body).await,
        Err(r) => r,
    }
}

async fn set_configuration_of(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    match a.check_user(Some(&id)) {
        Ok(u) => save_config(&st, u, &body).await,
        Err(r) => r,
    }
}

async fn logout(State(st): State<Arc<AppState>>, a: JfAuth) -> Response {
    let _ = st.identity.revoke_session(a.p.session_id).await;
    no_content()
}

async fn capabilities(State(st): State<Arc<AppState>>, a: JfAuth, body: Bytes) -> Response {
    let b = body_json(&body);
    if let Some(profile) = field(&b, "DeviceProfile").filter(|p| p.is_object())
        && body.len() <= 64 * 1024
    {
        let mut m = st.jellyfin.profiles.lock().unwrap_or_else(|e| e.into_inner());
        // A few devices per user, a bounded total.
        let mine: Vec<(i64, String)> = m.keys().filter(|(u, _)| *u == a.user()).cloned().collect();
        if mine.len() >= 10
            && let Some(k) = mine.into_iter().next()
        {
            m.remove(&k);
        }
        if m.len() < 2000 {
            m.insert((a.user(), a.dev.device_id.clone()), profile.clone());
        }
    }
    no_content()
}

async fn sessions(State(st): State<Arc<AppState>>, a: JfAuth, ip: ClientIp, q: Q) -> Response {
    if let Some(d) = q.get("deviceid")
        && d != a.dev.device_id
    {
        return json(json!([]));
    }
    let sid = a.p.device_id.unwrap_or(a.p.session_id);
    json(json!([dto::session_dto(&ctx(&st), &a.p.user, sid, &a.dev, &ip.0.to_string())]))
}

/// Display preference ids: `usersettings` or an item/view id (anything else isn't stored).
fn display_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn display_key(id: &str, q: &Q) -> String {
    format!("display:{}:{}", q.get("client").unwrap_or("emby").to_ascii_lowercase(), id.to_ascii_lowercase())
}

async fn display_get(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    let uid = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Some(v) = stored(&st, uid, &display_key(&id, &q)).await {
        return json(v);
    }
    json(json!({
        "Id": id, "SortBy": "SortName", "RememberIndexing": false, "PrimaryImageHeight": 250, "PrimaryImageWidth": 250,
        "CustomPrefs": {}, "ScrollDirection": "Horizontal", "ShowBackdrop": true, "RememberSorting": false,
        "SortOrder": "Ascending", "ShowSidebar": false, "Client": q.get("client").unwrap_or("emby"),
    }))
}

async fn display_set(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path(id): Path<String>,
    q: Q,
    body: Bytes,
) -> Response {
    let uid = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if !display_id_ok(&id)
        || body.len() > 64 * 1024
        || serde_json::from_slice::<Value>(&body).map(|v| !v.is_object()).unwrap_or(true)
    {
        return fail(StatusCode::BAD_REQUEST, "Bad preferences");
    }
    if st.identity.jf_pref_count(uid).await.unwrap_or(0) >= 300
        && stored(&st, uid, &display_key(&id, &q)).await.is_none()
    {
        return fail(StatusCode::BAD_REQUEST, "Too many stored preferences");
    }
    match st.identity.set_jf_pref(uid, &display_key(&id, &q), String::from_utf8_lossy(&body).into_owned()).await {
        Ok(()) => no_content(),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE, "Try again"),
    }
}
