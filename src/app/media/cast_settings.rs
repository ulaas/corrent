//! Settings → Playback → Casting: the server's casting settings and what is casting now. Saved on
//! the server (the one this app hosts, or the one it is connected to).

use std::time::{Duration, Instant};

use corrent_proto::library::{CastCommand, CastConfig, CastStatus};
use egui::{RichText, Ui};

use super::client::{Slot, fetch, ready, take_ready};
use crate::app::CorrentApp;
use crate::app::settings_window::{heading, row, toggle};
use crate::ui::theme::Palette;
use crate::ui::widgets::{card, primary_button};

#[derive(Default)]
pub struct CastSettingsUi {
    loading: Option<Slot<CastConfig>>,
    draft: Option<CastConfig>,
    /// The saved settings: what the draft is compared with.
    saved: Option<CastConfig>,
    /// One device a line.
    devices: String,
    saving: Option<Slot<CastConfig>>,
    message: Option<(bool, String)>,
    active: Option<Slot<Vec<CastStatus>>>,
    active_at: Option<Instant>,
}

/// The resolutions offered: lines, and how they're called.
fn heights() -> [(u32, &'static str); 4] {
    [(720, "720p"), (1080, "1080p"), (2160, "4K"), (0, t!("No limit"))]
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

impl CastSettingsUi {
    /// Tests: as if these lines were typed into the devices box.
    #[cfg(test)]
    pub(crate) fn dev_type_devices(&mut self, text: &str) {
        self.devices = text.to_string();
    }
}

impl CorrentApp {
    pub(crate) fn cast_settings(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(client) = self.media.client.clone() else { return };
        let connected = self.server.is_some();
        let st = &mut self.media.cast_settings;
        if st.draft.is_none() && st.loading.is_none() {
            let c = client.clone();
            st.loading = Some(fetch(&self.rt, &self.ctx, async move { c.get_reason("/api/v1/cast/config").await }));
        }
        if let Some(r) = st.loading.as_ref().and_then(take_ready) {
            st.loading = None;
            match r {
                Ok(c) => {
                    st.devices = c.devices.join("\n");
                    st.saved = Some(c.clone());
                    st.draft = Some(c);
                }
                Err(e) => st.message = Some((false, e)),
            }
        }
        if let Some(r) = st.saving.as_ref().and_then(take_ready) {
            st.saving = None;
            match r {
                Ok(c) => {
                    st.devices = c.devices.join("\n");
                    st.saved = Some(c.clone());
                    st.draft = Some(c);
                    st.message = Some((true, t!("Saved.").to_string()));
                }
                Err(e) => st.message = Some((false, e)),
            }
        }
        // What is casting: looked at again while the page is open.
        if st.active_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            let c = client.clone();
            st.active = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/cast/active").await }));
            st.active_at = Some(Instant::now());
        }
        self.ctx.request_repaint_after(Duration::from_secs(3));
        let active = st.active.as_ref().and_then(ready).and_then(Result::ok).unwrap_or_default();

        let mut save = false;
        let mut stop = None;
        ui.add_space(12.0);
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            heading(ui, p, t!("Casting"));
            if connected {
                ui.label(
                    RichText::new(t!("These are the settings of the server you are connected to."))
                        .small()
                        .color(p.text_mute),
                );
            }
            if let Some((ok, m)) = &st.message {
                ui.label(RichText::new(m).color(if *ok { p.success } else { p.error }));
            }
            for s in &active {
                let text = if s.title.is_empty() {
                    s.device.name.clone()
                } else {
                    format!("{}  ·  {}", s.device.name, s.title)
                };
                row(ui, p, &text, t!("Casting now"), |ui| {
                    if ui.button(t!("Stop casting")).clicked() {
                        stop = Some(s.device.id.clone());
                    }
                });
            }
            let Some(d) = st.draft.as_mut() else {
                if st.loading.is_some() {
                    ui.spinner();
                }
                return;
            };
            toggle(
                ui,
                p,
                t!("Allow casting to Chromecasts"),
                t!("While something is casting, the device fetches it from this computer over plain HTTP on the local network. Only signed links are served, and the port closes soon after casting ends."),
                &mut d.enabled,
            );
            row(ui, p, t!("Largest picture sent as it is"), t!("Taller videos are converted first. Older Chromecasts stop at 1080p."), |ui| {
                let list = heights();
                let shown = list.iter().find(|(h, _)| *h == d.max_height).map(|(_, l)| *l).unwrap_or("");
                egui::ComboBox::from_id_salt("cast-height").selected_text(shown).show_ui(ui, |ui| {
                    for (h, l) in list {
                        ui.selectable_value(&mut d.max_height, h, l);
                    }
                });
            });
            row(ui, p, t!("Highest bitrate sent as it is"), t!("In Mbit/s; for a slow Wi-Fi. 0 means no limit."), |ui| {
                let mut mbit = d.max_bitrate_kbps / 1000;
                if ui.add(egui::DragValue::new(&mut mbit).range(0..=200)).changed() {
                    d.max_bitrate_kbps = mbit * 1000;
                }
            });
            toggle(
                ui,
                p,
                t!("The devices play H.265"),
                t!("Chromecast with Google TV and most Cast-enabled TVs do; such files then needn’t be converted."),
                &mut d.hevc,
            );
            toggle(
                ui,
                p,
                t!("Copy streams when only the file type is in the way"),
                t!("MKV files the device can play aren’t converted, which is much lighter on this computer. If a device fails on the copy, it gets a converted stream instead."),
                &mut d.copy_streams,
            );
            row(ui, p, t!("Port for the devices"), t!("0 picks one. Set one to open it in a firewall."), |ui| {
                ui.add(egui::DragValue::new(&mut d.port).range(0..=65535));
            });
            row(ui, p, t!("Devices to add by hand"), t!("One a line, for networks where the search finds nothing: 192.168.1.20 or Bedroom=192.168.1.20"), |ui| {
                ui.add(egui::TextEdit::multiline(&mut st.devices).desired_rows(2).desired_width(220.0));
            });
            let mut pending = d.clone();
            pending.devices = lines(&st.devices);
            if st.saved.as_ref() != Some(&pending) && primary_button(ui, p, t!("Save")).clicked() {
                *d = pending;
                save = true;
            }
        });
        if save && let Some(d) = st.draft.clone() {
            st.message = None;
            let c = client.clone();
            st.saving = Some(fetch(&self.rt, &self.ctx, async move {
                c.send_reason(reqwest::Method::PUT, "/api/v1/cast/config", &d).await
            }));
        }
        if let Some(id) = stop {
            let c = client.clone();
            self.rt.spawn(async move {
                let path = format!("/api/v1/cast/{id}/command");
                if let Err(e) = c.send::<serde_json::Value, _>(reqwest::Method::POST, &path, &CastCommand::Stop).await {
                    tracing::warn!("cast: {e}");
                }
            });
            st.active_at = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_lines_lose_blanks_and_spaces() {
        assert_eq!(lines(" 10.0.0.2 \n\n  Den=10.0.0.3\n"), ["10.0.0.2", "Den=10.0.0.3"]);
    }

    #[test]
    fn the_default_height_is_on_offer() {
        let default = CastConfig::default().max_height;
        assert!(heights().iter().any(|(h, _)| *h == default));
    }
}
