# Patches to propose upstream (librqbit)

Corrent vendors two librqbit crates with small changes (`vendor/*/CORRENT-PATCH.md`). These are
the diffs against the crates.io 9.0.1 sources, ready to turn into upstream pull requests:

- `librqbit-dht-connreset.patch` — keep the DHT worker alive on transient UDP receive errors
  (Windows reports ICMP port-unreachable as WSAECONNRESET on the socket).
- `librqbit-runtime-controls.patch` — runtime speed limits per torrent, a shared (group) limiter
  and per-file download order on `ManagedTorrent`.

The owner submits them; Corrent drops the vendored copies once a release contains equivalents.
