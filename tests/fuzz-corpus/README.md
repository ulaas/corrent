# Fuzz corpus (STABILITY.md workstream Z)

One folder per fuzz target (`crates/corrent-core/src/fuzz.rs` `TARGETS`, and `z4` in
`crates/corrent-server/src/webapi/fuzz.rs`). Every file is replayed by `fuzz_corpus_replay` in
plain `cargo test`, and the stable runners mutate them as seeds.

- `seed-*`: valid inputs the mutations start from.
- `crash-<yyyymmdd>-<slug>`: an input that once broke a target. Add it **before** the fix (the
  replay test must fail), in the same commit as the fix, and never delete it.

Files are binary (`.gitattributes`): don't edit them with tools that change line endings.
