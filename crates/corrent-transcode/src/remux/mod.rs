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
/// Remux work (indexing, segments) at the same time: each segment holds its clusters and the
/// output in memory, so many parallel requests for big segments must wait their turn.
const SLOTS: usize = 4;
/// Video frames after a cut that can be shown before it (open GOPs' leading pictures).
const LEADING_FRAMES: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// File positions `[from, to)` (cluster, offset in its data).
    pub from: mkv::Pos,
    pub to: mkv::Pos,
    pub start_ns: i64,
    pub duration_ns: i64,
}

/// What a file's remuxes share, whichever audio track they take.
#[derive(Debug)]
pub struct Index {
    pub path: PathBuf,
    pub header: Header,
    pub segments: Vec<Segment>,
    /// Added to every timestamp so the first (a B-frame shown before 0) isn't negative.
    pub shift_ns: i64,
    /// Changes when the file does (size and modification time): goes into segment URLs and
    /// ETags so a replaced file isn't served from caches.
    pub version: String,
}

/// A file's index with the tracks of one audio choice.
#[derive(Debug)]
pub struct Remux {
    pub index: Arc<Index>,
    pub tracks: Vec<OutTrack>,
}

impl std::ops::Deref for Remux {
    type Target = Index;
    fn deref(&self) -> &Index {
        &self.index
    }
}

/// The codecs [`prepare`] can copy, as the library names them (mpv/ffprobe codec names).
pub const VIDEO_CODECS: &[&str] = &["h264", "hevc"];
/// MP3 is left out although the muxer copies it: Chromium's Media Source doesn't take MP3 in
/// MP4 (checked with `MediaSource.isTypeSupported`), so those files transcode their audio.
pub const AUDIO_CODECS: &[&str] = &["aac", "ac3", "eac3", "opus"];

/// Indexes `path` for remuxing its video and audio track `audio` (the n-th audio track,
/// 1-based as mpv counts; `None` = no audio).
pub fn prepare(path: &Path, audio: Option<i64>) -> Result<Remux, MkvError> {
    let index = Arc::new(index(path)?);
    let tracks = tracks(&index, audio)?;
    Ok(Remux { index, tracks })
}

/// A file's [`Index::version`] from its metadata (without indexing it).
pub fn version(meta: &std::fs::Metadata) -> String {
    let modified =
        meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    format!("{:x}-{modified:x}", meta.len())
}

/// Reads the header, finds the keyframes and plans the segments.
pub fn index(path: &Path) -> Result<Index, MkvError> {
    let version = version(&std::fs::metadata(path)?);
    let header = mkv::open(path)?;
    if !header.tracks.iter().any(|t| t.kind == 1) {
        return Err(MkvError::Unsupported("no video".into()));
    }
    let segments = plan(&header, &cuts(path, &header)?)?;
    let shift_ns = shift(path, &header);
    Ok(Index { path: path.to_path_buf(), header, segments, shift_ns, version })
}

/// How far the earliest timestamp of the first cluster lies before 0 (B-frames shown before
/// the first keyframe). One offset for the whole file keeps the segments back to back.
fn shift(path: &Path, h: &Header) -> i64 {
    let tracks: Vec<u64> = h.tracks.iter().filter(|t| t.kind == 1 || t.kind == 2).map(|t| t.number).collect();
    let first = mkv::frames(path, h, (0, 0), (1, 0), &tracks).unwrap_or_default();
    first.iter().map(|f| f.pts_ns).min().map_or(0, |m| m.saturating_neg().max(0))
}

/// The output tracks for `audio` (see [`prepare`]). Reads at most the first frames of an
/// AC-3/E-AC-3 track.
pub fn tracks(index: &Index, audio: Option<i64>) -> Result<Vec<OutTrack>, MkvError> {
    let header = &index.header;
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
        let first = if a.unsupported.is_none() && ["A_AC3", "A_EAC3", "A_MPEG/L3"].contains(&a.codec.as_str()) {
            let n = header.clusters.len().min(4);
            mkv::frames(&index.path, header, (0, 0), (n, 0), &[a.number])?.into_iter().next().map(|f| f.data)
        } else {
            None
        };
        tracks.push(mp4::out_track(a, 2, first.as_deref()).map_err(MkvError::Unsupported)?);
    }
    Ok(tracks)
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
        scanned = scanned.saturating_add(h.clusters[i].size);
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
        let Some(cur) = out.last_mut() else { break };
        if pos == (0, 0) || t.saturating_sub(cur.start_ns) < TARGET_NS {
            continue;
        }
        cur.to = pos;
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
    let last_start = out[n - 1].start_ns;
    let end = h.duration_ns.filter(|d| *d > last_start).unwrap_or(last_start.saturating_add(TARGET_NS));
    for i in 0..n {
        let next = if i + 1 < n { out[i + 1].start_ns } else { end };
        out[i].duration_ns = next.saturating_sub(out[i].start_ns).max(1_000_000);
    }
    Ok(out)
}

impl Remux {
    /// The HLS `CODECS` attribute.
    pub fn codecs(&self) -> String {
        self.tracks.iter().map(|t| t.codec_string()).collect::<Vec<_>>().join(",")
    }

    /// A VOD playlist naming `init.mp4` and `<n>.m4s`, each followed by `query` (the server
    /// passes `?v=<version>`; a folder of files on disk takes `""`).
    pub fn playlist(&self, query: &str) -> String {
        let target = self.segments.iter().map(|s| s.duration_ns).max().unwrap_or(TARGET_NS);
        let mut p = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"init.mp4{query}\"\n",
            (target as f64 / 1e9).ceil() as u64
        );
        for (i, s) in self.segments.iter().enumerate() {
            p.push_str(&format!("#EXTINF:{:.3},\n{i}.m4s{query}\n", s.duration_ns as f64 / 1e9));
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
        let mut end = s.start_ns.saturating_add(s.duration_ns);
        // Open GOPs: the next segment's first decode time lies before its keyframe.
        if let Some(next) = self.segments.get(n + 1)
            && let Ok(Some(t)) = mkv::earliest_pts(&self.path, &self.header, next.from, LEADING_FRAMES)
        {
            end = end.min(t);
        }
        Ok(mp4::media_segment(n as u32 + 1, &self.tracks, &frames, self.shift_ns, end))
    }
}

/// Recently prepared files: the index once per file (it reads every cluster header), the
/// tracks per audio choice.
pub struct Cache {
    files: Mutex<Vec<(FileKey, Arc<Index>)>>,
    remuxes: Mutex<Vec<(RemuxKey, Arc<Remux>)>>,
    slots: Arc<tokio::sync::Semaphore>,
    /// One index build per file at a time: a player's first requests arrive together.
    building: Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl Default for Cache {
    fn default() -> Self {
        Cache {
            files: Mutex::default(),
            remuxes: Mutex::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(SLOTS)),
            building: Mutex::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct FileKey {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileKey {
    fn new(path: &Path, meta: &std::fs::Metadata) -> FileKey {
        FileKey { path: path.to_path_buf(), len: meta.len(), modified: meta.modified().ok() }
    }
}

/// A file and its audio choice.
type RemuxKey = (FileKey, Option<i64>);

const CACHE_SIZE: usize = 16;

/// Most recently used last; the oldest goes when full.
fn lookup<K: PartialEq, V: Clone>(list: &Mutex<Vec<(K, V)>>, key: &K) -> Option<V> {
    let mut c = list.lock().unwrap_or_else(|e| e.into_inner());
    let i = c.iter().position(|(k, _)| k == key)?;
    let hit = c.remove(i);
    let v = hit.1.clone();
    c.push(hit);
    Some(v)
}

fn remember<K, V>(list: &Mutex<Vec<(K, V)>>, key: K, value: V, same: impl Fn(&K) -> bool, size: usize) {
    let mut c = list.lock().unwrap_or_else(|e| e.into_inner());
    c.retain(|(k, _)| !same(k));
    c.push((key, value));
    if c.len() > size {
        c.remove(0);
    }
}

impl Cache {
    /// Waits for a turn at remux work; hold it (into the blocking task) while calling
    /// [`Cache::get`] and [`Remux::segment`].
    pub async fn slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        #[expect(clippy::expect_used, reason = "the semaphore is never closed")]
        self.slots.clone().acquire_owned().await.expect("never closed")
    }

    /// The remux of `path` if it is prepared already (no slot needed to use its playlist or init
    /// segment).
    pub async fn cached(&self, path: &Path, audio: Option<i64>) -> Option<Arc<Remux>> {
        let meta = tokio::fs::metadata(path).await.ok()?;
        lookup(&self.remuxes, &(FileKey::new(path, &meta), audio))
    }

    /// The prepared remux of `path` (blocking: call off the async threads).
    pub fn get(&self, path: &Path, audio: Option<i64>) -> Result<Arc<Remux>, MkvError> {
        let meta = std::fs::metadata(path)?;
        let file = FileKey::new(path, &meta);
        let key = (file.clone(), audio);
        if let Some(r) = lookup(&self.remuxes, &key) {
            return Ok(r);
        }
        let index = match lookup(&self.files, &file) {
            Some(i) => i,
            None => self.build(&file)?,
        };
        let r = Arc::new(Remux { tracks: tracks(&index, audio)?, index });
        remember(&self.remuxes, key, r.clone(), |(k, a)| k.path == file.path && *a == audio, CACHE_SIZE * 2);
        Ok(r)
    }

    /// Indexes `file`, or waits for the request already doing it.
    fn build(&self, file: &FileKey) -> Result<Arc<Index>, MkvError> {
        let lock = {
            let mut b = self.building.lock().unwrap_or_else(|e| e.into_inner());
            b.entry(file.path.clone()).or_default().clone()
        };
        let result = {
            let _turn = lock.lock().unwrap_or_else(|e| e.into_inner());
            match lookup(&self.files, file) {
                Some(i) => Ok(i),
                None => index(&file.path).map(|i| {
                    let i = Arc::new(i);
                    remember(&self.files, file.clone(), i.clone(), |k| k.path == file.path, CACHE_SIZE);
                    i
                }),
            }
        };
        let mut b = self.building.lock().unwrap_or_else(|e| e.into_inner());
        // Ours goes under the map's lock (clones are only taken under it): when only the map's is
        // left, nobody else is waiting. Dropped after the check, two finishing requests could each
        // see the other's and leave the entry behind.
        drop(lock);
        if b.get(&file.path).is_some_and(|l| Arc::strong_count(l) == 1) {
            b.remove(&file.path);
        }
        result
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
        let pl = r.playlist("");
        assert!(
            pl.contains("#EXT-X-MAP:URI=\"init.mp4\"")
                && pl.contains("#EXTINF:2.000,\n2.m4s\n")
                && pl.ends_with("#EXT-X-ENDLIST\n")
        );
        let pl = r.playlist("?v=1");
        assert!(pl.contains("URI=\"init.mp4?v=1\"") && pl.contains("\n2.m4s?v=1\n"), "{pl}");
        let seg = r.segment(1).unwrap();
        assert_eq!(&seg[4..8], b"moof");
        // The video samples of a segment fill it exactly: the last frame runs to the next
        // segment (frames at 2000, 2040 and 2080 ms, then 4000).
        let seg0 = r.segment(0).unwrap();
        let traf = mp4::tests::boxes(mp4::tests::boxes(mp4::tests::boxes(&seg0)[0].1)[1].1);
        let trun = traf[2].1;
        let count = u32::from_be_bytes(trun[4..8].try_into().unwrap()) as usize;
        let total: u64 =
            (0..count).map(|i| u32::from_be_bytes(trun[12 + i * 16..16 + i * 16].try_into().unwrap()) as u64).sum();
        assert_eq!(total, 4 * mp4::VIDEO_TIMESCALE as u64);
        assert!(r.segment(9).is_err());
        // No audio track 2; and video only works.
        assert!(prepare(&p, Some(2)).is_err());
        assert_eq!(prepare(&p, None).unwrap().tracks.len(), 1);
        // One index per file, whatever the audio choice (bogus ones included).
        let cache = Cache::default();
        let a = cache.get(&p, Some(1)).unwrap();
        let b = cache.get(&p, Some(1)).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(cache.get(&p, Some(7)).is_err());
        let c = cache.get(&p, None).unwrap();
        assert!(Arc::ptr_eq(&a.index, &c.index) && !Arc::ptr_eq(&a, &c));
        assert_eq!(cache.files.lock().unwrap().len(), 1);
        assert!(!a.version.is_empty());
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

    #[test]
    fn odd_files_still_remux() {
        use mkv::tests::Sample;
        // A zlib-compressed subtitle track doesn't stop the film; a compressed audio track does
        // when it is chosen.
        let p = Sample { compressed_subtitles: true, ..Sample::new(3) }.write("subs");
        let r = prepare(&p, Some(1)).unwrap();
        assert_eq!(r.tracks.len(), 2);
        let _ = std::fs::remove_file(p);
        let p = Sample { compressed_audio: true, ..Sample::new(3) }.write("zaudio");
        assert!(matches!(prepare(&p, Some(1)), Err(MkvError::Unsupported(_))));
        assert!(prepare(&p, None).is_ok());
        let _ = std::fs::remove_file(p);
        // B-frames before the first keyframe's time: everything moves by 40 ms.
        let p = Sample { first_rel: -40, ..Sample::new(3) }.write("negative");
        assert_eq!(prepare(&p, Some(1)).unwrap().shift_ns, 40_000_000);
        let _ = std::fs::remove_file(p);
        // Absurd timestamp scales neither panic nor wrap.
        let p = Sample { scale: u64::MAX, ..Sample::new(3) }.write("scale");
        let h = mkv::open(&p).unwrap();
        assert_eq!(h.timecode_scale, 1_000_000_000);
        let _ = prepare(&p, Some(1)).map(|r| r.segment(0));
        let _ = std::fs::remove_file(p);
        // Open GOPs: segment 1 starts decoding 40 ms before its keyframe, so segment 0's last
        // frame ends there instead of overlapping it.
        let p = Sample { leading: true, ..Sample::new(5) }.write("opengop");
        let r = prepare(&p, None).unwrap();
        assert_eq!(mkv::earliest_pts(&p, &r.header, r.segments[1].from, LEADING_FRAMES).unwrap(), Some(3_960_000_000));
        let seg0 = r.segment(0).unwrap();
        let traf = mp4::tests::boxes(mp4::tests::boxes(mp4::tests::boxes(&seg0)[0].1)[1].1);
        let trun = traf[2].1;
        let count = u32::from_be_bytes(trun[4..8].try_into().unwrap()) as usize;
        let total: u64 =
            (0..count).map(|i| u32::from_be_bytes(trun[12 + i * 16..16 + i * 16].try_into().unwrap()) as u64).sum();
        assert_eq!(total, 3960 * 90);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn concurrent_first_requests_index_once() {
        let p = mkv::tests::write_sample("once", 5);
        let cache = Arc::new(Cache::default());
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let (cache, p) = (cache.clone(), p.clone());
                std::thread::spawn(move || cache.get(&p, if i % 2 == 0 { Some(1) } else { None }).unwrap())
            })
            .collect();
        let got: Vec<Arc<Remux>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(got.iter().all(|r| Arc::ptr_eq(&r.index, &got[0].index)));
        assert!(cache.building.lock().unwrap().is_empty());
        let _ = std::fs::remove_file(p);
    }
}
