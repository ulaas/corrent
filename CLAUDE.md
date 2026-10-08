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
cargo test                  # unit tests (19 at last count; one touches HKCU\Software\CorrentTest)
./target/debug/corrent.exe --portable [magnet|url|file]
corrent --status | --register | --unregister | --write-icons <dir> | --help
```

Debug-only UI hooks (compiled out of release): `CORRENT_DEV_SELECT=<row>`,
`CORRENT_DEV_TAB=general|files|peers|trackers|pieces|speed`,
`CORRENT_DEV_OPEN=settings|magnet|about|log|stats`, `CORRENT_DEV_LIGHT=1`.
To pre-configure the test profile, write `target/debug/corrent-data/settings.toml`
(e.g. `skip_add_dialog = true`, a scratch `save_path`).

Linux: the whole crate cross-checks and cross-builds from Windows with Zig as C compiler/linker —
`powershell -File scripts/cross/build-linux.ps1 [-CheckOnly] [-Release] [-Python <python with ziglang>]`
(see the script header for the one-time setup). macOS: type-check macOS-only modules by including them
via `#[path]` in a small scratch crate and running `cargo check --target aarch64-apple-darwin`.
Nothing has run on real Linux/macOS yet — `docs/PLATFORM-TESTING.md` is the checklist for that.

## Architecture

```
src/main.rs                 CLI flags, single-instance negotiation, tokio runtime, run_native
src/engine/actor.rs         Actor task owning the librqbit Session; commands in, ArcSwap<Snapshot> out
src/engine/{session,types,automation,restore}.rs
                            SessionOptions from Settings; view types; watch folders & run-on-complete;
                            recovery of torrents the engine failed to load at startup
src/store.rs                App-side per-torrent data (categories, tags, dates, queue, pause reasons,
                            cumulative counters) — atomic JSON with .bak
src/config.rs               Settings (TOML, serde defaults), paths (portable vs OS dirs)
src/integration/            single_instance (local socket IPC), os_handler(+_windows) magnet/.torrent
                            registration, macos_events (Apple Events)
src/app/                    eframe App: titlebar/toolbar, sidebar, table, details, dialogs, settings
                            window, tray, window lifecycle, toasts, extras (about/log/stats)
src/ui/                     theme (palettes, fonts), widgets (progress bar, sparkline, badges…), file tree
vendor/librqbit-dht/        Patched DHT (keeps running on Windows WSAECONNRESET) — see CORRENT-PATCH.md
docs/                       research.md, PLAN.md (v1 plan + review), ROADMAP.md (future)
```

Key invariants:
- The UI thread never blocks on I/O: it sends `EngineCommand`s and reads snapshots.
- Torrents are identified by info hash everywhere (engine ids change on re-add).
- librqbit has no runtime API for limits/trackers/location: those use forget + re-add, which
  re-verifies (never re-downloads); always keep a metainfo backup until the re-add succeeds.
- Fast resume must stay on by default; never rewrite the engine's `session.json` while it runs.
- Fonts in `assets/fonts` are subset without Private-Use-Area glyphs (they shadowed Phosphor icons).

## Status (2026-10-08)

Done and committed: engine actor and snapshots; add flow with metadata preview; magnet/.torrent
OS integration (Windows/Linux/macOS) + single instance; list with sorting, columns, filters
(status/category/tag), search; details tabs (General, Files, Peers, Trackers, Pieces, Speed);
settings window (all sections); queue, seeding limits, alt-speed schedule, watch folders,
run-on-complete; tray, notifications, close/minimize to tray; custom title bar; About/Log/Stats;
failed-restore recovery; fixes from a Fable code review. Default save folder: `Downloads/Corrent`.

Next steps: see the milestone table in `docs/ROADMAP.md` (M1 = async file dialogs).

Known gaps: engine lacks encryption, per-file priorities, sequential mode and per-torrent speed
limits; file pickers block the window while open; macOS/Linux builds are type-checked only, never
run; no CI. See `docs/ROADMAP.md` for what comes next.
