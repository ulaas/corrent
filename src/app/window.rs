//! Window lifecycle: tray actions, close/minimize to tray, exit confirmation, notifications.

use egui::{Context, ViewportCommand};

use super::CorrentApp;
use super::dialogs::Modal;
use super::tray::{Tray, TrayAction};
use crate::engine::EngineCommand;
use crate::util::fmt_speed;

impl CorrentApp {
    pub(super) fn init_tray(&mut self, ctx: &Context) {
        if self.settings.behavior.show_tray_icon {
            self.tray = Tray::new(ctx);
        }
        // Never leave the user without a way back to a hidden window.
        if self.window_hidden && self.tray.is_none() {
            self.window_hidden = false;
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    }

    pub fn hide_to_tray(&mut self, ctx: &Context) {
        self.window_hidden = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    pub(super) fn window_logic(&mut self, ctx: &Context) {
        // Tray toggled in settings.
        if self.settings.behavior.show_tray_icon && self.tray.is_none() && !self.tray_failed {
            self.tray = Tray::new(ctx);
            self.tray_failed = self.tray.is_none();
        } else if !self.settings.behavior.show_tray_icon && self.tray.is_some() {
            self.tray = None;
            if self.window_hidden {
                self.show_window(ctx);
            }
        }

        let actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for a in actions {
            match a {
                TrayAction::ToggleWindow => {
                    if self.window_hidden {
                        self.show_window(ctx);
                    } else {
                        self.hide_to_tray(ctx);
                    }
                }
                TrayAction::ShowWindow => self.show_window(ctx),
                TrayAction::PauseAll => {
                    self.engine.send(EngineCommand::PauseAll);
                }
                TrayAction::ResumeAll => {
                    self.engine.send(EngineCommand::ResumeAll);
                }
                TrayAction::AltSpeed => {
                    self.engine.send(EngineCommand::SetAltSpeed(!self.snap.session.alt_speed));
                }
                TrayAction::Settings => {
                    self.show_window(ctx);
                    self.open_settings();
                }
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
        if let Some(t) = &mut self.tray {
            let units = self.settings.appearance.speed_units;
            let s = &self.snap.session;
            let active = self.snap.torrents.iter().filter(|t| t.down_bps > 0 || t.up_bps > 0).count();
            t.update(&fmt_speed(s.down_bps, units), &fmt_speed(s.up_bps, units), active, s.alt_speed);
        }

        // Minimize to tray.
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        if minimized && self.settings.behavior.minimize_to_tray && self.tray.is_some() && !self.window_hidden {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            self.hide_to_tray(ctx);
        }

        // Close button.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.settings.behavior.close_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            } else if self.settings.behavior.confirm_exit_while_active
                && self.snap.torrents.iter().any(|t| t.down_bps > 0 || (t.state.is_active() && t.progress < 1.0))
            {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.modal = Some(Modal::ConfirmExit);
            }
        }
    }

    pub(super) fn notify(&self, title: &str, body: &str) {
        static NOTIFIER: std::sync::OnceLock<super::notify::Notifier> = std::sync::OnceLock::new();
        #[cfg(windows)]
        let paths = self.paths.clone();
        let notifier = NOTIFIER.get_or_init(move || {
            super::notify::Notifier::new(move |title, body| {
                let mut n = notify_rust::Notification::new();
                n.appname("Corrent").summary(title).body(body);
                // Windows: show as "Corrent" with its icon rather than the PowerShell default.
                #[cfg(windows)]
                if let Some(id) = crate::integration::app_identity::ensure(&paths) {
                    n.app_id(id);
                }
                #[cfg(all(unix, not(target_os = "macos")))]
                n.icon("corrent");
                if let Err(e) = n.show() {
                    tracing::debug!("notification failed: {e}");
                }
            })
        });
        notifier.push(title, body);
    }
}
