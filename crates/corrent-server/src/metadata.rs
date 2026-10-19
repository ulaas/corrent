//! Metadata from provider plugins: identifying items after scans (only confident matches —
//! the rest wait for "Identify…"), applying details and artwork, and filling in episodes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use corrent_library::meta::{MetaDetails, Pending};
use corrent_plugins::{Candidate, ItemDetails, Kind, MediaKind, PluginHost, SearchQuery};
use corrent_proto::library::ItemKind;

use crate::state::AppState;

fn media_kind(k: ItemKind) -> Option<MediaKind> {
    match k {
        ItemKind::Movie => Some(MediaKind::Movie),
        ItemKind::Show => Some(MediaKind::Show),
        ItemKind::Album => Some(MediaKind::Album),
        ItemKind::Artist => Some(MediaKind::Artist),
        _ => None,
    }
}

fn norm(s: &str) -> String {
    corrent_library::parse::sort_title(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The candidate that is clearly the item: same title (ignoring case, articles and punctuation)
/// and the same year (±1) when both are known; a lone result with the same title also counts.
pub fn confident<'a>(p: &Pending, found: &'a [Candidate]) -> Option<&'a Candidate> {
    let title = norm(&p.title);
    let same: Vec<&Candidate> = found.iter().filter(|c| norm(&c.title) == title).collect();
    let year_ok = |c: &&Candidate| match (p.year, c.year) {
        (Some(a), Some(b)) => (a - b as i32).abs() <= 1,
        _ => true,
    };
    match same.iter().filter(|c| year_ok(c)).collect::<Vec<_>>().as_slice() {
        [one] => Some(one),
        [first, ..] if p.year.is_some() => Some(first),
        _ => None,
    }
}

/// Providers ready for a kind, in their configured order (supplements excluded: they don't
/// identify items).
pub(crate) async fn providers_for(host: &Arc<PluginHost>, kind: MediaKind) -> Vec<String> {
    let supplements: Vec<String> =
        host.list().into_iter().filter(|p| p.manifest.supplements).map(|p| p.manifest.id).collect();
    let mut out = Vec::new();
    for id in host.ready(Kind::Metadata).into_iter().filter(|id| !supplements.contains(id)) {
        if host.metadata_kinds(&id).await.is_ok_and(|k| k.contains(&kind)) {
            out.push(id);
        }
    }
    out
}

fn to_meta(d: &ItemDetails, provider: &str, provider_id: &str) -> MetaDetails {
    let mut ids: BTreeMap<String, String> = d.ids.iter().cloned().collect();
    ids.entry(provider.to_string()).or_insert_with(|| provider_id.to_string());
    MetaDetails {
        title: d.title.clone(),
        original_title: d.original_title.clone(),
        overview: d.overview.clone(),
        year: d.year.map(|y| y as i32),
        premiered: d.premiered.clone(),
        runtime_minutes: d.runtime_minutes,
        rating: d.rating,
        official_rating: d.official_rating.clone(),
        genres: d.genres.clone(),
        credits: d.credits.iter().map(|c| (c.name.clone(), c.role.clone(), c.character.clone())).collect(),
        ids,
        collection: d.collection.clone(),
    }
}

/// Downloads an image URL for the library (public addresses only, 20 MiB cap).
async fn fetch_image(st: &AppState, url: &str) -> Option<Vec<u8>> {
    crate::net::check_public_url(url).await.ok()?;
    let res = st.http.get(url).timeout(Duration::from_secs(30)).send().await.ok()?;
    if !res.status().is_success() || res.content_length().is_some_and(|l| l > 20 * 1024 * 1024) {
        return None;
    }
    let b = res.bytes().await.ok()?;
    (b.len() <= 20 * 1024 * 1024).then(|| b.to_vec())
}

/// Applies one provider's record to an item: details, artwork, and for shows the episodes.
pub async fn apply(
    st: &AppState,
    host: &Arc<PluginHost>,
    item: i64,
    kind: ItemKind,
    provider: &str,
    id: &str,
    language: &str,
) -> Result<(), String> {
    let mk = media_kind(kind).ok_or("this kind of item has no provider metadata")?;
    let d =
        host.metadata_details(provider, mk, id.to_string(), language.to_string()).await.map_err(|e| e.to_string())?;
    st.library.apply_details(item, to_meta(&d, provider, id)).await.map_err(|e| e.to_string())?;
    for a in &d.artwork {
        if let Some(bytes) = fetch_image(st, &a.url).await {
            let _ = st.library.set_artwork(item, &a.kind, bytes, &a.url, true).await;
        }
    }
    // Supplementing providers (fanart.tv) add the kinds of artwork the main one lacks.
    let have: Vec<String> = d.artwork.iter().map(|a| a.kind.clone()).collect();
    let mut ids: Vec<(String, String)> = d.ids.clone();
    ids.push((provider.to_string(), id.to_string()));
    for sp in host.list().into_iter().filter(|p| p.manifest.supplements && p.enabled && p.missing.is_empty()) {
        let sid = &sp.manifest.id;
        if !host.metadata_kinds(sid).await.is_ok_and(|k| k.contains(&mk)) {
            continue;
        }
        let q = SearchQuery {
            kind: mk,
            title: d.title.clone().unwrap_or_default(),
            year: d.year,
            language: language.to_string(),
            ids: ids.clone(),
            artist: None,
        };
        let Some(c) = host.metadata_search(sid, q).await.ok().and_then(|c| c.into_iter().next()) else { continue };
        let Ok(extra) = host.metadata_details(sid, mk, c.id, language.to_string()).await else { continue };
        for a in extra.artwork.iter().filter(|a| !have.contains(&a.kind)) {
            if let Some(bytes) = fetch_image(st, &a.url).await {
                let _ = st.library.set_artwork(item, &a.kind, bytes, &a.url, true).await;
            }
        }
    }
    if kind == ItemKind::Show {
        for (season_item, n) in st.library.seasons(item).await.unwrap_or_default() {
            let Ok(eps) = host.metadata_episodes(provider, id.to_string(), n as u32, language.to_string()).await else {
                continue;
            };
            let list =
                eps.iter().map(|e| (e.episode as i64, e.title.clone(), e.overview.clone(), e.aired.clone())).collect();
            let touched = st.library.apply_episodes(season_item, list).await.unwrap_or_default();
            for (num, ep_item) in touched {
                if let Some(url) = eps.iter().find(|e| e.episode as i64 == num).and_then(|e| e.still_url.clone())
                    && let Some(bytes) = fetch_image(st, &url).await
                {
                    let _ = st.library.set_artwork(ep_item, "thumb", bytes, &url, true).await;
                }
            }
        }
    }
    st.library.mark_meta_checked(item, Some(provider.to_string())).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Searches every ready provider for an item (the "Identify…" list).
pub async fn candidates(
    host: &Arc<PluginHost>,
    p: &Pending,
    title: Option<String>,
    year: Option<i32>,
    language: &str,
) -> Vec<(String, Candidate)> {
    let Some(mk) = media_kind(p.kind) else { return Vec::new() };
    let mut out = Vec::new();
    for provider in providers_for(host, mk).await {
        let q = SearchQuery {
            kind: mk,
            title: title.clone().unwrap_or_else(|| p.title.clone()),
            year: year.or(p.year).map(|y| y as u32),
            language: language.to_string(),
            ids: p.ids.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            artist: p.artist.clone(),
        };
        match host.metadata_search(&provider, q).await {
            Ok(found) => out.extend(found.into_iter().map(|c| (provider.clone(), c))),
            Err(e) => tracing::debug!("metadata search with {provider}: {e}"),
        }
    }
    out
}

/// Identifies one item automatically: the first provider with a confident match wins.
pub async fn identify(st: &AppState, host: &Arc<PluginHost>, p: &Pending, language: &str) -> bool {
    let Some(mk) = media_kind(p.kind) else { return false };
    for provider in providers_for(host, mk).await {
        let q = SearchQuery {
            kind: mk,
            title: p.title.clone(),
            year: p.year.map(|y| y as u32),
            language: language.to_string(),
            ids: p.ids.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            artist: p.artist.clone(),
        };
        let Ok(found) = host.metadata_search(&provider, q).await else { continue };
        if let Some(c) = confident(p, &found) {
            match apply(st, host, p.id, p.kind, &provider, &c.id, language).await {
                Ok(()) => return true,
                Err(e) => tracing::debug!("metadata from {provider} for {}: {e}", p.title),
            }
        }
    }
    let _ = st.library.mark_meta_checked(p.id, Some(String::new())).await;
    false
}

/// Fetches metadata for a library's items that never had it (one at a time).
pub async fn refresh_library(st: Arc<AppState>, library_id: i64) {
    let Some(host) = st.plugins.clone() else { return };
    let Ok(lib) = st.library.get(library_id).await else { return };
    if !lib.settings.fetch_metadata {
        return;
    }
    let language = if lib.settings.language.is_empty() { "en".to_string() } else { lib.settings.language.clone() };
    let _one = st.metadata_slot.lock().await;
    let mut done = 0;
    loop {
        let batch = st.library.items_needing_metadata(library_id, 20).await.unwrap_or_default();
        if batch.is_empty() {
            break;
        }
        for p in batch {
            if identify(&st, &host, &p, &language).await {
                done += 1;
            }
        }
    }
    if done > 0 {
        tracing::info!("metadata: {done} items in {} identified", lib.name);
        st.library.emit_changed(library_id);
    }
}

/// After every finished scan, look for metadata.
pub fn start(st: Arc<AppState>) {
    let mut rx = st.library.subscribe();
    tokio::spawn(async move {
        let stopped = st.stopped();
        tokio::pin!(stopped);
        loop {
            let ev = tokio::select! {
                ev = rx.recv() => ev,
                _ = &mut stopped => break,
            };
            match ev {
                Ok(corrent_library::LibraryEvent::Scan(s)) if !s.running && s.error.is_none() => {
                    tokio::spawn(refresh_library(st.clone(), s.library_id));
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(title: &str, year: Option<u32>) -> Candidate {
        Candidate { id: title.into(), title: title.into(), year, overview: None, poster_url: None }
    }

    fn p(title: &str, year: Option<i32>) -> Pending {
        Pending {
            id: 1,
            library_id: 1,
            kind: ItemKind::Movie,
            title: title.into(),
            year,
            ids: Default::default(),
            artist: None,
        }
    }

    #[test]
    fn confidence() {
        let found = [c("The Matrix", Some(1999)), c("The Matrix Reloaded", Some(2003))];
        assert_eq!(confident(&p("Matrix", Some(1999)), &found).map(|c| c.id.as_str()), Some("The Matrix"));
        assert!(confident(&p("The Matrix", Some(2010)), &found).is_none(), "wrong year");
        let remakes = [c("Dune", Some(1984)), c("Dune", Some(2021))];
        assert!(confident(&p("Dune", None), &remakes).is_none(), "ambiguous without a year");
        assert_eq!(confident(&p("Dune", Some(2021)), &remakes).map(|c| c.year), Some(Some(2021)));
    }
}
