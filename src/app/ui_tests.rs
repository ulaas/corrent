//! Headless UI tests with egui_kittest: real app, fake snapshot, queries over the AccessKit tree.

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::{CorrentApp, StatusFilter};
use crate::config::{Paths, Settings};
use crate::engine::types::{InfoHash, Snapshot, TorrentState, TorrentView};

struct Fixture {
    _rt: tokio::runtime::Runtime,
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn settings(dir: &std::path::Path) -> Settings {
    let mut s = Settings::default();
    s.downloads.save_path = dir.join("dl");
    s.connection.listen_address = "127.0.0.1".into();
    s.connection.random_port = true;
    s.connection.upnp = false;
    s.bittorrent.dht = false;
    s.bittorrent.lsd = false;
    s.bittorrent.trackers_enabled = false;
    s.behavior.handler_prompt_dismissed = true;
    s
}

fn torrent(n: u8, name: &str, state: TorrentState) -> TorrentView {
    TorrentView {
        hash: InfoHash([n; 20]),
        name: name.into(),
        state,
        size: 1 << 20,
        done: if state == TorrentState::Seeding { 1 << 20 } else { 1 << 19 },
        progress: if state == TorrentState::Seeding { 1.0 } else { 0.5 },
        completed_at: (state == TorrentState::Seeding).then_some(1),
        has_metadata: true,
        queue_pos: n as u32,
        ..Default::default()
    }
}

fn app(name: &str, torrents: Vec<TorrentView>) -> (Fixture, CorrentApp) {
    let dir = std::env::temp_dir().join(format!("corrent-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let ctx = egui::Context::default();
    let (_tx, ipc) = std::sync::mpsc::channel();
    let paths = Paths { data_dir: dir.join("data"), portable: true };
    let mut app =
        CorrentApp::build(&ctx, None, rt.handle().clone(), paths, settings(&dir), false, Vec::new(), ipc, false, false);
    app.snap = Arc::new(Snapshot { torrents, generation: 1, ..Default::default() });
    (Fixture { _rt: rt, dir }, app)
}

fn harness(app: CorrentApp) -> Harness<'static, CorrentApp> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui_state(|ui, app: &mut CorrentApp| app.frame_ui(ui), app);
    // The harness owns its own context; the app installs its fonts there on the first frame.
    h.state_mut().ctx = h.ctx.clone();
    h
}

#[test]
fn empty_list_offers_add_actions() {
    let (_f, app) = app("empty", vec![]);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("No torrents yet");
    // Offered both in the toolbar and in the empty state.
    assert!(h.query_all_by_label_contains("Add torrent file").count() >= 2);
    assert!(h.query_all_by_label_contains("Add magnet link").count() >= 2);
}

#[test]
fn toolbar_and_window_controls_are_labeled() {
    let (_f, app) = app("toolbar", vec![]);
    let mut h = harness(app);
    h.run_steps(3);
    for label in ["Add torrent file", "Settings", "Resume", "Pause", "Remove"] {
        assert!(h.query_by_label(label).is_some(), "toolbar button '{label}' is labeled");
    }
}

#[test]
fn clicking_a_row_selects_it_and_shows_details() {
    let (_f, app) =
        app("select", vec![torrent(1, "Alpha", TorrentState::Downloading), torrent(2, "Beta", TorrentState::Seeding)]);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Beta").click();
    h.run_steps(3);
    assert!(h.state().selected.contains(&InfoHash([2; 20])));
    assert!(!h.state().selected.contains(&InfoHash([1; 20])));
    // General tab shows the selected torrent's hash.
    h.get_by_label(&InfoHash([2; 20]).to_hex());
}

#[test]
fn sidebar_filter_hides_other_torrents() {
    let (_f, app) = app(
        "filter",
        vec![
            torrent(1, "Downloading one", TorrentState::Downloading),
            torrent(2, "Seeding one", TorrentState::Seeding),
        ],
    );
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.query_by_label("Downloading one").is_some());
    h.get_by_label("Seeding").click();
    h.run_steps(3);
    assert_eq!(h.state().prefs.status_filter, StatusFilter::Seeding);
    assert!(h.query_by_label("Downloading one").is_none());
    assert!(h.query_by_label("Seeding one").is_some());
}

#[test]
fn search_filters_and_empty_search_state() {
    let (_f, app) = app(
        "search",
        vec![torrent(1, "Ubuntu ISO", TorrentState::Seeding), torrent(2, "Debian ISO", TorrentState::Seeding)],
    );
    let mut h = harness(app);
    h.state_mut().search = "debian".into();
    h.run_steps(3);
    assert!(h.query_by_label("Ubuntu ISO").is_none());
    assert!(h.query_by_label("Debian ISO").is_some());
    h.state_mut().search = "nothing-matches".into();
    h.run_steps(3);
    h.get_by_label("No matches");
}

#[test]
fn delete_confirmation_defaults_to_keeping_files() {
    let (_f, app) = app("delete", vec![torrent(1, "Gamma", TorrentState::Seeding)]);
    let mut h = harness(app);
    h.run_steps(2);
    h.state_mut().request_delete(vec![InfoHash([1; 20])], false);
    h.run_steps(3);
    h.get_by_label("Remove torrent?");
    let keep = h.get_by_label("Remove from list, keep downloaded files");
    let node = keep.accesskit_node();
    let on = node.is_selected() == Some(true) || matches!(node.toggled(), Some(egui::accesskit::Toggled::True));
    assert!(on, "keeping files is the default");
}

#[test]
fn settings_window_navigation_and_search() {
    let (_f, app) = app("settings", vec![]);
    let mut h = harness(app);
    h.state_mut().open_settings();
    h.run_steps(3);
    for page in
        ["General", "Downloads", "Connection", "Speed", "BitTorrent", "Queue", "Seeding", "Appearance", "Advanced"]
    {
        assert!(h.query_all_by_label(page).next().is_some(), "settings page '{page}' listed");
    }
    h.get_by_label("Connection").click();
    h.run_steps(3);
    h.get_by_label("Use a SOCKS5 proxy for peer connections");
    h.state_mut().settings_draft.as_mut().unwrap().search = "proxy".into();
    h.run_steps(3);
    assert!(h.query_all_by_label("Appearance").next().is_none(), "search narrows the page list");
}

#[test]
fn magnet_dialog_counts_recognized_links() {
    let (_f, app) = app("magnet", vec![]);
    let mut h = harness(app);
    h.state_mut().modal = Some(super::dialogs::Modal::Magnet {
        text: "magnet:?xt=urn:btih:0101010101010101010101010101010101010101\nnot a link\nhttps://x/y.torrent".into(),
    });
    h.run_steps(3);
    h.get_by_label("2 item(s) recognized");
}
