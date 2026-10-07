use std::collections::BTreeMap;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as icons;

use super::table::tracker_host;
use super::{CorrentApp, GroupFilter, StatusFilter};
use crate::engine::EngineCommand;
use crate::engine::types::Snapshot;
use crate::ui::theme::Palette;
use crate::ui::widgets::section_header;

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
fn row(ui: &mut Ui, p: &Palette, icon: &str, icon_color: Color32, label: &str, count: usize, selected: bool) -> egui::Response {
    let h = 28.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
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
    let mut job = egui::text::LayoutJob::simple_singleline(label.to_string(), egui::FontId::proportional(13.0), text_color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(rect.left_center() + vec2(36.0, -galley.size().y / 2.0), galley, text_color);
    if count > 0 || selected {
        let txt = count.to_string();
        let g = ui.painter().layout_no_wrap(txt, egui::FontId::proportional(11.0), if selected { p.accent } else { p.text_dim });
        let pill = egui::Rect::from_center_size(
            rect.right_center() - vec2(10.0 + g.size().x / 2.0 + 6.0, 0.0),
            g.size() + vec2(12.0, 4.0),
        );
        ui.painter().rect_filled(pill, CornerRadius::same(9), if selected { p.accent.gamma_multiply(0.15) } else { p.bg3 });
        ui.painter().galley(pill.center() - g.size() / 2.0, g, p.text_dim);
    }
    resp
}

fn section(ui: &mut Ui, p: &Palette, title: &str, add: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        section_header(ui, p, title);
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
    clicked
}

impl CorrentApp {
    pub(super) fn sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        self.counts.update(&self.snap);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(4.0);
            section(ui, p, "Status", None);
            for (i, f) in StatusFilter::ALL.iter().enumerate() {
                let count = self.counts.status.get(i).copied().unwrap_or(0);
                // Hide rarely-relevant filters when empty to keep the list calm.
                if count == 0 && matches!(f, StatusFilter::Errored | StatusFilter::Stalled) && self.prefs.status_filter != *f {
                    continue;
                }
                let color = match f {
                    StatusFilter::Errored => p.error,
                    StatusFilter::Seeding => p.success,
                    StatusFilter::Downloading => p.accent,
                    _ => p.text_dim,
                };
                if row(ui, p, f.icon(), color, f.label(), count, self.prefs.status_filter == *f).clicked() {
                    self.prefs.status_filter = *f;
                }
            }

            ui.add_space(10.0);
            if section(ui, p, "Categories", Some("New category…")) {
                self.open_new_category(None);
            }
            let total = self.snap.torrents.len();
            if row(ui, p, icons::FOLDERS, p.text_dim, "All", total, self.prefs.category_filter == GroupFilter::All).clicked() {
                self.prefs.category_filter = GroupFilter::All;
            }
            if row(ui, p, icons::FOLDER_DASHED, p.text_dim, "Uncategorized", self.counts.uncategorized, self.prefs.category_filter == GroupFilter::None).clicked() {
                self.prefs.category_filter = GroupFilter::None;
            }
            let cats: Vec<(String, usize)> = self.counts.categories.iter().map(|(k, v)| (k.clone(), *v)).collect();
            for (c, n) in cats {
                let depth = c.matches('/').count();
                let label = c.rsplit('/').next().unwrap_or(&c).to_string();
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    let sel = self.prefs.category_filter == GroupFilter::Named(c.clone());
                    let r = row(ui, p, icons::FOLDER_SIMPLE, p.warn, &label, n, sel);
                    if r.clicked() {
                        self.prefs.category_filter = GroupFilter::Named(c.clone());
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

            ui.add_space(10.0);
            if section(ui, p, "Tags", Some("New tag…")) {
                self.open_new_tag(None);
            }
            if row(ui, p, icons::TAG, p.text_dim, "All", total, self.prefs.tag_filter == GroupFilter::All).clicked() {
                self.prefs.tag_filter = GroupFilter::All;
            }
            if row(ui, p, icons::TAG_SIMPLE, p.text_dim, "Untagged", self.counts.untagged, self.prefs.tag_filter == GroupFilter::None).clicked() {
                self.prefs.tag_filter = GroupFilter::None;
            }
            let tags: Vec<(String, usize)> = self.counts.tags.iter().map(|(k, v)| (k.clone(), *v)).collect();
            for (tag, n) in tags {
                let sel = self.prefs.tag_filter == GroupFilter::Named(tag.clone());
                let r = row(ui, p, icons::HASH, p.queued, &tag, n, sel);
                if r.clicked() {
                    self.prefs.tag_filter = GroupFilter::Named(tag.clone());
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

            if !self.counts.trackers.is_empty() {
                ui.add_space(10.0);
                section(ui, p, "Trackers", None);
                if row(ui, p, icons::GLOBE, p.text_dim, "All", total, self.prefs.tracker_filter == GroupFilter::All).clicked() {
                    self.prefs.tracker_filter = GroupFilter::All;
                }
                if self.counts.trackerless > 0
                    && row(ui, p, icons::GLOBE_X, p.text_dim, "Trackerless", self.counts.trackerless, self.prefs.tracker_filter == GroupFilter::None).clicked()
                {
                    self.prefs.tracker_filter = GroupFilter::None;
                }
                let trackers: Vec<(String, usize)> = self.counts.trackers.iter().map(|(k, v)| (k.clone(), *v)).collect();
                for (host, n) in trackers {
                    let sel = self.prefs.tracker_filter == GroupFilter::Named(host.clone());
                    if row(ui, p, icons::BROADCAST, p.text_dim, &host, n, sel).clicked() {
                        self.prefs.tracker_filter = GroupFilter::Named(host);
                    }
                }
            }
            ui.add_space(12.0);
        });
    }
}
