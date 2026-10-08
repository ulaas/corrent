# Build the Windows release artifacts:
#   dist\Corrent-<version>-windows-portable.zip  (corrent.exe + "portable" marker: data next to the exe)
#   dist\Corrent-<version>-x86_64.msi            (only with -Msi; needs `cargo install cargo-wix` and WiX 3)
# Usage: powershell -File scripts\package-windows.ps1 [-Msi]
param([switch]$Msi)
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
New-Item -ItemType Directory -Force dist | Out-Null
$stage = Join-Path 'target' 'portable\Corrent'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\corrent.exe $stage
Set-Content -Path (Join-Path $stage 'portable') -Value 'Corrent keeps its data in the corrent-data folder next to corrent.exe while this file exists.'
Copy-Item wix\License.txt (Join-Path $stage 'LICENSE.txt')
$zip = "dist\Corrent-$version-windows-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "built $zip"
if ($Msi) {
    cargo wix --no-build --nocapture --output "dist\Corrent-$version-x86_64.msi"
    if ($LASTEXITCODE -ne 0) { throw 'cargo wix failed' }
    Write-Host "built dist\Corrent-$version-x86_64.msi"
}
