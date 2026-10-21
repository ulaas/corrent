# STABILITY.md O2: resolves a crash report's frames (corrent.exe+0x…) to functions and lines.
# Needs the exact build that wrote the report, with its .pdb (release.yml keeps both as the
# symbols-windows artifact of each release). Both are copied to a scratch folder, so a running
# Corrent isn't touched; the report's image line must match the exe or nothing is resolved.
#   powershell -File scripts/symbolize.ps1 -Report crash-123.txt -Exe symbols\corrent.exe [-Pdb symbols\corrent.pdb] [-Out resolved.txt]
param(
    [Parameter(Mandatory)][string]$Report,
    [Parameter(Mandatory)][string]$Exe,
    [string]$Pdb = [IO.Path]::ChangeExtension($Exe, '.pdb'),
    [string]$Out
)
$ErrorActionPreference = 'Stop'
foreach ($f in $Report, $Exe, $Pdb) { if (-not (Test-Path $f)) { throw "not found: $f" } }
$scratch = Join-Path ([IO.Path]::GetTempPath()) ("corrent-symbolize-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $scratch | Out-Null
try {
    Copy-Item $Exe, $Pdb $scratch
    $copy = Join-Path $scratch (Split-Path $Exe -Leaf)
    $stdout = Join-Path $scratch 'out.txt'
    $stderr = Join-Path $scratch 'err.txt'
    # corrent.exe is a GUI program: Start-Process waits for it and catches its output.
    $p = Start-Process -FilePath $copy -ArgumentList '--symbolize', ('"' + (Resolve-Path $Report) + '"') `
        -Wait -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $text = Get-Content -Raw $stdout
    if ($p.ExitCode -ne 0) { throw ("symbolize failed: " + (Get-Content -Raw $stderr)) }
    if ($Out) { Set-Content -NoNewline $Out $text } else { $text }
} finally {
    Remove-Item -Recurse -Force $scratch
}
