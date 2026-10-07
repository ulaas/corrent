//! The eframe application: UI state, event handling and frame layout.

mod actions;
mod details;
mod dialogs;
mod dnd;
mod extras;
pub mod icon;
mod integration_ui;
pub mod log;
mod settings_window;
mod sidebar;
mod statusbar;
mod table;
mod toasts;
mod toolbar;
mod tray;
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
}

impl Col {
    pub const ALL: [Col; 19] = [
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
            Col::Added | Col::Completed => 130.0,
            _ => 86.0,
        }
    }

    pub fn right_aligned(self) -> bool {
        matches!(
            self,
            Col::Size | Col::Down | Col::Up | Col::Eta | Col::Ratio | Col::Downloaded | Col::Uploaded | Col::Peers | Col::SeedTime
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
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            columns: vec![Col::Name, Col::Size, Col::Progress, Col::Status, Col::Peers, Col::Down, Col::Up, Col::Eta, Col::Ratio],
            sort: Col::Queue,
            sort_asc: true,
            show_sidebar: true,
            show_details: true,
            details_tab: DetailsTab::General,
            status_filter: StatusFilter::All,
            category_filter: GroupFilter::All,
            tag_filter: GroupFilter::All,
            tracker_filter: GroupFilter::All,
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
    pub first_run: bool,
    pub quitting: bool,
    applied_appearance: Option<crate::config::Appearance>,

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
        theme::install_fonts(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let prefs: UiPrefs = cc.storage.and_then(|s| eframe::get_value(s, "ui_prefs")).unwrap_or_default();
        let (tx, rx) = std::sync::mpsc::channel();
        let engine = EngineHandle::start(&rt, paths.clone(), settings.clone(), cc.egui_ctx.clone(), tx);
        if first_run {
            let _ = settings.save(&paths);
        }
        CorrentApp {
            paths,
            settings,
            engine,
            rt,
            events: rx,
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
            first_run,
            quitting: false,
            applied_appearance: None,
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
            extras: Default::default(),
            icon_tex: None,
            #[cfg(debug_assertions)]
            dev_done: false,
        }
        .init_integration()
        .with_tray(&cc.egui_ctx)
    }

    fn with_tray(mut self, ctx: &Context) -> Self {
        self.init_tray(ctx);
        self
    }

    pub fn palette(&self, ctx: &Context) -> Palette {
        theme::palette(ctx, &self.settings.appearance)
    }

    pub fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.paths) {
            self.toasts.push(ToastKind::Error, format!("Could not save settings: {e:#}"));
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
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                EngineEvent::Toast(kind, text) => self.toasts.push(kind, text),
                EngineEvent::Added { name, .. } => {
                    self.toasts.push(ToastKind::Success, format!("Added “{name}”"));
                    if self.settings.behavior.notify_on_add && self.window_hidden {
                        self.notify("Torrent added", &name);
                    }
                }
                EngineEvent::Completed { name, size, .. } => {
                    self.toasts.push(ToastKind::Success, format!("Download complete: “{name}”"));
                    if self.settings.behavior.notify_on_complete {
                        self.notify("Download complete", &format!("{name} ({})", crate::util::fmt_bytes(size)));
                    }
                }
                EngineEvent::Removed { hash } => {
                    self.selected.remove(&hash);
                }
                EngineEvent::PreviewReady { req, result } => self.on_preview_ready(req, result),
            }
        }
        let _ = ctx;
    }

    fn apply_appearance_if_changed(&mut self, ctx: &Context) {
        if self.applied_appearance.as_ref() != Some(&self.settings.appearance) {
            theme::apply(ctx, &self.settings.appearance);
            self.applied_appearance = Some(self.settings.appearance.clone());
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
        };
        self.engine.send(crate::engine::EngineCommand::Add(Box::new(req)));
    }

    pub fn pick_torrent_files(&mut self) {
        if let Some(files) = rfd::FileDialog::new().add_filter("Torrent files", &["torrent"]).pick_files() {
            for f in files {
                self.launch_queue.push_back(AddSource::File(f));
            }
        }
    }

    fn process_launch_queue(&mut self) {
        while let Some(src) = self.launch_queue.pop_front() {
            self.begin_add(src);
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
        self.drain_events(ctx);
        self.window_logic(ctx);
        self.process_launch_queue();
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_appearance_if_changed(&ctx);
        let p = self.palette(&ctx);

        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin::symmetric(10, 6)))
            .show_separator_line(true)
            .show(ui, |ui| self.toolbar(ui, &p));

        egui::Panel::bottom("statusbar")
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ui, |ui| self.status_bar(ui, &p));

        if self.prefs.show_sidebar {
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(220.0)
                .size_range(170.0..=360.0)
                .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 6 }))
                .show(ui, |ui| self.sidebar(ui, &p));
        }

        self.sync_detail_target();
        if self.prefs.show_details {
            egui::Panel::bottom("details")
                .resizable(true)
                .default_size(280.0)
                .size_range(160.0..=700.0)
                .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin { left: 14, right: 14, top: 6, bottom: 8 }))
                .show(ui, |ui| self.details_panel(ui, &p));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg0).inner_margin(egui::Margin::same(0)))
            .show(ui, |ui| {
                self.handler_banner(ui, &p);
                self.torrent_table(ui, &p);
            });

        self.drag_and_drop(&ctx, &p);
        self.extra_windows(&ctx, &p);
        self.dialogs(&ctx, &p);
        self.settings_window(&ctx, &p);
        self.shortcuts(&ctx);
        self.toasts.show(&ctx, &p);
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
                "magnet" => self.open_magnet_dialog(),
                "about" => self.extras.about = true,
                "log" => self.extras.log = true,
                "stats" => self.extras.stats = true,
                _ => {}
            }
        }
        if std::env::var("CORRENT_DEV_LIGHT").is_ok() {
            ctx.set_theme(egui::ThemePreference::Light);
        }
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
