//! Headless UI tests with egui_kittest: real app, fake snapshot, queries over the AccessKit tree.

use std::path::PathBuf;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use super::{CorrentApp, StatusFilter};
use crate::config::{Paths, Settings};
use crate::engine::types::{InfoHash, Snapshot, TorrentState, TorrentView};

struct Fixture {
    rt: Option<tokio::runtime::Runtime>,
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Stop the engine's tasks (closing its files) before deleting; Windows refuses to remove
        // open files, so retry briefly.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(std::time::Duration::from_secs(2));
        }
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.dir).is_ok() || !self.dir.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
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
    // Tests read English labels, whatever the machine's language.
    s.appearance.language = "en".into();
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
    (Fixture { rt: Some(rt), dir }, app)
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
    for page in [
        "General",
        "Downloads",
        "Connection",
        "Speed",
        "BitTorrent",
        "Queue",
        "Seeding",
        "Feeds",
        "Search",
        "Remote control",
        "Appearance",
        "Playback",
        "Advanced",
    ] {
        assert!(h.query_all_by_label(page).next().is_some(), "settings page '{page}' listed");
    }
    h.get_by_label("Playback").click();
    h.run_steps(3);
    h.get_by_label("Exclusive mode");
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
    h.get_by_label("Recognized: 2");
}

#[test]
fn first_run_shows_onboarding_and_finishes() {
    let (_f, mut app) = app("onboarding", vec![]);
    app.onboarding = Some(super::onboarding::Onboarding { step: 0 });
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Welcome to Corrent");
    h.get_by_label("Next").click();
    h.run_steps(3);
    h.get_by_label("Pick a look");
    h.get_by_label("Next").click();
    h.run_steps(3);
    h.get_by_label("Open magnet links with Corrent?");
    h.get_by_label("Get started").click();
    h.run_steps(3);
    assert!(h.state().onboarding.is_none());
    assert!(h.state().settings.behavior.handler_prompt_dismissed);
}

#[test]
fn sidebar_sections_collapse() {
    let (_f, app) = app("collapse", vec![torrent(1, "A", TorrentState::Seeding)]);
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.query_by_label("Uncategorized").is_some());
    h.get_by_label_contains("CATEGORIES").click();
    h.run_steps(3);
    assert!(h.query_by_label("Uncategorized").is_none());
    assert!(h.state().prefs.collapsed.contains(&"categories".to_string()));
}

#[test]
fn activity_column_and_queue_drag() {
    let (_f, mut app) =
        app("drag", vec![torrent(1, "Alpha", TorrentState::Downloading), torrent(2, "Beta", TorrentState::Queued)]);
    app.prefs.columns.push(super::Col::Activity);
    app.prefs.sort = super::Col::Queue;
    app.prefs.sort_asc = true;
    let hist: std::collections::HashMap<InfoHash, Vec<(u32, u32)>> =
        [(InfoHash([1; 20]), (0..40).map(|i| (i * 10, i)).collect())].into_iter().collect();
    let snap = Snapshot { row_history: Arc::new(hist), ..(*app.snap).clone() };
    app.snap = Arc::new(snap);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Activity");
    let from = h.get_by_label("Beta").rect().center();
    let to = h.get_by_label("Alpha").rect().center() - egui::vec2(0.0, 8.0);
    h.event(egui::Event::PointerMoved(from));
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.run_steps(2);
    for i in 1..=5 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 5.0)));
        h.run_steps(1);
    }
    assert_eq!(h.state().queue_drag.as_deref(), Some(&[InfoHash([2; 20])][..]), "dragging Beta");
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.run_steps(2);
    assert!(h.state().queue_drag.is_none(), "dropped");
}

#[test]
fn high_contrast_preset_previews_live_and_reverts_on_cancel() {
    let (_f, app) = app("contrast", vec![torrent(1, "Alpha", TorrentState::Downloading)]);
    let mut h = harness(app);
    h.state_mut().open_settings_at(super::settings_window::Page::Appearance);
    h.run_steps(3);
    assert!(!h.state().palette(&h.ctx).high_contrast);
    h.get_by_label("High contrast").click();
    h.run_steps(3);
    assert!(h.state().palette(&h.ctx).high_contrast, "previewed before applying");
    assert_eq!(h.state().settings.appearance.preset, crate::config::PalettePreset::Corrent, "not saved yet");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!h.state().palette(&h.ctx).high_contrast, "cancel restores the palette");
}

#[test]
fn icon_buttons_have_accessible_names() {
    let (_f, app) = app("a11y", vec![torrent(1, "Alpha", TorrentState::Downloading)]);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Alpha").click();
    h.run_steps(3);
    for name in ["Hide details", "Open folder", "Copy"] {
        assert!(h.query_all_by_label(name).next().is_some(), "button '{name}' has a name");
    }
}

#[test]
fn when_done_counts_down_after_downloads_finish_and_can_be_cancelled() {
    use crate::integration::power::WhenDone;
    let (_f, app) = app("whendone", vec![torrent(1, "Alpha", TorrentState::Downloading)]);
    let mut h = harness(app);
    h.run_steps(2);
    h.state_mut().set_when_done(WhenDone::Sleep);
    h.run_steps(2);
    assert!(h.state().when_done.countdown.is_none(), "still downloading");
    let app = h.state_mut();
    app.snap = Arc::new(Snapshot {
        torrents: vec![torrent(1, "Alpha", TorrentState::Seeding)],
        generation: 2,
        ..Default::default()
    });
    h.run_steps(3);
    assert!(h.state().when_done.countdown.is_some(), "countdown started");
    h.get_by_label("All downloads finished");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().when_done.countdown.is_none());
    assert_eq!(h.state().when_done.action, WhenDone::Nothing, "cancel clears the action");
}

/// Scale check (A9): 5,000 torrents. Run with
/// `cargo test --release perf_5000 -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_5000_torrents() {
    use std::time::Instant;
    let states = [
        TorrentState::Downloading,
        TorrentState::Seeding,
        TorrentState::Paused,
        TorrentState::Stalled,
        TorrentState::Queued,
    ];
    let views: Vec<TorrentView> = (0..5000u32)
        .map(|i| {
            let mut h = [0u8; 20];
            h[..4].copy_from_slice(&i.to_le_bytes());
            TorrentView {
                hash: InfoHash(h),
                name: format!("Some.Linux.Distro.{i}.amd64.iso"),
                state: states[i as usize % states.len()],
                size: 1 << 30,
                done: (i as u64 * 7919) % (1 << 30),
                progress: (i % 100) as f32 / 100.0,
                down_bps: (i as u64 % 13) * 1000,
                up_bps: (i as u64 % 7) * 1000,
                category: (i % 3 == 0).then(|| format!("cat{}", i % 10)),
                tags: if i % 4 == 0 { vec!["a".into(), "b".into()] } else { vec![] },
                trackers: vec![format!("udp://tracker{}.example.org:1337/announce", i % 20)],
                save_path: PathBuf::from("C:/Downloads/Corrent"),
                content_path: PathBuf::from(format!("C:/Downloads/Corrent/file{i}")),
                added_at: i as i64,
                queue_pos: i,
                has_metadata: true,
                ..Default::default()
            }
        })
        .collect();
    let snap = Snapshot { torrents: views, generation: 1, ..Default::default() };

    let t = Instant::now();
    for _ in 0..20 {
        std::hint::black_box(snap.clone());
    }
    println!("snapshot clone: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 20.0);

    let (_fx, mut a) = app("perf", Vec::new());
    a.snap = Arc::new(snap.clone());
    let t = Instant::now();
    for g in 0..20 {
        let mut s = (*a.snap).clone();
        s.generation = 100 + g;
        a.snap = Arc::new(s);
        std::hint::black_box(a.visible_rows().len());
    }
    println!("snapshot clone + visible_rows: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 20.0);
    let t = Instant::now();
    for _ in 0..20 {
        a.counts.update(&a.snap);
    }
    println!("sidebar counts: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 20.0);

    let mut h = harness(a);
    for _ in 0..3 {
        h.step();
    }
    let frames = 30;
    let t = Instant::now();
    for g in 0..frames {
        let app = h.state_mut();
        let mut s = (*app.snap).clone();
        s.generation = 1000 + g;
        app.snap = Arc::new(s);
        h.step();
    }
    println!(
        "frame with a new snapshot (incl. kittest/AccessKit overhead): {:.2} ms",
        t.elapsed().as_secs_f64() * 1000.0 / frames as f64
    );
    let t = Instant::now();
    for _ in 0..frames {
        h.step();
    }
    println!("frame, unchanged snapshot: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / frames as f64);
}

#[test]
fn streaming_status_bar_and_window() {
    use crate::engine::types::{SessionView, StreamState, StreamView};
    let (_f, mut app) = app("streams", vec![torrent(1, "Sintel", TorrentState::Downloading)]);
    let stream = |id, state| StreamView {
        id,
        hash: InfoHash([1; 20]),
        torrent_name: "Sintel".into(),
        file_name: "Sintel/sintel.mkv".into(),
        file_len: 1 << 30,
        client: "127.0.0.1".into(),
        player: "VLC 3.0.20".into(),
        user_agent: "VLC/3.0.20 LibVLC/3.0.20".into(),
        state,
        connections: 1,
        requests: 3,
        served: 200 << 20,
        rate_bps: 2 << 20,
        position: 300 << 20,
        buffered_ahead: Some(40 << 20),
        availability: vec![255; 60].into_iter().chain(vec![0; 60]).collect(),
        ..Default::default()
    };
    app.snap = Arc::new(Snapshot {
        session: SessionView {
            stream: Some((5000, "tok".into())),
            streams: vec![stream(1, StreamState::Ended), stream(2, StreamState::Buffering)],
            stream_slots: (1, 3),
            ..Default::default()
        },
        ..(*app.snap).clone()
    });
    let mut h = harness(app);
    h.run_steps(3);
    // Only the active stream counts in the status bar.
    h.get_by_label_contains("Streaming to 1 player").click();
    h.run_steps(3);
    assert!(h.state().extras.streams, "the chip opens the Streaming window");
    h.get_by_label("Server on 127.0.0.1:5000");
    h.get_by_label("1 of 3 slots in use");
    h.get_by_label("Buffering");
    h.get_by_label("Ended");
    assert_eq!(h.query_all_by_label("VLC 3.0.20").count(), 2);
    h.get_by_label("40.0 MiB buffered ahead");
    // Active streams can be stopped, ended ones played again.
    assert_eq!(h.query_all_by_label_contains("Stop").count(), 1);
    assert_eq!(h.query_all_by_label_contains("Play again").count(), 1);
    assert_eq!(h.query_all_by_label_contains("Copy link").count(), 2);

    // Empty state.
    let app = h.state_mut();
    app.snap = Arc::new(Snapshot { session: SessionView::default(), ..(*app.snap).clone() });
    h.run_steps(3);
    h.get_by_label("Nothing is streaming");
    assert!(h.query_by_label_contains("Streaming to 1 player").is_none());
}

/// Steps the app (its per-frame logic included) until `done` or a timeout.
fn step_until(
    h: &mut Harness<'static, CorrentApp>,
    what: &str,
    done: impl Fn(&mut Harness<'static, CorrentApp>) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !done(h) {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        let ctx = h.ctx.clone();
        h.state_mut().media_tick(&ctx);
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

/// The media pages against the in-process server: a photo library (no libmpv needed) shows on
/// Home, a card opens its page, and in TV mode the arrow keys move the focus between cards.
#[test]
fn media_pages_browse_a_library_and_tv_mode_moves_focus() {
    let (f, mut app) = app("media", vec![]);
    let photos = f.dir.join("Photos/Trip");
    std::fs::create_dir_all(&photos).unwrap();
    for (name, c) in [("beach", [200, 120, 40]), ("forest", [20, 140, 60]), ("harbour", [30, 60, 160])] {
        image::RgbImage::from_pixel(64, 48, image::Rgb(c)).save(photos.join(format!("{name}.png"))).unwrap();
    }
    app.main_view = super::feeds::MainView::Media;
    let mut h = harness(app);
    step_until(&mut h, "the in-process server", |h| h.state().media.client.is_some());
    let client = h.state().media.client.clone().unwrap();
    let rt = f.rt.as_ref().unwrap();
    let lib: corrent_proto::library::Library = rt
        .block_on(client.send(
            reqwest::Method::POST,
            "/api/v1/libraries",
            &serde_json::json!({ "name": "Photos", "kind": "photos", "folders": [f.dir.join("Photos")] }),
        ))
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let page: serde_json::Value = rt.block_on(client.get(&format!("/api/v1/items?library_id={}", lib.id))).unwrap();
        if page["total"] == 3 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "scan: {page}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    h.state_mut().media.invalidate();
    step_until(&mut h, "the home rows", |h| h.query_by_label("beach").is_some());
    assert!(h.query_by_label_contains("Recently added in Photos").is_some());

    // TV mode: the first card has the focus, the right arrow moves it to the next one.
    h.state_mut().set_tv(true);
    step_until(&mut h, "a focused card", |h| h.state().ctx.memory(|m| m.focused()).is_some());
    let first = h.state().ctx.memory(|m| m.focused());
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    let second = h.state().ctx.memory(|m| m.focused());
    assert!(second.is_some() && second != first, "focus moved: {first:?} → {second:?}");
    // Enter opens the focused item; Escape goes back home.
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(matches!(h.state().media.page, Some(super::media::Page::Item(_))), "{:?}", h.state().media.page);
    h.key_press(egui::Key::Escape);
    let ctx = h.ctx.clone();
    h.state_mut().tv_tick(&ctx);
    h.run_steps(2);
    // TV settings: size and screensaver change from big buttons.
    h.get_by_label_contains("Settings").click();
    h.run_steps(3);
    assert_eq!(h.state().media.page, Some(super::media::Page::TvSettings));
    h.get_by_label("175%").click();
    h.get_by_label("Never").click();
    h.run_steps(3);
    assert_eq!(h.state().settings.appearance.tv_scale, 1.75);
    assert_eq!(h.state().settings.appearance.tv_screensaver_min, 0);
    h.get_by_label_contains("Leave TV mode").click();
    h.run_steps(2);
    assert!(!h.state().tv.on);
}

/// Scale check (U2): a 50,000-item library grid scrolls within the frame budget. Run with
/// `cargo test --release perf_50k_grid -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_50k_grid() {
    use corrent_proto::library::{Item, ItemKind};
    use std::time::Instant;
    let (_f, mut app) = app("perf-grid", vec![]);
    app.main_view = super::feeds::MainView::Media;
    // A client to nowhere: artwork requests fail quietly, as with a slow server.
    app.media.client = Some(super::media::client::MediaClient::new("http://127.0.0.1:9", "t", None).unwrap());
    let items: Vec<Item> = (0..50_000)
        .map(|i| Item {
            id: i,
            library_id: 1,
            kind: ItemKind::Movie,
            parent_id: None,
            title: format!("Movie number {i}"),
            year: Some(1950 + (i % 70) as i32),
            index: None,
            runtime_ms: Some(5_400_000),
            context: Vec::new(),
            images: Vec::new(),
            state: Default::default(),
            added_at: i,
            child_count: None,
            partial: false,
            grandparent_id: None,
            parent_index: None,
        })
        .collect();
    app.media.fill_grid(1, items);
    app.media.page = Some(super::media::Page::Library(1));
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.query_by_label("Movie number 0").is_some(), "the grid is drawn");
    // The pointer over the grid, so the wheel scrolls it.
    h.event(egui::Event::PointerMoved(egui::pos2(700.0, 400.0)));
    let mut times = Vec::new();
    for frame in 0..200 {
        // Scroll down steadily, with a long fling now and then.
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -if frame % 50 == 0 { 20_000.0 } else { 400.0 }),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        let t = Instant::now();
        h.step();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    assert!(h.query_by_label("Movie number 0").is_none(), "scrolled away from the top");
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    println!("50k grid: frame p50 {p50:.2} ms, p99 {p99:.2} ms");
    assert!(p99 < 16.0, "frame p99 {p99:.2} ms");
}
