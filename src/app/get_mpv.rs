//! Getting mpv for playback where no package manager provides it (Windows): libmpv is not part
//! of Corrent's releases (docs/PACKAGING.md), so on the user's request the app downloads it from
//! its makers (`corrent_core::mpv_get`) into `corrent-lib` next to the executable, where the
//! player and the `corrent-mpvw` worker look first. Elsewhere the dialog says how to install it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use corrent_core::mpv_get::{self, Progress};
use egui::{Context, Id, RichText};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::ToastKind;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, primary_button, secondary_button};

pub struct GetMpvDialog {
    /// Opened because something was to be played (says so), or from Settings.
    for_playback: bool,
    state: State,
}

enum State {
    Ask,
    Working { rx: mpsc::Receiver<Msg>, progress: Option<Progress>, cancel: Arc<AtomicBool> },
    Failed(String),
}

enum Msg {
    Progress(Progress),
    /// The loaded library's version, or why it failed.
    Done(Result<String, String>),
}

/// Where a downloaded libmpv goes: `corrent-lib` next to the executable.
pub fn folder() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(corrent_mpv::LIB_DIR))
}

/// Whether this system gets mpv through the dialog's download (Windows) or a package manager.
pub const CAN_DOWNLOAD: bool = cfg!(windows);

/// Whether an error from opening libmpv means it isn't installed (not that it failed).
pub fn is_missing(e: &corrent_mpv::Error) -> bool {
    matches!(e, corrent_mpv::Error::NotAvailable(_))
}

impl CorrentApp {
    pub(super) fn open_get_mpv(&mut self, for_playback: bool) {
        if self.get_mpv.as_ref().is_some_and(|d| matches!(d.state, State::Working { .. })) {
            return;
        }
        self.get_mpv = Some(GetMpvDialog { for_playback, state: State::Ask });
    }

    fn start_get_mpv(&mut self) {
        let Some(dir) = folder() else {
            if let Some(d) = &mut self.get_mpv {
                d.state = State::Failed(t!("Corrent's own folder couldn't be found.").into());
            }
            return;
        };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c2, ctx, proxy) = (cancel.clone(), self.ctx.clone(), self.settings.connection.proxy_url());
        self.rt.spawn(async move {
            let client = mpv_get::client(proxy);
            let (tx2, ctx2) = (tx.clone(), ctx.clone());
            let progress = move |p| {
                let _ = tx2.send(Msg::Progress(p));
                ctx2.request_repaint();
            };
            let r = match mpv_get::install(&client, &dir, progress, &c2).await {
                // Load it now: the player and Settings see it at once, and a library that
                // doesn't load is reported here rather than at the next play.
                Ok((path, _)) => tokio::task::spawn_blocking(move || match corrent_mpv::Library::get() {
                    Ok(l) => Ok(l.version_string()),
                    Err(e) => {
                        let _ = std::fs::remove_file(&path);
                        Err(e.to_string())
                    }
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
                Err(e) => Err(e),
            };
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
        if let Some(d) = &mut self.get_mpv {
            d.state = State::Working { rx, progress: None, cancel };
        }
    }

    pub(super) fn get_mpv_window(&mut self, ctx: &Context, p: &Palette) {
        let Some(d) = &mut self.get_mpv else { return };
        let mut finished = None;
        if let State::Working { rx, progress, .. } = &mut d.state {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(pr) => *progress = Some(pr),
                    Msg::Done(r) => finished = Some(r),
                }
            }
        }
        match finished {
            Some(Ok(version)) => {
                self.get_mpv = None;
                // Settings › Playback asks mpv for its sound devices again.
                self.media.audio_devices = None;
                self.toasts.push(
                    ToastKind::Success,
                    t!("mpv {version} is ready: videos and music play now.", version = version),
                );
                return;
            }
            Some(Err(e)) => d.state = State::Failed(e),
            None => {}
        }
        let (mut start, mut close) = (false, false);
        let r = egui::Modal::new(Id::new("get_mpv")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(t!("Get mpv for playback")).strong().size(16.0).color(p.text));
            ui.add_space(4.0);
            if d.for_playback {
                ui.label(RichText::new(t!("Playing needs mpv, which isn't installed yet.")).color(p.text));
                ui.add_space(4.0);
            }
            if !CAN_DOWNLOAD {
                ui.label(
                    RichText::new(t!(
                        "Install mpv with your system's package manager (on macOS: brew install mpv), then play again."
                    ))
                    .color(p.text_dim),
                );
                ui.add_space(12.0);
                if secondary_button(ui, t!("Close")).clicked() {
                    close = true;
                }
                return;
            }
            ui.label(
                RichText::new(t!(
                    "Corrent plays videos and music with mpv's library. It isn't part of Corrent: Corrent can \
                     download it for you from the mpv-winbuild-cmake project on GitHub (about 30 MB, checked \
                     against a known checksum). It is free software under the GPL."
                ))
                .small()
                .color(p.text_dim),
            );
            ui.add_space(4.0);
            ui.hyperlink_to(RichText::new(mpv_get::PROJECT_URL).small(), mpv_get::PROJECT_URL);
            if let Some(dir) = folder() {
                ui.label(RichText::new(t!("Into: {folder}", folder = dir.display())).small().color(p.text_mute));
            }
            ui.add_space(12.0);
            match &d.state {
                State::Ask => {
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, &format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Download"))).clicked() {
                            start = true;
                        }
                        if secondary_button(ui, t!("Cancel")).clicked() {
                            close = true;
                        }
                    });
                }
                State::Working { progress, cancel, .. } => {
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
                        Some(Progress::Unpack) => (1.0, t!("Unpacking…").to_string()),
                        None => (0.0, t!("Connecting…").to_string()),
                    };
                    ui.label(RichText::new(text).small().color(p.text_dim));
                    ui.add_space(4.0);
                    let w = ui.available_width();
                    widgets::progress_bar(ui, Id::new("get_mpv_bar"), frac, p.accent, p.bg3, 6.0, w, false, true);
                    ui.add_space(8.0);
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
                State::Failed(e) => {
                    ui.label(RichText::new(t!("Couldn't get mpv: {e}", e = e)).color(p.error));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, t!("Try again")).clicked() {
                            start = true;
                        }
                        if secondary_button(ui, t!("Close")).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        let working = matches!(d.state, State::Working { .. });
        if start {
            self.start_get_mpv();
        } else if close || (r.should_close() && !working) {
            self.get_mpv = None;
        }
    }
}
