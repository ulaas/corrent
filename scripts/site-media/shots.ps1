# Media screenshots for the website and README (site/img/media-*.webp). Runs a copy of the debug build
# in a scratch folder with its own portable profile, adds the open-movie library made by library.py
# through CORRENT_DEV_LIBRARY, stages a little watch history (stage.py) and captures the app's own frame
# with the debug hooks (no input is sent to the desktop). Then converts with scripts/site-images.py.
#   cargo build -p corrent -p corrent-mpv --bins
#   python -I scripts/site-media/library.py <scratch>\media
#   powershell -File scripts/site-media/shots.ps1 -Scratch <scratch>
param(
    [Parameter(Mandatory)] [string] $Scratch,
    [string] $Out = "site/img"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$media = Join-Path $Scratch "media"
if (-not (Test-Path (Join-Path $media "Movies"))) { throw "no library in ${media}: run library.py first" }
$app = Join-Path $Scratch "app"
$shots = Join-Path $Scratch "shots"
New-Item -ItemType Directory -Force $app, $shots | Out-Null
foreach ($f in @("corrent.exe", "corrent-mpvw.exe")) { Copy-Item (Join-Path $root "target\debug\$f") $app -Force }
Get-ChildItem (Join-Path $root "target\debug") -Filter *.dll | Copy-Item -Destination $app -Force
$data = Join-Path $app "corrent-data"
Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
$dl = (Join-Path $Scratch "dl") -replace '\\', '/'

# UTF-8 without a byte-order mark (Windows PowerShell's utf8 adds one).
function Write-Plain([string] $path) { process { [IO.File]::WriteAllText($path, $_) } }

function Shot([string] $name, [hashtable] $envs, [double] $delay = 25, [string] $theme = "Dark", [string] $accent = "Blue", [string[]] $extra = @()) {
    New-Item -ItemType Directory -Force $data | Out-Null
    @"
schema_version = 1

[downloads]
save_path = "$dl"

[connection]
listen_address = "127.0.0.1"
random_port = true
upnp = false

[bittorrent]
dht = false
lsd = false
trackers_enabled = false

[behavior]
handler_prompt_dismissed = true
find_servers = false

[appearance]
theme = "$theme"
accent = "$accent"
language = "en"
zoom = 1.25
"@ | Write-Plain (Join-Path $data "settings.toml")
    # 1536x960 at 1.25x, like scripts/site-shots.ps1.
    '{"window": "(inner_position_pixels:None,outer_position_pixels:None,fullscreen:false,maximized:false,inner_size_points:Some((x:1920.0,y:1200.0)))"}' |
        Write-Plain (Join-Path $data "window.ron")
    $png = Join-Path $shots "$name.png"
    Remove-Item $png -ErrorAction SilentlyContinue
    $all = @{
        CORRENT_DEV_LIBRARY = "$media\Movies;movies;Movies|$media\Shows;shows;Shows"
        CORRENT_DEV_SHOT = $png; CORRENT_DEV_SHOT_DELAY = "$delay"; CORRENT_DEV_EXIT = "1"
    }
    foreach ($k in $envs.Keys) { $all[$k] = $envs[$k] }
    foreach ($k in $all.Keys) { Set-Item "env:$k" $all[$k] }
    try {
        $p = Start-Process -FilePath (Join-Path $app "corrent.exe") -ArgumentList (@("--portable") + $extra) -PassThru
        if (-not $p.WaitForExit(240000)) { $p.Kill(); throw "$name timed out" }
    } finally {
        foreach ($k in $all.Keys) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
    }
    "{0}: {1}" -f $name, (Test-Path $png)
}

# First run: add the libraries and let the scan, artwork and frame thumbnails finish.
Shot "scan" @{ CORRENT_DEV_VIEW = "media" } 90
python -I (Join-Path $PSScriptRoot "stage.py") (Join-Path $data "corrent.db")
if ($LASTEXITCODE) { throw "stage.py failed" }
$ids = python -I -c "import sqlite3,sys,json; print(json.dumps(dict(sqlite3.connect(sys.argv[1]).execute('SELECT title, id FROM items'))))" (Join-Path $data "corrent.db") | ConvertFrom-Json
# The player first: playing saves Big Buck Bunny's position, and stage.py puts it back behind Sintel.
Shot "media-player" @{ CORRENT_DEV_PLAY = "$($ids.'Big Buck Bunny')"; CORRENT_DEV_PAUSE = "75" } 82
python -I (Join-Path $PSScriptRoot "stage.py") (Join-Path $data "corrent.db") | Out-Null
Shot "media-home" @{ CORRENT_DEV_VIEW = "media" }
Shot "media-home-light" @{ CORRENT_DEV_VIEW = "media" } 25 "Light" "Violet"
Shot "media-item" @{ CORRENT_DEV_ITEM = "$($ids.Sintel)" }
Shot "media-show" @{ CORRENT_DEV_ITEM = "$($ids.Caminandes)" }
Shot "media-library" @{ CORRENT_DEV_PAGE = "library:1" }
Shot "media-tv" @{ CORRENT_DEV_VIEW = "media" } 25 "Dark" "Blue" @("--tv")
python -I (Join-Path $root "scripts\site-images.py") $shots (Join-Path $root $Out)
Remove-Item -Recurse -Force $app
