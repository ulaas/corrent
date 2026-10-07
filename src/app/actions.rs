//! Context menu, keyboard shortcuts and clipboard actions.

use egui::{Key, KeyboardShortcut, Modifiers, RichText, Ui};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::EngineCommand;
use crate::engine::types::{AddSource, TorrentState};
use crate::store::QueueMove;
use crate::ui::theme::Palette;

fn item(ui: &mut Ui, icon: &str, text: &str) -> bool {
    ui.button(format!("{icon}  {text}")).clicked()
}

impl CorrentApp {
    pub(super) fn context_menu(&mut self, ui: &mut Ui, p: &Palette) {
        ui.set_min_width(220.0);
        let sel: Vec<_> = self.selected_views().into_iter().cloned().collect();
        if sel.is_empty() {
            return;
        }
        let hashes: Vec<_> = sel.iter().map(|t| t.hash).collect();
        let single = sel.len() == 1;
        let first = sel[0].clone();

        if sel.iter().any(|t| t.state.is_paused_like() || t.state == TorrentState::Error)
            && item(ui, icons::PLAY, "Resume")
        {
            self.engine.send(EngineCommand::Resume(hashes.clone()));
        }
        if sel.iter().any(|t| !t.state.is_paused_like()) && item(ui, icons::PAUSE, "Pause") {
            self.engine.send(EngineCommand::Pause(hashes.clone()));
        }
        if item(ui, icons::LIGHTNING, "Force start") {
            self.engine.send(EngineCommand::ForceStart(hashes.clone()));
        }
        ui.separator();
        if single && item(ui, icons::FOLDER_OPEN, "Open folder") {
            super::open_path(&first.save_path);
        }
        if single && first.progress >= 1.0 && item(ui, icons::ARROW_SQUARE_OUT, "Open") {
            super::open_path(&first.content_path);
        }
        if item(ui, icons::FOLDER_SIMPLE_DASHED, "Move to…") {
            self.open_move_dialog(hashes.clone());
        }
        if single && item(ui, icons::PENCIL_SIMPLE, "Rename…  (F2)") {
            self.open_rename_dialog(first.hash, first.name.clone());
        }

        ui.menu_button(format!("{}  Category", icons::FOLDER_SIMPLE), |ui| {
            if ui.button("New category…").clicked() {
                self.open_new_category(Some(hashes.clone()));
            }
            ui.separator();
            if ui.selectable_label(sel.iter().all(|t| t.category.is_none()), "None").clicked() {
                self.engine.send(EngineCommand::SetCategory { hashes: hashes.clone(), category: None });
            }
            for c in self.snap.categories.keys() {
                let on = sel.iter().all(|t| t.category.as_ref() == Some(c));
                if ui.selectable_label(on, c).clicked() {
                    self.engine.send(EngineCommand::SetCategory { hashes: hashes.clone(), category: Some(c.clone()) });
                }
            }
        });
        ui.menu_button(format!("{}  Tags", icons::TAG), |ui| {
            if ui.button("New tag…").clicked() {
                self.open_new_tag(Some(hashes.clone()));
            }
            if !self.snap.tags.is_empty() {
                ui.separator();
            }
            for tag in self.snap.tags.clone() {
                let all = sel.iter().all(|t| t.tags.contains(&tag));
                let mut on = all;
                if ui.checkbox(&mut on, &tag).changed() {
                    self.engine.send(EngineCommand::ToggleTag { hashes: hashes.clone(), tag: tag.clone(), on });
                }
            }
            if sel.iter().any(|t| !t.tags.is_empty()) {
                ui.separator();
                if ui.button("Remove all tags").clicked() {
                    self.engine.send(EngineCommand::SetTags { hashes: hashes.clone(), tags: Vec::new() });
                }
            }
        });
        ui.menu_button(format!("{}  Queue", icons::LIST_NUMBERS), |ui| {
            for (label, mv) in [
                ("Move to top", QueueMove::Top),
                ("Move up", QueueMove::Up),
                ("Move down", QueueMove::Down),
                ("Move to bottom", QueueMove::Bottom),
            ] {
                if ui.button(label).clicked() {
                    self.engine.send(EngineCommand::QueueMove { hashes: hashes.clone(), mv });
                }
            }
        });
        if item(ui, icons::SCALES, "Seeding limits…") {
            self.open_seed_limits(hashes.clone());
        }
        ui.separator();
        if item(ui, icons::ARROWS_CLOCKWISE, "Force recheck") {
            self.engine.send(EngineCommand::Recheck(hashes.clone()));
        }
        if single && item(ui, icons::BROADCAST, "Add trackers…") {
            self.open_trackers_dialog(first.hash);
        }
        ui.menu_button(format!("{}  Copy", icons::COPY), |ui| {
            if ui.button("Name").clicked() {
                self.copy(sel.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button("Info hash").clicked() {
                self.copy(sel.iter().map(|t| t.hash.to_hex()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button("Magnet link").clicked() {
                self.copy(sel.iter().map(|t| t.magnet()).collect::<Vec<_>>().join("\n"));
            }
            if ui.button("Save path").clicked() {
                self.copy(sel.iter().map(|t| t.content_path.display().to_string()).collect::<Vec<_>>().join("\n"));
            }
        });
        ui.separator();
        if ui.button(RichText::new(format!("{}  Remove…", icons::TRASH))).clicked() {
            self.request_delete(hashes.clone(), false);
        }
        if ui.button(RichText::new(format!("{}  Remove with files…", icons::TRASH_SIMPLE)).color(p.error)).clicked() {
            self.request_delete(hashes, true);
        }
    }

    pub fn copy(&mut self, text: String) {
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(()) => self.toasts.push(crate::engine::ToastKind::Info, "Copied to clipboard"),
            Err(e) => self.toasts.push(crate::engine::ToastKind::Error, format!("Clipboard error: {e}")),
        }
    }

    /// Global keyboard shortcuts. Skipped while a text field has focus or a modal is open.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let mut consume = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));

        if consume(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::O)) {
            self.open_magnet_dialog();
        }
        if consume(sc(Modifiers::COMMAND, Key::O)) {
            self.pick_torrent_files();
        }
        if consume(sc(Modifiers::COMMAND, Key::Comma)) {
            self.open_settings();
        }
        if consume(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)) {
            self.engine.send(EngineCommand::SetAltSpeed(!self.snap.session.alt_speed));
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
        if self.modal_open() || typing {
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
            let sources: Vec<AddSource> = text.lines().filter_map(AddSource::parse).collect();
            if sources.is_empty() {
                self.toasts.push(crate::engine::ToastKind::Warning, "The clipboard doesn't contain a magnet link or torrent URL.");
            }
            for s in sources {
                self.launch_queue.push_back(s);
            }
        }
        if consume(sc(Modifiers::COMMAND, Key::A)) {
            let rows = self.visible_rows().to_vec();
            self.selected = rows.iter().map(|&i| self.snap.torrents[i].hash).collect();
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
            if sel.len() == 1 {
                let (h, n) = (sel[0].hash, sel[0].name.clone());
                self.open_rename_dialog(h, n);
            }
        }
        if consume(sc(Modifiers::NONE, Key::Enter)) {
            let sel = self.selected_views();
            if sel.len() == 1 {
                super::open_path(&sel[0].content_path.clone());
            }
        }
        if consume(sc(Modifiers::NONE, Key::Escape)) {
            self.selected.clear();
        }
        // Arrow navigation
        let up = consume(sc(Modifiers::NONE, Key::ArrowUp)) || consume(sc(Modifiers::SHIFT, Key::ArrowUp));
        let down = consume(sc(Modifiers::NONE, Key::ArrowDown)) || consume(sc(Modifiers::SHIFT, Key::ArrowDown));
        if up || down {
            let shift = ctx.input(|i| i.modifiers.shift);
            let rows = self.visible_rows().to_vec();
            if rows.is_empty() {
                return;
            }
            let hashes: Vec<_> = rows.iter().map(|&i| self.snap.torrents[i].hash).collect();
            let cur = self.focus.and_then(|f| hashes.iter().position(|h| *h == f));
            let next = match (cur, up) {
                (None, _) => 0,
                (Some(c), true) => c.saturating_sub(1),
                (Some(c), false) => (c + 1).min(hashes.len() - 1),
            };
            let h = hashes[next];
            if shift {
                self.selected.insert(h);
            } else {
                self.selected.clear();
                self.selected.insert(h);
                self.anchor = Some(h);
            }
            self.focus = Some(h);
            self.scroll_to_focus = true;
        }
    }
}
