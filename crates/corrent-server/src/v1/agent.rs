//! AI agents: the MCP endpoint (Streamable HTTP, JSON answers) and the proposals they make.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::agent::Proposal;
use corrent_proto::auth::Perms;
use serde::Deserialize;
use serde_json::Value;

use super::{ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/mcp", post(mcp).get(no_stream).delete(end_session))
        .route("/agent/proposals", get(list))
        .route("/agent/proposals/{id}/approve", post(approve))
        .route("/agent/proposals/{id}/reject", post(reject))
}

/// Who an MCP request acts for: the desktop app's agent key (library rights only), or an
/// account's own token (bearer only: a browser's cookie never drives an agent endpoint).
async fn mcp_principal(st: &AppState, headers: &HeaderMap, ip: &str) -> Option<crate::identity::Principal> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)?;
    let key = st.agent_key.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some((secret, user)) = key
        && aws_lc_rs::constant_time::verify_slices_are_equal(secret.as_bytes(), bearer.as_bytes()).is_ok()
    {
        let mut u = st.identity.user(user).await.ok()?;
        // Proposals only (a person approves each one), and only through this endpoint: the
        // library and every download may be looked at and proposed for.
        u.perms = Perms::LIBRARY_MANAGE | Perms::TORRENTS_MANAGE_ALL;
        return Some(crate::identity::Principal {
            user: u,
            session_id: 0,
            device_id: None,
            kind: crate::identity::SessionKind::Device,
            csrf: String::new(),
        });
    }
    st.identity.verify_token(bearer, ip).await.ok().flatten()
}

/// Browsers may not drive it from other sites (DNS rebinding): no Origin, or a loopback one.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]")
}

/// One JSON-RPC message (or a batch) per request; answers are plain JSON. Notifications and
/// responses from the client get 202 with no body.
async fn mcp(State(st): State<Arc<AppState>>, ip: ClientIp, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if !origin_ok(&headers) {
        return (StatusCode::FORBIDDEN, "cross-origin MCP requests are refused").into_response();
    }
    let Some(who) = mcp_principal(&st, &headers, &ip.0.to_string()).await else {
        return (StatusCode::UNAUTHORIZED, "not signed in").into_response();
    };
    let auth = Auth(who);
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        let e =
            serde_json::json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } });
        return (StatusCode::BAD_REQUEST, Json(e)).into_response();
    };
    let answers: Vec<Value> = match &msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch {
                if let Some(a) = crate::agent::handle(&st, &auth.0, m).await {
                    out.push(a);
                }
            }
            out
        }
        m => crate::agent::handle(&st, &auth.0, m).await.into_iter().collect(),
    };
    let is_init = msg.get("method").and_then(Value::as_str) == Some("initialize");
    let mut res = match (&msg, answers.as_slice()) {
        (_, []) => StatusCode::ACCEPTED.into_response(),
        (Value::Array(_), _) => Json(Value::Array(answers.clone())).into_response(),
        (_, [one]) => Json(one.clone()).into_response(),
        (_, many) => Json(Value::Array(many.to_vec())).into_response(),
    };
    if is_init {
        // Sessions carry no state here; the id only satisfies clients that expect one.
        let id = crate::util::random_hex(16);
        if let Ok(v) = HeaderValue::from_str(&id) {
            res.headers_mut().insert("mcp-session-id", v);
        }
    }
    res
}

/// No server-initiated stream: everything is request and answer.
async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn end_session() -> StatusCode {
    StatusCode::OK
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListQuery {
    pending: bool,
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth, Query(q): Query<ListQuery>) -> ApiResult<Vec<Proposal>> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    Ok(Json(crate::agent::proposals(&st, q.pending).await.map_err(|e| err(ErrorCode::Internal, e.to_string()))?))
}

async fn approve(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Proposal> {
    decide(&st, &auth, id, true).await
}

async fn reject(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Proposal> {
    decide(&st, &auth, id, false).await
}

async fn decide(st: &Arc<AppState>, auth: &Auth, id: i64, approve: bool) -> ApiResult<Proposal> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let p = crate::agent::decide(st, &auth.0, id, approve).await.map_err(|e| err(ErrorCode::Conflict, e))?;
    st.identity
        .audit(
            Some(auth.name()),
            "",
            if approve { "agent_approve" } else { "agent_reject" },
            &id.to_string(),
            true,
            &p.summary,
        )
        .await;
    Ok(Json(p))
}
