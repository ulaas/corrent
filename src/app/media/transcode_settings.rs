//! Settings → Playback → Transcoding: the server's ceiling for conversions (`[transcode]` in its
//! `server.toml`) and the encoder it found. Saved on the server (the one this app hosts, or the
//! one it is connected to), like the casting settings above it.

use corrent_proto::library::{TranscodeConfig, TranscodeInfo, X264_PRESETS};
use egui::{RichText, Ui};

use super::client::{Slot, fetch, take_ready};
use crate::app::CorrentApp;
use crate::app::settings_window::{heading, row};
use crate::ui::theme::Palette;
use crate::ui::widgets::{card, primary_button, toggle_chip};

#[derive(Default)]
pub struct TranscodeSettingsUi {
    loading: Option<Slot<TranscodeInfo>>,
    draft: Option<TranscodeConfig>,
    saved: Option<TranscodeConfig>,
    encoder: Option<String>,
    saving: Option<Slot<TranscodeInfo>>,
    message: Option<(bool, String)>,
}

/// The picture sizes offered as a ceiling: lines, and how they're called.
pub(super) fn heights() -> [(u32, &'static str); 5] {
    [(0, t!("No limit")), (2160, "4K"), (1080, "1080p"), (720, "720p"), (480, "480p")]
}

fn preset_name(p: &str) -> &'static str {
    match p {
        "ultrafast" => t!("Lightest"),
        "superfast" => t!("Lighter"),
        "veryfast" => t!("Balanced"),
        "faster" => t!("Better"),
        "fast" => t!("Better still"),
        _ => t!("Best picture"),
    }
}

impl CorrentApp {
    pub(crate) fn transcode_settings(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(client) = self.media.client.clone() else { return };
        let connected = self.server.is_some();
        let st = &mut self.media.transcode_settings;
        if st.draft.is_none() && st.loading.is_none() && st.message.is_none() {
            let c = client.clone();
            st.loading =
                Some(fetch(&self.rt, &self.ctx, async move { c.get_reason("/api/v1/transcode/config").await }));
        }
        for done in [
            st.loading.as_ref().and_then(take_ready).map(|r| (r, false)),
            st.saving.as_ref().and_then(take_ready).map(|r| (r, true)),
        ]
        .into_iter()
        .flatten()
        {
            let (r, saved) = done;
            if saved {
                st.saving = None;
            } else {
                st.loading = None;
            }
            match r {
                Ok(info) => {
                    st.encoder = info.encoder;
                    st.saved = Some(info.config.clone());
                    st.draft = Some(info.config);
                    st.message = saved.then(|| (true, t!("Saved.").to_string()));
                }
                Err(e) => st.message = Some((false, e)),
            }
        }
        let mut save = false;
        ui.add_space(12.0);
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            heading(ui, p, t!("Transcoding"));
            let about = if connected {
                t!("How the server you are connected to converts media that a player can’t play as it is, or that is asked for in a lower quality. Files that play as they are aren’t touched.")
            } else {
                t!("How media is converted when a player can’t play it as it is, or asks for a lower quality. Files that play as they are aren’t touched.")
            };
            ui.label(RichText::new(about).small().color(p.text_mute));
            if let Some((ok, m)) = &st.message {
                ui.label(RichText::new(m).color(if *ok { p.success } else { p.error }));
            }
            let Some(d) = st.draft.as_mut() else {
                if st.loading.is_some() {
                    ui.spinner();
                }
                return;
            };
            let encoder = match st.encoder.as_deref() {
                Some(e) => t!("Encoder in use: {name}", name = e),
                None => t!("No working video encoder was found: only sound can be converted.").to_string(),
            };
            ui.label(RichText::new(encoder).color(p.text_dim));
            row(ui, p, t!("Largest converted picture"), t!("Whatever a player asks for, conversions are no taller than this."), |ui| {
                for (h, l) in heights() {
                    if toggle_chip(ui, p, l, d.max_height == h).clicked() {
                        d.max_height = h;
                    }
                }
            });
            row(ui, p, t!("Highest converted bitrate"), t!("In Mbit/s; for a slow upload link. 0 means no limit."), |ui| {
                let mut mbit = d.max_bitrate_kbps / 1000;
                if ui.add(egui::DragValue::new(&mut mbit).range(0..=100)).changed() {
                    d.max_bitrate_kbps = mbit * 1000;
                }
            });
            row(ui, p, t!("Encoding effort"), t!("Lighter is easier on the processor; better looks cleaner at the same bitrate. Graphics-card encoders ignore this."), |ui| {
                for preset in X264_PRESETS {
                    if toggle_chip(ui, p, preset_name(preset), d.x264_preset == *preset).clicked() {
                        d.x264_preset = preset.to_string();
                    }
                }
            });
            row(ui, p, t!("Conversions at once"), t!("More players than this get an error instead of a stuttering picture."), |ui| {
                ui.add(egui::DragValue::new(&mut d.max_sessions).range(1..=16));
            });
            if st.saved.as_ref() != Some(d) && primary_button(ui, p, t!("Save")).clicked() {
                save = true;
            }
        });
        if save && let Some(d) = st.draft.clone() {
            st.message = None;
            let c = client.clone();
            st.saving = Some(fetch(&self.rt, &self.ctx, async move {
                c.send_reason(reqwest::Method::PUT, "/api/v1/transcode/config", &d).await
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_on_offer() {
        let d = TranscodeConfig::default();
        assert!(heights().iter().any(|(h, _)| *h == d.max_height));
        assert!(X264_PRESETS.contains(&d.x264_preset.as_str()));
    }
}
