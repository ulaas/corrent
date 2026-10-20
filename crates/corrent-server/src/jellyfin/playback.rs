//! Playback: `PlaybackInfo` decides (direct play or HLS) from the app's DeviceProfile; the
//! file is served as it is (`Videos/{id}/stream`) or through an HLS session that starts when the
//! app fetches `master.m3u8` (apps ask for info more often than they play). Subtitles are
//! served as WebVTT.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use corrent_proto::auth::Perms;
use corrent_proto::library::{ItemDetail, MediaFile, PlaybackRequest};
use corrent_transcode::decide::{self, ClientProfile, Decision, PlayRequest};
use serde_json::{Value, json};

use super::system::ctx;
use super::{JfAuth, Q, dto, fail, ids, json, no_content, not_found};
use crate::state::AppState;

const PLAY_TTL: Duration = Duration::from_secs(6 * 3600);
const MAX_PLAYS: usize = 1000;

/// A `PlaySessionId` from playback info.
pub(crate) struct PlaySession {
    pub user: i64,
    pub item: i64,
    pub file: i64,
    pub profile: ClientProfile,
    pub audio_track: Option<i64>,
    pub burn_subtitle: Option<i64>,
    pub max_bitrate: Option<i64>,
    /// The HLS session, once the app asked for the playlist.
    pub hls: Option<String>,
    pub created: Instant,
}

/// What playback info decided for a file.
pub(crate) struct SourcePlay {
    pub token: String,
    pub direct: bool,
    pub can_transcode: bool,
    pub transcoding_url: Option<String>,
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/items/{id}/playbackinfo", get(info).post(info))
        .route("/videos/{id}/{name}", get(video))
        .route("/videos/{id}/hls1/{pl}/{seg}", get(segment))
        .route("/audio/{id}/{name}", get(audio))
        .route("/audio/{id}/hls1/{pl}/{seg}", get(segment))
        .route("/videos/{item}/{source}/subtitles/{index}/{start}/{file}", get(subtitle6))
        .route("/videos/{item}/{source}/subtitles/{index}/{file}", get(subtitle5))
        .route("/videos/activeencodings", delete(stop_encoding))
        .route("/items/{id}/download", get(download))
        .route("/items/{id}/file", get(download))
}

// ---- DeviceProfile → ClientProfile

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn containers(name: &str) -> &'static [&'static str] {
    match name {
        "mp4" | "m4v" | "mov" | "m4a" | "m4b" | "3gp" => &["mp4", "mov", "m4a"],
        "mkv" | "matroska" => &["matroska", "mkv"],
        "webm" | "webma" => &["webm"],
        "ts" | "m2ts" | "mpegts" => &["mpegts"],
        "avi" => &["avi"],
        "mpg" | "mpeg" => &["mpeg", "mpegvideo"],
        "flv" => &["flv"],
        "wmv" | "asf" | "wma" => &["asf"],
        "mp3" => &["mp3"],
        "flac" => &["flac"],
        "ogg" | "oga" | "opus" => &["ogg"],
        "wav" => &["wav"],
        "aac" => &["aac"],
        _ => &[],
    }
}

fn codec(c: &str) -> String {
    match c {
        "h265" | "hevc" => "hevc".into(),
        "pcm" => "pcm_s16le".into(),
        other => other.into(),
    }
}

/// What an app's DeviceProfile can play, as the transcode decision understands it.
pub(crate) fn profile_from(p: &Value) -> ClientProfile {
    let mut c = ClientProfile {
        name: "jellyfin".into(),
        containers: Vec::new(),
        video_codecs: Vec::new(),
        audio_codecs: Vec::new(),
        h264_10bit: false,
        max_height: None,
        max_audio_channels: 2,
        max_bitrate: p.get("MaxStreamingBitrate").and_then(Value::as_i64).filter(|b| *b > 0),
        hls: false,
        everything: false,
    };
    let list = |v: &str| -> Vec<String> {
        v.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect()
    };
    for d in p.get("DirectPlayProfiles").and_then(Value::as_array).into_iter().flatten() {
        for ct in list(s(d, "Container")) {
            for m in containers(&ct) {
                if !c.containers.iter().any(|x| x == m) {
                    c.containers.push(m.to_string());
                }
            }
        }
        for v in list(s(d, "VideoCodec")) {
            let v = codec(&v);
            if !c.video_codecs.contains(&v) {
                c.video_codecs.push(v);
            }
        }
        for a in list(s(d, "AudioCodec")) {
            let a = codec(&a);
            if !c.audio_codecs.contains(&a) {
                c.audio_codecs.push(a);
            }
        }
    }
    if c.audio_codecs.iter().any(|a| matches!(a.as_str(), "ac3" | "eac3" | "dts")) {
        c.max_audio_channels = 6;
    }
    for cp in p.get("CodecProfiles").and_then(Value::as_array).into_iter().flatten() {
        for cond in cp.get("Conditions").and_then(Value::as_array).into_iter().flatten() {
            let prop = s(cond, "Property").to_ascii_lowercase();
            let op = s(cond, "Condition").to_ascii_lowercase();
            let val = s(cond, "Value");
            match (prop.as_str(), op.as_str()) {
                ("height", "lessthanequal") => {
                    if let Ok(h) = val.parse::<i64>() {
                        c.max_height = Some(c.max_height.map_or(h, |m| m.min(h)));
                    }
                }
                ("width", "lessthanequal") => {
                    if let Ok(w) = val.parse::<i64>() {
                        let h = w * 9 / 16;
                        c.max_height = Some(c.max_height.map_or(h, |m| m.min(h)));
                    }
                }
                ("audiochannels", "lessthanequal") => {
                    if let Ok(n) = val.parse::<i64>() {
                        c.max_audio_channels = n.clamp(1, 8);
                    }
                }
                ("videoprofile", _) if s(cp, "Codec").to_ascii_lowercase().contains("h264") => {
                    c.h264_10bit |= val.to_ascii_lowercase().contains("high 10");
                }
                _ => {}
            }
        }
    }
    c.hls = p
        .get("TranscodingProfiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|t| s(t, "Protocol").eq_ignore_ascii_case("hls"));
    if c.containers.is_empty() {
        // An app that sent no profile at all: what browsers play.
        return ClientProfile::web_chromium();
    }
    c
}

/// A field of the body, or the same name in the query (the query wins).
fn param(b: &Value, q: &Q, name: &str) -> Option<Value> {
    if let Some(v) = q.get(&name.to_ascii_lowercase()) {
        if v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false") {
            return Some(Value::Bool(v.eq_ignore_ascii_case("true")));
        }
        return Some(v.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(v)));
    }
    let lower = format!("{}{}", name[..1].to_ascii_lowercase(), &name[1..]);
    b.get(name).or_else(|| b.get(&lower)).filter(|v| !v.is_null()).cloned()
}

fn pick_file<'a>(d: &'a ItemDetail, source: Option<&str>) -> Option<&'a MediaFile> {
    match source.and_then(|s| ids::dec_kind(s, ids::FILE)) {
        Some(f) => d.files.iter().find(|x| x.id == f && !x.missing),
        None => d.files.iter().find(|x| x.role == "main" && !x.missing),
    }
}

fn new_play_id() -> String {
    corrent_core::util::random_hex(16)
}

fn remember(st: &AppState, id: String, p: PlaySession) {
    let mut m = st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner());
    m.retain(|_, s| s.created.elapsed() < PLAY_TTL);
    if m.len() >= MAX_PLAYS {
        let oldest = m.iter().min_by_key(|(_, s)| s.created).map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            m.remove(&k);
        }
    }
    m.insert(id, p);
}

/// Our stream number back from the app's (audio +100, subtitles +200).
fn our_index(i: i64, base: i64) -> i64 {
    if i >= base { i - base } else { i }
}

async fn info(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q, body: Bytes) -> Response {
    let user = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let Ok(d) = st.library.item(user, item).await else { return not_found() };
    let b: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let src = param(&b, &q, "MediaSourceId").and_then(|v| v.as_str().map(str::to_string));
    let Some(file) = pick_file(&d, src.as_deref()) else {
        return json(json!({ "MediaSources": [], "ErrorCode": "NoCompatibleStream" }));
    };
    let profile_json =
        b.get("DeviceProfile").or_else(|| b.get("deviceProfile")).filter(|p| p.is_object()).cloned().or_else(|| {
            st.jellyfin
                .profiles
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(user, a.dev.device_id.clone()))
                .cloned()
        });
    let mut profile = profile_json.as_ref().map(profile_from).unwrap_or_else(ClientProfile::web_chromium);
    if let Some(b) = param(&b, &q, "MaxStreamingBitrate").and_then(|v| v.as_i64()).filter(|b| *b > 0) {
        profile.max_bitrate = Some(b);
    }
    let audio_track =
        param(&b, &q, "AudioStreamIndex").and_then(|v| v.as_i64()).filter(|i| *i >= 0).map(|i| our_index(i, 100));
    let sub =
        param(&b, &q, "SubtitleStreamIndex").and_then(|v| v.as_i64()).filter(|i| *i >= 0).map(|i| our_index(i, 200));
    // Text subtitles the app shows itself; image ones (or when asked) are burned in.
    let always_burn = param(&b, &q, "AlwaysBurnInSubtitleWhenTranscoding").and_then(|v| v.as_bool()).unwrap_or(false);
    let burn = sub.filter(|i| {
        file.streams
            .iter()
            .find(|s| s.kind == "subtitle" && s.index == *i)
            .is_some_and(|s| always_burn || !dto::text_sub(s) || !s.external)
    });
    let direct_allowed = param(&b, &q, "EnableDirectPlay").and_then(|v| v.as_bool()).unwrap_or(true);
    let downloading = crate::integrate::downloading_file(&st, std::path::Path::new(&file.path)).await.is_some();
    let decision = decide::decide(
        &crate::v1::playback::facts(file),
        &profile,
        &PlayRequest {
            audio_track,
            burn_subtitle: burn,
            max_bitrate: profile.max_bitrate,
            max_height: None,
            force_transcode: !direct_allowed || downloading,
        },
    );
    let direct = matches!(decision, Decision::Direct { .. });
    let can_transcode = a.p.can(Perms::PLAYBACK_TRANSCODE)
        && (st.hls_encoder.lock().unwrap_or_else(|e| e.into_inner()).is_some()
            || !file.streams.iter().any(|s| s.kind == "video"));
    let play_id = new_play_id();
    let token = a.dev.token.clone().unwrap_or_default();
    let item_guid = ids::enc(ids::ITEM, item);
    let source_guid = ids::enc(ids::FILE, file.id);
    let audio_only = !file.streams.iter().any(|s| s.kind == "video");
    let transcoding_url = (!direct && can_transcode).then(|| {
        let kind = if audio_only { "Audio" } else { "Videos" };
        let reasons = match &decision {
            Decision::Transcode { reasons, .. } => reasons.join(","),
            _ => String::new(),
        };
        format!(
            "/{kind}/{item_guid}/master.m3u8?MediaSourceId={source_guid}&PlaySessionId={play_id}&DeviceId={}&VideoCodec=h264&AudioCodec=aac&SegmentContainer=ts&TranscodingMaxAudioChannels={}&TranscodeReasons={}&api_key={token}",
            url::form_urlencoded::byte_serialize(a.dev.device_id.as_bytes()).collect::<String>(),
            profile.max_audio_channels.min(2),
            url::form_urlencoded::byte_serialize(reasons.as_bytes()).collect::<String>(),
        )
    });
    if !direct && transcoding_url.is_none() {
        return json(json!({ "MediaSources": [], "PlaySessionId": play_id, "ErrorCode": "NoCompatibleStream" }));
    }
    remember(
        &st,
        play_id.clone(),
        PlaySession {
            user,
            item,
            file: file.id,
            profile,
            audio_track,
            burn_subtitle: burn,
            max_bitrate: None,
            hls: None,
            created: Instant::now(),
        },
    );
    let c = ctx(&st);
    let play = SourcePlay { token, direct, can_transcode, transcoding_url };
    json(json!({ "MediaSources": [dto::media_source(&c, &d.item, file, Some(&play))], "PlaySessionId": play_id }))
}

/// The file itself, with Range (`Static=true` or a matching extension).
async fn serve_file(st: &AppState, user: i64, item: i64, q: &Q, req: Request) -> Response {
    let file = match q.get("mediasourceid").and_then(|s| ids::dec_kind(s, ids::FILE)) {
        Some(f) => st.library.file_path(user, f).await.ok(),
        None => st.library.main_file(user, item).await.ok().map(|(_, p)| p),
    };
    let Some(path) = file else { return not_found() };
    // A download still in progress has holes: Jellyfin apps get it through a transcode instead.
    if crate::integrate::downloading_file(st, std::path::Path::new(&path)).await.is_some() {
        return fail(StatusCode::CONFLICT, "Still downloading");
    }
    match crate::v1::library::serve_path(path.into(), req).await {
        Ok(r) => r,
        Err(_) => not_found(),
    }
}

/// A play session id an app chose itself (Jellyfin Web counts them): short and plain.
fn valid_play_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Starts an HLS session for a play session's choices.
#[allow(clippy::too_many_arguments, clippy::result_large_err)]
async fn start_hls(
    st: &Arc<AppState>,
    a: &JfAuth,
    item: i64,
    file: i64,
    profile: ClientProfile,
    audio_track: Option<i64>,
    burn: Option<i64>,
    max_bitrate: Option<i64>,
) -> Result<String, Response> {
    let req = PlaybackRequest {
        file_id: (file > 0).then_some(file),
        audio_track,
        burn_subtitle: burn,
        max_bitrate,
        force_transcode: true,
        ..Default::default()
    };
    let info = crate::v1::playback::prepare(st, &crate::v1::Auth(a.p.clone()), item, req, Some(profile), false)
        .await
        .map_err(|e| {
        fail(StatusCode::from_u16(e.0.code.http_status()).unwrap_or(StatusCode::BAD_REQUEST), &e.0.message)
    })?;
    info.session_id.ok_or_else(|| fail(StatusCode::BAD_REQUEST, "Nothing to transcode"))
}

/// The live HLS session of a play session: started again when the server reaped it (a long
/// pause), so playback resumes instead of failing.
async fn live_hls(st: &Arc<AppState>, a: &JfAuth, q: &Q) -> Option<String> {
    let p = q.get("playsessionid")?.to_string();
    let found = {
        let m = st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&p)
            .filter(|s| s.user == a.user())
            .map(|s| (s.hls.clone(), s.item, s.file, s.profile.clone(), s.audio_track, s.burn_subtitle, s.max_bitrate))
    };
    let (hls, item, file, profile, audio, burn, maxb) = found?;
    if let Some(h) = hls.filter(|h| st.hls.owner(h).is_some()) {
        st.hls.keep_alive(&h);
        return Some(h);
    }
    let h = start_hls(st, a, item, file, profile, audio, burn, maxb).await.ok()?;
    if let Some(s) =
        st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&p).filter(|s| s.user == a.user())
    {
        s.hls = Some(h.clone());
    }
    Some(h)
}

/// Keeps a play session's transcode alive (progress reports while paused).
pub(crate) fn touch_play(st: &AppState, user: i64, id: &str) {
    let m = st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = m.get(id).filter(|s| s.user == user).and_then(|s| s.hls.as_deref()) {
        st.hls.keep_alive(h);
    }
}

/// Starts (or finds) the HLS session of a play session; the master playlist.
async fn master(st: &Arc<AppState>, a: &JfAuth, item: i64, q: &Q, raw_query: &str) -> Response {
    let client_id = q.get("playsessionid").filter(|p| valid_play_id(p)).map(str::to_string);
    let existing = client_id.as_ref().and_then(|p| {
        let m = st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner());
        m.get(p)
            .filter(|s| s.user == a.user() && s.item == item)
            .map(|s| (s.hls.clone(), s.file, s.profile.clone(), s.audio_track, s.burn_subtitle, s.max_bitrate))
    });
    let known = existing.is_some();
    let (hls, file, profile, audio_track, burn, max_bitrate) = match existing {
        Some(e) => e,
        // An app that built the URL itself: the browser profile.
        None => {
            let file = q.get("mediasourceid").and_then(|s| ids::dec_kind(s, ids::FILE)).unwrap_or(0);
            (None, file, ClientProfile::web_chromium(), None, None, q.i64("maxstreamingbitrate"))
        }
    };
    let hls = match hls.filter(|h| st.hls.owner(h).is_some()) {
        Some(h) => h,
        None => match start_hls(st, a, item, file, profile.clone(), audio_track, burn, max_bitrate).await {
            Ok(h) => h,
            Err(r) => return r,
        },
    };
    let mut query = raw_query.to_string();
    let id = match client_id {
        Some(id) => id,
        None => {
            let id = new_play_id();
            query.push_str(&format!("&PlaySessionId={id}"));
            id
        }
    };
    if known {
        if let Some(s) =
            st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&id).filter(|s| s.user == a.user())
        {
            s.hls = Some(hls.clone());
        }
    } else {
        // Remember it so main.m3u8 and the segments find it.
        remember(
            st,
            id,
            PlaySession {
                user: a.user(),
                item,
                file,
                profile,
                audio_track,
                burn_subtitle: burn,
                max_bitrate,
                hls: Some(hls.clone()),
                created: Instant::now(),
            },
        );
    }
    let bandwidth = 4_000_000;
    let body = format!(
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH={bandwidth},AVERAGE-BANDWIDTH={bandwidth},CODECS=\"avc1.640028,mp4a.40.2\"\nmain.m3u8?{query}\n"
    );
    playlist_response(body)
}

fn playlist_response(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/vnd.apple.mpegurl")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response()
}

/// The media playlist, segments pointing at `hls1/main/N.ts?<query>` (the token travels in the
/// query: players fetch segments without headers).
async fn main_playlist(st: &Arc<AppState>, a: &JfAuth, q: &Q, raw_query: &str) -> Response {
    let Some(hls) = live_hls(st, a, q).await else { return not_found() };
    let Ok(body) = st.hls.playlist(&hls) else { return not_found() };
    let out: String = body
        .lines()
        .map(|l| {
            if l.ends_with(".ts") && !l.starts_with('#') {
                format!("hls1/main/{l}?{raw_query}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    playlist_response(out)
}

async fn segment(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((_id, _pl, seg)): Path<(String, String, String)>,
    q: Q,
    req: Request,
) -> Response {
    let Some(hls) = live_hls(&st, &a, &q).await else { return not_found() };
    let Some(n) = seg.strip_suffix(".ts").and_then(|n| n.parse::<u64>().ok()) else { return not_found() };
    let Ok(path) = st.hls.segment(&hls, n).await else { return not_found() };
    match tower::ServiceExt::oneshot(
        tower_http::services::ServeFile::new_with_mime(&path, &crate::static_mime("video/mp2t")),
        req,
    )
    .await
    {
        Ok(r) => r.map(axum::body::Body::new),
        Err(_) => not_found(),
    }
}

async fn video(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((id, name)): Path<(String, String)>,
    q: Q,
    req: Request,
) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let raw = req.uri().query().unwrap_or("").to_string();
    match name.as_str() {
        "master.m3u8" => master(&st, &a, item, &q, &raw).await,
        "main.m3u8" => main_playlist(&st, &a, &q, &raw).await,
        n if n == "stream" || n.starts_with("stream.") => {
            if q.bool("static") == Some(false) {
                return fail(StatusCode::BAD_REQUEST, "Progressive transcoding isn't supported: use HLS");
            }
            serve_file(&st, a.user(), item, &q, req).await
        }
        _ => not_found(),
    }
}

/// `Audio/{id}/universal`: the file when the app plays its format, else HLS (the master
/// playlist is the answer, as Jellyfin does).
async fn audio(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((id, name)): Path<(String, String)>,
    q: Q,
    req: Request,
) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let raw = req.uri().query().unwrap_or("").to_string();
    match name.as_str() {
        "master.m3u8" => master(&st, &a, item, &q, &raw).await,
        "main.m3u8" => main_playlist(&st, &a, &q, &raw).await,
        n if n == "stream" || n.starts_with("stream.") => serve_file(&st, a.user(), item, &q, req).await,
        n if n == "universal" || n.starts_with("universal.") => {
            let Ok(d) = st.library.item(a.user(), item).await else { return not_found() };
            let Some(file) = pick_file(&d, q.get("mediasourceid")) else { return not_found() };
            // "mp3,flac|flac,webma|opus": containers, each optionally with its codecs.
            let mut profile =
                ClientProfile { containers: Vec::new(), audio_codecs: Vec::new(), ..ClientProfile::web_chromium() };
            for entry in q.get("container").unwrap_or("mp3,aac,flac,ogg,wav,m4a").split(',') {
                let mut parts = entry.split('|');
                let ct = parts.next().unwrap_or("").trim().to_ascii_lowercase();
                for m in containers(&ct) {
                    profile.containers.push(m.to_string());
                }
                let codecs: Vec<String> = parts.map(|c| codec(&c.trim().to_ascii_lowercase())).collect();
                if codecs.is_empty() {
                    profile.audio_codecs.push(match ct.as_str() {
                        "m4a" | "mp4" | "aac" => "aac".into(),
                        "ogg" | "oga" | "opus" | "webma" | "webm" => "opus".into(),
                        other => other.into(),
                    });
                    if matches!(ct.as_str(), "ogg" | "oga") {
                        profile.audio_codecs.push("vorbis".into());
                    }
                } else {
                    profile.audio_codecs.extend(codecs);
                }
            }
            if let Some(c) = q.get("audiocodec") {
                profile.audio_codecs.extend(c.split(',').map(|x| codec(&x.trim().to_ascii_lowercase())));
            }
            profile.max_audio_channels = q.i64("maxaudiochannels").unwrap_or(2).clamp(1, 8);
            let decision = decide::decide(&crate::v1::playback::facts(file), &profile, &PlayRequest::default());
            if matches!(decision, Decision::Direct { .. }) {
                return serve_file(&st, a.user(), item, &q, req).await;
            }
            if !q.get("transcodingprotocol").is_some_and(|p| p.eq_ignore_ascii_case("hls")) {
                return fail(StatusCode::BAD_REQUEST, "Progressive transcoding isn't supported: use HLS");
            }
            let given = q.get("playsessionid").filter(|p| valid_play_id(p)).map(str::to_string);
            let play_id = given.clone().unwrap_or_else(new_play_id);
            remember(
                &st,
                play_id.clone(),
                PlaySession {
                    user: a.user(),
                    item,
                    file: file.id,
                    profile,
                    audio_track: None,
                    burn_subtitle: None,
                    max_bitrate: q.i64("maxstreamingbitrate"),
                    hls: None,
                    created: Instant::now(),
                },
            );
            let raw = if given.is_some() { raw } else { format!("{raw}&PlaySessionId={play_id}") };
            let q2 = Q::parse(Some(&raw));
            master(&st, &a, item, &q2, &raw).await
        }
        _ => not_found(),
    }
}

async fn subtitle(st: &AppState, a: &JfAuth, item: String, source: String, index: i64, file: String) -> Response {
    let _ = item;
    let Some(fid) = ids::dec_kind(&source, ids::FILE) else { return not_found() };
    let idx = our_index(index, 200);
    let Ok(path) = st.library.subtitle_path(a.user(), fid, idx).await else { return not_found() };
    let ext = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let want = file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_else(|| "vtt".into());
    let Ok(bytes) = tokio::fs::read(&path).await else { return not_found() };
    if bytes.len() > 20 * 1024 * 1024 {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "Subtitle file too large");
    }
    let (ct, body) = if want == "vtt" {
        match corrent_transcode::vtt::to_vtt(&ext, &bytes) {
            Some(v) => ("text/vtt; charset=utf-8", v.into_bytes()),
            None => return not_found(),
        }
    } else if want == ext {
        ("text/plain; charset=utf-8", bytes)
    } else {
        return not_found();
    };
    ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "private, max-age=600")], body).into_response()
}

async fn subtitle6(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((item, source, index, _start, file)): Path<(String, String, i64, String, String)>,
) -> Response {
    subtitle(&st, &a, item, source, index, file).await
}

async fn subtitle5(
    State(st): State<Arc<AppState>>,
    a: JfAuth,
    Path((item, source, index, file)): Path<(String, String, i64, String)>,
) -> Response {
    subtitle(&st, &a, item, source, index, file).await
}

/// Ends a play session (and its transcode).
pub(crate) async fn end_play(st: &AppState, user: i64, id: &str) {
    let hls = {
        let mut m = st.jellyfin.plays.lock().unwrap_or_else(|e| e.into_inner());
        match m.get(id) {
            Some(s) if s.user == user => m.remove(id).and_then(|s| s.hls),
            _ => None,
        }
    };
    if let Some(h) = hls {
        st.hls.stop(&h).await;
    }
}

async fn stop_encoding(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    if let Some(p) = q.get("playsessionid") {
        end_play(&st, a.user(), p).await;
    }
    no_content()
}

async fn download(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q, req: Request) -> Response {
    let Some(item) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let mut r = serve_file(&st, a.user(), item, &q, req).await;
    if r.status().is_success() {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_profiles() {
        // jellyfin-web's browser profile, trimmed.
        let web = json!({
            "MaxStreamingBitrate": 120000000,
            "DirectPlayProfiles": [
                { "Container": "webm", "Type": "Video", "VideoCodec": "vp8,vp9,av1", "AudioCodec": "vorbis,opus" },
                { "Container": "mp4,m4v", "Type": "Video", "VideoCodec": "h264,vp9,av1", "AudioCodec": "aac,mp3,opus,flac" },
                { "Container": "opus", "Type": "Audio" }, { "Container": "mp3", "Type": "Audio", "AudioCodec": "mp3" }
            ],
            "TranscodingProfiles": [{ "Container": "ts", "Type": "Video", "AudioCodec": "aac", "VideoCodec": "h264", "Protocol": "hls" }],
            "CodecProfiles": [{ "Type": "Video", "Codec": "h264", "Conditions": [
                { "Condition": "EqualsAny", "Property": "VideoProfile", "Value": "high|main|baseline|constrained baseline" },
                { "Condition": "LessThanEqual", "Property": "Height", "Value": "2160" }
            ]}]
        });
        let p = profile_from(&web);
        assert!(p.containers.contains(&"mp4".to_string()) && p.containers.contains(&"webm".to_string()));
        assert!(!p.containers.contains(&"matroska".to_string()));
        assert!(p.video_codecs.contains(&"h264".to_string()) && p.audio_codecs.contains(&"aac".to_string()));
        assert_eq!((p.max_height, p.h264_10bit, p.hls, p.max_bitrate), (Some(2160), false, true, Some(120000000)));
        // Kodi plays anything (mkv, hevc, dts…).
        let kodi = json!({ "DirectPlayProfiles": [{ "Container": "mkv,mp4,ts", "Type": "Video", "VideoCodec": "h264,hevc", "AudioCodec": "aac,ac3,dts" }] });
        let p = profile_from(&kodi);
        assert!(p.containers.contains(&"matroska".to_string()));
        assert_eq!(p.max_audio_channels, 6);
        assert_eq!(profile_from(&json!({})).name, ClientProfile::web_chromium().name);
        assert_eq!((our_index(102, 100), our_index(1203, 200), our_index(5, 200)), (2, 1003, 5));
    }
}
