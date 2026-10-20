# STABILITY.md R7: builds the release binaries and checks that nothing meant for development
# reached them: debug hooks (CORRENT_DEV_*), fault injection, the test kit, fuzzing and tokio
# console. Prints "RELEASE HYGIENE OK" or what was found (exit code 1). Slow (an LTO build), so
# not part of gate.ps1; run it before tagging a release.
#   powershell -File scripts/release-hygiene.ps1
$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')
$markers = @('CORRENT_DEV_', 'CORRENT_FAULTS_COMPILED_IN', 'CORRENT_TESTKIT_COMPILED_IN', 'CORRENT_FUZZ_ITERS',
    'corrent_fuzz', 'console_subscriber', 'tokio_unstable')
function Find-Markers([string]$exe) {
    $text = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($exe))
    $markers | Where-Object { $text.Contains($_) }
}

# The scan must be able to see a marker: a debug build has the hooks compiled in.
cargo build -p corrent *> $null
if ($LASTEXITCODE -ne 0) { 'RELEASE HYGIENE FAIL: debug build'; exit 1 }
if (-not ((Find-Markers 'target/debug/corrent.exe') -contains 'CORRENT_DEV_')) {
    'RELEASE HYGIENE FAIL: the scan finds nothing in a debug build (broken check)'; exit 1
}

cargo build --release --locked -p corrent -p correntd *> $null
if ($LASTEXITCODE -ne 0) { 'RELEASE HYGIENE FAIL: release build'; exit 1 }
$bad = $false
foreach ($exe in 'target/release/corrent.exe', 'target/release/correntd.exe') {
    foreach ($m in Find-Markers $exe) { "RELEASE HYGIENE FAIL: $exe contains $m"; $bad = $true }
}
if ($bad) { exit 1 }
'RELEASE HYGIENE OK'
