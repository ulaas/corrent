# Runs every CI gate (fmt, clippy -D warnings, all tests, cargo deny) and prints "GATE OK" or
# which one failed (exit code 1). Same checks as gate.sh:
#   powershell -File scripts/gate.ps1 *> gate.log; if (Select-String -Quiet "GATE OK" gate.log) { git commit ... }
# -Stress also runs the tests 3 times on 2 cores (stress-tests.ps1): timing flakes that a fast
# machine hides and CI runners show. Use it after touching tests that wait on the engine.
param([switch]$Stress)
$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')
cargo fmt --all --check *> $null
if ($LASTEXITCODE -ne 0) { 'GATE FAIL: fmt'; exit 1 }
$out = cargo clippy --workspace --all-targets -- -D warnings 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { 'GATE FAIL: clippy'; ($out -split "`n" | Select-String -Pattern '^(error|warning)' -Context 0, 7 | Select-Object -First 40); exit 1 }
# Release builds compile code that debug builds don't (and leave out `cfg(debug_assertions)`).
$out = cargo check --workspace --release 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { 'GATE FAIL: release check'; ($out -split "`n" | Select-String -Pattern '^error' -Context 0, 7 | Select-Object -First 40); exit 1 }
# The fuzz targets as the cargo-fuzz crate builds them (feature `fuzz`, not only in test builds).
$out = cargo check --manifest-path fuzz/Cargo.toml 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { 'GATE FAIL: fuzz check'; ($out -split "`n" | Select-String -Pattern '^error' -Context 0, 7 | Select-Object -First 40); exit 1 }
$out = cargo test --workspace 2>&1 | Out-String
if ($out -match 'FAILED|panicked|error(\[|:)') { 'GATE FAIL: tests'; ($out -split "`n" | Select-String -Pattern 'FAILED|panicked|^error' -Context 0, 5 | Select-Object -First 40); exit 1 }
$deny = cargo deny check 2>&1 | Out-String
if ($deny -notmatch 'advisories ok, bans ok, licenses ok, sources ok') { 'GATE FAIL: deny'; exit 1 }
if ($Stress) {
    $st = & powershell -NoProfile -File (Join-Path $PSScriptRoot 'stress-tests.ps1') 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { 'GATE FAIL: stress'; ($st -split "`n" | Select-Object -Last 40); exit 1 }
}
$n = ([regex]::Matches($out, 'test result: ok')).Count
"GATE OK ($n test binaries)"
