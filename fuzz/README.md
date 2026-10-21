# Fuzz targets (cargo-fuzz, nightly)

Coverage-guided fuzzing of the parsers that take bytes from outside the machine (STABILITY.md
workstream Z). Each target calls the same body as the stable runner in `cargo test`, so the
two never drift:

| Target | Body | Input |
|---|---|---|
| `z1` | `corrent_core::fuzz::z1` | torrent files |
| `z2_peer` | `corrent_core::fuzz::z2_peer` | peer wire messages, extended messages, ut_metadata info |
| `z2_dht` | `corrent_core::fuzz::z2_dht` | DHT (KRPC) packets |
| `z3` | `corrent_core::fuzz::z3` | streaming server request heads |
| `z4` | `corrent_server::webapi::fuzz::z4` | qBittorrent Web API inputs |
| `z5` | `corrent_core::fuzz::z5` | RSS/Atom/JSON feeds, Torznab answers |
| `z6` | `corrent_core::fuzz::z6` | magnets, URLs, launch items |
| `z7` | `corrent_core::fuzz::z7` | qBittorrent/Transmission resume data |

```sh
cargo install cargo-fuzz          # once
cd fuzz
cargo +nightly fuzz run z1 corpus/z1 ../tests/fuzz-corpus/z1 -- -rss_limit_mb=256 -max_len=65536 -max_total_time=1800
```

The corpus folders are `tests/fuzz-corpus/<target>/`.
libfuzzer adds what it finds to the first folder (`fuzz/corpus/<target>`, not committed) and
only reads the second; commit only inputs that broke something.
Every crash or timeout under `fuzz/artifacts/` goes into `tests/fuzz-corpus/<target>/` as
`crash-<yyyymmdd>-<slug>`, in the same commit as its fix (`tests/fuzz-corpus/README.md`).

Without nightly, `powershell -File scripts/fuzz-long.ps1` runs the same bodies with seeded
mutations (not coverage-guided). On stable, `cargo check --manifest-path fuzz/Cargo.toml`
type-checks this crate.
