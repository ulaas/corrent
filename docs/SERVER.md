# Corrent server (`correntd`)

`correntd` is Corrent's headless server: the torrent engine, API v1 with realtime events, the
qBittorrent-compatible API (Sonarr/Radarr) and the web interface, served over HTTPS with the
server's own certificate. The media-server roadmap is `docs/MEDIASERVER.md`.

## Running

```sh
correntd                         # profile in the OS data folder ("Corrent Server")
correntd --portable              # profile in correntd-data/ next to the executable
correntd --data-dir /srv/corrent # any folder
correntd --help
```

The first start creates `server.toml`, `corrent.db` and `tls/` in the profile. The desktop app and
the server never share a profile (two engines must not share one session); a second `correntd`
on the same profile refuses to start.

Create the administrator either on the server itself (open the HTTPS address shown at startup —
setup only works from the server's own computer) or with
`CORRENT_ADMIN_PASSWORD=… correntd --init-admin <name>` (without the variable the password is
read from standard input). An existing qBittorrent-API account of the profile becomes the first
administrator automatically.

Chores from a terminal (also next to a running server; add `--data-dir`/`--portable` as for the
server): `correntd admin status` (users, libraries, last backup, a staged restore), `admin users`,
`admin reset-password <name>` (password like `--init-admin`), `admin enable|disable <name>`, and
`admin scan` (asks the running server to scan every library, with a device token it deletes
afterwards).

## Listening and access

| Setting (`server.toml`) | Default | Meaning |
|---|---|---|
| `bind`, `https_port` | `0.0.0.0`, `8443` | HTTPS listener (`--bind`, `--https-port`) |
| `http_port` | `8088` | plain HTTP on 127.0.0.1 only; `0` turns it off (`--http-port`) |
| `tls` | `true` | `--no-tls` serves plain HTTP on the main port (testing only) |
| `cert_path`, `key_path` | none | your own certificate (e.g. from ACME) instead of the built-in CA |
| `remote_access` | `false` | off: only loopback, private, link-local, CGNAT/Tailscale (100.64.0.0/10) and `allowed_networks` clients get in |
| `allowed_networks` | `[]` | extra CIDRs treated as local |
| `trusted_proxies` | `[]` | reverse proxies whose `X-Forwarded-For` is believed |
| `hostnames` | `[]` | host names browsers may use besides IP addresses, `localhost`, `<computer>` and `<computer>.local` |
| `qbt_api` | `true` | serve the qBittorrent-compatible API at `/api/v2` and the classic web UI at `/` |
| `download_roots` | `[]` | extra folders non-admin users may download to and browse |

### What non-admin users can reach

Users without the `settings.write` permission are confined to the download folder, the category
folders, the library folders and `download_roots`: save paths, moves, the folder browser and
"new folder" all resolve the path (symlinks and `..` can't escape) and refuse anything outside.
Adding a torrent by URL only fetches from public addresses for them (no loopback, LAN,
link-local or cloud-metadata addresses). Other users' torrents are listed without their tracker
URLs (private trackers embed the account's passkey), folders and comments. Accounts with the
`downloader` permission (for Sonarr/Radarr through the qBittorrent API) are trusted with any
save path, as in qBittorrent.

### Certificates and fingerprints

The server makes a private certificate authority once and signs a server certificate for the
computer's names and addresses (re-issued when they change, and before it is 700 days old).
Native clients pin the CA's public key: its SHA-256 is the **fingerprint** printed at startup,
shown in the clients, embedded in pairing links, and printed by `correntd --print-fingerprint`.
Browsers show a warning for the self-signed certificate; to avoid it, install `tls/ca.pem` as a
trusted authority on your devices (compare the fingerprint first).

## Accounts, devices and pairing

Users have a role (`admin`, `user`, `guest`, `kids`) with default permissions that an admin can
override. Passwords are argon2id; changing your own password needs the current one. Browsers get a 12-hour sliding session cookie (HttpOnly,
SameSite=Strict) and must echo a CSRF token on changes; apps get a 90-day sliding device token.
Ten wrong passwords lock an account for 15 minutes; ten failures from one address block it for
15 minutes; a global limit slows floods. Every sign-in, account change, settings change and
removal with data is in the audit log (`GET /api/v1/server/audit`).

A signed-in user creates a 6-digit pairing code (`POST /api/v1/auth/pair/code`, valid 10 minutes,
single use; an address that guesses wrong five times can't pair for 15 minutes, and fifty wrong
guesses in total burn every outstanding code); a new device redeems it (`POST /api/v1/auth/pair`)
and receives its own token. Devices are listed and revoked at `/api/v1/devices`.

## API v1 in brief

All JSON; errors are `{ "code": "...", "message": "...", "params": {...} }` with a stable code.

| Area | Endpoints |
|---|---|
| Auth | `POST auth/login`, `auth/logout`, `GET auth/me`, `POST auth/setup`, `auth/pair`, `auth/pair/code` |
| Users, devices | `GET/POST users`, `GET/PATCH/DELETE users/{id}`, `GET devices`, `DELETE devices/{id}` |
| Server | `GET server/info` (public), `GET server/ca` (public: the CA clients pin), `GET/PUT server/settings` (secrets masked), `GET server/audit`, `GET server/log`, `GET server/fs?path=`, `POST server/fs/mkdir`, `POST server/alt-speed` |
| Torrents | `GET/POST torrents` (JSON or multipart), `GET/PATCH torrents/{hash}`, `GET torrents/{hash}/detail`, `POST torrents/{hash}/files`, `POST torrents/{hash}/trackers`, `POST torrents/preview` (magnet/URL/.torrent → name and files), `POST torrents/actions`, `POST torrents/queue` (drag and drop), `POST torrents/move`, `GET/PUT torrents/categories`, `PUT torrents/tags` |
| RSS, search | `GET rss`, `GET/PUT rss/config`, `POST rss/refresh`, `POST rss/download`, `GET search?q=` |
| Library | `GET/POST libraries`, `GET/PATCH/DELETE libraries/{id}`, `GET/POST/DELETE libraries/{id}/scan`, `GET items?library_id=&parent_id=&kind=&genre=&person_id=&favorite=&played=&sort=title\|added\|year\|played\|index&desc=&offset=&limit=`, `GET/PATCH items/{id}`, `PUT items/{id}/state`, `GET items/{id}/file` and `GET files/{id}` (Range), `GET images/{sha}?w=`, `GET home`, `GET library/search?q=`, `GET/PUT users/{id}/libraries` |
| Playback | `POST playback/{item}` `{profile: "web"\|"safari"\|"native", audio_track?, burn_subtitle?, max_bitrate?, max_height?, force_transcode?, file_id?}` → `{method: "direct"\|"hls", url, session_id?, subtitles[{url?}], trickplay?, reasons}`, `GET hls/{session}/index.m3u8`, `GET hls/{session}/{n}.ts`, `GET/DELETE playback/sessions[/{id}]`, `GET files/{id}/subtitles/{idx}.vtt`, `GET files/{id}/trickplay/index.vtt` |
| Wanted | `GET/POST wanted`, `DELETE wanted/{id}`, `POST wanted/{id}/search` → judged releases, `POST wanted/{id}/grab {link}`, `GET/POST quality-profiles`, `PUT/DELETE quality-profiles/{id}`, `GET items/{id}/releases`, `POST items/{id}/grab {link}`, `GET torrents/{hash}/files/{i}/stream` (Range; waits for pieces) |
| Plugins, metadata | `GET/POST plugins` (install: multipart `manifest` + `wasm`), `DELETE plugins/{id}`, `PUT plugins/{id}/settings`, `PUT plugins/{id}/enabled`, `GET/POST items/{id}/identify`, `POST items/{id}/refresh-metadata`, `POST libraries/{id}/refresh-metadata` |
| Events | `GET events` (WebSocket; see below) |

**Events:** send `{"t":"subscribe","topics":[{"topic":"torrents"},{"topic":"session"}]}`. The
torrent list arrives whole once (`torrents_full`) and then as `torrents_delta` (`upserted`,
`patched`, `removed`), at most every `?rate_ms=` (default 1000). `{"topic":"torrent","arg":"<hash>"}`
streams one torrent's details every second; `library` sends `library_changed` and `scan_status`;
admins can subscribe to `log`. Messages over 32 KiB
are zlib-compressed binary frames (`DecompressionStream("deflate")` in browsers).

## Libraries

A library has a kind (`movies`, `shows`, `music`, `audiobooks`, `videos`, `photos`) and folders.
Scans read file names and folders, `.nfo` files, audio tags, local artwork (`poster.jpg`,
`fanart.jpg`, `cover.jpg`, `seasonNN-poster.jpg`, embedded covers) and external subtitles, and
probe media with libmpv through the `corrent-mpvw` worker (install it next to `correntd`; without
it libmpv is used in-process, and without libmpv files are added unprobed). Folders are watched
for changes and every library is rescanned every six hours. Files that disappear are marked
missing and removed after 30 days; a folder that can't be read at all (unplugged drive, offline
share) leaves the library untouched. Admins can limit which libraries a user sees.

**Podcasts**: a library of the kind *Podcasts* holds a folder per podcast with dated episode
files (`2026-10-01 Title.mp3`). Settings → Podcasts subscribes to feeds; the `podcasts` task
(hourly) downloads new episodes into the library's first folder and keeps the newest N (0 = all;
only files it named are removed). API: `GET/POST /api/v1/podcasts`, `DELETE /podcasts/{id}`,
`POST /podcasts/{id}/refresh`.

**Radio**: administrators add internet radio stations (name and stream or `.pls`/`.m3u`
address) on the web client's Radio page; everyone can play them there (relayed by the server at
`/api/v1/radio/{id}/stream`, so HTTPS pages can play HTTP streams) or in the desktop app (Radio
in the media top bar; mpv plays the address itself).

**Stop seeding once everyone has watched** (library setting, for libraries fed by torrent
categories): the hourly `stop_watched_seeding` task pauses a finished torrent of those categories
once every enabled account that can see the library has played all playable items whose files
came from it.

**`.nfo` files**: movie and show libraries can write Kodi `.nfo` files (library setting) after
identification and edits — `<movie file>.nfo`, `tvshow.nfo`, `<episode file>.nfo`. Files Corrent
didn't write (no `<!-- written by Corrent -->` line) are never replaced.

Lyrics come from a `.lrc` (synced) or `.txt` file named like the track, else the track's lyrics
tag (`GET /api/v1/items/{id}/lyrics`); the web player shows them in its queue panel (synced lines
follow the music, click one to jump), the desktop player behind the microphone button.

## Torrents and libraries together

A library lists the torrent categories that feed it (`settings.import_categories`). Those
categories download into the library's first folder (the server sets their folder when they
have none), and when a download completes the library imports it: **in place** (default — the
file keeps seeding where it is), or **hard-linked**/**copied** into the library's first folder.
Libraries can set seeding goals (`seed_ratio`, `seed_minutes`) for the torrents that feed them.
Removing a torrent *with its data* is refused while library files are inside it (send
`"force": true` to confirm).

From any library item, `releases` searches the Torznab indexers and ranks what it finds against
the library's quality profile (resolution, source, codec, words, size, seeders); `grab` adds
the chosen one so it imports into that library. The **wanted list** remembers movies, episodes
and seasons you want; every 15 minutes the server searches entries not searched in the last six
hours and, when the profile allows auto-grab, adds the best acceptable release. A finished
download marks its wanted entry done. Files that are still downloading play right away: the
player (or the transcoder) reads them through the engine, which fetches the needed pieces first.

## Plugins and metadata

Plugins are WebAssembly components (`crates/corrent-plugins/wit/corrent.wit`) running in a
sandbox: no files, no sockets, HTTP only to the hosts their manifest declares (and only to
public addresses unless allowed), 64 MiB of memory and 30 seconds per call; a plugin that
crashes three times is switched off. Built in: **TMDB** (movies and shows — enter a free API key
under Plugins) and **MusicBrainz + Cover Art Archive** (albums and artists, no key). After every
scan, items without metadata are looked up; only clear matches (same title, and year) are applied
automatically — the rest can be matched with "Identify…". Provider details never overwrite
locked fields, and local artwork always wins over downloaded artwork. Admins can install other
plugins (a `plugin.toml` and the `.wasm` component); see `plugins/example` for a minimal one.

## Playback and transcoding

Clients ask `POST /api/v1/playback/{item}` how to play: files the client can play are served as
they are (Range requests); anything else becomes an HLS stream encoded by libmpv in the
`corrent-mpvw` worker (H.264 + AAC, hardware NVENC/QSV when a trial encode works, else
libx264). The playlist covers the whole file from the start; seeking far ahead restarts the
encoder at that point. External text subtitles are served as WebVTT; image subtitles (and
others on request) are burned into the picture. Seek-bar thumbnails (trickplay) are made in the
background the first time a video is played. Transcodes need the `playback.transcode`
permission; at most three run at once and idle ones stop after 90 seconds.

## Watching together

The player's **Watch together** button starts a room for what is playing and copies a link
(`/play/<item>?room=<code>`); others open it in a browser, or type the code into the desktop
player's Watch together menu. Play, pause and seek from anyone apply to everyone; positions are
computed from the server's clock, so late joiners start at the right place. Everyone needs an
account that may see the item. Rooms close when the last person leaves or after six idle hours
(at most 200 rooms, 20 people each). API: `POST /api/v1/syncplay`, `GET /syncplay/{code}`,
`POST /syncplay/{code}/join|leave|command`; WebSocket topic `room` with the code as argument. Membership is per
account: two devices of one account count once (leaving on one leaves both).

## DLNA for TVs and players

Settings → Remote access → **Share libraries with TVs and players (DLNA)** (or `dlna = true` in
`server.toml`) turns on a UPnP-AV MediaServer: smart TVs, receivers, VLC, Kodi and BubbleUPnP on
the local network find it by SSDP and browse libraries → shows → seasons → episodes, artists →
albums → tracks, folders and photos, then play the files as they are (Range requests; no
transcoding — the TV must support the format). DLNA has no sign-in, so it shows what one account
may see (`dlna_user`, empty = the first administrator; a limited account keeps libraries
private), answers local addresses only, and plays only the signed links it handed out (new key at
every start). It is plain HTTP on `dlna_port` (8201) because TVs don't trust self-signed
certificates; it doesn't run when the server listens on loopback only.

## Casting to Chromecasts

The players' **Cast to a device** button lists the Google Cast receivers (Chromecast, Google TV,
Nest Hub, Cast-enabled TVs) on the *server's* network, found by mDNS. The server talks to the
device itself (Cast v2 over TLS, the Default Media Receiver) and gives it a signed link on the
media listener (`dlna_port`, plain HTTP, local addresses only — started on demand even when DLNA
is off): the file when the device plays it (MP4/WebM, H.264/VP8/VP9, AAC/MP3/Opus/FLAC up to
1080p), otherwise an HLS transcode (needs the transcode permission). Unfinished downloads always
go through a transcode. The players then drive the device (play, pause, seek, stop) and show its
position. API: `GET /api/v1/cast/devices[?refresh=true]`, `POST /cast/{device}/play`
`{item_id, position_ms}`, `GET /cast/{device}`, `POST /cast/{device}/command`
`{action: play|pause|seek|volume|stop}`. AirPlay isn't supported.

## Jellyfin apps

With Remote access → **Let Jellyfin apps connect** (`jellyfin_api = true`), Jellyfin's own apps
work with Corrent: Jellyfin Web / Media Player, Android TV, Findroid, Swiftfin, Infuse and the
Kodi add-on sign in with a Corrent account, browse the libraries (movies, shows, music,
audiobooks, home videos, photos — podcast libraries aren't shown) and play them.

- **Address:** in the app type `http://<this computer's IP>:8096` (`jellyfin_port`; plain HTTP,
  local network only, because the apps don't accept Corrent's private certificate authority).
  Browsers that trust the server's certificate can also use `https://<server>:8443/jellyfin`.
  With **Let Jellyfin apps find this server** the apps' network search (UDP 7359) lists it.
- **What it says it is:** the apps only talk to a "Jellyfin Server" of a current version, so
  Corrent answers as one (`jellyfin_version`, default 12.2.0); its own name is in the server name
  ("Corrent (…)").
- **Accounts:** the apps' sign-in creates a device (Settings → Devices) per app installation;
  signing out or revoking it ends the session. Administrators aren't administrators in the apps
  (no Jellyfin dashboard); Corrent's own clients manage the server. Accepted credentials:
  `Authorization: MediaBrowser … Token=…`, `X-Emby-Token`, `X-MediaBrowser-Token`, `api_key` /
  `ApiKey` query (broader than Jellyfin 12's defaults); never the browser cookie.
- **Playback:** the app's device profile decides; files it plays are served as they are (Range),
  others become an HLS transcode when the app starts playing (transcode permission needed).
  External text subtitles are offered as WebVTT; embedded ones are burned in. Progress, played
  and favorite marks are the same as in Corrent's clients.
- **Artwork** loads without a token when the app asks for it by its tag (the image hash);
  `jellyfin_public_images = true` serves it by item id too, as Jellyfin does.
- **Not supported:** the admin dashboard, playlists and collections, live TV, SyncPlay through
  the apps, trickplay thumbnails, lyrics, remote control of other sessions, and progressive
  (non-HLS) transcodes. `jellyfin_cors = true` adds CORS for a Jellyfin Web hosted elsewhere.

## Installing as a service

Packages: `Corrent-Server-<v>-linux-x86_64.tar.gz` / `-darwin-*.tar.gz` (`scripts/package-server.sh`),
`Corrent-Server-<v>-windows-x86_64.zip` (`scripts/package-windows.ps1`), or the Docker image. Each
has `correntd` and `corrent-mpvw` (the libmpv worker; keep it next to `correntd`). libmpv comes
from the system: Debian/Ubuntu `libmpv2`, Fedora `mpv-libs`, macOS `brew install mpv`, Windows
`libmpv-2.dll` next to the exe.

**Linux (systemd)** — `packaging/server/linux/`:

```sh
sudo install -m755 correntd corrent-mpvw /usr/bin/
sudo cp correntd.sysusers /usr/lib/sysusers.d/correntd.conf && sudo systemd-sysusers
sudo cp correntd.service /etc/systemd/system/ && sudo systemctl enable --now correntd
sudo -u corrent correntd --data-dir /var/lib/correntd --init-admin admin
```

The unit is `Type=notify` (readiness and shutdown via `sd_notify`), hardened
(`ProtectSystem=strict`, `ProtectHome=read-only`, …) and stops gracefully on SIGTERM. Grant the
media and download folders with `ReadWritePaths=` and file permissions for the `corrent` user;
GPU encoding needs `SupplementaryGroups=render video`. `correntd.user.service` runs it as your own
user instead (`systemctl --user`, `loginctl enable-linger`).

**macOS (launchd)** — `packaging/server/macos/io.github.ulaas.correntd.plist` is a per-user agent
(it can read your media); see the comments in the file for `launchctl bootstrap`.

**Windows** — from an elevated prompt: `correntd service install` (data in
`%ProgramData%\Corrent Server`, automatic start, restarted after failures; `--data-dir`,
`--account`/`--password` to run as a user who can read the media), `correntd service uninstall`.
The service handles Stop and Shutdown like Ctrl+C.

**Docker** — `packaging/server/docker/Dockerfile` and `compose.yaml` (data in `/data`, media
mounted at `/media`; `--network host` for LAN discovery; `/dev/dri` for VAAPI).

## The desktop app as a client

The desktop app can use a server's engine instead of its own: **⋮ → Connect to a server…**,
paste the pairing link from the web interface (Settings → Devices → Pair a device) or type the
address and sign in. The app checks the server's CA against the fingerprint in the link (or shows
it for you to compare), then keeps a device token and the CA in `servers.toml` (owner-only) in
its data folder. The torrent list, details, add dialog, categories, tags, queue, RSS and media
pages then work against the server; creating torrents, importing from other clients, the
streaming window and "open folder" are hidden (the files are on the server). **⋮ → Use this
computer's engine** switches back.

### Offline copies

Connected to a remote server, the app's item pages have **Download**: the original file
(resumable) or a smaller 720p copy the server converts while it downloads (its HLS segments
joined into one file, resumable by segment); seasons and albums download every episode or track.
The Downloads page (arrow in the top bar) lists them, plays them without the server and pauses,
resumes or removes them. Copies live in `<app data>/offline/`. Items with an offline copy play
from it even while connected. Watch progress made while the server can't be reached is kept and
sent once it is back — unless the server saw that item played later.

## Remote access

`server.toml` (or `PUT /api/v1/server/remote`, admins) controls how the server is reached:

| Key | Default | What |
|---|---|---|
| `discovery` | `true` | Announce `_corrent._tcp` on the LAN (mDNS) with the id, name, version and certificate fingerprint. The desktop app lists it in Connect; the fingerprint there is a hint, pairing links pin it. |
| `connect` | `false` | **Corrent Connect** (iroh): reachable from anywhere by the server's key (`connect.key` in the data folder), through NATs, relayed when hole punching fails, end-to-end encrypted. Pairing links then carry `connect=<id>`, and the app uses Connect when the server's address can't be reached. |
| `connect_relay` | `true` | Use n0's public relays and address lookup (the server's addresses are published to `iroh.link` while Connect is on). Off: direct connections only. |
| `port_mapping` | `false` | Ask the router (UPnP / NAT-PMP / PCP) to forward the HTTPS port. Clients outside the LAN also need `remote_access = true`. |
| `remote_access` | `false` | Accept clients outside the local network on the HTTPS port. Connect clients are always admitted (turning Connect on is the opt-in). |

`GET /api/v1/server/remote` also reports what is running: the Connect id and home relay, the
mapped public address, and errors.


**Your own relay:** run iroh's relay server yourself (`cargo install iroh-relay --features
server`, then `iroh-relay --config-path relay.toml` on a host with a public name and TLS) and
put its address in Remote access → Your own relay (`connect_relay_url` in `server.toml`). The
server then uses it as its home relay; apps reach it from the address the lookup service
publishes, with no change on their side.

## Tasks, notifications, backups, metrics

**Tasks** (`GET /api/v1/tasks`, `PUT /tasks/{id}`, `POST /tasks/{id}/run`, `GET /tasks/{id}/history`;
web: Settings → Tasks and backups): `scan_libraries` (6 h), `refresh_metadata` (1 day),
`wanted_search` (15 min), `cleanup_cache` (1 day: transcode leftovers), `backup` (1 day). Each can
be turned off, given another interval or run by hand; the last 50 runs are kept.

**Notifications** (`/api/v1/notifications`, web: Settings → Notifications): ntfy topics, Discord
webhooks, generic JSON webhooks (`{event, title, body, time, server}`) or notifier plugins, for
`download_done`, `torrent_added`, `release_grabbed`, `device_paired`, `task_failed`. Texts are in
the server's language.

**Backups** (`GET/POST /api/v1/backups`, `GET /backups/{name}`, admins): zips in
`<data>/backups/` holding a consistent copy of `corrent.db` (`VACUUM INTO`), `server.toml`,
`settings.toml`, `store.json`, `rss.json`, `connect.key` and `tls/`; the newest seven are kept.
They contain private keys (owner-only files). Restore: **Restore** next to a backup (or **Upload…**
one from another server) in Settings → Tasks and backups stages it
(`POST /backups/{name}/restore`, `POST /backups/upload`, `GET/DELETE /backups/restore`); the next
start applies it before opening the database and keeps the replaced files in
`<data>/before-restore-<time>/` (a failed restore puts them back). By hand: stop the server,
unzip into the data folder, start it.

**Metrics**: `metrics = true` in `server.toml` serves Prometheus text at `/metrics` (torrents by
state, rates, totals, peers, DHT, free space, library items, transcode sessions, WebSocket
clients, users). It is subject to the same LAN guard as everything else, is never served over
Corrent Connect, and with `metrics_token = "…"` needs `Authorization: Bearer …`
(Prometheus `bearer_token_file`).
