use std::cmp::Ordering;
use std::time::Duration;

use egui::{Align, Color32, Id, Layout, RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icons;

use super::{Col, CorrentApp, GroupFilter, UiPrefs};
use crate::engine::types::{InfoHash, Snapshot, TorrentState, TorrentView};
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, Labeled, badge, chip};
use crate::util::{fmt_bytes, fmt_date, fmt_duration, fmt_ratio, fmt_speed};

pub fn tracker_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| url.to_string())
}

pub fn matches_filters(t: &TorrentView, prefs: &UiPrefs, search: &str) -> bool {
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
            match &t.category {
                Some(tc) if tc == c || tc.starts_with(&format!("{c}/")) => {}
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
            if !t.trackers.iter().any(|u| &tracker_host(u) == host) {
                return false;
            }
        }
    }
    if !search.trim().is_empty() {
        let q = search.trim().to_lowercase();
        let hay_name = t.name.to_lowercase();
        let all_terms = q.split_whitespace().all(|term| {
            hay_name.contains(term)
                || t.hash.to_hex().starts_with(term)
                || t.tags.iter().any(|tag| tag.to_lowercase() == term)
                || t.category.as_ref().map(|c| c.to_lowercase() == term).unwrap_or(false)
        });
        if !all_terms {
            return false;
        }
    }
    true
}

pub fn compute_rows(snap: &Snapshot, prefs: &UiPrefs, search: &str) -> Vec<usize> {
    let mut rows: Vec<usize> =
        (snap.torrents.iter().enumerate()).filter(|(_, t)| matches_filters(t, prefs, search)).map(|(i, _)| i).collect();
    let ts = &snap.torrents;
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
            Col::Tracker => a.trackers.first().cmp(&b.trackers.first()),
            Col::SeedTime => a.seeding_secs.cmp(&b.seeding_secs),
            Col::LastActivity => a.last_activity.cmp(&b.last_activity),
        }
    };
    rows.sort_by(|&a, &b| {
        let (Some(a), Some(b)) = (ts.get(a), ts.get(b)) else { return Ordering::Equal };
        let o = cmp(a, b).then_with(|| a.queue_pos.cmp(&b.queue_pos));
        if prefs.sort_asc { o } else { o.reverse() }
    });
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

/// Natural ordering: "ep2" < "ep10".
pub fn natord(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
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

        let seen_before = self.rows_seen_once;
        // Drag rows to reorder the queue (only while the list is sorted by queue position).
        let reorder = self.prefs.sort == Col::Queue && self.prefs.sort_asc;
        let mut drag_start: Option<InfoHash> = None;
        let mut drop_before: Option<Option<InfoHash>> = None;
        let pointer = ui.ctx().pointer_interact_pos();
        egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 0 }).show(ui, |ui| {
            let mut tb = TableBuilder::new(ui)
                .id_salt("torrents")
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
                    Column::remainder().at_least(180.0).clip(true)
                } else {
                    Column::initial(c.default_width()).at_least(32.0).clip(true)
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
                    Col::Name | Col::Queue | Col::Category | Col::Tags | Col::SavePath | Col::Tracker | Col::Eta
                );
            }
        }
        if header_menu {
            self.column_menu = ui.ctx().pointer_latest_pos();
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
                badge(ui, &status_text(t), color).on_hover_text(t.error.clone().unwrap_or_else(|| status_text(t)));
            }
            Col::Peers => {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if t.peers_seen > 0 {
                        ui.label(RichText::new(format!("/ {}", t.peers_seen)).small().monospace().color(p.text_mute));
                    }
                    ui.label(num(t.peers_live.to_string()));
                })
                .response
                .on_hover_text(t!(
                    "{live} connected · {connecting} connecting · {known} known\nA torrent with few seeders may only have a handful of connections.",
                    live = t.peers_live,
                    connecting = t.peers_connecting,
                    known = t.peers_seen
                ));
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
                    let downs: Vec<f32> = hist.iter().map(|(d, _)| *d as f32).collect();
                    let ups: Vec<f32> = hist.iter().map(|(_, u)| *u as f32).collect();
                    let max = downs.iter().chain(&ups).fold(1.0f32, |m, v| m.max(*v));
                    widgets::paint_series(ui.painter(), rect, &ups, p.success, Some(max), false);
                    widgets::paint_series(ui.painter(), rect, &downs, p.accent, Some(max), true);
                    resp.on_hover_text(t!(
                        "Last minute — down {down} · up {up}",
                        down = fmt_speed(t.down_bps, units),
                        up = fmt_speed(t.up_bps, units)
                    ));
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
                    RichText::new(t.trackers.first().map(|u| tracker_host(u)).unwrap_or_default())
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
            widgets::empty_state(
                ui,
                p,
                icons::MAGNIFYING_GLASS,
                t!("No matches"),
                &t!("No torrents match “{q}”.", q = self.search.trim()),
            );
            ui.vertical_centered(|ui| {
                if widgets::secondary_button(ui, t!("Clear search")).clicked() {
                    self.search.clear();
                }
            });
        } else {
            widgets::empty_state(
                ui,
                p,
                icons::FUNNEL,
                &t!("Nothing in “{f}”", f = self.prefs.status_filter.label()),
                t!("No torrents match the current filters."),
            );
            ui.vertical_centered(|ui| {
                if widgets::secondary_button(ui, t!("Show all torrents")).clicked() {
                    self.prefs.status_filter = super::StatusFilter::All;
                    self.prefs.category_filter = GroupFilter::All;
                    self.prefs.tag_filter = GroupFilter::All;
                    self.prefs.tracker_filter = GroupFilter::All;
                }
            });
        }
    }

    fn column_chooser(&mut self, ui: &mut Ui, p: &Palette) {
        let id = Id::new("column_chooser");
        let Some(pos) = self.column_menu else { return };
        let mut close = false;
        egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(RichText::new(t!("Columns")).strong().color(p.text));
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
                ui.separator();
                if ui.button(t!("Reset to defaults")).clicked() {
                    self.prefs.columns = super::UiPrefs::default().columns;
                }
                if ui.button(t!("Close")).clicked() {
                    close = true;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        let _ = vec2;
        if close {
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
        assert!(matches_filters(&a, &prefs, ""));
        assert!(!matches_filters(&b, &prefs, ""));
        let none = UiPrefs { category_filter: GroupFilter::None, ..Default::default() };
        assert!(matches_filters(&t(3, "c"), &none, ""));
        assert!(!matches_filters(&a, &none, ""));
    }

    #[test]
    fn tag_and_tracker_filters() {
        let mut a = t(1, "a");
        a.tags = vec!["linux".into()];
        a.trackers = vec!["udp://tracker.example.org:1337/announce".into()];
        let tag = UiPrefs { tag_filter: GroupFilter::Named("linux".into()), ..Default::default() };
        assert!(matches_filters(&a, &tag, ""));
        assert!(!matches_filters(&t(2, "b"), &tag, ""));
        let tr = UiPrefs { tracker_filter: GroupFilter::Named("tracker.example.org".into()), ..Default::default() };
        assert!(matches_filters(&a, &tr, ""));
        let none = UiPrefs { tracker_filter: GroupFilter::None, ..Default::default() };
        assert!(matches_filters(&t(2, "b"), &none, ""));
    }

    #[test]
    fn search_terms_all_must_match() {
        let mut a = t(1, "Ubuntu Server 24.04");
        a.tags = vec!["iso".into()];
        let p = UiPrefs::default();
        assert!(matches_filters(&a, &p, "ubuntu 24"));
        assert!(matches_filters(&a, &p, "ubuntu iso"), "tags match whole terms");
        assert!(!matches_filters(&a, &p, "ubuntu desktop"));
        assert!(matches_filters(&a, &p, "0101"), "info hash prefix");
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
    fn sorting_by_name_natural_and_size_desc() {
        let sn = snap(vec![t(1, "ep10"), t(2, "ep2"), t(3, "Ep1")]);
        let mut p = UiPrefs { sort: Col::Name, sort_asc: true, ..Default::default() };
        let rows = compute_rows(&sn, &p, "");
        let names: Vec<&str> = rows.iter().map(|&i| sn.torrents[i].name.as_str()).collect();
        assert_eq!(names, vec!["Ep1", "ep2", "ep10"]);
        p.sort = Col::Size;
        p.sort_asc = false;
        let rows = compute_rows(&sn, &p, "");
        assert_eq!(sn.torrents[rows[0]].size, 30);
    }

    #[test]
    fn eta_sort_puts_unknown_last() {
        let mut a = t(1, "a");
        a.eta = Some(Duration::from_secs(5));
        let b = t(2, "b");
        let sn = snap(vec![b, a]);
        let p = UiPrefs { sort: Col::Eta, sort_asc: true, ..Default::default() };
        let rows = compute_rows(&sn, &p, "");
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
