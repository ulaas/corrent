//! Corrent servers on this network (`link::discover`): a notice when a server the app isn't
//! paired with announces itself (connecting still means pairing), and an offer to switch back
//! to the server the app left because it couldn't be reached, once its address answers again.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use corrent_server::discovery::{Browser, Found};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::media::client::{Slot, fetch, ready};
use crate::link::ServerEntry;
use crate::link::discover::{self, Verdict};
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

/// Why a paired server is being checked.
enum Purpose {
    /// The server the app fell back from: offer it again.
    Return,
    /// The server in use can't be reached at its address but announces another one.
    Moved,
}

/// New servers kept for the notice, address checks in flight, addresses tried per server, and
/// checks remembered (anyone on the network can announce anything).
const MAX_NEW: usize = 8;
const MAX_PROBES: usize = 2;
const MAX_ADDRESSES: usize = 4;
const MAX_PROBED: usize = 64;

#[derive(Default)]
pub struct DiscoverState {
    browser: Option<Browser>,
    starting: Option<Receiver<Result<Browser, String>>>,
    retry_at: Option<Instant>,
    first_frame: Option<Instant>,
    last_tick: Option<Instant>,
    /// The new-server notice was answered this session (it is shown once).
    noticed: bool,
    new: Vec<Found>,
    /// Paired servers checked this session, by key and announced addresses (each set once).
    probed: HashSet<String>,
    probes: Vec<(Purpose, String, Slot<String>)>,
    /// The server the app fell back from answers again.
    offer: Option<ServerEntry>,
    #[cfg(debug_assertions)]
    dev_done: bool,
}

impl CorrentApp {
    /// Every frame: listens while wanted, handles what was found and the address checks.
    pub(super) fn discovery_tick(&mut self, ctx: &egui::Context) {
        #[cfg(debug_assertions)]
        self.discovery_dev_hook();
        // An offer that no longer applies (the user chose a server, or dropped the fallback).
        if self
            .discover
            .offer
            .as_ref()
            .is_some_and(|e| self.server.is_some() || self.servers.fallback.as_deref() != Some(e.key().as_str()))
        {
            self.discover.offer = None;
        }
        self.discovery_probes();
        if self.settings.behavior.auto_switch_server
            && self.calm()
            && let Some(e) = self.discover.offer.take()
        {
            self.use_server(Some(e));
        }
        let now = Instant::now();
        let st = &mut self.discover;
        let first = *st.first_frame.get_or_insert(now);
        if !discover::wanted(self.settings.behavior.find_servers, self.onboarding.is_some(), now - first) {
            st.browser = None;
            st.starting = None;
            st.retry_at = None;
            return;
        }
        if st.browser.is_none() && st.starting.is_none() && st.retry_at.is_none_or(|t| now >= t) {
            // Binding the multicast sockets is I/O: off the UI thread.
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("corrent-mdns-start".into())
                .spawn(move || {
                    let _ = tx.send(Browser::start().map_err(|e| format!("{e:#}")));
                })
                .is_ok();
            st.starting = spawned.then_some(rx);
            st.retry_at = Some(now + Duration::from_secs(600));
        }
        if let Some(rx) = &st.starting {
            match rx.try_recv() {
                Ok(Ok(b)) => {
                    st.browser = Some(b);
                    st.starting = None;
                    st.retry_at = None;
                }
                // No multicast here (some VPNs, a blocked firewall): try again later, quietly.
                Ok(Err(e)) => {
                    tracing::debug!("looking for servers on this network: {e}");
                    st.starting = None;
                }
                Err(TryRecvError::Disconnected) => st.starting = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        // Back from sleep: ask again now (a failed request starts over), and check paired
        // servers again.
        if st.last_tick.is_some_and(|t| now.duration_since(t) > Duration::from_secs(60)) {
            if st.browser.as_mut().is_some_and(|b| b.requery().is_err()) {
                st.browser = None;
                st.retry_at = None;
            }
            st.probed.clear();
        }
        st.last_tick = Some(now);
        let found = st.browser.as_ref().map(Browser::poll).unwrap_or_default();
        if st.browser.is_some() {
            ctx.request_repaint_after(Duration::from_secs(5));
        }
        for f in found {
            self.discovery_found(f);
        }
    }

    /// An announcement (from the network, or a test).
    pub(crate) fn discovery_found(&mut self, f: Found) {
        match discover::classify(&f, &self.servers) {
            Verdict::Skip => {}
            Verdict::New => {
                let st = &mut self.discover;
                // Bounded: anyone on the network can announce any number of servers.
                if !st.noticed && st.new.len() < MAX_NEW && !st.new.iter().any(|x| x.advert.id == f.advert.id) {
                    st.new.push(f);
                }
            }
            Verdict::Paired(key) => {
                let Some(purpose) = self.purpose(&key) else { return };
                let Some(entry) = self.servers.list.iter().find(|e| e.key() == key).cloned() else { return };
                let mut urls = discover::candidates(&entry, &f);
                if matches!(purpose, Purpose::Moved) {
                    // Same address: the connection keeps retrying it by itself.
                    urls.retain(|u| *u != entry.url);
                }
                urls.truncate(MAX_ADDRESSES);
                let st = &mut self.discover;
                if st.probed.len() > MAX_PROBED {
                    st.probed.clear();
                }
                if urls.is_empty()
                    || st.probes.len() >= MAX_PROBES
                    || !st.probed.insert(format!("{key}|{}", urls.join(",")))
                {
                    return;
                }
                let slot =
                    fetch(
                        &self.rt,
                        &self.ctx,
                        async move { discover::probe(&entry, &urls).await.ok_or_else(String::new) },
                    );
                self.discover.probes.push((purpose, key, slot));
            }
        }
    }

    /// Why the paired server with `key` matters now, if it does: the one in use while it can't
    /// be reached, or the one the app fell back from (not offered yet).
    fn purpose(&self, key: &str) -> Option<Purpose> {
        match &self.server {
            // Still the user's choice (not while a switch to this computer's engine is pending).
            Some(s) if s.entry.key() == key && !s.connected() && self.servers.active.as_deref() == Some(key) => {
                Some(Purpose::Moved)
            }
            Some(_) => None,
            None if self.servers.fallback.as_deref() == Some(key) && self.discover.offer.is_none() => {
                Some(Purpose::Return)
            }
            None => None,
        }
    }

    /// Address checks that finished.
    fn discovery_probes(&mut self) {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.discover.probes.len() {
            match self.discover.probes.get(i).and_then(|(_, _, s)| ready(s)) {
                Some(r) => {
                    let (_, key, _) = self.discover.probes.swap_remove(i);
                    done.push((key, r));
                }
                None => i += 1,
            }
        }
        for (key, r) in done {
            if let Ok(url) = r {
                self.discovery_probe_done(&key, &url);
            }
        }
    }

    /// `key`'s server answered at `url`. Acts only if that still matters (the check may finish
    /// after a switch, or after the server came back at its old address).
    pub(crate) fn discovery_probe_done(&mut self, key: &str, url: &str) {
        match self.purpose(key) {
            Some(Purpose::Moved) => {
                let Some(entry) = self.discovery_verified(key, url) else { return };
                self.toasts.push(
                    crate::engine::ToastKind::Info,
                    t!("“{name}” has a new address on this network", name = entry.name).to_string(),
                );
                self.use_server(Some(entry));
            }
            Some(Purpose::Return) => {
                if let Some(entry) = self.discovery_verified(key, url) {
                    self.discover.offer = Some(entry);
                }
            }
            None => {}
        }
    }

    /// `key`'s server answered at `url`: its entry, moved there if that is a new address
    /// (saved). Also the entry point for tests.
    pub(crate) fn discovery_verified(&mut self, key: &str, url: &str) -> Option<ServerEntry> {
        let entry = self.servers.list.iter().find(|e| e.key() == key)?.clone();
        if entry.url == url {
            return Some(entry);
        }
        let moved = self.servers.replace_address(key, url)?;
        self.save_servers();
        Some(moved)
    }

    /// Debug hook: `CORRENT_DEV_FOUND=<name>[;<ip>]|…` announces servers as if on the network.
    #[cfg(debug_assertions)]
    fn discovery_dev_hook(&mut self) {
        if std::mem::replace(&mut self.discover.dev_done, true) {
            return;
        }
        let Ok(v) = std::env::var("CORRENT_DEV_FOUND") else { return };
        for (i, item) in v.split('|').filter(|s| !s.is_empty()).enumerate() {
            let (name, ip) = item.split_once(';').unwrap_or((item, "192.0.2.10"));
            let Ok(ip) = ip.parse() else { continue };
            self.discovery_found(Found {
                advert: corrent_server::discovery::Advert {
                    id: format!("de{i:010x}"),
                    name: name.to_string(),
                    version: "dev".into(),
                    fingerprint: String::new(),
                    https: true,
                    port: 8443,
                },
                addrs: vec![ip],
            });
        }
    }

    /// The paired server answers again (tests).
    #[cfg(test)]
    pub(crate) fn discovery_offer(&mut self, e: ServerEntry) {
        self.discover.offer = Some(e);
    }

    /// Nothing the user is in the middle of (an automatic switch resets the torrent and media views).
    fn calm(&self) -> bool {
        self.modal.is_none()
            && self.connect.is_none()
            && self.settings_draft.is_none()
            && self.onboarding.is_none()
            && self.add_dialogs.is_empty()
            && self.batch.is_none()
            && self.creator.is_none()
            && self.import.is_none()
            && self.web_account.is_none()
            && self.media.player.is_none()
            && !self.tv.on
    }

    /// The notices (normal view; in TV mode they wait).
    pub(super) fn discovery_banner(&mut self, ui: &mut Ui, p: &Palette) {
        if let Some(e) = self.discover.offer.clone() {
            let (mut switch, mut keep) = (false, false);
            frame(ui, p, |ui| {
                ui.label(RichText::new(icons::PLUGS_CONNECTED).size(20.0).color(p.accent));
                ui.vertical(|ui| {
                    ui.label(RichText::new(t!("“{name}” is reachable again", name = e.name)).strong().color(p.text));
                    ui.label(
                        RichText::new(t!("You switched to this computer's engine while it couldn't be reached."))
                            .small()
                            .color(p.text_dim),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    keep = secondary_button(ui, t!("Keep this computer's engine")).clicked();
                    switch = primary_button(ui, p, t!("Switch to it")).clicked();
                });
            });
            if switch {
                self.discover.offer = None;
                self.use_server(Some(e));
            } else if keep {
                self.discover.offer = None;
                self.servers.fallback = None;
                self.save_servers();
            }
            return;
        }
        if self.discover.noticed || self.discover.new.is_empty() {
            return;
        }
        let names: Vec<String> = self.discover.new.iter().map(|f| f.advert.name.clone()).collect();
        let (mut connect, mut later, mut never) = (false, false, false);
        frame(ui, p, |ui| {
            ui.label(RichText::new(icons::HARD_DRIVES).size(20.0).color(p.accent));
            ui.vertical(|ui| {
                let title = match names.as_slice() {
                    [one] => t!("“{name}” is on this network", name = one),
                    more => t!("{n} Corrent servers are on this network", n = more.len()),
                };
                ui.label(RichText::new(title).strong().color(p.text));
                ui.label(
                    RichText::new(t!(
                        "A Corrent server can run your torrents and media. Connecting needs its pairing code."
                    ))
                    .small()
                    .color(p.text_dim),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                never = secondary_button(ui, t!("Don't ask again")).clicked();
                later = secondary_button(ui, t!("Not now")).clicked();
                let label = if names.len() == 1 { t!("Connect…") } else { t!("Choose…") };
                connect = primary_button(ui, p, label).clicked();
            });
        });
        if connect || later || never {
            // Always https: the announcement can't steer pairing to plain HTTP.
            let one = (self.discover.new.len() == 1)
                .then(|| self.discover.new.first().and_then(Found::url))
                .flatten()
                .map(|u| u.replacen("http://", "https://", 1));
            self.discover.noticed = true;
            self.discover.new.clear();
            if connect {
                self.open_connect();
                if let (Some(d), Some(url)) = (self.connect.as_mut(), one) {
                    d.dev_fill(&url);
                }
            }
            if never {
                self.settings.behavior.find_servers = false;
                if let Some(d) = self.settings_draft.as_mut() {
                    d.settings.behavior.find_servers = false;
                }
                self.save_settings();
            }
        }
    }
}

/// A notice's frame, like the other banners.
fn frame(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(p.accent.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, p.accent.gamma_multiply(0.45)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
        .show(ui, |ui| {
            ui.horizontal(add);
        });
}
