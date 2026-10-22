# Changelog

All notable changes to Corrent. The format follows [Keep a Changelog](https://keepachangelog.com/),
and versions follow [Semantic Versioning](https://semver.org/) (pre-1.0: minor versions may break things).

## [0.1.0] — 2026-10-22

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

[0.1.0]: https://github.com/ulaas/corrent/releases/tag/v0.1.0
