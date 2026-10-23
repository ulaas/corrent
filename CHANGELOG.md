# Changelog

All notable changes to Corrent. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/) (pre-1.0: minor versions may break things).

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

[0.3.2]: https://github.com/ulaas/corrent/releases/tag/v0.3.2
[0.3.1]: https://github.com/ulaas/corrent/releases/tag/v0.3.1
[0.3.0]: https://github.com/ulaas/corrent/releases/tag/v0.3.0
