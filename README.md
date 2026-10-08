# Corrent

A beautiful, stable, cross-platform BitTorrent client written in Rust with an
[egui](https://github.com/emilk/egui) interface and the [librqbit](https://github.com/ikatson/rqbit) engine.

## Features

**Adding torrents**
- `.torrent` files, magnet links, info hashes and `http(s)` links to `.torrent` files
- Opens magnet links from your browser and `.torrent` files from your file manager
  (one click in Settings → General → *Open with Corrent*, or `corrent --register`)
- Single instance: launching Corrent again hands the link to the running window
- Add dialog with metadata preview (magnets are resolved first), file tree selection,
  save folder (default: a `Corrent` folder in Downloads) with recent folders, categories, tags and free-space check
- Drag & drop `.torrent` files or text files with magnet links; paste a magnet anywhere (Ctrl+V)
- Watch folders with per-folder category, save path and start-paused options
- Batch add dialog for many torrents at once; import from qBittorrent, Transmission and Deluge
  (data is verified in place, never downloaded again)
- Create torrents (private flag, comment, piece size) and start seeding them right away

**Discover**
- RSS/Atom feeds with download rules: words or regex, exclusions, episode filters
  (`1x1-5;8`), one release per episode, per-rule category and folder — each item is added once,
  also across restarts
- Search through Torznab providers you configure (Jackett, Prowlarr, …)

**Managing**
- Virtualized list that stays fast with thousands of torrents; two-line rows or compact mode
- Sort by any column, show/hide columns (right-click the header), multi-select, full context menu
- Sidebar filters by status, category (nested with `/`), tag and tracker, with live counts
- Queue with active download/upload/total limits; slow torrents can be excluded
- Seeding limits by ratio, seeding time or inactivity (pause or remove), with per-torrent overrides
- Move data, force recheck, add trackers, rename, copy magnet/hash, open folder
- Per-torrent and per-category speed limits; per-file priorities (high/normal/low)
- Drag rows to reorder the queue; optional per-row activity graphs
- Play media files in your player while they download (local streaming with seeking)
- Undo for removed torrents; "when downloads finish": quit, sleep or shut down after a countdown
- Details panel: general stats, file tree with per-file progress and selection, peers with live
  speeds, trackers, a piece map and speed graphs

**Network**
- DHT, PEX, local peer discovery, UPnP port forwarding, IPv6, uTP (experimental)
- Global and alternative speed limits with a weekly schedule
- SOCKS5 proxy, interface binding (e.g. a VPN adapter), connection limits
- IP filter: blocklist and allowlist (URL or file, plain or gzip) plus your own addresses
- Falls back to a random port if the configured one is unavailable

**Desktop**
- Custom title bar in the app's own style (native window buttons on macOS)
- Dark and light themes (or follow the system), six accent colors plus a custom color,
  interface scaling, speed units, animations
- System tray with quick actions, close/minimize to tray, start minimized
- Desktop notifications, run a program on completion (`%N %F %D %C %T %Z %I` placeholders)
- Statistics, in-app log viewer, keyboard shortcuts throughout
- Palettes (Corrent, Nord, Solarized, High contrast, your own via a theme editor with
  `theme.toml` import/export); follows Windows contrast themes
- Screen-reader support (AccessKit), keyboard focus rings, labelled icon buttons
- English and Turkish (follows the system language or pick one in Settings → Appearance)

**Remote control** (off by default)
- qBittorrent-compatible Web API, so Sonarr, Radarr and similar tools can use Corrent as their
  download client; a small web interface at the same address
- Local-only unless you allow other devices; password required (stored as a PBKDF2 hash)

**Stability**
- The engine runs on its own async runtime; the UI only reads immutable snapshots, so it never
  blocks on disk or network I/O
- Fast resume is on by default: restarting never re-downloads and normally skips re-verification
- Settings and app data are written atomically with a backup copy; transfer totals survive restarts
- Graceful shutdown flushes everything before exit
- Crash reports and an "engine stopped" banner with a restart that replays queued actions
- Damaged settings/data files are recovered from backups; backup and restore of all app data
- Opt-in update check (GitHub releases; nothing is installed automatically)

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+O | Add torrent file |
| Ctrl+Shift+O | Add magnet link / URL |
| Ctrl+V | Add magnet links from the clipboard |
| Space | Pause / resume selection |
| Delete / Shift+Delete | Remove / remove with files |
| F2 | Rename |
| Enter / double-click | Open the downloaded file or folder |
| Ctrl+A, ↑/↓, Shift+↑/↓ | Select |
| Ctrl+F | Search |
| Ctrl+I / Ctrl+B | Toggle details panel / sidebar |
| Ctrl+Shift+S | Toggle alternative speed limits |
| Ctrl+, | Settings |

## Building

Requires Rust 1.95 or newer.

```sh
cargo build --release
```

The binary is `target/release/corrent` (`corrent.exe` on Windows).

- **Windows:** MSVC toolchain. NASM is used if available; otherwise prebuilt objects are used.
- **Linux:** a C compiler plus the usual windowing libraries (`libxkbcommon`, Wayland/X11).
  The tray icon uses the D-Bus StatusNotifierItem protocol and needs no GTK.
- **macOS:** build an app bundle with [`cargo-bundle`](https://github.com/burtonageo/cargo-bundle):
  `cargo bundle --release`. The bundle declares the `magnet` URL scheme and `.torrent` documents,
  and Corrent receives them via Apple Events.

## Command line

```
corrent [OPTIONS] [MAGNET | URL | FILE.torrent]...

  --portable     keep all data next to the executable (or create a file named `portable` there)
  --minimized    start hidden in the tray
  --register     register as handler for magnet links and .torrent files
  --unregister   remove the registration (restores the previous handler on Windows)
  --status       print the registration status
  --write-icons  write the app icon set to a folder
  --help         show this help
```

Installers: see `docs/PACKAGING.md` (MSI, portable zip, winget, AppImage, Flatpak, dmg).

## Where data lives

| Platform | Location |
|---|---|
| Windows | `%APPDATA%\Corrent\data` |
| Linux | `~/.local/share/corrent` |
| macOS | `~/Library/Application Support/Corrent` |

`settings.toml` holds all settings, `store.json` the app's per-torrent data (categories, tags,
dates, totals, limits, priorities), `rss.json` the feed history, `session/` the engine's resume
data and `logs/` the log files and crash reports. On Windows, the first notification registers
Corrent's notification identity under `HKCU\Software\Classes\AppUserModelId\Corrent.Corrent`
(not in portable mode).

## Engine limitations

Corrent uses librqbit, the only actively maintained pure-Rust BitTorrent engine. Protocol
encryption and BitTorrent v2 are not available in it. librqbit downloads each file's pieces in
order, so there is no separate "sequential" switch. Moving data, rechecking and adding trackers
briefly re-add the torrent, which re-verifies its data (it is never re-downloaded).

Two librqbit crates are vendored with small patches: `vendor/librqbit-dht` keeps the DHT running
on Windows, and `vendor/librqbit` adds runtime speed limits, a shared per-category limiter and
per-file download order (see each `CORRENT-PATCH.md`; diffs for upstream in `docs/upstream/`).

## License

MIT or Apache-2.0. Bundled fonts: Inter and JetBrains Mono (SIL Open Font License, see
`assets/fonts`). Icons: Phosphor (MIT).
