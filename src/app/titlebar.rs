//! Custom window title bar: the toolbar doubles as the title bar (drag to move, double-click to
//! maximize) with app-styled window controls. On macOS the native traffic lights are kept.

use egui::{
    Align2, Color32, Context, CornerRadius, CursorIcon, FontId, Id, Rect, ResizeDirection, Sense, Ui, ViewportCommand,
    vec2,
};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::ui::theme::Palette;

pub const CUSTOM_DECORATIONS: bool = cfg!(not(target_os = "macos"));
pub const TITLEBAR_HEIGHT: f32 = 44.0;
/// Space reserved for the macOS traffic-light buttons.
pub const MACOS_INSET: f32 = if cfg!(target_os = "macos") { 74.0 } else { 0.0 };

impl CorrentApp {
    /// Make empty title bar space drag the window. Call before adding the bar's widgets so they
    /// take precedence over this background interaction.
    pub(super) fn titlebar_background(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let bg = ui.interact(rect, Id::new("titlebar_drag"), Sense::click_and_drag());
        if bg.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if bg.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized).unwrap_or(false);
            ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
    }

    /// The app logo at the start of the title bar.
    pub(super) fn titlebar_logo(&mut self, ui: &mut Ui, p: &Palette) {
        ui.add_space(MACOS_INSET);
        let tex = self.app_icon_texture(ui.ctx());
        ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(22.0, 22.0)).corner_radius(CornerRadius::same(5)));
        ui.add_space(2.0);
        ui.label(egui::RichText::new("Corrent").font(crate::ui::theme::bold(15.0)).color(p.text));
    }

    pub fn app_icon_texture(&mut self, ctx: &Context) -> egui::TextureHandle {
        self.icon_tex
            .get_or_insert_with(|| {
                let img = egui::ColorImage::from_rgba_unmultiplied([128, 128], &super::icon::icon_rgba(128));
                ctx.load_texture("app_icon", img, egui::TextureOptions::LINEAR)
            })
            .clone()
    }

    /// Minimize / maximize / close, flush with the window edge (right-to-left layout).
    pub(super) fn window_controls(&mut self, ui: &mut Ui, p: &Palette) {
        if !CUSTOM_DECORATIONS {
            return;
        }
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
        let spacing = ui.spacing().item_spacing.x;
        ui.spacing_mut().item_spacing.x = 0.0;
        if control(ui, p, icons::X, "Close", true).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        let (icon, tip) = if maximized { (icons::COPY_SIMPLE, "Restore") } else { (icons::SQUARE, "Maximize") };
        if control(ui, p, icon, tip, false).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        if control(ui, p, icons::MINUS, "Minimize", false).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        ui.spacing_mut().item_spacing.x = spacing;
    }

    /// Resize handles along the window edges plus a hairline border (borderless windows only).
    pub(super) fn window_frame(&mut self, ctx: &Context, p: &Palette) {
        if !CUSTOM_DECORATIONS {
            return;
        }
        let (maximized, fullscreen) =
            ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
        if maximized || fullscreen {
            return;
        }
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("window_border")));
        painter.rect_stroke(
            rect.shrink(0.5),
            CornerRadius::ZERO,
            egui::Stroke::new(1.0, p.border_hi),
            egui::StrokeKind::Inside,
        );

        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = 5.0;
        let (l, r, t, b) =
            (pos.x - rect.left() < m, rect.right() - pos.x < m, pos.y - rect.top() < m, rect.bottom() - pos.y < m);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)),
            (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)),
            (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)),
            (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)),
            (true, ..) => Some((ResizeDirection::West, CursorIcon::ResizeWest)),
            (_, true, ..) => Some((ResizeDirection::East, CursorIcon::ResizeEast)),
            (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeNorth)),
            (_, _, _, true) => Some((ResizeDirection::South, CursorIcon::ResizeSouth)),
            _ => None,
        };
        if let Some((dir, cursor)) = dir {
            ctx.set_cursor_icon(cursor);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        }
    }
}

fn control(ui: &mut Ui, p: &Palette, icon: &str, tip: &str, danger: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, TITLEBAR_HEIGHT), Sense::click());
    let hovered = resp.hovered();
    let t = ui.ctx().animate_bool_with_time(resp.id, hovered, 0.1);
    if t > 0.0 {
        let fill = if danger { Color32::from_rgb(0xE8, 0x11, 0x23) } else { p.bg3 };
        ui.painter().rect_filled(Rect::from_min_max(rect.min, rect.max), CornerRadius::ZERO, fill.gamma_multiply(t));
    }
    let color = if danger && hovered {
        Color32::WHITE
    } else if hovered {
        p.text
    } else {
        p.text_dim
    };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(15.0), color);
    resp.on_hover_text(tip)
}
