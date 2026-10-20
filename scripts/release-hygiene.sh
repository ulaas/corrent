#!/usr/bin/env bash
# STABILITY.md R7: builds the release binaries and checks that nothing meant for development
# reached them (debug hooks, fault injection, the test kit, fuzzing, tokio console). Prints
# "RELEASE HYGIENE OK" or what was found (exit code 1). Same check as release-hygiene.ps1.
set -u
cd "$(dirname "$0")/.."
markers=(CORRENT_DEV_ CORRENT_FAULTS_COMPILED_IN CORRENT_TESTKIT_COMPILED_IN CORRENT_FUZZ_ITERS corrent_fuzz
  console_subscriber tokio_unstable)
ext=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) ext=".exe" ;; esac

# The scan must be able to see a marker: a debug build has the hooks compiled in.
cargo build -p corrent >/dev/null 2>&1 || { echo "RELEASE HYGIENE FAIL: debug build"; exit 1; }
grep -qa CORRENT_DEV_ "target/debug/corrent$ext" ||
  { echo "RELEASE HYGIENE FAIL: the scan finds nothing in a debug build (broken check)"; exit 1; }

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
