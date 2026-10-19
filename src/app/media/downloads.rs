//! The Downloads page and the item page's download button (offline copies, see `offline`).

use corrent_proto::library::{Item, ItemKind};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::MediaClient;
use super::offline::{Quality, State, Store};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;

impl CorrentApp {
    /// The offline store (remote servers only: a local server's files are already here).
    /// `None` until its index is read (in the background).
    pub(super) fn offline_store(&mut self) -> Option<std::sync::Arc<Store>> {
        self.server.as_ref()?;
        if let Some(s) = &self.media.offline {
            return Some(s.clone());
        }
        match &self.media.offline_loading {
            None => {
                let dir = self.paths.data_dir.clone();
                self.media.offline_loading = Some(super::client::fetch(&self.rt, &self.ctx, async move {
                    tokio::task::spawn_blocking(move || Store::open(&dir)).await.map_err(|e| e.to_string())
                }));
                None
            }
            Some(slot) => {
                let s = super::client::ready(slot)?.ok()?;
                self.media.offline = Some(s.clone());
                self.media.offline_loading = None;
                Some(s)
            }
        }
    }

    /// Sends progress made offline, once a minute while some is waiting.
    pub(super) fn flush_offline(&mut self, client: &MediaClient) {
        let Some(store) = self.offline_store() else { return };
        if !self.media.offline_resumed {
            self.media.offline_resumed = true;
            store.resume_queued(client, &self.rt, &self.ctx);
        }
        if self.media.flushed.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60))
            || !store.has_pending(client.base())
        {
            return;
        }
        self.media.flushed = Some(std::time::Instant::now());
        let c = client.clone();
        self.rt.spawn(async move { store.flush(&c).await });
    }

    /// "Download": originals or a smaller copy; seasons and albums download every child.
    pub(super) fn download_button(&mut self, ui: &mut Ui, client: &MediaClient, it: &Item, children: &[Item]) {
        let Some(store) = self.offline_store() else { return };
        let targets: Vec<Item> = if it.kind.playable() {
            vec![it.clone()]
        } else if matches!(it.kind, ItemKind::Season | ItemKind::Album) {
            children.iter().filter(|c| c.kind.playable()).cloned().collect()
        } else {
            Vec::new()
        };
        if targets.is_empty() {
            return;
        }
        let have: Vec<_> = targets.iter().filter_map(|t| store.find(client.base(), t.id)).collect();
        if have.len() == targets.len() {
            let label = if have.iter().all(|d| d.state == State::Done) {
                t!("Downloaded")
            } else if have.iter().any(|d| matches!(d.state, State::Failed(_))) {
                t!("Download failed")
            } else if have.iter().any(|d| matches!(d.state, State::Running | State::Queued)) {
                t!("Downloading…")
            } else {
                t!("Paused")
            };
            if ui.button(format!("{}  {label}", icons::CHECK_CIRCLE)).on_hover_text(t!("Open downloads")).clicked() {
                self.open(super::Page::Downloads);
            }
            return;
        }
        let r = ui.button(format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Download")));
        let video = targets.iter().any(|t| !matches!(t.kind, ItemKind::Track | ItemKind::Book));
        crate::ui::widgets::popup(egui::Popup::from_toggle_button_response(&r), |ui| {
            ui.set_min_width(240.0);
            let mut pick = None;
            if ui.button(t!("Original file")).on_hover_text(t!("The file as it is on the server.")).clicked() {
                pick = Some(Quality::Original);
            }
            if video
                && ui
                    .button(t!("Smaller copy (720p)"))
                    .on_hover_text(t!("The server converts it while downloading; takes less space."))
                    .clicked()
            {
                pick = Some(Quality::Smaller);
            }
            if let Some(q) = pick {
                for t in &targets {
                    store.add(client, &self.rt, &self.ctx, t.clone(), q);
                }
                self.toasts.push(crate::engine::ToastKind::Info, t!("Downloading for offline use"));
            }
        });
    }

    pub(super) fn downloads_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        let Some(store) = self.offline_store() else { return };
        let list = store.downloads(client.base());
        if list.is_empty() {
            crate::ui::widgets::empty_state(
                ui,
                p,
                icons::DOWNLOAD_SIMPLE,
                t!("Nothing downloaded"),
                t!("Download movies, episodes and music from their pages to watch them without the server."),
            );
            return;
        }
        if list.iter().any(|d| d.state == State::Running) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
        let mut play = None;
        for d in &list {
            egui::Frame::new()
                .fill(p.bg2)
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(12)
                .outer_margin(egui::Margin { left: 24, right: 24, top: 0, bottom: 8 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&d.item.title).strong().color(p.text));
                            let what = match d.quality {
                                Quality::Original => t!("Original file"),
                                Quality::Smaller => t!("Smaller copy (720p)"),
                            };
                            let state = match &d.state {
                                State::Done if d.quality == Quality::Original => corrent_core::util::fmt_bytes(d.total),
                                State::Done => String::new(),
                                State::Queued => t!("Waiting").to_string(),
                                State::Paused => t!("Paused").to_string(),
                                State::Running if d.total == 0 => t!("Starting…").to_string(),
                                State::Running => match d.quality {
                                    Quality::Original => format!(
                                        "{} / {}",
                                        corrent_core::util::fmt_bytes(d.done),
                                        corrent_core::util::fmt_bytes(d.total)
                                    ),
                                    Quality::Smaller => t!("{pct}% converted", pct = d.done * 100 / d.total.max(1)),
                                },
                                State::Failed(e) => e.clone(),
                            };
                            let color = if matches!(d.state, State::Failed(_)) { p.error } else { p.text_dim };
                            let line: Vec<String> = [super::subtitle_of(&d.item), what.to_string(), state]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect();
                            ui.label(RichText::new(line.join(" · ")).small().color(color));
                            if matches!(d.state, State::Running | State::Paused) && d.total > 0 {
                                crate::ui::widgets::progress_bar(
                                    ui,
                                    ui.id().with(("dl", d.key)),
                                    d.done as f32 / d.total as f32,
                                    p.accent,
                                    p.bg3,
                                    4.0,
                                    280.0,
                                    false,
                                    d.state == State::Running,
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(icons::TRASH).on_hover_text(t!("Remove")).clicked() {
                                store.remove(d.key);
                            }
                            match &d.state {
                                State::Done => {
                                    if ui.button(format!("{}  {}", icons::PLAY, t!("Play"))).clicked() {
                                        play = Some(d.item.clone());
                                    }
                                }
                                State::Running => {
                                    if ui.button(icons::PAUSE).on_hover_text(t!("Pause")).clicked() {
                                        store.pause(d.key);
                                    }
                                }
                                State::Queued | State::Paused | State::Failed(_) => {
                                    if ui.button(icons::PLAY).on_hover_text(t!("Resume")).clicked() {
                                        store.start(client, &self.rt, &self.ctx, d.key);
                                    }
                                }
                            }
                        });
                    });
                });
        }
        if let Some(item) = play {
            self.play_item(&item, std::slice::from_ref(&item));
        }
    }

    /// Internet radio: the server's stations, played by mpv straight from their address.
    pub(super) fn radio_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        if self.media.radio.is_none() {
            let c = client.clone();
            self.media.radio =
                Some(super::client::fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/radio").await }));
        }
        let list = match self.media.radio.as_ref().and_then(super::client::ready) {
            None => {
                ui.vertical_centered(|ui| ui.spinner());
                return;
            }
            Some(Err(e)) => {
                crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Couldn't load the stations"), &e);
                return;
            }
            Some(Ok(l)) => l,
        };
        if list.is_empty() {
            crate::ui::widgets::empty_state(
                ui,
                p,
                icons::BROADCAST,
                t!("No stations yet"),
                t!("Stations are added in the web interface (Radio)."),
            );
            return;
        }
        let mut play = None;
        for s in &list {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                if crate::ui::widgets::primary_button(ui, p, icons::PLAY).on_hover_text(t!("Play")).clicked() {
                    play = Some(s.clone());
                }
                ui.label(RichText::new(&s.name).size(16.0).color(p.text));
            });
            ui.add_space(6.0);
        }
        if let Some(s) = play {
            let e = super::player::QueueEntry {
                item_id: -1,
                title: s.name,
                subtitle: t!("Radio").to_string(),
                resume_ms: 0,
                poster: None,
                local: None,
                stream: Some(s.url),
            };
            self.play_queue(vec![e], true);
        }
    }
}
