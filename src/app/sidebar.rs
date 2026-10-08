use std::collections::BTreeMap;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icons;

use super::table::tracker_host;
use super::{CorrentApp, GroupFilter, StatusFilter};
use crate::engine::EngineCommand;
use crate::engine::types::Snapshot;
use crate::ui::theme::Palette;

#[derive(Default)]
pub struct Counts {
    generation: u64,
    pub status: Vec<usize>,
    pub categories: BTreeMap<String, usize>,
    pub uncategorized: usize,
    pub tags: BTreeMap<String, usize>,
    pub untagged: usize,
    pub trackers: BTreeMap<String, usize>,
    pub trackerless: usize,
}

impl Counts {
    pub fn update(&mut self, snap: &Snapshot) {
        if self.generation == snap.generation && !self.status.is_empty() {
            return;
        }
        *self = Counts { generation: snap.generation, ..Default::default() };
        self.status = StatusFilter::ALL.iter().map(|f| snap.torrents.iter().filter(|t| f.matches(t)).count()).collect();
        for c in snap.categories.keys() {
            self.categories.insert(c.clone(), 0);
        }
        for t in &snap.tags {
            self.tags.insert(t.clone(), 0);
        }
        for t in &snap.torrents {
            match &t.category {
                Some(c) => {
                    // Count towards the category and all of its parents ("Movies/HD" → "Movies").
                    let mut path = String::new();
                    for (i, part) in c.split('/').enumerate() {
                        if i > 0 {
                            path.push('/');
                        }
                        path.push_str(part);
                        *self.categories.entry(path.clone()).or_default() += 1;
                    }
                }
                None => self.uncategorized += 1,
            }
            if t.tags.is_empty() {
                self.untagged += 1;
            }
            for tag in &t.tags {
                *self.tags.entry(tag.clone()).or_default() += 1;
            }
            if t.trackers.is_empty() {
                self.trackerless += 1;
            }
            let mut hosts: Vec<String> = t.trackers.iter().map(|u| tracker_host(u)).collect();
            hosts.sort();
            hosts.dedup();
            for h in hosts {
                *self.trackers.entry(h).or_default() += 1;
            }
        }
    }
}

/// One sidebar row: icon, label, count pill; selected rows get an accent bar.
pub(super) fn row(
    ui: &mut Ui,
    p: &Palette,
    icon: &str,
    icon_color: Color32,
    label: &str,
    count: Option<usize>,
    selected: bool,
) -> egui::Response {
    let h = 28.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label));
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hov"), resp.hovered() || selected, 0.12);
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(7), p.accent_soft());
        let bar = egui::Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(3.0, h - 12.0));
        ui.painter().rect_filled(bar, CornerRadius::same(2), p.accent);
    } else if hover > 0.0 {
        ui.painter().rect_filled(rect, CornerRadius::same(7), p.bg3.gamma_multiply(hover));
    }
    let text_color = if selected { p.text } else { p.text.lerp_to_gamma(p.text_dim, 0.35) };
    ui.painter().text(
        rect.left_center() + vec2(14.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(15.0),
        if selected { p.accent } else { icon_color },
    );
    let max_w = rect.width() - 80.0;
    let mut job =
        egui::text::LayoutJob::simple_singleline(label.to_string(), egui::FontId::proportional(13.0), text_color);
    job.wrap = egui::text::TextWrapping {
        max_width: max_w.max(10.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(rect.left_center() + vec2(36.0, -galley.size().y / 2.0), galley, text_color);
    if let Some(count) = count.filter(|c| *c > 0 || selected) {
        let txt = count.to_string();
        let g = ui.painter().layout_no_wrap(
            txt,
            egui::FontId::proportional(11.0),
            if selected { p.accent } else { p.text_dim },
        );
        let pill = egui::Rect::from_center_size(
            rect.right_center() - vec2(10.0 + g.size().x / 2.0 + 6.0, 0.0),
            g.size() + vec2(12.0, 4.0),
        );
        ui.painter().rect_filled(
            pill,
            CornerRadius::same(9),
            if selected { p.accent.gamma_multiply(0.15) } else { p.bg3 },
        );
        ui.painter().galley(pill.center() - g.size() / 2.0, g, p.text_dim);
    }
    resp
}

fn hint(ui: &mut Ui, p: &Palette, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(RichText::new(text).small().color(p.text_mute)).wrap());
    });
    ui.add_space(2.0);
}

/// Section header with a collapse toggle and an optional "+" button. Returns (add clicked).
fn section(ui: &mut Ui, p: &Palette, title: &str, add: Option<&str>, collapsed: &mut bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        let caret = if *collapsed { icons::CARET_RIGHT } else { icons::CARET_DOWN };
        let header = ui.add(
            egui::Label::new(
                RichText::new(format!("{caret} {}", title.to_uppercase())).size(10.5).color(p.text_mute).strong(),
            )
            .sense(Sense::click()),
        );
        if header.on_hover_text(if *collapsed { "Expand" } else { "Collapse" }).clicked() {
            *collapsed = !*collapsed;
        }
        if let Some(tip) = add {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(4.0);
                if ui
                    .add(egui::Button::new(RichText::new(icons::PLUS).size(12.0).color(p.text_dim)).frame(false))
                    .on_hover_text(tip)
                    .clicked()
                {
                    clicked = true;
                }
            });
        }
    });
    ui.add_space(2.0);
    clicked
}

impl CorrentApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        self.counts.update(&self.snap);
        let tv = self.main_view == super::feeds::MainView::Torrents;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let total = self.snap.torrents.len();
            ui.add_space(4.0);
            let mut c = self.prefs.collapsed.contains(&"status".to_string());
            section(ui, p, "Status", None, &mut c);
            set_collapsed(&mut self.prefs.collapsed, "status", c);
            if !c {
                for (i, f) in StatusFilter::ALL.iter().enumerate() {
                    let count = self.counts.status.get(i).copied().unwrap_or(0);
                    // Hide rarely-relevant filters when empty to keep the list calm.
                    if count == 0
                        && matches!(f, StatusFilter::Errored | StatusFilter::Stalled)
                        && self.prefs.status_filter != *f
                    {
                        continue;
                    }
                    let color = match f {
                        StatusFilter::Errored => p.error,
                        StatusFilter::Seeding => p.success,
                        StatusFilter::Downloading => p.accent,
                        _ => p.text_dim,
                    };
                    if row(ui, p, f.icon(), color, f.label(), Some(count), tv && self.prefs.status_filter == *f)
                        .clicked()
                    {
                        self.prefs.status_filter = *f;
                        self.main_view = super::feeds::MainView::Torrents;
                    }
                }
            }
            ui.add_space(10.0);
            let mut c = self.prefs.collapsed.contains(&"categories".to_string());
            if section(ui, p, "Categories", Some("New category…"), &mut c) {
                self.open_new_category(None);
            }
            set_collapsed(&mut self.prefs.collapsed, "categories", c);
            if !c {
                if row(
                    ui,
                    p,
                    icons::FOLDERS,
                    p.text_dim,
                    "All",
                    Some(total),
                    tv && self.prefs.category_filter == GroupFilter::All,
                )
                .clicked()
                {
                    self.prefs.category_filter = GroupFilter::All;
                    self.main_view = super::feeds::MainView::Torrents;
                }
                if row(
                    ui,
                    p,
                    icons::FOLDER_DASHED,
                    p.text_dim,
                    "Uncategorized",
                    Some(self.counts.uncategorized),
                    tv && self.prefs.category_filter == GroupFilter::None,
                )
                .clicked()
                {
                    self.prefs.category_filter = GroupFilter::None;
                    self.main_view = super::feeds::MainView::Torrents;
                }
                let cats: Vec<(String, usize)> = self.counts.categories.iter().map(|(k, v)| (k.clone(), *v)).collect();
                if cats.is_empty() {
                    hint(ui, p, "Categories group torrents and can have their own save folder.");
                }
                for (c, n) in cats {
                    let depth = c.matches('/').count();
                    let label = c.rsplit('/').next().unwrap_or(&c).to_string();
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 14.0);
                        let sel = self.prefs.category_filter == GroupFilter::Named(c.clone());
                        let r = row(ui, p, icons::FOLDER_SIMPLE, p.warn, &label, Some(n), tv && sel);
                        if r.clicked() {
                            self.prefs.category_filter = GroupFilter::Named(c.clone());
                            self.main_view = super::feeds::MainView::Torrents;
                        }
                        r.context_menu(|ui| {
                            if ui.button(format!("{}  Edit…", icons::PENCIL_SIMPLE)).clicked() {
                                self.open_edit_category(c.clone());
                            }
                            if ui.button(format!("{}  Add subcategory…", icons::FOLDER_SIMPLE_PLUS)).clicked() {
                                self.open_new_category(None);
                                if let Some(super::dialogs::Modal::Category { name, .. }) = &mut self.modal {
                                    *name = format!("{c}/");
                                }
                            }
                            if ui.button(format!("{}  Remove category", icons::TRASH)).clicked() {
                                let mut cats = self.snap.categories.clone();
                                cats.retain(|k, _| k != &c && !k.starts_with(&format!("{c}/")));
                                self.engine.send(EngineCommand::SetCategories(cats));
                                if self.prefs.category_filter == GroupFilter::Named(c.clone()) {
                                    self.prefs.category_filter = GroupFilter::All;
                                }
                            }
                        });
                    });
                }
            }
            ui.add_space(10.0);
            let mut c = self.prefs.collapsed.contains(&"tags".to_string());
            if section(ui, p, "Tags", Some("New tag…"), &mut c) {
                self.open_new_tag(None);
            }
            set_collapsed(&mut self.prefs.collapsed, "tags", c);
            if !c {
                if row(
                    ui,
                    p,
                    icons::TAG,
                    p.text_dim,
                    "All",
                    Some(total),
                    tv && self.prefs.tag_filter == GroupFilter::All,
                )
                .clicked()
                {
                    self.prefs.tag_filter = GroupFilter::All;
                    self.main_view = super::feeds::MainView::Torrents;
                }
                if row(
                    ui,
                    p,
                    icons::TAG_SIMPLE,
                    p.text_dim,
                    "Untagged",
                    Some(self.counts.untagged),
                    tv && self.prefs.tag_filter == GroupFilter::None,
                )
                .clicked()
                {
                    self.prefs.tag_filter = GroupFilter::None;
                    self.main_view = super::feeds::MainView::Torrents;
                }
                let tags: Vec<(String, usize)> = self.counts.tags.iter().map(|(k, v)| (k.clone(), *v)).collect();
                if tags.is_empty() {
                    hint(ui, p, "Tags are free-form labels; a torrent can have several.");
                }
                for (tag, n) in tags {
                    let sel = self.prefs.tag_filter == GroupFilter::Named(tag.clone());
                    let r = row(ui, p, icons::HASH, p.queued, &tag, Some(n), tv && sel);
                    if r.clicked() {
                        self.prefs.tag_filter = GroupFilter::Named(tag.clone());
                        self.main_view = super::feeds::MainView::Torrents;
                    }
                    r.context_menu(|ui| {
                        if ui.button(format!("{}  Remove tag", icons::TRASH)).clicked() {
                            let tags: Vec<String> = self.snap.tags.iter().filter(|t| **t != tag).cloned().collect();
                            self.engine.send(EngineCommand::SetTagList(tags));
                            if sel {
                                self.prefs.tag_filter = GroupFilter::All;
                            }
                        }
                    });
                }
            }
            ui.add_space(10.0);
            let mut c = self.prefs.collapsed.contains(&"feeds".to_string());
            section(ui, p, "Discover", None, &mut c);
            set_collapsed(&mut self.prefs.collapsed, "feeds", c);
            if !c {
                let rss = self.snap.rss.clone();
                let errors = rss.feeds.iter().filter(|f| f.error.is_some()).count();
                let (icon, color) = if errors > 0 { (icons::WARNING_CIRCLE, p.error) } else { (icons::RSS, p.accent) };
                let feeds_view = self.main_view == super::feeds::MainView::Feeds;
                let r = row(ui, p, icon, color, "RSS feeds", Some(rss.feeds.len()), feeds_view);
                let r = if errors > 0 {
                    r.on_hover_text(format!(
                        "{errors} feed{} couldn't be refreshed",
                        if errors == 1 { "" } else { "s" }
                    ))
                } else {
                    r.on_hover_text("Follow feeds and download new items automatically")
                };
                if r.clicked() {
                    self.main_view = super::feeds::MainView::Feeds;
                }
                let search_view = self.main_view == super::feeds::MainView::Search;
                let n = self.torznab.results.len();
                if row(ui, p, icons::MAGNIFYING_GLASS, p.text_dim, "Search", Some(n), search_view)
                    .on_hover_text("Search your Torznab providers (Jackett, Prowlarr, …)")
                    .clicked()
                {
                    self.main_view = super::feeds::MainView::Search;
                    self.torznab.focus = true;
                }
            }
            ui.add_space(12.0);
        });
    }
}

fn set_collapsed(list: &mut Vec<String>, key: &str, collapsed: bool) {
    list.retain(|k| k != key);
    if collapsed {
        list.push(key.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{InfoHash, TorrentView};

    #[test]
    fn counts_include_parent_categories_and_untagged() {
        let mut a = TorrentView { hash: InfoHash([1; 20]), category: Some("Movies/HD".into()), ..Default::default() };
        a.tags = vec!["x".into()];
        let b = TorrentView { hash: InfoHash([2; 20]), ..Default::default() };
        let snap = Snapshot { torrents: vec![a, b], generation: 1, tags: vec!["unused".into()], ..Default::default() };
        let mut c = Counts::default();
        c.update(&snap);
        assert_eq!(c.categories.get("Movies"), Some(&1));
        assert_eq!(c.categories.get("Movies/HD"), Some(&1));
        assert_eq!(c.uncategorized, 1);
        assert_eq!(c.untagged, 1);
        assert_eq!(c.tags.get("unused"), Some(&0), "known tags are listed even when empty");
        assert_eq!(c.status[0], 2, "All");
    }
}
