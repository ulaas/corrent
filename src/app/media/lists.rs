//! Playlists and collections in the desktop app: the list, a list's items (play, shuffle),
//! and "Add to playlist" on item pages.

use corrent_proto::library::{Collection, Item, Playlist};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::Page;
use super::client::{MediaClient, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;

impl CorrentApp {
    pub(super) fn playlists_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        if self.media.playlists.is_none() {
            let c = client.clone();
            self.media.playlists = Some(fetch(&self.rt, &self.ctx, async move {
                let lists: Vec<Playlist> = c.get("/api/v1/playlists").await?;
                let cols: Vec<Collection> = c.get("/api/v1/collections").await.unwrap_or_default();
                Ok((lists, cols))
            }));
        }
        let Some(r) = self.media.playlists.as_ref().and_then(ready) else {
            ui.vertical_centered(|ui| ui.spinner());
            return;
        };
        let (lists, cols) = match r {
            Ok(v) => v,
            Err(e) => {
                crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Couldn't load this item"), &e);
                return;
            }
        };
        let mut open = None;
        let section = |ui: &mut Ui, title: &str| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new(title).size(18.0).strong().color(p.text));
            });
            ui.add_space(4.0);
        };
        section(ui, t!("Playlists"));
        if lists.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new(t!("No playlists yet")).color(p.text_dim));
            });
        }
        for l in &lists {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                let n = if l.count == 1 { t!("1 item").to_string() } else { t!("{n} items", n = l.count) };
                if ui
                    .add(
                        egui::Button::new(RichText::new(format!("{}  {}", icons::PLAYLIST, l.name)).size(15.0))
                            .frame(false),
                    )
                    .clicked()
                {
                    open = Some(Page::Playlist(l.id, false));
                }
                let owner = if l.mine { String::new() } else { format!(" · {}", l.owner) };
                ui.label(RichText::new(format!("{n}{owner}")).small().color(p.text_dim));
            });
        }
        if !cols.is_empty() {
            section(ui, t!("Collections"));
            for c in &cols {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    if ui
                        .add(
                            egui::Button::new(RichText::new(format!("{}  {}", icons::STACK, c.name)).size(15.0))
                                .frame(false),
                        )
                        .clicked()
                    {
                        open = Some(Page::Playlist(c.id, true));
                    }
                    let n = if c.count == 1 { t!("1 item").to_string() } else { t!("{n} items", n = c.count) };
                    ui.label(RichText::new(n).small().color(p.text_dim));
                });
            }
        }
        if let Some(pg) = open {
            self.open(pg);
        }
    }

    /// A playlist's (or collection's) items.
    pub(super) fn playlist_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, id: i64, collection: bool) {
        if self.media.list_items.as_ref().is_none_or(|(k, _)| *k != (id, collection)) {
            let c = client.clone();
            let path = if collection {
                format!("/api/v1/collections/{id}/items")
            } else {
                format!("/api/v1/playlists/{id}/items")
            };
            self.media.list_items =
                Some(((id, collection), fetch(&self.rt, &self.ctx, async move { c.get::<Vec<Item>>(&path).await })));
        }
        let Some(r) = self.media.list_items.as_ref().and_then(|(_, s)| ready(s)) else {
            ui.vertical_centered(|ui| ui.spinner());
            return;
        };
        let items = match r {
            Ok(v) => v,
            Err(e) => {
                crate::ui::widgets::empty_state(ui, p, icons::WARNING_CIRCLE, t!("Couldn't load this item"), &e);
                return;
            }
        };
        let mut play: Option<(usize, bool)> = None;
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            if crate::ui::widgets::primary_button(ui, p, &format!("{}  {}", icons::PLAY, t!("Play"))).clicked() {
                play = Some((0, false));
            }
            if ui.button(format!("{}  {}", icons::SHUFFLE, t!("Shuffle"))).clicked() {
                play = Some((0, true));
            }
        });
        ui.add_space(8.0);
        for (i, it) in items.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new(format!("{:>3}", i + 1)).monospace().color(p.text_mute));
                if ui.add(egui::Button::new(RichText::new(&it.title).color(p.text)).frame(false)).clicked() {
                    play = Some((i, false));
                }
                if !it.context.is_empty() {
                    ui.label(RichText::new(it.context.join(" · ")).small().color(p.text_dim));
                }
            });
        }
        if let Some((start, shuffle)) = play
            && !items.is_empty()
        {
            let mut order: Vec<Item> = items.clone();
            if shuffle {
                // A simple shuffle (the time is random enough for the order of a playlist).
                let mut seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(1);
                for k in (1..order.len()).rev() {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    order.swap(k, (seed % (k as u64 + 1)) as usize);
                }
            }
            let first = order[if shuffle { 0 } else { start }].clone();
            let rest: Vec<Item> = if shuffle { order } else { order[start..].to_vec() };
            self.play_item(&first, &rest);
        }
    }

    /// "Add to playlist" (the caller's playlists, or a new one).
    pub(super) fn add_to_playlist_button(&mut self, ui: &mut Ui, client: &MediaClient, item: i64) {
        let r = ui.button(format!("{}  {}", icons::PLAYLIST, t!("Add to playlist")));
        if r.clicked() {
            let c = client.clone();
            self.media.my_playlists = Some(fetch(&self.rt, &self.ctx, async move {
                let all: Vec<Playlist> = c.get("/api/v1/playlists").await?;
                Ok(all.into_iter().filter(|p| p.mine).collect::<Vec<_>>())
            }));
        }
        let lists = self.media.my_playlists.as_ref().and_then(ready).and_then(Result::ok).unwrap_or_default();
        let mut add: Option<Option<i64>> = None;
        egui::Popup::from_toggle_button_response(&r).show(|ui| {
            ui.set_min_width(240.0);
            for l in &lists {
                if ui.button(&l.name).clicked() {
                    add = Some(Some(l.id));
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.media.new_playlist)
                        .hint_text(t!("New playlist"))
                        .desired_width(160.0),
                );
                if ui.add_enabled(!self.media.new_playlist.trim().is_empty(), egui::Button::new(icons::PLUS)).clicked()
                {
                    add = Some(None);
                }
            });
        });
        let Some(target) = add else { return };
        let c = client.clone();
        let tx = self.ui_events_tx.clone();
        let name = std::mem::take(&mut self.media.new_playlist);
        self.media.playlists = None;
        self.rt.spawn(async move {
            let r: Result<serde_json::Value, String> = match target {
                Some(id) => {
                    c.send(
                        reqwest::Method::POST,
                        &format!("/api/v1/playlists/{id}/items"),
                        &serde_json::json!({ "items": [item] }),
                    )
                    .await
                }
                None => {
                    c.send(
                        reqwest::Method::POST,
                        "/api/v1/playlists",
                        &serde_json::json!({ "name": name.trim(), "items": [item] }),
                    )
                    .await
                }
            };
            let msg = match r {
                Ok(_) => (crate::engine::ToastKind::Success, t!("Added to the playlist").to_string()),
                Err(e) => (crate::engine::ToastKind::Error, e),
            };
            let _ = tx.send(crate::engine::EngineEvent::Toast(msg.0, msg.1));
        });
    }
}
