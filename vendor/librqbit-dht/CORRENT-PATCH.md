# Vendored librqbit-dht 9.0.1

Unmodified copy of `librqbit-dht` 9.0.1 from crates.io with one change in `src/dht.rs`
(search for `CORRENT PATCH`): transient UDP receive errors (WSAECONNRESET and friends,
which Windows reports when a remote node answers with ICMP port-unreachable) no longer
terminate the DHT worker. Without this, DHT dies seconds after start on Windows and
magnet links cannot be resolved trackerlessly.

A second, test-only change in `src/lib.rs` (`CORRENT PATCH (Z2)`): `bprotocol` (the KRPC message
parser) is public and `#[doc(hidden)]`, so Corrent's Z2 fuzz target (`corrent_core::fuzz::z2_dht`,
STABILITY.md S9) can reach it. No upstream diff: it exists for Corrent's tests.

Drop this vendored copy once upstream ships an equivalent fix.
