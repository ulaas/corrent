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

use super::{ApiResult, Auth, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/mcp", post(mcp).get(no_stream).delete(end_session))
        .route("/agent/proposals", get(list))
        .route("/agent/proposals/{id}/approve", post(approve))
        .route("/agent/proposals/{id}/reject", post(reject))
}

/// One JSON-RPC message (or a batch) per request; answers are plain JSON. Notifications and
/// responses from the client get 202 with no body.
async fn mcp(State(st): State<Arc<AppState>>, auth: Auth, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    // Browsers may not drive it from other sites (DNS rebinding): only local or same-origin.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        && !(origin.starts_with("http://127.0.0.1") || origin.starts_with("http://localhost") || origin == "null")
    {
        return (StatusCode::FORBIDDEN, "cross-origin MCP requests are refused").into_response();
    }
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
