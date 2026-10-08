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
                ui.label(RichText::new(fmt_speed(s.up_bps, units)).monospace().color(p.success))
                    .on_hover_text(format!("Uploaded this session: {}", fmt_bytes(s.session_uploaded)));
                ui.label(RichText::new(icons::ARROW_UP).color(p.success));
                if self.settings.appearance.show_sparklines {
                    sparkline(ui, &ups, p.success, vec2(70.0, 16.0));
                }
                ui.add_space(8.0);
                ui.label(RichText::new(fmt_speed(s.down_bps, units)).monospace().color(p.accent))
                    .on_hover_text(format!("Downloaded this session: {}", fmt_bytes(s.session_downloaded)));
                ui.label(RichText::new(icons::ARROW_DOWN).color(p.accent));
                if self.settings.appearance.show_sparklines {
                    sparkline(ui, &downs, p.accent, vec2(70.0, 16.0));
                }
            });
        });
    }
}
