//! Paired devices (native and TV clients) and their tokens.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use corrent_proto::auth::{Device, Perms};
use serde::Deserialize;

use super::{ApiResult, Auth, ClientIp};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/devices", get(list)).route("/devices/{id}", delete(remove))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListQuery {
    /// Admins: every user's devices.
    all: bool,
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth, Query(q): Query<ListQuery>) -> ApiResult<Vec<Device>> {
    let owner = if q.all {
        auth.require(Perms::USERS_MANAGE)?;
        None
    } else {
        Some(auth.0.user.id)
    };
    let mut v = st.identity.devices(owner).await?;
    for d in &mut v {
        d.current = Some(d.id) == auth.0.device_id;
    }
    Ok(Json(v))
}

async fn remove(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    let owner = if auth.0.can(Perms::USERS_MANAGE) { None } else { Some(auth.0.user.id) };
    let r = st.identity.delete_device(id, owner).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "device_delete", &id.to_string(), r.is_ok(), "").await;
    r?;
    Ok(Json(serde_json::json!({})))
}
