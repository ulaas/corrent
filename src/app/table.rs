use std::cmp::Ordering;
use std::time::Duration;

use egui::{Align, Color32, Id, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;

use super::{Col, CorrentApp, GroupFilter, UiPrefs};
use crate::engine::types::{InfoHash, Snapshot, TorrentState, TorrentView};
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, Labeled, badge, chip};
use crate::util::{fmt_bytes, fmt_date, fmt_duration, fmt_ratio, fmt_speed};

/// The id of the column chooser popup.
pub(super) const COLUMN_CHOOSER: &str = "column_chooser";

pub fn tracker_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| url.to_string())
}

/// Announce URL → host, remembered: with thousands of torrents, parsing each tracker URL on
/// every engine tick is real work on the UI thread.
#[derive(Default)]
pub struct TrackerHosts(std::collections::HashMap<String, String>);

impl TrackerHosts {
    pub fn get(&mut self, url: &str) -> &str {
        if !self.0.contains_key(url) {
            // A session doesn't see this many; the bound is for a hostile or broken list.
            if self.0.len() >= 200_000 {
                self.0.clear();
            }
            self.0.insert(url.to_string(), tracker_host(url));
        }
        self.0.get(url).map_or("", String::as_str)
    }

    /// The host of a URL `get` has seen; the URL itself otherwise.
    pub fn peek<'a>(&'a self, url: &'a str) -> &'a str {
        self.0.get(url).map_or(url, String::as_str)
    }
}

/// `term` (lower case) somewhere in `text`, ignoring case. Names are nearly always ASCII, and
/// this runs for every torrent on every engine tick while a search is typed.
fn contains_lower(text: &str, term: &str) -> bool {
    if text.is_ascii() && term.is_ascii() {
        let (t, n) = (text.as_bytes(), term.as_bytes());
        n.is_empty() || t.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
    } else {
        text.to_lowercase().contains(term)
    }
}

fn eq_lower(text: &str, term: &str) -> bool {
    if text.is_ascii() { text.eq_ignore_ascii_case(term) } else { text.to_lowercase() == term }
}

/// The list's filters, with what they need worked out once per pass over the torrents.
pub struct RowFilter<'a> {
    prefs: &'a UiPrefs,
    /// Lower case.
    terms: Vec<String>,
    /// "Movies/" for the category "Movies": its subcategories count too.
    category_prefix: Option<String>,
}

impl<'a> RowFilter<'a> {
    pub fn new(prefs: &'a UiPrefs, search: &str) -> RowFilter<'a> {
        let terms = search.trim().to_lowercase().split_whitespace().map(String::from).collect();
        let category_prefix = match &prefs.category_filter {
            GroupFilter::Named(c) => Some(format!("{c}/")),
            _ => None,
        };
        RowFilter { prefs, terms, category_prefix }
    }

    pub fn matches(&self, t: &TorrentView, hosts: &mut TrackerHosts) -> bool {
        let prefs = self.prefs;
        if !prefs.status_filter.matches(t) {
            return false;
        }
        match &prefs.category_filter {
            GroupFilter::All => {}
            GroupFilter::None => {
                if t.category.is_some() {
                    return false;
                }
            }
            GroupFilter::Named(c) => {
                // Subcategories ("Movies/HD") are included when filtering by a parent.
                let prefix = self.category_prefix.as_deref().unwrap_or_default();
                match &t.category {
                    Some(tc) if tc == c || tc.starts_with(prefix) => {}
                    _ => return false,
                }
            }
        }
        match &prefs.tag_filter {
            GroupFilter::All => {}
            GroupFilter::None => {
                if !t.tags.is_empty() {
                    return false;
                }
            }
            GroupFilter::Named(tag) => {
                if !t.tags.contains(tag) {
                    return false;
                }
            }
        }
        match &prefs.tracker_filter {
            GroupFilter::All => {}
            GroupFilter::None => {
                if !t.trackers.is_empty() {
                    return false;
                }
            }
            GroupFilter::Named(host) => {
                if !t.trackers.iter().any(|u| hosts.get(u) == host) {
                    return false;
                }
            }
        }
        self.terms.iter().all(|term| {
            contains_lower(&t.name, term)
                || (term.bytes().all(|b| b.is_ascii_hexdigit()) && t.hash.to_hex().starts_with(term.as_str()))
                || t.tags.iter().any(|tag| eq_lower(tag, term))
                || t.category.as_ref().is_some_and(|c| eq_lower(c, term))
        })
    }
}

/// The last sort of the list. Most engine ticks only change speeds and progress: with a list
/// sorted by something else the order is reused (with 5,000 torrents, a sort compares names
/// about 60,000 times per tick).
#[derive(Default)]
pub struct SortMemo {
    fingerprint: u64,
    sorted: Vec<usize>,
    /// How many times the list was really sorted.
    pub sorts: u64,
}

/// What the order of the rows depends on, for the columns whose values rarely change.
fn sort_fingerprint(rows: &[usize], snap: &Snapshot, prefs: &UiPrefs, hosts: &TrackerHosts) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (prefs.sort, prefs.sort_asc).hash(&mut h);
    for t in rows.iter().filter_map(|&i| snap.torrents.get(i)) {
        t.hash.hash(&mut h);
        t.queue_pos.hash(&mut h);
        match prefs.sort {
            Col::Queue => {}
            Col::Name => t.name.hash(&mut h),
            Col::Size => t.size.hash(&mut h),
            Col::Category => t.category.hash(&mut h),
            Col::Tags => t.tags.hash(&mut h),
            Col::Added => t.added_at.hash(&mut h),
            Col::Completed => t.completed_at.hash(&mut h),
            Col::SavePath => t.save_path.hash(&mut h),
            Col::Tracker => t.trackers.first().map(|u| hosts.peek(u)).hash(&mut h),
            Col::Error => t.error.hash(&mut h),
            _ => return None,
        }
    }
    rows.hash(&mut h);
    Some(h.finish())
}

pub fn compute_rows(
    snap: &Snapshot,
    prefs: &UiPrefs,
    search: &str,
    hosts: &mut TrackerHosts,
    memo: &mut SortMemo,
) -> Vec<usize> {
    let filter = RowFilter::new(prefs, search);
    let mut rows: Vec<usize> =
        (snap.torrents.iter().enumerate()).filter(|(_, t)| filter.matches(t, hosts)).map(|(i, _)| i).collect();
    let ts = &snap.torrents;
    if prefs.sort == Col::Tracker {
        for t in rows.iter().filter_map(|&i| ts.get(i)) {
            if let Some(u) = t.trackers.first() {
                hosts.get(u);
            }
        }
    }
    let hosts = &*hosts;
    let fingerprint = sort_fingerprint(&rows, snap, prefs, hosts);
    if fingerprint.is_some_and(|f| f == memo.fingerprint && memo.sorted.len() == rows.len()) {
        return memo.sorted.clone();
    }
    let cmp = |a: &TorrentView, b: &TorrentView| -> Ordering {
        let f = |x: f64, y: f64| x.partial_cmp(&y).unwrap_or(Ordering::Equal);
        match prefs.sort {
            Col::Queue => a.queue_pos.cmp(&b.queue_pos),
            Col::Name => natord(&a.name, &b.name),
            Col::Size => a.size.cmp(&b.size),
            Col::Progress => f(a.progress as f64, b.progress as f64),
            Col::Status => state_rank(a.state).cmp(&state_rank(b.state)),
            Col::Peers => a.peers_live.cmp(&b.peers_live),
            Col::Down => a.down_bps.cmp(&b.down_bps),
            Col::Activity => (a.down_bps + a.up_bps).cmp(&(b.down_bps + b.up_bps)),
            Col::Up => a.up_bps.cmp(&b.up_bps),
            Col::Eta => a.eta.unwrap_or(Duration::MAX).cmp(&b.eta.unwrap_or(Duration::MAX)),
            Col::Ratio => f(a.ratio as f64, b.ratio as f64),
            Col::Category => a.category.cmp(&b.category),
            Col::Tags => a.tags.cmp(&b.tags),
            Col::Added => a.added_at.cmp(&b.added_at),
            Col::Completed => a.completed_at.cmp(&b.completed_at),
            Col::Downloaded => a.downloaded.cmp(&b.downloaded),
            Col::Uploaded => a.uploaded.cmp(&b.uploaded),
            Col::SavePath => a.save_path.cmp(&b.save_path),
            // By what the column shows: the host, not the URL's scheme.
            Col::Tracker => a.trackers.first().map(|u| hosts.peek(u)).cmp(&b.trackers.first().map(|u| hosts.peek(u))),
            Col::SeedTime => a.seeding_secs.cmp(&b.seeding_secs),
            Col::LastActivity => a.last_activity.cmp(&b.last_activity),
            // Those with an error first when ascending.
            Col::Error => a.error.is_none().cmp(&b.error.is_none()).then_with(|| a.error.cmp(&b.error)),
        }
    };
    rows.sort_by(|&a, &b| {
        let (Some(a), Some(b)) = (ts.get(a), ts.get(b)) else { return Ordering::Equal };
        let o = cmp(a, b).then_with(|| a.queue_pos.cmp(&b.queue_pos));
        if prefs.sort_asc { o } else { o.reverse() }
    });
    memo.sorts += 1;
    if let Some(f) = fingerprint {
        memo.fingerprint = f;
        memo.sorted.clone_from(&rows);
    } else {
        memo.sorted.clear();
    }
    rows
}

fn state_rank(s: TorrentState) -> u8 {
    match s {
        TorrentState::Downloading => 0,
        TorrentState::Metadata => 1,
        TorrentState::Stalled => 2,
        TorrentState::Checking => 3,
        TorrentState::Seeding => 4,
        TorrentState::Queued => 5,
        TorrentState::Paused => 6,
        TorrentState::Completed => 7,
        TorrentState::Error => 8,
    }
}

/// Splits a leading run of ASCII digits off `s`.
fn digit_run(s: &str) -> (&str, &str) {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    s.split_at_checked(end).unwrap_or((s, ""))
}

/// Natural ordering: "ep2" < "ep10". Doesn't allocate: it runs tens of thousands of times per sort.
pub fn natord(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a, b);
    loop {
        let (mut ai, mut bi) = (a.chars(), b.chars());
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let ((na, rest_a), (nb, rest_b)) = (digit_run(a), digit_run(b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
                (a, b) = (rest_a, rest_b);
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                (a, b) = (ai.as_str(), bi.as_str());
            }
        }
    }
}

pub fn status_text(t: &TorrentView) -> String {
    if let Some(b) = &t.busy {
        return b.clone();
    }
    match t.state {
        TorrentState::Checking => t!("Checking {pct}%", pct = format!("{:.0}", t.progress * 100.0)),
        TorrentState::Error => t!("Error").into(),
        s => s.label().into(),
    }
}

impl CorrentApp {
    pub(super) fn torrent_table(&mut self, ui: &mut Ui, p: &Palette) {
        let rows = self.visible_rows().to_vec();
        let snap = self.snap.clone();

        if rows.is_empty() {
            self.empty_list(ui, p, snap.torrents.is_empty());
            return;
        }

        let compact = self.settings.appearance.compact_rows;
        let row_h = if compact { 30.0 } else { 52.0 };
        self.page_rows = ((ui.available_height() - 30.0) / (row_h + ui.spacing().item_spacing.y)).max(1.0) as usize;
        let focus_mark = self.focus.filter(|_| self.focus_visible);
        let cols = self.prefs.columns.clone();
        let show_bar_in_name = !cols.contains(&Col::Progress);
        let units = self.settings.appearance.speed_units;
        let animate = self.settings.appearance.animations;
        let state_colors = self.settings.appearance.state_colored_progress;

        let mut clicked: Option<(InfoHash, egui::Modifiers, bool)> = None;
        let mut context_target: Option<InfoHash> = None;
        let mut sort_clicked: Option<Col> = None;
        let mut header_menu = false;

        let scroll_to = if self.scroll_to_focus {
            self.scroll_to_focus = false;
            self.focus.and_then(|f| rows.iter().position(|&i| snap.torrents.get(i).is_some_and(|t| t.hash == f)))
        } else {
            None
        };

        if std::mem::take(&mut self.fit_columns) {
            self.measure_columns(ui, &rows, units);
        }
        let seen_before = self.rows_seen_once;
        // Drag rows to reorder the queue (only while the list is sorted by queue position).
        let reorder = self.prefs.sort == Col::Queue && self.prefs.sort_asc;
        let mut drag_start: Option<InfoHash> = None;
        let mut drop_before: Option<Option<InfoHash>> = None;
        let pointer = ui.ctx().pointer_interact_pos();
        egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 0 }).show(ui, |ui| {
            let mut tb = TableBuilder::new(ui)
                .id_salt(("torrents", self.prefs.columns_fitted))
                .striped(self.settings.appearance.striped_rows)
                .resizable(true)
                .sense(if reorder { Sense::click_and_drag() } else { Sense::click() })
                .cell_layout(Layout::left_to_right(Align::Center))
                .min_scrolled_height(0.0)
                .auto_shrink([false, false]);
            if let Some(r) = scroll_to {
                tb = tb.scroll_to_row(r, None);
            }
            for c in &cols {
                let col = if *c == Col::Name {
                    // Not resizable: egui keeps a resizable column's width, so after the window
                    // shrank (restored from maximized, a larger zoom) the name pushed the other
                    // columns out of view. It takes what the others leave instead.
                    Column::remainder().at_least(180.0).clip(true).resizable(false)
                } else {
                    let saved = self.prefs.column_widths.iter().find(|(x, _)| x == c).map(|(_, w)| *w);
                    Column::initial(saved.unwrap_or(c.default_width())).at_least(32.0).clip(true)
                };
                tb = tb.column(col);
            }
            tb.header(26.0, |mut header| {
                for c in &cols {
                    let (_, resp) = header.col(|ui| {
                        let active = self.prefs.sort == *c;
                        let arrow = if !active {
                            ""
                        } else if self.prefs.sort_asc {
                            icons::CARET_UP
                        } else {
                            icons::CARET_DOWN
                        };
                        let layout = if c.right_aligned() {
                            Layout::right_to_left(Align::Center)
                        } else {
                            Layout::left_to_right(Align::Center)
                        };
                        ui.with_layout(layout, |ui| {
                            let color = if active { p.text } else { p.text_dim };
                            if c.right_aligned() {
                                ui.label(RichText::new(arrow).small().color(p.accent));
                                ui.label(RichText::new(c.title()).small().strong().color(color));
                            } else {
                                ui.label(RichText::new(c.title()).small().strong().color(color));
                                ui.label(RichText::new(arrow).small().color(p.accent));
                            }
                        });
                    });
                    if resp.clicked() {
                        sort_clicked = Some(*c);
                    }
                    if resp.secondary_clicked() {
                        header_menu = true;
                    }
                }
            })
            .body(|body| {
                body.rows(row_h, rows.len(), |mut row| {
                    // A row list from an older snapshot may point past this one (STABILITY.md D12).
                    let Some(t) = rows.get(row.index()).and_then(|&i| snap.torrents.get(i)) else { return };
                    let selected = self.selected.contains(&t.hash);
                    let fade = self.row_fade(t.hash);
                    row.set_selected(selected);
                    let color = if state_colors { p.state_color(t.state) } else { p.accent };
                    let dim = t.state.is_paused_like() && t.state != TorrentState::Queued;
                    for c in &cols {
                        row.col(|ui| {
                            if fade < 1.0 {
                                ui.multiply_opacity(fade);
                            }
                            self.cell(ui, p, t, *c, compact, show_bar_in_name, color, dim, units, animate);
                        });
                    }
                    let resp = row.response();
                    if focus_mark == Some(t.hash) {
                        // Where the keyboard is, also on a row that isn't selected.
                        let stroke = egui::Stroke::new(1.0, if p.high_contrast { p.text } else { p.accent });
                        let at = egui::LayerId::new(egui::Order::Foreground, Id::new("list_focus"));
                        resp.ctx.layer_painter(at).rect_stroke(
                            resp.rect,
                            egui::CornerRadius::same(3),
                            stroke,
                            egui::StrokeKind::Inside,
                        );
                    }
                    if reorder && resp.drag_started() && !t.pending {
                        drag_start = Some(t.hash);
                    }
                    if let (Some(drag), Some(pos)) = (&self.queue_drag, pointer)
                        && !t.pending
                        && resp.rect.y_range().contains(pos.y)
                    {
                        let upper = pos.y < resp.rect.center().y;
                        let before = if upper {
                            Some(t.hash)
                        } else {
                            (rows.get(row.index() + 1..).unwrap_or_default().iter())
                                .filter_map(|&i| snap.torrents.get(i))
                                .find(|n| !n.pending)
                                .map(|n| n.hash)
                        };
                        if before.is_none_or(|b| !drag.contains(&b)) || drag.contains(&t.hash) {
                            drop_before = Some(before);
                            let y = if upper { resp.rect.top() } else { resp.rect.bottom() };
                            resp.ctx
                                .layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("queue_drop")))
                                .hline(resp.rect.x_range(), y, egui::Stroke::new(2.0, p.accent));
                        }
                    }
                    if resp.clicked() {
                        clicked = Some((t.hash, resp.ctx.input(|i| i.modifiers), false));
                    }
                    if resp.double_clicked() {
                        clicked = Some((t.hash, egui::Modifiers::NONE, true));
                    }
                    if resp.secondary_clicked() && !t.pending {
                        context_target = Some(t.hash);
                    }
                    crate::ui::widgets::context_menu(&resp, |ui| {
                        if t.pending {
                            if ui.button(format!("{}  {}", icons::X_CIRCLE, t!("Cancel"))).clicked() {
                                self.engine.send(crate::engine::EngineCommand::CancelAdd(t.hash));
                            }
                        } else {
                            self.context_menu(ui, p);
                        }
                    });
                });
            });
        });

        if !seen_before && !snap.torrents.is_empty() {
            self.rows_seen_once = true;
        }
        if let Some(h) = drag_start {
            // Dragging a selected row moves the whole selection.
            let hashes = if self.selected.contains(&h) { self.selected.iter().copied().collect() } else { vec![h] };
            self.queue_drag = Some(hashes);
        }
        if let Some(drag) = &self.queue_drag {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if !ui.input(|i| i.pointer.primary_down()) {
                if let Some(before) = drop_before
                    && !before.is_some_and(|b| drag.contains(&b))
                {
                    self.engine.send(crate::engine::EngineCommand::QueuePlace { hashes: drag.clone(), before });
                }
                self.queue_drag = None;
            }
        }
        if self.row_first_seen.len() > snap.torrents.len() + 64 {
            let live: std::collections::HashSet<InfoHash> = snap.torrents.iter().map(|t| t.hash).collect();
            self.row_first_seen.retain(|h, _| live.contains(h));
        }
        if let Some(c) = sort_clicked {
            if self.prefs.sort == c {
                self.prefs.sort_asc = !self.prefs.sort_asc;
            } else {
                self.prefs.sort = c;
                self.prefs.sort_asc = matches!(
                    c,
                    Col::Name
                        | Col::Queue
                        | Col::Category
                        | Col::Tags
                        | Col::SavePath
                        | Col::Tracker
                        | Col::Eta
                        | Col::Error
                );
            }
        }
        if header_menu {
            self.column_menu = ui.ctx().pointer_latest_pos();
            egui::Popup::open_id(ui.ctx(), Id::new(COLUMN_CHOOSER));
        }
        self.column_chooser(ui, p);

        if let Some(h) = context_target
            && !self.selected.contains(&h)
        {
            self.selected.clear();
            self.selected.insert(h);
            self.anchor = Some(h);
            self.focus = Some(h);
        }
        if let Some((h, mods, double)) = clicked {
            if double {
                if self.server.is_none()
                    && let Some(t) = snap.torrents.iter().find(|t| t.hash == h)
                {
                    super::open_path(&t.content_path);
                }
            } else {
                self.click_row(h, mods, &rows);
            }
        }
    }

    /// "Fit columns": every column (but the name, which takes the rest) as wide as its title and
    /// the first rows of the list need.
    fn measure_columns(&mut self, ui: &Ui, rows: &[usize], units: crate::config::SpeedUnits) {
        let snap = self.snap.clone();
        let hosts = &self.counts.hosts;
        let shown: Vec<&TorrentView> = rows.iter().take(100).filter_map(|&i| snap.torrents.get(i)).collect();
        let width = |text: String, font: egui::FontId| ui.painter().layout_no_wrap(text, font, Color32::WHITE).size().x;
        let body = egui::TextStyle::Body.resolve(ui.style());
        let small = egui::TextStyle::Small.resolve(ui.style());
        let mono = egui::TextStyle::Monospace.resolve(ui.style());
        let mut widths = Vec::new();
        for &c in &self.prefs.columns {
            let cell = |t: &TorrentView| -> f32 {
                match c {
                    Col::Name => 0.0,
                    Col::Progress => 120.0,
                    Col::Activity => 100.0,
                    Col::Queue => width((t.queue_pos + 1).to_string(), mono.clone()),
                    Col::Size => width(fmt_bytes(t.size), mono.clone()),
                    Col::Status => width(status_text(t), egui::FontId::proportional(11.0)) + 14.0,
                    Col::Peers => width(format!("{} / {}", t.peers_live, t.peers_seen), mono.clone()),
                    Col::Down => width(fmt_speed(t.down_bps, units), mono.clone()),
                    Col::Up => width(fmt_speed(t.up_bps, units), mono.clone()),
                    Col::Eta => width(t.eta.map(fmt_duration).unwrap_or_else(|| "∞".into()), mono.clone()),
                    Col::Ratio => width(fmt_ratio(t.ratio), mono.clone()),
                    Col::Category => width(t.category.clone().unwrap_or_default(), body.clone()),
                    Col::Tags => width(t.tags.join(", "), body.clone()),
                    Col::Added => width(fmt_date(t.added_at), small.clone()),
                    Col::Completed => width(t.completed_at.map(fmt_date).unwrap_or_default(), small.clone()),
                    Col::Downloaded => width(fmt_bytes(t.downloaded), mono.clone()),
                    Col::Uploaded => width(fmt_bytes(t.uploaded), mono.clone()),
                    Col::SavePath => width(t.save_path.display().to_string(), small.clone()),
                    Col::Tracker => {
                        width(t.trackers.first().map(|u| hosts.peek(u)).unwrap_or_default().to_string(), small.clone())
                    }
                    Col::SeedTime => width(fmt_duration(Duration::from_secs(t.seeding_secs)), mono.clone()),
                    Col::LastActivity => width(fmt_date(t.last_activity), small.clone()),
                    Col::Error => width(
                        t.error.as_deref().and_then(|e| e.lines().next()).unwrap_or_default().to_string(),
                        small.clone(),
                    ),
                }
            };
            let content = shown.iter().map(|t| cell(t)).fold(0.0f32, f32::max);
            // Room for the title and its sort arrow, and the cell's margins.
            let title = width(c.title().to_string(), small.clone()) + 16.0;
            widths.push((c, (content.max(title) + 14.0).clamp(32.0, 420.0)));
        }
        self.prefs.column_widths = widths;
        self.prefs.columns_fitted = self.prefs.columns_fitted.wrapping_add(1);
    }

    /// Opacity for a row: new rows fade in over 250 ms (rows present at startup don't).
    fn row_fade(&mut self, h: InfoHash) -> f32 {
        if !self.settings.appearance.animations {
            return 1.0;
        }
        let first = !self.rows_seen_once;
        let since = *self.row_first_seen.entry(h).or_insert_with(|| {
            if first {
                std::time::Instant::now() - std::time::Duration::from_secs(1)
            } else {
                std::time::Instant::now()
            }
        });
        let t = (since.elapsed().as_secs_f32() / 0.25).min(1.0);
        if t < 1.0 {
            self.ctx.request_repaint();
        }
        t
    }

    fn click_row(&mut self, h: InfoHash, mods: egui::Modifiers, rows: &[usize]) {
        let snap = self.snap.clone();
        if mods.shift
            && let Some(anchor) = self.anchor
        {
            let pos = |x: InfoHash| rows.iter().position(|&i| snap.torrents.get(i).is_some_and(|t| t.hash == x));
            if let (Some(a), Some(b)) = (pos(anchor), pos(h)) {
                if !mods.command {
                    self.selected.clear();
                }
                let range = rows.get(a.min(b)..=a.max(b)).unwrap_or_default();
                for t in range.iter().filter_map(|&i| snap.torrents.get(i)) {
                    self.selected.insert(t.hash);
                }
            }
        } else if mods.command {
            if !self.selected.remove(&h) {
                self.selected.insert(h);
            }
            self.anchor = Some(h);
        } else {
            self.selected.clear();
            self.selected.insert(h);
            self.anchor = Some(h);
        }
        self.focus = Some(h);
        self.focus_visible = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn cell(
        &self,
        ui: &mut Ui,
        p: &Palette,
        t: &TorrentView,
        c: Col,
        compact: bool,
        bar_in_name: bool,
        color: Color32,
        dim: bool,
        units: crate::config::SpeedUnits,
        animate: bool,
    ) {
        let txt = |s: String| RichText::new(s).color(if dim { p.text_dim } else { p.text });
        let num = |s: String| RichText::new(s).monospace().color(if dim { p.text_dim } else { p.text });
        let right = |ui: &mut Ui, r: RichText| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(r);
            });
        };
        match c {
            Col::Name => {
                if compact {
                    ui.label(RichText::new(crate::ui::theme::Palette::state_icon(t.state)).color(color));
                    ui.add(egui::Label::new(txt(t.name.clone())).truncate());
                } else {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(
                                RichText::new(crate::ui::theme::Palette::state_icon(t.state)).size(15.0).color(color),
                            );
                            ui.add(egui::Label::new(txt(t.name.clone()).strong()).truncate());
                        });
                        if bar_in_name {
                            let w = ui.available_width().min(520.0);
                            widgets::progress_bar(
                                ui,
                                Id::new(("pb", t.hash)),
                                t.progress,
                                color,
                                p.bg3,
                                4.0,
                                w,
                                matches!(t.state, TorrentState::Checking | TorrentState::Metadata),
                                animate,
                            );
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(self.meta_line(t)).small().color(p.text_dim));
                            if let Some(cat) = &t.category {
                                chip(ui, p, icons::FOLDER_SIMPLE, cat);
                            }
                            for tag in t.tags.iter().take(4) {
                                chip(ui, p, icons::TAG, tag);
                            }
                        });
                    });
                }
            }
            Col::Queue => right(ui, num(if t.pending { "—".into() } else { format!("{}", t.queue_pos + 1) })),
            Col::Size => right(ui, num(if t.size == 0 { "—".into() } else { fmt_bytes(t.size) })),
            Col::Progress => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let w = (ui.available_width() - 44.0).max(20.0);
                    widgets::progress_bar(
                        ui,
                        Id::new(("pb", t.hash)),
                        t.progress,
                        color,
                        p.bg3,
                        if compact { 6.0 } else { 7.0 },
                        w,
                        matches!(t.state, TorrentState::Checking | TorrentState::Metadata),
                        animate,
                    );
                    ui.label(
                        RichText::new(format!("{:.0}%", (t.progress * 100.0).floor()))
                            .small()
                            .monospace()
                            .color(p.text_dim),
                    );
                });
            }
            Col::Status => {
                let status = status_text(t);
                badge(ui, &status, color).on_hover_ui(|ui| {
                    ui.label(t.error.as_deref().unwrap_or(&status));
                });
            }
            Col::Peers => {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if t.peers_seen > 0 {
                        ui.label(RichText::new(format!("/ {}", t.peers_seen)).small().monospace().color(p.text_mute));
                    }
                    ui.label(num(t.peers_live.to_string()));
                })
                .response
                .on_hover_ui(|ui| {
                    ui.label(t!(
                        "{live} connected · {connecting} connecting · {known} known\nA torrent with few seeders may only have a handful of connections.",
                        live = t.peers_live,
                        connecting = t.peers_connecting,
                        known = t.peers_seen
                    ));
                });
            }
            Col::Activity => {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), Sense::hover());
                let hist = self.snap.row_history.get(&t.hash).map(Vec::as_slice).unwrap_or(&[]);
                if hist.iter().all(|(d, u)| *d == 0 && *u == 0) {
                    ui.painter().line_segment(
                        [rect.left_center(), rect.right_center()],
                        egui::Stroke::new(1.0, p.text_mute.gamma_multiply(0.4)),
                    );
                } else {
                    let max = hist.iter().fold(1.0f32, |m, (d, u)| m.max(*d as f32).max(*u as f32));
                    let (downs, ups) = (hist.iter().map(|(d, _)| *d as f32), hist.iter().map(|(_, u)| *u as f32));
                    widgets::paint_samples(ui.painter(), rect, ups, p.success, max, false);
                    widgets::paint_samples(ui.painter(), rect, downs, p.accent, max, true);
                    resp.on_hover_ui(|ui| {
                        ui.label(t!(
                            "Last minute — down {down} · up {up}",
                            down = fmt_speed(t.down_bps, units),
                            up = fmt_speed(t.up_bps, units)
                        ));
                    });
                }
            }
            Col::Down => right(
                ui,
                RichText::new(fmt_speed(t.down_bps, units)).monospace().color(if t.down_bps > 0 {
                    p.accent
                } else {
                    p.text_mute
                }),
            ),
            Col::Up => right(
                ui,
                RichText::new(fmt_speed(t.up_bps, units)).monospace().color(if t.up_bps > 0 {
                    p.success
                } else {
                    p.text_mute
                }),
            ),
            Col::Eta => right(
                ui,
                num(t
                    .eta
                    .map(fmt_duration)
                    .unwrap_or_else(|| if t.progress >= 1.0 { t!("Done").into() } else { "∞".into() })),
            ),
            Col::Ratio => right(ui, num(fmt_ratio(t.ratio))),
            Col::Category => {
                ui.label(txt(t.category.clone().unwrap_or_default()));
            }
            Col::Tags => {
                ui.add(egui::Label::new(txt(t.tags.join(", "))).truncate());
            }
            Col::Added => {
                ui.label(RichText::new(fmt_date(t.added_at)).small().color(p.text_dim));
            }
            Col::Completed => {
                ui.label(RichText::new(t.completed_at.map(fmt_date).unwrap_or_default()).small().color(p.text_dim));
            }
            Col::Downloaded => right(ui, num(fmt_bytes(t.downloaded))),
            Col::Uploaded => right(ui, num(fmt_bytes(t.uploaded))),
            Col::SavePath => {
                ui.add(
                    egui::Label::new(RichText::new(t.save_path.display().to_string()).small().color(p.text_dim))
                        .truncate(),
                );
            }
            Col::Tracker => {
                ui.label(
                    RichText::new(t.trackers.first().map(|u| self.counts.hosts.peek(u)).unwrap_or_default())
                        .small()
                        .color(p.text_dim),
                );
            }
            Col::SeedTime => right(
                ui,
                num(if t.seeding_secs > 0 { fmt_duration(Duration::from_secs(t.seeding_secs)) } else { "—".into() }),
            ),
            Col::LastActivity => {
                ui.label(RichText::new(fmt_date(t.last_activity)).small().color(p.text_dim));
            }
            Col::Error => {
                if let Some(e) = &t.error {
                    let first = e.lines().next().unwrap_or_default();
                    ui.add(egui::Label::new(RichText::new(first).small().color(p.error)).truncate()).on_hover_ui(
                        |ui| {
                            ui.label(e);
                        },
                    );
                }
            }
        }
    }

    fn meta_line(&self, t: &TorrentView) -> String {
        let units = self.settings.appearance.speed_units;
        if t.pending {
            return t!("Fetching metadata from peers…").into();
        }
        if let Some(e) = &t.error {
            return e.lines().next().unwrap_or("").to_string();
        }
        let mut parts = Vec::with_capacity(4);
        parts.push(status_text(t));
        if t.size > 0 {
            if t.done < t.size {
                parts.push(t!("{done} of {size}", done = fmt_bytes(t.done), size = fmt_bytes(t.size)));
            } else {
                parts.push(fmt_bytes(t.size));
            }
        }
        match t.state {
            TorrentState::Downloading | TorrentState::Stalled => {
                if t.down_bps > 0 {
                    parts.push(format!("↓ {}", fmt_speed(t.down_bps, units)));
                }
                if let Some(eta) = t.eta {
                    parts.push(t!("{eta} left", eta = fmt_duration(eta)));
                }
                parts.push(t!(
                    "{live} of {known} peers connected",
                    live = t.peers_live,
                    known = t.peers_seen.max(t.peers_live)
                ));
            }
            TorrentState::Seeding => {
                if t.up_bps > 0 {
                    parts.push(format!("↑ {}", fmt_speed(t.up_bps, units)));
                }
                parts.push(t!("ratio {r}", r = fmt_ratio(t.ratio)));
            }
            _ => {}
        }
        parts.join("  ·  ")
    }

    fn empty_list(&mut self, ui: &mut Ui, p: &Palette, no_torrents: bool) {
        if no_torrents {
            widgets::empty_state(
                ui,
                p,
                icons::DOWNLOAD_SIMPLE,
                t!("No torrents yet"),
                t!("Drop a .torrent file here, paste a magnet link, or use the buttons below."),
            );
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    let w = 96.0 * 2.0 + 120.0;
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    if widgets::primary_button(ui, p, &format!("{}  {}", icons::FILE_PLUS, t!("Add torrent file")))
                        .clicked()
                    {
                        self.pick_torrent_files();
                    }
                    if widgets::secondary_button(ui, &format!("{}  {}", icons::MAGNET, t!("Add magnet link"))).clicked()
                    {
                        self.open_magnet_dialog();
                    }
                });
            });
        } else if !self.search.trim().is_empty() {
            let filters = self.active_filters();
            let q = self.search.trim().to_string();
            let subtitle = if filters.is_empty() {
                t!("No torrents match “{q}”.", q = q)
            } else {
                t!("No torrents match “{q}” in {filters}.", q = q, filters = filters.join(" · "))
            };
            widgets::empty_state(ui, p, icons::MAGNIFYING_GLASS, t!("No matches"), &subtitle);
            ui.vertical_centered(|ui| {
                if widgets::secondary_button(ui, t!("Clear search")).clicked() {
                    self.search.clear();
                }
                if !filters.is_empty() && widgets::secondary_button(ui, t!("Show all torrents")).clicked() {
                    self.clear_filters();
                    self.search.clear();
                }
            });
        } else {
            let filters = self.active_filters().join(" · ");
            widgets::empty_state(
                ui,
                p,
                icons::FUNNEL,
                &t!("Nothing in “{f}”", f = filters),
                t!("No torrents match the current filters."),
            );
            ui.vertical_centered(|ui| {
                if widgets::secondary_button(ui, t!("Show all torrents")).clicked() {
                    self.clear_filters();
                }
            });
        }
    }

    /// The filters in force, as the sidebar names them.
    fn active_filters(&self) -> Vec<String> {
        let group = |g: &GroupFilter, none: &str| match g {
            GroupFilter::All => None,
            GroupFilter::None => Some(none.to_string()),
            GroupFilter::Named(n) => Some(n.clone()),
        };
        let status = self.prefs.status_filter;
        (status != super::StatusFilter::All)
            .then(|| status.label().to_string())
            .into_iter()
            .chain(group(&self.prefs.category_filter, t!("Uncategorized")))
            .chain(group(&self.prefs.tag_filter, t!("Untagged")))
            .chain(group(&self.prefs.tracker_filter, t!("No trackers")))
            .collect()
    }

    pub(super) fn clear_filters(&mut self) {
        self.prefs.status_filter = super::StatusFilter::All;
        self.prefs.category_filter = GroupFilter::All;
        self.prefs.tag_filter = GroupFilter::All;
        self.prefs.tracker_filter = GroupFilter::All;
    }

    fn column_chooser(&mut self, ui: &mut Ui, p: &Palette) {
        let id = Id::new(COLUMN_CHOOSER);
        let Some(pos) = self.column_menu else { return };
        let popup = egui::Popup::new(id, ui.ctx().clone(), pos, ui.layer_id())
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        widgets::popup(popup, |ui| {
            ui.set_width(240.0);
            ui.label(RichText::new(t!("Columns")).strong().color(p.text));
            // Above the list, which can be taller than the popup.
            ui.horizontal(|ui| {
                if ui
                    .button(t!("Fit columns"))
                    .on_hover_text(t!("Make every column as wide as its contents need"))
                    .clicked()
                {
                    self.fit_columns = true;
                }
                if ui.button(t!("Reset to defaults")).clicked() {
                    self.prefs.columns = super::UiPrefs::default().columns;
                    self.prefs.column_widths.clear();
                    self.prefs.columns_fitted = self.prefs.columns_fitted.wrapping_add(1);
                }
            });
            ui.separator();
            // Shown columns in their order (with move buttons), then the hidden ones.
            let shown: Vec<Col> = self.prefs.columns.clone();
            let mut hidden: Vec<Col> = Col::ALL.into_iter().filter(|c| !shown.contains(c)).collect();
            hidden.retain(|c| *c != Col::Name);
            let mut moved: Option<(usize, usize)> = None;
            let mut toggled: Option<(Col, bool)> = None;
            for (i, c) in shown.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut on = true;
                    let fixed = *c == Col::Name; // the name column always stays
                    if ui.add_enabled(!fixed, egui::Checkbox::new(&mut on, c.title())).changed() {
                        toggled = Some((*c, false));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let last = i + 1 == shown.len();
                        if ui
                            .add_enabled(!last, egui::Button::new(icons::CARET_DOWN).small())
                            .labeled(t!("Move right"))
                            .clicked()
                        {
                            moved = Some((i, i + 1));
                        }
                        if ui
                            .add_enabled(i > 0, egui::Button::new(icons::CARET_UP).small())
                            .labeled(t!("Move left"))
                            .clicked()
                        {
                            moved = Some((i, i - 1));
                        }
                    });
                });
            }
            for c in hidden {
                let mut on = false;
                if ui.checkbox(&mut on, c.title()).changed() {
                    toggled = Some((c, true));
                }
            }
            if let Some((a, b)) = moved {
                self.prefs.columns.swap(a, b);
            }
            match toggled {
                Some((c, true)) => self.prefs.columns.push(c),
                Some((c, false)) => self.prefs.columns.retain(|x| *x != c),
                None => {}
            }
        });
        // Closed by a click elsewhere or Escape.
        if !egui::Popup::is_id_open(ui.ctx(), id) {
            self.column_menu = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        assert_eq!(natord("ep2", "ep10"), Ordering::Less);
        assert_eq!(natord("A", "a"), Ordering::Equal);
        assert_eq!(natord("b", "a10"), Ordering::Greater);
    }

    #[test]
    fn natural_order_details() {
        assert_eq!(natord("a007", "a7"), Ordering::Equal, "leading zeros don't count");
        assert_eq!(natord("a7b", "a007c"), Ordering::Less, "and the rest still does");
        assert_eq!(natord("", "a"), Ordering::Less);
        assert_eq!(natord("é2", "É10"), Ordering::Less);
        assert_eq!(natord("99999999999999999999999", "100000000000000000000000"), Ordering::Less);
    }

    /// Counts this thread's allocations, for `natord` and the search matching.
    struct Counting;
    thread_local! { static ALLOCS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
    // SAFETY: passes everything to the system allocator; the counter doesn't allocate.
    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
            let _ = ALLOCS.try_with(|a| a.set(a.get() + 1));
            // SAFETY: the caller's contract, passed on.
            unsafe { std::alloc::System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
            // SAFETY: the caller's contract, passed on.
            unsafe { std::alloc::System.dealloc(p, l) }
        }
    }
    #[global_allocator]
    static COUNTING: Counting = Counting;

    #[test]
    fn natord_and_matching_dont_allocate() {
        let before = ALLOCS.with(|a| a.get());
        let mut o = Ordering::Equal;
        for _ in 0..1000 {
            o = natord("Some.Linux.Distro.1234.amd64.iso", "Some.Linux.Distro.1235.amd64.iso");
            std::hint::black_box(contains_lower("Some.Linux.Distro.1234.amd64.iso", "distro"));
        }
        assert_eq!(o, Ordering::Less);
        assert_eq!(ALLOCS.with(|a| a.get()) - before, 0);
    }

    #[test]
    fn host() {
        assert_eq!(tracker_host("udp://tracker.example.org:1337/announce"), "tracker.example.org");
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::app::{StatusFilter, UiPrefs};
    use crate::engine::types::InfoHash;

    fn t(n: u8, name: &str) -> TorrentView {
        TorrentView {
            hash: InfoHash([n; 20]),
            name: name.into(),
            size: n as u64 * 10,
            queue_pos: n as u32,
            ..Default::default()
        }
    }

    fn rows_of(sn: &Snapshot, p: &UiPrefs) -> Vec<usize> {
        compute_rows(sn, p, "", &mut TrackerHosts::default(), &mut SortMemo::default())
    }

    fn mf(t: &TorrentView, prefs: &UiPrefs, search: &str) -> bool {
        RowFilter::new(prefs, search).matches(t, &mut TrackerHosts::default())
    }

    #[test]
    fn tracker_sort_goes_by_host() {
        let tracked = |n: u8, url: &str| TorrentView {
            trackers: [url.to_string()].into_iter().collect(),
            ..t(n, &format!("t{n}"))
        };
        let sn = snap(vec![tracked(1, "udp://a.example:80/announce"), tracked(2, "https://z.example/announce")]);
        let p = UiPrefs { sort: Col::Tracker, sort_asc: true, ..Default::default() };
        let rows = rows_of(&sn, &p);
        assert_eq!(sn.torrents[rows[0]].name, "t1", "a.example before z.example, whatever the scheme");
    }

    fn snap(ts: Vec<TorrentView>) -> Snapshot {
        Snapshot { torrents: ts, ..Default::default() }
    }

    #[test]
    fn category_filter_includes_children() {
        let mut a = t(1, "a");
        a.category = Some("Movies/HD".into());
        let mut b = t(2, "b");
        b.category = Some("Music".into());
        let prefs = UiPrefs { category_filter: GroupFilter::Named("Movies".into()), ..Default::default() };
        assert!(mf(&a, &prefs, ""));
        assert!(!mf(&b, &prefs, ""));
        let none = UiPrefs { category_filter: GroupFilter::None, ..Default::default() };
        assert!(mf(&t(3, "c"), &none, ""));
        assert!(!mf(&a, &none, ""));
    }

    #[test]
    fn tag_and_tracker_filters() {
        let mut a = t(1, "a");
        a.tags = vec!["linux".into()];
        a.trackers = vec!["udp://tracker.example.org:1337/announce".into()];
        let tag = UiPrefs { tag_filter: GroupFilter::Named("linux".into()), ..Default::default() };
        assert!(mf(&a, &tag, ""));
        assert!(!mf(&t(2, "b"), &tag, ""));
        let tr = UiPrefs { tracker_filter: GroupFilter::Named("tracker.example.org".into()), ..Default::default() };
        assert!(mf(&a, &tr, ""));
        let none = UiPrefs { tracker_filter: GroupFilter::None, ..Default::default() };
        assert!(mf(&t(2, "b"), &none, ""));
    }

    #[test]
    fn search_terms_all_must_match() {
        let mut a = t(1, "Ubuntu Server 24.04");
        a.tags = vec!["iso".into()];
        let p = UiPrefs::default();
        assert!(mf(&a, &p, "ubuntu 24"));
        assert!(mf(&a, &p, "ubuntu iso"), "tags match whole terms");
        assert!(!mf(&a, &p, "ubuntu desktop"));
        assert!(mf(&a, &p, "0101"), "info hash prefix");
    }

    #[test]
    fn status_filters() {
        let mut d = t(1, "d");
        d.state = TorrentState::Downloading;
        d.down_bps = 5;
        let mut s = t(2, "s");
        s.state = TorrentState::Seeding;
        s.completed_at = Some(1);
        assert!(StatusFilter::Downloading.matches(&d));
        assert!(!StatusFilter::Downloading.matches(&s));
        assert!(StatusFilter::Seeding.matches(&s));
        assert!(StatusFilter::Completed.matches(&s));
        assert!(StatusFilter::Active.matches(&d));
        assert!(StatusFilter::Inactive.matches(&s));
    }

    #[test]
    fn the_order_is_reused_until_what_it_depends_on_changes() {
        let mut sn = snap(vec![t(1, "ep10"), t(2, "ep2"), t(3, "Ep1")]);
        let p = UiPrefs { sort: Col::Name, sort_asc: true, ..Default::default() };
        let (mut hosts, mut memo) = (TrackerHosts::default(), SortMemo::default());
        let first = compute_rows(&sn, &p, "", &mut hosts, &mut memo);
        assert_eq!(memo.sorts, 1);
        // A tick: speeds and progress change.
        sn.torrents[0].down_bps = 500;
        sn.torrents[1].progress = 0.5;
        assert_eq!(compute_rows(&sn, &p, "", &mut hosts, &mut memo), first);
        assert_eq!(memo.sorts, 1, "not sorted again");
        // A rename can change the order.
        sn.torrents[2].name = "ep99".into();
        let renamed = compute_rows(&sn, &p, "", &mut hosts, &mut memo);
        assert_eq!(memo.sorts, 2);
        assert_eq!(sn.torrents[renamed[2]].name, "ep99");
        // A filter that hides one.
        let filtered = compute_rows(&sn, &p, "ep2", &mut hosts, &mut memo);
        assert_eq!(filtered.len(), 1);
        // Columns that change all the time always sort.
        let by_speed = UiPrefs { sort: Col::Down, ..p.clone() };
        let before = memo.sorts;
        compute_rows(&sn, &by_speed, "", &mut hosts, &mut memo);
        compute_rows(&sn, &by_speed, "", &mut hosts, &mut memo);
        assert_eq!(memo.sorts, before + 2);
    }

    #[test]
    fn search_ignores_case_outside_ascii_too() {
        let a = t(1, "ÇOCUK Filmi");
        let p = UiPrefs::default();
        assert!(mf(&a, &p, "çocuk"));
        assert!(mf(&a, &p, "FİLM") || mf(&a, &p, "film"));
    }

    #[test]
    fn sorting_by_name_natural_and_size_desc() {
        let sn = snap(vec![t(1, "ep10"), t(2, "ep2"), t(3, "Ep1")]);
        let mut p = UiPrefs { sort: Col::Name, sort_asc: true, ..Default::default() };
        let rows = rows_of(&sn, &p);
        let names: Vec<&str> = rows.iter().map(|&i| sn.torrents[i].name.as_str()).collect();
        assert_eq!(names, vec!["Ep1", "ep2", "ep10"]);
        p.sort = Col::Size;
        p.sort_asc = false;
        let rows = rows_of(&sn, &p);
        assert_eq!(sn.torrents[rows[0]].size, 30);
    }

    #[test]
    fn eta_sort_puts_unknown_last() {
        let mut a = t(1, "a");
        a.eta = Some(Duration::from_secs(5));
        let b = t(2, "b");
        let sn = snap(vec![b, a]);
        let p = UiPrefs { sort: Col::Eta, sort_asc: true, ..Default::default() };
        let rows = rows_of(&sn, &p);
        assert_eq!(sn.torrents[rows[0]].name, "a");
    }

    #[test]
    fn status_text_variants() {
        let mut a = t(1, "a");
        a.state = TorrentState::Checking;
        a.progress = 0.5;
        assert_eq!(status_text(&a), "Checking 50%");
        a.busy = Some("Moving files…".into());
        assert_eq!(status_text(&a), "Moving files…");
    }
}
