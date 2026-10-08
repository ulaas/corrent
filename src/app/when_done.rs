//! "When downloads finish" (quit / sleep / shut down with a cancellable countdown) and update
//! checks.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Align, Context, Id, Layout, RichText, ViewportCommand};
use egui_phosphor::regular as icons;

use super::{CorrentApp, toasts};
use crate::engine::ToastKind;
use crate::integration::power::WhenDone;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{danger_button, secondary_button};
use crate::updates::Release;

pub const COUNTDOWN: Duration = Duration::from_secs(30);

type UpdateReply = (bool, Result<Option<Release>, String>);

#[derive(Default)]
pub struct WhenDoneState {
    pub action: WhenDone,
    /// Something was downloading since the action was chosen.
    had_active: bool,
    pub countdown: Option<Instant>,
    update_rx: Option<mpsc::Receiver<UpdateReply>>,
    auto_checked: bool,
}

impl CorrentApp {
    pub fn set_when_done(&mut self, action: WhenDone) {
        self.when_done.action = action;
        self.when_done.had_active = crate::engine::logic::downloads_active(&self.snap.torrents);
        self.when_done.countdown = None;
    }

    pub(super) fn when_done_logic(&mut self, ctx: &Context) {
        let w = &mut self.when_done;
        if w.action == WhenDone::Nothing || w.countdown.is_some() {
            return;
        }
        let active = crate::engine::logic::downloads_active(&self.snap.torrents);
        if active {
            w.had_active = true;
        } else if w.had_active {
            w.had_active = false;
            w.countdown = Some(Instant::now());
            ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
    }

    pub(super) fn when_done_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(started) = self.when_done.countdown else { return };
        let left = COUNTDOWN.saturating_sub(started.elapsed());
        let action = self.when_done.action;
        let mut cancel = false;
        let mut now = left.is_zero();
        egui::Modal::new(Id::new("when_done")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::POWER).size(22.0).color(p.accent));
                ui.label(RichText::new("All downloads finished").font(theme::title()).color(p.text));
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("{} in {} seconds.", action.verb(), left.as_secs() + 1))
                    .size(theme::type_scale::HEADING)
                    .color(p.text),
            );
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if danger_button(ui, p, "Now").clicked() {
                    now = true;
                }
                if secondary_button(ui, "Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if cancel {
            self.when_done =
                WhenDoneState { update_rx: self.when_done.update_rx.take(), auto_checked: true, ..Default::default() };
            self.toasts.push(ToastKind::Info, "Cancelled; Corrent will keep running.");
            return;
        }
        if !now {
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }
        // One-shot: reset before acting.
        self.when_done.action = WhenDone::Nothing;
        self.when_done.countdown = None;
        match action {
            WhenDone::Nothing => {}
            WhenDone::Quit => self.quit_now(ctx),
            WhenDone::Sleep => {
                if let Err(e) = crate::integration::power::perform(WhenDone::Sleep) {
                    self.toasts.push(ToastKind::Error, format!("Couldn't put the computer to sleep: {e}"));
                }
            }
            WhenDone::Shutdown => match crate::integration::power::perform(WhenDone::Shutdown) {
                Ok(()) => self.quit_now(ctx),
                Err(e) => self.toasts.push(ToastKind::Error, format!("Couldn't shut down: {e}")),
            },
        }
    }

    fn quit_now(&mut self, ctx: &Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    // ------------------------------------------------------------------ updates

    pub fn check_for_updates(&mut self, manual: bool) {
        if self.when_done.update_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (proxy, ctx) = (self.settings.connection.proxy_url(), self.ctx.clone());
        self.rt.spawn(async move {
            let _ = tx.send((manual, crate::updates::check(proxy).await));
            ctx.request_repaint();
        });
        self.when_done.update_rx = Some(rx);
        self.settings.updates.last_check = crate::util::now_unix();
        self.save_settings();
    }

    pub(super) fn poll_updates(&mut self) {
        if !self.when_done.auto_checked {
            self.when_done.auto_checked = true;
            let u = &self.settings.updates;
            if u.check_automatically && crate::util::now_unix() - u.last_check > 24 * 3600 {
                self.check_for_updates(false);
            }
        }
        let Some(rx) = &self.when_done.update_rx else { return };
        let Ok((manual, result)) = rx.try_recv() else { return };
        self.when_done.update_rx = None;
        match result {
            Ok(Some(r)) if manual || r.version != self.settings.updates.skip_version => {
                self.toasts.push_with(
                    ToastKind::Info,
                    format!("Corrent {} is available (you have {}).", r.version, env!("CARGO_PKG_VERSION")),
                    vec![
                        ("Download".into(), toasts::ToastAction::OpenUrl(r.url)),
                        ("Skip this version".into(), toasts::ToastAction::SkipVersion(r.version)),
                    ],
                );
            }
            Ok(_) if manual => self.toasts.push(ToastKind::Success, "Corrent is up to date."),
            Ok(_) => {}
            Err(e) if manual => self.toasts.push(ToastKind::Error, format!("Couldn't check for updates: {e}")),
            Err(e) => tracing::info!("update check failed: {e}"),
        }
    }
}
