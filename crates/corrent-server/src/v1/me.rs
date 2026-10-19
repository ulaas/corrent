//! Per-user client preferences (theme, home layout, card size…) shared by a user's devices,
//! and the server's default theme for clients that have none yet.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use corrent_core::config::{Accent, PalettePreset};
use corrent_db::rusqlite::{OptionalExtension, params};
use corrent_proto::ErrorCode;

use super::{ApiResult, Auth, err};
use crate::state::AppState;

const MAX_PREFS: usize = 64 * 1024;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/me/prefs", get(prefs).put(set_prefs).layer(axum::extract::DefaultBodyLimit::max(MAX_PREFS)))
        .route("/theme", get(theme))
}

/// The stored document (`{}` until a client saves one). Clients own its shape; the server only
/// keeps it.
async fn prefs(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<serde_json::Value> {
    let uid = auth.0.user.id;
    let json: Option<String> = st
        .identity
        .db()
        .read(move |c| c.query_row("SELECT json FROM user_prefs WHERE user_id = ?1", [uid], |r| r.get(0)).optional())
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    Ok(Json(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_else(|| serde_json::json!({}))))
}

async fn set_prefs(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(v): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    if !v.is_object() {
        return Err(err(ErrorCode::BadRequest, "preferences are a JSON object"));
    }
    let text = v.to_string();
    if text.len() > MAX_PREFS {
        return Err(err(ErrorCode::PayloadTooLarge, "preferences too large"));
    }
    let (uid, now) = (auth.0.user.id, corrent_core::util::now_unix());
    st.identity
        .db()
        .write(move |t| {
            t.execute(
                "INSERT INTO user_prefs(user_id, json, updated_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(user_id) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
                params![uid, text, now],
            )
            .map(drop)
        })
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    Ok(Json(v))
}

/// The server's look (its owner's appearance settings): the default for clients.
async fn theme(State(st): State<Arc<AppState>>, _auth: Auth) -> ApiResult<serde_json::Value> {
    let a = st.settings.load().appearance.clone();
    let preset = match a.preset {
        PalettePreset::Corrent | PalettePreset::Custom => "corrent",
        PalettePreset::Nord => "nord",
        PalettePreset::Solarized => "solarized",
        PalettePreset::HighContrast => "high_contrast",
    };
    let accent = match a.accent {
        Accent::Blue => "blue",
        Accent::Violet => "violet",
        Accent::Teal => "teal",
        Accent::Green => "green",
        Accent::Orange => "orange",
        Accent::Pink => "pink",
        Accent::Custom => "custom",
    };
    let [r, g, b] = a.custom_accent;
    Ok(Json(serde_json::json!({
        "preset": preset,
        "accent": accent,
        "custom_accent": format!("#{r:02x}{g:02x}{b:02x}"),
    })))
}
