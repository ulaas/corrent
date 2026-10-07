# Corrent — Implementation Plan (v1)

Inputs: `docs/research.md`. API names verified against local crate sources (librqbit 9.0.1, egui/eframe/egui_extras 0.36.2,
egui_plot 0.37.0, egui-phosphor 0.14.0).

## 0. Verified API facts
- eframe: `trait App { fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame); fn logic(&mut self, ctx, frame) {} fn save(..); fn on_exit(..) }`.
  `logic` also runs while the window is hidden (tray mode). `run_native(name, NativeOptions, AppCreator)`.
  Close interception: `i.viewport().close_requested()` + `ViewportCommand::CancelClose`. `ViewportCommand::Visible/Focus`.
  Dropped files: `raw.dropped_files: Vec<DroppedFileHandle>` (trait: `path()`, `bytes()`), `raw.hovered_files`.
- egui: `egui::Panel::{left,right,top,bottom}(id).resizable().default_size().size_range().exact_size().frame().show(ui, ..)`;
  `CentralPanel::default().show(ui, ..)`; `egui::Modal::new(id).show(ctx, ..)`; `Response::context_menu`; `MenuBar`;
  `ctx.set_theme(ThemePreference)`, `ctx.style_mut_of(Theme, ..)`, `consume_shortcut`, `animate_bool_with_time`.
- egui_extras TableBuilder: `.sense(Sense::click())`, `row.set_selected()`, `row.response()`, `body.rows(h, n, ..)`.
- egui_plot 0.37: `Line::new(name, PlotPoints)`.
- librqbit: `Session::new_with_opts`, `add_torrent(AddTorrent, Option<AddTorrentOptions>)` (list_only preview works for
  magnets → `ListOnlyResponse{info_hash, info, torrent_bytes, ..}`), `get(TorrentIdOrHash)`, `delete(id, files)`,
  `pause/unpause`, `update_only_files`, `with_torrents` (Fn), `stats_snapshot`, `get_dht().stats()`, `stop()`,
  `session.ratelimits.set_{upload,download}_bps(Option<NonZeroU32>)`. `ManagedTorrent::{id,name,info_hash,output_folder,
  only_files,is_paused,stats,with_metadata,shared().trackers,live()}`. Piece bitfield only via `Api::api_dump_haves`
  (bitvec BitBox<u8,Msb0>). ETA computed app-side. Persistence persists only {info_hash, trackers, output_folder,
  only_files, is_paused} → everything else lives in our store.

## 1. v1 scope
**Engine-backed:** add via .torrent / magnet / http URL / clipboard / drag-drop / CLI / OS link handler; metadata preview
with file tree; pause/resume (+all); delete with/without data; file include/exclude at runtime; global limits live;
DHT/PEX/LSD/UPnP/uTP/IPv6/SOCKS5/blocklist/bind device/peer limit/default trackers; JSON persistence + fastresume;
peers, files, pieces, speed graph, trackers (URL list).

**App-side:** categories, tags, added/completed dates, display rename; queueing; seeding ratio/time/inactivity limits;
alt-speed + schedule; per-torrent limits / move storage / recheck / edit trackers via forget + re-add
(`overwrite: true`, cached torrent bytes); watch folders; run-on-complete; notifications; tray; single instance;
close-to-tray; portable mode; **magnet/.torrent OS handler registration**.

**Hidden in v1:** per-file priority levels (include/exclude only), per-tracker status, encryption, BT v2, manual add-peer,
super-seeding, sequential toggle, Web UI, RSS and torrent creator (v1.x).

## 2. Architecture
Single crate. Features: `default = ["tray", "notify"]`.
```
src/main.rs              runtime bootstrap, CLI args, single-instance, run_native
src/app/                 CorrentApp (eframe::App), actions, shortcuts, toasts
src/engine/              EngineHandle {cmd_tx, snapshot: ArcSwap<Snapshot>}, actor (owns Session + Api, 500ms tick),
                         session options builder, snapshot builder, add flow, re-add helpers, types
src/store.rs             app-side per-hash metadata, atomic JSON (tmp+rename), debounced
src/config.rs            Settings (TOML, serde defaults), paths (portable vs ProjectDirs)
src/features/            queue, seeding, scheduler, watch, run_on_complete, notifications
src/integration/         single_instance (interprocess local socket), os_handler (magnet/.torrent registration), tray
src/ui/theme.rs          palettes, fonts (Inter, JetBrains Mono, phosphor), apply_theme
src/ui/widgets/          progress bar (Mesh gradient), sparkline, badge, chip, piece bar, empty state
src/ui/panels/           toolbar, sidebar, torrent_table, details/{general,trackers,peers,files,pieces,speed}, status_bar
src/ui/dialogs/          add_torrent, magnet, delete_confirm, settings/*, about, log_viewer, limits, move_storage
src/util/                formatting, time, fs
```
Threading: tokio multi-thread runtime built in main (`rt.enter()`), engine actor task; UI sends `EngineCommand` over an
unbounded mpsc and reads `ArcSwap<Snapshot>`; actor publishes every 500 ms + after commands and calls
`ctx.request_repaint()`. Details (peers/files/pieces) fetched only for the selected torrent. Settings that need a restart
recreate the Session (stop → rebuild with same persistence folder, retry ×3, "Reconnecting…" overlay).

## 3. Data model
`TorrentState { Downloading, Seeding, Completed, Paused, Queued, Checking, Stalled, Error, Metadata }`,
`TorrentView { id, hash, name, state, error, size, done, progress, uploaded, downloaded, down_bps, up_bps, eta, ratio,
peers_live, seeds_live, peers_seen, save_path, category, tags, added_at, completed_at, queue_pos, trackers, ... }`,
`FileView`, `PeerView`, `TorrentDetail`, `Snapshot { torrents, detail, session: SessionView, engine_state, history, generation }`,
`EngineCommand { Add, Preview, Pause, Resume, PauseAll, ResumeAll, ForceStart, Delete, SetFiles, SetLimits, SetGlobalLimits,
ToggleAltSpeed, MoveStorage, Recheck, SetTrackers, SetCategory, SetTags, Rename, QueueMove, SetDetailTarget, ApplySettings,
RestartSession, Shutdown }`. State mapping: Initializing→Checking, Error→Error, Paused→Paused/Queued/Completed,
Live+finished→Seeding, Live without metadata→Metadata, Live idle >30s with 0 peers→Stalled, else Downloading.

## 4. Settings ([L] live, [R] engine restart)
- Behavior: confirm exit while active, close/minimize to tray, start minimized, tray icon, notifications on add/complete,
  confirm delete, skip add dialog, run-on-complete command, register as magnet/.torrent handler, check handler on startup [L].
- Downloads: default save path, recent paths, remember last path, start paused, delete .torrent after add [L].
- Connection: listen port, random port, UPnP, listen mode (TCP/uTP/both), IPv4 only, bind device, SOCKS5 proxy, global peer
  limit, blocklist URL, connect timeout [R]; per-torrent peer limit [L, new adds].
- Speed: down/up limit, alt down/up, alt enabled, alt schedule (days + from/to) [L].
- BitTorrent: DHT, LSD, trackers enabled, default trackers, fastresume [R].
- Queue: enabled, max active downloads/uploads/total, slow torrents don't count + threshold [L].
- Seeding: ratio limit, seeding time limit, inactive limit, action (pause/remove/remove+data) [L].
- Watch folders: path, save path, category, start paused, after-add action [L].
- Appearance: theme System/Dark/Light, accent preset/custom, compact rows, zoom, speed units, animations [L].
- Advanced: log level, snapshot rate, concurrent init limit [R], open data folder, reset to defaults.
Settings window edits a draft; Apply computes `requires_restart` and shows a non-blocking "Apply & reconnect" banner.

## 5. UI
Toolbar (add file, add magnet/URL, resume, pause, delete, queue up/down, alt-speed, search, settings); sidebar (status filters
with counts, categories, tags, trackers); TableBuilder torrent list (two-line card rows, sortable/hideable/persisted columns,
multi-select, context menu, keyboard); resizable details panel (General, Trackers, Peers, Files, Pieces, Speed);
status bar (DHT, port/UPnP, free space, speeds + sparklines, alt indicator); Add dialog modal (non-blocking metadata fetch,
file tree, save path + browse + recent, category/tags, start paused, advanced expander); magnet dialog; delete confirm;
settings window (left nav + search + cards); about; log viewer; drag & drop overlay; toasts; empty states.

## 6. Magnet link & file association (HARD REQUIREMENT)
1. CLI: every positional arg that is a `magnet:` URI, http(s) URL to a .torrent, or path to a `.torrent` is an add request.
2. Single instance: `interprocess` local socket (namespaced name `corrent-<user>`). Startup tries to connect; if a primary
   is running, send args (one per line) and exit 0. Primary listens on a thread → channel → UI, `request_repaint`,
   then `ViewportCommand::Visible(true)` + `Focus` and opens the Add dialog (or adds directly if skip-dialog is set).
3. Registration (`integration/os_handler.rs`), from a Settings toggle + first-run prompt, no admin rights:
   - Windows: `HKCU\Software\Classes\magnet` (default "URL:Magnet Link", `URL Protocol` = "",
     `DefaultIcon`, `shell\open\command` = `"<exe>" "%1"`), ProgID `Corrent.Torrent` + `.torrent` default value,
     `HKCU\Software\Clients\...`/`RegisteredApplications` capabilities so it shows in Windows Default Apps, then
     `SHChangeNotify(SHCNE_ASSOCCHANGED)`. (Windows 10/11 may still ask the user to confirm the default for `magnet` in
     Settings → Default apps; we detect and offer a button to open `ms-settings:defaultapps`.)
   - Linux: write `~/.local/share/applications/corrent.desktop` (`Exec=<exe> %U`,
     `MimeType=x-scheme-handler/magnet;application/x-bittorrent;`), then `xdg-mime default corrent.desktop
     x-scheme-handler/magnet` and `application/x-bittorrent`, `update-desktop-database` (best effort).
   - macOS: `CFBundleURLTypes` (magnet) + `CFBundleDocumentTypes` (.torrent) in the bundle's Info.plist
     (`[package.metadata.bundle]` for cargo-bundle). winit delivers URLs via Apple Events which eframe does not expose;
     mitigation: packaged builds launch the binary with the URL as argv through a tiny launcher script/LSEnvironment,
     and single-instance forwarding handles the rest. Documented as a known limitation.
   Implementation notes (verified): `windows-registry 0.6` (`CURRENT_USER.create(path)?.set_string(..)`), also
   `Corrent.Torrent` ProgID + `.torrent\OpenWithProgids`, `Software\RegisteredApplications\Corrent` →
   `Software\Corrent\Capabilities\{URLAssociations\magnet, FileAssociations\.torrent}`; never write hash-protected `UserChoice`.
   `interprocess 2.4` sync local socket on a std thread (`GenericNamespaced` name; `GenericFilePath` in data dir on macOS,
   stale-socket cleanup on ECONNREFUSED). Wire protocol: one line per item, `MAGNET <uri>` / `FILE <path>` / `URL <u>` / `FOCUS`.
   Focus fallback: `RequestUserAttention` when the OS refuses foreground. macOS: `NSAppleEventManager` handler for
   'GURL'/'GURL' and 'aevt'/'odoc' installed before `run_native` (winit does not surface open-URL events).
   CLI flags: `--register`, `--unregister`, `--status`, `--portable`, `--minimized`.
   Launch items are queued in `UiState` and drained in `App::logic()` (runs while hidden in tray).
   First-run banner: "Make Corrent the default for magnet links and .torrent files? [Set as default] [Not now]".
4. Status check in Settings → Behavior showing whether Corrent is the current default handler (Windows: read
   `HKCU\...\Shell\Associations\UrlAssociations\magnet\UserChoice` ProgId; Linux: `xdg-mime query default`).

## 7. App-side features
Queue manager (positions, pause extras as "Queued", force start exempt); seeding enforcer; alt-speed scheduler (local time);
watch folders (`notify` watcher + poll, stable-size check, delete/rename/keep); run-on-complete with %N %F %D %I %Z;
notifications (notify-rust, feature gated); tray (create in AppCreator, menu: show/hide, pause all, resume all, alt speed,
quit; events drained in `logic()`); close-to-tray; portable mode (`portable` marker next to exe or `--portable`).

## 8. Milestones (one commit each, always compiles and runs)
1. Skeleton: Cargo, runtime + engine actor + snapshot, theme + fonts, toolbar add-file, basic table, status bar.
2. Config + store + add flow (preview dialog, magnet/URL dialog, CLI args, drag & drop, clipboard).
3. **Magnet links: single instance forwarding + OS handler registration + first-run prompt.**
4. Actions, selection, context menu, shortcuts, delete modal, toasts, empty states.
5. Sidebar filters, search, sorting, column management, compact mode, persisted UI layout.
6. Details panel (all tabs).
7. Settings window + live/restart apply + engine restart.
8. Queue, seeding limits, per-torrent limits, move storage, recheck, rename, edit trackers.
9. Scheduler, watch folders, run-on-complete, notifications.
10. Tray, close-to-tray, start minimized, confirm exit, graceful shutdown.
11. Visual polish: gradient bars, sparklines, stripes, badges, DnD overlay, accents, About, log viewer.
12. Hardening: perf with 2000 torrents, error paths (port in use → fallback), README + packaging notes.

## 9. Risks
Linux tray deps (feature gate, non-fatal failure); librqbit private types (Api facade, own ETA); engine restart port reuse
(retry with backoff); re-add side effects (key by info hash, busy state, cached torrent bytes); store/session drift
(tolerate unknown hashes, prune); UI stalls (snapshot off-thread, detail only for selection, cached sort index);
macOS Apple-Event URL delivery (documented limitation + argv forwarding).

## 10. Review outcomes (Fable review, applied)
Blockers fixed in the plan:
- **B1 Re-add == full recheck.** librqbit's `delete` removes the `.bitv` fastresume file, so forget + re-add
  (`overwrite: true`, `preferred_id: Some(old_id)`) never re-downloads but always re-hashes. Per-torrent limits,
  tracker edits and move storage are therefore explicit actions that warn "this will re-verify N GB", run as
  background jobs showing Checking progress, and never block the UI.
- **B2 `fastresume = true` by default** (library default is false → full hash on every start). Toggle only in Advanced
  with a warning.
- **B3 Cumulative counters app-side.** Store keeps per-hash `uploaded_total`/`downloaded_total` bases; deltas are
  folded in on each tick and when the ManagedTorrent id/counters reset. Ratio, seeding limits and columns use these.
- **B4 Windows registration:** ProgIDs `Corrent.Magnet` (URL Protocol) and `Corrent.Torrent`, `Software\Corrent\Capabilities`
  + `RegisteredApplications`, `.torrent\OpenWithProgids`, plus `Classes\magnet` fallback. Status = UserChoice ProgId
  (if present) must be ours; otherwise our class must be present. Deep link `ms-settings:defaultapps?registeredAppUser=Corrent`.
  Silent re-registration on exe path drift.
- **B5 Single-instance race:** bind failure → retry connect ×3 with backoff before becoming primary; stale socket cleanup;
  secondary canonicalizes paths; primary validates every line.
- **B6 Graceful shutdown from M1:** `on_exit` → `block_on(timeout(5s, session.stop()))`, store flush, runtime
  `shutdown_timeout`.
Suggestions adopted: listener failure → retry on random port (M1/M7); DHT persistence file and session folder both under
the data dir; store records pause reason (user/queue/seed-limit) so queue-paused torrents restore correctly; "Queued for
check" vs "Checking" state; session statistics; open folder / open file actions; per-category default save path;
font size setting; magnet preview has Cancel and "Add without preview"; clipboard detection is opt-in;
sequential download explicitly unsupported by the engine; Availability column dropped; macOS Apple Event handler
is in scope for the magnet milestone (verified on Windows only in this environment).
