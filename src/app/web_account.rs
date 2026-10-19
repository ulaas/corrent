//! The web interface's sign-in for this computer's server: the desktop app owns that account
//! (it has no password until set here), so only the app can name it and choose a password.

use egui::{Id, RichText};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::media::client::{Slot, fetch, ready};
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

#[derive(Default)]
pub struct WebAccount {
    name: String,
    password: String,
    saving: Option<Slot<serde_json::Value>>,
    error: Option<String>,
    done: bool,
}

impl CorrentApp {
    pub fn open_web_account(&mut self) {
        self.web_account = Some(WebAccount { name: "admin".into(), ..Default::default() });
    }

    pub(super) fn web_account_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut w) = self.web_account.take() else { return };
        if let Some(r) = w.saving.as_ref().and_then(ready) {
            w.saving = None;
            match r {
                Ok(_) => w.done = true,
                Err(e) => w.error = Some(e),
            }
        }
        let mut keep = true;
        let url = self.media.local().map(|l| l.base_url.clone()).unwrap_or_default();
        let r = egui::Modal::new(Id::new("web_account")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(format!("{}  {}", icons::GLOBE, t!("Web interface sign-in"))).strong().size(17.0));
            ui.add_space(6.0);
            if w.done {
                ui.label(t!("Done. Sign in at {url} with this name and password.", url = url));
            } else {
                ui.label(
                    RichText::new(t!("Choose a name and password to sign in to the web interface on this computer."))
                        .color(p.text_dim),
                );
                ui.add_space(8.0);
                ui.label(t!("User name"));
                ui.add(egui::TextEdit::singleline(&mut w.name).desired_width(f32::INFINITY));
                ui.label(t!("Password"));
                ui.add(egui::TextEdit::singleline(&mut w.password).password(true).desired_width(f32::INFINITY));
                if let Some(e) = &w.error {
                    ui.label(RichText::new(e).color(p.error));
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if w.saving.is_some() {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w.done {
                        if primary_button(ui, p, t!("Close")).clicked() {
                            keep = false;
                        }
                        return;
                    }
                    let ok = w.saving.is_none() && !w.name.trim().is_empty() && w.password.len() >= 8;
                    if ui.add_enabled_ui(ok, |ui| primary_button(ui, p, t!("Save"))).inner.clicked()
                        && let Some(client) = self.media.client.clone()
                    {
                        let body =
                            serde_json::json!({ "name": w.name.trim(), "password": w.password, "role": "admin" });
                        w.error = None;
                        w.saving = Some(fetch(&self.rt, ctx, async move {
                            client.send(reqwest::Method::POST, "/api/v1/auth/setup", &body).await
                        }));
                    }
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        keep = false;
                    }
                });
            });
        });
        if r.should_close() {
            keep = false;
        }
        if keep {
            self.web_account = Some(w);
        }
    }
}
