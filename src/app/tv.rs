//! TV mode (the 10-foot interface): the media pages only, full screen and 1.5× larger, driven by
//! arrow keys or a gamepad. Gamepad input becomes ordinary key events before egui sees them, so
//! egui's own spatial focus (arrows), activation (Enter/Space) and our Back handling do the rest.
//! An on-screen keyboard fills the search field; after a while without input a screensaver
//! shows backdrops.

use std::time::{Duration, Instant};

use egui::{Color32, Key, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::ui::theme::Palette;

const SLIDE: Duration = Duration::from_secs(12);
/// Held direction: first repeat after this, then every `REPEAT_EVERY`.
const REPEAT_AFTER: Duration = Duration::from_millis(400);
const REPEAT_EVERY: Duration = Duration::from_millis(110);

pub struct TvState {
    pub on: bool,
    /// The zoom we set (to restore the user's when leaving).
    applied: Option<f32>,
    #[cfg(feature = "gamepad")]
    gamepad: Option<gilrs::Gilrs>,
    #[cfg(feature = "gamepad")]
    gamepad_failed: bool,
    /// The direction being held (key, since, last repeat).
    held: Option<(Key, Instant, Instant)>,
    stick: (f32, f32),
    last_input: Instant,
    /// Showing the screensaver (since).
    saver: Option<Instant>,
    pub keyboard: bool,
    #[cfg(debug_assertions)]
    dev_started: Option<Instant>,
    #[cfg(debug_assertions)]
    dev_sent: usize,
}

impl Default for TvState {
    fn default() -> Self {
        TvState {
            on: false,
            applied: None,
            #[cfg(feature = "gamepad")]
            gamepad: None,
            #[cfg(feature = "gamepad")]
            gamepad_failed: false,
            held: None,
            stick: (0.0, 0.0),
            last_input: Instant::now(),
            saver: None,
            keyboard: false,
            #[cfg(debug_assertions)]
            dev_started: None,
            #[cfg(debug_assertions)]
            dev_sent: 0,
        }
    }
}

fn key_event(key: Key, pressed: bool) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
}

impl CorrentApp {
    pub fn with_tv(mut self, on: bool) -> Self {
        if on {
            self.set_tv(true);
        }
        self
    }

    pub fn set_tv(&mut self, on: bool) {
        self.tv.on = on;
        self.tv.saver = None;
        self.tv.last_input = Instant::now();
        if on {
            self.main_view = super::feeds::MainView::Media;
        }
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /// Gamepad buttons and sticks become key presses (before egui reads input); any input ends
    /// the screensaver.
    pub(super) fn tv_raw_input(&mut self, raw: &mut egui::RawInput) {
        if !self.tv.on {
            return;
        }
        let mut keys: Vec<Key> = Vec::new();
        #[cfg(feature = "gamepad")]
        self.poll_gamepad(&mut keys);
        #[cfg(debug_assertions)]
        self.dev_keys(&mut keys);
        // Held direction repeats.
        if let Some((k, since, last)) = self.tv.held
            && since.elapsed() > REPEAT_AFTER
            && last.elapsed() > REPEAT_EVERY
        {
            keys.push(k);
            self.tv.held = Some((k, since, Instant::now()));
        }
        let in_player = self.media.player.as_ref().is_some_and(|p| !p.audio_only);
        for k in keys {
            // "A" pauses in the player and activates elsewhere.
            let k = if k == Key::Enter && in_player { Key::Space } else { k };
            raw.events.push(key_event(k, true));
            raw.events.push(key_event(k, false));
        }
        let active = raw.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Key { .. }
                    | egui::Event::Text(_)
                    | egui::Event::PointerButton { .. }
                    | egui::Event::MouseWheel { .. }
                    | egui::Event::PointerMoved(_)
            )
        });
        if active {
            self.tv.last_input = Instant::now();
            if self.tv.saver.take().is_some() {
                self.tv.held = None;
                // The input that woke the screen does nothing else.
                raw.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::PointerButton { .. }));
            }
        }
    }

    /// Debug hook: `CORRENT_DEV_KEYS=Right,Right,Down,Enter` presses those keys, one every
    /// 300 ms starting 3 s after launch (as a remote would).
    #[cfg(debug_assertions)]
    fn dev_keys(&mut self, keys: &mut Vec<Key>) {
        let Ok(list) = std::env::var("CORRENT_DEV_KEYS") else { return };
        let start = *self.tv.dev_started.get_or_insert_with(Instant::now);
        let due = (start.elapsed().as_millis().saturating_sub(3000) / 300) as usize;
        let all: Vec<&str> = list.split(',').collect();
        while self.tv.dev_sent < due.min(all.len()) {
            if let Some(k) = Key::from_name(all[self.tv.dev_sent].trim()) {
                keys.push(k);
            }
            self.tv.dev_sent += 1;
        }
        if self.tv.dev_sent < all.len() {
            self.ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    #[cfg(feature = "gamepad")]
    fn poll_gamepad(&mut self, keys: &mut Vec<Key>) {
        use gilrs::{Axis, Button, EventType};
        if self.tv.gamepad.is_none() && !self.tv.gamepad_failed {
            match gilrs::Gilrs::new() {
                Ok(g) => self.tv.gamepad = Some(g),
                Err(e) => {
                    tracing::info!("no gamepad support: {e}");
                    self.tv.gamepad_failed = true;
                }
            }
        }
        let Some(g) = &mut self.tv.gamepad else { return };
        let dir = |b: Button| match b {
            Button::DPadUp => Some(Key::ArrowUp),
            Button::DPadDown => Some(Key::ArrowDown),
            Button::DPadLeft => Some(Key::ArrowLeft),
            Button::DPadRight => Some(Key::ArrowRight),
            _ => None,
        };
        while let Some(ev) = g.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(k) = dir(b) {
                        keys.push(k);
                        self.tv.held = Some((k, Instant::now(), Instant::now()));
                        continue;
                    }
                    match b {
                        Button::South => keys.push(Key::Enter),
                        Button::East | Button::Select => keys.push(Key::Escape),
                        Button::Start => keys.push(Key::Space),
                        Button::North => self.tv.keyboard = !self.tv.keyboard,
                        _ => {}
                    }
                }
                EventType::ButtonReleased(b, _) if dir(b).is_some() => self.tv.held = None,
                // A pad that goes away mid-press must not keep "pressing".
                EventType::Disconnected => {
                    self.tv.held = None;
                    self.tv.stick = (0.0, 0.0);
                }
                EventType::AxisChanged(axis, v, _) => {
                    let (x, y) = &mut self.tv.stick;
                    match axis {
                        Axis::LeftStickX => *x = v,
                        Axis::LeftStickY => *y = v,
                        _ => continue,
                    }
                    // Crossing the threshold presses a direction; centring releases it.
                    let pressed = if *x > 0.6 {
                        Some(Key::ArrowRight)
                    } else if *x < -0.6 {
                        Some(Key::ArrowLeft)
                    } else if *y > 0.6 {
                        Some(Key::ArrowUp)
                    } else if *y < -0.6 {
                        Some(Key::ArrowDown)
                    } else {
                        None
                    };
                    match (pressed, self.tv.held) {
                        (Some(k), Some((h, ..))) if k == h => {}
                        (Some(k), _) => {
                            keys.push(k);
                            self.tv.held = Some((k, Instant::now(), Instant::now()));
                        }
                        (None, Some(_)) if x.abs() < 0.3 && y.abs() < 0.3 => self.tv.held = None,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Per frame: the zoom, Back, and waking up for held keys and the screensaver.
    pub(super) fn tv_tick(&mut self, ctx: &egui::Context) {
        let user_zoom = self.settings.appearance.zoom.clamp(0.6, 2.5);
        if self.tv.on {
            let want = (user_zoom * self.settings.appearance.tv_scale.clamp(1.0, 2.5)).min(3.0);
            if (ctx.zoom_factor() - want).abs() > 0.001 {
                ctx.set_zoom_factor(want);
            }
            self.tv.applied = Some(want);
            // Back: Escape or Backspace (TV pages have no text fields: the on-screen keyboard
            // types; the player handles its own Escape).
            let in_player = self.media.player.as_ref().is_some_and(|p| !p.audio_only);
            if !in_player && ctx.input(|i| i.key_pressed(Key::Escape) || i.key_pressed(Key::Backspace)) {
                if self.tv.keyboard {
                    self.tv.keyboard = false;
                } else {
                    self.media_back();
                }
            }
            let after = self.settings.appearance.tv_screensaver_min;
            if !in_player
                && after > 0
                && self.tv.saver.is_none()
                && self.tv.last_input.elapsed() > Duration::from_secs(u64::from(after) * 60)
            {
                self.tv.saver = Some(Instant::now());
            }
            // Gamepads and held keys need frames even when nothing else changes.
            #[cfg(feature = "gamepad")]
            // Frame-rate polling only while a direction is held; otherwise ~10 Hz is enough to
            // notice a button press (a TV box shouldn't render 60 fps for nothing).
            if self.tv.held.is_some() {
                ctx.request_repaint_after(Duration::from_millis(16));
            } else if self.tv.gamepad.as_ref().is_some_and(|g| g.gamepads().next().is_some()) {
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        } else if let Some(z) = self.tv.applied.take()
            && (ctx.zoom_factor() - z).abs() < 0.001
        {
            ctx.set_zoom_factor(user_zoom);
        }
    }

    /// TV mode's own settings: what makes sense from a sofa (the full settings window needs a
    /// mouse and stays reachable by leaving TV mode).
    pub(super) fn tv_settings_page(&mut self, ui: &mut Ui, p: &Palette) {
        let a = self.settings.appearance.clone();
        let mut new = a.clone();
        let row = |ui: &mut Ui, title: &str, body: &mut dyn FnMut(&mut Ui)| {
            ui.horizontal(|ui| {
                ui.add_space(48.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(20.0).strong().color(p.text));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        body(ui);
                    });
                });
            });
            ui.add_space(22.0);
        };
        let choice = |ui: &mut Ui, sel: bool, label: &str| -> bool {
            ui.add(egui::Button::selectable(sel, RichText::new(label).size(18.0)).min_size(vec2(120.0, 48.0))).clicked()
        };
        ui.add_space(12.0);
        row(ui, t!("Interface size"), &mut |ui| {
            for s in [1.25f32, 1.5, 1.75, 2.0] {
                if choice(ui, (new.tv_scale - s).abs() < 0.01, &format!("{}%", (s * 100.0) as u32)) {
                    new.tv_scale = s;
                }
            }
        });
        row(ui, t!("Screensaver"), &mut |ui| {
            for m in [0u32, 2, 5, 10, 30] {
                let label = if m == 0 { t!("Never").to_string() } else { t!("After {n} min", n = m) };
                if choice(ui, new.tv_screensaver_min == m, &label) {
                    new.tv_screensaver_min = m;
                }
            }
        });
        row(ui, t!("Theme"), &mut |ui| {
            use corrent_core::config::ThemeMode;
            for (mode, label) in
                [(ThemeMode::System, t!("System")), (ThemeMode::Dark, t!("Dark")), (ThemeMode::Light, t!("Light"))]
            {
                if choice(ui, new.theme == mode, label) {
                    new.theme = mode;
                }
            }
        });
        row(ui, t!("Language"), &mut |ui| {
            let sys = t!("System ({lang})", lang = crate::i18n::system_language().native_name());
            if choice(ui, new.language.is_empty(), &sys) {
                new.language.clear();
            }
            for l in crate::i18n::Lang::ALL {
                if choice(ui, new.language == l.code(), l.native_name()) {
                    new.language = l.code().to_string();
                }
            }
        });
        let mut leave = false;
        let mut profiles = false;
        row(ui, t!("This screen"), &mut |ui| {
            if self.profile_name().is_some()
                && choice(ui, false, &format!("{}  {}", icons::USER_CIRCLE, t!("Switch profile")))
            {
                profiles = true;
            }
            if choice(ui, false, &format!("{}  {}", icons::MONITOR, t!("Leave TV mode"))) {
                leave = true;
            }
        });
        if new != a {
            self.settings.appearance = new;
            self.save_settings();
        }
        if profiles {
            self.open_profiles();
        }
        if leave {
            self.set_tv(false);
        }
    }

    /// The whole window in TV mode.
    pub(super) fn tv_view(&mut self, ui: &mut Ui, p: &Palette) {
        if let Some(since) = self.tv.saver {
            self.screensaver(ui, since);
            return;
        }
        self.media_view(ui, p);
    }

    /// Letters and digits as focusable keys (arrow/gamepad typing into the search field).
    pub(super) fn tv_keyboard(&mut self, ui: &mut Ui, p: &Palette) {
        const ROWS: [&str; 4] = ["abcdefghij", "klmnopqrst", "uvwxyz0123", "456789.-'&"];
        let mut changed = false;
        egui::Frame::new().fill(p.bg2).corner_radius(egui::CornerRadius::same(12)).inner_margin(12).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for row in ROWS {
                ui.horizontal(|ui| {
                    for c in row.chars() {
                        let b = egui::Button::new(RichText::new(c.to_string()).size(18.0).monospace())
                            .min_size(vec2(40.0, 40.0));
                        if ui.add(b).clicked() {
                            self.media.query.push(c);
                            changed = true;
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(t!("Space")).min_size(vec2(160.0, 40.0))).clicked() {
                    self.media.query.push(' ');
                    changed = true;
                }
                if ui
                    .add(
                        egui::Button::new(format!("{}  {}", icons::BACKSPACE, t!("Delete")))
                            .min_size(vec2(100.0, 40.0)),
                    )
                    .clicked()
                {
                    self.media.query.pop();
                    changed = true;
                }
                if ui.add(egui::Button::new(t!("Clear")).min_size(vec2(100.0, 40.0))).clicked() {
                    self.media.query.clear();
                    changed = true;
                }
                if ui.add(egui::Button::new(t!("Done")).min_size(vec2(100.0, 40.0))).clicked() {
                    self.tv.keyboard = false;
                }
            });
        });
        if changed && !self.media.query.trim().is_empty() {
            self.media_search_now();
        }
    }

    /// Backdrops of the home screen, slowly cross-fading.
    fn screensaver(&mut self, ui: &mut Ui, since: Instant) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        let backdrops = self.media_backdrops();
        if let (Some(client), false) = (self.media.client.clone(), backdrops.is_empty()) {
            let t = since.elapsed().as_secs_f32() / SLIDE.as_secs_f32();
            let (i, frac) = (t.floor() as usize, t.fract());
            let fade = ((frac - 0.85) / 0.15).clamp(0.0, 1.0);
            let ppp = ui.ctx().pixels_per_point();
            for (k, alpha) in [(i, 1.0 - fade), (i + 1, fade)] {
                if alpha <= 0.0 {
                    continue;
                }
                let sha = &backdrops[k % backdrops.len()];
                if let Some(tex) = self.media.image(&client, &self.rt, &self.ctx, sha, (rect.width() * ppp) as u32) {
                    // Slow drift (Ken Burns).
                    let zoom = 1.0 + 0.06 * frac;
                    let img = rect.scale_from_center(zoom);
                    ui.painter().image(
                        tex.id(),
                        img,
                        super::media::cover_uv(tex.size_vec2(), img.size()),
                        Color32::WHITE.gamma_multiply(alpha * 0.85),
                    );
                }
            }
        }
        let (mins, _) = crate::util::local_minutes_and_weekday();
        let now = format!("{:02}:{:02}", mins / 60, mins % 60);
        ui.painter().text(
            rect.right_bottom() - vec2(40.0, 40.0),
            egui::Align2::RIGHT_BOTTOM,
            now,
            egui::FontId::proportional(48.0),
            Color32::from_white_alpha(200),
        );
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }
}
