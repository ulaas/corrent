//! A Matroska reader for remuxing: the header (tracks, timing), an index of the clusters, and
//! the frames of a run of clusters. Only what copying H.264/HEVC and AAC/AC-3/E-AC-3/Opus into
//! fragmented MP4 needs; anything else is an error and the caller transcodes instead.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub const EBML: u32 = 0x1A45_DFA3;
const DOC_TYPE: u32 = 0x4282;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const DEFAULT_DURATION: u32 = 0x23_E383;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const AUDIO: u32 = 0xE1;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const CHANNELS: u32 = 0x9F;
const CONTENT_ENCODINGS: u32 = 0x6D80;
const CONTENT_ENCODING: u32 = 0x6240;
const CONTENT_COMPRESSION: u32 = 0x5034;
const CONTENT_COMP_ALGO: u32 = 0x4254;
const CONTENT_COMP_SETTINGS: u32 = 0x4255;
const CONTENT_ENCRYPTION: u32 = 0x5035;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const REFERENCE_BLOCK: u32 = 0xFB;
const CUES: u32 = 0x1C53_BB6B;
const CUE_POINT: u32 = 0xBB;
const CUE_TIME: u32 = 0xB3;
const CUE_TRACK_POSITIONS: u32 = 0xB7;
const CUE_TRACK: u32 = 0xF7;
const CUE_CLUSTER_POSITION: u32 = 0xF1;
const CUE_RELATIVE_POSITION: u32 = 0xF0;
const SEEK_HEAD: u32 = 0x114D_9B74;
const CHAPTERS: u32 = 0x1043_A770;
const TAGS: u32 = 0x1254_C367;
const ATTACHMENTS: u32 = 0x1941_A469;
const VOID: u32 = 0xEC;
const CRC32: u32 = 0xBF;

/// Elements larger than this aren't read into memory (header parts, a cluster).
const MAX_ELEMENT: u64 = 256 * 1024 * 1024;
/// Timestamp ticks of at most a second; anything larger is damage (and would overflow).
const MAX_SCALE: u64 = 1_000_000_000;

/// Nanoseconds of `ticks` timestamp units. Values come from the file: saturate instead of
/// wrapping (release builds) or panicking (debug builds).
fn ns(ticks: i64, scale: u64) -> i64 {
    ticks.saturating_mul(scale.min(MAX_SCALE) as i64)
}

#[derive(Debug)]
pub enum MkvError {
    Io(std::io::Error),
    Invalid(&'static str),
    Unsupported(String),
}

impl std::fmt::Display for MkvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MkvError::Io(e) => write!(f, "{e}"),
            MkvError::Invalid(w) => write!(f, "not a valid Matroska file ({w})"),
            MkvError::Unsupported(w) => write!(f, "can't remux: {w}"),
        }
    }
}

impl From<std::io::Error> for MkvError {
    fn from(e: std::io::Error) -> Self {
        MkvError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, MkvError>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub number: u64,
    /// 1 video, 2 audio, 17 subtitles.
    pub kind: u64,
    pub codec: String,
    pub private: Vec<u8>,
    pub default_duration_ns: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub sample_rate: f64,
    pub channels: u32,
    /// Header-stripping compression: bytes removed from the start of every frame.
    pub strip_prefix: Vec<u8>,
    /// Why the frames can't be copied (other compression, encryption). Only matters for the
    /// tracks that are remuxed: a VobSub track compressed with zlib doesn't stop the film.
    pub unsupported: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    /// File offset of the cluster element (its id), as Cues refer to it.
    pub start: u64,
    /// File offset of the cluster's data (after its header) and its size.
    pub offset: u64,
    pub size: u64,
    /// Timestamp in nanoseconds.
    pub time_ns: i64,
    /// Starts with a keyframe of the video track (a place to cut).
    pub key: bool,
}

/// A video keyframe from the Cues: when, in which cluster, and (when the file says) where in
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub time_ns: i64,
    /// Cluster element offset in the file.
    pub cluster_start: u64,
    /// Offset of the block within the cluster's data.
    pub relative: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub timecode_scale: u64,
    pub duration_ns: Option<i64>,
    pub tracks: Vec<Track>,
    pub clusters: Vec<Cluster>,
    /// Keyframes of the video track listed in the Cues (empty without Cues).
    pub cues: Vec<Cue>,
}

/// A place in the file: a cluster and an offset within its data (block boundaries only).
pub type Pos = (usize, u64);

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub track: u64,
    pub pts_ns: i64,
    pub key: bool,
    /// A slice of the cluster read from the file (not copied), unless a prefix was put back.
    pub data: bytes::Bytes,
}

/// A variable-length integer: (value, length). `keep_marker` for element ids.
fn vint(r: &mut impl Read, keep_marker: bool) -> Result<Option<(u64, usize)>> {
    let mut first = [0u8; 1];
    if r.read(&mut first)? == 0 {
        return Ok(None);
    }
    let len = first[0].leading_zeros() as usize + 1;
    if len > 8 {
        return Err(MkvError::Invalid("variable-length integer"));
    }
    let mut v = if keep_marker { first[0] as u64 } else { (first[0] as u64) & (0xFF >> len) };
    let mut rest = [0u8; 7];
    r.read_exact(&mut rest[..len - 1])?;
    for b in &rest[..len - 1] {
        v = (v << 8) | *b as u64;
    }
    Ok(Some((v, len)))
}

/// An element header: (id, size; `None` = unknown size, header length).
fn element(r: &mut impl Read) -> Result<Option<(u32, Option<u64>, usize)>> {
    let Some((id, il)) = vint(r, true)? else { return Ok(None) };
    if il > 4 {
        return Err(MkvError::Invalid("element id"));
    }
    let (size, sl) = vint(r, false)?.ok_or(MkvError::Invalid("truncated element"))?;
    let unknown = size == (1u64 << (7 * sl)) - 1;
    Ok(Some((id as u32, (!unknown).then_some(size), il + sl)))
}

fn read_body(r: &mut impl Read, size: u64) -> Result<Vec<u8>> {
    if size > MAX_ELEMENT {
        return Err(MkvError::Invalid("element too large"));
    }
    let mut b = vec![0u8; size as usize];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0, |v, x| (v << 8) | *x as u64)
}

fn float(b: &[u8]) -> f64 {
    match b.len() {
        4 => f32::from_be_bytes(b.try_into().unwrap_or_default()) as f64,
        8 => f64::from_be_bytes(b.try_into().unwrap_or_default()),
        _ => 0.0,
    }
}

/// The children of an element body: (id, body).
fn children(mut b: &[u8]) -> Result<Vec<(u32, &[u8])>> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let mut cur = std::io::Cursor::new(b);
        let Some((id, size, hl)) = element(&mut cur)? else { break };
        let size = size.ok_or(MkvError::Invalid("unknown size inside a header element"))? as usize;
        let end = hl.checked_add(size).filter(|e| *e <= b.len()).ok_or(MkvError::Invalid("truncated child"))?;
        out.push((id, &b[hl..end]));
        b = &b[end..];
    }
    Ok(out)
}

fn track(b: &[u8]) -> Result<Track> {
    let mut t = Track::default();
    for (id, v) in children(b)? {
        match id {
            TRACK_NUMBER => t.number = uint(v),
            TRACK_TYPE => t.kind = uint(v),
            CODEC_ID => t.codec = String::from_utf8_lossy(v).trim_end_matches('\0').to_string(),
            CODEC_PRIVATE => t.private = v.to_vec(),
            DEFAULT_DURATION => t.default_duration_ns = Some(uint(v)).filter(|d| *d > 0),
            VIDEO => {
                for (id, v) in children(v)? {
                    match id {
                        PIXEL_WIDTH => t.width = uint(v) as u32,
                        PIXEL_HEIGHT => t.height = uint(v) as u32,
                        _ => {}
                    }
                }
            }
            AUDIO => {
                t.sample_rate = 8000.0;
                t.channels = 1;
                for (id, v) in children(v)? {
                    match id {
                        SAMPLING_FREQUENCY => t.sample_rate = float(v),
                        CHANNELS => t.channels = uint(v) as u32,
                        _ => {}
                    }
                }
            }
            CONTENT_ENCODINGS => {
                for (id, enc) in children(v)? {
                    if id != CONTENT_ENCODING {
                        continue;
                    }
                    for (id, c) in children(enc)? {
                        match id {
                            CONTENT_ENCRYPTION => t.unsupported = Some("encrypted track".into()),
                            CONTENT_COMPRESSION => {
                                let parts = children(c)?;
                                let algo =
                                    parts.iter().find(|(i, _)| *i == CONTENT_COMP_ALGO).map_or(0, |(_, v)| uint(v));
                                if algo != 3 {
                                    t.unsupported = Some(format!("compressed track (algorithm {algo})"));
                                    continue;
                                }
                                t.strip_prefix = parts
                                    .iter()
                                    .find(|(i, _)| *i == CONTENT_COMP_SETTINGS)
                                    .map(|(_, v)| v.to_vec())
                                    .unwrap_or_default();
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(t)
}

/// A block's header: (track, relative timestamp, flags, header length).
fn block_header(b: &[u8]) -> Result<(u64, i16, u8, usize)> {
    let mut cur = std::io::Cursor::new(b);
    let (track, tl) = vint(&mut cur, false)?.ok_or(MkvError::Invalid("block"))?;
    if b.len() < tl + 3 {
        return Err(MkvError::Invalid("short block"));
    }
    let rel = i16::from_be_bytes([b[tl], b[tl + 1]]);
    Ok((track, rel, b[tl + 2], tl + 3))
}

/// The frames in a block body (after its header), undoing lacing.
fn unlace(flags: u8, b: &[u8]) -> Result<Vec<&[u8]>> {
    let lacing = (flags >> 1) & 3;
    if lacing == 0 {
        return Ok(vec![b]);
    }
    let count = *b.first().ok_or(MkvError::Invalid("lace"))? as usize + 1;
    let mut pos = 1;
    let mut sizes: Vec<usize> = Vec::with_capacity(count);
    match lacing {
        // Xiph: 255-continued sizes.
        1 => {
            for _ in 0..count - 1 {
                let mut s = 0usize;
                loop {
                    let x = *b.get(pos).ok_or(MkvError::Invalid("lace"))?;
                    pos += 1;
                    s += x as usize;
                    if x != 255 {
                        break;
                    }
                }
                sizes.push(s);
            }
        }
        // Fixed: equal sizes.
        2 => {
            let rest = b.len() - 1;
            if !rest.is_multiple_of(count) {
                return Err(MkvError::Invalid("fixed lace"));
            }
            sizes.extend(std::iter::repeat_n(rest / count, count - 1));
        }
        // EBML: the first size, then signed differences.
        _ => {
            let mut cur = std::io::Cursor::new(&b[pos..]);
            let (first, l) = vint(&mut cur, false)?.ok_or(MkvError::Invalid("lace"))?;
            pos += l;
            let mut last = first as i64;
            sizes.push(first as usize);
            for _ in 1..count - 1 {
                let mut cur = std::io::Cursor::new(&b[pos..]);
                let (raw, l) = vint(&mut cur, false)?.ok_or(MkvError::Invalid("lace"))?;
                pos += l;
                let bias = (1i64 << (7 * l - 1)) - 1;
                last = last.checked_add(raw as i64 - bias).ok_or(MkvError::Invalid("lace"))?;
                if last < 0 || last as usize > b.len() {
                    return Err(MkvError::Invalid("lace"));
                }
                sizes.push(last as usize);
            }
        }
    }
    // Sizes come from the file: bound them before adding (crafted ones could wrap the sum).
    let mut used = 0usize;
    for s in &sizes {
        used = used.checked_add(*s).filter(|u| *u <= b.len()).ok_or(MkvError::Invalid("lace sizes"))?;
    }
    let last = b
        .len()
        .checked_sub(pos.checked_add(used).ok_or(MkvError::Invalid("lace sizes"))?)
        .ok_or(MkvError::Invalid("lace sizes"))?;
    sizes.push(last);
    let mut out = Vec::with_capacity(count);
    for s in sizes {
        out.push(&b[pos..pos + s]);
        pos += s;
    }
    Ok(out)
}

/// Reads the header and indexes every cluster (one small read each; the frames aren't read).
pub fn open(path: &Path) -> Result<Header> {
    // Every cluster seeks, which drops the buffer: keep it small (one read per cluster).
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut r = BufReader::with_capacity(8 * 1024, file);
    let (id, size, _) = element(&mut r)?.ok_or(MkvError::Invalid("empty file"))?;
    if id != EBML {
        return Err(MkvError::Invalid("no EBML header"));
    }
    let head = read_body(&mut r, size.ok_or(MkvError::Invalid("EBML header size"))?)?;
    let doc = children(&head)?.into_iter().find(|(i, _)| *i == DOC_TYPE).map(|(_, v)| v.to_vec()).unwrap_or_default();
    if doc != b"matroska" && doc != b"webm" {
        return Err(MkvError::Invalid("not a Matroska document"));
    }
    let (id, seg_size, _) = element(&mut r)?.ok_or(MkvError::Invalid("no segment"))?;
    if id != SEGMENT {
        return Err(MkvError::Invalid("no segment"));
    }
    let seg_start = r.stream_position()?;
    let seg_end = seg_size.map(|s| seg_start.saturating_add(s));
    let mut h = Header {
        timecode_scale: 1_000_000,
        duration_ns: None,
        tracks: Vec::new(),
        clusters: Vec::new(),
        cues: Vec::new(),
    };
    let mut raw_cues: Vec<(u64, u64, u64, Option<u64>)> = Vec::new();
    let mut duration_ticks: Option<f64> = None;
    let mut video: Option<u64> = None;
    loop {
        let pos = r.stream_position()?;
        if seg_end.is_some_and(|e| pos >= e) {
            break;
        }
        let el = match element(&mut r) {
            Ok(Some(e)) => e,
            Ok(None) => break,
            // Damaged data: carry on at the next cluster.
            Err(MkvError::Invalid(_)) => {
                if resync(&mut r, pos + 1)? {
                    continue;
                }
                break;
            }
            Err(e) => return Err(e),
        };
        let (id, size, _) = el;
        match id {
            INFO => {
                let b = read_body(&mut r, size.ok_or(MkvError::Invalid("info size"))?)?;
                for (id, v) in children(&b)? {
                    match id {
                        TIMECODE_SCALE => h.timecode_scale = uint(v).clamp(1, MAX_SCALE),
                        DURATION => duration_ticks = Some(float(v)),
                        _ => {}
                    }
                }
            }
            TRACKS => {
                let b = read_body(&mut r, size.ok_or(MkvError::Invalid("tracks size"))?)?;
                for (id, v) in children(&b)? {
                    if id == TRACK_ENTRY {
                        h.tracks.push(track(v)?);
                    }
                }
                video = h.tracks.iter().find(|t| t.kind == 1).map(|t| t.number);
            }
            CLUSTER => {
                let size =
                    size.ok_or_else(|| MkvError::Unsupported("clusters of unknown size (a live recording)".into()))?;
                let offset = r.stream_position()?;
                // A cut-off file: the last cluster ends where the file does.
                let size = size.min(file_len.saturating_sub(offset));
                match cluster_start(&mut r, size, video, h.timecode_scale) {
                    Ok((time, key)) => {
                        h.clusters.push(Cluster { start: pos, offset, size, time_ns: time, key });
                        r.seek(SeekFrom::Start(offset + size))?;
                    }
                    // A damaged cluster is left out; carry on at the next one.
                    Err(e) if is_damage(&e) => {
                        if !resync(&mut r, pos + 1)? {
                            break;
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            CUES => {
                let b = read_body(&mut r, size.ok_or(MkvError::Invalid("cues size"))?)?;
                for (id, cp) in children_lenient(&b) {
                    if id != CUE_POINT {
                        continue;
                    }
                    let kids = children_lenient(cp);
                    let Some(time) = kids.iter().find(|(i, _)| *i == CUE_TIME).map(|(_, v)| uint(v)) else { continue };
                    for (id, tp) in kids {
                        if id != CUE_TRACK_POSITIONS {
                            continue;
                        }
                        let f = children_lenient(tp);
                        let get = |want: u32| f.iter().find(|(i, _)| *i == want).map(|(_, v)| uint(v));
                        if let (Some(track), Some(cluster)) = (get(CUE_TRACK), get(CUE_CLUSTER_POSITION)) {
                            raw_cues.push((time, track, cluster, get(CUE_RELATIVE_POSITION)));
                        }
                    }
                }
            }
            SEEK_HEAD | CHAPTERS | TAGS | ATTACHMENTS | VOID | CRC32 => match size {
                Some(s) => {
                    r.seek_relative(s as i64)?;
                }
                None => return Err(MkvError::Invalid("unknown-size element")),
            },
            // Not a top-level element: junk or damage. Skipping by its "size" could jump over
            // clusters, so look for the next cluster instead.
            _ => {
                if !resync(&mut r, pos + 1)? {
                    break;
                }
            }
        }
    }
    if h.tracks.is_empty() {
        return Err(MkvError::Invalid("no tracks"));
    }
    h.duration_ns = duration_ticks.map(|d| (d * h.timecode_scale as f64) as i64);
    if let Some(v) = video {
        h.cues = raw_cues
            .into_iter()
            .filter(|c| c.1 == v)
            .map(|(time, _, cluster, relative)| Cue {
                time_ns: ns(time.min(i64::MAX as u64) as i64, h.timecode_scale),
                cluster_start: seg_start.saturating_add(cluster),
                relative,
            })
            .collect();
        h.cues.sort_by_key(|c| (c.cluster_start, c.relative));
    }
    Ok(h)
}

/// Errors that mean damaged data (rather than a failing disk).
fn is_damage(e: &MkvError) -> bool {
    match e {
        MkvError::Invalid(_) => true,
        MkvError::Io(e) => e.kind() == std::io::ErrorKind::UnexpectedEof,
        MkvError::Unsupported(_) => false,
    }
}

/// Moves to the next cluster id at or after `from`; false at the end of the file.
fn resync(r: &mut BufReader<File>, from: u64) -> Result<bool> {
    let magic = CLUSTER.to_be_bytes();
    r.seek(SeekFrom::Start(from))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut base = from;
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(false);
        }
        let mut hay = std::mem::take(&mut carry);
        let hay_start = base - hay.len() as u64;
        hay.extend_from_slice(&buf[..n]);
        if let Some(i) = hay.windows(4).position(|w| w == magic) {
            r.seek(SeekFrom::Start(hay_start + i as u64))?;
            return Ok(true);
        }
        base += n as u64;
        carry = hay[hay.len().saturating_sub(3)..].to_vec();
    }
}

/// The video keyframes inside cluster `i`: (offset in its data, timestamp). Reads the cluster.
pub fn keyframes(path: &Path, h: &Header, i: usize) -> Result<Vec<(u64, i64)>> {
    let Some(video) = h.tracks.iter().find(|t| t.kind == 1).map(|t| t.number) else { return Ok(Vec::new()) };
    let c = h.clusters.get(i).ok_or(MkvError::Invalid("cluster"))?;
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(c.offset))?;
    let body = read_body(&mut f, c.size)?;
    let mut time: i64 = c.time_ns / h.timecode_scale as i64;
    let mut out = Vec::new();
    for (off, id, v) in children_at(&body) {
        let (blk, group_key) = match id {
            TIMECODE => {
                time = uint(v).min(i64::MAX as u64) as i64;
                continue;
            }
            SIMPLE_BLOCK => (v, None),
            BLOCK_GROUP => {
                let kids = children_lenient(v);
                let Some((_, b)) = kids.iter().find(|(i, _)| *i == BLOCK) else { continue };
                (*b, Some(!kids.iter().any(|(i, _)| *i == REFERENCE_BLOCK)))
            }
            _ => continue,
        };
        let (track, rel, flags, _) = block_header(blk)?;
        if track == video && group_key.unwrap_or(flags & 0x80 != 0) {
            out.push((off as u64, ns(time.saturating_add(rel as i64), h.timecode_scale)));
        }
    }
    Ok(out)
}

/// A cluster's timestamp and whether its first video block is a keyframe (reads only headers).
fn cluster_start(r: &mut BufReader<File>, size: u64, video: Option<u64>, scale: u64) -> Result<(i64, bool)> {
    let end = r.stream_position()? + size;
    let mut time: Option<i64> = None;
    while r.stream_position()? < end {
        let Some((id, s, _)) = element(r)? else { break };
        let s = s.ok_or(MkvError::Invalid("unknown size in a cluster"))?;
        match id {
            TIMECODE => time = Some(uint(&read_body(r, s.min(8))?).min(i64::MAX as u64) as i64),
            SIMPLE_BLOCK | BLOCK_GROUP => {
                let Some(t) = time else { return Err(MkvError::Invalid("block before the cluster timestamp")) };
                let (track, key) = if id == SIMPLE_BLOCK {
                    // Enough of the block to see its track and flags.
                    let peek = s.min(16);
                    let b = read_body(r, peek)?;
                    r.seek_relative((s - peek) as i64)?;
                    let (tr, _, flags, _) = block_header(&b)?;
                    (tr, flags & 0x80 != 0)
                } else {
                    let Some(found) = group_start(r, s)? else { continue };
                    found
                };
                if Some(track) == video || video.is_none() {
                    return Ok((ns(t, scale), key));
                }
            }
            _ => {
                r.seek_relative(s as i64)?;
            }
        }
    }
    Ok((ns(time.unwrap_or(0), scale), video.is_none()))
}

/// A block group's track and whether it is a keyframe (no `ReferenceBlock`), reading only the
/// headers of its children: the block can be megabytes with the reference after it.
fn group_start(r: &mut BufReader<File>, size: u64) -> Result<Option<(u64, bool)>> {
    let end = r.stream_position()? + size;
    let (mut track, mut reference) = (None, false);
    while r.stream_position()? < end {
        let Some((id, s, _)) = element(r)? else { break };
        let s = s.ok_or(MkvError::Invalid("unknown size in a block group"))?;
        let mut skip = s;
        match id {
            BLOCK => {
                let peek = s.min(16);
                track = Some(block_header(&read_body(r, peek)?)?.0);
                skip = s - peek;
            }
            REFERENCE_BLOCK => reference = true,
            _ => {}
        }
        r.seek_relative(skip.min(i64::MAX as u64) as i64)?;
    }
    r.seek(SeekFrom::Start(end))?;
    Ok(track.map(|t| (t, !reference)))
}

/// Children of a possibly cut-off body (the last one may be partial: it's dropped).
fn children_lenient(b: &[u8]) -> Vec<(u32, &[u8])> {
    children_at(b).into_iter().map(|(_, id, v)| (id, v)).collect()
}

/// Like [`children_lenient`], with each child's offset in `b`.
fn children_at(b: &[u8]) -> Vec<(usize, u32, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < b.len() {
        let mut cur = std::io::Cursor::new(&b[pos..]);
        let Ok(Some((id, Some(size), hl))) = element(&mut cur) else { break };
        let Some(end) = (pos + hl).checked_add(size as usize).filter(|e| *e <= b.len()) else { break };
        out.push((pos, id, &b[pos + hl..end]));
        pos = end;
    }
    out
}

/// Every frame of `tracks` between positions `from` (inclusive) and `to` (exclusive), in file
/// order.
pub fn frames(path: &Path, h: &Header, from: Pos, to: Pos, tracks: &[u64]) -> Result<Vec<Frame>> {
    let mut f = File::open(path)?;
    let mut out = Vec::new();
    let last = if to.1 == 0 { to.0 } else { to.0 + 1 };
    let clusters = h.clusters.get(from.0..last.min(h.clusters.len())).ok_or(MkvError::Invalid("cluster range"))?;
    for (k, c) in clusters.iter().enumerate() {
        let ci = from.0 + k;
        f.seek(SeekFrom::Start(c.offset))?;
        // Frames are slices of this buffer, so they are copied only once more (into the segment).
        let body = bytes::Bytes::from(read_body(&mut f, c.size)?);
        let mut time: i64 = c.time_ns / h.timecode_scale as i64;
        for (off, id, v) in children_at(&body) {
            // The cluster timestamp counts wherever it is; blocks only inside the range.
            let here = (ci, off as u64);
            if id != TIMECODE && (here < from || here >= to) {
                continue;
            }
            let (blk, group_key) = match id {
                TIMECODE => {
                    time = uint(v).min(i64::MAX as u64) as i64;
                    continue;
                }
                SIMPLE_BLOCK => (v, None),
                BLOCK_GROUP => {
                    let kids = children_lenient(v);
                    let Some((_, b)) = kids.iter().find(|(i, _)| *i == BLOCK) else { continue };
                    (*b, Some(!kids.iter().any(|(i, _)| *i == REFERENCE_BLOCK)))
                }
                _ => continue,
            };
            let (track, rel, flags, hl) = block_header(blk)?;
            if !tracks.contains(&track) {
                continue;
            }
            let key = group_key.unwrap_or(flags & 0x80 != 0);
            let prefix = h.tracks.iter().find(|t| t.number == track).map(|t| t.strip_prefix.as_slice()).unwrap_or(&[]);
            let pts_ns = ns(time.saturating_add(rel as i64), h.timecode_scale);
            for (i, data) in unlace(flags, &blk[hl..])?.into_iter().enumerate() {
                let data =
                    if prefix.is_empty() { body.slice_ref(data) } else { bytes::Bytes::from([prefix, data].concat()) };
                // Laced frames share the block's timestamp; the muxer spaces audio by frame size.
                out.push(Frame { track, pts_ns, key: key && i == 0, data });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn ebml_size(n: usize) -> Vec<u8> {
        // 8-byte sizes keep the writer simple.
        let mut v = vec![0x01];
        v.extend_from_slice(&(n as u64).to_be_bytes()[1..]);
        v
    }

    pub fn el(id: u32, body: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = id.to_be_bytes().iter().copied().skip_while(|b| *b == 0).collect();
        v.extend(ebml_size(body.len()));
        v.extend_from_slice(body);
        v
    }

    pub fn u(id: u32, x: u64) -> Vec<u8> {
        el(id, &x.to_be_bytes())
    }

    fn simple_block(track: u8, rel: i16, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut b = vec![0x80 | track];
        b.extend_from_slice(&rel.to_be_bytes());
        b.push(flags);
        b.extend_from_slice(data);
        el(SIMPLE_BLOCK, &b)
    }

    /// A small Matroska file: one H.264 track and one AAC track, `clusters` clusters of a
    /// keyframe + two frames of video and two audio frames (one of them a Xiph lace of two).
    pub struct Sample {
        pub clusters: usize,
        /// Every cluster after the first opens with two frames of the previous picture group
        /// and the keyframe comes after them (no Cues).
        pub mid_key: bool,
        pub scale: u64,
        /// Relative timestamp of the first cluster's keyframe.
        pub first_rel: i16,
        /// Adds a zlib-compressed subtitle track.
        pub compressed_subtitles: bool,
        /// Marks the audio track zlib-compressed.
        pub compressed_audio: bool,
        /// Cluster bodies added after the generated ones.
        pub extra: Vec<Vec<u8>>,
    }

    impl Sample {
        pub fn new(clusters: usize) -> Sample {
            Sample {
                clusters,
                mid_key: false,
                scale: 1_000_000,
                first_rel: 0,
                compressed_subtitles: false,
                compressed_audio: false,
                extra: Vec::new(),
            }
        }

        pub fn bytes(&self) -> Vec<u8> {
            let mut out = el(EBML, &el(DOC_TYPE, b"matroska"));
            let mut seg = el(
                INFO,
                &[u(TIMECODE_SCALE, self.scale), el(DURATION, &(self.clusters as f64 * 2000.0).to_be_bytes())].concat(),
            );
            let avcc = [1u8, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 4, 0x67, 0x64, 0, 0x1f, 1, 0, 4, 0x68, 0xee, 0x3c, 0x80];
            let zlib = el(CONTENT_ENCODINGS, &el(CONTENT_ENCODING, &el(CONTENT_COMPRESSION, &u(CONTENT_COMP_ALGO, 0))));
            let video = el(
                TRACK_ENTRY,
                &[
                    u(TRACK_NUMBER, 1),
                    u(TRACK_TYPE, 1),
                    el(CODEC_ID, b"V_MPEG4/ISO/AVC"),
                    el(CODEC_PRIVATE, &avcc),
                    u(DEFAULT_DURATION, 40_000_000),
                    el(VIDEO, &[u(PIXEL_WIDTH, 320), u(PIXEL_HEIGHT, 240)].concat()),
                ]
                .concat(),
            );
            let mut audio = [
                u(TRACK_NUMBER, 2),
                u(TRACK_TYPE, 2),
                el(CODEC_ID, b"A_AAC"),
                el(CODEC_PRIVATE, &[0x11, 0x90]),
                el(AUDIO, &[el(SAMPLING_FREQUENCY, &48000f64.to_be_bytes()), u(CHANNELS, 2)].concat()),
            ]
            .concat();
            if self.compressed_audio {
                audio.extend(&zlib);
            }
            let mut tracks = [video, el(TRACK_ENTRY, &audio)].concat();
            if self.compressed_subtitles {
                tracks.extend(el(
                    TRACK_ENTRY,
                    &[u(TRACK_NUMBER, 3), u(TRACK_TYPE, 17), el(CODEC_ID, b"S_VOBSUB"), zlib.clone()].concat(),
                ));
            }
            seg.extend(el(TRACKS, &tracks));
            for c in 0..self.clusters {
                let mut body = u(TIMECODE, c as u64 * 2000);
                let sb = simple_block;
                if self.mid_key && c > 0 {
                    body.extend(sb(1, -40, 0, &[0, 0, 0, 2, 0x41, 9]));
                    body.extend(sb(2, -21, 0, &[0xDD; 3]));
                }
                // Decode order I P B: presentation 0, 80, 40 ms.
                let rel = if c == 0 { self.first_rel } else { 0 };
                body.extend(sb(1, rel, 0x80, &[0, 0, 0, 2, 0x65, c as u8]));
                body.extend(sb(2, 0, 0, &[0xAA; 6]));
                body.extend(sb(1, 80, 0, &[0, 0, 0, 2, 0x41, 1]));
                body.extend(sb(1, 40, 0, &[0, 0, 0, 2, 0x01, 2]));
                // A Xiph lace of two audio frames (3 + 4 bytes).
                body.extend(sb(2, 21, 0x02, &[1, 3, 0xB1, 0xB2, 0xB3, 0xC1, 0xC2, 0xC3, 0xC4]));
                seg.extend(el(CLUSTER, &body));
            }
            for body in &self.extra {
                seg.extend(el(CLUSTER, body));
            }
            out.extend(el(SEGMENT, &seg));
            out
        }

        pub fn write(&self, name: &str) -> std::path::PathBuf {
            let p = std::env::temp_dir().join(format!("corrent-remux-{}-{name}.mkv", std::process::id()));
            std::fs::write(&p, self.bytes()).unwrap();
            p
        }
    }

    pub fn write_sample(name: &str, clusters: usize) -> std::path::PathBuf {
        Sample::new(clusters).write(name)
    }

    pub fn write_sample_with(name: &str, clusters: usize, mid_key: bool) -> std::path::PathBuf {
        Sample { mid_key, ..Sample::new(clusters) }.write(name)
    }

    #[test]
    fn big_block_groups_read_their_reference() {
        // A 1.5 MiB video frame in a block group with its ReferenceBlock after it: not a
        // keyframe, although the reference lies beyond any small peek.
        let group = |reference: bool| {
            let mut blk = vec![0x81, 0, 0, 0];
            blk.extend(std::iter::repeat_n(0x41u8, 1536 * 1024));
            let mut g = el(BLOCK, &blk);
            if reference {
                g.extend(u(REFERENCE_BLOCK, 1));
            }
            el(BLOCK_GROUP, &g)
        };
        let extra = vec![[u(TIMECODE, 0), group(false)].concat(), [u(TIMECODE, 2000), group(true)].concat()];
        let p = Sample { extra, ..Sample::new(0) }.write("group");
        let h = open(&p).unwrap();
        assert_eq!(h.clusters.iter().map(|c| c.key).collect::<Vec<_>>(), vec![true, false]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn damaged_and_cut_off_files_index() {
        let good = Sample::new(5).bytes();
        let h = {
            let p = std::env::temp_dir().join(format!("corrent-remux-{}-good.mkv", std::process::id()));
            std::fs::write(&p, &good).unwrap();
            let h = open(&p).unwrap();
            let _ = std::fs::remove_file(p);
            h
        };
        // Cluster 2's timestamp turned into a block: that cluster is left out, the rest stay.
        let mut bad = good.clone();
        let at = h.clusters[2].offset as usize;
        assert_eq!(bad[at], 0xE7);
        bad[at] = SIMPLE_BLOCK as u8;
        let p = std::env::temp_dir().join(format!("corrent-remux-{}-damaged.mkv", std::process::id()));
        std::fs::write(&p, &bad).unwrap();
        let d = open(&p).unwrap();
        assert_eq!(d.clusters.iter().map(|c| c.time_ns / 1_000_000_000).collect::<Vec<_>>(), vec![0, 2, 6, 8]);
        // Cut off in the middle of the last cluster: it ends where the file does.
        let cut = &good[..good.len() - 10];
        std::fs::write(&p, cut).unwrap();
        let c = open(&p).unwrap();
        let last = c.clusters.last().unwrap();
        assert_eq!((c.clusters.len(), last.offset + last.size), (5, cut.len() as u64));
        assert!(frames(&p, &c, (4, 0), (5, 0), &[1, 2]).is_ok());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn reads_header_index_and_frames() {
        let p = write_sample("mkv", 3);
        let h = open(&p).unwrap();
        assert_eq!(h.tracks.len(), 2);
        assert_eq!((h.tracks[0].codec.as_str(), h.tracks[0].width, h.tracks[0].height), ("V_MPEG4/ISO/AVC", 320, 240));
        assert_eq!((h.tracks[1].sample_rate, h.tracks[1].channels), (48000.0, 2));
        assert_eq!(h.duration_ns, Some(6_000_000_000));
        assert_eq!(h.clusters.len(), 3);
        assert!(h.clusters.iter().all(|c| c.key));
        assert_eq!(h.clusters[2].time_ns, 4_000_000_000);
        let f = frames(&p, &h, (1, 0), (2, 0), &[1, 2]).unwrap();
        let v: Vec<_> = f.iter().filter(|x| x.track == 1).map(|x| (x.pts_ns / 1_000_000, x.key)).collect();
        assert_eq!(v, vec![(2000, true), (2080, false), (2040, false)]);
        let a: Vec<_> = f.iter().filter(|x| x.track == 2).map(|x| x.data.clone()).collect();
        assert_eq!(a, vec![vec![0xAA; 6], vec![0xB1, 0xB2, 0xB3], vec![0xC1, 0xC2, 0xC3, 0xC4]]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn lacing_kinds() {
        // Fixed: 3 frames of 2 bytes.
        assert_eq!(unlace(0x04, &[2, 1, 1, 2, 2, 3, 3]).unwrap(), vec![&[1, 1][..], &[2, 2], &[3, 3]]);
        // EBML: sizes 2, then +1 (0xC0 = 1 + bias 63... as a 1-byte signed vint: 63 + 1 = 64 → 0xC0).
        assert_eq!(unlace(0x06, &[2, 0x82, 0xC0, 1, 1, 2, 2, 2, 3]).unwrap(), vec![&[1, 1][..], &[2, 2, 2], &[3]]);
        assert!(unlace(0x02, &[1, 200, 1]).is_err());
        // EBML lace sizes that wrap when summed (255 frames of 2^56 - 2 bytes) are refused.
        let mut huge = vec![254u8, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE];
        huge.extend(std::iter::repeat_n(0xBF, 253));
        huge.extend([0u8; 64]);
        assert!(unlace(0x06, &huge).is_err());
        assert!(open(Path::new("/nonexistent/x.mkv")).is_err());
    }

    #[test]
    fn refuses_what_it_cant_copy() {
        let p = std::env::temp_dir().join(format!("corrent-remux-{}-bad.mkv", std::process::id()));
        std::fs::write(&p, b"RIFF....WAVE").unwrap();
        assert!(matches!(open(&p), Err(MkvError::Invalid(_))));
        // Random bytes never panic.
        let mut x = 7u64;
        for len in [0usize, 1, 5, 64, 300] {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect();
            let mut file = el(EBML, &el(DOC_TYPE, b"matroska"));
            file.extend(bytes);
            std::fs::write(&p, &file).unwrap();
            let _ = open(&p);
        }
        let _ = std::fs::remove_file(p);
    }
}
