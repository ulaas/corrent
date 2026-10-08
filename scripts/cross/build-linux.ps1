# Cross-build the Linux x86-64 binary from Windows using Zig as the C compiler and linker.
#
# One-time setup (no admin; installs Zig into a local virtualenv):
#   python -m venv .zigvenv
#   .zigvenv\Scripts\python -m pip install ziglang
#   rustup target add x86_64-unknown-linux-gnu
#
# Usage:  powershell -File scripts/cross/build-linux.ps1 [-Release] [-CheckOnly] [-Clippy]
#         -Clippy runs clippy on all targets with warnings denied, like the Linux CI job.
# Output: target/linux-check/x86_64-unknown-linux-gnu/<profile>/corrent
param([switch]$Release, [switch]$CheckOnly, [switch]$Clippy, [string]$Python = "")

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$py = if ($Python) { $Python } else { Join-Path $root ".zigvenv\Scripts\python.exe" }
if (-not (Test-Path $py)) { throw "Missing $py - see the setup steps at the top of this script." }
$shim = Join-Path $PSScriptRoot "zshim.py"
$wrappers = Join-Path $root "target\zig-wrappers"
New-Item -ItemType Directory -Force $wrappers | Out-Null
foreach ($w in @(@("zcc", "cc"), @("zcxx", "c++"), @("zar", "ar"))) {
    Set-Content -Encoding ascii (Join-Path $wrappers "$($w[0]).bat") "@`"$py`" `"$shim`" $($w[1]) %*"
}
$env:CC_x86_64_unknown_linux_gnu = Join-Path $wrappers "zcc.bat"
$env:CXX_x86_64_unknown_linux_gnu = Join-Path $wrappers "zcxx.bat"
$env:AR_x86_64_unknown_linux_gnu = Join-Path $wrappers "zar.bat"
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER = Join-Path $wrappers "zcc.bat"
$env:CARGO_TARGET_DIR = Join-Path $root "target\linux-check"

$cmd = if ($Clippy) { "clippy" } elseif ($CheckOnly) { "check" } else { "build" }
$cargoArgs = @($cmd, "--target", "x86_64-unknown-linux-gnu")
if ($Release) { $cargoArgs += "--release" }
if ($Clippy) { $cargoArgs += @("--all-targets", "--", "-D", "warnings") }
Push-Location $root
try { cargo @cargoArgs; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE } } finally { Pop-Location }
