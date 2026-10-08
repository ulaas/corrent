//! Feeds view (RSS/Atom): feed list, articles, and the download-rules editor with a live
//! match preview.

use crate::ui::widgets::Labeled;
use egui::{Align, Context, Id, Layout, RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::engine::types::AddSource;
use crate::engine::{EngineCommand, ToastKind};
use crate::rss::{FeedConfig, FeedView, Matcher, RssRule, rule_applies_to, rule_is_active};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{empty_compact, empty_state, primary_button, secondary_button};
use crate::util::{fmt_bytes, fmt_date};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum MainView {
    #[default]
    Torrents,
    Feeds,
    Search,
}

#[derive(Default)]
pub struct FeedsUi {
    /// Selected feed URL; None = all feeds.
    pub selected: Option<String>,
    pub new_url: String,
    pub search: String,
    pub rules: Option<RulesEditor>,
}

pub struct RulesEditor {
    pub rules: Vec<RssRule>,
    pub selected: usize,
}

fn valid_feed_url(u: &str) -> bool {
    url::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
}

impl CorrentApp {
    /// Change the feed/rule settings and hand them to the engine.
    fn update_rss(&mut self, f: impl FnOnce(&mut crate::rss::RssSettings)) {
        let mut s = self.settings.clone();
        f(&mut s.rss);
        if s == self.settings {
            return;
        }
        self.settings = s.clone();
        self.save_settings();
        self.engine.send(EngineCommand::ApplySettings(Box::new(s)));
    }

    fn add_feed(&mut self) {
        let url = self.feeds.new_url.trim().to_string();
        if !valid_feed_url(&url) {
            self.toasts.push(ToastKind::Error, t!("That doesn't look like a feed address (http:// or https://)."));
            return;
        }
        if self.settings.rss.feeds.iter().any(|f| f.url == url) {
            self.toasts.push(ToastKind::Info, t!("That feed is already in your list."));
            return;
        }
        self.update_rss(|r| r.feeds.push(FeedConfig { url: url.clone(), ..Default::default() }));
        self.feeds.new_url.clear();
        self.feeds.selected = Some(url);
    }

    pub(super) fn feeds_view(&mut self, ui: &mut Ui, p: &Palette) {
        let rss = self.snap.rss.clone();
        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 8 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::RSS).size(20.0).color(p.accent));
                ui.label(RichText::new(t!("Feeds")).font(theme::title()).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let n = self.settings.rss.rules.iter().filter(|r| rule_is_active(r)).count();
                    if primary_button(ui, p, &format!("{}  Download rules ({n})", icons::FUNNEL)).clicked() {
                        self.feeds.rules = Some(RulesEditor { rules: self.settings.rss.rules.clone(), selected: 0 });
                    }
                    let loading = rss.feeds.iter().any(|f| f.loading);
                    if ui
                        .add_enabled(
                            !loading && !rss.feeds.is_empty(),
                            egui::Button::new(format!("{}  {}", icons::ARROWS_CLOCKWISE, t!("Refresh all"))),
                        )
                        .clicked()
                    {
                        self.engine.send(EngineCommand::RssRefresh(None));
                    }
                    if loading {
                        ui.add(egui::Spinner::new().color(p.accent));
                    }
                    let mut auto = self.settings.rss.auto_download;
                    if ui
                        .checkbox(&mut auto, t!("Auto-download"))
                        .on_hover_text(t!("Add matching articles automatically using your download rules"))
                        .changed()
                    {
                        self.update_rss(|r| r.auto_download = auto);
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.feeds.new_url)
                        .desired_width((ui.available_width() - 120.0).max(200.0))
                        .hint_text(t!(
                            "Paste a feed address (RSS or Atom), e.g. a tracker's or a distro's release feed"
                        )),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ok = !self.feeds.new_url.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new(format!("{}  {}", icons::PLUS, t!("Add feed")))).clicked()
                    || (enter && ok)
                {
                    self.add_feed();
                }
            });
        });
        ui.separator();

        if rss.feeds.is_empty() {
            empty_state(
                ui,
                p,
                icons::RSS,
                t!("No feeds yet"),
                t!("Add a feed above. Rules can then download new items automatically — e.g. every new episode."),
            );
            return;
        }
        if let Some(sel) = &self.feeds.selected
            && !rss.feeds.iter().any(|f| &f.url == sel)
        {
            self.feeds.selected = None;
        }

        let avail = ui.available_rect_before_wrap();
        let list_w = 250.0_f32.min(avail.width() * 0.35);
        let list_rect = egui::Rect::from_min_size(avail.min, vec2(list_w, avail.height()));
        let body_rect = egui::Rect::from_min_max(egui::pos2(avail.min.x + list_w + 1.0, avail.min.y), avail.max);
        ui.painter().vline(list_rect.right(), list_rect.y_range(), egui::Stroke::new(1.0, p.border));
        ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect.shrink2(vec2(8.0, 6.0))), |ui| {
            self.feed_list(ui, p, &rss.feeds);
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(body_rect.shrink2(vec2(10.0, 6.0))), |ui| {
            self.article_list(ui, p, &rss.feeds);
        });
        ui.allocate_rect(avail, Sense::hover());
    }

    fn feed_list(&mut self, ui: &mut Ui, p: &Palette, feeds: &[FeedView]) {
        egui::ScrollArea::vertical().id_salt("feed_list").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let total: usize = feeds.iter().map(|f| f.articles.len()).sum();
            if super::sidebar::row(
                ui,
                p,
                icons::STACK,
                p.text_dim,
                t!("All feeds"),
                Some(total),
                self.feeds.selected.is_none(),
            )
            .clicked()
            {
                self.feeds.selected = None;
            }
            ui.add_space(4.0);
            let mut remove = None;
            let mut toggle = None;
            for f in feeds {
                let (icon, color) = if f.error.is_some() {
                    (icons::WARNING_CIRCLE, p.error)
                } else if !f.enabled {
                    (icons::PAUSE_CIRCLE, p.text_mute)
                } else {
                    (icons::RSS_SIMPLE, p.accent)
                };
                let sel = self.feeds.selected.as_deref() == Some(f.url.as_str());
                let resp = super::sidebar::row(ui, p, icon, color, &f.title, Some(f.articles.len()), sel);
                let resp = match &f.error {
                    Some(e) => resp.on_hover_text(format!("{}\n\nLast refresh failed: {e}", f.url)),
                    None => resp.on_hover_text(&f.url),
                };
                if resp.clicked() {
                    self.feeds.selected = Some(f.url.clone());
                }
                resp.context_menu(|ui| {
                    if ui.button(format!("{}  {}", icons::ARROWS_CLOCKWISE, t!("Refresh"))).clicked() {
                        self.engine.send(EngineCommand::RssRefresh(Some(f.url.clone())));
                        ui.close();
                    }
                    if ui.button(format!("{}  {}", icons::LINK, t!("Copy address"))).clicked() {
                        self.copy(f.url.clone());
                        ui.close();
                    }
                    let label = if f.enabled { t!("Pause updates") } else { t!("Resume updates") };
                    if ui.button(format!("{}  {label}", icons::PAUSE)).clicked() {
                        toggle = Some(f.url.clone());
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .button(RichText::new(format!("{}  {}", icons::TRASH, t!("Remove feed"))).color(p.error))
                        .clicked()
                    {
                        remove = Some(f.url.clone());
                        ui.close();
                    }
                });
            }
            if let Some(u) = toggle {
                self.update_rss(|r| {
                    if let Some(f) = r.feeds.iter_mut().find(|f| f.url == u) {
                        f.enabled = !f.enabled;
                    }
                });
            }
            if let Some(u) = remove {
                self.update_rss(|r| {
                    r.feeds.retain(|f| f.url != u);
                    for rule in &mut r.rules {
                        if rule.feeds.contains(&u) {
                            rule.feeds.retain(|f| f != &u);
                            // An empty list means "all feeds": don't widen the rule silently.
                            if rule.feeds.is_empty() {
                                rule.enabled = false;
                            }
                        }
                    }
                });
                self.toasts.push(ToastKind::Info, t!("Feed removed"));
            }
        });
    }

    fn article_list(&mut self, ui: &mut Ui, p: &Palette, feeds: &[FeedView]) {
        let selected = self.feeds.selected.clone();
        let shown: Vec<&FeedView> = feeds.iter().filter(|f| selected.as_ref().is_none_or(|s| s == &f.url)).collect();
        ui.horizontal(|ui| {
            if let [one] = shown.as_slice() {
                ui.label(RichText::new(&one.title).font(theme::heading()).color(p.text));
                let status = match (&one.error, one.last_ok) {
                    (Some(e), _) => RichText::new(format!("{}  {e}", icons::WARNING)).small().color(p.error),
                    (None, Some(t)) => RichText::new(format!("Updated {}", fmt_date(t))).small().color(p.text_dim),
                    (None, None) if one.loading => RichText::new(t!("Loading…")).small().color(p.text_dim),
                    (None, None) => RichText::new(t!("Not loaded yet")).small().color(p.text_dim),
                };
                ui.label(status);
            } else {
                ui.label(RichText::new(t!("All feeds")).font(theme::heading()).color(p.text));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.feeds.search).desired_width(200.0).hint_text(format!(
                    "{}  {}",
                    icons::MAGNIFYING_GLASS,
                    t!("Filter articles")
                )));
            });
        });
        ui.add_space(6.0);

        let q = self.feeds.search.trim().to_lowercase();
        let mut rows: Vec<(&FeedView, &crate::rss::ArticleView)> = shown
            .iter()
            .flat_map(|f| f.articles.iter().map(move |a| (*f, a)))
            .filter(|(_, a)| q.is_empty() || a.article.title.to_lowercase().contains(&q))
            .collect();
        if shown.len() > 1 {
            rows.sort_by_key(|(_, a)| std::cmp::Reverse(a.article.published.unwrap_or(0)));
        }
        if rows.is_empty() {
            let hint = if q.is_empty() {
                t!("Articles appear here after the next refresh.")
            } else {
                t!("Try a different filter.")
            };
            empty_compact(ui, p, icons::NEWSPAPER, t!("No articles"), hint);
            return;
        }

        let matchers: Vec<(String, Matcher, RssRule)> = self
            .settings
            .rss
            .rules
            .iter()
            .filter(|r| rule_is_active(r))
            .filter_map(|r| Matcher::new(r).ok().map(|m| (r.name.clone(), m, r.clone())))
            .collect();
        let mut download: Option<(String, String, String)> = None;
        let multi = shown.len() > 1;
        let mut tb = TableBuilder::new(ui)
            .id_salt("articles")
            .striped(self.settings.appearance.striped_rows)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .auto_shrink([false, false])
            .column(Column::exact(26.0))
            .column(Column::remainder().at_least(200.0).clip(true));
        if multi {
            tb = tb.column(Column::initial(140.0).clip(true));
        }
        tb.column(Column::initial(80.0))
            .column(Column::initial(130.0))
            .column(Column::exact(112.0))
            .header(24.0, |mut h| {
                let head = |h: &mut egui_extras::TableRow, t: &str| {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().strong().color(p.text_dim));
                    });
                };
                head(&mut h, "");
                head(&mut h, t!("Title"));
                if multi {
                    head(&mut h, t!("Feed"));
                }
                head(&mut h, t!("Size"));
                head(&mut h, t!("Published"));
                head(&mut h, "");
            })
            .body(|body| {
                body.rows(30.0, rows.len(), |mut row| {
                    let (f, a) = rows[row.index()];
                    let rule = matchers
                        .iter()
                        .find(|(_, m, r)| rule_applies_to(r, &f.url) && m.matches(&a.article.title))
                        .map(|(n, _, _)| n.as_str());
                    row.col(|ui| {
                        if a.downloaded {
                            ui.label(RichText::new(icons::CHECK_CIRCLE).color(p.success))
                                .on_hover_text(t!("Downloaded"));
                        } else if let Some(r) = rule {
                            ui.label(RichText::new(icons::FUNNEL).color(p.accent))
                                .on_hover_text(t!("Matches rule “{r}”", r = r));
                        }
                    });
                    row.col(|ui| {
                        let color = if a.downloaded { p.text_dim } else { p.text };
                        let resp = ui.add(egui::Label::new(RichText::new(&a.article.title).color(color)).truncate());
                        if let Some(page) = &a.article.page {
                            resp.context_menu(|ui| {
                                if ui.button(format!("{}  {}", icons::GLOBE, t!("Open web page"))).clicked() {
                                    let _ = open::that_detached(page);
                                    ui.close();
                                }
                            });
                        }
                    });
                    if multi {
                        row.col(|ui| {
                            ui.add(egui::Label::new(RichText::new(&f.title).small().color(p.text_dim)).truncate());
                        });
                    }
                    row.col(|ui| {
                        if let Some(s) = a.article.size {
                            ui.label(RichText::new(fmt_bytes(s)).small().color(p.text_dim));
                        }
                    });
                    row.col(|ui| {
                        if let Some(t) = a.article.published {
                            ui.label(RichText::new(fmt_date(t)).small().color(p.text_dim));
                        }
                    });
                    row.col(|ui| match &a.article.torrent {
                        Some(link) => {
                            let label = if a.downloaded { t!("Again") } else { t!("Download") };
                            if ui
                                .small_button(format!("{}  {label}", icons::DOWNLOAD_SIMPLE))
                                .on_hover_text(link)
                                .clicked()
                            {
                                download = Some((link.clone(), f.url.clone(), a.article.guid.clone()));
                            }
                        }
                        None => {
                            ui.label(RichText::new(t!("No link")).small().color(p.text_mute));
                        }
                    });
                });
            });
        if let Some((link, feed, guid)) = download {
            let src = if link.starts_with("magnet:") { AddSource::Magnet(link) } else { AddSource::Url(link) };
            self.launch_queue.push_back(src);
            self.engine.send(EngineCommand::RssMarkDownloaded { feed, guid });
        }
    }

    pub(super) fn rules_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut ed) = self.feeds.rules.take() else { return };
        let rss = self.snap.rss.clone();
        let mut close = false;
        let mut save = false;
        let mut pick = false;
        let categories: Vec<String> = self.snap.categories.keys().cloned().collect();
        let auto = self.settings.rss.auto_download;
        let resp = egui::Modal::new(Id::new("rss_rules")).show(ctx, |ui| {
            ui.set_width(860.0_f32.min(ctx.content_rect().width() - 60.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::FUNNEL).size(20.0).color(p.accent));
                ui.label(RichText::new(t!("Download rules")).font(theme::title()).color(p.text));
            });
            ui.label(
                RichText::new(
                    "New feed articles that match a rule are added automatically. Words must all appear (any order); \
                     | separates alternatives; * and ? are wildcards.",
                )
                .small()
                .color(p.text_dim),
            );
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                // Rule list
                ui.vertical(|ui| {
                    ui.set_width(200.0);
                    egui::ScrollArea::vertical().id_salt("rule_list").max_height(420.0).show(ui, |ui| {
                        for (i, r) in ed.rules.iter().enumerate() {
                            let name = if r.name.trim().is_empty() { t!("Unnamed rule") } else { r.name.as_str() };
                            let (icon, color) = if rule_is_active(r) {
                                (icons::FUNNEL, p.accent)
                            } else {
                                (icons::FUNNEL_SIMPLE_X, p.text_mute)
                            };
                            if super::sidebar::row(ui, p, icon, color, name, None, ed.selected == i).clicked() {
                                ed.selected = i;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(format!("{}  {}", icons::PLUS, t!("New rule"))).clicked() {
                            let n = ed.rules.len() + 1;
                            ed.rules.push(RssRule { name: t!("Rule {n}", n = n), ..Default::default() });
                            ed.selected = ed.rules.len() - 1;
                        }
                        if ui
                            .add_enabled(!ed.rules.is_empty(), egui::Button::new(icons::TRASH))
                            .labeled(t!("Delete rule"))
                            .clicked()
                        {
                            ed.rules.remove(ed.selected.min(ed.rules.len() - 1));
                            ed.selected = ed.selected.saturating_sub(1);
                        }
                    });
                });
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.set_width(620.0);
                    let Some(rule) = ed.rules.get_mut(ed.selected) else {
                        empty_compact(
                            ui,
                            p,
                            icons::FUNNEL,
                            t!("No rules"),
                            t!("Create one to download matching articles."),
                        );
                        return;
                    };
                    rule_form(ui, p, rule, &rss.feeds, &categories, &mut pick);
                    ui.add_space(8.0);
                    rule_preview(ui, p, rule, &rss.feeds, auto);
                });
            });
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let names_ok = {
                    let mut names: Vec<&str> = ed.rules.iter().map(|r| r.name.trim()).collect();
                    let n = names.len();
                    names.sort();
                    names.dedup();
                    names.len() == n && !names.contains(&"")
                };
                let errors = ed.rules.iter().any(|r| Matcher::new(r).is_err());
                if ui.add_enabled_ui(names_ok && !errors, |ui| primary_button(ui, p, t!("Save rules"))).inner.clicked()
                {
                    save = true;
                }
                if secondary_button(ui, t!("Cancel")).clicked() {
                    close = true;
                }
                if !names_ok {
                    ui.label(RichText::new(t!("Each rule needs a unique name")).small().color(p.warn));
                } else if errors {
                    ui.label(RichText::new(t!("Fix the highlighted rule first")).small().color(p.warn));
                }
            });
        });
        if resp.should_close() {
            close = true;
        }
        if pick {
            let start = ed.rules.get(ed.selected).and_then(|r| r.save_path.clone());
            self.pick(PickTarget::RssRuleFolder, PickKind::Folder, start);
        }
        if save {
            let rules: Vec<RssRule> = std::mem::take(&mut ed.rules)
                .into_iter()
                .map(|mut r| {
                    r.name = r.name.trim().to_string();
                    r
                })
                .collect();
            self.update_rss(|s| s.rules = rules);
            self.toasts.push(ToastKind::Success, t!("Download rules saved"));
            return;
        }
        if !close {
            self.feeds.rules = Some(ed);
        }
    }
}

fn rule_form(ui: &mut Ui, p: &Palette, r: &mut RssRule, feeds: &[FeedView], categories: &[String], pick: &mut bool) {
    egui::Grid::new("rule_form").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
        ui.label(t!("Name"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(260.0));
            ui.checkbox(&mut r.enabled, t!("Enabled"));
        });
        ui.end_row();
        ui.label(t!("Must contain"));
        ui.add(
            egui::TextEdit::singleline(&mut r.must_contain)
                .desired_width(420.0)
                .hint_text(t!("e.g. sintel 1080p | big buck bunny")),
        );
        ui.end_row();
        ui.label(t!("Must not contain"));
        ui.add(
            egui::TextEdit::singleline(&mut r.must_not_contain).desired_width(420.0).hint_text(t!("e.g. cam | sample")),
        );
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut r.use_regex, t!("Use regular expressions"));
        ui.end_row();
        ui.label(t!("Episodes"));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut r.episode_filter).desired_width(160.0).hint_text("e.g. 1x1-5;8;10-"),
            );
            ui.checkbox(&mut r.smart_episodes, t!("Each episode only once"))
                .on_hover_text(t!("Skip other releases of an episode this rule already downloaded"));
        });
        ui.end_row();
        ui.label(t!("Feeds"));
        ui.horizontal_wrapped(|ui| {
            let mut all = r.feeds.is_empty();
            if ui.checkbox(&mut all, t!("All feeds")).changed() && all {
                r.feeds.clear();
            }
            for f in feeds {
                let mut on = r.feeds.contains(&f.url);
                if ui.checkbox(&mut on, &f.title).changed() {
                    if on {
                        r.feeds.push(f.url.clone());
                    } else {
                        r.feeds.retain(|u| u != &f.url);
                    }
                }
            }
        });
        ui.end_row();
        ui.label(t!("Category"));
        let current = r.category.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("rule_category")
            .selected_text(if current.is_empty() { t!("None").to_string() } else { current.clone() })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut r.category, None, t!("None"));
                for c in categories {
                    ui.selectable_value(&mut r.category, Some(c.clone()), c);
                }
            });
        ui.end_row();
        ui.label(t!("Save to"));
        ui.horizontal(|ui| {
            let label = r
                .save_path
                .as_ref()
                .map(|s| s.display().to_string())
                .unwrap_or_else(|| t!("Category or default folder").into());
            ui.add(egui::Label::new(RichText::new(label).color(p.text_dim)).truncate());
            if ui.button(icons::FOLDER_OPEN).labeled(t!("Choose folder")).clicked() {
                *pick = true;
            }
            if r.save_path.is_some() && ui.button(icons::X).labeled(t!("Use the default")).clicked() {
                r.save_path = None;
            }
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut r.start_paused, t!("Add paused"));
        ui.end_row();
    });
}

fn rule_preview(ui: &mut Ui, p: &Palette, r: &RssRule, feeds: &[FeedView], auto: bool) {
    let m = match Matcher::new(r) {
        Ok(m) => m,
        Err(e) => {
            ui.label(RichText::new(format!("{}  {e}", icons::WARNING)).color(p.error));
            return;
        }
    };
    if !rule_is_active(r) {
        let why =
            if r.enabled { t!("Add words to match to activate this rule.") } else { t!("This rule is disabled.") };
        ui.label(RichText::new(why).small().color(p.text_dim));
        return;
    }
    let hits: Vec<(&str, bool)> = feeds
        .iter()
        .filter(|f| rule_applies_to(r, &f.url))
        .flat_map(|f| f.articles.iter())
        .filter(|a| a.article.torrent.is_some() && m.matches(&a.article.title))
        .map(|a| (a.article.title.as_str(), a.downloaded))
        .collect();
    let new = hits.iter().filter(|(_, d)| !d).count();
    let text = match (hits.len(), new) {
        (0, _) => t!("Matches no current articles").to_string(),
        (n, 0) => format!("Matches {n} current article{} (all downloaded)", if n == 1 { "" } else { "s" }),
        (n, k) if auto => {
            format!(
                "Matches {n} current article{} — {k} will be downloaded when you save",
                if n == 1 { "" } else { "s" }
            )
        }
        (n, _) => format!("Matches {n} current article{} (auto-download is off)", if n == 1 { "" } else { "s" }),
    };
    ui.label(RichText::new(text).strong().color(if new > 0 { p.accent } else { p.text_dim }));
    egui::ScrollArea::vertical().id_salt("rule_preview").max_height(150.0).show(ui, |ui| {
        for (t, d) in hits.iter().take(100) {
            ui.horizontal(|ui| {
                let (i, c) = if *d { (icons::CHECK_CIRCLE, p.success) } else { (icons::DOWNLOAD_SIMPLE, p.accent) };
                ui.label(RichText::new(i).color(c));
                ui.add(egui::Label::new(RichText::new(*t).small().color(p.text)).truncate());
            });
        }
    });
}
