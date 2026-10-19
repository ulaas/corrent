//! Media probing: container, duration, tracks, chapters and tags of a file, read by a headless
//! mpv instance that opens the file paused, with null video/audio outputs.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{EndReason, Error, Event, Mpv, Node, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Audio,
    Sub,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Track {
    /// mpv's track id within its kind (for `vid`/`aid`/`sid`).
    pub id: i64,
    pub kind: Option<TrackKind>,
    pub codec: Option<String>,
    pub codec_profile: Option<String>,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub external: bool,
    /// Embedded cover art (an image "video" track).
    pub albumart: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub channels: Option<i64>,
    pub channel_layout: Option<String>,
    pub sample_rate: Option<i64>,
    pub bitrate: Option<i64>,
    pub ff_index: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub title: Option<String>,
    pub start: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProbeResult {
    pub format: Option<String>,
    pub duration: Option<f64>,
    pub size: Option<i64>,
    pub tracks: Vec<Track>,
    pub chapters: Vec<Chapter>,
    /// Container/stream tags (title, artist, album, date, …), keys lower-cased.
    pub tags: BTreeMap<String, String>,
}

impl ProbeResult {
    pub fn video(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == Some(TrackKind::Video) && !t.albumart)
    }
    pub fn audio(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == Some(TrackKind::Audio))
    }
    pub fn subtitles(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == Some(TrackKind::Sub))
    }
}

fn opt_str(n: &Node, k: &str) -> Option<String> {
    n.get(k).and_then(Node::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

fn flag(n: &Node, k: &str) -> bool {
    n.get(k).and_then(Node::as_bool).unwrap_or(false)
}

pub(crate) fn track_from_node(n: &Node) -> Track {
    Track {
        id: n.get("id").and_then(Node::as_i64).unwrap_or(0),
        kind: match n.get("type").and_then(Node::as_str) {
            Some("video") => Some(TrackKind::Video),
            Some("audio") => Some(TrackKind::Audio),
            Some("sub") => Some(TrackKind::Sub),
            _ => None,
        },
        codec: opt_str(n, "codec"),
        codec_profile: opt_str(n, "codec-profile"),
        title: opt_str(n, "title"),
        lang: opt_str(n, "lang"),
        default: flag(n, "default"),
        forced: flag(n, "forced"),
        external: flag(n, "external"),
        albumart: flag(n, "albumart") || flag(n, "image"),
        width: n.get("demux-w").and_then(Node::as_i64),
        height: n.get("demux-h").and_then(Node::as_i64),
        fps: n.get("demux-fps").and_then(Node::as_f64),
        channels: n.get("demux-channel-count").and_then(Node::as_i64),
        channel_layout: opt_str(n, "demux-channels"),
        sample_rate: n.get("demux-samplerate").and_then(Node::as_i64),
        bitrate: n.get("demux-bitrate").and_then(Node::as_i64),
        ff_index: n.get("ff-index").and_then(Node::as_i64),
    }
}

/// Opens `input` (a path or URL mpv understands) and reads its structure. Blocking; run it on a
/// worker thread or in the `corrent-mpvw` process.
pub fn probe(input: &str, timeout: Duration) -> Result<ProbeResult> {
    let mpv = Mpv::headless(&[("vo", "null"), ("ao", "null"), ("pause", "yes"), ("audio-display", "no")])?;
    mpv.command(&["loadfile", input])?;
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Timeout(format!("probing {input}")));
        }
        match mpv.wait_event(left.as_secs_f64()) {
            Some(Event::FileLoaded) => break,
            Some(Event::EndFile(reason)) => {
                let code = match reason {
                    EndReason::Error(c) => c,
                    _ => -13,
                };
                return Err(Error::Mpv {
                    what: format!("open {input}"),
                    code,
                    message: mpv.library().error_text(code),
                });
            }
            Some(Event::Shutdown) => {
                return Err(Error::Mpv { what: "probe".into(), code: -20, message: "shut down".into() });
            }
            _ => {}
        }
    }
    let tracks = mpv.get("track-list").map(|n| n.as_array().iter().map(track_from_node).collect()).unwrap_or_default();
    let chapters = mpv
        .get("chapter-list")
        .map(|n| {
            n.as_array()
                .iter()
                .map(|c| Chapter {
                    title: opt_str(c, "title"),
                    start: c.get("time").and_then(Node::as_f64).unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();
    let tags = match mpv.get("metadata") {
        Ok(Node::Map(m)) => {
            m.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k.to_ascii_lowercase(), s.to_string()))).collect()
        }
        _ => BTreeMap::new(),
    };
    Ok(ProbeResult {
        format: mpv.get("file-format").ok().and_then(|n| n.as_str().map(str::to_string)),
        duration: mpv.get_f64("duration").ok().flatten(),
        size: mpv.get("file-size").ok().and_then(|n| n.as_i64()),
        tracks,
        chapters,
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    #[test]
    fn track_parsing() {
        let n = Node::Map(BTreeMap::from([
            ("id".into(), Node::Int(2)),
            ("type".into(), Node::String("audio".into())),
            ("codec".into(), Node::String("aac".into())),
            ("lang".into(), Node::String("tur".into())),
            ("default".into(), Node::Flag(true)),
            ("demux-channel-count".into(), Node::Int(6)),
            ("demux-samplerate".into(), Node::Int(48000)),
        ]));
        let t = track_from_node(&n);
        assert_eq!(t.kind, Some(TrackKind::Audio));
        assert_eq!(t.channels, Some(6));
        assert_eq!(t.lang.as_deref(), Some("tur"));
        assert!(t.default && !t.forced);
    }

    #[test]
    fn probe_generated_media() {
        if !Library::available() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        // lavfi sources have no duration; only the structure is checked here (see encode tests).
        let r = probe("av://lavfi:testsrc=duration=3:size=320x240:rate=25", Duration::from_secs(20)).unwrap();
        let v = r.video().next().expect("a video track");
        assert_eq!((v.width, v.height), (Some(320), Some(240)));
        assert!(probe("/nonexistent/file.mkv", Duration::from_secs(20)).is_err());
    }
}
