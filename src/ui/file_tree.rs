//! Collapsible file tree with tri-state checkboxes, sizes and optional progress.

use std::collections::{BTreeMap, HashSet};

use egui::{Id, RichText, Ui};
use egui_phosphor::regular as icons;

use super::theme::Palette;
use super::widgets::Labeled;
use crate::util::fmt_bytes;

pub struct Entry<'a> {
    pub index: usize,
    pub components: &'a [String],
    pub len: u64,
    pub done: Option<u64>,
    /// Download priority (1 high, 0 normal, -1 low); None hides priorities (add dialog).
    pub priority: Option<i8>,
    /// A media player is reading this file through the streaming server.
    pub streaming: bool,
    pub checked: bool,
}

pub fn priority_label(p: i8) -> &'static str {
    match p {
        1.. => t!("High"),
        0 => t!("Normal"),
        _ => t!("Low"),
    }
}

/// Folders shown nested; deeper ones are shown as a path on the file's row. The tree is built
/// recursively on the UI thread (1 MiB of stack on Windows), and a torrent already in the list
/// may have any depth (new ones are checked, `engine::paths`).
const MAX_TREE_DEPTH: usize = 64;

#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    /// (index into entries, how many of its components are folders above it in the tree).
    files: Vec<(usize, usize)>,
}

impl Node {
    fn insert(&mut self, comps: &[String], at: usize, entry_idx: usize) {
        match comps.get(at) {
            Some(dir) if comps.len() - at > 1 && at < MAX_TREE_DEPTH => {
                self.dirs.entry(dir.clone()).or_default().insert(comps, at + 1, entry_idx)
            }
            _ => self.files.push((entry_idx, at)),
        }
    }
}

/// Result of user interaction: the new checked state per entry index (if changed).
pub struct TreeChange {
    pub set: Vec<(usize, bool)>,
    /// New download priority per entry index.
    pub priority: Vec<(usize, i8)>,
}

/// "Priority" submenu for a set of files.
fn priority_menu(ui: &mut Ui, files: &[usize], current: Option<i8>, change: &mut TreeChange) {
    crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::FLAG, t!("Priority")), |ui| {
        for (v, icon) in [(1i8, icons::ARROW_FAT_UP), (0, icons::MINUS), (-1, icons::ARROW_FAT_DOWN)] {
            let label = format!("{icon}  {}", priority_label(v));
            if ui.add(egui::Button::selectable(current == Some(v), label)).clicked() {
                change.priority.extend(files.iter().map(|&i| (i, v)));
                ui.close();
            }
        }
    });
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

/// What is shown of a tree, kept between frames: a torrent's list can have tens of thousands of
/// files, so it is built when the list or the filter changes, the rows are drawn only as they
/// scroll into view, and a folder's numbers come from one pass over the files.
#[derive(Default)]
pub struct TreeState {
    key: Option<u64>,
    dirs: Vec<Dir>,
    files: Vec<FileRef>,
    rows: Vec<Row>,
    /// `rows` with what collapsed folders hide left out, as indices into `rows`.
    visible: Vec<u32>,
    visible_dirty: bool,
    /// Per folder, redone every frame: (files checked, bytes done).
    live: Vec<(usize, u64)>,
    /// Folders the user closed. All are open while a filter is typed.
    closed: HashSet<Id>,
    filtering: bool,
    /// How many rows the last frame laid out.
    pub drawn: usize,
}

struct Dir {
    name: String,
    id: Id,
    parent: Option<usize>,
    /// Its files and those of its subfolders, as a range of `TreeState::files`.
    files: std::ops::Range<usize>,
    size: u64,
}

struct FileRef {
    entry: usize,
    /// How many of its components are folders above it in the tree.
    at: usize,
    dir: Option<usize>,
}

#[derive(Clone, Copy)]
struct Row {
    kind: RowKind,
    depth: usize,
    /// For a folder: the first row after everything inside it.
    end: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum RowKind {
    Dir(usize),
    File(usize),
}

const ROW_HEIGHT: f32 = 22.0;
const INDENT: f32 = 18.0;

impl TreeState {
    fn flatten(&mut self, node: &Node, parent: Option<usize>, depth: usize, id: Id, entries: &dyn Fn(usize) -> u64) {
        for (name, sub) in &node.dirs {
            let dir = self.dirs.len();
            let dir_id = id.with(name);
            let start = self.files.len();
            self.dirs.push(Dir { name: name.clone(), id: dir_id, parent, files: start..start, size: 0 });
            let row = self.rows.len();
            self.rows.push(Row { kind: RowKind::Dir(dir), depth, end: row + 1 });
            self.flatten(sub, Some(dir), depth + 1, dir_id, entries);
            let (end_file, end_row) = (self.files.len(), self.rows.len());
            if let Some(r) = self.rows.get_mut(row) {
                r.end = end_row;
            }
            let size = self.files.get(start..end_file).unwrap_or_default().iter().map(|f| entries(f.entry)).sum();
            if let Some(d) = self.dirs.get_mut(dir) {
                d.files = start..end_file;
                d.size = size;
            }
        }
        for &(entry, at) in &node.files {
            let file = self.files.len();
            self.files.push(FileRef { entry, at, dir: parent });
            let row = self.rows.len();
            self.rows.push(Row { kind: RowKind::File(file), depth, end: row + 1 });
        }
    }

    /// Builds the tree when the torrent, its number of files or the filter changed.
    fn prepare<'a>(&mut self, id: Id, count: usize, entry: &dyn Fn(usize) -> Option<Entry<'a>>, filter: &str) {
        use std::hash::{Hash, Hasher};
        let filter = filter.trim();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (id, count, filter).hash(&mut h);
        let key = h.finish();
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.dirs.clear();
        self.files.clear();
        self.rows.clear();
        let filter = filter.to_lowercase();
        self.filtering = !filter.is_empty();
        let mut root = Node::default();
        for i in 0..count {
            let Some(e) = entry(i) else { continue };
            if self.filtering && !e.components.join("/").to_lowercase().contains(&filter) {
                continue;
            }
            if !e.components.is_empty() {
                root.insert(e.components, 0, i);
            }
        }
        self.flatten(&root, None, 0, id, &|i| entry(i).map_or(0, |e| e.len));
        self.visible_dirty = true;
    }

    fn is_closed(&self, dir: &Dir) -> bool {
        !self.filtering && self.closed.contains(&dir.id)
    }

    fn update_visible(&mut self) {
        if !self.visible_dirty {
            return;
        }
        self.visible_dirty = false;
        self.visible.clear();
        let mut i = 0;
        while let Some(row) = self.rows.get(i) {
            self.visible.push(i as u32);
            let closed = matches!(row.kind, RowKind::Dir(d) if self.dirs.get(d).is_some_and(|d| self.is_closed(d)));
            i = if closed { row.end } else { i + 1 };
        }
    }

    /// Checked files and bytes done per folder: each file counts towards its folder, each
    /// folder towards its parent (parents come first in `dirs`).
    fn update_live<'a>(&mut self, entry: &dyn Fn(usize) -> Option<Entry<'a>>) {
        self.live.clear();
        self.live.resize(self.dirs.len(), (0, 0));
        for f in &self.files {
            if let (Some(e), Some(l)) = (entry(f.entry), f.dir.and_then(|d| self.live.get_mut(d))) {
                l.0 += usize::from(e.checked);
                l.1 += e.done.unwrap_or(0);
            }
        }
        for i in (0..self.dirs.len()).rev() {
            let (Some(own), Some(parent)) = (self.live.get(i).copied(), self.dirs.get(i).and_then(|d| d.parent)) else {
                continue;
            };
            if let Some(p) = self.live.get_mut(parent) {
                p.0 += own.0;
                p.1 += own.1;
            }
        }
    }

    fn toggle_open(&mut self, dir: usize) {
        if let Some(d) = self.dirs.get(dir) {
            if !self.closed.remove(&d.id) {
                self.closed.insert(d.id);
            }
            self.visible_dirty = true;
        }
    }
}

/// Draw the tree in its own vertical scroll area of at most `max_height` (`entry(i)` for `i` below
/// `count` is what it shows); returns changes (if any).
#[allow(clippy::too_many_arguments)]
pub fn show<'a>(
    ui: &mut Ui,
    p: &Palette,
    state: &mut TreeState,
    id: Id,
    count: usize,
    entry: &dyn Fn(usize) -> Option<Entry<'a>>,
    filter: &str,
    editable: bool,
    max_height: f32,
) -> Option<TreeChange> {
    state.prepare(id, count, entry, filter);
    state.update_visible();
    state.update_live(entry);
    let mut change = TreeChange { set: Vec::new(), priority: Vec::new() };
    let show_done = entry(0).is_some_and(|e| e.done.is_some());
    let priorities = editable && entry(0).is_some_and(|e| e.priority.is_some());
    let mut toggle = None;
    let mut drawn = 0;
    // Dense and of one height: rows are placed by number, not measured.
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.spacing_mut().interact_size.y = ROW_HEIGHT - 2.0;
    let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if max_height.is_finite() {
        area = area.max_height(max_height).min_scrolled_height(max_height);
    }
    area.show_rows(ui, ROW_HEIGHT, state.visible.len(), |ui, range| {
        for &r in state.visible.get(range).unwrap_or_default() {
            let Some(row) = state.rows.get(r as usize) else { continue };
            drawn += 1;
            let line = ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_height(ROW_HEIGHT);
                    ui.add_space(row.depth as f32 * INDENT);
                    match row.kind {
                        RowKind::Dir(d) => {
                            if dir_row(ui, p, state, d, entry, show_done, editable, &mut change) {
                                toggle = Some(d);
                            }
                        }
                        RowKind::File(f) => {
                            if let Some(file) = state.files.get(f) {
                                file_row(ui, p, file, entry, editable, &mut change);
                            }
                        }
                    }
                },
            );
            if priorities {
                let resp = line.response.interact(egui::Sense::click());
                crate::ui::widgets::context_menu(&resp, |ui| match row.kind {
                    RowKind::Dir(d) => {
                        let inside = state.dirs.get(d).and_then(|d| state.files.get(d.files.clone()));
                        let entries: Vec<Entry<'a>> =
                            inside.unwrap_or_default().iter().filter_map(|f| entry(f.entry)).collect();
                        let ids: Vec<usize> = entries.iter().map(|e| e.index).collect();
                        let first = entries.first().and_then(|e| e.priority);
                        let same = entries.iter().all(|e| e.priority == first);
                        priority_menu(ui, &ids, first.filter(|_| same), &mut change);
                    }
                    RowKind::File(f) => {
                        if let Some(e) = state.files.get(f).and_then(|f| entry(f.entry)) {
                            priority_menu(ui, &[e.index], e.priority, &mut change);
                        }
                    }
                });
            }
        }
    });
    state.drawn = drawn;
    if let Some(d) = toggle {
        state.toggle_open(d);
    }
    (!change.set.is_empty() || !change.priority.is_empty()).then_some(change)
}

/// A folder's row; true when its arrow was clicked.
#[allow(clippy::too_many_arguments)]
fn dir_row<'a>(
    ui: &mut Ui,
    p: &Palette,
    state: &TreeState,
    d: usize,
    entry: &dyn Fn(usize) -> Option<Entry<'a>>,
    show_done: bool,
    editable: bool,
    change: &mut TreeChange,
) -> bool {
    let Some(dir) = state.dirs.get(d) else { return false };
    let (on_count, done) = state.live.get(d).copied().unwrap_or_default();
    let total = dir.files.len();
    let closed = state.is_closed(dir);
    let (rect, arrow) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if closed { icons::CARET_RIGHT } else { icons::CARET_DOWN },
        egui::FontId::proportional(12.0),
        if arrow.hovered() { p.text } else { p.text_dim },
    );
    crate::ui::widgets::focus_ring(ui, &arrow, p, 4);
    let arrow = arrow.labeled(if closed { t!("Expand") } else { t!("Collapse") });
    let mut on = on_count == total && total > 0;
    let cb = ui.add_enabled(editable, egui::Checkbox::new(&mut on, "").indeterminate(on_count > 0 && on_count < total));
    if cb.changed() {
        let target = on_count < total;
        for f in state.files.get(dir.files.clone()).unwrap_or_default() {
            if let Some(e) = entry(f.entry) {
                change.set.push((e.index, target));
            }
        }
    }
    ui.label(RichText::new(icons::FOLDER).color(p.warn));
    ui.label(RichText::new(&dir.name).color(p.text));
    row_tail(ui, p, dir.size, show_done.then_some(done), None);
    arrow.clicked()
}

fn file_row<'a>(
    ui: &mut Ui,
    p: &Palette,
    file: &FileRef,
    entry: &dyn Fn(usize) -> Option<Entry<'a>>,
    editable: bool,
    change: &mut TreeChange,
) {
    let Some(e) = entry(file.entry) else { return };
    // The rest of the path for a file kept at MAX_TREE_DEPTH (usually just its name).
    let name = e.components.get(file.at..).unwrap_or_default().join("/");
    ui.add_space(INDENT);
    let mut on = e.checked;
    if ui.add_enabled(editable, egui::Checkbox::new(&mut on, "")).changed() {
        change.set.push((e.index, on));
    }
    if e.streaming {
        ui.label(RichText::new(icons::PLAY_CIRCLE).color(p.accent))
            .on_hover_text(t!("Being streamed to a media player"));
    } else {
        let own = e.components.last().map(String::as_str).unwrap_or(&name);
        ui.label(RichText::new(file_icon(own)).color(p.text_dim));
    }
    ui.add(egui::Label::new(RichText::new(&name).color(if on { p.text } else { p.text_mute })).truncate());
    row_tail(ui, p, e.len, e.done, e.priority);
}

fn row_tail(ui: &mut Ui, p: &Palette, len: u64, done: Option<u64>, priority: Option<i8>) {
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
            cell(
                ui,
                40.0,
                RichText::new(format!("{:.0}%", (frac * 100.0).floor())).monospace().small().color(p.text_dim),
            );
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
        if let Some(pr) = priority.filter(|v| *v != 0) {
            let (icon, color) =
                if pr > 0 { (icons::ARROW_FAT_UP, p.accent) } else { (icons::ARROW_FAT_DOWN, p.text_mute) };
            ui.label(RichText::new(format!("{icon} {}", priority_label(pr))).small().color(color))
                .on_hover_text(t!("Download priority (right-click to change)"));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_by_extension() {
        assert_eq!(file_icon("movie.MKV"), icons::FILE_VIDEO);
        assert_eq!(file_icon("song.flac"), icons::FILE_AUDIO);
        assert_eq!(file_icon("archive.7z"), icons::FILE_ZIP);
        assert_eq!(file_icon("noext"), icons::FILE);
    }

    fn sample() -> Vec<Vec<String>> {
        vec![vec!["a".into(), "x.txt".into()], vec!["a".into(), "b".into(), "y.txt".into()], vec!["z.txt".into()]]
    }

    fn entry(comps: &[Vec<String>], checked: &[bool], i: usize) -> Option<Entry<'static>> {
        // Leaked: test data only, so the closures below stay simple.
        let c: &'static [String] = Box::leak(comps.get(i)?.clone().into_boxed_slice());
        Some(Entry {
            index: i,
            components: c,
            len: 10 * (i as u64 + 1),
            done: Some(i as u64),
            priority: None,
            streaming: false,
            checked: *checked.get(i)?,
        })
    }

    fn visible_names(s: &TreeState) -> Vec<String> {
        s.visible
            .iter()
            .filter_map(|&r| s.rows.get(r as usize))
            .map(|r| match r.kind {
                RowKind::Dir(d) => format!("{}/", s.dirs[d].name),
                RowKind::File(f) => format!("file{}", s.files[f].entry),
            })
            .collect()
    }

    #[test]
    fn tree_groups_by_directory() {
        let comps = sample();
        let mut root = Node::default();
        for (i, c) in comps.iter().enumerate() {
            root.insert(c, 0, i);
        }
        assert_eq!(root.files, vec![(2, 0)]);
        assert_eq!(root.dirs["a"].files, vec![(0, 1)]);
    }

    #[test]
    fn rows_flatten_and_collapse() {
        let comps = sample();
        let on = [true, true, true];
        let e = |i| entry(&comps, &on, i);
        let mut s = TreeState::default();
        s.prepare(Id::new("t"), 3, &e, "");
        s.update_visible();
        assert_eq!(visible_names(&s), ["a/", "b/", "file1", "file0", "file2"]);
        assert_eq!(s.rows[2].depth, 2, "y.txt is two folders down");
        // Closing "b" hides its file, closing "a" everything in it.
        s.toggle_open(1);
        s.update_visible();
        assert_eq!(visible_names(&s), ["a/", "b/", "file0", "file2"]);
        s.toggle_open(1);
        s.toggle_open(0);
        s.update_visible();
        assert_eq!(visible_names(&s), ["a/", "file2"]);
        // A filter opens everything without forgetting what was closed.
        s.prepare(Id::new("t"), 3, &e, "y.txt");
        s.update_visible();
        assert_eq!(visible_names(&s), ["a/", "b/", "file1"]);
        s.prepare(Id::new("t"), 3, &e, "");
        s.update_visible();
        assert_eq!(visible_names(&s), ["a/", "file2"]);
    }

    #[test]
    fn folders_count_their_whole_subtree() {
        let comps = sample();
        let on = [true, false, true];
        let e = |i| entry(&comps, &on, i);
        let mut s = TreeState::default();
        s.prepare(Id::new("t"), 3, &e, "");
        s.update_live(&e);
        // a: x.txt (index 0, on) and b/y.txt (index 1, off).
        assert_eq!(s.dirs[0].name, "a");
        assert_eq!(s.dirs[0].files.len(), 2);
        assert_eq!(s.dirs[0].size, 10 + 20);
        assert_eq!(s.live[0], (1, 1), "tri-state: one of two, its bytes done from both");
        assert_eq!(s.live[1], (0, 1));
    }

    #[test]
    fn the_tree_is_only_built_when_something_changed() {
        let comps = sample();
        let on = [true, true, true];
        let built = std::cell::Cell::new(0);
        let e = |i| {
            built.set(built.get() + 1);
            entry(&comps, &on, i)
        };
        let mut s = TreeState::default();
        s.prepare(Id::new("t"), 3, &e, "");
        let first = built.get();
        assert!(first > 0);
        s.prepare(Id::new("t"), 3, &e, "");
        assert_eq!(built.get(), first, "same torrent, same filter");
        s.prepare(Id::new("t"), 3, &e, "x");
        assert!(built.get() > first);
    }
}

#[cfg(test)]
mod depth_tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn deep(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("d{i}")).collect()
    }

    #[test]
    fn deep_paths_stop_nesting_at_the_cap() {
        let comps = deep(10_000);
        let mut root = Node::default();
        root.insert(&comps, 0, 0);
        let (mut node, mut levels) = (&root, 0);
        while let Some(d) = node.dirs.values().next() {
            node = d;
            levels += 1;
        }
        assert_eq!(levels, MAX_TREE_DEPTH);
        assert_eq!(node.files, vec![(0, MAX_TREE_DEPTH)]);
    }

    #[test]
    fn a_deep_tree_draws_on_a_small_stack() {
        // The Windows UI thread has 1 MiB of stack; a debug build has big frames.
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let comps = deep(10_000);
                let e = |_| {
                    Some(Entry {
                        index: 0,
                        components: &comps,
                        len: 1,
                        done: None,
                        priority: None,
                        streaming: false,
                        checked: true,
                    })
                };
                let p = crate::ui::theme::Palette::dark(egui::Color32::from_rgb(0x4a, 0x9e, 0xff));
                let mut state = TreeState::default();
                let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 3000.0)).build_ui(|ui| {
                    show(ui, &p, &mut state, Id::new("tree"), 1, &e, "", false, f32::INFINITY);
                });
                h.run();
                // The rest of the path is on the file's row.
                let rest = comps[MAX_TREE_DEPTH..].join("/");
                assert!(h.query_by_label(&rest).is_some());
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
