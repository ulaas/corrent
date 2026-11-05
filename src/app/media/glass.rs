//! The look of everything over the picture: panels and overlays on a blurred copy of the video, and
//! the small set of controls they are made of. All drawn here rather than styled egui widgets, so
//! a panel reads like the meters do. `Tone` lets the same controls sit on the app's own surfaces
//! (Settings, the mini player), where there is no picture to blur.

use egui::epaint::RectShape;
use egui::{
    Color32, ColorImage, Context, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    WidgetInfo, WidgetType, pos2, vec2,
};

use crate::ui::theme::Palette;

const RADIUS: u8 = 10;
const PAD: f32 = 12.0;
const ROW: f32 = 30.0;
const TEXT: f32 = 13.0;
/// Over the blur: enough to keep white text readable on a bright scene, and the scene still shows.
const TINT_OVER_BLUR: u8 = 150;
/// With nothing blurred behind it, text has only this to stand on.
const TINT_PLAIN: u8 = 205;

/// The colours the controls draw with.
#[derive(Clone, Copy)]
pub struct Tone {
    pub text: Color32,
    pub dim: Color32,
    /// Behind a chip, a graph, a text box.
    pub faint: Color32,
    pub hover: Color32,
    /// An unfilled slider or switch.
    pub rail: Color32,
    pub accent: Color32,
    pub warn: Color32,
}

impl Tone {
    /// Over the picture, whatever the app's theme is.
    pub fn glass(p: &Palette) -> Self {
        Self {
            text: Color32::WHITE,
            dim: Color32::from_white_alpha(190),
            faint: Color32::from_white_alpha(20),
            hover: Color32::from_white_alpha(30),
            rail: Color32::from_white_alpha(70),
            accent: p.accent,
            warn: p.warn,
        }
    }

    /// On the app's own surfaces.
    pub fn theme(p: &Palette) -> Self {
        Self {
            text: p.text,
            dim: p.text_dim,
            faint: p.bg2,
            hover: p.bg3,
            rail: p.border_hi,
            accent: p.accent,
            warn: p.warn,
        }
    }
}

/// What goes behind a panel: the blurred picture when there is one, a darker tint when not (no
/// video yet, casting, or a GPU without framebuffer blits).
#[derive(Clone, Copy)]
pub struct Backdrop {
    tex: Option<egui::TextureId>,
    /// What the texture covers on screen.
    window: Rect,
}

impl Backdrop {
    pub fn new(tex: Option<egui::TextureId>, window: Rect) -> Self {
        Self { tex, window }
    }

    pub fn plain() -> Self {
        Self { tex: None, window: Rect::NOTHING }
    }

    /// The texture's part under `rect`, so the blur lines up with the picture around the panel.
    fn uv(&self, rect: Rect) -> Rect {
        let at = |p: Pos2| {
            pos2((p.x - self.window.left()) / self.window.width(), (p.y - self.window.top()) / self.window.height())
        };
        Rect::from_min_max(at(rect.min), at(rect.max))
    }

    pub fn shapes(&self, rect: Rect) -> Vec<Shape> {
        let radius = CornerRadius::same(RADIUS);
        let mut out = Vec::with_capacity(3);
        let tint = match self.tex {
            Some(tex) => {
                out.push(RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, self.uv(rect)).into());
                TINT_OVER_BLUR
            }
            None => TINT_PLAIN,
        };
        out.push(Shape::rect_filled(rect, radius, Color32::from_black_alpha(tint)));
        out.push(Shape::rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_white_alpha(38)), StrokeKind::Inside));
        out
    }
}

/// Dark surroundings for what still is an egui widget (the text boxes).
fn style(ui: &mut Ui, tone: &Tone) {
    let v = ui.visuals_mut();
    v.override_text_color = Some(tone.text);
    v.text_edit_bg_color = Some(tone.faint);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, tone.rail);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, tone.dim);
    ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
}

/// A panel with its bottom right corner at `anchor`. Gives back where it landed.
pub fn panel<R>(
    ctx: &Context,
    id: Id,
    anchor: Pos2,
    width: f32,
    backdrop: &Backdrop,
    tone: &Tone,
    add: impl FnOnce(&mut Ui) -> R,
) -> (Rect, R) {
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .show(ctx, |ui| {
            let inner = card(ui, backdrop, tone, width, add);
            (inner.response.rect, inner.inner)
        })
        .inner
}

/// The panel's surface, in place: for what lives in the player's own layer (the countdown).
pub fn card<R>(
    ui: &mut Ui,
    backdrop: &Backdrop,
    tone: &Tone,
    width: f32,
    add: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let behind = ui.painter().add(Shape::Noop);
    style(ui, tone);
    let inner = egui::Frame::new().inner_margin(PAD).show(ui, |ui| {
        ui.set_width(width - 2.0 * PAD);
        add(ui)
    });
    ui.painter().set(behind, Shape::Vec(backdrop.shapes(inner.response.rect)));
    inner
}

/// A panel's title and its close button; true when that was clicked.
pub fn header(ui: &mut Ui, tone: &Tone, title: &str) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, title, FontId::proportional(14.0), tone.text);
    let at = Rect::from_center_size(pos2(r.right() - 11.0, r.center().y), vec2(22.0, 22.0));
    let close = ui.interact(at, ui.id().with(("panel-close", title)), Sense::click());
    let close = close.on_hover_text(t!("Close"));
    close.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, t!("Close")));
    if close.hovered() {
        ui.painter().rect_filled(at, CornerRadius::same(6), tone.hover);
    }
    ui.painter().text(
        at.center(),
        egui::Align2::CENTER_CENTER,
        egui_phosphor::regular::X,
        FontId::proportional(14.0),
        tone.dim,
    );
    close.clicked()
}

/// A quiet label over a group of controls.
pub fn section(ui: &mut Ui, tone: &Tone, text: &str) {
    ui.add_space(4.0);
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), tone.dim);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 2.0), Sense::hover());
    ui.painter().galley(r.min, g, tone.dim);
}

pub fn separator(ui: &mut Ui, tone: &Tone) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, tone.faint));
}

fn focus_ring(ui: &Ui, resp: &Response, rect: Rect, tone: &Tone) {
    if resp.has_focus() {
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.5, tone.accent), StrokeKind::Outside);
    }
}

/// A row the whole of which answers: hover shading and a focus ring come with it.
fn row(ui: &mut Ui, tone: &Tone) -> (Rect, Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let shade = rect.expand2(vec2(4.0, 0.0));
    if resp.hovered() {
        ui.painter().rect_filled(shade, CornerRadius::same(6), tone.hover);
    }
    focus_ring(ui, &resp, shade, tone);
    (rect, resp)
}

/// A label with a switch on the right; the whole row flips it.
pub fn switch(ui: &mut Ui, tone: &Tone, on: &mut bool, label: &str) -> Response {
    let enabled = ui.is_enabled();
    let (rect, mut resp) = row(ui, tone);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, value, label));
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(TEXT), tone.text);
    let k = crate::ui::motion::toggle(ui.ctx(), resp.id, value, crate::ui::motion::MICRO);
    let track = Rect::from_min_size(pos2(rect.right() - 30.0, rect.center().y - 8.0), vec2(30.0, 16.0));
    ui.painter().rect_filled(track, CornerRadius::same(8), if value { tone.accent } else { tone.rail });
    ui.painter().circle_filled(pos2(track.left() + 8.0 + 14.0 * k, track.center().y), 6.0, Color32::WHITE);
    resp
}

/// A label with `−  value  +`. Gives -1 or +1 for a click on either.
pub fn stepper(ui: &mut Ui, tone: &Tone, label: &str, value: &str) -> i8 {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(TEXT), tone.text);
    let plus = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(24.0, 24.0));
    let minus = Rect::from_center_size(pos2(rect.right() - 92.0, rect.center().y), vec2(24.0, 24.0));
    ui.painter().text(
        pos2((plus.left() + minus.right()) / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        value,
        FontId::monospace(12.0),
        tone.text,
    );
    let mut step = 0;
    for (at, icon, name, delta) in
        [(minus, egui_phosphor::regular::MINUS, "minus", -1), (plus, egui_phosphor::regular::PLUS, "plus", 1)]
    {
        let resp = ui.interact(at, ui.id().with((label, name)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{label} {name}")));
        if resp.hovered() {
            ui.painter().rect_filled(at, CornerRadius::same(6), tone.hover);
        }
        focus_ring(ui, &resp, at, tone);
        ui.painter().text(at.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(13.0), tone.dim);
        if resp.clicked() {
            step = delta;
        }
    }
    step
}

fn pill(ui: &mut Ui, tone: &Tone, label: &str, selected: bool) -> Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(12.5), tone.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 26.0), Sense::click());
    let fill = match (selected, resp.hovered()) {
        (true, _) => tone.accent,
        (false, true) => tone.hover,
        (false, false) => tone.faint,
    };
    ui.painter().rect_filled(rect, CornerRadius::same(13), fill);
    focus_ring(ui, &resp, rect, tone);
    let color = if selected { Color32::WHITE } else { tone.text };
    let at = rect.center() - g.size() / 2.0;
    ui.painter().galley(at, g, color);
    resp
}

/// One of several (or none), wrapping to the panel's width. Gives the one clicked.
pub fn chips<S: AsRef<str>>(ui: &mut Ui, tone: &Tone, items: &[(S, bool)]) -> Option<usize> {
    let mut hit = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, (label, selected)) in items.iter().enumerate() {
            let label = label.as_ref();
            let resp = pill(ui, tone, label, *selected);
            let value = *selected;
            let enabled = ui.is_enabled();
            resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, enabled, value, label));
            if resp.clicked() {
                hit = Some(i);
            }
        }
    });
    hit
}

/// A button for an action.
pub fn button(ui: &mut Ui, tone: &Tone, label: &str) -> Response {
    let resp = pill(ui, tone, label, false);
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    resp
}

/// A row in a list: a track, a device. A check on the right when it's the current one.
pub fn item(ui: &mut Ui, tone: &Tone, icon: Option<&str>, label: &str, selected: bool) -> Response {
    let enabled = ui.is_enabled();
    let (rect, resp) = row(ui, tone);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, enabled, selected, label));
    let mut left = rect.left();
    if let Some(icon) = icon {
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, icon, FontId::proportional(15.0), tone.dim);
        left += 22.0;
    }
    let width = (rect.right() - left - if selected { 22.0 } else { 0.0 }).max(10.0);
    let g = egui::WidgetText::from(label).color(tone.text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        width,
        FontId::proportional(TEXT),
    );
    let top = rect.center().y - g.size().y / 2.0;
    ui.painter().galley(pos2(left, top), g, tone.text);
    if selected {
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            egui_phosphor::regular::CHECK,
            FontId::proportional(14.0),
            tone.accent,
        );
    }
    resp
}

/// A small icon that does something, for the end of a row.
pub fn icon_button(ui: &mut Ui, tone: &Tone, icon: &str, name: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, name));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), tone.hover);
    }
    focus_ring(ui, &resp, rect, tone);
    let color = if enabled { tone.dim } else { tone.rail };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon, FontId::proportional(14.0), color);
    resp
}

/// Takes the arrows while focused, so they move this control rather than the focus (and seek).
pub fn claim_arrows(ui: &Ui, resp: &Response, horizontal: bool) {
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter { vertical_arrows: !horizontal, horizontal_arrows: horizontal, ..Default::default() },
            );
        });
    }
}

/// `label  ───●───  value`: drag anywhere on the track, double-click for `default`, arrows when
/// focused.
#[allow(clippy::too_many_arguments)]
pub fn slider(
    ui: &mut Ui,
    tone: &Tone,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    default: f32,
    text: String,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let (lo, hi) = (*range.start(), *range.end());
    let track = Rect::from_min_max(
        pos2(rect.left() + 92.0, rect.center().y - 2.0),
        pos2(rect.right() - 56.0, rect.center().y + 2.0),
    );
    let resp = ui.interact(track.expand2(vec2(6.0, 9.0)), ui.id().with(("slider", label)), Sense::click_and_drag());
    let snap = |v: f32| ((v / step).round() * step).clamp(lo, hi);
    let mut next = *value;
    if let Some(p) = resp.interact_pointer_pos() {
        next = snap(lo + (hi - lo) * ((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
    }
    if resp.double_clicked() {
        next = default;
    }
    claim_arrows(ui, &resp, true);
    if resp.has_focus() {
        let (l, r) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
        next = snap(next + step * (f32::from(u8::from(r)) - f32::from(u8::from(l))));
    }
    *value = next;
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::slider(enabled, f64::from(next), label));
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(12.0), tone.dim);
    ui.painter().rect_filled(track, CornerRadius::same(2), tone.rail);
    let x = track.left() + track.width() * ((next - lo) / (hi - lo)).clamp(0.0, 1.0);
    ui.painter().rect_filled(
        Rect::from_min_max(track.min, pos2(x, track.bottom())),
        CornerRadius::same(2),
        tone.accent,
    );
    ui.painter().circle_filled(pos2(x, track.center().y), if resp.hovered() { 7.0 } else { 6.0 }, Color32::WHITE);
    focus_ring(ui, &resp, track.expand(6.0), tone);
    ui.painter().text(rect.right_center(), egui::Align2::RIGHT_CENTER, text, FontId::monospace(11.0), tone.text);
    resp
}

/// A row that opens what is under it. Closed until asked for.
pub fn disclosure(ui: &mut Ui, tone: &Tone, title: &str, add: impl FnOnce(&mut Ui)) {
    let id = ui.id().with(("disclosure", title));
    let mut open: bool = ui.data(|d| d.get_temp(id)).unwrap_or(false);
    let enabled = ui.is_enabled();
    let (rect, resp) = row(ui, tone);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::CollapsingHeader, enabled, open, title));
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, title, FontId::proportional(TEXT), tone.text);
    ui.painter().text(
        rect.right_center(),
        egui::Align2::RIGHT_CENTER,
        if open { egui_phosphor::regular::CARET_UP } else { egui_phosphor::regular::CARET_DOWN },
        FontId::proportional(13.0),
        tone.dim,
    );
    if open {
        add(ui);
    }
}

/// A click away from the panel closes it, except on the row of buttons (those that open panels
/// decide for themselves) or when the panel is one that stays while you listen.
pub fn clicked_away(click: Option<Pos2>, panel: Rect, buttons: Rect, stays: bool) -> bool {
    !stays && click.is_some_and(|at| !panel.contains(at) && !buttons.contains(at))
}

/// The software renderer's blur: `frame` (RGBA, `size`) placed at `at` (x, y, w, h) in a window of
/// `window` pixels, black around it, shrunk and smoothed. Small enough to run on each new frame.
#[expect(clippy::indexing_slicing, reason = "positions are clamped to the buffers they index, sized just above")]
pub fn soft_backdrop(frame: &[u8], size: [usize; 2], window: [usize; 2], at: [usize; 4]) -> ColorImage {
    const SCALE: usize = 12;
    let w = (window[0] / SCALE).clamp(4, 200);
    let h = (window[1] / SCALE).clamp(4, 200);
    let mut px = vec![[0f32; 3]; w * h];
    let [fw, fh] = size;
    if fw > 0 && fh > 0 && frame.len() >= fw * fh * 4 && window[0] > 0 && window[1] > 0 {
        for y in 0..h {
            for x in 0..w {
                let wx = (x * 2 + 1) * window[0] / (w * 2);
                let wy = (y * 2 + 1) * window[1] / (h * 2);
                if wx < at[0] || wy < at[1] || wx >= at[0] + at[2] || wy >= at[1] + at[3] {
                    continue;
                }
                let sx = (wx - at[0]) * fw / at[2].max(1);
                let sy = (wy - at[1]) * fh / at[3].max(1);
                let i = (sy.min(fh - 1) * fw + sx.min(fw - 1)) * 4;
                px[y * w + x] = [f32::from(frame[i]), f32::from(frame[i + 1]), f32::from(frame[i + 2])];
            }
        }
    }
    let mut next = px.clone();
    for _ in 0..3 {
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0f32; 3];
                let mut n = 0f32;
                for yy in y.saturating_sub(2)..=(y + 2).min(h - 1) {
                    for xx in x.saturating_sub(2)..=(x + 2).min(w - 1) {
                        let c = px[yy * w + xx];
                        sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                        n += 1.0;
                    }
                }
                next[y * w + x] = [sum[0] / n, sum[1] / n, sum[2] / n];
            }
        }
        std::mem::swap(&mut px, &mut next);
    }
    let bytes: Vec<u8> = px.iter().flat_map(|c| [c[0] as u8, c[1] as u8, c[2] as u8, 255]).collect();
    ColorImage::from_rgba_unmultiplied([w, h], &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blur_lines_up_with_the_picture_around_the_panel() {
        let window = Rect::from_min_size(pos2(100.0, 50.0), vec2(1000.0, 500.0));
        let b = Backdrop::new(None, window);
        let uv = b.uv(Rect::from_min_max(pos2(600.0, 300.0), pos2(1100.0, 550.0)));
        assert_eq!((uv.min, uv.max), (pos2(0.5, 0.5), pos2(1.0, 1.0)));
    }

    #[test]
    fn without_a_blur_there_is_only_the_tint() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0));
        assert_eq!(Backdrop::plain().shapes(rect).len(), 2);
        let tex = egui::TextureId::Managed(1);
        assert_eq!(Backdrop::new(Some(tex), rect).shapes(rect).len(), 3);
    }

    #[test]
    fn a_click_away_closes_but_not_on_the_buttons_or_the_panel() {
        let panel = Rect::from_min_size(pos2(100.0, 100.0), vec2(100.0, 100.0));
        let buttons = Rect::from_min_size(pos2(0.0, 300.0), vec2(400.0, 50.0));
        assert!(clicked_away(Some(pos2(10.0, 10.0)), panel, buttons, false));
        assert!(!clicked_away(Some(pos2(150.0, 150.0)), panel, buttons, false));
        assert!(!clicked_away(Some(pos2(150.0, 320.0)), panel, buttons, false));
        assert!(!clicked_away(Some(pos2(10.0, 10.0)), panel, buttons, true), "sound stays");
        assert!(!clicked_away(None, panel, buttons, false));
    }

    #[test]
    fn the_soft_blur_keeps_the_letterbox_dark() {
        // A red picture in the middle third of the height.
        let frame = [255u8, 0, 0, 255].repeat(16 * 9);
        let img = soft_backdrop(&frame, [16, 9], [1200, 900], [0, 300, 1200, 300]);
        let (w, h) = (img.size[0], img.size[1]);
        assert_eq!((w, h), (100, 75));
        let at = |x: usize, y: usize| img.pixels[y * w + x];
        assert!(at(50, 37).r() > 240, "{:?}", at(50, 37));
        assert_eq!(at(50, 0).r(), 0);
        assert_eq!(at(50, h - 1).r(), 0);
    }

    #[test]
    fn the_soft_blur_takes_odd_sizes() {
        for (size, window, at) in
            [([0, 0], [100, 100], [0, 0, 100, 100]), ([4, 4], [0, 0], [0, 0, 0, 0]), ([4, 4], [3, 3], [0, 0, 3, 3])]
        {
            let img = soft_backdrop(&vec![9u8; size[0] * size[1] * 4], size, window, at);
            assert!(img.size[0] >= 4 && img.size[1] >= 4);
        }
    }
}
