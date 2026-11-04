//! Command palette (Ctrl+K): every action, settings page, sidebar filter and torrent by typing
//! a few letters of its name, without leaving the keyboard.

use egui::{Context, CornerRadius, Id, Key, Modifiers, RichText, Sense, vec2};
use egui_phosphor::regular as icons;

use super::actions::SameAs;
use super::feeds::MainView;
use super::settings_window::Page;
use super::{CorrentApp, GroupFilter, StatusFilter};
use crate::engine::EngineCommand;
use crate::engine::types::InfoHash;
use crate::store::QueueMove;
use crate::ui::theme::Palette;
use crate::ui::widgets;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Pause,
    Resume,
    ForceStart,
    Recheck,
    Remove,
    RemoveWithFiles,
    MoveTo,
    SpeedLimits,
    SeedLimits,
    AddTrackers,
    Rename,
    Queue(QueueMove),
    CopyName,
    CopyHash,
    CopyMagnet,
    Invert,
    SelectAll,
    Same(SameAs),
    AddFile,
    AddMagnet,
    Create,
    ToggleSidebar,
    ToggleDetails,
    AltSpeed,
    PauseAll,
    ResumeAll,
    Statistics,
    Log,
    Streaming,
    Shortcuts,
    About,
    TvMode,
    Import,
    Diagnostics,
    View(MainView),
    Settings(Page),
    SavedView(String),
    SaveView,
    Status(StatusFilter),
    Category(GroupFilter),
    Tag(GroupFilter),
    Tracker(GroupFilter),
    ShowTorrent(InfoHash),
}

struct Item {
    icon: &'static str,
    label: String,
    /// What kind of thing it is, shown on the right.
    kind: &'static str,
    /// Lower case: the label, the kind and any further words it answers to.
    hay: String,
    action: Action,
    /// Torrents only show up once something is typed.
    needs_query: bool,
}

impl Item {
    fn new(icon: &'static str, label: &str, kind: &'static str, action: Action) -> Item {
        let hay = format!("{label} {kind}").to_lowercase();
        Item { icon, label: label.to_string(), kind, hay, action, needs_query: false }
    }

    fn with_words(mut self, words: &str) -> Item {
        self.hay.push(' ');
        self.hay.push_str(&words.to_lowercase());
        self
    }
}

#[derive(Default)]
pub struct CommandPalette {
    pub query: String,
    items: Vec<Item>,
    /// `items` that match the query, best first.
    shown: Vec<usize>,
    shown_for: Option<String>,
    selected: usize,
    focused: bool,
    /// The list's scroll position, and whether the keyboard just moved the selection.
    scroll: f32,
    moved: bool,
}

/// How well `q` (lower case words) fits an item; None when it doesn't.
fn score(item: &Item, q: &str) -> Option<u32> {
    if !q.split_whitespace().all(|w| item.hay.contains(w)) {
        return None;
    }
    let label = item.label.to_lowercase();
    Some(if label == q {
        0
    } else if label.starts_with(q) {
        1
    } else if label.split_whitespace().any(|w| w.starts_with(q)) {
        2
    } else {
        3
    })
}

impl CommandPalette {
    fn refresh(&mut self) {
        let q = self.query.trim().to_lowercase();
        if self.shown_for.as_deref() == Some(q.as_str()) {
            return;
        }
        let mut hits: Vec<(u32, usize)> = (self.items.iter().enumerate())
            .filter(|(_, it)| !q.is_empty() || !it.needs_query)
            .filter_map(|(i, it)| score(it, &q).map(|s| (s, i)))
            .collect();
        // Stable: within a score, actions keep their place ahead of filters and torrents.
        hits.sort_by_key(|(s, _)| *s);
        self.shown = hits.into_iter().map(|(_, i)| i).collect();
        self.shown_for = Some(q);
        self.selected = 0;
        self.scroll = 0.0;
        self.moved = true;
    }

    fn select(&mut self, by: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(by).min(last);
        self.moved = true;
    }
}

const ROW: f32 = 30.0;
const GAP: f32 = 2.0;
const LIST_HEIGHT: f32 = 340.0;

impl CorrentApp {
    pub(super) fn toggle_palette(&mut self) {
        self.palette = match self.palette {
            Some(_) => None,
            None => Some(CommandPalette { items: self.palette_items(), ..Default::default() }),
        };
    }

    fn palette_items(&mut self) -> Vec<Item> {
        let mut v: Vec<Item> = Vec::new();
        let local = self.server.is_none();
        let n = self.selected_hashes().len();
        if n > 0 {
            let on = t!("Selected");
            let single = n == 1;
            let mut sel = |icon, label: &str, action| v.push(Item::new(icon, label, on, action));
            sel(icons::PAUSE, t!("Pause"), Action::Pause);
            sel(icons::PLAY, t!("Resume"), Action::Resume);
            sel(icons::LIGHTNING, t!("Force start"), Action::ForceStart);
            sel(icons::ARROWS_CLOCKWISE, t!("Force recheck"), Action::Recheck);
            sel(icons::FOLDER_SIMPLE_DASHED, t!("Move to…"), Action::MoveTo);
            sel(icons::GAUGE, t!("Speed limits…"), Action::SpeedLimits);
            sel(icons::SCALES, t!("Seeding limits…"), Action::SeedLimits);
            sel(icons::BROADCAST, t!("Add trackers…"), Action::AddTrackers);
            if single {
                sel(icons::PENCIL_SIMPLE, t!("Rename…"), Action::Rename);
            }
            sel(icons::CARET_DOUBLE_UP, t!("Move to top"), Action::Queue(QueueMove::Top));
            sel(icons::CARET_UP, t!("Move up"), Action::Queue(QueueMove::Up));
            sel(icons::CARET_DOWN, t!("Move down"), Action::Queue(QueueMove::Down));
            sel(icons::CARET_DOUBLE_DOWN, t!("Move to bottom"), Action::Queue(QueueMove::Bottom));
            sel(icons::COPY, t!("Copy name"), Action::CopyName);
            sel(icons::COPY, t!("Copy info hash"), Action::CopyHash);
            sel(icons::COPY, t!("Copy magnet link"), Action::CopyMagnet);
            sel(icons::SELECTION, t!("Invert selection"), Action::Invert);
            sel(icons::SELECTION, t!("Same category"), Action::Same(SameAs::Category));
            sel(icons::SELECTION, t!("Same tag"), Action::Same(SameAs::Tag));
            sel(icons::SELECTION, t!("Same tracker"), Action::Same(SameAs::Tracker));
            sel(icons::TRASH, t!("Remove…"), Action::Remove);
            sel(icons::TRASH_SIMPLE, t!("Remove with files…"), Action::RemoveWithFiles);
        }
        let app = t!("Command");
        let mut cmd = |icon, label: &str, action| v.push(Item::new(icon, label, app, action));
        cmd(icons::FILE_PLUS, t!("Add torrent files"), Action::AddFile);
        cmd(icons::MAGNET, t!("Add magnet links or URLs"), Action::AddMagnet);
        if local {
            cmd(icons::FILE_PLUS, t!("Create a torrent"), Action::Create);
            cmd(icons::ARROW_SQUARE_IN, t!("Import from another client…"), Action::Import);
            cmd(icons::PLAY_CIRCLE, t!("Streaming"), Action::Streaming);
        }
        cmd(icons::SELECTION_ALL, t!("Select all"), Action::SelectAll);
        cmd(icons::PAUSE, t!("Pause all"), Action::PauseAll);
        cmd(icons::PLAY, t!("Resume all"), Action::ResumeAll);
        cmd(icons::GAUGE, t!("Alternative speed limits on or off"), Action::AltSpeed);
        cmd(icons::SIDEBAR, t!("Show or hide the sidebar"), Action::ToggleSidebar);
        cmd(icons::ROWS, t!("Show or hide the details"), Action::ToggleDetails);
        cmd(icons::CHART_BAR, t!("Statistics"), Action::Statistics);
        cmd(icons::SCROLL, t!("Log"), Action::Log);
        cmd(icons::KEYBOARD, t!("Keyboard shortcuts"), Action::Shortcuts);
        cmd(icons::FIRST_AID_KIT, t!("Save diagnostics…"), Action::Diagnostics);
        cmd(icons::TELEVISION, t!("TV mode"), Action::TvMode);
        cmd(icons::INFO, t!("About Corrent"), Action::About);
        let view = t!("View");
        v.push(Item::new(icons::LIST, t!("Torrents"), view, Action::View(MainView::Torrents)));
        v.push(Item::new(icons::FILM_SLATE, t!("Media"), view, Action::View(MainView::Media)));
        v.push(Item::new(icons::RSS, t!("RSS feeds"), view, Action::View(MainView::Feeds)));
        v.push(Item::new(icons::MAGNIFYING_GLASS, t!("Search"), view, Action::View(MainView::Search)));
        for sv in &self.prefs.views {
            v.push(Item::new(icons::BOOKMARK_SIMPLE, &sv.name, view, Action::SavedView(sv.name.clone())));
        }
        v.push(Item::new(icons::BOOKMARK_SIMPLE, t!("Save view…"), app, Action::SaveView));
        for page in Page::ALL {
            v.push(
                Item::new(page.icon(), page.label(), t!("Settings"), Action::Settings(page))
                    .with_words(page.keywords()),
            );
        }
        for f in StatusFilter::ALL {
            v.push(Item::new(f.icon(), f.label(), t!("Status"), Action::Status(f)));
        }
        let category = t!("Category");
        v.push(Item::new(icons::FOLDERS, t!("All"), category, Action::Category(GroupFilter::All)));
        v.push(Item::new(icons::FOLDER_DASHED, t!("Uncategorized"), category, Action::Category(GroupFilter::None)));
        for c in self.snap.categories.keys() {
            v.push(Item::new(icons::FOLDER_SIMPLE, c, category, Action::Category(GroupFilter::Named(c.clone()))));
        }
        let tag = t!("Tag");
        v.push(Item::new(icons::TAG, t!("All"), tag, Action::Tag(GroupFilter::All)));
        v.push(Item::new(icons::TAG_SIMPLE, t!("Untagged"), tag, Action::Tag(GroupFilter::None)));
        for x in &self.snap.tags {
            v.push(Item::new(icons::HASH, x, tag, Action::Tag(GroupFilter::Named(x.clone()))));
        }
        self.counts.update(&self.snap);
        let tracker = t!("Tracker");
        v.push(Item::new(icons::BROADCAST, t!("All"), tracker, Action::Tracker(GroupFilter::All)));
        v.push(Item::new(icons::PROHIBIT, t!("No trackers"), tracker, Action::Tracker(GroupFilter::None)));
        for host in self.counts.trackers.keys() {
            v.push(Item::new(icons::GLOBE_SIMPLE, host, tracker, Action::Tracker(GroupFilter::Named(host.clone()))));
        }
        let torrent = t!("Torrent");
        for t in &self.snap.torrents {
            let mut item = Item::new(
                crate::ui::theme::Palette::state_icon(t.state),
                &t.name,
                torrent,
                Action::ShowTorrent(t.hash),
            );
            item.needs_query = true;
            v.push(item);
        }
        v
    }

    fn run_palette_action(&mut self, ctx: &Context, action: Action) {
        let hashes = self.selected_hashes();
        match action {
            Action::Pause => {
                self.engine.send(EngineCommand::Pause(hashes));
            }
            Action::Resume => {
                self.engine.send(EngineCommand::Resume(hashes));
            }
            Action::ForceStart => {
                self.engine.send(EngineCommand::ForceStart(hashes));
            }
            Action::Recheck => {
                self.engine.send(EngineCommand::Recheck(hashes));
            }
            Action::Remove => self.request_delete(hashes, false),
            Action::RemoveWithFiles => self.request_delete(hashes, true),
            Action::MoveTo => self.open_move_dialog(hashes),
            Action::SpeedLimits => self.open_speed_limits(hashes),
            Action::SeedLimits => self.open_seed_limits(hashes),
            Action::AddTrackers => self.open_trackers_dialog(hashes),
            Action::Rename => {
                let found = self.selected_views().first().map(|t| (t.hash, t.name.clone()));
                if let Some((h, name)) = found {
                    self.open_rename_dialog(h, name);
                }
            }
            Action::Queue(mv) => {
                self.engine.send(EngineCommand::QueueMove { hashes, mv });
            }
            Action::CopyName => {
                let text: Vec<String> = self.selected_views().iter().map(|t| t.name.clone()).collect();
                self.copy(text.join("\n"));
            }
            Action::CopyHash => {
                let text: Vec<String> = hashes.iter().map(|h| h.to_hex()).collect();
                self.copy(text.join("\n"));
            }
            Action::CopyMagnet => {
                let text: Vec<String> = self.selected_views().iter().map(|t| t.magnet()).collect();
                self.copy(text.join("\n"));
            }
            Action::Invert => self.invert_selection(),
            Action::SelectAll => {
                let snap = self.snap.clone();
                self.main_view = MainView::Torrents;
                self.selected =
                    self.visible_rows().iter().filter_map(|&i| snap.torrents.get(i).map(|t| t.hash)).collect();
            }
            Action::Same(what) => self.select_same(what),
            Action::AddFile => self.pick_torrent_files(),
            Action::AddMagnet => self.open_magnet_dialog(),
            Action::Create => self.open_creator(),
            Action::ToggleSidebar => self.prefs.show_sidebar = !self.prefs.show_sidebar,
            Action::ToggleDetails => self.prefs.show_details = !self.prefs.show_details,
            Action::AltSpeed => {
                self.engine.send(EngineCommand::SetAltSpeed(!self.snap.session.alt_speed));
            }
            Action::PauseAll => {
                self.engine.send(EngineCommand::PauseAll);
            }
            Action::ResumeAll => {
                self.engine.send(EngineCommand::ResumeAll);
            }
            Action::Statistics => self.extras.stats = true,
            Action::Log => self.extras.log = true,
            Action::Streaming => self.extras.streams = true,
            Action::Shortcuts => self.extras.shortcuts = true,
            Action::About => self.extras.about = true,
            Action::TvMode => self.set_tv(true),
            Action::Import => self.open_import(),
            Action::Diagnostics => self.open_diagnostics(),
            Action::View(v) => self.main_view = v,
            Action::Settings(page) => self.open_settings_at(page),
            Action::SavedView(name) => {
                if let Some(sv) = self.prefs.views.iter().find(|v| v.name == name).cloned() {
                    self.apply_view(&sv);
                }
            }
            Action::SaveView => self.modal = Some(super::dialogs::Modal::SaveView { name: String::new() }),
            Action::Status(f) => {
                self.prefs.status_filter = f;
                self.main_view = MainView::Torrents;
            }
            Action::Category(f) => {
                self.prefs.category_filter = f;
                self.main_view = MainView::Torrents;
            }
            Action::Tag(f) => {
                self.prefs.tag_filter = f;
                self.main_view = MainView::Torrents;
            }
            Action::Tracker(f) => {
                self.prefs.tracker_filter = f;
                self.main_view = MainView::Torrents;
            }
            Action::ShowTorrent(h) => self.show_torrents(ctx, vec![h]),
        }
    }

    pub(super) fn palette_window(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut pal) = self.palette.take() else { return };
        // Taken before the text field sees them.
        let step = ROW + GAP;
        let page = (LIST_HEIGHT / step) as isize;
        let (enter, moves) = ctx.input_mut(|i| {
            let mut key = |k| i.consume_key(Modifiers::NONE, k);
            let enter = key(Key::Enter);
            let moves = [
                (key(Key::ArrowDown) || key(Key::Tab), 1),
                (key(Key::ArrowUp), -1),
                (key(Key::PageDown), page),
                (key(Key::PageUp), -page),
            ];
            (enter, moves)
        });
        pal.refresh();
        for (hit, by) in moves {
            if hit {
                pal.select(by);
            }
        }
        let mut run: Option<Action> = None;
        let width = widgets::modal_width(ctx, 600.0);
        let id = Id::new("command_palette");
        let area = egui::Modal::default_area(id).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 70.0));
        let frame = egui::Frame::new()
            .fill(p.bg1)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(14))
            .shadow(ctx.global_style().visuals.window_shadow)
            .inner_margin(egui::Margin::same(10));
        let resp = egui::Modal::new(id).area(area).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut pal.query)
                    .hint_text(format!(
                        "{}  {}",
                        icons::MAGNIFYING_GLASS,
                        t!("Type a command, a setting, a filter or a torrent")
                    ))
                    .frame(egui::Frame::NONE)
                    .font(egui::TextStyle::Heading)
                    .desired_width(f32::INFINITY),
            );
            if !pal.focused {
                pal.focused = true;
                edit.request_focus();
            }
            ui.add_space(6.0);
            ui.separator();
            if pal.shown.is_empty() {
                ui.add_space(14.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t!("Nothing found")).color(p.text_dim));
                });
                ui.add_space(14.0);
                return;
            }
            ui.spacing_mut().item_spacing.y = GAP;
            let height = (ctx.content_rect().height() - 180.0).clamp(120.0, LIST_HEIGHT);
            let mut area =
                egui::ScrollArea::vertical().id_salt("palette_list").max_height(height).auto_shrink([false, true]);
            if pal.moved {
                // Keep the selection in view.
                let top = pal.selected as f32 * step;
                pal.scroll = pal.scroll.min(top).max(top + step - height);
                area = area.vertical_scroll_offset(pal.scroll);
                pal.moved = false;
            }
            let out = area.show_rows(ui, ROW, pal.shown.len(), |ui, range| {
                for pos in range {
                    let Some(item) = pal.shown.get(pos).and_then(|&i| pal.items.get(i)) else { continue };
                    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                    let on = pos == pal.selected;
                    if on || row.hovered() {
                        let fill = if on { p.accent_soft() } else { p.bg3 };
                        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
                    }
                    let at = rect.left_center() + vec2(10.0, 0.0);
                    let color = if on { p.accent } else { p.text_dim };
                    ui.painter().text(
                        at,
                        egui::Align2::LEFT_CENTER,
                        item.icon,
                        egui::FontId::proportional(15.0),
                        color,
                    );
                    let kind = ui.painter().layout_no_wrap(
                        item.kind.to_string(),
                        egui::FontId::proportional(11.0),
                        p.text_mute,
                    );
                    let kind_at = rect.right_center() - vec2(10.0 + kind.size().x, kind.size().y / 2.0);
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        item.label.clone(),
                        egui::FontId::proportional(13.0),
                        p.text,
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: (rect.width() - 56.0 - kind.size().x).max(20.0),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let label = ui.painter().layout_job(job);
                    let label_at = rect.left_center() + vec2(34.0, -label.size().y / 2.0);
                    ui.painter().galley(label_at, label, p.text);
                    ui.painter().galley(kind_at, kind, p.text_mute);
                    row.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, item.label.clone())
                    });
                    if row.clicked() {
                        run = Some(item.action.clone());
                    }
                }
            });
            pal.scroll = out.state.offset.y;
        });
        if enter && run.is_none() {
            run = pal.shown.get(pal.selected).and_then(|&i| pal.items.get(i)).map(|it| it.action.clone());
        }
        let close = resp.should_close();
        if let Some(action) = run {
            self.run_palette_action(ctx, action);
        } else if !close {
            self.palette = Some(pal);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(items: Vec<Item>, q: &str) -> Vec<String> {
        let mut p = CommandPalette { items, query: q.into(), ..Default::default() };
        p.refresh();
        p.shown.iter().map(|&i| p.items[i].label.clone()).collect()
    }

    #[test]
    fn best_matches_first_and_torrents_only_when_typing() {
        let torrent = || {
            let mut t = Item::new("", "Paused.Movie.2010", "Torrent", Action::ShowTorrent(InfoHash([1; 20])));
            t.needs_query = true;
            t
        };
        let items = || {
            vec![
                Item::new("", "Pause all", "Command", Action::PauseAll),
                Item::new("", "Pause", "Selected", Action::Pause),
                Item::new("", "Unpause", "Command", Action::Resume),
                torrent(),
            ]
        };
        assert_eq!(palette(items(), ""), ["Pause all", "Pause", "Unpause"]);
        assert_eq!(palette(items(), "pause"), ["Pause", "Pause all", "Paused.Movie.2010", "Unpause"]);
        assert_eq!(palette(items(), "movie 2010"), ["Paused.Movie.2010"]);
        assert!(palette(items(), "nothing like it").is_empty());
    }

    #[test]
    fn extra_words_find_a_settings_page() {
        let page = Item::new("", "Connection", "Settings", Action::Settings(Page::Connection)).with_words("proxy upnp");
        assert_eq!(palette(vec![page], "proxy"), ["Connection"]);
    }
}
