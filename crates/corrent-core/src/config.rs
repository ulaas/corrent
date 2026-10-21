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

    /// The headless server's profile: never the desktop app's (two engines must not share a
    /// session). `--data-dir` wins; portable keeps it next to the executable.
    pub fn resolve_server(force_portable: bool, data_dir: Option<PathBuf>) -> Paths {
        if let Some(d) = data_dir {
            return Paths { data_dir: d, portable: force_portable };
        }
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        if force_portable {
            let base = exe_dir.unwrap_or_else(|| PathBuf::from("."));
            return Paths { data_dir: base.join("correntd-data"), portable: true };
        }
        let data_dir = directories::ProjectDirs::from("", "", "Corrent Server")
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".corrent-server"));
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
    pub search: crate::search::SearchSettings,
    pub web_api: WebApiSettings,
    pub updates: crate::updates::UpdateSettings,
    pub appearance: Appearance,
    /// The media player (sound output and processing).
    pub playback: Playback,
    pub advanced: Advanced,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EqPreset {
    #[default]
    Flat,
    BassBoost,
    Treble,
    Vocal,
    Loudness,
}

impl EqPreset {
    pub const ALL: [EqPreset; 5] =
        [EqPreset::Flat, EqPreset::BassBoost, EqPreset::Treble, EqPreset::Vocal, EqPreset::Loudness];

    /// Bands as (frequency Hz, gain dB).
    pub fn bands(self) -> &'static [(u32, i32)] {
        match self {
            EqPreset::Flat => &[],
            EqPreset::BassBoost => &[(60, 6), (150, 4), (400, 1)],
            EqPreset::Treble => &[(4000, 2), (8000, 4), (14000, 5)],
            EqPreset::Vocal => &[(150, -2), (1000, 2), (3000, 4), (8000, 1)],
            EqPreset::Loudness => &[(60, 5), (150, 2), (3000, -1), (10000, 4)],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playback {
    /// mpv `audio-device` ("auto" = the system default).
    pub audio_device: String,
    /// Bit-perfect: take the device for ourselves (WASAPI exclusive, CoreAudio hog mode).
    pub exclusive: bool,
    /// mpv `audio-channels`: "auto-safe", "auto", "stereo", "5.1", "7.1".
    pub channels: String,
    /// Send Dolby/DTS untouched to a receiver (S/PDIF, HDMI).
    pub passthrough: bool,
    /// Even out loudness between tracks and films (EBU R128).
    pub normalize: bool,
    pub equalizer: EqPreset,
    /// "off", "track" or "album".
    pub replaygain: String,
    /// No gap between album tracks.
    pub gapless: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Playback {
            audio_device: "auto".into(),
            exclusive: false,
            channels: "auto-safe".into(),
            passthrough: false,
            normalize: false,
            equalizer: EqPreset::Flat,
            replaygain: "album".into(),
            gapless: true,
        }
    }
}

impl Playback {
    /// The player's audio options for mpv.
    pub fn mpv_options(&self) -> Vec<(&'static str, String)> {
        let mut filters: Vec<String> = Vec::new();
        if self.normalize {
            filters.push("loudnorm=I=-18:TP=-1.5:LRA=11".into());
        }
        for (f, g) in self.equalizer.bands() {
            filters.push(format!("equalizer=f={f}:t=o:w=1:g={g}"));
        }
        let af = if filters.is_empty() { String::new() } else { format!("lavfi=[{}]", filters.join(",")) };
        let device = if self.audio_device.trim().is_empty() { "auto".to_string() } else { self.audio_device.clone() };
        vec![
            ("audio-device", device),
            ("audio-exclusive", if self.exclusive { "yes" } else { "no" }.into()),
            ("audio-channels", if self.channels.is_empty() { "auto-safe".into() } else { self.channels.clone() }),
            ("audio-spdif", if self.passthrough { "ac3,eac3,dts,dts-hd,truehd".into() } else { String::new() }),
            ("af", af),
            (
                "replaygain",
                if matches!(self.replaygain.as_str(), "off" | "no") { "no".into() } else { self.replaygain.clone() },
            ),
            ("gapless-audio", if self.gapless { "weak" } else { "no" }.into()),
        ]
    }
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
    /// Notice Corrent servers that announce themselves on this network (connecting to a new one
    /// still needs pairing).
    pub find_servers: bool,
    /// Switch back to the server the app fell back from as soon as it is reachable again,
    /// without asking.
    pub auto_switch_server: bool,
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
            find_servers: true,
            auto_switch_server: false,
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
    /// Blocklist: http(s)/file URL or a local path; P2P format, plain or gzip.
    pub blocklist_url: String,
    /// Only connect to peers in this list (empty = everyone not blocked).
    pub allowlist_url: String,
    /// The user's own blocked addresses: IPs, CIDR blocks or ranges, one per line.
    pub custom_blocklist: String,
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
            allowlist_url: String::new(),
            custom_blocklist: String::new(),
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
            Accent::Blue => t!("Blue"),
            Accent::Violet => t!("Violet"),
            Accent::Teal => t!("Teal"),
            Accent::Green => t!("Green"),
            Accent::Orange => t!("Orange"),
            Accent::Pink => t!("Pink"),
            Accent::Custom => t!("Custom"),
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
    pub preset: PalettePreset,
    /// The user's own palette (used with `PalettePreset::Custom`).
    pub custom_palette: Option<PaletteDef>,
    /// Switch to the high-contrast palette while the OS high-contrast mode is on.
    pub follow_os_high_contrast: bool,
    /// Interface language code ("" = follow the system).
    pub language: String,
    /// TV mode: scale on top of `zoom`.
    pub tv_scale: f32,
    /// TV mode: minutes without input before the screensaver (0 = never).
    pub tv_screensaver_min: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
pub enum PalettePreset {
    #[default]
    Corrent,
    Nord,
    Solarized,
    HighContrast,
    Custom,
}

impl PalettePreset {
    pub const ALL: [PalettePreset; 5] = [
        PalettePreset::Corrent,
        PalettePreset::Nord,
        PalettePreset::Solarized,
        PalettePreset::HighContrast,
        PalettePreset::Custom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PalettePreset::Corrent => t!("Corrent"),
            PalettePreset::Nord => t!("Nord"),
            PalettePreset::Solarized => t!("Solarized"),
            PalettePreset::HighContrast => t!("High contrast"),
            PalettePreset::Custom => t!("Custom"),
        }
    }
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
            preset: PalettePreset::Corrent,
            custom_palette: None,
            follow_os_high_contrast: true,
            language: String::new(),
            tv_scale: 1.5,
            tv_screensaver_min: 5,
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
    /// Load settings. Returns (settings, first_run, warning). A damaged (or empty) file is kept
    /// as `settings.toml.bad-<time>`; then `settings.toml.bak` is used if it is good, otherwise
    /// every section that still parses is salvaged.
    pub fn load(paths: &Paths) -> (Settings, bool, Option<String>) {
        let file = paths.settings_file();
        match std::fs::read_to_string(&file) {
            Ok(text) => match Settings::parse(&text) {
                Ok(mut s) => {
                    use crate::migrate::{SETTINGS_VERSION, migrate_settings};
                    let from = s.schema_version;
                    if from > SETTINGS_VERSION {
                        crate::migrate::SETTINGS_READ_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
                        return (
                            s,
                            false,
                            Some(t!(
                                "Your settings were saved by a newer version of Corrent (format {from}). They are used as-is \
                                 but won't be saved, so the newer version keeps working. Update Corrent to change settings.",
                                from = from
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
                        Some(t!("Your settings were saved by a newer version of Corrent and couldn't be fully read. Defaults are used for the rest of this session; the file is left untouched.").into()),
                    )
                }
                Err(e) => {
                    tracing::error!("settings file is invalid ({e})");
                    let bad = file.with_extension(format!("toml.bad-{}", crate::util::now_unix()));
                    let kept = std::fs::copy(&file, &bad).is_ok();
                    let bak = file.with_extension("toml.bak");
                    let good = std::fs::read_to_string(&bak)
                        .ok()
                        .and_then(|b| Settings::parse(&b).ok())
                        .filter(|b| b.schema_version <= crate::migrate::SETTINGS_VERSION);
                    // Put the good copy back first, so even a failed save below leaves a good file;
                    // if that fails (a locked file) a save could rotate the damage over `.bak`.
                    if kept
                        && let Some(mut s) = good
                        && std::fs::copy(&bak, &file).is_ok()
                    {
                        let from = s.schema_version;
                        crate::migrate::migrate_settings(&mut s, from);
                        let _ = s.save(paths);
                        tracing::info!("settings restored from {}", bak.display());
                        return (
                            s,
                            false,
                            Some(t!(
                                "Your settings file was damaged and was restored from its backup. The damaged file was \
                                 kept as {path}.",
                                path = bad.display()
                            )),
                        );
                    }
                    tracing::warn!("no usable settings backup; salvaging what parses");
                    let (mut s, lost) = Settings::salvage(&text);
                    crate::migrate::migrate_settings(&mut s, crate::migrate::toml_version(&text));
                    if kept {
                        let _ = s.save(paths);
                    } else {
                        // No copy of the original: don't overwrite it.
                        crate::migrate::SETTINGS_READ_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    let what = if lost.is_empty() {
                        t!("Some settings could not be read").to_string()
                    } else {
                        t!("These settings were reset to defaults: {list}", list = lost.join(", "))
                    };
                    let tail = if kept {
                        t!("The damaged file was kept as {path}.", path = bad.display())
                    } else {
                        t!("Changes won't be saved this session, so the file stays as it is.").to_string()
                    };
                    (s, false, Some(format!("{what}. {tail}")))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (Settings { schema_version: crate::migrate::SETTINGS_VERSION, ..Default::default() }, true, None)
            }
            Err(e) => {
                // Exists but unreadable (lock, permissions, not UTF-8): never treat it as a first run.
                tracing::error!("settings file can't be read ({e})");
                crate::migrate::SETTINGS_READ_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
                (
                    Settings { schema_version: crate::migrate::SETTINGS_VERSION, ..Default::default() },
                    false,
                    Some(t!(
                        "Your settings file couldn't be read ({e}). Defaults are used and nothing is saved this \
                         session, so the file stays as it is.",
                        e = e
                    )),
                )
            }
        }
    }

    /// Parse a settings file. An empty file, or a very short one without `schema_version`, is
    /// damage (a crash while writing it), not "all defaults".
    pub(crate) fn parse(text: &str) -> Result<Settings, String> {
        if text.trim().is_empty() || (text.len() < 16 && !text.contains("schema_version")) {
            return Err("the file is empty".into());
        }
        toml::from_str(text).map_err(|e| e.to_string())
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
        section!("search", search);
        section!("web_api", web_api);
        section!("updates", updates);
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

    /// Like `save`, but written on the writer thread (the UI must never wait on the disk). The
    /// outcome comes back through `writer::global().take_result(&paths.settings_file())`.
    pub fn save_in_background(&self, paths: &Paths) -> anyhow::Result<()> {
        if crate::migrate::settings_read_only() {
            tracing::warn!("not saving settings: the file belongs to a newer version of Corrent");
            return Ok(());
        }
        let mut s = self.clone();
        s.schema_version = crate::migrate::SETTINGS_VERSION;
        let text = toml::to_string_pretty(&s)?;
        crate::writer::global().submit(&paths.settings_file(), text.into_bytes(), None);
        Ok(())
    }

    /// True if switching from `self` to `other` requires the engine session to be rebuilt.
    pub fn requires_restart(&self, other: &Settings) -> bool {
        self.connection != other.connection
            || self.bittorrent != other.bittorrent
            || self.advanced.concurrent_init_limit != other.advanced.concurrent_init_limit
    }
}

/// Settings of the qBittorrent-compatible Web API (served by the GUI or the server).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WebApiSettings {
    pub enabled: bool,
    pub port: u16,
    /// Listen on all interfaces instead of only this computer.
    pub allow_lan: bool,
    pub username: String,
    /// PBKDF2 hash (see `webapi::auth::hash_password`); empty = not set, the API stays off.
    pub password_hash: String,
}

impl Default for WebApiSettings {
    fn default() -> Self {
        WebApiSettings {
            enabled: false,
            port: 8080,
            allow_lan: false,
            username: "admin".into(),
            password_hash: String::new(),
        }
    }
}

/// A palette as stored in `theme.toml` (colors as `#rrggbb`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PaletteDef {
    pub name: String,
    pub dark: bool,
    pub bg0: String,
    pub bg1: String,
    pub bg2: String,
    pub bg3: String,
    pub border: String,
    pub border_hi: String,
    pub text: String,
    pub text_dim: String,
    pub text_mute: String,
    pub accent: String,
    pub success: String,
    pub warn: String,
    pub error: String,
    pub queued: String,
    pub paused: String,
    pub completed: String,
}

#[cfg(test)]
mod tests {

    #[test]
    fn playback_options_for_mpv() {
        let d = Playback::default().mpv_options();
        let get = |o: &[(&str, String)], k: &str| o.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get(&d, "af"), "");
        assert_eq!(get(&d, "audio-device"), "auto");
        let p = Playback {
            normalize: true,
            equalizer: EqPreset::BassBoost,
            replaygain: "off".into(),
            passthrough: true,
            ..Default::default()
        };
        let o = p.mpv_options();
        assert!(get(&o, "af").starts_with("lavfi=[loudnorm=") && get(&o, "af").contains("equalizer=f=60"));
        assert_eq!(get(&o, "replaygain"), "no");
        assert!(get(&o, "audio-spdif").contains("truehd"));
    }

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

    fn profile(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("corrent-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Paths { data_dir: dir, portable: true }
    }

    fn bad_copies(p: &Paths) -> usize {
        std::fs::read_dir(&p.data_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("settings.toml.bad-"))
            .count()
    }

    #[test]
    fn damaged_settings_come_back_from_the_backup() {
        let good = format!("schema_version = {}\n\n[speed]\ndownload_limit = 5\n", crate::migrate::SETTINGS_VERSION);
        for (name, damaged) in [
            ("empty", ""),
            ("blank", "  \r\n\t\n"),
            ("short", "[speed]\n"),
            ("cut", "schema_version = 1\n[speed]\ndownload_limit = "),
            ("garbage", "\u{0}\u{0}\u{0}PK\u{3}\u{4} not toml at all"),
        ] {
            let p = profile(name);
            std::fs::write(p.settings_file(), damaged).unwrap();
            std::fs::write(p.data_dir.join("settings.toml.bak"), &good).unwrap();
            let (s, first_run, warning) = Settings::load(&p);
            assert_eq!(s.speed.download_limit, 5, "{name}");
            assert!(!first_run, "{name}");
            assert!(warning.unwrap_or_default().contains("restored from its backup"), "{name}");
            assert_eq!(bad_copies(&p), 1, "{name}: the damaged file is kept");
            // The file is good again, and so is the backup after the next save.
            let (again, _, warning) = Settings::load(&p);
            assert_eq!(again.speed.download_limit, 5, "{name}");
            assert!(warning.is_none(), "{name}");
            again.save(&p).unwrap();
            let bak = std::fs::read_to_string(p.data_dir.join("settings.toml.bak")).unwrap();
            assert!(Settings::parse(&bak).is_ok(), "{name}");
            let _ = std::fs::remove_dir_all(&p.data_dir);
        }
    }

    #[test]
    fn empty_settings_without_a_backup_are_not_silent() {
        let p = profile("nobak");
        std::fs::write(p.settings_file(), "").unwrap();
        let (s, first_run, warning) = Settings::load(&p);
        assert_eq!(s, Settings { schema_version: crate::migrate::SETTINGS_VERSION, ..Default::default() });
        assert!(!first_run);
        assert!(warning.is_some());
        assert_eq!(bad_copies(&p), 1);
        // A backup that is itself damaged is not used either.
        let p = profile("badbak");
        std::fs::write(p.settings_file(), "").unwrap();
        std::fs::write(p.data_dir.join("settings.toml.bak"), "").unwrap();
        let (_, _, warning) = Settings::load(&p);
        assert!(!warning.unwrap_or_default().contains("restored from its backup"));
        let _ = std::fs::remove_dir_all(&p.data_dir);
        let _ = std::fs::remove_dir_all(profile("nobak").data_dir);
    }

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
