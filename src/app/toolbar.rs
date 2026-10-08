use egui::{Align, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::EngineCommand;
use crate::ui::theme::Palette;
use crate::ui::widgets::{icon_button, icon_button_sized};

impl CorrentApp {
    pub(super) fn toolbar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.set_min_height(super::titlebar::TITLEBAR_HEIGHT);
        self.titlebar_background(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            self.titlebar_logo(ui, p);
            ui.add_space(16.0);

            if icon_button(ui, p, icons::FILE_PLUS, "Add torrent file… (Ctrl+O)", true).clicked() {
                self.pick_torrent_files();
            }
            if icon_button(ui, p, icons::MAGNET, "Add magnet link or URL… (Ctrl+Shift+O)", true).clicked() {
                self.open_magnet_dialog();
            }
            separator(ui, p);

            let sel = self.selected_views();
            let any = !sel.is_empty();
            let can_resume =
                sel.iter().any(|t| t.state.is_paused_like() || t.state == crate::engine::types::TorrentState::Error);
            let can_pause = sel.iter().any(|t| !t.state.is_paused_like());
            let hashes = self.selected_hashes();
            if icon_button(ui, p, icons::PLAY, "Resume (Space)", can_resume).clicked() {
                self.engine.send(EngineCommand::Resume(hashes.clone()));
            }
            if icon_button(ui, p, icons::PAUSE, "Pause (Space)", can_pause).clicked() {
                self.engine.send(EngineCommand::Pause(hashes.clone()));
            }
            if icon_button(ui, p, icons::TRASH, "Remove… (Del)", any).clicked() {
                self.request_delete(hashes.clone(), false);
            }
            separator(ui, p);
            if icon_button(ui, p, icons::CARET_DOUBLE_UP, "Move to top of queue", any).clicked() {
                self.engine.send(EngineCommand::QueueMove { hashes: hashes.clone(), mv: crate::store::QueueMove::Top });
            }
            if icon_button(ui, p, icons::CARET_UP, "Move up in queue", any).clicked() {
                self.engine.send(EngineCommand::QueueMove { hashes: hashes.clone(), mv: crate::store::QueueMove::Up });
            }
            if icon_button(ui, p, icons::CARET_DOWN, "Move down in queue", any).clicked() {
                self.engine
                    .send(EngineCommand::QueueMove { hashes: hashes.clone(), mv: crate::store::QueueMove::Down });
            }
            if icon_button(ui, p, icons::CARET_DOUBLE_DOWN, "Move to bottom of queue", any).clicked() {
                self.engine.send(EngineCommand::QueueMove { hashes, mv: crate::store::QueueMove::Bottom });
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.window_controls(ui, p);
                if super::titlebar::CUSTOM_DECORATIONS {
                    ui.add_space(4.0);
                    separator(ui, p);
                } else {
                    ui.add_space(8.0);
                }
                self.overflow_menu(ui, p);
                if icon_button(ui, p, icons::GEAR_SIX, "Settings (Ctrl+,)", true).clicked() {
                    self.open_settings();
                }
                let alt = self.snap.session.alt_speed;
                if icon_button_sized(
                    ui,
                    p,
                    icons::GAUGE,
                    if alt {
                        "Alternative speed limits are ON (Ctrl+Shift+S)"
                    } else {
                        "Use alternative speed limits (Ctrl+Shift+S)"
                    },
                    true,
                    alt,
                    32.0,
                )
                .clicked()
                {
                    self.engine.send(EngineCommand::SetAltSpeed(!alt));
                }
                ui.add_space(6.0);
                self.search_box(ui, p);
            });
        });
    }

    fn search_box(&mut self, ui: &mut Ui, p: &Palette) {
        let id = egui::Id::new("search_box");
        let frame = egui::Frame::new()
            .fill(p.bg0)
            .stroke(egui::Stroke::new(1.0, if ui.memory(|m| m.has_focus(id)) { p.accent } else { p.border }))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(8, 3));
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(icons::MAGNIFYING_GLASS).color(p.text_dim));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(id)
                        .hint_text("Search torrents  (Ctrl+F)")
                        .frame(egui::Frame::NONE)
                        .desired_width(220.0),
                );
                if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.search.clear();
                    resp.surrender_focus();
                }
                if !self.search.is_empty() && ui.add(egui::Button::new(icons::X).frame(false).small()).clicked() {
                    self.search.clear();
                }
            });
        });
    }
}

fn separator(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
    ui.add_space(4.0);
}
