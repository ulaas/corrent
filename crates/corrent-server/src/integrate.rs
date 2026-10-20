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
use corrent_proto::library::{
    ImportAction, ImportFile, ImportMode, ImportPlan, Library as LibraryInfo, LibraryKind, Release, TorrentImport,
    Wanted,
};
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
        EngineEvent::Completed { name, .. } => {
            crate::notify::send_on(rt, st, "download_done", t!("Download complete").to_string(), name.clone())
        }
        EngineEvent::Added { name, .. } => {
            crate::notify::send_on(rt, st, "torrent_added", t!("Torrent added").to_string(), name.clone())
        }
        _ => {}
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

/// "Add to library…": puts a download's videos into a library (see [`ImportAction`]), then
/// scans it. With `dry_run` only the plan comes back.
pub async fn import_torrent(st: &Arc<AppState>, hash: InfoHash, req: &TorrentImport) -> Result<ImportPlan, ApiErr> {
    let libs = st.library.list().await.map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    let lib =
        libs.into_iter().find(|l| l.id == req.library_id).ok_or_else(|| err(ErrorCode::NotFound, "no such library"))?;
    if !matches!(lib.kind, LibraryKind::Movies | LibraryKind::Shows | LibraryKind::Videos) {
        return Err(err(ErrorCode::BadRequest, t!("That library isn't for videos.")));
    }
    let folder =
        PathBuf::from(lib.folders.first().ok_or_else(|| err(ErrorCode::BadRequest, "the library has no folder"))?);
    let t = st
        .engine
        .snapshot()
        .torrents
        .iter()
        .find(|t| t.hash == hash && !t.pending && !t.restore_failed)
        .cloned()
        .ok_or_else(|| err(ErrorCode::NotFound, "no such torrent"))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    st.engine.send(EngineCommand::FileList { hash, reply: tx });
    let list = rx.await.ok().flatten().ok_or_else(|| err(ErrorCode::NotFound, "no such torrent"))?;
    // Relative to the content: a folder torrent's files are under it, a single file is it.
    let multi = list.len() > 1 || list.first().is_some_and(|f| f.components.len() > 1);
    let source = |rel: &Path| if multi { t.content_path.join(rel) } else { t.content_path.clone() };
    let files: Vec<PathBuf> = list.iter().filter(|f| f.included).map(|f| f.components.iter().collect()).collect();
    let done = list.iter().filter(|f| f.included).all(|f| f.done >= f.len);
    let mut plan = ImportPlan { folder: folder.display().to_string(), ..Default::default() };

    if req.action == ImportAction::MoveDownload {
        // The whole download, as it is named; a single file gets a folder of its own.
        let target = if multi {
            folder.clone()
        } else {
            let p = corrent_library::parse::parse_name(&t.name);
            let title = corrent_library::organize::clean(&p.title);
            folder.join(match p.year {
                Some(y) => format!("{title} ({y})"),
                None => title,
            })
        };
        let content_name = t.content_path.file_name().map(PathBuf::from).unwrap_or_default();
        for rel in &files {
            let to = if multi { target.join(&content_name).join(rel) } else { target.join(&content_name) };
            plan.files.push(ImportFile {
                from: source(rel).display().to_string(),
                exists: to.exists(),
                to: to.display().to_string(),
            });
        }
        if t.save_path == target {
            plan.warnings.push(t!("It is already in that library.").to_string());
        }
        if !req.dry_run {
            st.engine.send(EngineCommand::MoveStorage { hashes: vec![hash], to: target.clone() });
            // Scan once the move is done (it may take a while across drives).
            let st = st.clone();
            let lib_id = lib.id;
            tokio::spawn(async move {
                let deadline = std::time::Instant::now() + Duration::from_secs(6 * 3600);
                while std::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let snap = st.engine.snapshot();
                    match snap.torrents.iter().find(|x| x.hash == hash) {
                        Some(x) if x.save_path == target && x.busy.is_none() => break,
                        Some(_) => {}
                        None => return,
                    }
                }
                st.library.start_scan(lib_id);
            });
        }
        return Ok(plan);
    }

    let names = corrent_library::organize::plan_names(lib.kind, &t.name, &files);
    if names.is_empty() {
        return Err(err(ErrorCode::BadRequest, t!("There are no videos in this download.")));
    }
    for (rel, dest) in &names {
        let to = folder.join(dest);
        plan.files.push(ImportFile {
            from: source(rel).display().to_string(),
            exists: to.exists(),
            to: to.display().to_string(),
        });
    }
    if plan.files.iter().any(|f| f.exists) {
        plan.warnings.push(t!("Some files are already in the library; they are left as they are.").to_string());
    }
    if req.action == ImportAction::MoveOut && !done {
        return Err(err(
            ErrorCode::Conflict,
            t!("The download isn't finished yet; only moving the whole download is possible now."),
        ));
    }
    if req.dry_run {
        return Ok(plan);
    }
    if req.action == ImportAction::MoveOut {
        // The engine lets go of the files first (they stay on disk).
        st.engine.send(EngineCommand::Delete { hashes: vec![hash], with_data: false });
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while st.engine.snapshot().torrents.iter().any(|x| x.hash == hash) {
            if std::time::Instant::now() > deadline {
                return Err(err(ErrorCode::Internal, t!("The torrent couldn't be removed; nothing was moved.")));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let action = req.action;
    let todo: Vec<(PathBuf, PathBuf)> =
        plan.files.iter().filter(|f| !f.exists).map(|f| (PathBuf::from(&f.from), PathBuf::from(&f.to))).collect();
    let content = multi.then(|| t.content_path.clone());
    let failed = tokio::task::spawn_blocking(move || {
        let mut failed = Vec::new();
        for (from, to) in todo {
            if let Err(e) = place_one(&from, &to, action) {
                failed.push(format!("{}: {e}", from.display()));
            }
        }
        // A moved-out download's folder goes when nothing is left in it.
        if action == ImportAction::MoveOut
            && let Some(dir) = content
        {
            remove_empty_dirs(&dir);
        }
        failed
    })
    .await
    .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    if !failed.is_empty() {
        tracing::warn!("import into {}: {}", lib.name, failed.join("; "));
        plan.warnings.push(t!("{n} files couldn't be placed; see the log.", n = failed.len()));
    }
    st.library.start_scan(lib.id);
    Ok(plan)
}

/// One file into the library: moved (a copy, then the original removed, across drives),
/// hard-linked (a copy across drives) or copied. A copy goes through `<name>.part`, so a cut-off
/// copy never looks like a whole file.
fn place_one(from: &Path, to: &Path, action: ImportAction) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    if to.exists() {
        return Ok(());
    }
    let copy = |from: &Path, to: &Path| -> std::io::Result<()> {
        let part =
            to.with_file_name(format!("{}.part", to.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        std::fs::copy(from, &part)?;
        std::fs::File::open(&part)?.sync_all()?;
        std::fs::rename(&part, to)
    };
    match action {
        ImportAction::MoveOut => {
            if std::fs::rename(from, to).is_err() {
                copy(from, to)?;
                std::fs::remove_file(from)?;
            }
            Ok(())
        }
        ImportAction::Hardlink => std::fs::hard_link(from, to).or_else(|_| copy(from, to)),
        ImportAction::Copy | ImportAction::MoveDownload => copy(from, to),
    }
}

/// Removes `dir` and its subfolders where they are empty (never a file).
fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&e.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
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
    let mut found = None;
    for _ in 0..20 {
        if let Some(t) = st.engine.snapshot().torrents.iter().find(|t| t.hash == hash) {
            found = Some((t.category.clone(), t.name.clone()));
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let Some((cat, name)) = found else { return };
    // Added some other way (an RSS rule, by hand, Sonarr): it may be what the wanted list
    // waits for, so the wanted search stops looking.
    claim_wanted(&st, hash, &name).await;
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

/// Marks wanted entries that `name` fits as grabbed by this torrent.
pub async fn claim_wanted(st: &AppState, hash: InfoHash, name: &str) {
    let Ok(waiting) = st.library.wanted(Some("wanted".into())).await else { return };
    for w in waiting {
        if w.kind != corrent_proto::library::WantedKind::Other && corrent_library::wanted::fits(name, &w).is_ok() {
            tracing::info!("wanted: “{}” arrived as {name}", w.title);
            let _ = st.library.set_wanted_status(w.id, "grabbed", Some(hash.to_hex())).await;
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

/// Libraries that ask for it: finished torrents feeding them stop seeding once everyone who can
/// see the library has watched what they brought (the `stop_watched_seeding` task).
pub async fn stop_watched_seeding(st: &Arc<AppState>) -> Result<String, String> {
    let libs = st.library.list().await.map_err(|e| e.to_string())?;
    let snap = st.engine.snapshot();
    let mut stopped = Vec::new();
    for lib in libs.iter().filter(|l| l.settings.stop_seeding_when_watched) {
        for t in snap.torrents.iter().filter(|t| {
            matches!(t.state, corrent_proto::TorrentState::Seeding | corrent_proto::TorrentState::Completed)
                && t.category.as_ref().is_some_and(|c| lib.settings.import_categories.contains(c))
        }) {
            // In place imports keep the download's path; copies/links were made under the library.
            let content = t.save_path.join(&t.name);
            let mut watched = st.library.watched_by_everyone(lib.id, &content).await.map_err(|e| e.to_string())?;
            if watched.is_none() {
                for f in &lib.folders {
                    if let Some(w) = st
                        .library
                        .watched_by_everyone(lib.id, &Path::new(f).join(&t.name))
                        .await
                        .map_err(|e| e.to_string())?
                    {
                        watched = Some(w);
                        break;
                    }
                }
            }
            if watched == Some(true) && !stopped.contains(&t.hash) {
                tracing::info!("“{}” was watched by everyone in {}: seeding stops", t.name, lib.name);
                stopped.push(t.hash);
            }
        }
    }
    if !stopped.is_empty() {
        st.engine.send(EngineCommand::Pause(stopped.clone()));
    }
    Ok(format!("{} stopped", stopped.len()))
}
