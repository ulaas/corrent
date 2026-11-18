//! The filter and sort buttons above a library page and Home: toggles, never drop-downs — every
//! choice is on the page and one click away (the web client's Library page has the same row).

use std::collections::HashMap;

use corrent_proto::library::{Facets, ItemFilter, ItemKind, SortBy, Watch};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready_arc};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::toggle_chip;

/// The filters chosen per page (`None`: Home, else a library) and what each can be filtered by.
#[derive(Default)]
pub(super) struct Filters {
    chosen: HashMap<Option<i64>, ItemFilter>,
    facets: HashMap<Option<i64>, Slot<Facets>>,
    /// The genre and year rows are shown.
    more: bool,
}

impl Filters {
    pub(super) fn of(&self, scope: Option<i64>) -> ItemFilter {
        self.chosen.get(&scope).cloned().unwrap_or_default()
    }

    /// Genres and decades are asked for again (after a scan or a metadata refresh).
    pub(super) fn forget_facets(&mut self) {
        self.facets.clear();
    }

    #[cfg(test)]
    pub(crate) fn set(&mut self, scope: Option<i64>, f: ItemFilter) {
        self.chosen.insert(scope, f);
    }
}

/// A filter as the query of `GET /items` and `GET /home` (`&watch=played&genre=…`; empty for
/// no filter). A decade is kept as its first year in `year_from`.
pub(super) fn query(f: &ItemFilter) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    if let Some(w) = f.watch {
        q.append_pair("watch", w.as_str());
    }
    if let Some(g) = &f.genre {
        q.append_pair("genre", g);
    }
    if f.favorite == Some(true) {
        q.append_pair("favorite", "true");
    }
    if let Some(d) = f.year_from {
        q.append_pair("decade", &d.to_string());
    }
    let q = q.finish();
    if q.is_empty() { q } else { format!("&{q}") }
}

/// The next sort when `clicked` is pressed: another order starts in its useful direction
/// (newest first for dates), the current one turns around.
pub(super) fn next_sort((sort, desc): (SortBy, bool), clicked: SortBy) -> (SortBy, bool) {
    if clicked == sort && sort != SortBy::Random {
        (sort, !desc)
    } else {
        (clicked, matches!(clicked, SortBy::Added | SortBy::Played | SortBy::Year))
    }
}

fn sort_name(s: SortBy) -> &'static str {
    match s {
        SortBy::Title => t!("Title"),
        SortBy::Added => t!("Date added"),
        SortBy::Year => t!("Year"),
        SortBy::Played => t!("Last watched"),
        SortBy::Index => t!("Number"),
        SortBy::Random => t!("Shuffle"),
    }
}

impl CorrentApp {
    /// The buttons of page `scope` (`None`: Home; a library's also sort, by `kind` items).
    /// Returns true when a filter or the order changed: the page's list must be fetched again.
    pub(super) fn filter_bar(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        client: &MediaClient,
        scope: Option<i64>,
        kind: Option<ItemKind>,
        count: Option<i64>,
    ) -> bool {
        let before = self.media.filters.of(scope);
        let mut f = before.clone();
        let sort_before = self.media.sort;
        let mut sort = sort_before;
        let mut more = self.media.filters.more;
        let facets = {
            let (c, rt, ctx) = (client.clone(), &self.rt, &self.ctx);
            let slot = self.media.filters.facets.entry(scope).or_insert_with(|| {
                let mut path = "/api/v1/facets?".to_string();
                if let Some(l) = scope {
                    path.push_str(&format!("library_id={l}"));
                }
                if let Some(k) = kind {
                    path.push_str(&format!("&kind={}", k.as_str()));
                }
                fetch(rt, ctx, async move { c.get::<Facets>(&path).await })
            });
            ready_arc(slot).and_then(Result::ok)
        };
        let group_gap = 14.0;
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(20, 0)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(n) = count {
                    let text = if n == 1 { t!("1 item").to_string() } else { t!("{n} items", n = n) };
                    ui.label(RichText::new(text).color(p.text_dim));
                    ui.add_space(group_gap);
                }
                for (w, label) in [
                    (None, t!("All")),
                    (Some(Watch::Unplayed), t!("Unplayed")),
                    (Some(Watch::InProgress), t!("In progress")),
                    (Some(Watch::Played), t!("Played")),
                ] {
                    if toggle_chip(ui, p, label, f.watch == w).clicked() {
                        f.watch = w;
                    }
                }
                ui.add_space(group_gap);
                let fav = f.favorite == Some(true);
                if toggle_chip(ui, p, &format!("{}  {}", icons::HEART, t!("Favorites")), fav).clicked() {
                    f.favorite = (!fav).then_some(true);
                }
                // Genre and year take rows of their own: shown on demand, and while one is chosen.
                let narrowed = f.genre.is_some() || f.year_from.is_some();
                let has_more = facets.as_ref().is_some_and(|x| !x.genres.is_empty() || !x.decades.is_empty());
                if (has_more || narrowed)
                    && toggle_chip(ui, p, &format!("{}  {}", icons::FUNNEL, t!("Genre and year")), more || narrowed)
                        .clicked()
                {
                    more = !(more || narrowed);
                    if !more {
                        f.genre = None;
                        f.year_from = None;
                    }
                }
                if !f.is_empty()
                    && toggle_chip(ui, p, &format!("{}  {}", icons::X, t!("Clear filters")), false).clicked()
                {
                    f = ItemFilter::default();
                    more = false;
                }
                if scope.is_some() {
                    ui.add_space(group_gap);
                    ui.label(RichText::new(t!("Sort")).color(p.text_dim));
                    for s in [SortBy::Title, SortBy::Added, SortBy::Year, SortBy::Played, SortBy::Random] {
                        let on = s == sort.0;
                        let label = if on && s != SortBy::Random {
                            let arrow = if sort.1 { icons::ARROW_DOWN } else { icons::ARROW_UP };
                            format!("{}  {arrow}", sort_name(s))
                        } else {
                            sort_name(s).to_string()
                        };
                        let r = toggle_chip(ui, p, &label, on);
                        let r = if on && s != SortBy::Random { r.on_hover_text(t!("Reverse order")) } else { r };
                        if r.clicked() {
                            sort = next_sort(sort, s);
                        }
                    }
                }
            });
            if let Some(x) = facets.as_ref().filter(|_| more || f.genre.is_some() || f.year_from.is_some()) {
                if !x.genres.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(t!("Genre")).color(p.text_dim));
                        for g in &x.genres {
                            let on = f.genre.as_ref() == Some(g);
                            if toggle_chip(ui, p, g, on).clicked() {
                                f.genre = (!on).then(|| g.clone());
                            }
                        }
                    });
                }
                if !x.decades.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(t!("Year")).color(p.text_dim));
                        for d in &x.decades {
                            let on = f.year_from == Some(*d);
                            if toggle_chip(ui, p, &format!("{d}–{}", d + 9), on).clicked() {
                                f.year_from = (!on).then_some(*d);
                            }
                        }
                    });
                }
            }
        });
        self.media.filters.more = more;
        let changed = f != before || sort != sort_before;
        if f != before {
            self.media.filters.chosen.insert(scope, f);
        }
        self.media.sort = sort;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_becomes_a_query() {
        assert_eq!(query(&ItemFilter::default()), "");
        let f = ItemFilter {
            watch: Some(Watch::InProgress),
            genre: Some("Sci-Fi & Fantasy".into()),
            favorite: Some(true),
            year_from: Some(1990),
            year_to: None,
        };
        assert_eq!(query(&f), "&watch=in_progress&genre=Sci-Fi+%26+Fantasy&favorite=true&decade=1990");
    }

    #[test]
    fn sort_buttons_turn_the_current_order_around() {
        assert_eq!(next_sort((SortBy::Title, false), SortBy::Title), (SortBy::Title, true));
        assert_eq!(next_sort((SortBy::Title, true), SortBy::Added), (SortBy::Added, true), "newest first");
        assert_eq!(next_sort((SortBy::Added, true), SortBy::Title), (SortBy::Title, false));
        assert_eq!(next_sort((SortBy::Random, false), SortBy::Random), (SortBy::Random, false), "shuffled again");
    }
}
