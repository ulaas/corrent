//! The player's sound: what this libmpv can do, and what the player hands over to be saved.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use corrent_core::audio_fx::{AudioFx, Band, Caps, FILTERS, Loudness};
use corrent_mpv::{Mpv, Node};
use egui::{Color32, CornerRadius, Rect, Ui, pos2, vec2};

use crate::ui::theme::Palette;

static PROBED: OnceLock<Caps> = OnceLock::new();
static PROBING: AtomicBool = AtomicBool::new(false);
static ASSUMED: LazyLock<Caps> = LazyLock::new(Caps::assumed);

/// What this libmpv can do, for use on the UI thread: the first call starts the probe in the
/// background, and until it has answered everything is assumed to be there.
pub fn caps() -> &'static Caps {
    if let Some(c) = PROBED.get() {
        return c;
    }
    if !PROBING.swap(true, Ordering::Relaxed) {
        let started = std::thread::Builder::new().name("mpv-filter-probe".into()).spawn(|| match probe() {
            Ok(c) => {
                tracing::debug!("sound filters: {:?}", c);
                let _ = PROBED.set(c);
            }
            Err(e) => tracing::debug!("sound filter probe: {e}"),
        });
        if let Err(e) = started {
            tracing::debug!("sound filter probe: {e}");
        }
    }
    &ASSUMED
}

/// Asks a headless mpv: the bridged form of a filter (`@probe:name`) is refused when the name is
/// unknown, while `lavfi=[graph]` text would pass, being read only when it runs. Blocking.
fn probe() -> Result<Caps, corrent_mpv::Error> {
    let mpv = Mpv::headless(&[("ao", "null")])?;
    // Without this answer the rest would say nothing.
    if mpv.set_property("af", "@probe:nosuchfilter_xyz").is_ok() {
        return Ok(Caps::assumed());
    }
    let filters = FILTERS
        .iter()
        .filter(|f| mpv.set_property("af", &format!("@probe:{f}")).is_ok())
        .map(|f| f.to_string())
        .collect();
    let list = mpv.get("command-list")?;
    let target = list.as_array().iter().any(|c| {
        c.get("name").and_then(Node::as_str) == Some("af-command")
            && c.get("args")
                .is_some_and(|a| a.as_array().iter().any(|a| a.get("name").and_then(Node::as_str) == Some("target")))
    });
    Ok(Caps { filters, target })
}

/// A closing player has no way to the settings, so what is left to save waits here for the next
/// frame.
static VOLUME: Mutex<Option<(f32, bool)>> = Mutex::new(None);
static FX: Mutex<Option<AudioFx>> = Mutex::new(None);

pub(super) fn post_fx(fx: AudioFx) {
    *crate::util::lock(&FX) = Some(fx);
}

pub(super) fn take_fx() -> Option<AudioFx> {
    crate::util::lock(&FX).take()
}

pub(super) fn post_volume(volume: f32, muted: bool) {
    *crate::util::lock(&VOLUME) = Some((volume, muted));
}

pub(super) fn take_volume() -> Option<(f32, bool)> {
    crate::util::lock(&VOLUME).take()
}

/// How long a peak stays up.
const PEAK_HOLD: Duration = Duration::from_secs(3);
/// 10 s of momentary loudness, at the 10 readings a second that come in.
const HISTORY: usize = 100;
/// Under this, a loudness reading means "not measured yet" (`ebur128` says -120.7).
const NOT_YET: f32 = -70.0;
/// The level at the bottom of the bars.
const FLOOR: f32 = -60.0;

/// What the metering filters last said. Values are in dB (dBFS, LUFS, dBTP), `None` until known.
#[derive(Clone, Debug, Default)]
pub struct Meters {
    pub momentary: Option<f32>,
    pub short_term: Option<f32>,
    pub integrated: Option<f32>,
    pub range: Option<f32>,
    pub rms: Vec<f32>,
    pub peak: Vec<f32>,
    /// True peak per channel, with what is held of it.
    pub true_peak: Vec<f32>,
    pub peak_hold: Vec<(f32, Option<Instant>)>,
    /// -1 to 1; stereo only.
    pub correlation: Option<f32>,
    /// RMS across channels in front of the effects.
    pub rms_in: Option<f32>,
    /// The momentary loudness of the last 10 s.
    pub history: VecDeque<f32>,
    history_at: Option<Instant>,
}

fn number(meta: &Node, key: &str) -> Option<f32> {
    match meta.get(key)? {
        Node::String(s) => s.parse().ok(),
        other => other.as_f64().map(|v| v as f32),
    }
}

/// `prefix` + 1, 2, … + `suffix`, as long as the keys go on.
fn per_channel(meta: &Node, first: usize, key: impl Fn(usize) -> String) -> Vec<f32> {
    (first..).map_while(|i| number(meta, &key(i))).collect()
}

fn mean_power_db(levels: &[f32]) -> Option<f32> {
    if levels.is_empty() {
        return None;
    }
    let sum: f32 = levels.iter().map(|l| 10f32.powf(l.max(-150.0) / 10.0)).sum();
    Some(10.0 * (sum / levels.len() as f32).log10())
}

impl Meters {
    /// `af-metadata/out`.
    pub fn read_output(&mut self, meta: &Node, now: Instant) {
        if !matches!(meta, Node::Map(_)) {
            return;
        }
        let loudness = |key: &str| number(meta, key).filter(|v| *v > NOT_YET);
        if number(meta, "lavfi.r128.M").is_some() {
            self.momentary = loudness("lavfi.r128.M");
            self.short_term = loudness("lavfi.r128.S");
            self.integrated = loudness("lavfi.r128.I");
            self.range = number(meta, "lavfi.r128.LRA");
        }
        self.correlation = number(meta, "lavfi.aphasemeter.phase");
        let rms = per_channel(meta, 1, |i| format!("lavfi.astats.{i}.RMS_level"));
        if !rms.is_empty() {
            self.rms = rms;
            self.peak = per_channel(meta, 1, |i| format!("lavfi.astats.{i}.Peak_level"));
        }
        // Linear, and from 0.
        let true_peak: Vec<f32> = per_channel(meta, 0, |i| format!("lavfi.r128.true_peaks_ch{i}"))
            .into_iter()
            .map(|v| if v > 0.0 { 20.0 * v.log10() } else { f32::NEG_INFINITY })
            .collect();
        if !true_peak.is_empty() {
            self.peak_hold.resize(true_peak.len(), (f32::NEG_INFINITY, None));
            for (hold, now_level) in self.peak_hold.iter_mut().zip(&true_peak) {
                let over = hold.1.is_none_or(|t| now.saturating_duration_since(t) >= PEAK_HOLD);
                if *now_level >= hold.0 || over {
                    *hold = (*now_level, Some(now));
                }
            }
            self.true_peak = true_peak;
        }
        if let Some(m) = self.momentary
            && self.history_at.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(95))
        {
            self.history_at = Some(now);
            if self.history.len() >= HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(m);
        }
    }

    /// `af-metadata/in`.
    pub fn read_input(&mut self, meta: &Node) {
        self.rms_in = mean_power_db(&per_channel(meta, 1, |i| format!("lavfi.astats.{i}.RMS_level")));
    }

    /// How much quieter the effects made it than they found it, the makeup gain aside. A rough
    /// figure with other effects in the chain.
    pub fn gain_reduction(&self, makeup_db: f32) -> Option<f32> {
        Some((self.rms_in? - mean_power_db(&self.rms)? + makeup_db).max(0.0))
    }
}

/// "s16 48000 Hz 5.1", from `audio-params` or `audio-out-params`.
pub fn audio_format(params: &Node) -> String {
    let text = |k: &str| params.get(k).and_then(Node::as_str).unwrap_or("");
    let rate = params.get("samplerate").and_then(Node::as_i64).unwrap_or(0);
    if rate == 0 {
        return String::new();
    }
    format!("{} {rate} Hz {}", text("format"), text("channels"))
}

/// What the Audio page shows besides the meters.
pub struct MeterInfo<'a> {
    pub layout: &'a str,
    pub chain: &'a [String],
    pub audio_in: &'a str,
    pub audio_out: &'a str,
    pub ao: &'a str,
    pub running: bool,
    pub passthrough: bool,
    /// The compressor's makeup gain, when it runs.
    pub makeup: Option<f32>,
}

/// Short channel names for the usual layouts.
fn channel_names(layout: &str) -> &'static [&'static str] {
    match layout {
        "mono" => &["M"],
        "stereo" => &["L", "R"],
        "5.1" => &["L", "R", "C", "LFE", "BL", "BR"],
        "5.1(side)" => &["L", "R", "C", "LFE", "SL", "SR"],
        "7.1" => &["L", "R", "C", "LFE", "BL", "BR", "SL", "SR"],
        _ => &[],
    }
}

fn lufs(v: Option<f32>) -> String {
    v.map_or_else(|| "—".to_string(), |v| format!("{v:.1}"))
}

fn level_x(r: Rect, db: f32) -> f32 {
    let f = if db.is_finite() { ((db - FLOOR) / -FLOOR).clamp(0.0, 1.0) } else { 0.0 };
    r.left() + r.width() * f
}

/// Paints the Audio page into the top of `area` (a dark box only as tall as it needs), dropping
/// what doesn't fit `area`'s height. Compact: the numbers without the history and the text lines.
pub fn paint_meters(painter: &egui::Painter, area: Rect, m: &Meters, info: &MeterInfo<'_>, compact: bool) {
    let white = Color32::WHITE;
    let dim = Color32::from_white_alpha(170);
    let mono = |size: f32| egui::FontId::monospace(size);
    let pad = 8.0;
    let x = area.left() + pad;
    let right = area.right() - pad;
    let inner = right - x;
    let mut shapes: Vec<egui::Shape> = Vec::new();
    let mut y = area.top() + pad;
    let mut history = None;
    let fits = |y: f32, h: f32| y + h <= area.bottom() - pad;

    if info.passthrough {
        let g = painter.layout(t!("No meters while Dolby and DTS pass through.").to_string(), mono(12.0), dim, inner);
        y += g.size().y;
        shapes.push(egui::Shape::galley(pos2(x, area.top() + pad), g, dim));
    } else {
        // Loudness.
        for (label, v) in [("M", m.momentary), ("S", m.short_term), ("I", m.integrated)] {
            let g = painter.layout_no_wrap(
                format!("{label} {:>6}", lufs(v)),
                mono(if compact { 12.0 } else { 15.0 }),
                white,
            );
            let h = g.size().y;
            let at = match label {
                "M" => x,
                "S" => x + inner / 3.0,
                _ => x + inner * 2.0 / 3.0,
            };
            shapes.push(egui::Shape::galley(pos2(at, y), g, white));
            if label == "I" {
                y += h + 2.0;
            }
        }
        let g = painter.layout_no_wrap(format!("LUFS · LRA {}", lufs(m.range)), mono(11.0), dim);
        y += g.size().y + 4.0;
        shapes.push(egui::Shape::galley(pos2(x, y - g.size().y - 4.0), g, dim));

        // The last 10 s of momentary loudness, -60 to 0.
        if !compact && fits(y, 32.0) {
            let r = Rect::from_min_size(pos2(x, y), vec2(inner, 28.0));
            shapes.push(egui::Shape::rect_filled(r, 2.0, Color32::from_white_alpha(14)));
            history = Some(r);
            y += 32.0;
        }

        // A row per channel: RMS bar, sample peak, held true peak, its number.
        let names = channel_names(info.layout);
        let channels = m.rms.len().max(m.true_peak.len());
        let name_w = 28.0;
        let value_w = 42.0;
        let bars_left = x + name_w;
        let bars_right = right - value_w;
        for i in 0..channels {
            if !fits(y, 14.0) {
                break;
            }
            let name = names.get(i).map_or_else(|| format!("{}", i + 1), |n| n.to_string());
            let g = painter.layout_no_wrap(name, mono(10.0), dim);
            shapes.push(egui::Shape::galley(pos2(x, y), g, dim));
            let r = Rect::from_min_max(pos2(bars_left, y + 2.0), pos2(bars_right, y + 11.0));
            shapes.push(egui::Shape::rect_filled(r, 1.0, Color32::from_white_alpha(22)));
            let rms = m.rms.get(i).copied().unwrap_or(f32::NEG_INFINITY);
            let fill = Rect::from_min_max(r.min, pos2(level_x(r, rms), r.bottom()));
            shapes.push(egui::Shape::rect_filled(fill, 1.0, Color32::from_rgb(88, 190, 130)));
            if let Some(p) = m.peak.get(i) {
                let px = level_x(r, *p);
                shapes.push(egui::Shape::line_segment(
                    [pos2(px, r.top()), pos2(px, r.bottom())],
                    egui::Stroke::new(1.0, Color32::from_rgb(170, 225, 190)),
                ));
            }
            let held = m.peak_hold.get(i).map_or(f32::NEG_INFINITY, |h| h.0);
            let over = held > -1.0;
            let hold_color = if over { Color32::from_rgb(240, 90, 80) } else { white };
            let hx = level_x(r, held);
            shapes.push(egui::Shape::line_segment(
                [pos2(hx, r.top() - 1.0), pos2(hx, r.bottom() + 1.0)],
                egui::Stroke::new(1.5, hold_color),
            ));
            // The -1 dBTP mark.
            let mark = level_x(r, -1.0);
            shapes.push(egui::Shape::line_segment(
                [pos2(mark, r.top()), pos2(mark, r.bottom())],
                egui::Stroke::new(1.0, Color32::from_rgb(240, 180, 80)),
            ));
            let number = if held.is_finite() { format!("{held:+.1}") } else { "−∞".to_string() };
            let g = painter.layout_no_wrap(number, mono(10.0), hold_color);
            let w = g.size().x;
            shapes.push(egui::Shape::galley(pos2(right - w, y), g, hold_color));
            y += 14.0;
        }

        // Correlation, -1 to +1, on stereo.
        if let Some(c) = m.correlation
            && fits(y, 14.0)
        {
            let g = painter.layout_no_wrap("φ".to_string(), mono(10.0), dim);
            shapes.push(egui::Shape::galley(pos2(x, y), g, dim));
            let r = Rect::from_min_max(pos2(bars_left, y + 3.0), pos2(bars_right, y + 10.0));
            shapes.push(egui::Shape::rect_filled(r, 1.0, Color32::from_white_alpha(22)));
            let (mid, at) = (r.center().x, r.center().x + r.width() / 2.0 * c.clamp(-1.0, 1.0));
            let color = if c < 0.0 { Color32::from_rgb(240, 90, 80) } else { Color32::from_rgb(120, 190, 255) };
            shapes.push(egui::Shape::rect_filled(
                Rect::from_min_max(pos2(mid.min(at), r.top()), pos2(mid.max(at), r.bottom())),
                0.0,
                color,
            ));
            let g = painter.layout_no_wrap(format!("{c:+.2}"), mono(10.0), dim);
            let w = g.size().x;
            shapes.push(egui::Shape::galley(pos2(right - w, y), g, dim));
            y += 14.0;
        }

        if let Some(gr) = info.makeup.and_then(|makeup| m.gain_reduction(makeup))
            && fits(y, 14.0)
        {
            let g = painter.layout_no_wrap(t!("Compressor −{db} dB", db = format!("{gr:.1}")), mono(11.0), dim);
            let h = g.size().y;
            shapes.push(egui::Shape::galley(pos2(x, y), g, dim));
            y += h + 2.0;
        }
        if !info.running && fits(y, 16.0) {
            let g = painter.layout_no_wrap(t!("Waiting for sound…").to_string(), mono(11.0), dim);
            let h = g.size().y;
            shapes.push(egui::Shape::galley(pos2(x, y), g, dim));
            y += h + 2.0;
        }

        // What is running, and the way through.
        if !compact {
            let mut lines = Vec::new();
            if !info.chain.is_empty() {
                lines.push(format!("{} {}", t!("Filters"), info.chain.join(" · ")));
            }
            if !info.audio_in.is_empty() {
                lines.push(format!("{} {}", t!("In"), info.audio_in));
            }
            if !info.audio_out.is_empty() {
                lines.push(format!("{} {} ({})", t!("Out"), info.audio_out, info.ao));
            }
            for line in lines {
                let g = painter.layout(line, mono(11.0), dim, inner);
                let h = g.size().y;
                if !fits(y, h) {
                    break;
                }
                shapes.push(egui::Shape::galley(pos2(x, y), g, dim));
                y += h;
            }
        }
    }
    let back = Rect::from_min_max(area.min, pos2(area.right(), (y + pad).min(area.bottom())));
    painter.rect_filled(back, CornerRadius::same(6), Color32::from_black_alpha(170));
    painter.extend(shapes);
    if let Some(r) = history {
        let samples: Vec<f32> = m.history.iter().map(|v| v - FLOOR).collect();
        crate::ui::widgets::paint_series(painter, r, &samples, Color32::from_rgb(120, 190, 255), Some(-FLOOR), true);
    }
}

/// The response of one band at `hz` in dB (the equalizer's own biquad at 48 kHz), for the curve.
pub fn band_response_db(band: &Band, hz: f32) -> f32 {
    use std::f64::consts::PI;
    const RATE: f64 = 48_000.0;
    let (f0, q, gain) = (f64::from(band.hz.max(1)), f64::from(band.q.max(0.01)), f64::from(band.db));
    let a = 10f64.powf(gain / 40.0);
    let w0 = 2.0 * PI * f0.min(RATE * 0.49) / RATE;
    let alpha = w0.sin() / (2.0 * q);
    let (b0, b1, b2) = (1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a);
    let (a0, a1, a2) = (1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a);
    let w = 2.0 * PI * f64::from(hz) / RATE;
    // |H(e^jw)|² from the coefficients.
    let power = |c0: f64, c1: f64, c2: f64| {
        let re = c0 + c1 * w.cos() + c2 * (2.0 * w).cos();
        let im = c1 * w.sin() + c2 * (2.0 * w).sin();
        re * re + im * im
    };
    (10.0 * (power(b0, b1, b2) / power(a0, a1, a2)).log10()) as f32
}

/// The equalizer's combined response, -12 to +12 dB over 20 Hz to 20 kHz.
fn eq_curve(ui: &mut Ui, p: &Palette, eq: &[Band]) {
    let width = ui.available_width().clamp(200.0, 420.0);
    let (r, _) = ui.allocate_exact_size(vec2(width, 54.0), egui::Sense::hover());
    let painter = ui.painter_at(r);
    painter.rect_filled(r, CornerRadius::same(4), p.bg2);
    let zero = r.center().y;
    painter.line_segment([pos2(r.left(), zero), pos2(r.right(), zero)], egui::Stroke::new(1.0, p.border));
    let steps = 96;
    let points: Vec<egui::Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let hz = 20.0 * 1000f32.powf(t);
            let db: f32 = eq.iter().map(|b| band_response_db(b, hz)).sum();
            pos2(r.left() + r.width() * t, zero - (db / 12.0).clamp(-1.0, 1.0) * (r.height() / 2.0 - 2.0))
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, p.accent)));
}

fn band_label(hz: u32) -> String {
    if hz >= 1000 { format!("{}k", hz / 1000) } else { hz.to_string() }
}

/// The sound sheet: presets, equalizer, night mode, dialogue, crossfeed, loudness and meters, each
/// offered only when this libmpv has its filters. Used in the player's settings and in Settings →
/// Playback. Returns whether something changed.
pub fn sheet(ui: &mut Ui, p: &Palette, fx: &mut AudioFx, passthrough: bool) -> bool {
    let caps = caps();
    let before = fx.clone();
    let mut picked = false;
    if passthrough {
        ui.label(
            egui::RichText::new(t!("Processing is off while Dolby and DTS pass through to the receiver."))
                .color(p.warn),
        );
    }
    ui.add_enabled_ui(!passthrough, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(!fx.has_effects() && fx.preset.is_none(), t!("Flat")).clicked() {
                fx.flatten();
                picked = true;
            }
            for (name, label, there) in [
                ("night", t!("Night"), caps.night()),
                ("dialogue", t!("Dialogue"), caps.eq()),
                ("crossfeed", t!("Crossfeed"), caps.crossfeed()),
            ] {
                if there && ui.selectable_label(fx.preset.as_deref() == Some(name), label).clicked() {
                    fx.apply_preset(name);
                    picked = true;
                }
            }
        });
        if caps.eq() {
            ui.add_space(4.0);
            eq_curve(ui, p, &fx.eq);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().slider_width = 84.0;
                for b in &mut fx.eq {
                    ui.allocate_ui(vec2(24.0, 110.0), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add(
                                egui::Slider::new(&mut b.db, -12.0..=12.0).vertical().step_by(0.5).show_value(false),
                            )
                            .on_hover_text(t!(
                                "{hz} Hz  {db} dB",
                                hz = b.hz,
                                db = format!("{:+.1}", b.db)
                            ));
                            ui.label(egui::RichText::new(band_label(b.hz)).small().color(p.text_dim));
                        })
                    });
                }
            });
        }
        egui::CollapsingHeader::new(t!("Effects")).id_salt("sound-effects").show(ui, |ui| {
            ui.checkbox(&mut fx.enabled, t!("On (off to compare)"));
            if caps.night() {
                let mut on = fx.night.is_some();
                if ui
                    .checkbox(&mut on, t!("Night mode"))
                    .on_hover_text(t!("Quiet parts come up, loud ones go down."))
                    .changed()
                {
                    fx.night = on.then(Default::default);
                }
                if let Some(n) = &mut fx.night {
                    ui.indent("night", |ui| {
                        ui.add(egui::Slider::new(&mut n.threshold_db, -60.0..=0.0).suffix(" dB").text(t!("Threshold")));
                        ui.add(egui::Slider::new(&mut n.ratio, 1.0..=20.0).text(t!("Ratio")));
                        ui.add(egui::Slider::new(&mut n.makeup_db, 0.0..=24.0).suffix(" dB").text(t!("Makeup")));
                    });
                }
            }
            if caps.eq() {
                ui.checkbox(&mut fx.dialogue, t!("Clearer dialogue"))
                    .on_hover_text(t!("Lifts voices; on 5.1 also the centre channel."));
            }
            if caps.crossfeed() {
                let mut on = fx.crossfeed.is_some();
                if ui
                    .checkbox(&mut on, t!("Headphone crossfeed"))
                    .on_hover_text(t!("Mixes a little of each side into the other. Stereo sources only."))
                    .changed()
                {
                    fx.crossfeed = on.then(Default::default);
                }
                if let Some(c) = &mut fx.crossfeed {
                    ui.indent("crossfeed", |ui| {
                        ui.add(egui::Slider::new(&mut c.strength, 0.0..=1.0).text(t!("Strength")));
                        ui.add(egui::Slider::new(&mut c.range, 0.0..=1.0).text(t!("Range")));
                    });
                }
            }
        });
        if caps.loudness() {
            ui.horizontal_wrapped(|ui| {
                ui.label(t!("Even out loudness")).on_hover_text(t!(
                    "A slow leveller: quiet and loud parts come closer. Holds about a second of sound."
                ));
                ui.selectable_value(&mut fx.loudness, Loudness::Off, t!("Off"));
                ui.selectable_value(&mut fx.loudness, Loudness::Live, t!("Live"));
            });
        }
    });
    if caps.meters() {
        ui.checkbox(&mut fx.meters, t!("Show meters"))
            .on_hover_text(t!("Loudness, peaks and more over the picture (Shift+I). They run only while shown."));
    }
    let changed = *fx != before;
    if changed && !picked {
        // By hand: no longer the preset, and on if an effect changed (the switch itself stays as set).
        let meters_only = AudioFx { meters: before.meters, enabled: before.enabled, ..fx.clone() } == before;
        if !meters_only {
            fx.preset = None;
            fx.enabled = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use corrent_core::audio_fx::{Crossfeed, Night, Source, af_string, chain, plan};
    use corrent_mpv::{Event, Library};

    use super::*;

    fn play(mpv: &Mpv, url: &str) {
        mpv.command(&["loadfile", url]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(Event::PlaybackRestart) = mpv.wait_event(0.1) {
                return;
            }
        }
        panic!("{url} never started");
    }

    fn level(mpv: &Mpv, key: &str, after: f64) -> f64 {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let at = mpv.get_f64("time-pos").ok().flatten().unwrap_or(0.0);
            if at >= after
                && let Ok(meta) = mpv.get("af-metadata/out")
                && let Some(v) = meta.get(key).and_then(Node::as_str).and_then(|v| v.parse().ok())
            {
                return v;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{key} never appeared");
    }

    fn labels(mpv: &Mpv) -> Vec<String> {
        mpv.get("af")
            .unwrap()
            .as_array()
            .iter()
            .filter_map(|f| f.get("label").and_then(Node::as_str).map(str::to_string))
            .collect()
    }

    #[test]
    fn the_probe_finds_what_the_chain_uses() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let caps = probe().unwrap();
        println!("{caps:?}");
        for f in ["equalizer", "acompressor", "alimiter", "crossfeed", "dynaudnorm", "astats", "ebur128", "aphasemeter"]
        {
            assert!(caps.has(f), "{f}");
        }
        assert!(caps.has("pan") && caps.has("volume"));
        assert!(caps.target);
    }

    fn with_everything(meters: bool) -> AudioFx {
        let mut fx = AudioFx {
            enabled: true,
            meters,
            night: Some(Night::default()),
            crossfeed: Some(Crossfeed::default()),
            dialogue: true,
            loudness: Loudness::Live,
            ..Default::default()
        };
        fx.eq[2].db = 3.0;
        fx
    }

    /// Everything the builder makes runs on a real libmpv, and every command a plan sends is taken.
    #[test]
    fn built_chains_run_and_their_plans_apply() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let caps = Caps::assumed();
        let stereo = Source { channels: Some(2), layout: Some("stereo".into()) };
        let mut fx = with_everything(true);
        let first = chain(&fx, &caps, &stereo);
        let mpv = Mpv::headless(&[("ao", "null"), ("af", &af_string(&first))]).unwrap();
        play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=8,aformat=channel_layouts=stereo");
        level(&mpv, "lavfi.r128.M", 0.2);
        assert_eq!(labels(&mpv), ["gain", "in", "eq", "dyn", "spatial", "norm", "out"]);
        assert_eq!(mpv.get("audio-out-params").unwrap().get("channel-count").and_then(Node::as_i64), Some(2));

        // Values go by command, and each is accepted.
        fx.eq[2].db = 5.0;
        fx.eq[7].db = -2.5;
        fx.night = Some(Night { threshold_db: -30.0, ratio: 2.5, makeup_db: 3.0 });
        fx.crossfeed = Some(Crossfeed { strength: 0.2, range: 0.8 });
        let p = plan(&first, &chain(&fx, &caps, &stereo), true);
        assert_eq!(p.set_af, None);
        assert!(p.commands.len() >= 8, "{:?}", p.commands);
        for [label, option, value, target] in &p.commands {
            let r = mpv.command(&["af-command", label, option, value, target]);
            assert!(r.is_ok(), "af-command {label} {option} {value} {target}: {r:?}");
        }
        // Bypassing too.
        fx.night = None;
        fx.crossfeed = None;
        let q = plan(&p.applied, &chain(&fx, &caps, &stereo), true);
        assert_eq!(q.set_af, None);
        for [label, option, value, target] in &q.commands {
            let r = mpv.command(&["af-command", label, option, value, target]);
            assert!(r.is_ok(), "af-command {label} {option} {value} {target}: {r:?}");
        }
        // And all off leaves no filter.
        let off = plan(&q.applied, &chain(&AudioFx::default(), &caps, &stereo), true);
        mpv.set_property("af", off.set_af.as_deref().unwrap()).unwrap();
        assert!(mpv.get("af").unwrap().as_array().is_empty());
    }

    /// The centre lift on 5.1: the layout stays, and only the centre gains on the others.
    #[test]
    fn dialogue_lifts_the_centre_of_5_1() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let url = "av://lavfi:sine=f=1000:r=48000:d=6,aformat=channel_layouts=5.1";
        let source = Source { channels: Some(6), layout: Some("5.1".into()) };
        let mut fx = AudioFx { meters: true, ..Default::default() };
        let plain =
            Mpv::headless(&[("ao", "null"), ("af", &af_string(&chain(&fx, &Caps::assumed(), &source)))]).unwrap();
        play(&plain, url);
        let before = level(&plain, "lavfi.astats.3.RMS_level", 1.0);
        fx.apply_preset("dialogue");
        fx.meters = true;
        let af = af_string(&chain(&fx, &Caps::assumed(), &source));
        assert!(af.contains("pan=5.1|"), "{af}");
        let mpv = Mpv::headless(&[("ao", "null"), ("af", &af)]).unwrap();
        play(&mpv, url);
        let after = level(&mpv, "lavfi.astats.3.RMS_level", 1.0);
        let out = mpv.get("audio-out-params").unwrap();
        println!("centre {before} -> {after}, {out:?}");
        assert_eq!(out.get("channel-count").and_then(Node::as_i64), Some(6));
        assert_eq!(out.get("channels").and_then(Node::as_str), Some("5.1"));
        // -7 dB headroom, +4 dB centre, and a 1 kHz tone is on the skirt of +3 dB at 2.5 kHz.
        let change = after - before;
        assert!((-3.5..=0.0).contains(&change), "{change}");
    }

    /// Nothing on: nothing runs and no metadata arrives.
    #[test]
    fn off_means_no_filter() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let af = corrent_core::config::Playback::default()
            .mpv_options()
            .into_iter()
            .find(|(k, _)| *k == "af")
            .map(|(_, v)| v)
            .unwrap();
        assert_eq!(af, "");
        let mpv = Mpv::headless(&[("ao", "null"), ("af", &af)]).unwrap();
        play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=2");
        assert!(mpv.get("af").unwrap().as_array().is_empty());
        assert!(mpv.get("af-metadata/out").is_err());
        // The source's own format reaches the output untouched.
        assert_eq!(mpv.get("audio-out-params").unwrap().get("format").and_then(Node::as_str), Some("s16"));
    }

    /// Captured from the pinned libmpv: stereo, with `aphasemeter`.
    fn captured() -> Node {
        let pairs = [
            ("lavfi.aphasemeter.phase", "1.000000"),
            ("lavfi.astats.1.Peak_level", "-21.072762"),
            ("lavfi.astats.1.RMS_level", "-24.066955"),
            ("lavfi.astats.2.Peak_level", "-21.072762"),
            ("lavfi.astats.2.RMS_level", "-24.066955"),
            ("lavfi.r128.I", "-21.070"),
            ("lavfi.r128.LRA", "0.000"),
            ("lavfi.r128.LRA.high", "0.000"),
            ("lavfi.r128.LRA.low", "0.000"),
            ("lavfi.r128.M", "-21.067"),
            ("lavfi.r128.S", "-120.691"),
            ("lavfi.r128.true_peak", "0.088"),
            ("lavfi.r128.true_peaks_ch0", "0.088"),
            ("lavfi.r128.true_peaks_ch1", "0.044"),
        ];
        Node::Map(pairs.iter().map(|(k, v)| (k.to_string(), Node::String(v.to_string()))).collect())
    }

    #[test]
    fn meters_read_what_mpv_sends() {
        let mut m = Meters::default();
        let now = Instant::now();
        m.read_output(&captured(), now);
        assert_eq!(m.momentary, Some(-21.067));
        assert_eq!(m.short_term, None, "-120.7 means not measured yet");
        assert_eq!(m.integrated, Some(-21.07));
        assert_eq!(m.range, Some(0.0));
        assert_eq!(m.rms, [-24.066955, -24.066955]);
        assert_eq!(m.peak, [-21.072762, -21.072762]);
        assert_eq!(m.correlation, Some(1.0));
        // True peak comes linear: 0.088 is -21.1 dBTP.
        assert!((m.true_peak[0] + 21.11).abs() < 0.01, "{:?}", m.true_peak);
        assert!((m.true_peak[1] + 27.13).abs() < 0.01, "{:?}", m.true_peak);
        assert_eq!(m.history.len(), 1);
    }

    #[test]
    fn silence_and_nonsense_are_harmless() {
        let mut m = Meters::default();
        let now = Instant::now();
        m.read_output(&Node::None, now);
        m.read_output(&Node::Map(Default::default()), now);
        assert!(m.rms.is_empty() && m.momentary.is_none());
        let silent = Node::Map(
            [
                ("lavfi.astats.1.RMS_level", "-inf"),
                ("lavfi.astats.1.Peak_level", "-inf"),
                ("lavfi.r128.M", "-120.691"),
                ("lavfi.r128.true_peaks_ch0", "0.000"),
                ("lavfi.astats.2.RMS_level", "garbage"),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), Node::String(v.to_string())))
            .collect(),
        );
        m.read_output(&silent, now);
        assert_eq!(m.rms, [f32::NEG_INFINITY], "up to the one that doesn't read");
        assert_eq!(m.momentary, None);
        assert_eq!(m.true_peak, [f32::NEG_INFINITY]);
        assert_eq!(m.history.len(), 0);
        assert_eq!(m.gain_reduction(0.0), None);
    }

    #[test]
    fn a_true_peak_is_held_for_three_seconds() {
        let meta = |linear: &str| {
            Node::Map(
                [("lavfi.r128.true_peaks_ch0".to_string(), Node::String(linear.to_string()))].into_iter().collect(),
            )
        };
        let mut m = Meters::default();
        let t0 = Instant::now();
        m.read_output(&meta("0.5"), t0);
        let high = m.peak_hold[0].0;
        m.read_output(&meta("0.1"), t0 + Duration::from_secs(2));
        assert_eq!(m.peak_hold[0].0, high, "still held");
        assert!(m.true_peak[0] < high);
        m.read_output(&meta("0.1"), t0 + Duration::from_millis(3100));
        assert!(m.peak_hold[0].0 < high, "let go");
        // A higher one takes over at once.
        m.read_output(&meta("0.9"), t0 + Duration::from_millis(3200));
        assert!(m.peak_hold[0].0 > high);
    }

    #[test]
    fn the_history_keeps_ten_seconds() {
        let mut m = Meters::default();
        let t0 = Instant::now();
        for i in 0..250u64 {
            m.read_output(&captured(), t0 + Duration::from_millis(i * 100));
        }
        assert_eq!(m.history.len(), HISTORY);
        // Readings closer than the rate it is kept at don't add up.
        let mut m = Meters::default();
        for i in 0..10u64 {
            m.read_output(&captured(), t0 + Duration::from_millis(i));
        }
        assert_eq!(m.history.len(), 1);
    }

    #[test]
    fn gain_reduction_from_in_and_out() {
        let mut m = Meters::default();
        m.read_output(&captured(), Instant::now());
        let input = |rms: &str| {
            Node::Map(
                ["lavfi.astats.1.RMS_level", "lavfi.astats.2.RMS_level"]
                    .iter()
                    .map(|k| (k.to_string(), Node::String(rms.to_string())))
                    .collect(),
            )
        };
        m.read_input(&input("-18.066955"));
        // Came in at -18, went out at -24, with 6 dB of makeup: 12 dB taken.
        let gr = m.gain_reduction(6.0).unwrap();
        assert!((gr - 12.0).abs() < 0.01, "{gr}");
        // Louder than it came in isn't a reduction.
        m.read_input(&input("-30"));
        assert_eq!(m.gain_reduction(0.0), Some(0.0));
    }

    #[test]
    fn formats_read_as_a_line() {
        let params = Node::Map(
            [
                ("format".to_string(), Node::String("s16".into())),
                ("samplerate".to_string(), Node::Int(44100)),
                ("channels".to_string(), Node::String("stereo".into())),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(audio_format(&params), "s16 44100 Hz stereo");
        assert_eq!(audio_format(&Node::None), "");
    }

    #[test]
    fn the_curve_matches_the_band() {
        let b = Band { hz: 1000, q: 1.0, db: 6.0 };
        assert!((band_response_db(&b, 1000.0) - 6.0).abs() < 0.01);
        assert!(band_response_db(&b, 20.0).abs() < 0.1, "far away it does nothing");
        assert_eq!(band_response_db(&Band { db: 0.0, ..b }, 500.0), 0.0);
    }
}
