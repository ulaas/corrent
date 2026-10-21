<#
.SYNOPSIS
    STABILITY.md S8: runs the fault matrix (corrent-core's fm_* tests) and repeats the
    timing-sensitive ones to catch flakes.

.DESCRIPTION
    1. every fm_* test once (storage, profile, network, HTTP, wake-up faults; the gate runs them too)
    2. the network tests (fm_net_*) and the wake-up test -Runs times in a row (default 20);
       stops at the first failure and prints its output
    Prints a summary table to paste into STABILITY.md's Results.

.PARAMETER Runs
    How many times the timing-sensitive tests run in a row (default 20).
#>
[CmdletBinding()]
param([int]$Runs = 20)

# cargo writes progress to stderr: exit codes decide, not PowerShell's error stream.
$ErrorActionPreference = 'Continue'
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    Write-Host "==> building"
    cargo test -p corrent-core --lib --no-run 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "build failed" }

    Write-Host "==> every fault test once"
    $out = cargo test -p corrent-core --lib fm_ -- --include-ignored 2>&1
    $lines = $out | Select-String '^test engine::fault_tests::(fm_\S+) \.\.\. (\w+)'
    $rows = foreach ($l in $lines) { [pscustomobject]@{ Test = $l.Matches[0].Groups[1].Value; Result = $l.Matches[0].Groups[2].Value } }
    $rows | Format-Table -AutoSize | Out-String | Write-Host
    if ($LASTEXITCODE -ne 0) { $out | Select-Object -Last 40 | Write-Host; throw "a fault test failed" }

    Write-Host "==> timing-sensitive tests, $Runs runs"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    for ($i = 1; $i -le $Runs; $i++) {
        $o = cargo test -p corrent-core --lib -- fm_net_ fm_wake fm_slow 2>&1
        if ($LASTEXITCODE -ne 0) {
            $o | Select-Object -Last 60 | Write-Host
            throw "run $i of $Runs failed"
        }
        Write-Host "  run $i ok"
    }
    Write-Host ("==> {0} runs green in {1:n0} s" -f $Runs, $watch.Elapsed.TotalSeconds)
    Write-Host ""
    Write-Host "| Test | Result |"
    Write-Host "|---|---|"
    foreach ($r in $rows) { Write-Host "| $($r.Test) | $($r.Result) |" }
} finally {
    Pop-Location
}
