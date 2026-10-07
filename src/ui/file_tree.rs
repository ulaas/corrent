//! Collapsible file tree with tri-state checkboxes, sizes and optional progress.

use std::collections::BTreeMap;

use egui::{Id, RichText, Ui};
use egui_phosphor::regular as icons;

use super::theme::Palette;
use crate::util::fmt_bytes;

pub struct Entry<'a> {
    pub index: usize,
    pub components: &'a [String],
    pub len: u64,
    pub done: Option<u64>,
}

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<usize>, // indices into entries
}

impl Node {
    fn insert(&mut self, comps: &[String], entry_idx: usize) {
        if comps.len() <= 1 {
            self.files.push(entry_idx);
        } else {
            self.dirs.entry(comps[0].clone()).or_default().insert(&comps[1..], entry_idx);
        }
    }

    fn collect(&self, out: &mut Vec<usize>) {
        out.extend(&self.files);
        for d in self.dirs.values() {
            d.collect(out);
        }
    }
}

/// Result of user interaction: the new checked state per entry index (if changed).
pub struct TreeChange {
    pub set: Vec<(usize, bool)>,
}

pub fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mkv" | "mp4" | "avi" | "mov" | "webm" | "m4v" | "wmv" | "ts" => icons::FILE_VIDEO,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" => icons::FILE_AUDIO,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" => icons::FILE_IMAGE,
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" => icons::FILE_ZIP,
        "iso" | "img" | "dmg" => icons::DISC,
        "txt" | "nfo" | "md" | "srt" | "sub" | "ass" => icons::FILE_TEXT,
        "pdf" | "epub" | "mobi" => icons::FILE_PDF,
        "exe" | "msi" | "deb" | "rpm" | "appimage" => icons::GEAR,
        _ => icons::FILE,
    }
}

/// Render the tree. `checked[entry.index]` is the current state; returns changes (if any).
pub fn show(
    ui: &mut Ui,
    p: &Palette,
    id: Id,
    entries: &[Entry<'_>],
    checked: &dyn Fn(usize) -> bool,
    filter: &str,
    editable: bool,
) -> Option<TreeChange> {
    let mut root = Node::default();
    let filter = filter.trim().to_lowercase();
    for (i, e) in entries.iter().enumerate() {
        if !filter.is_empty() && !e.components.join("/").to_lowercase().contains(&filter) {
            continue;
        }
        root.insert(e.components, i);
    }
    let mut change = TreeChange { set: Vec::new() };
    show_node(ui, p, id, &root, entries, checked, editable, !filter.is_empty(), &mut change);
    (!change.set.is_empty()).then_some(change)
}

#[allow(clippy::too_many_arguments)]
fn show_node(
    ui: &mut Ui,
    p: &Palette,
    id: Id,
    node: &Node,
    entries: &[Entry<'_>],
    checked: &dyn Fn(usize) -> bool,
    editable: bool,
    force_open: bool,
    change: &mut TreeChange,
) {
    for (name, dir) in &node.dirs {
        let mut all = Vec::new();
        dir.collect(&mut all);
        let n_on = all.iter().filter(|&&i| checked(entries[i].index)).count();
        let total: u64 = all.iter().map(|&i| entries[i].len).sum();
        let done: Option<u64> = entries.first().and_then(|e| e.done).map(|_| all.iter().filter_map(|&i| entries[i].done).sum());
        let dir_id = id.with(name);
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), dir_id, true);
        if force_open {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| {
                let mut on = n_on == all.len() && !all.is_empty();
                let cb = ui.add_enabled(editable, egui::Checkbox::new(&mut on, "").indeterminate(n_on > 0 && n_on < all.len()));
                if cb.changed() {
                    let target = n_on < all.len();
                    for &i in &all {
                        change.set.push((entries[i].index, target));
                    }
                }
                ui.label(RichText::new(icons::FOLDER).color(p.warn));
                ui.label(RichText::new(name).color(p.text));
                row_tail(ui, p, total, done);
            })
            .body(|ui| {
                show_node(ui, p, dir_id, dir, entries, checked, editable, force_open, change);
            });
    }
    for &i in &node.files {
        let e = &entries[i];
        let name = e.components.last().map(String::as_str).unwrap_or("");
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            let mut on = checked(e.index);
            if ui.add_enabled(editable, egui::Checkbox::new(&mut on, "")).changed() {
                change.set.push((e.index, on));
            }
            ui.label(RichText::new(file_icon(name)).color(p.text_dim));
            ui.add(egui::Label::new(RichText::new(name).color(if on { p.text } else { p.text_mute })).truncate());
            row_tail(ui, p, e.len, e.done);
        });
    }
}

fn row_tail(ui: &mut Ui, p: &Palette, len: u64, done: Option<u64>) {
    // Fixed-width cells so sizes, percentages and bars line up across rows.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let cell = |ui: &mut Ui, w: f32, text: RichText| {
            ui.allocate_ui_with_layout(egui::vec2(w, 18.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(text);
            });
        };
        cell(ui, 72.0, RichText::new(fmt_bytes(len)).monospace().small().color(p.text_dim));
        if let Some(d) = done {
            let frac = if len > 0 { d as f32 / len as f32 } else { 1.0 };
            cell(ui, 40.0, RichText::new(format!("{:.0}%", (frac * 100.0).floor())).monospace().small().color(p.text_dim));
            super::widgets::progress_bar(
                ui,
                Id::new(("ft", len, d)),
                frac,
                if frac >= 1.0 { p.success } else { p.accent },
                p.bg3,
                5.0,
                90.0,
                false,
                false,
            );
        }
    });
}
