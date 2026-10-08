//! Streaming window: media players reading files through the local streaming server.

use egui::{Align, Context, CornerRadius, Id, Layout, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::EngineCommand;
use crate::engine::types::{InfoHash, StreamState, StreamView};
use crate::ui::theme::Palette;
use crate::ui::widgets::{badge, card, empty_compact};
use crate::util::{fmt_bytes, fmt_duration, fmt_speed};

pub fn state_label(s: StreamState) -> &'static str {
    match s {
        StreamState::Playing => t!("Playing"),
        StreamState::Buffering => t!("Buffering"),
        StreamState::Paused => t!("Paused"),
        StreamState::Ended => t!("Ended"),
        StreamState::Stopped => t!("Stopped"),
    }
}

fn state_color(s: StreamState, p: &Palette) -> egui::Color32 {
    match s {
        StreamState::Playing => p.success,
        StreamState::Buffering => p.warn,
        StreamState::Paused => p.accent,
        StreamState::Ended | StreamState::Stopped => p.text_mute,
    }
}

fn state_hint(s: StreamState) -> &'static str {
    match s {
        StreamState::Playing => t!("Data is flowing to the player."),
        StreamState::Buffering => t!("The player is waiting for pieces that haven't downloaded yet."),
        StreamState::Paused => t!("The player isn't reading: it's paused or its buffer is full."),
        StreamState::Ended => t!("The player closed its connection."),
        StreamState::Stopped => t!("Stopped from Corrent."),
    }
}

/// File availability strip with the playback position: downloaded parts in the accent color,
/// the part buffered ahead of the position highlighted, and a marker at the position.
fn stream_bar(ui: &mut Ui, p: &Palette, s: &StreamView, height: f32) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), p.bg3);
    let n = s.availability.len();
    if n > 0 {
        let w = rect.width() / n as f32;
        for (i, &a) in s.availability.iter().enumerate() {
            if a == 0 {
                continue;
            }
            let x = rect.left() + i as f32 * w;
            painter.rect_filled(
                egui::Rect::from_min_max(pos2(x, rect.top()), pos2(x + w + 0.5, rect.bottom())),
                CornerRadius::ZERO,
                p.accent.gamma_multiply(0.2 + 0.4 * a as f32 / 255.0),
            );
        }
    }
    if s.file_len > 0 {
        let frac = |b: u64| (b.min(s.file_len) as f64 / s.file_len as f64) as f32;
        let px = rect.left() + rect.width() * frac(s.position);
        if let Some(ahead) = s.buffered_ahead.filter(|a| *a > 0) {
            let end = rect.left() + rect.width() * frac(s.position.saturating_add(ahead));
            painter.rect_filled(
                egui::Rect::from_min_max(pos2(px, rect.top()), pos2(end.max(px + 1.0), rect.bottom())),
                CornerRadius::ZERO,
                p.success.gamma_multiply(0.75),
            );
        }
        painter.rect_filled(
            egui::Rect::from_center_size(pos2(px, rect.center().y), vec2(3.0, rect.height() + 4.0)),
            CornerRadius::same(1),
            p.text,
        );
    }
    painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, p.border), StrokeKind::Inside);
    resp
}

fn state_badge(ui: &mut Ui, p: &Palette, s: StreamState) {
    let r = badge(ui, state_label(s), state_color(s, p));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, state_label(s)));
    r.on_hover_text(state_hint(s));
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 { "0%".into() } else { format!("{:.0}%", part.min(whole) as f64 * 100.0 / whole as f64) }
}

impl CorrentApp {
    /// One line in the Files tab while a file of this torrent is streamed.
    pub(super) fn stream_notice(&mut self, ui: &mut Ui, p: &Palette, hash: InfoHash) {
        let streams: Vec<StreamView> =
            self.snap.session.streams.iter().filter(|s| s.hash == hash && s.state.is_active()).cloned().collect();
        egui::Frame::new()
            .fill(p.accent.gamma_multiply(0.10))
            .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.35)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for s in &streams {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::PLAY_CIRCLE).color(p.accent));
                        let name = s.file_name.rsplit(['/', '\\']).next().unwrap_or(&s.file_name);
                        ui.add(
                            egui::Label::new(
                                RichText::new(t!("“{name}” to {player}", name = name, player = player(s)))
                                    .color(p.text),
                            )
                            .truncate(),
                        );
                        state_badge(ui, p, s.state);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button(t!("Details")).clicked() {
                                self.extras.streams = true;
                            }
                            ui.label(
                                RichText::new(percent(s.position, s.file_len)).monospace().small().color(p.text_dim),
                            );
                        });
                    });
                }
            });
    }

    pub(super) fn streams_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.extras.streams {
            return;
        }
        let units = self.settings.appearance.speed_units;
        let session = self.snap.session.clone();
        let mut open = true;
        egui::Window::new(RichText::new(format!("{}  {}", icons::PLAY_CIRCLE, t!("Streaming"))).strong())
            .id(Id::new("streams_window"))
            .open(&mut open)
            .collapsible(false)
            .default_size(vec2(560.0, 620.0))
            .min_width(420.0)
            .show(ctx, |ui| {
                // Server line.
                ui.horizontal_wrapped(|ui| match &session.stream {
                    Some((port, _)) => {
                        ui.label(RichText::new(icons::BROADCAST).color(p.success));
                        ui.label(
                            RichText::new(t!("Server on 127.0.0.1:{port}", port = port)).small().color(p.text_dim),
                        )
                        .on_hover_text(t!(
                            "Only programs on this computer can connect, and every link carries a secret token."
                        ));
                        let (used, max) = session.stream_slots;
                        ui.label(
                            RichText::new(t!("{used} of {max} slots in use", used = used, max = max))
                                .small()
                                .color(if used >= max && max > 0 { p.warn } else { p.text_dim }),
                        )
                        .on_hover_text(t!("Files are sent to at most this many player connections at once."));
                    }
                    None => {
                        ui.label(RichText::new(icons::WARNING).color(p.warn));
                        ui.label(RichText::new(t!("The streaming server isn't running.")).small().color(p.warn));
                    }
                });
                ui.add_space(6.0);
                if session.streams.is_empty() {
                    ui.allocate_ui(vec2(ui.available_width(), 160.0), |ui| {
                        empty_compact(
                            ui,
                            p,
                            icons::PLAY_CIRCLE,
                            t!("Nothing is streaming"),
                            t!("Use Play on a torrent with a video or audio file to watch it while it downloads."),
                        );
                    });
                    return;
                }
                egui::ScrollArea::vertical().max_height(520.0).auto_shrink([false, true]).show(ui, |ui| {
                    // Active first, newest first.
                    let mut list: Vec<&StreamView> = session.streams.iter().collect();
                    list.sort_by_key(|s| (!s.state.is_active(), std::cmp::Reverse(s.id)));
                    for s in list {
                        self.stream_card(ui, p, s, units);
                        ui.add_space(8.0);
                    }
                });
            });
        if !open {
            self.extras.streams = false;
        }
    }

    fn stream_card(&mut self, ui: &mut Ui, p: &Palette, s: &StreamView, units: crate::config::SpeedUnits) {
        let active = s.state.is_active();
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let name = s.file_name.rsplit(['/', '\\']).next().unwrap_or(&s.file_name).to_string();
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::ui::file_tree::file_icon(&name)).size(18.0).color(p.accent));
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(&name).strong().color(p.text)).truncate())
                        .on_hover_text(&s.file_name);
                    if !s.torrent_name.is_empty() {
                        ui.add(egui::Label::new(RichText::new(&s.torrent_name).small().color(p.text_dim)).truncate());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    state_badge(ui, p, s.state);
                });
            });
            ui.add_space(6.0);
            stream_bar(ui, p, s, 10.0).on_hover_text(t!(
                "Downloaded parts of the file; green is buffered ahead of the playback position (white marker)."
            ));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} / {}  ·  {}",
                        fmt_bytes(s.position.min(s.file_len)),
                        fmt_bytes(s.file_len),
                        percent(s.position, s.file_len)
                    ))
                    .monospace()
                    .small()
                    .color(p.text_dim),
                );
                if let Some(ahead) = s.buffered_ahead
                    && active
                {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let color = if ahead == 0 { p.warn } else { p.success };
                        ui.label(
                            RichText::new(t!("{size} buffered ahead", size = fmt_bytes(ahead))).small().color(color),
                        );
                    });
                }
            });
            ui.add_space(6.0);
            egui::Grid::new(("stream_info", s.id)).num_columns(4).spacing(vec2(18.0, 4.0)).show(ui, |ui| {
                let k = |ui: &mut Ui, text: &str| {
                    ui.label(RichText::new(text).small().color(p.text_mute));
                };
                let v = |ui: &mut Ui, text: String| {
                    ui.label(RichText::new(text).color(p.text));
                };
                k(ui, t!("Player"));
                ui.label(RichText::new(player(s)).color(p.text)).on_hover_text(if s.user_agent.is_empty() {
                    t!("The player didn't identify itself.").to_string()
                } else {
                    t!("User-Agent: {ua}", ua = s.user_agent)
                });
                k(ui, t!("Client"));
                v(ui, s.client.clone());
                ui.end_row();
                k(ui, t!("Speed"));
                v(ui, if active { fmt_speed(s.rate_bps, units) } else { "–".into() });
                k(ui, t!("Sent"));
                v(ui, fmt_bytes(s.served));
                ui.end_row();
                k(ui, t!("Duration"));
                v(ui, fmt_duration(std::time::Duration::from_secs(s.elapsed_secs)));
                k(ui, t!("Connections"));
                let requests = if s.requests == 1 {
                    t!("1 request so far").to_string()
                } else {
                    t!("{n} requests so far", n = s.requests)
                };
                ui.label(RichText::new(t!("{n} open", n = s.connections)).color(p.text)).on_hover_text(format!(
                    "{requests}
{}",
                    t!("Players open extra connections to probe and seek.")
                ));
                ui.end_row();
            });
            if s.refused > 0 {
                ui.add_space(4.0);
                let text = if s.refused == 1 {
                    t!("1 request was refused because every streaming slot was busy.").to_string()
                } else {
                    t!("{n} requests were refused because every streaming slot was busy.", n = s.refused)
                };
                ui.label(RichText::new(format!("{}  {text}", icons::WARNING)).small().color(p.warn));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if active
                    && ui
                        .button(format!("{}  {}", icons::STOP, t!("Stop")))
                        .on_hover_text(t!("Close the player's connection"))
                        .clicked()
                {
                    self.engine.send(EngineCommand::StopStream(s.id));
                }
                if !active && ui.button(format!("{}  {}", icons::PLAY, t!("Play again"))).clicked() {
                    self.play(s.hash, s.file, &s.file_name);
                }
                if ui.button(format!("{}  {}", icons::LINK, t!("Copy link"))).clicked()
                    && let Some(url) = self.stream_url(s.hash, s.file, &s.file_name)
                {
                    self.copy(url);
                }
            });
        });
    }
}

/// Player name for display, or a placeholder when it didn't identify itself.
fn player(s: &StreamView) -> String {
    if s.player.is_empty() { t!("Unknown player").to_string() } else { s.player.clone() }
}
