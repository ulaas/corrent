//! Custom-painted widgets.

use egui::epaint::Mesh;
use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2, pos2, vec2,
};

use super::theme::Palette;

/// A rounded progress bar with a horizontal gradient and optional animated stripes.
pub fn progress_bar(
    ui: &mut Ui,
    id: Id,
    frac: f32,
    color: Color32,
    track: Color32,
    height: f32,
    width: f32,
    striped: bool,
    animate: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    paint_progress(ui, rect, id, frac, color, track, striped, animate);
    resp
}

#[allow(clippy::too_many_arguments)]
pub fn paint_progress(
    ui: &Ui,
    rect: Rect,
    id: Id,
    frac: f32,
    color: Color32,
    track: Color32,
    striped: bool,
    animate: bool,
) {
    let painter = ui.painter_at(rect.expand(1.0));
    let r = (rect.height() / 2.0).min(255.0) as u8;
    painter.rect_filled(rect, CornerRadius::same(r), track);
    let frac = frac.clamp(0.0, 1.0);
    let t = if animate { ui.ctx().animate_value_with_time(id.with("p"), frac, 0.35) } else { frac };
    if t <= 0.0005 {
        return;
    }
    let w = (rect.width() * t).max(rect.height());
    let fill = Rect::from_min_size(rect.min, vec2(w.min(rect.width()), rect.height()));
    let c0 = color.gamma_multiply(0.78);
    let c1 = color;
    // Gradient body as a mesh, clipped into a rounded shape by drawing rounded caps.
    painter.rect_filled(fill, CornerRadius::same(r), c1);
    let inner =
        Rect::from_min_max(pos2(fill.left(), fill.top()), pos2(fill.right() - rect.height() * 0.5, fill.bottom()));
    if inner.width() > 1.0 {
        let left_cap = Rect::from_min_max(fill.min, pos2(fill.left() + rect.height(), fill.bottom()));
        painter.rect_filled(left_cap, CornerRadius::same(r), c0);
        let body = Rect::from_min_max(pos2(fill.left() + rect.height() * 0.5, fill.top()), inner.right_bottom());
        let mut m = Mesh::default();
        m.colored_vertex(body.left_top(), c0);
        m.colored_vertex(body.right_top(), c1);
        m.colored_vertex(body.right_bottom(), c1);
        m.colored_vertex(body.left_bottom(), c0);
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(m));
    }
    // Subtle top highlight for depth.
    painter.line_segment(
        [pos2(fill.left() + r as f32, fill.top() + 0.5), pos2(fill.right() - r as f32, fill.top() + 0.5)],
        Stroke::new(1.0, Color32::from_white_alpha(28)),
    );
    if striped {
        let time = ui.input(|i| i.time) as f32;
        let sp = painter.with_clip_rect(fill);
        let step = 10.0;
        let mut x = fill.left() - step * 2.0 + (time * 22.0) % step;
        while x < fill.right() + step {
            sp.line_segment(
                [pos2(x, fill.bottom()), pos2(x + rect.height(), fill.top())],
                Stroke::new(3.0, Color32::from_white_alpha(40)),
            );
            x += step;
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
    }
}

/// Area sparkline with a vertical gradient fill. `samples` newest last.
pub fn sparkline(ui: &mut Ui, samples: &[f32], color: Color32, size: Vec2) -> Response {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect.shrink2(vec2(0.0, 1.5));
    paint_series(&painter, r, samples, color, None, true);
    resp
}

/// Draw one series into `r`, scaled to `max` (or its own max).
pub fn paint_series(painter: &egui::Painter, r: Rect, samples: &[f32], color: Color32, max: Option<f32>, fill: bool) {
    if samples.len() < 2 {
        painter.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, color.gamma_multiply(0.4)));
        return;
    }
    let max = max.unwrap_or_else(|| samples.iter().copied().fold(0.0f32, f32::max)).max(1.0);
    let n = (samples.len() - 1) as f32;
    let pts: Vec<Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(r.left() + r.width() * i as f32 / n, r.bottom() - r.height() * (v / max).clamp(0.0, 1.0)))
        .collect();
    if fill {
        let mut m = Mesh::default();
        for (i, pt) in pts.iter().enumerate() {
            m.colored_vertex(*pt, color.gamma_multiply(0.32));
            m.colored_vertex(pos2(pt.x, r.bottom()), color.gamma_multiply(0.0));
            if i > 0 {
                let b = i as u32 * 2;
                m.add_triangle(b - 2, b - 1, b);
                m.add_triangle(b - 1, b + 1, b);
            }
        }
        painter.add(Shape::mesh(m));
    }
    painter.add(Shape::line(pts, Stroke::new(1.5, color)));
}

/// Small rounded pill with tinted background.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = galley.size() + vec2(14.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.16));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// A chip for categories/tags (outlined).
pub fn chip(ui: &mut Ui, p: &Palette, icon: &str, text: &str) -> Response {
    let font = FontId::proportional(11.0);
    let label = format!("{icon} {text}");
    let galley = ui.painter().layout_no_wrap(label, font, p.text_dim);
    let size = galley.size() + vec2(12.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(9), p.bg2, Stroke::new(1.0, p.border), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, p.text_dim);
    resp
}

/// Frameless icon button that shows a soft pill on hover.
pub fn icon_button(ui: &mut Ui, p: &Palette, icon: &str, tooltip: &str, enabled: bool) -> Response {
    icon_button_sized(ui, p, icon, tooltip, enabled, false, 32.0)
}

pub fn icon_button_sized(
    ui: &mut Ui,
    p: &Palette,
    icon: &str,
    tooltip: &str,
    enabled: bool,
    active: bool,
    size: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let t = ui.ctx().animate_bool_with_time(resp.id.with("h"), hovered || active, 0.12);
    if t > 0.0 {
        let fill = if active { p.accent_soft() } else { p.bg3.gamma_multiply(t) };
        ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    }
    let color = if !enabled {
        p.text_mute
    } else if active {
        p.accent
    } else if hovered {
        p.text
    } else {
        p.text_dim.lerp_to_gamma(p.text, 0.35)
    };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(size * 0.56), color);
    let label = tooltip.split(" (").next().unwrap_or(tooltip).trim_end_matches('…').to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
    let resp = resp.on_hover_text(tooltip);
    if enabled { resp } else { resp.on_disabled_hover_text(tooltip) }
}

/// Primary (accent filled) button.
pub fn primary_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let fg = Color32::WHITE;
    ui.add(
        egui::Button::new(RichText::new(text).color(fg).strong())
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(96.0, 30.0)),
    )
}

pub fn danger_button(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(p.error)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(96.0, 30.0)),
    )
}

pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(text).corner_radius(CornerRadius::same(8)).min_size(vec2(84.0, 30.0)))
}

/// Piece map: buckets pieces into columns; brightness = fraction of pieces we have.
pub fn piece_bar(ui: &mut Ui, p: &Palette, bits: &[u8], count: u32, height: f32) -> Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), p.bg3);
    if count == 0 {
        return resp;
    }
    let cols = (rect.width() as usize).max(1);
    let have = |i: u32| -> bool {
        let byte = (i / 8) as usize;
        bits.get(byte).map(|b| b & (0x80 >> (i % 8)) != 0).unwrap_or(false)
    };
    let mut x = rect.left();
    let colw = rect.width() / cols as f32;
    let mut run_start: Option<(f32, f32)> = None; // (x, alpha) to merge equal columns
    let flush = |painter: &egui::Painter, from: f32, to: f32, a: f32| {
        if a > 0.0 {
            painter.rect_filled(
                Rect::from_min_max(pos2(from, rect.top()), pos2(to, rect.bottom())),
                CornerRadius::ZERO,
                p.accent.gamma_multiply(0.25 + 0.75 * a),
            );
        }
    };
    for c in 0..cols {
        let a = (c as u64 * count as u64 / cols as u64) as u32;
        let b = (((c + 1) as u64 * count as u64 / cols as u64) as u32).max(a + 1).min(count);
        let mut n = 0;
        for i in a..b {
            if have(i) {
                n += 1;
            }
        }
        let frac = n as f32 / (b - a).max(1) as f32;
        let alpha = if frac > 0.0 { frac } else { 0.0 };
        match run_start {
            Some((sx, sa)) if (sa - alpha).abs() < 0.01 => {
                let _ = sx;
            }
            Some((sx, sa)) => {
                flush(&painter, sx, x, sa);
                run_start = Some((x, alpha));
            }
            None => run_start = Some((x, alpha)),
        }
        x += colw;
    }
    if let Some((sx, sa)) = run_start {
        flush(&painter, sx, rect.right(), sa);
    }
    painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, p.border), StrokeKind::Inside);
    resp
}

/// Centered empty-state block with an icon, title, subtitle.
pub fn empty_state(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.28).max(20.0));
        ui.label(RichText::new(icon).size(56.0).color(p.text_mute));
        ui.add_space(10.0);
        ui.label(RichText::new(title).font(super::theme::bold(18.0)).color(p.text));
        ui.add_space(4.0);
        ui.label(RichText::new(subtitle).color(p.text_dim));
        ui.add_space(14.0);
    });
}

/// Compact empty state for panels and tabs: icon, title and hint, centered.
pub fn empty_compact(ui: &mut Ui, p: &Palette, icon: &str, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.22).clamp(8.0, 60.0));
        ui.label(RichText::new(icon).size(30.0).color(p.text_mute));
        ui.add_space(4.0);
        ui.label(RichText::new(title).strong().color(p.text_dim));
        if !hint.is_empty() {
            ui.label(RichText::new(hint).small().color(p.text_mute));
        }
    });
}

/// Card frame used for grouped content (settings, details).
pub fn card(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg2)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
}

/// Draw a dashed rounded rectangle (drag & drop overlay).
pub fn dashed_rect(painter: &egui::Painter, rect: Rect, color: Color32, width: f32) {
    let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    painter.extend(Shape::dashed_line(&pts, Stroke::new(width, color), 10.0, 7.0));
}
