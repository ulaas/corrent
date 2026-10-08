# Patches to propose upstream (librqbit)

Corrent vendors two librqbit crates with small changes (`vendor/*/CORRENT-PATCH.md`). These are
the diffs against the crates.io 9.0.1 sources, ready to turn into upstream pull requests:

- `librqbit-dht-connreset.patch` — keep the DHT worker alive on transient UDP receive errors
  (Windows reports ICMP port-unreachable as WSAECONNRESET on the socket).
- `librqbit-runtime-ratelimits.patch` — `ManagedTorrent::set_ratelimits` / `ratelimits` to change
  a running torrent's speed limits.

The owner submits them; Corrent drops the vendored copies once a release contains equivalents.
