# Patches to propose upstream (librqbit)

Corrent vendors two librqbit crates with small changes (`vendor/*/CORRENT-PATCH.md`). These are
the diffs against the crates.io 9.0.1 sources (or against the previous patch for the same file),
ready to turn into upstream pull requests:

- `librqbit-dht-connreset.patch` — keep the DHT worker alive on transient UDP receive errors
  (Windows reports ICMP port-unreachable as WSAECONNRESET on the socket).
- `librqbit-runtime-controls.patch` — runtime speed limits per torrent, a shared (group) limiter
  and per-file download order on `ManagedTorrent`.
- `librqbit-json-next-id.patch` — the JSON session store hands each concurrent add its own id
  (before, concurrent adds got the same id and all but one failed).
- `librqbit-persistence-fsync.patch` — `session.json`, the initial `.bitv` and the `.torrent`
  copies are synced before their rename, so a power cut can't leave them empty or cut off. It
  also drops a `to_str().unwrap()` that panicked on non-Unicode paths. It applies on top of the
  next-id patch.

The owner submits them; Corrent drops the vendored copies once a release contains equivalents.
