//! Metadata from provider plugins: identifying items after scans (only confident matches —
//! the rest wait for "Identify…"), applying details and artwork, and filling in episodes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use corrent_library::meta::{MetaDetails, Pending, Replace};
use corrent_plugins::{Candidate, ItemDetails, Kind, MediaKind, PluginHost, SearchQuery};
use corrent_proto::library::{ItemKind, MetadataRun, ProviderError, ProviderTest, RefreshMode, RefreshOutcome};

use crate::state::AppState;

fn media_kind(k: ItemKind) -> Option<MediaKind> {
    match k {
        // A home video is identified only by hand (it is never in the automatic list), as a
        // movie.
        ItemKind::Movie | ItemKind::Video => Some(MediaKind::Movie),
        ItemKind::Show => Some(MediaKind::Show),
        ItemKind::Album => Some(MediaKind::Album),
        ItemKind::Artist => Some(MediaKind::Artist),
        _ => None,
    }
}

fn norm(s: &str) -> String {
    corrent_library::parse::sort_title(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The candidate that is clearly the item, or none: the only result with the same title
/// (ignoring case, articles and punctuation) and the same year; else the only one a year off (a
/// release date in another country). Without a year, only a lone result with the same title.
/// Two that fit as well as each other are a question for "Identify…", never a guess.
pub fn confident<'a>(p: &Pending, found: &'a [Candidate]) -> Option<&'a Candidate> {
    let title = norm(&p.title);
    if title.is_empty() {
        return None;
    }
    let same: Vec<&Candidate> = found.iter().filter(|c| norm(&c.title) == title).collect();
    let Some(year) = p.year else {
        return match same.as_slice() {
            [one] => Some(one),
            _ => None,
        };
    };
    for off in [0, 1] {
        let near: Vec<&Candidate> =
            same.iter().copied().filter(|c| c.year.is_some_and(|y| (y as i32 - year).abs() <= off)).collect();
        match near.as_slice() {
            [one] => return Some(one),
            [] => {}
            _ => return None,
        }
    }
    // A provider that doesn't know the year: only its lone result with the title.
    match same.as_slice() {
        [one] if one.year.is_none() => Some(one),
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
    apply_with(st, host, item, kind, provider, id, language, Replace::IfMissingOrFrame).await
}

/// `apply`, with how the provider's artwork may replace the item's (`Always` for a refresh).
#[allow(clippy::too_many_arguments)]
async fn apply_with(
    st: &AppState,
    host: &Arc<PluginHost>,
    item: i64,
    kind: ItemKind,
    provider: &str,
    id: &str,
    language: &str,
    art: Replace,
) -> Result<(), String> {
    let mk = media_kind(kind).ok_or("this kind of item has no provider metadata")?;
    let d =
        host.metadata_details(provider, mk, id.to_string(), language.to_string()).await.map_err(|e| e.to_string())?;
    st.library.apply_details(item, to_meta(&d, provider, id)).await.map_err(|e| e.to_string())?;
    for a in &d.artwork {
        if let Some(bytes) = fetch_image(st, &a.url).await {
            let _ = st.library.set_artwork(item, &a.kind, bytes, &a.url, art, false).await;
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
        // Rating sites (OMDb): `rating:<source>` entries.
        let ratings: BTreeMap<String, String> = extra
            .ids
            .iter()
            .filter_map(|(k, v)| k.strip_prefix("rating:").map(|k| (k.to_string(), v.clone())))
            .collect();
        let _ = st.library.set_ratings(item, ratings).await;
        for a in extra.artwork.iter().filter(|a| !have.contains(&a.kind)) {
            if let Some(bytes) = fetch_image(st, &a.url).await {
                let _ = st.library.set_artwork(item, &a.kind, bytes, &a.url, art, false).await;
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
                    let _ =
                        st.library.set_artwork(ep_item, "thumb", bytes, &url, Replace::IfMissingOrFrame, false).await;
                }
            }
        }
    }
    st.library.mark_meta_checked(item, Some(provider.to_string())).await.map_err(|e| e.to_string())?;
    write_nfo(st, item).await;
    if kind == ItemKind::Show {
        for (season, _) in st.library.seasons(item).await.unwrap_or_default() {
            let q =
                corrent_proto::library::ItemQuery { parent_id: Some(season), limit: Some(1000), ..Default::default() };
            for ep in st.library.items(0, q).await.map(|p| p.items).unwrap_or_default() {
                write_nfo(st, ep.id).await;
            }
        }
    }
    Ok(())
}

/// Writes the item's `.nfo` when its library asks for it (movies, shows, episodes). Never
/// replaces an `.nfo` someone else wrote.
pub async fn write_nfo(st: &AppState, item: i64) {
    let Ok(d) = st.library.item(0, item).await else { return };
    let Ok(lib) = st.library.get(d.item.library_id).await else { return };
    if !lib.settings.write_nfo {
        return;
    }
    let kind = match d.item.kind {
        ItemKind::Movie => "movie",
        ItemKind::Show => "tvshow",
        ItemKind::Episode => "episodedetails",
        _ => return,
    };
    let Ok(Some(file)) = st.library.any_file(item).await else { return };
    let file = std::path::PathBuf::from(file);
    let target = if d.item.kind == ItemKind::Show {
        let roots: Vec<std::path::PathBuf> = lib.folders.iter().map(std::path::PathBuf::from).collect();
        let Some(root) = roots.iter().filter(|r| file.starts_with(r)).max_by_key(|r| r.as_os_str().len()) else {
            return;
        };
        let Some(show) = corrent_library::parse::show_home(root, &file) else { return };
        root.join(show).join("tvshow.nfo")
    } else {
        file.with_extension("nfo")
    };
    let season = match (d.item.kind, d.item.parent_id) {
        (ItemKind::Episode, Some(p)) => st.library.item(0, p).await.ok().and_then(|s| s.item.index),
        _ => None,
    };
    let people = |role: &str| d.credits.iter().filter(|c| c.role == role).map(|c| c.name.clone()).collect::<Vec<_>>();
    let n = corrent_library::nfo::Nfo {
        kind: kind.into(),
        title: Some(d.item.title.clone()),
        original_title: d.original_title.clone(),
        year: d.item.year,
        plot: d.overview.clone(),
        rating: d.rating,
        mpaa: d.official_rating.clone(),
        premiered: d.premiered.clone(),
        season,
        episode: if d.item.kind == ItemKind::Episode { d.item.index } else { None },
        runtime_min: d.item.runtime_ms.map(|r| r / 60_000).filter(|m| *m > 0),
        genres: d.genres.clone(),
        directors: people("director"),
        writers: people("writer"),
        actors: d.credits.iter().filter(|c| c.role == "actor").map(|c| (c.name.clone(), c.character.clone())).collect(),
        ids: d.provider_ids.clone(),
        ..Default::default()
    };
    let text = corrent_library::nfo::render(&n);
    let _ = tokio::task::spawn_blocking(move || {
        // Only a file we wrote (or none) is replaced; anything unreadable is left alone.
        match std::fs::read(&target) {
            Ok(old)
                if !old
                    .windows(corrent_library::nfo::MARK.len())
                    .any(|w| w == corrent_library::nfo::MARK.as_bytes()) =>
            {
                return;
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return,
        }
        // A plain replace (no backup copies left in media folders).
        let tmp = target.with_extension("nfo.corrent-tmp");
        let r = std::fs::write(&tmp, text.as_bytes()).and_then(|_| std::fs::rename(&tmp, &target));
        if let Err(e) = r {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("{}: {e}", target.display());
        }
    })
    .await;
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

/// Identifies one item automatically: the first provider with a confident match wins. An item
/// a provider identified before (perhaps by hand) gets that provider's current record by its
/// id, without a search; `again` (a refresh of everything) also replaces its artwork.
pub async fn identify(
    st: &AppState,
    host: &Arc<PluginHost>,
    p: &Pending,
    language: &str,
    again: bool,
) -> RefreshOutcome {
    let mut out = RefreshOutcome::default();
    let Some(mk) = media_kind(p.kind) else { return out };
    let providers = providers_for(host, mk).await;
    out.no_providers = providers.is_empty();
    let art = if again { Replace::Always } else { Replace::IfMissingOrFrame };
    let matched_before = p.provider.as_deref().filter(|s| !s.is_empty());
    if let Some(prov) = matched_before
        && providers.iter().any(|x| x == prov)
        && let Some(pid) = p.ids.get(prov)
    {
        if !still_same(st, p).await {
            out.skipped = true;
            return out;
        }
        match apply_with(st, host, p.id, p.kind, prov, pid, language, art).await {
            Ok(()) => {
                out.provider = Some(prov.to_string());
                return out;
            }
            Err(e) => out.errors.push(ProviderError { provider: prov.to_string(), error: e }),
        }
    }
    // Checked (not asked again on its own) only when a provider answered: without one (no key
    // yet, offline) the item is tried again later.
    let mut answered = false;
    for provider in providers {
        let q = SearchQuery {
            kind: mk,
            title: p.title.clone(),
            year: p.year.map(|y| y as u32),
            language: language.to_string(),
            ids: p.ids.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            artist: p.artist.clone(),
        };
        let found = match host.metadata_search(&provider, q).await {
            Ok(found) => found,
            Err(e) => {
                out.errors.push(ProviderError { provider, error: e.to_string() });
                continue;
            }
        };
        answered = true;
        if let Some(c) = confident(p, &found) {
            // The search took a while: a scan may have moved the item's files since it was
            // listed, or removed it and given its id to another item. Only the item that was
            // searched for gets the result.
            if !still_same(st, p).await {
                out.skipped = true;
                return out;
            }
            match apply_with(st, host, p.id, p.kind, &provider, &c.id, language, art).await {
                Ok(()) => {
                    out.provider = Some(provider);
                    return out;
                }
                Err(e) => {
                    tracing::debug!("metadata from {provider} for {}: {e}", p.title);
                    out.errors.push(ProviderError { provider, error: e });
                }
            }
        }
    }
    // An item matched before (perhaps by hand) keeps its match when a refresh finds none.
    if answered && matched_before.is_none() {
        let _ = st.library.mark_meta_checked(p.id, Some(String::new())).await;
    }
    out
}

async fn still_same(st: &AppState, p: &Pending) -> bool {
    matches!(st.library.pending_item(p.id).await, Ok(now) if same_item(&now, p))
}

/// Still the item a search was made for (its provider ids may have grown meanwhile).
fn same_item(now: &Pending, then: &Pending) -> bool {
    now.library_id == then.library_id
        && now.kind == then.kind
        && now.title == then.title
        && now.year == then.year
        && now.artist == then.artist
}

/// A provider became usable (a key entered, turned on): items no provider matched before are
/// tried again, in every library.
pub async fn retry_unmatched(st: Arc<AppState>) {
    match st.library.reset_unmatched(None).await {
        Ok(n) if n > 0 => tracing::info!("metadata: trying {n} unmatched items again"),
        Ok(_) => {}
        Err(e) => tracing::warn!("metadata: {e}"),
    }
    for l in st.library.list().await.unwrap_or_default() {
        refresh_library(st.clone(), l.id, RefreshMode::Missing, None).await;
    }
}

/// A library's shown metadata refresh and the run it belongs to.
pub struct RunEntry {
    pub run: MetadataRun,
    owner: u64,
    by_hand: bool,
}

fn next_owner() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The library's running or last metadata refresh.
pub fn run_of(st: &AppState, library_id: i64) -> Option<MetadataRun> {
    st.metadata_runs.lock().unwrap_or_else(|e| e.into_inner()).get(&library_id).map(|e| e.run.clone())
}

/// Whether a refresh asked for by hand is waiting or running in the library.
pub fn by_hand_running(st: &AppState, library_id: i64) -> Option<MetadataRun> {
    let runs = st.metadata_runs.lock().unwrap_or_else(|e| e.into_inner());
    runs.get(&library_id).filter(|e| e.by_hand && e.run.running).map(|e| e.run.clone())
}

/// Shows a run's status (and sends it to clients) when the run may: a refresh asked for by hand
/// owns the library's status from `queue` on; automatic runs never cover one.
fn publish(st: &AppState, run: &MetadataRun, owner: u64, by_hand: bool) {
    {
        let mut runs = st.metadata_runs.lock().unwrap_or_else(|e| e.into_inner());
        let may = match runs.get(&run.library_id) {
            None => true,
            Some(e) if e.owner == owner => true,
            Some(e) => !by_hand && !e.by_hand,
        };
        if !may {
            return;
        }
        runs.insert(run.library_id, RunEntry { run: run.clone(), owner, by_hand });
    }
    st.hub.publish(corrent_proto::event::Event::MetadataStatus { status: run.clone() });
}

/// A refresh asked for by hand, shown as running from now on (it may wait for another run
/// first). Returns the status and the owner to pass to `refresh_library`.
pub fn queue(st: &AppState, library_id: i64, mode: RefreshMode) -> (MetadataRun, u64) {
    let owner = next_owner();
    let run = MetadataRun { library_id, running: true, mode, started_at: now(), ..Default::default() };
    st.metadata_runs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(library_id, RunEntry { run: run.clone(), owner, by_hand: true });
    st.hub.publish(corrent_proto::event::Event::MetadataStatus { status: run.clone() });
    (run, owner)
}

/// Looks up metadata for a library's items, one at a time: those that never had it (`Missing`;
/// asked `by_hand`, also those no provider matched before), or every item again (`All`).
/// Automatic runs respect the library's "fetch metadata" setting; one asked for by hand
/// (`hand`: its owner from `queue`) runs anyway.
pub async fn refresh_library(st: Arc<AppState>, library_id: i64, mode: RefreshMode, hand: Option<u64>) {
    let by_hand = hand.is_some();
    let owner = hand.unwrap_or_else(next_owner);
    let (Some(host), Ok(lib)) = (st.plugins.clone(), st.library.get(library_id).await) else {
        if by_hand {
            let started_at = now();
            let run = MetadataRun { library_id, mode, started_at, finished_at: Some(started_at), ..Default::default() };
            publish(&st, &run, owner, true);
        }
        return;
    };
    if !lib.settings.fetch_metadata && !by_hand {
        return;
    }
    let language = if lib.settings.language.is_empty() { "en".to_string() } else { lib.settings.language.clone() };
    let _one = st.metadata_slot.lock().await;
    if by_hand && mode == RefreshMode::Missing {
        let _ = st.library.reset_unmatched(Some(library_id)).await;
    }
    // The list as it is now: items a scan adds meanwhile get the run after that scan.
    let items = match mode {
        RefreshMode::Missing => st.library.items_needing_metadata_except(library_id, &[], 1_000_000).await,
        RefreshMode::All => st.library.items_for_metadata_except(library_id, &[], 1_000_000).await,
    }
    .unwrap_or_default();
    if items.is_empty() && !by_hand {
        return;
    }
    let show = |run: &MetadataRun| publish(&st, run, owner, by_hand);
    let mut run = MetadataRun {
        library_id,
        running: true,
        mode,
        total: items.len() as u64,
        started_at: now(),
        ..Default::default()
    };
    show(&run);
    let mut shown = std::time::Instant::now();
    for p in items {
        if *st.stopping.borrow() {
            break;
        }
        let o = identify(&st, &host, &p, &language, mode == RefreshMode::All).await;
        run.done += 1;
        run.no_providers |= o.no_providers;
        if o.provider.is_some() {
            run.matched += 1;
        } else if !o.no_providers && !o.skipped && o.errors.is_empty() {
            run.unmatched += 1;
        }
        for e in o.errors {
            if !run.errors.iter().any(|x| x.provider == e.provider) {
                tracing::warn!("metadata: {} failed: {}", e.provider, e.error);
                run.errors.push(e);
            }
        }
        if shown.elapsed() > Duration::from_millis(500) {
            shown = std::time::Instant::now();
            show(&run);
        }
    }
    run.running = false;
    run.finished_at = Some(now());
    show(&run);
    if run.matched > 0 {
        tracing::info!("metadata: {} items in {} identified", run.matched, lib.name);
        st.library.emit_changed(library_id);
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// One real lookup of a well-known title (Settings → Metadata → Test): whether a provider and
/// its key work, in the provider's own words when they don't.
pub async fn test_provider(host: &Arc<PluginHost>, id: &str) -> ProviderTest {
    let mut t = ProviderTest::default();
    let Some(info) = host.list().into_iter().find(|p| p.manifest.id == id) else {
        t.error = Some("no such plugin".into());
        return t;
    };
    let ids = |list: &[(&str, &str)]| list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    let matrix = [("imdb", "tt0133093"), ("tmdb", "603")];
    if info.manifest.kinds.contains(&Kind::Subtitles) && !info.manifest.kinds.contains(&Kind::Metadata) {
        t.query = "The Matrix (1999)".into();
        let q = corrent_plugins::SubtitleQuery {
            title: "The Matrix".into(),
            year: Some(1999),
            season: None,
            episode: None,
            languages: vec!["en".into()],
            ids: ids(&matrix),
            file_hash: None,
            file_size: None,
        };
        match host.subtitles_search(id, q).await {
            Ok(found) => {
                t.results = found.len() as u32;
                t.found = found.first().map(|s| s.name.clone());
                t.ok = !found.is_empty();
            }
            Err(e) => t.error = Some(e.to_string()),
        }
        return t;
    }
    let kinds = match host.metadata_kinds(id).await {
        Ok(k) => k,
        Err(e) => {
            t.error = Some(e.to_string());
            return t;
        }
    };
    let (kind, title, year, known, artist) = if kinds.contains(&MediaKind::Movie) {
        (MediaKind::Movie, "The Matrix", Some(1999), ids(&matrix), None)
    } else if kinds.contains(&MediaKind::Show) {
        let bb = [("tvdb", "81189"), ("imdb", "tt0903747"), ("tmdb", "1396")];
        (MediaKind::Show, "Breaking Bad", Some(2008), ids(&bb), None)
    } else if kinds.contains(&MediaKind::Album) {
        (MediaKind::Album, "Abbey Road", Some(1969), Vec::new(), Some("The Beatles".to_string()))
    } else {
        (MediaKind::Artist, "The Beatles", None, Vec::new(), None)
    };
    t.query = match year {
        Some(y) => format!("{title} ({y})"),
        None => title.to_string(),
    };
    let q = SearchQuery { kind, title: title.into(), year, language: "en".into(), ids: known, artist };
    let found = match host.metadata_search(id, q).await {
        Ok(f) => f,
        Err(e) => {
            t.error = Some(e.to_string());
            return t;
        }
    };
    t.results = found.len() as u32;
    let Some(first) = found.first() else { return t };
    // Some providers (fanart.tv, OMDb) only reach their service for the details.
    match host.metadata_details(id, kind, first.id.clone(), "en".into()).await {
        Ok(d) => {
            t.ok = true;
            t.found = Some(d.title.clone().unwrap_or_else(|| first.title.clone()));
            t.year = d.year.or(first.year);
            t.artwork = d.artwork.len() as u32;
            t.credits = d.credits.len() as u32;
        }
        Err(e) => t.error = Some(e.to_string()),
    }
    t
}

/// After every finished scan, look for metadata.
pub fn start(st: Arc<AppState>) {
    // Videos added before thumbnails existed (or while the server was off) get theirs now.
    crate::thumbs::start(&st);
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
                    tokio::spawn(refresh_library(st.clone(), s.library_id, RefreshMode::Missing, None));
                    crate::thumbs::start(&st);
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
            provider: None,
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
        // The exact year wins over one a year off; two a year off (or two exact) are a question.
        let close = [c("Heat", Some(1995)), c("Heat", Some(1996))];
        assert_eq!(confident(&p("Heat", Some(1995)), &close).map(|c| c.year), Some(Some(1995)));
        assert!(confident(&p("Heat", Some(1994)), &[c("Heat", Some(1993)), c("Heat", Some(1995))]).is_none());
        assert!(confident(&p("Heat", Some(1995)), &[c("Heat", Some(1995)), c("Heat", Some(1995))]).is_none());
        assert_eq!(confident(&p("Heat", Some(1996)), &[c("Heat", Some(1995))]).map(|c| c.year), Some(Some(1995)));
        // Results without a year count only alone.
        assert!(confident(&p("Heat", Some(1995)), &[c("Heat", None)]).is_some());
        assert!(confident(&p("Heat", Some(1995)), &[c("Heat", None), c("Heat", None)]).is_none());
        assert!(confident(&p("", None), &[c("", None)]).is_none(), "an empty title matches nothing");
    }
}
