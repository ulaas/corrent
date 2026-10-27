<#
.SYNOPSIS
    Runs the workspace's tests on a few CPU cores, several times, to find the timing flakes that
    only show on slow machines (CI runners have 2-4 cores; a fast desktop hides them).

.DESCRIPTION
    Builds every test binary once, then runs each one -Runs times with its process pinned to the
    first -Cores cores (child processes such as mpv workers inherit the pinning). Every test that
    failed at least once is listed with how often and its first panic message; exit code 1 then.
    The usual cause is a test that waits for one thing and then asserts on another that the
    engine sets a moment later: wait for the state the assertions need, not only for its start.

.PARAMETER Runs
    How many times each test binary runs (default 3).

.PARAMETER Cores
    How many CPU cores the tests get (default 2).

.PARAMETER Package
    Only these packages (cargo -p names); default: the whole workspace.

.EXAMPLE
    powershell -File scripts/stress-tests.ps1 -Runs 5 -Package corrent-core
#>
[CmdletBinding()]
param([int]$Runs = 3, [int]$Cores = 2, [string[]]$Package = @())

# cargo writes progress to stderr: exit codes decide, not PowerShell's error stream.
$ErrorActionPreference = 'Continue'
# `-File` hands `-Package a,b` over as one string.
$Package = @($Package | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    $sel = if ($Package.Count) { $Package | ForEach-Object { '-p'; $_ } } else { @('--workspace') }
    Write-Host "==> building the test binaries"
    $json = cargo test $sel --no-run --message-format=json 2>$null
    if ($LASTEXITCODE -ne 0) {
        $json | Where-Object { $_.StartsWith('{') } | ConvertFrom-Json |
            Where-Object { $_.reason -eq 'compiler-message' -and $_.message.level -eq 'error' } |
            Select-Object -First 5 | ForEach-Object { Write-Host $_.message.rendered }
        cargo test $sel --no-run 2>&1 | Select-Object -Last 20 | ForEach-Object { Write-Host $_ }
        Write-Host "STRESS FAIL: the test build failed"
        exit 1
    }
    $bins = @($json | Where-Object { $_.StartsWith('{') } | ConvertFrom-Json |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.profile.test } |
        ForEach-Object { [pscustomobject]@{ Exe = $_.executable; Dir = Split-Path -Parent $_.manifest_path; Name = $_.target.name } })
    $cores = [math]::Min($Cores, [Environment]::ProcessorCount)
    $mask = [IntPtr]([long][math]::Pow(2, $cores) - 1)
    Write-Host ("==> {0} test binaries, {1} runs each, on {2} cores" -f $bins.Count, $Runs, $cores)

    $failed = @{}
    $tmp = Join-Path ([IO.Path]::GetTempPath()) "corrent-stress-$PID"
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $watch = [Diagnostics.Stopwatch]::StartNew()
    for ($run = 1; $run -le $Runs; $run++) {
        foreach ($b in $bins) {
            $log = Join-Path $tmp "$($b.Name).txt"
            # What `cargo test` gives a test binary.
            $env:CARGO_MANIFEST_DIR = $b.Dir
            $p = Start-Process -FilePath $b.Exe -WorkingDirectory $b.Dir -NoNewWindow -PassThru `
                -RedirectStandardOutput $log -RedirectStandardError "$log.err"
            # Windows PowerShell only keeps the exit code of a process whose handle was taken.
            $null = $p.Handle
            try { $p.ProcessorAffinity = $mask } catch { Write-Warning "$($b.Name): could not pin ($_)" }
            $p.WaitForExit()
            if ($p.ExitCode -eq 0) { continue }
            $text = Get-Content -Raw $log
            $names = [regex]::Matches($text, '(?m)^test (\S+) \.\.\. FAILED') | ForEach-Object { $_.Groups[1].Value }
            if (-not $names) { $names = @("$($b.Name) (exit $($p.ExitCode), no test reported)") }
            foreach ($n in $names) {
                $key = "$($b.Name)::$n"
                if (-not $failed.ContainsKey($key)) {
                    $m = [regex]::Match($text, "(?s)---- $([regex]::Escape($n)) stdout ----\s*(.{0,600})")
                    $failed[$key] = [pscustomobject]@{ Test = $key; Failures = 0; First = ($m.Groups[1].Value -split "`n" | Select-Object -First 6) -join "`n" }
                }
                $failed[$key].Failures++
            }
        }
        Write-Host ("  run {0}/{1}: {2} flaky so far ({3:n0} s)" -f $run, $Runs, $failed.Count, $watch.Elapsed.TotalSeconds)
    }
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    if ($failed.Count -eq 0) {
        Write-Host ("STRESS OK ({0} binaries x {1} runs on {2} cores)" -f $bins.Count, $Runs, $cores)
        exit 0
    }
    Write-Host "STRESS FAIL: tests that failed at least once"
    foreach ($f in $failed.Values | Sort-Object Failures -Descending) {
        Write-Host ("`n{0}  ({1}/{2} runs)" -f $f.Test, $f.Failures, $Runs)
        Write-Host $f.First
    }
    exit 1
} finally {
    Pop-Location
}
