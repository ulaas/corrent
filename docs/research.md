# Corrent — Research Notes

Consolidated from three research passes (feature survey, engine/crates, UI design). Date: 2026-10-08.
Toolchain: rustc 1.98.1. Verified: `librqbit 9.0.1` (features `rust-tls`, no defaults) + `eframe 0.36.2` +
`egui_extras 0.36.2` + `egui_plot 0.37.0` + `egui-phosphor 0.14.0` build together on Windows (cmake+nasm present).

## 1. Feature survey (qBittorrent, Transmission, Deluge, Tixati, BiglyBT, Motrix)

### Must-have (v1)
- Add via .torrent file, magnet, http(s) URL, clipboard auto-detect, drag & drop, paste.
- Add dialog: save path (remembered + recent), category, tags, start paused, sequential, file tree with
  select all/none/invert, sizes, free-space; advanced options behind expander; option to skip dialog.
- File selection editable post-add. Queueing (max active downloads/uploads/total, move up/down/top/bottom,
  force start). Categories (default save path per category) and tags; sidebar filters by status/category/tag/tracker.
- Actions: pause/resume (+all), delete with/without data, force recheck, move storage, rename, copy
  name/hash/magnet, open folder, edit trackers.
- Never lose the session: atomic, frequent persistence; graceful shutdown flushes; never silently redownload.
- Global up/down limits, alternative speed limits + weekly schedule, per-torrent limits, connection limits.
- Seeding limits: ratio, seeding time, inactive time → action (pause/remove/remove+data); per-torrent override.
- Network: listen port, UPnP, IPv6, DHT/PEX/LSD toggles, proxy (SOCKS5), IP blocklist URL, interface binding.
- Watch folders (save path/category per folder, delete/rename .torrent after add). Run external program on complete
  with placeholders (%N name, %F content path, %D save dir, %I hash, %Z size).
- Main list: hideable/persisted columns (#, Name, Size, Progress, Status, Seeds, Peers, Down, Up, ETA, Ratio,
  Category, Tags, Added On, Completed On, Uploaded, Downloaded, Save Path...). Search box, sort any column,
  multi-select, full context menu.
- Detail tabs: General, Trackers, Peers, Files, Speed graph, Pieces map.
- Status bar: DHT nodes, connection status, speeds (+alt toggle), free disk space.
- Dark mode (real theme, follow OS / manual), HiDPI, keyboard shortcuts, log window.
- Single instance forwarding args; magnet handler / .torrent association; tray icon (pause/resume all, quit),
  close/minimize to tray, start minimized; desktop notifications; confirm exit while active; portable mode.
- Settings categories: Behavior, Downloads, Connection, Speed, BitTorrent, Queue, RSS, Appearance, Advanced; search box.

### Should-have (v1.x)
RSS auto-download (feeds, rules: must contain / must not contain / regex / episode filter, preview), Web API,
torrent creator, default tracker list, per-category defaults, shutdown-on-completion, statistics dialog.

### UX lessons
1. Defaults that just work. 2. Progressive disclosure. 3. Real dark mode everywhere. 4. Nothing modal-blocking;
async everything; fast shutdown. 5. Polish: remember last save path, auto-scaling units, Delete key works, column
widths persist. Stability: never lose the session, bounded memory, virtualized list with thousands of torrents,
reliable automation.

## 2. Engine: librqbit 9.0.1 (only maintained pure-Rust engine; libtorrent bindings are painful/dead)

```rust
Session::new_with_opts(default_output_folder: PathBuf, opts: SessionOptions) -> BoxFuture<anyhow::Result<Arc<Session>>>
session.add_torrent(AddTorrent<'_>, Option<AddTorrentOptions>) -> BoxFuture<anyhow::Result<AddTorrentResponse>>
session.get(TorrentIdOrHash) -> Option<Arc<ManagedTorrent>>     // librqbit::api::TorrentIdOrHash::{Id(usize),Hash(Id20)}
session.with_torrents(|it: &mut dyn Iterator<Item=(usize,&Arc<ManagedTorrent>)>| ..)  // Fn; collect inside
session.pause(&Arc<ManagedTorrent>).await; session.unpause(&handle).await   // unpause: self: &Arc<Self>
session.delete(id, delete_files: bool).await
session.update_only_files(&handle, &HashSet<usize>).await
session.stats_snapshot() -> { download_speed, upload_speed, peers, uptime_seconds, .. }
session.get_dht() -> Option<&Dht>; session.listen_addr(); session.announce_port()
session.stop().await
session.ratelimits: Limits  // GLOBAL runtime: set_download_bps(Option<NonZeroU32>), set_upload_bps(..)
```
SessionOptions (Default): `dht: Option<DhtSessionConfig>` (None=off), `persistence: Option<SessionPersistenceConfig::Json{folder}>`,
`fastresume: bool`, `listen: Option<ListenerOptions{mode: ListenerMode::{TcpOnly,UtpOnly,TcpAndUtp}, listen_addr,
enable_upnp_port_forwarding, announce_port, ipv4_only, ..}>`, `connect: Option<ConnectionOptions{proxy_url (socks5),
enable_tcp, peer_opts}>`, `ratelimits: LimitsConfig{upload_bps, download_bps: Option<NonZeroU32>}`, `peer_limit`,
`disable_trackers`, `disable_local_service_discovery`, `ipv4_only`, `trackers: HashSet<Url>`, `blocklist_url`,
`allowlist_url`, `bind_device_name`, `client_name_and_version`, ...

AddTorrent::{from_url (magnet/http), from_bytes, from_local_filename}. AddTorrentOptions{paused, only_files: Option<Vec<usize>>,
overwrite (true needed to resume existing files), list_only (metadata preview), output_folder (excl. with sub_folder),
ratelimits (PER-TORRENT, add-time only), trackers, peer_limit, initial_peers, ..}.
AddTorrentResponse::{Added(id,h), AlreadyManaged(id,h), ListOnly(ListOnlyResponse)}.

ManagedTorrent: id(), name(), info_hash(), output_folder(), only_files(), is_paused(), stats() -> TorrentStats
{state: Initializing|Live|Paused|Error, file_progress: Vec<u64>, error, progress_bytes, uploaded_bytes, total_bytes,
finished, live: Option<LiveStats{download_speed: Speed{mbps}, upload_speed, time_remaining, snapshot{peer_stats{live, seen, connecting, ..}}}>},
live() -> Option<Arc<TorrentStateLive>> (per_peer_stats_snapshot), with_metadata(|m| m.info / file_infos), shared.trackers.
Many live types are not re-exported; `librqbit::Api` facade returns Serialize'able responses (api_torrent_list,
api_torrent_details, api_peer_stats, api_session_stats, api_dht_stats).

Supported: DHT, PEX, LSD, UPnP, uTP (opt-in), SOCKS5, IPv6, JSON persistence + fastresume, global runtime limits,
streaming, file include/exclude at runtime.
MISSING (implement app-side or disable in UI): protocol encryption, BT v2, per-file priority levels (only include/exclude),
per-tracker status, queueing, seeding ratio/time limits, categories/tags, move storage, per-torrent limit changes at
runtime (re-add trick: forget + re-add with new ratelimits & overwrite=true), add peer at runtime.

## 3. GUI stack & API notes (egui 0.36.2, MSRV 1.95)
- `eframe::App` now requires `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)` (old `update` removed).
  `Ui` derefs to `Context`. `Context::style` → `global_style`.
- `SidePanel`/`TopBottomPanel` replaced by `egui::Panel::left/right/top/bottom(id).resizable().default_size().show(ui, ..)`;
  `CentralPanel::default().show(ui, ..)`. (Verify in source.)
- `CornerRadius{nw,ne,sw,se: u8}`, `Margin{..: i8}`, `Shadow{offset:[i8;2], blur:u8, spread:u8, color}`, `Frame::new()`,
  `Painter::rect(rect, cr, fill, stroke, StrokeKind)`, `egui::Modal`, `Popup` API, `ctx.set_theme(ThemePreference)`,
  `ctx.style_mut_of(Theme, ..)`, `animate_bool_with_time`, `animate_value_with_time`.
- egui_extras TableBuilder (virtualized `body.rows`), egui_plot 0.37 `Line::new(name, points)`.
- Crates: rfd 0.17 (file dialogs), tray-icon 0.26 (create in AppCreator; event handlers → channel + request_repaint;
  Linux needs gtk), notify-rust 4.18, directories 6, serde + toml 1.1, tokio 1.53, tracing, interprocess 2.4
  (single instance + arg forwarding via local socket), open 5, arboard (clipboard), egui-notify 0.23 (toasts) optional.
- Runtime: build tokio Runtime in main before run_native, `rt.enter()`, create Session via block_on, give App a Handle;
  never block_on inside ui(); async ops via spawn + mpsc back + request_repaint. Poll stats ~2x/s. On exit `session.stop()`.

## 4. UI design spec (summary)
Layout: toolbar (44px) / left sidebar 220px (status filters with count pills, CATEGORIES, TAGS, TRACKERS) / torrent list
(56px two-line card rows: state icon + name + numeric columns; line 2 = 6px gradient progress bar + meta + chips;
compact mode 32px) / resizable bottom detail panel (General, Trackers, Peers, Files, Pieces, Speed) / status bar 26px
(DHT dot, speeds + sparklines, free space).

Palette (dark / light): bg0 #0F1115/#F3F5F9, bg1 #161920/#FFFFFF, bg2 #1D212B/#F7F8FB, bg3 #262B36/#E9ECF2,
border #2A303C/#DDE1EA, border_hi #3A4150/#C7CDD9, text #E6E8EE/#1A1F2B, text_dim #8B93A5/#5B6476,
text_mute #5C6479/#8C94A6, accent #4F8CFF/#2F6FEB. Accent presets: Blue #4F8CFF, Violet #8B7CFF, Teal #2DD4BF,
Green #3DDC84, Orange #FF8A3D, Pink #FF6B9D.
State colours: Downloading=accent; Seeding #3DDC84; Completed #9AA3B5; Paused #7C8597; Queued #B48CFF;
Checking #F5B942 (animated stripes); Stalled = 55% alpha; Error #FF5C5C.
Radii: buttons 6, rows 8, windows 10, pills 9, progress 3. Spacing 4/8/12/16/24. Type: Body 13, Heading 16, Title 20,
Small 11, Mono 12. Floating thin scrollbars, animation_time 0.12.
Custom painters: gradient progress bar (Mesh), sparklines with area gradient + hover crosshair, piece bar,
state badges (pill with 18% tint). Drag & drop overlay (dashed accent border). Empty states with CTA buttons.
Delete modal (keep files default / also delete files in red). Settings window: left nav, card sections, Apply/OK/Cancel.
Shortcuts: Ctrl+O, Ctrl+Shift+O (magnet), Ctrl+V magnet paste, Space pause/resume, Del, Shift+Del, F2, Ctrl+A, Ctrl+F,
Esc, Ctrl+I toggle details, Ctrl+, settings, Ctrl+Shift+S alt speed.
