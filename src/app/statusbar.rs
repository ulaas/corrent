use egui::{Align, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::types::EngineState;
use crate::ui::theme::Palette;
use crate::ui::widgets::sparkline;
use crate::util::{fmt_bytes, fmt_speed};

impl CorrentApp {
    pub(super) fn status_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let units = self.settings.appearance.speed_units;
        let s = self.snap.session.clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (dot, text) = match &self.snap.engine_state {
                _ if !self.engine.is_alive() => (p.error, "Engine stopped".to_string()),
                EngineState::Starting => (p.warn, "Starting…".to_string()),
                EngineState::Restarting => (p.warn, "Reconnecting…".to_string()),
                EngineState::Failed(e) => (p.error, format!("Engine error: {e}")),
                EngineState::Running => match s.dht_nodes {
                    Some(n) if n > 0 => (p.success, format!("DHT: {n} nodes")),
                    Some(_) => (p.warn, "DHT: bootstrapping".into()),
                    None => (p.text_mute, "DHT off".into()),
                },
            };
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, dot);
            ui.label(RichText::new(text).small().color(p.text_dim));

            if let Some(port) = s.listen_port {
                ui.label(RichText::new(format!("{} Port {port}", icons::PLUGS)).small().color(p.text_dim))
                    .on_hover_text("Incoming connection port");
            }
            ui.label(RichText::new(format!("{} {} peers", icons::USERS, s.peers_live)).small().color(p.text_dim));
            if let Some(free) = self.snap.free_space {
                ui.label(
                    RichText::new(format!("{} {} free", icons::HARD_DRIVE, fmt_bytes(free))).small().color(p.text_dim),
                )
                .on_hover_text("Free space in the default download folder");
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let ups: Vec<f32> = self.snap.history.iter().rev().take(90).rev().map(|(_, u)| *u as f32).collect();
                let downs: Vec<f32> = self.snap.history.iter().rev().take(90).rev().map(|(d, _)| *d as f32).collect();
                if s.alt_speed {
                    ui.label(RichText::new(format!("{} Alt speed", icons::GAUGE)).small().color(p.warn));
                }
                let up_r = ui
                    .add(
                        egui::Label::new(RichText::new(fmt_speed(s.up_bps, units)).monospace().color(p.success))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!(
                        "Uploaded this session: {}\nClick to set an upload limit",
                        fmt_bytes(s.session_uploaded)
                    ));
                self.limit_menu(&up_r, false);
                ui.label(RichText::new(icons::ARROW_UP).color(p.success));
                if self.settings.appearance.show_sparklines {
                    sparkline(ui, &ups, p.success, vec2(70.0, 16.0));
                }
                ui.add_space(8.0);
                let down_r = ui
                    .add(
                        egui::Label::new(RichText::new(fmt_speed(s.down_bps, units)).monospace().color(p.accent))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!(
                        "Downloaded this session: {}\nClick to set a download limit",
                        fmt_bytes(s.session_downloaded)
                    ));
                self.limit_menu(&down_r, true);
                ui.label(RichText::new(icons::ARROW_DOWN).color(p.accent));
                if self.settings.appearance.show_sparklines {
                    sparkline(ui, &downs, p.accent, vec2(70.0, 16.0));
                }
            });
        });
    }

    /// Quick global speed-limit presets under a speed readout.
    fn limit_menu(&mut self, resp: &egui::Response, download: bool) {
        egui::Popup::menu(resp).show(|ui| {
            ui.set_min_width(170.0);
            ui.label(RichText::new(if download { "Download limit" } else { "Upload limit" }).strong());
            let current = if download { self.settings.speed.download_limit } else { self.settings.speed.upload_limit };
            for (label, kib) in [
                ("Unlimited", 0u32),
                ("256 KiB/s", 256),
                ("1 MiB/s", 1024),
                ("5 MiB/s", 5120),
                ("10 MiB/s", 10240),
                ("50 MiB/s", 51200),
            ] {
                if ui.selectable_label(current == kib, label).clicked() {
                    let mut s = self.settings.clone();
                    if download {
                        s.speed.download_limit = kib;
                    } else {
                        s.speed.upload_limit = kib;
                    }
                    self.settings = s.clone();
                    self.save_settings();
                    self.engine.send(crate::engine::EngineCommand::ApplySettings(Box::new(s)));
                    self.engine.send(crate::engine::EngineCommand::SetAltSpeed(self.snap.session.alt_speed));
                    ui.close();
                }
            }
            if self.snap.session.alt_speed {
                ui.separator();
                ui.label(RichText::new("Alternative limits are active").small());
            }
        });
    }
}
