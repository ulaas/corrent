//! Motion: every animation goes through here, so turning animations off (Appearance →
//! Animations, which sets egui's `animation_time` to 0) stops all of them at once. Durations
//! follow the design tokens: micro interactions ≤ 120 ms, panels ~180 ms, page-level 250 ms.

use egui::{Context, Id};

/// Hover, press, toggles.
pub const MICRO: f32 = 0.12;
/// Overlays, panels, indicators sliding.
pub const PANEL: f32 = 0.18;
/// Image fade-ins, page changes.
pub const PAGE: f32 = 0.25;
/// Slow progress easing.
pub const SLOW: f32 = 0.35;

pub fn enabled(ctx: &Context) -> bool {
    ctx.global_style().animation_time > 0.0
}

/// 0 → 1 as `on` becomes true (instantly when animations are off).
pub fn toggle(ctx: &Context, id: Id, on: bool, secs: f32) -> f32 {
    ctx.animate_bool_with_time(id, on, if enabled(ctx) { secs } else { 0.0 })
}

/// Eases towards `target` (jumps when animations are off).
pub fn value(ctx: &Context, id: Id, target: f32, secs: f32) -> f32 {
    ctx.animate_value_with_time(id, target, if enabled(ctx) { secs } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &Context, f: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(Default::default(), f);
        out.textures_delta.clear();
    }

    #[test]
    fn animations_off_jump_straight_to_the_end() {
        let ctx = Context::default();
        frame(&ctx, |ui| ui.ctx().global_style_mut(|s| s.animation_time = 0.0));
        let mut t = 0.0;
        frame(&ctx, |ui| {
            toggle(ui.ctx(), Id::new("x"), false, PAGE);
        });
        frame(&ctx, |ui| t = toggle(ui.ctx(), Id::new("x"), true, PAGE));
        assert_eq!(t, 1.0);
    }
}
