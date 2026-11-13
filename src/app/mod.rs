//! The eframe application: UI state, event handling and frame layout.

mod actions;
mod batch;
pub mod connect;
mod creator;
#[cfg(debug_assertions)]
mod demo;
mod details;
mod diagnostics_ui;
mod dialogs;
mod discover_ui;
mod dnd;
mod extras;
mod feeds;
mod get_mpv;
pub mod icon;
mod import_dialog;
mod integration_ui;
pub mod log;
pub mod media;
mod notify;
mod onboarding;
mod palette;
mod pickers;
mod remote;
mod search_view;
mod server_fs;
mod settings_window;
mod sidebar;
mod statusbar;
mod streams;
mod table;
mod titlebar;
mod toasts;
mod toolbar;
mod tray;
mod tv;
#[cfg(test)]
mod ui_tests;
mod update_ui;
mod web_account;
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
            StatusFilter::All => t!("All"),
            StatusFilter::Downloading => t!("Downloading"),
            StatusFilter::Seeding => t!("Seeding"),
            StatusFilter::Completed => t!("Completed"),
            StatusFilter::Paused => t!("Paused"),
            StatusFilter::Active => t!("Active"),
            StatusFilter::Inactive => t!("Inactive"),
            StatusFilter::Stalled => t!("Stalled"),
            StatusFilter::Errored => t!("Errored"),
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
    Error,
}

impl Col {
    pub const ALL: [Col; 22] = [
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
        Col::Error,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Col::Queue => "#",
            Col::Name => t!("Name"),
            Col::Size => t!("Size"),
            Col::Progress => t!("Progress"),
            Col::Status => t!("Status"),
            Col::Peers => t!("Peers"),
            Col::Down => t!("Down"),
            Col::Up => t!("Up"),
            Col::Eta => t!("ETA"),
            Col::Ratio => t!("Ratio"),
            Col::Category => t!("Category"),
            Col::Tags => t!("Tags"),
            Col::Added => t!("Added"),
            Col::Completed => t!("Completed"),
            Col::Downloaded => t!("Downloaded"),
            Col::Uploaded => t!("Uploaded"),
            Col::SavePath => t!("Save path"),
            Col::Tracker => t!("Tracker"),
            Col::SeedTime => t!("Seeding time"),
            Col::LastActivity => t!("Last activity"),
            Col::Activity => t!("Activity"),
            Col::Error => t!("Error"),
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
            Col::Error => 200.0,
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
    /// Collapsed sidebar sections ("status", "categories", "tags", "trackers").
    pub collapsed: Vec<String>,
    // What the views were showing (copied from them when saved, see `sync_prefs`).
    pub speed_scope_torrent: bool,
    pub show_all_peers: bool,
    pub file_filter: String,
    pub feeds_selected: Option<String>,
    pub search_sort: search_view::SortBy,
    pub search_asc: bool,
    /// Named filter and sort combinations in the sidebar.
    pub views: Vec<SavedView>,
    /// Column widths from "Fit columns".
    pub column_widths: Vec<(Col, f32)>,
    /// Counts "Fit columns": a new table state, so the widths above apply.
    pub columns_fitted: u32,
}

/// A saved view: what the list is filtered and sorted by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    pub name: String,
    pub status: StatusFilter,
    pub category: GroupFilter,
    pub tag: GroupFilter,
    pub tracker: GroupFilter,
    pub search: String,
    pub sort: Col,
    pub sort_asc: bool,
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
            speed_scope_torrent: true,
            show_all_peers: false,
            file_filter: String::new(),
            feeds_selected: None,
            search_sort: search_view::SortBy::Seeders,
            search_asc: false,
            views: Vec::new(),
            column_widths: Vec::new(),
            columns_fitted: 0,
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

/// Free space per folder: (measured at, bytes).
pub type FreeSpaceCache = std::collections::HashMap<String, (std::time::Instant, Option<u64>)>;

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
    sort_memo: table::SortMemo,
    pub scroll_to_focus: bool,
    /// The focus was last moved with the keyboard: the focused row is marked.
    focus_visible: bool,
    /// How many rows fit in the list (Page Up/Down).
    page_rows: usize,
    pub column_menu: Option<egui::Pos2>,
    /// "Fit columns" was asked for: measured on the next frame.
    fit_columns: bool,
    /// A widget had the keyboard focus at the end of the last frame.
    had_focus: bool,

    // launch / add
    pub launch_queue: VecDeque<AddSource>,
    /// Dropped files read off the UI thread: (what to add, how many were ignored).
    dropped_tx: std::sync::mpsc::Sender<(Vec<AddSource>, usize)>,
    dropped_rx: std::sync::mpsc::Receiver<(Vec<AddSource>, usize)>,
    /// The clipboard read for the magnet dialog, and when it was asked.
    clipboard: Option<(std::sync::mpsc::Receiver<String>, std::time::Instant)>,
    /// Ctrl+K, while open.
    palette: Option<palette::CommandPalette>,
    pub modal: Option<dialogs::Modal>,
    /// The open modal's text field has been given the focus.
    modal_focused: bool,
    pub add_dialogs: VecDeque<dialogs::AddDialog>,
    pub next_req: u64,
    pub settings_draft: Option<settings_window::SettingsDraft>,
    /// First-run welcome flow (shown when no settings file existed).
    pub onboarding: Option<onboarding::Onboarding>,
    pub quitting: bool,
    /// "When done → Shut down": run once the engine has stopped (see `on_exit`).
    pub power_after_exit: Option<crate::integration::power::WhenDone>,
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
    /// The Files tab's tree.
    file_tree: crate::ui::file_tree::TreeState,
    /// Playable files of the torrent in the details data: (hash, files, files selected), list.
    media_files_cache: Option<actions::MediaFilesCache>,
    pub show_all_peers: bool,
    pub speed_scope_torrent: bool,
    tray: Option<tray::Tray>,
    tray_failed: bool,
    pub ctx: egui::Context,
    previous_run: crate::crash::PreviousRun,
    /// Task panics already told about (E7).
    task_panics_seen: u64,
    task_panic_told: Option<std::time::Instant>,
    /// ⋮ → Save diagnostics… while open (O4).
    diagnostics: Option<diagnostics_ui::DiagnosticsDialog>,
    /// Getting mpv for playback (Windows: a download), while open.
    get_mpv: Option<get_mpv::GetMpvDialog>,
    pub data_notes: Vec<String>,
    /// Free space per folder typed in a dialog: (measured at, bytes), filled in the background.
    pub free_cache: Arc<std::sync::Mutex<FreeSpaceCache>>,
    free_inflight: Arc<std::sync::Mutex<HashSet<String>>>,
    pub batch: Option<batch::BatchDialog>,
    pub creator: Option<creator::CreatorState>,
    pub import: Option<import_dialog::ImportDialog>,
    pub when_done: when_done::WhenDoneState,
    /// Update checks, the release that was found and "Update and restart".
    pub update: update_ui::UpdateState,
    /// Newest crash report while the engine is down (looked up once).
    engine_report: Option<Option<std::path::PathBuf>>,
    /// Generation of the latest engine snapshot and when it arrived (stall detection).
    last_snapshot: (u64, std::time::Instant),
    /// Commands the engine refused so far (its queue was full), to tell the user once per burst.
    refused_seen: u64,
    /// Mica spike (B5b, debug only): translucent backgrounds over the DWM backdrop.
    mica: bool,
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
    /// The fullscreen state the native frame was last set up for.
    chrome_fullscreen: bool,
    pub pickers: pickers::Pickers,
    pub extras: extras::Extras,
    icon_tex: Option<egui::TextureHandle>,
    #[cfg(debug_assertions)]
    dev_done: bool,
    #[cfg(debug_assertions)]
    media_dev_done: bool,
    #[cfg(debug_assertions)]
    media_dev_played: bool,
    #[cfg(debug_assertions)]
    media_dev_shot_sent: bool,
    #[cfg(debug_assertions)]
    media_dev_started: Option<std::time::Instant>,
    #[cfg(debug_assertions)]
    media_dev_pair: Option<media::client::Slot<crate::link::ServerEntry>>,
    #[cfg(debug_assertions)]
    media_dev_profiles: bool,
    /// CORRENT_DEV_LIBRARY: libraries being set up, then shown.
    #[cfg(debug_assertions)]
    media_dev_library: Option<media::client::Slot<usize>>,
    #[cfg(debug_assertions)]
    media_dev_library_sent: bool,
    /// CORRENT_DEV_PAUSE=<s>: when the hooked playback was paused (once).
    #[cfg(debug_assertions)]
    media_dev_paused: bool,
    pub media: media::MediaState,
    pub tv: tv::TvState,
    pub web_account: Option<web_account::WebAccount>,
    server_fs: Option<server_fs::ServerFs>,
    /// The server used instead of the local engine (None: local).
    pub server: Option<connect::ServerLink>,
    pub connect: Option<connect::ConnectDialog>,
    /// Corrent servers on this network.
    discover: discover_ui::DiscoverState,
    /// Saved servers (`servers.toml`).
    pub servers: crate::link::Servers,
    /// Switching back to the local engine once the previous local session has stopped.
    pending_local: bool,
    /// A local engine stopping after a switch to a server (joined before starting a new one).
    local_stopping: Option<std::thread::JoinHandle<()>>,
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
        .with_media_gl(cc)
        .with_mica_spike(cc)
        .with_end_session(cc)
        .init_integration()
        .with_tray(&cc.egui_ctx)
    }

    /// Save and stop when the OS ends the session (see `integration::end_session`).
    fn with_end_session(self, cc: &eframe::CreationContext<'_>) -> Self {
        crate::integration::end_session::track(&self.engine, &self.paths);
        #[cfg(windows)]
        crate::integration::end_session::install(cc);
        #[cfg(not(windows))]
        let _ = cc;
        self
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
        let prefs = prefs.unwrap_or_default();
        let (tx, rx) = std::sync::mpsc::channel();
        let ui_events_tx = tx.clone();
        // Read once at startup (a small file); later changes are kept in memory and saved off
        // the UI thread.
        let servers = crate::link::Servers::load(&paths);
        let (engine, server) = connect::start_engine(
            &rt,
            &paths,
            &settings,
            {
                let ctx = ctx.clone();
                crate::engine::Wake::new(move || ctx.request_repaint())
            },
            tx,
            servers.active_entry().cloned(),
        );
        if first_run {
            let _ = settings.save(&paths);
        }
        let remote = remote::RemoteCtl::new(&settings);
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let feeds = feeds::FeedsUi { selected: prefs.feeds_selected.clone(), ..Default::default() };
        let mut torznab = search_view::SearchUi::default();
        (torznab.sort, torznab.asc) = (prefs.search_sort, prefs.search_asc);
        CorrentApp {
            paths,
            settings,
            engine,
            rt,
            events: rx,
            ui_events_tx,
            snap: Arc::new(Snapshot::default()),
            toasts: Toasts::default(),
            search: String::new(),
            selected: HashSet::new(),
            anchor: None,
            focus: None,
            rows: Vec::new(),
            rows_key: None,
            sort_memo: Default::default(),
            scroll_to_focus: false,
            focus_visible: false,
            page_rows: 10,
            column_menu: None,
            fit_columns: false,
            had_focus: false,
            launch_queue: launch.into(),
            dropped_tx,
            dropped_rx,
            clipboard: None,
            palette: None,
            modal: None,
            modal_focused: false,
            add_dialogs: VecDeque::new(),
            next_req: 0,
            settings_draft: None,
            onboarding: first_run.then_some(onboarding::Onboarding { step: 0 }),
            quitting: false,
            power_after_exit: None,
            applied_appearance: None,
            appearance_preview: None,
            ipc,
            handler_status: Arc::new(std::sync::Mutex::new(None)),
            window_hidden: start_hidden,
            frame_seen: false,
            counts: sidebar::Counts::default(),
            sent_detail_target: None,
            file_filter: prefs.file_filter.clone(),
            file_tree: Default::default(),
            media_files_cache: None,
            show_all_peers: prefs.show_all_peers,
            speed_scope_torrent: prefs.speed_scope_torrent,
            tray: None,
            tray_failed: false,
            ctx: ctx.clone(),
            previous_run: Default::default(),
            task_panics_seen: crate::crash::task_panics().0,
            task_panic_told: None,
            diagnostics: None,
            get_mpv: None,
            data_notes: Vec::new(),
            free_cache: Default::default(),
            free_inflight: Default::default(),
            batch: None,
            creator: None,
            import: None,
            when_done: Default::default(),
            update: Default::default(),
            engine_report: None,
            last_snapshot: (0, std::time::Instant::now()),
            refused_seen: 0,
            mica: false,
            main_view: Default::default(),
            feeds,
            torznab,
            remote,
            row_first_seen: Default::default(),
            rows_seen_once: false,
            queue_drag: None,
            native_frame,
            chrome_fullscreen: false,
            pickers: Default::default(),
            extras: Default::default(),
            icon_tex: None,
            #[cfg(debug_assertions)]
            dev_done: false,
            #[cfg(debug_assertions)]
            media_dev_done: false,
            #[cfg(debug_assertions)]
            media_dev_played: false,
            #[cfg(debug_assertions)]
            media_dev_shot_sent: false,
            #[cfg(debug_assertions)]
            media_dev_started: None,
            media: Default::default(),
            server,
            tv: Default::default(),
            web_account: None,
            server_fs: None,
            servers,
            pending_local: false,
            connect: None,
            discover: Default::default(),
            #[cfg(debug_assertions)]
            media_dev_pair: None,
            #[cfg(debug_assertions)]
            media_dev_profiles: false,
            #[cfg(debug_assertions)]
            media_dev_library: None,
            #[cfg(debug_assertions)]
            media_dev_library_sent: false,
            #[cfg(debug_assertions)]
            media_dev_paused: false,
            local_stopping: None,
            prefs,
        }
    }

    /// The list as it is now, under this name (replacing a view of that name).
    pub(super) fn save_view(&mut self, name: &str) {
        let view = self.current_view(name.trim());
        match self.prefs.views.iter_mut().find(|v| v.name == view.name) {
            Some(v) => *v = view,
            None => self.prefs.views.push(view),
        }
    }

    pub(super) fn current_view(&self, name: &str) -> SavedView {
        SavedView {
            name: name.to_string(),
            status: self.prefs.status_filter,
            category: self.prefs.category_filter.clone(),
            tag: self.prefs.tag_filter.clone(),
            tracker: self.prefs.tracker_filter.clone(),
            search: self.search.clone(),
            sort: self.prefs.sort,
            sort_asc: self.prefs.sort_asc,
        }
    }

    pub(super) fn apply_view(&mut self, view: &SavedView) {
        self.prefs.status_filter = view.status;
        self.prefs.category_filter = view.category.clone();
        self.prefs.tag_filter = view.tag.clone();
        self.prefs.tracker_filter = view.tracker.clone();
        self.search.clone_from(&view.search);
        self.prefs.sort = view.sort;
        self.prefs.sort_asc = view.sort_asc;
        self.main_view = feeds::MainView::Torrents;
    }

    /// What the views show now, into the preferences that get saved.
    fn sync_prefs(&mut self) {
        self.prefs.speed_scope_torrent = self.speed_scope_torrent;
        self.prefs.show_all_peers = self.show_all_peers;
        self.prefs.file_filter.clone_from(&self.file_filter);
        self.prefs.feeds_selected.clone_from(&self.feeds.selected);
        self.prefs.search_sort = self.torznab.sort;
        self.prefs.search_asc = self.torznab.asc;
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
                Ok(n) => (ToastKind::Success, t!("Backed up {n} files to {path}", n = n, path = dest.display())),
                Err(e) => (ToastKind::Error, t!("Backup failed: {e}", e = format!("{e:#}"))),
            };
            let _ = tx.send(EngineEvent::Toast(msg.0, msg.1));
            ctx.request_repaint();
        });
    }

    pub fn import_backup(&mut self, src: std::path::PathBuf) {
        // Unpacking a large backup takes a moment: off the UI thread.
        let (paths, tx, ctx) = (self.paths.clone(), self.ui_events_tx.clone(), self.ctx.clone());
        self.rt.spawn_blocking(move || {
            let ev = match crate::backup::stage_import(&paths, &src) {
                Ok(parts) => EngineEvent::DataWarning(t!(
                    "The backup ({parts}) will be restored the next time Corrent starts. Quit and reopen Corrent to finish.",
                    parts = parts.join(", ")
                )),
                Err(e) => EngineEvent::Toast(ToastKind::Error, t!("Can't restore: {e}", e = format!("{e:#}"))),
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    pub fn with_previous_run(mut self, p: crate::crash::PreviousRun) -> Self {
        self.previous_run = p;
        self
    }

    /// Shown when the engine task stopped unexpectedly; offers a restart.
    fn engine_banner(&mut self, ui: &mut Ui, p: &Palette) {
        // The engine publishes a snapshot every tick; none for a long time means it is stuck.
        let stalled = self.engine.is_alive()
            && !self.engine.is_remote()
            && !self.quitting
            && self.snap.engine_state == crate::engine::types::EngineState::Running
            && self.last_snapshot.1.elapsed() > std::time::Duration::from_secs(30);
        if stalled {
            let secs = self.last_snapshot.1.elapsed().as_secs();
            let text = match stall_cause(&corrent_core::watchdog::crumb().doing) {
                Some(what) => t!(
                    "The torrent engine hasn't responded for {secs} seconds (busy with: {what}). If this doesn't clear up, restart Corrent.",
                    secs = secs,
                    what = what
                ),
                None => t!(
                    "The torrent engine hasn't responded for {secs} seconds. If this doesn't clear up, restart Corrent.",
                    secs = secs
                ),
            };
            egui::Frame::new()
                .fill(p.warn.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0, p.warn.gamma_multiply(0.5)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(14, 10))
                .outer_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 2 })
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(p.text));
                });
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        if self.engine.is_alive() || self.quitting {
            self.engine_report = None;
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
                            egui::RichText::new(t!(
                                "A crash report was saved. Restart the engine to continue; actions you take now are \
                                 applied after the restart."
                            ))
                            .small()
                            .color(p.text_dim),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if crate::ui::widgets::primary_button(ui, p, t!("Restart engine")).clicked() {
                            self.engine.restart(self.settings.clone());
                            crate::crash::dismiss(&self.paths.log_dir());
                        }
                        // Look the report up once per outage, not every frame.
                        let report = self
                            .engine_report
                            .get_or_insert_with(|| crate::crash::reports(&self.paths.log_dir()).first().cloned())
                            .clone();
                        if let Some(r) = report
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

    /// E7: a task panicked and the app went on: say so, with its report (at most once a minute).
    fn task_panic_notice(&mut self) {
        let (n, report) = crate::crash::task_panics();
        if n > self.task_panics_seen
            && self.task_panic_told.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(60))
        {
            self.task_panics_seen = n;
            self.task_panic_told = Some(std::time::Instant::now());
            let actions = report.map(|r| vec![(t!("View report").to_string(), toasts::ToastAction::OpenFile(r))]);
            self.toasts.push_with(
                ToastKind::Warning,
                t!("Something inside Corrent failed and was stopped; Corrent keeps running. A crash report was saved."),
                actions.unwrap_or_default(),
            );
        }
    }

    /// "Closed unexpectedly" banner shown after an unclean exit.
    fn crash_banner(&mut self, ui: &mut Ui, p: &Palette) {
        if !self.previous_run.unclean {
            return;
        }
        let report = self.previous_run.report.clone().or_else(|| self.previous_run.note.clone());
        let noted = self.previous_run.report.is_none() && self.previous_run.note.is_some();
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
                        let detail = if noted {
                            t!("It may have been ended by the system or another program; the end of its log was saved. Your torrents were restored.")
                        } else if report.is_some() {
                            t!("A crash report was saved. Your torrents and settings were restored.")
                        } else {
                            t!("It may have been ended by the system or another program. Your torrents were restored.")
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
        let mut p = theme::palette(ctx, self.appearance_preview.as_ref().unwrap_or(&self.settings.appearance));
        if self.mica {
            p.bg0 = p.bg0.gamma_multiply(0.35);
            p.bg1 = p.bg1.gamma_multiply(0.55);
        }
        p
    }

    #[allow(unused_variables, unused_mut)]
    fn with_mica_spike(mut self, cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(all(windows, debug_assertions))]
        if std::env::var_os("CORRENT_DEV_MICA").is_some() {
            self.mica = win_chrome::mica(cc);
        }
        self
    }

    pub fn save_settings(&mut self) {
        self.sync_remote();
        // On the writer thread: a slow or sleeping disk must not freeze the window. The outcome
        // comes back in `drain_background`.
        if let Err(e) = self.settings.save_in_background(&self.paths) {
            self.settings_save_failed(&format!("{e:#}"));
        }
    }

    fn settings_save_failed(&mut self, why: &str) {
        let msg = t!("Your settings couldn't be saved: {why}", why = crate::util::explain_io(why));
        if !self.data_notes.contains(&msg) {
            self.data_notes.push(msg);
        }
    }

    /// Results of work done off the UI thread: settings saves, dropped files, the clipboard.
    fn drain_background(&mut self) {
        if let Some(Err(e)) = corrent_core::writer::global().take_result(&self.paths.settings_file()) {
            self.settings_save_failed(&e);
        }
        while let Ok((sources, ignored)) = self.dropped_rx.try_recv() {
            self.launch_queue.extend(sources);
            if ignored > 0 {
                self.toasts
                    .push(ToastKind::Warning, t!("Ignored {ignored} file(s) that aren't torrents.", ignored = ignored));
            }
        }
        if let Some((rx, asked)) = &self.clipboard {
            match rx.try_recv() {
                Ok(clip) => {
                    // Only into an untouched dialog.
                    if let Some(dialogs::Modal::Magnet { text }) = &mut self.modal
                        && text.is_empty()
                    {
                        *text = clip;
                    }
                    self.clipboard = None;
                }
                // A clipboard that hasn't answered by now is left alone.
                Err(_) if asked.elapsed() > std::time::Duration::from_millis(500) => self.clipboard = None,
                Err(_) => {}
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
            self.counts.update(&self.snap);
            self.rows =
                table::compute_rows(&self.snap, &self.prefs, &self.search, &mut self.counts.hosts, &mut self.sort_memo);
            self.rows_key = Some(key);
            // Never act on torrents the user can't see.
            let visible: HashSet<InfoHash> =
                self.rows.iter().filter_map(|&i| self.snap.torrents.get(i).map(|t| t.hash)).collect();
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
        let mut added: Vec<String> = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            // The in-process media server acts on engine events too (imports, wanted list).
            if let Some(local) = self.media.local() {
                corrent_server::integrate::on_engine_event(&local.state, &self.rt, ev.clone());
            } else if self.server.is_none()
                && self.media.error.is_none()
                && self.media.early_events.len() < 1000
                && matches!(ev, EngineEvent::Completed { .. } | EngineEvent::Added { .. })
            {
                self.media.early_events.push(ev.clone());
            }
            match ev {
                EngineEvent::Toast(kind, text) => self.toasts.push(kind, text),
                EngineEvent::Added { name, hash } => {
                    self.toasts.push_added(hash, &name);
                    added.push(name);
                }
                EngineEvent::Completed { name, size, content_path, save_path, .. } => {
                    let actions = if self.server.is_none() {
                        vec![
                            (t!("Open").into(), toasts::ToastAction::OpenFile(content_path)),
                            (t!("Show in folder").into(), toasts::ToastAction::OpenFolder(save_path)),
                        ]
                    } else {
                        Vec::new()
                    };
                    self.toasts.push_with(ToastKind::Success, t!("Download complete: “{name}”", name = name), actions);
                    if self.settings.behavior.notify_on_complete {
                        self.notify(t!("Download complete"), &format!("{name} ({})", crate::util::fmt_bytes(size)));
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
                                (t!("Show file").into(), toasts::ToastAction::OpenFolder(folder)),
                                (t!("Copy magnet").into(), toasts::ToastAction::Copy(magnet)),
                            ];
                            if let Some(h) = seeding {
                                actions.push((t!("Show").into(), toasts::ToastAction::Show(h)));
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
                        vec![(t!("Show").into(), toasts::ToastAction::Show(hash))],
                    );
                }
                EngineEvent::PreviewReady { req, result } => self.on_preview_ready(req, result),
            }
        }
        if self.settings.behavior.notify_on_add && self.window_hidden {
            match added.as_slice() {
                [] => {}
                [name] => self.notify(t!("Torrent added"), name),
                many => self.notify(t!("Torrent added"), &t!("Added {n} torrents", n = many.len())),
            }
        }
        #[cfg(debug_assertions)]
        if !removed.is_empty() && std::env::var("CORRENT_DEV_OPEN").as_deref() == Ok("remove-undo") {
            self.engine.send(crate::engine::EngineCommand::UndoRemove(removed.iter().map(|(h, _)| *h).collect()));
        }
        // One toast (with Undo) per burst of removals.
        if !removed.is_empty() {
            let text = if removed.len() == 1 {
                t!("Removed “{name}”", name = removed.first().map(|r| r.1.as_str()).unwrap_or_default())
            } else {
                t!("Removed {n} torrents", n = removed.len())
            };
            let hashes = removed.into_iter().map(|(h, _)| h).collect();
            self.toasts.push_with(ToastKind::Info, text, vec![(t!("Undo").into(), toasts::ToastAction::Undo(hashes))]);
        }
        let _ = ctx;
    }

    fn run_toast_actions(&mut self, ctx: &Context, actions: Vec<toasts::ToastAction>) {
        for a in actions {
            match a {
                toasts::ToastAction::Undo(h) => {
                    self.engine.send(crate::engine::EngineCommand::UndoRemove(h));
                }
                toasts::ToastAction::OpenFolder(p) | toasts::ToastAction::OpenFile(p) => open_path(&p),
                toasts::ToastAction::Copy(text) => self.copy(text),
                toasts::ToastAction::SkipVersion(v) => {
                    self.settings.updates.skip_version = v;
                    self.save_settings();
                }
                toasts::ToastAction::Update => self.open_update(),
                toasts::ToastAction::Show(h) => self.show_torrents(ctx, vec![h]),
                toasts::ToastAction::ShowAll(hashes) => self.show_torrents(ctx, hashes),
            }
        }
    }

    /// Brings the list up with these torrents visible and selected.
    pub(super) fn show_torrents(&mut self, ctx: &Context, hashes: Vec<InfoHash>) {
        self.show_window(ctx);
        self.main_view = feeds::MainView::Torrents;
        self.clear_filters();
        self.search.clear();
        let first = hashes.first().copied();
        self.selected = hashes.into_iter().collect();
        self.focus = first;
        self.anchor = first;
        self.scroll_to_focus = true;
    }

    fn apply_appearance_if_changed(&mut self, ctx: &Context) {
        let a = self.appearance_preview.as_ref().unwrap_or(&self.settings.appearance);
        // Also re-apply when the OS high-contrast mode is switched.
        let high_contrast = a.follow_os_high_contrast && theme::os_high_contrast();
        if self.applied_appearance.as_ref().is_none_or(|(applied, hc)| applied != a || *hc != high_contrast) {
            crate::i18n::set_language(&a.language);
            theme::apply(ctx, a);
            self.applied_appearance = Some((a.clone(), high_contrast));
        }
    }

    /// Add a source: either opens the add dialog or adds directly with defaults.
    pub fn add_source(&mut self, source: AddSource) {
        let d = self.download_defaults();
        let req = crate::engine::AddRequest {
            source,
            torrent_bytes: None,
            save_path: d.save_path,
            create_subfolder: d.create_subfolder,
            category: None,
            tags: Vec::new(),
            paused: d.start_paused,
            only_files: None,
            add_to_top: false,
            initial_peers: None,
            added_by: None,
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
        let mut sources: Vec<AddSource> = self.launch_queue.drain(..).collect();
        if self.settings.behavior.skip_add_dialog {
            for src in sources {
                self.add_source(src);
            }
        } else if let [_] = sources[..]
            && self.batch.is_none()
            && let Some(one) = sources.pop()
        {
            self.begin_add(one);
        } else {
            self.begin_batch(sources);
        }
    }
}

impl eframe::App for CorrentApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.mica { [0.0; 4] } else { visuals.panel_fill.to_normalized_gamma_f32() }
    }

    fn logic(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        if !self.frame_seen {
            crate::integration::macos_events::reinstall();
        }
        self.frame_seen = true;
        // The watchdog's heartbeat (O6): frames stop while the window is hidden or minimized.
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        corrent_core::watchdog::ui_frame(!self.window_hidden && !minimized);
        self.snap = self.engine.snapshot();
        #[cfg(debug_assertions)]
        if std::env::var_os("CORRENT_DEV_DEMO").is_some() {
            self.snap = Arc::new(demo::snapshot(self.detail_hash()));
        }
        if self.snap.generation != self.last_snapshot.0 {
            self.last_snapshot = (self.snap.generation, std::time::Instant::now());
        }
        #[cfg(debug_assertions)]
        self.dev_hooks(ctx);
        self.drain_ipc(ctx);
        self.drain_background();
        self.drain_pickers();
        self.drain_events(ctx);
        self.task_panic_notice();
        let refused = self.engine.refused();
        if refused > self.refused_seen {
            self.toasts.push(ToastKind::Warning, t!("Corrent is busy and couldn't do that. Try again in a moment."));
        }
        self.refused_seen = refused;
        self.media_tick(ctx);
        self.tv_tick(ctx);
        self.switch_tick();
        self.discovery_tick(ctx);
        #[cfg(debug_assertions)]
        self.media_dev_hooks(ctx);
        self.window_logic(ctx);
        let fullscreen = ctx.input(|i| i.viewport().fullscreen).unwrap_or(false);
        if self.native_frame && fullscreen != self.chrome_fullscreen {
            self.chrome_fullscreen = fullscreen;
            win_chrome::fullscreen(frame, fullscreen);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        self.process_launch_queue();
        self.poll_remote();
        self.poll_updates();
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        if let Some(pl) = &mut self.media.player {
            pl.register(frame);
        }
        self.frame_ui(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.sync_prefs();
        eframe::set_value(storage, "ui_prefs", &self.prefs);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.tv_raw_input(raw_input);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The window is hidden by now (`hide_for_exit`); the tray icon goes too, so nothing of
        // Corrent stays on screen while the engine stops.
        self.tray = None;
        // The last settings save is queued on the writer thread.
        if !corrent_core::writer::global().flush(&self.paths.settings_file(), std::time::Duration::from_secs(2)) {
            tracing::warn!("the settings were still being saved when Corrent quit");
        }
        let power = self.power_after_exit.take();
        when_done::exit_steps(
            || {
                self.engine.shutdown_blocking(std::time::Duration::from_secs(8));
                self.wait_for_old_engine();
            },
            power,
            crate::integration::power::perform,
        );
    }
}

#[cfg(debug_assertions)]
impl CorrentApp {
    /// Debug-only hooks for screenshot testing (see CLAUDE.md for the full list):
    /// CORRENT_DEV_SELECT=<n> selects the n-th row, CORRENT_DEV_TAB picks the details tab,
    /// CORRENT_DEV_OPEN opens a window or view, CORRENT_DEV_SEARCH runs a search,
    /// CORRENT_DEV_PANIC/LIGHT/MICA, CORRENT_DEV_STREAM_URL=<file> writes the first torrent's stream
    /// URL prefix. They run once, on the first frame with at least one torrent.
    fn dev_hooks(&mut self, ctx: &Context) {
        if self.dev_done || self.snap.torrents.is_empty() {
            return;
        }
        self.dev_done = true;
        if let Ok(n) = std::env::var("CORRENT_DEV_SELECT").map(|v| v.parse::<usize>().unwrap_or(0)) {
            let rows = self.visible_rows().to_vec();
            if let Some(h) = rows.get(n).and_then(|&i| self.snap.torrents.get(i)).map(|t| t.hash) {
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
                "settings" => {
                    self.open_settings();
                    if let (Some(d), Ok(q)) = (&mut self.settings_draft, std::env::var("CORRENT_DEV_SETTINGS_SEARCH")) {
                        d.search = q;
                    }
                }
                "remote" => self.open_settings_at(settings_window::Page::Remote),
                "appearance" => self.open_settings_at(settings_window::Page::Appearance),
                "playback" => self.open_settings_at(settings_window::Page::Playback),
                "magnet" => self.open_magnet_dialog(),
                "about" => self.extras.about = true,
                "shortcuts" => self.extras.shortcuts = true,
                "palette" => self.toggle_palette(),
                "columns" => {
                    self.column_menu = Some(egui::pos2(500.0, 90.0));
                    egui::Popup::open_id(ctx, egui::Id::new(table::COLUMN_CHOOSER));
                }
                "creator" => self.open_creator(),
                "connect" => self.open_connect(),
                "import" => self.open_import(),
                "feeds" => self.main_view = feeds::MainView::Feeds,
                "search" => self.main_view = feeds::MainView::Search,
                "rules" => {
                    self.main_view = feeds::MainView::Feeds;
                    self.feeds.rules = Some(feeds::RulesEditor { rules: self.settings.rss.rules.clone(), selected: 0 });
                }
                "log" => self.extras.log = true,
                "stats" => self.extras.stats = true,
                "streams" => self.extras.streams = true,
                "diagnostics" => self.open_diagnostics(),
                "mpv" => self.open_get_mpv(true),
                "remove" | "remove-undo" => {
                    if let Some(t) = self.snap.torrents.first() {
                        self.engine
                            .send(crate::engine::EngineCommand::Delete { hashes: vec![t.hash], with_data: false });
                    }
                }
                _ => {}
            }
        }
        // O6: a hung window (the UI thread sleeps) or a busy engine; both leave logs/hang-*.txt.
        match std::env::var("CORRENT_DEV_HANG").as_deref() {
            Ok("ui") => std::thread::sleep(std::time::Duration::from_secs(15)),
            Ok("engine") => {
                self.engine.send(crate::engine::EngineCommand::DevHang(std::time::Duration::from_secs(40)));
            }
            _ => {}
        }
        match std::env::var("CORRENT_DEV_PANIC").as_deref() {
            #[expect(clippy::panic, reason = "debug-only hook that tests crash reports")]
            Ok("ui") => panic!("CORRENT_DEV_PANIC=ui"),
            Ok("engine") => {
                self.engine.send(crate::engine::EngineCommand::DevPanic);
            }
            Ok("torrent") => {
                self.engine.send(crate::engine::EngineCommand::DevTaskPanic);
            }
            _ => {}
        }
        if let Ok(q) = std::env::var("CORRENT_DEV_SEARCH") {
            self.main_view = feeds::MainView::Search;
            self.torznab.query = q;
            self.dev_start_search();
        }
        // Base stream URL of the first torrent, so a test can play it with curl.
        if let Ok(out) = std::env::var("CORRENT_DEV_STREAM_URL")
            && let Some((port, token)) = &self.snap.session.stream
            && let Some(t) = self.snap.torrents.first()
        {
            let base = format!("http://127.0.0.1:{port}/{token}/{}/", t.hash.to_hex());
            let _ = std::fs::write(out, base);
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

        // TV mode: the media pages fill the window.
        if self.tv.on {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg0)).show(ui, |ui| self.tv_view(ui, &p));
            self.dialogs(&ctx, &p);
            self.update_window(&ctx, &p);
            self.toasts.bottom_inset = 16.0;
            let actions = self.toasts.show(&ctx, &p);
            self.run_toast_actions(&ctx, actions);
            return;
        }
        // A playing video takes the whole window (Esc or the close button returns).
        let immersive =
            self.main_view == feeds::MainView::Media && self.media.player.as_ref().is_some_and(|pl| !pl.audio_only);
        if immersive {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                .show(ui, |ui| self.media_view(ui, &p));
            // No title bar while a video plays, but the edges still resize the window.
            self.window_frame(&ctx, &p);
            self.dialogs(&ctx, &p);
            self.toasts.bottom_inset = 16.0;
            let actions = self.toasts.show(&ctx, &p);
            self.run_toast_actions(&ctx, actions);
            return;
        }

        self.shortcuts(&ctx);

        egui::Panel::top("toolbar")
            .exact_size(titlebar::TITLEBAR_HEIGHT)
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }))
            .show_separator_line(true)
            .show(ui, |ui| self.toolbar(ui, &p));

        let status = egui::Panel::bottom("statusbar")
            .frame(egui::Frame::new().fill(p.bg1).inner_margin(egui::Margin::symmetric(12, 4)))
            .show(ui, |ui| self.status_bar(ui, &p));
        self.toasts.bottom_inset = status.response.rect.height() + 8.0;

        if self.media.player.as_ref().is_some_and(|pl| pl.audio_only) {
            let mini = egui::Panel::bottom("mini-player")
                .frame(egui::Frame::new().fill(p.bg2).inner_margin(egui::Margin::symmetric(14, 8)))
                .show(ui, |ui| self.media_mini_player(ui, &p));
            self.toasts.bottom_inset += mini.response.rect.height();
        }

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
                self.server_banner(ui, &p);
                self.discovery_banner(ui, &p);
                self.crash_banner(ui, &p);
                self.data_banner(ui, &p);
                self.handler_banner(ui, &p);
                match self.main_view {
                    feeds::MainView::Torrents => self.torrent_table(ui, &p),
                    feeds::MainView::Feeds => self.feeds_view(ui, &p),
                    feeds::MainView::Search => self.search_view(ui, &p),
                    feeds::MainView::Media => self.media_view(ui, &p),
                }
            },
        );

        self.drag_and_drop(&ctx, &p);
        self.window_frame(&ctx, &p);
        self.extra_windows(&ctx, &p);
        self.connect_dialog(&ctx, &p);
        self.web_account_window(&ctx, &p);
        self.server_fs_window(&ctx, &p);
        self.onboarding(&ctx, &p);
        self.creator_dialog(&ctx, &p);
        self.rules_dialog(&ctx, &p);
        self.import_dialog(&ctx, &p);
        self.when_done_logic(&ctx);
        self.when_done_dialog(&ctx, &p);
        self.batch_dialog(&ctx, &p);
        self.dialogs(&ctx, &p);
        self.settings_window(&ctx, &p);
        self.palette_window(&ctx, &p);
        self.had_focus = ctx.memory(|m| m.focused().is_some());
        let actions = self.toasts.show(&ctx, &p);
        self.run_toast_actions(&ctx, actions);
    }
}

/// What the stalled engine is busy with, for the banner (None when it is waiting, i.e. the
/// stall is somewhere else). Command names are shown as they are: they are for bug reports.
fn stall_cause(doing: &corrent_core::watchdog::Doing) -> Option<String> {
    use corrent_core::watchdog::Doing;
    Some(match doing {
        Doing::Idle => return None,
        Doing::Tick => t!("updating the torrent list").to_string(),
        Doing::StartingSession => t!("starting the engine").to_string(),
        Doing::StoppingSession => t!("stopping the engine").to_string(),
        Doing::Shutdown => t!("shutting down").to_string(),
        Doing::Command(name) => t!("the command {name}", name = name),
    })
}

pub fn open_path(p: &std::path::Path) {
    let target: PathBuf = p.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = open::that_detached(&target) {
            tracing::warn!("could not open {}: {e}", target.display());
        }
    });
}
