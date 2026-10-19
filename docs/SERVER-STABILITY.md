# Server stability (MS17)

The hardening pass for the media server, in the style of `docs/STABILITY.md` (which covers the
torrent client). Each failure mode lists what happens, what guards it, and the test that proves
it. "Open" items need the owner or long-running hardware.

## Failure inventory

| Area | Failure | Handling | Proof |
|---|---|---|---|
| Database | Writer thread stalls or a write fails mid-batch | One writer thread, batched transactions with savepoints per job; a failed job rolls back only itself; callers get the error after commit; `busy_timeout` 5 s on every connection | `corrent-db` tests (batching, begin failures) |
| Database | Disk full during a write | SQLite returns `SQLITE_FULL`; the job's savepoint rolls back, the API answers 500 for that request, earlier data stays intact (WAL) | covered by SQLite's guarantees; backups (`backups.rs`) give a restore point |
| Database | Corruption / bad upgrade | Numbered migrations in one transaction each; the GUI keeps `.bak` copies of its files; daily `backup` task zips a `VACUUM INTO` copy | `migrations` test, `tasks_notifications_backups_metrics` |
| Transcoding | Worker hangs or crashes | `corrent-mpvw` runs as a child with `kill_on_drop`; idle sessions (90 s) are stopped and their folders removed; a seek outside the encoder's reach restarts it | `corrent-transcode` HLS tests |
| Transcoding | Too many sessions | `max_sessions` (3); the longest-idle session is dropped first, else 429 | HLS tests |
| Transcoding | Cache disk full | No new session below 2 GiB free (`NoSpace`, HTTP 507); the `cleanup_cache` task removes leftovers older than a day | `HlsError::NoSpace` path |
| Probing | A broken file hangs or crashes libmpv | Probes run in the worker process (`kill_on_drop`, 2–3 at a time); a failed probe marks the file, the scan continues | library scan tests |
| Plugins | Runaway CPU, memory or traps | wasmtime epoch deadline (30 s), 64 MiB memory cap, fresh instance per call, three traps disable the plugin; HTTP only to declared hosts, public addresses unless allowed, rate-limited | `corrent-plugins` sandbox tests (`loop`, `alloc`, `undeclared`, `local`) |
| Plugins | A provider is down | Errors are per item; identification falls back to the next provider; nothing blocks scans | `plugins_identify_library_items` |
| Network | Slow or hostile clients | Body limits per route (64 KiB default), WebSocket queues bounded, deltas compressed, per-address sign-in limits plus a global token bucket, pairing codes burn after 50 wrong guesses | API tests |
| Network | Corrent Connect abuse | Peers need the server's key; at most 64 concurrent streams per peer; `/metrics` never over Connect; Connect clients still need accounts | `p2p` tests |
| Inputs | Malformed names, NFOs, subtitles, deltas, API bodies | Robustness tests with seeded random inputs (`corrent-fuzz`): no panic, no hang, sane output; API bodies are refused with 4xx quickly | `fuzz_tests` in `corrent-library` and `corrent-transcode`, `deltas_reproduce_any_change`, `api_inputs_are_refused_cleanly` |
| Engine | A `.torrent` that isn't one | Previews validate the bytes before the session sees them | `api_inputs_are_refused_cleanly` (found the stall) |
| Tasks | A task panics or runs for hours | Each run executes in its own task; a panic is recorded as a failure and never leaves the task "running"; "Run now" answers at once | `tasks_notifications_backups_metrics` |
| Shutdown | Service stopped or OS shutting down | systemd `Type=notify` + `TimeoutStopSec=45`; Windows service reports StopPending (45 s) and takes preshutdown; the engine flushes its session first | manual [O] on Windows |
| Clients | Server unreachable | The app's remote link reconnects with backoff (1–30 s), re-resolves the route (address, then Connect), detects silent connections (75 s read deadline) and shows a banner | `mirrors_and_drives_a_server`, `falls_back_to_corrent_connect` |

Bugs these tests found: subtitle timestamps with huge numbers overflowed (`vtt.rs`), and a
malformed `.torrent` preview could wait two minutes for an answer that never came.

## Robustness tests

`corrent-fuzz` is a dependency-free generator (xorshift, seeded): it mutates valid examples and
mixes in noise (separators, control characters, combining marks, RTL, emoji). Every target runs
a few thousand cases in `cargo test`; for a deeper run:

```sh
CORRENT_FUZZ_ITERS=1000000 cargo test --release -p corrent-library -p corrent-transcode -p corrent-proto fuzz
CORRENT_FUZZ_ITERS=50000 cargo test --release -p corrent-server api_inputs
```

A failing case prints its seed; `Rng::new(seed)` replays it. (cargo-fuzz needs a nightly
toolchain, which the project doesn't use; the targets above are the same functions a
`fuzz/` crate would call.)

## Soak

`scripts/soak.sh [minutes] [dir]` runs a scratch `correntd` on loopback with a photo library and
(when mpv is installed) a generated movie, drives the API in a loop, scans libraries, starts
transcodes, and samples RSS, threads, open files, WebSocket clients and transcodes into
`samples.csv`. It fails on any request error, failed task or panic.

| Run | Result |
|---|---|
| 3 min, this machine (2026-10-19) | 314 rounds, 0 errors, 0 failed tasks, 0 panics; RSS 105 → 107 MB, 41 threads and 29 files steady |
| 72 h with 5k torrents, scans and transcodes | Open [O]: needs a machine that can run for three days |

## Release bar for the server

- Gate green (`fmt`, `clippy -D warnings`, all tests incl. robustness), `cargo deny check`.
- A 24 h soak with flat RSS/files and no failed tasks.
- Real runs on Windows (service install/stop/shutdown) and macOS (launchd) — `docs/PLATFORM-TESTING.md` [O].
