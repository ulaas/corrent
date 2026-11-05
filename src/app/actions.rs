//! Context menu, keyboard shortcuts and clipboard actions.

use egui::{Key, KeyboardShortcut, Modifiers, RichText, Ui};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::EngineCommand;
use crate::engine::types::{AddSource, InfoHash, TorrentState, TorrentView};
use crate::store::QueueMove;
use crate::ui::theme::Palette;

/// (torrent, its files, how many are selected) and its playable files.
pub(super) type MediaFilesCache =
    (crate::engine::types::InfoHash, usize, usize, std::sync::Arc<Vec<(usize, String, u64)>>);

#[derive(Clone, Copy, Debug)]
enum Move {
    By(isize),
    First,
    Last,
}

/// What a selection can be widened by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SameAs {
    Category,
    Tag,
    Tracker,
}

fn item(ui: &mut Ui, icon: &str, text: &str) -> bool {
    ui.button(format!("{icon}  {text}")).clicked()
}

impl CorrentApp {
    pub(super) fn context_menu(&mut self, ui: &mut Ui, p: &Palette) {
        ui.set_min_width(220.0);
        // Drawn every frame while open, with possibly thousands selected: borrowed, not copied.
        let snap = self.snap.clone();
        let sel: Vec<&TorrentView> =
            snap.torrents.iter().filter(|t| self.selected.contains(&t.hash) && !t.pending).collect();
        let hashes: Vec<_> = sel.iter().map(|t| t.hash).collect();
        let single = sel.len() == 1;
        let Some(&first) = sel.first() else { return };

        let label = if sel.iter().all(|t| t.restore_failed || t.state == TorrentState::Error) {
            t!("Retry")
        } else {
            t!("Resume")
        };
        if sel.iter().any(|t| t.state.is_paused_like() || t.state == TorrentState::Error)
            && item(ui, if label == t!("Retry") { icons::ARROW_CLOCKWISE } else { icons::PLAY }, label)
        {
            self.engine.send(EngineCommand::Resume(hashes.clone()));
        }
        if sel.iter().any(|t| !t.state.is_paused_like()) && item(ui, icons::PAUSE, t!("Pause")) {
            self.engine.send(EngineCommand::Pause(hashes.clone()));
        }
        if item(ui, icons::LIGHTNING, t!("Force start")) {
            self.engine.send(EngineCommand::ForceStart(hashes.clone()));
        }
        // Disk-full / access problems: the fix is usually a different folder.
        let io_problem = first.error.as_deref().map(crate::util::classify_io).is_some_and(|k| {
            matches!(k, crate::util::IoKind::DiskFull | crate::util::IoKind::Permission | crate::util::IoKind::NotFound)
        });
        if single && io_problem && !first.restore_failed {
            ui.separator();
            if ui
                .button(
                    RichText::new(format!("{}  {}", icons::FOLDER_SIMPLE_DASHED, t!("Choose another folder…")))
                        .color(p.accent),
                )
                .clicked()
            {
                self.open_move_dialog(hashes.clone());
            }
        }
        ui.separator();
        // A server's files aren't on this computer.
        let local = self.server.is_none();
        if single && local && item(ui, icons::FOLDER_OPEN, t!("Open folder")) {
            super::open_path(&first.save_path);
        }
        if single && local && first.progress >= 1.0 && item(ui, icons::ARROW_SQUARE_OUT, t!("Open")) {
            super::open_path(&first.content_path);
        }
        if single {
            let media = self.media_files(first.hash);
            if !media.is_empty() {
                crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::PLAY_CIRCLE, t!("Play")), |ui| {
                    ui.label(RichText::new(t!("Streams while downloading")).small().color(p.text_dim));
                    for (idx, name, _) in media.iter().take(12) {
                        let short = name.rsplit(['/', '\\']).next().unwrap_or(name);
                        if ui.button(format!("{}  {short}", icons::PLAY)).clicked() {
                            self.play(first.hash, *idx, name);
                        }
                    }
                    ui.separator();
                    if let Some((idx, name, _)) = media.first()
                        && ui.button(format!("{}  {}", icons::LINK, t!("Copy stream link"))).clicked()
                        && let Some(url) = self.stream_url(first.hash, *idx, name)
                    {
                        self.copy(url);
                    }
                });
            }
        }
        if item(ui, icons::FOLDER_SIMPLE_DASHED, t!("Move to…")) {
            self.open_move_dialog(hashes.clone());
        }
        // This computer's libraries (the media pages show a connected server's instead).
        if single
            && local
            && first.has_metadata
            && !first.restore_failed
            && self.media.client.is_some()
            && item(ui, icons::FILM_SLATE, t!("Add to library…"))
        {
            self.open_add_to_library(first.hash);
        }
        if single && item(ui, icons::PENCIL_SIMPLE, t!("Rename…  (F2)")) {
            self.open_rename_dialog(first.hash, first.name.clone());
        }

        crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::FOLDER_SIMPLE, t!("Category")), |ui| {
            if ui.button(t!("New category…")).clicked() {
                self.open_new_category(Some(hashes.clone()));
            }
            ui.separator();
            if ui.selectable_label(sel.iter().all(|t| t.category.is_none()), t!("None")).clicked() {
                self.engine.send(EngineCommand::SetCategory { hashes: hashes.clone(), category: None });
            }
            for c in snap.categories.keys() {
                let on = sel.iter().all(|t| t.category.as_ref() == Some(c));
                if ui.selectable_label(on, c).clicked() {
                    self.engine.send(EngineCommand::SetCategory { hashes: hashes.clone(), category: Some(c.clone()) });
                }
            }
        });
        crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::TAG, t!("Tags")), |ui| {
            if ui.button(t!("New tag…")).clicked() {
                self.open_new_tag(Some(hashes.clone()));
            }
            if !snap.tags.is_empty() {
                ui.separator();
            }
            for tag in &snap.tags {
                let all = sel.iter().all(|t| t.tags.contains(tag));
                let mut on = all;
                if ui.checkbox(&mut on, tag).changed() {
                    self.engine.send(EngineCommand::ToggleTag { hashes: hashes.clone(), tag: tag.clone(), on });
                }
            }
            if sel.iter().any(|t| !t.tags.is_empty()) {
                ui.separator();
                if ui.button(t!("Remove all tags")).clicked() {
                    self.engine.send(EngineCommand::SetTags { hashes: hashes.clone(), tags: Vec::new() });
                }
            }
        });
        crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::LIST_NUMBERS, t!("Queue")), |ui| {
            for (label, mv) in [
                (t!("Move to top"), QueueMove::Top),
                (t!("Move up"), QueueMove::Up),
                (t!("Move down"), QueueMove::Down),
                (t!("Move to bottom"), QueueMove::Bottom),
            ] {
                if ui.button(label).clicked() {
                    self.engine.send(EngineCommand::QueueMove { hashes: hashes.clone(), mv });
                }
            }
        });
        if item(ui, icons::GAUGE, t!("Speed limits…")) {
            self.open_speed_limits(hashes.clone());
        }
        if item(ui, icons::SCALES, t!("Seeding limits…")) {
            self.open_seed_limits(hashes.clone());
        }
        ui.separator();
        if item(ui, icons::ARROWS_CLOCKWISE, t!("Force recheck")) {
            self.engine.send(EngineCommand::Recheck(hashes.clone()));
        }
        if item(ui, icons::BROADCAST, t!("Add trackers…")) {
            self.open_trackers_dialog(hashes.clone());
        }
        crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::COPY, t!("Copy")), |ui| {
            if ui.button(t!("Name")).clicked() {
                self.copy(sel.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button(t!("Info hash")).clicked() {
                self.copy(sel.iter().map(|t| t.hash.to_hex()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button(t!("Magnet link")).clicked() {
                self.copy(sel.iter().map(|t| t.magnet()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button(t!("Save path")).clicked() {
                self.copy(sel.iter().map(|t| t.content_path.display().to_string()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button(t!("Tracker hosts")).clicked() {
                let mut hosts: Vec<String> = Vec::new();
                for u in sel.iter().flat_map(|t| t.trackers.iter()) {
                    hosts.push(self.counts.hosts.get(u).to_string());
                }
                hosts.sort();
                hosts.dedup();
                self.copy(hosts.join("\n"));
            }
        });
        crate::ui::widgets::menu_button(ui, format!("{}  {}", icons::SELECTION, t!("Select")), |ui| {
            if ui.button(t!("Invert selection")).clicked() {
                self.invert_selection();
                ui.close();
            }
            ui.separator();
            for (label, what) in [
                (t!("Same category"), SameAs::Category),
                (t!("Same tag"), SameAs::Tag),
                (t!("Same tracker"), SameAs::Tracker),
            ] {
                if ui.button(label).clicked() {
                    self.select_same(what);
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui.button(RichText::new(format!("{}  {}", icons::TRASH, t!("Remove…")))).clicked() {
            self.request_delete(hashes.clone(), false);
        }
        if ui
            .button(RichText::new(format!("{}  {}", icons::TRASH_SIMPLE, t!("Remove with files…"))).color(p.error))
            .clicked()
        {
            self.request_delete(hashes, true);
        }
    }

    /// Playable files of a torrent (from the details data), largest first: (index, path, size).
    /// Asked every frame by the Files tab and the context menu; worked out when the files change.
    pub fn media_files(&mut self, hash: crate::engine::types::InfoHash) -> std::sync::Arc<Vec<(usize, String, u64)>> {
        let snap = self.snap.clone();
        let Some(d) = snap.detail.as_ref().filter(|d| d.hash == hash) else { return Default::default() };
        let included = d.files.iter().filter(|f| f.included).count();
        if let Some((h, n, i, list)) = &self.media_files_cache
            && (*h, *n, *i) == (hash, d.files.len(), included)
        {
            return list.clone();
        }
        let mut v: Vec<(usize, String, u64)> = d
            .files
            .iter()
            .filter(|f| f.included)
            .map(|f| (f.index, f.components.join("/"), f.len))
            .filter(|(_, n, _)| crate::engine::stream::is_media(n))
            .collect();
        v.sort_by_key(|f| std::cmp::Reverse(f.2));
        let list = std::sync::Arc::new(v);
        self.media_files_cache = Some((hash, d.files.len(), included, list.clone()));
        list
    }

    pub fn stream_url(&self, hash: crate::engine::types::InfoHash, idx: usize, name: &str) -> Option<String> {
        let (port, token) = self.snap.session.stream.clone()?;
        Some(crate::engine::stream::stream_url(port, &token, hash, idx, name))
    }

    /// Open a file in the default media player via the local streaming server.
    pub fn play(&mut self, hash: crate::engine::types::InfoHash, idx: usize, name: &str) {
        let Some(url) = self.stream_url(hash, idx, name) else {
            self.toasts.push(crate::engine::ToastKind::Error, t!("Streaming isn't available right now."));
            return;
        };
        // A player stopped from the Streaming window is refused for a few seconds; this is the
        // user asking for it again.
        self.engine.send(crate::engine::EngineCommand::AllowStream { hash, file: idx });
        let title = name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
        if let Err(e) = crate::engine::stream::open_in_player(&self.paths.data_dir, &title, &url) {
            self.toasts.push(crate::engine::ToastKind::Error, t!("Couldn't open a media player: {e}", e = e));
        }
    }

    /// Through eframe: the clipboard can be held by another process, and the UI thread mustn't
    /// wait for it.
    pub fn copy(&mut self, text: String) {
        self.ctx.copy_text(text);
        self.toasts.push(crate::engine::ToastKind::Info, t!("Copied to clipboard"));
    }

    /// Global keyboard shortcuts, taken before anything is drawn: a key used here doesn't also
    /// reach a focused widget, and a dialog that closes on this key this frame still counts as
    /// open. Skipped while a widget has the focus or a dialog is open; the keys that act on the
    /// list only work on the list, with no menu open.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let consume = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));

        if consume(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)) {
            self.engine.send(EngineCommand::SetAltSpeed(!self.snap.session.alt_speed));
        }
        if self.palette.is_some() && consume(sc(Modifiers::COMMAND, Key::K)) {
            self.toggle_palette();
        }
        // Also the dialogs not listed in `modal_open` (About, Connect, the media pages' own).
        if self.modal_open() || ctx.memory(|m| m.top_modal_layer().is_some()) {
            return;
        }
        if consume(sc(Modifiers::COMMAND, Key::K)) {
            self.toggle_palette();
        }
        if consume(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::O)) {
            self.open_magnet_dialog();
        }
        if consume(sc(Modifiers::COMMAND, Key::O)) {
            self.pick_torrent_files();
        }
        if consume(sc(Modifiers::COMMAND, Key::N)) {
            self.open_creator();
        }
        if consume(sc(Modifiers::COMMAND, Key::Comma)) {
            self.open_settings();
        }
        if consume(sc(Modifiers::COMMAND, Key::F)) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search_box")));
        }
        if consume(sc(Modifiers::COMMAND, Key::I)) {
            self.prefs.show_details = !self.prefs.show_details;
        }
        if consume(sc(Modifiers::COMMAND, Key::B)) {
            self.prefs.show_sidebar = !self.prefs.show_sidebar;
        }
        if consume(sc(Modifiers::NONE, Key::F1)) {
            self.extras.shortcuts = !self.extras.shortcuts;
        }
        if typing {
            return;
        }
        // Pasting a magnet / URL / hash anywhere adds it.
        let pasted: Option<String> = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(s) => Some(s.clone()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            let sources: Vec<AddSource> = text.lines().filter_map(AddSource::parse_text).collect();
            if sources.is_empty() {
                self.toasts.push(
                    crate::engine::ToastKind::Warning,
                    t!("The clipboard doesn't contain a magnet link or torrent URL."),
                );
            }
            for s in sources {
                self.launch_queue.push_back(s);
            }
        }
        if self.main_view != super::feeds::MainView::Torrents || egui::Popup::is_any_open(ctx) {
            return;
        }
        if self.main_view != super::feeds::MainView::Torrents || egui::Popup::is_any_open(ctx) {
            return;
        }
        if consume(sc(Modifiers::COMMAND, Key::A)) {
            let rows = self.visible_rows().to_vec();
            self.selected = rows.iter().filter_map(|&i| self.snap.torrents.get(i).map(|t| t.hash)).collect();
        }
        if consume(sc(Modifiers::SHIFT, Key::Delete)) {
            let h = self.selected_hashes();
            if !h.is_empty() {
                self.request_delete(h, true);
            }
        }
        if consume(sc(Modifiers::NONE, Key::Delete)) {
            let h = self.selected_hashes();
            if !h.is_empty() {
                self.request_delete(h, false);
            }
        }
        if consume(sc(Modifiers::NONE, Key::Space)) {
            let sel = self.selected_views();
            if !sel.is_empty() {
                let any_running = sel.iter().any(|t| !t.state.is_paused_like());
                let h = self.selected_hashes();
                self.engine.send(if any_running { EngineCommand::Pause(h) } else { EngineCommand::Resume(h) });
            }
        }
        if consume(sc(Modifiers::NONE, Key::F2)) {
            let sel = self.selected_views();
            if let [t] = sel.as_slice() {
                let (h, n) = (t.hash, t.name.clone());
                self.open_rename_dialog(h, n);
            }
        }
        if consume(sc(Modifiers::NONE, Key::Enter)) {
            let sel = self.selected_views();
            if let [t] = sel.as_slice()
                && self.server.is_none()
            {
                super::open_path(&t.content_path.clone());
            }
        }
        // egui has already dropped the focus for this key: it was only for the widget that had it.
        if !self.had_focus && consume(sc(Modifiers::NONE, Key::Escape)) {
            self.selected.clear();
        }
        if consume(sc(Modifiers::NONE, Key::Slash)) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search_box")));
        }
        if consume(sc(Modifiers::COMMAND, Key::Space)) {
            self.toggle_focused();
        }
        // Moving through the list. Ctrl moves only the focus; Shift selects from the anchor.
        let page = self.page_rows.max(1);
        let moves = [
            (Key::ArrowUp, Move::By(-1)),
            (Key::ArrowDown, Move::By(1)),
            (Key::PageUp, Move::By(-(page as isize))),
            (Key::PageDown, Move::By(page as isize)),
            (Key::Home, Move::First),
            (Key::End, Move::Last),
        ];
        let moved = moves.into_iter().find_map(|(key, how)| {
            let hit = consume(sc(Modifiers::COMMAND, key)) || consume(sc(Modifiers::NONE, key));
            hit.then_some(how)
        });
        if let Some(how) = moved {
            let mods = ctx.input(|i| i.modifiers);
            self.move_focus(how, mods.shift, mods.command);
        }
    }

    /// The torrents in the list, in its order.
    fn visible_hashes(&mut self) -> Vec<InfoHash> {
        let snap = self.snap.clone();
        self.visible_rows().iter().filter_map(|&i| snap.torrents.get(i).map(|t| t.hash)).collect()
    }

    fn move_focus(&mut self, how: Move, extend: bool, focus_only: bool) {
        let hashes = self.visible_hashes();
        let Some(last) = hashes.len().checked_sub(1) else { return };
        let cur = self.focus.and_then(|f| hashes.iter().position(|h| *h == f));
        let next = match (how, cur) {
            (Move::First, _) => 0,
            (Move::Last, _) => last,
            (Move::By(_), None) => 0,
            (Move::By(n), Some(c)) => c.saturating_add_signed(n).min(last),
        };
        let Some(&h) = hashes.get(next) else { return };
        if focus_only {
            // The selection stays.
        } else if extend {
            // From the anchor, which stays for the next Shift+move or Shift+click.
            let anchor = self.anchor.or(self.focus).unwrap_or(h);
            self.anchor = Some(anchor);
            let a = hashes.iter().position(|x| *x == anchor).unwrap_or(next);
            self.selected = hashes.get(a.min(next)..=a.max(next)).unwrap_or_default().iter().copied().collect();
        } else {
            self.selected.clear();
            self.selected.insert(h);
            self.anchor = Some(h);
        }
        self.focus = Some(h);
        self.focus_visible = true;
        self.scroll_to_focus = true;
    }

    /// Ctrl+Space: the focused row in or out of the selection.
    fn toggle_focused(&mut self) {
        if let Some(h) = self.focus {
            if !self.selected.remove(&h) {
                self.selected.insert(h);
            }
            self.anchor = Some(h);
            self.focus_visible = true;
        }
    }

    pub(super) fn invert_selection(&mut self) {
        let all = self.visible_hashes();
        self.selected = all.into_iter().filter(|h| !self.selected.contains(h)).collect();
        self.anchor = None;
    }

    /// Selects the visible torrents that have what the focused one (or the first selected) has.
    pub(super) fn select_same(&mut self, what: SameAs) {
        let snap = self.snap.clone();
        let current =
            self.focus.filter(|f| self.selected.contains(f)).or_else(|| self.selected_hashes().first().copied());
        let Some(base) = current.and_then(|h| snap.torrents.iter().find(|t| t.hash == h)) else { return };
        let mut base_hosts: Vec<String> = Vec::new();
        for u in base.trackers.iter() {
            base_hosts.push(self.counts.hosts.get(u).to_string());
        }
        let hosts = &self.counts.hosts;
        let same = |t: &TorrentView| match what {
            SameAs::Category => t.category == base.category,
            SameAs::Tag => t.tags.iter().any(|x| base.tags.contains(x)),
            SameAs::Tracker => t.trackers.iter().any(|u| base_hosts.iter().any(|h| h == hosts.peek(u))),
        };
        let chosen: Vec<InfoHash> =
            (self.rows.iter().filter_map(|&i| snap.torrents.get(i))).filter(|t| same(t)).map(|t| t.hash).collect();
        self.selected = chosen.into_iter().collect();
    }
}
