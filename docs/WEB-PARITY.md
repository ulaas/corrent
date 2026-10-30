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
the desktop item page's buttons (a superset: the web has no other place for them on grids).

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
- [ ] **S9 Player gaps**: T / "Use this frame as the thumbnail", subtitle delay and size, picture
  aspect, stats (hls.js), reset. Not possible in browsers: audio delay, screenshot.
- [ ] **S10 AI agent page**: proposals with Approve/Reject; the server's MCP endpoint instead of
  the desktop's stdio command.
- [ ] **S11 Torrent polish**: drag to reorder the queue (`POST /torrents/queue`,
  TORRENTS_MANAGE_ALL), speed sparkline in details (collected from deltas), initials placeholder
  on cards. (Undo after remove exists already.)

Known gaps kept: the torrent creator (Ctrl+N) has no web version; `Torrent` has no owner, so
users who may manage only their own torrents see entries the server then refuses.

## Verification

`npm run check` (svelte-check, i18n, vitest); `node web/scripts/screenshots.mjs` against
`correntd --data-dir target/debug/correntd-data` with media from `scripts/dev-media.ps1` (phone
and desktop, dark and light), next to desktop shots from the `CORRENT_DEV_*` hooks
(`CORRENT_DEV_DEMO=1` for torrents). No real mouse/keyboard automation.

## Intentional differences

No file-system actions (Open, Open folder), no offline downloads, no TV
mode, no mpv-only player features; long-press sheets on phones; Play ▸ for torrents streams from
the server rather than an external player.
