//! Finding subtitles for a movie or episode with the server's subtitle-provider plugins.

use egui::{Id, RichText};
use egui_phosphor::regular as icons;
use serde::Deserialize;

use super::client::{Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

#[derive(Clone, Debug, Deserialize)]
pub struct FoundSub {
    pub provider: String,
    pub id: String,
    pub language: String,
    pub name: String,
    #[serde(default)]
    pub downloads: Option<u32>,
    #[serde(default)]
    pub hearing_impaired: bool,
    #[serde(default)]
    pub exact: bool,
}

pub struct SubFinder {
    item: i64,
    languages: String,
    list: Option<Slot<Vec<FoundSub>>>,
    saving: Option<(String, Slot<serde_json::Value>)>,
}

impl CorrentApp {
    pub(super) fn open_subtitle_finder(&mut self, item: i64) {
        let lang = crate::i18n::current().code();
        let languages = if lang.is_empty() || lang == "en" { "en".to_string() } else { format!("{lang},en") };
        self.media.sub_finder = Some(SubFinder { item, languages, list: None, saving: None });
    }

    pub(super) fn subtitle_finder_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut f) = self.media.sub_finder.take() else { return };
        let Some(client) = self.media.client.clone() else { return };
        if f.list.is_none() {
            let (c, path) = (client.clone(), format!("/api/v1/items/{}/subtitles?languages={}", f.item, f.languages));
            f.list = Some(fetch(&self.rt, ctx, async move { c.get(&path).await }));
        }
        if let Some((_, slot)) = &f.saving
            && let Some(r) = ready(slot)
        {
            f.saving = None;
            match r {
                Ok(_) => {
                    self.toasts
                        .push(crate::engine::ToastKind::Success, t!("Subtitle saved next to the video.").to_string());
                    self.media.invalidate();
                    return;
                }
                Err(e) => self.toasts.push(crate::engine::ToastKind::Error, e),
            }
        }
        let mut close = false;
        let mut save: Option<FoundSub> = None;
        let mut research = false;
        let r = egui::Modal::new(Id::new("find_subtitles")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new(format!("{}  {}", icons::CLOSED_CAPTIONING, t!("Find subtitles"))).strong().size(16.0));
            ui.horizontal(|ui| {
                ui.label(t!("Languages"));
                ui.add(egui::TextEdit::singleline(&mut f.languages).desired_width(140.0));
                if secondary_button(ui, t!("Search")).clicked() {
                    research = true;
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| match f.list.as_ref().and_then(ready) {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(p.error));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(
                        RichText::new(t!(
                            "No subtitles found. Subtitle providers (like OpenSubtitles) need their keys in the server's plugin settings."
                        ))
                        .color(p.text_dim),
                    );
                }
                Some(Ok(list)) => {
                    for s in list {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&s.language).monospace().color(p.accent));
                            if s.exact {
                                ui.label(RichText::new(t!("Exact match")).small().color(p.success));
                            }
                            if s.hearing_impaired {
                                ui.label(RichText::new("SDH").small().color(p.text_dim));
                            }
                            ui.label(RichText::new(&s.name).color(p.text)).on_hover_text(&s.provider);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let busy = f.saving.as_ref().is_some_and(|(id, _)| *id == s.id);
                                if busy {
                                    ui.spinner();
                                } else if ui
                                    .add_enabled(f.saving.is_none(), egui::Button::new(format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Save"))))
                                    .clicked()
                                {
                                    save = Some(s.clone());
                                }
                                if let Some(n) = s.downloads {
                                    ui.label(RichText::new(n.to_string()).small().color(p.text_mute));
                                }
                            });
                        });
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, p, t!("Close")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if close || r.should_close() {
            return;
        }
        if research {
            f.list = None;
        }
        if let Some(s) = save {
            let (c, path) = (client.clone(), format!("/api/v1/items/{}/subtitles", f.item));
            let body = serde_json::json!({ "provider": s.provider, "id": s.id, "language": s.language });
            f.saving = Some((
                s.id.clone(),
                fetch(&self.rt, ctx, async move { c.send(reqwest::Method::POST, &path, &body).await }),
            ));
        }
        self.media.sub_finder = Some(f);
    }
}
