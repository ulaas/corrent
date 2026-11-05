# Changelog

All notable changes to Corrent. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/) (pre-1.0: minor versions may break things).

## [Unreleased]

### Added

- **Real-time sound processing in the player** (desktop and web): a 10-band equalizer with its
  curve, Night mode, Clearer dialogue (with a centre-channel lift on 5.1), headphone crossfeed and
  a live loudness leveller, with Flat/Night/Dialogue/Crossfeed presets and your own saved ones. The
  desktop also has output profiles (device, channels, exclusive mode, effects). With everything
  off, no filter runs at all.
- **Meters and stats over the picture**: `Shift+I` shows LUFS M/S/I, LRA, true peak per channel,
  RMS, stereo correlation and compressor gain reduction, plus the live chain and the audio format
  in and out. `I` shows a fuller A/V page. The web adds a spectrum.
- Desktop: command palette (`Ctrl+K`), Trackers in the sidebar, saved views, an Error column, Fit
  columns, a Keyboard shortcuts window, Home/End/PageUp/PageDown and `/` for search, bulk Add
  trackers, Invert selection, Select same category/tag/tracker, a double-click setting, and settings
  search that highlights the matching rows.
- Web: Back keeps your scroll position without reloading the grid; phone gestures (a tap shows the
  controls, double-tap skips ±10 s); TV remote and media keys in the player; "See all" lists the
  whole of Continue watching and Next up; a `?` shortcuts overlay; images sized to the screen.
- Watch folders also pick up `.magnet` files holding one magnet link.
- API: `GET /items` takes `in_progress=true` and `next_up=true`, and items carry `missing`.
- Server: gzip and Brotli on text responses.

### Changed

- "Even out loudness" uses a live leveller instead of `loudnorm`, so it sounds different, delays
  sound less and no longer changes the output's sample rate. Your setting carries over.
- Equalizer presets keep headroom, so a boost no longer clips on loud masters.
- The player remembers its volume and mute.
- Library scans run several probes at once and write in batches, which is much faster on first
  scan.
- Far fewer idle repaints in the desktop app, and much less memory and work per tick with thousands
  of torrents. Pause all writes the torrent list once rather than once per torrent; a crash can now
  lose the last half second of pauses, resumes and file selections (adds and deletes are saved at
  once as before).

### Fixed

- A Resume made while a torrent was still being checked was lost; with many torrents, Resume all
  left a lot of them paused with no error.
- Music and video queues now advance on their own and mark the last item played; gapless albums
  really are gapless (music only).
- Settings changes no longer drop the sound while something plays, unless the output itself changed.
- Player shortcuts no longer fire while typing in the player's own text box. Closing the player
  from picture-in-picture restores the window.
- A magnet add waiting for metadata can no longer overwrite the torrent list after a settings
  restart, and re-adding a torrent keeps its category, tags and date.
- A mixed-case 32-character clipboard text (a password or token) is no longer taken for an info
  hash, and the magnet dialog no longer touches the disk on every frame.
- The delete dialog no longer confirms "also delete files" on a bare Enter. Esc keeps the selection
  while closing menus. Bulk adds show one toast. Fixed-width dialogs fit small windows at high zoom.
- Web: signing in as another user no longer shows the previous user's libraries; a dead connection
  is noticed and every page recovers after sleep; rapid track changes no longer leak a player; a
  tap on a phone no longer pauses the video.
- Server: an HLS encoder that ended early was restarted up to 600 times per request; a failed
  trickplay was retried with a full decode on every play; files added during a scan were missed;
  items whose file is gone are now marked.

### Security

- `.torrent` URLs and artwork are fetched pinned to the checked address, with every redirect checked
  (they could reach the server's own network).
- RSS feed links, default trackers, proxy details and list URLs (private-tracker passkeys) are
  masked for accounts without `settings.write`.
- qBittorrent API sessions end when the account is disabled, deleted, or loses the permission.
- The DLNA listener checks `Host` (DNS rebinding). Jellyfin's `/users/public` answers on the LAN only.
- Wanted, monitored and scan status respect library visibility. Restores cap entry sizes and keep
  private keys owner-only. Response bodies are capped while reading rather than after.

## [0.3.8] — 2026-10-31

The 0.3.6 and 0.3.7 tags never got release builds (CI failed); this release carries their
changes, listed below.

### Security

- Plugins: wasmtime 49.0.5, for RUSTSEC-2026-0420, -0422, -0423 and -0424 (type-check and
  resource-handling flaws in the WASM runtime that runs metadata providers).

### Fixed

- Two tests that failed on slow or fast CI runners (HLS pacing, the qBittorrent API add).

## [0.3.7] — 2026-10-31

### Added

- Web interface: a **Metadata** section in Settings, as in the app — the metadata and
  subtitle providers with their status, API keys, a Test button and where to get a key. The
  "no metadata provider" warning under a library links there.

### Fixed

- Media server: transcoding no longer keeps the CPU busy for a minute and a half after you
  stop watching. The encoder now stays about three minutes ahead of the player and pauses
  there (also while playback is paused), and the web player ends its transcode as soon as the
  tab is closed or left.

## [0.3.6] — 2026-10-31

### Added

- Metadata: **Refresh metadata** for an item (its page, its right-click menu) or a whole library
  (Settings → Libraries), either filling in what is missing — including titles no provider
  matched before — or replacing everything the providers gave (fields you locked and matches
  you chose with Identify… stay). A library shows how its refresh goes: how many titles were
  identified, how many had no clear match, and each provider's own error ("TMDB refused the API
  key").
- Metadata: a **Test** button for each provider looks up a well-known title with the saved key
  and shows what came back or why it failed. Settings → Metadata says which providers can
  identify titles, and that OMDb and fanart.tv only add ratings and artwork to what TMDB or
  TVDB found.
- Right-click menus on media cards, episodes, album tracks and playlist rows (app and web):
  play, add to a playlist, played and favorite, find subtitles, Identify…, refresh metadata,
  IMDb/TMDB pages, copy link; playlist rows can be moved and removed.
- Web interface, to work like the app:
  - right-click (or long-press, or Shift+F10) menus for torrents — the app's full menu with
    Play ▸, Category ▸, Tags ▸, Queue ▸, Copy ▸, Choose another folder… and more — and for
    categories, tags, feeds, articles, search results and files (priority);
  - the app's keyboard shortcuts on the torrent list (Space, F2, Shift+Del, Esc, Ctrl+I,
    Ctrl+F, Ctrl+Shift+S);
  - all of the app's torrent columns, chosen and ordered by right-clicking the header;
  - Add to library… for a torrent; dragging torrents to a new queue place; a Speed tab in the
    details;
  - Identify…, IMDb/TMDB links and IMDb, Rotten Tomatoes and Metacritic scores on item pages;
    person pages; "See all" for every home row;
  - Switch profile ("Who's watching?") with a PIN pad;
  - player: subtitle delay and size, picture shape, stats, and "Use this frame as the
    thumbnail" (T);
  - the AI agent page: approve or reject what agents propose, and connect one to the server.
- Torrents that are still being added (waiting for metadata) can be cancelled from the web
  interface and the API.

### Changed

- Failed metadata lookups are logged as warnings (once per provider and run) instead of being
  hidden.

### Security

- Errors from plugins' web requests no longer include the request address, which can carry an
  API key (OMDb puts it there), in logs and in what clients see.

## [0.3.5] — 2026-10-28

### Fixed

- Media server: when several players started the same video at once, a little bookkeeping for
  that file could stay in memory for as long as the server ran.

## [0.3.4] — 2026-10-28

### Fixed

- Once the torrent engine has started, the start screen no longer keeps showing its last step
  ("loading torrents") until the next refresh.

## [0.3.3] — 2026-10-28

### Added

- Media player (app and web): the audio track and subtitles chosen for a video are remembered
  and chosen again the next time it plays, on every device of the same user.

## [0.3.2] — 2026-10-23

### Security

- tokio 1.53.3 (RUSTSEC-2026-0386: `AsyncFd` constructors violated I/O safety).

## [0.3.1] — 2026-10-23

### Fixed

- Media library: details and artwork of one movie or show no longer end up on another. The
  scanner used to put several titles into one item: movies in a folder that groups them
  (`Marvel/Thor.mkv`), two shows under one folder (`Anime/Show A`, `Anime/Show B`), every movie of
  a folder sharing its `movie.nfo` and `poster.jpg`, trailers and samples made into movies of
  their own, and remakes without a year merged with each other.
- Existing libraries are repaired once, on their next scan: files move to the right movie or
  show, items that held several titles are identified again, and watch progress, playlists and
  followed shows go along. Matches you picked by hand are kept.
- Show folders: `Season 2024` (daily shows), year folders, season folders in other languages and
  release folders inside a show's folder are no longer taken for the show; multi-part and
  multi-version movies keep their folder's title and year; DVD and Blu-ray folder structures take
  the movie folder's name.
- Automatic identification only applies a match that is clearly the item (same title and year),
  and checks the item is still the same one before applying it.
- Undoing a removal right after removing a torrent no longer gets lost (the server's undo).

### Added

- Windows: when playback needs mpv, Corrent offers to download it from the mpv-winbuild-cmake
  project (about 30 MB, checked against a known checksum) into its own folder; also in
  Settings › Playback, which now shows the mpv in use.

### Security

- ctutils 0.4.5 (RUSTSEC-2026-0385: secret-dependent branches, under password hashing and
  signing); clipboard-win 5.4.3 (5.4.1 was yanked).

## [0.3.0] — 2026-10-22

The first public release.

### Torrents

- Add `.torrent` files, magnet links, info hashes and links; Corrent can open them from your
  browser and file manager, and a second launch hands them to the running window.
- Add dialog with a metadata preview, file selection, save folder, category and tags; batch add,
  drag and drop, watch folders, and import from qBittorrent, Transmission and Deluge.
- A fast list for thousands of torrents with sorting, columns, filters by status, category, tag
  and tracker, search, and drag-to-reorder queueing.
- Details: general stats, files with priorities, live peers, trackers, a piece map and speed graphs.
- Queue limits, seeding goals, global, alternative (scheduled), per-category and per-torrent
  speed limits, IP filter, SOCKS5 proxy, interface binding, UPnP, DHT, PEX and local discovery.
- Create torrents; play files while they download; actions when downloads finish.
- RSS feeds with download rules, and search through your Torznab providers.

### Media server

- Libraries of movies, shows, music and photos with a built-in player (subtitles from the
  library, the web or a file you choose; audio tracks; picture in picture), TV mode, watching
  together and casting.
- A headless server (`correntd`) with a web client and a native client that finds it on the
  network and pairs with a short code.
- A qBittorrent-compatible API for Sonarr, Radarr and similar tools, and an MCP bridge for AI agents.

### Look and feel

- Custom title bar, dark and light themes, five palettes including high contrast, accent colors,
  a theme editor, interface scaling, tray icon and notifications.
- Screen-reader support, keyboard navigation, English and Turkish.

### Stability

- The interface never waits on disk or network; fast resume; atomic writes with backups and
  recovery; an engine restart that replays queued actions.
- Crash reports that resolve to source lines, hang reports, health checks and a redacted
  diagnostics bundle.
- Fuzz and property tests for everything that parses outside data, and a test that kills the app
  at every write.

### Known limitations

- Linux and macOS builds are produced by CI but have not been tested on real machines yet.
- The engine (librqbit) has no protocol encryption and no BitTorrent v2.

[0.3.5]: https://github.com/ulaas/corrent/releases/tag/v0.3.5
[0.3.4]: https://github.com/ulaas/corrent/releases/tag/v0.3.4
[0.3.3]: https://github.com/ulaas/corrent/releases/tag/v0.3.3
[0.3.2]: https://github.com/ulaas/corrent/releases/tag/v0.3.2
[0.3.1]: https://github.com/ulaas/corrent/releases/tag/v0.3.1
[0.3.0]: https://github.com/ulaas/corrent/releases/tag/v0.3.0
