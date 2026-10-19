//! User management (admins) and self-service profile changes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use corrent_proto::ErrorCode;
use corrent_proto::auth::{NewUser, Perms, User, UserUpdate};

use super::{ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/{id}", get(one).patch(update).delete(remove))
        .route("/users/{id}/pin", axum::routing::put(set_pin))
}

/// Admins set or remove any profile's PIN (e.g. for the kids' profiles on the TV).
async fn set_pin(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(req): Json<corrent_proto::auth::SetPin>,
) -> ApiResult<serde_json::Value> {
    if id != auth.0.user.id {
        auth.require(Perms::USERS_MANAGE)?;
    }
    st.identity.set_pin(id, req.pin).await?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "user_pin", &id.to_string(), true, "").await;
    Ok(Json(serde_json::json!({})))
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<User>> {
    auth.require(Perms::USERS_MANAGE)?;
    Ok(Json(st.identity.users().await?))
}

async fn create(State(st): State<Arc<AppState>>, auth: Auth, ip: ClientIp, Json(u): Json<NewUser>) -> ApiResult<User> {
    auth.require(Perms::USERS_MANAGE)?;
    let r = st.identity.create_user(u).await;
    let target = r.as_ref().map(|u| u.name.clone()).unwrap_or_default();
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "user_create", &target, r.is_ok(), "").await;
    Ok(Json(r?))
}

async fn one(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<User> {
    if id != auth.0.user.id {
        auth.require(Perms::USERS_MANAGE)?;
    }
    Ok(Json(st.identity.user(id).await?))
}

/// Admins change anything; everyone else only their own display name and password.
async fn update(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(u): Json<UserUpdate>,
) -> ApiResult<User> {
    let admin = auth.0.can(Perms::USERS_MANAGE);
    if !admin && (id != auth.0.user.id || u.role.is_some() || u.perms.is_some() || u.disabled.is_some()) {
        return Err(err(ErrorCode::Forbidden, "permission denied"));
    }
    // Changing one's own password needs the current one (a stolen session can't lock the
    // owner out); admins resetting someone else's don't.
    if u.password.is_some() && (id == auth.0.user.id || !admin) {
        let current = u.current_password.clone().unwrap_or_default();
        if !st.identity.check_password(id, &current).await? {
            return Err(err(ErrorCode::InvalidCredentials, "current password is wrong"));
        }
    }
    let what = [
        u.display_name.as_ref().map(|_| "display_name"),
        u.password.as_ref().map(|_| "password"),
        u.role.map(|_| "role"),
        u.perms.map(|_| "perms"),
        u.disabled.map(|_| "disabled"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    let r = st.identity.update_user(id, u).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "user_update", &id.to_string(), r.is_ok(), &what).await;
    Ok(Json(r?))
}

async fn remove(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::USERS_MANAGE)?;
    let r = st.identity.delete_user(id).await;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "user_delete", &id.to_string(), r.is_ok(), "").await;
    r?;
    Ok(Json(serde_json::json!({})))
}
