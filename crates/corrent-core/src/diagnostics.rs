//! The diagnostics bundle (STABILITY.md O4): one zip a user can attach to a bug report, made by
//! ⋮ → Save diagnostics…, `corrent --diagnostics <file>` or `correntd --diagnostics <file>`.
//! Everything in it is read-only and redacted: settings go through an allow-list (a field that
//! isn't listed is left out, and a test fails until it is classified), logs lose addresses'
//! credentials and queries (and, unless asked for, info hashes and the home folder), and the
//! profile's private files (accounts, tokens, keys, feeds, the torrent list itself) are never
//! included, only counted.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::config::{Paths, Settings};

/// What the user chose to include.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// File and folder paths as they are (otherwise `<path>`, and the home folder as `~`).
    pub paths: bool,
    /// Info hashes (otherwise `<hash>`).
    pub hashes: bool,
}

/// What only the running app knows (the command line leaves it empty).
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// "Corrent 0.1.0 (abc123)".
    pub app: String,
    /// Display language, scale, monitor… as (name, value).
    pub system: Vec<(String, String)>,
    /// The engine's numbers (torrents by state, tick time, queue, refusals…).
    pub metrics: Vec<(String, String)>,
    pub health: Vec<crate::health::Issue>,
}

/// Never in the bundle, whatever the options: accounts and tokens (`corrent.db`,
/// `servers.toml`), keys (`tls/`, `connect.key`), feeds with their passkeys (`rss.json`), the
/// engine's list and the torrents themselves (only counted).
const NEVER: &[&str] =
    &["corrent.db", "servers.toml", "tls", "connect.key", "rss.json", "session.json", "store.json", "settings.toml"];

fn never(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    NEVER.iter().any(|n| lower == *n || lower.starts_with(&format!("{n}.")) || lower.starts_with(&format!("{n}-")))
        || lower.ends_with(".torrent")
        || lower.ends_with(".key")
        || lower.ends_with(".pem")
}

/// How a settings field is exported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Keep,
    /// A file or folder: `<path>` unless paths are included.
    Path,
    Drop,
}

/// Every settings field by its dotted path (`[]` for a list's items). A field that isn't here
/// is left out; `every_settings_field_is_classified` fails until it is added.
pub const SETTINGS_FIELDS: &[(&str, Rule)] = {
    use Rule::*;
    &[
        ("schema_version", Keep),
        ("advanced.concurrent_init_limit", Keep),
        ("advanced.log_level", Keep),
        ("advanced.refresh_interval_ms", Keep),
        ("appearance.accent", Keep),
        ("appearance.animations", Keep),
        ("appearance.compact_rows", Keep),
        ("appearance.custom_accent[]", Keep),
        ("appearance.custom_palette.*", Keep),
        ("appearance.follow_os_high_contrast", Keep),
        ("appearance.language", Keep),
        ("appearance.preset", Keep),
        ("appearance.show_sparklines", Keep),
        ("appearance.speed_units", Keep),
        ("appearance.state_colored_progress", Keep),
        ("appearance.striped_rows", Keep),
        ("appearance.theme", Keep),
        ("appearance.tv_scale", Keep),
        ("appearance.tv_screensaver_min", Keep),
        ("appearance.zoom", Keep),
        ("behavior.auto_switch_server", Keep),
        ("behavior.close_to_tray", Keep),
        ("behavior.confirm_delete", Keep),
        ("behavior.confirm_exit_while_active", Keep),
        ("behavior.find_servers", Keep),
        ("behavior.handler_prompt_dismissed", Keep),
        ("behavior.keep_handler_registration", Keep),
        ("behavior.minimize_to_tray", Keep),
        ("behavior.notify_on_add", Keep),
        ("behavior.notify_on_complete", Keep),
        ("behavior.register_magnet_handler", Keep),
        ("behavior.register_torrent_handler", Keep),
        // A command line: may carry anything.
        ("behavior.run_on_complete", Drop),
        ("behavior.run_on_complete_enabled", Keep),
        ("behavior.show_tray_icon", Keep),
        ("behavior.skip_add_dialog", Keep),
        ("behavior.start_minimized", Keep),
        // Private trackers' announce URLs carry passkeys.
        ("bittorrent.default_trackers[]", Drop),
        ("bittorrent.dht", Keep),
        ("bittorrent.fastresume", Keep),
        ("bittorrent.lsd", Keep),
        ("bittorrent.trackers_enabled", Keep),
        ("connection.allowlist_url", Drop),
        ("connection.bind_device", Keep),
        ("connection.blocklist_url", Drop),
        ("connection.connect_timeout_secs", Keep),
        ("connection.custom_blocklist", Drop),
        ("connection.ipv4_only", Keep),
        ("connection.listen_address", Keep),
        ("connection.listen_mode", Keep),
        ("connection.listen_port", Keep),
        ("connection.max_peers_global", Keep),
        ("connection.max_peers_per_torrent", Keep),
        ("connection.proxy_enabled", Keep),
        ("connection.proxy_host", Drop),
        ("connection.proxy_password", Drop),
        ("connection.proxy_port", Keep),
        ("connection.proxy_username", Drop),
        ("connection.random_port", Keep),
        ("connection.upnp", Keep),
        ("downloads.create_subfolder", Keep),
        ("downloads.delete_torrent_file_after_add", Keep),
        ("downloads.recent_paths[]", Path),
        ("downloads.remember_last_path", Keep),
        ("downloads.save_path", Path),
        ("downloads.start_paused", Keep),
        ("playback.audio_device", Keep),
        ("playback.channels", Keep),
        ("playback.equalizer", Keep),
        ("playback.exclusive", Keep),
        ("playback.gapless", Keep),
        ("playback.normalize", Keep),
        ("playback.passthrough", Keep),
        ("playback.replaygain", Keep),
        ("queue.enabled", Keep),
        ("queue.ignore_slow", Keep),
        ("queue.max_active_downloads", Keep),
        ("queue.max_active_total", Keep),
        ("queue.max_active_uploads", Keep),
        ("queue.slow_down_kib", Keep),
        ("queue.slow_up_kib", Keep),
        ("rss.auto_download", Keep),
        ("rss.feeds[].enabled", Keep),
        ("rss.feeds[].name", Drop),
        ("rss.feeds[].url", Drop),
        ("rss.max_articles", Keep),
        ("rss.refresh_minutes", Keep),
        ("rss.rules[].category", Drop),
        ("rss.rules[].enabled", Keep),
        ("rss.rules[].episode_filter", Drop),
        ("rss.rules[].feeds[]", Drop),
        ("rss.rules[].must_contain", Drop),
        ("rss.rules[].must_not_contain", Drop),
        ("rss.rules[].name", Drop),
        ("rss.rules[].save_path", Path),
        ("rss.rules[].smart_episodes", Keep),
        ("rss.rules[].start_paused", Keep),
        ("rss.rules[].use_regex", Keep),
        ("search.providers[].api_key", Drop),
        ("search.providers[].enabled", Keep),
        ("search.providers[].name", Drop),
        ("search.providers[].url", Drop),
        ("seeding.action", Keep),
        ("seeding.inactive_enabled", Keep),
        ("seeding.inactive_minutes", Keep),
        ("seeding.ratio", Keep),
        ("seeding.ratio_enabled", Keep),
        ("seeding.time_enabled", Keep),
        ("seeding.time_minutes", Keep),
        ("speed.alt_download_limit", Keep),
        ("speed.alt_enabled", Keep),
        ("speed.alt_upload_limit", Keep),
        ("speed.download_limit", Keep),
        ("speed.schedule.days", Keep),
        ("speed.schedule.enabled", Keep),
        ("speed.schedule.from_minutes", Keep),
        ("speed.schedule.to_minutes", Keep),
        ("speed.upload_limit", Keep),
        ("updates.check_automatically", Keep),
        ("updates.last_check", Keep),
        ("updates.skip_version", Keep),
        ("watch[].after_add", Keep),
        ("watch[].category", Drop),
        ("watch[].enabled", Keep),
        ("watch[].path", Path),
        ("watch[].save_path", Path),
        ("watch[].start_paused", Keep),
        ("web_api.allow_lan", Keep),
        ("web_api.enabled", Keep),
        ("web_api.password_hash", Drop),
        ("web_api.port", Keep),
        ("web_api.username", Drop),
    ]
};

/// The rule for a field (None: not classified, so left out).
pub fn rule_for(path: &str) -> Option<Rule> {
    SETTINGS_FIELDS.iter().find_map(|(p, r)| match p.strip_suffix(".*") {
        Some(prefix) => path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.')).then_some(*r),
        None => (*p == path).then_some(*r),
    })
}

/// Every leaf of `v` with its dotted path.
#[cfg(test)]
fn leaves<'a>(prefix: &str, v: &'a toml::Value, out: &mut Vec<(String, &'a toml::Value)>) {
    match v {
        toml::Value::Table(t) => {
            for (k, v) in t {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                leaves(&p, v, out);
            }
        }
        toml::Value::Array(a) => {
            for v in a {
                leaves(&format!("{prefix}[]"), v, out);
            }
        }
        _ => out.push((prefix.to_string(), v)),
    }
}

/// `v` with each leaf kept, made `<path>`, or removed by its rule; unlisted leaves are removed.
fn redact_value(prefix: &str, v: &toml::Value, opts: Options) -> Option<toml::Value> {
    match v {
        toml::Value::Table(t) => {
            let mut out = toml::Table::new();
            for (k, v) in t {
                let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                if let Some(v) = redact_value(&p, v, opts) {
                    out.insert(k.clone(), v);
                }
            }
            Some(toml::Value::Table(out))
        }
        toml::Value::Array(a) => {
            let p = format!("{prefix}[]");
            Some(toml::Value::Array(a.iter().filter_map(|v| redact_value(&p, v, opts)).collect()))
        }
        _ => match rule_for(prefix)? {
            Rule::Keep => Some(v.clone()),
            Rule::Path if opts.paths => Some(v.clone()),
            Rule::Path => Some(toml::Value::String("<path>".into())),
            Rule::Drop => None,
        },
    }
}

/// The settings as exported: only listed fields, secrets left out.
pub fn redact_settings(s: &Settings, opts: Options) -> String {
    let Ok(v) = toml::Value::try_from(s) else { return String::new() };
    let v = redact_value("", &v, opts).unwrap_or(toml::Value::Table(Default::default()));
    toml::to_string_pretty(&v).unwrap_or_default()
}

/// A constant pattern (the tests compile each).
fn pattern(re: &str) -> regex::Regex {
    #[expect(clippy::expect_used, reason = "constant patterns; the tests compile them")]
    regex::Regex::new(re).expect("valid regex")
}

static URL: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| pattern(r#"(?i)\b(?:https?|udp|wss?)://[^\s"'<>)\]]+"#));
static MAGNET: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| pattern(r#"(?i)\bmagnet:\?[^\s"'<>]+"#));
static HASH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| pattern(r"(?i)\b[0-9a-f]{40}\b"));

/// A log or report line as exported: addresses without credentials and queries, magnet links
/// out, and (unless asked for) no info hashes or home folder.
pub fn redact_line(line: &str, opts: Options, home: Option<&str>) -> String {
    let mut s = URL.replace_all(line, |c: &regex::Captures<'_>| crate::util::redact_url(&c[0])).into_owned();
    s = MAGNET.replace_all(&s, "<magnet link>").into_owned();
    if !opts.hashes {
        s = HASH.replace_all(&s, "<hash>").into_owned();
    }
    if !opts.paths
        && let Some(home) = home.filter(|h| h.len() > 3)
    {
        s = s.replace(home, "~");
        let escaped = home.replace('\\', "\\\\");
        s = s.replace(&escaped, "~");
    }
    s
}

/// Reads a file the app may be replacing right now (written to a temporary file and renamed):
/// tried a few times.
fn read_retrying(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut last = None;
    for _ in 0..3 {
        match std::fs::read(path) {
            Ok(b) => return Ok(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("unreadable")))
}

/// The log folder's files that go into the bundle (logs, crash and hang reports).
fn log_files(paths: &Paths) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(paths.log_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                (n.contains(".log") || (n.starts_with("crash-") || n.starts_with("hang-")) && n.ends_with(".txt"))
                    && !never(n)
            })
        })
        .collect();
    v.sort();
    v
}

/// The names of the bundle's entries (shown before saving).
pub fn entries(paths: &Paths) -> Vec<String> {
    let mut v: Vec<String> = log_files(paths)
        .iter()
        .filter_map(|p| p.file_name().map(|n| format!("logs/{}", n.to_string_lossy())))
        .collect();
    v.extend(["settings.toml", "profile.txt", "versions.txt", "health.txt", "metrics.txt"].map(String::from));
    v
}

fn home() -> Option<String> {
    directories::BaseDirs::new().map(|b| b.home_dir().to_string_lossy().into_owned())
}

/// Counts and sizes of the profile's files (never their contents).
fn profile_text(paths: &Paths, opts: Options) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let shown = |p: &Path| if opts.paths { p.display().to_string() } else { "<path>".into() };
    let _ = writeln!(out, "data folder: {}", shown(&paths.data_dir));
    let _ = writeln!(out, "portable: {}", paths.portable);
    match read_retrying(&paths.store_file()) {
        Ok(b) => match serde_json::from_slice::<crate::store::StoreData>(&b) {
            Ok(d) => {
                let paused = d.torrents.values().filter(|t| t.paused).count();
                let _ = writeln!(
                    out,
                    "store: {} torrents ({paused} paused), {} categories, {} tags, {} bytes",
                    d.torrents.len(),
                    d.categories.len(),
                    d.tags.len(),
                    b.len()
                );
            }
            Err(e) => _ = writeln!(out, "store: unreadable ({e})"),
        },
        Err(e) => _ = writeln!(out, "store: unreadable ({e})"),
    }
    let session = paths.session_dir();
    let _ = writeln!(out, "engine list: {} torrents", crate::engine::session_torrent_count(&session));
    if opts.hashes {
        let mut hashes: Vec<String> = std::fs::read_dir(&session)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".torrent").map(str::to_string))
            .collect();
        hashes.sort();
        let _ = writeln!(out, "torrent files: {}", hashes.join(", "));
    }
    let _ = writeln!(out, "\nfiles:");
    let mut files: Vec<(String, u64)> = std::fs::read_dir(&paths.data_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            Some((if md.is_dir() { format!("{name}/") } else { name }, md.len()))
        })
        .collect();
    files.sort();
    for (name, len) in files {
        let _ = writeln!(out, "{name} {len}");
    }
    out
}

fn pairs_text(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
}

/// Writes the bundle to `out`. Returns the entries written.
pub fn write(
    paths: &Paths,
    settings: &Settings,
    extra: &Extra,
    opts: Options,
    out: &Path,
) -> anyhow::Result<Vec<String>> {
    let home = home();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(out)?);
    let zopts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut written = Vec::new();
    let mut put = |name: String, text: String, zip: &mut zip::ZipWriter<std::fs::File>| -> anyhow::Result<()> {
        zip.start_file(name.as_str(), zopts)?;
        zip.write_all(text.as_bytes())?;
        written.push(name);
        Ok(())
    };
    for f in log_files(paths) {
        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let text = match read_retrying(&f) {
            Ok(b) => {
                String::from_utf8_lossy(&b).lines().map(|l| redact_line(l, opts, home.as_deref()) + "\n").collect()
            }
            Err(e) => format!("unreadable: {e}\n"),
        };
        put(format!("logs/{name}"), text, &mut zip)?;
    }
    put("settings.toml".into(), redact_settings(settings, opts), &mut zip)?;
    put("profile.txt".into(), profile_text(paths, opts), &mut zip)?;
    let mut versions = vec![
        (
            "app".to_string(),
            if extra.app.is_empty() { env!("CARGO_PKG_VERSION").to_string() } else { extra.app.clone() },
        ),
        ("os".into(), format!("{} {} ({})", std::env::consts::OS, std::env::consts::ARCH, std::env::consts::FAMILY)),
    ];
    versions.extend(extra.system.iter().cloned());
    put("versions.txt".into(), pairs_text(&versions), &mut zip)?;
    let health: String = if extra.health.is_empty() {
        "no problems found\n".into()
    } else {
        extra.health.iter().map(|i| format!("{i:?}\n")).collect()
    };
    put("health.txt".into(), redact_line(&health, opts, home.as_deref()), &mut zip)?;
    put("metrics.txt".into(), pairs_text(&extra.metrics), &mut zip)?;
    zip.finish()?;
    Ok(written)
}

/// `--diagnostics <file>`: the bundle from the command line, read-only (also beside a running
/// app). Settings are read without repairing or migrating the file.
pub fn cli(paths: &Paths, out: &Path, app: &str) -> i32 {
    let settings = std::fs::read_to_string(paths.settings_file()).map(|t| Settings::salvage(&t).0).unwrap_or_default();
    let free = crate::engine::free_space_for(&settings.downloads.save_path);
    let probe = Ok(());
    let extra = Extra {
        app: app.to_string(),
        health: crate::health::evaluate(&probe, free, None),
        metrics: vec![("free space (save folder)".into(), free.map(|f| f.to_string()).unwrap_or_default())],
        ..Default::default()
    };
    match write(paths, &settings, &extra, Options::default(), out) {
        Ok(entries) => {
            println!("wrote {} ({} entries; no paths or info hashes)", out.display(), entries.len());
            0
        }
        Err(e) => {
            eprintln!("{}: {e:#}", out.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings with an item in every list and every optional field set, so each field shows.
    fn sample() -> Settings {
        let mut s = Settings::default();
        s.watch.push(crate::config::WatchFolder {
            path: "/w".into(),
            save_path: Some("/ws".into()),
            category: Some("c".into()),
            ..Default::default()
        });
        s.downloads.recent_paths.push("/r".into());
        s.bittorrent.default_trackers.push("https://t/announce?passkey=1".into());
        s.rss.feeds.push(crate::rss::FeedConfig { url: "https://f".into(), ..Default::default() });
        s.rss.rules.push(crate::rss::rules::RssRule {
            feeds: vec!["https://f".into()],
            category: Some("c".into()),
            save_path: Some("/rs".into()),
            ..Default::default()
        });
        s.search.providers.push(crate::search::TorznabProvider::default());
        let colors = ["bg0", "bg1", "bg2", "bg3", "border", "border_hi", "text", "text_dim", "text_mute", "accent"];
        let colors = colors.iter().chain(&["success", "warn", "error", "queued", "paused", "completed"]);
        let def: String = colors
            .map(|c| {
                format!(
                    "{c} = \"#000000\"
"
                )
            })
            .collect();
        s.appearance.custom_palette = Some(
            toml::from_str(&format!(
                "name = \"p\"
dark = true
{def}"
            ))
            .unwrap(),
        );
        s
    }

    #[test]
    fn every_settings_field_is_classified() {
        let v = toml::Value::try_from(sample()).unwrap();
        let mut all = Vec::new();
        leaves("", &v, &mut all);
        let missing: Vec<&str> = all.iter().filter(|(p, _)| rule_for(p).is_none()).map(|(p, _)| p.as_str()).collect();
        assert!(missing.is_empty(), "add these to SETTINGS_FIELDS (Keep, Path or Drop):\n{}", missing.join("\n"));
        // A list left empty here would hide its fields from this test.
        let text = toml::to_string(&sample()).unwrap();
        assert!(!text.contains("= []"), "fill every list in sample():\n{text}");
        let stale: Vec<&str> = SETTINGS_FIELDS
            .iter()
            .map(|(p, _)| *p)
            .filter(|p| !p.ends_with(".*") && !all.iter().any(|(q, _)| q == p))
            .collect();
        assert!(stale.is_empty(), "no such settings field any more: {stale:?}");
    }

    #[test]
    fn no_secret_leaves_the_bundle() {
        // Every string field holds its own name, so a leak names itself.
        let mut v = toml::Value::try_from(sample()).unwrap();
        fn mark(prefix: &str, v: &mut toml::Value) {
            match v {
                toml::Value::Table(t) => {
                    for (k, v) in t.iter_mut() {
                        let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                        mark(&p, v);
                    }
                }
                toml::Value::Array(a) => a.iter_mut().for_each(|v| mark(&format!("{prefix}[]"), v)),
                toml::Value::String(s) => *s = format!("SECRET:{prefix}"),
                _ => {}
            }
        }
        mark("", &mut v);
        let export = |opts| toml::to_string(&redact_value("", &v, opts).unwrap()).unwrap();
        let out = export(Options::default());
        for (p, r) in SETTINGS_FIELDS {
            let marker = format!("SECRET:{p}");
            match r {
                Rule::Drop | Rule::Path => assert!(!out.contains(&format!("{marker}\"")), "{p} leaked:\n{out}"),
                Rule::Keep => {}
            }
        }
        assert!(out.contains("<path>"), "{out}");
        assert!(out.contains("SECRET:connection.bind_device"), "kept fields stay:\n{out}");
        let with_paths = export(Options { paths: true, hashes: false });
        assert!(with_paths.contains("SECRET:downloads.save_path"));
        assert!(!with_paths.contains("SECRET:connection.proxy_password"));
    }

    #[test]
    fn log_lines_lose_secrets() {
        let home = if cfg!(windows) { r"C:\Users\someone" } else { "/home/someone" };
        let line = format!(
            "tracker https://user:pw@t.example/announce?passkey=abc failed; udp://t2:80/a?k=1 ok; \
             magnet:?xt=urn:btih:{h}&tr=x hash {h} at {home}{sep}Downloads",
            h = "0123456789abcdef0123456789abcdef01234567",
            sep = std::path::MAIN_SEPARATOR
        );
        let out = redact_line(&line, Options::default(), Some(home));
        for bad in ["passkey", "abc", "user:pw", "k=1", "0123456789abcdef", "someone"] {
            assert!(!out.contains(bad), "{bad} in {out}");
        }
        assert!(
            out.contains("https://t.example/announce?…") && out.contains("<magnet link>") && out.contains("~"),
            "{out}"
        );
        let kept = redact_line(&line, Options { paths: true, hashes: true }, Some(home));
        assert!(kept.contains("hash 0123456789abcdef") && kept.contains("someone"), "{kept}");
    }

    #[test]
    fn the_bundle_has_its_entries_and_nothing_private() {
        let dir = std::env::temp_dir().join(format!("corrent-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths { data_dir: dir.join("data"), portable: true };
        std::fs::create_dir_all(paths.log_dir()).unwrap();
        std::fs::create_dir_all(paths.session_dir()).unwrap();
        std::fs::write(paths.log_dir().join("corrent.log"), "INFO feed https://x/rss?passkey=secret1 ok\n").unwrap();
        std::fs::write(paths.log_dir().join("crash-1.txt"), "Corrent crash report\n").unwrap();
        std::fs::write(paths.log_dir().join("hang-2-ui.txt"), "Corrent hang report\n").unwrap();
        for private in ["servers.toml", "corrent.db", "rss.json", "settings.toml"] {
            std::fs::write(paths.data_dir.join(private), "token = \"secret2\"").unwrap();
        }
        std::fs::write(paths.store_file(), r#"{"torrents":{},"categories":{},"tags":["t"]}"#).unwrap();
        std::fs::write(paths.session_dir().join(format!("{}.torrent", "ab".repeat(20))), "x").unwrap();
        let out = dir.join("diag.zip");
        let extra = Extra { app: "Corrent test".into(), ..Default::default() };
        let written = write(&paths, &Settings::default(), &extra, Options::default(), &out).unwrap();
        let planned = entries(&paths);
        assert_eq!(written, planned);
        for want in ["logs/corrent.log", "logs/crash-1.txt", "logs/hang-2-ui.txt", "settings.toml", "profile.txt"] {
            assert!(written.iter().any(|w| w == want), "{want} missing: {written:?}");
        }
        let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut all = String::new();
        for i in 0..z.len() {
            let mut f = z.by_index(i).unwrap();
            let name = f.name().unwrap().to_string();
            assert!(!never(name.rsplit('/').next().unwrap_or_default()) || name == "settings.toml", "{name}");
            std::io::Read::read_to_string(&mut f, &mut all).unwrap();
        }
        assert!(!all.contains("secret1") && !all.contains("secret2"), "{all}");
        assert!(!all.contains(&"ab".repeat(20)), "no hashes unless asked");
        assert!(all.contains("store: 0 torrents (0 paused), 0 categories, 1 tags"), "{all}");
        assert!(all.contains("servers.toml"), "private files are listed by name and size only");
        for name in ["servers.toml", "corrent.db", "tls", "rss.json", "session.json", "x.torrent", "connect.key"] {
            assert!(never(name), "{name}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
