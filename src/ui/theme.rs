//! Visual design system: palettes, fonts and egui style setup.

use std::sync::Arc;

use egui::style::{HandleShape, ScrollStyle, Selection, WidgetVisuals};
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    Theme, ThemePreference, Visuals, vec2,
};

use crate::config::{Accent, Appearance, PalettePreset, ThemeMode};
use crate::engine::types::TorrentState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Thicker outlines and focus rings.
    pub high_contrast: bool,
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
            high_contrast: false,
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
            high_contrast: false,
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
            warn: hex(0xB07400),
            error: hex(0xDC3C3C),
            queued: hex(0x7B5CF5),
            paused: hex(0x6B7382),
            completed: hex(0x6B7382),
        }
    }

    pub fn nord(dark: bool, accent: Color32) -> Palette {
        if dark {
            Palette {
                dark,
                high_contrast: false,
                bg0: hex(0x242933),
                bg1: hex(0x2E3440),
                bg2: hex(0x353C4A),
                bg3: hex(0x3B4252),
                border: hex(0x434C5E),
                border_hi: hex(0x4C566A),
                text: hex(0xECEFF4),
                text_dim: hex(0xB4BDCC),
                text_mute: hex(0x8691A6),
                accent,
                success: hex(0xA3BE8C),
                warn: hex(0xEBCB8B),
                error: hex(0xD7747C),
                queued: hex(0xB48EAD),
                paused: hex(0x8F99AC),
                completed: hex(0xA5AEBF),
            }
        } else {
            Palette {
                dark,
                high_contrast: false,
                bg0: hex(0xE5E9F0),
                bg1: hex(0xF2F4F8),
                bg2: hex(0xECEFF4),
                bg3: hex(0xDDE2EA),
                border: hex(0xD0D6E0),
                border_hi: hex(0xB8C0CD),
                text: hex(0x2E3440),
                text_dim: hex(0x4C566A),
                text_mute: hex(0x6B7589),
                accent,
                success: hex(0x4E7A3A),
                warn: hex(0x93670A),
                error: hex(0xA8434D),
                queued: hex(0x7E5A8C),
                paused: hex(0x5F687A),
                completed: hex(0x5F687A),
            }
        }
    }

    pub fn solarized(dark: bool, accent: Color32) -> Palette {
        if dark {
            Palette {
                dark,
                high_contrast: false,
                bg0: hex(0x001F27),
                bg1: hex(0x002B36),
                bg2: hex(0x073642),
                bg3: hex(0x0A4555),
                border: hex(0x0E4A5A),
                border_hi: hex(0x2A5F6C),
                text: hex(0xEEE8D5),
                text_dim: hex(0xA3B0B0),
                text_mute: hex(0x7B8F94),
                accent,
                success: hex(0x9DB600),
                warn: hex(0xD1A000),
                error: hex(0xF0605A),
                queued: hex(0x9A9EE8),
                paused: hex(0x8C9C9E),
                completed: hex(0xA3B0B0),
            }
        } else {
            Palette {
                dark,
                high_contrast: false,
                bg0: hex(0xEEE8D5),
                bg1: hex(0xFDF6E3),
                bg2: hex(0xF7F0DC),
                bg3: hex(0xE8E1CB),
                border: hex(0xDDD6C1),
                border_hi: hex(0xC9C2AE),
                text: hex(0x073642),
                text_dim: hex(0x45585F),
                text_mute: hex(0x6D7E83),
                accent,
                success: hex(0x5C6B00),
                warn: hex(0x8A6600),
                error: hex(0xB52520),
                queued: hex(0x52569E),
                paused: hex(0x586E75),
                completed: hex(0x586E75),
            }
        }
    }

    /// 7:1 text contrast everywhere; the accent is fixed (yellow on black, blue on white).
    pub fn high_contrast(dark: bool) -> Palette {
        if dark {
            Palette {
                dark,
                high_contrast: true,
                bg0: hex(0x000000),
                bg1: hex(0x000000),
                bg2: hex(0x0A0A0A),
                bg3: hex(0x1F1F1F),
                border: hex(0xA0A0A0),
                border_hi: hex(0xFFFFFF),
                text: hex(0xFFFFFF),
                text_dim: hex(0xE6E6E6),
                text_mute: hex(0xC4C4C4),
                accent: hex(0xFFD400),
                success: hex(0x4CFF4C),
                warn: hex(0xFFB84D),
                error: hex(0xFF8080),
                queued: hex(0xDDB3FF),
                paused: hex(0xD6D6D6),
                completed: hex(0xE6E6E6),
            }
        } else {
            Palette {
                dark,
                high_contrast: true,
                bg0: hex(0xFFFFFF),
                bg1: hex(0xFFFFFF),
                bg2: hex(0xFFFFFF),
                bg3: hex(0xEBEBEB),
                border: hex(0x595959),
                border_hi: hex(0x000000),
                text: hex(0x000000),
                text_dim: hex(0x1F1F1F),
                text_mute: hex(0x3D3D3D),
                accent: hex(0x0033A6),
                success: hex(0x005C1A),
                warn: hex(0x6B4400),
                error: hex(0x9E001C),
                queued: hex(0x4F2591),
                paused: hex(0x333333),
                completed: hex(0x262626),
            }
        }
    }

    pub fn preset(preset: PalettePreset, dark: bool, accent: Color32) -> Palette {
        match preset {
            PalettePreset::Nord => Palette::nord(dark, accent),
            PalettePreset::Solarized => Palette::solarized(dark, accent),
            PalettePreset::HighContrast => Palette::high_contrast(dark),
            PalettePreset::Corrent | PalettePreset::Custom => {
                if dark {
                    Palette::dark(accent)
                } else {
                    Palette::light(accent)
                }
            }
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

pub use corrent_core::config::PaletteDef;

pub fn color_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

pub fn parse_color(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(hex(v))
}

/// The editable colors, in display order: (field name, label).
pub const PALETTE_FIELDS: [(&str, &str); 16] = [
    ("bg0", tk!("Window background")),
    ("bg1", tk!("Panels")),
    ("bg2", tk!("Cards")),
    ("bg3", tk!("Controls")),
    ("border", tk!("Borders")),
    ("border_hi", tk!("Strong borders")),
    ("text", tk!("Text")),
    ("text_dim", tk!("Secondary text")),
    ("text_mute", tk!("Muted text")),
    ("accent", tk!("Accent")),
    ("success", tk!("Seeding / success")),
    ("warn", tk!("Checking / warning")),
    ("error", tk!("Errors")),
    ("queued", tk!("Queued")),
    ("paused", tk!("Paused")),
    ("completed", tk!("Completed")),
];

impl Palette {
    pub fn color_mut(&mut self, field: &str) -> Option<&mut Color32> {
        Some(match field {
            "bg0" => &mut self.bg0,
            "bg1" => &mut self.bg1,
            "bg2" => &mut self.bg2,
            "bg3" => &mut self.bg3,
            "border" => &mut self.border,
            "border_hi" => &mut self.border_hi,
            "text" => &mut self.text,
            "text_dim" => &mut self.text_dim,
            "text_mute" => &mut self.text_mute,
            "accent" => &mut self.accent,
            "success" => &mut self.success,
            "warn" => &mut self.warn,
            "error" => &mut self.error,
            "queued" => &mut self.queued,
            "paused" => &mut self.paused,
            "completed" => &mut self.completed,
            _ => return None,
        })
    }
}

/// Conversions between the stored palette (core config) and the UI palette.
pub trait PaletteDefExt: Sized {
    fn from_palette(name: &str, p: &Palette) -> PaletteDef;
    fn to_palette(&self) -> Result<Palette, String>;
    /// Parse a `theme.toml`, with errors a person can act on.
    fn from_toml(text: &str) -> Result<PaletteDef, String>;
    fn to_toml(&self) -> String;
}

impl PaletteDefExt for PaletteDef {
    fn from_palette(name: &str, p: &Palette) -> PaletteDef {
        let h = color_hex;
        PaletteDef {
            name: name.to_string(),
            dark: p.dark,
            bg0: h(p.bg0),
            bg1: h(p.bg1),
            bg2: h(p.bg2),
            bg3: h(p.bg3),
            border: h(p.border),
            border_hi: h(p.border_hi),
            text: h(p.text),
            text_dim: h(p.text_dim),
            text_mute: h(p.text_mute),
            accent: h(p.accent),
            success: h(p.success),
            warn: h(p.warn),
            error: h(p.error),
            queued: h(p.queued),
            paused: h(p.paused),
            completed: h(p.completed),
        }
    }

    fn to_palette(&self) -> Result<Palette, String> {
        let c = |field: &str, v: &str| {
            parse_color(v).ok_or_else(|| t!("“{field}” is not a #rrggbb color: {v}", field = field, v = v))
        };
        Ok(Palette {
            dark: self.dark,
            high_contrast: false,
            bg0: c("bg0", &self.bg0)?,
            bg1: c("bg1", &self.bg1)?,
            bg2: c("bg2", &self.bg2)?,
            bg3: c("bg3", &self.bg3)?,
            border: c("border", &self.border)?,
            border_hi: c("border_hi", &self.border_hi)?,
            text: c("text", &self.text)?,
            text_dim: c("text_dim", &self.text_dim)?,
            text_mute: c("text_mute", &self.text_mute)?,
            accent: c("accent", &self.accent)?,
            success: c("success", &self.success)?,
            warn: c("warn", &self.warn)?,
            error: c("error", &self.error)?,
            queued: c("queued", &self.queued)?,
            paused: c("paused", &self.paused)?,
            completed: c("completed", &self.completed)?,
        })
    }

    /// Parse a `theme.toml`, with errors a person can act on.
    fn from_toml(text: &str) -> Result<PaletteDef, String> {
        let def: PaletteDef =
            toml::from_str(text).map_err(|e| t!("not a Corrent theme file ({why})", why = e.message()))?;
        def.to_palette()?;
        Ok(def)
    }

    fn to_toml(&self) -> String {
        format!("# Corrent theme\n{}", toml::to_string_pretty(self).unwrap_or_default())
    }
}

/// Whether the OS asks for high contrast (Windows: Settings › Accessibility › Contrast themes).
pub fn os_high_contrast() -> bool {
    #[cfg(windows)]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = *c
            && at.elapsed() < Duration::from_secs(3)
        {
            return v;
        }
        // HCF_HIGHCONTRASTON = 0x1 in the "Flags" string value.
        let v = windows_registry::CURRENT_USER
            .open(r"Control Panel\Accessibility\HighContrast")
            .and_then(|k| k.get_string("Flags"))
            .ok()
            .and_then(|f| f.trim().parse::<u32>().ok())
            .is_some_and(|f| f & 1 != 0);
        *c = Some((Instant::now(), v));
        v
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// The palette for a theme (dark/light) under these appearance settings.
pub fn resolve(dark: bool, a: &Appearance) -> Palette {
    let preset = if a.follow_os_high_contrast && os_high_contrast() { PalettePreset::HighContrast } else { a.preset };
    if preset == PalettePreset::Custom
        && let Some(p) = a.custom_palette.as_ref().and_then(|d| d.to_palette().ok())
    {
        return p;
    }
    Palette::preset(preset, dark, accent_color(a.accent, a.custom_accent, dark))
}

/// The custom palette decides dark/light on its own.
pub fn forced_theme(a: &Appearance) -> Option<bool> {
    if a.follow_os_high_contrast && os_high_contrast() {
        return None;
    }
    (a.preset == PalettePreset::Custom).then(|| a.custom_palette.as_ref().map(|d| d.dark)).flatten()
}

/// The palette currently in effect for this frame.
pub fn palette(ctx: &Context, a: &Appearance) -> Palette {
    resolve(ctx.theme() == Theme::Dark, a)
}

/// Whether `install_fonts` ran for this context (fonts apply from the next frame on).
pub fn fonts_installed(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new("corrent_fonts_installed"))).unwrap_or(false)
}

pub fn install_fonts(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("corrent_fonts_installed"), true));
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

/// Type scale used across the app.
pub mod type_scale {
    pub const DISPLAY: f32 = 22.0;
    pub const TITLE: f32 = 18.0;
    pub const HEADING: f32 = 15.0;
    pub const BODY: f32 = 13.5;
    pub const CAPTION: f32 = 11.5;
    pub const MONO: f32 = 12.5;
}

pub fn title() -> FontId {
    bold(type_scale::TITLE)
}

pub fn heading() -> FontId {
    bold(type_scale::HEADING)
}

/// Monospace for numbers so values don't jitter as they change.
pub fn mono() -> FontId {
    FontId::monospace(type_scale::MONO)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn apply(ctx: &Context, a: &Appearance) {
    for theme in [Theme::Dark, Theme::Light] {
        let p = resolve(theme == Theme::Dark, a);
        ctx.style_mut_of(theme, |s| style(s, &p, a));
    }
    ctx.set_theme(match (forced_theme(a), a.theme) {
        (Some(true), _) => ThemePreference::Dark,
        (Some(false), _) => ThemePreference::Light,
        (None, ThemeMode::System) => ThemePreference::System,
        (None, ThemeMode::Dark) => ThemePreference::Dark,
        (None, ThemeMode::Light) => ThemePreference::Light,
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
    if p.high_contrast {
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered] {
            w.bg_stroke.width = 1.5;
        }
        v.widgets.inactive.bg_stroke.color = p.border_hi;
        v.widgets.hovered.bg_stroke = Stroke::new(2.0, p.accent);
        // Focused widgets use the "active" look: a 2 px ring in the text color.
        v.widgets.active.bg_stroke = Stroke::new(2.0, p.text);
        v.widgets.active.fg_stroke = Stroke::new(2.0, if p.dark { Color32::BLACK } else { Color32::WHITE });
        v.selection.stroke = Stroke::new(2.0, p.text);
        v.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.45 } else { 0.3 });
        v.window_stroke = Stroke::new(2.0, p.border_hi);
    }
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
        (TextStyle::Heading, bold(type_scale::HEADING + 2.0)),
        (TextStyle::Body, FontId::proportional(type_scale::BODY)),
        (TextStyle::Button, FontId::proportional(type_scale::BODY)),
        (TextStyle::Small, FontId::proportional(type_scale::CAPTION)),
        (TextStyle::Monospace, FontId::monospace(type_scale::MONO)),
    ]
    .into();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `design/tokens.json` is the source of truth for both clients: every palette here must
    /// match it (the web client builds its CSS from the same file).
    #[test]
    fn palettes_match_the_design_tokens() {
        let tokens: serde_json::Value = serde_json::from_str(include_str!("../../design/tokens.json")).unwrap();
        let hexc = |v: &serde_json::Value| -> Color32 {
            let s = v.as_str().unwrap().trim_start_matches('#');
            hex(u32::from_str_radix(s, 16).unwrap())
        };
        let blue = |dark| accent_color(Accent::Blue, [0; 3], dark);
        let palettes: Vec<(&str, bool, Palette)> = vec![
            ("corrent", true, Palette::dark(blue(true))),
            ("corrent", false, Palette::light(blue(false))),
            ("nord", true, Palette::nord(true, blue(true))),
            ("nord", false, Palette::nord(false, blue(false))),
            ("solarized", true, Palette::solarized(true, blue(true))),
            ("solarized", false, Palette::solarized(false, blue(false))),
            ("high_contrast", true, Palette::high_contrast(true)),
            ("high_contrast", false, Palette::high_contrast(false)),
        ];
        for (name, dark, p) in palettes {
            let t = &tokens["palettes"][name][if dark { "dark" } else { "light" }];
            let fields = [
                ("bg0", p.bg0),
                ("bg1", p.bg1),
                ("bg2", p.bg2),
                ("bg3", p.bg3),
                ("border", p.border),
                ("border_hi", p.border_hi),
                ("text", p.text),
                ("text_dim", p.text_dim),
                ("text_mute", p.text_mute),
                ("success", p.success),
                ("warn", p.warn),
                ("error", p.error),
                ("queued", p.queued),
                ("paused", p.paused),
                ("completed", p.completed),
                ("accent", p.accent),
            ];
            for (k, c) in fields {
                if t[k].is_null() {
                    assert_ne!(k, "bg0", "{name}/{dark}: missing in tokens");
                    continue;
                }
                assert_eq!(c, hexc(&t[k]), "{name} {} {k}", if dark { "dark" } else { "light" });
            }
        }
        for (k, a) in [
            ("blue", Accent::Blue),
            ("violet", Accent::Violet),
            ("teal", Accent::Teal),
            ("green", Accent::Green),
            ("orange", Accent::Orange),
            ("pink", Accent::Pink),
        ] {
            assert_eq!(accent_color(a, [0; 3], true), hexc(&tokens["accents"][k]["dark"]), "{k}");
            assert_eq!(accent_color(a, [0; 3], false), hexc(&tokens["accents"][k]["light"]), "{k}");
        }
    }

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

    /// WCAG 2 contrast ratio.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn palettes_meet_contrast_targets() {
        let mut failures = Vec::new();
        for preset in
            [PalettePreset::Corrent, PalettePreset::Nord, PalettePreset::Solarized, PalettePreset::HighContrast]
        {
            for dark in [true, false] {
                let p = Palette::preset(preset, dark, accent_color(Accent::Blue, [0; 3], dark));
                let hc = preset == PalettePreset::HighContrast;
                // Body text: 7:1 (AAA). Secondary text: 4.5:1 (AA), 7:1 in high contrast.
                let mut checks = vec![("text", p.text, 7.0), ("text_dim", p.text_dim, if hc { 7.0 } else { 4.5 })];
                if hc {
                    for (n, c) in [
                        ("text_mute", p.text_mute),
                        ("accent", p.accent),
                        ("success", p.success),
                        ("warn", p.warn),
                        ("error", p.error),
                        ("queued", p.queued),
                        ("paused", p.paused),
                    ] {
                        checks.push((n, c, 7.0));
                    }
                } else {
                    for (n, c) in [("success", p.success), ("error", p.error), ("warn", p.warn)] {
                        checks.push((n, c, 3.0)); // icons and large text
                    }
                }
                for (bg_name, bg) in [("bg1", p.bg1), ("bg2", p.bg2)] {
                    for (name, c, min) in &checks {
                        let r = contrast(*c, bg);
                        if r < *min {
                            failures.push(format!("{preset:?} dark={dark}: {name} on {bg_name} = {r:.2} < {min}"));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn theme_files_round_trip_and_reject_garbage() {
        let p = Palette::nord(true, hex(0x88C0D0));
        let def = PaletteDef::from_palette("Mine", &p);
        let text = def.to_toml();
        let back = PaletteDef::from_toml(&text).unwrap();
        assert_eq!(back, def);
        assert_eq!(back.to_palette().unwrap(), p);
        assert!(PaletteDef::from_toml("name = 1").unwrap_err().contains("not a Corrent theme"));
        let bad = text.replace(&color_hex(p.text), "#12345");
        assert!(PaletteDef::from_toml(&bad).unwrap_err().contains("text"));
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
