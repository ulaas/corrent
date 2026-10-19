//! Direct play or transcode? A pure function of the media's streams, what the client can play,
//! and what the user asked for (audio track, burned subtitles, bandwidth).

use serde::{Deserialize, Serialize};

/// What a client can play natively.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientProfile {
    pub name: String,
    /// Container names as mpv reports them (`file-format`), matched by any comma-separated part.
    pub containers: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
    /// Plays 10-bit H.264 ("High 10")? Browsers don't.
    pub h264_10bit: bool,
    pub max_height: Option<i64>,
    pub max_audio_channels: i64,
    /// Total bitrate above which the client asks for a smaller transcode (bits/s).
    pub max_bitrate: Option<i64>,
    pub hls: bool,
    /// Plays anything (the native client: mpv).
    pub everything: bool,
}

impl Default for ClientProfile {
    fn default() -> Self {
        ClientProfile::web_chromium()
    }
}

impl ClientProfile {
    pub fn native() -> ClientProfile {
        ClientProfile {
            name: "native".into(),
            everything: true,
            max_audio_channels: 8,
            hls: true,
            ..ClientProfile::web_chromium()
        }
    }

    pub fn web_chromium() -> ClientProfile {
        ClientProfile {
            name: "web".into(),
            containers: vec![
                "mp4".into(),
                "mov".into(),
                "webm".into(),
                "ogg".into(),
                "mp3".into(),
                "flac".into(),
                "wav".into(),
            ],
            video_codecs: vec!["h264".into(), "vp8".into(), "vp9".into(), "av1".into()],
            audio_codecs: vec![
                "aac".into(),
                "mp3".into(),
                "opus".into(),
                "vorbis".into(),
                "flac".into(),
                "pcm_s16le".into(),
            ],
            h264_10bit: false,
            max_height: None,
            max_audio_channels: 2,
            max_bitrate: None,
            hls: true,
            everything: false,
        }
    }

    pub fn web_safari() -> ClientProfile {
        let mut p = ClientProfile::web_chromium();
        p.name = "safari".into();
        p.video_codecs.push("hevc".into());
        p.audio_codecs.push("alac".into());
        p.audio_codecs.push("eac3".into());
        p.audio_codecs.push("ac3".into());
        p.max_audio_channels = 6;
        p
    }

    pub fn named(name: &str) -> ClientProfile {
        match name {
            "native" => ClientProfile::native(),
            "safari" => ClientProfile::web_safari(),
            _ => ClientProfile::web_chromium(),
        }
    }
}

/// The facts of a file the decision needs (from the library's probe).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaFacts {
    pub container: Option<String>,
    pub duration_ms: Option<i64>,
    pub video: Option<VideoFacts>,
    pub audio: Vec<AudioFacts>,
    pub subtitles: Vec<SubFacts>,
    pub bitrate: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoFacts {
    pub codec: String,
    pub profile: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioFacts {
    pub index: i64,
    pub codec: String,
    pub channels: Option<i64>,
    pub default: bool,
    pub lang: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubFacts {
    pub index: i64,
    pub codec: Option<String>,
    pub external: bool,
    pub lang: Option<String>,
}

/// What the user asked for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayRequest {
    /// mpv audio track id (None = default).
    pub audio_track: Option<i64>,
    /// Subtitle to burn into the picture.
    pub burn_subtitle: Option<i64>,
    /// Bits per second cap (bandwidth).
    pub max_bitrate: Option<i64>,
    pub max_height: Option<i64>,
    /// Force a transcode (e.g. the player failed to play the original).
    pub force_transcode: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoTarget {
    pub height: Option<i64>,
    pub bitrate: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioTarget {
    pub track: Option<i64>,
    pub channels: i64,
    pub bitrate: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "method")]
pub enum Decision {
    /// Serve the file as it is (Range requests).
    Direct { reasons: Vec<String> },
    /// HLS from the transcoder; `video: None` = audio only.
    Transcode {
        video: Option<VideoTarget>,
        audio: Option<AudioTarget>,
        burn_subtitle: Option<i64>,
        reasons: Vec<String>,
    },
}

fn matches_container(fmt: &str, list: &[String]) -> bool {
    fmt.split(',').any(|part| list.iter().any(|c| c == part.trim()))
}

/// mpv names Matroska and WebM alike ("matroska,webm"): it is WebM only with WebM codecs.
fn effective_container(c: &str, m: &MediaFacts, audio: Option<&AudioFacts>) -> String {
    if !c.contains("matroska") {
        return c.to_string();
    }
    let video_webm = m.video.as_ref().is_none_or(|v| ["vp8", "vp9", "av1"].contains(&v.codec.as_str()));
    let audio_webm = audio.is_none_or(|a| ["opus", "vorbis"].contains(&a.codec.as_str()));
    if video_webm && audio_webm { "webm".into() } else { "matroska".into() }
}

/// Bitrate for an H.264 transcode at a height (bits/s).
fn bitrate_for(height: i64) -> i64 {
    match height {
        h if h > 1440 => 20_000_000,
        h if h > 1080 => 12_000_000,
        h if h > 720 => 8_000_000,
        h if h > 480 => 4_000_000,
        _ => 2_000_000,
    }
}

pub fn decide(m: &MediaFacts, p: &ClientProfile, r: &PlayRequest) -> Decision {
    let mut reasons = Vec::new();
    let audio_choice = r
        .audio_track
        .and_then(|t| m.audio.iter().find(|a| a.index == t))
        .or_else(|| m.audio.iter().find(|a| a.default))
        .or(m.audio.first());
    if p.everything
        && !r.force_transcode
        && r.burn_subtitle.is_none()
        && r.max_bitrate.is_none()
        && r.max_height.is_none()
    {
        return Decision::Direct { reasons: vec!["the client plays everything".into()] };
    }
    if r.force_transcode {
        reasons.push("transcode requested".into());
    }
    if let Some(c) = &m.container
        && !p.everything
        && !matches_container(&effective_container(c, m, audio_choice), &p.containers)
    {
        reasons.push(format!("container {c} not supported"));
    }
    let mut video_ok = true;
    if let Some(v) = &m.video
        && !p.everything
    {
        if !p.video_codecs.contains(&v.codec) {
            reasons.push(format!("video codec {} not supported", v.codec));
            video_ok = false;
        }
        if v.codec == "h264" && !p.h264_10bit && v.profile.as_deref().is_some_and(|x| x.contains("10")) {
            reasons.push("10-bit H.264 not supported".into());
            video_ok = false;
        }
    }
    let limit_h = [p.max_height, r.max_height].into_iter().flatten().min();
    if let (Some(lim), Some(h)) = (limit_h, m.video.as_ref().and_then(|v| v.height))
        && h > lim
    {
        reasons.push(format!("{h}p above the {lim}p limit"));
        video_ok = false;
    }
    let limit_b = [p.max_bitrate, r.max_bitrate].into_iter().flatten().min();
    if let (Some(lim), Some(b)) = (limit_b, m.bitrate)
        && b > lim
    {
        reasons.push(format!("bitrate {b} above the {lim} limit"));
        video_ok = false;
    }
    let mut audio_ok = true;
    if let Some(a) = audio_choice
        && !p.everything
    {
        if !p.audio_codecs.contains(&a.codec) {
            reasons.push(format!("audio codec {} not supported", a.codec));
            audio_ok = false;
        }
        if a.channels.unwrap_or(2) > p.max_audio_channels {
            reasons.push(format!("{} audio channels", a.channels.unwrap_or(2)));
            audio_ok = false;
        }
    }
    // Browsers play the default audio track only.
    if r.audio_track.is_some() && audio_choice.is_some_and(|a| !a.default) && m.audio.len() > 1 {
        reasons.push("another audio track was chosen".into());
    }
    if let Some(s) = r.burn_subtitle {
        reasons.push(format!("subtitle {s} burned in"));
    }
    if reasons.is_empty() {
        return Decision::Direct { reasons: vec!["the client plays this file".into()] };
    }
    let _ = (video_ok, audio_ok);
    let video = m.video.as_ref().map(|v| {
        let src_h = v.height.unwrap_or(1080);
        let height = limit_h.map(|l| l.min(src_h)).filter(|h| *h < src_h);
        let target_h = height.unwrap_or(src_h);
        let mut bitrate = bitrate_for(target_h);
        if let Some(lim) = limit_b {
            bitrate = bitrate.min((lim - 256_000).max(500_000));
        }
        VideoTarget { height, bitrate }
    });
    let audio = audio_choice.map(|a| AudioTarget {
        track: Some(a.index),
        channels: a.channels.unwrap_or(2).min(p.max_audio_channels).max(1),
        bitrate: if a.channels.unwrap_or(2) > 2 && p.max_audio_channels > 2 { 384_000 } else { 192_000 },
    });
    Decision::Transcode { video, audio, burn_subtitle: r.burn_subtitle, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(container: &str, v: &str, a: &str, ch: i64) -> MediaFacts {
        MediaFacts {
            container: Some(container.into()),
            duration_ms: Some(60_000),
            video: (!v.is_empty()).then(|| VideoFacts { codec: v.into(), height: Some(1080), ..Default::default() }),
            audio: vec![AudioFacts { index: 1, codec: a.into(), channels: Some(ch), default: true, lang: None }],
            subtitles: vec![],
            bitrate: Some(8_000_000),
        }
    }

    #[test]
    fn web_direct_and_transcode() {
        let web = ClientProfile::web_chromium();
        let none = PlayRequest::default();
        assert!(matches!(
            decide(&facts("mov,mp4,m4a,3gp,3g2,mj2", "h264", "aac", 2), &web, &none),
            Decision::Direct { .. }
        ));
        let d = decide(&facts("matroska,webm", "h264", "ac3", 6), &web, &none);
        let Decision::Transcode { video, audio, reasons, .. } = d else { panic!() };
        assert!(reasons.iter().any(|r| r.contains("container")) && reasons.iter().any(|r| r.contains("ac3")));
        assert_eq!(video.unwrap().height, None, "no downscale without a limit");
        assert_eq!(audio.unwrap().channels, 2);
        // WebM with VP9/Opus plays directly.
        assert!(matches!(decide(&facts("matroska,webm", "vp9", "opus", 2), &web, &none), Decision::Direct { .. }));
        // Audio-only FLAC plays directly, ALAC does not.
        assert!(matches!(decide(&facts("flac", "", "flac", 2), &web, &none), Decision::Direct { .. }));
        let d = decide(&facts("mov,mp4,m4a,3gp,3g2,mj2", "", "alac", 2), &web, &none);
        assert!(matches!(d, Decision::Transcode { video: None, .. }));
        // Safari plays HEVC.
        assert!(matches!(
            decide(&facts("mov,mp4,m4a,3gp,3g2,mj2", "hevc", "aac", 2), &ClientProfile::web_safari(), &none),
            Decision::Direct { .. }
        ));
    }

    #[test]
    fn limits_and_requests() {
        let web = ClientProfile::web_chromium();
        let r = PlayRequest { max_height: Some(720), ..Default::default() };
        let Decision::Transcode { video, .. } = decide(&facts("mov,mp4", "h264", "aac", 2), &web, &r) else { panic!() };
        let v = video.unwrap();
        assert_eq!(v.height, Some(720));
        assert_eq!(v.bitrate, 4_000_000);
        let r = PlayRequest { burn_subtitle: Some(3), ..Default::default() };
        assert!(matches!(
            decide(&facts("mov,mp4", "h264", "aac", 2), &web, &r),
            Decision::Transcode { burn_subtitle: Some(3), .. }
        ));
        // The native client plays everything unless asked otherwise.
        assert!(matches!(
            decide(&facts("avi", "mpeg4", "dts", 6), &ClientProfile::native(), &PlayRequest::default()),
            Decision::Direct { .. }
        ));
        let mut hi10 = facts("mov,mp4", "h264", "aac", 2);
        hi10.video.as_mut().unwrap().profile = Some("High 10".into());
        assert!(matches!(decide(&hi10, &web, &PlayRequest::default()), Decision::Transcode { .. }));
    }
}
