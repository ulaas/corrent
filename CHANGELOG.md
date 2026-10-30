# Changelog

All notable changes to Corrent. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/) (pre-1.0: minor versions may break things).

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
