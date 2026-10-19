//! Direct stream: Matroska files whose codecs a client plays (H.264/HEVC with AAC, AC-3, E-AC-3
//! or Opus) become HLS with fragmented-MP4 segments by copying the frames — no encoder, no
//! worker process, almost no CPU. The cluster index is built once per file (one small read per
//! cluster); each segment runs from one video keyframe to a later one, cut on request. Keyframes
//! come from cluster starts, else the Cues, else a scan of the clusters that need it.

pub mod mkv;
pub mod mp4;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mkv::{Header, MkvError};
use mp4::OutTrack;

/// Segments are cut at the first keyframe cluster after this much media.
const TARGET_NS: i64 = 4_000_000_000;
/// A segment larger than this (keyframes far apart, huge bitrate) isn't served.
const MAX_SEGMENT_BYTES: u64 = 192 * 1024 * 1024;
/// At most this much of a file is read to find keyframes the index doesn't give.
const MAX_SCAN_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// File positions `[from, to)` (cluster, offset in its data).
    pub from: mkv::Pos,
    pub to: mkv::Pos,
    pub start_ns: i64,
    pub duration_ns: i64,
}

#[derive(Debug)]
pub struct Remux {
    pub path: PathBuf,
    pub header: Header,
    pub tracks: Vec<OutTrack>,
    pub segments: Vec<Segment>,
}

/// The codecs [`prepare`] can copy, as the library names them (mpv/ffprobe codec names).
pub const VIDEO_CODECS: &[&str] = &["h264", "hevc"];
/// MP3 is left out although the muxer copies it: Chromium's Media Source doesn't take MP3 in
/// MP4 (checked with `MediaSource.isTypeSupported`), so those files transcode their audio.
pub const AUDIO_CODECS: &[&str] = &["aac", "ac3", "eac3", "opus"];

/// Indexes `path` for remuxing its video and audio track `audio` (the n-th audio track,
/// 1-based as mpv counts; `None` = no audio).
pub fn prepare(path: &Path, audio: Option<i64>) -> Result<Remux, MkvError> {
    let header = mkv::open(path)?;
    let video = header.tracks.iter().find(|t| t.kind == 1).ok_or_else(|| MkvError::Unsupported("no video".into()))?;
    let mut tracks = vec![mp4::out_track(video, 1, None).map_err(MkvError::Unsupported)?];
    if let Some(n) = audio {
        let a = header
            .tracks
            .iter()
            .filter(|t| t.kind == 2)
            .nth((n.max(1) - 1) as usize)
            .ok_or_else(|| MkvError::Unsupported(format!("no audio track {n}")))?;
        // AC-3 and E-AC-3 are described from their first frame.
        let first = if ["A_AC3", "A_EAC3", "A_MPEG/L3"].contains(&a.codec.as_str()) {
            let n = header.clusters.len().min(4);
            mkv::frames(path, &header, (0, 0), (n, 0), &[a.number])?.into_iter().next().map(|f| f.data)
        } else {
            None
        };
        tracks.push(mp4::out_track(a, 2, first.as_deref()).map_err(MkvError::Unsupported)?);
    }
    let segments = plan(&header, &cuts(path, &header)?)?;
    Ok(Remux { path: path.to_path_buf(), header, tracks, segments })
}

/// Every place the video can start: (position, timestamp), in file order.
fn cuts(path: &Path, h: &Header) -> Result<Vec<(mkv::Pos, i64)>, MkvError> {
    let by_start: std::collections::HashMap<u64, usize> =
        h.clusters.iter().enumerate().map(|(i, c)| (c.start, i)).collect();
    let mut cues: std::collections::HashMap<usize, Vec<&mkv::Cue>> = std::collections::HashMap::new();
    for c in &h.cues {
        if let Some(i) = by_start.get(&c.cluster_start) {
            cues.entry(*i).or_default().push(c);
        }
    }
    let mut out = Vec::new();
    // Looking inside clusters reads them: give up (and transcode) rather than read a whole film.
    let mut scanned = 0u64;
    let mut scan = |i: usize| -> Result<Vec<(mkv::Pos, i64)>, MkvError> {
        scanned += h.clusters[i].size;
        if scanned > MAX_SCAN_BYTES {
            return Err(MkvError::Unsupported("no keyframe index (no Cues, clusters don't start at keyframes)".into()));
        }
        Ok(mkv::keyframes(path, h, i)?.into_iter().map(|(off, t)| ((i, off), t)).collect())
    };
    for (i, c) in h.clusters.iter().enumerate() {
        if c.key {
            out.push(((i, 0), c.time_ns));
            continue;
        }
        match cues.get(&i) {
            Some(list) if list.iter().all(|c| c.relative.is_some()) => {
                out.extend(list.iter().map(|c| ((i, c.relative.unwrap_or(0)), c.time_ns)));
            }
            // Listed without a position, or no Cues at all: look inside.
            Some(_) => out.extend(scan(i)?),
            None if h.cues.is_empty() => out.extend(scan(i)?),
            // The Cues list every keyframe: none here.
            None => {}
        }
    }
    Ok(out)
}

/// Runs from keyframe to keyframe, about [`TARGET_NS`] long. The first starts at the very
/// beginning so nothing before the first keyframe is lost.
fn plan(h: &Header, cuts: &[(mkv::Pos, i64)]) -> Result<Vec<Segment>, MkvError> {
    let Some(first) = h.clusters.first() else { return Err(MkvError::Unsupported("no clusters".into())) };
    let mut out = vec![Segment {
        from: (0, 0),
        to: (0, 0),
        start_ns: cuts.first().map_or(first.time_ns, |c| c.1.min(first.time_ns)),
        duration_ns: 0,
    }];
    for &(pos, t) in cuts {
        let cur = out.last().expect("one segment");
        if pos == (0, 0) || t - cur.start_ns < TARGET_NS {
            continue;
        }
        out.last_mut().expect("one segment").to = pos;
        out.push(Segment { from: pos, to: (0, 0), start_ns: t, duration_ns: 0 });
    }
    let n = out.len();
    out[n - 1].to = (h.clusters.len(), 0);
    for s in &out {
        let last = if s.to.1 == 0 { s.to.0 } else { s.to.0 + 1 };
        let bytes: u64 = h.clusters[s.from.0..last.min(h.clusters.len())].iter().map(|c| c.size).sum();
        if bytes > MAX_SEGMENT_BYTES {
            return Err(MkvError::Unsupported("keyframes too far apart".into()));
        }
    }
    let end = h.duration_ns.filter(|d| *d > out[n - 1].start_ns).unwrap_or(out[n - 1].start_ns + TARGET_NS);
    for i in 0..n {
        let next = if i + 1 < n { out[i + 1].start_ns } else { end };
        out[i].duration_ns = (next - out[i].start_ns).max(1_000_000);
    }
    Ok(out)
}

impl Remux {
    /// The HLS `CODECS` attribute.
    pub fn codecs(&self) -> String {
        self.tracks.iter().map(|t| t.codec_string()).collect::<Vec<_>>().join(",")
    }

    /// A VOD playlist naming `init.mp4` and `<n>.m4s`.
    pub fn playlist(&self) -> String {
        let target = self.segments.iter().map(|s| s.duration_ns).max().unwrap_or(TARGET_NS);
        let mut p = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"init.mp4\"\n",
            (target as f64 / 1e9).ceil() as u64
        );
        for (i, s) in self.segments.iter().enumerate() {
            p.push_str(&format!("#EXTINF:{:.3},\n{i}.m4s\n", s.duration_ns as f64 / 1e9));
        }
        p.push_str("#EXT-X-ENDLIST\n");
        p
    }

    pub fn init(&self) -> Vec<u8> {
        mp4::init_segment(&self.tracks)
    }

    /// Media segment `n` (reads its clusters).
    pub fn segment(&self, n: usize) -> Result<Vec<u8>, MkvError> {
        let s = self.segments.get(n).ok_or(MkvError::Invalid("no such segment"))?;
        let wanted: Vec<u64> = self.tracks.iter().map(|t| t.source).collect();
        let frames = mkv::frames(&self.path, &self.header, s.from, s.to, &wanted)?;
        Ok(mp4::media_segment(n as u32 + 1, &self.tracks, &frames))
    }
}

/// Recently prepared files (index building reads every cluster header once).
#[derive(Default)]
pub struct Cache(Mutex<Vec<(Key, Arc<Remux>)>>);

#[derive(Clone, Debug, PartialEq)]
struct Key {
    path: PathBuf,
    audio: Option<i64>,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

const CACHE_SIZE: usize = 16;

impl Cache {
    /// The prepared remux of `path` (blocking: call off the async threads).
    pub fn get(&self, path: &Path, audio: Option<i64>) -> Result<Arc<Remux>, MkvError> {
        let meta = std::fs::metadata(path)?;
        let key = Key { path: path.to_path_buf(), audio, len: meta.len(), modified: meta.modified().ok() };
        {
            let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = c.iter().position(|(k, _)| *k == key) {
                let hit = c.remove(i);
                let r = hit.1.clone();
                c.push(hit);
                return Ok(r);
            }
        }
        let r = Arc::new(prepare(path, audio)?);
        let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
        c.retain(|(k, _)| k.path != key.path || k.audio != key.audio);
        c.push((key, r.clone()));
        if c.len() > CACHE_SIZE {
            c.remove(0);
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_and_cuts_segments() {
        let p = mkv::tests::write_sample("plan", 5);
        let r = prepare(&p, Some(1)).unwrap();
        // 2 s clusters, 4 s target: clusters 0-1, 2-3, 4.
        assert_eq!(
            r.segments.iter().map(|s| (s.from, s.to)).collect::<Vec<_>>(),
            vec![((0, 0), (2, 0)), ((2, 0), (4, 0)), ((4, 0), (5, 0))]
        );
        assert_eq!(r.segments.iter().map(|s| s.duration_ns / 1_000_000).collect::<Vec<_>>(), vec![4000, 4000, 2000]);
        assert_eq!(r.codecs(), "avc1.64001f,mp4a.40.2");
        let pl = r.playlist();
        assert!(
            pl.contains("#EXT-X-MAP:URI=\"init.mp4\"")
                && pl.contains("#EXTINF:2.000,\n2.m4s")
                && pl.ends_with("#EXT-X-ENDLIST\n")
        );
        let seg = r.segment(1).unwrap();
        assert_eq!(&seg[4..8], b"moof");
        assert!(r.segment(9).is_err());
        // No audio track 2; and video only works.
        assert!(prepare(&p, Some(2)).is_err());
        assert_eq!(prepare(&p, None).unwrap().tracks.len(), 1);
        let cache = Cache::default();
        let a = cache.get(&p, Some(1)).unwrap();
        let b = cache.get(&p, Some(1)).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn cuts_inside_clusters_without_losing_frames() {
        let p = mkv::tests::write_sample_with("mid", 5, true);
        let h = mkv::open(&p).unwrap();
        assert!(h.clusters[0].key && h.clusters[1..].iter().all(|c| !c.key) && h.cues.is_empty());
        let r = prepare(&p, Some(1)).unwrap();
        // Cut at the keyframe inside cluster 2 (4 s), and inside cluster 4 (8 s).
        assert_eq!(r.segments.len(), 3);
        assert_eq!(r.segments[1].from.0, 2);
        assert!(r.segments[1].from.1 > 0, "{:?}", r.segments[1]);
        assert_eq!(r.segments[1].start_ns, 4_000_000_000);
        // Every frame lands in exactly one segment: the frames before the keyframe go to the
        // segment before.
        let all = mkv::frames(&p, &h, (0, 0), (h.clusters.len(), 0), &[1, 2]).unwrap();
        let mut n = 0;
        for s in &r.segments {
            let f = mkv::frames(&p, &h, s.from, s.to, &[1, 2]).unwrap();
            let first_video = f.iter().find(|x| x.track == 1).unwrap();
            assert!(first_video.key, "segment {s:?} starts with a keyframe");
            n += f.len();
        }
        assert_eq!(n, all.len());
        let _ = std::fs::remove_file(p);
    }
}
