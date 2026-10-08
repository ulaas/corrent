//! Modal dialogs.

use crate::ui::widgets::Labeled;
use std::path::PathBuf;
use std::time::Instant;

use egui::{Align, Context, Id, Layout, RichText, Ui, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::engine::types::{AddSource, InfoHash};
use crate::engine::{AddRequest, EngineCommand, Preview, ToastKind};
use crate::ui::file_tree;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{danger_button, primary_button, secondary_button};
use crate::util::fmt_bytes;

pub enum Modal {
    Magnet {
        text: String,
    },
    Delete {
        hashes: Vec<InfoHash>,
        with_data: bool,
        names: Vec<String>,
        total: u64,
    },
    Move {
        hashes: Vec<InfoHash>,
        path: String,
    },
    Rename {
        hash: InfoHash,
        name: String,
    },
    Category {
        name: String,
        path: String,
        apply_to: Option<Vec<InfoHash>>,
        editing: Option<String>,
        /// Shared speed limits in KiB/s (0 = none).
        down: u32,
        up: u32,
    },
    Tag {
        name: String,
        apply_to: Option<Vec<InfoHash>>,
    },
    SeedLimits {
        hashes: Vec<InfoHash>,
        ratio_on: bool,
        ratio: f32,
        time_on: bool,
        minutes: u32,
    },
    SpeedLimits {
        hashes: Vec<InfoHash>,
        down: u32,
        up: u32,
    },
    Trackers {
        hash: InfoHash,
        text: String,
    },
    ConfirmExit,
}

pub struct AddDialog {
    pub req: u64,
    pub source: AddSource,
    pub preview: Option<Box<Preview>>,
    pub error: Option<String>,
    pub save_path: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub start_paused: bool,
    pub create_subfolder: bool,
    pub add_to_top: bool,
    pub checked: Vec<bool>,
    pub filter: String,
    pub started: Instant,
    pub tag_input: String,
    /// (path, measured at, free bytes) — refreshed at most every two seconds.
    pub free_cache: Option<(String, Instant, Option<u64>)>,
}

fn modal_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg1)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(egui::CornerRadius::same(14))
        .shadow(egui::Shadow {
            offset: [0, 16],
            blur: 40,
            spread: 0,
            color: egui::Color32::from_black_alpha(if p.dark { 140 } else { 60 }),
        })
        .inner_margin(egui::Margin::same(22))
}

fn title(ui: &mut Ui, p: &Palette, icon: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(20.0).color(p.accent));
        ui.label(RichText::new(text).font(theme::title()).color(p.text));
    });
    ui.add_space(10.0);
}

/// A path text field with a Browse button. Returns true when Browse was clicked; the caller opens
/// a non-blocking picker whose result is applied later.
fn path_field(ui: &mut Ui, p: &Palette, path: &mut String, width: f32, busy: bool) -> bool {
    let mut browse = false;
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(path).desired_width(width - 40.0));
        if busy {
            ui.add(egui::Spinner::new().color(p.accent));
        } else if ui.button(icons::FOLDER_OPEN).labeled("Browse…").clicked() {
            browse = true;
        }
    });
    browse
}

impl CorrentApp {
    pub fn modal_open(&self) -> bool {
        self.modal.is_some()
            || !self.add_dialogs.is_empty()
            || self.settings_draft.is_some()
            || self.batch.is_some()
            || self.onboarding.is_some()
            || self.creator.is_some()
            || self.feeds.rules.is_some()
    }

    /// Free space for a path typed in a dialog, refreshed at most every two seconds.
    pub fn cached_free_space(&mut self, path: &str) -> Option<u64> {
        let stale = self
            .free_cache
            .as_ref()
            .is_none_or(|(p, t, _)| p != path || t.elapsed() >= std::time::Duration::from_secs(2));
        if stale {
            let free = crate::engine::free_space_for(&PathBuf::from(path));
            self.free_cache = Some((path.to_string(), Instant::now(), free));
        }
        self.free_cache.as_ref().and_then(|c| c.2)
    }

    pub fn open_magnet_dialog(&mut self) {
        // Pre-fill from the clipboard when it holds something addable.
        let clip = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok()).unwrap_or_default();
        let text = if clip.lines().any(|l| AddSource::parse(l).is_some_and(|s| !matches!(s, AddSource::File(_)))) {
            clip.trim().to_string()
        } else {
            String::new()
        };
        self.modal = Some(Modal::Magnet { text });
    }

    pub fn request_delete(&mut self, hashes: Vec<InfoHash>, with_data: bool) {
        let sel: Vec<_> = self.snap.torrents.iter().filter(|t| hashes.contains(&t.hash)).collect();
        if sel.is_empty() {
            return;
        }
        if !self.settings.behavior.confirm_delete && !with_data {
            self.engine.send(EngineCommand::Delete { hashes, with_data });
            return;
        }
        let names = sel.iter().map(|t| t.name.clone()).collect();
        let total = sel.iter().map(|t| t.done).sum();
        self.modal = Some(Modal::Delete { hashes, with_data, names, total });
    }

    pub fn open_move_dialog(&mut self, hashes: Vec<InfoHash>) {
        let path = self
            .snap
            .torrents
            .iter()
            .find(|t| hashes.first() == Some(&t.hash))
            .map(|t| t.save_path.display().to_string())
            .unwrap_or_default();
        self.modal = Some(Modal::Move { hashes, path });
    }

    pub fn open_rename_dialog(&mut self, hash: InfoHash, name: String) {
        self.modal = Some(Modal::Rename { hash, name });
    }

    pub fn open_new_category(&mut self, apply_to: Option<Vec<InfoHash>>) {
        self.modal =
            Some(Modal::Category { name: String::new(), path: String::new(), apply_to, editing: None, down: 0, up: 0 });
    }

    pub fn open_edit_category(&mut self, name: String) {
        let info = self.snap.categories.get(&name).cloned().unwrap_or_default();
        let path = info.save_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
        self.modal = Some(Modal::Category {
            name: name.clone(),
            path,
            apply_to: None,
            editing: Some(name),
            down: info.down_limit_kib,
            up: info.up_limit_kib,
        });
    }

    pub fn open_new_tag(&mut self, apply_to: Option<Vec<InfoHash>>) {
        self.modal = Some(Modal::Tag { name: String::new(), apply_to });
    }

    pub fn open_seed_limits(&mut self, hashes: Vec<InfoHash>) {
        let s = &self.settings.seeding;
        self.modal = Some(Modal::SeedLimits {
            hashes,
            ratio_on: s.ratio_enabled,
            ratio: s.ratio,
            time_on: s.time_enabled,
            minutes: s.time_minutes,
        });
    }

    pub fn open_speed_limits(&mut self, hashes: Vec<InfoHash>) {
        // Start from the first torrent's current limits.
        let (down, up) = self
            .snap
            .torrents
            .iter()
            .find(|t| hashes.first() == Some(&t.hash))
            .map(|t| (t.down_limit_kib, t.up_limit_kib))
            .unwrap_or((0, 0));
        self.modal = Some(Modal::SpeedLimits { hashes, down, up });
    }

    pub fn open_trackers_dialog(&mut self, hash: InfoHash) {
        self.modal = Some(Modal::Trackers { hash, text: String::new() });
    }

    /// Queue a source for adding: opens the add dialog (with preview) unless disabled.
    pub fn begin_add(&mut self, source: AddSource) {
        if self.settings.behavior.skip_add_dialog {
            self.add_source(source);
            return;
        }
        // Don't open two dialogs for the same source.
        if self.add_dialogs.iter().any(|d| d.source == source) {
            return;
        }
        self.next_req += 1;
        let req = self.next_req;
        self.engine.send(EngineCommand::Preview { req, source: source.clone() });
        let d = &self.settings.downloads;
        self.add_dialogs.push_back(AddDialog {
            req,
            source,
            preview: None,
            error: None,
            save_path: d.save_path.display().to_string(),
            category: None,
            tags: Vec::new(),
            start_paused: d.start_paused,
            create_subfolder: d.create_subfolder,
            add_to_top: false,
            checked: Vec::new(),
            filter: String::new(),
            started: Instant::now(),
            tag_input: String::new(),
            free_cache: None,
        });
    }

    pub fn on_preview_ready(&mut self, req: u64, result: Result<Box<Preview>, String>) {
        if let Some(d) = self.add_dialogs.iter_mut().find(|d| d.req == req) {
            match result {
                Ok(p) => {
                    d.checked = vec![true; p.files.len()];
                    d.preview = Some(p);
                }
                Err(e) => d.error = Some(e),
            }
        }
    }

    pub(super) fn dialogs(&mut self, ctx: &Context, p: &Palette) {
        self.add_dialog(ctx, p);
        let Some(modal) = self.modal.take() else { return };
        let mut keep = true;
        let mut next: Option<Modal> = None;
        let resp = egui::Modal::new(Id::new("modal")).frame(modal_frame(p)).show(ctx, |ui| {
            let mut modal = modal;
            match &mut modal {
                Modal::Magnet { text } => {
                    ui.set_width(560.0);
                    title(ui, p, icons::MAGNET, "Add magnet links or URLs");
                    ui.label(
                        RichText::new("One per line: magnet links, info hashes or http(s) links to .torrent files.")
                            .color(p.text_dim),
                    );
                    ui.add_space(6.0);
                    let r = ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY)
                            .hint_text("magnet:?xt=urn:btih:…")
                            .font(egui::TextStyle::Monospace),
                    );
                    if !r.has_focus() && text.is_empty() {
                        r.request_focus();
                    }
                    let sources: Vec<AddSource> = text.lines().filter_map(AddSource::parse).collect();
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} item(s) recognized", sources.len())).small().color(p.text_dim),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let add =
                                ui.add_enabled_ui(!sources.is_empty(), |ui| primary_button(ui, p, "Continue")).inner;
                            if add.clicked()
                                || (ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
                                    && !sources.is_empty())
                            {
                                for s in sources.clone() {
                                    self.launch_queue.push_back(s);
                                }
                                keep = false;
                            }
                            if secondary_button(ui, "Cancel").clicked() {
                                keep = false;
                            }
                        });
                    });
                }
                Modal::Delete { hashes, with_data, names, total } => {
                    ui.set_width(480.0);
                    let n = names.len();
                    title(
                        ui,
                        p,
                        icons::TRASH,
                        &if n == 1 { "Remove torrent?".to_string() } else { format!("Remove {n} torrents?") },
                    );
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for name in names.iter().take(50) {
                            ui.label(RichText::new(format!("•  {name}")).color(p.text));
                        }
                        if n > 50 {
                            ui.label(RichText::new(format!("…and {} more", n - 50)).color(p.text_dim));
                        }
                    });
                    ui.add_space(10.0);
                    ui.radio_value(with_data, false, "Remove from list, keep downloaded files");
                    ui.radio_value(
                        with_data,
                        true,
                        RichText::new(format!("Also delete downloaded files ({})", fmt_bytes(*total))).color(p.error),
                    );
                    if *with_data {
                        ui.label(
                            RichText::new(format!("{}  Files will be permanently deleted from disk.", icons::WARNING))
                                .small()
                                .color(p.error),
                        );
                    }
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let go = if *with_data {
                            danger_button(ui, p, "Delete files")
                        } else {
                            primary_button(ui, p, "Remove")
                        };
                        if go.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.engine.send(EngineCommand::Delete { hashes: hashes.clone(), with_data: *with_data });
                            for h in hashes.iter() {
                                self.selected.remove(h);
                            }
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Move { hashes, path } => {
                    ui.set_width(520.0);
                    title(ui, p, icons::FOLDER_SIMPLE_DASHED, "Move data to another folder");
                    if path_field(ui, p, path, 476.0, self.picker_busy()) {
                        self.pick(PickTarget::MoveFolder, PickKind::Folder, Some(PathBuf::from(path.as_str())));
                    }
                    ui.add_space(4.0);
                    let need: u64 =
                        self.snap.torrents.iter().filter(|t| hashes.contains(&t.hash)).map(|t| t.done).sum();
                    let free = self.cached_free_space(path);
                    if let Some(free) = free {
                        let short = need > free;
                        ui.label(
                            RichText::new(format!("{} needed · {} free", fmt_bytes(need), fmt_bytes(free)))
                                .small()
                                .color(if short { p.error } else { p.text_dim }),
                        );
                        if short {
                            ui.label(RichText::new("There isn't enough space in that folder.").small().color(p.error));
                        }
                    }
                    ui.label(
                        RichText::new(
                            "Files are moved, then verified at the new location. Transfers pause during the move.",
                        )
                        .small()
                        .color(p.text_dim),
                    );
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled_ui(!path.trim().is_empty(), |ui| primary_button(ui, p, "Move"))
                            .inner
                            .clicked()
                        {
                            self.engine.send(EngineCommand::MoveStorage {
                                hashes: hashes.clone(),
                                to: PathBuf::from(path.trim()),
                            });
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Rename { hash, name } => {
                    ui.set_width(460.0);
                    title(ui, p, icons::PENCIL_SIMPLE, "Rename torrent");
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    r.request_focus();
                    ui.label(
                        RichText::new("Changes the name shown in Corrent; files on disk keep their names.")
                            .small()
                            .color(p.text_dim),
                    );
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, p, "Rename").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            self.engine
                                .send(EngineCommand::Rename { hash: *hash, name: Some(name.trim().to_string()) });
                            keep = false;
                        }
                        if secondary_button(ui, "Reset").clicked() {
                            self.engine.send(EngineCommand::Rename { hash: *hash, name: None });
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Category { name, path, apply_to, editing, down, up } => {
                    ui.set_width(480.0);
                    title(
                        ui,
                        p,
                        icons::FOLDER_SIMPLE_PLUS,
                        if editing.is_some() { "Edit category" } else { "New category" },
                    );
                    egui::Grid::new("cat").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                        ui.label("Name");
                        ui.add_enabled(
                            editing.is_none(),
                            egui::TextEdit::singleline(name).desired_width(340.0).hint_text("e.g. Movies or Movies/HD"),
                        );
                        ui.end_row();
                        ui.label("Save path");
                        if path_field(ui, p, path, 340.0, self.picker_busy()) {
                            self.pick(PickTarget::CategoryFolder, PickKind::Folder, Some(PathBuf::from(path.as_str())));
                        }
                        ui.end_row();
                        for (label, v) in [("Download limit", &mut *down), ("Upload limit", &mut *up)] {
                            ui.label(label);
                            ui.add(egui::DragValue::new(v).range(0..=10_000_000).speed(16.0).custom_formatter(
                                |n, _| {
                                    if n <= 0.0 { "∞ Unlimited".into() } else { format!("{n:.0} KiB/s") }
                                },
                            ))
                            .on_hover_text("Shared by all torrents in this category");
                            ui.end_row();
                        }
                    });
                    ui.label(
                        RichText::new("Torrents added to this category are saved here (leave empty for the default).")
                            .small()
                            .color(p.text_dim),
                    );
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ok = !name.trim().is_empty();
                        if ui.add_enabled_ui(ok, |ui| primary_button(ui, p, "Save")).inner.clicked() {
                            let mut cats = self.snap.categories.clone();
                            let path = path.trim();
                            cats.insert(
                                name.trim().to_string(),
                                crate::store::CategoryInfo {
                                    save_path: (!path.is_empty()).then(|| PathBuf::from(path)),
                                    down_limit_kib: *down,
                                    up_limit_kib: *up,
                                },
                            );
                            self.engine.send(EngineCommand::SetCategories(cats));
                            if let Some(h) = apply_to.take() {
                                self.engine.send(EngineCommand::SetCategory {
                                    hashes: h,
                                    category: Some(name.trim().to_string()),
                                });
                            }
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Tag { name, apply_to } => {
                    ui.set_width(400.0);
                    title(ui, p, icons::TAG, "New tag");
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).hint_text("Tag name"));
                    r.request_focus();
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ok = !name.trim().is_empty() && !name.contains(',');
                        if ui.add_enabled_ui(ok, |ui| primary_button(ui, p, "Create")).inner.clicked()
                            || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            let tag = name.trim().to_string();
                            let mut tags = self.snap.tags.clone();
                            if !tags.contains(&tag) {
                                tags.push(tag.clone());
                                tags.sort();
                            }
                            self.engine.send(EngineCommand::SetTagList(tags));
                            if let Some(h) = apply_to.take() {
                                self.engine.send(EngineCommand::ToggleTag { hashes: h, tag, on: true });
                            }
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::SeedLimits { hashes, ratio_on, ratio, time_on, minutes } => {
                    ui.set_width(440.0);
                    title(ui, p, icons::SCALES, "Seeding limits");
                    ui.label(
                        RichText::new("Overrides the global seeding limits for the selected torrents.")
                            .color(p.text_dim),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(ratio_on, "Stop at ratio");
                        ui.add_enabled(
                            *ratio_on,
                            egui::DragValue::new(ratio).range(0.1..=100.0).speed(0.05).fixed_decimals(2),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(time_on, "Stop after seeding for");
                        ui.add_enabled(*time_on, egui::DragValue::new(minutes).range(1..=525600).suffix(" min"));
                    });
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, p, "Apply").clicked() {
                            self.engine.send(EngineCommand::SetSeedLimits {
                                hashes: hashes.clone(),
                                ratio: ratio_on.then_some(*ratio),
                                minutes: time_on.then_some(*minutes),
                            });
                            keep = false;
                        }
                        if secondary_button(ui, "Use global").clicked() {
                            self.engine.send(EngineCommand::SetSeedLimits {
                                hashes: hashes.clone(),
                                ratio: None,
                                minutes: None,
                            });
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::SpeedLimits { hashes, down, up } => {
                    ui.set_width(420.0);
                    title(ui, p, icons::GAUGE, "Speed limits");
                    let n = hashes.len();
                    ui.label(
                        RichText::new(if n == 1 {
                            "Limits for this torrent, on top of the global limits.".to_string()
                        } else {
                            format!("Limits for each of the {n} selected torrents, on top of the global limits.")
                        })
                        .color(p.text_dim),
                    );
                    ui.add_space(8.0);
                    egui::Grid::new("speed_limits").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                        for (label, v) in [("Download", &mut *down), ("Upload", &mut *up)] {
                            ui.label(label);
                            ui.add(
                                egui::DragValue::new(v)
                                    .range(0..=10_000_000)
                                    .speed(16.0)
                                    .custom_formatter(|n, _| {
                                        if n <= 0.0 { "∞ Unlimited".into() } else { format!("{n:.0} KiB/s") }
                                    })
                                    .custom_parser(|s| {
                                        let t = s.trim().trim_end_matches("KiB/s").trim();
                                        if t.is_empty() || t == "∞" || t.eq_ignore_ascii_case("unlimited") {
                                            Some(0.0)
                                        } else {
                                            t.parse().ok()
                                        }
                                    }),
                            );
                            ui.end_row();
                        }
                    });
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, p, "Apply").clicked() {
                            self.engine.send(EngineCommand::SetTorrentLimits {
                                hashes: hashes.clone(),
                                down_kib: *down,
                                up_kib: *up,
                            });
                            keep = false;
                        }
                        if secondary_button(ui, "No limits").clicked() {
                            self.engine.send(EngineCommand::SetTorrentLimits {
                                hashes: hashes.clone(),
                                down_kib: 0,
                                up_kib: 0,
                            });
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::Trackers { hash, text } => {
                    ui.set_width(520.0);
                    title(ui, p, icons::BROADCAST, "Add trackers");
                    ui.label(RichText::new("One tracker URL per line.").color(p.text_dim));
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{}  The torrent is briefly re-added; its data is re-verified (not re-downloaded).",
                            icons::INFO
                        ))
                        .small()
                        .color(p.text_dim),
                    );
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let list: Vec<String> =
                            text.lines().map(|l| l.trim().to_string()).filter(|l| url::Url::parse(l).is_ok()).collect();
                        if ui.add_enabled_ui(!list.is_empty(), |ui| primary_button(ui, p, "Add")).inner.clicked() {
                            self.engine.send(EngineCommand::AddTrackers { hash: *hash, trackers: list });
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Modal::ConfirmExit => {
                    ui.set_width(420.0);
                    title(ui, p, icons::SIGN_OUT, "Quit Corrent?");
                    let active = self.snap.torrents.iter().filter(|t| t.state.is_active()).count();
                    ui.label(
                        RichText::new(format!(
                            "{active} torrent(s) are still transferring. They will resume next time you start Corrent."
                        ))
                        .color(p.text_dim),
                    );
                    ui.add_space(14.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, p, "Quit").clicked() {
                            self.quitting = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            keep = false;
                        }
                        if secondary_button(ui, "Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
            }
            if keep {
                next = Some(modal);
            }
        });
        if resp.should_close() {
            next = None;
        }
        self.modal = next;
    }

    fn add_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut d) = self.add_dialogs.pop_front() else { return };
        let mut close = false;
        let mut add = false;
        let mut add_without_preview = false;
        let queued = self.add_dialogs.len();
        let categories = self.snap.categories.clone();
        let all_tags = self.snap.tags.clone();
        let recent = self.settings.downloads.recent_paths.clone();
        let picker_busy = self.picker_busy();
        let mut browse_req = false;

        let resp = egui::Modal::new(Id::new("add_dialog")).frame(modal_frame(p)).show(ctx, |ui| {
            ui.set_width(820.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::DOWNLOAD_SIMPLE).size(20.0).color(p.accent));
                ui.label(RichText::new("Add torrent").font(theme::title()).color(p.text));
                if queued > 0 {
                    crate::ui::widgets::badge(ui, &format!("+{queued} more"), p.accent);
                }
            });
            let name = d.preview.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| d.source.describe());
            ui.add(egui::Label::new(RichText::new(&name).color(p.text_dim)).truncate());
            ui.add_space(12.0);

            ui.horizontal_top(|ui| {
                // ---- left: options
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    ui.label(RichText::new("Save to").small().strong().color(p.text_dim));
                    if path_field(ui, p, &mut d.save_path, 300.0, picker_busy) {
                        browse_req = true;
                    }
                    if !recent.is_empty() {
                        egui::ComboBox::from_id_salt("recent_paths")
                            .selected_text(RichText::new("Recent folders").small())
                            .width(300.0)
                            .show_ui(ui, |ui| {
                                for r in &recent {
                                    let s = r.display().to_string();
                                    if ui.selectable_label(d.save_path == s, &s).clicked() {
                                        d.save_path = s;
                                    }
                                }
                            });
                    }
                    let stale =
                        d.free_cache.as_ref().is_none_or(|(p, t, _)| *p != d.save_path || t.elapsed().as_secs() >= 2);
                    if stale {
                        let free = crate::engine::free_space_for(&PathBuf::from(&d.save_path));
                        d.free_cache = Some((d.save_path.clone(), Instant::now(), free));
                    }
                    if let Some(free) = d.free_cache.as_ref().and_then(|c| c.2) {
                        let need: u64 = d.preview.as_ref().map(|pv| selected_size(pv, &d.checked)).unwrap_or(0);
                        let color = if need > free { p.error } else { p.text_dim };
                        ui.label(RichText::new(format!("{} free", fmt_bytes(free))).small().color(color));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Category").small().strong().color(p.text_dim));
                    egui::ComboBox::from_id_salt("add_cat")
                        .selected_text(d.category.clone().unwrap_or_else(|| "None".into()))
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(d.category.is_none(), "None").clicked() {
                                d.category = None;
                            }
                            for (c, info) in &categories {
                                if ui.selectable_label(d.category.as_ref() == Some(c), c).clicked() {
                                    d.category = Some(c.clone());
                                    if let Some(path) = &info.save_path {
                                        d.save_path = path.display().to_string();
                                    }
                                }
                            }
                        });
                    ui.add_space(8.0);
                    ui.label(RichText::new("Tags").small().strong().color(p.text_dim));
                    ui.horizontal_wrapped(|ui| {
                        let mut remove = None;
                        for (i, t) in d.tags.iter().enumerate() {
                            if ui.small_button(format!("{t}  {}", icons::X)).clicked() {
                                remove = Some(i);
                            }
                        }
                        if let Some(i) = remove {
                            d.tags.remove(i);
                        }
                        egui::ComboBox::from_id_salt("add_tags").selected_text(icons::PLUS).width(40.0).show_ui(
                            ui,
                            |ui| {
                                for t in &all_tags {
                                    if !d.tags.contains(t) && ui.selectable_label(false, t).clicked() {
                                        d.tags.push(t.clone());
                                    }
                                }
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut d.tag_input)
                                            .hint_text("New tag")
                                            .desired_width(110.0),
                                    );
                                    if ui.button("Add").clicked() && !d.tag_input.trim().is_empty() {
                                        let t = d.tag_input.trim().to_string();
                                        if !d.tags.contains(&t) {
                                            d.tags.push(t);
                                        }
                                        d.tag_input.clear();
                                    }
                                });
                            },
                        );
                    });
                    ui.add_space(8.0);
                    ui.checkbox(&mut d.start_paused, "Add paused");
                    ui.checkbox(&mut d.create_subfolder, "Create subfolder for multi-file torrents");
                    ui.checkbox(&mut d.add_to_top, "Add to top of queue");

                    if let Some(pv) = &d.preview {
                        ui.add_space(10.0);
                        crate::ui::widgets::card(p).show(ui, |ui| {
                            ui.set_width(272.0);
                            kv(ui, p, "Size", &fmt_bytes(pv.total_size));
                            kv(ui, p, "Files", &pv.files.len().to_string());
                            kv(ui, p, "Pieces", &format!("{} × {}", pv.piece_count, fmt_bytes(pv.piece_length as u64)));
                            kv(ui, p, "Hash", &pv.info_hash.to_hex()[..16]);
                            if let Some(c) = &pv.comment {
                                ui.add(egui::Label::new(RichText::new(c).small().color(p.text_dim)).wrap());
                            }
                        });
                    }
                });
                ui.add_space(12.0);
                // ---- right: files
                ui.vertical(|ui| {
                    ui.set_min_height(360.0);
                    match (&d.preview, &d.error) {
                        (Some(pv), _) => {
                            if pv.already_added {
                                ui.label(
                                    RichText::new(format!("{}  This torrent is already in your list.", icons::WARNING))
                                        .color(p.warn),
                                );
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button("Select all").clicked() {
                                    d.checked.iter_mut().for_each(|c| *c = true);
                                }
                                if ui.small_button("None").clicked() {
                                    d.checked.iter_mut().for_each(|c| *c = false);
                                }
                                if ui.small_button("Invert").clicked() {
                                    d.checked.iter_mut().for_each(|c| *c = !*c);
                                }
                                ui.add(
                                    egui::TextEdit::singleline(&mut d.filter)
                                        .hint_text(format!("{} Filter files", icons::FUNNEL))
                                        .desired_width(180.0),
                                );
                            });
                            ui.add_space(4.0);
                            egui::Frame::new()
                                .fill(p.bg0)
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .max_height(340.0)
                                        .min_scrolled_height(340.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            let entries: Vec<file_tree::Entry<'_>> = pv
                                                .files
                                                .iter()
                                                .enumerate()
                                                .map(|(i, f)| file_tree::Entry {
                                                    index: i,
                                                    components: &f.components,
                                                    len: f.len,
                                                    done: None,
                                                    priority: None,
                                                })
                                                .collect();
                                            let checked = d.checked.clone();
                                            if let Some(ch) = file_tree::show(
                                                ui,
                                                p,
                                                Id::new(("addtree", d.req)),
                                                &entries,
                                                &|i| checked.get(i).copied().unwrap_or(true),
                                                &d.filter,
                                                true,
                                            ) {
                                                for (i, on) in ch.set {
                                                    if let Some(c) = d.checked.get_mut(i) {
                                                        *c = on;
                                                    }
                                                }
                                            }
                                        });
                                });
                            let sel = d.checked.iter().filter(|c| **c).count();
                            ui.label(
                                RichText::new(format!(
                                    "{} of {} files selected · {}",
                                    sel,
                                    pv.files.len(),
                                    fmt_bytes(selected_size(pv, &d.checked))
                                ))
                                .small()
                                .color(p.text_dim),
                            );
                        }
                        (None, Some(err)) => {
                            ui.add_space(60.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new(icons::WARNING_CIRCLE).size(40.0).color(p.error));
                                ui.label(RichText::new("Couldn't read this torrent").strong().color(p.text));
                                ui.add(egui::Label::new(RichText::new(err).color(p.text_dim)).wrap());
                            });
                        }
                        (None, None) => {
                            ui.add_space(80.0);
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Spinner::new().size(28.0).color(p.accent));
                                ui.add_space(8.0);
                                let msg = if matches!(d.source, AddSource::Magnet(_)) {
                                    "Fetching metadata from peers…"
                                } else {
                                    "Reading torrent…"
                                };
                                ui.label(RichText::new(msg).color(p.text));
                                let secs = d.started.elapsed().as_secs();
                                if secs >= 4 {
                                    ui.label(
                                        RichText::new(format!("{secs}s — this can take a while for rare torrents."))
                                            .small()
                                            .color(p.text_dim),
                                    );
                                }
                                if matches!(d.source, AddSource::Magnet(_)) {
                                    ui.add_space(8.0);
                                    if ui
                                        .button("Add without waiting")
                                        .on_hover_text("Start the download now; all files will be selected.")
                                        .clicked()
                                    {
                                        add_without_preview = true;
                                    }
                                }
                            });
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                        }
                    }
                });
            });

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can_add =
                        d.preview.as_ref().is_some_and(|pv| !pv.already_added && d.checked.iter().any(|c| *c))
                            && !d.save_path.trim().is_empty();
                    if ui
                        .add_enabled_ui(can_add, |ui| {
                            primary_button(ui, p, &format!("{}  Add torrent", icons::DOWNLOAD_SIMPLE))
                        })
                        .inner
                        .clicked()
                    {
                        add = true;
                    }
                    if secondary_button(ui, "Cancel").clicked() {
                        close = true;
                    }
                });
            });
        });
        if resp.should_close() {
            close = true;
        }
        if browse_req {
            self.pick(PickTarget::AddDialogFolder(d.req), PickKind::Folder, Some(PathBuf::from(&d.save_path)));
        }

        if add || add_without_preview {
            let save = PathBuf::from(d.save_path.trim());
            let only_files = d.preview.as_ref().and_then(|_| {
                (!d.checked.iter().all(|c| *c))
                    .then(|| d.checked.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect())
            });
            let req = AddRequest {
                source: d.source.clone(),
                torrent_bytes: d.preview.as_ref().map(|pv| pv.torrent_bytes.clone()),
                save_path: save.clone(),
                create_subfolder: d.create_subfolder,
                category: d.category.clone(),
                tags: d.tags.clone(),
                paused: d.start_paused,
                only_files,
                add_to_top: d.add_to_top,
                initial_peers: None,
            };
            self.engine.send(EngineCommand::CancelPreview(d.req));
            self.engine.send(EngineCommand::Add(Box::new(req)));
            if self.settings.downloads.remember_last_path {
                self.settings.downloads.push_recent(save);
                self.save_settings();
            }
            if !d.tags.is_empty() {
                let mut tags = self.snap.tags.clone();
                for t in &d.tags {
                    if !tags.contains(t) {
                        tags.push(t.clone());
                    }
                }
                tags.sort();
                self.engine.send(EngineCommand::SetTagList(tags));
            }
        } else if close {
            self.engine.send(EngineCommand::CancelPreview(d.req));
        } else {
            self.add_dialogs.push_front(d);
        }
        let _ = ToastKind::Info;
    }
}

fn selected_size(pv: &Preview, checked: &[bool]) -> u64 {
    pv.files.iter().zip(checked.iter()).filter(|(_, c)| **c).map(|(f, _)| f.len).sum()
}

fn kv(ui: &mut Ui, p: &Palette, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(k).small().color(p.text_dim));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(v).small().monospace().color(p.text));
        });
    });
}
