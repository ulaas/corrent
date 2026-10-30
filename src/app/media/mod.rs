//! The media side of the desktop app: home, libraries, details, search and the player.
//!
//! The app hosts the media server in-process (loopback only) and talks to it through the same
//! API as remote clients, so the pages work identically against a remote server.

pub mod agent;
pub mod cast;
pub mod client;
pub mod downloads;
mod grid;
mod identify;
pub mod images;
mod item_menu;
pub mod libraries;
pub mod lists;
pub mod offline;
pub mod player;
pub mod profiles;
pub mod providers;
mod refresh;
pub mod subtitles;
pub mod syncplay;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use corrent_proto::library::{HomeRow, Item, ItemDetail, ItemKind, Library, Page as ItemPage, SearchResults};
use egui::{Color32, CornerRadius, Rect, RichText, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as icons;

use self::client::{MediaClient, Slot, fetch, ready};
use self::images::ImageCache;
use self::player::{GlAccess, Player, PlayerAction, QueueEntry};
use super::CorrentApp;
use crate::ui::theme::Palette;

#[derive(Clone, Debug, PartialEq)]
pub enum Page {
    Home,
    Library(i64),
    Item(i64),
    Search,
    /// Everything a person is credited in (id, name).
    Person(i64, String),
    /// Adding, scanning and removing libraries.
    Libraries,
    /// Offline copies on this computer.
    Downloads,
    /// TV mode's settings.
    TvSettings,
    /// Internet radio stations.
    Radio,
    /// Playlists and collections.
    Playlists,
    /// A playlist (or, with `true`, a collection).
    Playlist(i64, bool),
    /// Every item of a home row ("See all"), by the row's id.
    Row(String),
    /// AI agents: connecting one, and the changes they propose.
    Agent,
}

const PAGE: i64 = 120;

#[derive(Default)]
struct Grid {
    total: Option<i64>,
    items: HashMap<i64, Item>,
    requested: HashSet<i64>,
    pending: Vec<(i64, Slot<ItemPage<Item>>)>,
}

/// Playlists and collections.
type Lists = (Vec<corrent_proto::library::Playlist>, Vec<corrent_proto::library::Collection>);
type ListItems = ((i64, bool), Slot<Vec<Item>>);

#[derive(Default)]
pub struct MediaState {
    local: Option<Arc<corrent_server::local::LocalServer>>,
    /// Engine events from before the in-process server was up (imports must still happen).
    pub early_events: Vec<crate::engine::EngineEvent>,
    starting: Option<Slot<Arc<corrent_server::local::LocalServer>>>,
    pub client: Option<MediaClient>,
    pub error: Option<String>,
    pub page: Option<Page>,
    back: Vec<Page>,
    /// The focused widget on each page in `back` (TV mode returns to the card you left).
    back_focus: Vec<Option<egui::Id>>,
    libraries: Option<Slot<Vec<Library>>>,
    /// The last libraries and home rows loaded: shown while a refresh is on its way, so the
    /// periodic refresh never blanks the page (or moves it).
    libraries_shown: Vec<Library>,
    home: Option<Slot<Vec<HomeRow>>>,
    home_shown: Option<Vec<HomeRow>>,
    grids: HashMap<i64, Grid>,
    detail: Option<(i64, Slot<ItemDetail>)>,
    children: HashMap<i64, Slot<ItemPage<Item>>>,
    person: Option<(i64, Slot<ItemPage<Item>>)>,
    pub profiles: profiles::Profiles,
    pub library_form: Option<libraries::LibraryForm>,
    sub_finder: Option<subtitles::SubFinder>,
    identify: Option<identify::Identify>,
    /// The item a card's menu asked for (its IMDb/TMDB ids).
    menu_detail: Option<(i64, Slot<ItemDetail>)>,
    /// Metadata refreshes (an item's, libraries').
    refresh: refresh::RefreshUi,
    /// Settings → Metadata.
    pub providers: providers::ProvidersUi,
    /// Media → AI agent.
    pub agent: agent::AgentUi,
    remove_library: Option<(i64, String)>,
    /// The user's client preferences on the server (home row order and hidden rows).
    user_prefs: Option<Slot<serde_json::Value>>,
    pub query: String,
    search: Option<(String, Slot<SearchResults>)>,
    images: ImageCache,
    pub player: Option<Player>,
    pub gl: Option<GlAccess>,
    selected_season: HashMap<i64, i64>,
    refreshed: Option<std::time::Instant>,
    /// The server URL the client uses (remote mode; changes when the route does).
    route: String,
    /// A focused card outside the view: the page scrolls to it.
    focus_rect: Option<Rect>,
    /// How library pages are sorted (and reversed).
    sort: (corrent_proto::library::SortBy, bool),
    offline: Option<Arc<offline::Store>>,
    offline_loading: Option<Slot<Arc<offline::Store>>>,
    radio: Option<Slot<Vec<corrent_server::radio::Station>>>,
    playlists: Option<Slot<Lists>>,
    /// (playlist or collection id, is a collection) and its items.
    list_items: Option<ListItems>,
    my_playlists: Option<Slot<Vec<corrent_proto::library::Playlist>>>,
    new_playlist: String,
    /// Sound devices for the Playback settings (asked once).
    pub audio_devices: Option<Slot<Vec<(String, String)>>>,
    /// Waiting downloads were restarted (once per session).
    offline_resumed: bool,
    /// When queued offline progress was last sent.
    flushed: Option<std::time::Instant>,
}

impl MediaState {
    pub fn libraries(&self) -> Vec<Library> {
        self.libraries.as_ref().and_then(ready).and_then(Result::ok).unwrap_or_else(|| self.libraries_shown.clone())
    }

    pub fn local(&self) -> Option<&Arc<corrent_server::local::LocalServer>> {
        self.local.as_ref()
    }

    /// An artwork texture (see [`ImageCache::get`]).
    pub fn image(
        &mut self,
        client: &MediaClient,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        sha: &str,
        width: u32,
    ) -> Option<egui::TextureHandle> {
        self.images.get(client, rt, ctx, sha, width)
    }

    /// Forgets the server and everything loaded from it (after switching servers).
    pub fn reset(&mut self) {
        let gl = self.gl.take();
        *self = MediaState { gl, ..Default::default() };
    }

    /// Tests: show an item's page.
    #[cfg(test)]
    pub(crate) fn dev_open_item(&mut self, id: i64) {
        self.page = Some(Page::Item(id));
    }

    /// Tests: a library grid with these items already loaded.
    #[cfg(test)]
    pub(crate) fn fill_grid(&mut self, library: i64, items: Vec<Item>) {
        let g = self.grids.entry(library).or_default();
        g.total = Some(items.len() as i64);
        g.items = items.into_iter().enumerate().map(|(i, it)| (i as i64, it)).collect();
    }

    /// Load everything again (after a change); what is on screen stays until the new data is
    /// there.
    pub(crate) fn invalidate(&mut self) {
        self.keep_shown();
        self.home = None;
        self.libraries = None;
        self.person = None;
        self.user_prefs = None;
        self.grids.clear();
        self.children.clear();
        self.detail = None;
    }

    /// The periodic refresh: home, libraries and the loaded parts of library grids are fetched
    /// again and replace what is shown when they arrive; nothing goes blank.
    fn refresh(&mut self) {
        self.keep_shown();
        self.home = None;
        self.libraries = None;
        self.user_prefs = None;
        for g in self.grids.values_mut() {
            // Visible pages are requested again and overwrite the items they bring.
            g.requested.clear();
        }
    }

    fn keep_shown(&mut self) {
        if let Some(Ok(rows)) = self.home.as_ref().and_then(ready) {
            self.home_shown = Some(rows);
        }
        if let Some(Ok(libs)) = self.libraries.as_ref().and_then(ready) {
            self.libraries_shown = libs;
        }
    }
}

fn kind_aspect(k: ItemKind) -> f32 {
    match k {
        ItemKind::Album | ItemKind::Artist | ItemKind::Track => 1.0,
        ItemKind::Episode | ItemKind::Video | ItemKind::Photo => 16.0 / 9.0,
        _ => 2.0 / 3.0,
    }
}

fn subtitle_of(i: &Item) -> String {
    match i.kind {
        ItemKind::Episode => {
            let show = i.context.first().cloned().unwrap_or_default();
            // "Season 2" → 2, for the compact S2E5 form.
            let season = i.context.get(1).and_then(|s| s.rsplit(' ').next()).and_then(|n| n.parse::<i64>().ok());
            match (season, i.index) {
                (Some(s), Some(e)) => format!("{show} · S{s}E{e}"),
                (None, Some(e)) => format!("{show} · E{e}"),
                _ => show,
            }
        }
        ItemKind::Season => i.context.first().cloned().unwrap_or_default(),
        ItemKind::Album | ItemKind::Track => i.context.first().cloned().unwrap_or_default(),
        _ => i.year.map(|y| y.to_string()).unwrap_or_default(),
    }
}

fn audio_kind(k: ItemKind) -> bool {
    matches!(k, ItemKind::Track | ItemKind::Album | ItemKind::Artist | ItemKind::Book)
}

impl CorrentApp {
    /// Keeps eframe's GL access for the player (video renders inside the window).
    pub(crate) fn with_media_gl(mut self, cc: &eframe::CreationContext<'_>) -> Self {
        if let (Some(gl), Some(proc)) = (cc.gl.clone(), cc.get_proc_address.clone()) {
            self.media.gl = Some(GlAccess { gl, proc });
        }
        self
    }

    /// Debug-only hooks: CORRENT_DEV_VIEW=media opens the media view, CORRENT_DEV_ITEM=<id>
    /// an item, CORRENT_DEV_PLAY=<id> plays one; CORRENT_DEV_SHOT=<png> saves a screenshot after
    /// CORRENT_DEV_SHOT_DELAY seconds (default 6) and CORRENT_DEV_EXIT=1 closes the app after it.
    /// CORRENT_DEV_LIBRARY=<folder>[;kind[;name]] (several joined by `|`) adds those libraries
    /// unless one already has the folder (kind: movies, shows, music, videos, photos; default
    /// movies), then shows them.
    #[cfg(debug_assertions)]
    pub(crate) fn media_dev_hooks(&mut self, ctx: &egui::Context) {
        if !self.media_dev_done {
            self.media_dev_done = true;
            if std::env::var("CORRENT_DEV_VIEW").as_deref() == Ok("media") {
                self.main_view = super::feeds::MainView::Media;
            }
            if std::env::var("CORRENT_DEV_VIEW").as_deref() == Ok("libraries") {
                self.main_view = super::feeds::MainView::Media;
                self.open(Page::Libraries);
            }
            // CORRENT_DEV_PAGE=library:<id> or row:<id> (a home row, "See all").
            if let Ok(page) = std::env::var("CORRENT_DEV_PAGE") {
                self.main_view = super::feeds::MainView::Media;
                if let Some(id) = page.strip_prefix("library:").and_then(|v| v.parse().ok()) {
                    self.open(Page::Library(id));
                } else if let Some(row) = page.strip_prefix("row:") {
                    self.open(Page::Row(row.to_string()));
                } else if page == "agent" {
                    self.open(Page::Agent);
                }
            }
            if let Some(id) = std::env::var("CORRENT_DEV_ITEM").ok().and_then(|v| v.parse().ok()) {
                self.main_view = super::feeds::MainView::Media;
                self.open(Page::Item(id));
            }
            // A pairing link: fill in the Connect dialog (CORRENT_DEV_CONNECT) or pair right away
            // (CORRENT_DEV_PAIR).
            if let Ok(link) = std::env::var("CORRENT_DEV_CONNECT") {
                self.open_connect();
                if let Some(d) = &mut self.connect {
                    d.dev_fill(&link);
                }
            }
            if let Ok(link) = std::env::var("CORRENT_DEV_PAIR")
                && let Some(l) = crate::link::pair::parse_link(&link)
                && let Some(url) = crate::link::pair::base_url(&l.host)
            {
                self.media_dev_pair = Some(fetch(&self.rt, ctx, async move {
                    let t = crate::link::pair::trust(&url, Some(l.fingerprint.as_str()), &l.connect).await?;
                    crate::link::pair::pair(&t, crate::link::pair::Credentials::Code(l.code)).await
                }));
            }
        }
        if let Some(r) = self.media_dev_pair.as_ref().and_then(ready) {
            self.media_dev_pair = None;
            match r {
                Ok(e) => self.use_server(Some(e)),
                Err(e) => tracing::warn!("CORRENT_DEV_PAIR: {e}"),
            }
        }
        if let Some(id) = std::env::var("CORRENT_DEV_PLAY").ok().and_then(|v| v.parse::<i64>().ok())
            && self.media.player.is_none()
            && self.media.client.is_some()
            && !self.media_dev_played
        {
            self.media_dev_played = true;
            self.main_view = super::feeds::MainView::Media;
            let e = QueueEntry {
                item_id: id,
                title: String::new(),
                subtitle: String::new(),
                resume_ms: 0,
                poster: None,
                local: None,
                stream: None,
                audio_track: None,
                subtitle_track: None,
            };
            self.play_queue(vec![e], false);
        }
        if let Ok(spec) = std::env::var("CORRENT_DEV_LIBRARY")
            && !self.media_dev_library_sent
            && let Some(c) = self.media.client.clone()
        {
            self.media_dev_library_sent = true;
            self.main_view = super::feeds::MainView::Media;
            let wanted: Vec<corrent_proto::library::NewLibrary> = spec
                .split('|')
                .filter(|s| !s.trim().is_empty())
                .map(|s| {
                    let mut parts = s.split(';');
                    let folder = parts.next().unwrap_or_default().trim().to_string();
                    let kind = parts.next().map(str::trim).unwrap_or("movies");
                    let name = parts.next().map(str::trim).filter(|n| !n.is_empty()).map(String::from);
                    let kind = serde_json::from_value(serde_json::Value::String(kind.into()))
                        .unwrap_or(corrent_proto::library::LibraryKind::Movies);
                    let name = name.unwrap_or_else(|| format!("{kind:?}"));
                    corrent_proto::library::NewLibrary {
                        name,
                        kind,
                        folders: vec![folder],
                        settings: corrent_proto::library::LibrarySettings::defaults(),
                    }
                })
                .collect();
            self.media_dev_library = Some(fetch(&self.rt, ctx, async move {
                let have: Vec<corrent_proto::library::Library> = c.get("/api/v1/libraries").await?;
                let mut added = 0;
                for lib in wanted {
                    if have.iter().any(|h| h.folders.iter().any(|f| lib.folders.contains(f))) {
                        continue;
                    }
                    let _: serde_json::Value = c.send(reqwest::Method::POST, "/api/v1/libraries", &lib).await?;
                    added += 1;
                }
                Ok(added)
            }));
        }
        if let Some(r) = self.media_dev_library.as_ref().and_then(ready) {
            self.media_dev_library = None;
            match r {
                Ok(n) => tracing::info!("CORRENT_DEV_LIBRARY: {n} libraries added"),
                Err(e) => tracing::warn!("CORRENT_DEV_LIBRARY: {e}"),
            }
            self.media.invalidate();
        }
        // CORRENT_DEV_PAUSE=<seconds>: pause the hooked playback after that long (the controls
        // stay up for a screenshot).
        if let Some(secs) = std::env::var("CORRENT_DEV_PAUSE").ok().and_then(|v| v.parse::<f64>().ok())
            && !self.media_dev_paused
            && let Some(pl) = &mut self.media.player
            && self.media_dev_started.is_some_and(|t| t.elapsed().as_secs_f64() > secs)
        {
            self.media_dev_paused = true;
            pl.toggle_pause();
        }
        if std::env::var("CORRENT_DEV_SERVER_FS").is_ok()
            && self.server.as_ref().is_some_and(|s| s.connected())
            && !self.media_dev_profiles
        {
            self.media_dev_profiles = true;
            self.pick(super::pickers::PickTarget::LibraryFolder, super::pickers::PickKind::Folder, None);
        }
        if std::env::var("CORRENT_DEV_PROFILES").is_ok() && self.media.client.is_some() && !self.media_dev_profiles {
            self.media_dev_profiles = true;
            self.main_view = super::feeds::MainView::Media;
            self.open_profiles();
        }
        if let Ok(path) = std::env::var("CORRENT_DEV_SHOT") {
            let delay: f64 = std::env::var("CORRENT_DEV_SHOT_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(6.0);
            let started = *self.media_dev_started.get_or_insert_with(std::time::Instant::now);
            if !self.media_dev_shot_sent && started.elapsed().as_secs_f64() > delay {
                self.media_dev_shot_sent = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            let shot = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Err(e) = image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                    tracing::warn!("screenshot: {e}");
                }
                if std::env::var("CORRENT_DEV_EXIT").as_deref() == Ok("1") {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// Starts the in-process media server (once).
    pub fn ensure_media_server(&mut self) {
        let m = &mut self.media;
        if m.client.is_some() || m.starting.is_some() || m.error.is_some() {
            return;
        }
        // Connected to a server: its media library, not this computer's.
        if let Some(s) = &self.server {
            // Wait for the link to find a route (its address or Corrent Connect).
            let Some(route) = s.status.route.load_full() else { return };
            // mpv reads the CA from a file (written off the UI thread, well before any playback).
            let ca_file = route.ca.as_ref().filter(|_| crate::link::pair::valid_id(&s.entry.id)).map(|pem| {
                let path = self.paths.data_dir.join(format!("server-{}-ca.pem", s.entry.id));
                let (p2, pem) = (path.clone(), pem.clone());
                self.rt.spawn_blocking(move || std::fs::write(p2, pem));
                path
            });
            m.route = route.url.clone();
            m.client = Some(route.api.clone().with_ca_file(ca_file));
            return;
        }
        let (engine, settings, paths) = (self.engine.clone(), self.remote.shared.clone(), self.paths.clone());
        m.starting = Some(fetch(&self.rt, &self.ctx, async move {
            corrent_server::local::start(engine, settings, paths).await.map(Arc::new).map_err(|e| format!("{e:#}"))
        }));
    }

    /// Per-frame media work: finishing server start, uploading images, player housekeeping.
    pub fn media_tick(&mut self, ctx: &egui::Context) {
        // The server is now reached another way: start over with the new route.
        if let Some(s) = &self.server
            && let Some(r) = s.status.route.load_full()
            && self.media.client.is_some()
            && r.url != self.media.route
        {
            self.media.reset();
        }
        // The server runs from the start: imports of completed downloads need it too.
        self.ensure_media_server();
        if let Some(s) = self.media.starting.as_ref().and_then(ready) {
            self.media.starting = None;
            match s {
                Ok(local) => match MediaClient::new(local.addr(), &local.token, None) {
                    Ok(c) => {
                        for ev in std::mem::take(&mut self.media.early_events) {
                            corrent_server::integrate::on_engine_event(&local.state, &self.rt, ev);
                        }
                        self.media.client = Some(c);
                        self.media.local = Some(local);
                    }
                    Err(e) => self.media.error = Some(e),
                },
                Err(e) => {
                    tracing::warn!("media server: {e}");
                    self.media.error = Some(e);
                }
            }
        }
        if self.media.libraries.is_none()
            && let Some(c) = self.media.client.clone()
        {
            self.media.libraries = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/libraries").await }));
        }
        self.media.images.tick(ctx);
        if let Some(pl) = &mut self.media.player {
            pl.tick();
        }
        // Lists go stale as scans and metadata land: refresh every 30 s while visible.
        if self.main_view == super::feeds::MainView::Media
            && self.media.refreshed.is_none_or(|t| t.elapsed().as_secs() > 30)
        {
            self.media.refreshed = Some(std::time::Instant::now());
            self.media.refresh();
        }
    }

    /// Tests: show a media page.
    #[cfg(test)]
    pub(crate) fn open_media_page(&mut self, page: Page) {
        self.main_view = super::feeds::MainView::Media;
        self.open(page);
    }

    /// Back one page (Home when there is no history).
    pub(super) fn media_back(&mut self) {
        self.media.page = Some(self.media.back.pop().unwrap_or(Page::Home));
        if let Some(id) = self.media.back_focus.pop().flatten() {
            self.ctx.memory_mut(|m| m.request_focus(id));
        }
    }

    /// The search page for the current query (on-screen keyboard).
    pub(super) fn media_search_now(&mut self) {
        if self.media.page != Some(Page::Search) {
            self.open(Page::Search);
        }
    }

    /// Backdrop images of the home rows (screensaver).
    pub(super) fn media_backdrops(&self) -> Vec<String> {
        let Some(rows) =
            self.media.home.as_ref().and_then(ready).and_then(Result::ok).or_else(|| self.media.home_shown.clone())
        else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for it in rows.iter().flat_map(|r| &r.items) {
            if let Some(im) = it.images.iter().find(|i| i.kind == "backdrop")
                && !out.contains(&im.sha)
            {
                out.push(im.sha.clone());
            }
        }
        out
    }

    fn open(&mut self, page: Page) {
        let cur = self.media.page.take().unwrap_or(Page::Home);
        if cur != page {
            self.media.back.push(cur);
            self.media.back_focus.push(self.ctx.memory(|m| m.focused()));
        }
        self.media.page = Some(page);
    }

    fn play_queue(&mut self, mut queue: Vec<QueueEntry>, audio_only: bool) {
        let Some(client) = self.media.client.clone() else { return };
        if queue.is_empty() {
            return;
        }
        // Offline copies play from disk (remote servers).
        let offline = self.offline_store();
        if let Some(store) = &offline {
            for e in &mut queue {
                if let Some(d) = store.find(client.base(), e.item_id).filter(|d| d.state == offline::State::Done) {
                    e.local = Some(store.path(&d)).filter(|p| p.is_file());
                }
            }
        }
        self.media.player = None;
        match Player::new(
            client.clone(),
            self.rt.clone(),
            &self.ctx,
            self.media.gl.as_ref(),
            queue,
            audio_only,
            &self.settings.playback,
        ) {
            Ok(mut p) => {
                p.offline = offline.map(|s| (s, client.base().to_string()));
                self.media.player = Some(p)
            }
            // mpv isn't installed: offer to get it rather than only saying so.
            Err(_) if corrent_mpv::Library::get().is_err_and(|e| super::get_mpv::is_missing(&e)) => {
                self.open_get_mpv(true)
            }
            Err(e) => self.toasts.push(crate::engine::ToastKind::Error, t!("Can't play: {why}", why = e)),
        }
    }

    fn entry(i: &Item) -> QueueEntry {
        QueueEntry {
            item_id: i.id,
            title: i.title.clone(),
            subtitle: subtitle_of(i),
            resume_ms: if i.state.played { 0 } else { i.state.position_ms },
            poster: i.images.first().map(|im| im.sha.clone()),
            local: None,
            stream: None,
            audio_track: i.state.audio_track,
            subtitle_track: i.state.subtitle_track,
        }
    }

    /// Plays an item; for an episode or track, the rest of its season/album follows.
    fn play_item(&mut self, item: &Item, siblings: &[Item]) {
        let start = siblings.iter().position(|s| s.id == item.id);
        let queue: Vec<QueueEntry> = match start {
            Some(i) => {
                siblings.get(i..).unwrap_or_default().iter().filter(|s| s.kind.playable()).map(Self::entry).collect()
            }
            None => vec![Self::entry(item)],
        };
        self.play_queue(queue, audio_kind(item.kind));
    }

    /// The media view (central panel).
    pub fn media_view(&mut self, ui: &mut Ui, p: &Palette) {
        self.ensure_media_server();
        if self.media.player.as_ref().is_some_and(|pl| !pl.audio_only) {
            let action = self.media.player.as_mut().map(|pl| pl.show(ui, p)).unwrap_or(PlayerAction::None);
            if let Some((kind, text)) = self.media.player.as_mut().and_then(|pl| pl.notice.take()) {
                let ok = kind == crate::engine::ToastKind::Success;
                self.toasts.push(kind, text);
                if ok {
                    // The new picture on the cards.
                    self.media.invalidate();
                }
            }
            match action {
                PlayerAction::Close => {
                    self.media.player = None;
                    if self.ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    }
                    self.media.invalidate();
                }
                PlayerAction::ToggleFullscreen => {
                    let fs = !self.ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fs));
                    if let Some(pl) = &mut self.media.player {
                        pl.fullscreen = fs;
                    }
                }
                PlayerAction::PickSubtitle => {
                    let start = self.media.player.as_ref().and_then(|pl| pl.local_folder());
                    self.pick(
                        crate::app::pickers::PickTarget::SubtitleFile,
                        crate::app::pickers::PickKind::Subtitle,
                        start,
                    );
                }
                PlayerAction::None => {}
            }
            return;
        }
        let Some(client) = self.media.client.clone() else {
            match &self.media.error {
                Some(e) => crate::ui::widgets::empty_state(
                    ui,
                    p,
                    icons::WARNING_CIRCLE,
                    t!("The media server couldn't start"),
                    e,
                ),
                None => {
                    ui.centered_and_justified(|ui| ui.spinner());
                }
            }
            return;
        };
        if self.media.libraries.is_none() {
            let c = client.clone();
            self.media.libraries = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/libraries").await }));
        }
        self.flush_offline(&client);
        let page = self.media.page.clone().unwrap_or(Page::Home);
        self.media_top_bar(ui, p, &page);
        let ctx = self.ctx.clone();
        self.profiles_window(&ctx, p);
        self.subtitle_finder_window(&ctx, p);
        self.identify_window(&ctx, p);
        self.refresh_tick(&client);
        egui::ScrollArea::vertical().id_salt(("media", format!("{page:?}"))).auto_shrink([false, false]).show(
            ui,
            |ui| {
                ui.add_space(8.0);
                match page {
                    Page::Home => self.home_page(ui, p, &client),
                    Page::Library(id) => self.library_page(ui, p, &client, id),
                    Page::Item(id) => self.item_page(ui, p, &client, id),
                    Page::Search => self.search_page(ui, p, &client),
                    Page::Person(id, _) => self.person_page(ui, p, &client, id),
                    Page::Libraries => self.libraries_page(ui, p, &client),
                    Page::Downloads => self.downloads_page(ui, p, &client),
                    Page::TvSettings => self.tv_settings_page(ui, p),
                    Page::Radio => self.radio_page(ui, p, &client),
                    Page::Playlists => self.playlists_page(ui, p, &client),
                    Page::Playlist(id, col) => self.playlist_page(ui, p, &client, id, col),
                    Page::Row(ref row) => self.row_page(ui, p, &client, row),
                    Page::Agent => self.agent_page(ui, p, &client),
                }
                ui.add_space(24.0);
                if let Some(r) = self.media.focus_rect.take() {
                    ui.scroll_to_rect(r, Some(egui::Align::Center));
                }
            },
        );
    }

    fn media_top_bar(&mut self, ui: &mut Ui, p: &Palette, page: &Page) {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let can_back = !self.media.back.is_empty();
            if ui
                .add_enabled(can_back, egui::Button::new(RichText::new(icons::ARROW_LEFT).size(18.0)).frame(false))
                .on_hover_text(t!("Back"))
                .clicked()
            {
                self.media_back();
            }
            if ui
                .add(egui::Button::new(RichText::new(icons::HOUSE).size(18.0)).frame(false))
                .on_hover_text(t!("Home"))
                .clicked()
            {
                self.open(Page::Home);
            }
            if ui
                .add(egui::Button::new(RichText::new(icons::PLAYLIST).size(18.0)).frame(false))
                .on_hover_text(t!("Playlists"))
                .clicked()
            {
                self.media.playlists = None;
                self.open(Page::Playlists);
            }
            if ui
                .add(egui::Button::new(RichText::new(icons::BROADCAST).size(18.0)).frame(false))
                .on_hover_text(t!("Radio"))
                .clicked()
            {
                // Fresh stations each time the page opens.
                self.media.radio = None;
                self.open(Page::Radio);
            }
            if self.server.is_some()
                && ui
                    .add(egui::Button::new(RichText::new(icons::DOWNLOAD_SIMPLE).size(18.0)).frame(false))
                    .on_hover_text(t!("Downloads"))
                    .clicked()
            {
                self.open(Page::Downloads);
            }
            let title = match page {
                Page::Home => t!("Home").to_string(),
                Page::Library(id) => {
                    self.media.libraries().iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default()
                }
                Page::Item(_) => String::new(),
                Page::Search => t!("Search").to_string(),
                Page::Person(_, name) => name.clone(),
                Page::Libraries => t!("Libraries").to_string(),
                Page::Downloads => t!("Downloads").to_string(),
                Page::TvSettings => t!("Settings").to_string(),
                Page::Radio => t!("Radio").to_string(),
                Page::Playlists => t!("Playlists").to_string(),
                Page::Playlist(..) => String::new(),
                Page::Agent => t!("AI agent").to_string(),
                Page::Row(id) => self
                    .media
                    .home
                    .as_ref()
                    .and_then(ready)
                    .and_then(|r| r.ok())
                    .or_else(|| self.media.home_shown.clone())
                    .and_then(|rows| rows.into_iter().find(|r| r.id == *id).map(|r| r.title))
                    .unwrap_or_default(),
            };
            ui.label(RichText::new(title).size(20.0).strong().color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                if let Some(name) = self.profile_name()
                    && ui
                        .button(format!("{}  {name}", icons::USER_CIRCLE))
                        .on_hover_text(t!("Switch profile"))
                        .clicked()
                {
                    self.open_profiles();
                }
                if self.tv.on {
                    if ui.button(format!("{}  {}", icons::GEAR_SIX, t!("Settings"))).clicked() {
                        self.open(Page::TvSettings);
                    }
                    if ui.button(format!("{}  {}", icons::MAGNIFYING_GLASS, t!("Search"))).clicked() {
                        self.tv.keyboard = true;
                        self.open(Page::Search);
                    }
                    for l in self.media.libraries().into_iter().rev() {
                        if ui.selectable_label(*page == Page::Library(l.id), &l.name).clicked() {
                            self.open(Page::Library(l.id));
                        }
                    }
                    return;
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.media.query)
                        .hint_text(t!("Search movies, shows, music…"))
                        .desired_width(260.0),
                );
                if r.changed() && !self.media.query.trim().is_empty() && *page != Page::Search {
                    self.open(Page::Search);
                }
            });
        });
        ui.add_space(6.0);
    }

    fn card(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, item: &Item, width: f32) -> egui::Response {
        let aspect = kind_aspect(item.kind);
        let h = width / aspect;
        let (rect, resp) = ui.allocate_exact_size(vec2(width, h + 44.0), Sense::click());
        // Screen readers (and UI tests) know the card by its title.
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.title));
        // TV mode starts with the first card focused; a focused card is kept in view.
        if self.tv.on && ui.ctx().memory(|m| m.focused()).is_none() {
            resp.request_focus();
        }
        // Keep the focused card in view (every frame until it is: scroll areas settle late).
        // (A row's horizontal scroll area swallows vertical requests, so the page scrolls in
        // `media_view` from `focus_rect`.)
        if resp.has_focus() && !ui.clip_rect().contains_rect(rect) {
            resp.scroll_to_me(Some(egui::Align::Center));
            self.media.focus_rect = Some(rect);
        }
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let focused = resp.has_focus();
        // TV mode: only the focus counts (a resting mouse pointer would light up a second card).
        let pointer = resp.hovered() && !self.tv.on;
        let hover = crate::ui::motion::toggle(ui.ctx(), resp.id, pointer || focused, crate::ui::motion::MICRO);
        let img_rect =
            Rect::from_min_size(rect.min, vec2(width, h)).shrink(-3.0 * hover).translate(vec2(0.0, -3.0 * hover));
        let painter = ui.painter().clone();
        let radius = CornerRadius::same(10);
        // Shadow when lifted.
        if hover > 0.0 {
            painter.rect_filled(
                img_rect.translate(vec2(0.0, 6.0)),
                radius,
                Color32::from_black_alpha((60.0 * hover) as u8),
            );
        }
        // Landscape cards: thumbnail, backdrop, poster; portrait ones: poster first.
        let order: [&str; 3] =
            if aspect >= 1.0 { ["thumb", "backdrop", "poster"] } else { ["poster", "thumb", "backdrop"] };
        let image = order
            .iter()
            .find_map(|k| item.images.iter().find(|i| i.kind == *k))
            .or_else(|| item.images.first())
            .cloned();
        let ppp = ui.ctx().pixels_per_point();
        let mut drawn = false;
        if let Some(im) = &image {
            if let Some(tex) = self.media.images.get(client, &self.rt, &self.ctx, &im.sha, (width * ppp) as u32) {
                let t = crate::ui::motion::toggle(ui.ctx(), resp.id.with("in"), true, crate::ui::motion::PAGE);
                painter.add(egui::Shape::Rect(
                    egui::epaint::RectShape::filled(img_rect, radius, Color32::WHITE.gamma_multiply(t))
                        .with_texture(tex.id(), cover_uv(tex.size_vec2(), img_rect.size())),
                ));
                drawn = true;
            } else if let Some(b) = self.media.images.blurhash(&self.ctx, &im.blurhash) {
                painter.add(egui::Shape::Rect(
                    egui::epaint::RectShape::filled(img_rect, radius, Color32::WHITE)
                        .with_texture(b.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                ));
                drawn = true;
            }
        }
        if !drawn {
            // No picture (yet): the title's initials over a soft gradient, and the kind's icon.
            let mut mesh = egui::Mesh::default();
            let (top, bottom) = (p.bg3, p.bg2);
            mesh.colored_vertex(img_rect.left_top(), top);
            mesh.colored_vertex(img_rect.right_top(), top);
            mesh.colored_vertex(img_rect.left_bottom(), bottom);
            mesh.colored_vertex(img_rect.right_bottom(), bottom);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(1, 2, 3);
            painter.add(egui::Shape::Rect(egui::epaint::RectShape::filled(img_rect, radius, p.bg2)));
            painter.with_clip_rect(img_rect.shrink(2.0)).add(mesh);
            let icon = match item.kind {
                ItemKind::Album | ItemKind::Track | ItemKind::Artist => icons::MUSIC_NOTES,
                ItemKind::Photo => icons::IMAGE,
                ItemKind::Book => icons::BOOK_OPEN,
                ItemKind::Show | ItemKind::Season | ItemKind::Episode => icons::TELEVISION_SIMPLE,
                _ => icons::FILM_STRIP,
            };
            let short = img_rect.width().min(img_rect.height());
            painter.text(
                img_rect.center(),
                egui::Align2::CENTER_CENTER,
                grid::initials(&item.title),
                crate::ui::theme::bold(short / 3.2),
                p.text_dim,
            );
            painter.text(
                img_rect.left_bottom() + vec2(10.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                icon,
                egui::FontId::proportional((short / 7.0).clamp(14.0, 22.0)),
                p.text_mute,
            );
        }
        if p.high_contrast {
            painter.rect_stroke(img_rect, radius, egui::Stroke::new(1.5, p.border_hi), egui::StrokeKind::Inside);
        }
        // Progress and played badge.
        if item.state.position_ms > 0 && !item.state.played && item.runtime_ms.unwrap_or(0) > 0 {
            let f = (item.state.position_ms as f32 / item.runtime_ms.unwrap_or(1) as f32).clamp(0.0, 1.0);
            let bar = Rect::from_min_size(
                pos2(img_rect.left() + 8.0, img_rect.bottom() - 10.0),
                vec2(img_rect.width() - 16.0, 4.0),
            );
            painter.rect_filled(bar, CornerRadius::same(2), Color32::from_black_alpha(140));
            painter.rect_filled(
                Rect::from_min_size(bar.min, vec2(bar.width() * f, 4.0)),
                CornerRadius::same(2),
                p.accent,
            );
        }
        if item.state.played || item.state.unplayed == Some(0) {
            let c = pos2(img_rect.right() - 16.0, img_rect.top() + 16.0);
            painter.circle_filled(c, 11.0, p.accent);
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                icons::CHECK,
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        } else if let Some(n) = item.state.unplayed.filter(|n| *n > 0) {
            let c = pos2(img_rect.right() - 16.0, img_rect.top() + 16.0);
            painter.circle_filled(c, 12.0, p.accent);
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                n.to_string(),
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        if hover > 0.0 && item.kind.playable() {
            let c = img_rect.center();
            painter.circle_filled(c, 26.0 * hover, Color32::from_black_alpha((150.0 * hover) as u8));
            painter.text(
                c,
                egui::Align2::CENTER_CENTER,
                egui_phosphor::regular::PLAY,
                egui::FontId::proportional(24.0 * hover),
                Color32::WHITE,
            );
        }
        // Keyboard / remote focus: a ring that reads from across the room.
        if focused {
            painter.rect_stroke(
                img_rect.expand(3.0),
                CornerRadius::same(12),
                egui::Stroke::new(3.0, p.accent),
                egui::StrokeKind::Outside,
            );
        }
        let text_top = rect.top() + h + 6.0;
        // One line each, cut with "…" (the full title is in the tooltip).
        let line = |text: String, size: f32, color: Color32, strong: bool| {
            let font = if strong { crate::ui::theme::bold(size) } else { egui::FontId::proportional(size) };
            let mut job = egui::text::LayoutJob::simple_singleline(text, font, color);
            job.wrap = egui::text::TextWrapping {
                max_width: (width - 4.0).max(10.0),
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            painter.layout_job(job)
        };
        painter.galley(pos2(rect.left() + 2.0, text_top), line(item.title.clone(), 13.5, p.text, true), p.text);
        painter.galley(
            pos2(rect.left() + 2.0, text_top + 19.0),
            line(subtitle_of(item), 12.0, p.text_dim, false),
            p.text_dim,
        );
        if !self.tv.on {
            self.item_menu(&resp, client, item, &[], None);
        }
        if self.tv.on { resp } else { resp.on_hover_text(&item.title) }
    }

    /// A titled section of cards that fills the width and wraps onto further rows (at most
    /// `max_rows`; "See all" then opens `more`). Nothing ever scrolls sideways.
    #[allow(clippy::too_many_arguments)]
    fn card_section(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        client: &MediaClient,
        title: &str,
        items: &[Item],
        max_rows: Option<usize>,
        more: Option<Page>,
    ) {
        let avail = ui.available_width() - 40.0;
        let landscape = items.first().is_some_and(|i| kind_aspect(i.kind) > 1.0);
        let (min, max) = grid::limits(landscape, avail);
        let g = grid::spec(avail, min, max, 16.0);
        let shown = max_rows.map_or(items.len(), |r| (r * g.cols).min(items.len()));
        let mut open: Option<Page> = None;
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new(title).size(17.0).strong().color(p.text));
            if items.len() > 1 {
                ui.label(RichText::new(items.len().to_string()).size(13.0).color(p.text_mute));
            }
            if let Some(page) = more.filter(|_| shown < items.len()) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(20.0);
                    let label = RichText::new(format!("{}  {}", t!("See all"), icons::CARET_RIGHT)).color(p.accent);
                    if ui.add(egui::Button::new(label).frame(false)).clicked() {
                        open = Some(page);
                    }
                });
            }
        });
        ui.add_space(8.0);
        let visible: Vec<&Item> = items.iter().take(shown).collect();
        let mut clicked: Option<usize> = None;
        for (r, row) in visible.chunks(g.cols).enumerate() {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.spacing_mut().item_spacing.x = g.gap;
                for (c, it) in row.iter().enumerate() {
                    if self.card(ui, p, client, it, g.card_w).clicked() {
                        clicked = Some(r * g.cols + c);
                    }
                }
            });
            ui.add_space(g.gap);
        }
        ui.add_space(10.0);
        if let Some(n) = clicked
            && let Some(it) = items.get(n)
        {
            if it.kind.playable() && !matches!(it.kind, ItemKind::Track) {
                self.play_item(it, items.get(n..n + 1).unwrap_or_default());
            } else {
                self.open(Page::Item(it.id));
            }
        }
        if let Some(page) = open {
            self.open(page);
        }
    }

    /// Every item of one home row ("See all").
    /// The home rows: the fresh ones, else the ones shown before a refresh (fetched when needed).
    fn home_rows(&mut self, client: &MediaClient) -> Option<Vec<HomeRow>> {
        if self.media.home.is_none() {
            let c = client.clone();
            self.media.home = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/home").await }));
        }
        self.media.home.as_ref().and_then(ready).and_then(Result::ok).or_else(|| self.media.home_shown.clone())
    }

    fn row_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, id: &str) {
        let row = self.home_rows(client).and_then(|rows| rows.into_iter().find(|r| r.id == id));
        match row {
            Some(r) => self.card_section(ui, p, client, &r.title, &r.items, None, None),
            None => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
        }
    }

    /// The band at the top of Home: the item you were watching (or something with a backdrop),
    /// large, with Resume/Play and More info. `buttons`: Some(resuming) on Home, None on the
    /// item's own page (it has its own).
    fn hero(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, item: &Item, buttons: Option<bool>) {
        let resuming = buttons == Some(true);
        let width = ui.available_width();
        let narrow = width < grid::NARROW;
        let h = (width * 0.36).clamp(190.0, 420.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.title));
        let backdrop = item
            .images
            .iter()
            .find(|i| i.kind == "backdrop")
            .or_else(|| item.images.iter().find(|i| i.kind == "thumb"))
            .or_else(|| item.images.first())
            .cloned();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, p.bg1);
        if let Some(im) = backdrop {
            let ppp = ui.ctx().pixels_per_point();
            if let Some(tex) = self.media.images.get(client, &self.rt, &self.ctx, &im.sha, (width * ppp) as u32) {
                painter.add(egui::Shape::Rect(
                    egui::epaint::RectShape::filled(rect, 0.0, Color32::WHITE)
                        .with_texture(tex.id(), cover_uv(tex.size_vec2(), rect.size())),
                ));
            } else if let Some(b) = self.media.images.blurhash(&self.ctx, &im.blurhash) {
                painter.add(egui::Shape::Rect(
                    egui::epaint::RectShape::filled(rect, 0.0, Color32::WHITE)
                        .with_texture(b.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                ));
            }
        }
        // Darken towards the text (left) and fade into the page (bottom).
        let gradient = |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2, d: egui::Pos2, from: Color32, to: Color32| {
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(a, from);
            mesh.colored_vertex(b, to);
            mesh.colored_vertex(c, from);
            mesh.colored_vertex(d, to);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(1, 2, 3);
            mesh
        };
        let shade = p.bg0.gamma_multiply(0.85);
        let mid = rect.left() + rect.width() * if narrow { 0.95 } else { 0.62 };
        painter.add(gradient(
            rect.left_top(),
            pos2(mid, rect.top()),
            rect.left_bottom(),
            pos2(mid, rect.bottom()),
            shade,
            Color32::TRANSPARENT,
        ));
        let fade_top = rect.top() + h * 0.55;
        painter.add(gradient(
            pos2(rect.left(), fade_top),
            pos2(rect.left(), rect.bottom()),
            pos2(rect.right(), fade_top),
            pos2(rect.right(), rect.bottom()),
            Color32::TRANSPARENT,
            p.bg0,
        ));
        // Title, details, progress and buttons, bottom left.
        let left = rect.left() + if narrow { 20.0 } else { 36.0 };
        let title_size = (width / 30.0).clamp(22.0, 40.0);
        let mut job =
            egui::text::LayoutJob::simple_singleline(item.title.clone(), crate::ui::theme::bold(title_size), p.text);
        job.wrap = egui::text::TextWrapping {
            max_width: (mid - left).max(120.0),
            max_rows: 2,
            break_anywhere: false,
            overflow_character: Some('…'),
        };
        let title = painter.layout_job(job);
        let buttons_y = if buttons.is_some() { rect.bottom() - 52.0 } else { rect.bottom() + 6.0 };
        let mut y = buttons_y - 14.0;
        let runtime = item.runtime_ms.unwrap_or(0);
        if resuming && runtime > 0 {
            let f = (item.state.position_ms as f32 / runtime as f32).clamp(0.0, 1.0);
            let left_min = ((runtime - item.state.position_ms).max(0) / 60_000).max(1);
            let bar = Rect::from_min_size(pos2(left, y - 4.0), vec2(160.0, 4.0));
            painter.rect_filled(bar, CornerRadius::same(2), Color32::from_white_alpha(50));
            painter.rect_filled(Rect::from_min_size(bar.min, vec2(160.0 * f, 4.0)), CornerRadius::same(2), p.accent);
            painter.text(
                pos2(bar.right() + 10.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                t!("{n} min left", n = left_min),
                egui::FontId::proportional(13.0),
                p.text_dim,
            );
            y -= 18.0;
        }
        painter.text(
            pos2(left, y),
            egui::Align2::LEFT_BOTTOM,
            subtitle_of(item),
            egui::FontId::proportional(15.0),
            p.text_dim,
        );
        y -= 24.0;
        painter.galley(pos2(left, y - title.size().y), title, p.text);
        let mut play = false;
        let mut info = false;
        let area = Rect::from_min_size(pos2(left, buttons_y), vec2(rect.right() - left, 40.0));
        if buttons.is_some() {
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let label = if resuming { t!("Resume watching") } else { t!("Play") };
                    play = crate::ui::widgets::primary_button(ui, p, &format!("{}  {label}", icons::PLAY)).clicked();
                    info = crate::ui::widgets::secondary_button(ui, &format!("{}  {}", icons::INFO, t!("More info")))
                        .clicked();
                });
            });
        }
        if play && item.kind.playable() {
            self.play_item(item, std::slice::from_ref(item));
        } else if buttons.is_some() && (play || info || resp.clicked()) {
            self.open(Page::Item(item.id));
        }
    }

    fn home_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        if self.media.home.is_none() {
            let c = client.clone();
            self.media.home = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/home").await }));
        }
        let rows = self.media.home.as_ref().and_then(ready).or_else(|| self.media.home_shown.clone().map(Ok));
        match rows {
            None => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Couldn't load the home screen"), &e)
            }
            Some(Ok(rows)) if rows.is_empty() => {
                let libs = self.media.libraries();
                if libs.is_empty() {
                    crate::ui::widgets::empty_state(
                        ui,
                        p,
                        icons::FILM_REEL,
                        t!("Your media library is empty"),
                        t!(
                            "Add a library (movies, shows, music…) with Manage libraries in the sidebar, pointing at your folders."
                        ),
                    );
                } else {
                    crate::ui::widgets::empty_state(
                        ui,
                        p,
                        icons::HOURGLASS,
                        t!("Nothing here yet"),
                        t!("The libraries are being scanned."),
                    );
                }
            }
            Some(Ok(rows)) => {
                let rows = self.arrange_home(client, rows);
                // The hero: what you were watching, else something with a backdrop, else the
                // newest thing with any picture.
                let resume = rows
                    .iter()
                    .find(|r| r.id == "continue")
                    .and_then(|r| r.items.iter().find(|i| !i.images.is_empty()))
                    .cloned();
                let backdrop = || {
                    rows.iter()
                        .flat_map(|r| r.items.iter())
                        .find(|i| i.images.iter().any(|im| im.kind == "backdrop"))
                        .or_else(|| {
                            rows.iter()
                                .flat_map(|r| r.items.iter())
                                .find(|i| kind_aspect(i.kind) > 1.0 && !i.images.is_empty())
                        })
                        .cloned()
                };
                match resume {
                    Some(it) => self.hero(ui, p, client, &it, Some(true)),
                    None => {
                        if let Some(it) = backdrop() {
                            self.hero(ui, p, client, &it, Some(false));
                        }
                    }
                }
                ui.add_space(12.0);
                for r in &rows {
                    let (max_rows, more) = match r.id.as_str() {
                        "continue" | "next_up" => (Some(1), Some(Page::Row(r.id.clone()))),
                        id => match id.strip_prefix("recent:").and_then(|n| n.parse().ok()) {
                            Some(lib) => (Some(2), Some(Page::Library(lib))),
                            None => (Some(2), Some(Page::Row(r.id.clone()))),
                        },
                    };
                    self.card_section(ui, p, client, &r.title, &r.items, max_rows, more);
                }
            }
        }
    }

    /// Orders and hides home rows as the user arranged them (in the web client; stored per
    /// user on the server, so every device shows the same home).
    fn arrange_home(&mut self, client: &MediaClient, mut rows: Vec<HomeRow>) -> Vec<HomeRow> {
        let c = client.clone();
        let prefs = self
            .media
            .user_prefs
            .get_or_insert_with(|| fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/me/prefs").await }));
        let Some(Ok(prefs)) = ready(prefs) else { return rows };
        let list = |k: &str| -> Vec<String> {
            prefs
                .get(k)
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default()
        };
        let (order, hidden) = (list("homeOrder"), list("homeHidden"));
        rows.retain(|r| !hidden.contains(&r.id));
        rows.sort_by_key(|r| order.iter().position(|o| *o == r.id).unwrap_or(usize::MAX));
        rows
    }

    fn library_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, id: i64) {
        use corrent_proto::library::{LibraryKind, SortBy};
        let width = ui.available_width() - 40.0;
        let lib_kind = self.media.libraries().iter().find(|l| l.id == id).map(|l| l.kind);
        let top_kind = match lib_kind {
            Some(LibraryKind::Music) => Some(ItemKind::Album),
            _ => None,
        };
        // The shape of the cards: home videos and photos are landscape, the rest posters.
        let aspect = match lib_kind {
            Some(LibraryKind::Videos) => kind_aspect(ItemKind::Video),
            Some(LibraryKind::Photos) => kind_aspect(ItemKind::Photo),
            _ => top_kind.map(kind_aspect).unwrap_or(2.0 / 3.0),
        };
        let (min, max) = grid::limits(aspect > 1.0, width);
        let spec = grid::spec(width, min, max, 18.0);
        let (card_w, gap, cols) = (spec.card_w, spec.gap, spec.cols as i64);
        let row_h = card_w / aspect + 44.0 + gap;
        let (sort, desc) = self.media.sort;
        // Load pages as they come into view.
        let g = self.media.grids.entry(id).or_default();
        let done: Vec<usize> =
            g.pending.iter().enumerate().filter(|(_, (_, s))| ready(s).is_some()).map(|(i, _)| i).collect();
        for i in done.into_iter().rev() {
            let (off, s) = g.pending.remove(i);
            if let Some(Ok(page)) = ready(&s) {
                g.total = Some(page.total);
                for (n, it) in page.items.into_iter().enumerate() {
                    g.items.insert(off + n as i64, it);
                }
            }
        }
        let request = |g: &mut Grid, off: i64, rt: &tokio::runtime::Handle, ctx: &egui::Context| {
            if g.requested.insert(off) {
                let c = client.clone();
                let kind = top_kind.map(|k| format!("&kind={}", k.as_str())).unwrap_or_default();
                let by = serde_json::to_value(sort).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
                let path =
                    format!("/api/v1/items?library_id={id}{kind}&sort={by}&desc={desc}&offset={off}&limit={PAGE}");
                g.pending.push((off, fetch(rt, ctx, async move { c.get(&path).await })));
            }
        };
        if g.total.is_none() {
            request(g, 0, &self.rt, &self.ctx);
        }
        let Some(total) = g.total else {
            ui.vertical_centered(|ui| ui.spinner());
            return;
        };
        if total == 0 {
            crate::ui::widgets::empty_state(
                ui,
                p,
                icons::FOLDER_OPEN,
                t!("This library is empty"),
                t!("Add files to its folders; they appear after the next scan."),
            );
            return;
        }
        let mut resort = None;
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(
                RichText::new(if total == 1 { t!("1 item").to_string() } else { t!("{n} items", n = total) })
                    .color(p.text_dim),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(20.0);
                let arrow = if desc { icons::SORT_DESCENDING } else { icons::SORT_ASCENDING };
                if ui
                    .add(egui::Button::new(RichText::new(arrow).size(16.0)).frame(false))
                    .on_hover_text(t!("Reverse order"))
                    .clicked()
                {
                    resort = Some((sort, !desc));
                }
                let name = |s: SortBy| match s {
                    SortBy::Title => t!("Title"),
                    SortBy::Added => t!("Date added"),
                    SortBy::Year => t!("Year"),
                    SortBy::Played => t!("Last watched"),
                    SortBy::Index => t!("Number"),
                    SortBy::Random => t!("Shuffle"),
                };
                egui::ComboBox::from_id_salt(("lib-sort", id)).selected_text(name(sort)).show_ui(ui, |ui| {
                    for s in [SortBy::Title, SortBy::Added, SortBy::Year, SortBy::Played, SortBy::Random] {
                        if ui.selectable_label(s == sort, name(s)).clicked() {
                            // Newest first is the useful order for dates.
                            resort = Some((s, matches!(s, SortBy::Added | SortBy::Played | SortBy::Year)));
                        }
                    }
                });
                ui.label(RichText::new(t!("Sort")).color(p.text_dim));
            });
        });
        if let Some(s) = resort {
            self.media.sort = s;
            // Fetched again in the new order.
            self.media.grids.remove(&id);
            return;
        }
        ui.add_space(8.0);
        let rows = (total + cols - 1) / cols;
        let first_row = ((ui.clip_rect().top() - ui.cursor().top()) / row_h).floor().max(0.0) as i64;
        let visible = (ui.clip_rect().height() / row_h).ceil() as i64 + 2;
        let start = (first_row - 1).max(0);
        let end = (first_row + visible).min(rows);
        // Space above the visible rows.
        ui.add_space(start as f32 * row_h);
        let mut clicked: Option<Item> = None;
        for r in start..end {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.spacing_mut().item_spacing.x = gap;
                for c in 0..cols {
                    let idx = r * cols + c;
                    if idx >= total {
                        break;
                    }
                    let item = self.media.grids.get(&id).and_then(|g| g.items.get(&idx).cloned());
                    // Every visible page is asked for (once; again after a refresh): what is
                    // shown stays until the fresh page replaces it.
                    let off = idx / PAGE * PAGE;
                    if let Some(g) = self.media.grids.get_mut(&id) {
                        request(g, off, &self.rt, &self.ctx);
                    }
                    match item {
                        Some(it) => {
                            if self.card(ui, p, client, &it, card_w).clicked() {
                                clicked = Some(it);
                            }
                        }
                        None => {
                            let (rect, _) =
                                ui.allocate_exact_size(vec2(card_w, card_w / aspect + 44.0), Sense::hover());
                            ui.painter().rect_filled(
                                Rect::from_min_size(rect.min, vec2(card_w, card_w / aspect)),
                                CornerRadius::same(10),
                                p.bg2,
                            );
                        }
                    }
                }
            });
            ui.add_space(gap);
        }
        ui.add_space((rows - end).max(0) as f32 * row_h);
        if let Some(it) = clicked {
            self.open(Page::Item(it.id));
        }
    }

    fn children(&mut self, client: &MediaClient, parent: i64) -> Option<Vec<Item>> {
        let slot = self.media.children.entry(parent).or_insert_with(|| {
            let c = client.clone();
            fetch(&self.rt, &self.ctx, async move {
                c.get::<ItemPage<Item>>(&format!("/api/v1/items?parent_id={parent}&sort=index&limit=500")).await
            })
        });
        ready(slot).and_then(Result::ok).map(|p| p.items)
    }

    fn item_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, id: i64) {
        if self.media.detail.as_ref().is_none_or(|(d, _)| *d != id) {
            let c = client.clone();
            self.media.detail =
                Some((id, fetch(&self.rt, &self.ctx, async move { c.get(&format!("/api/v1/items/{id}")).await })));
        }
        let d = self.media.detail.as_ref().and_then(|(_, s)| ready(s));
        let d = match d {
            None => {
                ui.vertical_centered(|ui| ui.spinner());
                return;
            }
            Some(Err(e)) => {
                crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Couldn't load this item"), &e);
                return;
            }
            Some(Ok(d)) => d,
        };
        let it = d.item.clone();
        self.hero(ui, p, client, &it, None);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(32.0);
            // Facts line.
            let mut facts: Vec<String> = Vec::new();
            if let Some(y) = it.year {
                facts.push(y.to_string());
            }
            if let Some(r) = it.runtime_ms.filter(|r| *r > 0) {
                facts.push(player::fmt_time(r as f64 / 1000.0));
            }
            if let Some(r) = &d.official_rating {
                facts.push(r.clone());
            }
            // Rating sites (OMDb) when known, else the provider's own score.
            let site =
                |k: &str| d.extra.get("ratings").and_then(|r| r.get(k)).and_then(|v| v.as_str()).map(String::from);
            let imdb = site("imdb");
            match (&imdb, d.rating) {
                (Some(r), _) => facts.push(format!("IMDb {r}")),
                (None, Some(r)) => facts.push(format!("★ {r:.1}")),
                (None, None) => {}
            }
            if let Some(rt) = site("rotten_tomatoes") {
                facts.push(format!("Rotten Tomatoes {rt}"));
            }
            if let Some(mc) = site("metacritic") {
                facts.push(format!("Metacritic {mc}"));
            }
            ui.label(RichText::new(facts.join("  ·  ")).color(p.text_dim));
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(32.0);
            let siblings: Vec<Item> = match it.kind {
                ItemKind::Season | ItemKind::Album => self.children(client, it.id).unwrap_or_default(),
                _ => vec![it.clone()],
            };
            let label = if it.state.position_ms > 0 && !it.state.played { t!("Resume") } else { t!("Play") };
            let playable =
                it.kind.playable() || !siblings.is_empty() && matches!(it.kind, ItemKind::Season | ItemKind::Album);
            if playable
                && crate::ui::widgets::primary_button(ui, p, &format!("{}  {label}", egui_phosphor::regular::PLAY))
                    .clicked()
            {
                if it.kind.playable() {
                    self.play_item(&it, std::slice::from_ref(&it));
                } else {
                    let first = siblings.iter().find(|s| !s.state.played).or(siblings.first()).cloned();
                    if let Some(f) = first {
                        self.play_item(&f, &siblings);
                    }
                }
            }
            let played = it.state.played || it.state.unplayed == Some(0);
            if ui
                .button(if played {
                    format!("{}  {}", icons::CHECK_CIRCLE, t!("Played"))
                } else {
                    format!("{}  {}", icons::CIRCLE, t!("Mark played"))
                })
                .clicked()
            {
                self.set_state(client, it.id, serde_json::json!({ "played": !played }));
            }
            let fav = it.state.favorite;
            if ui
                .button(if fav { egui_phosphor::regular::HEART } else { icons::HEART })
                .on_hover_text(t!("Favorite"))
                .clicked()
            {
                self.set_state(client, it.id, serde_json::json!({ "favorite": !fav }));
            }
            if matches!(it.kind, ItemKind::Movie | ItemKind::Episode | ItemKind::Video)
                && ui.button(format!("{}  {}", icons::CLOSED_CAPTIONING, t!("Find subtitles"))).clicked()
            {
                self.open_subtitle_finder(it.id);
            }
            if matches!(it.kind, ItemKind::Movie | ItemKind::Show | ItemKind::Video)
                && ui
                    .button(format!("{}  {}", icons::MAGNIFYING_GLASS, t!("Identify…")))
                    .on_hover_text(t!("Choose what this is from the metadata providers"))
                    .clicked()
            {
                self.open_identify(it.id, &it.title, it.year);
            }
            if matches!(
                it.kind,
                ItemKind::Movie | ItemKind::Show | ItemKind::Video | ItemKind::Album | ItemKind::Artist
            ) && let Some(mode) = refresh::refresh_menu(ui)
            {
                self.refresh_item(client, it.id, mode);
            }
            // Pages about it elsewhere (ids the providers gave).
            let tmdb_kind = if it.kind == ItemKind::Show { "tv" } else { "movie" };
            for (label, url) in [
                ("IMDb", d.provider_ids.get("imdb").map(|id| format!("https://www.imdb.com/title/{id}/"))),
                ("TMDB", d.provider_ids.get("tmdb").map(|id| format!("https://www.themoviedb.org/{tmdb_kind}/{id}"))),
            ] {
                if let Some(url) = url
                    && ui.button(format!("{}  {label}", icons::ARROW_SQUARE_OUT)).on_hover_text(&url).clicked()
                {
                    let _ = open::that_detached(&url);
                }
            }
            self.download_button(ui, client, &it, &siblings);
            if it.kind.playable() || matches!(it.kind, ItemKind::Season | ItemKind::Album) {
                self.add_to_playlist_button(ui, client, it.id);
            }
        });
        ui.add_space(12.0);
        if !d.genres.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                for g in &d.genres {
                    crate::ui::widgets::chip(ui, p, icons::TAG, g);
                }
            });
            ui.add_space(8.0);
        }
        if let Some(o) = &d.overview {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                ui.set_max_width(820.0);
                ui.label(RichText::new(o).size(14.5).color(p.text));
            });
            ui.add_space(14.0);
        }
        match it.kind {
            ItemKind::Show => {
                let seasons = self.children(client, it.id).unwrap_or_default();
                if !seasons.is_empty() {
                    let sel = *self.media.selected_season.entry(it.id).or_insert_with(|| {
                        seasons
                            .iter()
                            .find(|s| s.state.unplayed.unwrap_or(0) > 0)
                            .or(seasons.first())
                            .map(|s| s.id)
                            .unwrap_or(0)
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(32.0);
                        for s in &seasons {
                            if ui.selectable_label(s.id == sel, &s.title).clicked() {
                                self.media.selected_season.insert(it.id, s.id);
                            }
                        }
                    });
                    ui.add_space(10.0);
                    let eps = self.children(client, sel).unwrap_or_default();
                    self.episode_list(ui, p, client, &eps);
                }
            }
            ItemKind::Season => {
                let eps = self.children(client, it.id).unwrap_or_default();
                self.episode_list(ui, p, client, &eps);
            }
            ItemKind::Album => {
                let tracks = self.children(client, it.id).unwrap_or_default();
                self.track_list(ui, p, client, &tracks);
            }
            ItemKind::Artist => {
                let albums = self.children(client, it.id).unwrap_or_default();
                self.card_section(ui, p, client, t!("Albums"), &albums, None, None);
            }
            _ => {}
        }
        let cast: Vec<_> = d.credits.iter().filter(|c| c.role == "actor").take(20).collect();
        if !cast.is_empty() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                ui.label(RichText::new(t!("Cast")).size(17.0).strong().color(p.text));
            });
            let mut open_person = None;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                for c in cast {
                    ui.vertical(|ui| {
                        ui.set_width(150.0);
                        let name = egui::Button::new(RichText::new(&c.name).strong().color(p.text)).frame(false);
                        if ui.add(name).clicked() {
                            open_person = Some((c.person_id, c.name.clone()));
                        }
                        if let Some(ch) = &c.character {
                            ui.label(RichText::new(ch).small().color(p.text_dim));
                        }
                    });
                }
            });
            if let Some((id, name)) = open_person {
                self.open(Page::Person(id, name));
                return;
            }
        }
        let crew: Vec<_> = d.credits.iter().filter(|c| c.role != "actor").collect();
        if !crew.is_empty() {
            ui.add_space(10.0);
            let mut open_person = None;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                for c in crew {
                    let text = RichText::new(format!("{} ({})", c.name, c.role)).color(p.text_dim);
                    if ui.add(egui::Button::new(text).frame(false)).clicked() {
                        open_person = Some((c.person_id, c.name.clone()));
                    }
                }
            });
            if let Some((id, name)) = open_person {
                self.open(Page::Person(id, name));
                return;
            }
        }
        if let Some(f) = d.files.first() {
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                let mut facts = vec![
                    f.path.rsplit(['/', '\\']).next().unwrap_or("").to_string(),
                    crate::util::fmt_bytes(f.size as u64),
                ];
                if let Some(v) = f.streams.iter().find(|s| s.kind == "video") {
                    facts.push(format!(
                        "{} {}×{}",
                        v.codec.clone().unwrap_or_default(),
                        v.width.unwrap_or(0),
                        v.height.unwrap_or(0)
                    ));
                }
                for a in f.streams.iter().filter(|s| s.kind == "audio").take(3) {
                    facts.push(format!(
                        "{} {}ch {}",
                        a.codec.clone().unwrap_or_default(),
                        a.channels.unwrap_or(2),
                        a.lang.clone().unwrap_or_default()
                    ));
                }
                ui.label(RichText::new(facts.join("  ·  ")).small().color(p.text_mute));
            });
        }
    }

    fn episode_list(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, eps: &[Item]) {
        let mut play: Option<Item> = None;
        for e in eps {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                let r = self.card(ui, p, client, e, 220.0);
                if r.clicked() {
                    play = Some(e.clone());
                }
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("{}. {}", e.index.unwrap_or(0), e.title))
                            .size(15.0)
                            .strong()
                            .color(p.text),
                    );
                    if let Some(r) = e.runtime_ms {
                        ui.label(RichText::new(player::fmt_time(r as f64 / 1000.0)).small().color(p.text_dim));
                    }
                });
            });
            ui.add_space(8.0);
        }
        if let Some(e) = play {
            self.play_item(&e, eps);
        }
    }

    fn track_list(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, tracks: &[Item]) {
        let mut play: Option<Item> = None;
        for t in tracks {
            ui.horizontal(|ui| {
                ui.add_space(32.0);
                let resp = ui.add(
                    egui::Button::new(
                        RichText::new(format!("{:>2}   {}", t.index.unwrap_or(0), t.title)).size(14.5).color(p.text),
                    )
                    .frame(false)
                    .min_size(vec2(520.0, 28.0)),
                );
                if resp.clicked() {
                    play = Some(t.clone());
                }
                self.item_menu(&resp, client, t, tracks, None);
                if let Some(r) = t.runtime_ms {
                    ui.label(RichText::new(player::fmt_time(r as f64 / 1000.0)).color(p.text_dim));
                }
            });
        }
        if let Some(t) = play {
            self.play_item(&t, tracks);
        }
    }

    fn search_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        if self.tv.on {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                if self.tv.keyboard {
                    self.tv_keyboard(ui, p);
                } else if ui.button(format!("{}  {}", icons::KEYBOARD, t!("On-screen keyboard"))).clicked() {
                    self.tv.keyboard = true;
                }
            });
            ui.add_space(8.0);
        }
        let q = self.media.query.trim().to_string();
        if q.is_empty() {
            crate::ui::widgets::empty_state(
                ui,
                p,
                icons::MAGNIFYING_GLASS,
                t!("Search your libraries"),
                t!("Titles, people, albums and artists."),
            );
            return;
        }
        if self.media.search.as_ref().is_none_or(|(s, _)| *s != q) {
            let c = client.clone();
            let enc: String = url::form_urlencoded::byte_serialize(q.as_bytes()).collect();
            self.media.search = Some((
                q.clone(),
                fetch(&self.rt, &self.ctx, async move { c.get(&format!("/api/v1/library/search?q={enc}")).await }),
            ));
        }
        match self.media.search.as_ref().and_then(|(_, s)| ready(s)) {
            None => {
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Search failed"), &e),
            Some(Ok(r)) if r.items.is_empty() && r.people.is_empty() => {
                crate::ui::widgets::empty_state(ui, p, icons::MAGNIFYING_GLASS, t!("No matches"), &q)
            }
            Some(Ok(r)) => {
                if !r.people.is_empty() {
                    let mut open_person = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(24.0);
                        ui.label(RichText::new(t!("People")).strong().color(p.text));
                        for c in r.people.iter().take(20) {
                            if ui.button(format!("{}  {}", icons::USER, c.name)).clicked() {
                                open_person = Some((c.person_id, c.name.clone()));
                            }
                        }
                    });
                    ui.add_space(8.0);
                    if let Some((id, name)) = open_person {
                        self.open(Page::Person(id, name));
                        return;
                    }
                }
                if !r.items.is_empty() {
                    self.card_section(ui, p, client, t!("Results"), &r.items, None, None);
                }
            }
        }
    }

    /// Everything a person is credited in, as a wrapping grid of cards.
    fn person_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, id: i64) {
        if self.media.person.as_ref().is_none_or(|(pid, _)| *pid != id) {
            let c = client.clone();
            self.media.person =
                Some((
                    id,
                    fetch(&self.rt, &self.ctx, async move {
                        c.get(&format!("/api/v1/items?person_id={id}&limit=500")).await
                    }),
                ));
        }
        match self.media.person.as_ref().and_then(|(_, s)| ready(s)) {
            None => {
                ui.vertical_centered(|ui| ui.spinner());
            }
            Some(Err(e)) => {
                crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Couldn't load this item"), &e)
            }
            Some(Ok(page)) if page.items.is_empty() => {
                crate::ui::widgets::empty_state(ui, p, icons::USER, t!("Nothing here yet"), "")
            }
            Some(Ok(page)) => {
                let mut clicked = None;
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(24.0);
                    ui.spacing_mut().item_spacing = vec2(16.0, 18.0);
                    for it in &page.items {
                        if self.card(ui, p, client, it, 150.0).clicked() {
                            clicked = Some(it.clone());
                        }
                    }
                });
                if let Some(it) = clicked {
                    self.open(Page::Item(it.id));
                }
            }
        }
    }

    fn set_state(&mut self, client: &MediaClient, id: i64, body: serde_json::Value) {
        let c = client.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let _: Result<serde_json::Value, String> =
                c.send(reqwest::Method::PUT, &format!("/api/v1/items/{id}/state"), &body).await;
            ctx.request_repaint();
        });
        self.media.detail = None;
        self.media.home = None;
        self.media.children.clear();
    }

    /// The "Media" section of the sidebar: home, each library, search.
    pub fn media_sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        let mut c = self.prefs.collapsed.contains(&"media".to_string());
        super::sidebar::section(ui, p, t!("Media"), None, &mut c);
        self.prefs.collapsed.retain(|k| k != "media");
        if c {
            self.prefs.collapsed.push("media".into());
            ui.add_space(10.0);
            return;
        }
        let in_media = self.main_view == super::feeds::MainView::Media;
        let page = self.media.page.clone().unwrap_or(Page::Home);
        if super::sidebar::row(ui, p, icons::HOUSE, p.accent, t!("Home"), None, in_media && page == Page::Home)
            .clicked()
        {
            self.main_view = super::feeds::MainView::Media;
            self.open(Page::Home);
        }
        for l in self.media.libraries() {
            let icon = match l.kind {
                corrent_proto::library::LibraryKind::Movies => icons::FILM_SLATE,
                corrent_proto::library::LibraryKind::Shows => icons::TELEVISION,
                corrent_proto::library::LibraryKind::Music => icons::MUSIC_NOTES,
                corrent_proto::library::LibraryKind::Audiobooks => icons::BOOK_OPEN,
                corrent_proto::library::LibraryKind::Photos => icons::IMAGES,
                corrent_proto::library::LibraryKind::Videos => icons::VIDEO_CAMERA,
                corrent_proto::library::LibraryKind::Podcasts => icons::MICROPHONE,
            };
            let count = Some(l.item_count.max(0) as usize);
            if super::sidebar::row(ui, p, icon, p.text_dim, &l.name, count, in_media && page == Page::Library(l.id))
                .clicked()
            {
                self.main_view = super::feeds::MainView::Media;
                self.open(Page::Library(l.id));
            }
        }
        if super::sidebar::row(
            ui,
            p,
            icons::GEAR_SIX,
            p.text_mute,
            t!("Manage libraries"),
            None,
            in_media && page == Page::Libraries,
        )
        .clicked()
        {
            self.main_view = super::feeds::MainView::Media;
            self.open(Page::Libraries);
        }
        // Proposals waiting show as a count.
        if let Some(c) = self.media.client.clone() {
            self.agent_tick(&c, in_media && page == Page::Agent);
        }
        let waiting = self.media.agent.pending();
        if super::sidebar::row(
            ui,
            p,
            icons::SPARKLE,
            if waiting > 0 { p.accent } else { p.text_mute },
            t!("AI agent"),
            (waiting > 0).then_some(waiting),
            in_media && page == Page::Agent,
        )
        .clicked()
        {
            self.main_view = super::feeds::MainView::Media;
            self.open(Page::Agent);
        }
        ui.add_space(10.0);
    }

    /// The music mini player (bottom of the window) while audio plays.
    pub fn media_mini_player(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(client) = self.media.client.clone() else { return };
        let poster = self.media.player.as_ref().and_then(|pl| pl.current()).and_then(|e| e.poster.clone());
        let Some(pl) = self.media.player.as_mut().filter(|pl| pl.audio_only) else { return };
        let tex = poster.and_then(|sha| self.media.images.get(&client, &self.rt, &self.ctx, &sha, 200));
        let action = ui
            .horizontal(|ui| {
                match &tex {
                    Some(t) => {
                        ui.add(egui::Image::new(t).fit_to_exact_size(vec2(40.0, 40.0)).corner_radius(6));
                    }
                    None => {
                        ui.label(RichText::new(icons::MUSIC_NOTES).size(28.0).color(p.text_mute));
                    }
                }
                pl.mini(ui, p)
            })
            .inner;
        if action == PlayerAction::Close {
            self.media.player = None;
        }
    }
}

/// UV rect cropping a texture to fill `target` (CSS `object-fit: cover`).
pub(super) fn cover_uv(tex: egui::Vec2, target: egui::Vec2) -> Rect {
    let (ta, ra) = (tex.x / tex.y.max(1.0), target.x / target.y.max(1.0));
    if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}
