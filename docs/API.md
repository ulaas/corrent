# Corrent server API (v1)

The media server's HTTP API, used by the web client, the desktop app and anything else. Base
path `/api/v1`, JSON in and out (UTF-8), served over HTTPS (or plain HTTP on loopback). The types
are the Rust structs in `crates/corrent-proto` (field names as written, snake_case); the
TypeScript mirror is `web/src/lib/types.ts`. Separately: the qBittorrent-compatible API at
`/api/v2` (for Sonarr/Radarr, see `src/webapi/`), Prometheus `/metrics` (opt-in), DLNA under
`/dlna/` on its own port, and the Jellyfin-compatible subset at `/jellyfin` (docs/SERVER.md "Jellyfin apps").

## Signing in

- **Setup:** `POST /auth/setup` creates the first administrator (only from the server's own
  computer, only while there is none).
- **Browsers:** `POST /auth/login {username, password, device?}` sets an HTTP-only
  `corrent_session` cookie and returns a `csrf` token; every non-GET request then needs
  `X-Corrent-CSRF: <csrf>`. Cookies never work across origins (DNS-rebinding guard on `Host`).
- **Apps and devices:** the same login with `device: {name, platform, client}` returns a
  long-lived bearer `token` (`Authorization: Bearer ct_…`, no CSRF); `GET/DELETE /devices`
  lists and revokes them. Pairing: an admin's `POST /auth/pair/code` makes a short code (and a
  `corrent://` link with the certificate fingerprint and Connect id); a new device redeems it with
  `POST /auth/pair`.
- **Profiles:** `GET /auth/profiles` (who can be picked on a shared screen), `POST /auth/profile
  {user_id, pin?}` switches, `PUT /auth/pin` sets your own PIN.
- **Permissions:** each account has a role (admin, user, guest, kids) and permission bits
  (`Perms` in `corrent-proto/src/auth.rs`); endpoints check the bit they need and answer 403.
- **Network guard:** clients outside the local network are refused (403 `remote_access_disabled`)
  unless remote access is on or they come through Corrent Connect.

## Errors

Any failure is `{"code": "...", "message": "...", "params": {...}}` with the HTTP status of the
code. Clients translate by `code` (and `params`), never by `message` (English, for logs).

| code | status |
|---|---|
| `bad_request` | 400 |
| `unauthorized`, `invalid_credentials`, `pairing_code_invalid` | 401 |
| `forbidden`, `remote_access_disabled` | 403 |
| `not_found` | 404 |
| `conflict` | 409 |
| `payload_too_large` | 413 |
| `too_many_requests`, `account_locked` | 429 |
| `engine_unavailable`, `timeout` | 503 / 504 |
| `no_space` | 507 |
| `internal` | 500 |

Bodies are limited per route (64 KiB unless the route takes files).

## Events (WebSocket)

`GET /events` upgrades to a WebSocket (same auth; browsers also need our `Origin`). Messages are
JSON objects tagged by `t`:

- client → server: `{"t":"subscribe","topics":[…]}`, `{"t":"unsubscribe","topics":[…]}`,
  `{"t":"ping","id":1}`. Topics: `{"topic":"torrents"}`, `{"topic":"torrent","arg":"<hash>"}`
  (files, peers, pieces), `{"topic":"session"}`, `{"topic":"log"}` (admins),
  `{"topic":"library"}`, `{"topic":"room","arg":"<code>"}` (members only).
- server → client: `hello`, `pong`, `torrents_full` (the whole list, first), then
  `torrents_delta` (`upserted` rows sent whole, `patched` fields of fast-changing columns,
  `removed` hashes — applying them in order reproduces the list; a gap or reconnect gets a fresh
  `torrents_full`), `labels`, `session`, `torrent_detail`, `notice`, `added`, `completed`,
  `removed`, `log`, `library_changed`, `scan_status`, `sync_play`, `bye`.

Lists are sent as diffs of the server's state on each tick, so a slow client just gets larger
deltas; discrete events (added, completed, notices) a client falls behind on are skipped. `bye`
comes before the server closes the socket (shutdown); reconnect and subscribe again.

## Media

Images are content-addressed (`/images/{sha}?w=`, immutable, ETag). Playback starts with
`POST /playback/{item}` (`PlaybackRequest`: profile, file, audio track, subtitle to burn,
bandwidth caps) and answers `direct` (a Range-capable file URL) or `hls` (a session playlist;
`DELETE /playback/sessions/{id}` ends it). Watch progress: `PUT /items/{id}/state`.

## Endpoints

### Accounts and sign-in

| Method | Path |
|---|---|
| POST | `/api/v1/auth/login` |
| POST | `/api/v1/auth/logout` |
| GET | `/api/v1/auth/me` |
| POST | `/api/v1/auth/setup` |
| POST | `/api/v1/auth/pair` |
| POST | `/api/v1/auth/pair/code` |
| GET | `/api/v1/auth/profiles` |
| POST | `/api/v1/auth/profile` |
| PUT | `/api/v1/auth/pin` |

### Devices

| Method | Path |
|---|---|
| GET | `/api/v1/devices` |
| DELETE | `/api/v1/devices/{id}` |

### Users (admins)

| Method | Path |
|---|---|
| GET POST | `/api/v1/users` |
| GET PATCH DELETE | `/api/v1/users/{id}` |
| PUT | `/api/v1/users/{id}/pin` |

### You

| Method | Path |
|---|---|
| GET PUT | `/api/v1/me/prefs` |
| GET | `/api/v1/theme` |

### Server

| Method | Path |
|---|---|
| GET | `/api/v1/server/info` |
| GET | `/api/v1/server/ca` |
| GET PUT | `/api/v1/server/settings` |
| GET | `/api/v1/server/audit` |
| GET | `/api/v1/server/log` |
| GET | `/api/v1/server/fs` |
| POST | `/api/v1/server/fs/mkdir` |
| POST | `/api/v1/server/alt-speed` |
| GET PUT | `/api/v1/server/remote` |

### Libraries and items

| Method | Path |
|---|---|
| GET POST | `/api/v1/libraries` |
| GET PATCH DELETE | `/api/v1/libraries/{id}` |
| GET POST DELETE | `/api/v1/libraries/{id}/scan` |
| GET | `/api/v1/items` |
| GET PATCH | `/api/v1/items/{id}` |
| PUT | `/api/v1/items/{id}/state` |
| GET | `/api/v1/items/{id}/file` |
| GET | `/api/v1/items/{id}/lyrics` |
| GET | `/api/v1/files/{id}` |
| GET | `/api/v1/images/{sha}` |
| GET | `/api/v1/home` |
| GET | `/api/v1/library/search` |
| GET PUT | `/api/v1/users/{id}/libraries` |

### Playlists and collections

| Method | Path |
|---|---|
| GET POST | `/api/v1/playlists` |
| GET PATCH DELETE | `/api/v1/playlists/{id}` |
| GET POST | `/api/v1/playlists/{id}/items` |
| DELETE | `/api/v1/playlists/{id}/items/{index}` |
| POST | `/api/v1/playlists/{id}/move` |
| GET | `/api/v1/collections` |
| GET | `/api/v1/collections/{id}/items` |

### Plugins and metadata

| Method | Path |
|---|---|
| GET POST | `/api/v1/plugins` |
| DELETE | `/api/v1/plugins/{id}` |
| GET | `/api/v1/plugins/catalog` |
| POST | `/api/v1/plugins/catalog/install` |
| GET | `/api/v1/plugins/web` |
| GET | `/api/v1/plugins/{id}/web/{*path}` |
| PUT | `/api/v1/plugins/{id}/settings` |
| PUT | `/api/v1/plugins/{id}/enabled` |
| GET POST | `/api/v1/items/{id}/identify` |
| POST | `/api/v1/items/{id}/refresh-metadata` |
| POST | `/api/v1/items/{id}/thumbnail` |
| POST | `/api/v1/libraries/{id}/refresh-metadata` |

### Subtitles

| Method | Path |
|---|---|
| GET POST | `/api/v1/items/{id}/subtitles` |

### Playback

| Method | Path |
|---|---|
| POST | `/api/v1/playback/{item}` |
| GET | `/api/v1/playback/sessions` |
| DELETE | `/api/v1/playback/sessions/{id}` |
| GET | `/api/v1/hls/{id}/index.m3u8` |
| GET | `/api/v1/hls/{id}/{segment}` |
| GET | `/api/v1/remux/{file}/{audio}/{name}` |
| GET | `/api/v1/files/{id}/subtitles/{name}` |
| GET | `/api/v1/files/{id}/trickplay/{name}` |

### Casting

| Method | Path |
|---|---|
| GET | `/api/v1/cast/devices` |
| GET | `/api/v1/cast/{device}` |
| POST | `/api/v1/cast/{device}/play` |
| POST | `/api/v1/cast/{device}/command` |

### Watching together

| Method | Path |
|---|---|
| POST | `/api/v1/syncplay` |
| GET | `/api/v1/syncplay/{id}` |
| POST | `/api/v1/syncplay/{id}/join` |
| POST | `/api/v1/syncplay/{id}/leave` |
| POST | `/api/v1/syncplay/{id}/command` |

### Podcasts

| Method | Path |
|---|---|
| GET POST | `/api/v1/podcasts` |
| DELETE | `/api/v1/podcasts/{id}` |
| POST | `/api/v1/podcasts/{id}/refresh` |

### Radio

| Method | Path |
|---|---|
| GET POST | `/api/v1/radio` |
| DELETE | `/api/v1/radio/{id}` |
| GET | `/api/v1/radio/{id}/stream` |

### Torrents

| Method | Path |
|---|---|
| GET POST | `/api/v1/torrents` |
| POST | `/api/v1/torrents/preview` |
| POST | `/api/v1/torrents/queue` |
| POST | `/api/v1/torrents/actions` |
| POST | `/api/v1/torrents/move` |
| GET PUT | `/api/v1/torrents/categories` |
| PUT | `/api/v1/torrents/tags` |
| GET PATCH | `/api/v1/torrents/{hash}` |
| GET | `/api/v1/torrents/{hash}/detail` |
| POST | `/api/v1/torrents/{hash}/files` |
| POST | `/api/v1/torrents/{hash}/trackers` |

### RSS

| Method | Path |
|---|---|
| GET | `/api/v1/rss` |
| GET PUT | `/api/v1/rss/config` |
| POST | `/api/v1/rss/refresh` |
| POST | `/api/v1/rss/download` |

### Torrent search

| Method | Path |
|---|---|
| GET | `/api/v1/search` |

### Wanted list, quality profiles, monitoring

| Method | Path |
|---|---|
| GET POST | `/api/v1/wanted` |
| DELETE | `/api/v1/wanted/{id}` |
| POST | `/api/v1/wanted/{id}/search` |
| POST | `/api/v1/wanted/{id}/grab` |
| GET POST | `/api/v1/quality-profiles` |
| PUT DELETE | `/api/v1/quality-profiles/{id}` |
| GET | `/api/v1/items/{id}/releases` |
| PUT DELETE | `/api/v1/items/{id}/monitor` |
| GET | `/api/v1/monitored` |
| POST | `/api/v1/items/{id}/grab` |
| GET | `/api/v1/torrents/{hash}/files/{file}/stream` |

### Tasks, notifications, backups (admins)

| Method | Path |
|---|---|
| GET | `/api/v1/tasks` |
| PUT | `/api/v1/tasks/{id}` |
| POST | `/api/v1/tasks/{id}/run` |
| GET | `/api/v1/tasks/{id}/history` |
| GET POST | `/api/v1/notifications` |
| PUT DELETE | `/api/v1/notifications/{id}` |
| POST | `/api/v1/notifications/{id}/test` |
| GET POST | `/api/v1/backups` |
| GET | `/api/v1/backups/{name}` |
| POST | `/api/v1/backups/{name}/restore` |
| POST | `/api/v1/backups/upload` |
| GET DELETE | `/api/v1/backups/restore` |

### Events

| Method | Path |
|---|---|
| GET | `/api/v1/events` |

Paths with `{…}` take ids (numbers), info hashes (40 hex) or names as shown. `server/settings`
and `torrents` mirror the desktop app's settings and engine; their bodies are the types in
`corrent-proto/src/{server,torrent}.rs`.
