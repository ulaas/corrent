//! The settings window: left navigation, searchable card sections, Apply/OK/Cancel.

use std::path::PathBuf;

use egui::{Align, Context, Id, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::config::{Accent, AfterAdd, ListenMode, SeedAction, Settings, SpeedUnits, ThemeMode};
use crate::engine::{EngineCommand, ToastKind};
use crate::integration::os_handler::{self, HandlerState};
use crate::ui::theme::{self, Palette, accent_color};
use crate::ui::widgets::{card, primary_button, secondary_button};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Behavior,
    Downloads,
    Connection,
    Speed,
    BitTorrent,
    Queue,
    Seeding,
    Appearance,
    Advanced,
}

impl Page {
    const ALL: [Page; 9] = [
        Page::Behavior,
        Page::Downloads,
        Page::Connection,
        Page::Speed,
        Page::BitTorrent,
        Page::Queue,
        Page::Seeding,
        Page::Appearance,
        Page::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Behavior => "General",
            Page::Downloads => "Downloads",
            Page::Connection => "Connection",
            Page::Speed => "Speed",
            Page::BitTorrent => "BitTorrent",
            Page::Queue => "Queue",
            Page::Seeding => "Seeding",
            Page::Appearance => "Appearance",
            Page::Advanced => "Advanced",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Behavior => icons::SLIDERS_HORIZONTAL,
            Page::Downloads => icons::DOWNLOAD_SIMPLE,
            Page::Connection => icons::PLUGS_CONNECTED,
            Page::Speed => icons::GAUGE,
            Page::BitTorrent => icons::SHARE_NETWORK,
            Page::Queue => icons::LIST_NUMBERS,
            Page::Seeding => icons::PLANT,
            Page::Appearance => icons::PALETTE,
            Page::Advanced => icons::WRENCH,
        }
    }

    /// Searchable keywords for every field on the page.
    fn keywords(self) -> &'static str {
        match self {
            Page::Behavior => "confirm exit tray close minimize start minimized notifications notify delete dialog skip run program command complete default handler magnet torrent association register",
            Page::Downloads => "save path folder default subfolder paused recent remember delete torrent file watch folders automatic",
            Page::Connection => "port listen upnp nat random tcp utp ipv4 ipv6 interface bind device vpn proxy socks5 peers connections limit blocklist ip filter timeout",
            Page::Speed => "download upload rate limit alternative alt schedule scheduler bandwidth",
            Page::BitTorrent => "dht lsd local peer discovery pex trackers default trackers fastresume resume",
            Page::Queue => "queue active downloads uploads total slow torrents",
            Page::Seeding => "ratio seeding time inactive limit pause remove share",
            Page::Appearance => "theme dark light system accent color colour compact rows zoom scale units animations sparklines progress",
            Page::Advanced => "log level refresh interval checking concurrent data folder reset defaults",
        }
    }
}

pub struct SettingsDraft {
    pub settings: Settings,
    pub page: Page,
    pub search: String,
    pub trackers_text: String,
}

fn row(ui: &mut Ui, p: &Palette, label: &str, help: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 280.0).max(200.0));
            ui.label(RichText::new(label).color(p.text));
            if !help.is_empty() {
                ui.add(egui::Label::new(RichText::new(help).small().color(p.text_dim)).wrap());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.set_width(270.0);
            add(ui);
        });
    });
    ui.add_space(6.0);
}

fn toggle(ui: &mut Ui, p: &Palette, label: &str, help: &str, value: &mut bool) {
    row(ui, p, label, help, |ui| {
        toggle_switch(ui, p, value);
    });
}

/// An iOS-style switch.
pub fn toggle_switch(ui: &mut Ui, p: &Palette, on: &mut bool) -> egui::Response {
    let size = vec2(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let bg = p.bg3.lerp_to_gamma(p.accent, t);
    ui.painter().rect_filled(rect, egui::CornerRadius::same(10), bg);
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 7.5, egui::Color32::WHITE);
    resp
}

fn heading(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).font(theme::bold(14.0)).color(p.text));
    ui.add_space(6.0);
}

fn kib(ui: &mut Ui, v: &mut u32) {
    ui.add(
        egui::DragValue::new(v)
            .range(0..=10_000_000)
            .speed(16.0)
            .custom_formatter(|n, _| if n <= 0.0 { "∞ Unlimited".into() } else { format!("{n:.0} KiB/s") })
            .custom_parser(|s| {
                let t = s.trim().trim_end_matches("KiB/s").trim();
                if t.is_empty() || t == "∞" || t.eq_ignore_ascii_case("unlimited") { Some(0.0) } else { t.parse().ok() }
            }),
    );
}

/// Path field with a Browse button; returns true when Browse was clicked (right-to-left layout).
fn path_input(ui: &mut Ui, path: &mut PathBuf) -> bool {
    let browse = ui.button(icons::FOLDER_OPEN).on_hover_text("Browse…").clicked();
    let mut s = path.display().to_string();
    if ui.add(egui::TextEdit::singleline(&mut s).desired_width(220.0)).changed() {
        *path = PathBuf::from(s);
    }
    browse
}

fn minutes_input(ui: &mut Ui, minutes: &mut u16) {
    let mut h = *minutes / 60;
    let mut m = *minutes % 60;
    ui.add(egui::DragValue::new(&mut m).range(0..=59).custom_formatter(|v, _| format!("{v:02}")));
    ui.label(":");
    ui.add(egui::DragValue::new(&mut h).range(0..=23).custom_formatter(|v, _| format!("{v:02}")));
    *minutes = h * 60 + m;
}

impl CorrentApp {
    pub fn open_settings(&mut self) {
        if self.settings_draft.is_some() {
            return;
        }
        let s = self.settings.clone();
        let trackers_text = s.bittorrent.default_trackers.join("\n");
        self.settings_draft = Some(SettingsDraft { settings: s, page: Page::Behavior, search: String::new(), trackers_text });
        self.refresh_handler_status(false);
    }

    fn apply_settings(&mut self, mut new: Settings, trackers_text: &str) {
        new.bittorrent.default_trackers =
            trackers_text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
        // Handler registration is applied immediately from its card, not through the draft.
        let b = &self.settings.behavior;
        new.behavior.register_magnet_handler = b.register_magnet_handler;
        new.behavior.register_torrent_handler = b.register_torrent_handler;
        new.behavior.keep_handler_registration = b.keep_handler_registration;
        new.behavior.handler_prompt_dismissed = b.handler_prompt_dismissed;
        if new == self.settings {
            return;
        }
        let restart = self.settings.requires_restart(&new);
        let speed_changed = self.settings.speed != new.speed;
        self.settings = new.clone();
        self.save_settings();
        self.engine.send(EngineCommand::ApplySettings(Box::new(new)));
        if speed_changed && !restart {
            // Re-apply current alt mode so changed limits take effect immediately.
            self.engine.send(EngineCommand::SetAltSpeed(self.snap.session.alt_speed));
        }
        if restart {
            self.toasts.push(ToastKind::Info, "Reconnecting with the new network settings…");
        } else {
            self.toasts.push(ToastKind::Success, "Settings saved");
        }
    }

    pub(super) fn settings_window(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut draft) = self.settings_draft.take() else { return };
        let mut close = false;
        let mut apply = false;
        let resp = egui::Modal::new(Id::new("settings_modal"))
            .frame(
                egui::Frame::new()
                    .fill(p.bg1)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(egui::CornerRadius::same(14))
                    .shadow(ctx.global_style().visuals.window_shadow)
                    .inner_margin(egui::Margin::same(0)),
            )
            .show(ctx, |ui| {
                let screen = ctx.content_rect();
                let size = vec2((screen.width() - 80.0).clamp(640.0, 980.0), (screen.height() - 80.0).clamp(420.0, 680.0));
                ui.set_width(size.x);
                ui.set_height(size.y);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // ---- navigation
                    egui::Frame::new()
                        .fill(p.bg2)
                        .corner_radius(egui::CornerRadius { nw: 14, sw: 14, ne: 0, se: 0 })
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(196.0);
                                ui.set_min_height(size.y - 24.0);
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.label(RichText::new("Settings").font(theme::bold(18.0)).color(p.text));
                                ui.add_space(6.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut draft.search)
                                        .hint_text(format!("{}  Search settings", icons::MAGNIFYING_GLASS))
                                        .desired_width(f32::INFINITY),
                                );
                                ui.add_space(8.0);
                                let q = draft.search.trim().to_lowercase();
                                let mut first_match = None;
                                for page in Page::ALL {
                                    let matches = q.is_empty()
                                        || page.label().to_lowercase().contains(&q)
                                        || q.split_whitespace().all(|w| page.keywords().contains(w));
                                    if !matches {
                                        continue;
                                    }
                                    first_match.get_or_insert(page);
                                    let sel = draft.page == page;
                                    let r = super::sidebar::row(ui, p, page.icon(), p.text_dim, page.label(), None, sel);
                                    if r.clicked() {
                                        draft.page = page;
                                    }
                                }
                                // Jump to the best match while searching.
                                if !q.is_empty()
                                    && let Some(page) = first_match
                                    && !Page::ALL.iter().any(|pg| *pg == draft.page && (pg.label().to_lowercase().contains(&q) || q.split_whitespace().all(|w| pg.keywords().contains(w))))
                                {
                                    draft.page = page;
                                }
                                if first_match.is_none() {
                                    ui.label(RichText::new("No matching settings").small().color(p.text_mute));
                                }
                            });
                        });
                    // ---- content
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 6.0);
                        let content_w = size.x - 220.0;
                        ui.set_width(content_w);
                        egui::Frame::new().inner_margin(egui::Margin { left: 22, right: 22, top: 18, bottom: 0 }).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(draft.page.icon()).size(20.0).color(p.accent));
                                ui.label(RichText::new(draft.page.label()).font(theme::bold(18.0)).color(p.text));
                            });
                            ui.add_space(8.0);
                            egui::ScrollArea::vertical().max_height(size.y - 120.0).auto_shrink([false, false]).show(ui, |ui| {
                                ui.set_width(content_w - 50.0);
                                self.settings_page(ui, p, &mut draft);
                                ui.add_space(12.0);
                            });
                        });
                        // ---- footer
                        egui::Frame::new().inner_margin(egui::Margin { left: 22, right: 22, top: 10, bottom: 14 }).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut pending = draft.settings.clone();
                                pending.bittorrent.default_trackers = draft
                                    .trackers_text
                                    .lines()
                                    .map(str::trim)
                                    .filter(|l| !l.is_empty())
                                    .map(String::from)
                                    .collect();
                                let b = &self.settings.behavior;
                                pending.behavior.register_magnet_handler = b.register_magnet_handler;
                                pending.behavior.register_torrent_handler = b.register_torrent_handler;
                                pending.behavior.keep_handler_registration = b.keep_handler_registration;
                                pending.behavior.handler_prompt_dismissed = b.handler_prompt_dismissed;
                                if self.settings.requires_restart(&pending) {
                                    ui.label(RichText::new(format!("{}  Applying will briefly reconnect all torrents.", icons::ARROWS_CLOCKWISE)).small().color(p.warn));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if primary_button(ui, p, "OK").clicked() {
                                        apply = true;
                                        close = true;
                                    }
                                    if ui.add_enabled_ui(pending != self.settings, |ui| secondary_button(ui, "Apply")).inner.clicked() {
                                        apply = true;
                                    }
                                    if secondary_button(ui, "Cancel").clicked() {
                                        close = true;
                                    }
                                });
                            });
                        });
                    });
                });
            });
        if resp.should_close() {
            close = true;
        }
        if apply {
            let new = draft.settings.clone();
            let text = draft.trackers_text.clone();
            self.apply_settings(new, &text);
        }
        if !close {
            self.settings_draft = Some(draft);
        }
    }

    fn settings_page(&mut self, ui: &mut Ui, p: &Palette, d: &mut SettingsDraft) {
        let mut browse: Option<(PickTarget, PathBuf)> = None;
        let mut pick_extra: Option<u8> = None;
        self.settings_page_inner(ui, p, d, &mut browse, &mut pick_extra);
        if let Some((target, start)) = browse {
            self.pick(target, PickKind::Folder, Some(start));
        }
        match pick_extra {
            Some(0) => {
                let name = format!("corrent-backup-{}.zip", crate::util::fmt_date(crate::util::now_unix()).replace([' ', ':'], "-"));
                self.pick(PickTarget::BackupExport, PickKind::SaveZip(name), None);
            }
            Some(1) => self.pick(PickTarget::BackupImport, PickKind::OpenZip, None),
            _ => {}
        }
    }

    fn settings_page_inner(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        d: &mut SettingsDraft,
        browse: &mut Option<(PickTarget, PathBuf)>,
        pick_extra: &mut Option<u8>,
    ) {
        let s = &mut d.settings;
        match d.page {
            Page::Behavior => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Open with Corrent");
                    self.handler_card(ui, p);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Window");
                    toggle(ui, p, "Show icon in system tray", "", &mut s.behavior.show_tray_icon);
                    toggle(ui, p, "Close to tray", "Closing the window keeps Corrent running in the tray.", &mut s.behavior.close_to_tray);
                    toggle(ui, p, "Minimize to tray", "", &mut s.behavior.minimize_to_tray);
                    toggle(ui, p, "Start minimized", "", &mut s.behavior.start_minimized);
                    toggle(ui, p, "Confirm exit while torrents are active", "", &mut s.behavior.confirm_exit_while_active);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Adding & removing");
                    let mut show = !s.behavior.skip_add_dialog;
                    toggle(ui, p, "Show the add dialog", "Preview files and choose a folder before adding.", &mut show);
                    s.behavior.skip_add_dialog = !show;
                    toggle(ui, p, "Confirm before removing torrents", "", &mut s.behavior.confirm_delete);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Notifications");
                    toggle(ui, p, "Notify when a download completes", "", &mut s.behavior.notify_on_complete);
                    toggle(ui, p, "Notify when a torrent is added", "", &mut s.behavior.notify_on_add);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Run a program on completion");
                    toggle(ui, p, "Enabled", "", &mut s.behavior.run_on_complete_enabled);
                    ui.add_enabled(
                        s.behavior.run_on_complete_enabled,
                        egui::TextEdit::singleline(&mut s.behavior.run_on_complete)
                            .hint_text(r#"e.g. notify-send "Done" "%N""#)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.label(
                        RichText::new("Runs the program directly (no shell). %N name · %F content path · %D save folder · %C category · %T tags · %Z size (bytes) · %I info hash")
                            .small()
                            .color(p.text_dim),
                    );
                });
            }
            Page::Downloads => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Saving");
                    row(ui, p, "Default save folder", "", |ui| {
                        if path_input(ui, &mut s.downloads.save_path) {
                            *browse = Some((PickTarget::SettingsSaveFolder, s.downloads.save_path.clone()));
                        }
                    });
                    toggle(ui, p, "Create a subfolder for multi-file torrents", "", &mut s.downloads.create_subfolder);
                    toggle(ui, p, "Add torrents paused", "", &mut s.downloads.start_paused);
                    toggle(ui, p, "Remember recently used folders", "", &mut s.downloads.remember_last_path);
                    toggle(ui, p, "Delete .torrent files after adding", "", &mut s.downloads.delete_torrent_file_after_add);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        heading(ui, p, "Watch folders");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(format!("{}  Add folder", icons::PLUS)).clicked() {
                                *browse = Some((PickTarget::WatchFolderAdd, s.downloads.save_path.clone()));
                            }
                        });
                    });
                    ui.label(RichText::new(".torrent files that appear in these folders are added automatically.").small().color(p.text_dim));
                    ui.add_space(6.0);
                    let cats: Vec<String> = self.snap.categories.keys().cloned().collect();
                    let mut remove = None;
                    for (i, w) in s.watch.iter_mut().enumerate() {
                        egui::Frame::new().fill(p.bg1).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                toggle_switch(ui, p, &mut w.enabled);
                                ui.label(RichText::new(icons::FOLDER_SIMPLE).color(p.warn));
                                ui.label(RichText::new(w.path.display().to_string()).strong().color(p.text));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.button(icons::TRASH).on_hover_text("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                });
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Category").small().color(p.text_dim));
                                egui::ComboBox::from_id_salt(("wcat", i))
                                    .selected_text(w.category.clone().unwrap_or_else(|| "None".into()))
                                    .show_ui(ui, |ui| {
                                        if ui.selectable_label(w.category.is_none(), "None").clicked() {
                                            w.category = None;
                                        }
                                        for c in &cats {
                                            if ui.selectable_label(w.category.as_ref() == Some(c), c).clicked() {
                                                w.category = Some(c.clone());
                                            }
                                        }
                                    });
                                ui.checkbox(&mut w.start_paused, "Add paused");
                                ui.label(RichText::new("Afterwards").small().color(p.text_dim));
                                egui::ComboBox::from_id_salt(("wafter", i))
                                    .selected_text(match w.after_add {
                                        AfterAdd::Delete => "Delete file",
                                        _ => "Rename to .added",
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut w.after_add, AfterAdd::Rename, "Rename to .added");
                                        ui.selectable_value(&mut w.after_add, AfterAdd::Delete, "Delete file");
                                    });
                            });
                            ui.horizontal(|ui| {
                                let mut custom = w.save_path.is_some();
                                ui.checkbox(&mut custom, "Save to a different folder");
                                if custom {
                                    let mut path = w.save_path.clone().unwrap_or_else(|| s.downloads.save_path.clone());
                                    if path_input(ui, &mut path) {
                                        *browse = Some((PickTarget::WatchFolderSave(i), path.clone()));
                                    }
                                    w.save_path = Some(path);
                                } else {
                                    w.save_path = None;
                                }
                            });
                        });
                        ui.add_space(6.0);
                    }
                    if let Some(i) = remove {
                        s.watch.remove(i);
                    }
                    if s.watch.is_empty() {
                        ui.label(RichText::new("No watch folders.").color(p.text_mute));
                    }
                });
            }
            Page::Connection => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Listening");
                    row(ui, p, "Incoming port", "Forward this port in your router if UPnP is off.", |ui| {
                        if ui.button(icons::SHUFFLE).on_hover_text("Pick a random port").clicked() {
                            s.connection.listen_port = 20000 + (crate::util::now_unix() % 40000) as u16;
                        }
                        ui.add_enabled(!s.connection.random_port, egui::DragValue::new(&mut s.connection.listen_port).range(1024..=65535));
                    });
                    toggle(ui, p, "Use a random port on each start", "", &mut s.connection.random_port);
                    toggle(ui, p, "UPnP / NAT-PMP port forwarding", "Ask your router to forward the port automatically.", &mut s.connection.upnp);
                    row(ui, p, "Protocol", "uTP is experimental in the engine.", |ui| {
                        egui::ComboBox::from_id_salt("listen_mode")
                            .selected_text(match s.connection.listen_mode {
                                ListenMode::Tcp => "TCP",
                                ListenMode::Utp => "uTP",
                                ListenMode::TcpAndUtp => "TCP and uTP",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.connection.listen_mode, ListenMode::Tcp, "TCP");
                                ui.selectable_value(&mut s.connection.listen_mode, ListenMode::TcpAndUtp, "TCP and uTP");
                                ui.selectable_value(&mut s.connection.listen_mode, ListenMode::Utp, "uTP");
                            });
                    });
                    toggle(ui, p, "IPv4 only", "Disable IPv6.", &mut s.connection.ipv4_only);
                    row(ui, p, "Bind to network interface", "e.g. a VPN adapter name. Leave empty for any.", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.connection.bind_device).desired_width(200.0).hint_text("Any interface"));
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Connections");
                    row(ui, p, "Maximum connections (global)", "0 = unlimited", |ui| {
                        ui.add(egui::DragValue::new(&mut s.connection.max_peers_global).range(0..=10000));
                    });
                    row(ui, p, "Maximum connections per torrent", "Applies to newly added torrents. 0 = unlimited", |ui| {
                        ui.add(egui::DragValue::new(&mut s.connection.max_peers_per_torrent).range(0..=2000));
                    });
                    row(ui, p, "Connection timeout", "", |ui| {
                        ui.add(egui::DragValue::new(&mut s.connection.connect_timeout_secs).range(2..=120).suffix(" s"));
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Proxy");
                    toggle(ui, p, "Use a SOCKS5 proxy for peer connections", "", &mut s.connection.proxy_enabled);
                    ui.add_enabled_ui(s.connection.proxy_enabled, |ui| {
                        row(ui, p, "Host and port", "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.connection.proxy_port).range(1..=65535));
                            ui.add(egui::TextEdit::singleline(&mut s.connection.proxy_host).desired_width(160.0).hint_text("127.0.0.1"));
                        });
                        row(ui, p, "Username", "Optional", |ui| {
                            ui.add(egui::TextEdit::singleline(&mut s.connection.proxy_username).desired_width(200.0));
                        });
                        row(ui, p, "Password", "Stored in the settings file.", |ui| {
                            ui.add(egui::TextEdit::singleline(&mut s.connection.proxy_password).password(true).desired_width(200.0));
                        });
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "IP filtering");
                    row(ui, p, "Blocklist URL", "A P2P/eMule-format blocklist downloaded at startup.", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.connection.blocklist_url).desired_width(250.0).hint_text("https://…"));
                    });
                });
            }
            Page::Speed => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Global limits");
                    row(ui, p, "Download limit", "Drag or type a value; 0 = unlimited.", |ui| kib(ui, &mut s.speed.download_limit));
                    row(ui, p, "Upload limit", "", |ui| kib(ui, &mut s.speed.upload_limit));
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Alternative limits");
                    ui.label(RichText::new("Toggle with the gauge button in the toolbar or Ctrl+Shift+S.").small().color(p.text_dim));
                    ui.add_space(6.0);
                    row(ui, p, "Download limit", "", |ui| kib(ui, &mut s.speed.alt_download_limit));
                    row(ui, p, "Upload limit", "", |ui| kib(ui, &mut s.speed.alt_upload_limit));
                    toggle(ui, p, "Use alternative limits at startup", "", &mut s.speed.alt_enabled);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Schedule");
                    toggle(ui, p, "Switch to alternative limits on a schedule", "", &mut s.speed.schedule.enabled);
                    ui.add_enabled_ui(s.speed.schedule.enabled, |ui| {
                        row(ui, p, "From", "", |ui| minutes_input(ui, &mut s.speed.schedule.from_minutes));
                        row(ui, p, "To", "", |ui| minutes_input(ui, &mut s.speed.schedule.to_minutes));
                        ui.horizontal(|ui| {
                            for (i, day) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
                                let mut on = s.speed.schedule.days & (1 << i) != 0;
                                if ui.toggle_value(&mut on, *day).changed() {
                                    if on {
                                        s.speed.schedule.days |= 1 << i;
                                    } else {
                                        s.speed.schedule.days &= !(1 << i);
                                    }
                                }
                            }
                        });
                    });
                });
            }
            Page::BitTorrent => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Peer discovery");
                    toggle(ui, p, "DHT (decentralized network)", "Needed for magnet links without trackers.", &mut s.bittorrent.dht);
                    toggle(ui, p, "Local peer discovery", "Find peers on your local network.", &mut s.bittorrent.lsd);
                    toggle(ui, p, "Use trackers", "", &mut s.bittorrent.trackers_enabled);
                    ui.label(RichText::new("Peer exchange (PEX) is always on.").small().color(p.text_dim));
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Default trackers");
                    ui.label(RichText::new("Added to every torrent. One URL per line.").small().color(p.text_dim));
                    ui.add(egui::TextEdit::multiline(&mut d.trackers_text).desired_rows(5).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Resume data");
                    toggle(
                        ui,
                        p,
                        "Fast resume",
                        "Skip full re-verification of data on startup. Turning this off makes every start re-check all torrents.",
                        &mut s.bittorrent.fastresume,
                    );
                });
            }
            Page::Queue => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Torrent queue");
                    toggle(ui, p, "Enable queueing", "Limit how many torrents run at once; the rest wait as “Queued”.", &mut s.queue.enabled);
                    ui.add_enabled_ui(s.queue.enabled, |ui| {
                        row(ui, p, "Maximum active downloads", "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.queue.max_active_downloads).range(1..=1000));
                        });
                        row(ui, p, "Maximum active uploads", "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.queue.max_active_uploads).range(1..=1000));
                        });
                        row(ui, p, "Maximum active torrents", "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.queue.max_active_total).range(1..=2000));
                        });
                        toggle(ui, p, "Don't count slow torrents", "Torrents below these rates don't take a slot.", &mut s.queue.ignore_slow);
                        ui.add_enabled_ui(s.queue.ignore_slow, |ui| {
                            row(ui, p, "Download rate threshold", "", |ui| {
                                ui.add(egui::DragValue::new(&mut s.queue.slow_down_kib).range(0..=100000).suffix(" KiB/s"));
                            });
                            row(ui, p, "Upload rate threshold", "", |ui| {
                                ui.add(egui::DragValue::new(&mut s.queue.slow_up_kib).range(0..=100000).suffix(" KiB/s"));
                            });
                        });
                    });
                });
            }
            Page::Seeding => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "When to stop seeding");
                    ui.label(RichText::new("Per-torrent overrides are available from the context menu.").small().color(p.text_dim));
                    ui.add_space(6.0);
                    row(ui, p, "Share ratio reaches", "", |ui| {
                        ui.add_enabled(s.seeding.ratio_enabled, egui::DragValue::new(&mut s.seeding.ratio).range(0.1..=100.0).speed(0.05).fixed_decimals(2));
                        toggle_switch(ui, p, &mut s.seeding.ratio_enabled);
                    });
                    row(ui, p, "Seeding time reaches", "", |ui| {
                        ui.add_enabled(s.seeding.time_enabled, egui::DragValue::new(&mut s.seeding.time_minutes).range(1..=525600).suffix(" min"));
                        toggle_switch(ui, p, &mut s.seeding.time_enabled);
                    });
                    row(ui, p, "No uploads for", "", |ui| {
                        ui.add_enabled(s.seeding.inactive_enabled, egui::DragValue::new(&mut s.seeding.inactive_minutes).range(1..=525600).suffix(" min"));
                        toggle_switch(ui, p, &mut s.seeding.inactive_enabled);
                    });
                    row(ui, p, "Then", "", |ui| {
                        egui::ComboBox::from_id_salt("seed_action")
                            .selected_text(match s.seeding.action {
                                SeedAction::Pause => "Pause the torrent",
                                SeedAction::Remove => "Remove the torrent",
                                SeedAction::RemoveWithData => "Remove torrent and files",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.seeding.action, SeedAction::Pause, "Pause the torrent");
                                ui.selectable_value(&mut s.seeding.action, SeedAction::Remove, "Remove the torrent");
                                ui.selectable_value(&mut s.seeding.action, SeedAction::RemoveWithData, "Remove torrent and files");
                            });
                    });
                });
            }
            Page::Appearance => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Theme");
                    ui.horizontal(|ui| {
                        for (mode, icon, label) in [
                            (ThemeMode::System, icons::DESKTOP, "System"),
                            (ThemeMode::Dark, icons::MOON, "Dark"),
                            (ThemeMode::Light, icons::SUN, "Light"),
                        ] {
                            let sel = s.appearance.theme == mode;
                            let r = ui.add_sized(
                                [110.0, 64.0],
                                egui::Button::new(RichText::new(format!("{icon}\n{label}")).size(14.0).color(if sel { p.text } else { p.text_dim }))
                                    .fill(if sel { p.accent_soft() } else { p.bg1 })
                                    .stroke(egui::Stroke::new(1.0, if sel { p.accent } else { p.border }))
                                    .corner_radius(egui::CornerRadius::same(10)),
                            );
                            if r.clicked() {
                                s.appearance.theme = mode;
                            }
                        }
                    });
                    ui.add_space(12.0);
                    heading(ui, p, "Accent color");
                    ui.horizontal(|ui| {
                        for a in Accent::ALL {
                            let c = accent_color(a, s.appearance.custom_accent, p.dark);
                            let (rect, r) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::click());
                            ui.painter().circle_filled(rect.center(), 12.0, c);
                            if s.appearance.accent == a {
                                ui.painter().circle_stroke(rect.center(), 14.5, egui::Stroke::new(2.0, p.text));
                            }
                            if r.on_hover_text(a.name()).clicked() {
                                s.appearance.accent = a;
                            }
                        }
                        ui.add_space(8.0);
                        let mut rgb = s.appearance.custom_accent;
                        if ui.color_edit_button_srgb(&mut rgb).on_hover_text("Custom color").changed() {
                            s.appearance.custom_accent = rgb;
                            s.appearance.accent = Accent::Custom;
                        }
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Layout");
                    toggle(ui, p, "Compact rows", "Single-line rows to fit more torrents.", &mut s.appearance.compact_rows);
                    row(ui, p, "Interface scale", "", |ui| {
                        ui.add(egui::Slider::new(&mut s.appearance.zoom, 0.75..=2.0).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    });
                    toggle(ui, p, "Color progress bars by state", "", &mut s.appearance.state_colored_progress);
                    toggle(ui, p, "Speed graphs in the status bar", "", &mut s.appearance.show_sparklines);
                    toggle(ui, p, "Animations", "", &mut s.appearance.animations);
                    row(ui, p, "Speed units", "", |ui| {
                        egui::ComboBox::from_id_salt("units")
                            .selected_text(match s.appearance.speed_units {
                                SpeedUnits::Binary => "KiB/s, MiB/s",
                                SpeedUnits::Decimal => "kB/s, MB/s",
                                SpeedUnits::Bits => "kbit/s, Mbit/s",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.appearance.speed_units, SpeedUnits::Binary, "KiB/s, MiB/s");
                                ui.selectable_value(&mut s.appearance.speed_units, SpeedUnits::Decimal, "kB/s, MB/s");
                                ui.selectable_value(&mut s.appearance.speed_units, SpeedUnits::Bits, "kbit/s, Mbit/s");
                            });
                    });
                });
            }
            Page::Advanced => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Engine");
                    row(ui, p, "Simultaneous data checks", "How many torrents verify data at the same time.", |ui| {
                        ui.add(egui::DragValue::new(&mut s.advanced.concurrent_init_limit).range(1..=32));
                    });
                    row(ui, p, "Refresh interval", "How often transfer statistics update (takes effect after restart).", |ui| {
                        ui.add(egui::DragValue::new(&mut s.advanced.refresh_interval_ms).range(200..=5000).speed(10.0).suffix(" ms"));
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, "Data");
                    let portable = if self.paths.portable { " (portable mode)" } else { "" };
                    ui.label(RichText::new(format!("{}{portable}", self.paths.data_dir.display())).monospace().color(p.text_dim));
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  Open data folder", icons::FOLDER_OPEN)).clicked() {
                            super::open_path(&self.paths.data_dir);
                        }
                        if ui.button(format!("{}  Open log folder", icons::SCROLL)).clicked() {
                            super::open_path(&self.paths.log_dir());
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  Back up data…", icons::DOWNLOAD_SIMPLE)).clicked() {
                            *pick_extra = Some(0);
                        }
                        if ui.button(format!("{}  Restore from backup…", icons::UPLOAD_SIMPLE)).clicked() {
                            *pick_extra = Some(1);
                        }
                    });
                    ui.label(RichText::new("Backups contain settings, categories/tags and the list of torrents (not the downloaded files).").small().color(p.text_dim));
                    ui.add_space(8.0);
                    if ui.button(RichText::new(format!("{}  Reset all settings to defaults", icons::ARROW_COUNTER_CLOCKWISE)).color(p.warn)).clicked() {
                        let keep_behavior_handlers = (s.behavior.register_magnet_handler, s.behavior.register_torrent_handler);
                        *s = Settings::default();
                        s.behavior.register_magnet_handler = keep_behavior_handlers.0;
                        s.behavior.register_torrent_handler = keep_behavior_handlers.1;
                        s.behavior.handler_prompt_dismissed = true;
                        d.trackers_text.clear();
                    }
                });
            }
        }
    }

    /// Status pills and buttons for the magnet / .torrent handler registration.
    fn handler_card(&mut self, ui: &mut Ui, p: &Palette) {
        let st = self.handler_status.lock().ok().and_then(|g| g.clone());
        let pill = |ui: &mut Ui, label: &str, state: Option<&HandlerState>| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, color) = match state {
                        Some(HandlerState::Ours) => ("Corrent".to_string(), p.success),
                        Some(HandlerState::OtherDefault(a)) => (a.clone(), p.warn),
                        Some(HandlerState::OtherPath) => ("Corrent (moved)".into(), p.warn),
                        Some(HandlerState::NotRegistered) => ("Not set".into(), p.text_dim),
                        Some(HandlerState::Managed) => ("Managed by package".into(), p.text_dim),
                        Some(HandlerState::Unsupported) => ("Unsupported".into(), p.text_dim),
                        None => ("Checking…".into(), p.text_dim),
                    };
                    crate::ui::widgets::badge(ui, &text, color);
                });
            });
        };
        pill(ui, "magnet: links", st.as_ref().map(|s| &s.magnet));
        pill(ui, ".torrent files", st.as_ref().map(|s| &s.torrent));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ours = st.as_ref().is_some_and(|s| s.magnet == HandlerState::Ours && s.torrent == HandlerState::Ours);
            if !ours && primary_button(ui, p, "Make Corrent the default").clicked() {
                self.set_handlers(true, true);
            }
            if (self.settings.behavior.register_magnet_handler || self.settings.behavior.register_torrent_handler)
                && secondary_button(ui, "Unregister").clicked()
            {
                self.set_handlers(false, false);
            }
            if cfg!(windows) && secondary_button(ui, "System settings…").clicked() {
                os_handler::open_system_default_settings();
            }
            if ui.button(icons::ARROWS_CLOCKWISE).on_hover_text("Refresh").clicked() {
                self.refresh_handler_status(false);
            }
        });
        if ui
            .checkbox(
                &mut self.settings.behavior.keep_handler_registration,
                RichText::new("Keep the registration up to date if Corrent is moved").small(),
            )
            .changed()
        {
            self.save_settings();
        }
    }
}
