//! Managing libraries from the desktop app: list, add (name, kind, folders), edit (name, folders,
//! settings), scan, remove.
//! Folders are paths on the computer the server runs on (picked here in local mode, typed for
//! a remote server).

use corrent_proto::library::{ImportMode, Library, LibraryKind, LibrarySettings};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, primary_button, secondary_button, toggle_chip};

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

/// The Edit dialog of a library. Its kind can't change: the items were placed by it.
pub struct LibraryEdit {
    id: i64,
    kind: LibraryKind,
    name: String,
    folders: Vec<String>,
    settings: LibrarySettings,
    saving: Option<Slot<Library>>,
    error: Option<String>,
}

impl LibraryEdit {
    pub fn new(l: &Library) -> LibraryEdit {
        let folders = if l.folders.is_empty() { vec![String::new()] } else { l.folders.clone() };
        LibraryEdit {
            id: l.id,
            kind: l.kind,
            name: l.name.clone(),
            folders,
            settings: l.settings.clone(),
            saving: None,
            error: None,
        }
    }

    /// The `PATCH` body, or what is missing. Settings the dialog doesn't show (seeding goals,
    /// quality profile) go back as they came.
    fn body(&self) -> Result<serde_json::Value, &'static str> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(t!("Give the library a name."));
        }
        let mut folders: Vec<&str> = Vec::new();
        for f in self.folders.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
            if !folders.contains(&f) {
                folders.push(f);
            }
        }
        if folders.is_empty() {
            return Err(t!("Add at least one folder."));
        }
        let mut settings = self.settings.clone();
        settings.played_threshold = settings.played_threshold.clamp(50, 100);
        settings.language = settings.language.trim().to_string();
        Ok(serde_json::json!({ "name": name, "folders": folders, "settings": settings }))
    }
}

fn import_mode_label(m: ImportMode) -> &'static str {
    match m {
        ImportMode::InPlace => t!("Leave the files where they are"),
        ImportMode::Hardlink => t!("Hard-link into the library"),
        ImportMode::Copy => t!("Copy into the library"),
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

    /// The picker chose folder `index` of the library being edited.
    pub fn library_edit_folder_picked(&mut self, index: usize, path: std::path::PathBuf) {
        if let Some(e) = &mut self.media.library_edit
            && let Some(f) = e.folders.get_mut(index)
        {
            *f = path.display().to_string();
        }
    }

    pub(super) fn open_library_edit(&mut self, l: &Library) {
        self.media.library_edit = Some(LibraryEdit::new(l));
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
        let mut edit = None;
        for l in libs.iter() {
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
                            self.library_refresh_status(ui, p, client, l.id);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(format!("{}  {}", icons::TRASH, t!("Remove"))).clicked() {
                                remove = Some((l.id, l.name.clone()));
                            }
                            if ui
                                .button(format!("{}  {}", icons::PENCIL_SIMPLE, t!("Edit")))
                                .on_hover_text(t!("Name, folders and settings of this library"))
                                .clicked()
                            {
                                edit = Some(l.clone());
                            }
                            if ui
                                .button(format!("{}  {}", icons::EYE_SLASH, t!("Ignored files")))
                                .on_hover_text(t!("Media removed from this library with its files kept"))
                                .clicked()
                            {
                                self.open_ignored(l.id, &l.name);
                            }
                            if let Some(mode) = super::refresh::refresh_menu(ui) {
                                self.refresh_library(client, l.id, mode);
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
        // CORRENT_DEV_EDIT_LIBRARY=<id> (debug builds): that library's Edit dialog, once.
        #[cfg(debug_assertions)]
        if edit.is_none()
            && let Some(id) = std::env::var("CORRENT_DEV_EDIT_LIBRARY").ok().and_then(|v| v.parse::<i64>().ok())
            && let Some(l) = libs.iter().find(|l| l.id == id)
        {
            static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                edit = Some(l.clone());
            }
        }
        if let Some(l) = edit {
            self.open_library_edit(&l);
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
            // (`pick` looks at the folder off the UI thread.)
            let start = self.media.library_form.as_ref().map(|f| std::path::PathBuf::from(&f.folder));
            self.pick(crate::app::pickers::PickTarget::LibraryFolder, crate::app::pickers::PickKind::Folder, start);
        }
    }

    pub(super) fn library_edit_window(&mut self, ctx: &egui::Context, p: &Palette, client: &MediaClient) {
        let Some(mut e) = self.media.library_edit.take() else { return };
        if let Some(r) = e.saving.as_ref().and_then(ready) {
            e.saving = None;
            match r {
                Ok(_) => {
                    self.toasts.push(crate::engine::ToastKind::Success, t!("Saved").to_string());
                    self.media.invalidate();
                    return;
                }
                Err(err) => e.error = Some(err),
            }
        }
        let local = self.server.is_none();
        let busy = self.picker_busy();
        let mut categories: Vec<String> = self.snap.categories.keys().cloned().collect();
        // Categories that are gone stay chosen until taken off.
        for c in &e.settings.import_categories {
            if !categories.contains(c) {
                categories.push(c.clone());
            }
        }
        let mut close = false;
        let mut save = false;
        let mut pick: Option<usize> = None;
        let r = egui::Modal::new(egui::Id::new("edit_library")).show(ctx, |ui| {
            widgets::modal_body(ui, |ui| {
                let w = widgets::modal_width(ui.ctx(), 600.0);
                ui.set_width(w);
                ui.label(
                    RichText::new(format!("{}  {}", icons::PENCIL_SIMPLE, t!("Edit library"))).strong().size(16.0),
                );
                ui.label(RichText::new(kind_label(e.kind)).small().color(p.text_dim));
                ui.add_space(8.0);
                let field = (w - 160.0).max(180.0);
                egui::Grid::new("edit_library_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(t!("Name"));
                    ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(field));
                    ui.end_row();
                    ui.label(t!("Folders"));
                    ui.vertical(|ui| {
                        let mut drop = None;
                        let many = e.folders.len() > 1;
                        egui::ScrollArea::vertical()
                            .id_salt("edit_library_folders")
                            .max_height(180.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (i, f) in e.folders.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(f)
                                                .hint_text(if local { "" } else { t!("A folder on the server") })
                                                .desired_width((field - 120.0).max(120.0)),
                                        );
                                        if ui.add_enabled(!busy, egui::Button::new(t!("Choose…"))).clicked() {
                                            pick = Some(i);
                                        }
                                        // (Not while a picker is open: it fills a row by index.)
                                        if many
                                            && ui
                                                .add_enabled(!busy, egui::Button::new(icons::X))
                                                .on_hover_text(t!("Remove folder"))
                                                .clicked()
                                        {
                                            drop = Some(i);
                                        }
                                    });
                                }
                            });
                        if let Some(i) = drop {
                            e.folders.remove(i);
                        }
                        if ui.button(format!("{}  {}", icons::PLUS, t!("Add folder"))).clicked() {
                            e.folders.push(String::new());
                        }
                    });
                    ui.end_row();
                    ui.label(t!("Counts as played after"));
                    ui.add(egui::DragValue::new(&mut e.settings.played_threshold).range(50..=100).suffix(" %"));
                    ui.end_row();
                    ui.label(t!("Metadata language"));
                    ui.add(
                        egui::TextEdit::singleline(&mut e.settings.language).hint_text(t!("en, tr…")).desired_width(120.0),
                    );
                    ui.end_row();
                });
                ui.add_space(8.0);
                ui.checkbox(&mut e.settings.watch, t!("Watch the folders for changes"));
                ui.checkbox(&mut e.settings.fetch_metadata, t!("Fetch metadata and artwork online"))
                    .on_hover_text(t!("Uses metadata plugins when they are installed."));
                if matches!(e.kind, LibraryKind::Movies | LibraryKind::Shows) {
                    ui.checkbox(&mut e.settings.write_nfo, t!("Write .nfo files")).on_hover_text(t!(
                        "Saves titles, plots and ids next to the files for Kodi and other players. Existing .nfo files that Corrent didn’t write are left alone."
                    ));
                }
                if !categories.is_empty() {
                    ui.add_space(8.0);
                    ui.label(t!("Import finished downloads of these categories"));
                    ui.horizontal_wrapped(|ui| {
                        for c in &categories {
                            let on = e.settings.import_categories.contains(c);
                            if toggle_chip(ui, p, c, on).clicked() {
                                if on {
                                    e.settings.import_categories.retain(|x| x != c);
                                } else {
                                    e.settings.import_categories.push(c.clone());
                                }
                            }
                        }
                    });
                    if !e.settings.import_categories.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(t!("How"));
                            egui::ComboBox::from_id_salt("edit_library_import")
                                .selected_text(import_mode_label(e.settings.import_mode))
                                .show_ui(ui, |ui| {
                                    for m in [ImportMode::InPlace, ImportMode::Hardlink, ImportMode::Copy] {
                                        ui.selectable_value(&mut e.settings.import_mode, m, import_mode_label(m));
                                    }
                                });
                        });
                        ui.checkbox(&mut e.settings.stop_seeding_when_watched, t!("Stop seeding once everyone has watched"))
                            .on_hover_text(t!(
                                "Pauses a finished torrent of these categories when every account that sees this library has played what it brought."
                            ));
                    }
                }
                if let Some(err) = &e.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err).color(p.error));
                }
                ui.add_space(10.0);
                widgets::dialog_footer(ui, |ui| {
                    let saving = e.saving.is_some();
                    if ui.add_enabled_ui(!saving, |ui| primary_button(ui, p, t!("Save"))).inner.clicked() {
                        save = true;
                    }
                    if secondary_button(ui, t!("Cancel")).clicked() {
                        close = true;
                    }
                    if saving {
                        ui.spinner();
                    }
                });
            });
        });
        if close || (r.should_close() && e.saving.is_none()) {
            return;
        }
        if save {
            match e.body() {
                Ok(body) => {
                    e.error = None;
                    let (c, path) = (client.clone(), format!("/api/v1/libraries/{}", e.id));
                    e.saving =
                        Some(fetch(
                            &self.rt,
                            ctx,
                            async move { c.send_reason(reqwest::Method::PATCH, &path, &body).await },
                        ));
                }
                Err(msg) => e.error = Some(msg.to_string()),
            }
        }
        let start = pick.and_then(|i| e.folders.get(i)).map(std::path::PathBuf::from);
        self.media.library_edit = Some(e);
        if let Some(i) = pick {
            self.pick(
                crate::app::pickers::PickTarget::LibraryEditFolder(i),
                crate::app::pickers::PickKind::Folder,
                start,
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        Library {
            id: 7,
            name: "Movies".into(),
            kind: LibraryKind::Movies,
            folders: vec!["D:/Movies".into()],
            settings: LibrarySettings { seed_ratio: Some(2.0), profile_id: Some(3), ..LibrarySettings::defaults() },
            item_count: 0,
            scanned_at: None,
            scanning: None,
        }
    }

    #[test]
    fn body_trims_dedupes_and_keeps_hidden_settings() {
        let mut e = LibraryEdit::new(&library());
        e.name = "  Films ".into();
        e.folders = vec![" D:/Movies ".into(), String::new(), "D:/Movies".into(), "E:/More".into()];
        e.settings.played_threshold = 20;
        e.settings.language = " tr ".into();
        let b = e.body().unwrap();
        assert_eq!(b["name"], "Films");
        assert_eq!(b["folders"], serde_json::json!(["D:/Movies", "E:/More"]));
        let s: LibrarySettings = serde_json::from_value(b["settings"].clone()).unwrap();
        assert_eq!(s.played_threshold, 50);
        assert_eq!(s.language, "tr");
        // Not in the dialog, sent back unchanged.
        assert_eq!(s.seed_ratio, Some(2.0));
        assert_eq!(s.profile_id, Some(3));
    }

    #[test]
    fn body_needs_a_name_and_a_folder() {
        let mut e = LibraryEdit::new(&library());
        e.name = "  ".into();
        assert!(e.body().is_err());
        e.name = "Films".into();
        e.folders = vec![" ".into()];
        assert!(e.body().is_err());
    }
}
