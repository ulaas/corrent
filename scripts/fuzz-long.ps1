<#
.SYNOPSIS
    STABILITY.md S9: the long fuzz and property-test run (the gate runs short versions).

.DESCRIPTION
    1. every fuzz target (corrent-core's fuzz::tests::z*, corrent-server's z4) for -Minutes,
       all at once (one test thread each), in an optimized build that keeps overflow checks
    2. the 8 MiB feed timing test (ignored in the gate)
    3. the property tests (prop_tests, and the P8 tests of rss and the theme) with -Cases cases
    A failing fuzz input is saved to target/fuzz-failures/ (copy it into tests/fuzz-corpus/, see
    its README); proptest writes a failing case to proptest-regressions/. Prints a table of
    cases per target for STABILITY.md's Results.

.PARAMETER Minutes
    Minutes per fuzz target (default 30).

.PARAMETER Cases
    Cases per property test (default 1000000).
#>
[CmdletBinding()]
param([int]$Minutes = 30, [int]$Cases = 1000000)

# cargo writes progress to stderr: exit codes decide, not PowerShell's error stream.
$ErrorActionPreference = 'Continue'
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
# Optimized, with debug assertions and overflow checks (a release build drops them, and some
# tests use debug-only hooks); its own target folder so the normal debug build stays as it is.
$env:CARGO_PROFILE_DEV_OPT_LEVEL = '3'
$target = @('--target-dir', 'target/fuzz')
$watch = [Diagnostics.Stopwatch]::StartNew()
try {
    Write-Host "==> building"
    cargo test @target -p corrent-core -p corrent-server -p corrent --no-run 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "build failed" }

    Write-Host "==> fuzz targets, $Minutes min each"
    $env:CORRENT_FUZZ_SECS = [string]($Minutes * 60)
    $threads = @('--test-threads', '16', '--nocapture')
    $core = cargo test @target -p corrent-core --lib fuzz::tests::z -- @threads 2>&1
    $coreOk = $LASTEXITCODE -eq 0
    $server = cargo test @target -p corrent-server --lib webapi::fuzz::tests::z4 -- @threads 2>&1
    $serverOk = $LASTEXITCODE -eq 0
    Remove-Item Env:CORRENT_FUZZ_SECS
    $rows = foreach ($l in ($core + $server | Select-String '^FUZZ (\S+): (\d+) cases in (\d+) s \((\d+)/s\)')) {
        $g = $l.Matches[0].Groups
        [pscustomobject]@{ Target = $g[1].Value; Cases = [long]$g[2].Value; Seconds = $g[3].Value; PerSecond = $g[4].Value }
    }
    $rows | Format-Table -AutoSize | Out-String | Write-Host
    if (-not ($coreOk -and $serverOk)) {
        $core + $server | Select-String 'panicked|saved to|FAILED' | Select-Object -First 40 | Write-Host
        throw "a fuzz target failed"
    }

    Write-Host "==> the 8 MiB feed"
    $o = cargo test @target -p corrent-core --lib z5_an_8_mib -- --ignored 2>&1
    if ($LASTEXITCODE -ne 0) { $o | Select-Object -Last 20 | Write-Host; throw "the 8 MiB feed was too slow" }

    Write-Host "==> property tests, $Cases cases each"
    $env:CORRENT_FUZZ_ITERS = [string]$Cases
    $o = cargo test @target -p corrent-core --lib -- prop_tests rss::fuzz_tests 2>&1
    $ok = $LASTEXITCODE -eq 0
    $o2 = cargo test @target -p corrent --bin corrent -- fuzz_tests 2>&1
    $ok = $ok -and $LASTEXITCODE -eq 0
    Remove-Item Env:CORRENT_FUZZ_ITERS
    $lines = $o + $o2 | Select-String '^test (\S+) \.\.\. (\w+)'
    $lines | ForEach-Object { [pscustomobject]@{ Test = $_.Matches[0].Groups[1].Value; Result = $_.Matches[0].Groups[2].Value } } |
        Format-Table -AutoSize | Out-String | Write-Host
    if (-not $ok) { $o + $o2 | Select-String 'panicked|minimal failing|FAILED' | Select-Object -First 40 | Write-Host; throw "a property test failed" }
    Write-Host ("==> all green in {0:n0} min" -f $watch.Elapsed.TotalMinutes)
} finally {
    Pop-Location
}
