//! Lightweight stacked toast notifications (bottom-right).

use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, CornerRadius, FontId, Id, Order, RichText, Stroke, vec2};

use crate::engine::ToastKind;
use crate::ui::theme::Palette;

/// Something a toast button can do; resolved by the app.
#[derive(Clone, Debug, PartialEq)]
pub enum ToastAction {
    Undo(Vec<crate::engine::types::InfoHash>),
    OpenFolder(std::path::PathBuf),
    OpenFile(std::path::PathBuf),
    Show(crate::engine::types::InfoHash),
}

struct Toast {
    id: u64,
    kind: ToastKind,
    text: String,
    created: Instant,
    ttl: Duration,
    actions: Vec<(String, ToastAction)>,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.push_with(kind, text, Vec::new());
    }

    /// A toast with buttons. Toasts with actions stay a little longer.
    pub fn push_with(&mut self, kind: ToastKind, text: impl Into<String>, actions: Vec<(String, ToastAction)>) {
        let text = text.into();
        // The log window doubles as notification history.
        match kind {
            ToastKind::Error => tracing::error!(target: "corrent::notice", "{text}"),
            ToastKind::Warning => tracing::warn!(target: "corrent::notice", "{text}"),
            _ => tracing::info!(target: "corrent::notice", "{text}"),
        }
        // Collapse exact duplicates that arrive in bursts, keeping the newest actions
        // (Undo lists are merged so every removal stays undoable).
        if let Some(t) = self.items.iter_mut().find(|t| t.text == text) {
            t.created = Instant::now();
            let mut merged = actions;
            for (label, a) in &mut merged {
                if let ToastAction::Undo(new) = a
                    && let Some((_, ToastAction::Undo(old))) = t.actions.iter().find(|(l, _)| l == label)
                {
                    for h in old {
                        if !new.contains(h) {
                            new.push(*h);
                        }
                    }
                }
            }
            t.actions = merged;
            return;
        }
        let mut ttl = match kind {
            ToastKind::Error => Duration::from_secs(8),
            ToastKind::Warning => Duration::from_secs(6),
            _ => Duration::from_secs(4),
        };
        if !actions.is_empty() {
            ttl += Duration::from_secs(12);
        }
        self.next += 1;
        self.items.push(Toast { id: self.next, kind, text, created: Instant::now(), ttl, actions });
        // Over the cap, drop the oldest toast without actions first.
        while self.items.len() > 6 {
            let idx = self.items.iter().position(|t| t.actions.is_empty()).unwrap_or(0);
            self.items.remove(idx);
        }
    }

    /// Draw toasts; returns the actions the user clicked.
    pub fn show(&mut self, ctx: &Context, p: &Palette) -> Vec<ToastAction> {
        let mut clicked = Vec::new();
        if self.items.is_empty() {
            return clicked;
        }
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 40.0;
        let mut remove = Vec::new();
        let mut remove_now: Vec<u64> = Vec::new();
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
                        ui.set_max_width(400.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).font(FontId::proportional(18.0)).color(accent));
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(RichText::new(&t.text).color(p.text)).wrap());
                                if !t.actions.is_empty() {
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        for (label, action) in &t.actions {
                                            let b = egui::Button::new(RichText::new(label).color(p.accent).strong())
                                                .fill(p.accent_soft())
                                                .stroke(Stroke::NONE)
                                                .corner_radius(CornerRadius::same(6));
                                            if ui.add(b).clicked() {
                                                clicked.push(action.clone());
                                                remove_now.push(t.id);
                                            }
                                        }
                                    });
                                }
                            });
                            if ui
                                .add(egui::Button::new(RichText::new(egui_phosphor::regular::X).color(p.text_dim)).frame(false))
                                .on_hover_text("Dismiss")
                                .clicked()
                            {
                                remove_now.push(t.id);
                            }
                        });
                    })
                    .response
            });
            let rect = resp.inner.rect;
            ui_accent_bar(ctx, rect, accent, t.id);
            let hovered_now = ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p));
            ctx.data_mut(|d| d.insert_temp(hovered_id, hovered_now));
            y -= rect.height() + 10.0;
        }
        self.items.retain(|t| !remove.contains(&t.id) && !remove_now.contains(&t.id));
        ctx.request_repaint_after(Duration::from_millis(250));
        clicked
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
        t.push_with(ToastKind::Info, "undo me", vec![("Undo".into(), ToastAction::Undo(vec![]))]);
        for i in 0..10 {
            t.push(ToastKind::Info, format!("n{i}"));
        }
        assert_eq!(t.items.len(), 6);
        assert!(t.items.iter().any(|x| x.text == "undo me"), "actionable toasts survive the cap");
        assert_eq!(t.items.last().unwrap().text, "n9");
    }

    #[test]
    fn collapsed_undo_merges_hashes() {
        use crate::engine::types::InfoHash;
        let mut t = Toasts::default();
        t.push_with(ToastKind::Info, "Removed 2 torrents", vec![("Undo".into(), ToastAction::Undo(vec![InfoHash([1; 20])]))]);
        t.push_with(ToastKind::Info, "Removed 2 torrents", vec![("Undo".into(), ToastAction::Undo(vec![InfoHash([2; 20])]))]);
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].actions[0].1, ToastAction::Undo(vec![InfoHash([2; 20]), InfoHash([1; 20])]));
    }

    #[test]
    fn errors_stay_longer() {
        let mut t = Toasts::default();
        t.push(ToastKind::Error, "e");
        t.push(ToastKind::Info, "i");
        assert!(t.items[0].ttl > t.items[1].ttl);
    }
}
