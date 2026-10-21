//! Native file/folder pickers that don't block the UI thread.
//!
//! Dialogs run as `rfd::AsyncFileDialog` futures on the tokio runtime; results come back through
//! a channel and are applied to whichever field asked for them.

use crate::ui::theme::PaletteDefExt;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use super::CorrentApp;
use super::dialogs::Modal;
use crate::config::WatchFolder;
use crate::engine::types::AddSource;

#[derive(Clone, Debug, PartialEq)]
pub enum PickTarget {
    /// .torrent files to add.
    AddTorrents,
    /// Save folder of the add dialog with this request id.
    AddDialogFolder(u64),
    MoveFolder,
    CategoryFolder,
    SettingsSaveFolder,
    WatchFolderAdd,
    /// Keyed by the watch folder's path (row indices can shift while the picker is open).
    WatchFolderSave(PathBuf),
    BackupExport,
    BackupImport,
    DiagnosticsExport,
    OnboardingFolder,
    CreatorSource,
    CreatorOutput,
    RssRuleFolder,
    ThemeImport,
    ThemeExport,
    ImportFolder,
    LibraryFolder,
    /// A subtitle file for the playing video.
    SubtitleFile,
}

type PickResult = (u64, PickTarget, Vec<PathBuf>);

pub struct Pickers {
    pub(super) tx: Sender<PickResult>,
    rx: Receiver<PickResult>,
    /// The open picker: sequence id, target and when it was opened.
    pub open: Option<(u64, PickTarget, std::time::Instant)>,
    seq: u64,
}

impl Default for Pickers {
    fn default() -> Self {
        let (tx, rx) = channel();
        Pickers { tx, rx, open: None, seq: 0 }
    }
}

/// Subtitle formats mpv reads from a file.
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup", "smi", "sami", "txt"];

pub enum PickKind {
    Folder,
    TorrentFiles,
    AnyFile,
    SaveTorrent(String),
    SaveZip(String),
    OpenZip,
    SaveTheme(String),
    OpenTheme,
    Subtitle,
}

impl CorrentApp {
    /// Open a native picker without blocking. Only one picker is open at a time.
    pub fn pick(&mut self, target: PickTarget, kind: PickKind, start: Option<PathBuf>) {
        if self.picker_busy() {
            return;
        }
        self.pickers.seq += 1;
        let seq = self.pickers.seq;
        self.pickers.open = Some((seq, target.clone(), std::time::Instant::now()));
        // Using a server: its folders, not this computer's.
        let server_path = matches!(
            target,
            PickTarget::AddDialogFolder(_)
                | PickTarget::MoveFolder
                | PickTarget::CategoryFolder
                | PickTarget::LibraryFolder
        );
        if self.server.is_some() && server_path && matches!(kind, PickKind::Folder) {
            self.open_server_fs(seq, target, start);
            return;
        }
        let tx = self.pickers.tx.clone();
        let ctx = self.ctx.clone();
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = start.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        self.rt.spawn(async move {
            let paths: Vec<PathBuf> = match kind {
                PickKind::Folder => {
                    dialog.pick_folder().await.map(|h| vec![h.path().to_path_buf()]).unwrap_or_default()
                }
                PickKind::TorrentFiles => dialog
                    .add_filter(t!("Torrent files"), &["torrent"])
                    .pick_files()
                    .await
                    .map(|v| v.iter().map(|h| h.path().to_path_buf()).collect())
                    .unwrap_or_default(),
                PickKind::AnyFile => dialog.pick_file().await.map(|h| vec![h.path().to_path_buf()]).unwrap_or_default(),
                PickKind::SaveTorrent(name) => dialog
                    .add_filter(t!("Torrent file"), &["torrent"])
                    .set_file_name(name)
                    .save_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::SaveZip(name) => dialog
                    .add_filter(t!("Corrent backup"), &["zip"])
                    .set_file_name(name)
                    .save_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::SaveTheme(name) => dialog
                    .add_filter(t!("Corrent theme"), &["toml"])
                    .set_file_name(name)
                    .save_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::OpenTheme => dialog
                    .add_filter(t!("Corrent theme"), &["toml"])
                    .pick_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::Subtitle => dialog
                    .add_filter(t!("Subtitles"), SUBTITLE_EXTENSIONS)
                    .add_filter(t!("All files"), &["*"])
                    .pick_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::OpenZip => dialog
                    .add_filter(t!("Corrent backup"), &["zip"])
                    .pick_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
            };
            let _ = tx.send((seq, target, paths));
            ctx.request_repaint();
        });
    }

    /// Apply finished picker results (called every frame from `logic`).
    pub(super) fn drain_pickers(&mut self) {
        while let Ok((seq, target, paths)) = self.pickers.rx.try_recv() {
            if self.pickers.open.as_ref().is_some_and(|(s, _, _)| *s == seq) {
                self.pickers.open = None;
            }
            let Some(first) = paths.first().cloned() else { continue };
            let s = first.display().to_string();
            match target {
                PickTarget::AddTorrents => {
                    self.launch_queue.extend(paths.into_iter().map(AddSource::File));
                }
                PickTarget::AddDialogFolder(req) => {
                    if let Some(b) = self.batch.as_mut().filter(|b| b.req == req) {
                        b.save_path = s.clone();
                    }
                    if let Some(d) = self.add_dialogs.iter_mut().find(|d| d.req == req) {
                        d.save_path = s;
                    }
                }
                PickTarget::MoveFolder => {
                    if let Some(Modal::Move { path, .. }) = &mut self.modal {
                        *path = s;
                    }
                }
                PickTarget::CategoryFolder => {
                    if let Some(Modal::Category { path, .. }) = &mut self.modal {
                        *path = s;
                    }
                }
                PickTarget::SettingsSaveFolder => {
                    if let Some(d) = &mut self.settings_draft {
                        d.settings.downloads.save_path = first;
                    }
                }
                PickTarget::WatchFolderAdd => {
                    if let Some(d) = &mut self.settings_draft {
                        d.settings.watch.push(WatchFolder { path: first, ..Default::default() });
                    }
                }
                PickTarget::WatchFolderSave(folder) => {
                    if let Some(w) = self
                        .settings_draft
                        .as_mut()
                        .and_then(|d| d.settings.watch.iter_mut().find(|w| w.path == folder))
                    {
                        w.save_path = Some(first);
                    }
                }
                PickTarget::OnboardingFolder => self.settings.downloads.save_path = first,
                PickTarget::CreatorSource => {
                    if let Some(c) = &mut self.creator {
                        c.set_source(first, &self.rt, &self.ctx);
                    }
                }
                PickTarget::RssRuleFolder => {
                    if let Some(ed) = &mut self.feeds.rules
                        && let Some(r) = ed.rules.get_mut(ed.selected)
                    {
                        r.save_path = Some(first);
                    }
                }
                PickTarget::CreatorOutput => {
                    if let Some(c) = &mut self.creator {
                        c.output = Some(first);
                    }
                }
                PickTarget::BackupExport => self.export_backup(first),
                PickTarget::DiagnosticsExport => self.save_diagnostics(first),
                PickTarget::SubtitleFile => {
                    if let Some(pl) = &self.media.player {
                        pl.add_subtitle(&first);
                    }
                }
                PickTarget::BackupImport => self.import_backup(first),
                PickTarget::ImportFolder => self.import_folder_picked(first),
                PickTarget::LibraryFolder => self.library_folder_picked(first),
                PickTarget::ThemeImport => {
                    let result = std::fs::read_to_string(&first)
                        .map_err(|e| e.to_string())
                        .and_then(|t| crate::ui::theme::PaletteDef::from_toml(&t));
                    match (result, &mut self.settings_draft) {
                        (Ok(def), Some(d)) => {
                            self.toasts.push(
                                crate::engine::ToastKind::Success,
                                t!("Imported theme “{name}”", name = def.name),
                            );
                            d.settings.appearance.custom_palette = Some(def);
                            d.settings.appearance.preset = crate::config::PalettePreset::Custom;
                        }
                        (Ok(_), None) => {}
                        (Err(e), _) => self
                            .toasts
                            .push(crate::engine::ToastKind::Error, t!("Couldn't import the theme: {e}", e = e)),
                    }
                }
                PickTarget::ThemeExport => {
                    let def = self
                        .settings_draft
                        .as_ref()
                        .and_then(|d| d.settings.appearance.custom_palette.clone())
                        .or_else(|| self.settings.appearance.custom_palette.clone());
                    if let Some(def) = def {
                        match crate::util::write_atomic(&first, def.to_toml().as_bytes()) {
                            Ok(()) => self.toasts.push(crate::engine::ToastKind::Success, t!("Theme exported")),
                            Err(e) => self.toasts.push(
                                crate::engine::ToastKind::Error,
                                t!("Couldn't save the theme: {why}", why = crate::util::explain_io(&format!("{e:#}"))),
                            ),
                        }
                    }
                }
            }
        }
    }

    /// A picker is open. A picker that never reports back (e.g. a failed dialog backend) stops
    /// blocking new ones after 15 minutes.
    pub fn picker_busy(&self) -> bool {
        self.pickers.open.as_ref().is_some_and(|(_, _, at)| at.elapsed() < std::time::Duration::from_secs(900))
    }
}
