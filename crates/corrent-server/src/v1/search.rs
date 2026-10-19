//! Torznab search across the providers configured in the settings.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use corrent_core::search::SearchResult;
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use serde::{Deserialize, Serialize};

use super::{ApiResult, Auth, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/search", get(search))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    /// Providers that failed, with the reason.
    pub errors: Vec<(String, String)>,
}

async fn search(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Query(q): Query<SearchQuery>,
) -> ApiResult<SearchResponse> {
    auth.require(Perms::TORRENTS_ADD)?;
    let query = q.q.trim().to_string();
    if query.is_empty() || query.len() > 200 {
        return Err(err(ErrorCode::BadRequest, "search text required"));
    }
    let mut out = search_all(&st, &query).await;
    out.results.sort_by_key(|r| std::cmp::Reverse(r.seeders.unwrap_or(0)));
    Ok(Json(out))
}

/// Searches every enabled provider at once (30 s each).
pub async fn search_all(st: &AppState, query: &str) -> SearchResponse {
    let providers: Vec<_> = st.settings.load().search.providers.iter().filter(|p| p.enabled).cloned().collect();
    let tasks = providers.into_iter().map(|p| {
        let (client, query) = (st.http.clone(), query.to_string());
        tokio::spawn(async move {
            let r = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                corrent_core::search::search(&client, &p, &query),
            )
            .await;
            (p.name.clone(), r.unwrap_or_else(|_| Err("timed out".into())))
        })
    });
    let mut out = SearchResponse::default();
    for t in tasks {
        match t.await {
            Ok((_, Ok(mut r))) => out.results.append(&mut r),
            Ok((name, Err(e))) => out.errors.push((name, e)),
            Err(e) => out.errors.push((String::new(), e.to_string())),
        }
    }
    out
}
