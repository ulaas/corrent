#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod engine;
mod store;
mod ui;
mod util;

use std::sync::Arc;

use config::{Paths, Settings};
use engine::types::AddSource;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let portable = args.iter().any(|a| a == "--portable");
    let paths = Paths::resolve(portable);
    let _ = std::fs::create_dir_all(&paths.data_dir);
    let _log_guard = init_logging(&paths);

    let (settings, first_run) = Settings::load(&paths);
    let launch: Vec<AddSource> = args.iter().filter(|a| !a.starts_with("--")).filter_map(|a| AddSource::parse(a)).collect();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("corrent-rt")
        .build()
        .expect("failed to build tokio runtime");
    let _guard = rt.enter();

    let icon = app::icon::app_icon();
    let viewport = egui::ViewportBuilder::default()
        .with_title("Corrent")
        .with_app_id("corrent")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([760.0, 480.0])
        .with_drag_and_drop(true)
        .with_icon(Arc::new(icon));
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
        Box::new(move |cc| Ok(Box::new(app::CorrentApp::new(cc, handle, paths, settings, first_run, launch)))),
    );
    // Let blocking hash/IO tasks finish briefly, but never hang on exit.
    rt.shutdown_timeout(std::time::Duration::from_secs(3));
    result
}

fn init_logging(paths: &Paths) -> Option<()> {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_env("CORRENT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,librqbit=info,librqbit_dht=warn,tracing=warn"));
    let _ = std::fs::create_dir_all(paths.log_dir());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_dir().join("corrent.log"))
        .ok();
    let file_layer = file.map(|f| {
        tracing_subscriber::fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(f))
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .with(app::log::LogCapture)
        .try_init()
        .ok()
}
