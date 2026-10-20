//! "Identify…": choosing what a movie, show or video is from the metadata providers' search
//! results, when the automatic match didn't find it (or found the wrong one).

use egui::{Id, RichText};
use egui_phosphor::regular as icons;
use serde::Deserialize;

use super::client::{Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

#[derive(Clone, Debug, Deserialize)]
pub struct Candidate {
    pub provider: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub year: Option<u32>,
    #[serde(default)]
    pub overview: Option<String>,
}

pub struct Identify {
    item: i64,
    title: String,
    year: String,
    list: Option<Slot<Vec<Candidate>>>,
    applying: Option<Slot<serde_json::Value>>,
}

impl CorrentApp {
    pub(super) fn open_identify(&mut self, item: i64, title: &str, year: Option<i32>) {
        self.media.identify = Some(Identify {
            item,
            title: title.to_string(),
            year: year.map(|y| y.to_string()).unwrap_or_default(),
            list: None,
            applying: None,
        });
    }

    pub(super) fn identify_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut f) = self.media.identify.take() else { return };
        let Some(client) = self.media.client.clone() else { return };
        if f.list.is_none() {
            let q = |s: &str| url::form_urlencoded::byte_serialize(s.trim().as_bytes()).collect::<String>();
            let mut path = format!("/api/v1/items/{}/identify?title={}", f.item, q(&f.title));
            if let Ok(y) = f.year.trim().parse::<i32>() {
                path.push_str(&format!("&year={y}"));
            }
            let c = client.clone();
            f.list = Some(fetch(&self.rt, ctx, async move { c.get(&path).await }));
        }
        if let Some(r) = f.applying.as_ref().and_then(ready) {
            f.applying = None;
            match r {
                Ok(_) => {
                    self.toasts.push(crate::engine::ToastKind::Success, t!("Details updated.").to_string());
                    self.media.invalidate();
                    return;
                }
                Err(e) => self.toasts.push(crate::engine::ToastKind::Error, e),
            }
        }
        let mut close = false;
        let mut search = false;
        let mut apply: Option<Candidate> = None;
        let r = egui::Modal::new(Id::new("identify")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.label(RichText::new(format!("{}  {}", icons::MAGNIFYING_GLASS, t!("Identify"))).strong().size(16.0));
            ui.label(RichText::new(t!("Search the metadata providers, then pick the right one.")).small().color(p.text_dim));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(t!("Title"));
                let r = ui.add(egui::TextEdit::singleline(&mut f.title).desired_width(300.0));
                ui.label(t!("Year"));
                let r2 = ui.add(egui::TextEdit::singleline(&mut f.year).desired_width(60.0));
                let enter = (r.lost_focus() || r2.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if secondary_button(ui, t!("Search")).clicked() || enter {
                    search = true;
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| match f.list.as_ref().and_then(ready) {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(p.error));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(
                        RichText::new(t!(
                            "Nothing found. Try another title or year; providers that need an API key (TMDB, OMDb) need it in Settings → Metadata."
                        ))
                        .color(p.text_dim),
                    );
                }
                Some(Ok(list)) => {
                    for c in list {
                        crate::ui::widgets::card(p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let year = c.year.map(|y| format!(" ({y})")).unwrap_or_default();
                                ui.label(RichText::new(format!("{}{year}", c.title)).strong().color(p.text));
                                ui.label(RichText::new(&c.provider).small().color(p.text_mute));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if f.applying.is_some() {
                                        ui.spinner();
                                    } else if ui.button(t!("This one")).clicked() {
                                        apply = Some(c.clone());
                                    }
                                });
                            });
                            if let Some(o) = c.overview.as_deref().filter(|o| !o.trim().is_empty()) {
                                let mut short: String = o.chars().take(220).collect();
                                if short.len() < o.len() {
                                    short.push('…');
                                }
                                ui.label(RichText::new(short).small().color(p.text_dim));
                            }
                        });
                        ui.add_space(4.0);
                    }
                }
            });
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, p, t!("Close")).clicked() {
                    close = true;
                }
            });
        });
        if close || r.should_close() {
            return;
        }
        if search {
            f.list = None;
        }
        if let Some(c) = apply {
            let (cl, path) = (client.clone(), format!("/api/v1/items/{}/identify", f.item));
            let body = serde_json::json!({ "provider": c.provider, "id": c.id });
            f.applying = Some(fetch(&self.rt, ctx, async move { cl.send(reqwest::Method::POST, &path, &body).await }));
        }
        self.media.identify = Some(f);
    }
}
