# Vendored librqbit 9.0.1

Unmodified copy of `librqbit` 9.0.1 from crates.io (without the `webui/` sources, which are only
used by the `webui`/`http-api` features Corrent doesn't enable), plus two methods on
`ManagedTorrent` in `src/torrent_state/mod.rs` (search for `CORRENT PATCH`):

- `set_ratelimits(LimitsConfig) -> bool` changes a live torrent's download/upload limiter.
- `ratelimits() -> Option<LimitsConfig>` reads the limits in effect.

librqbit rebuilds the limiter from the add-time options whenever a torrent goes live again, so
Corrent stores per-torrent limits itself and re-applies them (see `Actor::apply_torrent_limits`).

Upstream: see `docs/upstream/` for the patch to propose. Drop this copy once a release has an
equivalent API.
