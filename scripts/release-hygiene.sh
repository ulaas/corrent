#!/usr/bin/env bash
# STABILITY.md R7: builds the release binaries and checks that nothing meant for development
# reached them (debug hooks, fault injection, the test kit, fuzzing, tokio console). Prints
# "RELEASE HYGIENE OK" or what was found (exit code 1). Same check as release-hygiene.ps1.
set -u
cd "$(dirname "$0")/.."
markers=(CORRENT_DEV_ CORRENT_FAULTS_COMPILED_IN CORRENT_FUZZ_ITERS CORRENT_FUZZ_SECS CORRENT_FUZZ_COMPILED_IN corrent_fuzz
  console_subscriber tokio_unstable)
ext=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) ext=".exe" ;; esac

# The scan must be able to see the markers: a debug build with the fuzz targets (and the test kit) has them compiled in
# (built into its own target folder, so the normal debug build stays as it is).
cargo build -p corrent --features corrent-core/fuzz --target-dir target/hygiene >/dev/null 2>&1 ||
  { echo "RELEASE HYGIENE FAIL: control build"; exit 1; }
for m in CORRENT_DEV_ CORRENT_FAULTS_COMPILED_IN CORRENT_FUZZ_COMPILED_IN; do
  grep -qa "$m" "target/hygiene/debug/corrent$ext" ||
    { echo "RELEASE HYGIENE FAIL: the scan can't see $m in a control build (broken check)"; exit 1; }
done

cargo build --release --locked -p corrent -p correntd >/dev/null 2>&1 ||
  { echo "RELEASE HYGIENE FAIL: release build"; exit 1; }
bad=0
for exe in "target/release/corrent$ext" "target/release/correntd$ext"; do
  for m in "${markers[@]}"; do
    if grep -qa "$m" "$exe"; then echo "RELEASE HYGIENE FAIL: $exe contains $m"; bad=1; fi
  done
done
[ "$bad" = 0 ] || exit 1
echo "RELEASE HYGIENE OK"
