//! Using a Corrent server instead of this computer's engine: the Connect dialog (pairing link or
//! address, certificate check, pairing code or password), switching engines, and the banner
//! shown while the server can't be reached.

use std::sync::Arc;

use egui::{Context, Id, RichText, Ui};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::media::client::{Slot, fetch, ready};
use crate::config::{Paths, Settings};
use crate::engine::{EngineEvent, EngineHandle, Wake};
use crate::link::ServerEntry;
use crate::link::pair::{self, Credentials, Trusted};
use crate::link::remote::RemoteStatus;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

/// The server the app is using.
pub struct ServerLink {
    pub entry: ServerEntry,
    pub status: Arc<RemoteStatus>,
}

impl ServerLink {
    pub fn connected(&self) -> bool {
        self.status.connected.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Starts the engine for `server`, or a local session.
pub fn start_engine(
    rt: &tokio::runtime::Handle,
    paths: &Paths,
    settings: &Settings,
    wake: Wake,
    events: std::sync::mpsc::Sender<EngineEvent>,
    server: Option<ServerEntry>,
) -> (EngineHandle, Option<ServerLink>) {
    match server {
        Some(entry) => {
            let (backend, status) = crate::link::remote::backend(entry.clone());
            tracing::info!("using the server {} at {}", entry.name, entry.url);
            (EngineHandle::with_backend(rt, paths.clone(), wake, events, backend), Some(ServerLink { entry, status }))
        }
        None => (EngineHandle::start(rt, paths.clone(), settings.clone(), wake, events), None),
    }
}

enum Step {
    Address,
    /// The certificate was fetched: show who it is (and its fingerprint when it wasn't pinned).
    SignIn(Box<Trusted>),
}

pub struct ConnectDialog {
    address: String,
    fingerprint: String,
    connect_id: String,
    code: String,
    user: String,
    password: String,
    use_password: bool,
    /// The fingerprint came from a pairing link (or was typed) and matched.
    pinned: bool,
    /// Unpinned: the person confirmed the shown fingerprint matches.
    confirmed: bool,
    step: Step,
    trusting: Option<Slot<Trusted>>,
    /// Servers announcing themselves on the LAN (searched when the dialog opens).
    found: Option<Slot<Vec<corrent_server::discovery::Found>>>,
    pairing: Option<Slot<ServerEntry>>,
    error: Option<String>,
}

impl Default for ConnectDialog {
    fn default() -> Self {
        ConnectDialog {
            address: String::new(),
            fingerprint: String::new(),
            connect_id: String::new(),
            code: String::new(),
            user: String::new(),
            password: String::new(),
            use_password: false,
            pinned: false,
            confirmed: false,
            step: Step::Address,
            trusting: None,
            found: None,
            pairing: None,
            error: None,
        }
    }
}

impl ConnectDialog {
    /// Debug hook: a pairing link as if pasted.
    pub fn dev_fill(&mut self, link: &str) {
        self.address = link.to_string();
    }
}

impl CorrentApp {
    /// Download defaults for new torrents: the server's when connected to one (falls back to
    /// this app's when the user may not read the server's settings).
    pub fn download_defaults(&self) -> crate::config::Downloads {
        let Some(s) = &self.server else { return self.settings.downloads.clone() };
        match s.status.settings.load_full() {
            Some(remote) => remote.downloads.clone(),
            // An empty folder lets the server choose its default.
            None => crate::config::Downloads { save_path: Default::default(), ..self.settings.downloads.clone() },
        }
    }

    pub fn open_connect(&mut self) {
        let found = fetch(&self.rt, &self.ctx, async {
            tokio::task::spawn_blocking(|| {
                corrent_server::discovery::browse(std::time::Duration::from_secs(3)).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())?
        });
        self.connect = Some(ConnectDialog { found: Some(found), ..Default::default() });
    }

    /// Switches to a server (`Some`) or back to this computer's engine (`None`) and remembers it.
    pub fn use_server(&mut self, entry: Option<ServerEntry>) {
        self.servers.active = entry.as_ref().map(ServerEntry::key);
        if let Some(e) = &entry {
            self.servers.upsert(e.clone());
        }
        let (servers, paths, tx) = (self.servers.clone(), self.paths.clone(), self.ui_events_tx.clone());
        self.rt.spawn_blocking(move || {
            if let Err(e) = servers.save(&paths) {
                let _ = tx.send(EngineEvent::Toast(
                    crate::engine::ToastKind::Error,
                    t!("Couldn't save the server list: {e}", e = e),
                ));
            }
        });
        // Back to this computer's engine: the previous local session must have stopped first
        // (it owns the profile). Checked every frame by `switch_tick`.
        if entry.is_none() && self.local_stopping.as_ref().is_some_and(|h| !h.is_finished()) {
            self.pending_local = true;
            self.toasts.push(crate::engine::ToastKind::Info, t!("Waiting for the torrent engine to stop…").to_string());
            return;
        }
        self.switch_engine(entry);
    }

    /// Finishes a switch that waited for the old local engine.
    pub(super) fn switch_tick(&mut self) {
        if !self.pending_local {
            return;
        }
        if self.local_stopping.as_ref().is_none_or(|h| h.is_finished()) {
            self.pending_local = false;
            self.switch_engine(None);
        } else {
            self.ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn switch_engine(&mut self, entry: Option<ServerEntry>) {
        // The old engine stops in the background (a local one flushes its state first).
        let old = self.engine.clone();
        let was_local = !old.is_remote();
        let stopping = std::thread::spawn(move || old.shutdown_blocking(std::time::Duration::from_secs(20)));
        if let Some(prev) = self.local_stopping.take() {
            if prev.is_finished() {
                let _ = prev.join();
            } else {
                // Still flushing (local → server → server): keep waiting for it at exit.
                self.local_stopping = Some(prev);
            }
        }
        if was_local {
            self.local_stopping = Some(stopping);
        }
        let wake = {
            let ctx = self.ctx.clone();
            Wake::new(move || ctx.request_repaint())
        };
        let (engine, link) =
            start_engine(&self.rt, &self.paths, &self.settings, wake, self.ui_events_tx.clone(), entry);
        self.engine = engine;
        self.server = link;
        self.snap = Arc::new(crate::engine::types::Snapshot::default());
        self.selected.clear();
        self.focus = None;
        self.anchor = None;
        self.sent_detail_target = None;
        self.add_dialogs.clear();
        self.media.reset();
        let text = match &self.server {
            Some(s) => t!("Using the server “{name}”", name = s.entry.name),
            None => t!("Using this computer's torrent engine").to_string(),
        };
        self.toasts.push(crate::engine::ToastKind::Info, text);
    }

    /// At exit: a local engine stopped by a switch finishes flushing (bounded).
    pub(super) fn wait_for_old_engine(&mut self) {
        if let Some(h) = self.local_stopping.take() {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !h.is_finished() && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }

    /// The Connect dialog (when open).
    pub(super) fn connect_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut d) = self.connect.take() else { return };
        // Results of the background steps.
        if let Some(r) = d.trusting.as_ref().and_then(ready) {
            d.trusting = None;
            match r {
                Ok(t) => {
                    d.error = None;
                    d.confirmed = false;
                    d.step = Step::SignIn(Box::new(t));
                }
                Err(e) => d.error = Some(e),
            }
        }
        let mut done: Option<ServerEntry> = None;
        if let Some(r) = d.pairing.as_ref().and_then(ready) {
            d.pairing = None;
            match r {
                Ok(e) => done = Some(e),
                Err(e) => d.error = Some(e),
            }
        }
        let mut keep = true;
        let busy = d.trusting.is_some() || d.pairing.is_some();
        let r = egui::Modal::new(Id::new("connect_server")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new(format!("{}  {}", icons::HARD_DRIVES, t!("Connect to a Corrent server"))).strong().size(17.0),
            );
            ui.add_space(6.0);
            match &d.step {
                Step::Address => {
                    ui.label(
                        RichText::new(t!(
                            "Paste the pairing link from the server's web interface (Settings → Devices → Pair a device), or type its address."
                        ))
                        .color(p.text_dim),
                    );
                    // Servers on this network: a click fills in the address (their announced
                    // fingerprint is only a hint, so it is still shown for checking).
                    match d.found.as_ref().and_then(ready) {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new(t!("Looking for servers on this network…")).small().color(p.text_dim));
                            });
                        }
                        Some(Ok(list)) if !list.is_empty() => {
                            ui.add_space(6.0);
                            ui.label(RichText::new(t!("On this network")).small().strong().color(p.text_dim));
                            for f in list {
                                let Some(url) = f.url() else { continue };
                                let label = format!("{}  {}   {}", icons::HARD_DRIVES, f.advert.name, url);
                                if ui.add(egui::Button::selectable(d.address == url, label)).clicked() {
                                    d.address = url;
                                }
                            }
                        }
                        Some(_) => {}
                    }
                    ui.add_space(8.0);
                    ui.label(t!("Pairing link or address"));
                    ui.add(
                        egui::TextEdit::singleline(&mut d.address)
                            .hint_text(t!("corrent://pair?…  or  192.168.1.10:8443"))
                            .desired_width(f32::INFINITY),
                    );
                    if let Some(l) = pair::parse_link(&d.address) {
                        d.address = l.host;
                        if !l.code.is_empty() {
                            d.code = l.code;
                            d.use_password = false;
                        }
                        if !l.fingerprint.is_empty() {
                            d.fingerprint = l.fingerprint;
                        }
                        d.connect_id = l.connect;
                    }
                    ui.add_space(4.0);
                    ui.label(t!("Certificate fingerprint (optional)"));
                    ui.add(
                        egui::TextEdit::singleline(&mut d.fingerprint)
                            .hint_text(t!("AB:CD:EF:…"))
                            .desired_width(f32::INFINITY),
                    );
                }
                Step::SignIn(t) => {
                    ui.label(RichText::new(&t.info.name).strong().color(p.text));
                    ui.label(RichText::new(&t.url).small().color(p.text_dim));
                    if !t.fingerprint.is_empty() && !d.pinned {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(t!(
                                "Check that this fingerprint matches the one the server shows before you continue:"
                            ))
                            .color(p.warn),
                        );
                        ui.label(
                            RichText::new(crate::link::pair::short(&t.fingerprint)).monospace().color(p.text),
                        );
                        ui.checkbox(&mut d.confirmed, t!("It matches"));
                    } else if t.url.starts_with("http://") {
                        ui.label(
                            RichText::new(t!("This connection isn't encrypted. Use it only on this computer."))
                                .small()
                                .color(p.warn),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut d.use_password, false, t!("Pairing code"));
                        ui.selectable_value(&mut d.use_password, true, t!("User name and password"));
                    });
                    if d.use_password {
                        ui.label(t!("User name"));
                        ui.add(egui::TextEdit::singleline(&mut d.user).desired_width(f32::INFINITY));
                        ui.label(t!("Password"));
                        ui.add(egui::TextEdit::singleline(&mut d.password).password(true).desired_width(f32::INFINITY));
                    } else {
                        ui.label(t!("Pairing code"));
                        ui.add(egui::TextEdit::singleline(&mut d.code).desired_width(f32::INFINITY));
                    }
                }
            }
            if let Some(e) = &d.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(p.error));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = match d.step {
                        Step::Address => t!("Continue"),
                        Step::SignIn(_) => t!("Connect"),
                    };
                    // Sending a code or password to an unchecked server needs the person's word.
                    let unchecked = matches!(&d.step, Step::SignIn(t) if !t.fingerprint.is_empty() && !d.pinned && !d.confirmed);
                    if ui.add_enabled_ui(!busy && !unchecked, |ui| primary_button(ui, p, label)).inner.clicked() {
                        d.error = None;
                        match &d.step {
                            Step::Address => match pair::base_url(&d.address) {
                                Some(url) => {
                                    let fp = d.fingerprint.trim().to_string();
                                    let connect = d.connect_id.trim().to_string();
                                    d.pinned = pair::normalize_fingerprint(&fp).len() >= 32 || !connect.is_empty();
                                    d.trusting = Some(fetch(&self.rt, ctx, async move {
                                        pair::trust(&url, Some(fp.as_str()).filter(|f| !f.is_empty()), &connect).await
                                    }));
                                }
                                None => d.error = Some(t!("The server address isn't valid.").to_string()),
                            },
                            Step::SignIn(t) => {
                                let creds = if d.use_password {
                                    Credentials::Password { user: d.user.clone(), password: d.password.clone() }
                                } else {
                                    Credentials::Code(d.code.clone())
                                };
                                let t = (**t).clone();
                                d.pairing = Some(fetch(&self.rt, ctx, async move { pair::pair(&t, creds).await }));
                            }
                        }
                    }
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        keep = false;
                    }
                    if matches!(d.step, Step::SignIn(_)) && !busy && secondary_button(ui, t!("Back")).clicked() {
                        d.step = Step::Address;
                    }
                });
            });
        });
        if r.should_close() && !busy {
            keep = false;
        }
        if let Some(entry) = done {
            self.use_server(Some(entry));
            return;
        }
        if keep {
            self.connect = Some(d);
        }
    }

    /// Server entries in the overflow menu.
    pub(super) fn server_menu(&mut self, ui: &mut Ui) {
        match &self.server {
            Some(s) => {
                let name = s.entry.name.clone();
                if ui.button(format!("{}  {}", icons::DESKTOP, t!("Use this computer's engine"))).clicked() {
                    self.use_server(None);
                }
                ui.label(RichText::new(t!("Connected to {name}", name = name)).small().weak());
            }
            None => {
                let saved = self.servers.list.clone();
                if ui.button(format!("{}  {}", icons::HARD_DRIVES, t!("Connect to a server…"))).clicked() {
                    self.open_connect();
                }
                for e in saved {
                    if ui.button(format!("{}  {}", icons::PLUGS_CONNECTED, t!("Use “{name}”", name = e.name))).clicked()
                    {
                        self.use_server(Some(e));
                    }
                }
            }
        }
    }

    /// Shown while the server can't be reached.
    pub(super) fn server_banner(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(s) = &self.server else { return };
        if s.connected() {
            return;
        }
        let problem = s.status.problem.load_full();
        let Some(problem) = problem else { return };
        let name = s.entry.name.clone();
        let mut local = false;
        egui::Frame::new()
            .fill(p.warn.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, p.warn.gamma_multiply(0.5)))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icons::PLUGS).size(20.0).color(p.warn));
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(t!("Can't reach the server “{name}”", name = name)).strong().color(p.text),
                        );
                        ui.label(
                            RichText::new(t!("{why} Trying again…", why = problem.as_str())).small().color(p.text_dim),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if secondary_button(ui, t!("Use this computer's engine")).clicked() {
                            local = true;
                        }
                    });
                });
            });
        if local {
            self.use_server(None);
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }
}
