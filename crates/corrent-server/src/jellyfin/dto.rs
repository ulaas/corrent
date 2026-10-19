//! Jellyfin's JSON shapes (PascalCase, nulls left out), built from Corrent's types.

use corrent_proto::auth::{Perms, User};
use corrent_proto::library::{ImageRef, Item, ItemDetail, ItemKind, Library, LibraryKind, MediaFile, Stream};
use serde_json::{Map, Value, json};

use super::ids;

/// What every DTO needs to know about the server.
pub(crate) struct Ctx {
    pub server_id: String,
    pub server_name: String,
}

/// Leaves out nulls (Jellyfin's `WhenWritingNull`).
pub(crate) fn clean(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            Value::Object(m.into_iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k, clean(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(clean).collect()),
        v => v,
    }
}

pub(crate) fn collection_type(k: LibraryKind) -> Option<&'static str> {
    Some(match k {
        LibraryKind::Movies => "movies",
        LibraryKind::Shows => "tvshows",
        LibraryKind::Music => "music",
        LibraryKind::Audiobooks => "books",
        LibraryKind::Videos => "homevideos",
        LibraryKind::Photos => "photos",
        // Audio "episodes" would play as videos in the apps: not shown.
        LibraryKind::Podcasts => return None,
    })
}

pub(crate) fn user_dto(c: &Ctx, u: &User, config: Option<Value>) -> Value {
    let transcode = u.perms.contains(Perms::PLAYBACK_TRANSCODE);
    let mut configuration = json!({
        "PlayDefaultAudioTrack": true, "SubtitleLanguagePreference": "", "DisplayMissingEpisodes": false,
        "GroupedFolders": [], "SubtitleMode": "Default", "DisplayCollectionsView": false, "EnableLocalPassword": false,
        "OrderedViews": [], "LatestItemsExcludes": [], "MyMediaExcludes": [], "HidePlayedInLatest": true,
        "RememberAudioSelections": true, "RememberSubtitleSelections": true, "EnableNextEpisodeAutoPlay": true,
        "CastReceiverId": "",
    });
    if let (Some(Value::Object(stored)), Value::Object(base)) = (config, &mut configuration) {
        for (k, v) in stored {
            base.insert(k, v);
        }
    }
    clean(json!({
        "Name": if u.display_name.trim().is_empty() { u.name.clone() } else { u.display_name.clone() },
        "ServerId": c.server_id,
        "ServerName": c.server_name,
        "Id": ids::enc(ids::USER, u.id),
        "HasPassword": true,
        "HasConfiguredPassword": true,
        "HasConfiguredEasyPassword": false,
        "EnableAutoLogin": false,
        "LastLoginDate": ids::date(u.created_at),
        "LastActivityDate": ids::date(corrent_core::util::now_unix()),
        "Configuration": configuration,
        "Policy": {
            // Never: Jellyfin Web would show its admin dashboard (Corrent's own UIs administer).
            "IsAdministrator": false, "IsHidden": false, "IsDisabled": u.disabled,
            "EnableUserPreferenceAccess": true, "AccessSchedules": [], "BlockUnratedItems": [], "BlockedTags": [],
            "AllowedTags": [], "EnableRemoteControlOfOtherUsers": false, "EnableSharedDeviceControl": false,
            "EnableRemoteAccess": true, "EnableLiveTvManagement": false, "EnableLiveTvAccess": false,
            "EnableMediaPlayback": true,
            "EnableAudioPlaybackTranscoding": transcode, "EnableVideoPlaybackTranscoding": transcode,
            "EnablePlaybackRemuxing": transcode, "ForceRemoteSourceTranscoding": false, "EnableContentDeletion": false,
            "EnableContentDeletionFromFolders": [], "EnableContentDownloading": true, "EnableSyncTranscoding": false,
            "EnableMediaConversion": false, "EnabledDevices": [], "EnableAllDevices": true, "EnabledChannels": [],
            "EnableAllChannels": false, "EnabledFolders": [], "EnableAllFolders": true, "InvalidLoginAttemptCount": 0,
            "LoginAttemptsBeforeLockout": -1, "MaxActiveSessions": 0, "EnablePublicSharing": false,
            "BlockedMediaFolders": [], "BlockedChannels": [], "RemoteClientBitrateLimit": 0,
            "AuthenticationProviderId": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
            "PasswordResetProviderId": "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
            "SyncPlayAccess": "None", "EnableCollectionManagement": false, "EnableSubtitleManagement": false,
            "EnableLyricManagement": false,
        },
    }))
}

pub(crate) fn session_dto(c: &Ctx, u: &User, session: i64, dev: &super::DeviceCtx, ip: &str) -> Value {
    let now = ids::date(corrent_core::util::now_unix());
    json!({
        "Id": format!("{session:032x}"),
        "UserId": ids::enc(ids::USER, u.id),
        "UserName": u.name,
        "Client": dev.client,
        "DeviceName": dev.device,
        "DeviceId": dev.device_id,
        "ApplicationVersion": dev.version,
        "ServerId": c.server_id,
        "LastActivityDate": now,
        "LastPlaybackCheckIn": now,
        "IsActive": true,
        "SupportsMediaControl": false,
        "SupportsRemoteControl": false,
        "PlayableMediaTypes": [],
        "SupportedCommands": [],
        "HasCustomDeviceName": false,
        "AdditionalUsers": [],
        "RemoteEndPoint": ip,
    })
}

pub(crate) fn view_dto(c: &Ctx, l: &Library) -> Value {
    let id = ids::enc(ids::VIEW, l.id);
    clean(json!({
        "Name": l.name, "ServerId": c.server_id, "Id": id, "Etag": l.id.to_string(), "Type": "CollectionFolder",
        "CollectionType": collection_type(l.kind), "IsFolder": true, "ChildCount": l.item_count,
        "DisplayPreferencesId": id, "ImageTags": {}, "BackdropImageTags": [], "LocationType": "FileSystem",
        "MediaType": "Unknown", "PlayAccess": "Full", "SortName": l.name.to_lowercase(),
        "ParentId": ids::enc(ids::VIEW, 0),
        "UserData": { "PlaybackPositionTicks": 0, "PlayCount": 0, "IsFavorite": false, "Played": false, "Key": id, "ItemId": id },
    }))
}

pub(crate) fn root_folder(c: &Ctx) -> Value {
    let id = ids::enc(ids::VIEW, 0);
    json!({ "Name": "Media Folders", "ServerId": c.server_id, "Id": id, "Type": "UserRootFolder", "IsFolder": true,
            "LocationType": "FileSystem", "MediaType": "Unknown", "ImageTags": {}, "BackdropImageTags": [] })
}

pub(crate) fn item_type(k: ItemKind, lib: Option<LibraryKind>) -> &'static str {
    match k {
        ItemKind::Movie => "Movie",
        ItemKind::Show => "Series",
        ItemKind::Season => "Season",
        ItemKind::Episode => "Episode",
        ItemKind::Artist => "MusicArtist",
        ItemKind::Album => "MusicAlbum",
        ItemKind::Track => "Audio",
        ItemKind::Book => "AudioBook",
        ItemKind::Video => "Video",
        ItemKind::Photo => "Photo",
        ItemKind::Folder if lib == Some(LibraryKind::Photos) => "PhotoAlbum",
        ItemKind::Folder => "Folder",
    }
}

/// Jellyfin's `IncludeItemTypes` names → our kinds (unknown names: nothing).
pub(crate) fn kinds_of(name: &str) -> Option<Vec<ItemKind>> {
    Some(match name.to_ascii_lowercase().as_str() {
        "movie" => vec![ItemKind::Movie],
        "series" => vec![ItemKind::Show],
        "season" => vec![ItemKind::Season],
        "episode" => vec![ItemKind::Episode],
        "musicartist" => vec![ItemKind::Artist],
        "musicalbum" => vec![ItemKind::Album],
        "audio" => vec![ItemKind::Track],
        "audiobook" | "book" => vec![ItemKind::Book],
        "video" => vec![ItemKind::Video],
        "photo" => vec![ItemKind::Photo],
        "folder" | "photoalbum" | "collectionfolder" => vec![ItemKind::Folder],
        _ => return None,
    })
}

pub(crate) fn media_type(k: ItemKind) -> &'static str {
    match k {
        ItemKind::Movie | ItemKind::Episode | ItemKind::Video => "Video",
        ItemKind::Track | ItemKind::Book => "Audio",
        ItemKind::Photo => "Photo",
        _ => "Unknown",
    }
}

/// Our image kinds → Jellyfin's.
fn image_type(kind: &str, item: ItemKind) -> Option<&'static str> {
    Some(match kind {
        "poster" | "cover" | "primary" => "Primary",
        "backdrop" | "fanart" => "Backdrop",
        "logo" | "clearlogo" => "Logo",
        "thumb" if matches!(item, ItemKind::Photo | ItemKind::Video | ItemKind::Episode) => "Primary",
        "thumb" => "Thumb",
        "banner" => "Banner",
        _ => return None,
    })
}

/// The image of a Jellyfin type an item shows (own first, then inherited).
pub(crate) fn image_for<'a>(it: &'a Item, ty: &str) -> Option<&'a ImageRef> {
    let mut found: Vec<&ImageRef> =
        it.images.iter().filter(|i| image_type(&i.kind, it.kind).is_some_and(|t| t.eq_ignore_ascii_case(ty))).collect();
    found.sort_by_key(|i| i.inherited);
    found.first().copied()
}

pub(crate) fn user_data(it: &Item) -> Value {
    let id = ids::enc(ids::ITEM, it.id);
    let s = &it.state;
    let pct = it.runtime_ms.filter(|r| *r > 0 && s.position_ms > 0).map(|r| s.position_ms as f64 * 100.0 / r as f64);
    clean(json!({
        "Rating": s.rating, "PlayedPercentage": pct, "UnplayedItemCount": s.unplayed,
        "PlaybackPositionTicks": ids::ticks(s.position_ms), "PlayCount": s.play_count, "IsFavorite": s.favorite,
        "LastPlayedDate": s.last_played_at.map(ids::date), "Played": s.played, "Key": id, "ItemId": id,
    }))
}

/// A list/grid item (and with `detail`, the full item page).
pub(crate) fn item_dto(c: &Ctx, it: &Item, lib: Option<LibraryKind>, detail: Option<&ItemDetail>) -> Value {
    let id = ids::enc(ids::ITEM, it.id);
    let parent = it.parent_id.map(|p| ids::enc(ids::ITEM, p)).unwrap_or_else(|| ids::enc(ids::VIEW, it.library_id));
    let mut tags = Map::new();
    let mut backdrops: Vec<String> = Vec::new();
    let mut hashes = Map::new();
    let mut parent_tags = Map::new();
    for img in &it.images {
        let Some(ty) = image_type(&img.kind, it.kind) else { continue };
        if !img.blurhash.is_empty() {
            let e = hashes.entry(ty.to_string()).or_insert_with(|| json!({}));
            e[&img.sha] = json!(img.blurhash);
        }
        if img.inherited {
            parent_tags.entry(ty.to_string()).or_insert_with(|| json!(img.sha));
        } else if ty == "Backdrop" {
            backdrops.push(img.sha.clone());
        } else {
            tags.entry(ty.to_string()).or_insert_with(|| json!(img.sha));
        }
    }
    let primary = image_for(it, "Primary").filter(|i| !i.inherited);
    let aspect = primary.filter(|i| i.height > 0).map(|i| i.width as f64 / i.height as f64);
    let mut v = json!({
        "Name": it.title, "ServerId": c.server_id, "Id": id, "Etag": it.added_at.to_string(),
        "DateCreated": ids::date(it.added_at), "SortName": it.title.to_lowercase(), "Type": item_type(it.kind, lib),
        "MediaType": media_type(it.kind), "IsFolder": !it.kind.playable(), "ParentId": parent,
        "ProductionYear": it.year, "RunTimeTicks": it.runtime_ms.map(ids::ticks), "IndexNumber": it.index,
        "ChildCount": it.child_count, "LocationType": "FileSystem", "PlayAccess": "Full", "CanDelete": false,
        "CanDownload": it.kind.playable(), "LockData": false, "LockedFields": [], "Taglines": [], "Tags": [],
        "Studios": [], "Genres": [], "GenreItems": [], "ProviderIds": {}, "ExternalUrls": [], "RemoteTrailers": [],
        "ImageTags": tags, "BackdropImageTags": backdrops, "ImageBlurHashes": hashes,
        "PrimaryImageAspectRatio": aspect, "UserData": user_data(it), "VideoType": (media_type(it.kind) == "Video").then_some("VideoFile"),
    });
    match it.kind {
        ItemKind::Episode => {
            v["SeriesName"] = json!(it.context.first());
            v["SeasonName"] = json!(it.context.get(1));
            v["SeasonId"] = json!(it.parent_id.map(|p| ids::enc(ids::ITEM, p)));
            v["SeriesId"] = json!(it.grandparent_id.map(|p| ids::enc(ids::ITEM, p)));
            v["ParentIndexNumber"] = json!(it.parent_index);
        }
        ItemKind::Season => {
            v["SeriesName"] = json!(it.context.last());
            v["SeriesId"] = json!(it.parent_id.map(|p| ids::enc(ids::ITEM, p)));
        }
        ItemKind::Track => {
            v["Album"] = json!(it.context.get(1));
            v["AlbumId"] = json!(it.parent_id.map(|p| ids::enc(ids::ITEM, p)));
            v["AlbumArtist"] = json!(it.context.first());
            if let Some(a) = it.context.first() {
                let aid = it.grandparent_id.map(|p| ids::enc(ids::ITEM, p));
                v["AlbumArtists"] = json!([{ "Name": a, "Id": aid }]);
                v["Artists"] = json!([a]);
                v["ArtistItems"] = json!([{ "Name": a, "Id": aid }]);
            }
        }
        ItemKind::Album => {
            if let Some(a) = it.context.first() {
                let aid = it.parent_id.map(|p| ids::enc(ids::ITEM, p));
                v["AlbumArtist"] = json!(a);
                v["AlbumArtists"] = json!([{ "Name": a, "Id": aid }]);
                v["Artists"] = json!([a]);
            }
        }
        _ => {}
    }
    // Images of the show/season (episodes) or album (tracks).
    let owner = it.grandparent_id.or(it.parent_id).map(|p| ids::enc(ids::ITEM, p));
    if let Some(sha) = parent_tags.get("Primary") {
        match it.kind {
            ItemKind::Episode | ItemKind::Season => v["SeriesPrimaryImageTag"] = sha.clone(),
            ItemKind::Track => v["AlbumPrimaryImageTag"] = sha.clone(),
            _ => {}
        }
        v["ParentPrimaryImageItemId"] = json!(owner);
        v["ParentPrimaryImageTag"] = sha.clone();
    }
    if let Some(sha) = parent_tags.get("Backdrop") {
        v["ParentBackdropItemId"] = json!(owner);
        v["ParentBackdropImageTags"] = json!([sha]);
    }
    if let Some(sha) = parent_tags.get("Thumb") {
        v["ParentThumbItemId"] = json!(owner);
        v["ParentThumbImageTag"] = sha.clone();
    }
    if let Some(sha) = parent_tags.get("Logo") {
        v["ParentLogoItemId"] = json!(owner);
        v["ParentLogoImageTag"] = sha.clone();
    }
    if let Some(d) = detail {
        add_detail(c, &mut v, it, d);
    }
    clean(v)
}

fn person_type(role: &str) -> &'static str {
    match role {
        "actor" | "guest" => "Actor",
        "director" => "Director",
        "writer" => "Writer",
        "composer" => "Composer",
        "producer" => "Producer",
        "artist" => "Artist",
        _ => "Unknown",
    }
}

fn provider_key(k: &str) -> String {
    match k {
        "tmdb" => "Tmdb".into(),
        "imdb" => "Imdb".into(),
        "tvdb" => "Tvdb".into(),
        "musicbrainz" | "mb_album" => "MusicBrainzAlbum".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn add_detail(c: &Ctx, v: &mut Value, it: &Item, d: &ItemDetail) {
    v["OriginalTitle"] = json!(d.original_title);
    v["Overview"] = json!(d.overview);
    v["CommunityRating"] = json!(d.rating);
    v["OfficialRating"] = json!(d.official_rating);
    v["PremiereDate"] = json!(d.premiered.as_ref().map(|p| format!("{}T00:00:00.0000000Z", p.get(..10).unwrap_or(p))));
    v["Genres"] = json!(d.genres);
    v["GenreItems"] = json!(d.genres.iter().map(|g| json!({ "Name": g, "Id": genre_id(g) })).collect::<Vec<_>>());
    let mut prov = Map::new();
    for (k, val) in &d.provider_ids {
        prov.insert(provider_key(k), json!(val));
    }
    if let Some(imdb) = d.provider_ids.get("imdb") {
        v["ExternalUrls"] = json!([{ "Name": "IMDb", "Url": format!("https://www.imdb.com/title/{imdb}") }]);
    }
    v["ProviderIds"] = Value::Object(prov);
    v["People"] = json!(
        d.credits
            .iter()
            .take(100)
            .map(|p| json!({ "Name": p.name, "Id": ids::enc(ids::PERSON, p.person_id), "Role": p.character.clone().unwrap_or_default(), "Type": person_type(&p.role) }))
            .collect::<Vec<_>>()
    );
    if let Some(f) = d.files.iter().find(|f| f.role == "main" && !f.missing) {
        v["Container"] = json!(container(f));
        v["MediaSources"] = json!([media_source(c, it, f, None)]);
        v["MediaStreams"] = json!(media_streams(it, f, None));
        v["HasSubtitles"] = json!(f.streams.iter().any(|s| s.kind == "subtitle"));
        if let Some(vs) = f.streams.iter().find(|s| s.kind == "video") {
            v["Width"] = json!(vs.width);
            v["Height"] = json!(vs.height);
            v["IsHD"] = json!(vs.height.is_some_and(|h| h >= 720));
        }
        v["MediaSourceCount"] = json!(1);
        v["PartCount"] = json!(1);
        v["Chapters"] = json!(
            f.chapters
                .iter()
                .map(|ch| json!({ "StartPositionTicks": ids::ticks(ch.start_ms), "Name": ch.title.clone().unwrap_or_default() }))
                .collect::<Vec<_>>()
        );
    }
}

/// A genre's id: a stable number from its name (genres have no ids of their own here).
pub(crate) fn genre_id(name: &str) -> String {
    let h = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, name.to_lowercase().as_bytes());
    let mut n = [0u8; 8];
    n.copy_from_slice(&h.as_ref()[..8]);
    ids::enc(ids::GENRE, (u64::from_be_bytes(n) >> 1) as i64)
}

pub(crate) fn container(f: &MediaFile) -> String {
    std::path::Path::new(&f.path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Stream numbers unique across kinds (mpv numbers each kind from 1): video as is, audio
/// +100, subtitles +200 (external subtitles are already ≥ 1000).
pub(crate) fn stream_index(s: &Stream) -> i64 {
    match s.kind.as_str() {
        "audio" => 100 + s.index,
        "subtitle" => 200 + s.index,
        _ => s.index,
    }
}

const IMAGE_SUBS: [&str; 6] = ["hdmv_pgs_subtitle", "pgs", "dvd_subtitle", "dvdsub", "dvb_subtitle", "sup"];

pub(crate) fn text_sub(s: &Stream) -> bool {
    !s.codec.as_deref().is_some_and(|c| IMAGE_SUBS.contains(&c))
}

fn channels_layout(n: i64) -> &'static str {
    match n {
        1 => "mono",
        2 => "stereo",
        6 => "5.1",
        8 => "7.1",
        _ => "",
    }
}

/// `MediaStreams`; `token` makes subtitle `DeliveryUrl`s (playback info).
pub(crate) fn media_streams(it: &Item, f: &MediaFile, token: Option<&str>) -> Vec<Value> {
    let item = ids::enc(ids::ITEM, it.id);
    let source = ids::enc(ids::FILE, f.id);
    f.streams
        .iter()
        .map(|s| {
            let idx = stream_index(s);
            let codec = s.codec.clone().unwrap_or_default();
            let lang = s.lang.clone();
            let (ty, display) = match s.kind.as_str() {
                "video" => {
                    let res = s.height.map(|h| format!("{h}p ")).unwrap_or_default();
                    ("Video", format!("{res}{} SDR", codec.to_uppercase()))
                }
                "audio" => {
                    let parts: Vec<String> = [
                        lang.clone().map(|l| l.to_uppercase()),
                        Some(codec.to_uppercase()),
                        s.channels.map(|c| channels_layout(c).to_string()).filter(|c| !c.is_empty()),
                        s.default.then(|| "Default".to_string()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    ("Audio", parts.join(" - "))
                }
                _ => {
                    let parts: Vec<String> = [
                        lang.clone().map(|l| l.to_uppercase()),
                        Some(codec.to_uppercase()),
                        s.external.then(|| "External".to_string()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    ("Subtitle", parts.join(" - "))
                }
            };
            let mut m = json!({
                "Codec": codec, "Language": lang, "Title": s.title, "DisplayTitle": display, "Type": ty, "Index": idx,
                "IsDefault": s.default, "IsForced": s.forced, "IsExternal": s.external, "IsHearingImpaired": false,
                "IsInterlaced": false, "Width": s.width, "Height": s.height, "RealFrameRate": s.fps, "AverageFrameRate": s.fps,
                "Profile": s.profile, "BitRate": s.bitrate, "Channels": s.channels,
                "ChannelLayout": s.channels.map(channels_layout).filter(|c| !c.is_empty()), "SampleRate": s.sample_rate,
            });
            if ty == "Video" {
                m["IsAVC"] = json!(codec == "h264");
                m["VideoRange"] = json!("SDR");
                m["VideoRangeType"] = json!("SDR");
                m["BitDepth"] = json!(if s.profile.as_deref().is_some_and(|p| p.contains("10")) { 10 } else { 8 });
                m["AspectRatio"] = json!(match (s.width, s.height) {
                    (Some(w), Some(h)) if h > 0 && (w * 9 - h * 16).abs() < h => "16:9".to_string(),
                    (Some(w), Some(h)) if h > 0 => format!("{:.2}", w as f64 / h as f64),
                    _ => String::new(),
                });
            }
            if ty == "Audio" {
                m["AudioSpatialFormat"] = json!("None");
            }
            if ty == "Subtitle" {
                let text = text_sub(s);
                m["IsTextSubtitleStream"] = json!(text);
                m["SupportsExternalStream"] = json!(s.external && text);
                if s.external && text {
                    m["DeliveryMethod"] = json!("External");
                    if let Some(t) = token {
                        m["DeliveryUrl"] =
                            json!(format!("/Videos/{item}/{source}/Subtitles/{idx}/0/Stream.vtt?api_key={t}"));
                    }
                } else {
                    m["DeliveryMethod"] = json!("Encode");
                }
            }
            clean(m)
        })
        .collect()
}

/// `MediaSourceInfo` for a file (`play` filled by playback info).
pub(crate) fn media_source(c: &Ctx, it: &Item, f: &MediaFile, play: Option<&super::playback::SourcePlay>) -> Value {
    let _ = c;
    let name = std::path::Path::new(&f.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dur = f.duration_ms.or(it.runtime_ms);
    let bitrate = dur.filter(|d| *d > 0).map(|d| f.size.saturating_mul(8000) / d);
    let default_audio = f
        .streams
        .iter()
        .filter(|s| s.kind == "audio")
        .find(|s| s.default)
        .or(f.streams.iter().find(|s| s.kind == "audio"));
    let default_sub = f.streams.iter().filter(|s| s.kind == "subtitle").find(|s| s.default || s.forced);
    let mut v = json!({
        "Protocol": "File", "Id": ids::enc(ids::FILE, f.id), "Path": format!("{name}.{}", container(f)), "Type": "Default",
        "Container": container(f), "Size": f.size, "Name": name, "IsRemote": false, "ETag": format!("{}-{}", f.id, f.size),
        "RunTimeTicks": dur.map(ids::ticks), "ReadAtNativeFramerate": false, "IgnoreDts": false, "IgnoreIndex": false,
        "GenPtsInput": false, "SupportsTranscoding": false, "SupportsDirectStream": false, "SupportsDirectPlay": true,
        "IsInfiniteStream": false, "RequiresOpening": false, "RequiresClosing": false, "RequiresLooping": false,
        "SupportsProbing": true, "VideoType": "VideoFile", "MediaStreams": media_streams(it, f, play.map(|p| p.token.as_str())),
        "MediaAttachments": [], "Formats": [], "Bitrate": bitrate, "RequiredHttpHeaders": {},
        "DefaultAudioStreamIndex": default_audio.map(stream_index), "DefaultSubtitleStreamIndex": default_sub.map(stream_index),
    });
    if let Some(p) = play {
        v["SupportsDirectPlay"] = json!(p.direct);
        v["SupportsTranscoding"] = json!(p.can_transcode);
        if let Some(u) = &p.transcoding_url {
            v["TranscodingUrl"] = json!(u);
            v["TranscodingSubProtocol"] = json!("hls");
            v["TranscodingContainer"] = json!("ts");
        }
    }
    clean(v)
}
