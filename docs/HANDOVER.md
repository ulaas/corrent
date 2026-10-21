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
2. **libmpv:** Corrent loads `libmpv-2.dll` (or `mpv-2.dll`/`mpv-1.dll`) at runtime, from next to
   the executables or from `PATH`. An installed mpv player doesn't include it.
   - Run `powershell -File scripts/fetch-libmpv.ps1` once. It downloads the pinned shinchiro
     `mpv-dev-x86_64` build, checks its SHA-256 and keeps `libmpv-2.dll` in `.cache\libmpv\`
     (ignored by git).
   - From then on, every build of the `corrent` crate copies it into `target\<profile>\` (`build.rs`).
     The script also copies it into the target folders that already exist.
   - `-Latest` takes the newest release (with GitHub's digest) once the pinned one has been pruned.
   - Cargo puts `target\debug` on the test binaries' search path, so the in-process mpv tests run
     too (the gate then covers them; they first ran on Windows on 2026-10-20).
   - Without it, playback, transcodes and the mpv-based tests are skipped or unavailable, and probing
     falls back to `ffprobe` if that is installed. The `corrent-mpvw` worker (`cargo build -p corrent-mpv --bin corrent-mpvw`)
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

- **Media round MS18 (2026-10-21, `023e519`..):** wrapping Home/library pages, frame thumbnails,
  Add to library…, "use this frame", provider keys in Settings → Metadata, Identify…, OMDb
  ratings, and an MCP server for AI agents whose changes are approved in Media → AI agent
  (`corrent --mcp` for stdio clients). Test media: `scripts/dev-media.ps1`; screenshots with
  `CORRENT_DEV_LIBRARY`/`CORRENT_DEV_PAGE`. Building plugins needs `rustup target add
  wasm32-wasip2` (installed on this machine) and `scripts/build-plugins.sh`. Details in
  `docs/MEDIASERVER.md` (MS18 row).
- **S8 (fault injection), 2026-10-21:** `crates/corrent-core/src/engine/fault_tests.rs` (`fm_*`)
  with test-only `faulty_storage.rs`, `netproxy.rs`, `faultserver.rs` and new `faults` rules
  (storage faults, profile write errors, `jump_ticks`). `scripts/fault-matrix.ps1` runs them and
  repeats the timing-sensitive ones 20 times. Product fixes it found: Resume restarts a torrent
  stopped by a disk error; the update check caps GitHub's answer; a wake from sleep no longer
  counts the time asleep (I1).
- **S9 (fuzz and property tests), 2026-10-21:** fuzz target bodies in
  `crates/corrent-core/src/fuzz.rs` (`z1`–`z7`, feature `fuzz`) and
  `crates/corrent-server/src/webapi/fuzz.rs` (`z4`); seeds and every failing input in
  `tests/fuzz-corpus/` (replayed by `cargo test`); cargo-fuzz crate in `fuzz/` (nightly);
  property tests P1–P10 in `crates/corrent-core/src/prop_tests.rs` (proptest). Long run:
  `scripts/fuzz-long.ps1`. Fixes it found: torrent sizes librqbit-core can't hold (a piece
  length of 0 hung the add; vendored `corrent_sizes::check`), magnets whose `so=` asked for all
  of memory (`corrent_sizes::parse_magnet`), counters and the schedule's weekday (saturating,
  guarded).
- **Logins:** no bans or lockouts after wrong passwords any more (owner's choice); wrong pairing
  codes and PINs are still limited per address.
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

**Desktop roadmap:** hardening per `docs/STABILITY.md`. S1 (profile data-loss guards), S2 (engine
persistence durability), S3 (no hangs) and S4 (untrusted input) are done on Windows; see their
Results rows. S5 (panic-free gate), S6 (durability tests) and S7 (store schema v2, session.json rebuild) are done too. Then upstreaming the librqbit patches (`docs/upstream/`),
and a first tagged release.
- S1 adds a lock that the desktop app and `correntd` share: `corrent.lock` in the data folder. It
  replaces `correntd.lock`, and a stale `correntd.lock` file is harmless.
- Store and RSS saves now go through `corrent_core::writer`.
- S2 adds a third librqbit patch (D5, `docs/upstream/librqbit-persistence-fsync.patch`). In
  `session/` it keeps `session.json.good-*` and `.bad-*` copies (`engine/session_db.rs`); in
  `logs/` the files `alive` and `power-loss.pending` (`crash.rs`).
- S3 adds:
  - test-only fault injection in `corrent_core::faults` (slow disk, stalled engine calls, start
    delay or deadline, HTTP timeout, all scoped to one test's folder);
  - one engine-call worker in the actor;
  - `corrent_core::logfile`: `corrent.log` now rotates to `corrent.log.1` … `.5`;
  - a fourth librqbit patch (`docs/upstream/librqbit-http-timeouts.patch`).
- S4 vendors a third crate, `librqbit-bencode` (bounded nesting and checked numbers), patched
  like the other two (`[patch.crates-io]`). Its tests live in
  `crates/corrent-core/src/engine/untrusted_tests.rs`.
- S5 adds:
  - workspace lints in the root `Cargo.toml`, with `clippy.toml` exempting tests: no `unwrap`,
    `expect` or `panic!` outside tests anywhere, and no indexing in corrent-core, the Web API or
    the GUI. A site that can't fail keeps `#[expect(clippy::…, reason = "…")]`.
  - `corrent_core::util::{lock, with_lock, read, write}` for std locks; a scan test rejects any
    other way of taking one.
  - a bounded command queue: `EngineHandle::send` returns false when it is full. Bounded internal
    results; events stay unbounded.
  - `corrent_core::retention`, a startup clean-up of old `.bad-*`, migration and `pre-restore-*`
    copies.
  - `scripts/release-hygiene.ps1`, a release check to run before tagging.
- S6 adds:
  - the durability harness `corrent-harness`, a test-only binary in corrent-core, and
    `crates/corrent-core/tests/durability.rs`. The full kill matrix runs with
    `cargo test -p corrent-core --features testkit --test durability -- --ignored --nocapture`.
  - kill points (`faults::kill_point`) in every durable write of the profile (not `servers.toml`
    or the server's TLS files and databases), plus a test-only hook in the vendored librqbit
    (`src/corrent_hooks.rs`).
  - a start-up sweep of `*.tmp` files left by a crash.
  - profile fixtures in `crates/corrent-core/tests/fixtures/`. Add one when a format changes
    (`write_current_fixture`).
- S7 adds:
  - store schema 2: `TorrentMeta::{output_folder, only_files, paused}`, copies of the engine's
    side, and `metainfo_missing`. `migrate::PROFILE_VERSION` names the fixtures (`profile-v2`).
  - `session_db::plan`/`commit`: restore from a good copy (D6a) or rebuild from
    `session/*.torrent` + the store (D6b); the store is saved before the list is written.
  - `corrent-harness damage <scratch> garbage|missing` for the rebuild in the kill test.
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
