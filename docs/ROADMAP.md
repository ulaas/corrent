# Corrent Roadmap

Drafted 2026-10-08 by a Fable planning agent from the code and docs; corrected after a second Fable
review that verified the API claims against librqbit 9.0.1, egui/eframe 0.36.2 and egui_kittest 0.36.2.

## Where we are

Corrent is a working v1: a ~10.5k-line Rust 2024 crate (eframe/egui 0.36.2, librqbit 9.0.1 with a
vendored DHT patch). The engine actor (`src/engine/actor.rs`) owns the librqbit `Session`, publishes
`ArcSwap<Snapshot>` every 500 ms and never lets the UI thread touch I/O. App-side state lives in
`src/store.rs`; settings in `src/config.rs`. Add flow, OS magnet/.torrent integration, single
instance, list/sidebar/details/settings, queue, seeding limits, scheduler, watch folders, tray,
toasts, custom title bar and failed-restore recovery are done.

What is thin: 19 unit tests and nothing else (no engine integration tests, no UI tests, no CI, no
clippy/fmt gate), no panic hook or crash log, no schema version in `settings.toml`/`store.json`,
blocking `rfd::FileDialog` calls, disk-full/permission errors collapse into generic strings,
macOS/Linux never run, and the DHT fix still lives in `vendor/`. Engine features that are missing
upstream (encryption, sequential, per-file priority, runtime per-torrent limits) are blocked;
several librqbit capabilities we already link are unused: `create_torrent`, `FileStream`
streaming, `http-api`, `upnp-serve-adapter` (DLNA, which also requires `http-api`), `allowlist_url`,
`AddTorrentOptions.ratelimits`, `initial_peers`.

Legend: **[U]** = needs an upstream librqbit change (PR or fork); **[u]** = uses an existing
upstream feature we don't enable yet.

---

## Track A — Robustness

### A1. Test foundation
- **What:** integration tests in `tests/`, a `src/testing.rs` helper; `engine`, `store`, `config`,
  `util` testable without a window.
- **How:** extract pure logic from `Actor::tick` (`run_queue`, `run_seed_limits`, state mapping in
  `build_view`) into free functions over `&[TorrentView]` + `&Store`. Tests for `Store::queue_move`,
  `TorrentMeta::absorb_counters`, `StatusFilter::matches`, `compute_rows`, `AddSource::parse`,
  `expand_command`/`split_args`. Target 80+ tests.
- **Accept:** green on all three OSes; no test touches the real profile or network.

### A2. Engine integration tests over loopback
- **What:** two librqbit sessions on `127.0.0.1` exchanging a generated torrent, driven through
  `EngineHandle`.
- **How:** seeder via `create_and_serve_torrent` on a temp dir [u]; leecher via `EngineCommand::Add`
  with `initial_peers` (new optional `AddRequest` field), `dht: None`, `disable_trackers`, listener on
  `127.0.0.1:0`, and session + DHT persistence pointed at a temp dir (both default to the user
  profile). No refactor is needed for headless runs: `egui::Context::default()` works without a window.
  Cases: add → complete → `Completed`; pause/resume; recheck never re-downloads; move storage;
  delete with/without data; restart keeps fast resume; failed-restore recovery with a locked file.
- **Accept:** < 60 s, offline, no flakes over 20 CI runs.

### A3. UI snapshot tests (egui_kittest)
- **What:** headless renders of toolbar, empty state, table with fake rows, each details tab, add
  dialog, settings pages, light + dark.
- **How:** `egui_kittest` 0.36.2 dev-dependency with only `snapshot` + `wgpu` (not its `eframe`
  feature, which would compile accesskit and wgpu into eframe for every `cargo test`); drive panels
  through `Harness::new_ui` with a `CorrentApp::ui_for_test(&mut Ui, Snapshot)` that bypasses engine,
  tray and IPC. PNGs in `tests/snapshots/`, `UPDATE_SNAPSHOTS=1` to refresh. Snapshots differ by
  OS/font renderer, so compare on Linux only from the start; other OSes run the same tests without
  image comparison.

### A4. CI and gates
- GitHub Actions matrix (Windows/Linux/macOS): `fmt --check`, `clippy -D warnings`, `test`,
  release build with artifacts, `cargo deny`. `rustfmt.toml` keeps the current wide line width.
  Linux runners need `libxkbcommon-dev libwayland-dev libx11-dev`. The owner pushes (no remote git
  from Claude).

### A5. Panic handling and crash logs
- `std::panic::set_hook` writes `logs/crash-<ts>.txt` (message, location, backtrace, version, OS,
  last 200 log lines) and flushes the store; next start shows "Corrent closed unexpectedly — view
  report". Keep the 5 newest reports.
- **Accept:** debug-only `CORRENT_DEV_PANIC=ui|engine` produces a report; restart keeps all torrents.

### A6. Actor supervision
- Respawning the actor means rebuilding the whole session, so keep it simple: a health timestamp in
  `Snapshot`, an "Engine stopped" banner with **Restart engine** (reusing the settings-driven restart
  path in `actor.rs`), the A5 crash report, and a bounded command queue (256) while restarting.
- **Accept:** an integration test panics the actor; the banner appears and Restart brings all
  torrents back.

### A7. Schema versioning and migrations
- `schema_version` on `Settings` and `StoreData` (0 = pre-versioning) with a migration chain; the
  Downloads→Downloads/Corrent fix becomes migration 0→1. Dated backup before any migration; refuse
  to overwrite files from a newer version.
- **Recovery first:** today a corrupt `settings.toml` is copied to `.toml.bad` and silently replaced
  by defaults, and an unreadable `store.json` falls back to `.bak` or silently starts empty. Add a
  `.json.bad` copy, partial TOML salvage (parse as `toml::Table`, keep valid sections) and a startup
  banner "Settings/data could not be read — Restore backup / Open folder".
- **Also:** data-folder backup/export and import (settings, store, metainfo backups, engine session)
  from Settings → Advanced; before a librqbit upgrade, back up the engine's own `session/` files too.
- **Accept:** fixtures from every release load losslessly (`tests/fixtures/`); corrupt files never
  lose data silently.

### A8. Disk-full and permission errors
- Typed `IoFailure { DiskFull | Permission | Locked | Other }` from `write_atomic`; torrent errors
  mentioning `os error 28/112/5/13` mapped to actionable row states ("Choose another folder",
  "Retry"). Sticky toasts with actions (B12); pre-flight free-space check before Move; warning when
  the save folder has < 1 GB free.
- **Accept:** on a tiny VHD/tmpfs, adds fail gracefully, the store never corrupts.

### A9. Scale: 5,000 torrents
- Criterion benchmarks with synthetic `TorrentView`s. Hot spots: per-tick clones of history and
  every view (move static data behind `Arc`, per-row generations), `publish()` cloning the whole
  snapshot, `visible_rows` re-sorting on every tick (key on a structural generation),
  `sidebar::Counts` O(n) per frame (compute in the actor).
- **Accept:** the UI stays < 8 ms per frame with 5,000 synthetic views and the actor tick < 20 ms;
  engine-side costs (librqbit tasks per torrent) are measured separately rather than budgeted.

### A10. Async file dialogs
- Replace blocking `rfd::FileDialog` with `rfd::AsyncFileDialog` (`app/pickers.rs`), results via a
  channel drained in `logic()`.
- **Accept:** the window keeps repainting while a picker is open.

### A11. Upstream the DHT patch [U]
- Prepare `docs/upstream/dht-connreset.patch` + PR text with a regression test; the owner opens the
  PR. Remove `vendor/librqbit-dht` once a release contains it.

### A12. Dependency and MSRV policy
- MSRV 1.95 job, weekly dependency updates, `cargo deny` (MIT/Apache/OFL only, advisories),
  egui/eframe/librqbit pinned exactly and bumped in dedicated commits. Documented in
  `docs/DEPENDENCIES.md`.

---

## Track B — Look and feel

| Item | What / why | How | Acceptance |
|---|---|---|---|
| B1 Onboarding | First-run flow: save folder, theme, handler registration, "add your first torrent". | `app/onboarding.rs`, 3 skippable steps, reuses the handler card. | Honors `handler_prompt_dismissed`; screenshot-tested. |
| B2 Empty states | Every empty list/tab gets an icon, short copy and a primary action. | Extend `widgets::empty_state` with an optional action. | Checklist covers table, tabs, peers, trackers, categories, log. |
| B3 Motion | Row fade in/out, progress easing, panel slide, toast slide; off when animations are disabled. | `ui/motion.rs` with shared durations (≤ 120 ms). | No idle repaint loops. |
| B4 Typography | Stable numbers and clearer hierarchy. | egui can't enable OpenType features at runtime, so either freeze `tnum` into the Inter subset (fontTools featfreeze) or keep numbers in JetBrains Mono; type tokens (display/title/body/caption/mono); a Medium weight would be a third subset (PUA-free). | Numeric columns don't jitter. |
| B5a Windows 11 corners | Rounded corners and shadow for the borderless window. | `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE=33)` on the HWND from `CreationContext`'s window handle. | Rounded on Win11, unchanged elsewhere. |
| B5b Mica (spike) | Translucent backdrop behind the title bar. | Needs `with_transparent(true)` at creation (restart to toggle), transparent `clear_color`, `DwmExtendFrameIntoClientArea`; alpha through a glow framebuffer is unverified. Time-boxed spike, not a deliverable. *Result (M27): **no-go.** Alpha through glow works and DWM applies Mica, but extending the frame makes DWM draw its own caption buttons over the custom title bar, and the tint barely shows behind panels that must stay mostly opaque for legibility. Kept as a debug-only hook (`CORRENT_DEV_MICA=1`, `win_chrome::mica`). Revisit if winit/egui gain native backdrop support with a custom title bar.* | Decide go/no-go with screenshots. |
| B6 Theme editor | Edit palette live, import/export `theme.toml`, 4 presets. | `Palette: Serialize`, `Appearance.custom_palette`. | Round-trips; invalid files rejected clearly. |
| B7 High contrast | Preset + follow OS high-contrast. | 7:1 palette, 2 px focus rings, state never by color alone. | Contrast script passes. |
| B8 Accessibility | Screen-reader support, labels on every icon button, focus order, announced toasts. | Enable eframe's `accesskit` feature in the production build (currently off); `icon_button` requires a label. | AccessKit tree asserts names. |
| B9 i18n | Language setting, Turkish as the second locale. | Touches every string: first extract a string table, adopt Fluent later; schedule after the main features. *Done (M27) gettext-style: the English text is the key, named placeholders, singular/plural as separate keys; Fluent is not needed for two languages.* | CI grep gate for hard-coded strings. |
| B10 Row sparklines | Optional per-row down/up sparkline column. | Compact history tail in `TorrentView` only when visible. | Stays within the A9 budget. |
| B11 Drag to reorder | Drag rows to reorder the queue; drag headers to reorder columns. | `dnd_drag_source`, new `QueueMove::To(pos)`. | Multi-select keeps order; persisted. |
| B12 Toasts with actions | "Undo remove", "Open folder", "Retry", "Show"; sticky errors; history in the Log window. | `ToastAction` enum resolved to commands. | Undo re-adds a removed torrent. |
| B14 Batch add | Dropping or picking 20 files opens one dialog, not 20. | "Add N torrents" dialog with shared folder/category/tags and a per-file list. | One dialog per batch. |
| B15 Notifications | Native notifications look like they come from Corrent. | Set a Corrent AppUserModelID and Start Menu shortcut (installer); actions only on Linux, in-app toasts (B12) elsewhere. | Windows toasts show Corrent's name and icon. |
| B13 Panel polish | Title bar, sidebar (collapsible sections), table (sticky header, zebra option), details, settings (search highlights, dirty markers), status bar. | One commit per panel with before/after screenshots. | Design checklist per panel. |

---

## Track C — Features

- **C1 RSS auto-download:** `feed-rs` + `reqwest` in `engine/rss.rs`; rules (title regex, episode
  filter, category, save path, paused); seen-GUID set in the store (needs A7). Sidebar "Feeds",
  article list, rule editor with match preview. *Accept:* matching items are added exactly once,
  also across restarts.
- **C2 Search:** Torznab (Jackett/Prowlarr) providers configured by the user; results tab with Add.
  No scraping of public sites.
- **C3 Torrent creator [u]:** `librqbit::create_torrent` / `create_and_serve_torrent`; dialog with
  file/folder picker, trackers, piece size, save `.torrent`, copy magnet, seed immediately. Private
  flag/comment/web seeds need a small upstream PR [U].
- **C4 Streaming and preview [u]:** `ManagedTorrent::stream(file_id)` behind a localhost HTTP
  endpoint with Range support; "Open in player". A cheap interim "play while downloading": reading a
  `FileStream` makes the engine prioritize pieces at the read position. DLNA via
  `upnp-serve-adapter` later (requires enabling `http-api` as well).
- **C5a qBittorrent-compatible API:** axum router on top of `EngineHandle` implementing exactly
  `auth/login`, `app/version`, `app/webapiVersion`, `torrents/info|add|pause|resume|delete|
  setCategory|categories` and `sync/maindata`, with a Sonarr/Radarr smoke test. Localhost-only by
  default; password/token required for LAN.
- **C5b Web UI:** a separate, later item once the API is stable.
- **C6 Engine gaps:** per-torrent speed limits (at add time [u]; runtime needs a small
  `set_ratelimits` upstream [U]); sequential download and per-file priorities [U] via piece-priority
  APIs; protocol encryption [U] is large — stay "unsupported" unless upstream adds it. Small APIs may
  live on a `[patch.crates-io]` branch while PRs are open, like the DHT patch. Per-category speed
  limits follow once runtime per-torrent limits exist.
- **C7 IP filter UI [u]:** blocklist + allowlist URLs or local files (gzip supported), a manual
  CIDR list written to a file, loaded-range count. Lists load only when the session starts, so
  "Refresh" restarts the engine.
- **C8 Import from other clients:** qBittorrent (`BT_backup` + `.fastresume`), Transmission; Deluge
  only as ".torrent files" (its state is a Python pickle); adds paused with `overwrite: true` (re-verify, never re-download), keeps categories/tags.
- **C9 Shutdown when done:** nothing / exit / sleep / shut down, 30 s cancellable countdown.
- **C10 Update checks:** opt-in, GitHub Releases JSON, toast with a Download action; no auto-install.
- **C11 Packaging:** MSI (cargo-wix, per-user, optional handler registration) + winget; portable
  zip with the `portable` marker; macOS app bundle + dmg (notarization with the owner's keys);
  AppImage and Flatpak with `.desktop`/MIME from `packaging/`.

---

## Milestones

Ordered so visible improvements land early while the safety net grows. CI can only run once the
owner pushes the repo, so nothing local is gated on it.

| M | Content | Depends | Upstream | Status |
|---|---|---|---|---|
| M1 | Async file dialogs (A10) — known bug, small | – | – | Done |
| M2 | Panic hook, crash logs, "closed unexpectedly" banner (A5) | – | – | Done |
| M3 | Corrupt-file recovery, data backup/export (A7 recovery part) | – | – | Done |
| M4 | Pure-logic extraction + 80+ unit tests (A1) | – | – | Done |
| M5 | Empty states + toasts with actions (B2, B12) | – | – | Done |
| M6 | First real run on Linux (VM/WSLg) and macOS: portal file dialogs, KSNI tray, IPC, MIME registration, Apple Events | – | – | Prepared — needs real hardware (`docs/PLATFORM-TESTING.md`); Linux cross-builds, macOS type-checks |
| M7 | CI config, fmt/clippy gates, cargo-deny, MSRV job (A4, A12) — owner pushes | – | – | Done — runs once pushed |
| M8 | Loopback engine integration tests (A2) | M4 | [u] | Done |
| M9 | Schema versioning + migrations (A7) | M3 | – | Done |
| M10 | Typed I/O errors, disk-full/permission handling (A8) | M5, M9 | – | Done |
| M11 | Engine health banner + restart (A6) | M2 | – | Done |
| M12 | Batch add dialog, Windows 11 corners (B14, B5a) | M1 | – | Done |
| M13 | kittest snapshot tests, Linux-only comparison (A3) | M4 | – | Done (deviation, see below) |
| M14 | Typography, motion (B3, B4) | M13 | – | Done |
| M15 | Onboarding + per-panel polish (B1, B13), notification identity (B15) | M14 | – | Done |
| M16 | Torrent creator (C3) | M1, M8 | [u], small [U] | Done |
| M17 | Streaming + open in player (C4) | M8 | [u] | Done |
| M18 | RSS feeds and rules (C1) | M9, M10 | – | Done |
| M19 | Torznab search (C2) | M18 | – | Done |
| M20 | qBittorrent-compatible API (C5a) | M8, M11 | – | Done |
| M21 | Scale pass with benchmarks; row sparklines; drag to reorder (A9, B10, B11) | M8 | – | Done (deviation) |
| M22 | High contrast, theme editor, accessibility (B7, B6, B8) | M13 | – | Done |
| M23 | Per-torrent limits, IP filter UI (C6 partial, C7) | M9 | [U] runtime limits | Done |
| M24 | Sequential + per-file priorities; per-category limits (C6) | M23 | [U] | Done |
| M25 | Import from other clients (C8); shutdown when done; update checks (C9, C10) | M9, M1 | – | Done |
| M26 | Packaging: MSI/winget, portable zip, dmg, AppImage/Flatpak (C11) | M7 | – | Prepared — first build on CI with a tag |
| M27 | i18n string table → Fluent, Turkish (B9); web UI (C5b); Mica spike (B5b) | M15, M20 | – | Done (Turkish, gettext-style; web UI; Mica no-go) |
| — | DHT patch upstream package (A11), any time | – | [U] | Patches ready in `docs/upstream/` — owner opens the PRs |

## Deviations from this plan (recorded at completion)

- **A3/M13:** UI tests are headless egui_kittest tests that assert behaviour and the AccessKit
  tree (15 tests) rather than PNG snapshots: rendering images needs kittest's wgpu feature and a
  GPU, which CI runners don't reliably have.
- **A5:** the store is flushed by its `Drop` (engine panics and normal unwinding) rather than by
  the panic hook itself; with `panic = "abort"` it would not be, so the profile keeps unwinding.
- **A7:** migration fixtures are inline constants in the tests, not files under `tests/fixtures/`.
- **A9/M21:** an ignored, explicitly run perf test (`perf_5000_torrents`, results in
  `docs/PERFORMANCE.md`) instead of Criterion benchmarks.
- **A11:** the DHT patch has no automated regression test (the failure needs Windows ICMP
  port-unreachable traffic); it is documented in `vendor/librqbit-dht/CORRENT-PATCH.md`.
- **B9/M27:** gettext-style catalog (English text is the key) instead of Fluent; the CI gate is a
  unit test (`i18n::tests::no_hard_coded_ui_strings`).
- **B11:** columns are shown, hidden and moved left/right in the column chooser (right-click the
  header), not by dragging headers (egui_extras tables don't support header drag).
- **C6:** "sequential download" is not offered because librqbit already downloads each file in
  piece order; per-file priorities control the file order instead.

## Risks

1. **egui_kittest without a GPU on CI** — Linux-only snapshot job or AccessKit-tree fallback.
2. **Windows backdrop with a custom title bar** — possible ghosting/hit-test issues with `glow`;
   keep behind a setting with an opaque fallback.
3. **Upstream velocity** — single-maintainer project; keep minimal `[patch.crates-io]` branches
   with PRs open, documented like the DHT patch.
4. **Re-add semantics** — limits/trackers/move via re-add re-verify data; always keep the metainfo
   backup and warn about the size; Undo (B12) relies on the same backup.
5. **Schema migrations** — a bug loses user data; dated backups and release fixtures (M5 before any
   store-shape change).
6. **Scale refactor** — touches every panel; land behind benchmarks and snapshot tests.
7. **Web API security** — localhost-only default, auth for LAN, CSRF-safe.
8. **Platform drift** — macOS/Linux untested at runtime; CI and packaging are the first real runs.
9. **Repo rules** — no remote git from Claude; CI, upstream PRs and releases need the owner to push;
   never add `Co-Authored-By`.
10. **Test build cost** — dev-dependency feature unification (kittest) can pull wgpu/accesskit into
    eframe for `cargo test`; keep kittest features minimal.
11. **Snapshot determinism** — images vary by OS/GPU/fonts; compare on Linux with tolerances.
12. **Distribution** — unsigned MSI triggers SmartScreen; macOS notarization needs a paid Apple
    account; winget needs public release URLs.
13. **Engine upgrades** — a librqbit major bump may change its `session/` layout; back it up and
    test restore before upgrading.
