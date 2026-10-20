// STABILITY.md F17: the UI doesn't index (a row list can be older than the snapshot it
// indexes, D12); tests may (clippy.toml).
#![deny(clippy::indexing_slicing)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![recursion_limit = "256"]

#[macro_use]
extern crate corrent_i18n;
use corrent_core::{backup, config, crash, engine, import, rss, search, store, updates, util};
use corrent_i18n as i18n;
use corrent_server::webapi;
mod app;
mod integration;
mod link;
mod ui;

use std::sync::Arc;

use config::{Paths, Settings};
use engine::types::AddSource;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let paths = Paths::resolve(flag("--portable"));
    let _ = std::fs::create_dir_all(&paths.data_dir);
    #[cfg(unix)]
    {
        // Settings may contain proxy credentials and the IPC socket lives here.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&paths.data_dir, std::fs::Permissions::from_mode(0o700));
    }

    if flag("--help") || flag("-h") {
        #[cfg(windows)]
        attach_console();
        println!("Corrent {}\n", env!("CARGO_PKG_VERSION"));
        println!("Usage: corrent [OPTIONS] [MAGNET | URL | FILE.torrent]...\n");
        println!("Options:");
        println!("  --portable     keep all data next to the executable");
        println!("  --minimized    start hidden in the tray");
        println!("  --tv           start in TV mode (full screen, remote/gamepad friendly)");
        println!("  --register     register as handler for magnet links and .torrent files");
        println!("  --unregister   remove the handler registration");
        println!("  --status       print handler registration status");
        println!("  --write-icons <dir>  write the app icon set to a folder");
        println!("  -h, --help     show this help");
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--write-icons") {
        let dir = args.get(i + 1).map(std::path::PathBuf::from).unwrap_or_else(|| "assets/icon".into());
        match app::icon::write_icon_set(&dir) {
            Ok(()) => println!("wrote icons to {}", dir.display()),
            Err(e) => eprintln!("error: {e}"),
        }
        return Ok(());
    }
    if flag("--register") || flag("--unregister") || flag("--status") {
        std::process::exit(handler_cli(flag("--register"), flag("--unregister")));
    }

    let items: Vec<String> = args.iter().filter(|a| !a.starts_with("--")).cloned().collect();
    let (mut listener, unresponsive) = match integration::single_instance::acquire(&paths, &items) {
        integration::single_instance::Acquire::Forwarded => return Ok(()),
        integration::single_instance::Acquire::Primary(l) => (l, false),
        integration::single_instance::Acquire::Unresponsive => (None, true),
    };
    // One process per data folder, locked before anything in it is restored or read (correntd
    // takes the same lock). Never run without it.
    let _data_lock = match corrent_core::lock::lock_data_dir(&paths.data_dir) {
        Ok(lock) if unresponsive => {
            // The other side didn't confirm but doesn't hold the lock: it was quitting, or it is a
            // Corrent from before confirmations (which gets the items anyway). Ask once more.
            match integration::single_instance::acquire(&paths, &items) {
                integration::single_instance::Acquire::Primary(l) => {
                    listener = l;
                    lock
                }
                _ => {
                    eprintln!("forwarded to a running Corrent that did not confirm");
                    return Ok(());
                }
            }
        }
        Ok(lock) => lock,
        Err(e) => {
            let text = match &e {
                corrent_core::lock::LockError::Held if unresponsive => t!(
                    "Corrent is already running but isn't responding, so this link or file couldn't be handed to it. \
                     Close it and try again."
                )
                .to_string(),
                corrent_core::lock::LockError::Held => {
                    t!("Corrent is already running with this data folder. Close it and try again.").to_string()
                }
                corrent_core::lock::LockError::Io(err) => {
                    t!("Corrent couldn't lock its data folder {path}: {e}", path = paths.data_dir.display(), e = err)
                }
            };
            eprintln!("{text}");
            rfd::MessageDialog::new()
                .set_title("Corrent")
                .set_description(&text)
                .set_level(rfd::MessageLevel::Error)
                .show();
            std::process::exit(1);
        }
    };

    // Serve forwarded launches right away: they are only queued, and a slow restore or settings
    // load below must not look like a hung instance to them.
    let (ipc_tx, ipc_rx) = std::sync::mpsc::channel();
    let ctx_cell: Arc<std::sync::OnceLock<egui::Context>> = Arc::new(std::sync::OnceLock::new());
    #[cfg(target_os = "macos")]
    integration::macos_events::install(&ipc_tx, &ctx_cell);
    if let Some(l) = listener {
        integration::single_instance::serve(l, ipc_tx, ctx_cell.clone());
    }

    let _log_guard = init_logging(&paths);
    let _log_flush = corrent_core::logfile::FlushGuard;
    crash::install_hook(paths.log_dir());
    let previous_run = crash::start(&paths.log_dir());
    if previous_run.unclean {
        tracing::warn!("previous run did not shut down cleanly (computer restarted since: {})", previous_run.rebooted);
    }
    let log_dir = paths.log_dir();
    std::thread::spawn(util::local_offset_secs);
    let restore_note = backup::apply_pending(&paths);
    // A media-server restore staged from the web interface replaces settings.toml too, so it goes
    // before the settings are read.
    let server_restore = corrent_server::backups::apply_pending(&paths.data_dir);
    match &server_restore {
        Some(Ok(m)) => tracing::info!("{m}"),
        Some(Err(e)) => tracing::error!("{e}"),
        None => {}
    }
    let (settings, first_run, settings_warning) = Settings::load(&paths);
    i18n::set_language(&settings.appearance.language);
    let data_notes: Vec<String> = restore_note
        .into_iter()
        .chain(server_restore.map(|r| r.unwrap_or_else(|e| e)))
        .chain(settings_warning)
        .collect();
    let launch: Vec<AddSource> = items.iter().filter_map(|a| AddSource::parse(a)).collect();
    let start_minimized = flag("--minimized") || settings.behavior.start_minimized;
    let tv = flag("--tv");

    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("corrent-rt").build() {
        Ok(rt) => rt,
        Err(e) => return Err(eframe::Error::AppCreation(Box::new(e))),
    };
    let _guard = rt.enter();
    #[cfg(unix)]
    integration::end_session::install_signals(rt.handle());

    let icon = app::icon::app_icon();
    let viewport = egui::ViewportBuilder::default()
        .with_title("Corrent")
        // Wayland matches windows to their .desktop file by this id (Flatpak uses its app ID).
        .with_app_id(std::env::var("FLATPAK_ID").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "corrent".into()))
        .with_inner_size([1280.0, 800.0])
        // Mica spike (roadmap B5b): the window must be created transparent.
        .with_transparent(mica_spike())
        .with_min_inner_size([760.0, 480.0])
        .with_drag_and_drop(true)
        .with_decorations(cfg!(target_os = "macos"))
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_icon(Arc::new(icon))
        .with_visible(!start_minimized);
    let options = eframe::NativeOptions {
        viewport,
        persist_window: true,
        persistence_path: Some(paths.data_dir.join("window.ron")),
        centered: true,
        ..Default::default()
    };

    let handle = rt.handle().clone();
    let result = eframe::run_native(
        "Corrent",
        options,
        Box::new(move |cc| {
            let _ = ctx_cell.set(cc.egui_ctx.clone());
            Ok(Box::new(
                app::CorrentApp::new(cc, handle, paths, settings, first_run, launch, ipc_rx, start_minimized)
                    .with_previous_run(previous_run)
                    .with_tv(tv)
                    .with_data_notes(data_notes),
            ))
        }),
    );
    // Let blocking hash/IO tasks finish briefly, but never hang on exit.
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
    crash::finish(&log_dir);
    result
}

fn handler_cli(register: bool, unregister: bool) -> i32 {
    use integration::os_handler;
    #[cfg(windows)]
    attach_console();
    let result = if register {
        os_handler::register(true, true)
    } else if unregister {
        integration::app_identity::remove();
        os_handler::unregister(true, true)
    } else {
        Ok(())
    };
    if let Err(e) = &result {
        eprintln!("error: {e:#}");
    }
    let st = os_handler::status();
    println!("magnet:   {}", st.magnet.label());
    println!(".torrent: {}", st.torrent.label());
    i32::from(result.is_err())
}

/// Release builds use the GUI subsystem; reattach to the parent console for CLI output.
#[cfg(windows)]
fn attach_console() {
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX);
    }
}

fn init_logging(paths: &Paths) -> Option<()> {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_env("CORRENT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,librqbit=info,librqbit_dht=warn,tracing=warn"));
    // Written by a background thread and rotated by size (corrent_core::logfile).
    let file_layer = corrent_core::logfile::init(&paths.log_dir(), "corrent")
        .map(|w| tracing_subscriber::fmt::layer().with_ansi(false).with_writer(w));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .with(app::log::LogCapture)
        .try_init()
        .ok()
}

/// The Mica spike (roadmap B5b), debug builds only: no `CORRENT_DEV_` name may reach a release
/// binary (STABILITY.md R7).
#[cfg(debug_assertions)]
fn mica_spike() -> bool {
    std::env::var_os("CORRENT_DEV_MICA").is_some()
}

#[cfg(not(debug_assertions))]
fn mica_spike() -> bool {
    false
}
