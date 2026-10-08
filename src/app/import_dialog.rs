//! "Import from another client" dialog.

use std::path::PathBuf;
use std::sync::mpsc;

use egui::{Align, Context, Id, Layout, RichText};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::engine::types::AddSource;
use crate::engine::{AddRequest, EngineCommand, ToastKind};
use crate::import::{Client, ImportItem};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{primary_button, secondary_button};

type Scanned = (Client, PathBuf, Result<Vec<ImportItem>, String>);

pub struct Source {
    pub client: Client,
    pub dir: PathBuf,
    pub result: Option<Result<Vec<ImportItem>, String>>,
    pub include: bool,
}

pub struct ImportDialog {
    pub sources: Vec<Source>,
    pub start_paused: bool,
    /// Client type for "Scan another folder…".
    pub other_client: Client,
    tx: mpsc::Sender<Scanned>,
    rx: mpsc::Receiver<Scanned>,
}

impl CorrentApp {
    pub fn open_import(&mut self) {
        let (tx, rx) = mpsc::channel();
        let mut d = ImportDialog { sources: Vec::new(), start_paused: true, other_client: Client::QBittorrent, tx, rx };
        for c in Client::ALL {
            if let Some(dir) = c.default_dir().filter(|d| d.is_dir()) {
                self.scan_import(&mut d, c, dir);
            }
        }
        self.import = Some(d);
    }

    fn scan_import(&self, d: &mut ImportDialog, client: Client, dir: PathBuf) {
        if d.sources.iter().any(|s| s.dir == dir) {
            return;
        }
        d.sources.push(Source { client, dir: dir.clone(), result: None, include: true });
        let (tx, ctx) = (d.tx.clone(), self.ctx.clone());
        self.rt.spawn_blocking(move || {
            let r = crate::import::scan(client, &dir);
            let _ = tx.send((client, dir, r));
            ctx.request_repaint();
        });
    }

    /// A folder picked for "Scan another folder…".
    pub fn import_folder_picked(&mut self, dir: PathBuf) {
        if let Some(mut d) = self.import.take() {
            let client = d.other_client;
            self.scan_import(&mut d, client, dir);
            self.import = Some(d);
        }
    }

    pub(super) fn import_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut d) = self.import.take() else { return };
        while let Ok((_, dir, r)) = d.rx.try_recv() {
            if let Some(s) = d.sources.iter_mut().find(|s| s.dir == dir) {
                s.result = Some(r);
            }
        }
        let present: std::collections::HashSet<_> = self.snap.torrents.iter().map(|t| t.hash).collect();
        let mut close = false;
        let mut import = false;
        let mut pick = false;
        let busy_picker = self.picker_busy();
        let resp = egui::Modal::new(Id::new("import")).show(ctx, |ui| {
            ui.set_width(620.0_f32.min(ctx.content_rect().width() - 60.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::ARROW_SQUARE_IN).size(20.0).color(p.accent));
                ui.label(RichText::new(t!("Import from another client")).font(theme::title()).color(p.text));
            });
            ui.label(
                RichText::new(
                    "Torrents keep their data where the other client saved it. Corrent checks the existing files \
                     and never downloads them again; categories and tags come along from qBittorrent.",
                )
                .small()
                .color(p.text_dim),
            );
            ui.add_space(10.0);
            if d.sources.is_empty() {
                ui.label(RichText::new(t!("No other torrent client was found in its usual place.")).color(p.text_dim));
            }
            for s in &mut d.sources {
                ui.horizontal(|ui| {
                    let (count, new) = match &s.result {
                        Some(Ok(items)) => (items.len(), items.iter().filter(|i| !present.contains(&i.hash)).count()),
                        _ => (0, 0),
                    };
                    ui.add_enabled(new > 0, egui::Checkbox::new(&mut s.include, ""));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(s.client.name()).strong().color(p.text));
                        ui.add(
                            egui::Label::new(RichText::new(s.dir.display().to_string()).small().color(p.text_mute))
                                .truncate(),
                        );
                        match &s.result {
                            None => {
                                ui.horizontal(|ui| {
                                    ui.add(egui::Spinner::new().size(12.0));
                                    ui.label(RichText::new(t!("Looking for torrents…")).small().color(p.text_dim));
                                });
                            }
                            Some(Err(e)) => {
                                ui.label(RichText::new(format!("{}  {e}", icons::WARNING)).small().color(p.warn));
                            }
                            Some(Ok(_)) => {
                                let already = count - new;
                                let mut t = if count == 1 {
                                    t!("1 torrent").to_string()
                                } else {
                                    t!("{count} torrents", count = count)
                                };
                                if already > 0 {
                                    t.push_str(&t!(" · {already} already in Corrent", already = already));
                                }
                                ui.label(RichText::new(t).small().color(p.text_dim));
                            }
                        }
                    });
                });
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("import_client").selected_text(d.other_client.name()).show_ui(ui, |ui| {
                    for c in Client::ALL {
                        ui.selectable_value(&mut d.other_client, c, c.name());
                    }
                });
                let hint = match d.other_client {
                    Client::QBittorrent => t!("Choose its BT_backup folder"),
                    Client::Transmission => t!("Choose the folder that contains “torrents” and “resume”"),
                    Client::Deluge => t!("Choose its “state” folder"),
                };
                if ui
                    .add_enabled(
                        !busy_picker,
                        egui::Button::new(format!("{}  {}", icons::FOLDER_OPEN, t!("Scan another folder…"))),
                    )
                    .on_hover_text(hint)
                    .clicked()
                {
                    pick = true;
                }
            });
            ui.add_space(8.0);
            ui.checkbox(&mut d.start_paused, t!("Add paused"))
                .on_hover_text(t!("Recommended while the other client still runs, so both don't use the same files"));
            ui.add_space(12.0);
            let total: usize = d
                .sources
                .iter()
                .filter(|s| s.include)
                .filter_map(|s| s.result.as_ref()?.as_ref().ok())
                .flatten()
                .filter(|i| !present.contains(&i.hash))
                .count();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if total == 1 {
                    t!("Import 1 torrent").to_string()
                } else {
                    t!("Import {total} torrents", total = total)
                };
                if ui.add_enabled_ui(total > 0, |ui| primary_button(ui, p, &label)).inner.clicked() {
                    import = true;
                }
                if secondary_button(ui, t!("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() {
            close = true;
        }
        if pick {
            self.pick(PickTarget::ImportFolder, PickKind::Folder, None);
        }
        if import {
            let mut seen = present.clone();
            let mut n = 0;
            for s in d.sources.iter().filter(|s| s.include) {
                let Some(Ok(items)) = &s.result else { continue };
                for it in items {
                    if !seen.insert(it.hash) {
                        continue;
                    }
                    n += 1;
                    self.engine.send(EngineCommand::Add(Box::new(AddRequest {
                        source: AddSource::Bytes { name: format!("{}.torrent", it.name), bytes: it.bytes.clone() },
                        torrent_bytes: Some(it.bytes.clone()),
                        save_path: it.save_path.clone().unwrap_or_else(|| self.settings.downloads.save_path.clone()),
                        // Other clients put multi-file torrents in a folder named after the torrent.
                        create_subfolder: it.save_path.is_some() || self.settings.downloads.create_subfolder,
                        category: it.category.clone(),
                        tags: it.tags.clone(),
                        paused: d.start_paused || it.paused,
                        only_files: None,
                        add_to_top: false,
                        initial_peers: None,
                    })));
                }
            }
            self.toasts.push(
                ToastKind::Info,
                if n == 1 {
                    t!("Importing 1 torrent; existing files are checked, not downloaded again.").to_string()
                } else {
                    t!("Importing {n} torrents; existing files are checked, not downloaded again.", n = n)
                },
            );
            return;
        }
        if !close {
            self.import = Some(d);
        }
    }
}
