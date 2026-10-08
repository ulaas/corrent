//! The eframe application: UI state, event handling and frame layout.

mod actions;
mod batch;
mod creator;
mod details;
mod dialogs;
mod dnd;
mod extras;
mod feeds;
pub mod icon;
mod import_dialog;
mod integration_ui;
pub mod log;
mod onboarding;
mod pickers;
mod remote;
mod search_view;
mod settings_window;
mod sidebar;
mod statusbar;
mod table;
mod titlebar;
mod toasts;
mod toolbar;
mod tray;
#[cfg(test)]
mod ui_tests;
mod when_done;
mod win_chrome;
mod window;

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Context, Ui};
use serde::{Deserialize, Serialize};

use crate::config::{Paths, Settings};
use crate::engine::types::{AddSource, InfoHash, Snapshot, TorrentState, TorrentView};
use crate::engine::{EngineEvent, EngineHandle, ToastKind};
use crate::ui::theme::{self, Palette};
use toasts::Toasts;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum StatusFilter {
    All,
    Downloading,
    Seeding,
    Completed,
    Paused,
    Active,
    Inactive,
    Stalled,
    Errored,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 9] = [
        StatusFilter::All,
        StatusFilter::Downloading,
        StatusFilter::Seeding,
        StatusFilter::Completed,
        StatusFilter::Paused,
        StatusFilter::Active,
        StatusFilter::Inactive,
        StatusFilter::Stalled,
        StatusFilter::Errored,
    ];

    pub fn matches(self, t: &TorrentView) -> bool {
        use TorrentState as S;
        match self {
            StatusFilter::All => true,
            StatusFilter::Downloading => {
                matches!(t.state, S::Downloading | S::Stalled | S::Metadata | S::Queued if t.completed_at.is_none())
                    || (t.state == S::Checking && t.completed_at.is_none())
            }
            StatusFilter::Seeding => t.state == S::Seeding,
            StatusFilter::Completed => t.completed_at.is_some() || t.state == S::Completed,
            StatusFilter::Paused => matches!(t.state, S::Paused | S::Completed),
            StatusFilter::Active => t.down_bps > 0 || t.up_bps > 0,
            StatusFilter::Inactive => t.down_bps == 0 && t.up_bps == 0,
            StatusFilter::Stalled => t.state == S::Stalled,
            StatusFilter::Errored => t.state == S::Error,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "All",
            StatusFilter::Downloading => "Downloading",
            StatusFilter::Seeding => "Seeding",
            StatusFilter::Completed => "Completed",
            StatusFilter::Paused => "Paused",
            StatusFilter::Active => "Active",
            StatusFilter::Inactive => "Inactive",
            StatusFilter::Stalled => "Stalled",
            StatusFilter::Errored => "Errored",
        }
    }

    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            StatusFilter::All => i::STACK,
            StatusFilter::Downloading => i::ARROW_DOWN,
            StatusFilter::Seeding => i::ARROW_UP,
            StatusFilter::Completed => i::CHECK_CIRCLE,
            StatusFilter::Paused => i::PAUSE,
            StatusFilter::Active => i::LIGHTNING,
            StatusFilter::Inactive => i::MOON,
            StatusFilter::Stalled => i::HOURGLASS_MEDIUM,
            StatusFilter::Errored => i::WARNING_CIRCLE,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum GroupFilter {
    All,
    None,
    Named(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Col {
    Queue,
    Name,
    Size,
    Progress,
    Status,
    Peers,
    Down,
    Up,
    Eta,
    Ratio,
    Category,
    Tags,
    Added,
    Completed,
    Downloaded,
    Uploaded,
    SavePath,
    Tracker,
    SeedTime,
    LastActivity,
    Activity,
}

impl Col {
    pub const ALL: [Col; 21] = [
        Col::Queue,
        Col::Name,
        Col::Size,
        Col::Progress,
        Col::Status,
        Col::Peers,
        Col::Down,
        Col::Up,
        Col::Eta,
        Col::Ratio,
        Col::Category,
        Col::Tags,
        Col::Added,
        Col::Completed,
        Col::Downloaded,
        Col::Uploaded,
        Col::SavePath,
        Col::Tracker,
        Col::SeedTime,
        Col::LastActivity,
        Col::Activity,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Col::Queue => "#",
            Col::Name => "Name",
            Col::Size => "Size",
            Col::Progress => "Progress",
            Col::Status => "Status",
            Col::Peers => "Peers",
            Col::Down => "Down",
            Col::Up => "Up",
            Col::Eta => "ETA",
            Col::Ratio => "Ratio",
            Col::Category => "Category",
            Col::Tags => "Tags",
            Col::Added => "Added",
            Col::Completed => "Completed",
            Col::Downloaded => "Downloaded",
            Col::Uploaded => "Uploaded",
            Col::SavePath => "Save path",
            Col::Tracker => "Tracker",
            Col::SeedTime => "Seeding time",
            Col::LastActivity => "Last activity",
            Col::Activity => "Activity",
        }
    }

    pub fn default_width(self) -> f32 {
        match self {
            Col::Queue => 36.0,
            Col::Name => 320.0,
            Col::Progress => 120.0,
            Col::Status => 110.0,
            Col::SavePath => 220.0,
            Col::Tracker => 160.0,
            Col::Tags | Col::Category => 110.0,
            Col::Added | Col::Completed | Col::LastActivity => 130.0,
            Col::Activity => 100.0,
            _ => 86.0,
        }
    }

    pub fn right_aligned(self) -> bool {
        matches!(
            self,
            Col::Size
                | Col::Down
                | Col::Up
                | Col::Eta
                | Col::Ratio
                | Col::Downloaded
                | Col::Uploaded
                | Col::Peers
                | Col::SeedTime
        )
    }
}

/// UI layout preferences persisted through eframe storage.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub columns: Vec<Col>,
    pub sort: Col,
    pub sort_asc: bool,
    pub show_sidebar: bool,
    pub show_details: bool,
    pub details_tab: DetailsTab,
    pub status_filter: StatusFilter,
    pub category_filter: GroupFilter,
    pub tag_filter: GroupFilter,
    pub tracker_filter: GroupFilter,
    /// Collapsed sidebar sections ("status", "categories", "tags").
    pub collapsed: Vec<String>,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            columns: vec![
                Col::Name,
                Col::Size,
                Col::Progress,
                Col::Status,
                Col::Peers,
                Col::Down,
                Col::Up,
                Col::Eta,
                Col::Ratio,
            ],
            sort: Col::Queue,
            sort_asc: true,
            show_sidebar: true,
            show_details: true,
            details_tab: DetailsTab::General,
            status_filter: StatusFilter::All,
            category_filter: GroupFilter::All,
            tag_filter: GroupFilter::All,
            tracker_filter: GroupFilter::All,
            collapsed: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetailsTab {
    General,
    Files,
    Peers,
    Trackers,
    Pieces,
    Speed,
}

pub struct CorrentApp {
    pub paths: Paths,
    pub settings: Settings,
    pub engine: EngineHandle,
    pub rt: tokio::runtime::Handle,
    events: std::sync::mpsc::Receiver<EngineEvent>,
    /// Lets background UI work report back through the engine event channel.
    ui_events_tx: std::sync::mpsc::Sender<EngineEvent>,
    pub snap: Arc<Snapshot>,
    pub prefs: UiPrefs,
    pub toasts: Toasts,

    // list state
    pub search: String,
    pub selected: HashSet<InfoHash>,
    pub anchor: Option<InfoHash>,
    pub focus: Option<InfoHash>,
    rows: Vec<usize>,
    rows_key: Option<u64>,
    pub scroll_to_focus: bool,
    pub column_menu: Option<egui::Pos2>,

    // launch / add
    pub launch_queue: VecDeque<AddSource>,
    pub modal: Option<dialogs::Modal>,
    pub add_dialogs: VecDeque<dialogs::AddDialog>,
    pub next_req: u64,
    pub settings_draft: Option<settings_window::SettingsDraft>,
    /// First-run welcome flow (shown when no settings file existed).
    pub onboarding: Option<onboarding::Onboarding>,
    pub quitting: bool,
    applied_appearance: Option<(crate::config::Appearance, bool)>,
    /// Appearance being edited in the settings window (previewed live).
    pub appearance_preview: Option<crate::config::Appearance>,

    // OS integration
    ipc: std::sync::mpsc::Receiver<crate::integration::single_instance::IpcMessage>,
    pub handler_status: Arc<std::sync::Mutex<Option<crate::integration::os_handler::HandlerStatus>>>,
    pub window_hidden: bool,
    frame_seen: bool,

    // sidebar & details
    counts: sidebar::Counts,
    sent_detail_target: Option<InfoHash>,
    pub file_filter: String,
    pub show_all_peers: bool,
    pub speed_scope_torrent: bool,
    tray: Option<tray::Tray>,
    tray_failed: bool,
    pub ctx: egui::Context,
    previous_run: crate::crash::PreviousRun,
    pub data_notes: Vec<String>,
    pub free_cache: Option<(String, std::time::Instant, Option<u64>)>,
    pub batch: Option<batch::BatchDialog>,
    pub creator: Option<creator::CreatorState>,
    pub import: Option<import_dialog::ImportDialog>,
    pub when_done: when_done::WhenDoneState,
    pub main_view: feeds::MainView,
    pub feeds: feeds::FeedsUi,
    pub torznab: search_view::SearchUi,
    pub remote: remote::RemoteCtl,
    row_first_seen: std::collections::HashMap<InfoHash, std::time::Instant>,
    rows_seen_once: bool,
    /// Rows being dragged to a new queue position.
    queue_drag: Option<Vec<InfoHash>>,
    /// Windows 11 draws a native rounded border, so ours is skipped.
    pub native_frame: bool,
    pub pickers: pickers::Pickers,
    pub extras: extras::Extras,
    icon_tex: Option<egui::TextureHandle>,
    #[cfg(debug_assertions)]
    dev_done: bool,
}

impl CorrentApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: tokio::runtime::Handle,
        paths: Paths,
        settings: Settings,
        first_run: bool,
        launch: Vec<AddSource>,
        ipc: std::sync::mpsc::Receiver<crate::integration::single_instance::IpcMessage>,
        start_hidden: bool,
    ) -> Self {
        let prefs: Option<UiPrefs> = cc.storage.and_then(|s| eframe::get_value(s, "ui_prefs"));
        Self::build(
            &cc.egui_ctx,
            prefs,
            rt,
            paths,
            settings,
            first_run,
            launch,
            ipc,
            start_hidden,
            win_chrome::round_corners(cc),
        )
        .init_integration()
        .with_tray(&cc.egui_ctx)
    }

    /// Construct the app state without OS integration (used by `new` and by UI tests).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        ctx: &Context,
        prefs: Option<UiPrefs>,
        rt: tokio::runtime::Handle,
        paths: Paths,
        settings: Settings,
        first_run: bool,
        launch: Vec<AddSource>,
        ipc: std::sync::mpsc::Receiver<crate::integration::single_instance::IpcMessage>,
        start_hidden: bool,
        native_frame: bool,
    ) -> Self {
        theme::install_fonts(ctx);
        egui_extras::install_image_loaders(ctx);
        let mut prefs = prefs.unwrap_or_default();
        prefs.tracker_filter = GroupFilter::All;
        let (tx, rx) = std::sync::mpsc::channel();
        let ui_events_tx = tx.clone();
        let engine = EngineHandle::start(&rt, paths.clone(), settings.clone(), ctx.clone(), tx);
        if first_run {
            let _ = settings.save(&paths);
        }
        let remote = remote::RemoteCtl::new(&settings);
        CorrentApp {
            paths,
            settings,
            engine,
            rt,
            events: rx,
            ui_events_tx,
            snap: Arc::new(Snapshot::default()),
            prefs,
            toasts: Toasts::default(),
            search: String::new(),
            selected: HashSet::new(),
            anchor: None,
            focus: None,
            rows: Vec::new(),
            rows_key: None,
            scroll_to_focus: false,
            column_menu: None,
            launch_queue: launch.into(),
            modal: None,
            add_dialogs: VecDeque::new(),
            next_req: 0,
            settings_draft: None,
            onboarding: first_run.then_some(onboarding::Onboarding { step: 0 }),
            quitting: false,
            applied_appearance: None,
            appearance_preview: None,
            ipc,
            handler_status: Arc::new(std::sync::Mutex::new(None)),
            window_hidden: start_hidden,
            frame_seen: false,
            counts: sidebar::Counts::default(),
            sent_detail_target: None,
            file_filter: String::new(),
            show_all_peers: false,
            speed_scope_torrent: true,
            tray: None,
            tray_failed: false,
            ctx: ctx.clone(),
            previous_run: Default::default(),
            data_notes: Vec::new(),
            free_cache: None,
            batch: None,
            creator: None,
            import: None,
            when_done: Default::default(),
            main_view: Default::default(),
            feeds: Default::default(),
            torznab: Default::default(),
            remote,
            row_first_seen: Default::default(),
            rows_seen_once: false,
            queue_drag: None,
            native_frame,
            pickers: Default::default(),
            extras: Default::default(),
            icon_tex: None,
            #[cfg(debug_assertions)]
            dev_done: false,
        }
    }

    pub fn with_data_notes(mut self, notes: Vec<String>) -> Self {
        self.data_notes = notes;
        self
    }

    /// Persistent notes about recovered/restored data.
    fn data_banner(&mut self, ui: &mut Ui, p: &Palette) {
        let mut dismiss = None;
        for (i, note) in self.data_notes.iter().enumerate() {
            egui::Frame::new()
                .fill(p.warn.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0, p.warn.gamma_multiply(0.5)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(14, 10))
                .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(egui_phosphor::regular::FLOPPY_DISK).size(20.0).color(p.warn));
                        let w = (ui.available_width() - 230.0).max(120.0);
                        ui.allocate_ui(egui::vec2(w, 0.0), |ui| {
                            ui.add(egui::Label::new(egui::RichText::new(note).color(p.text)).wrap());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::ui::widgets::secondary_button(ui, t!("Dismiss")).clicked() {
                                dismiss = Some(i);
                            }
                            if crate::ui::widgets::secondary_button(ui, t!("Open folder")).clicked() {
                                open_path(&self.paths.data_dir);
                            }
                        });
                    });
                });
        }
        if let Some(i) = dismiss {
            self.data_notes.remove(i);
        }
    }

    pub fn export_backup(&mut self, dest: std::path::PathBuf) {
        let paths = self.paths.clone();
        let tx = self.ui_events_tx.clone();
        let ctx = self.ctx.clone();
        self.toasts.push(ToastKind::Info, t!("Backing up…"));
        self.rt.spawn_blocking(move || {
            let msg = match crate::backup::export(&paths, &dest) {
                Ok(n) => (ToastKind::Success, format!("Backed up {n} files to {}", dest.display())),
                Err(e) => (ToastKind::Error, format!("Backup failed: {e:#}")),
            };
            let _ = tx.send(EngineEvent::Toast(msg.0, msg.1));
            ctx.request_repaint();
        });
    }

    pub fn import_backup(&mut self, src: std::path::PathBuf) {
        match crate::backup::stage_import(&self.paths, &src) {
            Ok(parts) => self.data_notes.push(format!(
                "The backup ({}) will be restored the next time Corrent starts. Quit and reopen Corrent to finish.",
                parts.join(", ")
            )),
            Err(e) => self.toasts.push(ToastKind::Error, format!("Can't restore: {e:#}")),
        }
    }

    pub fn with_previous_run(mut self, p: crate::crash::PreviousRun) -> Self {
        self.previous_run = p;
        self
    }

    /// Shown when the engine task stopped unexpectedly; offers a restart.
    fn engine_banner(&mut self, ui: &mut Ui, p: &Palette) {
        if self.engine.is_alive() || self.quitting {
            return;
        }
        egui::Frame::new()
            .fill(p.error.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, p.error.gamma_multiply(0.5)))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(egui_phosphor::regular::WARNING_OCTAGON).size(20.0).color(p.error));
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(t!("The torrent engine stopped unexpectedly")).strong().color(p.text),
                        );
                        ui.label(
                            egui::RichText::new(
                                "A crash report was saved. Restart the engine to continue; actions you take now are \
                                 applied after the restart.",
                            )
                            .small()
                            .color(p.text_dim),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::ui::widgets::primary_button(ui, p, t!("Restart engine")).clicked() {
                            self.engine.restart(self.settings.clone());
                            crate::crash::dismiss(&self.paths.log_dir());
                        }
                        if let Some(r) = crate::crash::reports(&self.paths.log_dir()).first().cloned()
                            && crate::ui::widgets::secondary_button(ui, t!("View report")).clicked()
                        {
                            open_path(&r);
                        }
                    });
                });
            });
        // Keep checking even when nothing else repaints.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }

    /// "Closed unexpectedly" banner shown after an unclean exit.
    fn crash_banner(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.previous_run.unclean {
            return;
        }
        let report = self.previous_run.report.clone();
        egui::Frame::new()
            .fill(p.warn.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, p.warn.gamma_multiply(0.5)))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(egui_phosphor::regular::WARNING).size(20.0).color(p.warn));
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(t!("Corrent closed unexpectedly last time")).strong().color(p.text),
                        );
                        let detail = if report.is_some() {
                            "A crash report was saved. Your torrents and settings were restored."
                        } else {
                            "It may have been ended by the system or another program. Your torrents were restored."
                        };
                        ui.label(egui::RichText::new(detail).small().color(p.text_dim));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::ui::widgets::secondary_button(ui, t!("Dismiss")).clicked() {
                            self.previous_run.unclean = false;
                            crate::crash::dismiss(&self.paths.log_dir());
                        }
                        if let Some(r) = &report
                            && crate::ui::widgets::secondary_button(ui, t!("View report")).clicked()
                        {
                            open_path(r);
                        }
                    });
                });
            });
    }

    fn with_tray(mut self, ctx: &Context) -> Self {
        self.init_tray(ctx);
        self
    }

    pub fn palette(&self, ctx: &Context) -> Palette {
        theme::palette(ctx, self.appearance_preview.as_ref().unwrap_or(&self.settings.appearance))
    }

    pub fn save_settings(&mut self) {
        self.sync_remote();
        if let Err(e) = self.settings.save(&self.paths) {
            let msg = format!("Your settings couldn't be saved: {}", crate::util::explain_io(&format!("{e:#}")));
            if !self.data_notes.contains(&msg) {
                self.data_notes.push(msg);
            }
        }
    }

    /// Torrents currently visible (filtered and sorted), as indices into `snap.torrents`.
    pub fn visible_rows(&mut self) -> &[usize] {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.snap.generation.hash(&mut h);
        self.search.hash(&mut h);
        self.prefs.sort.hash(&mut h);
        self.prefs.sort_asc.hash(&mut h);
        self.prefs.status_filter.hash(&mut h);
        self.prefs.category_filter.hash(&mut h);
        self.prefs.tag_filter.hash(&mut h);
        self.prefs.tracker_filter.hash(&mut h);
        let key = h.finish();
        if self.rows_key != Some(key) {
            self.rows = table::compute_rows(&self.snap, &self.prefs, &self.search);
            self.rows_key = Some(key);
            // Never act on torrents the user can't see.
            let visible: HashSet<InfoHash> = self.rows.iter().map(|&i| self.snap.torrents[i].hash).collect();
            self.selected.retain(|h| visible.contains(h));
            if self.anchor.is_some_and(|a| !visible.contains(&a)) {
                self.anchor = None;
            }
            if self.focus.is_some_and(|f| !visible.contains(&f)) {
                self.focus = None;
            }
        }
        &self.rows
    }

    pub fn selected_views(&self) -> Vec<&TorrentView> {
        self.snap.torrents.iter().filter(|t| self.selected.contains(&t.hash) && !t.pending).collect()
    }

    pub fn selected_hashes(&self) -> Vec<InfoHash> {
        self.selected_views().iter().map(|t| t.hash).collect()
    }

    fn drain_events(&mut self, ctx: &Context) {
        let mut removed: Vec<(InfoHash, String)> = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                EngineEvent::Toast(kind, text) => self.toasts.push(kind, text),
                EngineEvent::Added { name, hash } => {
                    self.toasts.push_with(
                        ToastKind::Success,
                        t!("Added “{name}”", name = name),
                        vec![("Show".into(), toasts::ToastAction::Show(hash))],
                    );
                    if self.settings.behavior.notify_on_add && self.window_hidden {
                        self.notify("Torrent added", &name);
                    }
                }
                EngineEvent::Completed { name, size, content_path, save_path, .. } => {
                    self.toasts.push_with(
                        ToastKind::Success,
                        t!("Download complete: “{name}”", name = name),
                        vec![
                            ("Open".into(), toasts::ToastAction::OpenFile(content_path)),
                            ("Show in folder".into(), toasts::ToastAction::OpenFolder(save_path)),
                        ],
                    );
                    if self.settings.behavior.notify_on_complete {
                        self.notify("Download complete", &format!("{name} ({})", crate::util::fmt_bytes(size)));
                    }
                }
                EngineEvent::DataWarning(w) => self.data_notes.push(w),
                EngineEvent::Created { id, result } => {
                    // Ignore answers for a dialog that was closed (or a newer request).
                    let current = self.creator.as_ref().is_some_and(|c| c.request == Some(id));
                    if let Some(c) = self.creator.as_mut().filter(|_| current) {
                        c.busy = false;
                        c.request = None;
                    }
                    match result {
                        Ok((name, path, magnet, seeding)) => {
                            if current {
                                self.creator = None;
                            }
                            let folder = path.parent().map(std::path::PathBuf::from).unwrap_or_default();
                            let mut actions = vec![
                                ("Show file".into(), toasts::ToastAction::OpenFolder(folder)),
                                ("Copy magnet".into(), toasts::ToastAction::Copy(magnet)),
                            ];
                            if let Some(h) = seeding {
                                actions.push(("Show".into(), toasts::ToastAction::Show(h)));
                            }
                            self.toasts.push_with(ToastKind::Success, t!("Created “{name}”", name = name), actions);
                        }
                        Err(e) => self.toasts.push(ToastKind::Error, t!("Couldn't create the torrent: {e}", e = e)),
                    }
                }
                EngineEvent::Removed { hash, name, undoable } => {
                    self.selected.remove(&hash);
                    if undoable {
                        removed.push((hash, name));
                    }
                }
                EngineEvent::Restored { hash, name } => {
                    self.toasts.push_with(
                        ToastKind::Success,
                        t!("Restored “{name}”", name = name),
                        vec![("Show".into(), toasts::ToastAction::Show(hash))],
                    );
                }
                EngineEvent::PreviewReady { req, result } => self.on_preview_ready(req, result),
            }
        }
        #[cfg(debug_assertions)]
        if !removed.is_empty() && std::env::var("CORRENT_DEV_OPEN").as_deref() == Ok("remove-undo") {
            self.engine.send(crate::engine::EngineCommand::UndoRemove(removed.iter().map(|(h, _)| *h).collect()));
        }
        // One toast (with Undo) per burst of removals.
        if !removed.is_empty() {
            let text = if removed.len() == 1 {
                format!("Removed “{}”", removed[0].1)
            } else {
                format!("Removed {} torrents", removed.len())
            };
            let hashes = removed.into_iter().map(|(h, _)| h).collect();
            self.toasts.push_with(ToastKind::Info, text, vec![("Undo".into(), toasts::ToastAction::Undo(hashes))]);
        }
        let _ = ctx;
    }

    fn run_toast_actions(&mut self, ctx: &Context, actions: Vec<toasts::ToastAction>) {
        for a in actions {
            match a {
                toasts::ToastAction::Undo(h) => self.engine.send(crate::engine::EngineCommand::UndoRemove(h)),
                toasts::ToastAction::OpenFolder(p) | toasts::ToastAction::OpenFile(p) => open_path(&p),
                toasts::ToastAction::Copy(text) => self.copy(text),
                toasts::ToastAction::OpenUrl(url) => {
                    if url.starts_with("https://") {
                        let _ = open::that_detached(url);
                    }
                }
                toasts::ToastAction::SkipVersion(v) => {
                    self.settings.updates.skip_version = v;
                    self.save_settings();
                }
                toasts::ToastAction::Show(h) => {
                    self.show_window(ctx);
                    self.prefs.status_filter = StatusFilter::All;
                    self.prefs.category_filter = GroupFilter::All;
                    self.prefs.tag_filter = GroupFilter::All;
                    self.search.clear();
                    self.selected.clear();
                    self.selected.insert(h);
                    self.focus = Some(h);
                    self.anchor = Some(h);
                    self.scroll_to_focus = true;
                }
            }
        }
    }

    fn apply_appearance_if_changed(&mut self, ctx: &Context) {
        let a = self.appearance_preview.as_ref().unwrap_or(&self.settings.appearance);
        // Also re-apply when the OS high-contrast mode is switched.
        let key = (a.clone(), a.follow_os_high_contrast && theme::os_high_contrast());
        if self.applied_appearance.as_ref() != Some(&key) {
            theme::apply(ctx, a);
            self.applied_appearance = Some(key);
        }
    }

    /// Add a source: either opens the add dialog or adds directly with defaults.
    pub fn add_source(&mut self, source: AddSource) {
        let req = crate::engine::AddRequest {
            source,
            torrent_bytes: None,
            save_path: self.settings.downloads.save_path.clone(),
            create_subfolder: self.settings.downloads.create_subfolder,
            category: None,
            tags: Vec::new(),
            paused: self.settings.downloads.start_paused,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
        };
        self.engine.send(crate::engine::EngineCommand::Add(Box::new(req)));
    }

    pub fn pick_torrent_files(&mut self) {
        self.pick(pickers::PickTarget::AddTorrents, pickers::PickKind::TorrentFiles, None);
    }

    fn process_launch_queue(&mut self) {
        if self.launch_queue.is_empty() {
            return;
        }
        if !self.settings.behavior.skip_add_dialog && !self.add_dialogs.is_empty() && self.batch.is_none() {
            // Wait for the open add dialog; a batch modal would stack behind it.
            return;
        }
        let sources: Vec<AddSource> = self.launch_queue.drain(..).collect();
        if self.settings.behavior.skip_add_dialog {
            for src in sources {
                self.add_source(src);
            }
        } else if sources.len() == 1 && self.batch.is_none() {
            self.begin_add(sources.into_iter().next().unwrap());
        } else {
            self.begin_batch(sources);
        }
    }
}

impl eframe::App for CorrentApp {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        if !self.frame_seen {
            crate::integration::macos_events::reinstall();
        }
        self.frame_seen = true;
        self.snap = self.engine.snapshot();
        #[cfg(debug_assertions)]
        self.dev_hooks(ctx);
        self.drain_ipc(ctx);
        self.drain_pickers();
        self.drain_events(ctx);
        self.window_logic(ctx);
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        self.process_launch_queue();
        self.poll_remote();
        self.poll_updates();
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame_ui(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "ui_prefs", &self.prefs);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.engine.shutdown_blocking(std::time::Duration::from_secs(8));
    }
}

#[cfg(debug_assertions)]
impl CorrentApp {
    /// Debug-only hooks for screenshot testing: CORRENT_DEV_SELECT=<n> selects the n-th row,
    /// CORRENT_DEV_TAB=<files|peers|trackers|pieces|speed> picks the details tab,
    /// CORRENT_DEV_OPEN=<settings|magnet|about|log> opens a window.
    fn dev_hooks(&mut self, ctx: &Context) {
        if self.dev_done || self.snap.torrents.is_empty() {
            return;
        }
        self.dev_done = true;
        if let Ok(n) = std::env::var("CORRENT_DEV_SELECT").map(|v| v.parse::<usize>().unwrap_or(0)) {
            let rows = self.visible_rows().to_vec();
            if let Some(&i) = rows.get(n) {
                let h = self.snap.torrents[i].hash;
                self.selected.insert(h);
                self.focus = Some(h);
            }
        }
        if let Ok(tab) = std::env::var("CORRENT_DEV_TAB") {
            self.prefs.details_tab = match tab.as_str() {
                "files" => DetailsTab::Files,
                "peers" => DetailsTab::Peers,
                "trackers" => DetailsTab::Trackers,
                "pieces" => DetailsTab::Pieces,
                "speed" => DetailsTab::Speed,
                _ => DetailsTab::General,
            };
        }
        if let Ok(w) = std::env::var("CORRENT_DEV_OPEN") {
            match w.as_str() {
                "settings" => self.open_settings(),
                "remote" => self.open_settings_at(settings_window::Page::Remote),
                "magnet" => self.open_magnet_dialog(),
                "about" => self.extras.about = true,
                "creator" => self.open_creator(),
                "import" => self.open_import(),
                "feeds" => self.main_view = feeds::MainView::Feeds,
                "search" => self.main_view = feeds::MainView::Search,
                "rules" => {
                    self.main_view = feeds::MainView::Feeds;
                    self.feeds.rules = Some(feeds::RulesEditor { rules: self.settings.rss.rules.clone(), selected: 0 });
                }
                "log" => self.extras.log = true,
                "stats" => self.extras.stats = true,
                "remove" | "remove-undo" => {
                    let h = self.snap.torrents[0].hash;
                    self.engine.send(crate::engine::EngineCommand::Delete { hashes: vec![h], with_data: false });
                }
                _ => {}
            }
        }
        match std::env::var("CORRENT_DEV_PANIC").as_deref() {
            Ok("ui") => panic!("CORRENT_DEV_PANIC=ui"),
            Ok("engine") => self.engine.send(crate::engine::EngineCommand::DevPanic),
            _ => {}
        }
        if let Ok(q) = std::env::var("CORRENT_DEV_SEARCH") {
            self.main_view = feeds::MainView::Search;
            self.torznab.query = q;
            self.dev_start_search();
        }
        if std::env::var("CORRENT_DEV_LIGHT").is_ok() {
            ctx.set_theme(egui::ThemePreference::Light);
        }
    }
}

impl CorrentApp {
    /// Lay out one frame of the whole window (eframe `ui` and UI tests).
    pub(crate) fn frame_ui(&mut self, ui: &mut Ui) {
        // A context without our fonts (e.g. a test harness) can't lay out bold text yet:
        // install them and draw from the next frame on.
        if !theme::fonts_installed(ui.ctx()) {
            theme::install_fonts(ui.ctx());
            ui.ctx().request_repaint();
            return;
        }
        let ctx = ui.ctx().clone();
        self.apply_appearance_if_changed(&ctx);
        let p = self.palette(&ctx);

        egui::Panel::top("toolbar")
            .exact_size(titlebar::TITLEBAR_HEIGHT)
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }))
            .show_separator_line(true)
            .show(ui, |ui| self.toolbar(ui, &p));

        egui::Panel::bottom("statusbar")
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ui, |ui| self.status_bar(ui, &p));

        let mut show_sidebar = self.prefs.show_sidebar;
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(220.0)
            .size_range(170.0..=360.0)
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 6 }))
            .show_collapsible(ui, &mut show_sidebar, |ui| self.sidebar(ui, &p));
        self.prefs.show_sidebar = show_sidebar;

        self.sync_detail_target();
        let torrents_view = self.main_view == feeds::MainView::Torrents;
        let mut show_details = self.prefs.show_details && torrents_view;
        egui::Panel::bottom("details")
            .resizable(true)
            .default_size(280.0)
            .size_range(160.0..=700.0)
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin { left: 14, right: 14, top: 6, bottom: 8 }))
            .show_collapsible(ui, &mut show_details, |ui| self.details_panel(ui, &p));
        if torrents_view {
            self.prefs.show_details = show_details;
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg0).inner_margin(egui::Margin::same(0))).show(
            ui,
            |ui| {
                self.engine_banner(ui, &p);
                self.crash_banner(ui, &p);
                self.data_banner(ui, &p);
                self.handler_banner(ui, &p);
                match self.main_view {
                    feeds::MainView::Torrents => self.torrent_table(ui, &p),
                    feeds::MainView::Feeds => self.feeds_view(ui, &p),
                    feeds::MainView::Search => self.search_view(ui, &p),
                }
            },
        );

        self.drag_and_drop(&ctx, &p);
        self.window_frame(&ctx, &p);
        self.extra_windows(&ctx, &p);
        self.onboarding(&ctx, &p);
        self.creator_dialog(&ctx, &p);
        self.rules_dialog(&ctx, &p);
        self.import_dialog(&ctx, &p);
        self.when_done_logic(&ctx);
        self.when_done_dialog(&ctx, &p);
        self.batch_dialog(&ctx, &p);
        self.dialogs(&ctx, &p);
        self.settings_window(&ctx, &p);
        self.shortcuts(&ctx);
        let actions = self.toasts.show(&ctx, &p);
        self.run_toast_actions(&ctx, actions);
    }
}

pub fn open_path(p: &std::path::Path) {
    let target: PathBuf = p.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = open::that_detached(&target) {
            tracing::warn!("could not open {}: {e}", target.display());
        }
    });
}
