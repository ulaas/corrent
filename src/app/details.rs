use std::time::Duration;

use egui::{Align, Layout, RichText, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;

use super::{CorrentApp, DetailsTab};
use crate::engine::EngineCommand;
use crate::engine::types::{TorrentDetail, TorrentView};
use crate::ui::file_tree;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, card};
use crate::util::{fmt_bytes, fmt_date, fmt_duration, fmt_ratio, fmt_speed};

const TABS: [(DetailsTab, &str, &str); 6] = [
    (DetailsTab::General, icons::INFO, "General"),
    (DetailsTab::Files, icons::FILES, "Files"),
    (DetailsTab::Peers, icons::USERS_THREE, "Peers"),
    (DetailsTab::Trackers, icons::BROADCAST, "Trackers"),
    (DetailsTab::Pieces, icons::SQUARES_FOUR, "Pieces"),
    (DetailsTab::Speed, icons::CHART_LINE, "Speed"),
];

impl CorrentApp {
    /// The torrent shown in the details panel: the focused one if selected, else the only selection.
    pub fn detail_hash(&self) -> Option<crate::engine::types::InfoHash> {
        if let Some(f) = self.focus
            && self.selected.contains(&f)
        {
            return Some(f);
        }
        (self.selected.len() == 1).then(|| *self.selected.iter().next().unwrap())
    }

    pub(super) fn sync_detail_target(&mut self) {
        let target = if self.prefs.show_details { self.detail_hash() } else { None };
        if target != self.sent_detail_target {
            self.sent_detail_target = target;
            self.engine.send(EngineCommand::SetDetailTarget(target));
        }
    }

    pub(super) fn details_panel(&mut self, ui: &mut Ui, p: &Palette) {
        // Tab strip
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (tab, icon, label) in TABS {
                let sel = self.prefs.details_tab == tab;
                let text = RichText::new(format!("{icon}  {label}")).color(if sel { p.text } else { p.text_dim });
                let r = ui.add(egui::Button::new(text).frame(false).min_size(vec2(0.0, 28.0)));
                if sel {
                    let rect = r.rect;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 4.0, rect.bottom() - 2.0),
                            egui::pos2(rect.right() - 4.0, rect.bottom()),
                        ),
                        egui::CornerRadius::same(1),
                        p.accent,
                    );
                }
                if r.clicked() {
                    self.prefs.details_tab = tab;
                }
                ui.add_space(6.0);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new(icons::CARET_DOWN).color(p.text_dim)).frame(false))
                    .on_hover_text("Hide details (Ctrl+I)")
                    .clicked()
                {
                    self.prefs.show_details = false;
                }
            });
        });
        ui.add_space(4.0);

        let Some(hash) = self.detail_hash() else {
            if self.selected.len() > 1 {
                let n = self.selected.len();
                let total: u64 = self.selected_views().iter().map(|t| t.size).sum();
                widgets::empty_compact(
                    ui,
                    p,
                    icons::STACK,
                    &format!("{n} torrents selected"),
                    &format!("{} in total", fmt_bytes(total)),
                );
            } else {
                widgets::empty_compact(
                    ui,
                    p,
                    icons::CURSOR_CLICK,
                    "Nothing selected",
                    "Select a torrent to see its details.",
                );
            }
            return;
        };
        let snap = self.snap.clone();
        let Some(t) = snap.torrents.iter().find(|t| t.hash == hash) else { return };
        let detail = snap.detail.as_ref().filter(|d| d.hash == hash);

        match self.prefs.details_tab {
            DetailsTab::General => self.tab_general(ui, p, t, detail),
            DetailsTab::Files => self.tab_files(ui, p, t, detail),
            DetailsTab::Peers => self.tab_peers(ui, p, detail),
            DetailsTab::Trackers => self.tab_trackers(ui, p, t),
            DetailsTab::Pieces => self.tab_pieces(ui, p, t, detail),
            DetailsTab::Speed => self.tab_speed(ui, p, detail),
        }
    }

    fn tab_general(&mut self, ui: &mut Ui, p: &Palette, t: &TorrentView, detail: Option<&TorrentDetail>) {
        let units = self.settings.appearance.speed_units;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // Progress + piece strip
            ui.horizontal(|ui| {
                let color = p.state_color(t.state);
                ui.label(
                    RichText::new(format!("{:.1}%", t.progress * 100.0))
                        .font(crate::ui::theme::bold(15.0))
                        .color(color),
                );
                let w = ui.available_width();
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    widgets::progress_bar(
                        ui,
                        egui::Id::new(("dpb", t.hash)),
                        t.progress,
                        color,
                        p.bg3,
                        6.0,
                        w,
                        false,
                        true,
                    );
                    if let Some((bits, n)) = detail.and_then(|d| d.pieces.as_ref()) {
                        ui.add_space(4.0);
                        widgets::piece_bar(ui, p, bits, *n, 8.0);
                    }
                });
            });
            ui.add_space(8.0);
            let cols = 3;
            let kv: Vec<(&str, String)> = vec![
                ("Status", super::table::status_text(t)),
                ("Size", if t.size > 0 { fmt_bytes(t.size) } else { "—".into() }),
                ("Done", fmt_bytes(t.done)),
                ("Download speed", fmt_speed(t.down_bps, units)),
                ("Upload speed", fmt_speed(t.up_bps, units)),
                ("ETA", t.eta.map(fmt_duration).unwrap_or_else(|| "—".into())),
                ("Downloaded", fmt_bytes(t.downloaded)),
                ("Uploaded", fmt_bytes(t.uploaded)),
                ("Ratio", fmt_ratio(t.ratio)),
                (
                    "Peers",
                    format!("{} connected · {} known · {} connecting", t.peers_live, t.peers_seen, t.peers_connecting),
                ),
                (
                    "Seeding time",
                    if t.seeding_secs > 0 { fmt_duration(Duration::from_secs(t.seeding_secs)) } else { "—".into() },
                ),
                ("Files", if t.file_count > 0 { t.file_count.to_string() } else { "—".into() }),
                (
                    "Pieces",
                    if t.piece_count > 0 {
                        format!("{} × {}", t.piece_count, fmt_bytes(t.piece_size))
                    } else {
                        "—".into()
                    },
                ),
                ("Added", fmt_date(t.added_at)),
                ("Completed", t.completed_at.map(fmt_date).unwrap_or_else(|| "—".into())),
                ("Created", t.creation_date.map(fmt_date).unwrap_or_else(|| "—".into())),
            ];
            egui::Grid::new("general_grid").num_columns(cols * 2).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                for (i, (k, v)) in kv.iter().enumerate() {
                    ui.label(RichText::new(*k).small().color(p.text_dim));
                    ui.label(RichText::new(v).color(p.text));
                    if i % cols == cols - 1 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(10.0);
            card(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("general_paths").num_columns(2).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                    ui.label(RichText::new("Save path").small().color(p.text_dim));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t.save_path.display().to_string()).color(p.text));
                        if ui.small_button(icons::FOLDER_OPEN).on_hover_text("Open folder").clicked() {
                            super::open_path(&t.save_path);
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Info hash").small().color(p.text_dim));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t.hash.to_hex()).monospace().color(p.text));
                        if ui.small_button(icons::COPY).on_hover_text("Copy").clicked() {
                            self.copy(t.hash.to_hex());
                        }
                    });
                    ui.end_row();
                    if let Some(c) = &t.created_by {
                        ui.label(RichText::new("Created by").small().color(p.text_dim));
                        ui.label(RichText::new(c).color(p.text));
                        ui.end_row();
                    }
                    if let Some(c) = &t.comment {
                        ui.label(RichText::new("Comment").small().color(p.text_dim));
                        ui.add(egui::Label::new(RichText::new(c).color(p.text)).wrap());
                        ui.end_row();
                    }
                    if let Some(e) = &t.error {
                        ui.label(RichText::new("Error").small().color(p.error));
                        ui.add(egui::Label::new(RichText::new(e).color(p.error)).wrap());
                        ui.end_row();
                    }
                });
            });
        });
    }

    fn tab_files(&mut self, ui: &mut Ui, p: &Palette, t: &TorrentView, detail: Option<&TorrentDetail>) {
        let Some(d) = detail else {
            loading(ui, p, t.has_metadata);
            return;
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.file_filter)
                    .hint_text(format!("{}  Filter files", icons::FUNNEL))
                    .desired_width(220.0),
            );
            let n_sel = d.files.iter().filter(|f| f.included).count();
            ui.label(RichText::new(format!("{n_sel} of {} files selected", d.files.len())).small().color(p.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Select all").clicked() {
                    self.engine.send(EngineCommand::SetFiles {
                        hash: t.hash,
                        included: d.files.iter().map(|f| f.index).collect(),
                    });
                }
            });
        });
        ui.add_space(4.0);
        let entries: Vec<file_tree::Entry<'_>> = d
            .files
            .iter()
            .map(|f| file_tree::Entry { index: f.index, components: &f.components, len: f.len, done: Some(f.done) })
            .collect();
        let included: std::collections::HashSet<usize> =
            d.files.iter().filter(|f| f.included).map(|f| f.index).collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let filter = self.file_filter.clone();
            if let Some(ch) = file_tree::show(
                ui,
                p,
                egui::Id::new(("files", t.hash)),
                &entries,
                &|i| included.contains(&i),
                &filter,
                true,
            ) {
                let mut next = included.clone();
                for (i, on) in ch.set {
                    if on {
                        next.insert(i);
                    } else {
                        next.remove(&i);
                    }
                }
                let mut v: Vec<usize> = next.into_iter().collect();
                v.sort_unstable();
                self.engine.send(EngineCommand::SetFiles { hash: t.hash, included: v });
            }
        });
    }

    fn tab_peers(&mut self, ui: &mut Ui, p: &Palette, detail: Option<&TorrentDetail>) {
        let units = self.settings.appearance.speed_units;
        let Some(d) = detail else {
            loading(ui, p, true);
            return;
        };
        let mut peers: Vec<_> = d.peers.iter().filter(|x| x.state == "live" || self.show_all_peers).collect();
        peers.sort_by(|a, b| {
            (b.down_bps + b.up_bps).cmp(&(a.down_bps + a.up_bps)).then_with(|| b.downloaded.cmp(&a.downloaded))
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} peers", peers.len())).small().color(p.text_dim));
            ui.checkbox(&mut self.show_all_peers, RichText::new("Show connecting and dead peers").small());
        });
        if peers.is_empty() {
            let hint = if self.selected_views().first().is_some_and(|t| t.state.is_paused_like()) {
                "Resume the torrent to connect to peers."
            } else {
                "Peers are found through trackers, DHT and peer exchange; this can take a minute."
            };
            widgets::empty_compact(ui, p, icons::USERS_THREE, "No connected peers", hint);
            return;
        }
        TableBuilder::new(ui)
            .id_salt("peers")
            .striped(true)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .auto_shrink([false, false])
            .column(Column::initial(230.0).at_least(120.0).clip(true))
            .column(Column::initial(150.0).clip(true))
            .column(Column::initial(70.0))
            .column(Column::initial(80.0))
            .columns(Column::initial(90.0), 4)
            .header(22.0, |mut h| {
                for title in ["Address", "Client", "Type", "State", "Down", "Up", "Downloaded", "Uploaded"] {
                    h.col(|ui| {
                        ui.label(RichText::new(title).small().strong().color(p.text_dim));
                    });
                }
            })
            .body(|body| {
                body.rows(22.0, peers.len(), |mut row| {
                    let peer = peers[row.index()];
                    row.col(|ui| {
                        ui.label(RichText::new(&peer.addr).monospace().color(p.text));
                    });
                    row.col(|ui| {
                        ui.label(peer.client.clone().unwrap_or_else(|| "—".into()));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&peer.kind).small().color(p.text_dim));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&peer.state).small().color(p.text_dim));
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(fmt_speed(peer.down_bps, units)).monospace().color(if peer.down_bps > 0 {
                                p.accent
                            } else {
                                p.text_mute
                            }),
                        );
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(fmt_speed(peer.up_bps, units)).monospace().color(if peer.up_bps > 0 {
                            p.success
                        } else {
                            p.text_mute
                        }));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(fmt_bytes(peer.downloaded)).monospace());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(fmt_bytes(peer.uploaded)).monospace());
                    });
                });
            });
    }

    fn tab_trackers(&mut self, ui: &mut Ui, p: &Palette, t: &TorrentView) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // ---- where peers come from
            ui.label(RichText::new("PEER SOURCES").size(10.5).strong().color(p.text_mute));
            ui.add_space(4.0);
            let s = &self.snap.session;
            let dht = match (self.settings.bittorrent.dht, s.dht_nodes) {
                (false, _) => ("Off".to_string(), p.text_mute),
                (true, Some(n)) if n > 0 => (format!("{n} nodes"), p.success),
                (true, _) => ("Connecting…".to_string(), p.warn),
            };
            let lsd = if self.settings.bittorrent.lsd { ("On".to_string(), p.success) } else { ("Off".to_string(), p.text_mute) };
            let trk = if !self.settings.bittorrent.trackers_enabled {
                ("Disabled".to_string(), p.text_mute)
            } else {
                (format!("{} tracker(s)", t.trackers.len()), if t.trackers.is_empty() { p.text_mute } else { p.success })
            };
            let sources = [
                (icons::GRAPH, "DHT", "Decentralized network", dht),
                (icons::ARROWS_LEFT_RIGHT, "PEX", "Peers share peers", ("On".to_string(), p.success)),
                (icons::WIFI_HIGH, "Local", "Same-network peers", lsd),
                (icons::BROADCAST, "Trackers", "Announce servers", trk),
            ];
            let w = ((ui.available_width() - 3.0 * 10.0) / 4.0).max(140.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for (icon, title, sub, (status, color)) in sources {
                    card(p).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                        ui.set_width(w - 22.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).size(18.0).color(color));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new(title).strong().color(p.text));
                                ui.label(RichText::new(sub).small().color(p.text_dim));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(status).small().color(color));
                            });
                        });
                    });
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "{} connected · {} connecting · {} known peers",
                    t.peers_live, t.peers_connecting, t.peers_seen
                ))
                .small()
                .color(p.text_dim),
            );

            // ---- trackers
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("TRACKERS  ·  {}", t.trackers.len())).size(10.5).strong().color(p.text_mute));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(format!("{}  Add trackers…", icons::PLUS)).clicked() {
                        self.open_trackers_dialog(t.hash);
                    }
                    if !t.trackers.is_empty() && ui.button(format!("{}  Copy all", icons::COPY)).clicked() {
                        self.copy(t.trackers.join("\n"));
                    }
                });
            });
            ui.add_space(4.0);
            if t.trackers.is_empty() {
                ui.label(RichText::new("No trackers — peers are found through DHT and peer exchange.").color(p.text_mute));
            }
            let mut trackers = t.trackers.clone();
            trackers.sort_by_key(|u| super::table::tracker_host(u));
            for url in &trackers {
                let parsed = url::Url::parse(url).ok();
                let scheme = parsed.as_ref().map(|u| u.scheme().to_ascii_uppercase()).unwrap_or_else(|| "?".into());
                let host = super::table::tracker_host(url);
                let port = parsed.as_ref().and_then(|u| u.port_or_known_default()).map(|p| format!(":{p}")).unwrap_or_default();
                let path = parsed.as_ref().map(|u| u.path().to_string()).unwrap_or_default();
                let color = match scheme.as_str() {
                    "UDP" => p.accent,
                    "HTTPS" => p.success,
                    "HTTP" => p.warn,
                    "WSS" | "WS" => p.queued,
                    _ => p.text_dim,
                };
                egui::Frame::new()
                    .fill(p.bg2)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            ui.allocate_ui_with_layout(vec2(56.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
                                widgets::badge(ui, &scheme, color);
                            });
                            ui.label(RichText::new(&host).strong().color(p.text));
                            ui.label(RichText::new(format!("{port}{path}")).monospace().small().color(p.text_dim));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button(icons::COPY).on_hover_text("Copy URL").clicked() {
                                    self.copy(url.clone());
                                }
                            });
                        })
                        .response
                        .on_hover_text(url);
                    });
                ui.add_space(4.0);
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("The engine doesn't report per-tracker announce results; connected peers above reflect all sources.")
                    .small()
                    .color(p.text_mute),
            );
        });
    }

    fn tab_pieces(&mut self, ui: &mut Ui, p: &Palette, t: &TorrentView, detail: Option<&TorrentDetail>) {
        let Some((bits, n)) = detail.and_then(|d| d.pieces.as_ref()) else {
            loading(ui, p, t.has_metadata);
            return;
        };
        let have: u32 = bits.iter().map(|b| b.count_ones()).sum();
        ui.label(
            RichText::new(format!("{have} of {n} pieces · {} each", fmt_bytes(t.piece_size))).small().color(p.text_dim),
        );
        ui.add_space(6.0);
        // A grid of cells, one per bucket of pieces.
        let avail = ui.available_size();
        let cell = 10.0f32;
        let gap = 2.0f32;
        let cols = ((avail.x + gap) / (cell + gap)).floor().max(1.0) as usize;
        let rows = ((avail.y + gap) / (cell + gap)).floor().max(1.0) as usize;
        let buckets = (cols * rows).min(*n as usize).max(1);
        let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let have_bit = |i: u32| bits.get((i / 8) as usize).is_some_and(|b| b & (0x80 >> (i % 8)) != 0);
        let mut hovered = None;
        for b in 0..buckets {
            let a = (b as u64 * *n as u64 / buckets as u64) as u32;
            let z = (((b + 1) as u64 * *n as u64 / buckets as u64) as u32).max(a + 1).min(*n);
            let got = (a..z).filter(|&i| have_bit(i)).count() as f32 / (z - a) as f32;
            let (cx, cy) = (b % cols, b / cols);
            let r = egui::Rect::from_min_size(
                rect.min + vec2(cx as f32 * (cell + gap), cy as f32 * (cell + gap)),
                vec2(cell, cell),
            );
            let color = if got >= 1.0 {
                p.accent
            } else if got > 0.0 {
                p.accent.gamma_multiply(0.25 + 0.6 * got)
            } else {
                p.bg3
            };
            painter.rect_filled(r, egui::CornerRadius::same(2), color);
            if resp.hover_pos().is_some_and(|h| r.contains(h)) {
                hovered = Some((a, z, got));
            }
        }
        if let Some((a, z, got)) = hovered {
            resp.on_hover_text(format!("Pieces {}–{} · {:.0}% downloaded", a, z - 1, got * 100.0));
        }
    }

    fn tab_speed(&mut self, ui: &mut Ui, p: &Palette, detail: Option<&TorrentDetail>) {
        let units = self.settings.appearance.speed_units;
        let interval = self.settings.advanced.refresh_interval_ms.max(200) as f64 / 1000.0;
        let (hist, title) = match detail {
            Some(d) if self.speed_scope_torrent => (d.history.clone(), "This torrent"),
            _ => (self.snap.history.clone(), "All torrents"),
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.speed_scope_torrent, true, "This torrent");
            ui.selectable_value(&mut self.speed_scope_torrent, false, "All torrents");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some((d, u)) = hist.back() {
                    ui.label(RichText::new(format!("↑ {}", fmt_speed(*u, units))).monospace().color(p.success));
                    ui.label(RichText::new(format!("↓ {}", fmt_speed(*d, units))).monospace().color(p.accent));
                }
            });
        });
        let n = hist.len();
        if n < 3 {
            widgets::empty_compact(ui, p, icons::CHART_LINE, "Collecting speed data…", "");
            return;
        }
        let down: Vec<[f64; 2]> =
            hist.iter().enumerate().map(|(i, (d, _))| [(i as f64 - n as f64) * interval, *d as f64]).collect();
        let up: Vec<[f64; 2]> =
            hist.iter().enumerate().map(|(i, (_, u))| [(i as f64 - n as f64) * interval, *u as f64]).collect();
        let _ = title;
        egui_plot::Plot::new("speed_plot")
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .show_background(false)
            .include_y(0.0)
            .include_y(1024.0)
            .show_axes([true, true])
            .show_grid([false, true])
            .x_axis_formatter(|m, _| {
                let s = -m.value;
                if s < 0.5 { "now".into() } else { format!("-{}", fmt_duration(Duration::from_secs_f64(s))) }
            })
            .y_axis_formatter(move |m, _| if m.value <= 0.0 { String::new() } else { fmt_speed(m.value as u64, units) })
            .label_formatter(move |hp| match hp {
                egui_plot::HoverPosition::NearDataPoint { plot_name, position, .. } => {
                    Some(format!("{plot_name}: {}", fmt_speed(position.y.max(0.0) as u64, units)))
                }
                _ => None,
            })
            .show(ui, |pui| {
                pui.line(egui_plot::Line::new("Download", down).color(p.accent).width(1.8).fill(0.0).fill_alpha(0.18));
                pui.line(egui_plot::Line::new("Upload", up).color(p.success).width(1.8).fill(0.0).fill_alpha(0.12));
            });
    }
}

fn loading(ui: &mut Ui, p: &Palette, has_metadata: bool) {
    if has_metadata {
        widgets::empty_compact(ui, p, icons::HOURGLASS_MEDIUM, "Loading…", "");
    } else {
        widgets::empty_compact(
            ui,
            p,
            icons::MAGNET,
            "Waiting for metadata",
            "File and piece information appears once peers send the torrent's metadata.",
        );
    }
}
