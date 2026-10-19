//! RSS feeds: articles, refresh, feed/rule configuration and manual downloads.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_core::engine::types::AddSource;
use corrent_core::engine::{AddRequest, EngineCommand};
use corrent_core::rss::{RssSettings, RssView};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use serde::Deserialize;

use super::{ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rss", get(view))
        .route("/rss/config", get(config).put(set_config).layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)))
        .route("/rss/refresh", post(refresh))
        .route("/rss/download", post(download))
}

async fn view(State(st): State<Arc<AppState>>, _auth: Auth) -> ApiResult<RssView> {
    Ok(Json((*st.engine.snapshot().rss).clone()))
}

async fn config(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<RssSettings> {
    auth.require(Perms::SETTINGS_READ)?;
    Ok(Json(st.settings.load().rss.clone()))
}

async fn set_config(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(rss): Json<RssSettings>,
) -> ApiResult<RssSettings> {
    auth.require(Perms::SETTINGS_WRITE)?;
    for r in &rss.rules {
        corrent_core::rss::rules::Matcher::new(r).map_err(|e| err(ErrorCode::BadRequest, e))?;
    }
    let saved = super::server::update_settings(&st, |old| {
        let mut s = old.clone();
        s.rss = rss;
        s
    })
    .await?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "rss_config", "", true, "").await;
    Ok(Json(saved.rss))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Refresh {
    url: Option<String>,
}

async fn refresh(
    State(st): State<Arc<AppState>>,
    _auth: Auth,
    body: Option<Json<Refresh>>,
) -> ApiResult<serde_json::Value> {
    st.engine.send(EngineCommand::RssRefresh(body.and_then(|b| b.0.url)));
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
struct DownloadRequest {
    feed: String,
    guid: String,
}

/// Adds an article's torrent by hand (rules skip it from then on).
async fn download(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Json(r): Json<DownloadRequest>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::TORRENTS_ADD)?;
    let snap = st.engine.snapshot();
    let article = snap
        .rss
        .feeds
        .iter()
        .filter(|f| f.url == r.feed)
        .flat_map(|f| f.articles.iter())
        .find(|a| a.article.guid == r.guid)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such article"))?;
    let link =
        article.article.torrent.clone().ok_or_else(|| err(ErrorCode::BadRequest, "the article has no torrent"))?;
    let source = if link.starts_with("magnet:") { AddSource::Magnet(link) } else { AddSource::Url(link) };
    let settings = st.settings.load_full();
    st.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source,
        torrent_bytes: None,
        save_path: settings.downloads.save_path.clone(),
        create_subfolder: settings.downloads.create_subfolder,
        category: None,
        tags: Vec::new(),
        paused: settings.downloads.start_paused,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: Some(auth.name().to_string()),
    })));
    st.engine.send(EngineCommand::RssMarkDownloaded { feed: r.feed, guid: r.guid });
    Ok(Json(serde_json::json!({})))
}
