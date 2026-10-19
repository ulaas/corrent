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

librqbit rebuilds these from the add-time options whenever a torrent goes live again, so Corrent
stores them itself and re-applies them (`Actor::apply_torrent_limits`).

Note: librqbit already downloads pieces in order within each file (first and last piece first),
so a separate "sequential download" switch would change nothing.

`docs/upstream/librqbit-runtime-controls.patch` is the diff to propose upstream. Drop this copy
once a release has equivalent APIs.
