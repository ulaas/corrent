//! About, statistics and log viewer windows plus the toolbar overflow menu.

use std::time::Duration;

use egui::{Align, Context, Id, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use crate::ui::widgets::Labeled;
use tracing::Level;

use super::CorrentApp;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{card, primary_button, secondary_button};
use crate::util::{fmt_bytes, fmt_duration, fmt_ratio};

#[derive(Default)]
pub struct Extras {
    pub about: bool,
    pub stats: bool,
    pub streams: bool,
    pub log: bool,
    /// The Keyboard shortcuts window.
    pub shortcuts: bool,
    pub log_filter: String,
    pub log_level: u8, // 0 = info+, 1 = warn+, 2 = error
    log_index: LogIndex,
}

/// Which log lines match the Log window's filter, by line number: lines already looked at aren't
/// looked at again, and only the visible ones are copied out of the buffer.
#[derive(Default)]
struct LogIndex {
    filter: String,
    min: Option<Level>,
    /// The newest line looked at.
    seen: u64,
    matches: Vec<u64>,
}

impl LogIndex {
    /// `filter` is lower case.
    fn update(&mut self, lines: &std::collections::VecDeque<super::log::LogLine>, filter: &str, min: Level) {
        if self.filter != filter || self.min != Some(min) {
            *self = LogIndex { filter: filter.to_string(), min: Some(min), ..Default::default() };
        }
        // Lines that fell out of the buffer (or Clear).
        let oldest = lines.front().map_or(u64::MAX, |l| l.seq);
        let gone = self.matches.partition_point(|s| *s < oldest);
        self.matches.drain(..gone);
        for l in lines.iter().rev().take_while(|l| l.seq > self.seen).collect::<Vec<_>>().into_iter().rev() {
            self.seen = l.seq;
            if l.level <= min
                && (filter.is_empty() || l.message.to_lowercase().contains(filter) || l.target.contains(filter))
            {
                self.matches.push(l.seq);
            }
        }
    }

    /// Copies of the matches in `range`.
    fn visible(
        &self,
        lines: &std::collections::VecDeque<super::log::LogLine>,
        range: std::ops::Range<usize>,
    ) -> Vec<super::log::LogLine> {
        let oldest = lines.front().map_or(0, |l| l.seq);
        (self.matches.get(range).unwrap_or_default().iter())
            .filter_map(|s| lines.get(usize::try_from(s.checked_sub(oldest)?).ok()?).cloned())
            .collect()
    }
}

/// One line of About: `key` is the English name "Copy info" writes, `label` what the window shows.
pub(super) struct AboutFact {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
}

/// What About says about this build and where it keeps its data (the version comes first).
pub(super) fn about_facts(paths: &crate::config::Paths) -> Vec<AboutFact> {
    let fact = |key, label, value: String| AboutFact { key, label, value };
    let build = if cfg!(debug_assertions) { "debug" } else { "release" };
    let mut data = paths.data_dir.display().to_string();
    if paths.portable {
        data.push_str(" (portable)");
    }
    vec![
        fact("Version", t!("Version"), env!("CARGO_PKG_VERSION").to_string()),
        fact("Commit", t!("Commit"), crate::BUILD_SHA.unwrap_or("unknown").to_string()),
        fact("Build", t!("Build"), build.to_string()),
        fact("Platform", t!("Platform"), format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH)),
        fact("Renderer", t!("Renderer"), "OpenGL (glow)".to_string()),
        fact("Engine", t!("Engine"), "librqbit 9.0.1".to_string()),
        fact("Data", t!("Data folder"), data),
    ]
}

/// The text "Copy info" puts on the clipboard (English, for bug reports).
pub(super) fn about_copy_text(facts: &[AboutFact]) -> String {
    let lines: Vec<String> = facts.iter().map(|f| format!("{}: {}", f.key, f.value)).collect();
    format!("Corrent\n{}", lines.join("\n"))
}

impl CorrentApp {
    pub(super) fn overflow_menu(&mut self, ui: &mut Ui, p: &Palette) {
        let r = crate::ui::widgets::menu_button(
            ui,
            RichText::new(icons::DOTS_THREE_VERTICAL).size(18.0).color(p.text_dim),
            |ui| {
                ui.set_min_width(220.0);
                if ui.button(format!("{}  {}", icons::PAUSE, t!("Pause all"))).clicked() {
                    self.engine.send(crate::engine::EngineCommand::PauseAll);
                }
                if ui.button(format!("{}  {}", icons::PLAY, t!("Resume all"))).clicked() {
                    self.engine.send(crate::engine::EngineCommand::ResumeAll);
                }
                if !self.snap.undoable.is_empty() {
                    let n = self.snap.undoable.len();
                    let label = if n == 1 {
                        format!(
                            "{}  {}",
                            icons::ARROW_COUNTER_CLOCKWISE,
                            t!(
                                "Undo remove “{name}”",
                                name = self.snap.undoable.first().map(|u| u.1.as_str()).unwrap_or_default()
                            )
                        )
                    } else {
                        format!("{}  {}", icons::ARROW_COUNTER_CLOCKWISE, t!("Undo remove ({n} torrents)", n = n))
                    };
                    if ui.button(label).clicked() {
                        let hashes = self.snap.undoable.iter().map(|(h, _)| *h).collect();
                        self.engine.send(crate::engine::EngineCommand::UndoRemove(hashes));
                    }
                }
                ui.separator();
                let mut sidebar = self.prefs.show_sidebar;
                if ui.checkbox(&mut sidebar, t!("Sidebar  (Ctrl+B)")).changed() {
                    self.prefs.show_sidebar = sidebar;
                }
                let mut details = self.prefs.show_details;
                if ui.checkbox(&mut details, t!("Details panel  (Ctrl+I)")).changed() {
                    self.prefs.show_details = details;
                }
                let mut compact = self.settings.appearance.compact_rows;
                if ui.checkbox(&mut compact, t!("Compact rows")).changed() {
                    self.settings.appearance.compact_rows = compact;
                    self.save_settings();
                }
                ui.separator();
                self.server_menu(ui);
                ui.separator();
                let local = self.server.is_none();
                if local && ui.button(format!("{}  {}", icons::FILE_PLUS, t!("Create torrent…  (Ctrl+N)"))).clicked()
                {
                    self.open_creator();
                }
                crate::ui::widgets::menu_button(
                    ui,
                    format!("{}  {}", icons::POWER, t!("When downloads finish")),
                    |ui| {
                        for a in crate::integration::power::WhenDone::ALL.into_iter().filter(|a| a.available()) {
                            if ui.add(egui::Button::selectable(self.when_done.action == a, a.label())).clicked() {
                                self.set_when_done(a);
                                ui.close();
                            }
                        }
                    },
                );
                // Once a newer version is known, the item is the update itself.
                if let Some(version) = self.update_version().map(str::to_string) {
                    let label = t!("Update to Corrent {version}", version = version);
                    if ui.button(format!("{}  {}", icons::ARROW_CIRCLE_UP, label)).clicked() {
                        self.open_update();
                    }
                } else if ui.button(format!("{}  {}", icons::ARROW_CIRCLE_UP, t!("Check for updates"))).clicked() {
                    self.check_for_updates(true);
                }
                if local
                    && ui.button(format!("{}  {}", icons::ARROW_SQUARE_IN, t!("Import from another client…"))).clicked()
                {
                    self.open_import();
                }
                if ui.button(format!("{}  {}", icons::CHART_BAR, t!("Statistics"))).clicked() {
                    self.extras.stats = true;
                }
                if local
                    && self.media.local().is_some()
                    && ui.button(format!("{}  {}", icons::GLOBE, t!("Web interface sign-in…"))).clicked()
                {
                    self.open_web_account();
                }
                if ui.button(format!("{}  {}", icons::TELEVISION, t!("TV mode"))).clicked() {
                    self.set_tv(true);
                }
                if local && ui.button(format!("{}  {}", icons::PLAY_CIRCLE, t!("Streaming"))).clicked() {
                    self.extras.streams = true;
                }
                if ui.button(format!("{}  {}", icons::SCROLL, t!("Log"))).clicked() {
                    self.extras.log = true;
                }
                if ui.button(format!("{}  {}", icons::FOLDER_OPEN, t!("Open data folder"))).clicked() {
                    super::open_path(&self.paths.data_dir);
                }
                if ui.button(format!("{}  {}", icons::FIRST_AID_KIT, t!("Save diagnostics…"))).clicked() {
                    self.open_diagnostics();
                }
                ui.separator();
                if ui.button(format!("{}  {}", icons::COMMAND, t!("Command palette  (Ctrl+K)"))).clicked() {
                    self.toggle_palette();
                }
                if ui.button(format!("{}  {}", icons::KEYBOARD, t!("Keyboard shortcuts  (F1)"))).clicked() {
                    self.extras.shortcuts = true;
                }
                if ui.button(format!("{}  {}", icons::INFO, t!("About Corrent"))).clicked() {
                    self.extras.about = true;
                }
                if ui.button(format!("{}  {}", icons::SIGN_OUT, t!("Quit"))).clicked() {
                    self.quitting = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );
        // Icon-only: give screen readers a name.
        let _ = r.response.labeled(t!("More"));
    }

    pub(super) fn extra_windows(&mut self, ctx: &Context, p: &Palette) {
        self.about_window(ctx, p);
        self.stats_window(ctx, p);
        self.streams_window(ctx, p);
        self.log_window(ctx, p);
        self.shortcuts_window(ctx, p);
        self.diagnostics_window(ctx, p);
        self.get_mpv_window(ctx, p);
        self.update_window(ctx, p);
    }

    fn about_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.about {
            return;
        }
        let facts = about_facts(&self.paths);
        let (mut update, mut check, mut copy, mut close) = (false, false, false, false);
        let r = egui::Modal::new(Id::new("about")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                let tex = self.app_icon_texture(ctx);
                ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(72.0, 72.0)));
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(t!("Corrent")).font(theme::bold(theme::type_scale::DISPLAY)).color(p.text));
                    ui.label(RichText::new(t!("Version {v}", v = env!("CARGO_PKG_VERSION"))).color(p.text_dim));
                    ui.label(RichText::new(t!("A beautiful, stable BitTorrent client.")).small().color(p.text_dim));
                });
            });
            ui.add_space(12.0);
            egui::Grid::new("about_facts").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                // The version is in the header.
                for f in facts.iter().skip(1) {
                    ui.label(RichText::new(f.label).color(p.text_dim));
                    let value = ui.add(egui::Label::new(RichText::new(&f.value).color(p.text)).truncate());
                    if f.key == "Commit" {
                        value.on_hover_text(t!("The commit this build was made from (+: with uncommitted changes)"));
                    }
                    ui.end_row();
                }
            });
            ui.add_space(10.0);
            ui.label(
                RichText::new(t!("Free software under the MIT or Apache 2.0 license, at your choice."))
                    .small()
                    .color(p.text_dim),
            );
            ui.label(
                RichText::new(t!("Built with Rust, egui and librqbit.\nInter and JetBrains Mono fonts (SIL OFL), Phosphor icons (MIT)."))
                    .small()
                    .color(p.text_dim),
            );
            ui.add_space(10.0);
            // A newer release the check has found, or the way to look for one.
            ui.horizontal(|ui| {
                if let Some(found) = self.update_found() {
                    let label = format!("{}  {}", icons::ARROW_CIRCLE_UP, found.action_label());
                    update = primary_button(ui, p, &label).clicked();
                    ui.label(
                        RichText::new(t!("Corrent {new} is available.", new = found.release.version)).color(p.accent),
                    );
                } else if self.update_checking() {
                    ui.add_enabled(false, egui::Button::new(t!("Checking…")).min_size(vec2(84.0, 30.0)));
                    ui.spinner();
                } else {
                    let label = format!("{}  {}", icons::ARROWS_CLOCKWISE, t!("Check for updates"));
                    check = secondary_button(ui, &label).clicked();
                    match self.update_checked() {
                        Some(Ok(())) => {
                            ui.label(RichText::new(t!("Corrent is up to date.")).color(p.text_dim));
                        }
                        Some(Err(e)) => {
                            let text = t!("Couldn't check for updates: {e}", e = e);
                            ui.add(egui::Label::new(RichText::new(text).color(p.error)).wrap());
                        }
                        None => {}
                    }
                }
            });
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                close = secondary_button(ui, t!("Close")).clicked();
                copy = secondary_button(ui, &format!("{}  {}", icons::COPY, t!("Copy info"))).clicked();
            });
        });
        if copy {
            self.copy(about_copy_text(&facts));
        }
        if check {
            self.check_for_updates(true);
        }
        if close || r.should_close() {
            self.extras.about = false;
        }
        if update {
            self.extras.about = false;
            self.open_update();
        }
    }

    fn shortcuts_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.shortcuts {
            return;
        }
        let groups: [(&str, &[(&str, &str)]); 2] = [
            (
                t!("Anywhere"),
                &[
                    ("Ctrl+K", t!("Command palette: everything, by name")),
                    ("Ctrl+O", t!("Add torrent files")),
                    ("Ctrl+Shift+O", t!("Add magnet links or URLs")),
                    ("Ctrl+N", t!("Create a torrent")),
                    ("Ctrl+,", t!("Settings")),
                    ("Ctrl+F", t!("Search torrents")),
                    ("Ctrl+B", t!("Show or hide the sidebar")),
                    ("Ctrl+I", t!("Show or hide the details")),
                    ("Ctrl+Shift+S", t!("Alternative speed limits on or off")),
                    ("F1", t!("This window")),
                ],
            ),
            (
                t!("In the torrent list"),
                &[
                    ("↑ ↓", t!("Move and select")),
                    (t!("Home End"), t!("First and last torrent")),
                    (t!("PgUp PgDn"), t!("A page up or down")),
                    (t!("Shift+move"), t!("Select from where you started")),
                    (t!("Ctrl+move"), t!("Move without changing the selection")),
                    (t!("Ctrl+Space"), t!("Add the row to the selection, or take it out")),
                    ("Ctrl+A", t!("Select all")),
                    (t!("Space"), t!("Pause the selection; resume it if all of it is paused")),
                    (t!("Enter"), t!("Open the selected torrent's files")),
                    ("F2", t!("Rename")),
                    (t!("Del"), t!("Remove")),
                    (t!("Shift+Del"), t!("Remove with files")),
                    ("/", t!("Search torrents")),
                    (t!("Esc"), t!("Clear the selection")),
                    ("Ctrl+V", t!("Add a magnet link from the clipboard")),
                ],
            ),
        ];
        let mut open = true;
        egui::Window::new(RichText::new(format!("{}  {}", icons::KEYBOARD, t!("Keyboard shortcuts"))).strong())
            .id(Id::new("shortcuts_window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let max = (ctx.content_rect().height() - 120.0).max(160.0);
                egui::ScrollArea::vertical().max_height(max).show(ui, |ui| {
                    for (i, (title, keys)) in groups.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(10.0);
                        }
                        ui.label(RichText::new(*title).strong().color(p.text));
                        ui.add_space(2.0);
                        egui::Grid::new(("shortcuts", i)).num_columns(2).spacing(vec2(16.0, 5.0)).show(ui, |ui| {
                            for (key, what) in keys.iter() {
                                ui.label(RichText::new(*key).monospace().small().color(p.accent));
                                ui.label(RichText::new(*what).color(p.text_dim));
                                ui.end_row();
                            }
                        });
                    }
                });
            });
        if !open {
            self.extras.shortcuts = false;
        }
    }

    fn stats_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.stats {
            return;
        }
        let snap = self.snap.clone();
        let s = &snap.session;
        let mut open = true;
        egui::Window::new(RichText::new(format!("{}  {}", icons::CHART_BAR, t!("Statistics"))).strong())
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
                    ui.label(RichText::new(t!("All time")).strong().color(p.text));
                    egui::Grid::new("stats_all").num_columns(2).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
                        kv(ui, p, t!("Downloaded"), fmt_bytes(snap.all_time_downloaded));
                        kv(ui, p, t!("Uploaded"), fmt_bytes(snap.all_time_uploaded));
                        kv(ui, p, t!("Share ratio"), fmt_ratio(all_ratio));
                    });
                });
                ui.add_space(8.0);
                card(p).show(ui, |ui| {
                    ui.set_width(380.0);
                    ui.label(RichText::new(t!("This session")).strong().color(p.text));
                    egui::Grid::new("stats_session").num_columns(2).spacing(vec2(24.0, 6.0)).show(ui, |ui| {
                        kv(ui, p, t!("Uptime"), fmt_duration(Duration::from_secs(s.uptime_secs)));
                        kv(ui, p, t!("Downloaded"), fmt_bytes(s.session_downloaded));
                        kv(ui, p, t!("Uploaded"), fmt_bytes(s.session_uploaded));
                        kv(ui, p, t!("Connected peers"), s.peers_live.to_string());
                        kv(ui, p, t!("DHT nodes"), s.dht_nodes.map(|n| n.to_string()).unwrap_or_else(|| "off".into()));
                        kv(ui, p, t!("Torrents"), snap.torrents.len().to_string());
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
        egui::Window::new(RichText::new(format!("{}  {}", icons::SCROLL, t!("Log"))).strong())
            .id(Id::new("log_window"))
            .open(&mut open)
            .default_size(vec2(760.0, 420.0))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.extras.log_filter)
                            .hint_text(format!("{}  {}", icons::FUNNEL, t!("Filter")))
                            .desired_width(220.0),
                    );
                    ui.selectable_value(&mut self.extras.log_level, 0, t!("All"));
                    ui.selectable_value(&mut self.extras.log_level, 1, t!("Warnings"));
                    ui.selectable_value(&mut self.extras.log_level, 2, t!("Errors"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(format!("{}  {}", icons::COPY, t!("Copy"))).clicked() {
                            let text = crate::util::with_lock(&super::log::LINES, |l| {
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
                            });
                            self.copy(text);
                        }
                        if ui.button(format!("{}  {}", icons::TRASH, t!("Clear"))).clicked() {
                            crate::util::lock(&super::log::LINES).clear();
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
                let index = &mut self.extras.log_index;
                crate::util::with_lock(&super::log::LINES, |l| index.update(l, &filter, min));
                let count = index.matches.len();
                if count == 0 {
                    crate::ui::widgets::empty_compact(
                        ui,
                        p,
                        icons::SCROLL,
                        t!("No log entries"),
                        t!("Messages appear here as Corrent works."),
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
                            count,
                            |ui, range| {
                                // Copied out: drawing may log, and the buffer's lock is not reentrant.
                                let index = &self.extras.log_index;
                                let lines = crate::util::with_lock(&super::log::LINES, |l| index.visible(l, range));
                                for l in &lines {
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

#[cfg(test)]
mod log_tests {
    use super::*;
    use crate::app::log::LogLine;

    fn line(seq: u64, level: Level, message: &str) -> LogLine {
        LogLine { seq, time: 0, level, target: "t".into(), message: message.into() }
    }

    #[test]
    fn the_log_index_follows_the_buffer() {
        let mut lines: std::collections::VecDeque<LogLine> =
            [line(5, Level::INFO, "Alpha"), line(6, Level::ERROR, "beta"), line(7, Level::INFO, "ALPHA 2")].into();
        let mut i = LogIndex::default();
        i.update(&lines, "alpha", Level::INFO);
        assert_eq!(i.matches, vec![5, 7]);
        // New lines are added, old ones go; what was seen isn't counted twice.
        lines.pop_front();
        lines.push_back(line(8, Level::INFO, "alpha 3"));
        i.update(&lines, "alpha", Level::INFO);
        assert_eq!(i.matches, vec![7, 8]);
        let shown: Vec<String> = i.visible(&lines, 1..2).into_iter().map(|l| l.message).collect();
        assert_eq!(shown, vec!["alpha 3"]);
        // Another filter or level starts over.
        i.update(&lines, "", Level::ERROR);
        assert_eq!(i.matches, vec![6]);
        // Clear.
        lines.clear();
        i.update(&lines, "", Level::ERROR);
        assert!(i.matches.is_empty());
        assert!(i.visible(&lines, 0..5).is_empty());
    }
}
