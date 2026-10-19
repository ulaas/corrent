//! Choosing a folder on the server (when the app uses a remote server): the same targets as the
//! local folder picker, answered through the same channel.

use std::path::PathBuf;

use corrent_proto::server::FsListing;
use egui::{Id, RichText};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::media::client::{Slot, fetch, ready};
use super::pickers::PickTarget;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

pub struct ServerFs {
    seq: u64,
    target: PickTarget,
    path: String,
    listing: Option<Slot<FsListing>>,
}

impl CorrentApp {
    /// Opens the server's folder browser for a picker request (`start`: a server path).
    pub(super) fn open_server_fs(&mut self, seq: u64, target: PickTarget, start: Option<PathBuf>) {
        let path = start.map(|p| p.display().to_string()).unwrap_or_default();
        self.server_fs = Some(ServerFs { seq, target, path, listing: None });
    }

    fn server_api(&self) -> Option<super::media::client::MediaClient> {
        self.server.as_ref()?.status.route.load_full().map(|r| r.api.clone())
    }

    pub(super) fn server_fs_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut fs) = self.server_fs.take() else { return };
        if fs.listing.is_none()
            && let Some(api) = self.server_api()
        {
            let enc: String = url::form_urlencoded::byte_serialize(fs.path.as_bytes()).collect();
            fs.listing =
                Some(fetch(&self.rt, ctx, async move { api.get(&format!("/api/v1/server/fs?path={enc}")).await }));
        }
        let mut done: Option<Option<String>> = None;
        let mut go: Option<String> = None;
        let r = egui::Modal::new(Id::new("server_fs")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(
                RichText::new(format!("{}  {}", icons::HARD_DRIVES, t!("Choose a folder on the server")))
                    .strong()
                    .size(16.0),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(if fs.path.is_empty() { t!("Locations") } else { &fs.path })
                    .monospace()
                    .color(p.text_dim),
            );
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                match fs.listing.as_ref().and_then(ready) {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(p.error));
                    }
                    Some(Ok(l)) => {
                        if let Some(parent) = &l.parent
                            && ui.button(format!("{}  ..", icons::ARROW_UP)).clicked()
                        {
                            go = Some(parent.clone());
                        }
                        for e in l.entries.iter().filter(|e| e.dir) {
                            if ui
                                .add(egui::Button::new(format!("{}  {}", icons::FOLDER, e.name)).frame(false))
                                .clicked()
                            {
                                go = Some(e.path.clone());
                            }
                        }
                        if let Some(free) = l.free_space {
                            ui.label(
                                RichText::new(t!("{size} free", size = crate::util::fmt_bytes(free)))
                                    .small()
                                    .color(p.text_mute),
                            );
                        }
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(!fs.path.is_empty(), |ui| primary_button(ui, p, t!("Choose this folder")))
                        .inner
                        .clicked()
                    {
                        done = Some(Some(fs.path.clone()));
                    }
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        done = Some(None);
                    }
                });
            });
        });
        if r.should_close() && done.is_none() {
            done = Some(None);
        }
        if let Some(go) = go {
            fs.path = go;
            fs.listing = None;
        }
        match done {
            Some(choice) => {
                // An empty result ends the picker request like a cancelled native dialog.
                let paths = choice.map(|p| vec![PathBuf::from(p)]).unwrap_or_default();
                let _ = self.pickers.tx.send((fs.seq, fs.target, paths));
            }
            None => self.server_fs = Some(fs),
        }
    }
}
