//! Settings → Playback → Casting: the server's casting settings and what is casting now. Saved on
//! the server (the one this app hosts, or the one it is connected to).

use std::time::{Duration, Instant};

use corrent_proto::library::{CastCommand, CastConfig, CastDevice, CastStatus};
use egui::{RichText, Ui};

use super::client::{Slot, fetch, ready, take_ready};
use crate::app::CorrentApp;
use crate::app::settings_window::{heading, row, toggle};
use crate::ui::theme::Palette;
use crate::ui::widgets::{card, primary_button, toggle_chip};

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
    /// The devices on the network (for the AirPlay ones: paired or not).
    found: Option<Slot<Vec<CastDevice>>>,
    /// A pairing under way: the device (id, name) shows its PIN, typed here.
    pairing: Option<(String, String, String)>,
    /// Pair, finish or forget on its way: what to say when it worked.
    pair_step: Option<(Slot<serde_json::Value>, Option<String>)>,
    pair_error: Option<String>,
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
                t!("Allow casting to Chromecasts and TVs"),
                t!("While something is casting, the device fetches it from this computer over plain HTTP on the local network. Only signed links are served, and the port closes soon after casting ends."),
                &mut d.enabled,
            );
            toggle(
                ui,
                p,
                t!("Also find AirPlay devices"),
                t!("Apple TVs and TVs with AirPlay built in (Samsung, LG, Sony). Each is paired once with a PIN it shows on its screen. Converted streams and files they play as they are both work; subtitles go into the picture."),
                &mut d.airplay,
            );
            toggle(
                ui,
                p,
                t!("Also find smart TVs and other DLNA players"),
                t!("Samsung, LG and Sony TVs, receivers, Kodi. They play most files as they are; what they can’t play is converted, with the subtitles in the picture. The picture size and H.265 settings below are for Chromecasts."),
                &mut d.dlna,
            );
            row(ui, p, t!("Largest picture sent as it is"), t!("Taller videos are converted first. Older Chromecasts stop at 1080p."), |ui| {
                for (h, l) in heights() {
                    if toggle_chip(ui, p, l, d.max_height == h).clicked() {
                        d.max_height = h;
                    }
                }
            });
            row(ui, p, t!("Quality of converted streams"), t!("What a video is converted to when the device can’t play it as it is. Automatic keeps the file’s size."), |ui| {
                let on = |d: &CastConfig, h: u32| d.transcode_height == h;
                if toggle_chip(ui, p, t!("Automatic"), on(d, 0)).clicked() {
                    (d.transcode_height, d.transcode_bitrate_kbps) = (0, 0);
                }
                for (id, h, bits) in corrent_proto::library::QUALITIES {
                    let label = if *id == "2160" { "4K".to_string() } else { format!("{id}p") };
                    if toggle_chip(ui, p, &label, on(d, *h as u32)).clicked() {
                        (d.transcode_height, d.transcode_bitrate_kbps) = (*h as u32, (*bits / 1000) as u32);
                    }
                }
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
            row(ui, p, t!("Devices to add by hand"), t!("One a line, for networks where the search finds nothing: 192.168.1.20 or Bedroom=192.168.1.20 for a Chromecast, the address of its description for a TV (http://192.168.1.20:9197/dmr on Samsung TVs)"), |ui| {
                ui.add(egui::TextEdit::multiline(&mut st.devices).desired_rows(2).desired_width(220.0));
            });
            let mut pending = d.clone();
            pending.devices = lines(&st.devices);
            if st.saved.as_ref() != Some(&pending) && primary_button(ui, p, t!("Save")).clicked() {
                *d = pending;
                save = true;
            }
        });
        // AirPlay devices are paired once each, here.
        if st.saved.as_ref().is_some_and(|c| c.enabled && c.airplay) {
            self.airplay_devices(ui, p, &client);
        }
        let st = &mut self.media.cast_settings;
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

impl CorrentApp {
    /// The AirPlay receivers on the network, each with Pair… (it shows a PIN) or Forget.
    fn airplay_devices(&mut self, ui: &mut Ui, p: &Palette, client: &super::client::MediaClient) {
        let st = &mut self.media.cast_settings;
        let ask = |refresh: bool| {
            let c = client.clone();
            fetch(&self.rt, &self.ctx, async move {
                c.get_reason(if refresh { "/api/v1/cast/devices?refresh=true" } else { "/api/v1/cast/devices" }).await
            })
        };
        if st.found.is_none() {
            st.found = Some(ask(false));
        }
        // A step that came back: say so, and look at the devices again.
        if let Some((slot, done)) = &st.pair_step
            && let Some(r) = take_ready(slot)
        {
            let done = done.clone();
            st.pair_step = None;
            match r {
                Ok(_) => {
                    st.pair_error = None;
                    if let Some(text) = done {
                        st.pairing = None;
                        self.toasts.push(crate::engine::ToastKind::Success, text);
                        st.found = Some(ask(false));
                    }
                }
                // The attempt is over with its error: Pair… starts another.
                Err(e) => {
                    st.pairing = None;
                    st.pair_error = Some(super::cast::reason_text(&e));
                }
            }
        }
        let list = st.found.as_ref().and_then(ready);
        let busy = st.pair_step.is_some();
        // (path, method, body, what to say when done)
        let mut send: Option<(String, reqwest::Method, serde_json::Value, Option<String>)> = None;
        let mut again = false;
        ui.add_space(12.0);
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            heading(ui, p, t!("AirPlay devices"));
            if let Some(e) = &st.pair_error {
                ui.label(RichText::new(e).color(p.error));
            }
            match &list {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(super::cast::reason_text(e)).color(p.error));
                }
                Some(Ok(all)) => {
                    let receivers: Vec<&CastDevice> = all.iter().filter(|d| d.kind == "airplay").collect();
                    if receivers.is_empty() {
                        ui.label(
                            RichText::new(t!("No AirPlay devices found on the server’s network.")).color(p.text_dim),
                        );
                    }
                    for d in receivers {
                        let name =
                            if d.model.is_empty() { d.name.clone() } else { format!("{}  ·  {}", d.name, d.model) };
                        let pairing_this = st.pairing.as_ref().is_some_and(|(id, ..)| *id == d.id);
                        let hint = if pairing_this {
                            t!("Enter the PIN shown on the TV.")
                        } else if d.needs_pairing {
                            t!("not paired")
                        } else {
                            t!("Paired")
                        };
                        row(ui, p, &name, hint, |ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                if let Some((id, _, pin)) = st.pairing.as_mut().filter(|(id, ..)| *id == d.id) {
                                    if ui.button(t!("Cancel")).clicked() {
                                        again = true;
                                    }
                                    let typed = !pin.trim().is_empty();
                                    if ui.add_enabled(typed, egui::Button::new(t!("Pair"))).clicked() {
                                        send = Some((
                                            format!("/api/v1/cast/{id}/pair/finish"),
                                            reqwest::Method::POST,
                                            serde_json::json!({ "pin": pin.trim() }),
                                            Some(t!("Paired with {name}.", name = d.name)),
                                        ));
                                    }
                                    ui.add(
                                        egui::TextEdit::singleline(pin)
                                            .desired_width(80.0)
                                            .char_limit(8)
                                            .hint_text("0000"),
                                    );
                                } else if d.needs_pairing {
                                    if ui.button(t!("Pair…")).clicked() {
                                        st.pairing = Some((d.id.clone(), d.name.clone(), String::new()));
                                        send = Some((
                                            format!("/api/v1/cast/{}/pair", d.id),
                                            reqwest::Method::POST,
                                            serde_json::Value::Null,
                                            None,
                                        ));
                                    }
                                } else if ui.button(t!("Forget")).clicked() {
                                    send = Some((
                                        format!("/api/v1/cast/{}/pair", d.id),
                                        reqwest::Method::DELETE,
                                        serde_json::Value::Null,
                                        Some(t!("Forgot the pairing with {name}.", name = d.name)),
                                    ));
                                }
                            });
                        });
                    }
                }
            }
            if ui.button(t!("Search again")).clicked() {
                st.found = Some(ask(true));
            }
        });
        if again {
            st.pairing = None;
        }
        if let Some((path, method, body, done)) = send {
            st.pair_error = None;
            let c = client.clone();
            let slot = fetch(&self.rt, &self.ctx, async move { c.send_reason(method, &path, &body).await });
            st.pair_step = Some((slot, done));
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
