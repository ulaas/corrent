# Corrent Media Server — plan

Drafted 2026-10-19 by a Fable planning agent from the code at `7054e6d` and the owner's request
(below); reviewed the same day by a second Fable agent against the code at `2ad9093`, the cargo
registry cache and mpv spikes run on the dev machine (see "Review notes" at the end). Crate
versions, mpv headers and eframe/egui APIs cited here were checked against crates.io, docs.rs, the
mpv repository and the local cargo cache on that date; everything that could not be checked is
marked **(unverified)** and listed again in "Verification notes" at the end.

> **The request (verbatim):** "ok now i want to turn corrent into a mediasever (like jellyfin
> and/or kodi) with a client server mechanism where torrent functionality willl be a tightly
> integrated part of it, it should be abel to serve a locally secure webpage for management,
> starting adding maintaining torrent jobs or any other possible maintenance user intterfaces.
> there will be 3 types of clients, one is the web based served by the server itself, an egui
> native client for all operating systems (server also will be cross platform as corrent is now),
> we want to use libmpv for encoding, transcoding and playback on server and native client side.
> library managament, plugin architecture, performance optimized architecture beautiful looking
> customizable fluid motion interfaces full audio support, ability to work behind network nats and
> any other idea that you may come up with for a full blown media server solution that is
> extendable and felxble. lay down a complete plan and start implementation"

## Where we are

Corrent is a single crate (`corrent`, ~25.4k lines of Rust 2024 including inline tests, 170 `#[test]`s) with an
engine actor over librqbit 9.0.1 (`src/engine/actor.rs`, commands in, `ArcSwap<Snapshot>` out), an
eframe/egui 0.36.2 (glow) desktop UI, a loopback HTTP stream server for media players
(`src/engine/stream.rs`), a qBittorrent-compatible Web API on axum 0.8 with a 340-line web UI
(`src/webapi`, `assets/webui`), RSS rules, Torznab search, gettext-style i18n with a Turkish
catalog, schema migrations, crash reports, backups, and packaging for three OSes. Everything in
`docs/ROADMAP.md` is done; `docs/STABILITY.md` is the hardening backlog.

What the media server adds, in one sentence: a headless, multi-user **server** that embeds the
torrent engine and a **media library** (SQLite, scanner, metadata, artwork, watch state), serves a
versioned **API** with realtime events, a **web client**, and a **native egui client** that plays
media through **libmpv** — while today's single-user desktop experience keeps working unchanged,
because the native client can host the server in-process.

The dev machine is now Fedora Linux (x86_64, rustc 1.99.1, Windows GNU target installed, Node 24).
`mpv` 0.41 (FFmpeg 9.0.1) and `/usr/lib64/libmpv.so.2` are installed but there are **no libmpv
headers and no ffmpeg/ffprobe** — which is exactly the situation a user's machine will be in, and
the reason the plan loads libmpv at runtime. The GPU is an NVIDIA RTX 5080 (driver 615,
`libnvidia-encode.so` present), so **NVENC can be tested locally**; there is no VAAPI/QSV device
here, so VAAPI stays owner-gated. `.cargo/config.toml` already sets `incremental = false`;
`target/` is 6.5 GB today with 1.4 TB free.

**Legend:** **[O]** = needs the owner (accounts, keys, signing, pushing, installing system
packages, running on other OSes); **[S]/[M]/[L]** = effort: S ≈ up to two days, M ≈ about a
week, L ≈ two to four weeks of focused agent work including reviews and tests; **(opt)** =
optional, can be dropped without hurting the core; **(unverified)** = see "Verification notes".
**Torrent-client regression is a blocker in every milestone**: `cargo test` and the kittest UI tests
of the existing client must stay green at every commit.

---

## Product shape

### Processes and binaries

| Binary | Crate | What it is |
|---|---|---|
| `corrent` | root package (today's crate) | The native egui client. By default it hosts the server **in-process** ("Local" mode: today's experience, zero sockets required), or connects to a remote `correntd`. One binary, so OS handler registration, packaging and single-instance IPC stay as they are. |
| `correntd` | `crates/correntd` | Headless cross-platform server: torrent engine + library + API + web client. Runs as a systemd/launchd service or a Windows service, or in the foreground (`--portable`). No GUI dependencies at all (no egui, no winit, no GL). |
| `corrent-mpvw` | `crates/corrent-mpv` (`[[bin]]`) | Small worker process around libmpv for server-side jobs: probe, thumbnails/trickplay, encode/transcode. One process per job; a decoder crash or leak never takes the server down. |
| `correntctl` (opt) | `crates/correntd` (`[[bin]]`) | Admin CLI over the API: create admin, reset password, scan, status. [S] — done as `correntd admin …` (`--init-admin` creates the admin) |

The native client has three ways to reach a server, behind one trait (`ServerLink`):

- **Local** — the server library runs inside the `corrent` process; the UI keeps reading the
  engine's `Arc<Snapshot>` directly (no serialization), exactly as today. This is the default and
  the mode every existing kittest/e2e test runs in.
- **Remote (LAN/Internet)** — HTTPS + WebSocket to `correntd`; the client rebuilds the same
  `Snapshot` struct from deltas, so the torrent table, details and dialogs are the same code.
- **Remote via iroh** — same HTTP over a QUIC connection dialed by the server's public key
  (hole-punched, relay fallback); for servers behind NAT with no port forwarding.

### Crates (target layout) and where today's modules go

```
Cargo.toml                  [workspace] members = [".", "crates/*", "plugins/*"]; root = package `corrent` (GUI client)
src/app, src/ui, src/integration, src/main.rs      stay in the root crate (the client); + src/app/media/*, src/app/player/*, src/link/*
crates/corrent-i18n         src/i18n.rs + assets/i18n (t!/tk!, catalogs, the two gate tests, now scanning the workspace)
crates/corrent-proto        API v1 wire types only (serde): InfoHash, Torrent/TorrentPatch, Library/Item/Stream DTOs,
                            Event, auth types, error codes. No tokio, no egui.
crates/corrent-core         src/engine/*, store.rs, config.rs, rss/*, search.rs, import.rs, backup.rs, migrate.rs,
                            crash.rs, util.rs, updates.rs — egui removed (EngineHandle takes a `Wake` closure)
crates/corrent-mpv          runtime-loaded libmpv (libloading): FFI subset, safe wrapper, render contexts (GL + SW),
                            stream_cb protocols, encode helper, probe; `corrent-mpvw` worker binary
crates/corrent-library      SQLite (rusqlite, bundled): schema + migrations, identity tables, libraries/items/streams/
                            images/watch state/playlists/collections/wanted list, scanner, filename parser, tag reader
                            (lofty), probe adapter, artwork + trickplay pipelines, FTS5 search
crates/corrent-transcode    client capability profiles, play-method decision engine, `Transcoder` trait, mpv and
                            ffmpeg backends, HLS session manager (virtual playlists, segment windows, cache)
crates/corrent-plugins      wasmtime component host, WIT worlds (wit/*.wit), manifest, permissions, catalog, process plugins
crates/corrent-server       lib: axum router (v1 + qBittorrent v2 moved from src/webapi), auth/sessions/devices/pairing,
                            WS event fan-out with deltas, TLS (rcgen / rustls-acme), discovery (mDNS, UPnP/NAT-PMP),
                            iroh listener, media/HLS/artwork endpoints, embedded web client (rust-embed), audit log
crates/correntd             bin: CLI, service wrappers (systemd notify, launchd, Windows service), logging, shutdown
plugins/{tmdb,tvdb,musicbrainz,coverart,fanart,opensubtitles}   first-party WASM components (wasm32-wasip2)
web/                        Svelte 5 + Vite + TypeScript client; `web/dist` is committed and embedded (no Node needed for cargo build)
design/tokens.json          shared design tokens (colors, type scale, radii, spacing, motion) for both clients
```

**Migration path that keeps every commit green** (each step is one or two commits; the root crate
compiles and all tests pass after each):

1. Add `[workspace]` to `Cargo.toml` (root package stays `corrent`; `[patch.crates-io]` and
   `[profile.*]` move to workspace level, dependencies become `[workspace.dependencies]`).
2. `crates/corrent-i18n`: `git mv src/i18n.rs assets/i18n` there; the root crate does
   `#[macro_use] extern crate corrent_i18n;` (or `pub(crate) use corrent_i18n::{t, tk};`). The gate
   tests `no_hard_coded_ui_strings` and `turkish_catalog_is_complete` take a list of crate roots
   (`CARGO_MANIFEST_DIR/..`) instead of the hard-coded `src/app, src/ui, src/engine, src/rss` list.
3. `crates/corrent-proto`: `InfoHash` (+ serde impls and tests) moves here; `engine::types` re-exports
   it; add `TorrentState`, `Torrent`, `TorrentPatch`, `SessionStats`, `Event`, `ApiError`.
4. `crates/corrent-core`: `git mv` of the modules listed above. The egui uses in those modules
   are `engine/mod.rs` (`EngineHandle { ctx }`, also used by `restart`), `engine/actor.rs`
   (`ctx.request_repaint()` in `publish` and four other places) and `engine/e2e_tests.rs`
   (`egui::Context::default()`): replace with `pub type Wake = Arc<dyn Fn() + Send + Sync>` passed
   to `EngineHandle::start` and stored in the handle. Two more couplings to the GUI must move with
   them (checked in the code): `crash.rs:103` reads `crate::app::log::LINES` (the log ring buffer
   in `src/app/log.rs` becomes `corrent_core::log`, the GUI's Log window reads it from there), and
   `config.rs:439` embeds `crate::ui::theme::PaletteDef` (plain `#rrggbb` strings with serde — move
   the struct to core, e.g. `config::PaletteDef`, and re-export it from `ui::theme`). `EngineCommand`
   and `EngineEvent` live in `engine/mod.rs`. The root crate keeps
   `mod engine { pub use corrent_core::engine::*; }` shims for a while so UI paths don't churn.
   (`config::Settings` keeps its `Appearance` section for now; the server ignores it.)
5. `crates/corrent-server`: `git mv src/webapi` → `server::qbt` + `server::auth`; `WebApiSettings`
   stays in `Settings`; the root app starts `corrent_server::Server` the same way it started
   `WebApiServer`. Sonarr/Radarr paths and behaviour unchanged.
6. New code (v1 API, `correntd`, mpv, library …) lands in new crates from here on. Moving the GUI
   itself to `crates/corrent-app` is a pure `git mv` that can happen any time later.

### Data directory

```
<data>/settings.toml  store.json(.bak)  session/  torrents/  rss.json  window.ron  logs/   (unchanged)
<data>/server.toml      server-only settings (bind, TLS, remote access, library roots are in the DB)
<data>/corrent.db(-wal) one SQLite file: identity + library (VACUUM INTO for backups)
<data>/tls/             ca.pem, server.pem/key (rcgen), acme/ cache
<data>/artwork/<xx>/<sha256>.{jpg,w200.jpg,w400.jpg,w800.jpg}    content-addressed, immutable URLs
<data>/trickplay/<item>/<w>/sheet-<n>.jpg + index.vtt
<data>/transcode/<session>/                 bounded LRU cache (default 10 GiB)
<data>/plugins/<id>/<version>/plugin.toml + *.wasm + data/ (plugin KV)
```
Portable mode (`--portable`) keeps all of it under `target/debug/corrent-data` as today.

---

## Track A — API and protocol

### A1. Versioned REST API `/api/v1` [M]
- **What:** JSON over HTTP, typed by `corrent-proto`. Resources: `auth`, `users`, `devices`,
  `server` (info, settings, tasks, audit, logs), `torrents` (list/add/preview/actions/files/peers/
  trackers/limits/queue/categories/tags, streams), `rss`, `search` (Torznab), `wanted`, `libraries`,
  `items` (query, detail, images, subtitles, chapters, trickplay, similar), `playback` (decide,
  sessions, progress), `playlists`, `collections`, `plugins`, `i18n/{lang}`, and `server/fs`
  (a permission-gated **server-side folder browser**: list drives/dirs, create folder, free space —
  remote clients cannot use `rfd` pickers for save paths, library roots or watch folders).
- **How:** axum 0.8 routers per resource in `corrent-server/src/v1/*.rs`; extractors `Auth(User)`
  and `Perm<T>`; errors are `ApiError { code: &'static str, params: Map, status }` with English
  text for logs — **clients translate by code** (the "never compare translated text" rule extends to
  the wire). Pagination by opaque cursors; list endpoints take `fields=` to trim payloads. ETags on
  items and artwork; `Cache-Control: immutable` on content-addressed images. OpenAPI generation via
  `utoipa` 6.0 behind a cargo feature (opt), so the build cost is zero by default; `docs/API.md`
  describes auth, events and the delta rules by hand. — `docs/API.md` written (auth, errors, events, every route; a test keeps the route list complete); utoipa not added.
- **Accept:** `tower::ServiceExt::oneshot` tests for every endpoint family; a `proto` round-trip
  test per type; the qBittorrent API (`/api/v2`) still passes its Sonarr/Radarr smoke test.

### A2. Realtime channel: WebSocket `/api/v1/events` with deltas [M]
- **What:** replaces snapshot polling for remote clients. Topics: `torrents`, `torrent:<hash>`
  (files/peers/pieces of a selected torrent), `session`, `library:<id>`, `tasks`, `sessions`
  (who plays what), `log` (admin). Client messages: `subscribe`, `unsubscribe`, `ping`.
- **How:** axum `ws` feature (axum 0.8.9 pins **tokio-tungstenite 0.29**, verified in its
  `Cargo.toml`; the cached 0.21 in the registry is from another project). Server-side the engine's
  `Wake` closure notifies a fan-out task (`tokio::sync::Notify`); it loads `engine.snapshot()` and,
  per connected client, diffs against the `Arc<Snapshot>` it last sent (an `Arc`, so keeping it is
  free) at that client's rate (default 2 Hz, 1 Hz over iroh/relay). The engine itself publishes at
  `advanced.refresh_interval_ms` (default 500 ms, clamped 200–5000), so a client rate above the
  engine rate is pointless; "4 Hz on LAN" only makes sense if the server's interval is lowered.
  Output is `torrents.full { rid, torrents, categories, tags }` on (re)subscribe (≈ 2 MB of JSON
  for 5,000 torrents — send it as a **deflate-compressed binary frame** (`flate2` is already a
  dependency; tungstenite has no permessage-deflate) and honour `fields=`) and
  `torrents.delta { rid, added: [Torrent], changed: [TorrentPatch], removed: [hash] }` afterwards,
  where `TorrentPatch` has every fast-changing field as `Option` with `skip_serializing_if`. 5,000
  torrents: a per-client diff is one hash-map pass (< 1 ms); a typical delta is a few hundred
  patches of ~80 bytes. Per-client send queue is bounded; when it fills, telemetry topics drop to
  "resync on next tick" instead of growing. Details of one torrent are produced on demand by a new
  `EngineCommand::Detail { hash, reply }` (like `FileList`), so several clients can look at different
  torrents (today `SetDetailTarget` is single-target).
- **Accept:** a test that mutates 5,000 synthetic views for 60 ticks and asserts bytes/tick and that
  applying the deltas on the client reproduces the snapshot exactly; a WS integration test with
  tokio-tungstenite.

### A3. Client-side `ServerLink` (native) [L]
- **What:** `src/link/{mod,local,remote}.rs`: `trait ServerLink { fn snapshot(&self) -> Arc<Snapshot>;
  fn send(&self, cmd: Command); fn library(&self) -> &dyn LibraryApi; fn images(&self) -> ImageSource; ... }`.
- **How:** `LocalLink` wraps `EngineHandle` + in-process library handles (no serialization).
  `RemoteLink` owns a reqwest client (rustls, pinned cert), a WS task that applies deltas into an
  `ArcSwap<Snapshot>` and calls `ctx.request_repaint()`, and a command queue with retries/backoff
  and offline banners. `EngineCommand` splits into the wire `Command` (proto) and local-only ones
  (pickers, file drops) which `LocalLink` executes directly and `RemoteLink` uploads (`.torrent`
  bytes go through `POST /torrents` multipart).
- **The long tail (why [L], not [M]):** the UI consumes more than `Snapshot`: the
  `std::sync::mpsc::Receiver<EngineEvent>` (Toast, PreviewReady, Added, Completed, Removed, Created,
  Restored, DataWarning) must be reproduced from WS `Event`s with request ids; `Snapshot.detail`
  becomes an on-demand subscription; `Snapshot.rss` (`Arc<RssView>`), `undoable`, `busy_ops`,
  `free_space`, `row_history` all need wire forms; the add dialog's save-path picker, watch folders
  and library roots need the `server/fs` browser (A1); the torrent creator, import-from-other-clients
  and "open folder" are `LocalOnly`. The kittest UI tests inject a fake snapshot straight into
  `app.snap` (`src/app/ui_tests.rs`), so the `ServerLink` design must keep a test seam for that
  (e.g. a `TestLink` or keeping `app.snap` as the single read path fed by the link).
- **Accept:** the full existing torrent UI (table, details tabs, add/batch dialogs, feeds, search,
  streams window) works against a `correntd` on the same machine with no code paths special-cased
  for remote beyond `LocalOnly` features (open folder, run on complete, creator, import) being hidden.

---

## Track S — Security ("a locally secure webpage for management")

- **S1 Users and roles [M]:** tables `users(id, name, display_name, pw_hash, role, perms, max_rating,
  disabled, created_at)`, roles `admin | user | guest | kids` with default permission sets and a
  per-user override bitset (`torrents.add`, `torrents.manage_own`, `torrents.manage_all`,
  `library.manage`, `playback.transcode`, `settings.read`, `settings.write`, `users.manage`,
  `plugins.manage`). `kids` adds a rating ceiling and a library allowlist; `guest` is read/play only.
  Library access per user (`user_libraries`). Torrents record `added_by`.
- **S2 Passwords and tokens [S]:** argon2id (`argon2` 0.6; m=64 MiB, t=3, p=1) on `spawn_blocking`
  behind the existing two-permit semaphore; the qBittorrent PBKDF2 hash migrates into the first
  admin user (`schema_version` bump + migration, dated backup as in `migrate.rs`). Opaque 32-byte
  random tokens stored hashed (SHA-256) in `sessions(id, user, device, kind, expires, last_seen, ip,
  ua)`. Two kinds: **session tokens** (web, 12 h sliding) and **device tokens** (native/TV clients,
  90 days sliding, revocable from "Devices"). Bearer header for API clients; the web client uses an
  `HttpOnly; Secure; SameSite=Strict` cookie plus a double-submit `X-Corrent-CSRF` header; the
  existing Host/Origin/Referer checks stay (DNS rebinding, CSRF). The qBittorrent cookie login maps
  to a user with the `downloader` permission set and keeps its path. On loopback plain HTTP the
  `Secure` cookie still works (browsers treat `localhost` as a secure context). Native clients keep
  device tokens in `<data>/servers.toml` created with mode 0600 (Unix) / user-only ACL (Windows);
  OS keyring storage (`keyring` crate) is a later option, not a blocker.
- **S3 Device pairing [S]:** admin or the user themselves creates a pairing code (6 digits, 10 min,
  single use) from any client; the new device posts it with its name/platform and receives a
  device token. The server's QR payload is `corrent://pair?host=<ip:port>&id=<server id>&fp=<SPKI
  sha256>&code=<6 digits>&relay=<url>` — the native client can paste it or scan it where a camera
  exists later. A code is burned after 5 wrong attempts (10⁶ combinations are brute-forceable
  otherwise) on top of the S6 token bucket; at most 3 outstanding codes per user. `qrcode` crate
  for rendering (version unverified; trivial to swap).
- **S4 TLS [M]:** on first start the server mints a CA + leaf with `rcgen` 0.14 (SANs: hostname,
  LAN IPs, `localhost`, server id), serves HTTPS with `axum-server` 0.8 (rustls), and prints/shows the
  fingerprint. Native clients **pin the SPKI hash** learned at pairing (TOFU with explicit confirm);
  browsers get the classic self-signed warning once, then the Settings → Security page (served over
  that HTTPS connection) offers the CA certificate for download with install instructions per OS,
  and the fingerprint is always visible in the native UI, `correntd --print-fingerprint` and the
  pairing QR so people can compare. **No "trust this server" page over plain HTTP on the LAN**: a
  LAN attacker could serve their own CA through it. Optional ACME via `rustls-acme` 0.15 when the
  owner has a domain and a reachable port 80/443 [O]. Plain HTTP stays available only on loopback.
- **S5 LAN-only default [S]:** the listener binds `0.0.0.0`/`::` but a middleware refuses
  non-loopback, non-RFC1918/ULA/link-local peers until "Remote access" is switched on (then the
  user also chooses port mapping / iroh / nothing). The allowlist must also include
  **100.64.0.0/10 (CGNAT — Tailscale addresses live there)**, normalise IPv4-mapped IPv6 peers
  (`::ffff:a.b.c.d`), be extensible with user CIDRs, and ignore `X-Forwarded-For` unless a trusted
  proxy address is configured (otherwise it is spoofable). Separate from `allow_lan` for the
  qBittorrent API, which stays as it is (that one picks the bind address instead).
- **S6 Rate limits and lockouts [S]:** keep the per-IP failed-login bans; add per-account lockout
  (10 failures → 15 min), a global token-bucket on `auth/*` and `pair/*`, request body caps per
  route, header/idle timeouts (STABILITY F6 lands here).
- **S7 Audit log [S]:** `audit(ts, user, ip, action, target, result, details)` for logins, token
  creation/revocation, settings changes, user/plugin changes, torrent removals with data, library
  deletes; viewable at `/admin/audit` in both clients; rotated by count.
- **S8 Web hardening [S]:** CSP as today (no inline scripts; hashes for the single bootstrap), COOP/
  CORP, `frame-ancestors 'none'`, `Permissions-Policy`, `Referrer-Policy: no-referrer`, no
  third-party origins (fonts and hls.js are bundled). Same-origin `<img>`/`<video>`/hls.js requests
  carry the session cookie, so artwork and streams need **no URL tokens** for the web client; the
  native client passes its device token to mpv with `--http-header-fields="Authorization: Bearer …"`.
  URL tokens (`?t=`) exist only for third-party players and casting: short-lived (6 h), bound to the
  issuing session, scoped to one item, revoked with it — never logged.
- **Accept:** security tests for CSRF/origin/rebinding/lockout/pinning; a short threat model in
  `docs/SECURITY.md` listing what is and isn't protected (LAN attacker, lost device, malicious plugin).

---

## Track L — Library

### L1. Storage: SQLite via rusqlite [M]
- **Choice:** `rusqlite` 0.40 with `bundled` (SQLite built in; `SQLITE_ENABLE_FTS5`, `JSON1`, `RTREE`
  are set unconditionally by libsqlite3-sys's bundled build — verified). Not `sqlx` 0.9: its
  compile-time-checked queries need a database at build time or an offline cache, the async driver
  adds nothing for an embedded file, and the rest of Corrent is synchronous-core + actors.
- **How:** `corrent-library/src/db/`: one connection on a dedicated **writer thread**
  (`BEGIN IMMEDIATE` transactions, batched work queue, coalescing like STABILITY F1's writer), a
  **read pool** of 4–8 connections used through `spawn_blocking`. Pragmas: `journal_mode=WAL`,
  `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`, `temp_store=MEMORY`,
  `mmap_size=256MiB`, `cache_size=-65536`. Migrations: numbered `migrations/NNNN_*.sql` embedded
  with `include_str!`, applied under `user_version`, dated `VACUUM INTO` backup before each,
  refusing to open a newer schema (same rules as `migrate.rs`). Integrity check at startup
  (`PRAGMA quick_check`) with the recovery banner path from M3.
- **Core schema** (abridged; full DDL lives in the migrations):
  ```sql
  users, sessions, devices, audit, user_libraries, pairing_codes                     -- identity (Track S)
  libraries(id, name, kind{movies,shows,music,audiobooks,podcasts,videos,photos}, settings_json)
  library_folders(library_id, path, watch, last_scan)
  files(id, library_id, path UNIQUE, size, mtime, quick_hash, probe_json, probed_at, missing_since, source_json)
  items(id, library_id, kind{movie,show,season,episode,artist,album,track,book,chapter,video,photo,album_photo},
        parent_id, title, sort_title, original_title, year, index_no, index2, overview, runtime_ms,
        rating, official_rating, premiered, added_at, updated_at, locked_fields, provider_ids_json)
  item_files(item_id, file_id, role{main,extra,trailer,theme})
  streams(file_id, idx, kind{video,audio,subtitle}, codec, profile, level, lang, title, default_, forced,
          width, height, fps, bitrate, channels, sample_rate, hdr, external_path)
  people(id, name, provider_ids_json, image_id), item_people(item_id, person_id, role, character, order)
  genres(id, name), item_genres; tags(id, name), item_tags; studios…
  images(id, owner_kind{item,person,collection,library,user}, owner_id, kind{poster,backdrop,logo,thumb,banner,art},
         sha256, width, height, blurhash, source)        -- people/collections have art too, so no item_id FK
  chapters(file_id, idx, start_ms, title); trickplay(file_id, width, interval_ms, tiles, ready)
  watch_state(user_id, item_id, position_ms, played, play_count, last_played_at, rating, favorite)
  playlists(id, user_id, name, public), playlist_items(playlist_id, pos, item_id)
  collections(id, library_id, name, provider_ids_json), collection_items
  wanted(id, kind, title, year, season, episode, provider_ids_json, profile_id, status, item_id, added_by)
  quality_profiles(id, name, rules_json); grabs(wanted_id, torrent_hash, state, when)
  tasks(id, kind, state, progress, message, started, finished, log /* capped at 64 KiB, tail kept */)
  items_fts (FTS5 external-content table over title/original_title/overview/people/album/artist)
  ```
- **Accept:** migration tests with fixtures; a 50k-item synthetic DB fixture generator used by the
  perf tests (Track Q).

### L2. Scanner [L]
- **What:** incremental, resumable, cancellable scans per library; filesystem watching; moved-file
  detection; "missing" grace period instead of deleting.
- **How:** `scanner/` pipeline as tasks over bounded channels: **walk** (`walkdir`, ignore patterns,
  `.nomedia`) → **classify** by extension and library kind → **diff** against `files` (path, size,
  mtime; unchanged rows skip everything else) → **quick hash** (xxhash of first+last 64 KiB + size)
  to recognise moves/renames → **parse** (L3) → **probe** (L4, bounded by a semaphore, mpv worker)
  → **tags** (L5) → **match/metadata** (L6, async, rate-limited) → **artwork** (L7) → **FTS** update.
  Watching with `notify` 8.2 + `notify-debouncer-full` (2 s debounce, per-folder; polling fallback for
  network shares and when inotify limits hit). A full rescan is scheduled (default nightly) and
  available per library/folder. Torrent-sourced files enter the same pipeline (Track T).
- **Accept:** scan ≥ 10k files/min on the diff path (no probing) on the dev machine; 1k new files
  with probing in < 5 min; adding/removing/moving files under watch updates items within 5 s;
  cancelling a scan leaves a consistent DB.

### L3. Filename and folder parsing [M]
- `parse/` (pure, table-driven, fuzzed): movies `Title (2019)`, `Title.2019.1080p.BluRay.x264-GRP`,
  editions, parts; episodes `S01E02`, `1x02`, `S01E02E03`, dates `2024-05-01`, anime ` - 01`,
  absolute numbering, specials; music by folder structure `Artist/Album/01 - Title`; cleanup of
  release-group noise; language and quality tokens (also used by the quality profiles in T4).
  Tests ported from the usual corpora (own fixtures, no scraping). Users can lock fields.

### L4. Probing with libmpv [M]
- **What:** container/stream facts without ffprobe: duration, chapters, tracks (codec, profile,
  level, language, title, default/forced, channels, sample rate), video params (size, fps, pixel
  format, HDR/colour), container metadata, embedded fonts/attachments count.
- **How:** `corrent-mpvw probe <path>` creates an mpv handle with `vo=null ao=null pause=yes
  idle=yes audio-display=no hwdec=no`, `loadfile`, waits for `MPV_EVENT_FILE_LOADED`, reads
  `track-list`, `duration`, `chapter-list`, `metadata`, `video-params`, `audio-params`,
  `container-fps`, `file-format`, then `quit`. Output is one JSON line (`MediaInfo`). Per-job timeout
  30 s; 2–4 workers in parallel. `ffprobe` is an optional second backend behind the same
  `Prober` trait (used only if installed; adds `codec_tag`, `bit_rate` per stream which mpv doesn't
  always expose — exact field coverage **(unverified)**, the spike measures it).
- **Accept:** probes of Big Buck Bunny / Sintel in MKV, MP4 and WebM match expected tracks;
  a corrupt file yields a typed error, not a stuck worker.

### L5. Music tags [S]
- `lofty` 0.25 (pure Rust; ID3v2, Vorbis comments, MP4 atoms, APE, RIFF INFO, FLAC pictures) for
  title/artist/album artist/album/track/disc/year/genre/MusicBrainz ids/ReplayGain gains and
  embedded cover art; album grouping by `album artist + album + year`; compilations; `cue` sheets
  (opt). Audiobooks: folder = book, files = chapters, `m4b` chapter atoms via probe.

### L6. Metadata providers (plugins) [L]
- TMDB (movies, shows, people, collections), TVDB (episodes/artwork alt), MusicBrainz + Cover Art
  Archive (music), fanart.tv (logos/backgrounds), OpenSubtitles (subtitles) — all as first-party
  **WASM plugins** (Track X) with **user-supplied API keys** stored as secret plugin settings.
  Matching: parsed title/year → provider search → scored candidates (title similarity, year, runtime)
  → auto-accept above a threshold, otherwise "Identify…" in the UI. Refresh policies (never / missing
  only / all) per library; locked fields are never overwritten; `.nfo` reading (Kodi format) as a
  built-in provider so existing libraries import their edits; `.nfo` writing (opt) — done (library setting; only ever replaces its own files).
- **Accept:** a fake provider plugin in tests; an identify flow test; rate limits respected
  (TMDB ~40 req/10 s) with a per-provider token bucket in the host, not in the plugin.

### L7. Artwork, thumbnails, trickplay [M]
- Fetch → verify (magic bytes, size caps) → decode (`image` 0.25) → resize (`fast_image_resize` 6.1,
  SIMD) into 200/400/800 px JPEG q85 + a 32×32 blurhash for placeholders → content-addressed
  store (`artwork/<xx>/<sha>…`) → DB. Work runs in a pool of `spawn_blocking` tasks bounded by
  CPU count; one in-flight fetch per URL; failures backoff. Trickplay: `corrent-mpvw thumbs` seeks
  every 10 s (configurable) and takes `screenshot-raw` (in-memory image; the command exists in mpv
  0.41 — `mpv --input-cmdlist` lists `screenshot-raw [flags] [format]` — fallback: `vo=image`),
  tiles 10×10 sheets at 320 px + a WebVTT index; generated by
  a scheduled task with a priority queue (recently added first). Chapter thumbnails from the same
  frames.
- **Accept:** a 50k-item library's artwork pipeline stays under 1 GiB RAM; image endpoints serve
  with ETags and `immutable`; thumbnails ≤ 15 ms each on the dev machine.

### L8. Subtitles [M]
- Embedded tracks from probe; external `.srt/.ass/.ssa/.vtt/.sub` by name (`Movie.en.forced.srt`
  conventions); `subtitles/` subfolders. For the web: SRT/ASS/SSA → WebVTT converter (own,
  fuzzed; ASS styling reduced to positions/italics) served at `/items/{id}/subtitles/{idx}.vtt`;
  burn-in when the client asks (image subs: PGS/VobSub always burn-in on the web). Native: mpv
  renders everything (libass) with user font/size/colour defaults from Settings. OpenSubtitles as a
  plugin with download quotas shown; "search subtitles" from the player.

### L9. Watch state, continue watching, collections, playlists, search [M]
- Progress `PUT /items/{id}/progress` every 10 s and on pause/stop/seek; "played" at ≥ 90 % (or
  per-library threshold); per-user "Continue watching", "Next up" (episodes), "Recently added",
  "Because you watched" (same genres/people, opt). Playlists (user, drag-to-reorder, play all/shuffle
  in both clients), collections (manual + TMDB collections), favorites, ratings. Search: FTS5
  (`unicode61` tokenizer + prefix index; `trigram` for substring search — available: the `trigram`
  tokenizer is part of FTS5 since SQLite 3.34 and libsqlite3-sys 0.38.2 bundles **SQLite 3.53.2**
  with `SQLITE_ENABLE_FTS5`, verified in its `build.rs`) over items/people/albums with per-kind
  ranking; `GET /search?q=` returns grouped
  results; both clients show a global search field (`/` shortcut).
- **Accept:** home endpoint p95 < 100 ms on the 50k fixture (Track Q); search < 50 ms.

---

## Track T — Torrent integration (the part Jellyfin/Kodi don't have)

- **T1 Category → library import [M]:** a library declares which torrent categories feed it and how:
  `in_place` (library scans the download folder; file stays seeding — default), `hardlink` (same
  volume), `copy`, `move_after_seeding_goal`. On `EngineEvent::Completed` (and at startup for
  already-complete torrents) the server runs an import job. **Today's `Completed { name,
  content_path, save_path, size }` carries no info hash** (`engine/mod.rs:81`) — add `hash: InfoHash`
  (and the category) to it in MS1, it is a two-line change with one caller. The job: classify files (video/audio/extras/
  samples/junk), parse, probe, create items with `files.source_json = {torrent: hash, file: idx}`.
  Removing a torrent with data warns when library items reference the files (and offers "keep
  files"); "never delete library data" is the default guard.
- **T2 Play while downloading [M]:** items whose source torrent isn't complete are marked
  `available: partial`; playing them goes through `GET /torrents/{hash}/files/{i}/stream` (the
  existing `FileStream`-backed Range server generalised into an authenticated axum route; reading
  prioritizes pieces at the read position, as today). The native client plays that URL in mpv
  (or, locally, through a `corrent://` `stream_cb` protocol reading the file directly — no HTTP hop).
  The transcoder reads the same URL when the web client needs HLS. The engine raises the file's
  priority and front-loads the first/last pieces (already librqbit's behaviour). Availability bars in
  the player seek slider come from `StreamView.availability`.
- **T3 Search → "Get this" [M]:** Torznab results (existing `search.rs`) gain a parsed quality
  (L3 tokens), size and seeders; from a library item or a "wanted" entry, "Find releases" lists
  matching results; "Get" adds with the right category, links `grabs` to the item/want, and the
  import closes the loop. Also from the web client.
- **T4 Wanted list + quality profiles (a light Sonarr/Radarr) [L]:** `wanted` rows from TMDB/TVDB
  ids (missing episodes of a monitored show, a movie you added from search); `quality_profiles`
  (allowed resolutions/sources/codecs, size bounds per minute, preferred/required/forbidden words,
  language, upgrade until); the **grab scheduler** runs Torznab searches on a cadence and matches
  RSS rule hits against wants; auto-grab above a score; upgrades replace files after import.
  Explicitly not: calendar/metadata features Sonarr has beyond this; users with a real Sonarr keep
  using the qBittorrent API.
- **T5 Seeding policy aware of the library [S]:** per-library seeding goals override the global
  ones for torrents that feed it; "stop seeding when watched by everyone" (opt) — done (library setting + hourly task).
- **T6 Torrent management in both clients [M]:** the native client already has it; the web client's
  torrents page (Track U) reaches parity for the common 90 % (list with live rates, add magnet/file/
  URL with preview, pause/resume/delete/recheck, files + priorities, peers, trackers, limits, queue,
  categories/tags, RSS feeds and rules, search), replacing `assets/webui`.

---

## Track V — Playback, encoding, transcoding (libmpv)

### V0. `corrent-mpv`: runtime-loaded libmpv [M]
- **Why runtime loading:** `libmpv2` 6.0.0 (May 2026) is a reasonable high-level binding but links
  at build time through `libmpv2-sys` (pkg-config), so every build machine and CI runner would need
  libmpv development files — the dev box doesn't have them — and the binary would refuse to start
  without the library. `libmpv-sys` 3.1.0 dates from 2020. With `libloading` 0.9 we `dlopen`
  `libmpv.so.2` / `libmpv-2.dll` / `libmpv.2.dylib` (Windows/macOS names **(unverified)**; a
  configurable path and a search list cover variants), resolve ~35 symbols, and degrade gracefully:
  "Playback needs mpv — install it / point to libmpv" in the UI, and the server simply has no
  probe/transcode until it's there. It also keeps Corrent's MIT/Apache license clean next to
  LGPL/GPL mpv builds (bundling an LGPL build in installers is the owner's call [O]).
- **How:** `ffi.rs` — hand-written declarations for the stable client API 2.x subset we use
  (`mpv_create`, `mpv_initialize`, `mpv_set_option_string`, `mpv_set_property`, `mpv_get_property`,
  `mpv_observe_property`, `mpv_command`, `mpv_command_async`, `mpv_command_node`, `mpv_wait_event`,
  `mpv_set_wakeup_callback`, `mpv_request_log_messages`, `mpv_terminate_destroy`,
  `mpv_free_node_contents`, `mpv_client_api_version`, `mpv_stream_cb_add_ro`,
  `mpv_render_context_{create,set_parameter,get_info,set_update_callback,update,render,
  report_swap,free}`) with the enum/struct values from `client.h`, `render.h`, `render_gl.h`,
  `stream_cb.h` (API 2.6 at the time of writing; all names verified against the headers, and
  `nm -D /usr/lib64/libmpv.so.2` exports all of them — 54 `mpv_*` symbols including every
  `mpv_render_context_*`, `mpv_stream_cb_add_ro`, `mpv_command_node(_async)` and
  `mpv_client_api_version`). A `bindgen`-once script is kept for regenerating against newer headers.
  Lifetime rules to encode in the wrapper's types: options before `mpv_initialize`; the render
  context is created *after* `mpv_initialize` and freed (`mpv_render_context_free`, GL context
  current) *before* `mpv_terminate_destroy`; render calls only on the thread that owns the GL
  context; the update/wakeup callbacks must not call back into mpv. `Mpv` (handle + options +
  typed properties + commands), `Events` (a thread calling `mpv_wait_event`, forwarding to a
  `tokio::sync::mpsc`; the wakeup callback also calls a user `Wake`), `RenderGl` and `RenderSw`
  (V1), `StreamCb` (custom `corrent://` protocols), `Encoder` (V3), `probe()` (L4), `Caps`
  (version, hwdec list, encoders via the `encoder-list` property — present in 0.41 together with
  `decoder-list`, `protocol-list`, `demuxer-lavf-list`, `hwdec-current`, `hwdec-interop` —
  protocols). `LC_NUMERIC` must be `C` (Rust never calls `setlocale`, documented anyway).
- **Accept:** unit tests that run only when a libmpv is found (`Mpv::available()`), skipping
  cleanly otherwise; `cargo run -p corrent-mpv --example probe <file>`; Linux CI installs the
  distro `libmpv` package so the tests actually run there [O for enabling].

### V1. Native playback embedded in eframe/glow [L] — **spike first (MS0)**
- **What:** video inside the egui window (not a child window), overlays drawn by egui, 60 fps,
  hwdec when available, audio-only mode for music, and a software fallback.
- **How (OpenGL path):** eframe 0.36.2's `CreationContext` gives `gl: Option<Arc<glow::Context>>`
  and `get_proc_address: Option<Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>>` (verified) —
  exactly what `mpv_opengl_init_params.get_proc_address` needs, and it is an `Arc`, so the render
  context can be created later than app construction (always on the main thread while eframe's
  context is current). Create `mpv` with `vo=libmpv`, `hwdec=auto-safe`, `gpu-api` default; create
  the render context with `MPV_RENDER_PARAM_API_TYPE="opengl"`, the init params,
  and **without** `MPV_RENDER_PARAM_ADVANCED_CONTROL` at first: with advanced control mpv blocks
  video playback until the app renders every frame in a timely manner, which eframe does not
  guarantee for minimized/occluded windows (then you must call `mpv_render_context_render` with
  `MPV_RENDER_PARAM_SKIP_RENDERING` yourself); the spike measures whether advanced control buys
  anything (DR/lower latency) before enabling it. The update callback (may run on any thread —
  verified) stores a flag and calls `egui::Context::request_repaint()` (thread-safe). Each frame,
  if the flag is set, call `mpv_render_context_update()` and, when `MPV_RENDER_UPDATE_FRAME` is
  reported, render into **our own** texture-backed FBO sized to the widget rect (mpv renders the
  whole FBO you hand it, so a sub-rect of egui's framebuffer isn't an option). **Where to render:**
  directly in `App::update` — eframe's glow backend makes the GL context current before calling
  the app (`Frame::gl()` is documented for "render things to offscreen buffers"), which avoids the
  paint-callback ordering question entirely; `egui_glow::CallbackFn` (`Fn(PaintCallbackInfo,
  &Painter)`, verified) remains an option but its `&Painter` is immutable, so
  `Painter::register_native_texture(&mut self)` **cannot be called from inside it**. Register the
  texture once with `eframe::Frame::register_native_glow_texture(glow::Texture) -> TextureId`
  (verified in eframe 0.36.2 `epi.rs`) and keep the *same GL texture name* for the player's
  lifetime: a resize re-allocates its storage with `tex_image_2d` and re-attaches it to the FBO, so
  the `TextureId` never changes and nothing has to be unregistered (eframe exposes no unregister).
  Then draw the texture as an ordinary `egui::Image` (so OSD, subtitles-by-egui, controls and
  transitions compose normally), `MPV_RENDER_PARAM_FLIP_Y` as needed. Save/restore GL state around
  the call (mpv expects defaults — verified; at minimum framebuffer, viewport, scissor, blend,
  program, VAO, active texture); `report_swap` is called at the start of the next frame
  (approximation; only affects mpv's timing stats). Audio-only: no render context (`vid=no`,
  `vo=null`), events still drive the UI (position, metadata, visualizer opt).
- **Software fallback:** `MPV_RENDER_API_TYPE="sw"` with `SW_SIZE/SW_FORMAT("rgb0")/SW_STRIDE/
  SW_POINTER` (verified) into a reusable buffer uploaded as an egui texture each frame. Costs a
  CPU copy (~8 MB per 1080p frame) — fine up to 1080p on desktops, and it is what the **first
  spike** uses because it has no GL interop risk. Chosen automatically when GL render context
  creation fails or `CORRENT_SW_VIDEO=1`.
- **Player UX:** full-window player view with auto-hiding controls (egui, motion tokens),
  seek bar with buffered/available ranges and chapter ticks, trickplay hover thumbnails, audio/
  subtitle track menus (`track-list`), subtitle delay/size, speed, A/V sync, frame step, screenshot,
  aspect/zoom, "Stats for nerds" (`demuxer-cache-*`, `video-params`, hwdec), picture-in-picture
  (viewport), keyboard map compatible with mpv's defaults where sensible, next/previous episode and
  auto-play countdown, "Skip intro" from chapters named like intros (opt), gapless album playback
  (`gapless-audio=yes`, `loadfile … append`), ReplayGain (`replaygain=track|album` from tags),
  lossless passthrough (native plays the original file: FLAC/ALAC/DTS/TrueHD via mpv; `audio-spdif`
  option for passthrough to receivers).
- **Audio completeness ("full audio support" in the request):** output device picker
  (`audio-device-list` / `audio-device`), exclusive/bit-perfect mode (`audio-exclusive`), channel
  layout policy (`audio-channels=auto-safe|stereo|5.1`, downmix in the decision engine for web
  clients), volume normalisation (`af=loudnorm`, opt), equalizer (`af=equalizer`/`superequalizer`,
  opt), crossfade (opt), lyrics (`.lrc` and embedded `USLT`/`LYRICS` tags via lofty → synced
  display, opt), podcasts (library kind exists; fetching uses the existing `feed-rs` RSS code with
  per-episode download = a plain HTTP task, not a torrent), internet radio stations (URL items,
  opt), album art from folder files (`cover.jpg`, `folder.jpg`) and embedded pictures, multi-disc
  albums, classical-music tags (composer/work/movement grouping, opt). The web music player gets
  gapless-ish prefetch, ReplayGain via `GainNode` and the Media Session API (V4c).
- **Accept (spike):** Big Buck Bunny plays inside the egui window in both paths; dragging another
  panel over it keeps 60 fps UI; CPU of the SW path measured at 1080p; a go/no-go note in
  `docs/spikes/mpv-embed.md`. **Accept (milestone):** kittest tests of the player UI with a fake
  `PlayerBackend`; manual run checklist on Linux/Windows [O for Windows/macOS].

### V2. Play-method decision engine [M]
- `corrent-transcode/src/decide.rs`: inputs are the item's `streams`, the **client profile**
  (sent at login/pairing and overridable in Settings: supported containers, video codecs with
  profiles/levels/HDR, audio codecs/channels, subtitle methods, max bitrate, HLS yes/no) and user
  prefs (bandwidth cap, "always transcode audio to stereo", preferred languages). Output:
  `DirectPlay { url }` (file served with Range), `DirectStream { hls_url }` (remux only — needs V4b),
  `Transcode { hls_url, video: Option<Ladder>, audio, subtitles: Burn|External|Embedded }`. Built-in
  profiles: web-chromium, web-safari, web-firefox, native (everything direct), tv-mode-native,
  chromecast (opt). Pure function, table-tested.

### V3. Server-side transcoding with libmpv [L] — **spike first (MS0)**
- **Capability assessment (from `DOCS/encoding.rst`, `client.h`, and spikes run on the dev box with
  mpv 0.41 / FFmpeg 9.0.1 on 2026-10-19):** mpv's encoding mode is driven by `--o`, `--of`,
  `--ofopts`, `--ovc`, `--ovcopts`, `--oac`, `--oacopts`, `--ocopyts`, `--orawts`, `--ofps`,
  `--oautofps`; the options must be set before `mpv_initialize`, so it works through libmpv
  (header text verified; the in-process libmpv path itself is still to be exercised by the
  worker). It re-encodes always: **"Direct stream copy" is listed under what does not work
  (verified)**, so remuxing (direct stream) cannot be done with mpv. **Verified by running it:**
  - `--of=help` lists `hls`, `segment`, `dash`, `mpegts`, `mp4`, `matroska`; `--ovc=help` lists
    `libx264`, `libx265`, `libopenh264`, `libvpx-vp9`, `libsvtav1`, `h264_nvenc`, `h264_vaapi`,
    `hevc_vaapi`, `av1_vaapi`, `h264_qsv`, `h264_amf`, `h264_vulkan`; `--oac=help` lists `aac`,
    `libopus`/`opus`, `flac`, `libmp3lame`.
  - **HLS/TS works:** `--o=DIR/index.m3u8 --of=hls --ofopts=hls_time=2,hls_list_size=0,
    hls_segment_filename=DIR/seg%03d.ts --ovc=libx264 --ovcopts=preset=veryfast,g=60 --oac=aac`
    on a 12 s 720p30 `av://lavfi:testsrc` input with `--start=4` gave 4 two-second segments and a
    VOD playlist in 0.7 s wall. Keyframe interval via `--ovcopts=g=` is honoured (exact 2.000 s
    segments).
  - **HLS/fMP4 works:** adding `hls_segment_type=fmp4,hls_fmp4_init_filename=init.mp4` and a
    `.m4s` segment pattern produced `init.mp4` + segments and a playlist with `#EXT-X-MAP`.
  - **Seek-by-restart stays on the original timeline:** mpv rebases output timestamps to 0, but
    `--ofopts=…,start_number=2,output_ts_offset=4` with `--start=4` emitted `seg002.ts…` with
    PTS ≈ 4 s (TS; the same options produced `seg002.m4s…` for fMP4). So a restarted encode at
    `n × segment_secs` can write segment `n` directly into the session directory.
  - **NVENC works in encode mode:** `--ovc=h264_nvenc --ovcopts=preset=p4,g=60,b=6M` encoded an 8 s
    1080p30 input to 4 HLS/TS segments in 1.26 s wall (RTX 5080). `hevc_nvenc`/`av1_nvenc` and
    `h264_videotoolbox` are expected to behave the same (system-memory frames) — macOS [O].
  - **Audio-only works:** `--no-video --of=mp4 --oac=aac` and `--of=webm --oac=libopus` from
    `av://lavfi:sine`. `--vf=scale=w=1280:h=720` works for the ladder.
  - `av://lavfi:` sources work in encode mode, so test media is generated, not downloaded.
  - **Still unverified:** `h264_vaapi`/`*_qsv` (need hardware frames: `--vf=format=vaapi` or
    `hwupload`; no VAAPI device on the dev box → [O] on an Intel/AMD machine), `--of=segment`,
    burn-in of subtitles in encode mode (`--sid`/`--sub-file`, should work: subtitles are rendered
    into the video by the VO filter chain), and `--of=hls` reading from an HTTP input (T2 path).
  There is no runtime seek in encode mode, so **seeking = stop the job and start a new one with
  `--start=<seconds>`** (and `--end` for bounded windows). Throttling a running encode isn't an mpv
  option, so the session manager bounds work with a forward window and stop/restart instead.
  Because the `mpv` CLI and libmpv share the encode code, the worker should support two
  interchangeable backends behind one option builder: **`MpvCli`** (exec `mpv --no-config …`
  when a binary is found — zero FFI risk, what the spikes used, immediately testable) and
  **`LibMpv`** (in-process in `corrent-mpvw` via `corrent-mpv`; default when the library is found,
  needed on machines that ship `libmpv` without the CLI).
- **Design:** `trait Transcoder { fn id(); async fn caps() -> Caps; async fn start(job: TranscodeJob)
  -> Box<dyn TranscodeHandle>; }` with `TranscodeJob { input: Path|Url, start_secs, video:
  Option<VideoSpec{codec, w, h, bitrate, keyint_secs}>, audio: AudioSpec{codec, channels, bitrate,
  track}, subtitle: Burn(track)|None, segment_secs, segment_format: Fmp4|Ts, out_dir }` and
  `TranscodeHandle { progress(), wait_segment(n), stop() }`. Backends: **`MpvEncoder`** (primary —
  spawns `corrent-mpvw encode --job <json>`; the worker sets the options above, `vo=null`,
  `ao=null`, `hwdec` per settings, `sid`/`sub-file` for burn-in, `af=lavrresample`/`aformat` for
  channel layout, `vf=scale=` for the ladder; reports produced segments on stdout), **`FfmpegCli`**
  (optional; only when an `ffmpeg` binary is found or configured; same job → `-ss … -c:v … -f hls`;
  also the only remux backend until V4b), and **`RemoteWorker`** (opt, later: a native client with
  a GPU offers its mpv encoder to the server over the API — this is the "libmpv on the client side
  for encoding" the owner mentioned, done safely).
- **HLS sessions (`hls.rs`):** the server owns the playlist, not the muxer: for a VOD of duration D it
  publishes a **virtual playlist** of ⌈D/4⌉ segments (`#EXT-X-PLAYLIST-TYPE:VOD`, `#EXT-X-INDEPENDENT-
  SEGMENTS`, fMP4 `#EXT-X-MAP` when applicable); a segment request that is already produced is served
  from `transcode/<session>/`; one inside the encoder's forward window (default 60 s) waits for it;
  one outside stops the encoder and restarts it at `n × 4 s`. Sessions die 30 s after the last
  request; completed transcodes stay in a bounded on-disk LRU and are reused by the same
  profile. Audio-only sessions (`--no-video`, `--of=mp4 --oac=aac` or `--of=webm --oac=libopus`)
  use the same path for the web music player.
- **Accept (spike, what remains after the CLI results above):** the same job run through
  **libmpv in-process** in `corrent-mpvw` (options before `mpv_initialize`, `MPV_EVENT_END_FILE`
  as completion, segment-ready detection by watching the playlist/dir); a 2-minute BBB clip
  (real decode, not `testsrc`) → HLS: segment durations within ±0.5 s, hls.js plays it, a mid-file
  restart's first segment appears within 2 s, CPU/time for libx264 `veryfast` vs `h264_nvenc`;
  burn-in of an ASS subtitle; numbers in `docs/spikes/mpv-hls.md`. The `--of=mpegts` + own
  segmenter fallback is **no longer needed** (the hls muxer works) and is dropped. **Accept (milestone):** web
  client plays a non-browser-friendly MKV via transcode, seeks across the file, switches audio
  tracks and burned subtitles; concurrency limit and cache eviction tests.

### V4. Direct play, direct stream, web player [M]
- **V4a Direct play:** authenticated Range-capable file serving (`tokio::fs` + `seek` +
  `ReaderStream`, `If-Range`/ETag, HEAD, multi-connection safe; the `stream.rs` session registry
  generalises to "playback sessions" shown in the server's Sessions page). Browser direct play
  for MP4/WebM (H.264/VP9/AV1 + AAC/Opus) and MP3/FLAC/Opus/AAC audio.
- **V4b Direct stream (remux) (opt, later):** a pure-Rust MKV → fMP4 remuxer for H.264/HEVC/AAC/
  AC-3/E-AC-3/Opus tracks to avoid re-encoding the most common case without ffmpeg — bounded but real
  work [L]; until then direct stream exists only with the ffmpeg backend.
- **V4c Web player:** `<video>` with native playback where possible; `hls.js` 1.7.3 (bundled,
  no CDN) for HLS in Chromium/Firefox, Safari's native HLS; WebVTT subtitle tracks; trickplay
  previews from the VTT sprite index; audio track switching by restarting the session; keyboard
  shortcuts; a persistent mini player for music with gapless-ish prefetch and ReplayGain through a
  `GainNode`; Media Session API for OS controls; casting (opt, Track E).

---

## Track U — Clients: "beautiful, customizable, fluid motion"

### U1. Design system shared by both clients [M]
- `design/tokens.json` is the source of truth: palette presets (dark/light/nord/solarized/high
  contrast — exported from today's `ui::theme::Palette`), accent colours, type scale (display/title/
  body/caption/mono), radii, spacing, elevation, motion durations/easings (≤ 120 ms micro, 240 ms
  page), focus ring. Rust: a test asserts `Palette::preset(..)` equals the tokens (or the presets are
  generated at build from the JSON with `build.rs`); web: a Vite plugin turns the same file into
  CSS custom properties. Custom themes (`theme.toml` today) export to the same shape and are served
  at `GET /api/v1/theme` so the web client follows the server admin's theme by default and users can
  override per device. "Customizable" beyond colours (the request asks for it): per-user **home
  layout** (which rows, their order, poster vs landscape, density), library view defaults
  (sort/group/card size), accent + font size + corner radius + motion intensity sliders in both
  clients, custom CSS for the web client (opt, admin-only, CSP-compatible via a served stylesheet)
  — all stored per user in the DB and exported with the theme.

### U2. Native client: media UI [L]
- New top-level mode switch in the sidebar (Media / Torrents / Settings); Home (continue watching,
  next up, recently added per library, hero backdrop with slow parallax), library grids (poster,
  landscape, square for music; sort/filter/group; letter index), detail pages (backdrop, logo,
  metadata, cast strip, seasons/episodes, tracks, versions, extras, "Find releases"), person/
  artist/album pages, playlists/collections, global search, per-user profiles (switch user with PIN
  for kids).
- **Virtualized grids:** `egui::ScrollArea::show_viewport` with manual row layout — only visible
  rows allocate widgets; stable per-item ids; 50k items scroll at 60 fps.
- **Image pipeline:** `ImageCache` (`src/app/media/images.rs`): requests keyed by (image id, size);
  fetch via `ServerLink` (local = file read) on a small thread pool, decode + downscale off-thread
  (`image` + `fast_image_resize`), upload as egui textures on the UI thread (≤ 2 ms budget per frame,
  queue the rest), LRU by bytes (default 256 MiB), blurhash placeholder → fade-in, cancellation when
  scrolled away, prefetch one screen ahead in scroll direction.
- **Motion:** a new `ui/motion.rs` (today there is no such module: animation is
  `Appearance.animations` plus `ctx.animate_bool_with_time`/`animate_value_with_time` calls spread
  over `ui/widgets.rs`, `app/sidebar.rs`, `app/details.rs`, `app/dnd.rs`, `app/titlebar.rs`,
  `app/settings_window.rs` — collect them behind one helper first): hover lift/scale on cards, shared-element feel for poster →
  detail (position/size tween), crossfade of hero backdrops, staggered grid fade-in on first load,
  page slide; all off when animations are disabled, no idle repaint loops (the STABILITY gate).
- **Accept:** kittest tests for every page with fixtures; frame p99 < 16 ms on the 50k fixture in a
  headless run; the torrent UI untouched.

### U3. 10-foot / TV mode [M]
- `--tv` or a setting: 1.5× scale, dense-free layouts, large focus ring, **spatial navigation**
  (own focus manager: nearest focusable in the pressed direction, remembered focus per row,
  wrap-safe), gamepad via `gilrs` (version **(unverified)**), keyboard arrows/Enter/Back, an
  on-screen keyboard, idle screensaver with backdrops, auto-fullscreen player, a reduced settings
  set. Runs on any mini-PC/HTPC with the same binary.

### U4. Web client [L]
- **Stack decision:** **Svelte 5 + Vite + TypeScript** (compiled, tiny runtime, fine-grained
  reactivity, first-class transitions/animations which this UI needs). Considered: Lit (no build
  step, but a large app in plain web components gets verbose), SolidJS (close second, similar
  profile), Rust/WASM Leptos (large bundles, slow builds, duplicates UI work we already do in egui,
  and TV browsers are weak). **Reproducibility:** `web/dist` is committed (a few hundred KB) and
  embedded with `rust-embed` 8.13 (feature `webui`, default on), so `cargo build` never needs Node;
  `scripts/web-build.sh` rebuilds it and CI (when Node is present) fails if the committed dist is
  stale. No CDN; fonts and hls.js bundled; CSP unchanged.
- **Pages:** login/pairing, home, libraries/grids (virtualized with `content-visibility` + a
  windowing list), detail pages, player (V4c), music mini player, torrents (T6 parity), RSS/search,
  wanted list, settings (server, users, devices, libraries, plugins, tasks, audit, logs),
  per-user settings and profile switch, PWA manifest (install on phones/TVs' browsers).
- **i18n:** the same catalogs: a Vite plugin reads `assets/i18n/*.toml` into JSON; web strings use
  the English text as key; `turkish_catalog_is_complete` scans `web/src` for `t("…")` too.
- **Accept:** Playwright smoke test (opt, needs Node in CI) or a Rust integration test that serves
  the embedded dist and checks the critical routes; Lighthouse-style budgets (first load < 300 KB
  JS gzipped, home interactive < 1 s on LAN).

---

## Track N — Network: LAN discovery, NAT, remote access

- **N1 LAN discovery [S]:** `mdns-sd` 0.21 advertises `_corrent._tcp` (TXT: `id`, `name`, `ver`,
  `fp`, `https`); the native client's "Connect" dialog lists servers and pre-fills the fingerprint.
- **N2 Port mapping (opt-in) [S]:** evaluate n0's **`portmapper`** crate first (the one iroh uses
  internally; UPnP-IGD + NAT-PMP + **PCP** in one maintained async crate, shared with N3 when both
  are on — version **(unverified)**); fallback pair `igd-next` 0.18 (UPnP-IGD, async) and `natpmp`
  0.5 (NAT-PMP, sync → `spawn_blocking`). Maps the HTTPS port with renewal; status in the Remote
  access page. librqbit keeps mapping the BitTorrent port itself (`enable_upnp_port_forwarding`).
- **N3 Hole punching with iroh [M]:** `iroh` 1.3 (`Endpoint`, `EndpointId` = public key,
  `EndpointAddr`, `RelayUrl`; relays by default are n0's with `RelayMode::Default`; self-hosting via
  the `iroh-relay` 1.3 crate — verified names). The server runs an `Endpoint` with ALPN `corrent/1`
  and accepts bi-streams on which it serves **the same axum router** with hyper: one HTTP/1.1
  connection per bi-stream, `tokio::io::join(recv, send)` (present in tokio 1.53) wrapped in
  `hyper_util::rt::TokioIo`, `hyper::server::conn::http1::Builder::serve_connection(io,
  TowerToHyperService::new(router))` with upgrades enabled for WS — so every API, WS and stream
  endpoint works unchanged. **Client side:** reqwest cannot dial iroh and neither can mpv, so the
  native client runs a **loopback bridge** (`127.0.0.1:<random port>`, one iroh bi-stream per
  accepted TCP connection, URLs prefixed with a per-run random path token exactly like
  `engine/stream.rs` does, so other local programs cannot ride the tunnel): reqwest, the WS client
  and mpv's HTTP demuxer all talk to the bridge with ordinary URLs. The bridge is also N4(d) for
  browsers for free. The link dials by the server's `EndpointId` from the pairing payload; direct
  when hole punching succeeds, relayed otherwise (end-to-end encrypted either way; relays see
  ciphertext). Costs: an extra ~3 MB binary, ~200 extra crates to compile (behind the `iroh`
  feature) and a background socket; off until "Remote access → Corrent Connect" is enabled. Local
  discovery via iroh is not used (mDNS covers it).
- **N4 Browsers remotely [S, docs]:** browsers can't speak iroh. Options offered in the Remote
  access page: (a) port mapping + ACME certificate when a domain exists (S4) [O], (b) your own
  reverse proxy, (c) Tailscale/WireGuard/ZeroTier — zero code, documented; (d) (opt, later) the
  native client as a local bridge (`http://127.0.0.1:<port>` → iroh) for the laptop case.
  Alternatives considered: embedding WireGuard (boringtun) — key distribution and TUN privileges make
  it heavier than iroh for no gain; a self-hosted HTTPS relay — requires a VPS and re-invents iroh's
  relay. Tailscale integration (tsnet) has no mature Rust binding.
- **N5 Addressing and pairing:** `ServerIdentity { id: iroh public key (also used when iroh is
  off), name, fingerprint }`; the client stores `servers.toml` with per-server tokens and routes
  (LAN addresses learned from mDNS, public address, iroh). Reconnect logic tries LAN → direct →
  iroh with backoff; banners show the path in use.
- **Accept:** two `correntd` instances on this machine connect over iroh (direct) in tests; a relayed
  test with the public relay behind an env flag; mDNS discovery test on loopback.

---

## Track X — Plugins

- **X1 Runtime decision [L]:** **WASM components on `wasmtime` 49** (component model, WIT interfaces
  with `wasmtime::component::bindgen!`, `wasmtime-wasi` for a minimal WASI p2 — clocks, random, no
  filesystem) for metadata/indexer/subtitle/notifier/task/parser plugins; **process plugins**
  (JSON-RPC 2.0 over stdio, same manifest) for heavy or native things (an ffmpeg wrapper, a Python
  scraper); **no dynamic libraries** (no stable Rust ABI, no isolation). `extism` 1.41 was
  considered: simpler SDKs in more languages, but a bytes/JSON ABI instead of typed interfaces and
  less control over capabilities; wasmtime + WIT gives typed, versioned interfaces, memory/fuel/
  epoch limits and per-call timeouts. Lua (as mpv has) was rejected for the server: weaker sandbox,
  another runtime.
- **X2 Host API (WIT, `crates/corrent-plugins/wit/`):** `corrent:host/http` (outbound requests only
  to hosts the manifest declares; host-side rate limits, caching, redaction of secrets in logs;
  **SSRF rules:** declared hosts that resolve to loopback, RFC1918/ULA/link-local/CGNAT or the
  server's own addresses are refused unless the admin ticks "allow local network" at install, the
  host resolves the name itself and pins the connection to the checked IP (`reqwest` `resolve()`)
  so DNS rebinding cannot bypass the check, redirects are re-checked, and `wasi:sockets` is never
  linked in — the plugin has no raw network),
  `kv` (per-plugin namespace, quota), `log`, `settings` (typed, secrets write-only), `events`
  (subscribe to torrent/library/playback events), `library` (read items, propose metadata — never
  direct DB writes), `tasks` (schedule). Worlds: `metadata-provider` (search, fetch, images,
  episodes), `indexer` (search releases), `subtitle-provider`, `notifier`, `scheduled-task`,
  `name-parser` (pre/post hooks around L3). A `web-extension` kind is just a manifest + static files
  mounted under `/plugins/<id>/` with an iframe-free slot system in the web client (opt).
- **X3 Manifest and lifecycle:** `plugin.toml { id, name, version, kinds, host_api = "1", permissions
  { hosts = [...], kv_quota, network = true }, settings = [ { key, type, secret, default, help } ] }`;
  install/enable/disable/upgrade/remove from both clients; **catalog**: a JSON index at a URL (default:
  a GitHub repo the owner creates [O]) with ed25519 signatures (`ed25519-dalek`) over archives;
  unsigned local installs allowed for development with a visible warning. Plugins run in a pool of
  `Store`s with fuel + epoch deadlines (default 30 s per call), memory cap 64 MiB, and a crash
  circuit-breaker (3 traps → disabled, toast).
- **X4 First-party plugins:** `plugins/*` are Rust crates compiled to `wasm32-wasip2` (toolchain
  target install needed: `rustup target add wasm32-wasip2` [O once]); built in CI and shipped
  inside the server binary (`include_bytes!`) so a fresh install has TMDB/TVDB/MusicBrainz/CAA/
  fanart/OpenSubtitles available once keys are entered. `cargo build` of the workspace does not
  require the wasm target: the embedded `.wasm` files are committed artifacts refreshed by a script,
  like `web/dist` (a CI job checks they are fresh).
- **Accept:** a `plugins/example-provider` used by tests; a malicious-plugin test (infinite loop,
  huge allocation, undeclared host) is contained; a plugin SDK doc with a 20-line example.

---

## Track Q — Performance architecture

- **Runtime layout:** one tokio multi-thread runtime (as today). Actors: engine (existing), **library
  writer** (one thread, batched transactions, coalesced watch-state updates), **scanner** (bounded
  pipeline, CPU work in `spawn_blocking` under a semaphore of `num_cpus`), **image workers**
  (`spawn_blocking`, semaphore `num_cpus/2`), **transcode sessions** (processes, limit configurable,
  default 2 video / 4 audio), **plugin pool** (`spawn_blocking` per call with deadlines), **event
  fan-out** (one task per WS client, bounded queues). No `std::sync::Mutex` across `.await`
  (existing lint), no unbounded channels for anything client-driven.
- **DB access pattern:** reads on the pool with prepared statement caches; the home screen is
  three indexed queries (continue watching, next up, recently added) plus one images batch — never
  N+1; counts cached per library with invalidation by the writer. Writes are queued; a scan's
  inserts are batched 500 per transaction.
- **Caching:** `moka` 0.12 (in the local cache; or an own LRU) for item DTOs and decisions;
  HTTP ETags everywhere; artwork immutable; the web client caches via service worker (opt). — done (`web/public/sw.js`: cached shell and hashed assets, network-first pages, never the API).
- **Serving:** Range reads with 256 KiB buffered chunks, `tokio::fs` on the blocking pool;
  per-connection write timeouts (STABILITY F5) and a global concurrent-stream cap; HLS segments
  served from disk with `immutable` caching per session.
- **Zero-regression for the torrent path:** `Snapshot` publishing stays as it is; the fan-out only
  reads `Arc`s; perf test `perf_5000_torrents` keeps its budgets (actor tick p99 < 20 ms, frame p99
  < 16 ms).
- **Budgets (tested by ignored perf tests like today's, results in `docs/PERFORMANCE.md`):** home
  API p95 < 100 ms and search < 50 ms on the 50k fixture; diff-path scan ≥ 10k files/min; thumbnail
  ≤ 15 ms; WS fan-out of 5k torrents at 2 Hz to 10 clients < 5 % of one core; native grid frame p99
  < 16 ms with 50k items and 400 visible images; `correntd` idle RSS < 150 MiB with a 50k library
  (excluding librqbit's per-torrent cost, measured separately); startup to "serving" < 2 s.

---

### Build cost and cargo features (the owner asked to watch disk and build time)

`incremental = false` is already set, so every workspace crate recompiles fully on each build; the
heavy third-party crates compile once per profile/feature set and are then cached in
`target/<profile>/deps`. What the new dependencies add (estimates; measure in MS1/MS2 and record
in `docs/PERFORMANCE.md`): `rusqlite bundled` compiles SQLite's C once (~1 min, ~60 MB of
artifacts); `wasmtime` + cranelift ≈ +150 crates, 5–8 min cold, 2–3 GB of debug artifacts;
`iroh` (quinn, pkarr, netwatch, …) ≈ +200 crates, 3–5 min, ~1.5 GB; `image` 0.25, `tower-http`,
`hyper-util`, `rustls` are already in `Cargo.lock` (642 crates today). Expect `Cargo.lock` to grow
to ~1,000 crates and `target/debug` to roughly double (6.5 GB → 13–15 GB) with everything on.
Rules for the implementation:

- Heavy, optional capabilities are **cargo features of `corrent-server`** and re-exported by the
  bins: `plugins` (wasmtime), `iroh`, `acme` (rustls-acme), `metrics`, `dlna`, `cast`, `openapi`
  (utoipa). `correntd` and the release workflow enable the full set; `corrent` (GUI) enables
  `plugins` (Local mode needs metadata providers) but **not** `iroh` (the GUI only dials; the
  server's listener side is `correntd`'s) — the client-side iroh bridge is its own small feature
  `iroh-client` in the root crate.
- Trim the heavy crates' own features: `wasmtime = { default-features = false, features =
  ["runtime", "cranelift", "component-model", "async", "std"] }` (no `wat`, `cache`, `profiling`,
  `coredump`, `addr2line`, `gc`, `threads`); `iroh = { default-features = false }` plus only the
  features the listener needs (exact names to confirm when adding, **(unverified)**); `image =
  { default-features = false, features = ["jpeg", "png", "webp"] }`; `rusqlite` features
  `["bundled"]` only (`modern_sqlite` comes with it).
- `cargo test --workspace` must stay complete with the **default** features of each crate, so the
  defaults are the full set *except* `iroh`, `acme`, `metrics`, `dlna`, `cast`, `openapi`
  (their tests run with `--features`), and the plugin host tests use a tiny committed
  `example-provider.wasm` (~100 KB) so no wasm toolchain is needed to test.
- One `target/` for the workspace (never a second `CARGO_TARGET_DIR`); no casual `--release`
  builds of the workspace during development (they double the artifacts) — the owner runs those.
  `cargo deny check` needs the licence allowlist reviewed when wasmtime/iroh land (both are
  Apache-2.0/MIT families already allowed; expect `BSD-3-Clause`, `Zlib`, `ISC` dependencies, all
  allowed; `Unicode-3.0` is allowed).
- Web client: `web/node_modules` (~200 MB) and `web/dist` build output are not needed for
  `cargo build`; `web/dist` is committed with `.gitattributes` `-diff` and kept ≤ 1 MB.

---

## Track E — Everything else a full server wants (pick-and-choose)

| Item | What | How | Size |
|---|---|---|---|
| E1 Scheduled tasks dashboard | Scan, refresh metadata, trickplay, cleanup caches, DB backup, plugin tasks; run/cancel; history | `tasks` table + `TaskRunner` with cron-like schedules; both clients | S |
| E2 Notifications/webhooks | Events → webhook/Discord/Telegram/ntfy/email; "episode available", "download done", "new device" | notifier plugins (X2) + a built-in generic webhook; per-user subscriptions | S |
| E3 Backups | Extend `backup.rs`: `VACUUM INTO` of corrent.db, server.toml, TLS, plugin settings; restore flow | existing zip format + manifest | S |
| E4 Metrics | `/metrics` Prometheus text (sessions, transcodes, scan rates, engine rates) | `metrics` + `metrics-exporter-prometheus` (versions unverified) behind a feature | S (opt) |
| E5 SyncPlay (watch together) | Rooms with a leader; play/pause/seek broadcast with clock offsets; chat opt | WS topic `room:<id>`; both players honour commands within ±100 ms | M (opt) |
| E6 Offline downloads / sync | Native client downloads originals or a transcoded copy for offline; watch-state merge on reconnect | job on the server, resumable Range downloads, local SQLite on the client | M (opt) |
| E7 DLNA/UPnP-AV | Expose libraries to TVs/receivers | evaluate reusing `librqbit-upnp-serve` 9.0.1 ("Simple UPnP MediaServer implementation", already a librqbit dependency) with a content-directory provider over the library; else `upnp-serve` from scratch | M (opt) |
| E8 Chromecast | Cast direct-play/HLS URLs | `rust_cast` 0.21; requires an HTTPS cert the device trusts or plain HTTP on the LAN; AirPlay out of scope (no Rust stack) | M (opt) |
| E9 Intro/credits skip | Chapter-based skip first; later audio-fingerprint detection across episodes — both done (`intro.rs` fingerprints, `detect_intros` task, segments in playback info and Jellyfin MediaSegments) | mpv `--af=lavfi=[…]` features or own fingerprint — later | L (opt) |
| E10 Live TV/DVR | — | **Out of scope** of this plan | — |
| E11 Multi-server in clients | Server list, switch, aggregate "continue watching" across servers | client-side only | S |
| E12 Jellyfin-compatible API subset | Lets existing Jellyfin apps (Android TV, iOS/tvOS, Roku, Kodi add-on) use Corrent | a separate router `/jellyfin/*` translating `System/Info/Public`, `Users/AuthenticateByName`, `Users/{id}/Views`, `Items`, `Items/{id}/PlaybackInfo`, `Videos/{id}/stream` + `master.m3u8`, `Items/{id}/Images`, `Sessions/Playing*`; decide after V3/V4 are stable; exact surface **(unverified)** — needs reading Jellyfin's OpenAPI | L (opt) |
| E13 i18n | Already there; API error codes + catalogs served to the web; Turkish for every new string (gate) | existing gates extended | ongoing |

### The "third client"

The owner names two clients. Recommendation, in order of value for effort:

1. **TV mode of the native client (U3)** — ships inside the same binary, covers HTPCs/mini-PCs on
   all three OSes, needs no new stack. Treat it as the third client for this plan.
2. **E12, a Jellyfin-compatible API subset** — the only realistic way onto smart TVs, phones, Roku
   and Kodi quickly, by borrowing their mature apps. Decide after playback is solid.
3. A **native mobile app** (egui on Android/iOS is possible but immature; a Flutter/Kotlin/Swift app
   is a separate project) — out of scope here; the PWA web client covers phones meanwhile.

---

## Milestones

Ordered so every milestone ends runnable and tested, the torrent client never regresses, and the
two risky unknowns (mpv inside eframe, mpv encode → HLS) are answered before anything depends on
them. Sizes are per the legend; milestones in the same row group can run in parallel.

| MS | Content | Depends | Size | Owner |
|---|---|---|---|---|
| MS0 | **Spikes** (time-boxed, each ends in a `docs/spikes/*.md` with numbers and go/no-go): (a) `corrent-mpv` runtime loading + `probe` on BBB/Sintel; (b) SW-render playback inside a scratch eframe window; (c) GL render via `get_proc_address` into an FBO texture registered with `Frame::register_native_glow_texture`; (d) mpv encode → HLS: **the CLI part is done** (TS + fMP4 segments, `g=` keyframes, restart-at-offset, NVENC, audio-only — see V3); what remains is the same job through libmpv in-process, a real BBB decode, burn-in, hls.js playback. The iroh echo spike moves to the start of MS12 (nothing before it depends on iroh) | – | S–M | VAAPI test [O] |
| MS1 | **Workspace split**: `corrent-i18n`, `corrent-proto`, `corrent-core` (egui-free engine, `Wake`, `log` ring buffer and `PaletteDef` moved, `Completed { hash }`), `corrent-server` (webapi moved); root crate unchanged in behaviour; CI matrix builds the workspace | – | M | – |
| MS2a | **Headless server + API v1 core**: `correntd` bin (CLI, portable, graceful shutdown, systemd notify), `corrent.db` with identity tables, argon2id users/roles, session + device tokens, pairing codes, self-signed TLS + LAN-only guard, `/api/v1/{auth,users,devices,server,torrents,rss,search}` incl. `server/fs`, audit log, qBittorrent API preserved under the new auth | MS1 | L | – |
| MS2b | **Realtime**: WS `/api/v1/events`, per-client deltas, compressed `full`, bounded queues, `EngineCommand::Detail` on demand, `EngineEvent` → `Event` mapping with request ids | MS2a | M | – |
| MS3 | **Native client ServerLink**: `LocalLink` (default) and `RemoteLink` (HTTPS+WS, pinning, delta → `Snapshot`), Connect/Pair dialogs, server list, offline banners, `server/fs` picker for remote paths, `LocalOnly` gating; the kittest seam kept | MS2b | L | – |
| MS4 | **Web client v1** (Svelte): login/pair, torrents parity (T6), RSS/search, users/devices/settings/audit; replaces `assets/webui`; `web/dist` embedded; i18n plugin | MS2a (REST), MS2b (live rates) | L | – |
| MS5 | **Library core**: schema + migrations, libraries/folders CRUD, scanner with watch, filename parser, lofty tags, mpv probe worker, artwork pipeline, FTS search, items API, watch state API; `.nfo` reader | MS1, MS0a | L | – |
| MS6 | **Plugin host + metadata providers**: wasmtime host, WIT worlds, manifest/permissions, first-party TMDB/TVDB/MusicBrainz/CAA/fanart as components, identify flow, settings UI for keys in both clients | MS5 | L | API keys [O] |
| MS7 | **Native media UI**: design tokens, home/grids/detail/person/album pages, virtualized grid, image cache, motion, global search, profiles/PIN | MS3, MS5 | L | – |
| MS8 | **Native playback**: `corrent-mpv` GL/SW render in the player view, full player UX, audio-only/gapless/ReplayGain, subtitles, chapters, trickplay hover, progress sync | MS0b/c, MS7 | L | Windows/macOS runs [O] |
| MS9 | **Server playback**: decision engine, Range direct play, HLS sessions with the mpv worker (+ ffmpeg backend when present), audio transcode, subtitle VTT conversion, trickplay generation task, web player (hls.js), Sessions page | MS0d, MS5 | L | – |
| MS10 | **Web media UI**: home/grids/detail/music/mini-player/playlists/collections/search/settings for libraries & plugins; PWA | MS4, MS9 | L | – |
| MS11 | **Torrent ↔ library**: category → library import modes, play-while-downloading from the library (native via stream_cb/HTTP, web via transcode of the stream URL), Torznab "Get this", wanted list + quality profiles + grab scheduler, seeding policy per library, library-aware delete guard | MS5, MS8/9 | L | – |
| MS12 | **Remote access**: iroh echo spike first (two in-process endpoints, direct + relayed), mDNS discovery, port mapping (`portmapper` or igd-next/natpmp), iroh listener serving the same router + client loopback bridge, pairing QR with fingerprint/relay, ACME (opt), Remote access settings page | MS3 | M | domain for ACME [O] |
| MS13 | **TV mode**: spatial navigation, gamepad, scale, OSK, screensaver, auto-fullscreen | MS7, MS8 | M | – |
| MS14 | **Services & packaging**: systemd unit, launchd plist, Windows service (`windows-service` crate — version unverified), AppImage/MSI/dmg of `correntd` + client, Docker image for `correntd` (opt), docs/SERVER.md | MS2a | M | signing, images [O] |
| MS15 | **Extras**: tasks dashboard (E1), notifications (E2), backups incl. DB (E3), multi-server (E11), SyncPlay (E5, opt), DLNA/Chromecast (E7/E8, opt), metrics (E4, opt), offline sync (E6, opt) | MS9, MS10 | S–M each | – |
| MS16 | **Jellyfin-compatible subset** (E12) — go/no-go after MS9 | MS9 | L (opt) | – |
| MS17 | **Hardening pass** for the server in the `docs/STABILITY.md` style: failure inventory (writer stalls, worker zombies, disk full in caches, plugin runaway), fuzzing of the filename parser/VTT converter/HLS playlist writer/API inputs, 72 h soak with scans + transcodes + 5k torrents, release bar | all | L | soak machine [O] |

Deviations are recorded under each milestone as they happen (like `docs/ROADMAP.md`).

### Status and deviations (updated as milestones land)

| MS | Status | Notes |
|---|---|---|
| MS0 | Done | `docs/spikes/mpv-embed.md` (GL + SW: go), `docs/spikes/mpv-hls.md` (go). The libmpv **in-process** encode path *is* exercised: `corrent-mpv` tests encode MKV/FLAC/HLS through libmpv (not only the CLI). The GL spike renders inside the `egui_glow` paint callback (works; the framebuffer binding must be restored by the callback). |
| MS1 | Done | Deviation: the i18n catalog stays in `assets/i18n/` (the web client reads it too). `InfoHash`/`TorrentState` live in `corrent-proto`; the GUI keeps `crate::engine` etc. through `use corrent_core::{…}` at the crate root. |
| MS2a | Done | Deviation: a separate `corrent-db` crate owns `corrent.db` (writer thread, read pool, migrations) for identity *and* library — one migration sequence. Torrent ownership is `added_by` in the engine's store (not a table). Fable review findings fixed (path confinement, passkey redaction, timing, limits). |
| MS2b | Done | Deltas use `upserted/patched/removed`; frames > 32 KiB zlib-compressed binary. |
| MS4 | Done | Svelte 5 web client (`web/`, embedded `web/dist`): sign-in/pairing, torrents parity, RSS, search, users/devices/settings/audit, Turkish catalog; replaced `assets/webui`. |
| MS10 | Done | Web media UI: home rows, virtual grids, item pages, music player + queue, search, player (hls.js), settings for libraries, plugins, tasks; playlists and collections (added late: `lists.rs`, Playlists pages in both clients); PWA manifest + service worker. |
| MS9 | Done | `corrent-transcode` (decision engine, HLS sessions on libmpv encode, NVENC/QSV/x264 detection, VTT, trickplay) + `/api/v1/playback`, `/hls`. Deviation: no ffmpeg backend (owner question 3 open; mpv re-encodes video for container-only mismatches, cheap with NVENC). The web player lands with the web client. |
| MS11 | Done | Category import (in place / hardlink / copy) on completion, library seeding goals, delete guard, releases + grab for items, wanted list + quality profiles + scheduler, play-while-downloading through the engine's stream (direct and as transcoder input). Show monitoring added later: "Follow new episodes" (`PUT/DELETE /items/{id}/monitor`, `GET /monitored`, task `monitor_shows`) puts aired episodes the library lacks on the wanted list. Torrents added any other way (RSS rules, by hand, Sonarr) that fit a wanted entry mark it grabbed (`integrate::claim_wanted`). |
| MS6 | Done (live-service checks need keys [O]) | `corrent-plugins` (wasmtime 49 components, WIT v1: metadata/indexer/notifier worlds, sandboxed host API), built-ins TMDB + MusicBrainz/CAA (+ example for tests), automatic identification after scans, manual identify, plugin management API. Added later: subtitle-provider, name-parser and scheduled-task worlds; process plugins (JSON-RPC over stdio); TheTVDB, fanart.tv (`supplements`: artwork only) and OpenSubtitles built in; subtitle search/download API; signed catalog (`plugin_catalog`, `plugin_keys`, `correntd plugin keygen|sign`); `docs/PLUGINS.md`. Keys for TMDB/TVDB/fanart/OpenSubtitles [O]; the new providers are untested against the live services without keys [O]; the catalog repository [O]. Added: `event-handler` world (`events.on-event`, every notification event) and a read-only `library` host interface behind a manifest permission (`example-events` test plugin). Web plugins: `runtime = "web"` folders whose module the web client imports (`window.corrent`: item actions, settings pages, API, toasts). |
| MS3 | Done | Deviation: no `ServerLink` trait. `EngineHandle::with_backend` lets a remote backend (`src/link/remote.rs`) answer the same `EngineCommand`s and publish the same `Snapshot`s, so the torrent UI (and its kittest seam through `app.snap`) is untouched. WS deltas → mirror → `Snapshot`; commands → REST (new `torrents/preview`, `torrents/queue`, `server/ca`); pairing pins the CA by fingerprint (`src/link/pair.rs`); `servers.toml` (0600) holds the server list. Local-only features are hidden in remote mode; the server's own settings stay in its web interface (the GUI settings window says so). Folder pickers (add dialog, move, category, library) browse the server (`server_fs.rs`). Changes made while the server is unreachable are queued (up to 200) and sent after reconnecting. |
| MS7/MS8 | Done | Native media UI on the API (in-process server in local mode, the paired server in remote mode): home (hero, rows), virtualized library grids, item pages (seasons/episodes, cast, tracks), search, image cache with blurhash, embedded mpv player (GL render, immersive video, mini-player for audio, tracks, chapters, resume). Added since: person pages (cast/crew links, people in search), profiles with PINs (`/auth/profiles`, `/auth/profile`, `/auth/pin`, `/users/{id}/pin`; PIN pad that works with a remote), trickplay thumbnails on the seek bar, accessible card labels, a kittest test driving the media pages and TV focus against the in-process server. `ui/motion.rs` routes every animation (off with Appearance → Animations). A 50,000-item library grid scrolls at frame p50 0.15 ms / p99 0.6 ms (release, `perf_50k_grid`). Audio: a Playback settings page (output device from mpv, exclusive mode, channel layout, Dolby/DTS passthrough, EBU R128 loudness, equalizer presets, ReplayGain, gapless) and lyrics (`.lrc`/`.txt` sidecars and tags, synced display in the web and desktop players). Podcasts: a `podcasts` library kind and feed subscriptions downloading episodes (hourly task, keep newest N). Internet radio: stations on the server, relayed streams (`.pls`/`.m3u` followed), web Radio page and a desktop Radio page. Classical grouping: the `work` tag groups an album's movements under work headings (web). Player extras: playback settings (speed, subtitle and audio delay, subtitle size, aspect/fill, screenshot to Pictures/Corrent), stats for nerds (`i`), mpv-style keys (`[ ] , . z x s n p`), next-episode countdown with cancel, picture in picture (small always-on-top window). Not done: crossfade (mpv plays one file at a time). |
| MS12 | Done | LAN discovery (`discovery.rs`, mDNS `_corrent._tcp`, listed in the Connect dialog), Corrent Connect (`p2p.rs`: router over iroh streams, loopback bridge on the client, route fallback in the app; spike `docs/spikes/iroh-connect.md`), port mapping of the HTTPS port with n0's `portmapper` (UPnP/NAT-PMP/PCP), `GET/PUT /api/v1/server/remote`, Connect id in pairing links and `server/info`. Web client: Remote access and Plugins settings pages, pairing QR (server-rendered SVG). The Connect dialog in the app lists servers found on the LAN. Self-hosted relay: `connect_relay_url` (an `iroh-relay` you run) replaces n0's relays for the server. ACME [O]: `acme_domains` in `server.toml` gets a Let's Encrypt certificate over TLS-ALPN-01 on the HTTPS port (`acme.rs`, `rustls-acme`, feature `acme`, on in `correntd`); a resolver serves it only to connections asking for those names, so LAN addresses and Connect keep the own CA (pairing links made at an ACME name carry a LAN address, since apps pin the own CA); failed orders pause 15–60 min (LE rate limits). Unit-tested (handshakes against the resolver); never run against a real domain. Fable review fixes for MS3/MS7 landed with it (mpv always verifies TLS, unpinnable CA refused, server ids validated and entries keyed by id+address, the desktop's hidden owner can be claimed by setup, non-blocking engine switch, scoped undo, WS read deadline). |
| MS14 | Done | systemd unit (+ sysusers, user unit), launchd agent, Windows service mode (`correntd service install/uninstall/run`, type-checked for `x86_64-pc-windows-gnu`, never run [O]), Docker image + compose, `scripts/package-server.sh`, server zip on Windows, `corrent-mpvw` in every package, release workflow uploads server artifacts. Deviation: no MSI/dmg for the server (archives + service files); libmpv isn't bundled. |
| MS13 | Done | `--tv` / ⋮ → TV mode: media pages only, full screen, 1.5× the user's zoom; egui's spatial focus drives arrows and Enter (focus ring on cards, focused row scrolled into view; a row's horizontal scroll area swallows vertical scroll requests, so the page scrolls from the remembered rect); gamepads via `gilrs` 0.11 (feature `gamepad`, needs libudev on Linux) become key events in `raw_input_hook` (D-pad/stick with repeat, A = Enter/Space in the player, B = Back, Y = keyboard); Back = Escape/Backspace; on-screen keyboard for search; screensaver after 5 min (home backdrops, Ken Burns, clock). A reduced settings page (top bar → Settings): interface size (125–200 %), screensaver delay or never, theme, language, switch profile, leave TV mode (`appearance.tv_scale`, `tv_screensaver_min`). Debug hook `CORRENT_DEV_KEYS`. |
| MS15 | Done | E1 tasks (`tasks.rs`: runner with intervals, history, run-now; replaces the separate wanted scheduler and correntd's rescan loop), E2 notifications (`notify.rs`: ntfy/Discord/webhook/notifier plugins, per-event subscriptions, test button), E3 backups (`backups.rs`: `VACUUM INTO` + config/keys zip, keep 7, download), E4 metrics (`/metrics`, hand-written Prometheus text, no new crates), E11 multi-server (the app's server list, MS3). Web pages: Tasks and backups, Notifications. E5 SyncPlay (`v1/syncplay.rs`: rooms of up to 20 people, play/pause/seek with server-clock positions; the web player follows over the `room` WebSocket topic and corrects drift over 0.75 s, the native player polls each second and corrects drift over 1 s; no chat). E7 DLNA (`dlna/`: own SSDP + ContentDirectory instead of librqbit-upnp-serve, which lists video only; all item kinds with artwork, signed media URLs, LAN only, one shared account, toggle in Remote access). E8 Chromecast (`cast/`: own Cast v2 client — protobuf framing by hand, rustls — instead of rust_cast; mDNS discovery, a `chromecast` direct-play profile, signed direct/HLS links on the DLNA listener, web and native cast menus; tested against a fake receiver, not yet a real device). E6 offline copies (`src/app/media/offline.rs`, `downloads.rs`: originals by resumable Range download, a smaller copy from the 720p HLS transcode joined into one TS file, a Downloads page, local playback, offline progress queue with a newer-on-server-wins merge; no server-side job needed). Restore from the UI: stage a listed or uploaded backup, applied at the next start with the old files kept aside. |
| MS16 | Done (untested with real apps [O]) | `jellyfin/` (plan and review: `docs/JELLYFIN-PLAN.md`): handshake as "Jellyfin Server 12.2.0", sign-in with every Jellyfin token style, devices per app id, users/configuration/display preferences, views, `/Items` translation (case-insensitive paths and keys), item pages, latest/resume/next up, seasons/episodes, artwork by tag, PlaybackInfo from DeviceProfiles, direct streams, lazy HLS, WebVTT subtitles, playstate, `/socket` keep-alive, UDP 7359 discovery, plain-HTTP LAN listener on 8096, Remote access toggle. Tests drive it the way Jellyfin Web, the Kotlin SDK and Kodi do. Not done: dashboard, playlists/collections, live TV, trickplay tiles, progressive transcodes, real-app verification [O]. |
| MS17 | Done (72 h soak open [O]) | `docs/SERVER-STABILITY.md`: failure inventory with guards and proofs; `corrent-fuzz` (seeded, dependency-free; no nightly for cargo-fuzz) drives robustness tests for the name/NFO parsers, subtitle converters, torrent deltas (property test) and API bodies; `scripts/soak.sh` (3-minute run clean, RSS flat). Found and fixed: subtitle timestamp overflow, previews of malformed `.torrent` bytes not failing fast; transcodes refuse to start below 2 GiB free (`NoSpace`, 507). |
| MS5 | Done | `corrent-library` + `/api/v1` library routes; watching + 6-hourly rescans in `correntd`. ffprobe backend added later (`probe.rs`, used without libmpv), trickplay (moves to MS9 with the encoder), metadata providers (MS6). STABILITY S1 was not done first: the library uses `corrent-db`'s writer, which is what S1's F1 writer pattern was needed for; S1 stays on the stability roadmap. |

**Relationship to `docs/STABILITY.md`:** CLAUDE.md names STABILITY S1 (profile data-loss guards,
incl. the `corrent-writer` thread from F1) as the next hardening step. Do **MS1 first** (it is a
mechanical move and touches the same files S1 will edit), then **STABILITY S1 inside
`corrent-core` before MS5**: the library's writer thread (L1) and the scanner's batching reuse
the F1 writer pattern, and the server inherits D1–D4's lock/atomic-write rules for `corrent.db`
and `server.toml`. F5/F6 (stream write timeout, HTTP timeouts) land with V4a/S6 as the plan says.

### Testing and driving on Linux (replaces the Windows-only notes while the dev box is Fedora)

- Everything still runs `--portable` with data under `target/debug/corrent-data`; `correntd
  --portable --data-dir target/debug/correntd-data` for the server.
- Headless UI tests stay egui_kittest (behaviour + AccessKit tree). For visual checks on Linux add a
  debug hook `CORRENT_DEV_SHOT=<png>` that sends `egui::ViewportCommand::Screenshot` (egui 0.36.2 has
  it; the image arrives as `Event::Screenshot`) and writes the PNG — the PrintWindow equivalent, no
  input automation. Playback checks use `CORRENT_DEV_PLAY=<file>` and the SW path in a VM-safe way.
- API tests are in-process (`oneshot`); HLS tests run the mpv worker if libmpv is present and skip
  otherwise; iroh tests use two in-process endpoints.
- Legal media: Big Buck Bunny, Sintel, Tears of Steel (CC-BY) under a scratch folder; audio test
  files generated with mpv's encode mode from `av://lavfi:sine=…` / `testsrc` inputs (verified:
  `av://lavfi:` works as an encode-mode source, a 12 s 720p clip takes < 1 s to make) — never the
  owner's real library during tests; generated media lives in the scratchpad and large downloads
  are deleted afterwards.
- CI: Linux job installs `libmpv2`/`libmpv-dev` and `mpv` so mpv-dependent tests run there [O to
  push]; Windows/macOS jobs skip them until the owner provides runners with mpv.

---

## Decisions

| Decision | Choice | Alternatives | Why |
|---|---|---|---|
| Process model | Server library embedded in the GUI (Local) **or** headless `correntd`; same code | Always separate processes; GUI-only | No regression for today's users; one API surface; tests stay in-process |
| Repo layout | Cargo workspace, root stays the `corrent` GUI crate, new crates under `crates/` | One big crate; a new repo | Incremental `git mv`, each commit green, clear dependency direction (proto ← core ← server ← bins) |
| Wire format | JSON REST + WebSocket, serde types in `corrent-proto` | gRPC/protobuf; GraphQL; CBOR/msgpack | Browser-native, debuggable, egui client doesn't care; binary framing can be added per-topic later |
| Realtime | WebSocket with per-client snapshot diffs (`TorrentPatch`) | SSE; JSON Patch; polling `sync/maindata` style | Bidirectional (subscriptions, detail requests), compact, 5k torrents at 2 Hz is cheap |
| Database | `rusqlite` 0.40 `bundled`, WAL, writer thread + read pool, SQL migrations | `sqlx` 0.9; `redb`/`sled`; Postgres | Embedded, synchronous core matches the actor design, FTS5/JSON1 built in; no build-time DB |
| libmpv binding | Own `corrent-mpv`: `libloading` + hand-written FFI subset | `libmpv2` 6.0 (build-time link); `libmpv-sys` 3.1 (2020) | Builds without headers anywhere, graceful absence, license-clean dynamic loading, exact control of render/stream_cb/encode |
| Native video | mpv OpenGL render API into an FBO texture drawn by egui (`get_proc_address` from eframe), SW render API fallback | Child window/HWND; `wgpu` interop; separate mpv process window | Composable overlays, one window, verified API surface; mpv has no Vulkan render API and Corrent is on glow anyway |
| Transcoding | mpv encode mode in a worker process (primary; `LibMpv` in-process in the worker, `MpvCli` exec when only the binary exists); ffmpeg CLI optional; remux-in-Rust later (opt) | ffmpeg only; GStreamer; libmpv encode inside the server process | Owner's choice honoured; HLS/TS+fMP4, NVENC and restart-at-offset verified with mpv 0.41; process isolation; mpv can't stream-copy so remux needs something else |
| HLS delivery | Server-owned virtual VOD playlist + segment files from the encoder; stop/restart at seek | Let the muxer own the playlist; MPEG-DASH; WebRTC | Seeking across unencoded ranges, cache reuse, player-agnostic; DASH adds nothing for this |
| Web stack | Svelte 5 + Vite + TS, `web/dist` committed and embedded | Lit; SolidJS; Leptos/WASM; keep vanilla | Fluid UI with a tiny runtime; `cargo build` stays Node-free; reproducible via CI freshness check |
| Theming | `design/tokens.json` shared by Rust presets and CSS variables | Separate themes per client | One look, one editor (today's theme editor exports both) |
| Auth | argon2id + opaque hashed tokens (session/device), cookie+CSRF for web, Bearer for apps | JWT; OAuth/OIDC; qBittorrent-style only | Revocation per device, simple, no key management; OIDC can be added as a plugin later |
| TLS | rcgen self-signed CA + SPKI pinning in native clients; `rustls-acme` optional | Plain HTTP; mandatory ACME; mkcert-style user CA | Secure by default on LANs without a domain; browsers get a clear trust flow |
| NAT traversal | iroh 1.3 (n0 relays by default, `iroh-relay` self-host option) for native/TV clients; port mapping + mDNS; Tailscale documented | Self-hosted HTTPS relay; embedded WireGuard; reverse SSH; only port forwarding | Real hole punching with relay fallback, Rust, key-based identity doubles as server id; browsers need a public port or a VPN regardless |
| Plugins | wasmtime 49 components + WIT typed host API; process plugins for heavy/native | extism; dynamic libraries; Lua/`mlua`; only process plugins | Typed, versioned, sandboxed with limits; one SDK for first-party and third-party; process plugins cover what WASM can't |
| First-party providers | Shipped as WASM components built in-repo | Built into the server as Rust modules | Dogfoods the plugin API; keys/permissions handled uniformly; replaceable by users |
| Image pipeline | `image` 0.25 + `fast_image_resize` 6.1, JPEG q85 sizes + blurhash, content-addressed | WebP/AVIF outputs; on-the-fly resizing | Universal decode (TV browsers, egui), SIMD resize, immutable caching |
| Third client | TV mode of the native client now; Jellyfin-compatible subset later (opt) | Native mobile apps; Kodi add-on | Same binary; borrowing mature TV/mobile apps is the fastest reach |

---

## Risks and mitigations

1. **mpv render inside eframe's GL context** — state leakage, FBO/texture ownership, HiDPI, Wayland
   vs X11 proc loaders. *Mitigation:* MS0 spikes both paths; SW render API is a complete fallback;
   GL state is saved/restored around the call; a `CORRENT_SW_VIDEO=1` escape hatch.
2. **mpv encode mode for live HLS** — segmenting, keyframe alignment, seek-restart and NVENC are
   now verified with the CLI (V3); open: the in-process libmpv path, VAAPI/QSV, burn-in, HTTP
   inputs, and "always re-encode" (no stream copy). *Mitigation:* the worker's `MpvCli` backend is
   a complete fallback for the libmpv path; the ffmpeg backend when installed; direct play is
   preferred by the decision engine so most LAN playback never transcodes; remux-in-Rust is
   scheduled as optional.
3. **libmpv availability and ABI** — users without mpv; name/ABI differences per OS; GPL builds.
   *Mitigation:* graceful absence with guidance; configurable library path; API version check at
   load; owner decides about bundling an LGPL build [O].
4. **Scope** — this is several products. *Mitigation:* (opt) items can be dropped; milestones end
   runnable; the torrent client is never gated on media work; one Fable review per milestone as the
   repo rules require.
5. **Torrent client regression during the crate split** — path churn, feature unification, test
   build times. *Mitigation:* shims, `git mv`, the full test suite at each step, workspace-level
   dependency pinning, kittest features kept minimal.
6. **Multi-user semantics on a single-user engine** — detail target, undo, toasts assume one
   viewer. *Mitigation:* `Detail` on demand, per-session toasts routed by `Event` with `user`/`device`
   scope, undo scoped to the remover.
7. **iroh maturity and relay dependence** — API churn (1.x is stable now), relay availability.
   *Mitigation:* iroh is one `ServerLink` route among three; self-hosted relay option; everything
   also works on LAN without it.
8. **WASM toolchain** — `wasm32-wasip2` target and wit-bindgen versions moving. *Mitigation:*
   pin versions in the workspace; committed `.wasm` artifacts so `cargo build` never needs the target.
9. **SQLite under heavy scans + playback** — writer contention, WAL growth. *Mitigation:* single
   writer with batching, `wal_autocheckpoint`, read pool, budgets tested on the 50k fixture.
10. **Web client build reproducibility** — committed `dist` drifting from source. *Mitigation:* CI
    freshness check, `scripts/web-build.sh` pins Node/npm versions via `package.json` engines.
11. **Security surface grows** (auth, plugins, remote). *Mitigation:* LAN-only default, pinning,
    rate limits, audit log, plugin sandboxing, `docs/SECURITY.md` threat model, a security-focused
    Fable review before enabling remote access by default.
12. **i18n volume** — hundreds of new strings in Rust and TypeScript. *Mitigation:* gates extended
    to the workspace and `web/src`; Turkish entries land with each feature, never batched later.
13. **Owner-gated steps** (API keys, signing, pushing CI, other OSes, nvenc hardware) can stall
    milestones. *Mitigation:* every such step is marked [O] and has a stub/fake for tests.

## Open questions for the owner (only the ones that change the plan)

1. **Third client:** OK with "TV mode of the native client now, Jellyfin-compatible API later
   (opt)", or is a native mobile app required in this plan?
2. **Browsers from outside the LAN:** accept "port mapping + ACME (your domain) or a VPN like
   Tailscale" for the web client, with iroh reserved for native clients?
3. **Transcoding fallbacks:** mpv is primary as asked; is an optional `ffmpeg` CLI backend
   acceptable when it is installed (needed for remux/direct stream until a Rust remuxer exists)?
4. **libmpv distribution:** rely on the user's mpv install (dlopen) or bundle an LGPL libmpv build in
   the Windows/macOS installers (license notice, bigger downloads)?
5. **Binary names:** `corrent` (client with embedded server) and `correntd` (headless) — fine?
6. **Web stack:** Svelte 5 + Vite + TypeScript with the built `web/dist` committed — fine? (Node is
   needed only by people changing the web client.)
7. **Order vs. hardening:** MS1 (workspace split) → STABILITY S1 → MS2a, as proposed above, or
   STABILITY S1 strictly first as CLAUDE.md's "next steps" reads?
8. **Transcode worker backends:** OK that the worker execs the `mpv` binary when it is present and
   uses libmpv in-process otherwise (same options, same output), or must it be libmpv only?

---

## First implementation slice (start right after review)

Goal: a headless server serving today's engine over an authenticated v1 API with realtime deltas,
the native client unchanged for users, and the libmpv crate with its spikes — all in commit-sized
steps, each leaving `cargo fmt --check && cargo clippy --all-targets -D warnings && cargo test`
green (and `cargo deny check` once new dependencies are in).

1. **Workspace skeleton** — `[workspace]` in `Cargo.toml` with `members = [".", "crates/*"]`,
   `[workspace.dependencies]` for shared pins, profiles and `[patch.crates-io]` at workspace level.
   *Commit:* "Workspace: root crate becomes a workspace member".
2. **`crates/corrent-i18n`** — `git mv src/i18n.rs` and `assets/i18n/`; root uses
   `#[macro_use] extern crate corrent_i18n;`; the two gate tests scan a list of crate roots.
   *Commit.*
3. **`crates/corrent-proto`** — `InfoHash` (moved), `TorrentState`, `Torrent`, `TorrentPatch`,
   `SessionStats`, `Categories`, `Event` (`#[serde(tag = "t")]`), `ApiError`, auth DTOs; round-trip
   tests. `engine::types` re-exports `InfoHash`. *Commit.*
4. **`crates/corrent-core`** — `git mv` of `engine`, `store`, `config`, `rss`, `search`, `import`,
   `backup`, `migrate`, `crash`, `util`, `updates`, plus `app/log.rs` → `core::log` (crash.rs reads
   its `LINES`) and `ui::theme::PaletteDef` → `config::PaletteDef` (re-exported by `ui::theme`);
   `EngineHandle::start(rt, paths, settings, wake: Wake, events)` with the wake stored for
   `restart`; every `ctx.request_repaint()` in `actor.rs` becomes `(self.wake)()`; e2e tests pass a
   no-op wake; `EngineEvent::Completed` gains `hash` (and `category`); root crate gets
   `pub use corrent_core::{engine, store, config, …};` shims. *Commit:* all existing tests green.
5. **`crates/corrent-server` (move)** — `src/webapi` → `corrent-server/src/{qbt,auth,lib}.rs`,
   `WebUi` static files served as before; root app unchanged. *Commit.*
6. **Identity DB** — `corrent-server/src/db/` (rusqlite bundled, WAL, writer thread, read pool,
   `migrations/0001_identity.sql`), `users/sessions/devices/pairing_codes/audit`, argon2id, token
   issue/verify/revoke, migration of the qBittorrent admin password; unit tests. *Commit.*
7. **API v1 core** — `v1/{auth,users,devices,server,torrents,rss,search}.rs` over `EngineHandle`
   (reusing `qbt.rs` helpers), `ApiError`, `Auth` extractor, CSRF/origin middleware reused, LAN-only
   guard; `oneshot` tests per route family. *Commits per family.*
8. **Events** — `v1/events.rs` WS endpoint (axum `ws` → tokio-tungstenite 0.29), `Notify`-driven
   fan-out, per-client diff into `torrents.full/delta` (`full` deflate-compressed), bounded queues;
   `EngineCommand::Detail { hash, reply }`; `EngineEvent` → `Event` mapping; delta round-trip test
   with 5k synthetic views. *Commit.* (= MS2b)
9. **TLS + `correntd`** — rcgen self-signed CA/leaf with fingerprint, `axum-server` rustls
   listener (plain HTTP on loopback), `crates/correntd` with `--portable --data-dir --bind --init-admin
   --print-fingerprint`, graceful shutdown, logging as in `main.rs`, `docs/SERVER.md`. *Commit.*
   → **MS2 done**: `correntd --portable` serves `https://localhost:8443/api/v1/…` and the old
   `/api/v2` for Sonarr/Radarr.
10. **`crates/corrent-mpv` (V0)** — `ffi.rs` subset, `lib.rs` loader + `Mpv` + events thread + typed
    properties, `probe()`; `examples/probe.rs`; tests skip without libmpv. *Commit.*
11. **Spike MS0b/MS0c** — `examples/egui_player.rs` (eframe scratch window) with SW render → texture,
    then the GL path: `cc.get_proc_address` for `mpv_opengl_init_params`, an FBO-backed texture
    registered once with `frame.register_native_glow_texture`, rendering in `App::update` (GL is
    current there), GL state saved/restored; with and without `ADVANCED_CONTROL`; test minimize/
    occlusion and a resize; write `docs/spikes/mpv-embed.md` with fps/CPU numbers and the go/no-go.
    *Commit.*
12. **Spike MS0d (remaining half)** — `corrent-mpvw encode --job <json>` through libmpv in-process
    (and the `MpvCli` twin) → HLS dir from a BBB clip; measure segment durations, restart-at-offset
    latency, CPU for libx264 vs `h264_nvenc`, burn-in; confirm hls.js plays the server-owned
    playlist; `docs/spikes/mpv-hls.md` (fold in the CLI results from V3). *Commit.*

Then MS3 (`ServerLink`) so the native client can talk to that server, and MS5 (library core) in
parallel. The iroh echo spike (`examples/iroh_echo.rs`, feature `iroh`) opens MS12.

---

## Verification notes (2026-10-19)

**Checked against sources:** eframe 0.36.2 `CreationContext { gl: Option<Arc<glow::Context>>,
get_proc_address: Option<Arc<dyn Fn(&CStr) -> *const c_void + Send + Sync>>, … }`; egui_glow 0.36.2
`CallbackFn::new(Fn(PaintCallbackInfo, &Painter))`, `Painter::gl()`, `Painter::intermediate_fbo()`;
egui 0.36.2 `ViewportCommand::Screenshot` → `Event::Screenshot`. mpv headers (master, client API
2.6): `render.h` functions and params incl. `MPV_RENDER_PARAM_ADVANCED_CONTROL`, the `sw` API
(`SW_SIZE/SW_FORMAT/SW_STRIDE/SW_POINTER`), "update callback can be called from any thread",
"`mpv_render_context_update` must be called after each update with advanced control";
`render_gl.h` `mpv_opengl_init_params { get_proc_address, get_proc_address_ctx }` and
`mpv_opengl_fbo { fbo, w, h, internal_format }`; `client.h` declarations listed in V0,
`mpv_load_config_file`, "all encoding mode options must be set before `mpv_initialize`";
`stream_cb.h` `mpv_stream_cb_add_ro` and callbacks; `DOCS/encoding.rst` options and "Direct stream
copy" under "what does not work". Crates (newest on crates.io): libmpv2 6.0.0 (build-time link via
libmpv2-sys; no runtime loading), libmpv-sys 3.1.0 (2020), iroh 1.3.0 and iroh-relay 1.3.0
(`Endpoint`, `EndpointId`, `EndpointAddr`, `RelayUrl`, `RelayMode::Default`, `RelayMap`),
wasmtime 49.0.3, wasmtime-wasi-http 49.0.3, wit-bindgen 0.57.1, extism 1.41.0, rusqlite 0.40.2
(bundled build sets `SQLITE_ENABLE_FTS5/JSON1/RTREE`), sqlx 0.9.0, lofty 0.25.4, igd-next 0.18.0,
natpmp 0.5.0 (Nov 2024), mdns-sd 0.21.5, argon2 0.6.0, tokio-tungstenite 0.30.0 (newest on crates.io; axum 0.8.9 pins 0.29), utoipa 6.0.0,
libloading 0.9.0, rust-embed 8.13.0, axum-server 0.8.0, rustls-acme 0.15.4, rust_cast 0.21.0,
fast_image_resize 6.1.0, hls.js v1.7.3, librqbit-upnp-serve 9.0.1 (librqbit has an
`upnp-serve-adapter` feature); in the local cache: notify 8.2.0, image 0.25.10, rcgen 0.14.10,
tower-http 0.6.11, symphonia 0.5.5, moka 0.12.16, metrics 0.24.6, ed25519-dalek 3.0.0,
libloading 0.9.0, rusqlite 0.40.2 + libsqlite3-sys 0.38.2 (SQLite 3.53.2), argon2 0.6.0,
hyper-util 0.1.21 (in `Cargo.lock`), tokio 1.53.3 (`tokio::io::join` present). Local machine:
mpv 0.41.0 with FFmpeg 9.0.1, `/usr/lib64/libmpv.so.2` exporting 54 `mpv_*` symbols (all the
render/stream_cb/node ones the plan uses), no headers, no ffmpeg/ffprobe, clang present, NVIDIA
RTX 5080 with `libnvidia-encode.so`.

**Verified by the review's spikes (2026-10-19, mpv CLI, `--no-config`):** `--of=hls` with
`--ofopts` pass-through (TS and fMP4 with `#EXT-X-MAP`); keyframe interval via `--ovcopts=g=`;
`--start=N` + `start_number`/`output_ts_offset` keep the original timeline; `h264_nvenc` in
encode mode; `--no-video` audio-only AAC/MP4 and Opus/WebM; `--vf=scale`; `av://lavfi:` inputs;
`screenshot-raw` exists (`--input-cmdlist`); `encoder-list`/`decoder-list`/`protocol-list`
properties exist. Code facts: axum 0.8.9 → tokio-tungstenite 0.29; eframe 0.36.2
`Frame::register_native_glow_texture(&mut self, glow::Texture) -> TextureId` and `Frame::gl()`
(documented for offscreen rendering during `update`); egui_glow 0.36.2
`Painter::register_native_texture(&mut self)` (not callable from a `CallbackFn`, which gets
`&Painter`); engine tick `advanced.refresh_interval_ms` default 500 ms; `EngineEvent::Completed`
has no hash; `crash.rs` → `app::log::LINES` and `config.rs` → `ui::theme::PaletteDef` couplings;
kittest tests inject `app.snap`; `.cargo/config.toml` `incremental = false`; CI = fmt + OS matrix
(clippy, test, release build) + cross-check job.

**Not verified (treat as hypotheses until the spikes/first use):** the libmpv *in-process* encode
path (same options, worker binary); `--of=segment`; `h264_vaapi`/`*_qsv` (need an Intel/AMD box
[O]) and `h264_videotoolbox` (macOS [O]); subtitle burn-in and HTTP inputs in encode mode; libmpv
file names on Windows (`libmpv-2.dll`, older `mpv-2.dll`) and macOS (`libmpv.2.dylib`,
Homebrew); exact probe field coverage of mpv vs ffprobe; versions of `qrcode`, `gilrs`,
`windows-service`, `portmapper`, `keyring`; iroh 1.3 and wasmtime 49 feature names for the
trimmed feature sets; the Jellyfin API surface needed for E12; the `librqbit-upnp-serve`
provider API's fitness for a library-backed content directory; whether iroh 1.3 still offers a
local-network discovery module; build-time/disk estimates for wasmtime and iroh (measure).

---

## Review notes (Fable, 2026-10-19)

Reviewed against the code at `2ad9093`, the cargo registry cache and spikes run on the dev
machine. The format of the plan is unchanged; the edits are in place.

**Changed in the plan**

1. *Verification:* folded in the mpv spike results (HLS TS + fMP4, `g=` keyframes,
   restart-at-offset, NVENC on the local RTX 5080, audio-only, `vf=scale`, `av://lavfi`,
   `screenshot-raw`, `encoder-list`); removed the matching "(unverified)" flags and the
   `--of=mpegts` + own segmenter fallback. Verified the `libmpv.so.2` symbol set, the trigram
   tokenizer (SQLite 3.53.2 bundled), `tokio::io::join`, moka/metrics versions.
2. *Factual fixes:* tokio-tungstenite is 0.29 under axum 0.8.9 (not 0.30); the engine publishes at
   500 ms by default, so "4 Hz on LAN" needs a lower server interval; "25.4k non-test lines" → total
   incl. tests, 170 tests; no `ui/motion.rs` exists today (animation is scattered `animate_*`
   calls); `EngineCommand`/`EngineEvent` live in `engine/mod.rs`.
3. *eframe/mpv GL integration (V1):* `Painter::register_native_texture` needs `&mut Painter` and
   is not reachable inside `egui_glow::CallbackFn`; use `eframe::Frame::register_native_glow_texture`
   once, keep one GL texture name across resizes, render in `App::update` (GL is current there).
   Start without `MPV_RENDER_PARAM_ADVANCED_CONTROL` (it makes playback block when the app does not
   render promptly, e.g. minimized); added the create/free ordering rules for the render context.
4. *Crate split (step 4 / MS1):* two couplings the plan missed — `crash.rs` reads
   `app::log::LINES` and `config::Settings` embeds `ui::theme::PaletteDef`; both move to core.
   `EngineEvent::Completed` carries no info hash, which T1 needs — add it in MS1.
5. *ServerLink (A3/MS3):* resized [M] → [L] and listed the long tail (EngineEvent mapping, rss/
   undoable/busy/free-space wire forms, kittest `app.snap` seam, `LocalOnly` set). Added a
   `server/fs` folder-browser endpoint — remote clients cannot pick server paths with `rfd`.
6. *Security:* dropped the plain-HTTP "trust this server" page (LAN MITM vector) in favour of
   fingerprint display + CA download over HTTPS; S5 allowlist gains CGNAT 100.64/10 (Tailscale),
   IPv4-mapped IPv6 normalisation, user CIDRs, trusted-proxy handling; S8 no URL tokens for the
   web client (same-origin cookie) or the native client (`--http-header-fields` Bearer for mpv) —
   short-lived scoped tokens only for external players/cast, plus `Referrer-Policy: no-referrer`;
   pairing codes burn after 5 wrong attempts; device-token file permissions; plugin HTTP SSRF rules
   (private-range refusal, resolve-and-pin, re-checked redirects, no `wasi:sockets`).
7. *Networking:* iroh client side needs a loopback bridge (reqwest and mpv cannot dial iroh) —
   it doubles as N4(d); server side spelled out (`tokio::io::join` + `TokioIo` + hyper http1 with
   upgrades); `portmapper` (n0, UPnP/NAT-PMP/PCP) to evaluate before igd-next + natpmp.
8. *Ordering:* MS0d's CLI half is done; MS0e (iroh echo) moved to the start of MS12; MS2 split
   into MS2a (REST/identity/TLS/`correntd`) and MS2b (WS deltas) so MS4 can start on REST; MS3 now
   depends on MS2b; added the relationship to STABILITY S1 (MS1 → S1 → MS2a; S1 before MS5 at the
   latest) and two owner questions (7, 8).
9. *Scope gaps vs. the request:* an "Audio completeness" list (device picker, exclusive mode,
   channel policy, loudness/EQ opt, lyrics, podcasts via `feed-rs`, radio, folder art, multi-disc,
   classical tags); "customizable" beyond colours in U1 (home layout, view defaults, density/
   motion sliders, admin custom CSS opt).
10. *Build cost:* new "Build cost and cargo features" subsection — features `plugins`, `iroh`,
    `iroh-client`, `acme`, `metrics`, `dlna`, `cast`, `openapi`; trimmed wasmtime/iroh/image
    feature sets; defaults chosen so `cargo test --workspace` stays complete; one `target/`;
    committed `example-provider.wasm` for host tests; expected lock/target growth.
11. *Schema:* `images` keyed by `owner_kind/owner_id` (people and collections have art);
    `tasks.log` capped.
12. *Worker backends:* recommended `MpvCli` (exec `mpv`) next to `LibMpv` in `corrent-mpvw` — same
    encode code, zero FFI risk, what the spikes used; libmpv stays the default when present
    (owner question 8).

**Still open / disagreements left for the implementer or owner**

- The libmpv in-process encode path is still unexercised; if it misbehaves, `MpvCli` is the
  fallback and the plan does not depend on which one wins.
- VAAPI/QSV/VideoToolbox encoders cannot be tested on this machine ([O]).
- `wasmtime` and `iroh` feature names for the trimmed sets, `portmapper`/`keyring`/`qrcode`/
  `gilrs`/`windows-service` versions: confirm against crates.io when the dependency is added.
- Whether to keep the web client's torrents page at "90 % parity" (T6) or accept less for v1 is a
  scope call the owner may want to make; the plan is implementable either way.
- The plan still assumes `Snapshot` (with `PathBuf`s and `Arc<RssView>`) can be reproduced client
  side from `corrent-proto` types; the proto types must carry paths as strings and the RSS view as
  its own DTO — flagged in A3, to be settled in MS2b/MS3.
