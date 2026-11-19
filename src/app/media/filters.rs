//! The filter and sort buttons above a library page and Home: toggles, never drop-downs — every
//! choice is on the page and one click away (the web client's Library page has the same row).
//! What is chosen is part of the saved preferences (`UiPrefs::media_views`): it is there again at
//! the next launch.

use std::collections::HashMap;

use corrent_proto::library::{Facets, ItemFilter, ItemKind, SortBy, Watch};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready_arc};
use crate::app::{CorrentApp, MediaView};
use crate::ui::theme::Palette;
use crate::ui::widgets::toggle_chip;

/// What each page (`None`: Home, else a library) can be filtered by.
#[derive(Default)]
pub(super) struct Filters {
    facets: HashMap<Option<i64>, Slot<Facets>>,
    /// The genre and year rows are shown.
    more: bool,
}

impl Filters {
    /// Genres and decades are asked for again (after a scan or a metadata refresh).
    pub(super) fn forget_facets(&mut self) {
        self.facets.clear();
    }
}

/// The view of page `scope` on `server` among the saved ones (the default one isn't kept).
fn view_of(views: &[MediaView], server: &str, scope: Option<i64>) -> MediaView {
    views.iter().find(|v| v.server == server && v.library == scope).cloned().unwrap_or_default()
}

fn set_view(views: &mut Vec<MediaView>, view: MediaView) {
    views.retain(|v| !(v.server == view.server && v.library == view.library));
    let unchanged = MediaView { server: view.server.clone(), library: view.library, ..Default::default() };
    if view != unchanged {
        views.push(view);
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
    /// Library ids are a server's own: the views are kept per server ("" is this computer).
    fn view_server(&self) -> String {
        self.server.as_ref().map(|s| s.entry.id.clone()).unwrap_or_default()
    }

    /// What page `scope` is filtered by.
    pub(crate) fn media_filter(&self, scope: Option<i64>) -> ItemFilter {
        view_of(&self.prefs.media_views, &self.view_server(), scope).filter
    }

    /// How library `id` is sorted (and reversed).
    pub(super) fn media_sort(&self, id: i64) -> (SortBy, bool) {
        let v = view_of(&self.prefs.media_views, &self.view_server(), Some(id));
        (v.sort, v.desc)
    }

    /// Tests: a page's filter as if chosen with its buttons.
    #[cfg(test)]
    pub(crate) fn set_media_filter(&mut self, scope: Option<i64>, filter: ItemFilter) {
        let server = self.view_server();
        let view = MediaView { filter, ..view_of(&self.prefs.media_views, &server, scope) };
        set_view(&mut self.prefs.media_views, MediaView { server, library: scope, ..view });
    }

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
        let server = self.view_server();
        let view = view_of(&self.prefs.media_views, &server, scope);
        let before = view.filter;
        let mut f = before.clone();
        let sort_before = (view.sort, view.desc);
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
        if changed {
            let view = MediaView { server, library: scope, filter: f, sort: sort.0, desc: sort.1 };
            set_view(&mut self.prefs.media_views, view);
        }
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
    fn views_are_kept_per_server_and_page_and_survive_saving() {
        let mut views = Vec::new();
        let played = ItemFilter { watch: Some(Watch::Played), ..Default::default() };
        set_view(&mut views, MediaView { library: Some(2), filter: played.clone(), ..Default::default() });
        let nas =
            MediaView { server: "nas".into(), library: Some(2), sort: SortBy::Added, desc: true, ..Default::default() };
        set_view(&mut views, nas);
        let favorites = ItemFilter { favorite: Some(true), ..Default::default() };
        set_view(&mut views, MediaView { filter: favorites, ..Default::default() });
        // As eframe stores the preferences.
        let prefs = crate::app::UiPrefs { media_views: views, ..Default::default() };
        let back: crate::app::UiPrefs = serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        let mut views = back.media_views;
        assert_eq!(view_of(&views, "", Some(2)).filter, played);
        assert_eq!(view_of(&views, "", Some(2)).sort, SortBy::Title);
        let nas = view_of(&views, "nas", Some(2));
        assert_eq!((nas.sort, nas.desc, nas.filter.is_empty()), (SortBy::Added, true, true));
        assert_eq!(view_of(&views, "", None).filter.favorite, Some(true));
        assert!(view_of(&views, "", Some(3)).filter.is_empty());
        // Back to the defaults: nothing is kept for the page.
        set_view(&mut views, MediaView { library: Some(2), ..Default::default() });
        assert_eq!(views.len(), 2);
    }

    #[test]
    fn sort_buttons_turn_the_current_order_around() {
        assert_eq!(next_sort((SortBy::Title, false), SortBy::Title), (SortBy::Title, true));
        assert_eq!(next_sort((SortBy::Title, true), SortBy::Added), (SortBy::Added, true), "newest first");
        assert_eq!(next_sort((SortBy::Added, true), SortBy::Title), (SortBy::Title, false));
        assert_eq!(next_sort((SortBy::Random, false), SortBy::Random), (SortBy::Random, false), "shuffled again");
    }
}
