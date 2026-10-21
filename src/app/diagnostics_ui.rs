//! ⋮ → Save diagnostics… (STABILITY.md O4): shows what the bundle will hold, lets the user add
//! paths and info hashes, then writes it off the UI thread.

use egui::{Context, Id, RichText};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::engine::{EngineEvent, ToastKind};
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};
use corrent_core::diagnostics::{self, Extra, Options};

pub struct DiagnosticsDialog {
    /// The entries, listed off the UI thread when the dialog opens.
    entries: Option<Vec<String>>,
    rx: std::sync::mpsc::Receiver<Vec<String>>,
    pub opts: Options,
}

impl CorrentApp {
    pub(super) fn open_diagnostics(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (paths, ctx) = (self.paths.clone(), self.ctx.clone());
        self.rt.spawn_blocking(move || {
            let _ = tx.send(diagnostics::entries(&paths));
            ctx.request_repaint();
        });
        self.diagnostics = Some(DiagnosticsDialog { entries: None, rx, opts: Options::default() });
    }

    pub(super) fn diagnostics_window(&mut self, ctx: &Context, p: &Palette) {
        let Some(d) = &mut self.diagnostics else { return };
        if d.entries.is_none()
            && let Ok(e) = d.rx.try_recv()
        {
            d.entries = Some(e);
        }
        let mut save = false;
        let mut close = false;
        let r = egui::Modal::new(Id::new("diagnostics")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(t!("Save diagnostics")).strong().size(16.0).color(p.text));
            ui.add_space(4.0);
            ui.label(
                RichText::new(t!(
                    "A zip file to attach to a bug report: logs, crash reports, settings without passwords, keys or \
                     feed addresses, and counts of your torrents. Nothing is sent anywhere."
                ))
                .small()
                .color(p.text_dim),
            );
            ui.add_space(8.0);
            match &d.entries {
                None => {
                    ui.spinner();
                }
                Some(entries) => {
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for e in entries {
                            ui.label(RichText::new(e).monospace().small().color(p.text_dim));
                        }
                    });
                }
            }
            ui.add_space(8.0);
            ui.checkbox(&mut d.opts.paths, t!("Include file and folder paths"));
            ui.checkbox(&mut d.opts.hashes, t!("Include torrent info hashes"));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if primary_button(ui, p, &format!("{}  {}", icons::FLOPPY_DISK, t!("Save…"))).clicked() {
                    save = true;
                }
                if secondary_button(ui, t!("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if save {
            let name = format!("corrent-diagnostics-{}.zip", chrono_stamp());
            self.pick(PickTarget::DiagnosticsExport, PickKind::SaveZip(name), None);
        }
        if close || r.should_close() {
            self.diagnostics = None;
        }
    }

    /// The picker chose `dest`: write the bundle there.
    pub(super) fn save_diagnostics(&mut self, dest: std::path::PathBuf) {
        let Some(d) = self.diagnostics.take() else { return };
        let snap = self.snap.clone();
        let mut by_state: Vec<(String, usize)> = Vec::new();
        for t in &snap.torrents {
            let s = format!("{:?}", t.state);
            match by_state.iter_mut().find(|(k, _)| *k == s) {
                Some((_, n)) => *n += 1,
                None => by_state.push((s, 1)),
            }
        }
        let torrents: Vec<String> = by_state.iter().map(|(s, n)| format!("{s} {n}")).collect();
        let s = &snap.session;
        let extra = Extra {
            app: format!("Corrent {}", crate::version_text()),
            system: vec![
                ("language".into(), format!("{:?}", corrent_i18n::current())),
                ("scale".into(), format!("{:.2}", self.ctx.pixels_per_point())),
                ("engine".into(), if self.server.is_some() { "a server".into() } else { "this computer".into() }),
                ("tv mode".into(), self.tv.on.to_string()),
            ],
            metrics: vec![
                ("engine state".into(), format!("{:?}", snap.engine_state)),
                ("torrents".into(), format!("{} ({})", snap.torrents.len(), torrents.join(", "))),
                ("categories".into(), snap.categories.len().to_string()),
                ("tags".into(), snap.tags.len().to_string()),
                ("slowest tick of the last 60 (ms)".into(), s.tick_max_ms.to_string()),
                ("peers".into(), s.peers_live.to_string()),
                ("dht nodes".into(), s.dht_nodes.map(|n| n.to_string()).unwrap_or_default()),
                ("refused commands".into(), self.engine.refused().to_string()),
                ("free space (save folder)".into(), snap.free_space.map(|f| f.to_string()).unwrap_or_default()),
            ],
            health: snap.health.clone(),
        };
        let (paths, settings, opts) = (self.paths.clone(), self.settings.clone(), d.opts);
        let (tx, ctx) = (self.ui_events_tx.clone(), self.ctx.clone());
        self.rt.spawn_blocking(move || {
            let ev = match diagnostics::write(&paths, &settings, &extra, opts, &dest) {
                Ok(_) => (ToastKind::Success, t!("Saved diagnostics to {path}", path = dest.display())),
                Err(e) => (ToastKind::Error, t!("Couldn't save diagnostics: {e}", e = format!("{e:#}"))),
            };
            let _ = tx.send(EngineEvent::Toast(ev.0, ev.1));
            ctx.request_repaint();
        });
    }
}

/// `20261021-1530` for the default file name.
fn chrono_stamp() -> String {
    let ts = corrent_core::util::now_unix();
    corrent_core::util::fmt_date(ts).replace(['-', ':'], "").replace(' ', "-")
}
