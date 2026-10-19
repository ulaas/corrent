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
| Server | `GET server/info` (public), `GET/PUT server/settings` (secrets masked), `GET server/audit`, `GET server/log`, `GET server/fs?path=`, `POST server/fs/mkdir`, `POST server/alt-speed` |
| Torrents | `GET/POST torrents` (JSON or multipart), `GET/PATCH torrents/{hash}`, `GET torrents/{hash}/detail`, `POST torrents/{hash}/files`, `POST torrents/{hash}/trackers`, `POST torrents/actions`, `POST torrents/move`, `GET/PUT torrents/categories`, `PUT torrents/tags` |
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

## systemd

```ini
[Unit]
Description=Corrent server
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStart=/usr/bin/correntd --data-dir /var/lib/corrent
User=corrent
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`correntd` reports readiness and shutdown through `sd_notify`; SIGTERM shuts it down gracefully.
