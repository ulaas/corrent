# Patches to propose upstream (librqbit)

Corrent vendors three librqbit crates with small changes (`vendor/*/CORRENT-PATCH.md`). These are
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
- `librqbit-bencode-depth.patch` — librqbit-bencode: nesting bounded at 512 levels and checked
  string lengths and integers, so hostile input can't overflow the stack or panic the parser
  (vendored as `vendor/librqbit-bencode`).
- `librqbit-http-timeouts.patch` — `SessionOptions::http_timeout` (60 s default, 15 s connect)
  for the session's HTTP client, and a 32 MiB cap on `.torrent` files downloaded from a URL.
- `librqbit-create-piece-count.patch` — `create_torrent` wrote an extra (empty) piece hash when
  the total size is an exact multiple of the piece length; libtorrent refuses those torrents.
- `librqbit-length-checks.patch` — refuse torrents whose sizes `Lengths` can't hold (a piece
  length of 0, file lengths adding up past 2^64, more than 2^32 pieces) before `validate()`. The
  upstream fix belongs in librqbit-core's `Lengths::from_torrent`/`new` (checked sum, a zero check,
  a `u32::try_from` for the piece count); this patch is the librqbit-side stopgap. It also parses
  magnets without `so=` ranges that expand to more than 2^20 indices (librqbit-core's
  `Magnet::parse` should bound the range instead of `files.extend(start..=end)`).

The owner submits them; Corrent drops the vendored copies once a release contains equivalents.
