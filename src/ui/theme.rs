//! Visual design system: palettes, fonts and egui style setup.

use std::sync::Arc;

use egui::style::{HandleShape, ScrollStyle, Selection, WidgetVisuals};
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    Theme, ThemePreference, Visuals, vec2,
};

use crate::config::{Accent, Appearance, ThemeMode};
use crate::engine::types::TorrentState;

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg0: Color32,
    pub bg1: Color32,
    pub bg2: Color32,
    pub bg3: Color32,
    pub border: Color32,
    pub border_hi: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_mute: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub queued: Color32,
    pub paused: Color32,
    pub completed: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub fn accent_color(a: Accent, custom: [u8; 3], dark: bool) -> Color32 {
    match (a, dark) {
        (Accent::Blue, true) => hex(0x4F8CFF),
        (Accent::Blue, false) => hex(0x2F6FEB),
        (Accent::Violet, true) => hex(0x8B7CFF),
        (Accent::Violet, false) => hex(0x6D5CF0),
        (Accent::Teal, true) => hex(0x2DD4BF),
        (Accent::Teal, false) => hex(0x0F9C8C),
        (Accent::Green, true) => hex(0x3DDC84),
        (Accent::Green, false) => hex(0x1FA463),
        (Accent::Orange, true) => hex(0xFF8A3D),
        (Accent::Orange, false) => hex(0xE06A15),
        (Accent::Pink, true) => hex(0xFF6B9D),
        (Accent::Pink, false) => hex(0xE0457A),
        (Accent::Custom, _) => Color32::from_rgb(custom[0], custom[1], custom[2]),
    }
}

impl Palette {
    pub fn dark(accent: Color32) -> Palette {
        Palette {
            dark: true,
            bg0: hex(0x0F1115),
            bg1: hex(0x161920),
            bg2: hex(0x1D212B),
            bg3: hex(0x262B36),
            border: hex(0x2A303C),
            border_hi: hex(0x3A4150),
            text: hex(0xE6E8EE),
            text_dim: hex(0x8B93A5),
            text_mute: hex(0x5C6479),
            accent,
            success: hex(0x3DDC84),
            warn: hex(0xF5B942),
            error: hex(0xFF5C5C),
            queued: hex(0xB48CFF),
            paused: hex(0x7C8597),
            completed: hex(0x9AA3B5),
        }
    }

    pub fn light(accent: Color32) -> Palette {
        Palette {
            dark: false,
            bg0: hex(0xF3F5F9),
            bg1: hex(0xFFFFFF),
            bg2: hex(0xF7F8FB),
            bg3: hex(0xE9ECF2),
            border: hex(0xDDE1EA),
            border_hi: hex(0xC7CDD9),
            text: hex(0x1A1F2B),
            text_dim: hex(0x5B6476),
            text_mute: hex(0x8C94A6),
            accent,
            success: hex(0x1FA463),
            warn: hex(0xD9930D),
            error: hex(0xDC3C3C),
            queued: hex(0x7B5CF5),
            paused: hex(0x6B7382),
            completed: hex(0x6B7382),
        }
    }

    pub fn accent_soft(&self) -> Color32 {
        self.accent.gamma_multiply(if self.dark { 0.20 } else { 0.14 })
    }

    pub fn state_color(&self, s: TorrentState) -> Color32 {
        match s {
            TorrentState::Downloading => self.accent,
            TorrentState::Metadata => self.accent.gamma_multiply(0.8),
            TorrentState::Stalled => self.accent.gamma_multiply(0.55),
            TorrentState::Seeding => self.success,
            TorrentState::Completed => self.completed,
            TorrentState::Paused => self.paused,
            TorrentState::Queued => self.queued,
            TorrentState::Checking => self.warn,
            TorrentState::Error => self.error,
        }
    }

    pub fn state_icon(s: TorrentState) -> &'static str {
        use egui_phosphor::regular as i;
        match s {
            TorrentState::Downloading => i::ARROW_CIRCLE_DOWN,
            TorrentState::Metadata => i::MAGNET,
            TorrentState::Stalled => i::HOURGLASS_MEDIUM,
            TorrentState::Seeding => i::ARROW_CIRCLE_UP,
            TorrentState::Completed => i::CHECK_CIRCLE,
            TorrentState::Paused => i::PAUSE_CIRCLE,
            TorrentState::Queued => i::CLOCK,
            TorrentState::Checking => i::CIRCLE_NOTCH,
            TorrentState::Error => i::WARNING_CIRCLE,
        }
    }
}

/// The palette currently in effect for this frame.
pub fn palette(ctx: &Context, a: &Appearance) -> Palette {
    let dark = ctx.theme() == Theme::Dark;
    let accent = accent_color(a.accent, a.custom_accent, dark);
    if dark { Palette::dark(accent) } else { Palette::light(accent) }
}

pub fn install_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "InterSemiBold".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"))),
    );
    fonts.font_data.insert(
        "JetBrainsMono".into(),
        Arc::new(FontData::from_static(include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"))),
    );
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Inter".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "JetBrainsMono".into());
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // A bold family for headings; falls back to the regular stack for icons and symbols.
    let mut bold = vec!["InterSemiBold".to_string()];
    bold.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn apply(ctx: &Context, a: &Appearance) {
    for theme in [Theme::Dark, Theme::Light] {
        let dark = theme == Theme::Dark;
        let accent = accent_color(a.accent, a.custom_accent, dark);
        let p = if dark { Palette::dark(accent) } else { Palette::light(accent) };
        ctx.style_mut_of(theme, |s| style(s, &p, a));
    }
    ctx.set_theme(match a.theme {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Dark => ThemePreference::Dark,
        ThemeMode::Light => ThemePreference::Light,
    });
    let zoom = a.zoom.clamp(0.6, 2.5);
    if (ctx.zoom_factor() - zoom).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
}

fn style(s: &mut egui::Style, p: &Palette, a: &Appearance) {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg1;
    v.window_fill = p.bg2;
    v.extreme_bg_color = p.bg0;
    v.faint_bg_color = p.bg2;
    v.code_bg_color = p.bg0;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.text_dim);
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.error;
    v.warn_fg_color = p.warn;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
    };
    v.popup_shadow =
        Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if p.dark { 80 } else { 30 }) };
    v.selection = Selection {
        bg_fill: p.accent.gamma_multiply(if p.dark { 0.35 } else { 0.2 }),
        stroke: Stroke::new(1.0, p.text),
    };
    v.striped = false;
    v.slider_trailing_fill = true;
    v.handle_shape = HandleShape::Circle;
    v.window_highlight_topmost = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let r = CornerRadius::same(5);
    v.widgets.noninteractive = WidgetVisuals {
        bg_fill: p.bg1,
        weak_bg_fill: p.bg1,
        bg_stroke: Stroke::new(1.0, p.border),
        fg_stroke: Stroke::new(1.0, p.text_dim),
        corner_radius: r,
        expansion: 0.0,
    };
    v.widgets.inactive = WidgetVisuals {
        bg_fill: p.bg3,
        weak_bg_fill: p.bg3,
        bg_stroke: Stroke::new(1.0, p.border),
        fg_stroke: Stroke::new(1.0, p.text),
        corner_radius: r,
        expansion: 0.0,
    };
    v.widgets.hovered = WidgetVisuals {
        bg_fill: p.bg3,
        weak_bg_fill: p.bg3.lerp_to_gamma(p.text, 0.06),
        bg_stroke: Stroke::new(1.0, p.border_hi),
        fg_stroke: Stroke::new(1.5, p.text),
        corner_radius: r,
        expansion: 1.0,
    };
    v.widgets.active = WidgetVisuals {
        bg_fill: p.accent,
        weak_bg_fill: p.accent.gamma_multiply(0.45),
        bg_stroke: Stroke::new(1.0, p.accent),
        fg_stroke: Stroke::new(2.0, p.text),
        corner_radius: r,
        expansion: 1.0,
    };
    v.widgets.open = v.widgets.hovered;
    s.visuals = v;

    s.spacing.item_spacing = vec2(8.0, 6.0);
    s.spacing.button_padding = vec2(10.0, 5.0);
    s.spacing.window_margin = Margin::same(18);
    s.spacing.menu_margin = Margin::same(6);
    s.spacing.interact_size = vec2(40.0, 26.0);
    s.spacing.indent = 18.0;
    s.spacing.combo_width = 160.0;
    s.spacing.scroll = ScrollStyle::floating();
    s.spacing.tooltip_width = 360.0;
    s.animation_time = if a.animations { 0.14 } else { 0.0 };
    s.interaction.selectable_labels = false;
    s.interaction.tooltip_delay = 0.35;
    s.text_styles = [
        (TextStyle::Heading, bold(17.0)),
        (TextStyle::Body, FontId::proportional(13.5)),
        (TextStyle::Button, FontId::proportional(13.5)),
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_colors_are_distinct_where_it_matters() {
        for p in [
            Palette::dark(accent_color(Accent::Blue, [0; 3], true)),
            Palette::light(accent_color(Accent::Blue, [0; 3], false)),
        ] {
            let c = |s| p.state_color(s);
            assert_ne!(c(TorrentState::Downloading), c(TorrentState::Seeding));
            assert_ne!(c(TorrentState::Error), c(TorrentState::Paused));
            assert_ne!(c(TorrentState::Queued), c(TorrentState::Paused));
        }
    }

    #[test]
    fn custom_accent_is_used() {
        assert_eq!(accent_color(Accent::Custom, [1, 2, 3], true), Color32::from_rgb(1, 2, 3));
        assert_ne!(accent_color(Accent::Teal, [0; 3], true), accent_color(Accent::Teal, [0; 3], false));
    }

    #[test]
    fn every_state_has_an_icon() {
        use TorrentState::*;
        for s in [Metadata, Checking, Downloading, Stalled, Seeding, Completed, Paused, Queued, Error] {
            assert!(!Palette::state_icon(s).is_empty());
        }
    }
}
