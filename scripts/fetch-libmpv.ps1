# Fetch libmpv for local Windows builds: downloads a pinned mpv dev build (shinchiro's
# mpv-winbuild-cmake, x86_64), checks its SHA-256, and keeps only libmpv-2.dll in
# .cache\libmpv\ (ignored by git). build.rs copies it next to the executables in
# target\<profile>\ on every build; this script also copies it into the target folders that
# already exist, so no rebuild is needed.
#
#   powershell -File scripts/fetch-libmpv.ps1            # the pinned build
#   powershell -File scripts/fetch-libmpv.ps1 -Latest    # the newest release (GitHub's digest)
#
# libmpv is not bundled with release packages (docs/PACKAGING.md); this is for development and
# the owner's own builds. The project prunes old releases: when the pinned one is gone, use
# -Latest and update $Tag/$Sha256 below.

param([switch]$Latest, [switch]$Force)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$repo = "shinchiro/mpv-winbuild-cmake"
$Tag = "20261019"
$Asset = "mpv-dev-x86_64-20261019-git-6a262d11f6.7z"
$Sha256 = "ca8604754170db6f1b23139d8449782b1ae294a883211131954d30085985f77b"

$cache = Join-Path $root ".cache\libmpv"
$dll = Join-Path $cache "libmpv-2.dll"
$versionFile = Join-Path $cache "VERSION"

if ($Latest) {
    $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers @{ "User-Agent" = "corrent-fetch-libmpv" }
    $a = $rel.assets | Where-Object { $_.name -like "mpv-dev-x86_64-2*.7z" } | Select-Object -First 1
    if (-not $a) { throw "no mpv-dev-x86_64 archive in release $($rel.tag_name)" }
    if (-not $a.digest -or -not $a.digest.StartsWith("sha256:")) { throw "release $($rel.tag_name) has no SHA-256 digest" }
    $Tag = $rel.tag_name
    $Asset = $a.name
    $Sha256 = $a.digest.Substring(7)
}

if ((Test-Path $dll) -and (Test-Path $versionFile) -and -not $Force -and ((Get-Content $versionFile -Raw).Trim() -eq $Asset)) {
    Write-Host "libmpv $Asset is already in $cache"
} else {
    # A new, empty folder for the download: it is untrusted until its hash matches.
    $work = Join-Path $cache ("download-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force $work | Out-Null
    try {
        $archive = Join-Path $work $Asset
        $url = "https://github.com/$repo/releases/download/$Tag/$Asset"
        Write-Host "downloading $url"
        $ProgressPreference = "SilentlyContinue"
        Invoke-WebRequest $url -OutFile $archive -UseBasicParsing
        # .NET directly: Get-FileHash is missing from some Windows PowerShell setups.
        $stream = [System.IO.File]::OpenRead($archive)
        try { $bytes = [System.Security.Cryptography.SHA256]::Create().ComputeHash($stream) } finally { $stream.Dispose() }
        $hash = -join ($bytes | ForEach-Object { $_.ToString("x2") })
        if ($hash -ne $Sha256.ToLower()) { throw "SHA-256 mismatch for ${Asset}: got $hash, expected $Sha256" }
        $out = Join-Path $work "out"
        New-Item -ItemType Directory -Force $out | Out-Null
        $sevenZip = @("$env:ProgramFiles\7-Zip\7z.exe", "${env:ProgramFiles(x86)}\7-Zip\7z.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($sevenZip) {
            & $sevenZip e $archive "-o$out" "libmpv-2.dll" -y | Out-Null
        } else {
            # Windows' own tar (libarchive) reads 7z too.
            & "$env:SystemRoot\System32\tar.exe" -xf $archive -C $out "libmpv-2.dll"
        }
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path (Join-Path $out "libmpv-2.dll"))) { throw "libmpv-2.dll not found in $Asset" }
        Copy-Item (Join-Path $out "libmpv-2.dll") $dll -Force
        Set-Content $versionFile $Asset
        Write-Host "libmpv-2.dll from $Asset is in $cache"
    } finally {
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }
}

# The builds that exist already get it right away (build.rs does it for later builds).
foreach ($p in @("debug", "release")) {
    $dir = Join-Path $root "target\$p"
    if (Test-Path $dir) {
        Copy-Item $dll (Join-Path $dir "libmpv-2.dll") -Force
        Write-Host "copied to target\$p"
    }
}
