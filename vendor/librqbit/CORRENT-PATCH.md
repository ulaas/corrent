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

librqbit rebuilds these from the add-time options whenever a torrent goes live again, so Corrent
stores them itself and re-applies them (`Actor::apply_torrent_limits`).

Note: librqbit already downloads pieces in order within each file (first and last piece first),
so a separate "sequential download" switch would change nothing.

`docs/upstream/librqbit-runtime-controls.patch` is the diff to propose upstream. Drop this copy
once a release has equivalent APIs.
