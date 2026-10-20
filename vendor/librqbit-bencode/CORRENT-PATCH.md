# Vendored librqbit-bencode 9.0.1

Copy of `librqbit-bencode` 9.0.1 from crates.io with two changes for untrusted input (STABILITY.md
E2), marked `CORRENT PATCH (E2)` in `src/deserialize.rs` and `src/lib.rs`:

- **Bounded nesting.** The deserializer recursed once per list or dict with no limit, so a few
  megabytes of `llll…` overflowed the stack and aborted the whole process. That input could come
  from a `.torrent` file, a magnet's metadata from peers (ut_metadata), a DHT packet, a peer's
  extended messages or a tracker response. `BencodeDeserializer` now counts open lists and dicts
  and refuses more than `MAX_DEPTH` (512) with "nested more than 512 levels deep". That covers
  every parser in every librqbit crate at once. Real data nests a few levels; a hybrid torrent's
  file tree nests two per folder.
- **Checked numbers.** String lengths and integers were parsed with unchecked arithmetic: 20 or
  more digits panicked in builds with overflow checks and wrapped to a wrong length without them.
  They now use atoi's checked parsers, so such a number is `InvalidValue`. One behaviour change: an
  integer above `i64::MAX` in a dynamically typed place (a skipped unknown key, a `BencodeValue`)
  now fails the whole message instead of wrapping silently.

The limit was measured in Corrent's dev profile, which builds dependencies at opt-level 2: 512
levels fit a 1 MiB stack there. Corrent parses on 2 MiB tokio threads.

The tests are in Corrent (`crates/corrent-core/src/engine/untrusted_tests.rs`), because vendored
crates aren't workspace members and their tests don't run in the gate.

`docs/upstream/librqbit-bencode-depth.patch` is the diff to propose upstream. Drop this copy once a
release has equivalent limits.
