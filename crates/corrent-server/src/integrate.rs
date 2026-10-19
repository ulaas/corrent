//! Torrents and the library working together: completed downloads import into libraries,
//! library seeding goals, grabbing releases for library items and the wanted list, the
//! scheduler that searches for wanted things, and playing files that are still downloading.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use corrent_core::engine::types::AddSource;
use corrent_core::engine::{AddRequest, EngineCommand, EngineEvent};
use corrent_core::store::CategoryInfo;
use corrent_library::wanted::{self, Candidate};
use corrent_proto::library::{ImportMode, Library as LibraryInfo, Release, Wanted};
use corrent_proto::{ErrorCode, InfoHash};

use crate::state::AppState;
use crate::v1::{ApiErr, err};

/// Forwards engine events to clients and acts on them (import, seeding goals, wanted).
pub fn forward_engine_events(rx: std::sync::mpsc::Receiver<EngineEvent>, st: Arc<AppState>) {
    let rt = tokio::runtime::Handle::current();
    let _ = std::thread::Builder::new().name("corrent-events".into()).spawn(move || {
        while let Ok(ev) = rx.recv() {
            on_engine_event(&st, &rt, ev);
        }
    });
}

/// Handles one engine event (for hosts that read the engine's events themselves, like the
/// desktop app hosting the server in-process).
pub fn on_engine_event(st: &Arc<AppState>, rt: &tokio::runtime::Handle, ev: EngineEvent) {
    if let EngineEvent::PreviewReady { req, .. } = &ev
        && *req >= crate::v1::torrents::PREVIEW_IDS
    {
        let waiting = st.previews.lock().unwrap_or_else(|e| e.into_inner()).remove(req);
        if let (Some(tx), EngineEvent::PreviewReady { result, .. }) = (waiting, ev) {
            let _ = tx.send(result);
        }
        return;
    }
    match &ev {
        EngineEvent::Completed { hash, category, content_path, .. } => {
            rt.spawn(on_completed(st.clone(), *hash, category.clone(), content_path.clone()));
        }
        EngineEvent::Added { hash, .. } => {
            rt.spawn(on_added(st.clone(), *hash));
        }
        _ => {}
    }
    st.hub.engine_event(ev);
}

async fn on_completed(st: Arc<AppState>, hash: InfoHash, category: Option<String>, content: PathBuf) {
    if let Ok(n) = st.library.complete_torrent(&hash.to_hex()).await
        && n > 0
    {
        tracing::info!("wanted: {n} entr{} complete", if n == 1 { "y is" } else { "ies are" });
    }
    let Some(cat) = category else { return };
    let libs = st.library.libraries_for_category(&cat).await.unwrap_or_default();
    for lib in libs {
        if let Err(e) = import(&st, &lib, &content).await {
            tracing::warn!("importing {} into {}: {e}", content.display(), lib.name);
        }
    }
}

/// Puts a finished download into a library: in place (the library's folders already contain
/// the download folder), or hard-linked/copied into the library's first folder. Then rescans.
pub async fn import(st: &AppState, lib: &LibraryInfo, content: &Path) -> Result<(), String> {
    let inside = lib.folders.iter().any(|f| content.starts_with(f));
    match lib.settings.import_mode {
        ImportMode::InPlace if !inside => {
            return Err(format!(
                "{} isn't inside the library's folders (set the category's folder to one of them)",
                content.display()
            ));
        }
        ImportMode::InPlace => {}
        mode @ (ImportMode::Hardlink | ImportMode::Copy) => {
            let dest = lib.folders.first().ok_or("the library has no folder")?;
            let (src, dest, link) = (content.to_path_buf(), PathBuf::from(dest), mode == ImportMode::Hardlink);
            let n = tokio::task::spawn_blocking(move || place_files(&src, &dest, link))
                .await
                .map_err(|e| e.to_string())??;
            tracing::info!("imported {n} files from {} into {}", content.display(), lib.name);
        }
    }
    st.library.start_scan(lib.id);
    Ok(())
}

/// Hard-links (falling back to copying across volumes) or copies `src` (a file or folder) into
/// `dest`, keeping its name. Existing files are left alone. Returns how many were placed.
fn place_files(src: &Path, dest: &Path, link: bool) -> Result<usize, String> {
    let name = src.file_name().ok_or("no file name")?;
    let mut n = 0;
    let mut stack = vec![(src.to_path_buf(), dest.join(name))];
    while let Some((from, to)) = stack.pop() {
        let meta = std::fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            for e in std::fs::read_dir(&from).map_err(|e| e.to_string())?.flatten() {
                stack.push((e.path(), to.join(e.file_name())));
            }
        } else if meta.is_file() && !to.exists() {
            if let Some(p) = to.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let linked = link && std::fs::hard_link(&from, &to).is_ok();
            if !linked {
                std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
            }
            n += 1;
        }
    }
    Ok(n)
}

async fn on_added(st: Arc<AppState>, hash: InfoHash) {
    // The torrent shows up in the snapshot on the next tick.
    let mut cat = None;
    for _ in 0..20 {
        if let Some(t) = st.engine.snapshot().torrents.iter().find(|t| t.hash == hash) {
            cat = t.category.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let Some(cat) = cat else { return };
    for lib in st.library.libraries_for_category(&cat).await.unwrap_or_default() {
        let s = &lib.settings;
        if s.seed_ratio.is_some() || s.seed_minutes.is_some() {
            st.engine.send(EngineCommand::SetSeedLimits {
                hashes: vec![hash],
                ratio: s.seed_ratio,
                minutes: s.seed_minutes,
            });
            break;
        }
    }
}

/// Categories that feed a library download into its first folder (unless the category
/// already has a folder), so "in place" import works without setup.
pub async fn ensure_category_folders(st: &AppState) {
    let Ok(libs) = st.library.list().await else { return };
    let snap = st.engine.snapshot();
    let mut cats = snap.categories.clone();
    let mut changed = false;
    for lib in &libs {
        let Some(folder) = lib.folders.first() else { continue };
        for c in &lib.settings.import_categories {
            let e = cats.entry(c.clone()).or_insert_with(|| {
                changed = true;
                CategoryInfo::default()
            });
            if e.save_path.is_none() {
                e.save_path = Some(PathBuf::from(folder));
                changed = true;
            }
        }
    }
    if changed {
        st.engine.send(EngineCommand::SetCategories(cats));
    }
}

/// Fetches a release link: magnets as they are, `.torrent` URLs downloaded by the server (so
/// the info hash is known). Torznab links that redirect to a magnet are followed.
async fn source_for(link: &str) -> Result<AddSource, ApiErr> {
    if link.starts_with("magnet:") {
        return Ok(AddSource::Magnet(link.to_string()));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    let mut url = link.to_string();
    for _ in 0..6 {
        if url.starts_with("magnet:") {
            return Ok(AddSource::Magnet(url));
        }
        let res = client
            .get(&url)
            .send()
            .await
            .map_err(|e| err(ErrorCode::BadRequest, format!("fetching the release: {e}")))?;
        if res.status().is_redirection() {
            url = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| err(ErrorCode::BadRequest, "redirect without location"))?
                .to_string();
            continue;
        }
        if !res.status().is_success() {
            return Err(err(ErrorCode::BadRequest, format!("the indexer answered {}", res.status())));
        }
        let bytes = res.bytes().await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(err(ErrorCode::PayloadTooLarge, "torrent file too large"));
        }
        return Ok(AddSource::Bytes { name: "release.torrent".into(), bytes: bytes.to_vec() });
    }
    Err(err(ErrorCode::BadRequest, "too many redirects"))
}

/// Adds a release for a library: into the library's first folder with its first import
/// category (so it imports in place). Returns the info hash when known.
pub async fn grab(
    st: &AppState,
    link: &str,
    library: Option<&LibraryInfo>,
    by: &str,
) -> Result<Option<InfoHash>, ApiErr> {
    let source = source_for(link).await?;
    let hash = source.info_hash();
    let settings = st.settings.load_full();
    let snap = st.engine.snapshot();
    let category = library.and_then(|l| l.settings.import_categories.first().cloned());
    let save_path = category
        .as_ref()
        .and_then(|c| snap.categories.get(c))
        .and_then(|c| c.save_path.clone())
        .or_else(|| library.and_then(|l| l.folders.first().map(PathBuf::from)))
        .unwrap_or_else(|| settings.downloads.save_path.clone());
    tracing::info!("{by} grabs {}", source.describe());
    st.engine.send(EngineCommand::Add(Box::new(AddRequest {
        source,
        torrent_bytes: None,
        save_path,
        create_subfolder: settings.downloads.create_subfolder,
        category,
        tags: Vec::new(),
        paused: false,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: Some(by.to_string()),
    })));
    Ok(hash)
}

/// Searches the configured indexers for a wanted entry and judges the results.
pub async fn releases_for(st: &AppState, w: &Wanted) -> Vec<Release> {
    let rules = st.library.profile(w.profile_id).await.unwrap_or_default();
    let found = crate::v1::search::search_all(st, &wanted::query_for(w)).await;
    let candidates: Vec<Candidate> = found
        .results
        .into_iter()
        .map(|r| Candidate { title: r.title, provider: r.provider, link: r.link, size: r.size, seeders: r.seeders })
        .collect();
    wanted::judge(&candidates, Some(w), &rules)
}

/// Searches due wanted entries and grabs the best acceptable release when the profile allows.
pub async fn run_wanted(st: &AppState) -> usize {
    let mut grabbed = 0;
    let due = st.library.due_wanted(6 * 3600).await.unwrap_or_default();
    for w in due {
        let _ = st.library.mark_searched(w.id).await;
        let rules = st.library.profile(w.profile_id).await.unwrap_or_default();
        if !rules.auto_grab {
            continue;
        }
        let best = releases_for(st, &w).await.into_iter().find(|r| r.score.is_some() && r.link.is_some());
        let Some(best) = best else { continue };
        let lib = match w.library_id {
            Some(id) => st.library.get(id).await.ok(),
            None => None,
        };
        let by = w.added_by.clone().unwrap_or_else(|| "wanted".into());
        match grab(st, best.link.as_deref().unwrap_or_default(), lib.as_ref(), &by).await {
            Ok(hash) => {
                let h = hash.map(|h| h.to_hex());
                let _ = st.library.set_wanted_status(w.id, "grabbed", h.clone()).await;
                let _ = st
                    .library
                    .record_grab(Some(w.id), None, &h.unwrap_or_default(), &best.title, best.score.unwrap_or(0))
                    .await;
                grabbed += 1;
            }
            Err(e) => tracing::warn!("wanted {}: grabbing failed: {}", w.title, e.0),
        }
    }
    grabbed
}

/// Runs the wanted search every 15 minutes (entries are searched at most every 6 hours).
pub fn start_wanted_scheduler(st: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15 * 60));
        let stopped = st.stopped();
        tokio::pin!(stopped);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = &mut stopped => break,
            }
            if st.settings.load().search.providers.iter().any(|p| p.enabled) {
                let n = run_wanted(&st).await;
                if n > 0 {
                    tracing::info!("wanted: grabbed {n} release(s)");
                }
            }
        }
    });
}

/// The torrent and file index of a path that is part of a download that isn't finished yet.
pub async fn downloading_file(st: &AppState, path: &Path) -> Option<(InfoHash, usize, String)> {
    let snap = st.engine.snapshot();
    let t = snap
        .torrents
        .iter()
        .find(|t| t.done < t.size && (path == t.content_path || path.starts_with(&t.content_path)))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.engine.send(EngineCommand::FileList { hash: t.hash, reply: tx });
    let files = tokio::time::timeout(Duration::from_secs(5), rx).await.ok()?.ok()??;
    let rel = path.strip_prefix(&t.content_path).ok()?;
    let f = if rel.as_os_str().is_empty() {
        files.first()?
    } else {
        let rel_parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        // Multi-file torrents: components include the torrent's folder name first.
        files.iter().find(|f| f.components.ends_with(&rel_parts))?
    };
    let name = f.components.last().cloned().unwrap_or_default();
    Some((t.hash, f.index, name))
}

/// The engine's loopback stream URL of a torrent file (reads wait for missing pieces).
pub fn loopback_stream_url(st: &AppState, hash: InfoHash, file: usize, name: &str) -> Option<String> {
    let (port, token) = st.engine.snapshot().session.stream.clone()?;
    Some(corrent_core::engine::stream::stream_url(port, &token, hash, file, name))
}
