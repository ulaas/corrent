//! The settings window: left navigation, searchable card sections, Apply/OK/Cancel.

use crate::ui::theme::PaletteDefExt;
use crate::ui::widgets::Labeled;
use std::path::PathBuf;

use egui::{Align, Context, Id, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::config::{Accent, AfterAdd, ListenMode, PalettePreset, SeedAction, Settings, SpeedUnits, ThemeMode};
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
    Feeds,
    Search,
    Remote,
    Metadata,
    Appearance,
    Playback,
    Advanced,
}

impl Page {
    const ALL: [Page; 14] = [
        Page::Behavior,
        Page::Downloads,
        Page::Connection,
        Page::Speed,
        Page::BitTorrent,
        Page::Queue,
        Page::Seeding,
        Page::Feeds,
        Page::Search,
        Page::Remote,
        Page::Metadata,
        Page::Appearance,
        Page::Playback,
        Page::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Behavior => t!("General"),
            Page::Downloads => t!("Downloads"),
            Page::Connection => t!("Connection"),
            Page::Speed => t!("Speed"),
            Page::BitTorrent => t!("BitTorrent"),
            Page::Queue => t!("Queue"),
            Page::Seeding => t!("Seeding"),
            Page::Feeds => t!("Feeds"),
            Page::Search => t!("Search"),
            Page::Remote => t!("Remote control"),
            Page::Metadata => t!("Metadata"),
            Page::Appearance => t!("Appearance"),
            Page::Playback => t!("Playback"),
            Page::Advanced => t!("Advanced"),
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
            Page::Feeds => icons::RSS,
            Page::Search => icons::MAGNIFYING_GLASS,
            Page::Remote => icons::PLUGS,
            Page::Metadata => icons::FILM_SLATE,
            Page::Appearance => icons::PALETTE,
            Page::Playback => icons::SPEAKER_HIGH,
            Page::Advanced => icons::WRENCH,
        }
    }

    /// Whether the draft differs from the saved settings on this page.
    fn changed(self, a: &Settings, b: &Settings) -> bool {
        match self {
            Page::Behavior => a.behavior != b.behavior,
            Page::Downloads => a.downloads != b.downloads || a.watch != b.watch,
            Page::Connection => a.connection != b.connection,
            Page::Speed => a.speed != b.speed,
            Page::BitTorrent => a.bittorrent != b.bittorrent,
            Page::Queue => a.queue != b.queue,
            Page::Seeding => a.seeding != b.seeding,
            Page::Search => a.search != b.search,
            Page::Remote => a.web_api != b.web_api,
            // Saved on the server right away, provider by provider.
            Page::Metadata => false,
            Page::Feeds => {
                (a.rss.auto_download, a.rss.refresh_minutes, a.rss.max_articles)
                    != (b.rss.auto_download, b.rss.refresh_minutes, b.rss.max_articles)
            }
            Page::Appearance => a.appearance != b.appearance,
            Page::Playback => a.playback != b.playback,
            Page::Advanced => a.advanced != b.advanced,
        }
    }

    /// Searchable keywords for every field on the page.
    fn keywords(self) -> &'static str {
        match self {
            Page::Behavior => {
                t!(
                    "confirm exit tray close minimize start minimized notifications notify delete dialog skip run program command complete default handler magnet torrent association register updates version server servers network lan find switch"
                )
            }
            Page::Downloads => {
                t!(
                    "save path folder default subfolder paused recent remember delete torrent file watch folders automatic"
                )
            }
            Page::Connection => {
                t!(
                    "port listen upnp nat random tcp utp ipv4 ipv6 interface bind device vpn proxy socks5 peers connections limit blocklist ip filter timeout"
                )
            }
            Page::Speed => t!("download upload rate limit alternative alt schedule scheduler bandwidth"),
            Page::BitTorrent => t!("dht lsd local peer discovery pex trackers default trackers fastresume resume"),
            Page::Queue => t!("queue active downloads uploads total slow torrents"),
            Page::Seeding => t!("ratio seeding time inactive limit pause remove share"),
            Page::Remote => {
                t!("remote control web api qbittorrent sonarr radarr lidarr prowlarr port password username lan")
            }
            Page::Search => t!("search torznab jackett prowlarr indexer provider api key"),
            Page::Metadata => {
                t!("metadata tmdb imdb omdb tvdb fanart musicbrainz opensubtitles posters api key scrape identify")
            }
            Page::Feeds => t!("rss atom feeds refresh interval articles auto download rules automatic episodes"),
            Page::Appearance => {
                t!(
                    "theme dark light system accent color colour palette preset nord solarized high contrast custom import export compact rows zoom scale units animations sparklines progress"
                )
            }
            Page::Playback => t!(
                "audio sound output device speakers headphones exclusive bit-perfect channels surround stereo 5.1 7.1 passthrough receiver hdmi spdif dolby dts normalize loudness equalizer bass treble replaygain gapless"
            ),
            Page::Advanced => t!("log level refresh interval checking concurrent data folder reset defaults"),
        }
    }
}

pub struct SettingsDraft {
    pub settings: Settings,
    /// New web API password (hashed when applied); empty = unchanged.
    pub new_password: String,
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
    let value = *on;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, value, ""));
    let t = crate::ui::motion::toggle(ui.ctx(), resp.id, *on, crate::ui::motion::MICRO);
    let bg = p.bg3.lerp_to_gamma(p.accent, t);
    ui.painter().rect_filled(rect, egui::CornerRadius::same(10), bg);
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 7.5, egui::Color32::WHITE);
    crate::ui::widgets::focus_ring(ui, &resp, p, 10);
    resp
}

fn heading(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).font(theme::heading()).color(p.text));
    ui.add_space(6.0);
}

fn kib(ui: &mut Ui, v: &mut u32) {
    ui.add(
        egui::DragValue::new(v)
            .range(0..=10_000_000)
            .speed(16.0)
            .custom_formatter(|n, _| if n <= 0.0 { t!("∞ Unlimited").into() } else { format!("{n:.0} KiB/s") })
            .custom_parser(|s| {
                let t = s.trim().trim_end_matches("KiB/s").trim();
                if t.is_empty() || t.starts_with('∞') || t.eq_ignore_ascii_case("unlimited") {
                    Some(0.0)
                } else {
                    t.parse().ok()
                }
            }),
    );
}

/// Path field with a Browse button; returns true when Browse was clicked (right-to-left layout).
fn path_input(ui: &mut Ui, path: &mut PathBuf) -> bool {
    let browse = ui.button(icons::FOLDER_OPEN).labeled(t!("Browse…")).clicked();
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
    pub fn open_settings_at(&mut self, page: Page) {
        self.open_settings();
        if let Some(d) = &mut self.settings_draft {
            d.page = page;
        }
    }

    pub fn open_settings(&mut self) {
        if self.settings_draft.is_some() {
            return;
        }
        let s = self.settings.clone();
        let trackers_text = s.bittorrent.default_trackers.join("\n");
        self.settings_draft = Some(SettingsDraft {
            settings: s,
            new_password: String::new(),
            page: Page::Behavior,
            search: String::new(),
            trackers_text,
        });
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
        // Kept outside the settings window (update check bookkeeping).
        new.updates.last_check = self.settings.updates.last_check;
        new.updates.skip_version = self.settings.updates.skip_version.clone();
        // Feeds and rules are edited in the Feeds view, possibly while this window is open.
        new.rss.feeds = self.settings.rss.feeds.clone();
        new.rss.rules = self.settings.rss.rules.clone();
        if new == self.settings {
            return;
        }
        let restart = self.settings.requires_restart(&new);
        if self.settings.playback != new.playback
            && let Some(pl) = &self.media.player
        {
            pl.apply_audio(&new.playback);
        }
        let speed_changed = self.settings.speed != new.speed;
        self.settings = new.clone();
        self.save_settings();
        self.engine.send(EngineCommand::ApplySettings(Box::new(new)));
        if speed_changed && !restart {
            // Re-apply current alt mode so changed limits take effect immediately.
            self.engine.send(EngineCommand::SetAltSpeed(self.snap.session.alt_speed));
        }
        if restart {
            self.toasts.push(ToastKind::Info, t!("Reconnecting with the new network settings…"));
        } else {
            self.toasts.push(ToastKind::Success, t!("Settings saved"));
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
                let size =
                    vec2((screen.width() - 80.0).clamp(640.0, 980.0), (screen.height() - 80.0).clamp(420.0, 680.0));
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
                                ui.label(RichText::new(t!("Settings")).font(theme::title()).color(p.text));
                                ui.add_space(6.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut draft.search)
                                        .hint_text(format!("{}  {}", icons::MAGNIFYING_GLASS, t!("Search settings")))
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
                                    let r =
                                        super::sidebar::row(ui, p, page.icon(), p.text_dim, page.label(), None, sel);
                                    if page.changed(&self.settings, &draft.settings) {
                                        let c = egui::pos2(r.rect.right() - 12.0, r.rect.center().y);
                                        ui.painter().circle_filled(c, 3.5, p.accent);
                                        r.clone().on_hover_text(t!("Unsaved changes"));
                                    }
                                    if r.clicked() {
                                        draft.page = page;
                                    }
                                }
                                // Jump to the best match while searching.
                                if !q.is_empty()
                                    && let Some(page) = first_match
                                    && !Page::ALL.iter().any(|pg| {
                                        *pg == draft.page
                                            && (pg.label().to_lowercase().contains(&q)
                                                || q.split_whitespace().all(|w| pg.keywords().contains(w)))
                                    })
                                {
                                    draft.page = page;
                                }
                                if first_match.is_none() {
                                    ui.label(RichText::new(t!("No matching settings")).small().color(p.text_mute));
                                }
                            });
                        });
                    // ---- content
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 6.0);
                        let content_w = size.x - 220.0;
                        ui.set_width(content_w);
                        egui::Frame::new().inner_margin(egui::Margin { left: 22, right: 22, top: 18, bottom: 0 }).show(
                            ui,
                            |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(draft.page.icon()).size(20.0).color(p.accent));
                                    ui.label(RichText::new(draft.page.label()).font(theme::title()).color(p.text));
                                });
                                ui.add_space(8.0);
                                egui::ScrollArea::vertical()
                                    .max_height(size.y - 120.0)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(content_w - 50.0);
                                        if let Some(s) = &self.server {
                                            ui.label(
                                                RichText::new(t!(
                                                    "Connected to “{name}”: its downloads, connection and queue settings are changed in its web interface. The settings here apply to this computer.",
                                                    name = s.entry.name
                                                ))
                                                .small()
                                                .color(p.warn),
                                            );
                                            ui.add_space(8.0);
                                        }
                                        self.settings_page(ui, p, &mut draft);
                                        ui.add_space(12.0);
                                    });
                            },
                        );
                        // ---- footer
                        egui::Frame::new()
                            .inner_margin(egui::Margin { left: 22, right: 22, top: 10, bottom: 14 })
                            .show(ui, |ui| {
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
                                        ui.label(
                                            RichText::new(format!(
                                                "{}  {}",
                                                icons::ARROWS_CLOCKWISE,
                                                t!("Applying will briefly reconnect all torrents.")
                                            ))
                                            .small()
                                            .color(p.warn),
                                        );
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if primary_button(ui, p, t!("OK")).clicked() {
                                            apply = true;
                                            close = true;
                                        }
                                        if ui
                                            .add_enabled_ui(
                                                pending != self.settings || !draft.new_password.is_empty(),
                                                |ui| secondary_button(ui, t!("Apply")),
                                            )
                                            .inner
                                            .clicked()
                                        {
                                            apply = true;
                                        }
                                        if secondary_button(ui, t!("Cancel")).clicked() {
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
            if !draft.new_password.is_empty() {
                draft.settings.web_api.password_hash = crate::webapi::hash_password(&draft.new_password);
                draft.new_password.clear();
            }
            let new = draft.settings.clone();
            let text = draft.trackers_text.clone();
            self.apply_settings(new, &text);
        }
        if !close {
            self.appearance_preview =
                (draft.settings.appearance != self.settings.appearance).then(|| draft.settings.appearance.clone());
            self.settings_draft = Some(draft);
        } else {
            self.appearance_preview = None;
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
                let name = format!(
                    "corrent-backup-{}.zip",
                    crate::util::fmt_date(crate::util::now_unix()).replace([' ', ':'], "-")
                );
                self.pick(PickTarget::BackupExport, PickKind::SaveZip(name), None);
            }
            Some(1) => self.pick(PickTarget::BackupImport, PickKind::OpenZip, None),
            Some(2) => self.pick(PickTarget::ThemeImport, PickKind::OpenTheme, None),
            Some(3) => {
                let name = d
                    .settings
                    .appearance
                    .custom_palette
                    .as_ref()
                    .map(|t| t.name.clone())
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "corrent".into());
                self.pick(PickTarget::ThemeExport, PickKind::SaveTheme(format!("{name}.theme.toml")), None);
            }
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
                    heading(ui, p, t!("Open with Corrent"));
                    self.handler_card(ui, p);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Updates"));
                    toggle(
                        ui,
                        p,
                        t!("Check for new versions automatically"),
                        t!("Once a day, asks GitHub for the latest release. Nothing is downloaded or installed."),
                        &mut s.updates.check_automatically,
                    );
                    if ui.button(format!("{}  {}", icons::ARROW_CIRCLE_UP, t!("Check now"))).clicked() {
                        self.check_for_updates(true);
                    }
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Window"));
                    toggle(ui, p, t!("Show icon in system tray"), "", &mut s.behavior.show_tray_icon);
                    toggle(
                        ui,
                        p,
                        t!("Close to tray"),
                        t!("Closing the window keeps Corrent running in the tray."),
                        &mut s.behavior.close_to_tray,
                    );
                    toggle(ui, p, t!("Minimize to tray"), "", &mut s.behavior.minimize_to_tray);
                    toggle(ui, p, t!("Start minimized"), "", &mut s.behavior.start_minimized);
                    toggle(
                        ui,
                        p,
                        t!("Confirm exit while torrents are active"),
                        "",
                        &mut s.behavior.confirm_exit_while_active,
                    );
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Adding & removing"));
                    let mut show = !s.behavior.skip_add_dialog;
                    toggle(
                        ui,
                        p,
                        t!("Show the add dialog"),
                        t!("Preview files and choose a folder before adding."),
                        &mut show,
                    );
                    s.behavior.skip_add_dialog = !show;
                    toggle(ui, p, t!("Confirm before removing torrents"), "", &mut s.behavior.confirm_delete);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Notifications"));
                    toggle(ui, p, t!("Notify when a download completes"), "", &mut s.behavior.notify_on_complete);
                    toggle(ui, p, t!("Notify when a torrent is added"), "", &mut s.behavior.notify_on_add);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Corrent servers on this network"));
                    toggle(
                        ui,
                        p,
                        t!("Look for Corrent servers on this network"),
                        t!("Shows a notice when a new server appears. Connecting to it still needs its pairing code."),
                        &mut s.behavior.find_servers,
                    );
                    ui.add_enabled_ui(s.behavior.find_servers, |ui| {
                        toggle(
                            ui,
                            p,
                            t!("Switch back to your server by itself"),
                            t!("When the server you left because it couldn't be reached answers again (not while a dialog or video is open)."),
                            &mut s.behavior.auto_switch_server,
                        );
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Run a program on completion"));
                    toggle(ui, p, t!("Enabled"), "", &mut s.behavior.run_on_complete_enabled);
                    ui.add_enabled(
                        s.behavior.run_on_complete_enabled,
                        egui::TextEdit::singleline(&mut s.behavior.run_on_complete)
                            .hint_text(r#"e.g. notify-send "Done" "%N""#)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.label(
                        RichText::new(t!("Runs the program directly (no shell). %N name · %F content path · %D save folder · %C category · %T tags · %Z size (bytes) · %I info hash"))
                            .small()
                            .color(p.text_dim),
                    );
                });
            }
            Page::Downloads => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Saving"));
                    row(ui, p, t!("Default save folder"), "", |ui| {
                        if path_input(ui, &mut s.downloads.save_path) {
                            *browse = Some((PickTarget::SettingsSaveFolder, s.downloads.save_path.clone()));
                        }
                    });
                    toggle(
                        ui,
                        p,
                        t!("Create a subfolder for multi-file torrents"),
                        "",
                        &mut s.downloads.create_subfolder,
                    );
                    toggle(ui, p, t!("Add torrents paused"), "", &mut s.downloads.start_paused);
                    toggle(ui, p, t!("Remember recently used folders"), "", &mut s.downloads.remember_last_path);
                    toggle(
                        ui,
                        p,
                        t!("Delete .torrent files after adding"),
                        "",
                        &mut s.downloads.delete_torrent_file_after_add,
                    );
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        heading(ui, p, t!("Watch folders"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(format!("{}  {}", icons::PLUS, t!("Add folder"))).clicked() {
                                *browse = Some((PickTarget::WatchFolderAdd, s.downloads.save_path.clone()));
                            }
                        });
                    });
                    ui.label(
                        RichText::new(t!(".torrent files that appear in these folders are added automatically."))
                            .small()
                            .color(p.text_dim),
                    );
                    ui.add_space(6.0);
                    let cats: Vec<String> = self.snap.categories.keys().cloned().collect();
                    let mut remove = None;
                    for (i, w) in s.watch.iter_mut().enumerate() {
                        egui::Frame::new()
                            .fill(p.bg1)
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    toggle_switch(ui, p, &mut w.enabled);
                                    ui.label(RichText::new(icons::FOLDER_SIMPLE).color(p.warn));
                                    ui.label(RichText::new(w.path.display().to_string()).strong().color(p.text));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button(icons::TRASH).labeled(t!("Remove")).clicked() {
                                            remove = Some(i);
                                        }
                                    });
                                });
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(t!("Category")).small().color(p.text_dim));
                                    egui::ComboBox::from_id_salt(("wcat", i))
                                        .selected_text(w.category.clone().unwrap_or_else(|| t!("None").into()))
                                        .show_ui(ui, |ui| {
                                            if ui.selectable_label(w.category.is_none(), t!("None")).clicked() {
                                                w.category = None;
                                            }
                                            for c in &cats {
                                                if ui.selectable_label(w.category.as_ref() == Some(c), c).clicked() {
                                                    w.category = Some(c.clone());
                                                }
                                            }
                                        });
                                    ui.checkbox(&mut w.start_paused, t!("Add paused"));
                                    ui.label(RichText::new(t!("Afterwards")).small().color(p.text_dim));
                                    egui::ComboBox::from_id_salt(("wafter", i))
                                        .selected_text(match w.after_add {
                                            AfterAdd::Delete => t!("Delete file"),
                                            _ => t!("Rename to .added"),
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut w.after_add,
                                                AfterAdd::Rename,
                                                t!("Rename to .added"),
                                            );
                                            ui.selectable_value(&mut w.after_add, AfterAdd::Delete, t!("Delete file"));
                                        });
                                });
                                ui.horizontal(|ui| {
                                    let mut custom = w.save_path.is_some();
                                    ui.checkbox(&mut custom, t!("Save to a different folder"));
                                    if custom {
                                        let mut path =
                                            w.save_path.clone().unwrap_or_else(|| s.downloads.save_path.clone());
                                        if path_input(ui, &mut path) {
                                            *browse = Some((PickTarget::WatchFolderSave(w.path.clone()), path.clone()));
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
                        ui.label(RichText::new(t!("No watch folders.")).color(p.text_mute));
                    }
                });
            }
            Page::Connection => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Listening"));
                    row(ui, p, t!("Incoming port"), t!("Forward this port in your router if UPnP is off."), |ui| {
                        if ui.button(icons::SHUFFLE).labeled(t!("Pick a random port")).clicked() {
                            s.connection.listen_port = 20000 + (crate::util::now_unix() % 40000) as u16;
                        }
                        ui.add_enabled(
                            !s.connection.random_port,
                            egui::DragValue::new(&mut s.connection.listen_port).range(1024..=65535),
                        );
                    });
                    toggle(ui, p, t!("Use a random port on each start"), "", &mut s.connection.random_port);
                    toggle(
                        ui,
                        p,
                        t!("UPnP / NAT-PMP port forwarding"),
                        t!("Ask your router to forward the port automatically."),
                        &mut s.connection.upnp,
                    );
                    row(ui, p, t!("Protocol"), t!("uTP is experimental in the engine."), |ui| {
                        egui::ComboBox::from_id_salt("listen_mode")
                            .selected_text(match s.connection.listen_mode {
                                ListenMode::Tcp => "TCP",
                                ListenMode::Utp => "uTP",
                                ListenMode::TcpAndUtp => t!("TCP and uTP"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.connection.listen_mode, ListenMode::Tcp, "TCP");
                                ui.selectable_value(
                                    &mut s.connection.listen_mode,
                                    ListenMode::TcpAndUtp,
                                    t!("TCP and uTP"),
                                );
                                ui.selectable_value(&mut s.connection.listen_mode, ListenMode::Utp, "uTP");
                            });
                    });
                    toggle(ui, p, t!("IPv4 only"), t!("Disable IPv6."), &mut s.connection.ipv4_only);
                    row(
                        ui,
                        p,
                        t!("Listen on address"),
                        t!("A specific local IP to accept connections on. Leave empty for all."),
                        |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.connection.listen_address)
                                    .desired_width(200.0)
                                    .hint_text(t!("All addresses")),
                            );
                            let addr = s.connection.listen_address.trim();
                            if !addr.is_empty() {
                                match addr.parse::<std::net::IpAddr>() {
                                    Err(_) => {
                                        ui.label(
                                            RichText::new(t!("Not an IP address; all addresses are used"))
                                                .small()
                                                .color(p.warn),
                                        );
                                    }
                                    Ok(ip) if ip.is_ipv6() && s.connection.ipv4_only => {
                                        ui.label(
                                            RichText::new(t!("IPv6 address, but “IPv4 only” is on"))
                                                .small()
                                                .color(p.warn),
                                        );
                                    }
                                    Ok(_) => {}
                                }
                            }
                        },
                    );
                    row(
                        ui,
                        p,
                        t!("Bind to network interface"),
                        t!("e.g. a VPN adapter name. Leave empty for any."),
                        |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.connection.bind_device)
                                    .desired_width(200.0)
                                    .hint_text(t!("Any interface")),
                            );
                        },
                    );
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Connections"));
                    row(ui, p, t!("Maximum connections (global)"), t!("0 = unlimited"), |ui| {
                        ui.add(egui::DragValue::new(&mut s.connection.max_peers_global).range(0..=10000));
                    });
                    row(
                        ui,
                        p,
                        t!("Maximum connections per torrent"),
                        t!("Applies to newly added torrents. 0 = unlimited"),
                        |ui| {
                            ui.add(egui::DragValue::new(&mut s.connection.max_peers_per_torrent).range(0..=2000));
                        },
                    );
                    row(ui, p, t!("Connection timeout"), "", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut s.connection.connect_timeout_secs).range(2..=120).suffix(" s"),
                        );
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Proxy"));
                    toggle(ui, p, t!("Use a SOCKS5 proxy for peer connections"), "", &mut s.connection.proxy_enabled);
                    ui.add_enabled_ui(s.connection.proxy_enabled, |ui| {
                        row(ui, p, t!("Host and port"), "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.connection.proxy_port).range(1..=65535));
                            ui.add(
                                egui::TextEdit::singleline(&mut s.connection.proxy_host)
                                    .desired_width(160.0)
                                    .hint_text("127.0.0.1"),
                            );
                        });
                        row(ui, p, t!("Username"), t!("Optional"), |ui| {
                            ui.add(egui::TextEdit::singleline(&mut s.connection.proxy_username).desired_width(200.0));
                        });
                        row(ui, p, t!("Password"), t!("Stored in the settings file."), |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.connection.proxy_password)
                                    .password(true)
                                    .desired_width(200.0),
                            );
                        });
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("IP filtering"));
                    row(
                        ui,
                        p,
                        t!("Blocklist"),
                        "P2P-format list (plain or .gz): a web address or a file on this computer. Loaded when the \
                         engine starts; the last download is reused if the server is unreachable.",
                        |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.connection.blocklist_url)
                                    .desired_width(250.0)
                                    .hint_text(t!("https://… or C:\\lists\\level1.gz")),
                            );
                        },
                    );
                    row(
                        ui,
                        p,
                        t!("Allowlist"),
                        t!("If set, only peers in this list are used. Leave empty normally."),
                        |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut s.connection.allowlist_url)
                                    .desired_width(250.0)
                                    .hint_text(t!("Not used")),
                            );
                        },
                    );
                    ui.label(RichText::new(t!("Your own blocked addresses")).color(p.text));
                    ui.label(
                        RichText::new(t!(
                            "One per line: an IP, a CIDR block (10.0.0.0/8) or a range (1.2.3.4-1.2.3.9)."
                        ))
                        .small()
                        .color(p.text_dim),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut s.connection.custom_blocklist)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                    for e in crate::engine::ipfilter::check_entries(&s.connection.custom_blocklist).iter().take(3) {
                        ui.label(RichText::new(format!("{}  {e}", icons::WARNING)).small().color(p.warn));
                    }
                    ui.add_space(4.0);
                    let sv = &self.snap.session;
                    ui.horizontal(|ui| {
                        let mut text = t!("{n} blocked ranges loaded", n = sv.blocked_ranges);
                        if let Some(a) = sv.allowed_ranges {
                            text.push_str(&t!(" · {a} allowed", a = a));
                        }
                        ui.label(RichText::new(text).small().color(p.text_dim));
                        if ui
                            .button(format!("{}  {}", icons::ARROWS_CLOCKWISE, t!("Reload lists")))
                            .on_hover_text(t!("Download the lists again (reconnects all torrents)"))
                            .clicked()
                        {
                            self.engine.send(EngineCommand::ReloadIpFilter);
                        }
                    });
                });
            }
            Page::Speed => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Global limits"));
                    row(ui, p, t!("Download limit"), t!("Drag or type a value; 0 = unlimited."), |ui| {
                        kib(ui, &mut s.speed.download_limit)
                    });
                    row(ui, p, t!("Upload limit"), "", |ui| kib(ui, &mut s.speed.upload_limit));
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Alternative limits"));
                    ui.label(
                        RichText::new(t!("Toggle with the gauge button in the toolbar or Ctrl+Shift+S."))
                            .small()
                            .color(p.text_dim),
                    );
                    ui.add_space(6.0);
                    row(ui, p, t!("Download limit"), "", |ui| kib(ui, &mut s.speed.alt_download_limit));
                    row(ui, p, t!("Upload limit"), "", |ui| kib(ui, &mut s.speed.alt_upload_limit));
                    toggle(ui, p, t!("Use alternative limits at startup"), "", &mut s.speed.alt_enabled);
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Schedule"));
                    toggle(ui, p, t!("Switch to alternative limits on a schedule"), "", &mut s.speed.schedule.enabled);
                    ui.add_enabled_ui(s.speed.schedule.enabled, |ui| {
                        row(ui, p, t!("From"), "", |ui| minutes_input(ui, &mut s.speed.schedule.from_minutes));
                        row(ui, p, t!("To"), "", |ui| minutes_input(ui, &mut s.speed.schedule.to_minutes));
                        ui.horizontal(|ui| {
                            for (i, day) in
                                [t!("Mon"), t!("Tue"), t!("Wed"), t!("Thu"), t!("Fri"), t!("Sat"), t!("Sun")]
                                    .iter()
                                    .enumerate()
                            {
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
                    heading(ui, p, t!("Peer discovery"));
                    toggle(
                        ui,
                        p,
                        t!("DHT (decentralized network)"),
                        t!("Needed for magnet links without trackers."),
                        &mut s.bittorrent.dht,
                    );
                    toggle(
                        ui,
                        p,
                        t!("Local peer discovery"),
                        t!("Find peers on your local network."),
                        &mut s.bittorrent.lsd,
                    );
                    toggle(ui, p, t!("Use trackers"), "", &mut s.bittorrent.trackers_enabled);
                    ui.label(RichText::new(t!("Peer exchange (PEX) is always on.")).small().color(p.text_dim));
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Default trackers"));
                    ui.label(RichText::new(t!("Added to every torrent. One URL per line.")).small().color(p.text_dim));
                    ui.add(
                        egui::TextEdit::multiline(&mut d.trackers_text)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Resume data"));
                    toggle(
                        ui,
                        p,
                        t!("Fast resume"), t!("Skip full re-verification of data on startup. Turning this off makes every start re-check all torrents."),
                        &mut s.bittorrent.fastresume,
                    );
                });
            }
            Page::Queue => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Torrent queue"));
                    toggle(
                        ui,
                        p,
                        t!("Enable queueing"),
                        t!("Limit how many torrents run at once; the rest wait as “Queued”."),
                        &mut s.queue.enabled,
                    );
                    ui.add_enabled_ui(s.queue.enabled, |ui| {
                        row(ui, p, t!("Maximum active downloads"), "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.queue.max_active_downloads).range(1..=1000));
                        });
                        row(ui, p, t!("Maximum active uploads"), "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.queue.max_active_uploads).range(1..=1000));
                        });
                        row(ui, p, t!("Maximum active torrents"), "", |ui| {
                            ui.add(egui::DragValue::new(&mut s.queue.max_active_total).range(1..=2000));
                        });
                        toggle(
                            ui,
                            p,
                            t!("Don't count slow torrents"),
                            t!("Torrents below these rates don't take a slot."),
                            &mut s.queue.ignore_slow,
                        );
                        ui.add_enabled_ui(s.queue.ignore_slow, |ui| {
                            row(ui, p, t!("Download rate threshold"), "", |ui| {
                                ui.add(
                                    egui::DragValue::new(&mut s.queue.slow_down_kib).range(0..=100000).suffix(" KiB/s"),
                                );
                            });
                            row(ui, p, t!("Upload rate threshold"), "", |ui| {
                                ui.add(
                                    egui::DragValue::new(&mut s.queue.slow_up_kib).range(0..=100000).suffix(" KiB/s"),
                                );
                            });
                        });
                    });
                });
            }
            Page::Seeding => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("When to stop seeding"));
                    ui.label(
                        RichText::new(t!("Per-torrent overrides are available from the context menu."))
                            .small()
                            .color(p.text_dim),
                    );
                    ui.add_space(6.0);
                    row(ui, p, t!("Share ratio reaches"), "", |ui| {
                        ui.add_enabled(
                            s.seeding.ratio_enabled,
                            egui::DragValue::new(&mut s.seeding.ratio).range(0.1..=100.0).speed(0.05).fixed_decimals(2),
                        );
                        toggle_switch(ui, p, &mut s.seeding.ratio_enabled);
                    });
                    row(ui, p, t!("Seeding time reaches"), "", |ui| {
                        ui.add_enabled(
                            s.seeding.time_enabled,
                            egui::DragValue::new(&mut s.seeding.time_minutes).range(1..=525600).suffix(" min"),
                        );
                        toggle_switch(ui, p, &mut s.seeding.time_enabled);
                    });
                    row(ui, p, t!("No uploads for"), "", |ui| {
                        ui.add_enabled(
                            s.seeding.inactive_enabled,
                            egui::DragValue::new(&mut s.seeding.inactive_minutes).range(1..=525600).suffix(" min"),
                        );
                        toggle_switch(ui, p, &mut s.seeding.inactive_enabled);
                    });
                    row(ui, p, t!("Then"), "", |ui| {
                        egui::ComboBox::from_id_salt("seed_action")
                            .selected_text(match s.seeding.action {
                                SeedAction::Pause => t!("Pause the torrent"),
                                SeedAction::Remove => t!("Remove the torrent"),
                                SeedAction::RemoveWithData => t!("Remove torrent and files"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.seeding.action, SeedAction::Pause, t!("Pause the torrent"));
                                ui.selectable_value(
                                    &mut s.seeding.action,
                                    SeedAction::Remove,
                                    t!("Remove the torrent"),
                                );
                                ui.selectable_value(
                                    &mut s.seeding.action,
                                    SeedAction::RemoveWithData,
                                    t!("Remove torrent and files"),
                                );
                            });
                    });
                });
            }
            Page::Appearance => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Language"));
                    let current = crate::i18n::Lang::from_code(&s.appearance.language);
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(match current {
                            Some(l) => l.native_name().to_string(),
                            None => t!("System ({lang})", lang = crate::i18n::system_language().native_name()),
                        })
                        .show_ui(ui, |ui| {
                            let sys = t!("System ({lang})", lang = crate::i18n::system_language().native_name());
                            ui.selectable_value(&mut s.appearance.language, String::new(), sys);
                            for l in crate::i18n::Lang::ALL {
                                ui.selectable_value(&mut s.appearance.language, l.code().to_string(), l.native_name());
                            }
                        });
                    ui.add_space(12.0);
                    heading(ui, p, t!("Theme"));
                    ui.horizontal(|ui| {
                        for (mode, icon, label) in [
                            (ThemeMode::System, icons::DESKTOP, t!("System")),
                            (ThemeMode::Dark, icons::MOON, t!("Dark")),
                            (ThemeMode::Light, icons::SUN, t!("Light")),
                        ] {
                            let sel = s.appearance.theme == mode;
                            let r = ui.add_sized(
                                [110.0, 64.0],
                                egui::Button::new(RichText::new(format!("{icon}\n{label}")).size(14.0).color(if sel {
                                    p.text
                                } else {
                                    p.text_dim
                                }))
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
                    heading(ui, p, t!("Accent color"));
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
                        if ui.color_edit_button_srgb(&mut rgb).on_hover_text(t!("Custom color")).changed() {
                            s.appearance.custom_accent = rgb;
                            s.appearance.accent = Accent::Custom;
                        }
                    });
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Palette"));
                    let a = &mut s.appearance;
                    ui.horizontal_wrapped(|ui| {
                        for preset in PalettePreset::ALL {
                            let sel = a.preset == preset;
                            let r = ui.add(
                                egui::Button::new(RichText::new(preset.name()).color(if sel {
                                    p.text
                                } else {
                                    p.text_dim
                                }))
                                .fill(if sel { p.accent_soft() } else { p.bg1 })
                                .stroke(egui::Stroke::new(
                                    if sel { 2.0 } else { 1.0 },
                                    if sel { p.accent } else { p.border },
                                ))
                                .corner_radius(egui::CornerRadius::same(8))
                                .min_size(vec2(96.0, 30.0)),
                            );
                            let r = r.on_hover_text(match preset {
                                PalettePreset::HighContrast => {
                                    t!("Maximum legibility: 7:1 text contrast and strong focus rings")
                                }
                                PalettePreset::Custom => t!("Your own colors; edit them below or import a theme file"),
                                _ => t!("Follows the Dark/Light choice above"),
                            });
                            if r.clicked() {
                                if preset == PalettePreset::Custom && a.custom_palette.is_none() {
                                    a.custom_palette = Some(theme::PaletteDef::from_palette(t!("Custom"), p));
                                }
                                a.preset = preset;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    if cfg!(windows) {
                        toggle(
                            ui,
                            p,
                            t!("Follow Windows contrast themes"),
                            t!("Use the high-contrast palette while a Windows contrast theme is on."),
                            &mut a.follow_os_high_contrast,
                        );
                    }
                    if a.preset == PalettePreset::Custom
                        && let Some(def) = &mut a.custom_palette
                    {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(t!("Name"));
                            ui.add(egui::TextEdit::singleline(&mut def.name).desired_width(180.0));
                            ui.checkbox(&mut def.dark, t!("Dark palette"));
                        });
                        ui.add_space(6.0);
                        if let Ok(mut pal) = def.to_palette() {
                            let mut changed = false;
                            egui::Grid::new("palette_editor").num_columns(4).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                                for (i, (field, label)) in theme::PALETTE_FIELDS.iter().enumerate() {
                                    if let Some(c) = pal.color_mut(field) {
                                        let label = crate::i18n::tr(label);
                                        changed |= ui.color_edit_button_srgba(c).labeled(label).changed();
                                        ui.label(RichText::new(label).color(p.text_dim));
                                    }
                                    if i % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                            if changed {
                                let (name, dark) = (def.name.clone(), def.dark);
                                pal.dark = dark;
                                *def = theme::PaletteDef::from_palette(&name, &pal);
                            }
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button(format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Import…"))).clicked() {
                                *pick_extra = Some(2);
                            }
                            if ui.button(format!("{}  {}", icons::UPLOAD_SIMPLE, t!("Export…"))).clicked() {
                                *pick_extra = Some(3);
                            }
                            if ui
                                .button(format!("{}  {}", icons::ARROW_COUNTER_CLOCKWISE, t!("Reset to Corrent")))
                                .clicked()
                            {
                                let base = theme::Palette::preset(PalettePreset::Corrent, p.dark, p.accent);
                                *def = theme::PaletteDef::from_palette(&def.name, &base);
                            }
                        });
                    }
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Layout"));
                    toggle(
                        ui,
                        p,
                        t!("Compact rows"),
                        t!("Single-line rows to fit more torrents."),
                        &mut s.appearance.compact_rows,
                    );
                    row(ui, p, t!("Interface scale"), "", |ui| {
                        ui.add(
                            egui::Slider::new(&mut s.appearance.zoom, 0.75..=2.0)
                                .step_by(0.05)
                                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                        );
                    });
                    toggle(ui, p, t!("Color progress bars by state"), "", &mut s.appearance.state_colored_progress);
                    toggle(
                        ui,
                        p,
                        t!("Striped rows"),
                        t!("Alternate row shading in the torrent list."),
                        &mut s.appearance.striped_rows,
                    );
                    toggle(ui, p, t!("Speed graphs in the status bar"), "", &mut s.appearance.show_sparklines);
                    toggle(ui, p, t!("Animations"), "", &mut s.appearance.animations);
                    row(ui, p, t!("Speed units"), "", |ui| {
                        egui::ComboBox::from_id_salt("units")
                            .selected_text(match s.appearance.speed_units {
                                SpeedUnits::Binary => t!("KiB/s, MiB/s"),
                                SpeedUnits::Decimal => t!("kB/s, MB/s"),
                                SpeedUnits::Bits => t!("kbit/s, Mbit/s"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut s.appearance.speed_units,
                                    SpeedUnits::Binary,
                                    t!("KiB/s, MiB/s"),
                                );
                                ui.selectable_value(
                                    &mut s.appearance.speed_units,
                                    SpeedUnits::Decimal,
                                    t!("kB/s, MB/s"),
                                );
                                ui.selectable_value(
                                    &mut s.appearance.speed_units,
                                    SpeedUnits::Bits,
                                    t!("kbit/s, Mbit/s"),
                                );
                            });
                    });
                });
            }
            Page::Metadata => self.metadata_settings(ui, p),
            Page::Remote => {
                let status = self.remote.status.clone();
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("qBittorrent-compatible Web API"));
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                t!("Lets Sonarr, Radarr, Lidarr and similar tools use Corrent as their download client: add it there as a “qBittorrent” client with the address, port, username and password below."),
                            )
                            .small()
                            .color(p.text_dim),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);
                    toggle(ui, p, t!("Enabled"), "", &mut s.web_api.enabled);
                    row(ui, p, t!("Port"), "", |ui| {
                        ui.add(egui::DragValue::new(&mut s.web_api.port).range(1024..=65535));
                    });
                    toggle(
                        ui,
                        p,
                        t!("Allow other devices on the network"), t!("Off: only programs on this computer can connect (recommended)."),
                        &mut s.web_api.allow_lan,
                    );
                    row(ui, p, t!("Username"), "", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.web_api.username).desired_width(200.0));
                    });
                    let has = !s.web_api.password_hash.is_empty();
                    row(ui, p, t!("Password"), if has { t!("Leave empty to keep the current password.") } else { t!("Required.") }, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut d.new_password)
                                .password(true)
                                .desired_width(200.0)
                                .hint_text(if has { "••••••••" } else { t!("Choose a password") }),
                        );
                    });
                    if s.web_api.allow_lan && !d.new_password.is_empty() && d.new_password.len() < 8 {
                        ui.label(
                            RichText::new(t!("Use at least 8 characters when other devices can connect.")).small().color(p.warn),
                        );
                    }
                    ui.add_space(4.0);
                    let (icon, color, text) = match &status {
                        super::remote::RemoteStatus::Off => (icons::CIRCLE, p.text_mute, t!("Off").to_string()),
                        super::remote::RemoteStatus::Starting => (icons::CIRCLE_NOTCH, p.accent, t!("Starting…").to_string()),
                        super::remote::RemoteStatus::Running(a) => {
                            let host = if a.ip().is_unspecified() { t!("this computer's address").to_string() } else { a.ip().to_string() };
                            (icons::CHECK_CIRCLE, p.success, t!("Running — host {host}, port {port}", host = host, port = a.port()))
                        }
                        super::remote::RemoteStatus::Failed(e) => (icons::WARNING_CIRCLE, p.error, e.clone()),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon).color(color));
                        ui.label(RichText::new(text).color(color));
                    });
                    if let super::remote::RemoteStatus::Running(a) = status {
                        ui.add_space(4.0);
                        let url = format!("http://127.0.0.1:{}/", a.port());
                        if ui
                            .button(format!("{}  {}", icons::BROWSER, t!("Open the web interface")))
                            .on_hover_text(t!("Control Corrent from a browser (sign in with the username and password above)."))
                            .clicked()
                        {
                            let _ = open::that_detached(&url);
                        }
                    }
                });
            }
            Page::Search => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Torznab providers"));
                    ui.add(
                        egui::Label::new(
                            RichText::new(t!(
                                "Corrent searches only providers you add here, such as a Jackett or Prowlarr you run. \
                                 Jackett: copy an indexer's “Torznab Feed” address and the API key from its dashboard."
                            ))
                            .small()
                            .color(p.text_dim),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);
                    let mut remove = None;
                    for (i, pr) in s.search.providers.iter_mut().enumerate() {
                        egui::Frame::new()
                            .fill(p.bg1)
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                egui::Grid::new(("provider", i)).num_columns(2).spacing(vec2(10.0, 6.0)).show(
                                    ui,
                                    |ui| {
                                        ui.label(t!("Name"));
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::TextEdit::singleline(&mut pr.name)
                                                    .desired_width(220.0)
                                                    .hint_text(t!("e.g. Jackett")),
                                            );
                                            toggle_switch(ui, p, &mut pr.enabled).on_hover_text(t!("Enabled"));
                                            if ui
                                                .button(RichText::new(icons::TRASH).color(p.error))
                                                .labeled(t!("Remove"))
                                                .clicked()
                                            {
                                                remove = Some(i);
                                            }
                                        });
                                        ui.end_row();
                                        ui.label(t!("Address"));
                                        ui.add(
                                            egui::TextEdit::singleline(&mut pr.url).desired_width(420.0).hint_text(
                                                "http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab",
                                            ),
                                        );
                                        ui.end_row();
                                        ui.label(t!("API key"));
                                        ui.add(
                                            egui::TextEdit::singleline(&mut pr.api_key)
                                                .desired_width(420.0)
                                                .password(true),
                                        );
                                        ui.end_row();
                                    },
                                );
                                if !pr.url.trim().is_empty() && crate::search::search_url(pr, "x").is_err() {
                                    ui.label(
                                        RichText::new(t!("The address must be an http:// or https:// URL"))
                                            .small()
                                            .color(p.warn),
                                    );
                                }
                            });
                        ui.add_space(6.0);
                    }
                    if let Some(i) = remove {
                        s.search.providers.remove(i);
                    }
                    if ui.button(format!("{}  {}", icons::PLUS, t!("Add provider"))).clicked() {
                        s.search.providers.push(Default::default());
                    }
                });
            }
            Page::Feeds => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("RSS feeds"));
                    ui.label(
                        RichText::new(t!("Add feeds and download rules from “RSS feeds” in the sidebar."))
                            .small()
                            .color(p.text_dim),
                    );
                    ui.add_space(6.0);
                    toggle(
                        ui,
                        p,
                        t!("Download automatically"),
                        t!("Add new articles that match a download rule."),
                        &mut s.rss.auto_download,
                    );
                    row(ui, p, t!("Refresh feeds every"), t!("At least 5 minutes, to be kind to the servers."), |ui| {
                        ui.add(egui::DragValue::new(&mut s.rss.refresh_minutes).range(5..=1440).suffix(" min"));
                    });
                    row(
                        ui,
                        p,
                        t!("Articles kept per feed"),
                        t!("Older ones are forgotten (their download status is kept)."),
                        |ui| {
                            ui.add(egui::DragValue::new(&mut s.rss.max_articles).range(10..=5000));
                        },
                    );
                });
            }
            Page::Playback => {
                if self.media.audio_devices.is_none() {
                    self.media.audio_devices = Some(super::media::client::fetch(&self.rt, &self.ctx, async {
                        tokio::task::spawn_blocking(audio_devices).await.map_err(|e| e.to_string())?
                    }));
                }
                let devices = self.media.audio_devices.as_ref().and_then(super::media::client::ready);
                let pb = &mut s.playback;
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Sound output"));
                    row(ui, p, t!("Output device"), t!("Where the media player sends sound."), |ui| {
                        let list = devices.as_ref().and_then(|d| d.as_ref().ok()).cloned().unwrap_or_default();
                        let shown =
                            list.iter().find(|(n, _)| *n == pb.audio_device).map(|(_, d)| d.clone()).unwrap_or_else(
                                || {
                                    if pb.audio_device == "auto" {
                                        t!("System default").to_string()
                                    } else {
                                        pb.audio_device.clone()
                                    }
                                },
                            );
                        egui::ComboBox::from_id_salt("audio_device").width(250.0).selected_text(shown).show_ui(
                            ui,
                            |ui| {
                                ui.selectable_value(&mut pb.audio_device, "auto".to_string(), t!("System default"));
                                for (name, desc) in &list {
                                    if name != "auto" {
                                        ui.selectable_value(&mut pb.audio_device, name.clone(), desc);
                                    }
                                }
                                if devices.is_none() {
                                    ui.spinner();
                                }
                            },
                        );
                    });
                    toggle(
                        ui,
                        p,
                        t!("Exclusive mode"),
                        t!("Takes the device for the player alone: bit-perfect sound, but other apps go quiet."),
                        &mut pb.exclusive,
                    );
                    row(ui, p, t!("Speakers"), t!("What the player mixes for."), |ui| {
                        let opts =
                            [("auto-safe", t!("Automatic")), ("stereo", t!("Stereo")), ("5.1", "5.1"), ("7.1", "7.1")];
                        let shown =
                            opts.iter().find(|(v, _)| *v == pb.channels).map(|(_, l)| *l).unwrap_or(t!("Automatic"));
                        egui::ComboBox::from_id_salt("audio_channels").selected_text(shown).show_ui(ui, |ui| {
                            for (v, l) in opts {
                                ui.selectable_value(&mut pb.channels, v.to_string(), l);
                            }
                        });
                    });
                    toggle(
                        ui,
                        p,
                        t!("Pass Dolby and DTS to the receiver"),
                        t!("For an AV receiver over HDMI or S/PDIF that decodes them itself."),
                        &mut pb.passthrough,
                    );
                    ui.add_space(8.0);
                    heading(ui, p, t!("Sound processing"));
                    toggle(
                        ui,
                        p,
                        t!("Even out loudness"),
                        t!("Quiet and loud films and tracks play at a similar level."),
                        &mut pb.normalize,
                    );
                    row(ui, p, t!("Equalizer"), "", |ui| {
                        let label = |e: corrent_core::config::EqPreset| match e {
                            corrent_core::config::EqPreset::Flat => t!("Off"),
                            corrent_core::config::EqPreset::BassBoost => t!("More bass"),
                            corrent_core::config::EqPreset::Treble => t!("More treble"),
                            corrent_core::config::EqPreset::Vocal => t!("Voices"),
                            corrent_core::config::EqPreset::Loudness => t!("Loudness"),
                        };
                        egui::ComboBox::from_id_salt("eq").selected_text(label(pb.equalizer)).show_ui(ui, |ui| {
                            for e in corrent_core::config::EqPreset::ALL {
                                ui.selectable_value(&mut pb.equalizer, e, label(e));
                            }
                        });
                    });
                    row(ui, p, t!("ReplayGain"), t!("Uses the volume levels stored in music files."), |ui| {
                        let opts = [("album", t!("By album")), ("track", t!("By track")), ("off", t!("Off"))];
                        let shown =
                            opts.iter().find(|(v, _)| *v == pb.replaygain).map(|(_, l)| *l).unwrap_or(t!("By album"));
                        egui::ComboBox::from_id_salt("replaygain").selected_text(shown).show_ui(ui, |ui| {
                            for (v, l) in opts {
                                ui.selectable_value(&mut pb.replaygain, v.to_string(), l);
                            }
                        });
                    });
                    toggle(ui, p, t!("Gapless albums"), t!("No pause between tracks of an album."), &mut pb.gapless);
                });
            }
            Page::Advanced => {
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Engine"));
                    row(
                        ui,
                        p,
                        t!("Simultaneous data checks"),
                        t!("How many torrents verify data at the same time."),
                        |ui| {
                            ui.add(egui::DragValue::new(&mut s.advanced.concurrent_init_limit).range(1..=32));
                        },
                    );
                    row(
                        ui,
                        p,
                        t!("Refresh interval"),
                        t!("How often transfer statistics update (takes effect after restart)."),
                        |ui| {
                            ui.add(
                                egui::DragValue::new(&mut s.advanced.refresh_interval_ms)
                                    .range(200..=5000)
                                    .speed(10.0)
                                    .suffix(" ms"),
                            );
                        },
                    );
                });
                ui.add_space(10.0);
                card(p).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    heading(ui, p, t!("Data"));
                    let portable = if self.paths.portable { t!(" (portable mode)") } else { "" };
                    ui.label(RichText::new(format!("{}{portable}", self.paths.data_dir.display())).monospace().color(p.text_dim));
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  {}", icons::FOLDER_OPEN, t!("Open data folder"))).clicked() {
                            super::open_path(&self.paths.data_dir);
                        }
                        if ui.button(format!("{}  {}", icons::SCROLL, t!("Open log folder"))).clicked() {
                            super::open_path(&self.paths.log_dir());
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Back up data…"))).clicked() {
                            *pick_extra = Some(0);
                        }
                        if ui.button(format!("{}  {}", icons::UPLOAD_SIMPLE, t!("Restore from backup…"))).clicked() {
                            *pick_extra = Some(1);
                        }
                    });
                    ui.label(RichText::new(t!("Backups contain settings, categories/tags and the list of torrents (not the downloaded files).")).small().color(p.text_dim));
                    ui.add_space(8.0);
                    if ui.button(RichText::new(format!("{}  {}", icons::ARROW_COUNTER_CLOCKWISE, t!("Reset all settings to defaults"))).color(p.warn)).clicked() {
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
        let st = crate::util::with_lock(&self.handler_status, |g| g.clone());
        let pill = |ui: &mut Ui, label: &str, state: Option<&HandlerState>| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, color) = match state {
                        Some(HandlerState::Ours) => (t!("Corrent").to_string(), p.success),
                        Some(HandlerState::OtherDefault(a)) => (a.clone(), p.warn),
                        Some(HandlerState::OtherPath) => (t!("Corrent (moved)").into(), p.warn),
                        Some(HandlerState::NotRegistered) => (t!("Not set").into(), p.text_dim),
                        Some(HandlerState::Managed) => (t!("Managed by package").into(), p.text_dim),
                        Some(HandlerState::Unsupported) => (t!("Unsupported").into(), p.text_dim),
                        None => (t!("Checking…").into(), p.text_dim),
                    };
                    crate::ui::widgets::badge(ui, &text, color);
                });
            });
        };
        pill(ui, t!("magnet: links"), st.as_ref().map(|s| &s.magnet));
        pill(ui, t!(".torrent files"), st.as_ref().map(|s| &s.torrent));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let ours = st.as_ref().is_some_and(|s| s.magnet == HandlerState::Ours && s.torrent == HandlerState::Ours);
            if !ours && primary_button(ui, p, t!("Make Corrent the default")).clicked() {
                self.set_handlers(true, true);
            }
            if (self.settings.behavior.register_magnet_handler || self.settings.behavior.register_torrent_handler)
                && secondary_button(ui, t!("Unregister")).clicked()
            {
                self.set_handlers(false, false);
            }
            if cfg!(windows) && secondary_button(ui, t!("System settings…")).clicked() {
                os_handler::open_system_default_settings();
            }
            if ui.button(icons::ARROWS_CLOCKWISE).labeled(t!("Refresh")).clicked() {
                self.refresh_handler_status(false);
            }
        });
        if ui
            .checkbox(
                &mut self.settings.behavior.keep_handler_registration,
                RichText::new(t!("Keep the registration up to date if Corrent is moved")).small(),
            )
            .changed()
        {
            self.save_settings();
        }
    }
}

/// The sound devices mpv can play to: (name, description). Blocking (starts a headless mpv).
fn audio_devices() -> Result<Vec<(String, String)>, String> {
    let lib = corrent_mpv::Library::get().map_err(|e| e.to_string())?;
    let mpv = corrent_mpv::Mpv::new(lib, corrent_mpv::HEADLESS).map_err(|e| e.to_string())?;
    let list = mpv.get("audio-device-list").map_err(|e| e.to_string())?;
    Ok(list
        .as_array()
        .iter()
        .filter_map(|d| {
            let name = d.get("name")?.as_str()?.to_string();
            let desc = d.get("description").and_then(|x| x.as_str()).unwrap_or(&name).to_string();
            Some((name, desc))
        })
        .collect())
}
