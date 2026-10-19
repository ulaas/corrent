//! Audio tags (ID3v2, Vorbis comments, MP4 atoms, APE, RIFF INFO) with lofty: titles, artists,
//! album grouping, numbering, MusicBrainz ids, ReplayGain and embedded cover art.

use std::path::Path;

use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::tag::ItemKey;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub genres: Vec<String>,
    pub composer: Option<String>,
    /// Classical music: the work a track is a movement of.
    pub work: Option<String>,
    pub compilation: bool,
    pub mb_track: Option<String>,
    pub mb_album: Option<String>,
    pub mb_artist: Option<String>,
    /// ReplayGain in dB.
    pub rg_track_gain: Option<f32>,
    pub rg_album_gain: Option<f32>,
    pub duration_ms: Option<u64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    #[serde(skip)]
    pub cover: Option<Vec<u8>>,
}

fn s(v: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    v.map(|c| c.trim().to_string()).filter(|x| !x.is_empty())
}

/// "-6.20 dB" → -6.2.
fn gain(v: Option<&str>) -> Option<f32> {
    let t = v?.trim().trim_end_matches("dB").trim_end_matches("db").trim();
    t.parse().ok().filter(|g: &f32| g.is_finite() && g.abs() < 60.0)
}

/// Reads the tags of an audio file (blocking).
pub fn read(path: &Path) -> Result<AudioTags, String> {
    let file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let props = file.properties();
    let mut t = AudioTags {
        duration_ms: Some(props.duration().as_millis() as u64).filter(|d| *d > 0),
        sample_rate: props.sample_rate(),
        channels: props.channels(),
        bitrate_kbps: props.audio_bitrate(),
        ..Default::default()
    };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else { return Ok(t) };
    t.title = s(tag.title());
    t.artist = s(tag.artist());
    t.album = s(tag.album());
    t.album_artist = tag.get_string(ItemKey::AlbumArtist).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    t.track = tag.track();
    t.track_total = tag.track_total();
    t.disc = tag.disk();
    t.year = tag.date().map(|d| d.year as u32).filter(|y| *y > 0);
    t.genres = tag
        .get_strings(ItemKey::Genre)
        .flat_map(|g| g.split([';', '/', '\0']))
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect();
    t.genres.dedup();
    t.composer = tag.get_string(ItemKey::Composer).map(str::to_string);
    t.work = tag.get_string(ItemKey::Work).map(|w| w.trim().to_string()).filter(|w| !w.is_empty());
    t.compilation =
        tag.get_string(ItemKey::FlagCompilation).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    t.mb_track = tag.get_string(ItemKey::MusicBrainzRecordingId).map(str::to_string);
    t.mb_album = tag.get_string(ItemKey::MusicBrainzReleaseId).map(str::to_string);
    t.mb_artist = tag.get_string(ItemKey::MusicBrainzArtistId).map(str::to_string);
    t.rg_track_gain = gain(tag.get_string(ItemKey::ReplayGainTrackGain));
    t.rg_album_gain = gain(tag.get_string(ItemKey::ReplayGainAlbumGain));
    let pics = tag.pictures();
    t.cover = pics
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pics.first())
        .map(|p| p.data().to_vec())
        .filter(|d| d.len() < 20 * 1024 * 1024);
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaygain_parsing() {
        assert_eq!(gain(Some("-6.20 dB")), Some(-6.2));
        assert_eq!(gain(Some("+1.5")), Some(1.5));
        assert_eq!(gain(Some("loud")), None);
        assert_eq!(gain(Some("1000")), None);
    }

    #[test]
    fn reads_tags_written_by_mpv() {
        // mpv's encoder copies `--metadata` style tags with `--oset-metadata`.
        if !corrent_mpv::Library::available() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("corrent-tags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("t.flac");
        let job = corrent_mpv::encode::EncodeJob {
            input: "av://lavfi:sine=frequency=440:duration=2".into(),
            output: out.display().to_string(),
            options: vec![
                ("of".into(), "flac".into()),
                ("oac".into(), "flac".into()),
                (
                    "oset-metadata".into(),
                    "title=\"Test Tone\",artist=\"Corrent\",album=\"Fixtures\",track=\"3\",date=\"2024\"".into(),
                ),
            ],
            audio_file: None,
        };
        corrent_mpv::encode::run(&job, std::time::Duration::from_secs(30), &mut |_| true).unwrap();
        let t = read(&out).unwrap();
        assert_eq!(t.title.as_deref(), Some("Test Tone"));
        assert_eq!(t.artist.as_deref(), Some("Corrent"));
        assert_eq!(t.album.as_deref(), Some("Fixtures"));
        assert_eq!(t.track, Some(3));
        assert_eq!(t.year, Some(2024));
        assert!(t.duration_ms.unwrap() > 1500);
        let _ = std::fs::remove_dir_all(dir);
    }
}
