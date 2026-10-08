//! Single-instance forwarding, window focus and "default app" registration UI.

use egui::{Align, Context, Layout, RichText, Ui};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::ToastKind;
use crate::integration::os_handler::{self, HandlerState};
use crate::integration::single_instance::IpcMessage;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

impl CorrentApp {
    pub(super) fn init_integration(self) -> Self {
        self.refresh_handler_status(true);
        self
    }

    /// Query registration status off the UI thread (it may shell out on Linux).
    /// With `repair`, silently re-register when the executable moved.
    pub fn refresh_handler_status(&self, repair: bool) {
        let slot = self.handler_status.clone();
        let b = self.settings.behavior.clone();
        std::thread::spawn(move || {
            let mut st = os_handler::status();
            if repair && b.keep_handler_registration {
                let fix_magnet = b.register_magnet_handler && st.magnet == HandlerState::OtherPath;
                let fix_torrent = b.register_torrent_handler && st.torrent == HandlerState::OtherPath;
                if (fix_magnet || fix_torrent) && os_handler::register(fix_magnet, fix_torrent).is_ok() {
                    tracing::info!("re-registered handlers for the new executable location");
                    st = os_handler::status();
                }
            }
            if let Ok(mut s) = slot.lock() {
                *s = Some(st);
            }
        });
    }

    pub fn set_handlers(&mut self, magnet: bool, torrent: bool) {
        let b = &mut self.settings.behavior;
        let (old_m, old_t) = (b.register_magnet_handler, b.register_torrent_handler);
        b.register_magnet_handler = magnet;
        b.register_torrent_handler = torrent;
        let result = if magnet || torrent { os_handler::register(magnet, torrent) } else { Ok(()) };
        let unreg = os_handler::unregister(old_m && !magnet, old_t && !torrent);
        match result.and(unreg) {
            Ok(()) => {
                if magnet || torrent {
                    self.toasts.push(ToastKind::Success, "Corrent is registered for magnet links and torrent files.");
                }
            }
            Err(e) => self.toasts.push(ToastKind::Error, format!("Registration failed: {e:#}")),
        }
        self.save_settings();
        // Status is read synchronously here so the follow-up hint is accurate.
        let st = os_handler::status();
        if magnet && matches!(st.magnet, HandlerState::OtherDefault(_)) {
            self.toasts.push(
                ToastKind::Info,
                "Another app is still the default for magnet links. Confirm Corrent in system settings.",
            );
        }
        if let Ok(mut s) = self.handler_status.lock() {
            *s = Some(st);
        }
    }

    pub(super) fn drain_ipc(&mut self, ctx: &Context) {
        let mut focus = false;
        while let Ok(msg) = self.ipc.try_recv() {
            match msg {
                IpcMessage::Add(src) => {
                    self.launch_queue.push_back(src);
                    focus = true;
                }
                IpcMessage::Focus => focus = true,
            }
        }
        if focus {
            self.show_window(ctx);
        }
    }

    pub fn show_window(&mut self, ctx: &Context) {
        self.window_hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // Some window managers refuse focus stealing; at least flash the taskbar entry.
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    }

    /// First-run / not-default banner above the list.
    pub(super) fn handler_banner(&mut self, ui: &mut Ui, p: &Palette) {
        if self.settings.behavior.handler_prompt_dismissed || self.onboarding.is_some() {
            return;
        }
        let st = self.handler_status.lock().ok().and_then(|g| g.clone());
        let Some(st) = st else { return };
        let needs = |s: &HandlerState| {
            matches!(s, HandlerState::NotRegistered | HandlerState::OtherDefault(_) | HandlerState::OtherPath)
        };
        if !needs(&st.magnet) && !needs(&st.torrent) {
            return;
        }
        egui::Frame::new()
            .fill(p.accent_soft())
            .stroke(egui::Stroke::new(1.0, p.accent.gamma_multiply(0.5)))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::MAGNET).size(20.0).color(p.accent));
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Open magnet links and .torrent files with Corrent?").strong().color(p.text),
                        );
                        let detail = match &st.magnet {
                            HandlerState::OtherDefault(app) => format!("Currently they open with {app}."),
                            _ => "Clicking a magnet link in your browser will add it here.".to_string(),
                        };
                        ui.label(RichText::new(detail).small().color(p.text_dim));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if secondary_button(ui, "Not now").clicked() {
                            self.settings.behavior.handler_prompt_dismissed = true;
                            self.save_settings();
                        }
                        let other = matches!(st.magnet, HandlerState::OtherDefault(_));
                        if cfg!(windows) && other && secondary_button(ui, "Open system settings").clicked() {
                            os_handler::open_system_default_settings();
                        }
                        if primary_button(ui, p, "Set as default").clicked() {
                            self.set_handlers(true, true);
                            self.settings.behavior.handler_prompt_dismissed = true;
                            self.save_settings();
                        }
                    });
                });
            });
    }
}
