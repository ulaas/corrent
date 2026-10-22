# Screenshots for the website (site/img). Runs a copy of the debug build in a scratch folder with
# its own portable profile and CORRENT_DEV_DEMO=1: made-up torrents, folders and peers, captured
# from the app's own frame (no desktop, no real paths or addresses).
#   powershell -File scripts/site-shots.ps1 -Scratch <empty folder> [-Exe target/debug/corrent.exe]
param(
    [Parameter(Mandatory)] [string] $Scratch,
    [string] $Exe = "target/debug/corrent.exe",
    [string] $Out = "site/img"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
function FromRoot([string] $p) { if ([IO.Path]::IsPathRooted($p)) { $p } else { Join-Path $root $p } }
$Exe = Resolve-Path (FromRoot $Exe)
$Out = FromRoot $Out
New-Item -ItemType Directory -Force $Scratch, $Out | Out-Null
$app = Join-Path $Scratch "app"
New-Item -ItemType Directory -Force $app | Out-Null
Copy-Item $Exe $app
Get-ChildItem (Split-Path $Exe) -Filter *.dll | Copy-Item -Destination $app
$exeCopy = Join-Path $app (Split-Path $Exe -Leaf)
$dl = (Join-Path $Scratch "dl") -replace '\\', '/'

# UTF-8 without a byte-order mark (Windows PowerShell's utf8 adds one).
function Write-Plain([string] $path) { process { [IO.File]::WriteAllText($path, $_) } }

function Shot([string] $name, [hashtable] $envs, [string] $theme = "Dark", [string] $accent = "Blue") {
    $data = Join-Path $app "corrent-data"
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
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
show_sparklines = true
zoom = 1.25
"@ | Write-Plain (Join-Path $data "settings.toml")
    # 1536x960 at 1.25x: sharp on high-density screens.
    # Columns that fit beside the name at this width.
    '{"window": "(inner_position_pixels:None,outer_position_pixels:None,fullscreen:false,maximized:false,inner_size_points:Some((x:1920.0,y:1200.0)))", "ui_prefs": "(columns:[Name,Size,Progress,Status,Peers,Down,Up,Eta],sort:Queue,sort_asc:true,show_sidebar:true,show_details:true,details_tab:General,status_filter:All,category_filter:All,tag_filter:All,tracker_filter:All,collapsed:[])"}' |
        Write-Plain (Join-Path $data "window.ron")
    $png = Join-Path $Out "$name.png"
    $all = @{ CORRENT_DEV_DEMO = "1"; CORRENT_DEV_SHOT = $png; CORRENT_DEV_SHOT_DELAY = "4"; CORRENT_DEV_EXIT = "1" }
    foreach ($k in $envs.Keys) { $all[$k] = $envs[$k] }
    foreach ($k in $all.Keys) { Set-Item "env:$k" $all[$k] }
    try {
        $p = Start-Process -FilePath $exeCopy -ArgumentList "--portable" -PassThru
        if (-not $p.WaitForExit(60000)) { $p.Kill(); throw "$name timed out" }
    } finally {
        foreach ($k in $all.Keys) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
    }
    "{0}: {1}" -f $name, (Test-Path $png)
}

Shot "list" @{ CORRENT_DEV_SELECT = "0"; CORRENT_DEV_TAB = "general" }
Shot "peers" @{ CORRENT_DEV_SELECT = "0"; CORRENT_DEV_TAB = "peers" }
Shot "pieces" @{ CORRENT_DEV_SELECT = "0"; CORRENT_DEV_TAB = "pieces" }
Shot "speed" @{ CORRENT_DEV_SELECT = "0"; CORRENT_DEV_TAB = "speed" }
Shot "files" @{ CORRENT_DEV_SELECT = "0"; CORRENT_DEV_TAB = "files" }
Shot "light" @{ CORRENT_DEV_SELECT = "2"; CORRENT_DEV_TAB = "files" } "Light" "Violet"
Shot "settings" @{ CORRENT_DEV_OPEN = "appearance" }
Shot "stats" @{ CORRENT_DEV_OPEN = "stats" }
Remove-Item -Recurse -Force $app
