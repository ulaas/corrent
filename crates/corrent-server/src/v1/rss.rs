//! RSS feeds: articles, refresh, feed/rule configuration and manual downloads.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use corrent_core::engine::types::AddSource;
use corrent_core::engine::{AddRequest, EngineCommand};
use corrent_core::rss::{FeedView, RssSettings, RssView};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use serde::{Deserialize, Serialize};

use super::server::{mask_rss_settings, mask_url};
use super::{ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rss", get(view))
        .route("/rss/config", get(config).put(set_config).layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)))
        .route("/rss/refresh", post(refresh))
        .route("/rss/download", post(download))
}

/// A feed's stable id: masked URLs aren't unique when two feeds share a host.
fn feed_id(url: &str) -> String {
    let h = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, url.as_bytes());
    h.as_ref().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `RssView` with each feed's id.
#[derive(Serialize)]
struct RssOut {
    feeds: Vec<FeedOut>,
}

#[derive(Serialize)]
struct FeedOut {
    id: String,
    #[serde(flatten)]
    feed: FeedView,
}

/// A feed as someone sees it who may not change the settings: private trackers put the account's
/// passkey into feed and download links, and fetch errors quote the feed's URL.
fn mask_feed(mut f: FeedView) -> FeedView {
    let masked = mask_url(&f.url);
    f.error = f.error.map(|e| e.replace(&f.url, &masked));
    f.url = masked;
    for a in &mut f.articles {
        a.article.torrent = a.article.torrent.as_deref().map(mask_url);
        // Some trackers put the passkey into every item's page link as well.
        a.article.page = a.article.page.as_deref().map(mask_url);
    }
    f
}

async fn view(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<RssOut> {
    let all = auth.0.can(Perms::SETTINGS_WRITE);
    let feeds = st
        .engine
        .snapshot()
        .rss
        .feeds
        .iter()
        .map(|f| FeedOut { id: feed_id(&f.url), feed: if all { f.clone() } else { mask_feed(f.clone()) } })
        .collect();
    Ok(Json(RssOut { feeds }))
}

async fn config(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<RssSettings> {
    auth.require(Perms::SETTINGS_READ)?;
    let mut rss = st.settings.load().rss.clone();
    if !auth.0.can(Perms::SETTINGS_WRITE) {
        mask_rss_settings(&mut rss);
    }
    Ok(Json(rss))
}

/// The feeds a client's `feed` value names: its id, its URL, or its masked URL (the only form
/// clients without `settings.write` have).
fn feeds_named<'a>(view: &'a RssView, name: &'a str) -> impl Iterator<Item = &'a FeedView> {
    view.feeds.iter().filter(move |f| f.url == name || feed_id(&f.url) == name || mask_url(&f.url) == name)
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
    match body.and_then(|b| b.0.url) {
        Some(name) => {
            let snap = st.engine.snapshot();
            for f in feeds_named(&snap.rss, &name) {
                st.engine.send(EngineCommand::RssRefresh(Some(f.url.clone())));
            }
        }
        None => {
            st.engine.send(EngineCommand::RssRefresh(None));
        }
    }
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
    let (feed, article) = feeds_named(&snap.rss, &r.feed)
        .find_map(|f| f.articles.iter().find(|a| a.article.guid == r.guid).map(|a| (f, a)))
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
    st.engine.send(EngineCommand::RssMarkDownloaded { feed: feed.url.clone(), guid: r.guid });
    Ok(Json(serde_json::json!({})))
}
