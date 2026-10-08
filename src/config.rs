//! User settings (TOML) and application paths.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Paths {
    /// Directory holding settings.toml, store.json and the engine session.
    pub data_dir: PathBuf,
    pub portable: bool,
}

impl Paths {
    pub fn resolve(force_portable: bool) -> Paths {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let marker = exe_dir.as_ref().map(|d| d.join("portable")).filter(|p| p.exists());
        if force_portable || marker.is_some() {
            let base = exe_dir.unwrap_or_else(|| PathBuf::from("."));
            return Paths { data_dir: base.join("corrent-data"), portable: true };
        }
        let data_dir = directories::ProjectDirs::from("", "", "Corrent")
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".corrent"));
        Paths { data_dir, portable: false }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.toml")
    }
    pub fn store_file(&self) -> PathBuf {
        self.data_dir.join("store.json")
    }
    pub fn session_dir(&self) -> PathBuf {
        self.data_dir.join("session")
    }
    pub fn torrents_dir(&self) -> PathBuf {
        self.data_dir.join("torrents")
    }
    pub fn dht_file(&self) -> PathBuf {
        self.data_dir.join("dht.json")
    }
    pub fn rss_file(&self) -> PathBuf {
        self.data_dir.join("rss.json")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}

/// The OS "Downloads" folder.
pub fn os_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(Path::to_path_buf))
        .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// Default save folder: a "Corrent" folder inside Downloads (created on demand).
pub fn default_download_dir() -> PathBuf {
    os_download_dir().join("Corrent")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Settings {
    /// File format version (see `migrate.rs`); 0 = written before versioning.
    pub schema_version: u32,
    pub behavior: Behavior,
    pub downloads: Downloads,
    pub connection: Connection,
    pub speed: Speed,
    pub bittorrent: BitTorrent,
    pub queue: Queue,
    pub seeding: Seeding,
    pub watch: Vec<WatchFolder>,
    pub rss: crate::rss::RssSettings,
    pub appearance: Appearance,
    pub advanced: Advanced,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Behavior {
    pub confirm_exit_while_active: bool,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub start_minimized: bool,
    pub show_tray_icon: bool,
    pub notify_on_complete: bool,
    pub notify_on_add: bool,
    pub confirm_delete: bool,
    pub skip_add_dialog: bool,
    pub run_on_complete: String,
    pub run_on_complete_enabled: bool,
    pub register_magnet_handler: bool,
    pub register_torrent_handler: bool,
    pub keep_handler_registration: bool,
    pub handler_prompt_dismissed: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            confirm_exit_while_active: true,
            close_to_tray: false,
            minimize_to_tray: false,
            start_minimized: false,
            show_tray_icon: true,
            notify_on_complete: true,
            notify_on_add: false,
            confirm_delete: true,
            skip_add_dialog: false,
            run_on_complete: String::new(),
            run_on_complete_enabled: false,
            register_magnet_handler: false,
            register_torrent_handler: false,
            keep_handler_registration: true,
            handler_prompt_dismissed: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Downloads {
    pub save_path: PathBuf,
    pub recent_paths: Vec<PathBuf>,
    pub remember_last_path: bool,
    pub start_paused: bool,
    pub delete_torrent_file_after_add: bool,
    pub create_subfolder: bool,
}

impl Default for Downloads {
    fn default() -> Self {
        Self {
            save_path: default_download_dir(),
            recent_paths: Vec::new(),
            remember_last_path: true,
            start_paused: false,
            delete_torrent_file_after_add: false,
            create_subfolder: true,
        }
    }
}

impl Downloads {
    pub fn push_recent(&mut self, p: PathBuf) {
        self.recent_paths.retain(|x| x != &p);
        self.recent_paths.insert(0, p);
        self.recent_paths.truncate(8);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ListenMode {
    Tcp,
    Utp,
    TcpAndUtp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Connection {
    pub listen_port: u16,
    pub random_port: bool,
    pub upnp: bool,
    pub listen_mode: ListenMode,
    pub ipv4_only: bool,
    pub bind_device: String,
    /// IP address to listen on (empty = all interfaces).
    pub listen_address: String,
    pub proxy_enabled: bool,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_username: String,
    pub proxy_password: String,
    pub max_peers_global: usize,
    pub max_peers_per_torrent: usize,
    pub blocklist_url: String,
    pub connect_timeout_secs: u64,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            listen_port: 6881,
            random_port: false,
            upnp: true,
            listen_mode: ListenMode::Tcp,
            ipv4_only: false,
            bind_device: String::new(),
            listen_address: String::new(),
            proxy_enabled: false,
            proxy_host: String::new(),
            proxy_port: 1080,
            proxy_username: String::new(),
            proxy_password: String::new(),
            max_peers_global: 500,
            max_peers_per_torrent: 0,
            blocklist_url: String::new(),
            connect_timeout_secs: 10,
        }
    }
}

impl Connection {
    pub fn proxy_url(&self) -> Option<String> {
        if !self.proxy_enabled || self.proxy_host.trim().is_empty() {
            return None;
        }
        let auth = if self.proxy_username.is_empty() {
            String::new()
        } else {
            format!(
                "{}:{}@",
                crate::engine::types::url_encode(&self.proxy_username),
                crate::engine::types::url_encode(&self.proxy_password)
            )
        };
        Some(format!("socks5://{auth}{}:{}", self.proxy_host.trim(), self.proxy_port))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    /// Bit 0 = Monday … bit 6 = Sunday.
    pub days: u8,
    pub from_minutes: u16,
    pub to_minutes: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Speed {
    /// KiB/s, 0 = unlimited.
    pub download_limit: u32,
    pub upload_limit: u32,
    pub alt_download_limit: u32,
    pub alt_upload_limit: u32,
    pub alt_enabled: bool,
    pub schedule: Schedule,
}

impl Default for Speed {
    fn default() -> Self {
        Self {
            download_limit: 0,
            upload_limit: 0,
            alt_download_limit: 1024,
            alt_upload_limit: 256,
            alt_enabled: false,
            schedule: Schedule { enabled: false, days: 0b0011111, from_minutes: 8 * 60, to_minutes: 18 * 60 },
        }
    }
}

impl Speed {
    pub fn effective(&self, alt: bool) -> (u32, u32) {
        if alt { (self.alt_download_limit, self.alt_upload_limit) } else { (self.download_limit, self.upload_limit) }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BitTorrent {
    pub dht: bool,
    pub lsd: bool,
    pub trackers_enabled: bool,
    pub default_trackers: Vec<String>,
    pub fastresume: bool,
}

impl Default for BitTorrent {
    fn default() -> Self {
        Self { dht: true, lsd: true, trackers_enabled: true, default_trackers: Vec::new(), fastresume: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Queue {
    pub enabled: bool,
    pub max_active_downloads: u32,
    pub max_active_uploads: u32,
    pub max_active_total: u32,
    pub ignore_slow: bool,
    pub slow_down_kib: u32,
    pub slow_up_kib: u32,
}

impl Default for Queue {
    fn default() -> Self {
        Self {
            enabled: true,
            max_active_downloads: 5,
            max_active_uploads: 10,
            max_active_total: 15,
            ignore_slow: true,
            slow_down_kib: 2,
            slow_up_kib: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeedAction {
    Pause,
    Remove,
    RemoveWithData,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Seeding {
    pub ratio_enabled: bool,
    pub ratio: f32,
    pub time_enabled: bool,
    pub time_minutes: u32,
    pub inactive_enabled: bool,
    pub inactive_minutes: u32,
    pub action: SeedAction,
}

impl Default for Seeding {
    fn default() -> Self {
        Self {
            ratio_enabled: false,
            ratio: 2.0,
            time_enabled: false,
            time_minutes: 24 * 60,
            inactive_enabled: false,
            inactive_minutes: 60,
            action: SeedAction::Pause,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AfterAdd {
    Delete,
    Rename,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WatchFolder {
    pub path: PathBuf,
    pub save_path: Option<PathBuf>,
    pub category: Option<String>,
    pub start_paused: bool,
    pub after_add: AfterAdd,
    pub enabled: bool,
}

impl Default for WatchFolder {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            save_path: None,
            category: None,
            start_paused: false,
            after_add: AfterAdd::Rename,
            enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Accent {
    Blue,
    Violet,
    Teal,
    Green,
    Orange,
    Pink,
    Custom,
}

impl Accent {
    pub const ALL: [Accent; 6] =
        [Accent::Blue, Accent::Violet, Accent::Teal, Accent::Green, Accent::Orange, Accent::Pink];

    pub fn name(self) -> &'static str {
        match self {
            Accent::Blue => "Blue",
            Accent::Violet => "Violet",
            Accent::Teal => "Teal",
            Accent::Green => "Green",
            Accent::Orange => "Orange",
            Accent::Pink => "Pink",
            Accent::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpeedUnits {
    Binary,
    Decimal,
    Bits,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Appearance {
    pub theme: ThemeMode,
    pub accent: Accent,
    pub custom_accent: [u8; 3],
    pub compact_rows: bool,
    pub zoom: f32,
    pub speed_units: SpeedUnits,
    pub animations: bool,
    pub show_sparklines: bool,
    pub state_colored_progress: bool,
    pub striped_rows: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            accent: Accent::Blue,
            custom_accent: [0x4F, 0x8C, 0xFF],
            compact_rows: false,
            zoom: 1.0,
            speed_units: SpeedUnits::Binary,
            animations: true,
            show_sparklines: true,
            state_colored_progress: true,
            striped_rows: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Advanced {
    pub log_level: String,
    pub concurrent_init_limit: usize,
    pub refresh_interval_ms: u64,
}

impl Default for Advanced {
    fn default() -> Self {
        Self { log_level: "info".into(), concurrent_init_limit: 3, refresh_interval_ms: 500 }
    }
}

impl Settings {
    /// Load settings. Returns (settings, first_run, warning). A damaged file is kept as
    /// `settings.toml.bad-<time>` and every section that still parses is salvaged.
    pub fn load(paths: &Paths) -> (Settings, bool, Option<String>) {
        let file = paths.settings_file();
        match std::fs::read_to_string(&file) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(mut s) => {
                    use crate::migrate::{SETTINGS_VERSION, migrate_settings};
                    let from = s.schema_version;
                    if from > SETTINGS_VERSION {
                        crate::migrate::SETTINGS_READ_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
                        return (
                            s,
                            false,
                            Some(format!(
                                "Your settings were saved by a newer version of Corrent (format {from}). They are used as-is \
                                 but won't be saved, so the newer version keeps working. Update Corrent to change settings."
                            )),
                        );
                    }
                    if from < SETTINGS_VERSION {
                        let backup = crate::migrate::backup_before_migration(&file, from);
                        migrate_settings(&mut s, from);
                        let _ = s.save(paths);
                        tracing::info!(
                            "migrated settings from format {from} to {SETTINGS_VERSION} (backup: {backup:?})"
                        );
                    }
                    (s, false, None)
                }
                Err(e) if crate::migrate::toml_version(&text) > crate::migrate::SETTINGS_VERSION => {
                    // A newer format we can't read is not damage: never overwrite it.
                    tracing::warn!("settings from a newer version can't be read ({e})");
                    crate::migrate::SETTINGS_READ_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
                    let (s, _) = Settings::salvage(&text);
                    (
                        s,
                        false,
                        Some("Your settings were saved by a newer version of Corrent and couldn't be fully read. Defaults are used for the rest of this session; the file is left untouched.".into()),
                    )
                }
                Err(e) => {
                    tracing::error!("settings file is invalid ({e}); salvaging what parses");
                    let bad = file.with_extension(format!("toml.bad-{}", crate::util::now_unix()));
                    let _ = std::fs::copy(&file, &bad);
                    let (mut s, lost) = Settings::salvage(&text);
                    crate::migrate::migrate_settings(&mut s, crate::migrate::toml_version(&text));
                    let _ = s.save(paths);
                    let what = if lost.is_empty() {
                        "Some settings could not be read".to_string()
                    } else {
                        format!("These settings were reset to defaults: {}", lost.join(", "))
                    };
                    (s, false, Some(format!("{what}. The damaged file was kept as {}.", bad.display())))
                }
            },
            Err(_) => (Settings { schema_version: crate::migrate::SETTINGS_VERSION, ..Default::default() }, true, None),
        }
    }

    /// Recover every section of a damaged settings file that still deserializes.
    pub fn salvage(text: &str) -> (Settings, Vec<String>) {
        let mut s = Settings { schema_version: crate::migrate::SETTINGS_VERSION, ..Default::default() };
        let Ok(table) = text.parse::<toml::Table>() else {
            return (s, vec!["all (the file is not valid TOML)".into()]);
        };
        let mut lost = Vec::new();
        macro_rules! section {
            ($key:literal, $field:ident) => {
                if let Some(v) = table.get($key) {
                    match v.clone().try_into() {
                        Ok(x) => s.$field = x,
                        Err(_) => lost.push($key.to_string()),
                    }
                }
            };
        }
        section!("behavior", behavior);
        section!("downloads", downloads);
        section!("connection", connection);
        section!("speed", speed);
        section!("bittorrent", bittorrent);
        section!("queue", queue);
        section!("seeding", seeding);
        section!("watch", watch);
        section!("rss", rss);
        section!("appearance", appearance);
        section!("advanced", advanced);
        (s, lost)
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        if crate::migrate::settings_read_only() {
            tracing::warn!("not saving settings: the file belongs to a newer version of Corrent");
            return Ok(());
        }
        let mut s = self.clone();
        s.schema_version = crate::migrate::SETTINGS_VERSION;
        let text = toml::to_string_pretty(&s)?;
        crate::util::write_atomic(&paths.settings_file(), text.as_bytes())
    }

    /// True if switching from `self` to `other` requires the engine session to be rebuilt.
    pub fn requires_restart(&self, other: &Settings) -> bool {
        self.connection != other.connection
            || self.bittorrent != other.bittorrent
            || self.advanced.concurrent_init_limit != other.advanced.concurrent_init_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_partial() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(s, back);
        let partial: Settings = toml::from_str("[speed]\ndownload_limit = 100\n").unwrap();
        assert_eq!(partial.speed.download_limit, 100);
        assert!(partial.bittorrent.dht);
    }

    #[test]
    fn salvage_keeps_good_sections() {
        let text = "[speed]\ndownload_limit = 42\n\n[queue]\nmax_active_downloads = \"lots\"\n";
        let (s, lost) = Settings::salvage(text);
        assert_eq!(s.speed.download_limit, 42);
        assert_eq!(lost, vec!["queue".to_string()]);
        let (_, lost) = Settings::salvage("this is [not toml");
        assert_eq!(lost.len(), 1);
    }

    #[test]
    fn proxy_url() {
        let mut c = Connection::default();
        assert_eq!(c.proxy_url(), None);
        c.proxy_enabled = true;
        c.proxy_host = "127.0.0.1".into();
        c.proxy_username = "a b".into();
        c.proxy_password = "p".into();
        assert_eq!(c.proxy_url().unwrap(), "socks5://a%20b:p@127.0.0.1:1080");
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn restart_needed_only_for_engine_settings() {
        let a = Settings::default();
        let mut b = a.clone();
        b.speed.download_limit = 10;
        b.appearance.compact_rows = true;
        b.queue.max_active_downloads = 1;
        assert!(!a.requires_restart(&b));
        b.connection.listen_port = 7000;
        assert!(a.requires_restart(&b));
        let mut c = a.clone();
        c.bittorrent.dht = false;
        assert!(a.requires_restart(&c));
    }

    #[test]
    fn effective_speed() {
        let s = Speed {
            download_limit: 1,
            upload_limit: 2,
            alt_download_limit: 3,
            alt_upload_limit: 4,
            ..Default::default()
        };
        assert_eq!(s.effective(false), (1, 2));
        assert_eq!(s.effective(true), (3, 4));
    }

    #[test]
    fn recent_paths_dedupe_and_cap() {
        let mut d = Downloads::default();
        for i in 0..12 {
            d.push_recent(PathBuf::from(format!("p{i}")));
        }
        d.push_recent(PathBuf::from("p5"));
        assert_eq!(d.recent_paths.len(), 8);
        assert_eq!(d.recent_paths[0], PathBuf::from("p5"));
        assert_eq!(d.recent_paths.iter().filter(|p| p.as_path() == Path::new("p5")).count(), 1);
    }

    #[test]
    fn default_folder_is_corrent_subfolder() {
        assert!(default_download_dir().ends_with("Corrent"));
        assert_eq!(default_download_dir().parent(), Some(os_download_dir().as_path()));
    }

    #[test]
    fn accents_have_names() {
        for a in Accent::ALL {
            assert!(!a.name().is_empty());
        }
    }
}
