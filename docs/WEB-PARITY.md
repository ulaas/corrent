# Web client parity with the desktop app

The owner wants the web client (`web/`, Svelte 5, served by correntd) to look and work like the
egui client, right-click menus included. Decision (2026-10-30): keep both clients and bring the
web up to the desktop's level (no wasm or Tauri rewrite).

## Where the menus are

The desktop's **media** pages have no right-click menus: item actions are a button row on the item
page (`src/app/media/mod.rs`, item page), popups only open from buttons (download, add to playlist,
player tracks/settings/cast/room/lyrics). The right-click menus are in the **torrent** UI:

- Torrent row (`src/app/actions.rs`, from `table.rs`): Resume/Retry, Pause, Force start, [Choose
  another folder… on I/O errors], Open folder, Open, Play ▸ (media files, Copy stream link),
  Move to…, Add to library…, Rename… (F2), Category ▸, Tags ▸, Queue ▸, Speed limits…,
  Seeding limits…, Force recheck, Add trackers…, Copy ▸ (name, info hash, magnet, save path),
  Remove…, Remove with files…. Pending rows: Cancel.
- Column header → column chooser (`table.rs`).
- Sidebar category (Edit…, Add subcategory…, Remove category) and tag (Remove tag) (`sidebar.rs`).
- Feed (Refresh, Copy address, Pause/Resume updates, Remove feed) and article (Open web page)
  (`feeds.rs`); Torznab result (Open details page) (`search_view.rs`).
- File tree: Priority ▸ (`src/ui/file_tree.rs`).

The web gets faithful ports of these, plus a context menu on media cards and rows that mirrors
the desktop item page's buttons; the desktop got the same card menu afterwards
(`src/app/media/item_menu.rs`), so both work alike.

## Steps (each a commit after `cd web && npm run check`; `npm run build` and commit `web/dist`
## once per group of steps — it is 1.3 MB of hashed files, replaced whole every build)

Strings: reuse the desktop's English text exactly where it exists (not keys with key hints
such as `"Rename…  (F2)"`) (the web catalog falls back to
`assets/i18n/tr.toml`); new ones go to `assets/i18n/web/tr.toml` (`web/scripts/check-i18n.mjs`).

- [x] **Metadata refresh** (done 2026-10-30): Refresh metadata ▸ on item pages and in
  Settings → Libraries with run status, provider Test button, supplement note.
- [x] **S1 ContextMenu component** (done 2026-10-30). A store `web/src/lib/contextmenu.svelte.ts`
  (`openMenu(items, at, {returnFocus, key})`, `closeMenu()`, items: item / submenu (lazy
  `() => Promise<MenuItem[]>` for Play ▸ files and IMDb/TMDB on cards) / sep / header) and one
  host `ContextMenu.svelte` mounted in `App.svelte` like `ConfirmHost.svelte`: fixed position,
  z-index above `Modal` (100) and below toasts (200). `Menu.svelte` stays as it is (trigger
  menus); the host reuses its look (`.mi/.sep/.mh`). A `use:contextMenu={{ items, before }}`
  action: right-click, long-press (≈450 ms; cancelled by movement and `pointercancel`; Android
  fires `contextmenu` too — handle once; iOS fires none; swallow the click that follows a
  long-press on `<a>` cards; `-webkit-touch-callout: none`), Shift+F10 / ContextMenu key.
  `items` returning null lets the native menu through (links, inputs, selected text); `before`
  selects an unselected row. Closes on outside pointerdown, Escape, scroll, resize, blur, route
  change, or when its `key` (a torrent) leaves the list; gives focus back explicitly. Keyboard:
  ↑↓ Home End, → opens a submenu, ← closes it, Enter/Space, first-letter jump; ARIA menu roles.
  Submenus flip near edges; long menus scroll (every menu must); phones get a bottom sheet
  (Modal's sheet styles). Tests (vitest runs in node, `web/tests/`): placement/flip math and the
  menu builders; interaction checked with `screenshots.mjs`.
- [x] **S2 Torrent row menu and shortcuts** (done 2026-10-30; left for later: Copy stream link,
  speed/seeding limits for several torrents at once — the Edit dialog takes one; Add to library…
  comes with S4) (`Torrents.svelte`): right-click selects an
  unselected row, then the `actions.rs` menu, including "Choose another folder…" (Move to…
  highlighted when the error is disk full / permission / not found: port `classify_io`).
  Play ▸ lists media files (port `engine::stream::is_media`; files from
  `GET /torrents/{hash}/detail`) and plays them in the browser — the stream route
  (`v1/wanted.rs`) takes the session cookie, so "Copy stream link" needs a token-carrying URL from
  the server first (add one, or leave the entry out). Copy ▸ builds the magnet client-side.
  Category ▸ / Tags ▸: one `PATCH /torrents/{hash}` per torrent, tags sent as each torrent's full
  list; "New category…" needs TORRENTS_MANAGE_ALL. Pending rows: Cancel needs a server
  `Action::Cancel` (the desktop sends `EngineCommand::CancelAdd`) — add it. Speed/Seeding limits
  open the Edit dialog at that section. Keys as on the desktop (`actions.rs` key handling):
  Space pauses/resumes (Ctrl+Space toggles selection instead), Del, Shift+Del, F2, Esc,
  Ctrl+A, Ctrl+I details, Ctrl+F / "/", Ctrl+Shift+S alternative speeds, pasting a magnet or
  link adds it. Browser-owned shortcuts (Ctrl+N, Ctrl+O) stay the browser's. No Open/Open
  folder. A pure `menuFor(selection, perms)` builder with vitest coverage per state.
- [x] **S3 Other menus** (done 2026-10-30; articles and results also offer Download / Add): sidebar category/tag (PUT `/torrents/categories|tags`,
  TORRENTS_MANAGE_ALL), RSS feed (refresh one feed: `POST /rss/refresh {url}`; pause/remove go
  through `PUT /rss/config`, SETTINGS_WRITE) and article, search result, file priority
  (`Details.svelte`, inside a modal).
- [x] **S3b Columns** (done 2026-10-30; columns that don't fit are left out from the end
  instead of the desktop's horizontal scrolling): the desktop's extra columns (queue, category, tags, save path, tracker,
  added, completed), a dynamic grid template instead of the fixed nine columns, and the chooser
  (header right-click: show/hide, reorder, reset; saved in prefs; not on phones).
- [x] **S4 Add to library…** (done 2026-10-30) dialog: dry-run `POST /torrents/{hash}/import`, show the plan,
  confirm.
- [x] **S5 Media card/row menu** (done 2026-10-30; no Cast entry — casting starts from the player): Play, Play from start, Add to playlist ▸, Mark played/unplayed,
  Favorite, Find subtitles… (movie/episode/video), Identify… (movie/show/video), Refresh metadata ▸
  (movie/show/video/album/artist), Open IMDb/TMDB (cards lack provider ids: a lazy submenu that
  fetches the item), Copy link, Cast; playlist rows add Move up/down, Remove. LIBRARY_MANAGE for
  identify/refresh/subtitles.
- [x] **S6 Identify… dialog** (done 2026-10-30) (`identify.rs`): title/year search, candidates, apply; IMDb/TMDB
  links and the ratings line on the item page.
- [x] **S7 Person page and "See all" rows** (done 2026-10-30) (`/items?person_id=`, home row pages).
- [x] **S8 Profiles** (done 2026-10-30; from the account menu, as on the desktop): "Who's watching?" after sign-in when there are several, PIN, Switch
  profile.
- [x] **S9 Player gaps** (done 2026-10-30): T / "Use this frame as the thumbnail", subtitle delay and size, picture
  aspect, stats (hls.js), reset. Not possible in browsers: audio delay, screenshot.
- [x] **S10 AI agent page** (done 2026-10-30; it creates a token for the agent): proposals with Approve/Reject; the server's MCP endpoint instead of
  the desktop's stdio command.
- [x] **S11 Torrent polish** (done 2026-10-30; the speed history is collected while the torrent
  list is open): drag to reorder the queue (`POST /torrents/queue`,
  TORRENTS_MANAGE_ALL), speed sparkline in details (collected from deltas), initials placeholder
  on cards. (Undo after remove exists already.)

Known gaps kept: the torrent creator (Ctrl+N) has no web version; `Torrent` has no owner, so
users who may manage only their own torrents see entries the server then refuses.

## Quality round 2026-11: sound processing and input

The web's sound processing mirrors `playback.fx` in `prefs.audioFx` (`lib/audiofxmodel.ts`, same field
names, per user through `/me/prefs`; volume stays per device). With it off there is no `AudioContext`
and the graph isn't even loaded. Once it has run, an element stays routed: off means source → volume.

- **Web only:** the spectrum (the browser's `AnalyserNode`).
- **Desktop only:** output device, exclusive mode, passthrough, profiles; centre boost on 5.1.
- **Implemented differently:** Live loudness is an AudioWorklet leveller toward -18 LUFS instead of
  `dynaudnorm`; the limiter is a `DynamicsCompressorNode` at -1 dB, not a true-peak limiter; Night's
  level is within about 1 dB of `acompressor` (the node's built-in makeup is compensated); the
  crossfeed shelf has Web Audio's fixed slope (S = 1, FFmpeg's default is 0.5). Meters follow
  BS.1770-4 and EBU Tech 3341/3342 at the context's real sample rate, tested in `web/tests/audiodsp.test.ts`.
- **No processing at all, the source plays as it is:** cross-origin radio streams (Web Audio would
  output silence; the music player swaps to a fresh element) and Safari (native HLS is unreliable
  through Web Audio). The overlay says so. The graph adds output latency the browser doesn't
  compensate; the overlay shows it.
- **Keys:** `i` A/V stats and `Shift+I` audio meters as on the desktop. Left over from the review:
  web arrows seek ±5 s and `j`/`l` ±10 s, the desktop's arrows seek ±10 s and it has no `j`/`k`/`l`;
  `?` lists them. Touch: first tap shows the controls, double tap on a half skips 10 s. A D-pad walks the
  controls; a remote with no pointer at all (`hover: none, pointer: none`) enters them with up or down,
  so its volume keys stay the TV's.
- **Rows:** "See all" for Continue watching and Next up needs the server's `in_progress` / `next_up`
  filters and falls back to the home row without them. Items with `missing` show a badge and can't play.

## Verification

`npm run check` (svelte-check, i18n, vitest); `node web/scripts/screenshots.mjs` against
`correntd --data-dir target/debug/correntd-data` with media from `scripts/dev-media.ps1` (phone
and desktop, dark and light), next to desktop shots from the `CORRENT_DEV_*` hooks
(`CORRENT_DEV_DEMO=1` for torrents). No real mouse/keyboard automation.

## Stream D (quality round, 2026-11): desktop features to port

Desktop only so far, to port or to decide against in a later web round: the command palette
(Ctrl+K), saved views in the sidebar (`UiPrefs.views`; `/me/prefs` would hold them on the web), the
Error column and Fit columns, list keys (Home/End, PgUp/PgDn, Ctrl+arrows focus only, Ctrl+Space,
`/`), Select → Invert/Same category/tag/tracker, Copy → Tracker hosts, the Trackers sidebar section
with its context menu, and the F1 Keyboard shortcuts window. The tracker filter needs each torrent's
trackers in the list response. The double-click setting has no meaning on the web (touch, no local
files).

## Intentional differences

No file-system actions (Open, Open folder), no offline downloads, no TV
mode, no mpv-only player features; long-press sheets on phones; Play ▸ for torrents streams from
the server rather than an external player.

No update check and no "Update and restart" (desktop: Settings › Behavior › Updates,
`src/app/update_ui.rs`). The web client is served by the server it talks to, so it is always that
server's version. `correntd` itself neither checks for releases nor updates itself: whatever
installed it does (the archive, the Docker image, a service package). A "newer version" notice for
administrators in the web client is open; replacing a running service is out of scope there.

## Sound processing, desktop side (quality round, stream C)

The sound sheet (Player → Playback settings → Sound; the mini player's sliders button; Settings →
Playback) and the Audio page (`Shift+I`) have a web counterpart with the same `AudioFx` shape and
meaning (`crates/corrent-core/src/audio_fx.rs`). What only the desktop has, and why:

- Output device, speakers, exclusive mode and passthrough, and with them output profiles (saved
  sounds exist on both): a browser chooses its own output and can't pass compressed audio through.
- Dialogue's centre channel lift on 5.1: the web graph plays what the browser hands it.
- Loudness Live is FFmpeg's `dynaudnorm` here, a worklet AGC on the web; they aim at the same thing
  but don't sound the same.
- Metering runs FFmpeg's `ebur128`/`astats`/`aphasemeter` here. There is no spectrum on the desktop,
  because no filter reports its bands as numbers.
