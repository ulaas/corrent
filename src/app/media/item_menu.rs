//! The context menu of a media card or row (right-click): the item page's buttons, reachable
//! from grids and lists — the same menu as the web client's (web/src/lib/itemmenu.ts).

use corrent_proto::library::{Item, ItemDetail, ItemKind, Playlist, RefreshMode};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, fetch, ready};
use crate::app::CorrentApp;
use crate::engine::{EngineEvent, ToastKind};
use crate::ui::widgets::{context_menu, menu_button};

/// A row of a playlist: its place, for Move up/down and Remove.
#[derive(Clone, Copy)]
pub(super) struct PlaylistRow {
    pub list: i64,
    pub index: usize,
    pub len: usize,
}

enum Act {
    Play,
    FromStart,
    AddTo(i64),
    Played(bool),
    Favorite(bool),
    Subtitles,
    Identify,
    Refresh(RefreshMode),
    Open(String),
    CopyLink,
    Move(i64, usize, usize),
    Remove(i64, usize),
}

impl CorrentApp {
    /// Attaches the menu to `resp` (a card or row of `item`); `siblings` play after it, and a
    /// playlist row gets Move up/down and Remove.
    pub(super) fn item_menu(
        &mut self,
        resp: &egui::Response,
        client: &MediaClient,
        item: &Item,
        siblings: &[Item],
        row: Option<PlaylistRow>,
    ) {
        let k = item.kind;
        let mut act: Option<Act> = None;
        context_menu(resp, |ui| {
            ui.set_min_width(220.0);
            let mut pick = |ui: &mut egui::Ui, icon: &str, label: &str, a: Act| {
                if ui.button(format!("{icon}  {label}")).clicked() {
                    act = Some(a);
                    ui.close();
                }
            };
            if k.playable() {
                pick(ui, icons::PLAY, t!("Play"), Act::Play);
                if item.state.position_ms > 0 && !item.state.played {
                    pick(ui, icons::ARROW_COUNTER_CLOCKWISE, t!("From the start"), Act::FromStart);
                }
            }
            if k.playable() || matches!(k, ItemKind::Album | ItemKind::Season) {
                // The user's playlists, asked for when the menu opens.
                if self.media.my_playlists.is_none() {
                    let c = client.clone();
                    self.media.my_playlists = Some(fetch(&self.rt, &self.ctx, async move {
                        let all: Vec<Playlist> = c.get("/api/v1/playlists").await?;
                        Ok(all.into_iter().filter(|p| p.mine).collect::<Vec<_>>())
                    }));
                }
                let lists = self.media.my_playlists.as_ref().and_then(ready);
                menu_button(ui, format!("{}  {}", icons::PLAYLIST, t!("Add to playlist")), |ui| match lists {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(e)) => {
                        ui.label(e);
                    }
                    Some(Ok(lists)) if lists.is_empty() => {
                        ui.label(t!("Nothing here"));
                    }
                    Some(Ok(lists)) => {
                        for l in lists {
                            if ui.button(&l.name).clicked() {
                                act = Some(Act::AddTo(l.id));
                                ui.close();
                            }
                        }
                    }
                });
            }
            ui.separator();
            let played = item.state.played;
            let (icon, label) = if played {
                (icons::CIRCLE, t!("Mark as unplayed"))
            } else {
                (icons::CHECK_CIRCLE, t!("Mark as played"))
            };
            if ui.button(format!("{icon}  {label}")).clicked() {
                act = Some(Act::Played(!played));
                ui.close();
            }
            let fav = item.state.favorite;
            let label = if fav { t!("Remove from favorites") } else { t!("Add to favorites") };
            if ui.button(format!("{}  {label}", icons::HEART)).clicked() {
                act = Some(Act::Favorite(!fav));
                ui.close();
            }
            ui.separator();
            if matches!(k, ItemKind::Movie | ItemKind::Episode | ItemKind::Video)
                && ui.button(format!("{}  {}", icons::CLOSED_CAPTIONING, t!("Find subtitles"))).clicked()
            {
                act = Some(Act::Subtitles);
                ui.close();
            }
            if matches!(k, ItemKind::Movie | ItemKind::Show | ItemKind::Video)
                && ui.button(format!("{}  {}", icons::MAGNIFYING_GLASS, t!("Identify…"))).clicked()
            {
                act = Some(Act::Identify);
                ui.close();
            }
            if matches!(k, ItemKind::Movie | ItemKind::Show | ItemKind::Video | ItemKind::Album | ItemKind::Artist)
                && let Some(mode) = super::refresh::refresh_menu(ui)
            {
                act = Some(Act::Refresh(mode));
            }
            if matches!(k, ItemKind::Movie | ItemKind::Show) {
                // Cards don't carry provider ids: the item is asked for when the menu opens.
                if self.media.menu_detail.as_ref().is_none_or(|(id, _)| *id != item.id) {
                    let (c, id) = (client.clone(), item.id);
                    self.media.menu_detail = Some((
                        id,
                        fetch(&self.rt, &self.ctx, async move { c.get(&format!("/api/v1/items/{id}")).await }),
                    ));
                }
                let detail = self.media.menu_detail.as_ref().and_then(|(_, s)| ready(s));
                menu_button(ui, format!("{}  IMDb / TMDB", icons::ARROW_SQUARE_OUT), |ui| match detail {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(e)) => {
                        ui.label(e);
                    }
                    Some(Ok(d)) => {
                        let links = provider_links(&d, k);
                        if links.is_empty() {
                            ui.label(t!("Nothing here"));
                        }
                        for (label, url) in links {
                            if ui.button(format!("{}  {label}", icons::ARROW_SQUARE_OUT)).on_hover_text(&url).clicked()
                            {
                                act = Some(Act::Open(url));
                                ui.close();
                            }
                        }
                    }
                });
            }
            ui.separator();
            if ui.button(format!("{}  {}", icons::LINK, t!("Copy link"))).clicked() {
                act = Some(Act::CopyLink);
                ui.close();
            }
            if let Some(r) = row {
                ui.separator();
                if ui
                    .add_enabled(r.index > 0, egui::Button::new(format!("{}  {}", icons::ARROW_UP, t!("Move up"))))
                    .clicked()
                {
                    act = Some(Act::Move(r.list, r.index, r.index - 1));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        r.index + 1 < r.len,
                        egui::Button::new(format!("{}  {}", icons::ARROW_DOWN, t!("Move down"))),
                    )
                    .clicked()
                {
                    act = Some(Act::Move(r.list, r.index, r.index + 1));
                    ui.close();
                }
                if ui.button(format!("{}  {}", icons::X, t!("Remove"))).clicked() {
                    act = Some(Act::Remove(r.list, r.index));
                    ui.close();
                }
            }
        });
        let Some(act) = act else { return };
        match act {
            Act::Play => {
                let rest: Vec<Item> = if siblings.is_empty() { vec![item.clone()] } else { siblings.to_vec() };
                self.play_item(item, &rest);
            }
            Act::FromStart => {
                // The player resumes from the item's position: this time from 0.
                let mut it = item.clone();
                it.state.position_ms = 0;
                self.play_item(&it, std::slice::from_ref(&it));
            }
            Act::AddTo(list) => self.background(
                client,
                reqwest::Method::POST,
                format!("/api/v1/playlists/{list}/items"),
                serde_json::json!({ "items": [item.id] }),
                Some(t!("Added to the playlist").to_string()),
            ),
            Act::Played(on) => self.set_state(
                client,
                item.id,
                if on {
                    serde_json::json!({ "played": true, "position_ms": 0 })
                } else {
                    serde_json::json!({ "played": false })
                },
            ),
            Act::Favorite(on) => self.set_state(client, item.id, serde_json::json!({ "favorite": on })),
            Act::Subtitles => self.open_subtitle_finder(item.id),
            Act::Identify => self.open_identify(item.id, &item.title, item.year),
            Act::Refresh(mode) => self.refresh_item(client, item.id, mode),
            Act::Open(url) => {
                let _ = open::that_detached(url);
            }
            Act::CopyLink => {
                self.ctx.copy_text(client.url(&format!("/item/{}", item.id)));
                self.toasts.push(ToastKind::Success, t!("Copied").to_string());
            }
            Act::Move(list, from, to) => {
                self.background(
                    client,
                    reqwest::Method::POST,
                    format!("/api/v1/playlists/{list}/move"),
                    serde_json::json!({ "from": from, "to": to }),
                    None,
                );
                self.media.list_items = None;
            }
            Act::Remove(list, index) => {
                self.background(
                    client,
                    reqwest::Method::DELETE,
                    format!("/api/v1/playlists/{list}/items/{index}"),
                    serde_json::Value::Null,
                    None,
                );
                self.media.list_items = None;
            }
        }
    }

    /// Sends a request without waiting for it; a toast says how it went (`done`: on success).
    fn background(
        &mut self,
        client: &MediaClient,
        method: reqwest::Method,
        path: String,
        body: serde_json::Value,
        done: Option<String>,
    ) {
        let (c, tx, ctx) = (client.clone(), self.ui_events_tx.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let r: Result<serde_json::Value, String> = c.send(method, &path, &body).await;
            match r {
                Ok(_) => {
                    if let Some(text) = done {
                        let _ = tx.send(EngineEvent::Toast(ToastKind::Success, text));
                    }
                }
                Err(e) => {
                    let _ = tx.send(EngineEvent::Toast(ToastKind::Error, e));
                }
            }
            ctx.request_repaint();
        });
    }
}

/// IMDb and TMDB pages of an item (the ids its providers gave), as on its page.
pub(super) fn provider_links(d: &ItemDetail, kind: ItemKind) -> Vec<(&'static str, String)> {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let tmdb_kind = if kind == ItemKind::Show { "tv" } else { "movie" };
    let mut out = Vec::new();
    if let Some(id) = d.provider_ids.get("imdb") {
        out.push(("IMDb", format!("https://www.imdb.com/title/{}/", enc(id))));
    }
    if let Some(id) = d.provider_ids.get("tmdb") {
        out.push(("TMDB", format!("https://www.themoviedb.org/{tmdb_kind}/{}", enc(id))));
    }
    out
}
