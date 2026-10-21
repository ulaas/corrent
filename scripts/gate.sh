#!/usr/bin/env bash
# Runs every CI gate (fmt, clippy -D warnings, all tests, cargo deny) and prints "GATE OK" or
# which one failed. Commit only after it prints GATE OK:
#   scripts/gate.sh > gate.log 2>&1; grep -q "GATE OK" gate.log && git commit ...
# (don't pipe it into `tail` before checking: that hides the exit status). Windows: gate.ps1.
cd "$(dirname "$0")/.." || exit 1
cargo fmt --all --check >/dev/null 2>&1 || { echo "GATE FAIL: fmt"; exit 1; }
out=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1) || { echo "GATE FAIL: clippy"; echo "$out" | grep -E '^(error|warning)' -A7 | head -40; exit 1; }
# Release builds compile code that debug builds do not (and leave out cfg(debug_assertions)).
out=$(cargo check --workspace --release 2>&1) || { echo "GATE FAIL: release check"; echo "$out" | grep -E '^error' -A7 | head -40; exit 1; }
# The fuzz targets as the cargo-fuzz crate builds them (feature `fuzz`, not only in test builds).
out=$(cargo check --manifest-path fuzz/Cargo.toml 2>&1) || { echo "GATE FAIL: fuzz check"; echo "$out" | grep -E '^error' -A7 | head -40; exit 1; }
out=$(cargo test --workspace 2>&1); if echo "$out" | grep -qE 'FAILED|panicked|error(\[|:)'; then echo "GATE FAIL: tests"; echo "$out" | grep -E 'FAILED|panicked|^error' -A5 | head -40; exit 1; fi
cargo deny check 2>&1 | tail -1 | grep -q 'advisories ok, bans ok, licenses ok, sources ok' || { echo "GATE FAIL: deny"; exit 1; }
echo "GATE OK ($(echo "$out" | grep -c 'test result: ok') test binaries)"
