# Vendored librqbit-dht 9.0.1

Unmodified copy of `librqbit-dht` 9.0.1 from crates.io with one change in `src/dht.rs`
(search for `CORRENT PATCH`): transient UDP receive errors (WSAECONNRESET and friends,
which Windows reports when a remote node answers with ICMP port-unreachable) no longer
terminate the DHT worker. Without this, DHT dies seconds after start on Windows and
magnet links cannot be resolved trackerlessly.

Drop this vendored copy once upstream ships an equivalent fix.
