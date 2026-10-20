//! Settings → Metadata: the server's metadata providers (TMDB, TVDB, fanart.tv, MusicBrainz,
//! OpenSubtitles, OMDb…): turned on or off, with their API keys. Saved at once on the server
//! (the one this app hosts, or the one it is connected to).

use std::collections::HashMap;

use egui::{RichText, Ui};
use egui_phosphor::regular as icons;
use serde_json::Value;

use super::client::{Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{card, primary_button};

#[derive(Default)]
pub struct ProvidersUi {
    list: Option<Slot<Vec<Value>>>,
    /// Typed but not saved yet: (plugin, setting) → value.
    edits: HashMap<(String, String), String>,
    /// A save or switch on its way (plugin id).
    busy: Option<(String, Slot<Value>)>,
    message: Option<(bool, String)>,
}

impl ProvidersUi {
    /// Tests: as if `value` were typed into a provider's setting.
    #[cfg(test)]
    pub(crate) fn dev_type(&mut self, plugin: &str, key: &str, value: &str) {
        self.edits.insert((plugin.to_string(), key.to_string()), value.to_string());
    }
}

/// Where to get a key, for the providers that need one.
fn key_page(id: &str) -> Option<&'static str> {
    match id {
        "tmdb" => Some("https://www.themoviedb.org/settings/api"),
        "omdb" => Some("https://www.omdbapi.com/apikey.aspx"),
        "tvdb" => Some("https://thetvdb.com/api-information"),
        "fanart" => Some("https://fanart.tv/get-an-api-key/"),
        "opensubtitles" => Some("https://www.opensubtitles.com/en/consumers"),
        _ => None,
    }
}

/// The value at `path` (Null when it isn't there).
fn at<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().try_fold(v, |v, k| v.get(*k)).unwrap_or(&Value::Null)
}

fn str_of<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    at(v, path).as_str().unwrap_or_default()
}

impl CorrentApp {
    pub(crate) fn metadata_settings(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(client) = self.media.client.clone() else {
            ui.label(RichText::new(t!("The media server isn't running.")).color(p.text_dim));
            return;
        };
        let st = &mut self.media.providers;
        if st.list.is_none() {
            let c = client.clone();
            st.list = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/plugins").await }));
        }
        if let Some((id, slot)) = &st.busy
            && let Some(r) = ready(slot)
        {
            let id = id.clone();
            st.busy = None;
            match r {
                Ok(_) => {
                    st.edits.retain(|(p, _), _| *p != id);
                    st.message = Some((true, t!("Saved. Titles without details are looked up again.").to_string()));
                }
                Err(e) => st.message = Some((false, e)),
            }
            st.list = None;
            return;
        }
        ui.label(
            RichText::new(t!(
                "Providers fill in titles, plots, cast, posters and ratings. Most need a free API key from their website; without one they are skipped."
            ))
            .color(p.text_dim),
        );
        if self.server.is_some() {
            ui.label(
                RichText::new(t!("These are the settings of the server you are connected to."))
                    .small()
                    .color(p.text_mute),
            );
        }
        if let Some((ok, m)) = &st.message {
            ui.label(RichText::new(m).color(if *ok { p.success } else { p.error }));
        }
        ui.add_space(8.0);
        let list = match st.list.as_ref().and_then(ready) {
            None => {
                ui.spinner();
                return;
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(p.error));
                return;
            }
            Some(Ok(list)) => list,
        };
        let mut action: Option<(String, reqwest::Method, String, Value)> = None;
        for plugin in list.iter().filter(|v| {
            at(v, &["manifest", "kinds"])
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "metadata" || k == "subtitles"))
        }) {
            let id = str_of(plugin, &["manifest", "id"]).to_string();
            if id.starts_with("example") {
                continue;
            }
            let missing: Vec<&str> =
                plugin["missing"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let enabled = plugin["enabled"].as_bool().unwrap_or(false);
            card(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = enabled;
                    if ui.checkbox(&mut on, RichText::new(str_of(plugin, &["manifest", "name"])).strong()).changed() {
                        action = Some((
                            id.clone(),
                            reqwest::Method::PUT,
                            format!("/api/v1/plugins/{id}/enabled"),
                            serde_json::json!({ "enabled": on }),
                        ));
                    }
                    let (badge, color) = if !enabled {
                        (t!("Off"), p.text_mute)
                    } else if missing.is_empty() {
                        (t!("Ready"), p.success)
                    } else {
                        (t!("Needs an API key"), p.warn)
                    };
                    ui.label(RichText::new(badge).small().color(color));
                });
                ui.label(RichText::new(str_of(plugin, &["manifest", "description"])).small().color(p.text_dim));
                let defs = at(plugin, &["manifest", "settings"]).as_array().cloned().unwrap_or_default();
                for def in &defs {
                    let key = str_of(def, &["key"]).to_string();
                    let secret = def["secret"].as_bool().unwrap_or(false);
                    let current = str_of(plugin, &["settings", key.as_str()]).to_string();
                    ui.add_space(4.0);
                    ui.label(RichText::new(str_of(def, &["label"])).color(p.text));
                    let entry = st.edits.entry((id.clone(), key.clone())).or_default();
                    // Secrets come back masked: shown as a hint, sent only when typed anew.
                    let hint = if current.is_empty() { str_of(def, &["default"]).to_string() } else { current.clone() };
                    ui.add(egui::TextEdit::singleline(entry).password(secret).hint_text(hint).desired_width(360.0));
                    let help = str_of(def, &["help"]);
                    if !help.is_empty() {
                        ui.label(RichText::new(help).small().color(p.text_mute));
                    }
                }
                ui.horizontal(|ui| {
                    let typed: serde_json::Map<String, Value> = st
                        .edits
                        .iter()
                        .filter(|((p, _), v)| *p == id && !v.trim().is_empty())
                        .map(|((_, k), v)| (k.clone(), Value::String(v.trim().to_string())))
                        .collect();
                    let busy = st.busy.as_ref().is_some_and(|(b, _)| *b == id);
                    if busy {
                        ui.spinner();
                    } else if !typed.is_empty() && primary_button(ui, p, t!("Save")).clicked() {
                        action = Some((
                            id.clone(),
                            reqwest::Method::PUT,
                            format!("/api/v1/plugins/{id}/settings"),
                            Value::Object(typed),
                        ));
                    }
                    if let Some(url) = key_page(&id)
                        && ui.link(format!("{}  {}", icons::ARROW_SQUARE_OUT, t!("Get a key"))).clicked()
                    {
                        let _ = open::that_detached(url);
                    }
                });
            });
            ui.add_space(8.0);
        }
        if let Some((id, method, path, body)) = action {
            let c = client.clone();
            st.message = None;
            st.busy = Some((id, fetch(&self.rt, &self.ctx, async move { c.send(method, &path, &body).await })));
        }
    }
}
