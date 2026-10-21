# Vendored librqbit 9.0.1

Unmodified copy of `librqbit` 9.0.1 from crates.io (without the `webui/` sources, which are only
used by the `webui`/`http-api` features Corrent doesn't enable), plus runtime controls on
`ManagedTorrent` (search for `CORRENT PATCH`):

- `set_ratelimits(LimitsConfig)` / `ratelimits()` — change a live torrent's speed limits.
- `set_ratelimit_group(Option<Arc<Limits>>)` / `ratelimit_group()` — also draw from a shared
  limiter (Corrent uses one per category); `Limits::set_group`/`group` in `src/limits.rs`.
- Rate limiters allow a burst of at least one 16 KiB chunk (`src/limits.rs`); without it any
  limit under 16 KiB/s made governor reject every chunk and transfers stopped.
- `set_file_priorities(Vec<usize>)` / `file_priorities()` — the order files are downloaded in
  (librqbit otherwise always uses file-name order).
- JSON session persistence reserves ids (`next_free` in `src/session_persistence/json.rs`):
  `next_id` returned "largest stored id + 1", and ids are stored only after an add finishes, so
  concurrent adds got the same id and all but one were refused as "already managed" (found by
  the soak adding 500 torrents at once). Diff: `docs/upstream/librqbit-json-next-id.patch`.
- Durable session files (`src/session_persistence/json.rs`, marked `CORRENT PATCH (D5)`):
  - `session.json` and the initial `<hash>.bitv` are synced before their rename.
  - `<hash>.torrent` goes through a synced temp file and a rename. Before, it was truncated and
    rewritten in place on every add and re-add.
  - Temp names no longer use `to_str().unwrap()`, which panicked for a data folder whose name
    isn't valid Unicode.
  - The renames are retried for about 1.5 s when a sharing or access error says another program
    holds the target (antivirus, indexer, sync clients on Windows).

  The folder itself isn't synced (that matters on Unix only): a lost rename leaves the previous,
  complete file. Diff: `docs/upstream/librqbit-persistence-fsync.patch`.
- Limits for the session's HTTP client (`src/session.rs`, marked `CORRENT PATCH (F4)`).
  - New `SessionOptions::http_timeout` caps a whole request at 60 s by default, with a 15 s
    connect timeout. The client had none, so a server that trickled bytes held a URL add (or an
    HTTP tracker announce, which uses the same client) forever.
  - A `.torrent` downloaded from a URL is read up to 32 MiB; before, the body was read whole.

  Diff: `docs/upstream/librqbit-http-timeouts.patch`.

- Test-only kill points (`src/corrent_hooks.rs`, call sites marked `CORRENT PATCH (D8)` in
  `src/session_persistence/json.rs` and `src/bitv.rs`): after the temp file and after the rename
  of `session.json`, `<hash>.torrent` and the initial `.bitv`, and after each write and sync of
  the `.bitv` flusher. They call a hook only when one is set (`set_kill_hook`), which only
  Corrent's durability harness does, to end the process there (STABILITY.md D8). No upstream
  diff: it exists for Corrent's tests.

- Torrent sizes checked before `validate()` (`src/corrent_sizes.rs`, call sites marked
  `CORRENT PATCH (Z1)` in `src/session.rs`: adds from bytes, and metadata received for a magnet).
  librqbit-core's `Lengths` sums file lengths unchecked, divides by the piece length and keeps the
  piece count in a u32. A piece length of 0 panicked in every build (the add never finished);
  file lengths adding up past 2^64 or more than 2^32 pieces panicked in debug builds and gave
  wrong sizes in release. Found by the Z1 fuzz target (STABILITY.md S9). The proper fix belongs
  in librqbit-core (`Lengths::from_torrent`/`new`); diff: `docs/upstream/librqbit-length-checks.patch`.

librqbit rebuilds these from the add-time options whenever a torrent goes live again, so Corrent
stores them itself and re-applies them (`Actor::apply_torrent_limits`).

Note: librqbit already downloads pieces in order within each file (first and last piece first),
so a separate "sequential download" switch would change nothing.

`docs/upstream/librqbit-runtime-controls.patch` is the diff to propose upstream. Drop this copy
once a release has equivalent APIs.
