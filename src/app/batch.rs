//! "Add N torrents" dialog: one dialog for many sources with shared options.

use crate::ui::widgets::Labeled;
use std::path::PathBuf;

use egui::{Align, Context, Id, Layout, RichText, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::engine::types::AddSource;
use crate::engine::{AddRequest, EngineCommand};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{primary_button, secondary_button};

pub struct BatchDialog {
    pub req: u64,
    pub items: Vec<(AddSource, bool)>,
    pub save_path: String,
    pub category: Option<String>,
    pub tags: String,
    pub start_paused: bool,
    pub create_subfolder: bool,
}

impl CorrentApp {
    /// Queue several sources at once: one shared dialog instead of one per torrent.
    pub fn begin_batch(&mut self, sources: Vec<AddSource>) {
        if let Some(b) = &mut self.batch {
            for s in sources {
                if !b.items.iter().any(|(x, _)| *x == s) {
                    b.items.push((s, true));
                }
            }
            return;
        }
        self.next_req += 1;
        let d = &self.settings.downloads;
        self.batch = Some(BatchDialog {
            req: self.next_req,
            items: sources.into_iter().map(|s| (s, true)).collect(),
            save_path: d.save_path.display().to_string(),
            category: None,
            tags: String::new(),
            start_paused: d.start_paused,
            create_subfolder: d.create_subfolder,
        });
    }

    pub(super) fn batch_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut b) = self.batch.take() else { return };
        let mut close = false;
        let mut add = false;
        let mut browse = false;
        let categories = self.snap.categories.clone();
        let busy = self.picker_busy();
        let resp = egui::Modal::new(Id::new("batch_dialog")).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::STACK_PLUS).size(20.0).color(p.accent));
                let n = b.items.iter().filter(|(_, on)| *on).count();
                ui.label(RichText::new(t!("Add {n} torrents", n = n)).font(theme::title()).color(p.text));
            });
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(p.bg0)
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                        for (src, on) in &mut b.items {
                            ui.horizontal(|ui| {
                                ui.checkbox(on, "");
                                let icon = match src {
                                    AddSource::Magnet(_) => icons::MAGNET,
                                    AddSource::Url(_) => icons::LINK,
                                    _ => icons::FILE,
                                };
                                ui.label(RichText::new(icon).color(p.text_dim));
                                ui.add(
                                    egui::Label::new(RichText::new(src.describe()).color(if *on {
                                        p.text
                                    } else {
                                        p.text_mute
                                    }))
                                    .truncate(),
                                );
                            });
                        }
                    });
                });
            ui.add_space(10.0);
            egui::Grid::new("batch_opts").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(t!("Save to"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut b.save_path).desired_width(420.0));
                    if busy {
                        ui.add(egui::Spinner::new());
                    } else if ui.button(icons::FOLDER_OPEN).labeled(t!("Browse…")).clicked() {
                        browse = true;
                    }
                });
                ui.end_row();
                ui.label(t!("Category"));
                egui::ComboBox::from_id_salt("batch_cat")
                    .selected_text(b.category.clone().unwrap_or_else(|| t!("None").into()))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(b.category.is_none(), t!("None")).clicked() {
                            b.category = None;
                        }
                        for (c, info) in &categories {
                            if ui.selectable_label(b.category.as_ref() == Some(c), c).clicked() {
                                b.category = Some(c.clone());
                                if let Some(path) = &info.save_path {
                                    b.save_path = path.display().to_string();
                                }
                            }
                        }
                    });
                ui.end_row();
                ui.label(t!("Tags"));
                ui.add(egui::TextEdit::singleline(&mut b.tags).hint_text(t!("comma-separated")).desired_width(300.0));
                ui.end_row();
            });
            ui.checkbox(&mut b.start_paused, t!("Add paused"));
            ui.checkbox(&mut b.create_subfolder, t!("Create subfolder for multi-file torrents"));
            ui.label(
                RichText::new(t!(
                    "All files of each torrent are selected; magnet links are resolved in the background."
                ))
                .small()
                .color(p.text_dim),
            );
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let any = b.items.iter().any(|(_, on)| *on) && !b.save_path.trim().is_empty();
                if ui
                    .add_enabled_ui(any, |ui| {
                        primary_button(ui, p, &format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Add all")))
                    })
                    .inner
                    .clicked()
                {
                    add = true;
                }
                if secondary_button(ui, t!("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() {
            close = true;
        }
        if browse {
            self.pick(PickTarget::AddDialogFolder(b.req), PickKind::Folder, Some(PathBuf::from(&b.save_path)));
        }
        if add {
            let tags: Vec<String> = b.tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
            let save = PathBuf::from(b.save_path.trim());
            for (source, on) in b.items.drain(..) {
                if !on {
                    continue;
                }
                self.engine.send(EngineCommand::Add(Box::new(AddRequest {
                    source,
                    torrent_bytes: None,
                    save_path: save.clone(),
                    create_subfolder: b.create_subfolder,
                    category: b.category.clone(),
                    tags: tags.clone(),
                    paused: b.start_paused,
                    only_files: None,
                    add_to_top: false,
                    initial_peers: None,
                })));
            }
            if self.settings.downloads.remember_last_path {
                self.settings.downloads.push_recent(save);
                self.save_settings();
            }
        } else if !close {
            self.batch = Some(b);
        }
    }
}
