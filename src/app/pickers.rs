//! Native file/folder pickers that don't block the UI thread.
//!
//! Dialogs run as `rfd::AsyncFileDialog` futures on the tokio runtime; results come back through
//! a channel and are applied to whichever field asked for them.

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
    WatchFolderSave(usize),
    BackupExport,
    BackupImport,
}

pub struct Pickers {
    tx: Sender<(PickTarget, Vec<PathBuf>)>,
    rx: Receiver<(PickTarget, Vec<PathBuf>)>,
    pub open: Option<PickTarget>,
}

impl Default for Pickers {
    fn default() -> Self {
        let (tx, rx) = channel();
        Pickers { tx, rx, open: None }
    }
}

pub enum PickKind {
    Folder,
    TorrentFiles,
    AnyFile,
    SaveTorrent(String),
    SaveZip(String),
    OpenZip,
}

impl CorrentApp {
    /// Open a native picker without blocking. Only one picker is open at a time.
    pub fn pick(&mut self, target: PickTarget, kind: PickKind, start: Option<PathBuf>) {
        if self.pickers.open.is_some() {
            return;
        }
        self.pickers.open = Some(target.clone());
        let tx = self.pickers.tx.clone();
        let ctx = self.ctx.clone();
        let mut dialog = rfd::AsyncFileDialog::new();
        if let Some(dir) = start.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        self.rt.spawn(async move {
            let paths: Vec<PathBuf> = match kind {
                PickKind::Folder => dialog.pick_folder().await.map(|h| vec![h.path().to_path_buf()]).unwrap_or_default(),
                PickKind::TorrentFiles => dialog
                    .add_filter("Torrent files", &["torrent"])
                    .pick_files()
                    .await
                    .map(|v| v.iter().map(|h| h.path().to_path_buf()).collect())
                    .unwrap_or_default(),
                PickKind::AnyFile => dialog.pick_file().await.map(|h| vec![h.path().to_path_buf()]).unwrap_or_default(),
                PickKind::SaveTorrent(name) => dialog
                    .add_filter("Torrent file", &["torrent"])
                    .set_file_name(name)
                    .save_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::SaveZip(name) => dialog
                    .add_filter("Corrent backup", &["zip"])
                    .set_file_name(name)
                    .save_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
                PickKind::OpenZip => dialog
                    .add_filter("Corrent backup", &["zip"])
                    .pick_file()
                    .await
                    .map(|h| vec![h.path().to_path_buf()])
                    .unwrap_or_default(),
            };
            let _ = tx.send((target, paths));
            ctx.request_repaint();
        });
    }

    /// Apply finished picker results (called every frame from `logic`).
    pub(super) fn drain_pickers(&mut self) {
        while let Ok((target, paths)) = self.pickers.rx.try_recv() {
            if self.pickers.open.as_ref() == Some(&target) {
                self.pickers.open = None;
            }
            let Some(first) = paths.first().cloned() else { continue };
            let s = first.display().to_string();
            match target {
                PickTarget::AddTorrents => {
                    self.launch_queue.extend(paths.into_iter().map(AddSource::File));
                }
                PickTarget::AddDialogFolder(req) => {
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
                PickTarget::WatchFolderSave(i) => {
                    if let Some(w) = self.settings_draft.as_mut().and_then(|d| d.settings.watch.get_mut(i)) {
                        w.save_path = Some(first);
                    }
                }
                PickTarget::BackupExport => self.export_backup(first),
                PickTarget::BackupImport => self.import_backup(first),
            }
        }
    }

    pub fn picker_busy(&self) -> bool {
        self.pickers.open.is_some()
    }
}
