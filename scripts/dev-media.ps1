<#
.SYNOPSIS
    Make a small scratch media library (synthetic clips, a poster) for UI work and screenshots.

.DESCRIPTION
    Uses target\debug\corrent-mpvw.exe (libmpv's encoder, lavfi test sources) to write:
      <Out>\Movies\Big Buck Bunny (2008)\Big Buck Bunny (2008).mkv  + poster.jpg
      <Out>\Movies\Sintel.2010.1080p.BluRay.x264-GRP\Sintel.2010.1080p.BluRay.x264-GRP.mkv  (no art)
      <Out>\Movies\Elephants Dream (2006)\Elephants Dream (2006).mkv
      <Out>\Shows\Theme Show\Season 01\Theme Show - S01E01..E03.mkv
      <Out>\Videos\clip1.mkv, clip2.mkv
    Existing files are kept. Then, for example:
      $env:CORRENT_DEV_LIBRARY = "<Out>\Movies;movies|<Out>\Shows;shows|<Out>\Videos;videos"
      $env:CORRENT_DEV_VIEW = 'media'
      target\debug\corrent.exe --portable

.PARAMETER Out
    Scratch folder (created). Never your real media folders.

.PARAMETER Seconds
    Length of each clip (default 20).
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Out,
    [int]$Seconds = 20
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$worker = Join-Path $repo 'target\debug\corrent-mpvw.exe'
if (-not (Test-Path -LiteralPath $worker)) { throw "no ${worker}: run cargo build first" }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path

function Encode($src, $output, $options, $audio) {
    if (Test-Path -LiteralPath $output) { return }
    New-Item -ItemType Directory -Force -Path (Split-Path $output) | Out-Null
    $job = @{ input = $src; output = $output; options = $options; audio_file = $audio }
    $file = Join-Path ([IO.Path]::GetTempPath()) "corrent-dev-media-$PID.json"
    # No BOM: the worker reads plain JSON.
    [IO.File]::WriteAllText($file, ($job | ConvertTo-Json -Depth 4), (New-Object Text.UTF8Encoding $false))
    try {
        $p = Start-Process -FilePath $worker -ArgumentList 'encode', "`"$file`"" -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput "$file.out"
        if ($p.ExitCode -ne 0) { throw "encode failed for $output (exit $($p.ExitCode))" }
    } finally {
        Remove-Item -LiteralPath $file, "$file.out" -ErrorAction SilentlyContinue
    }
    Write-Host "  $output"
}

$video = @(@('of', 'matroska'), @('ovc', 'libx264'), @('ovcopts', 'preset=ultrafast'), @('oac', 'aac'))
function Clip($source, $path, $freq) {
    Encode "av://lavfi:$source=duration=$($Seconds):size=640x360:rate=25,format=yuv420p" $path $video `
        "av://lavfi:sine=frequency=$($freq):duration=$Seconds"
}

Write-Host "==> clips in $Out"
Clip 'testsrc2' (Join-Path $Out 'Movies\Big Buck Bunny (2008)\Big Buck Bunny (2008).mkv') 440
Clip 'smptehdbars' (Join-Path $Out 'Movies\Sintel.2010.1080p.BluRay.x264-GRP\Sintel.2010.1080p.BluRay.x264-GRP.mkv') 330
Clip 'rgbtestsrc' (Join-Path $Out 'Movies\Elephants Dream (2006)\Elephants Dream (2006).mkv') 550
foreach ($n in 1..3) {
    Clip 'testsrc' (Join-Path $Out "Shows\Theme Show\Season 01\Theme Show - S01E0$n.mkv") (200 + 50 * $n)
}
Clip 'yuvtestsrc' (Join-Path $Out 'Videos\clip1.mkv') 660
Clip 'pal100bars' (Join-Path $Out 'Videos\clip2.mkv') 770

# One poster, so a card with art sits next to ones without.
Encode 'av://lavfi:gradients=size=600x900:duration=1' (Join-Path $Out 'Movies\Big Buck Bunny (2008)\poster.jpg') `
    @(@('of', 'image2'), @('ovc', 'mjpeg'), @('frames', '1')) $null
Write-Host "==> done"
