//! The media side of the desktop app: home, libraries, details, search and the player.
//!
//! The app hosts the media server in-process (loopback only) and talks to it through the same
//! API as remote clients, so the pages work identically against a remote server.

pub mod client;
pub mod images;
pub mod player;

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
}

const PAGE: i64 = 120;

#[derive(Default)]
struct Grid {
    total: Option<i64>,
    items: HashMap<i64, Item>,
    requested: HashSet<i64>,
    pending: Vec<(i64, Slot<ItemPage<Item>>)>,
}

#[derive(Default)]
pub struct MediaState {
    local: Option<Arc<corrent_server::local::LocalServer>>,
    starting: Option<Slot<Arc<corrent_server::local::LocalServer>>>,
    pub client: Option<MediaClient>,
    pub error: Option<String>,
    pub page: Option<Page>,
    back: Vec<Page>,
    libraries: Option<Slot<Vec<Library>>>,
    home: Option<Slot<Vec<HomeRow>>>,
    grids: HashMap<i64, Grid>,
    detail: Option<(i64, Slot<ItemDetail>)>,
    children: HashMap<i64, Slot<ItemPage<Item>>>,
    pub query: String,
    search: Option<(String, Slot<SearchResults>)>,
    images: ImageCache,
    pub player: Option<Player>,
    pub gl: Option<GlAccess>,
    selected_season: HashMap<i64, i64>,
    refreshed: Option<std::time::Instant>,
}

impl MediaState {
    pub fn libraries(&self) -> Vec<Library> {
        self.libraries.as_ref().and_then(ready).and_then(Result::ok).unwrap_or_default()
    }

    pub fn local(&self) -> Option<&Arc<corrent_server::local::LocalServer>> {
        self.local.as_ref()
    }

    fn invalidate(&mut self) {
        self.home = None;
        self.libraries = None;
        self.grids.clear();
        self.children.clear();
        self.detail = None;
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
    #[cfg(debug_assertions)]
    pub(crate) fn media_dev_hooks(&mut self, ctx: &egui::Context) {
        if !self.media_dev_done {
            self.media_dev_done = true;
            if std::env::var("CORRENT_DEV_VIEW").as_deref() == Ok("media") {
                self.main_view = super::feeds::MainView::Media;
            }
            if let Some(id) = std::env::var("CORRENT_DEV_ITEM").ok().and_then(|v| v.parse().ok()) {
                self.main_view = super::feeds::MainView::Media;
                self.open(Page::Item(id));
            }
        }
        if let Some(id) = std::env::var("CORRENT_DEV_PLAY").ok().and_then(|v| v.parse::<i64>().ok())
            && self.media.player.is_none()
            && self.media.client.is_some()
            && !self.media_dev_played
        {
            self.media_dev_played = true;
            self.main_view = super::feeds::MainView::Media;
            let e =
                QueueEntry { item_id: id, title: String::new(), subtitle: String::new(), resume_ms: 0, poster: None };
            self.play_queue(vec![e], false);
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
        let (engine, settings, paths) = (self.engine.clone(), self.remote.shared.clone(), self.paths.clone());
        m.starting = Some(fetch(&self.rt, &self.ctx, async move {
            corrent_server::local::start(engine, settings, paths).await.map(Arc::new).map_err(|e| format!("{e:#}"))
        }));
    }

    /// Per-frame media work: finishing server start, uploading images, player housekeeping.
    pub fn media_tick(&mut self, ctx: &egui::Context) {
        // The server runs from the start: imports of completed downloads need it too.
        self.ensure_media_server();
        if let Some(s) = self.media.starting.as_ref().and_then(ready) {
            self.media.starting = None;
            match s {
                Ok(local) => match MediaClient::new(local.addr(), &local.token, None) {
                    Ok(c) => {
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
            self.media.invalidate();
        }
    }

    fn open(&mut self, page: Page) {
        if let Some(cur) = self.media.page.take()
            && cur != page
        {
            self.media.back.push(cur);
        }
        self.media.page = Some(page);
    }

    fn play_queue(&mut self, queue: Vec<QueueEntry>, audio_only: bool) {
        let Some(client) = self.media.client.clone() else { return };
        if queue.is_empty() {
            return;
        }
        self.media.player = None;
        match Player::new(client, self.rt.clone(), &self.ctx, self.media.gl.as_ref(), queue, audio_only) {
            Ok(p) => self.media.player = Some(p),
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
        }
    }

    /// Plays an item; for an episode or track, the rest of its season/album follows.
    fn play_item(&mut self, item: &Item, siblings: &[Item]) {
        let start = siblings.iter().position(|s| s.id == item.id);
        let queue: Vec<QueueEntry> = match start {
            Some(i) => siblings[i..].iter().filter(|s| s.kind.playable()).map(Self::entry).collect(),
            None => vec![Self::entry(item)],
        };
        self.play_queue(queue, audio_kind(item.kind));
    }

    /// The media view (central panel).
    pub fn media_view(&mut self, ui: &mut Ui, p: &Palette) {
        self.ensure_media_server();
        if self.media.player.as_ref().is_some_and(|pl| !pl.audio_only) {
            let action = self.media.player.as_mut().map(|pl| pl.show(ui, p)).unwrap_or(PlayerAction::None);
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
        let page = self.media.page.clone().unwrap_or(Page::Home);
        self.media_top_bar(ui, p, &page);
        egui::ScrollArea::vertical().id_salt(("media", format!("{page:?}"))).auto_shrink([false, false]).show(
            ui,
            |ui| {
                ui.add_space(8.0);
                match page {
                    Page::Home => self.home_page(ui, p, &client),
                    Page::Library(id) => self.library_page(ui, p, &client, id),
                    Page::Item(id) => self.item_page(ui, p, &client, id),
                    Page::Search => self.search_page(ui, p, &client),
                }
                ui.add_space(24.0);
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
                self.media.page = self.media.back.pop();
            }
            if ui
                .add(egui::Button::new(RichText::new(icons::HOUSE).size(18.0)).frame(false))
                .on_hover_text(t!("Home"))
                .clicked()
            {
                self.open(Page::Home);
            }
            let title = match page {
                Page::Home => t!("Home").to_string(),
                Page::Library(id) => {
                    self.media.libraries().iter().find(|l| l.id == *id).map(|l| l.name.clone()).unwrap_or_default()
                }
                Page::Item(_) => String::new(),
                Page::Search => t!("Search").to_string(),
            };
            ui.label(RichText::new(title).size(20.0).strong().color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
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
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.15);
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
        let wanted = if aspect >= 1.0 { "thumb" } else { "poster" };
        let image = item
            .images
            .iter()
            .find(|i| i.kind == wanted)
            .or_else(|| item.images.iter().find(|i| i.kind == "poster"))
            .or_else(|| item.images.first())
            .cloned();
        let ppp = ui.ctx().pixels_per_point();
        let mut drawn = false;
        if let Some(im) = &image {
            if let Some(tex) = self.media.images.get(client, &self.rt, &self.ctx, &im.sha, (width * ppp) as u32) {
                let t = ui.ctx().animate_bool_with_time(resp.id.with("in"), true, 0.25);
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
            painter.rect_filled(img_rect, radius, p.bg2);
            let icon = match item.kind {
                ItemKind::Album | ItemKind::Track | ItemKind::Artist => icons::MUSIC_NOTES,
                ItemKind::Photo => icons::IMAGE,
                ItemKind::Book => icons::BOOK_OPEN,
                _ => icons::FILM_STRIP,
            };
            painter.text(
                img_rect.center(),
                egui::Align2::CENTER_CENTER,
                icon,
                egui::FontId::proportional(width / 4.0),
                p.text_mute,
            );
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
        let text_top = rect.top() + h + 6.0;
        let clip = Rect::from_min_size(pos2(rect.left(), text_top), vec2(width, 40.0));
        let p2 = painter.with_clip_rect(clip);
        p2.text(
            pos2(rect.left() + 2.0, text_top),
            egui::Align2::LEFT_TOP,
            &item.title,
            egui::FontId::proportional(13.5),
            p.text,
        );
        p2.text(
            pos2(rect.left() + 2.0, text_top + 19.0),
            egui::Align2::LEFT_TOP,
            subtitle_of(item),
            egui::FontId::proportional(12.0),
            p.text_dim,
        );
        resp.on_hover_text(&item.title)
    }

    fn row_of_cards(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        client: &MediaClient,
        title: &str,
        items: &[Item],
        width: f32,
    ) {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new(title).size(17.0).strong().color(p.text));
        });
        ui.add_space(6.0);
        egui::ScrollArea::horizontal().id_salt(("row", title)).auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.spacing_mut().item_spacing.x = 16.0;
                for (n, it) in items.iter().enumerate() {
                    let w = if kind_aspect(it.kind) > 1.0 { width * 1.6 } else { width };
                    if self.card(ui, p, client, it, w).clicked() {
                        if it.kind.playable() && !matches!(it.kind, ItemKind::Track) {
                            self.play_item(it, &items[n..n + 1]);
                        } else {
                            self.open(Page::Item(it.id));
                        }
                    }
                }
                ui.add_space(20.0);
            });
        });
        ui.add_space(18.0);
    }

    fn hero(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, item: &Item) {
        let width = ui.available_width();
        let h = (width * 0.32).clamp(220.0, 420.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
        let backdrop = item
            .images
            .iter()
            .find(|i| i.kind == "backdrop")
            .or_else(|| item.images.iter().find(|i| i.kind == "thumb"))
            .cloned();
        let painter = ui.painter().clone();
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
        // Fade into the page.
        let mut mesh = egui::Mesh::default();
        let (top, bottom) = (Color32::TRANSPARENT, p.bg0);
        mesh.colored_vertex(pos2(rect.left(), rect.top() + h * 0.35), top);
        mesh.colored_vertex(pos2(rect.right(), rect.top() + h * 0.35), top);
        mesh.colored_vertex(rect.left_bottom(), bottom);
        mesh.colored_vertex(rect.right_bottom(), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        painter.add(mesh);
        painter.text(
            pos2(rect.left() + 32.0, rect.bottom() - 70.0),
            egui::Align2::LEFT_BOTTOM,
            &item.title,
            egui::FontId::proportional(34.0),
            p.text,
        );
        painter.text(
            pos2(rect.left() + 34.0, rect.bottom() - 44.0),
            egui::Align2::LEFT_BOTTOM,
            subtitle_of(item),
            egui::FontId::proportional(15.0),
            p.text_dim,
        );
        if resp.clicked() {
            self.open(Page::Item(item.id));
        }
    }

    fn home_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        if self.media.home.is_none() {
            let c = client.clone();
            self.media.home = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/home").await }));
        }
        let rows = self.media.home.as_ref().and_then(ready);
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
                            "Add a library (movies, shows, music…) in the web interface or Settings → Media, pointing at your folders."
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
                if let Some(first) = rows
                    .iter()
                    .flat_map(|r| r.items.iter())
                    .find(|i| i.images.iter().any(|im| im.kind == "backdrop"))
                    .cloned()
                {
                    self.hero(ui, p, client, &first);
                    ui.add_space(8.0);
                }
                for r in &rows {
                    self.row_of_cards(ui, p, client, &r.title, &r.items, 150.0);
                }
            }
        }
    }

    fn library_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, id: i64) {
        let width = ui.available_width() - 40.0;
        let card_w = 160.0;
        let gap = 18.0;
        let lib_kind = self.media.libraries().iter().find(|l| l.id == id).map(|l| l.kind);
        let top_kind = match lib_kind {
            Some(corrent_proto::library::LibraryKind::Music) => Some(ItemKind::Album),
            _ => None,
        };
        let cols = ((width + gap) / (card_w + gap)).floor().max(1.0) as i64;
        let aspect = top_kind.map(kind_aspect).unwrap_or(2.0 / 3.0);
        let row_h = card_w / aspect + 44.0 + gap;
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
                let path = format!("/api/v1/items?library_id={id}{kind}&offset={off}&limit={PAGE}");
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
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(
                RichText::new(if total == 1 { t!("1 item").to_string() } else { t!("{n} items", n = total) })
                    .color(p.text_dim),
            );
        });
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
                    match item {
                        Some(it) => {
                            if self.card(ui, p, client, &it, card_w).clicked() {
                                clicked = Some(it);
                            }
                        }
                        None => {
                            let off = idx / PAGE * PAGE;
                            if let Some(g) = self.media.grids.get_mut(&id) {
                                request(g, off, &self.rt, &self.ctx);
                            }
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
        self.hero(ui, p, client, &it);
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
            if let Some(r) = d.rating {
                facts.push(format!("★ {r:.1}"));
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
                self.track_list(ui, p, &tracks);
            }
            ItemKind::Artist => {
                let albums = self.children(client, it.id).unwrap_or_default();
                self.row_of_cards(ui, p, client, t!("Albums"), &albums, 160.0);
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
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                for c in cast {
                    ui.vertical(|ui| {
                        ui.set_width(150.0);
                        ui.label(RichText::new(&c.name).strong().color(p.text));
                        if let Some(ch) = &c.character {
                            ui.label(RichText::new(ch).small().color(p.text_dim));
                        }
                    });
                }
            });
        }
        let crew: Vec<String> =
            d.credits.iter().filter(|c| c.role != "actor").map(|c| format!("{} ({})", c.name, c.role)).collect();
        if !crew.is_empty() {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(32.0);
                ui.label(RichText::new(crew.join(" · ")).color(p.text_dim));
            });
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

    fn track_list(&mut self, ui: &mut Ui, p: &Palette, tracks: &[Item]) {
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
            Some(Ok(r)) if r.items.is_empty() => {
                crate::ui::widgets::empty_state(ui, p, icons::MAGNIFYING_GLASS, t!("No matches"), &q)
            }
            Some(Ok(r)) => self.row_of_cards(ui, p, client, t!("Results"), &r.items, 150.0),
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
            };
            let count = Some(l.item_count.max(0) as usize);
            if super::sidebar::row(ui, p, icon, p.text_dim, &l.name, count, in_media && page == Page::Library(l.id))
                .clicked()
            {
                self.main_view = super::feeds::MainView::Media;
                self.open(Page::Library(l.id));
            }
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
fn cover_uv(tex: egui::Vec2, target: egui::Vec2) -> Rect {
    let (ta, ra) = (tex.x / tex.y.max(1.0), target.x / target.y.max(1.0));
    if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}
