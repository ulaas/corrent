//! About, statistics and log viewer windows plus the toolbar overflow menu.

use std::time::Duration;

use egui::{Align, Context, Id, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;
use tracing::Level;

use super::CorrentApp;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{card, secondary_button};
use crate::util::{fmt_bytes, fmt_duration, fmt_ratio};

#[derive(Default)]
pub struct Extras {
    pub about: bool,
    pub stats: bool,
    pub log: bool,
    pub log_filter: String,
    pub log_level: u8, // 0 = info+, 1 = warn+, 2 = error
}

impl CorrentApp {
    pub(super) fn overflow_menu(&mut self, ui: &mut Ui, p: &Palette) {
        ui.menu_button(RichText::new(icons::DOTS_THREE_VERTICAL).size(18.0).color(p.text_dim), |ui| {
            ui.set_min_width(220.0);
            if ui.button(format!("{}  Pause all", icons::PAUSE)).clicked() {
                self.engine.send(crate::engine::EngineCommand::PauseAll);
            }
            if ui.button(format!("{}  Resume all", icons::PLAY)).clicked() {
                self.engine.send(crate::engine::EngineCommand::ResumeAll);
            }
            if !self.snap.undoable.is_empty() {
                let n = self.snap.undoable.len();
                let label = if n == 1 {
                    format!("{}  Undo remove “{}”", icons::ARROW_COUNTER_CLOCKWISE, self.snap.undoable[0].1)
                } else {
                    format!("{}  Undo remove ({n} torrents)", icons::ARROW_COUNTER_CLOCKWISE)
                };
                if ui.button(label).clicked() {
                    let hashes = self.snap.undoable.iter().map(|(h, _)| *h).collect();
                    self.engine.send(crate::engine::EngineCommand::UndoRemove(hashes));
                }
            }
            ui.separator();
            let mut sidebar = self.prefs.show_sidebar;
            if ui.checkbox(&mut sidebar, "Sidebar  (Ctrl+B)").changed() {
                self.prefs.show_sidebar = sidebar;
            }
            let mut details = self.prefs.show_details;
            if ui.checkbox(&mut details, "Details panel  (Ctrl+I)").changed() {
                self.prefs.show_details = details;
            }
            let mut compact = self.settings.appearance.compact_rows;
            if ui.checkbox(&mut compact, "Compact rows").changed() {
                self.settings.appearance.compact_rows = compact;
                self.save_settings();
            }
            ui.separator();
            if ui.button(format!("{}  Statistics", icons::CHART_BAR)).clicked() {
                self.extras.stats = true;
            }
            if ui.button(format!("{}  Log", icons::SCROLL)).clicked() {
                self.extras.log = true;
            }
            if ui.button(format!("{}  Open data folder", icons::FOLDER_OPEN)).clicked() {
                super::open_path(&self.paths.data_dir);
            }
            ui.separator();
            if ui.button(format!("{}  About Corrent", icons::INFO)).clicked() {
                self.extras.about = true;
            }
            if ui.button(format!("{}  Quit", icons::SIGN_OUT)).clicked() {
                self.quitting = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    pub(super) fn extra_windows(&mut self, ctx: &Context, p: &Palette) {
        self.about_window(ctx, p);
        self.stats_window(ctx, p);
        self.log_window(ctx, p);
    }

    fn about_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.about {
            return;
        }
        let r = egui::Modal::new(Id::new("about")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.vertical_centered(|ui| {
                let tex = self.app_icon_texture(ctx);
                ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(72.0, 72.0)));
                ui.add_space(8.0);
                ui.label(RichText::new("Corrent").font(theme::bold(theme::type_scale::DISPLAY)).color(p.text));
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(p.text_dim));
                ui.add_space(8.0);
                ui.label(RichText::new("A beautiful, stable BitTorrent client.").color(p.text));
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Built with Rust, egui and librqbit.\nInter and JetBrains Mono fonts (SIL OFL), Phosphor icons (MIT).")
                        .small()
                        .color(p.text_dim),
                );
                ui.add_space(12.0);
                if secondary_button(ui, "Close").clicked() {
                    self.extras.about = false;
                }
            });
        });
        if r.should_close() {
            self.extras.about = false;
        }
    }

    fn stats_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.stats {
            return;
        }
        let snap = self.snap.clone();
        let s = &snap.session;
        let mut open = true;
        egui::Window::new(RichText::new(format!("{}  Statistics", icons::CHART_BAR)).strong())
            .id(Id::new("stats_window"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                let all_ratio = if snap.all_time_downloaded > 0 {
                    snap.all_time_uploaded as f32 / snap.all_time_downloaded as f32
                } else {
                    0.0
                };
                card(p).show(ui, |ui| {
                    ui.set_width(380.0);
                    ui.label(RichText::new("All time").strong().color(p.text));
                    egui::Grid::new("stats_all").num_columns(2).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
                        kv(ui, p, "Downloaded", fmt_bytes(snap.all_time_downloaded));
                        kv(ui, p, "Uploaded", fmt_bytes(snap.all_time_uploaded));
                        kv(ui, p, "Share ratio", fmt_ratio(all_ratio));
                    });
                });
                ui.add_space(8.0);
                card(p).show(ui, |ui| {
                    ui.set_width(380.0);
                    ui.label(RichText::new("This session").strong().color(p.text));
                    egui::Grid::new("stats_session").num_columns(2).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
                        kv(ui, p, "Uptime", fmt_duration(Duration::from_secs(s.uptime_secs)));
                        kv(ui, p, "Downloaded", fmt_bytes(s.session_downloaded));
                        kv(ui, p, "Uploaded", fmt_bytes(s.session_uploaded));
                        kv(ui, p, "Connected peers", s.peers_live.to_string());
                        kv(ui, p, "DHT nodes", s.dht_nodes.map(|n| n.to_string()).unwrap_or_else(|| "off".into()));
                        kv(ui, p, "Torrents", snap.torrents.len().to_string());
                    });
                });
            });
        if !open {
            self.extras.stats = false;
        }
    }

    fn log_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.log {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new(format!("{}  Log", icons::SCROLL)).strong())
            .id(Id::new("log_window"))
            .open(&mut open)
            .default_size(vec2(760.0, 420.0))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.extras.log_filter)
                            .hint_text(format!("{}  Filter", icons::FUNNEL))
                            .desired_width(220.0),
                    );
                    ui.selectable_value(&mut self.extras.log_level, 0, "All");
                    ui.selectable_value(&mut self.extras.log_level, 1, "Warnings");
                    ui.selectable_value(&mut self.extras.log_level, 2, "Errors");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(format!("{}  Copy", icons::COPY)).clicked() {
                            let text = super::log::LINES
                                .lock()
                                .map(|l| {
                                    l.iter()
                                        .map(|x| {
                                            format!(
                                                "{} {} {}: {}",
                                                crate::util::fmt_date(x.time),
                                                x.level,
                                                x.target,
                                                x.message
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .unwrap_or_default();
                            self.copy(text);
                        }
                        if ui.button(format!("{}  Clear", icons::TRASH)).clicked()
                            && let Ok(mut l) = super::log::LINES.lock()
                        {
                            l.clear();
                        }
                    });
                });
                ui.add_space(4.0);
                let filter = self.extras.log_filter.to_lowercase();
                let min = match self.extras.log_level {
                    2 => Level::ERROR,
                    1 => Level::WARN,
                    _ => Level::INFO,
                };
                let lines: Vec<super::log::LogLine> = super::log::LINES
                    .lock()
                    .map(|l| {
                        l.iter()
                            .filter(|x| x.level <= min)
                            .filter(|x| {
                                filter.is_empty()
                                    || x.message.to_lowercase().contains(&filter)
                                    || x.target.contains(&filter)
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                if lines.is_empty() {
                    crate::ui::widgets::empty_compact(
                        ui,
                        p,
                        icons::SCROLL,
                        "No log entries",
                        "Messages appear here as Corrent works.",
                    );
                    return;
                }
                egui::Frame::new()
                    .fill(p.bg0)
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        egui::ScrollArea::both().stick_to_bottom(true).auto_shrink([false, false]).show_rows(
                            ui,
                            18.0,
                            lines.len(),
                            |ui, range| {
                                for l in &lines[range] {
                                    let color = match l.level {
                                        Level::ERROR => p.error,
                                        Level::WARN => p.warn,
                                        _ => p.text_dim,
                                    };
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(crate::util::fmt_date(l.time))
                                                .monospace()
                                                .small()
                                                .color(p.text_mute),
                                        );
                                        ui.label(
                                            RichText::new(format!("{:5}", l.level)).monospace().small().color(color),
                                        );
                                        ui.label(RichText::new(&l.target).monospace().small().color(p.text_mute));
                                        ui.label(RichText::new(&l.message).monospace().small().color(p.text));
                                    });
                                }
                            },
                        );
                    });
            });
        if !open {
            self.extras.log = false;
        }
    }
}

fn kv(ui: &mut Ui, p: &Palette, k: &str, v: String) {
    ui.label(RichText::new(k).color(p.text_dim));
    ui.label(RichText::new(v).monospace().color(p.text));
    ui.end_row();
}
