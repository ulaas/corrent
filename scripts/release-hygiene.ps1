# STABILITY.md R7: builds the release binaries and checks that nothing meant for development
# reached them: debug hooks (CORRENT_DEV_*), fault injection, the test kit, fuzzing and tokio
# console. Prints "RELEASE HYGIENE OK" or what was found (exit code 1). Slow (an LTO build), so
# not part of gate.ps1; run it before tagging a release.
#   powershell -File scripts/release-hygiene.ps1
$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')
$markers = @('CORRENT_DEV_', 'CORRENT_FAULTS_COMPILED_IN', 'CORRENT_FUZZ_ITERS', 'CORRENT_FUZZ_SECS', 'CORRENT_FUZZ_COMPILED_IN',
    'corrent_fuzz', 'console_subscriber', 'tokio_unstable')
function Find-Markers([string]$exe) {
    $text = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($exe))
    $markers | Where-Object { $text.Contains($_) }
}

# The scan must be able to see the markers: a debug build with the fuzz targets (and the test kit) has them compiled in
# (built into its own target folder, so the normal debug build stays as it is).
cargo build -p corrent --features corrent-core/fuzz --target-dir target/hygiene *> $null
if ($LASTEXITCODE -ne 0) { 'RELEASE HYGIENE FAIL: control build'; exit 1 }
$seen = Find-Markers 'target/hygiene/debug/corrent.exe'
foreach ($m in 'CORRENT_DEV_', 'CORRENT_FAULTS_COMPILED_IN', 'CORRENT_FUZZ_COMPILED_IN') {
    if (-not ($seen -contains $m)) { "RELEASE HYGIENE FAIL: the scan can't see $m in a control build (broken check)"; exit 1 }
}

cargo build --release --locked -p corrent -p correntd *> $null
if ($LASTEXITCODE -ne 0) { 'RELEASE HYGIENE FAIL: release build'; exit 1 }
$bad = $false
foreach ($exe in 'target/release/corrent.exe', 'target/release/correntd.exe') {
    foreach ($m in Find-Markers $exe) { "RELEASE HYGIENE FAIL: $exe contains $m"; $bad = $true }
}
if ($bad) { exit 1 }
'RELEASE HYGIENE OK'
