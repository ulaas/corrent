//! Update checks and "Update and restart".
//!
//! Once a day (Settings › Behavior › Updates, on by default) the app asks GitHub for the latest
//! release. A newer one is announced by a toast, in About and in the Updates card; nothing is
//! downloaded until the user clicks. The click downloads the release file for this kind of
//! installation, checks it, puts it in place and restarts (`corrent_core::self_update`,
//! `integration::updater`), with this dialog showing progress, Cancel and what went wrong. Copies
//! a package manager owns, or that Corrent can't write to, only get told how to update.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use corrent_core::self_update::{self, InstallKind, Os, Progress};
use egui::{Context, Id, RichText};
use egui_phosphor::regular as icons;

use super::{CorrentApp, toasts};
use crate::engine::ToastKind;
use crate::integration::updater;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, primary_button, secondary_button};
use crate::updates::{Release, Source};

/// A newer release, and how this copy of Corrent can take it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub release: Release,
    pub kind: InstallKind,
}

impl Found {
    /// What the button that starts the update says.
    pub fn action_label(&self) -> &'static str {
        if self.kind.installs_itself() {
            t!("Update and restart")
        } else if self.kind.downloads() {
            t!("Download the update")
        } else {
            t!("How to update")
        }
    }
}

type CheckReply = (bool, Result<Option<Found>, String>);

#[derive(Default)]
pub struct UpdateState {
    check_rx: Option<mpsc::Receiver<CheckReply>>,
    /// The newest release found in this session, if it is newer than this build.
    pub available: Option<Found>,
    /// The version the user was last told about (one toast per version and session).
    notified: String,
    /// A release was found while the window was hidden (in the tray): the toast waits for it.
    announce: bool,
    /// How the last check of this session ended when it found nothing newer, for About.
    checked: Option<Result<(), String>>,
    dialog: Option<Dialog>,
}

#[cfg(test)]
impl UpdateState {
    /// The answer of a check, as if one had just come back.
    pub(super) fn deliver(&mut self, manual: bool, result: Result<Option<Found>, String>) {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send((manual, result));
        self.check_rx = Some(rx);
    }

    /// (The update dialog is up, something is being downloaded or installed.)
    pub(super) fn dialog(&self) -> (bool, bool) {
        let busy = |d: &Dialog| matches!(d.state, State::Working { .. } | State::Restarting);
        (self.dialog.is_some(), self.dialog.as_ref().is_some_and(busy))
    }
}

struct Dialog {
    found: Found,
    state: State,
}

enum State {
    /// This copy doesn't update itself: how it is updated.
    Info,
    Working {
        rx: mpsc::Receiver<Msg>,
        progress: Option<Progress>,
        cancel: Arc<AtomicBool>,
    },
    Failed(String),
    /// The new version is in place and waits for this one to quit.
    Restarting,
    /// macOS: the checked disk image is open.
    Opened(PathBuf),
}

enum Msg {
    Progress(Progress),
    Done(Result<Outcome, String>),
}

enum Outcome {
    Restart,
    Opened(PathBuf),
}

/// Where releases come from, and whether plain HTTP is allowed there (never in a release build).
fn source() -> (Source, bool) {
    // Debug builds can be pointed at a folder served on loopback (CLAUDE.md, CORRENT_DEV_UPDATE).
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("CORRENT_DEV_UPDATE") {
        let base = base.trim_end_matches('/');
        let s = Source { api: format!("{base}/latest.json"), pages: format!("{base}/"), downloads: format!("{base}/") };
        return (s, true);
    }
    (Source::github(), false)
}

/// Screenshot and demo runs (debug builds) never ask GitHub on their own.
fn automatic_checks_allowed() -> bool {
    // Nor do tests.
    if cfg!(test) {
        return false;
    }
    #[cfg(debug_assertions)]
    if std::env::var_os("CORRENT_DEV_DEMO").is_some() || std::env::var_os("CORRENT_DEV_SHOT").is_some() {
        return false;
    }
    true
}

/// Downloads, checks and installs `found` (or opens the disk image). Runs on the runtime.
async fn run(
    found: Found,
    data_dir: PathBuf,
    proxy: Option<String>,
    plain_http: bool,
    mut progress: impl FnMut(Progress) + Send,
    cancel: &AtomicBool,
) -> Result<Outcome, String> {
    let plan = self_update::plan(&found.release, &found.kind)?;
    let client = if plain_http { corrent_core::mpv_get::client(proxy) } else { self_update::client(proxy)? };
    if let InstallKind::MacApp { .. } = found.kind {
        // Not replaced in place (see docs/PACKAGING.md): the checked image is opened instead.
        let folder = crate::config::os_download_dir();
        let cant_write = |e: std::io::Error| t!("Can't write to {folder}: {e}", folder = folder.display(), e = e);
        std::fs::create_dir_all(&folder).map_err(cant_write)?;
        let file = self_update::fetch(&client, &plan, &folder, &mut progress, cancel).await?;
        open::that_detached(&file)
            .map_err(|e| t!("The update is in {path}, but it couldn't be opened: {e}", path = file.display(), e = e))?;
        return Ok(Outcome::Opened(file));
    }
    let work = self_update::work_dir(&found.kind, &data_dir)?;
    let r = install(&found.kind, &plan, &client, &work, &data_dir, &mut progress, cancel).await;
    // The MSI's runner works from the folder; everything else is done with it.
    if r.is_err() || !matches!(found.kind, InstallKind::WindowsMsi { .. }) {
        let _ = std::fs::remove_dir_all(&work);
    }
    r.map(|()| Outcome::Restart)
}

async fn install(
    kind: &InstallKind,
    plan: &self_update::Plan,
    client: &reqwest::Client,
    work: &Path,
    data_dir: &Path,
    progress: &mut (impl FnMut(Progress) + Send),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let file = self_update::fetch(client, plan, work, &mut *progress, cancel).await?;
    if cancel.load(Ordering::Relaxed) {
        return Err(t!("Cancelled.").into());
    }
    progress(Progress::Install);
    let (kind, work, data_dir) = (kind.clone(), work.to_path_buf(), data_dir.to_path_buf());
    let installed = tokio::task::spawn_blocking(move || {
        if let InstallKind::WindowsMsi { .. } = kind {
            return updater::start_installer(&file, &work, &data_dir)
                .map_err(|e| t!("The installer couldn't be started: {e}", e = e));
        }
        let program = updater::program_of(&kind).ok_or_else(|| t!("This copy of Corrent can't update itself."))?;
        let pairs = self_update::stage(&kind, &file, &work)?;
        let swap = self_update::swap(&pairs, self_update::Aside::for_os(Os::current()))?;
        if let Err(e) = updater::start_new_version(&program) {
            return Err(match swap.rollback() {
                Ok(()) => t!("The new version couldn't be started ({e}), so the one you had was put back.", e = e),
                Err(worse) => worse,
            });
        }
        Ok(())
    });
    installed.await.map_err(|e| e.to_string())?
}

impl CorrentApp {
    pub fn check_for_updates(&mut self, manual: bool) {
        if self.update.check_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (proxy, ctx) = (self.settings.connection.proxy_url(), self.ctx.clone());
        let (source, _) = source();
        self.rt.spawn(async move {
            let client = crate::rss::feed::client(proxy);
            let found = match crate::updates::check_at(&client, &source).await {
                Ok(Some(release)) => {
                    // Looks at the disk: not here on the runtime's own threads, nor on the UI's.
                    let kind = tokio::task::spawn_blocking(updater::detect).await.unwrap_or(InstallKind::Unknown);
                    Ok(Some(Found { release, kind }))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            let _ = tx.send((manual, found));
            ctx.request_repaint();
        });
        self.update.check_rx = Some(rx);
        self.settings.updates.last_check = crate::util::now_unix();
        self.save_settings();
    }

    pub(super) fn poll_updates(&mut self) {
        if self.update.check_rx.is_none()
            && self.settings.updates.due(crate::util::now_unix())
            && automatic_checks_allowed()
        {
            self.check_for_updates(false);
        }
        if self.update.announce && !self.window_hidden {
            self.update.announce = false;
            self.announce_update();
        }
        let Some(rx) = &self.update.check_rx else { return };
        let Ok((manual, result)) = rx.try_recv() else { return };
        self.update.check_rx = None;
        self.update.checked = match &result {
            Ok(Some(_)) => None,
            Ok(None) => Some(Ok(())),
            Err(e) => Some(Err(e.clone())),
        };
        // About says itself how a check ended.
        let toast = manual && !self.extras.about;
        match result {
            Ok(Some(found)) => {
                let version = found.release.version.clone();
                let skipped = version == self.settings.updates.skip_version;
                let told = version == self.update.notified;
                // About and Settings show it either way.
                self.update.available = Some(found);
                if manual || !(skipped || told) {
                    self.update.notified = version;
                    // A toast shown to a hidden window is gone before anyone sees it.
                    if self.window_hidden {
                        self.update.announce = true;
                    } else {
                        self.announce_update();
                    }
                }
                #[cfg(debug_assertions)]
                if std::env::var_os("CORRENT_DEV_UPDATE_NOW").is_some() && self.update.dialog.is_none() {
                    self.open_update();
                }
            }
            Ok(None) => {
                self.update.available = None;
                self.update.announce = false;
                if toast {
                    self.toasts.push(ToastKind::Success, t!("Corrent is up to date."));
                }
            }
            Err(e) if toast => self.toasts.push(ToastKind::Error, t!("Couldn't check for updates: {e}", e = e)),
            Err(e) => tracing::info!("update check failed: {e}"),
        }
    }

    /// The toast for the release that was found.
    fn announce_update(&mut self) {
        let Some(found) = &self.update.available else { return };
        let version = found.release.version.clone();
        let label = found.action_label();
        self.toasts.push_with(
            ToastKind::Info,
            t!("Corrent {new} is available (you have {current}).", new = version, current = env!("CARGO_PKG_VERSION")),
            vec![
                (label.into(), toasts::ToastAction::Update),
                (t!("Skip this version").into(), toasts::ToastAction::SkipVersion(version)),
            ],
        );
    }

    /// The version of the release that was found, for the menu.
    pub(super) fn update_version(&self) -> Option<&str> {
        self.update.available.as_ref().map(|f| f.release.version.as_str())
    }

    /// The newer release that was found, for About.
    pub(super) fn update_found(&self) -> Option<&Found> {
        self.update.available.as_ref()
    }

    /// A check is on its way.
    pub(super) fn update_checking(&self) -> bool {
        self.update.check_rx.is_some()
    }

    /// How the last check ended, when it found nothing newer (`Err`: it failed).
    pub(super) fn update_checked(&self) -> Option<&Result<(), String>> {
        self.update.checked.as_ref()
    }

    /// The user asked for the update that was found: starts it where Corrent updates itself (or
    /// downloads the disk image), and says how to update where it doesn't.
    pub fn open_update(&mut self) {
        if self.update.dialog.as_ref().is_some_and(|d| matches!(d.state, State::Working { .. } | State::Restarting)) {
            return;
        }
        let Some(found) = self.update.available.clone() else { return };
        let start = found.kind.downloads();
        self.update.dialog = Some(Dialog { found, state: State::Info });
        if start {
            self.start_update();
        }
    }

    fn start_update(&mut self) {
        let Some(d) = &mut self.update.dialog else { return };
        let found = d.found.clone();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c2, ctx, proxy) = (cancel.clone(), self.ctx.clone(), self.settings.connection.proxy_url());
        let data_dir = self.paths.data_dir.clone();
        let (_, plain_http) = source();
        self.rt.spawn(async move {
            let (tx2, ctx2) = (tx.clone(), ctx.clone());
            let progress = move |p| {
                let _ = tx2.send(Msg::Progress(p));
                ctx2.request_repaint();
            };
            let r = run(found, data_dir, proxy, plain_http, progress, &c2).await;
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
        d.state = State::Working { rx, progress: None, cancel };
    }

    /// "Corrent x is available" with the button that starts the update, for Settings.
    /// Returns whether the button was clicked.
    pub(super) fn update_notice(&self, ui: &mut egui::Ui, p: &Palette) -> bool {
        let Some(found) = &self.update.available else { return false };
        ui.label(RichText::new(t!("Corrent {new} is available.", new = found.release.version)).color(p.text));
        ui.add_space(4.0);
        primary_button(ui, p, &format!("{}  {}", icons::ARROW_CIRCLE_UP, found.action_label())).clicked()
    }

    pub(super) fn update_window(&mut self, ctx: &Context, p: &Palette) {
        let Some(d) = &mut self.update.dialog else { return };
        let mut finished = None;
        let mut was_cancelled = false;
        if let State::Working { rx, progress, cancel } = &mut d.state {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(pr) => *progress = Some(pr),
                    Msg::Done(r) => finished = Some(r),
                }
            }
            was_cancelled = cancel.load(Ordering::Relaxed);
        }
        match finished {
            Some(Ok(Outcome::Restart)) => {
                tracing::info!("update to {} is in place; restarting", d.found.release.version);
                d.state = State::Restarting;
                // The new version waits for this one: quit now (the engine saves and stops).
                self.quit_now(ctx);
                return;
            }
            Some(Ok(Outcome::Opened(file))) => d.state = State::Opened(file),
            Some(Err(_)) if was_cancelled => {
                self.update.dialog = None;
                return;
            }
            Some(Err(e)) => {
                tracing::warn!("update to {} failed: {e}", d.found.release.version);
                d.state = State::Failed(e);
            }
            None => {}
        }
        let Some(d) = &self.update.dialog else { return };
        let version = d.found.release.version.clone();
        let page = d.found.release.url.clone();
        let (mut start, mut close, mut quit) = (false, false, false);
        let mut copy = None;
        let r = egui::Modal::new(Id::new("update")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(
                RichText::new(t!("Update to Corrent {version}", version = version)).strong().size(16.0).color(p.text),
            );
            ui.add_space(6.0);
            match &d.state {
                State::Info => {
                    let (text, command) = match &d.found.kind {
                        InstallKind::Managed(m) => (m.advice(), Some(m.command())),
                        InstallKind::ReadOnly => (
                            t!("Corrent can't write to the folder it is in, so it can't update itself. The new version is on the release page.")
                                .to_string(),
                            None,
                        ),
                        _ => (
                            t!("This copy isn't one of Corrent's packages (it may be a build made from the source), so it can't update itself. The new version is on the release page.")
                                .to_string(),
                            None,
                        ),
                    };
                    ui.label(RichText::new(text).color(p.text));
                    if let Some(command) = command {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(command).monospace().color(p.accent));
                            if ui.small_button(t!("Copy")).clicked() {
                                copy = Some(command.to_string());
                            }
                        });
                    }
                    ui.add_space(6.0);
                    ui.hyperlink_to(RichText::new(t!("Open the release page")).small(), &page);
                    ui.add_space(12.0);
                    if secondary_button(ui, t!("Close")).clicked() {
                        close = true;
                    }
                }
                State::Working { progress, cancel, .. } => {
                    let what = if d.found.kind.installs_itself() {
                        t!("Corrent downloads the new version from its GitHub releases, checks it against the release's checksums, installs it and restarts. Your torrents carry on after the restart.")
                    } else {
                        t!("Corrent downloads the new version from its GitHub releases, checks it against the release's checksums and opens it.")
                    };
                    ui.label(RichText::new(what).small().color(p.text_dim));
                    ui.add_space(10.0);
                    let (frac, text) = match progress {
                        Some(Progress::Download { done, total: Some(t) }) if *t > 0 => (
                            *done as f32 / *t as f32,
                            t!(
                                "Downloading… {done} of {total}",
                                done = corrent_core::util::fmt_bytes(*done),
                                total = corrent_core::util::fmt_bytes(*t)
                            ),
                        ),
                        Some(Progress::Download { done, .. }) => {
                            (0.0, t!("Downloading… {done}", done = corrent_core::util::fmt_bytes(*done)))
                        }
                        Some(Progress::Checksums) => (0.0, t!("Reading the release's checksums…").to_string()),
                        Some(Progress::Install) => (1.0, t!("Installing…").to_string()),
                        None => (0.0, t!("Connecting…").to_string()),
                    };
                    ui.label(RichText::new(text).small().color(p.text_dim));
                    ui.add_space(4.0);
                    let w = ui.available_width();
                    widgets::progress_bar(ui, Id::new("update_bar"), frac, p.accent, p.bg3, 6.0, w, false, true);
                    ui.add_space(8.0);
                    // No way back by cancelling once the files are being replaced.
                    let installing = matches!(progress, Some(Progress::Install));
                    if ui.add_enabled(!installing, egui::Button::new(t!("Cancel"))).clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                State::Failed(e) => {
                    ui.label(RichText::new(t!("Corrent wasn't updated: {e}", e = e)).color(p.error));
                    ui.add_space(6.0);
                    ui.hyperlink_to(RichText::new(t!("Open the release page")).small(), &page);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, t!("Try again")).clicked() {
                            start = true;
                        }
                        if secondary_button(ui, t!("Close")).clicked() {
                            close = true;
                        }
                    });
                }
                State::Restarting => {
                    ui.label(
                        RichText::new(t!("The new version is in place. Corrent is closing and comes back in a moment…"))
                            .color(p.text),
                    );
                }
                State::Opened(file) => {
                    ui.label(
                        RichText::new(t!(
                            "The new version is downloaded and checked, and its disk image is open. Quit Corrent, then drag Corrent into Applications and replace the one that is there."
                        ))
                        .color(p.text),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(file.display().to_string()).small().color(p.text_mute));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, t!("Quit Corrent")).clicked() {
                            quit = true;
                        }
                        if secondary_button(ui, t!("Close")).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        let busy = matches!(d.state, State::Working { .. } | State::Restarting);
        if let Some(text) = copy {
            self.copy(text);
        }
        if start {
            self.start_update();
        } else if quit {
            self.update.dialog = None;
            self.quit_now(ctx);
        } else if close || (r.should_close() && !busy) {
            self.update.dialog = None;
        }
    }
}
