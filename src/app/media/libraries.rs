//! Managing libraries from the desktop app: list, add (name, kind, folders), scan, remove.
//! Folders are paths on the computer the server runs on (picked here in local mode, typed for
//! a remote server).

use corrent_proto::library::{Library, LibraryKind};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{primary_button, secondary_button};

pub struct LibraryForm {
    pub name: String,
    pub kind: LibraryKind,
    pub folder: String,
    saving: Option<Slot<Library>>,
    error: Option<String>,
}

impl Default for LibraryForm {
    fn default() -> Self {
        LibraryForm { name: String::new(), kind: LibraryKind::Movies, folder: String::new(), saving: None, error: None }
    }
}

pub fn kind_label(k: LibraryKind) -> &'static str {
    match k {
        LibraryKind::Movies => t!("Movies"),
        LibraryKind::Shows => t!("Shows"),
        LibraryKind::Music => t!("Music"),
        LibraryKind::Audiobooks => t!("Audiobooks"),
        LibraryKind::Videos => t!("Videos"),
        LibraryKind::Photos => t!("Photos"),
        LibraryKind::Podcasts => t!("Podcasts"),
    }
}

impl CorrentApp {
    /// The picker chose a library folder.
    pub fn library_folder_picked(&mut self, path: std::path::PathBuf) {
        let form = self.media.library_form.get_or_insert_with(Default::default);
        form.folder = path.display().to_string();
        if form.name.trim().is_empty() {
            form.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
    }

    fn library_action(&mut self, client: &MediaClient, method: reqwest::Method, path: String) {
        let c = client.clone();
        let tx = self.ui_events_tx.clone();
        self.rt.spawn(async move {
            if let Err(e) = c.send::<serde_json::Value, _>(method, &path, &serde_json::json!({})).await {
                let _ = tx.send(crate::engine::EngineEvent::Toast(crate::engine::ToastKind::Error, e));
            }
        });
        self.media.invalidate();
    }

    pub(super) fn libraries_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        let libs = self.media.libraries();
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label(
                RichText::new(t!(
                    "Libraries are folders the server scans for movies, shows, music and photos. New files appear on their own."
                ))
                .color(p.text_dim),
            );
        });
        ui.add_space(10.0);
        let mut remove = None;
        for l in &libs {
            egui::Frame::new()
                .fill(p.bg2)
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(12)
                .outer_margin(egui::Margin { left: 24, right: 24, top: 0, bottom: 8 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&l.name).strong().color(p.text));
                            let count = if l.item_count == 1 {
                                t!("1 item").to_string()
                            } else {
                                t!("{n} items", n = l.item_count)
                            };
                            ui.label(
                                RichText::new(format!("{} · {count}", kind_label(l.kind))).small().color(p.text_dim),
                            );
                            for f in &l.folders {
                                ui.label(RichText::new(f).small().monospace().color(p.text_mute));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(format!("{}  {}", icons::TRASH, t!("Remove"))).clicked() {
                                remove = Some((l.id, l.name.clone()));
                            }
                            if let Some(pct) = l.scanning {
                                ui.label(RichText::new(t!("Scanning… {pct}%", pct = pct)).small().color(p.text_dim));
                            } else if ui.button(format!("{}  {}", icons::ARROWS_CLOCKWISE, t!("Scan now"))).clicked() {
                                self.library_action(
                                    client,
                                    reqwest::Method::POST,
                                    format!("/api/v1/libraries/{}/scan", l.id),
                                );
                            }
                        });
                    });
                });
        }
        if let Some((id, name)) = remove {
            self.media.remove_library = Some((id, name));
        }
        self.remove_library_confirm(ui.ctx(), p, client);

        // Adding one.
        let local = self.server.is_none();
        let form = self.media.library_form.get_or_insert_with(Default::default);
        if let Some(r) = form.saving.as_ref().and_then(ready) {
            form.saving = None;
            match r {
                Ok(_) => {
                    *form = LibraryForm::default();
                    self.media.invalidate();
                    self.media.library_form = Some(LibraryForm::default());
                    return;
                }
                Err(e) => form.error = Some(e),
            }
        }
        let mut pick = false;
        let mut save = None;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(12)
            .outer_margin(egui::Margin { left: 24, right: 24, top: 8, bottom: 8 })
            .show(ui, |ui| {
                ui.label(RichText::new(t!("Add a library")).strong().color(p.text));
                ui.add_space(6.0);
                egui::Grid::new("new_library").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(t!("Name"));
                    ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(320.0));
                    ui.end_row();
                    ui.label(t!("Kind"));
                    egui::ComboBox::from_id_salt("library_kind").selected_text(kind_label(form.kind)).show_ui(
                        ui,
                        |ui| {
                            for k in LibraryKind::ALL {
                                ui.selectable_value(&mut form.kind, k, kind_label(k));
                            }
                        },
                    );
                    ui.end_row();
                    ui.label(t!("Folder"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut form.folder)
                                .hint_text(if local { "" } else { t!("A folder on the server") })
                                .desired_width(320.0),
                        );
                        if secondary_button(ui, t!("Choose…")).clicked() {
                            pick = true;
                        }
                    });
                    ui.end_row();
                });
                if let Some(e) = &form.error {
                    ui.label(RichText::new(e).color(p.error));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ok = form.saving.is_none() && !form.name.trim().is_empty() && !form.folder.trim().is_empty();
                    if ui.add_enabled_ui(ok, |ui| primary_button(ui, p, t!("Add library"))).inner.clicked() {
                        save = Some(serde_json::json!({
                            "name": form.name.trim(),
                            "kind": form.kind,
                            "folders": [form.folder.trim()],
                        }));
                    }
                    if form.saving.is_some() {
                        ui.spinner();
                    }
                });
            });
        if let Some(body) = save {
            let c = client.clone();
            if let Some(form) = &mut self.media.library_form {
                form.error = None;
                form.saving = Some(fetch(&self.rt, &self.ctx, async move {
                    c.send(reqwest::Method::POST, "/api/v1/libraries", &body).await
                }));
            }
        }
        if pick {
            let start = self
                .media
                .library_form
                .as_ref()
                .map(|f| std::path::PathBuf::from(&f.folder))
                .filter(|p| !local || p.is_dir());
            self.pick(crate::app::pickers::PickTarget::LibraryFolder, crate::app::pickers::PickKind::Folder, start);
        }
    }

    fn remove_library_confirm(&mut self, ctx: &egui::Context, p: &Palette, client: &MediaClient) {
        let Some((id, name)) = self.media.remove_library.clone() else { return };
        let mut close = false;
        let r = egui::Modal::new(egui::Id::new("remove_library")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(RichText::new(t!("Remove the library “{name}”?", name = name)).strong().size(16.0));
            ui.label(
                RichText::new(t!("Its files stay on disk; only the library and its watch history go."))
                    .color(p.text_dim),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(t!("Remove")).color(egui::Color32::WHITE)).fill(p.error))
                        .clicked()
                    {
                        self.library_action(client, reqwest::Method::DELETE, format!("/api/v1/libraries/{id}"));
                        close = true;
                    }
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        close = true;
                    }
                });
            });
        });
        if close || r.should_close() {
            self.media.remove_library = None;
        }
    }
}
