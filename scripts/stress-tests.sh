#!/usr/bin/env bash
# Runs the workspace's tests on a few CPU cores, several times, to find the timing flakes that only
# show on slow machines (CI runners have 2-4 cores). Lists every test that failed at least once.
#   scripts/stress-tests.sh [runs=3] [cores=2] [cargo test selection, default --workspace]
# Windows: stress-tests.ps1 (same idea; also prints each flake's first panic).
cd "$(dirname "$0")/.." || exit 1
runs=${1:-3}
cores=${2:-2}
shift 2 2>/dev/null
sel=("$@")
[ ${#sel[@]} -eq 0 ] && sel=(--workspace)
command -v taskset >/dev/null || { echo "taskset is needed (util-linux)"; exit 1; }
echo "==> building the test binaries"
cargo test "${sel[@]}" --no-run >/dev/null 2>&1 || { echo "build failed (run cargo test --no-run for the errors)"; exit 1; }
failed=$(mktemp)
for run in $(seq 1 "$runs"); do
    taskset -c "0-$((cores - 1))" cargo test "${sel[@]}" --no-fail-fast 2>&1 | grep -E '^test \S+ \.\.\. FAILED' >>"$failed"
    echo "  run $run/$runs: $(sort -u "$failed" | wc -l) flaky so far"
done
if [ -s "$failed" ]; then
    echo "STRESS FAIL: tests that failed at least once (count, test)"
    sed -E 's/^test (\S+) .*/\1/' "$failed" | sort | uniq -c | sort -rn
    rm -f "$failed"
    exit 1
fi
rm -f "$failed"
echo "STRESS OK ($runs runs on $cores cores)"
