# MS16 / E12 — Jellyfin-compatible API subset: implementation plan

Read-only planning result. Sources: the repo (`crates/corrent-server/src/{lib.rs,v1/*,identity.rs,state.rs,config_file.rs,net.rs,remote.rs,discovery.rs,dlna/*,api_tests.rs}`, `crates/corrent-proto/src/library.rs`, `crates/corrent-library/src/query.rs`, `crates/corrent-transcode/src/{decide.rs,hls.rs}`, `web/src/pages/settings/Remote.svelte`, `docs/SERVER.md`, `docs/MEDIASERVER.md`) plus Jellyfin's `release-10.10.z`/`master` sources on GitHub, the Kotlin SDK, jellyfin-web, jellyfin-apiclient-javascript and jellyfin-kodi. Everything below is verified against those unless marked **(unverified)**.

## 0. Findings that change the design (read first)

1. **Jellyfin renumbered.** Releases are now `v12.0` (2026-09-08), `v12.1`, `v12.2` (2026-10-05); the notes say they dropped the leading `10.` (10.11 → 12). The Kotlin SDK (Android TV, Findroid) on master has `minimumVersion = ServerVersion(12,0,0,0)` and its `RecommendedServerDiscovery` scores a server **BAD (unusable)** when `ProductName != "Jellyfin Server"` or the version is missing/unparseable, OK/GOOD when below minimum (apps then filter it out). `ServerVersion.fromString` needs **3 or 4 numeric components** (`"12.2"` fails). ⇒ `/System/Info/Public` must report `ProductName: "Jellyfin Server"` and a 3-part version ≥ 12.0.0; make it `jellyfin_version = "12.2.0"` in `server.toml` (default tracks the current Jellyfin stable at release time). Honest identification goes in `ServerName` ("Corrent (hostname)").
2. **Jellyfin 12 turned legacy auth off by default**: on master `X-Emby-Authorization`, the `Emby` scheme, `X-Emby-Token`, `X-MediaBrowser-Token` and `?api_key=` are gated; always-on are `Authorization: MediaBrowser …` and `?ApiKey=`. New clients therefore use `Authorization: MediaBrowser` + `ApiKey`; 10.10-era clients (jellyfin-web's apiclient, Kodi) use `api_key`/`X-Emby-*`. **We accept all of them** (document that this is more permissive than Jellyfin 12 defaults). `/emby/*` and `/mediabrowser/*` prefixes were removed in 12 — don't implement them.
3. **TLS is the real blocker for native clients.** Kotlin-SDK clients mark `SecureConnectionFailed` as BAD; Swiftfin/Infuse need a trusted cert **(unverified for those two)**. Our HTTPS uses a private CA. Jellyfin's own default is plain HTTP on **8096**. ⇒ Besides `/jellyfin` on the HTTPS listener, open a **plain-HTTP LAN-only listener on `jellyfin_port = 8096`** (same pattern as the DLNA listener, which exists for exactly this reason) with the Jellyfin router at its root. UDP discovery advertises `http://<ip>:8096`. That makes "type the IP" work on Android TV.
4. **Images are anonymous in Jellyfin** (`ImageController` has no `[Authorize]`; `BaseJellyfinApiController` has none; clients load art with plain `<img>`/image loaders without credentials). `Videos/{id}/stream` likewise has no `[Authorize]` in 10.10, but clients put the token in stream URLs (jellyfin-web verified: `api_key`; Kotlin SDK's `QUERY_ACCESS_TOKEN = "ApiKey"`). ⇒ Images: anonymous **only when `tag=<sha>` matches** (the sha is unguessable), otherwise auth required. Streams/HLS/subtitles: auth required (header or `api_key`/`ApiKey` query).
5. **Legacy `/Users/{userId}/…` routes still exist on master** (marked `[Obsolete]`, hidden from docs) and jellyfin-apiclient-javascript + jellyfin-kodi still use them (`Users/{id}/Items`, `Users/{id}/Items/{id}`, `Users/{id}/Views`, `Users/{id}/Items/Resume`, `Users/{id}/Items/Latest`, `Users/{id}/PlayedItems/{id}`, `Users/{id}/FavoriteItems/{id}`), while jellyfin-web ≥10.9 (SDK) and Kotlin/Swift SDKs use the new ones (`/UserViews`, `/Items`, `/UserItems/Resume`, `/Items/Latest`, `/UserPlayedItems`, `/UserFavoriteItems`). Both must be served.
6. **Routing is case-insensitive in ASP.NET** and apiclient.js literally calls `users/authenticatebyname`, `system/info/public`, `users/public`. axum is case-sensitive ⇒ lowercase the path before routing (see §2.2).
7. **Jellyfin Android (phone)** is a WebView wrapper: `WebViewFragment` loads `"${server.hostname}/"` — i.e. whatever the server serves at its root. For us that is Corrent's own web client (works, but no Jellyfin-native hooks) **(unverified beyond the loadUrl line)**. Jellyfin Media Player bundles jellyfin-web, so it behaves like Jellyfin Web **(unverified)**.
8. **jellyfin-web keeps HLS sessions lazy**: `PlaybackInfo` only decides; the transcode starts when `master.m3u8` is fetched. We must not create HLS sessions in `PlaybackInfo` (3-session cap).

## 1. Client matrix (what each one needs)

| Client | Base URL handling | Startup calls (verified) | Notes |
|---|---|---|---|
| Jellyfin Web 10.10 / JMP | typed URL kept as-is (tries `https://` then `http://` if no scheme; no default ports) | `system/info/public` → login page: `users/public`, `Branding/Configuration`, `QuickConnect/Enabled` → `Users/authenticatebyname` → `System/Info` (token check), `Users/{id}`, `POST Sessions/Capabilities/Full`, WS `/socket?api_key&deviceId`, `DisplayPreferences/usersettings?userId&client=emby` **(unverified)** → home: `UserViews`, `UserItems/Resume?MediaTypes=Video/Audio/Book&Limit&Fields&ImageTypeLimit&EnableImageTypes&EnableTotalRecordCount=false`, `Shows/NextUp?Limit&Fields&ImageTypeLimit&EnableImageTypes&EnableTotalRecordCount=false&NextUpDateCutoff&EnableResumable=false&EnableRewatching`, `Items/Latest?ParentId&Limit&Fields&ImageTypeLimit&EnableImageTypes` per view (skips `playlists,livetv,boxsets,channels,folders`) | needs CORS when hosted elsewhere (Jellyfin sends `CorsHosts=["*"]`); progress every 10 s; stream URLs carry `api_key`; min server version 3.2.33 |
| Android TV / Findroid (Kotlin SDK) | typed URL; SDK adds candidates **(unverified)** | `System/Info/Public` (3.5 s timeout, ProductName + Version gate), `Branding/Configuration`, `Users/Public`, `Users/AuthenticateByName`, `Users/Me`, `UserViews`, `UserItems/Resume`, `Shows/NextUp`, `Items/Latest`, `Items`, `Items/{id}`, `Items/{id}/PlaybackInfo`, `Sessions/Playing*`, `MediaSegments/{id}` **(call list partly unverified)** | needs plain HTTP (self-signed CA = BAD); token via `Authorization: MediaBrowser` + `ApiKey` |
| Kodi "Jellyfin for Kodi" | typed URL | `System/Info/Public`, `Users/AuthenticateByName`, `Users/{id}`, `Users/{id}/Views`, `Users/{id}/Items` (sync, big pages, `Fields=Path,Genres,SortName,Studios,Writer,Taglines,…,MediaSources,People,…`), `Users/{id}/Items/{id}`, `Items?Ids=`, `Shows/NextUp`, `Shows/{id}/Seasons|Episodes`, `Items/{id}/PlaybackInfo`, `Sessions/Capabilities/Full`, `Sessions/Playing*`, `Users/{id}/PlayedItems|FavoriteItems/{id}`, `Videos/ActiveEncodings`, `Items/{id}/Images/{type}?MaxWidth&format`, `Jellyfin.Plugin.KodiSyncQueue/*` (optional plugin → 404 is fine), `System/Configuration` | relies on `MediaSources` in list results and on `TotalRecordCount` paging |
| Swiftfin / Infuse | typed URL | same SDK-style surface **(unverified)** | likely need trusted TLS or http |

## 2. Where it plugs in

### 2.1 Module layout (`crates/corrent-server/src/jellyfin/`, ~2.7k lines total)

```
mod.rs        service(st) → tower Service: lowercase-path layer + CORS + runtime on/off gate + Router;
              JfState (play sessions, cached device profiles, ws clients); JfAuth extractor; constants
ids.rs        GUID codec (+tests)
dto.rs        serde DTOs (PascalCase, skip None): BaseItemDto, UserDto/Policy/Configuration, PublicSystemInfo,
              SystemInfo, AuthenticationResult, SessionInfoDto, QueryResult<T>, UserItemDataDto, MediaSourceInfo,
              MediaStream, PlaybackInfoResponse, PlaybackInfoDto+DeviceProfile (input), PlaybackProgressInfo,
              PlaybackStopInfo, DisplayPreferencesDto, BrandingOptions
system.rs     /System/*, /Branding/*, /QuickConnect/*, /Localization/*, misc stubs
users.rs      AuthenticateByName, Me, Public, {id}, Configuration, Sessions/Logout, Sessions/Capabilities*, DisplayPreferences
items.rs      UserViews (+legacy), Items (+legacy), Items/{id}, Latest, Resume, Shows/*, Similar, Filters2, stubs
images.rs     Items/{id}/Images/*
playback.rs   PlaybackInfo, DeviceProfile→ClientProfile, MediaSourceInfo, Videos/* (stream, master/main.m3u8, hls1),
              Audio/* (universal, stream, m3u8), Subtitles, ActiveEncodings
playstate.rs  Sessions/Playing*, UserPlayedItems, UserFavoriteItems, UserItems/{id}/UserData, Rating
socket.rs     /socket            (MS16c)
discovery.rs  UDP 7359 responder (MS16c)
tests.rs
```

### 2.2 Mounting, path case, guard, CORS

- **HTTPS listener** (`lib.rs::router`): `app.nest_service("/jellyfin", jellyfin::service(st.clone()))`. `nest_service` strips the prefix; the service wraps the Jellyfin `Router` in `tower::util::MapRequestLayer` that **lowercases the URI path** (query untouched; no path segment carries case-significant data — GUIDs are hex, `DisplayPreferences/{id}` ids are compared case-insensitively). Jellyfin routes are registered lowercase. Doing the rewrite inside `nest_service` avoids the axum rule that `Router::layer` runs after routing.
- The existing `v1::guard` (LAN rule, Host check, security headers) stays outermost, so it covers the nested service. Body limit: raise to 1 MiB for this service (jellyfin-web's `DeviceProfile` JSON is tens of KB).
- **Runtime gate**: a tiny layer checks `st.cfg.load().jellyfin_api`; off ⇒ 404 — the toggle works without restart (unlike `qbt_api`).
- **CORS** (needed only when Jellyfin Web is hosted elsewhere): enable tower-http's `cors` feature; `Allow-Origin: *`, methods `GET,HEAD,POST,DELETE,OPTIONS`, headers `Authorization, X-Emby-Authorization, X-Emby-Token, X-MediaBrowser-Token, Content-Type, Range, X-Requested-With`, expose `Content-Range, Content-Length`; preflight 204. Safe because this router never accepts cookies.
- **Root alias (MS16c, default on with `jellyfin_api`)**: users will paste `https://host:8443` without `/jellyfin`; today the SPA fallback answers `/System/Info/Public` with `index.html` (200!). Wrap `webclient::serve` in a fallback service that forwards to the Jellyfin service when (a) the request carries a Jellyfin marker (`Authorization: MediaBrowser|Emby …`, `X-Emby-Authorization`, `api_key`/`ApiKey` query) — apiclient and the SDKs send the header even on the first anonymous probe — or (b) the lowercased first segment is in the Jellyfin set minus SPA collisions (`items, videos, audio, users, useritems, userviews, userplayeditems, userfavoriteitems, shows, sessions, system, branding, quickconnect, displaypreferences, socket, artists, genres, persons, studios, playlists, mediasegments, localization, playback, livestreams, environment, plugins, scheduledtasks, packages, devices, auth, startup, web`). SPA routes are `/find /item /library /play /rss /search /settings /torrents` — only `library` and `search` collide; both are non-essential Jellyfin endpoints (admin `Library/*`, legacy `Search/Hints`) and are excluded from (b).
- **Plain-HTTP listener on `jellyfin_port` (default 8096, 0 = off)** started/stopped by `remote::apply` like DLNA: `Router::new().nest_service("/", service).nest_service("/jellyfin", service)` + a `lan_only` layer (refuse non-`is_local` peers regardless of `remote_access`) + `v1::guard`. Never over Corrent Connect. Bind failure → `RemoteStatus.jellyfin_error`.
- `TranscodingUrl`, `DeliveryUrl` and HLS variant/segment URLs must be **relative to the client's base URL** (apiclient: `getUrl('/videos/…')` = `serverAddress + path`), so they are emitted as `/Videos/…` with no `/jellyfin` and no host — correct on both listeners and with the root alias.
- axum 0.8 cannot match `stream.{container}` inside a segment: register `/videos/{item}/{name}` and `/audio/{item}/{name}` and dispatch on `name` (`stream`, `stream.<ext>`, `master.m3u8`, `main.m3u8`, `universal`, `universal.<ext>`); `/videos/{item}/hls1/{pl}/{seg}`; subtitles `/videos/{item}/{source}/subtitles/{index}/{start}/{file}` and the 4-segment variant. axum answers HEAD for `get` routes.

### 2.3 Auth mapping (`JfAuth` extractor)

Parse, in Jellyfin's verified order: `X-Emby-Authorization` then `Authorization`; scheme `MediaBrowser`/`Emby` (case-insensitive); comma-separated `Key=Value` pairs, quotes toggled by `"`, values `Trim('"')` + URL-decoded; keys `Client, Device, DeviceId, Version, Token` (case-sensitive). Token fallbacks: `X-Emby-Token`, `X-MediaBrowser-Token`, `?ApiKey`, `?api_key`. Then `st.identity.verify_token(token, ip)` (device tokens are `ct_…`, ≤80 chars — fits). **Never** accept the `corrent_session` cookie here (no CSRF model on this surface). `JfAuth` also exposes the parsed `DeviceCtx {client, device, device_id, version}` for pre-login requests. Errors: 401 only for genuinely missing/invalid tokens (jellyfin-web logs the user out on 401), 403 for permission, 404 for unknown/invisible items. `userId` query/path values must equal the token's user (else 403), except admins (`USERS_MANAGE`) reading other users.

### 2.4 Identity integration

- `Users/AuthenticateByName` → `limited()` rate limit, `identity.authenticate(name, pw)`, on failure `limiter.failed(ip)` + audit `login`/`jellyfin`; on success **`identity.issue_external(user_id, DeviceInfo{name: Device, platform: "jellyfin", client: "{Client} {Version}"}, external_id: DeviceId, ip, ua)`** — new method that first `DELETE FROM devices WHERE user_id=? AND external_id=?` (re-logins from the same Jellyfin DeviceId replace the row instead of piling up; same pattern as `issue_profile`). Needs migration **`0009_jellyfin.sql`**: `ALTER TABLE devices ADD COLUMN external_id TEXT` (+ index), and `CREATE TABLE jf_prefs(user_id, key, json, updated_at, PRIMARY KEY(user_id,key))` for UserConfiguration/DisplayPreferences documents (don't mix into `user_prefs`, whose shape our clients own). Devices then appear in `/api/v1/devices` and can be revoked there.
- `Sessions/Logout` → `revoke_session`. `Users/Public` → enabled users with a password (`pw_hash != ''`; the desktop's passwordless owner must not appear, or clients will try an empty-password login) — Jellyfin does the same (non-hidden, non-disabled).
- `Policy.IsAdministrator` is **always false** (otherwise jellyfin-web shows the Dashboard, which needs ~25 admin endpoints); admin rights stay in Corrent's own UIs.

### 2.5 Ids, ticks, dates, JSON

- **GUIDs**: Jellyfin serializes `Guid` with format `"N"` (32 hex, no dashes) and parses dashed or undashed. Jellyfin's own ids are MD5 hashes (arbitrary version bits), so clients accept any 32-hex. Reversible codec: 16 bytes = `kind(1) ‖ 0x00×7 ‖ i64 big-endian(8)` → hex. Kinds: `01` item, `02` user, `03` library/view (`03…0` = user root folder), `04` file (= `MediaSourceId`), `05` person, `06` genre. Decode validates the zero pad and returns 404 (never 500) on garbage. Server id: `PublicSystemInfo.Id`/`ServerId`/discovery `Id` = `server_id` (24 hex) padded with 8 zeros to 32 hex — Android TV does `systemInfo.id!!.toUUID()`.
- **Ticks** = ms × 10 000 (`RunTimeTicks`, `PlaybackPositionTicks`, `PositionTicks`, `StartPositionTicks`). **Dates**: unix seconds → `YYYY-MM-DDTHH:MM:SS.0000000Z` (Jellyfin emits 7 fraction digits). **JSON**: PascalCase, nulls omitted (`DefaultIgnoreCondition = WhenWritingNull`), enums as strings (`CollectionType` values are lowercase: `movies, tvshows, music, homevideos, books, photos`; `MediaType`: `Video|Audio|Photo|Book|Unknown`; `BaseItemKind` PascalCase). Accept PascalCase bodies with camelCase `serde(alias)`.
- Stream indices: `MediaStream.Index` must be unique across types (jellyfin-web finds tracks by `Index`); our `streams.idx` looks per-type (mpv track ids) — **verify in `probe.rs`**; if so expose `video = idx`, `audio = 100 + idx`, `subtitle = 200 + idx` (external subs are already ≥1000 → 1200+) and map back when a client sends `AudioStreamIndex`/`SubtitleStreamIndex`.

### 2.6 Small changes outside the module

- `corrent-proto/library.rs`: `ItemQuery` gains `kinds: Vec<ItemKind>`, `ids: Option<Vec<i64>>`, `ancestor_id: Option<i64>` (recursive CTE, as in `set_state`), `resumable: Option<bool>` (`position_ms > 0 AND NOT played`), `name_starts_with: Option<String>`, `SortBy::Random`; `Item` gains `grandparent_id` and `parent_index` (`p.index_no`, already joined in `ITEM_SELECT`) for `SeriesId`/`ParentIndexNumber`/`AlbumArtist`; `ImageRef` gains `inherited: bool` (set by `attach_images` for parent fallbacks) so episodes get `ParentThumbImageTag`/`SeriesPrimaryImageTag` instead of a poster as `Primary`. All serde-defaulted and skip-if-default — wire-compatible.
- `corrent-library/query.rs`: implement the above; make `next_up` public as `Library::next_up(user, series: Option<i64>, limit, include_first_episode)`; add `Library::genres(library: Option<i64>)` (MS16c) and `children_count` is already there.
- `config_file.rs`: `jellyfin_api: bool = false`, `jellyfin_port: u16 = 8096`, `jellyfin_discovery: bool = true`, `jellyfin_version: String = "12.2.0"`, `jellyfin_public_images: bool = false` (true = also serve art without `tag`/auth, Jellyfin-style). `RemoteConfig` carries `jellyfin_api`, `jellyfin_port`; `RemoteStatus` gets `jellyfin_port: Option<u16>`, `jellyfin_discovery: bool`, `jellyfin_error: Option<String>`; `remote::{config_of, with_config, apply}` extended like DLNA.
- `state.rs`: `pub jellyfin: jellyfin::JfState`.
- `Cargo.toml` (corrent-server): tower-http `cors` feature; no new crates (UDP via `tokio::net::UdpSocket`, JSON via serde).

## 3. Endpoint catalogue

Conventions: `Q` = query, `B` = JSON body, `→` = response. `QR(T)` = `{"Items":[T],"TotalRecordCount":n,"StartIndex":s}`. All paths matched case-insensitively. "auth" = `JfAuth` required; "anon" = no auth.

### 3.1 Handshake (MS16a)

| Route | Auth | Response skeleton |
|---|---|---|
| `GET /System/Info/Public` | anon | `{"LocalAddress":"http://<ip>:8096","ServerName":"<display name>","Version":"12.2.0","ProductName":"Jellyfin Server","OperatingSystem":"","Id":"<32hex>","StartupWizardCompleted":true}` (`LocalAddress` = the URL of the listener the request came in on) |
| `GET /System/Info` | auth | public fields + `OperatingSystemDisplayName:"", PackageName:"", HasPendingRestart:false, IsShuttingDown:false, SupportsLibraryMonitor:true, WebSocketPortNumber:<port>, CompletedInstallations:[], CanSelfRestart:false, CanLaunchWebBrowser:false, ProgramDataPath:"", WebPath:"", ItemsByNamePath:"", CachePath:"", LogPath:"", InternalMetadataPath:"", TranscodingTempPath:"", CastReceiverApplications:[], HasUpdateAvailable:false, EncoderLocation:"NotFound", SystemArchitecture:"X64"` (never real paths) |
| `GET|POST /System/Ping` | anon | JSON string of the server name (Jellyfin returns `_appHost.Name`) |
| `GET /System/Endpoint` | auth | `{"IsLocal":<loopback>,"IsInNetwork":<is_local>}` |
| `GET /Branding/Configuration` | anon | `{"LoginDisclaimer":"","CustomCss":"","SplashscreenEnabled":false}` |
| `GET /Branding/Css[.css]` | anon | `text/css`, empty 200 |
| `GET /QuickConnect/Enabled` | anon | `false` |
| `POST /QuickConnect/Initiate`, `GET /QuickConnect/Connect`, `POST /QuickConnect/Authorize` | — | 401 `"Quick connect is disabled"` (Jellyfin's behaviour) |

### 3.2 Auth & users (MS16a)

| Route | Auth | Request → Response |
|---|---|---|
| `POST /Users/AuthenticateByName` | anon + `Authorization: MediaBrowser Client=…, Device=…, DeviceId=…, Version=…` (no Token) | B `{"Username":"…","Pw":"…"}` → `{"User":<UserDto>,"SessionInfo":<SessionInfoDto>,"AccessToken":"ct_…","ServerId":"<32hex>"}`; 401 on bad credentials, 429 from the limiter |
| `GET /Users/Me`, `GET /Users/{userId}` | auth | `<UserDto>` |
| `GET /Users/Public` | anon | `[<UserDto> (Policy/Configuration included)]` |
| `GET /Users` | auth | own user only unless `USERS_MANAGE` |
| `POST /Users/Configuration`, `POST /Users/{userId}/Configuration` | auth | B `UserConfiguration` → store in `jf_prefs("configuration")`, 204 |
| `POST /Sessions/Logout` | auth | revoke session, 204 |
| `POST /Sessions/Capabilities`, `POST /Sessions/Capabilities/Full` | auth | B `ClientCapabilitiesDto {PlayableMediaTypes, SupportedCommands, SupportsMediaControl, SupportsPersistentIdentifier, DeviceProfile?, AppStoreUrl?, IconUrl?}` → remember `DeviceProfile` per (user, DeviceId) in `JfState` (bounded map; used when `PlaybackInfo` has none — Jellyfin does the same), 204 |
| `GET /Sessions` | auth | `[]` (MS16c: our own session) |
| `GET /DisplayPreferences/{id}?userId&client` | auth | stored doc or defaults: `{"Id":"<id>","ViewType":null,"SortBy":"SortName","IndexBy":null,"RememberIndexing":false,"PrimaryImageHeight":250,"PrimaryImageWidth":250,"CustomPrefs":{},"ScrollDirection":"Horizontal","ShowBackdrop":true,"RememberSorting":false,"SortOrder":"Ascending","ShowSidebar":false,"Client":"<client>"}` |
| `POST /DisplayPreferences/{id}?userId&client` | auth | store whole body in `jf_prefs("display:<client>:<id>")` (≤64 KiB), 204 |

**UserDto** (fields clients read): `{"Name","ServerId","ServerName","Id":"02…","PrimaryImageTag":null,"HasPassword":true,"HasConfiguredPassword":true,"HasConfiguredEasyPassword":false,"EnableAutoLogin":false,"LastLoginDate","LastActivityDate","Configuration":{…},"Policy":{…}}`.
`Configuration` defaults: `PlayDefaultAudioTrack:true, SubtitleLanguagePreference:"", DisplayMissingEpisodes:false, GroupedFolders:[], SubtitleMode:"Default", DisplayCollectionsView:false, EnableLocalPassword:false, OrderedViews:[], LatestItemsExcludes:[], MyMediaExcludes:[], HidePlayedInLatest:true, RememberAudioSelections:true, RememberSubtitleSelections:true, EnableNextEpisodeAutoPlay:true, CastReceiverId:""` (overlaid with the stored document).
`Policy`: `IsAdministrator:false, IsHidden:false, IsDisabled, EnableUserPreferenceAccess:true, AccessSchedules:[], BlockUnratedItems:[], BlockedTags:[], AllowedTags:[], EnableRemoteControlOfOtherUsers:false, EnableSharedDeviceControl:false, EnableRemoteAccess:true, EnableLiveTvManagement:false, EnableLiveTvAccess:false, EnableMediaPlayback:true, EnableAudioPlaybackTranscoding|EnableVideoPlaybackTranscoding|EnablePlaybackRemuxing: perms.contains(PLAYBACK_TRANSCODE), ForceRemoteSourceTranscoding:false, EnableContentDeletion:false, EnableContentDeletionFromFolders:[], EnableContentDownloading:true, EnableSyncTranscoding:false, EnableMediaConversion:false, EnabledDevices:[], EnableAllDevices:true, EnabledChannels:[], EnableAllChannels:false, EnabledFolders:[view guids or []], EnableAllFolders:<no restriction>, InvalidLoginAttemptCount:0, LoginAttemptsBeforeLockout:-1, MaxActiveSessions:0, EnablePublicSharing:false, BlockedMediaFolders:[], BlockedChannels:[], RemoteClientBitrateLimit:0, AuthenticationProviderId:"Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider", PasswordResetProviderId:"Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider", SyncPlayAccess:"None", EnableCollectionManagement:false, EnableSubtitleManagement:false, EnableLyricManagement:false, MaxParentalRating:null`. (The two provider ids are `[Required]`.)
**SessionInfoDto** minimum: `{"Id":"<session id hex>","UserId","UserName","Client","DeviceName","DeviceId","ApplicationVersion","ServerId","LastActivityDate","LastPlaybackCheckIn","IsActive":true,"SupportsMediaControl":false,"SupportsRemoteControl":false,"PlayableMediaTypes":[],"SupportedCommands":[],"HasCustomDeviceName":false,"AdditionalUsers":[],"RemoteEndPoint":"<ip>"}`.

### 3.3 Views and browsing (MS16a)

| Route | Q params used | Response |
|---|---|---|
| `GET /UserViews?userId`, `GET /Users/{userId}/Views` | — | `QR(view)` where view = `{"Name":lib.name,"ServerId","Id":"03…","Type":"CollectionFolder","CollectionType":"movies|tvshows|music|homevideos|books|photos","IsFolder":true,"ChildCount":item_count,"DisplayPreferencesId":"03…","ImageTags":{},"BackdropImageTags":[],"UserData":{…zeros…},"LocationType":"FileSystem","MediaType":"Unknown","PlayAccess":"Full","SortName","Etag":"<lib id>"}`; filtered by `user_libraries` |
| `GET /Items`, `GET /Users/{userId}/Items` | `ParentId, Recursive, IncludeItemTypes, ExcludeItemTypes, MediaTypes, Ids, SearchTerm, SortBy, SortOrder, StartIndex, Limit, Filters, IsFavorite, IsPlayed, Genres|GenreIds, PersonIds, NameStartsWith|NameStartsWithOrGreater, Fields` | `QR(BaseItemDto)` — translation in the table below |
| `GET /Items/{id}`, `GET /Users/{userId}/Items/{id}` | — | full `BaseItemDto` (with MediaSources/MediaStreams/Chapters/People); `Items/Root` and the all-zero id → the user root folder `{"Type":"UserRootFolder","Id":"03…0","Name":"Media Folders","IsFolder":true}` |
| `GET /Items/Latest`, `GET /Users/{userId}/Items/Latest` | `ParentId, Limit(20), IncludeItemTypes, IsPlayed, GroupItems` | **bare array** of `BaseItemDto`: per library kind the same selection as `home()`'s `recent:<lib>` row (episodes for shows; `GroupItems` ignored in 16a) |
| `GET /UserItems/Resume`, `GET /Users/{userId}/Items/Resume` | `MediaTypes, Limit, StartIndex, ParentId, IncludeItemTypes` | `QR` of `resumable=true` items sorted by last played |
| `GET /Shows/NextUp` | `SeriesId, ParentId, Limit, StartIndex, DisableFirstEpisode, EnableResumable, EnableRewatching, NextUpDateCutoff` | `QR(episodes)` from `Library::next_up`; with `SeriesId` and nothing watched → S01E01 unless `DisableFirstEpisode=true` |
| `GET /Shows/{seriesId}/Seasons` | `userId` | `QR(seasons)` = children of the show, kind season, sort index |
| `GET /Shows/{seriesId}/Episodes` | `SeasonId, Season, StartItemId, StartIndex, Limit, SortBy` | `QR(episodes)`: with `SeasonId` children of that season; with `Season=n` the season with that index; else all episodes (`ancestor_id = show`, sort index = season then episode) |
| `GET /Items/{id}/Similar` | `Limit` | `QR` same kind + library, sharing the first genre, excluding itself; else recently added |
| `GET /Items/{id}/Ancestors` | — | array `[season, show, view]` |
| `GET /Items/Filters2` | `ParentId, IncludeItemTypes` | `{"Genres":[{"Name","Id":"06…"}],"Tags":[]}` (16a: empty lists; 16c: real genres) |
| `GET /Items/Filters` | — | `{"Genres":[],"Tags":[],"OfficialRatings":[],"Years":[]}` |
| `GET /Items/{id}/Intros`, `/LocalTrailers`, `/SpecialFeatures`, `/ThemeMedia`, `GET /Videos/{id}/AdditionalParts`, `GET /Users/{u}/Items/{id}/Intros|LocalTrailers|SpecialFeatures` | — | empty `QR` / `[]`; ThemeMedia: `{"ThemeVideosResult":QR∅,"ThemeSongsResult":QR∅,"SoundtrackSongsResult":QR∅}` |
| `GET /Items/Counts` | — | `{"MovieCount","SeriesCount","EpisodeCount","ArtistCount","AlbumCount","SongCount","BookCount","ItemCount",…}` (cheap counts, 16c) |
| `GET /Artists`, `GET /Artists/AlbumArtists`, `GET /Persons`, `GET /Genres`, `GET /Studios`, `GET /MusicGenres`, `GET /Years`, `GET /Playlists` | — | 16a: empty `QR`; 16c: Artists → `kind=artist`, Persons → `people LIKE`, Genres → `Library::genres` |
| `GET /MediaSegments/{id}` | auth | 16a: `QR∅`; 16c: chapter-based `Intro/Recap/Outro` segments from `chapter_kind` (`{"Id","ItemId","Type":"Intro","StartTicks","EndTicks"}`) **(DTO field names unverified)** |
| `GET /Localization/Options|Cultures|Countries|ParentalRatings` | auth | `[]` (Options: `[{"Name":"English","Value":"en-US"},{"Name":"Türkçe","Value":"tr"}]`) |
| `GET /Plugins`, `/Packages`, `/ScheduledTasks`, `/Notifications/*`, `/System/Configuration`, `/Library/VirtualFolders|MediaFolders` | auth | `[]` / 404 — Kodi reads `System/Configuration` (give `{}`) |

**`/Items` translation**

| Jellyfin | ItemQuery |
|---|---|
| `ParentId` = `03…L` (view) | `library_id = L`, `parent_id = None` → top level (movies: Movie; shows: Show; music: Artist; videos/photos: folders + files) |
| `ParentId` = `01…I` (item) | `parent_id = I`; with `Recursive=true` and `IncludeItemTypes` → `ancestor_id = I` + `kinds` |
| `ParentId` = root / absent | without `IncludeItemTypes`/`Recursive`: the views; otherwise across all visible libraries |
| `IncludeItemTypes` | `Movie→movie, Series→show, Season→season, Episode→episode, MusicArtist→artist, MusicAlbum→album, Audio→track, AudioBook|Book→book, Video→video, Photo→photo, Folder|PhotoAlbum→folder`; `BoxSet, Playlist, Trailer, MusicVideo, Person, Genre, Studio, …` → empty result |
| `MediaTypes` | `Video→{movie,episode,video}`, `Audio→{track}`, `Book→{book}`, `Photo→{photo}` |
| `Ids` | `ids` (comma list of GUIDs; Kodi sync) |
| `SearchTerm` | `library.search()` then filter by kinds/library, paginate in memory |
| `Filters` | `IsFavorite→favorite=true`, `IsPlayed/IsUnplayed→played`, `IsResumable→resumable=true`, `IsFolder/IsNotFolder→kinds` split; `IsFavorite=`/`IsPlayed=` likewise |
| `Genres`/`GenreIds`, `PersonIds` | `genre` (first), `person_id` |
| `SortBy` (first key) | `SortName|Name|Default→Title`, `DateCreated→Added`, `PremiereDate|ProductionYear→Year`, `DatePlayed→Played`, `IndexNumber|ParentIndexNumber|AiredEpisodeOrder→Index`, `Random→Random`, others → Title; `SortOrder=Descending→desc` |
| `StartIndex`/`Limit` | `offset`/`limit`; honour `Limit` up to 2000 by looping over our 500-row pages (Kodi advances `StartIndex` by the limit it asked for, not by what it got) |
| `Fields` | ignored for cheap fields (always sent); `MediaSources|MediaStreams|Chapters|People` are added to list results only when requested and `Limit ≤ 200` (one `item()` per row — cache `ItemDetail`→`MediaSourceInfo` in a small LRU in `JfState` for Kodi syncs) |
| `EnableImages, ImageTypeLimit, EnableImageTypes, EnableUserData, EnableTotalRecordCount, CollapseBoxSetItems, …` | ignored |

### 3.4 BaseItemDto (from `Item` / `ItemDetail`)

Always: `Name:title, OriginalTitle, ServerId, Id:"01…", Etag:"<added_at>", DateCreated, SortName:title, Type, MediaType, IsFolder:!kind.playable(), ParentId (season/album/show, else the view id), ProductionYear:year, RunTimeTicks:runtime_ms×10000, IndexNumber:index, ParentIndexNumber:parent_index, ChildCount, RecursiveItemCount (shows: episodes, 16c), LocationType:"FileSystem", PlayAccess:"Full", CanDelete:false, CanDownload:perms?, LockData:false, LockedFields:[], Taglines:[], Tags:[], Studios:[], Genres:[names], GenreItems:[{Name,Id:"06…"}], ProviderIds:{Tmdb,Imdb,Tvdb,MusicBrainzAlbum,…} (keys re-cased), ExternalUrls:[{Name:"IMDb",Url}], RemoteTrailers:[], CommunityRating:rating, OfficialRating, Overview, PremiereDate, PrimaryImageAspectRatio:w/h of Primary, ImageTags:{Primary,Backdrop,Logo,Thumb → sha}, BackdropImageTags:[sha], ImageBlurHashes:{Primary:{sha:blurhash},…}, UserData:{…}, Container:<file extension>, HasSubtitles, Width/Height (video/photo), IsHD:height≥720, VideoType:"VideoFile", MediaSourceCount:1, PartCount:1, Chapters:[{StartPositionTicks,Name}]` (detail), `People:[{Name,Id:"05…",Role:character,Type:PersonKind(actor→Actor, director→Director, writer→Writer, composer→Composer, artist→Artist, albumartist→AlbumArtist, else Unknown)}]` (detail), `MediaSources:[…]`, `MediaStreams:[…]` (detail / on request), `Path` (admins only).
Kind-specific: `Type` = `Movie|Series|Season|Episode|MusicArtist|MusicAlbum|Audio|AudioBook(books)|Video|Photo|Folder(PhotoAlbum in photo libraries)`; `MediaType` = Video for movie/episode/video, Audio for track/book, Photo, else Unknown. Episode: `SeriesName=context[0], SeasonName=context[1], SeriesId=grandparent, SeasonId=parent, ParentIndexNumber=season index, SeriesPrimaryImageTag, ParentBackdropItemId/ParentBackdropImageTags, ParentThumbItemId/ParentThumbImageTag, ParentLogoItemId/ParentLogoImageTag` — own images only when `!inherited`, else parent fields. Track: `Album, AlbumId=parent, AlbumArtist=context[0], AlbumArtists:[{Name,Id:grandparent}], Artists:[name], ArtistItems, AlbumPrimaryImageTag`. Views: `CollectionType, DisplayPreferencesId`.
**UserItemDataDto**: `{"Rating":state.rating,"PlayedPercentage":pos/runtime×100,"UnplayedItemCount":state.unplayed,"PlaybackPositionTicks":position_ms×10000,"PlayCount","IsFavorite","Likes":null,"LastPlayedDate","Played","Key":"<item guid>","ItemId":"<item guid>"}` (`Key` is `required`).
Image kind map: `Primary ← poster (fallback cover/thumb for videos)`, `Backdrop ← backdrop`, `Logo ← logo`, `Thumb ← thumb (fallback backdrop)`; `Banner`, `Art`, `Disc` → absent/404.

### 3.5 Images (MS16a)

`GET|HEAD /Items/{id}/Images/{type}[/{index}]` with `tag, maxWidth|width|fillWidth, maxHeight|height|fillHeight, quality, format, blur, percentPlayed, unplayedCount` (only the size and `tag` are used): width = width-ish param, else height-ish × 0.7, else 400 → `artwork::existing(root, sha, Some(w))` (variants 200/400/800). With `tag` (64 hex) the sha is served directly (anonymous allowed; `Cache-Control: public, max-age=31536000, immutable`, `ETag: "sha"`, 304 on `If-None-Match`); without `tag` auth is required (or `jellyfin_public_images`) and the item's current image of that type is used (`Cache-Control: private, max-age=3600`). 404 with Jellyfin's phrasing when the item has no such image. `GET /UserImage`, `/Users/{id}/Images/*`, `/Artists|Genres|Persons|Studios/{name}/Images/*` → 404.

### 3.6 Playback (MS16b)

**`POST /Items/{id}/PlaybackInfo`** (also `GET`), Q overrides B (Jellyfin: "Query has higher precedence"). Body `PlaybackInfoDto`: `UserId, MaxStreamingBitrate, StartTimeTicks, AudioStreamIndex, SubtitleStreamIndex, MaxAudioChannels, MediaSourceId, LiveStreamId, DeviceProfile, EnableDirectPlay, EnableDirectStream, EnableTranscoding, AllowVideoStreamCopy, AllowAudioStreamCopy, AutoOpenLiveStream, AlwaysBurnInSubtitleWhenTranscoding`.
`DeviceProfile {Name, Id, MaxStreamingBitrate(8e6 default), MaxStaticBitrate, MusicStreamingTranscodingBitrate, MaxStaticMusicBitrate, DirectPlayProfiles[{Container, AudioCodec, VideoCodec, Type:"Video|Audio|Photo"}], TranscodingProfiles[{Container, Type, VideoCodec, AudioCodec, Protocol:"http|hls", Context:"Streaming|Static", MaxAudioChannels, MinSegments, SegmentLength, BreakOnNonKeyFrames, EnableSubtitlesInManifest, Conditions[]}], ContainerProfiles[], CodecProfiles[{Type:"Video|VideoAudio|Audio", Codec, Container, Conditions[{Condition:"Equals|NotEquals|LessThanEqual|GreaterThanEqual|EqualsAny", Property:<ProfileConditionValue>, Value, IsRequired}], ApplyConditions[]}], SubtitleProfiles[{Format, Method:"Encode|Embed|External|Hls|Drop", Language, Container}]}` → **`ClientProfile`**:
- `containers` = union of Video/Audio `DirectPlayProfiles.Container` split on `,`, mapped to mpv names: `mp4|m4v|mov|m4a|m4b|3gp→mp4,mov,m4a`; `mkv→matroska,mkv`; `webm|webma→webm`; `ts|m2ts|mpegts→mpegts`; `avi→avi`; `mpg|mpeg→mpeg,mpegvideo`; `flv→flv`; `wmv|asf|wma→asf`; `mp3→mp3`; `flac→flac`; `ogg|oga|opus→ogg`; `wav→wav`; `aac→aac`.
- `video_codecs`/`audio_codecs` = unions (lower-case; `h265→hevc`, `pcm→pcm_s16le`); `h264_10bit` = a CodecProfile for h264 with a `VideoProfile` value containing `high 10`; `max_height` = min of `Height ≤` conditions (or `Width ≤ w → w×9/16`); `max_audio_channels` = min `AudioChannels ≤` (default 2, 6 if ac3/eac3/dts listed); `max_bitrate` = `MaxStreamingBitrate` (body/query over profile); `hls` = a Video `TranscodingProfile` with `Protocol=hls`; `everything=false`.
- Request: `audio_track = AudioStreamIndex−100`, `burn_subtitle` when the chosen subtitle is image-based or has no External profile (or `AlwaysBurnInSubtitleWhenTranscoding`), `force_transcode = !EnableDirectPlay`. No `DeviceProfile` → the one cached from `Sessions/Capabilities/Full`, else `ClientProfile::web_chromium()`.
→ `{"MediaSources":[MediaSourceInfo],"PlaySessionId":"<32hex>"}` (empty + `"ErrorCode":"NoCompatibleStream"` when nothing can play).
**MediaSourceInfo**: `{"Protocol":"File","Id":"04…file","Path":<file name; full path for LIBRARY_MANAGE>,"Type":"Default","Container":"mkv","Size","Name":<stem>,"IsRemote":false,"ETag":"<file id>-<size>","RunTimeTicks","ReadAtNativeFramerate":false,"IgnoreDts":false,"IgnoreIndex":false,"GenPtsInput":false,"SupportsTranscoding":<perm PLAYBACK_TRANSCODE && encoder available (or audio-only)>,"SupportsDirectStream":false,"SupportsDirectPlay":<Decision::Direct>,"IsInfiniteStream":false,"RequiresOpening":false,"RequiresClosing":false,"RequiresLooping":false,"SupportsProbing":true,"VideoType":"VideoFile","MediaStreams":[…],"MediaAttachments":[],"Formats":[],"Bitrate":size*8*1000/duration,"RequiredHttpHeaders":{},"TranscodingUrl":"/Videos/{itemGuid}/master.m3u8?MediaSourceId=04…&PlaySessionId=…&DeviceId=…&VideoCodec=h264&AudioCodec=aac&SegmentContainer=ts&TranscodingMaxAudioChannels=2&TranscodeReasons=ContainerNotSupported,…&api_key=<token>" (only when not direct),"TranscodingSubProtocol":"hls","TranscodingContainer":"ts","DefaultAudioStreamIndex":100+idx,"DefaultSubtitleStreamIndex":200+idx or omitted}`. `PlaySessionId` → `JfState.play_sessions[id] = {user, file, item, decision params, hls_id: None, created}` (TTL 6 h, cap 1000) — **no HLS session yet**.
**MediaStream**: `{"Codec","Language","Title","DisplayTitle":"English - AAC - Stereo - Default" / "1080p H264 SDR" / "English - SRT - External","Type":"Video|Audio|Subtitle","Index":<unique>,"IsDefault","IsForced","IsExternal","IsHearingImpaired":false,"IsInterlaced":false,"IsAVC":codec==h264,"Width","Height","AspectRatio":"16:9","RealFrameRate","AverageFrameRate","Profile","Level":null,"BitRate","BitDepth":8|10,"Channels","ChannelLayout":"stereo|5.1","SampleRate","VideoRange":"SDR","VideoRangeType":"SDR","AudioSpatialFormat":"None","IsTextSubtitleStream":<codec not pgs/dvdsub/dvbsub/sup/sub>,"SupportsExternalStream":external&&text,"DeliveryMethod":"External|Encode","DeliveryUrl":"/Videos/{item}/{source}/Subtitles/{index}/0/Stream.vtt?api_key=…"}`. Embedded text subtitles get `Encode` (we don't extract; selecting one forces a burn-in transcode — documented limitation).

| Route | Behaviour |
|---|---|
| `GET|HEAD /Videos/{id}/stream[.ext]` Q `Static, MediaSourceId, api_key/ApiKey, DeviceId, Tag, PlaySessionId, …` | auth (header or query). Resolve file (`MediaSourceId` → file id, else main file; visibility via `file_path`); `Static=true` (or omitted and the requested ext matches the file) → `serve_path` (Range, 206, HEAD). Unfinished torrent files → proxy through `integrate::downloading_file` like v1 (`/api/v1/torrents/{hash}/files/{idx}/stream` logic reused, not redirected). Non-static (progressive transcode) → 400 `{"message":"use HLS"}` |
| `GET /Videos/{id}/master.m3u8?PlaySessionId&MediaSourceId&…` | auth; look up play session (owner check); create the HLS session now (`st.hls.create(SessionSpec{…})`, `PLAYBACK_TRANSCODE`, encoder check) if `hls_id` is None → `#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=<b>,AVERAGE-BANDWIDTH=<b>,CODECS="avc1.640028,mp4a.40.2",RESOLUTION=WxH\nmain.m3u8?<same query>\n`; `Content-Type: application/vnd.apple.mpegurl`, `Cache-Control: no-store` |
| `GET /Videos/{id}/main.m3u8?…` | `st.hls.playlist(hls_id)` rewritten: each `N.ts` → `hls1/main/N.ts?<same query>` (Jellyfin's layout is `hls1/{name}/{index}.ts?query`; the token travels in the query because hls.js/ExoPlayer fetch segments without headers) |
| `GET /Videos/{id}/hls1/{name}/{n}.ts?…` | auth via query; `st.hls.segment(hls_id, n)` (waits for the encoder; far seeks restart it — existing behaviour) |
| `DELETE /Videos/ActiveEncodings?deviceId&playSessionId` | stop the HLS session of that play session (owner check), 204 |
| `GET|HEAD /Audio/{id}/universal[.ext]` Q `Container(list with `|codecs`), AudioCodec, MaxStreamingBitrate, TranscodingContainer, TranscodingProtocol, MaxAudioChannels, StartTimeTicks, EnableRedirection, EnableRemoteMedia, MediaSourceId, DeviceId, UserId, api_key` | build a `ClientProfile` from `Container=` and decide; direct → `serve_path`; else `TranscodingProtocol=hls` → create an audio-only HLS session and **return the master playlist body** (Jellyfin does exactly this) whose `main.m3u8?…` resolves to `/Audio/{id}/main.m3u8`; progressive transcode requested → 400 |
| `GET /Audio/{id}/stream[.ext]?Static=true`, `/Audio/{id}/main.m3u8`, `/Audio/{id}/hls1/{name}/{n}.ts` | as for video |
| `GET /Videos/{item}/{source}/Subtitles/{index}[/{start}]/Stream.{fmt}` | auth via query; `index−200` (external ≥1200 → `idx−200`); `vtt` → `corrent_transcode::vtt::to_vtt` (as `v1::playback::subtitle`); `srt|ass|ssa` → raw file when the source extension matches, else 404; `text/vtt; charset=utf-8`, `private, max-age=600` |
| `GET /Items/{id}/Download`, `GET /Items/{id}/File` | main file with `Content-Disposition: attachment` (MS16c) |

Trickplay (`BaseItemDto.Trickplay` + `/Videos/{id}/Trickplay/{w}/{n}.jpg`) is **not** in MS16: our sheets (`generate(…,10,240,…)`) would need tile geometry exposed first — list as a follow-up.

### 3.7 Progress & user data (MS16b)

| Route | Body/Q → effect | Response |
|---|---|---|
| `POST /Sessions/Playing` | `PlaybackStartInfo` (= `PlaybackProgressInfo`): `ItemId, MediaSourceId, PositionTicks, IsPaused, IsMuted, PlayMethod:"Transcode|DirectStream|DirectPlay", PlaySessionId, CanSeek, AudioStreamIndex, SubtitleStreamIndex, VolumeLevel, RepeatMode, PlaybackOrder, NowPlayingQueue, PlaylistItemId, LiveStreamId, SessionId` → `set_state{position_ms}` when `PositionTicks` present; remember now-playing on the play session | 204 |
| `POST /Sessions/Playing/Progress` | same DTO → `set_state{position_ms}` (threshold → played, existing logic); throttle identical positions | 204 |
| `POST /Sessions/Playing/Stopped` | `PlaybackStopInfo {ItemId, MediaSourceId, PositionTicks, PlaySessionId, Failed, NextMediaType, PlaylistItemId, NowPlayingQueue, LiveStreamId, SessionId}` → `set_state{position_ms}`; stop that play session's HLS | 204 |
| `POST /Sessions/Playing/Ping?playSessionId` | touch session | 204 |
| `POST|DELETE /UserPlayedItems/{id}?userId&datePlayed`, `POST|DELETE /Users/{u}/PlayedItems/{id}` | `set_state{played: true/false}` (containers cascade, existing) | `UserItemDataDto` |
| `POST|DELETE /UserFavoriteItems/{id}`, `POST|DELETE /Users/{u}/FavoriteItems/{id}` | `set_state{favorite}` | `UserItemDataDto` |
| `GET|POST /UserItems/{id}/UserData`, `/Users/{u}/Items/{id}/UserData` | POST body `UpdateUserItemDataDto {Played, IsFavorite, PlaybackPositionTicks, Rating, Likes, PlayCount, LastPlayedDate}` → `StateUpdate` | `UserItemDataDto` |
| `POST|DELETE /UserItems/{id}/Rating?likes`, legacy | no "likes" in our model → return current data | `UserItemDataDto` |
| legacy `POST /PlayingItems/{id}`, `/PlayingItems/{id}/Progress`, `DELETE /PlayingItems/{id}` (+ `/Users/{u}/…`) | query-param variants of the above | 204 |

### 3.8 WebSocket `/socket?api_key=&deviceId=` (MS16c)

Auth as everywhere (query token). Messages are UTF-8 JSON text frames `{"MessageType":"…","Data":…,"MessageId":"<32hex>"}`. On connect send `{"MessageType":"ForceKeepAlive","Data":60}`; on inbound `KeepAlive` reply `{"MessageType":"KeepAlive"}` (Jellyfin echoes and refreshes its timestamp); re-send `ForceKeepAlive` when idle > 45 s and drop at 60 s idle (Jellyfin's constants: `WebSocketLostTimeout=60`, factors 0.75/0.2). Ignore `SessionsStart/Stop`, `ActivityLogEntryStart/Stop`, `ScheduledTasksInfoStart/Stop`. Push `LibraryChanged` on `Hub` library events (`{"ItemsAdded":[],"ItemsUpdated":[],"ItemsRemoved":[],"FoldersAddedTo":[],"FoldersRemovedFrom":[],"CollectionFolders":["03…"],"IsEmpty":false}` **(Data shape unverified)**) and `UserDataChanged` (`{"UserId":"02…","UserDataList":[UserItemDataDto]}` **(unverified)**) after state changes made through this router. Same `max_message_size` and same-origin rules as `v1::events` are unnecessary here (no cookies).

## 4. Server discovery (UDP 7359, MS16c, opt-in via `jellyfin_discovery`)

Verified protocol (Jellyfin 10.8 `UdpServer.cs`, Kotlin SDK, a captured reply): bind `0.0.0.0:7359` UDP; any datagram whose UTF-8 text **contains `"who is JellyfinServer?"` case-insensitively** (clients send `Who is JellyfinServer?`) gets a JSON reply **to the sender's endpoint**: `{"Address":"http://192.168.1.5:8096","Id":"<32hex>","Name":"<display name>","EndpointAddress":null}`. `Address` = the plain-HTTP listener on the interface facing the sender (`librqbit_upnp::get_local_ip_relative_to`, as SSDP does); if `jellyfin_port = 0`, the HTTPS URL + `/jellyfin`. Reply only to `is_local` senders, rate-limit 5/s, run as a task in `remote::apply` next to the listener; IPv4 only (Jellyfin binds `IPAddress.Any`). Also keep advertising on mDNS as today (unrelated).

## 5. Config, UI, docs

- `server.toml`: `jellyfin_api = false`, `jellyfin_port = 8096`, `jellyfin_discovery = true`, `jellyfin_version = "12.2.0"`, `jellyfin_public_images = false`. `PUT /api/v1/server/remote` carries `jellyfin_api`/`jellyfin_port` (`RemoteConfig`); status shows the listener port/error and the two URLs to type.
- Web `Remote.svelte`, group "On this network", after DLNA: Toggle **"Let Jellyfin apps connect"** (hint: "Jellyfin Web, Android TV, Swiftfin, Findroid, Infuse and the Kodi add-on can sign in with their Corrent account and play the libraries. Apps on this network use http://<ip>:8096 (plain HTTP, local network only); browsers that trust the server's certificate can also use https://<host>:8443/jellyfin."), a port field, a "Announce to Jellyfin apps (UDP 7359)" toggle, and status lines/errors like DLNA's. New strings → `assets/i18n/tr.toml` (gate `turkish_catalog_is_complete`). The native app has no remote-access page today (DLNA toggle is web-only), so no egui work.
- Docs: `docs/SERVER.md` new section "Jellyfin apps" (what works, the two URLs, TLS caveat, that Corrent reports itself as "Jellyfin Server 12.2.0" because the apps refuse anything else, which auth headers are accepted, anonymous-with-tag images, limitations: no admin dashboard, no playlists/collections/live TV/SyncPlay/trickplay, embedded text subtitles burn in, music basics only, Android phone app shows Corrent's own web client); `docs/MEDIASERVER.md` MS16 status row + deviations; CLAUDE.md status line; `docs/ROADMAP.md` if it lists E12.

## 6. Test strategy (no real apps needed)

Reuse `api_tests.rs`'s harness (`harness`, `call`, `admin_token`, photo/MKV fixtures) by making those helpers `pub(crate)`; new tests live in `crates/corrent-server/src/jellyfin/tests.rs`. Request sequences and fixtures copy what the real clients send (captured here from source): keep `fixtures/deviceprofile-web.json` (jellyfin-web's `browserDeviceProfile` shape), `fixtures/deviceprofile-kodi.json`, `fixtures/deviceprofile-androidtv.json` **(unverified contents — capture once from a real app/HAR during manual testing and freeze)**.

1. `jellyfin_handshake_and_login` — `/jellyfin/System/Info/Public` (anon; `ProductName == "Jellyfin Server"`, 3-part version ≥ 12, 32-hex `Id`), `/jellyfin/system/info/public` (lowercase), `Users/Public`, `Branding/Configuration`, `QuickConnect/Enabled`; `POST Users/AuthenticateByName` with `Authorization: MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="abc", Version="10.10.7"` → token; wrong password 401 + limiter; `GET Users/Me` via `Authorization: MediaBrowser … Token="…"`, `X-Emby-Authorization`, `X-Emby-Token`, `?api_key`, `?ApiKey`; cookie refused; same DeviceId twice → one device row in `/api/v1/devices` with client `Jellyfin Web 10.10.7`; `Sessions/Logout` revokes; `jellyfin_api=false` → 404; `System/Info` requires auth; CORS preflight 204.
2. `jellyfin_browse_and_userdata` (photos, no libmpv) — `UserViews` (CollectionType `photos`), `Users/{u}/Views` legacy, `Items?ParentId=<view>&Recursive=true&IncludeItemTypes=Photo&SortBy=SortName&SortOrder=Ascending&StartIndex=0&Limit=1` (`TotalRecordCount 2`, `Type Photo`, `ImageTags.Primary`), `Users/{u}/Items/{id}`, `Items/{id}/Images/Primary?tag=<sha>&maxWidth=200` anonymous 200 + immutable + 304 on ETag, without tag and auth → 401, `UserFavoriteItems` POST/DELETE, `UserPlayedItems`, `Items?Filters=IsFavorite`, `Items?SearchTerm=fore`, `Items?Ids=`, guest without the library sees no view and gets 404 on the item; bad GUIDs → 404; `DisplayPreferences/usersettings?client=emby` GET defaults, POST 204, GET echoes `CustomPrefs`; `Users/Configuration` round-trip.
3. `jellyfin_shows_next_up` (shows library from named small files; scans work unprobed without libmpv) — `Shows/{id}/Seasons`, `Shows/{id}/Episodes?SeasonId`, `Items?ParentId=<show>&Recursive=true&IncludeItemTypes=Episode`, mark E01 played → `Shows/NextUp` = E02 (+ `SeriesId`, `DisableFirstEpisode`), `Items/Latest?ParentId=<view>` array, episode DTO has `SeriesId/SeasonId/ParentIndexNumber/IndexNumber`, `UserItems/Resume` after a `Sessions/Playing/Progress`.
4. `jellyfin_playback` (skips without libmpv, like `playback_direct_and_hls`) — encode the MKV fixture + `.en.srt`; `PlaybackInfo` with the web profile → `SupportsDirectPlay false`, `TranscodingUrl` with `PlaySessionId`, subtitle `DeliveryMethod External` + `DeliveryUrl`; no HLS session exists yet (`/api/v1/playback/sessions` empty); GET master → variant `main.m3u8?`; GET main → `hls1/main/0.ts?…api_key`; GET segment 0 → 200 `video/mp2t`; DeliveryUrl → VTT; `Sessions/Playing/Progress {PositionTicks: 50000000}` → `UserItems/{id}/UserData.PlaybackPositionTicks`; `DELETE Videos/ActiveEncodings?playSessionId` → session gone; with an mkv-capable profile → `SupportsDirectPlay true`, `Videos/{id}/stream.mkv?Static=true&MediaSourceId&api_key` with Range → 206, HEAD → headers only; `Audio/{id}/universal` on a FLAC fixture → direct 200; stream indices unique across types.
5. `jellyfin_websocket` — real `serve_http` + `tokio-tungstenite` (pattern of `events_stream_full_list_then_deltas`): first frame `ForceKeepAlive`/`Data 60`, `KeepAlive` echoed, `LibraryChanged` after a scan.
6. `jellyfin_udp_discovery` — responder on an ephemeral port (port configurable for tests), send `Who is JellyfinServer?` → JSON with the four fields; a non-local sender gets nothing.
7. Unit tests: auth header parser (quotes, URL-encoding, commas inside quotes, scheme case, fallbacks order), GUID codec round-trip/garbage, ticks/date formatting, `DeviceProfile → ClientProfile` for the three fixtures, `/Items` query translation table, playlist rewriting, plain-HTTP listener refuses a non-local peer.
8. `api_inputs_are_refused_cleanly`-style sweep over the Jellyfin routes (fuzz-ish garbage GUIDs, `Limit=1e9`, negative `StartIndex`, 200 KB DeviceProfile, malformed `Authorization`) → never 5xx.

Manual notes (for the owner's machine; portable profiles only): jellyfin-web static build served by `npx serve` + "Add server" `https://host:8443/jellyfin` (exercises CORS); Android TV / Findroid in an emulator against `http://<lan-ip>:8096`; Kodi (Flatpak) with the add-on against the same; record a HAR from Jellyfin Web against a real `jellyfin/jellyfin:12` container once to freeze the request sequence into the fixtures.

## 7. Phasing, size, risks

| Phase | Content | ≈ lines |
|---|---|---|
| **MS16a** | module skeleton, mounting (nest_service + lowercase + CORS + gate), plain-HTTP 8096 listener, `JfAuth`, GUID codec, DTOs, §3.1–3.5 (handshake, auth/users/devices + migration 0009, views, items + query translation, item detail, latest/resume/next-up/seasons/episodes, images, display prefs, stubs), proto/library additions, config + Remote page + i18n + docs, tests 1,2,3,7,8 | ~1400 Rust, ~80 Svelte |
| **MS16b** | §3.6–3.7: PlaybackInfo + DeviceProfile mapping + MediaSourceInfo/MediaStream, play sessions, stream/HLS/audio/subtitles routes, ActiveEncodings, playstate + user data, test 4, docs | ~950 |
| **MS16c** | UDP discovery, `/socket` (keepalive + LibraryChanged/UserDataChanged), root alias on HTTPS, `/Artists|Persons|Genres|Filters2|Items/Counts|MediaSegments(chapters)|Download`, Sessions GET, tests 5,6 | ~500 |

Risks (honest):
1. **Identity spoofing is mandatory**: clients gate on `ProductName == "Jellyfin Server"` and version ≥ 12.0.0; claiming 12.x invites calls to 12-only endpoints (MediaSegments, Lyrics, Trickplay, SyncPlay…) — every unknown route must fail softly (404/empty, never 401); future client bumps may need `jellyfin_version` raised.
2. **TLS**: native clients won't trust our CA; the 8096 plain-HTTP listener is the practical path and means bearer tokens cross the LAN unencrypted (same as stock Jellyfin; documented, off by default, LAN-only, never over Connect).
3. **Anonymous art**: some clients may omit `tag` → missing art unless `jellyfin_public_images` (then any LAN peer can enumerate artwork by id, as with stock Jellyfin).
4. **Stream index collisions** (per-type `idx`) must be confirmed in `probe.rs` before MS16b; the offset scheme fixes it either way.
5. **Container/codec mapping** drives direct-play vs transcode; mistakes cost encoder slots (max 3) or failed playback — fixtures from real profiles are the guard.
6. **Progressive transcodes** (`/Videos/{id}/stream` without `Static`, `/Audio/{id}/universal` with `TranscodingProtocol=http`) are not supported → 400; verified that jellyfin-web asks HLS for video; Kodi/ATV audio behaviour **(unverified)**.
7. **Episode artwork** needs the `ImageRef.inherited` flag; without it episodes show posters.
8. **Kodi full sync** asks `Fields=MediaSources,People` on big pages → N detail queries; cap `Limit`-with-heavy-fields at 200 and add the LRU; measure with the 5k-item perf fixture.
9. **Root alias** touches the SPA fallback; the marker/segment rule is covered by tests but keep it a flag.
10. **Legacy auth permissiveness** (`api_key`, `X-Emby-*`) is intentionally broader than Jellyfin 12 defaults; tokens in query strings must never be logged (check no URI-logging layer; redact in `tracing` if one is added).
11. Not planned: admin dashboard, playlists/collections, Live TV, SyncPlay API, trickplay tiles, lyrics, embedded-subtitle extraction, server-hosted jellyfin-web for the Android phone app (it will show Corrent's own web client **(unverified)**).

## 8. Verification ledger

Verified from source: `AuthorizationContext.cs` (10.10 + master gating), `BaseItemDto`, `UserDto`, `UserPolicy`, `UserConfiguration`, `AuthenticationResult`/`SessionInfoDto`, `PublicSystemInfo`/`SystemInfo`, `UserItemDataDto`, `QueryResult`, `MediaSourceInfo`, `MediaStream` (+ `DisplayTitle`/`IsTextSubtitleStream` rules), `PlaybackInfoResponse`, `PlaybackProgressInfo`/`StopInfo`/`StartInfo`, `PlayMethod`, `DeviceProfile` + `DirectPlayProfile`/`TranscodingProfile`/`CodecProfile`/`SubtitleProfile`/`ProfileCondition*`, `SubtitleDeliveryMethod`, `MediaProtocol`, `MediaStreamType`, `ImageType`, `ItemSortBy`, `ItemFilter`, `BaseItemKind`, `CollectionType`, `MediaType`, `PersonKind`, `TranscodeReason`, `ChapterInfo`, `BaseItemPerson`, `DisplayPreferencesDto`, `BrandingOptions`, `ClientCapabilitiesDto`, `JsonDefaults`/`JsonGuidConverter` (PascalCase, nulls omitted, GUID `N`), `ServerConfiguration.CorsHosts=["*"]`, controllers Users/Items/UserLibrary/Playstate/MediaInfo/TvShows/UserViews/Image/Videos/DynamicHls/UniversalAudio/Subtitle/Session/DisplayPreferences/System/Branding/QuickConnect/Library/Artists/Filter/Localization/MediaSegments (routes, params, auth attributes, 204s), `DynamicHlsHelper` master playlist + `DynamicHlsPlaylistGenerator` media playlist format, `StreamInfo.ToUrl` URL shapes + params, `MediaInfoHelper` (PlaySessionId `N`, flags, TranscodingUrl, DeliveryUrl), 10.8 `UdpServer.cs` + captured reply, WebSocket keep-alive (`SessionWebSocketListener`, `WebSocketConnection`, `ForceKeepAliveMessage`, `SessionMessageType`), jellyfin-apiclient-javascript (header, lowercase paths, endpoint list, `/socket` URL, connection flow), jellyfin-web 10.10 `browserDeviceProfile.js`, master home sections (resume/nextUp/recentlyAdded/liveTv params), playbackmanager (direct/transcode URL building, 10 s progress), jellyfin-kodi `api.py` endpoint list, jellyfin-android `WebViewFragment` (loads server root), Kotlin SDK `Jellyfin.minimumVersion=12.0.0`, `RecommendedServerDiscovery` scoring, `ServerVersion.fromString`, `QUERY_ACCESS_TOKEN="ApiKey"`, GitHub releases v12.0–v12.2 and the 12.0 notes.
Unverified (marked inline): Android TV/Swiftfin/Infuse exact call lists and TLS behaviour, Kotlin SDK default-port candidates and whether its stream URL builders append `ApiKey`, UDP server source for 10.10+/12 (only 10.8 fetched; protocol unchanged per the SDK and a live capture), `LibraryChanged`/`UserDataChanged`/`MediaSegmentDto`/`ThemeMediaResult.OwnerId` payload shapes, jellyfin-web's `DisplayPreferences/usersettings?client=emby` call, JMP bundling jellyfin-web, the Android phone app working against a non-Jellyfin root page, `MimeTypes.GetMimeType("playlist.m3u8")` value.

Note for the implementer: while I was reading, `library.rs`, `query.rs`, `v1/mod.rs`, `lib.rs`, `Cargo.toml`, `SERVER.md` were touched on disk by someone else, but `git diff` shows only `Cargo.lock` changed, so the signatures referenced above (`ItemQuery`, `Item`, `Library::{items,item,main_file,file_path,subtitle_path,search,home,set_state,user_libraries}`, `identity::{issue,verify_token,authenticate}`, `v1::playback::prepare`, `HlsManager::{create,playlist,segment,owner,stop}`, `remote::apply`, `dlna` listener pattern) are current.
---

# Review (Fable) — corrections folded into the implementation

P0 (blocking): (1) query-string keys and `MediaBrowser` header keys matched case-insensitively
everywhere (one `JfQuery` extractor); (2) `GET /Sessions` returns the caller's own session from
MS16a (Kodi indexes `[0]`), plus 204 stubs for `POST /Sessions/{id}/Playing` and
`POST|DELETE /Sessions/{id}/User/{userId}`; (3) podcasts aren't typed by item kind — podcast
libraries are left out of the Jellyfin views for now; `MediaType` comes from the main file's
streams (no video → Audio); (4) CORS off by default (`jellyfin_cors = false`); (5) realistic size
≈ 4k lines + tests for 16a+b; responses are built with `serde_json::json!`, only inputs are typed;
MS16c is a separate decision.

P1: stubs for `Movies/Recommendations`, `Shows/Upcoming`, `Users/{id}/Suggestions`,
`Items/Suggestions`, `Items/{id}/Images` (`[]`), `POST Items/{id}/Refresh`,
`System/Configuration/encoding`; `Items/{id}` resolves every id kind (views, people, genres);
music/TV query parameters (`ArtistIds`, `AlbumArtistIds`, `AlbumIds`, `IsFolder`, `Years`);
seasons carry `SeriesId`/`SeriesName`/series image tags; `AuthenticateByName` without device
info synthesizes a device id; `Identity::signin_users()`; migrations array; `Applied` key includes
the Jellyfin settings; `StartTimeTicks` ignored on full-VOD playlists; the Remote page tells
people to type the IP (or add a host name).

P2: Connect peers count as local (documented); `OperatingSystem` omitted; photos served from
the original for `Primary`; `GroupItems` ignored; root alias (MS16c) off or dropped.

Order: foundations (nest, lowercase paths, case-insensitive query, auth, ids/ticks, errors,
listener, migration) → users/sessions → browse → playback/playstate → fuzz, docs, UI →
optional UDP 7359 discovery and `/socket` keep-alive.
