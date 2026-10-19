//! Search view: queries the user's Torznab providers (Jackett, Prowlarr, …) in parallel.

use std::sync::mpsc;

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::types::AddSource;
use crate::search::SearchResult;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{empty_compact, empty_state, primary_button};
use crate::util::{fmt_bytes, fmt_date};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    Title,
    Size,
    Seeders,
    Published,
}

type Reply = (u64, String, Result<Vec<SearchResult>, String>);

pub struct SearchUi {
    pub query: String,
    pub filter: String,
    pub results: Vec<SearchResult>,
    /// (provider, error) for providers that failed.
    pub errors: Vec<(String, String)>,
    pub pending: usize,
    /// Answers from older searches are ignored.
    generation: u64,
    pub searched: bool,
    pub sort: SortBy,
    pub asc: bool,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    client: Option<(Option<String>, reqwest::Client)>,
    pub focus: bool,
}

impl Default for SearchUi {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        SearchUi {
            query: String::new(),
            filter: String::new(),
            results: Vec::new(),
            errors: Vec::new(),
            pending: 0,
            generation: 0,
            searched: false,
            sort: SortBy::Seeders,
            asc: false,
            tx,
            rx,
            client: None,
            focus: true,
        }
    }
}

/// Merge results for the same torrent from several providers, keeping the best-seeded one.
fn dedupe(results: &mut Vec<SearchResult>) {
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    let mut out: Vec<SearchResult> = Vec::with_capacity(results.len());
    for r in results.drain(..) {
        let key = r.info_hash.clone().unwrap_or_else(|| format!("{}|{}", r.title.to_lowercase(), r.size.unwrap_or(0)));
        match seen.get(&key) {
            Some(&i) if out[i].seeders.unwrap_or(0) >= r.seeders.unwrap_or(0) => {}
            Some(&i) => out[i] = r,
            None => {
                seen.insert(key, out.len());
                out.push(r);
            }
        }
    }
    *results = out;
}

fn sort(results: &mut [SearchResult], by: SortBy, asc: bool) {
    results.sort_by(|a, b| {
        let o = match by {
            SortBy::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortBy::Size => a.size.cmp(&b.size),
            SortBy::Seeders => a.seeders.cmp(&b.seeders),
            SortBy::Published => a.published.cmp(&b.published),
        };
        if asc { o } else { o.reverse() }
    });
}

impl CorrentApp {
    fn start_search(&mut self) {
        let q = self.torznab.query.trim().to_string();
        let providers: Vec<_> = self.settings.search.providers.iter().filter(|p| p.enabled).cloned().collect();
        if q.is_empty() || providers.is_empty() {
            return;
        }
        let proxy = self.settings.connection.proxy_url();
        let s = &mut self.torznab;
        if s.client.as_ref().is_none_or(|(p, _)| *p != proxy) {
            s.client = Some((proxy.clone(), crate::rss::feed::client(proxy)));
        }
        let client = s.client.as_ref().map(|(_, c)| c.clone()).expect("client was just set");
        s.generation += 1;
        s.results.clear();
        s.errors.clear();
        s.pending = providers.len();
        s.searched = true;
        for p in providers {
            let (tx, client, q, generation) = (s.tx.clone(), client.clone(), q.clone(), s.generation);
            let ctx = self.ctx.clone();
            self.rt.spawn(async move {
                let name = if p.name.trim().is_empty() { p.url.clone() } else { p.name.clone() };
                let r = crate::search::search(&client, &p, &q).await;
                let _ = tx.send((generation, name, r));
                ctx.request_repaint();
            });
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn dev_start_search(&mut self) {
        self.start_search();
    }

    fn poll_search(&mut self) {
        let s = &mut self.torznab;
        let mut changed = false;
        while let Ok((generation, name, r)) = s.rx.try_recv() {
            if generation != s.generation {
                continue;
            }
            s.pending = s.pending.saturating_sub(1);
            match r {
                Ok(mut items) => {
                    s.results.append(&mut items);
                    changed = true;
                }
                Err(e) => s.errors.push((name, e)),
            }
        }
        if changed {
            dedupe(&mut s.results);
            sort(&mut s.results, s.sort, s.asc);
        }
    }

    pub(super) fn search_view(&mut self, ui: &mut Ui, p: &Palette) {
        self.poll_search();
        let configured = self.settings.search.providers.iter().filter(|p| p.enabled).count();
        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 8 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::MAGNIFYING_GLASS).size(20.0).color(p.accent));
                ui.label(RichText::new(t!("Search")).font(theme::title()).color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button(format!("{}  {}", icons::GEAR_SIX, t!("Providers…")))
                        .on_hover_text(t!("Configure Torznab providers (Jackett, Prowlarr, …)"))
                        .clicked()
                    {
                        self.open_settings_at(super::settings_window::Page::Search);
                    }
                    ui.label(
                        RichText::new(if configured == 1 {
                            t!("1 provider").to_string()
                        } else {
                            t!("{n} providers", n = configured)
                        })
                        .small()
                        .color(p.text_dim),
                    );
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let edit = ui.add_enabled(
                    configured > 0,
                    egui::TextEdit::singleline(&mut self.torznab.query)
                        .desired_width((ui.available_width() - 110.0).max(200.0))
                        .hint_text(t!("Search your providers, e.g. a Linux distribution or an open movie")),
                );
                if self.torznab.focus && configured > 0 {
                    edit.request_focus();
                    self.torznab.focus = false;
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let ok = configured > 0 && !self.torznab.query.trim().is_empty();
                if ui
                    .add_enabled_ui(ok, |ui| {
                        primary_button(ui, p, &format!("{}  {}", icons::MAGNIFYING_GLASS, t!("Search")))
                    })
                    .inner
                    .clicked()
                    || (enter && ok)
                {
                    self.start_search();
                }
            });
        });
        ui.separator();

        if configured == 0 {
            empty_state(
                ui,
                p,
                icons::MAGNIFYING_GLASS,
                t!("No search providers"),
                t!("Corrent searches through Torznab providers you run or trust, such as Jackett or Prowlarr."),
            );
            ui.vertical_centered(|ui| {
                if primary_button(ui, p, &format!("{}  {}", icons::PLUS, t!("Add a provider"))).clicked() {
                    self.open_settings_at(super::settings_window::Page::Search);
                }
            });
            return;
        }

        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 4, bottom: 4 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let s = &self.torznab;
                if s.pending > 0 {
                    ui.add(egui::Spinner::new().color(p.accent));
                    ui.label(
                        RichText::new(if s.pending == 1 {
                            t!("Searching 1 provider…").to_string()
                        } else {
                            t!("Searching {n} providers…", n = s.pending)
                        })
                        .color(p.text_dim),
                    );
                } else if s.searched {
                    ui.label(
                        RichText::new(if s.results.len() == 1 {
                            t!("1 result").to_string()
                        } else {
                            t!("{n} results", n = s.results.len())
                        })
                        .color(p.text_dim),
                    );
                }
                for (name, e) in &s.errors {
                    ui.label(RichText::new(format!("{}  {name}", icons::WARNING)).small().color(p.error))
                        .on_hover_text(e);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.torznab.filter).desired_width(200.0).hint_text(format!(
                            "{}  {}",
                            icons::FUNNEL,
                            t!("Filter results")
                        )),
                    );
                });
            });
        });

        let f = self.torznab.filter.trim().to_lowercase();
        let rows: Vec<usize> = (0..self.torznab.results.len())
            .filter(|&i| f.is_empty() || self.torznab.results[i].title.to_lowercase().contains(&f))
            .collect();
        if rows.is_empty() {
            if self.torznab.searched && self.torznab.pending == 0 {
                let hint = if self.torznab.errors.is_empty() {
                    t!("Try other words.")
                } else {
                    t!("Some providers failed; hover the warnings for details.")
                };
                empty_compact(ui, p, icons::MAGNIFYING_GLASS, t!("Nothing found"), hint);
            } else if !self.torznab.searched {
                empty_compact(
                    ui,
                    p,
                    icons::MAGNIFYING_GLASS,
                    t!("Type something and press Enter"),
                    t!("Results from all providers appear here."),
                );
            }
            return;
        }
        let multi = self.settings.search.providers.iter().filter(|p| p.enabled).count() > 1;
        let mut add: Option<String> = None;
        let mut sort_clicked = None;
        egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 0, bottom: 0 }).show(ui, |ui| {
            let mut tb = TableBuilder::new(ui)
                .id_salt("search_results")
                .striped(self.settings.appearance.striped_rows)
                .resizable(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .auto_shrink([false, false])
                .column(Column::remainder().at_least(220.0).clip(true));
            if multi {
                tb = tb.column(Column::initial(120.0).clip(true));
            }
            tb.column(Column::initial(90.0).clip(true))
                .column(Column::initial(80.0))
                .column(Column::initial(70.0))
                .column(Column::initial(120.0))
                .column(Column::exact(112.0))
                .header(24.0, |mut h| {
                    let (sort, asc) = (self.torznab.sort, self.torznab.asc);
                    let mut head = |h: &mut egui_extras::TableRow, t: &str, by: Option<SortBy>| {
                        let (_, r) = h.col(|ui| {
                            let arrow = match by {
                                Some(b) if b == sort => {
                                    if asc {
                                        icons::CARET_UP
                                    } else {
                                        icons::CARET_DOWN
                                    }
                                }
                                _ => "",
                            };
                            ui.label(RichText::new(format!("{t} {arrow}")).small().strong().color(p.text_dim));
                        });
                        if let Some(b) = by
                            && r.interact(Sense::click()).clicked()
                        {
                            sort_clicked = Some(b);
                        }
                    };
                    head(&mut h, t!("Name"), Some(SortBy::Title));
                    if multi {
                        head(&mut h, t!("Provider"), None);
                    }
                    head(&mut h, t!("Category"), None);
                    head(&mut h, t!("Size"), Some(SortBy::Size));
                    head(&mut h, t!("Seeds"), Some(SortBy::Seeders));
                    head(&mut h, t!("Published"), Some(SortBy::Published));
                    head(&mut h, "", None);
                })
                .body(|body| {
                    body.rows(30.0, rows.len(), |mut row| {
                        let r = &self.torznab.results[rows[row.index()]];
                        row.col(|ui| {
                            let resp = ui.add(egui::Label::new(RichText::new(&r.title).color(p.text)).truncate());
                            if let Some(page) = &r.page {
                                crate::ui::widgets::context_menu(&resp, |ui| {
                                    if ui.button(format!("{}  {}", icons::GLOBE, t!("Open details page"))).clicked() {
                                        let _ = open::that_detached(page);
                                        ui.close();
                                    }
                                });
                            }
                        });
                        if multi {
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(&r.provider).small().color(p.text_dim)).truncate(),
                                );
                            });
                        }
                        row.col(|ui| {
                            if let Some(c) = &r.category {
                                ui.add(egui::Label::new(RichText::new(c).small().color(p.text_dim)).truncate());
                            }
                        });
                        row.col(|ui| {
                            if let Some(s) = r.size {
                                ui.label(RichText::new(fmt_bytes(s)).small().color(p.text_dim));
                            }
                        });
                        row.col(|ui| {
                            if let Some(s) = r.seeders {
                                let c = if s == 0 {
                                    p.error
                                } else if s < 5 {
                                    p.warn
                                } else {
                                    p.success
                                };
                                ui.label(RichText::new(s.to_string()).small().color(c)).on_hover_text(t!(
                                    "{s} seeders, {p} peers",
                                    s = s,
                                    p = r.peers.unwrap_or(0)
                                ));
                            }
                        });
                        row.col(|ui| {
                            if let Some(t) = r.published {
                                ui.label(RichText::new(fmt_date(t)).small().color(p.text_dim));
                            }
                        });
                        row.col(|ui| match &r.link {
                            Some(link) => {
                                if ui.small_button(format!("{}  {}", icons::DOWNLOAD_SIMPLE, t!("Download"))).clicked()
                                {
                                    add = Some(link.clone());
                                }
                            }
                            None => {
                                ui.label(RichText::new(t!("No link")).small().color(p.text_mute));
                            }
                        });
                    });
                });
        });
        if let Some(b) = sort_clicked {
            let s = &mut self.torznab;
            if s.sort == b {
                s.asc = !s.asc;
            } else {
                s.sort = b;
                s.asc = b == SortBy::Title;
            }
            sort(&mut s.results, s.sort, s.asc);
        }
        if let Some(link) = add {
            let src = if link.starts_with("magnet:") { AddSource::Magnet(link) } else { AddSource::Url(link) };
            self.launch_queue.push_back(src);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_by_hash_keeping_best_seeded() {
        let r = |p: &str, h: Option<&str>, s: u32| SearchResult {
            provider: p.into(),
            title: "X".into(),
            info_hash: h.map(String::from),
            seeders: Some(s),
            ..Default::default()
        };
        let mut v = vec![r("a", Some("h"), 3), r("b", Some("h"), 9), r("c", None, 1), r("d", Some("g"), 2)];
        dedupe(&mut v);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].provider, "b");
        sort(&mut v, SortBy::Seeders, false);
        assert_eq!(v.iter().map(|x| x.seeders.unwrap()).collect::<Vec<_>>(), vec![9, 2, 1]);
    }
}
