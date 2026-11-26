//! Taking media out of a library: the dialog that asks whether its files stay (the library then
//! ignores them) or are deleted, and a library's list of ignored files with "Restore".

use corrent_proto::library::{IgnoredFile, RemovalPlan, Removed};
use egui::{Id, RichText};
use egui_phosphor::regular as icons;

use super::Page;
use super::client::{Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::engine::ToastKind;
use crate::ui::theme::Palette;
use crate::ui::widgets::{danger_button, primary_button, secondary_button};

pub struct Removal {
    item: i64,
    title: String,
    plan: Option<Slot<RemovalPlan>>,
    /// The removal under way, and whether it deletes the files.
    doing: Option<(Slot<Removed>, bool)>,
}

pub struct Ignored {
    library: i64,
    name: String,
    list: Option<Slot<Vec<IgnoredFile>>>,
    restoring: Option<Slot<serde_json::Value>>,
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

impl CorrentApp {
    pub(super) fn open_removal(&mut self, item: i64, title: &str) {
        self.media.removal = Some(Removal { item, title: title.to_string(), plan: None, doing: None });
    }

    pub(super) fn open_ignored(&mut self, library: i64, name: &str) {
        self.media.ignored = Some(Ignored { library, name: name.to_string(), list: None, restoring: None });
    }

    pub(super) fn removal_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(client) = self.media.client.clone() else { return };
        let Some(mut f) = self.media.removal.take() else { return };
        if f.plan.is_none() {
            let (c, path) = (client.clone(), format!("/api/v1/items/{}/removal", f.item));
            f.plan = Some(fetch(&self.rt, ctx, async move { c.get_reason(&path).await }));
        }
        if let Some((r, deleting)) = f.doing.as_ref().and_then(|(s, d)| ready(s).map(|r| (r, *d))) {
            match r {
                Ok(done) => {
                    if let Some(first) = done.failed.first() {
                        self.toasts.push(
                            ToastKind::Error,
                            if done.failed.len() == 1 {
                                t!("1 file couldn't be deleted and stays in the library: {error}", error = first.error)
                            } else {
                                t!(
                                    "{n} files couldn't be deleted and stay in the library: {error}",
                                    n = done.failed.len(),
                                    error = first.error
                                )
                            },
                        );
                    } else if deleting {
                        self.toasts.push(ToastKind::Success, t!("Removed, and its files deleted.").to_string());
                    } else {
                        self.toasts
                            .push(ToastKind::Success, t!("Removed from the library. Its files stay.").to_string());
                    }
                    // Its page shows nothing any more.
                    if self.media.page == Some(Page::Item(f.item)) && done.failed.is_empty() {
                        self.media_back();
                    }
                    self.media.invalidate();
                    return;
                }
                Err(e) => {
                    self.toasts.push(ToastKind::Error, e);
                    f.doing = None;
                }
            }
        }
        let plan = f.plan.as_ref().and_then(ready);
        if f.title.is_empty()
            && let Some(Ok(plan)) = &plan
        {
            f.title = plan.title.clone();
        }
        let busy = f.doing.is_some();
        let mut close = false;
        let mut go: Option<bool> = None;
        let r = egui::Modal::new(Id::new("remove_item")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(
                RichText::new(format!("{}  {}", icons::TRASH, t!("Remove “{title}” from the library?", title = f.title)))
                    .strong()
                    .size(16.0),
            );
            ui.label(
                RichText::new(t!(
                    "Its files can stay where they are, and the library then leaves them out, or be deleted from the disk."
                ))
                .small()
                .color(p.text_dim),
            );
            ui.add_space(8.0);
            let mut held = false;
            match &plan {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(p.error));
                }
                Some(Ok(plan)) => {
                    let size = corrent_core::util::fmt_bytes(plan.size);
                    ui.label(
                        RichText::new(if plan.files.len() == 1 {
                            t!("1 file, {size}", size = size)
                        } else {
                            t!("{n} files, {size}", n = plan.files.len(), size = size)
                        })
                        .color(p.text),
                    );
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        for file in &plan.files {
                            ui.horizontal(|ui| {
                                let color = if file.media { p.text } else { p.text_dim };
                                crate::ui::widgets::truncated_plain(
                                    ui,
                                    RichText::new(file_name(&file.path)).small().color(color),
                                    80.0,
                                )
                                .on_hover_text(&file.path);
                                ui.label(
                                    RichText::new(corrent_core::util::fmt_bytes(file.size)).small().color(p.text_mute),
                                );
                            });
                        }
                    });
                    if !plan.torrents.is_empty() {
                        held = true;
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(t!(
                                "These files belong to a torrent in the list ({names}). To delete them, remove the torrent with its data.",
                                names = plan.torrents.join(", ")
                            ))
                            .small()
                            .color(p.warn),
                        );
                    }
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let loaded = matches!(plan, Some(Ok(_)));
                if busy {
                    ui.spinner();
                }
                if ui
                    .add_enabled_ui(loaded && !busy && !held, |ui| danger_button(ui, p, t!("Delete the files")))
                    .inner
                    .clicked()
                {
                    go = Some(true);
                }
                if ui
                    .add_enabled_ui(loaded && !busy, |ui| primary_button(ui, p, t!("Remove, keep the files")))
                    .inner
                    .clicked()
                {
                    go = Some(false);
                }
                // Not while it runs: what it did (and what it couldn't delete) must be shown.
                if ui.add_enabled_ui(!busy, |ui| secondary_button(ui, t!("Cancel"))).inner.clicked() {
                    close = true;
                }
            });
        });
        if !busy && (close || r.should_close()) {
            return;
        }
        if let Some(delete) = go {
            let (c, path) = (client.clone(), format!("/api/v1/items/{}?delete_files={delete}", f.item));
            f.doing = Some((
                fetch(&self.rt, ctx, async move {
                    c.send_reason(reqwest::Method::DELETE, &path, &serde_json::Value::Null).await
                }),
                delete,
            ));
        }
        self.media.removal = Some(f);
    }

    pub(super) fn ignored_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(client) = self.media.client.clone() else { return };
        let Some(mut f) = self.media.ignored.take() else { return };
        if let Some(r) = f.restoring.as_ref().and_then(ready) {
            f.restoring = None;
            f.list = None;
            match r {
                Ok(_) => {
                    self.toasts.push(ToastKind::Success, t!("Restored. The library is being scanned.").to_string());
                    self.media.invalidate();
                }
                Err(e) => self.toasts.push(ToastKind::Error, e),
            }
        }
        if f.list.is_none() {
            let (c, path) = (client.clone(), format!("/api/v1/libraries/{}/ignored", f.library));
            f.list = Some(fetch(&self.rt, ctx, async move { c.get_reason(&path).await }));
        }
        let mut close = false;
        let mut restore: Option<Vec<String>> = None;
        let r = egui::Modal::new(Id::new("ignored_files")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.label(
                RichText::new(format!("{}  {}", icons::EYE_SLASH, t!("Ignored files of “{name}”", name = f.name)))
                    .strong()
                    .size(16.0),
            );
            ui.label(
                RichText::new(t!(
                    "Media removed from this library with its files kept. Scans skip these files until they are restored."
                ))
                .small()
                .color(p.text_dim),
            );
            ui.add_space(8.0);
            let list = f.list.as_ref().and_then(ready);
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| match &list {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(p.error));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(RichText::new(t!("Nothing is ignored.")).color(p.text_dim));
                }
                Some(Ok(list)) => {
                    for file in list {
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add_enabled(f.restoring.is_none(), egui::Button::new(t!("Restore")))
                                    .clicked()
                                {
                                    restore = Some(vec![file.path.clone()]);
                                }
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.label(RichText::new(&file.title).color(p.text));
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(file_name(&file.path)).small().monospace().color(p.text_mute),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&file.path);
                                });
                            });
                        });
                        ui.add_space(2.0);
                    }
                }
            });
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_button(ui, p, t!("Close")).clicked() {
                    close = true;
                }
                let all: Vec<String> =
                    list.and_then(|l| l.ok()).map(|l| l.into_iter().map(|i| i.path).collect()).unwrap_or_default();
                if all.len() > 1
                    && ui.add_enabled(f.restoring.is_none(), egui::Button::new(t!("Restore all"))).clicked()
                {
                    restore = Some(all);
                }
            });
        });
        if close || r.should_close() {
            return;
        }
        if let Some(paths) = restore {
            let (c, path) = (client.clone(), format!("/api/v1/libraries/{}/ignored/restore", f.library));
            f.restoring = Some(fetch(&self.rt, ctx, async move {
                c.send_reason(reqwest::Method::POST, &path, &serde_json::json!({ "paths": paths })).await
            }));
        }
        self.media.ignored = Some(f);
    }
}
