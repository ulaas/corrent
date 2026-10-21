# Corrent — notes for Claude

Corrent is a cross-platform BitTorrent desktop client: Rust 2024, eframe/egui 0.36.2 UI,
librqbit 9.0.1 engine. Goal: full-featured, beautiful, stable, easy to use, fully configurable.

## Rules and permissions (set by the owner)

- **Git:** local read *and* write commands (add, commit, branch, …) are allowed in this repo
  without asking. **Never** run commands that touch a remote (push, pull, fetch, clone of remotes).
- **Commit every completed step** with a descriptive message. **Do not add Claude as co-author**
  (no `Co-Authored-By` lines in commits or PRs).
- **Plan and review with Fable:** for new milestones, draft the plan with Fable agents
  (`model: "fable"`), have a Fable agent review it, then implement. Use Fable for code reviews of
  larger changes and fix what it finds.
- **Never register Corrent as the system magnet/.torrent handler** on the owner's machine
  (`--register`, "Set as default") without asking — the owner manages that from their own build.
- **Never use real mouse/keyboard automation** on the desktop (other windows overlap the app).
  Drive the UI with the debug-only hooks below and capture with `PrintWindow`.
- **Test only in portable mode** (`--portable`): data goes to `target/debug/corrent-data`, never
  the owner's real profile in `%APPDATA%\Corrent`. Use legal, freely licensed torrents for tests
  (Sintel, Big Buck Bunny, Ubuntu/Debian ISOs) and save them under a scratch folder; delete large
  test downloads afterwards.
- Read-only inspection of the owner's registry/files is fine; writing outside the repo, the
  scratchpad and the portable test profile is not (unit tests may use `HKCU\Software\CorrentTest`
  and must clean it up).

## Commands

```sh
cargo build                 # debug
cargo build --release       # target/release/corrent.exe (owner runs this build)
powershell -File scripts/fetch-libmpv.ps1   # once: libmpv-2.dll into .cache/libmpv; build.rs copies it next to the exe
cargo test --workspace      # unit + loopback engine integration tests (crates/corrent-core/src/engine/e2e_tests.rs)
cargo test -p corrent-core --features testkit --test durability -- --ignored --nocapture   # kill at every write (~2 min)
powershell -File scripts/fault-matrix.ps1   # S8 fault tests (fm_*), timing ones 20x in a row
powershell -File scripts/fuzz-long.ps1 [-Minutes 30] [-Cases 1000000]   # S9: fuzz targets and property tests, long
# fuzz targets: crates/corrent-core/src/fuzz.rs (corpus tests/fuzz-corpus/, cargo-fuzz in fuzz/), property tests: crates/corrent-core/src/prop_tests.rs
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo deny check   # CI gates
./target/debug/corrent.exe --portable [magnet|url|file]
./target/debug/correntd --data-dir target/debug/correntd-data   # headless server (docs/SERVER.md)
corrent --status | --register | --unregister | --write-icons <dir> | --version | --mcp | --help   # --mcp: stdio MCP bridge for AI agents
```

Debug-only UI hooks (compiled out of release): `CORRENT_DEV_SELECT=<row>`,
`CORRENT_DEV_TAB=general|files|peers|trackers|pieces|speed`,
`CORRENT_DEV_OPEN=settings|remote|magnet|about|log|stats|streams|creator|import|feeds|rules|remove|remove-undo`,
`CORRENT_DEV_STREAM_URL=<file>` (writes the first torrent's stream URL prefix; play it with `curl`),
`CORRENT_DEV_PANIC=ui|engine`, `CORRENT_DEV_HANG=ipc` (the primary takes forwarded launches but never confirms them) `|ui` (the UI thread sleeps 15 s) `|engine` (the engine is busy 40 s; both write `logs/hang-*.txt`), `CORRENT_DEV_SEARCH=<query>`, `CORRENT_DEV_LIGHT=1`, `CORRENT_DEV_MICA=1` (Mica spike).
Media/server hooks (run without torrents): `CORRENT_DEV_VIEW=media|libraries`, `CORRENT_DEV_PROFILES=1`, `CORRENT_DEV_SERVER_FS=1` (with a paired server), `CORRENT_DEV_ITEM=<id>`, `CORRENT_DEV_PLAY=<id>`,
`CORRENT_DEV_SHOT=<png>` (+ `CORRENT_DEV_SHOT_DELAY=<s>`, `CORRENT_DEV_EXIT=1`; works on Linux too),
`CORRENT_DEV_LIBRARY=<folder>[;kind[;name]]|…` (adds those libraries to the profile; make test media with
`powershell -File scripts/dev-media.ps1 -Out <scratch>`), `CORRENT_DEV_PAGE=library:<id>|row:<id>|agent`, `CORRENT_DEV_PAUSE=<s>` (pauses the hooked playback so the controls stay up), `CORRENT_DEV_CONNECT=<pairing link>` (fills the Connect dialog), `CORRENT_DEV_FOUND=<name>[;<ip>]|…` (servers announced as if on the network: the notice), `CORRENT_DEV_PAIR=<pairing link>` (pairs and switches —
writes `servers.toml` into the test profile; delete it afterwards). `CORRENT_DEV_OPEN` also takes `connect`. TV mode: `--tv`, and `CORRENT_DEV_KEYS=ArrowDown,ArrowRight,Enter` presses keys
(one per 300 ms from 3 s) like a remote.
To pre-configure the test profile, write `target/debug/corrent-data/settings.toml`
(e.g. `skip_add_dialog = true`, a scratch `save_path`). The hooks run once, on the first frame
that has at least one torrent in the list.

Linux: the whole crate cross-checks and cross-builds from Windows with Zig as C compiler/linker —
`powershell -File scripts/cross/build-linux.ps1 [-CheckOnly | -Clippy] [-Release] [-Python <python with ziglang>]`
(see the script header for the one-time setup). macOS: type-check macOS-only modules by including them
via `#[path]` in a small scratch crate and running `cargo check --target aarch64-apple-darwin`.
Nothing has run on real Linux/macOS yet — `docs/PLATFORM-TESTING.md` is the checklist for that.

## Architecture

```
Cargo workspace: the root package is the `corrent` GUI; library crates live in `crates/`.
src/main.rs                 CLI flags, single-instance negotiation, tokio runtime, run_native
crates/corrent-core/src/    Everything without UI (the GUI and the server both use it):
  engine/actor.rs           Actor task owning the librqbit Session; commands in, ArcSwap<Snapshot> out;
                            `Wake` callback instead of egui (the GUI passes request_repaint)
  engine/{session,types,automation,restore}.rs
                            SessionOptions from Settings; view types; watch folders & run-on-complete;
                            recovery of torrents the engine failed to load at startup
  engine/{creator,stream}.rs Torrent creation; token-protected loopback HTTP streaming
  engine/testkit.rs         Loopback seeder/leecher helpers (feature `testkit` for other crates' tests)
  rss/, search.rs           Feeds + download rules (rss.json); Torznab client (user-configured providers)
  import.rs, updates.rs     Import from other clients; opt-in GitHub release check
  backup.rs, crash.rs, migrate.rs, logbuf.rs
                            Backup/restore, panic hook + crash reports, schema migrations, in-memory log
  store.rs                  Per-torrent data (categories, tags, dates, queue, pause reasons, counters,
                            a copy of each torrent's folder/file selection to rebuild the engine's list)
  config.rs                 Settings (TOML, serde defaults), paths (portable vs OS dirs)
crates/corrent-i18n + assets/i18n/
                            Translations: t!("English text", name = x) / tk! in consts; tr.toml catalog
                            (the source of truth — edit it directly)
crates/corrent-mpv/         Runtime-loaded libmpv (dlopen): probe, encode, GL/SW render; spike examples
crates/corrent-proto/       API v1 wire types (serde): torrents + deltas, events, errors, auth DTOs
crates/corrent-db/          SQLite (bundled): batching writer thread, read pool, migrations (corrent.db)
crates/corrent-server/      Media server: v1/ (API v1 + WebSocket events), identity (users, tokens, pairing,
                            audit), net (LAN guard), tls (own CA), webapi/ (qBittorrent API for Sonarr/Radarr)
crates/correntd/            Headless server binary (docs/SERVER.md)
assets/webui/               Web interface served by corrent-server's webapi at /
src/integration/            single_instance (local socket IPC), os_handler(+_windows) magnet/.torrent
                            registration, macos_events (Apple Events), app_identity, power (when done)
src/app/                    eframe App: titlebar/toolbar, sidebar, table, details, dialogs, settings
                            window, tray, window lifecycle, toasts, extras (about/log/stats)
src/ui/                     theme (palettes, fonts), widgets (progress bar, sparkline, badges…), file tree
vendor/librqbit-dht/        Patched DHT (keeps running on Windows WSAECONNRESET) — see CORRENT-PATCH.md
vendor/librqbit-bencode/    librqbit-bencode 9.0.1 with bounded nesting and checked numbers — see CORRENT-PATCH.md
vendor/librqbit/            librqbit 9.0.1 + runtime limits/group limiter/file order — see CORRENT-PATCH.md;
                            diffs for upstream PRs in docs/upstream/
docs/                       research.md, PLAN.md, ROADMAP.md (with status), STABILITY.md, PACKAGING.md, PERFORMANCE.md,
                            PLATFORM-TESTING.md, DEPENDENCIES.md, upstream/
```

Key invariants:
- Every user-visible string goes through `t!` (gate: `corrent-i18n` test `no_hard_coded_ui_strings`, scans the workspace); new
  strings need a Turkish entry in `assets/i18n/tr.toml` (`turkish_catalog_is_complete`). Use named
  placeholders and separate keys for singular/plural; never compare against translated text.
- The UI thread never blocks on I/O: it sends `EngineCommand`s and reads snapshots.
- Torrents are identified by info hash everywhere (engine ids change on re-add).
- librqbit has no runtime API for limits/trackers/location: those use forget + re-add, which
  re-verifies (never re-downloads); always keep a metainfo backup until the re-add succeeds.
- Fast resume must stay on by default; never rewrite the engine's `session.json` while it runs.
- Fonts in `assets/fonts` are subset without Private-Use-Area glyphs (they shadowed Phosphor icons).

## Media server work and handover (2026-10-19)

The media server (plan `docs/MEDIASERVER.md`, all milestones done) was built on Linux; development
continues on Windows. Start with `docs/HANDOVER.md`: Windows build steps, open items (including the
remuxer review list) and conventions. Gate: `scripts/gate.ps1` (Windows) / `scripts/gate.sh`.

## Status (2026-10-08)

Done and committed: engine actor and snapshots; add flow with metadata preview; magnet/.torrent
OS integration (Windows/Linux/macOS) + single instance; list with sorting, columns, filters
(status/category/tag), search; details tabs; settings window; queue, seeding limits, alt-speed
schedule, watch folders, run-on-complete; tray, notifications; custom title bar; About/Log/Stats;
failed-restore recovery. Roadmap M1–M18 done: async pickers, crash reports, corrupt-file recovery
and backups, logic tests, toasts with actions/undo, CI config, loopback e2e tests, schema
migrations, typed I/O errors, engine restart, batch add, kittest UI tests, typography/motion,
onboarding, torrent creator (M16), streaming to a media player (M17), RSS feeds + rules (M18), Torznab
search (M19), qBittorrent-compatible API (M20), 5k-torrent perf check + Activity sparklines +
drag-to-reorder queue (M21), palettes/high contrast/theme editor/accessibility (M22),
per-torrent speed limits + IP filter (M23), per-file priorities + category limits (M24), import from
qBittorrent/Transmission/Deluge, when-done actions, opt-in update checks (M25), packaging (M26:
MSI/portable zip/winget, AppImage/Flatpak, dmg; release workflow — see docs/PACKAGING.md), Turkish
translation + language setting, web interface, Mica spike (no-go) (M27).
Fable reviews of every milestone range and a final roadmap audit were addressed. Default save folder: `Downloads/Corrent`.

Next steps: the feature roadmap is implemented; hardening follows `docs/STABILITY.md` (S1–S9 done 2026-10-21, S10 next). Also real Linux/macOS runs (docs/PLATFORM-TESTING.md),
upstreaming the librqbit patches (docs/upstream/) and a first tagged release.

Known gaps: engine lacks protocol encryption (librqbit doesn't implement it); macOS/Linux builds are type-checked/cross-built only, never run; CI only runs once pushed.
