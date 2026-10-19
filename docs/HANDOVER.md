# Handover — media server work, moving development to Windows (2026-10-19)

From 2026-10 until this handover, Corrent was turned from a torrent client into a media server with
clients. That work was developed and tested on **Linux (Fedora, x86_64)**. Development continues on
**Windows** from here. This page is the starting point: what exists, how to build and check it on
Windows, and what is still open.

## Where things are

- **Plan and status:** `docs/MEDIASERVER.md`. The milestone table near the end has the status, and
  every milestone MS0–MS17 is marked Done. `docs/JELLYFIN-PLAN.md` covers the Jellyfin API.
- **Running the server:** `docs/SERVER.md` (headless `correntd`: config, remote access, ACME,
  libraries, playback and direct stream, DLNA, Cast, Jellyfin apps, services, backups).
- **API:** `docs/API.md`. A test keeps it complete: every `.route()` in `crates/corrent-server/src/v1`
  must be listed.
- **Plugins:** `docs/PLUGINS.md`. **Stability:** `docs/SERVER-STABILITY.md` (failure inventory,
  fuzzing, soak results) and `docs/STABILITY.md` (desktop engine).
- **Workspace:** the root crate is the desktop app, and the libraries live in `crates/`:
  - `corrent-core`: engine, settings, RSS, store.
  - `corrent-server`: API v1, web client hosting, DLNA, Cast, Jellyfin, ACME, backups, tasks.
  - `correntd`: the headless binary.
  - `corrent-library`: scanning, metadata, queries, playlists.
  - `corrent-transcode`: play decision, HLS transcodes, trickplay, intro detection, and the direct
    stream remuxer in `remux/`.
  - `corrent-mpv`: libmpv loaded at runtime, plus the `corrent-mpvw` worker.
  - `corrent-plugins`: wasmtime component host and built-in plugins.
  - `corrent-db`: SQLite, migrations 0001–0010.
  - `corrent-proto`, `corrent-i18n`, `corrent-fuzz`.
- **Clients:** the web client is in `web/` (Svelte 5; built into `web/dist`, which is committed and
  embedded). The desktop media UI is in `src/app/media/`.

## Building and checking on Windows

1. **Toolchains:** Rust (MSVC, see `rust-version` in the manifests); Node 20+ for `web/`;
   `cargo install cargo-deny`; `rustup target add wasm32-wasip2` only if you rebuild plugins.
2. **libmpv:** Corrent loads `libmpv-2.dll` (or `mpv-2.dll`/`mpv-1.dll`) at runtime. Put one next to
   the executables or on `PATH` (for example from a shinchiro mpv dev build). Without it, playback,
   transcodes and the mpv-based tests are skipped or unavailable, and probing falls back to
   `ffprobe` if that is installed. The `corrent-mpvw` worker (`cargo build -p corrent-mpv --bin corrent-mpvw`)
   is found next to the server binary.
3. **Gate:** `powershell -File scripts/gate.ps1`, the same checks as `scripts/gate.sh`. Commit only after
   it prints `GATE OK`.
4. **Web client:** the scripts are bash: run `scripts/web-build.sh --no-ci` from Git Bash, or run its
   steps in PowerShell: `cd web; npm ci; npm run check; npm run build`. Commit `web/dist` with the
   change.
5. **Plugins:** `scripts/build-plugins.sh` (Git Bash) rebuilds the built-in wasm plugins into
   `crates/corrent-plugins/builtin/`.
6. **Soak:** run `bash scripts/soak.sh [minutes]` from Git Bash. On Windows it samples with
   `Get-Process` (working set, threads, and handles in the `fds` column). A 2-minute check run on
   2026-10-19 ended with no errors and stayed flat at about 36 MB. The video and transcode part runs
   only when `mpv` is on Git Bash's `PATH`.
7. **Test profiles:** the desktop app uses `--portable` (`target/debug/corrent-data`). The server takes
   `correntd --data-dir target/debug/correntd-data`.

**Verified on Windows (2026-10-19):** the full gate passes (`c68d054`). The first round of fixes:
`paths::resolve` returned verbatim `\\?\C:\…` paths, so library file paths never matched torrent
or settings paths; and a fast download could announce `Completed` before its add was recorded
(no category). With `mpv.exe` on `PATH`, the mpv-based server tests run (they make test clips with
it); without `libmpv-2.dll` the in-process ones are skipped.

**Not yet verified on Windows:** `correntd` as a Windows service, the mpv worker, DLNA/SSDP and
mDNS through the Windows firewall, and the Jellyfin listener on port 8096.

Windows notes: run mpv checks with `--no-config` (a user `mpv.conf` with `keep-open` makes it
never exit); an `ffprobe.exe` from a Python package may be on `PATH` and isn't the real one.

## Recent work worth knowing

- **Direct stream (V4b), `3a857b1`:** Matroska with H.264/HEVC + AAC/AC-3/E-AC-3/Opus is copied into
  HLS/fMP4 in Rust instead of being transcoded (`method: "remux"`, `/api/v1/remux/...`).
  - Checked with mpv against the Matroska test suite files, and in Chrome (AAC and Opus, seeking, no
    dropped frames).
  - `cargo run -p corrent-transcode --example remux -- in.mkv out/ [audio | --index]` writes a file's
    HLS, or prints its cluster index.
- **Concurrent adds bug, `e7b4050`:** the vendored librqbit's JSON persistence handed the same id to
  adds that ran at the same time, so a batch add of 500 kept only 157. It now reserves ids. The diff
  for upstream is in `docs/upstream/librqbit-json-next-id.patch`, with a regression test in the
  engine e2e suite.
- **ACME, `0c469e8`:** Let's Encrypt certificates for `acme_domains` (TLS-ALPN-01 on the HTTPS port).
  It has never run against a real domain.
- **Fetch safety and fixes, `1f2cd7d`:** radio and podcast fetches go to public addresses only (with
  size caps); also the streamed backup upload, restore order and rollback.

## Open items

**From the Fable review of the remuxer (`3a857b1`):** all eleven items are done (`f68ba87`): a
4-slot limit on remux work and zero-copy frames, compression checked only on the remuxed tracks,
the index cached per file, saturating timestamps, block groups read child by child, one global
shift for negative timestamps, segment-end durations, half-frame audio gaps, resync after damaged
clusters, `?v=` + ETag on segments, and an 8 KiB index buffer. Checked with mpv against an x264
(B-frames) + AAC file, playing through and after a seek.

**Soak.**
- An 82-minute run with 5,000 torrents went cleanly; see `docs/SERVER-STABILITY.md`.
- Memory rose to about 700 MB and leveled off.
- The 72-hour run is still open; watch whether memory stays flat.

**Needs the owner, real hardware, or keys ([O] in the plan):**
- the Jellyfin API with real Jellyfin apps;
- Cast to a real Chromecast (only tested against a fake receiver);
- ACME with a real domain;
- the TMDB, TVDB, fanart.tv and OpenSubtitles providers with API keys;
- the plugin catalog repository;
- macOS runs (`docs/PLATFORM-TESTING.md`);
- Windows service install, stop and shutdown;
- crossfade, which mpv can't do because it plays one file at a time.

**Desktop roadmap:** hardening per `docs/STABILITY.md`. S1 (profile data-loss guards) is done on
Windows; see its Results rows. S2 is next. Then upstreaming the librqbit patches (`docs/upstream/`),
and a first tagged release.
- S1 adds a lock that the desktop app and `correntd` share: `corrent.lock` in the data folder. It
  replaces `correntd.lock`, and a stale `correntd.lock` file is harmless.
- Store and RSS saves now go through `corrent_core::writer`.
- Left open from the S1 review:
  - Closing the console window of a `correntd` started by hand on Windows, or logging off, still
    kills it before it saves. A fix needs a `SetConsoleCtrlHandler` callback that blocks until
    shutdown has run; the service path handles this.
  - On Windows, E1 refuses torrents with a `:` in a file name (for example `Title: Subtitle.mkv`).
    Offering a rename in the add dialog would be friendlier.
  - A watch-folder add that the user cancels renames the file to `.failed`, the same as a failed
    add.

## Conventions that bit during this work

- **Strings:** every user-visible string goes through `t!` with a Turkish entry. Rust strings go in
  `assets/i18n/tr.toml`; web strings go in `assets/i18n/web/tr.toml`, and the web check reads both.
- **Gate output:** never pipe the gate into `tail` before deciding to commit; check for `GATE OK`.
- **Disk:** incremental builds are off. `target/` grows past 80 GB with the whole workspace, so
  clear stale `target/debug/deps/corrent*` artifacts now and then.
- **Plans and reviews:** plan milestones and review larger changes with Fable agents, then fix what
  they find.
