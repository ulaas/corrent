//! First-run welcome: download folder, look, and opening magnet links with Corrent.

use crate::ui::widgets::Labeled;
use std::path::PathBuf;

use egui::{Align, Context, Id, Layout, RichText, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::config::{Accent, ThemeMode};
use crate::integration::os_handler::HandlerState;
use crate::ui::theme::{self, Palette, accent_color};
use crate::ui::widgets::{primary_button, secondary_button};

pub struct Onboarding {
    pub step: usize,
}

const STEPS: usize = 3;

impl CorrentApp {
    pub(super) fn onboarding(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut ob) = self.onboarding.take() else { return };
        let mut done = false;
        let mut browse = false;
        let resp = egui::Modal::new(Id::new("onboarding")).show(ctx, |ui| {
            ui.set_width(560.0);
            // Progress dots
            ui.horizontal(|ui| {
                for i in 0..STEPS {
                    let (r, _) =
                        ui.allocate_exact_size(vec2(if i == ob.step { 22.0 } else { 8.0 }, 8.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        r,
                        egui::CornerRadius::same(4),
                        if i <= ob.step { p.accent } else { p.bg3 },
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new(t!("Skip")).color(p.text_dim)).frame(false)).clicked() {
                        done = true;
                    }
                });
            });
            ui.add_space(14.0);
            match ob.step {
                0 => {
                    ui.horizontal(|ui| {
                        let tex = self.app_icon_texture(ui.ctx());
                        ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(44.0, 44.0)));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(t!("Welcome to Corrent")).font(theme::title()).color(p.text));
                            ui.label(
                                RichText::new(t!("A few quick choices and you're ready to go.")).color(p.text_dim),
                            );
                        });
                    });
                    ui.add_space(16.0);
                    ui.label(RichText::new(t!("Where should downloads go?")).strong().color(p.text));
                    ui.horizontal(|ui| {
                        let mut s = self.settings.downloads.save_path.display().to_string();
                        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(440.0)).changed() {
                            self.settings.downloads.save_path = PathBuf::from(s);
                        }
                        if ui.button(icons::FOLDER_OPEN).labeled(t!("Browse…")).clicked() {
                            browse = true;
                        }
                    });
                    ui.label(
                        RichText::new(t!("You can choose a different folder for each torrent when adding it."))
                            .small()
                            .color(p.text_dim),
                    );
                }
                1 => {
                    ui.label(RichText::new(t!("Pick a look")).font(theme::title()).color(p.text));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        for (mode, icon, label) in [
                            (ThemeMode::System, icons::DESKTOP, t!("System")),
                            (ThemeMode::Dark, icons::MOON, t!("Dark")),
                            (ThemeMode::Light, icons::SUN, t!("Light")),
                        ] {
                            let sel = self.settings.appearance.theme == mode;
                            let r = ui.add_sized(
                                [120.0, 70.0],
                                egui::Button::new(RichText::new(format!("{icon}\n{label}")).size(15.0))
                                    .fill(if sel { p.accent_soft() } else { p.bg1 })
                                    .stroke(egui::Stroke::new(1.0, if sel { p.accent } else { p.border }))
                                    .corner_radius(egui::CornerRadius::same(10)),
                            );
                            if r.clicked() {
                                self.settings.appearance.theme = mode;
                            }
                        }
                    });
                    ui.add_space(12.0);
                    ui.label(RichText::new(t!("Accent color")).strong().color(p.text));
                    ui.horizontal(|ui| {
                        for a in Accent::ALL {
                            let c = accent_color(a, self.settings.appearance.custom_accent, p.dark);
                            let (rect, r) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::click());
                            ui.painter().circle_filled(rect.center(), 12.0, c);
                            if self.settings.appearance.accent == a {
                                ui.painter().circle_stroke(rect.center(), 14.5, egui::Stroke::new(2.0, p.text));
                            }
                            if r.on_hover_text(a.name()).clicked() {
                                self.settings.appearance.accent = a;
                            }
                        }
                    });
                }
                _ => {
                    ui.label(RichText::new(t!("Open magnet links with Corrent?")).font(theme::title()).color(p.text));
                    ui.add_space(6.0);
                    let st = self.handler_status.lock().ok().and_then(|g| g.clone());
                    let current = match st.as_ref().map(|s| &s.magnet) {
                        Some(HandlerState::Ours) => t!("Corrent already opens magnet links.").to_string(),
                        Some(HandlerState::OtherDefault(app)) => t!("Right now they open with {app}.", app = app),
                        _ => t!("Clicking a magnet link in your browser will add it here.").to_string(),
                    };
                    ui.label(RichText::new(current).color(p.text_dim));
                    ui.add_space(12.0);
                    let ours = st.as_ref().is_some_and(|s| s.magnet == HandlerState::Ours);
                    if !ours
                        && primary_button(ui, p, &format!("{}  {}", icons::MAGNET, t!("Make Corrent the default")))
                            .clicked()
                    {
                        self.set_handlers(true, true);
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(t!("You can change this any time in Settings → General."))
                            .small()
                            .color(p.text_dim),
                    );
                }
            }
            ui.add_space(18.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let last = ob.step + 1 == STEPS;
                if primary_button(ui, p, if last { t!("Get started") } else { t!("Next") }).clicked() {
                    if last {
                        done = true;
                    } else {
                        ob.step += 1;
                    }
                }
                if ob.step > 0 && secondary_button(ui, t!("Back")).clicked() {
                    ob.step -= 1;
                }
            });
        });
        if browse {
            self.pick(PickTarget::OnboardingFolder, PickKind::Folder, Some(self.settings.downloads.save_path.clone()));
        }
        if resp.should_close() || done {
            self.settings.behavior.handler_prompt_dismissed = true;
            self.save_settings();
            self.engine.send(crate::engine::EngineCommand::ApplySettings(Box::new(self.settings.clone())));
        } else {
            self.onboarding = Some(ob);
        }
    }
}
