//! Profiles on a shared device: who is watching (their watch state, favorites and allowed
//! libraries). Switching asks for the profile's PIN when it has one; the PIN pad works with a
//! remote too. Torrents are unaffected: only the media pages change user.

use corrent_proto::auth::{LoginResponse, Profile, ProfileSwitch, User};
use egui::{Id, RichText, vec2};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

#[derive(Default)]
pub struct Profiles {
    /// The device's own client and user (what the app signed in with).
    home: Option<(MediaClient, Slot<User>)>,
    /// The profile in use when not the device's own.
    pub current: Option<User>,
    picker: Option<Picker>,
}

struct Picker {
    list: Slot<Vec<Profile>>,
    /// Asking for this profile's PIN.
    pin_for: Option<Profile>,
    pin: String,
    switching: Option<Slot<LoginResponse>>,
    error: Option<String>,
}

impl CorrentApp {
    /// The name shown on the profile button.
    pub(crate) fn profile_name(&mut self) -> Option<String> {
        if let Some(u) = &self.media.profiles.current {
            return Some(if u.display_name.is_empty() { u.name.clone() } else { u.display_name.clone() });
        }
        let client = self.media.client.clone()?;
        let (_, me) = self.media.profiles.home.get_or_insert_with(|| {
            let c = client.clone();
            (client, fetch(&self.rt, &self.ctx, async move { c.get::<User>("/api/v1/auth/me").await }))
        });
        let me = ready(me)?.ok()?;
        Some(if me.display_name.is_empty() { me.name } else { me.display_name })
    }

    pub(crate) fn open_profiles(&mut self) {
        let Some(client) = self.media.client.clone() else { return };
        let list = fetch(&self.rt, &self.ctx, async move { client.get("/api/v1/auth/profiles").await });
        self.media.profiles.picker =
            Some(Picker { list, pin_for: None, pin: String::new(), switching: None, error: None });
    }

    fn use_profile(&mut self, client: MediaClient, user: Option<User>) {
        let player = self.media.player.take();
        drop(player);
        self.media.client = Some(client);
        self.media.profiles.current = user;
        self.media.invalidate();
        self.media.page = None;
    }

    fn switch_to(&mut self, p: &Profile, pin: Option<String>) {
        let home_user = self.media.profiles.home.as_ref().and_then(|(_, u)| ready(u)).and_then(Result::ok);
        // Back to the device's own profile without a PIN: no server round trip needed.
        if let (Some(me), Some((home, _))) = (&home_user, &self.media.profiles.home)
            && me.id == p.id
            && !p.has_pin
        {
            let home = home.clone();
            self.use_profile(home, None);
            self.media.profiles.picker = None;
            return;
        }
        let Some(client) = self.media.client.clone() else { return };
        let req = ProfileSwitch { user_id: p.id, pin, device: crate::link::pair::this_device() };
        if let Some(pk) = &mut self.media.profiles.picker {
            pk.error = None;
            pk.switching = Some(fetch(&self.rt, &self.ctx, async move {
                client.send(reqwest::Method::POST, "/api/v1/auth/profile", &req).await
            }));
        }
    }

    /// The picker window (when open).
    pub(super) fn profiles_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut pk) = self.media.profiles.picker.take() else { return };
        if let Some(r) = pk.switching.as_ref().and_then(ready) {
            pk.switching = None;
            match r {
                Ok(res) => {
                    let base = self.media.client.clone();
                    if let (Some(token), Some(base)) = (res.token, base) {
                        let client = base.with_token(&token);
                        let home_id = self
                            .media
                            .profiles
                            .home
                            .as_ref()
                            .and_then(|(_, u)| ready(u))
                            .and_then(Result::ok)
                            .map(|u| u.id);
                        let user = (Some(res.user.id) != home_id).then_some(res.user);
                        self.use_profile(client, user);
                    }
                    return;
                }
                Err(e) => {
                    pk.error = Some(e);
                    pk.pin.clear();
                }
            }
        }
        let mut close = false;
        let mut choose: Option<(Profile, Option<String>)> = None;
        let r = egui::Modal::new(Id::new("profiles")).show(ctx, |ui| {
            ui.set_width(420.0);
            match pk.pin_for.clone() {
                None => {
                    ui.label(RichText::new(t!("Who's watching?")).size(20.0).strong().color(p.text));
                    ui.add_space(10.0);
                    match ready(&pk.list) {
                        None => {
                            ui.spinner();
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(e).color(p.error));
                        }
                        Some(Ok(list)) => {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                                for prof in list {
                                    let name =
                                        if prof.display_name.is_empty() { &prof.name } else { &prof.display_name };
                                    let lock =
                                        if prof.has_pin { format!("  {}", icons::LOCK_SIMPLE) } else { String::new() };
                                    let b = egui::Button::new(
                                        RichText::new(format!("{}\n{name}{lock}", icons::USER_CIRCLE)).size(18.0),
                                    )
                                    .min_size(vec2(120.0, 90.0));
                                    if ui.add(b).clicked() {
                                        if prof.has_pin {
                                            pk.pin_for = Some(prof.clone());
                                            pk.pin.clear();
                                        } else {
                                            choose = Some((prof.clone(), None));
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
                Some(prof) => {
                    let name = if prof.display_name.is_empty() { prof.name.clone() } else { prof.display_name.clone() };
                    ui.label(RichText::new(t!("PIN for {name}", name = name)).size(18.0).strong().color(p.text));
                    ui.add_space(6.0);
                    ui.label(RichText::new("•".repeat(pk.pin.len()).to_string() + " ").size(28.0).monospace());
                    ui.add_space(6.0);
                    // A keypad that works with arrow keys and a remote (and typing digits).
                    for row in [["1", "2", "3"], ["4", "5", "6"], ["7", "8", "9"]] {
                        ui.horizontal(|ui| {
                            for d in row {
                                if ui
                                    .add(egui::Button::new(RichText::new(d).size(20.0)).min_size(vec2(64.0, 52.0)))
                                    .clicked()
                                    && pk.pin.len() < 8
                                {
                                    pk.pin.push_str(d);
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new(icons::BACKSPACE).size(20.0))
                                    .min_size(vec2(64.0, 52.0)),
                            )
                            .clicked()
                        {
                            pk.pin.pop();
                        }
                        if ui.add(egui::Button::new(RichText::new("0").size(20.0)).min_size(vec2(64.0, 52.0))).clicked()
                            && pk.pin.len() < 8
                        {
                            pk.pin.push('0');
                        }
                        let ok = ui.add_enabled(
                            pk.pin.len() >= 4,
                            egui::Button::new(RichText::new(icons::CHECK).size(20.0)).min_size(vec2(64.0, 52.0)),
                        );
                        if ok.clicked() {
                            choose = Some((prof.clone(), Some(pk.pin.clone())));
                        }
                    });
                    let typed: Vec<char> = ui.input(|i| {
                        i.events
                            .iter()
                            .filter_map(|e| match e {
                                egui::Event::Text(t) => {
                                    Some(t.chars().filter(char::is_ascii_digit).collect::<Vec<_>>())
                                }
                                _ => None,
                            })
                            .flatten()
                            .collect()
                    });
                    for c in typed {
                        if pk.pin.len() < 8 {
                            pk.pin.push(c);
                        }
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) && pk.pin.len() >= 4 && choose.is_none() {
                        choose = Some((prof.clone(), Some(pk.pin.clone())));
                    }
                }
            }
            if let Some(e) = &pk.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(p.error));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if pk.switching.is_some() {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        close = true;
                    }
                    if pk.pin_for.is_some() && primary_button(ui, p, t!("Back")).clicked() {
                        pk.pin_for = None;
                    }
                });
            });
        });
        if r.should_close() {
            close = true;
        }
        if close {
            return;
        }
        self.media.profiles.picker = Some(pk);
        if let Some((prof, pin)) = choose {
            self.switch_to(&prof, pin);
        }
    }
}
