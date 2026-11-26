//! The Streaming performance window: what the server is converting and casting right now, how
//! well it keeps up, and what that costs the computer (`GET /playback/stats`, asked for every
//! two seconds while the window is open). The web client's Settings → Streaming performance
//! page shows the same.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use corrent_proto::library::{PlaybackSession, PlaybackStats};
use egui::{Context, Id, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::client::{Slot, fetch, take_ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{badge, card, empty_compact, sparkline};
use crate::util::fmt_bytes;

const EVERY: Duration = Duration::from_secs(2);
/// Two minutes of history.
const HISTORY: usize = 60;

#[derive(Default)]
pub struct PerformanceUi {
    pub open: bool,
    asking: Option<Slot<PlaybackStats>>,
    asked: Option<Instant>,
    shown: Option<Arc<PlaybackStats>>,
    error: Option<String>,
    cpu: VecDeque<f32>,
    /// Everything going out to players, in bits per second.
    out: VecDeque<f32>,
    /// Per conversion: when it was last read, the bytes sent by then, and the rate that gave.
    sent: HashMap<String, (Instant, u64, f32)>,
}

/// How a conversion is doing for its player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Health {
    /// Nothing made yet, or the encoder is getting going.
    Starting,
    Smooth,
    Barely,
    TooSlow,
    /// The player isn't fetching (paused, or gone).
    Waiting,
}

/// With this much ready beyond the player, a slow moment doesn't show.
const COMFORTABLE_AHEAD: f64 = 8.0;

pub(super) fn health(s: &PlaybackSession) -> Health {
    if s.produced == 0 {
        return Health::Starting;
    }
    if s.ahead_secs >= COMFORTABLE_AHEAD {
        return if !s.encoding && s.idle_secs > 10 { Health::Waiting } else { Health::Smooth };
    }
    // Not encoding with a player fetching: it has made all there is (the end of the file).
    if !s.encoding {
        return if s.idle_secs > 10 { Health::Waiting } else { Health::Smooth };
    }
    match s.speed {
        v if v <= 0.0 => Health::Starting,
        v if v >= 1.1 => Health::Smooth,
        v if v >= 0.95 => Health::Barely,
        _ => Health::TooSlow,
    }
}

/// "720p · 4.0 Mbit/s video · 192 kbit/s audio".
fn target(s: &PlaybackSession) -> String {
    let mut parts = Vec::new();
    if s.video {
        parts.push(match s.height {
            Some(h) => format!("{h}p"),
            None => t!("Original size").to_string(),
        });
        if let Some(b) = s.video_bitrate {
            parts.push(t!("{rate} Mbit/s video", rate = format!("{:.1}", b as f64 / 1e6)));
        }
    } else {
        parts.push(t!("Sound only").to_string());
    }
    if let Some(b) = s.audio_bitrate {
        parts.push(t!("{rate} kbit/s audio", rate = b / 1000));
    }
    parts.join(" · ")
}

fn verdict(h: Health) -> &'static str {
    match h {
        Health::Starting => t!("Starting"),
        Health::Smooth => t!("Smooth"),
        Health::Barely => t!("Barely keeping up"),
        Health::TooSlow => t!("Too slow"),
        Health::Waiting => t!("Waiting for the player"),
    }
}

/// A conversion in one line, for the player's own stats: "Server: 720p · 4.0 Mbit/s video ·
/// h264_nvenc · 3.2× · 24 s ahead · Smooth".
pub(super) fn line(s: &PlaybackSession) -> String {
    let speed = if s.encoding { format!("{:.1}×", s.speed) } else { t!("Resting").to_string() };
    format!(
        "{}: {} · {} · {speed} · {} {} · {}",
        t!("Server"),
        target(s),
        s.encoder,
        t!("Ready ahead"),
        t!("{n} s", n = format!("{:.0}", s.ahead_secs)),
        verdict(health(s)),
    )
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn push(history: &mut VecDeque<f32>, v: f32) {
    history.push_back(v);
    while history.len() > HISTORY {
        history.pop_front();
    }
}

impl PerformanceUi {
    /// A fresh reading: bandwidth from what each conversion sent since the last one.
    fn take(&mut self, stats: PlaybackStats) {
        let now = Instant::now();
        let mut total = 0.0f32;
        let mut next = HashMap::new();
        for s in &stats.sessions {
            let rate = match self.sent.get(&s.id) {
                Some((at, bytes, _)) if now.duration_since(*at) > Duration::from_millis(200) => {
                    s.sent_bytes.saturating_sub(*bytes) as f32 * 8.0 / now.duration_since(*at).as_secs_f32()
                }
                Some((_, _, rate)) => *rate,
                None => 0.0,
            };
            total += rate;
            next.insert(s.id.clone(), (now, s.sent_bytes, rate));
        }
        self.sent = next;
        push(&mut self.cpu, stats.cpu_percent);
        push(&mut self.out, total);
        self.shown = Some(Arc::new(stats));
        self.error = None;
    }

    #[cfg(test)]
    pub(crate) fn show_for_test(&mut self, stats: PlaybackStats) {
        self.open = true;
        self.asked = Some(Instant::now() + Duration::from_secs(3600));
        self.take(stats);
    }
}

impl CorrentApp {
    pub(crate) fn performance_window(&mut self, ctx: &Context, p: &Palette) {
        if !self.media.performance.open {
            return;
        }
        let Some(client) = self.media.client.clone() else { return };
        let ui_state = &mut self.media.performance;
        if let Some(r) = ui_state.asking.as_ref().and_then(take_ready) {
            ui_state.asking = None;
            match r {
                Ok(stats) => ui_state.take(stats),
                Err(e) => ui_state.error = Some(e),
            }
        }
        if ui_state.asking.is_none() && ui_state.asked.is_none_or(|t| t.elapsed() >= EVERY) {
            ui_state.asked = Some(Instant::now());
            let c = client.clone();
            ui_state.asking =
                Some(fetch(&self.rt, &self.ctx, async move { c.get_reason("/api/v1/playback/stats").await }));
        }
        ctx.request_repaint_after(EVERY);
        let stats = ui_state.shown.clone();
        let mut open = true;
        let mut stop: Option<String> = None;
        egui::Window::new(RichText::new(format!("{}  {}", icons::GAUGE, t!("Streaming performance"))).strong())
            .id(Id::new("performance_window"))
            .open(&mut open)
            .collapsible(false)
            .default_size(vec2(600.0, 640.0))
            .min_width(440.0)
            .show(ctx, |ui| {
                let st = &self.media.performance;
                if let Some(e) = &st.error {
                    ui.label(RichText::new(e).color(p.error));
                }
                let Some(stats) = stats else {
                    if st.error.is_none() {
                        ui.spinner();
                    }
                    return;
                };
                self.performance_server(ui, p, &stats);
                ui.add_space(8.0);
                egui::ScrollArea::vertical().max_height(480.0).auto_shrink([false, true]).show(ui, |ui| {
                    if stats.sessions.is_empty() && stats.casts.is_empty() {
                        ui.allocate_ui(vec2(ui.available_width(), 150.0), |ui| {
                            empty_compact(
                                ui,
                                p,
                                icons::GAUGE,
                                t!("Nothing is being converted"),
                                t!("Files that play as they are cost the server almost nothing and aren’t listed."),
                            );
                        });
                    }
                    for s in &stats.sessions {
                        let rate = self.media.performance.sent.get(&s.id).map_or(0.0, |x| x.2);
                        if conversion_card(ui, p, s, rate) {
                            stop = Some(s.id.clone());
                        }
                        ui.add_space(8.0);
                    }
                    for c in &stats.casts {
                        card(p).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icons::SCREENCAST).color(p.accent));
                                let half = ui.available_width() / 2.0;
                                crate::ui::widgets::truncated(
                                    ui,
                                    RichText::new(&c.device.name).strong().color(p.text),
                                    half,
                                );
                                if c.device.kind == "dlna" {
                                    badge(ui, t!("TV"), p.text_dim);
                                }
                                crate::ui::widgets::truncated(ui, RichText::new(&c.title).color(p.text_dim), 0.0);
                            });
                            let length = c.duration_ms.map(|d| format!(" / {}", clock(d as f64 / 1000.0)));
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}{}",
                                    cast_state(&c.state),
                                    clock(c.position_ms as f64 / 1000.0),
                                    length.unwrap_or_default()
                                ))
                                .small()
                                .color(p.text_dim),
                            );
                        });
                        ui.add_space(8.0);
                    }
                });
            });
        if let Some(id) = stop {
            let c = client.clone();
            self.rt.spawn(async move {
                let path = format!("/api/v1/playback/sessions/{id}");
                let _: Result<serde_json::Value, String> =
                    c.send(reqwest::Method::DELETE, &path, &serde_json::Value::Null).await;
            });
            self.media.performance.asked = None;
        }
        if !open {
            self.media.performance = PerformanceUi::default();
        }
    }

    /// The computer's side: processor, memory, encoder, and what goes out.
    fn performance_server(&self, ui: &mut Ui, p: &Palette, stats: &PlaybackStats) {
        let st = &self.media.performance;
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let cpu: Vec<f32> = st.cpu.iter().copied().collect();
            let out: Vec<f32> = st.out.iter().copied().collect();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(t!("Processor")).small().color(p.text_dim));
                    let color = if stats.cpu_percent > 90.0 { p.warn } else { p.text };
                    ui.label(RichText::new(format!("{:.0}%", stats.cpu_percent)).size(20.0).strong().color(color));
                    let cores =
                        if stats.cores == 1 { t!("1 core").to_string() } else { t!("{n} cores", n = stats.cores) };
                    ui.label(RichText::new(cores).small().color(p.text_mute));
                });
                sparkline(ui, &cpu, p.accent, vec2(120.0, 44.0));
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(t!("Sent to players")).small().color(p.text_dim));
                    let now = out.last().copied().unwrap_or(0.0);
                    ui.label(RichText::new(format!("{:.1} Mbit/s", now / 1e6)).size(20.0).strong().color(p.text));
                    let running = stats.sessions.len();
                    ui.label(
                        RichText::new(t!("{n} of {max} conversions", n = running, max = stats.max_sessions))
                            .small()
                            .color(if running as u32 >= stats.max_sessions { p.warn } else { p.text_mute }),
                    );
                });
                sparkline(ui, &out, p.success, vec2(120.0, 44.0));
            });
            ui.add_space(4.0);
            let encoder = match &stats.encoder {
                Some(e) => t!("Encoder in use: {name}", name = e),
                None => t!("No working video encoder was found: only sound can be converted.").to_string(),
            };
            let memory = if stats.memory_total > 0 {
                format!(
                    " · {}",
                    t!(
                        "Memory {used} of {total}",
                        used = fmt_bytes(stats.memory_used),
                        total = fmt_bytes(stats.memory_total)
                    )
                )
            } else {
                String::new()
            };
            ui.label(RichText::new(format!("{encoder}{memory}")).small().color(p.text_dim));
        });
    }
}

fn cast_state(state: &str) -> &'static str {
    match state {
        "playing" => t!("Playing"),
        "paused" => t!("Paused"),
        "buffering" | "loading" | "connecting" => t!("Loading…"),
        "error" => t!("Error"),
        _ => t!("Idle"),
    }
}

/// One conversion; true when Stop was pressed.
fn conversion_card(ui: &mut Ui, p: &Palette, s: &PlaybackSession, rate: f32) -> bool {
    let mut stop = false;
    card(p).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let h = health(s);
        ui.horizontal(|ui| {
            // The verdict and Stop stay in sight after a long title.
            crate::ui::widgets::truncated(ui, RichText::new(&s.title).strong().color(p.text), 200.0);
            let color = match h {
                Health::Smooth => p.success,
                Health::Barely => p.warn,
                Health::TooSlow => p.error,
                Health::Starting | Health::Waiting => p.text_dim,
            };
            badge(ui, verdict(h), color);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t!("Stop")).clicked() {
                    stop = true;
                }
            });
        });
        ui.label(RichText::new(format!("{} · {} · {}", s.user, target(s), s.encoder)).small().color(p.text_dim));
        if h == Health::TooSlow {
            ui.label(
                RichText::new(t!(
                    "The server can’t convert this fast enough. Choose a lower quality, or a lighter encoding effort in Settings → Playback → Transcoding."
                ))
                .small()
                .color(p.error),
            );
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            let number = |ui: &mut Ui, label: &str, value: String| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(label).small().color(p.text_mute));
                    ui.label(RichText::new(value).monospace().color(p.text));
                });
            };
            let speed = if !s.encoding {
                t!("Resting").to_string()
            } else {
                match s.fps.filter(|f| *f > 0.0 && s.video) {
                    Some(fps) => {
                        t!("{speed}× · {fps} fps", speed = format!("{:.1}", s.speed), fps = format!("{:.0}", fps * f64::from(s.speed)))
                    }
                    None => format!("{:.1}×", s.speed),
                }
            };
            number(ui, t!("Encoding speed"), speed);
            number(ui, t!("Ready ahead"), t!("{n} s", n = format!("{:.0}", s.ahead_secs)));
            number(ui, t!("Bandwidth"), format!("{:.1} Mbit/s", rate / 1e6));
            number(ui, t!("Sent"), fmt_bytes(s.sent_bytes));
            if let Some(at) = s.position_secs {
                number(ui, t!("Position"), clock(at));
            }
        });
    });
    stop
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(produced: u64, encoding: bool, speed: f32, ahead: f64, idle: u64) -> PlaybackSession {
        PlaybackSession {
            id: "s".into(),
            user: "u".into(),
            title: "Film".into(),
            item_id: None,
            encoder: "libx264".into(),
            video: true,
            segments: 100,
            produced,
            idle_secs: idle,
            height: Some(720),
            video_bitrate: Some(4_000_000),
            audio_bitrate: Some(192_000),
            fps: Some(24.0),
            encoding,
            speed,
            ahead_secs: ahead,
            position_secs: Some(12.0),
            sent_bytes: 0,
        }
    }

    #[test]
    fn a_conversion_is_judged_by_what_its_player_has_ahead() {
        assert_eq!(health(&session(0, true, 0.0, 0.0, 0)), Health::Starting);
        assert_eq!(health(&session(5, true, 3.0, 4.0, 0)), Health::Smooth, "fast encoder, little ahead yet");
        assert_eq!(health(&session(50, true, 0.5, 40.0, 0)), Health::Smooth, "slow now, but far ahead");
        assert_eq!(health(&session(5, true, 1.0, 2.0, 0)), Health::Barely);
        assert_eq!(health(&session(5, true, 0.6, 0.0, 0)), Health::TooSlow);
        assert_eq!(health(&session(50, false, 0.0, 180.0, 0)), Health::Smooth, "resting ahead of a playing player");
        assert_eq!(health(&session(50, false, 0.0, 180.0, 60)), Health::Waiting, "nobody fetched for a minute");
        assert_eq!(health(&session(10, false, 0.0, 4.0, 0)), Health::Smooth, "all of a short file is made");
    }

    #[test]
    fn what_it_is_converted_to() {
        assert_eq!(target(&session(1, true, 1.0, 0.0, 0)), "720p · 4.0 Mbit/s video · 192 kbit/s audio");
        let mut sound = session(1, true, 1.0, 0.0, 0);
        sound.video = false;
        sound.video_bitrate = None;
        assert_eq!(target(&sound), "Sound only · 192 kbit/s audio");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(65.0), "1:05");
    }
}
