//! Lightweight stacked toast notifications (bottom-right).

use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, CornerRadius, FontId, Id, Order, RichText, Sense, Stroke, vec2};

use crate::engine::ToastKind;
use crate::ui::theme::Palette;

struct Toast {
    id: u64,
    kind: ToastKind,
    text: String,
    created: Instant,
    ttl: Duration,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        let text = text.into();
        // Collapse exact duplicates that arrive in bursts.
        if let Some(t) = self.items.iter_mut().find(|t| t.text == text) {
            t.created = Instant::now();
            return;
        }
        let ttl = match kind {
            ToastKind::Error => Duration::from_secs(8),
            ToastKind::Warning => Duration::from_secs(6),
            _ => Duration::from_secs(4),
        };
        self.next += 1;
        self.items.push(Toast { id: self.next, kind, text, created: Instant::now(), ttl });
        if self.items.len() > 6 {
            self.items.remove(0);
        }
    }

    pub fn show(&mut self, ctx: &Context, p: &Palette) {
        if self.items.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 40.0;
        let mut remove = Vec::new();
        let now = Instant::now();
        for t in self.items.iter().rev() {
            let age = now.duration_since(t.created);
            let hovered_id = Id::new("toast").with(t.id);
            let hovered = ctx.data(|d| d.get_temp::<bool>(hovered_id)).unwrap_or(false);
            if age > t.ttl && !hovered {
                remove.push(t.id);
                continue;
            }
            let fade_in = (age.as_secs_f32() / 0.18).min(1.0);
            let (accent, icon) = match t.kind {
                ToastKind::Info => (p.accent, egui_phosphor::regular::INFO),
                ToastKind::Success => (p.success, egui_phosphor::regular::CHECK_CIRCLE),
                ToastKind::Warning => (p.warn, egui_phosphor::regular::WARNING),
                ToastKind::Error => (p.error, egui_phosphor::regular::X_CIRCLE),
            };
            let area = egui::Area::new(Id::new("toast_area").with(t.id))
                .order(Order::Tooltip)
                .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0 + (1.0 - fade_in) * 40.0, y - screen.bottom()))
                .interactable(true);
            let resp = area.show(ctx, |ui| {
                egui::Frame::new()
                    .fill(p.bg2)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(10))
                    .shadow(ui.visuals().popup_shadow)
                    .inner_margin(egui::Margin { left: 14, right: 12, top: 10, bottom: 10 })
                    .show(ui, |ui| {
                        ui.set_max_width(380.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).font(FontId::proportional(18.0)).color(accent));
                            ui.add(egui::Label::new(RichText::new(&t.text).color(p.text)).wrap());
                        });
                    })
                    .response
            });
            let rect = resp.inner.rect;
            ui_accent_bar(ctx, rect, accent, t.id);
            let r = resp.response.interact(Sense::click());
            ctx.data_mut(|d| d.insert_temp(hovered_id, r.hovered()));
            if r.clicked() {
                remove.push(t.id);
            }
            y -= rect.height() + 10.0;
        }
        self.items.retain(|t| !remove.contains(&t.id));
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn ui_accent_bar(ctx: &Context, rect: egui::Rect, color: Color32, id: u64) {
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("toast_area").with(id)));
    let bar = egui::Rect::from_min_max(rect.left_top() + vec2(0.0, 8.0), egui::pos2(rect.left() + 3.0, rect.bottom() - 8.0));
    painter.rect_filled(bar, CornerRadius::same(2), color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_collapse_and_list_is_capped() {
        let mut t = Toasts::default();
        t.push(ToastKind::Info, "same");
        t.push(ToastKind::Info, "same");
        assert_eq!(t.items.len(), 1);
        for i in 0..10 {
            t.push(ToastKind::Info, format!("n{i}"));
        }
        assert_eq!(t.items.len(), 6);
        assert_eq!(t.items.last().unwrap().text, "n9");
    }

    #[test]
    fn errors_stay_longer() {
        let mut t = Toasts::default();
        t.push(ToastKind::Error, "e");
        t.push(ToastKind::Info, "i");
        assert!(t.items[0].ttl > t.items[1].ttl);
    }
}
