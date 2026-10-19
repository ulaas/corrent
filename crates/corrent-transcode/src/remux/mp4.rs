//! Fragmented MP4 (CMAF-style) for HLS: an init segment (`ftyp` + `moov`) and media segments
//! (`moof` + `mdat`) from frames copied out of Matroska.

use super::mkv::{Frame, Track};

/// Video timestamps in 90 kHz; audio in its sample rate.
pub const VIDEO_TIMESCALE: u32 = 90_000;

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + body.len());
    v.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// How a track goes into MP4.
#[derive(Clone, Debug, PartialEq)]
pub enum Codec {
    Avc { avcc: Vec<u8> },
    Hevc { hvcc: Vec<u8> },
    Aac { asc: Vec<u8> },
    Ac3 { dac3: [u8; 3] },
    Eac3 { dec3: Vec<u8>, samples: u32 },
    Opus { dops: Vec<u8>, pre_skip: u16 },
    Mp3 { samples: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutTrack {
    /// Matroska track number.
    pub source: u64,
    /// MP4 track id (1-based).
    pub id: u32,
    pub codec: Codec,
    pub timescale: u32,
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub sample_rate: u32,
    /// Duration of one video frame when the next one isn't known (timescale units).
    pub frame_duration: u32,
}

impl OutTrack {
    pub fn is_video(&self) -> bool {
        matches!(self.codec, Codec::Avc { .. } | Codec::Hevc { .. })
    }

    /// The HLS `CODECS` string part for this track.
    pub fn codec_string(&self) -> String {
        match &self.codec {
            Codec::Avc { avcc } if avcc.len() >= 4 => format!("avc1.{:02x}{:02x}{:02x}", avcc[1], avcc[2], avcc[3]),
            Codec::Avc { .. } => "avc1.640028".into(),
            Codec::Hevc { hvcc } => hevc_codec_string(hvcc),
            Codec::Aac { asc } => format!("mp4a.40.{}", asc.first().map_or(2, |b| b >> 3)),
            Codec::Ac3 { .. } => "ac-3".into(),
            Codec::Eac3 { .. } => "ec-3".into(),
            Codec::Opus { .. } => "opus".into(),
            Codec::Mp3 { .. } => "mp4a.6B".into(),
        }
    }
}

/// `hvc1.<profile>.<compat>.<tier><level>.<constraints>` from an hvcC record.
fn hevc_codec_string(h: &[u8]) -> String {
    if h.len() < 13 {
        return "hvc1.1.6.L93.B0".into();
    }
    let space = match h[1] >> 6 {
        1 => "A",
        2 => "B",
        3 => "C",
        _ => "",
    };
    let profile = h[1] & 0x1f;
    let tier = if h[1] & 0x20 != 0 { "H" } else { "L" };
    let compat = u32::from_be_bytes([h[2], h[3], h[4], h[5]]).reverse_bits();
    let level = h[12];
    let mut cons: Vec<u8> = h[6..12].to_vec();
    while cons.last() == Some(&0) {
        cons.pop();
    }
    let cons: Vec<String> = cons.iter().map(|b| format!("{b:X}")).collect();
    let mut s = format!("hvc1.{space}{profile}.{compat:X}.{tier}{level}");
    if !cons.is_empty() {
        s.push('.');
        s.push_str(&cons.join("."));
    }
    s
}

/// What to do with a Matroska track, or why it can't be copied.
pub fn out_track(t: &Track, id: u32, first_frame: Option<&[u8]>) -> Result<OutTrack, String> {
    if let Some(why) = &t.unsupported {
        return Err(why.clone());
    }
    let video = |codec| OutTrack {
        source: t.number,
        id,
        codec,
        timescale: VIDEO_TIMESCALE,
        width: t.width,
        height: t.height,
        channels: 0,
        sample_rate: 0,
        frame_duration: t
            .default_duration_ns
            .map(|d| (d as u128 * VIDEO_TIMESCALE as u128 / 1_000_000_000) as u32)
            .filter(|d| *d > 0)
            .unwrap_or(VIDEO_TIMESCALE / 24),
    };
    let rate = t.sample_rate.round().max(1.0) as u32;
    let audio = |codec, rate: u32| OutTrack {
        source: t.number,
        id,
        codec,
        timescale: rate,
        width: 0,
        height: 0,
        channels: t.channels.max(1),
        sample_rate: rate,
        frame_duration: 0,
    };
    match t.codec.as_str() {
        "V_MPEG4/ISO/AVC" if t.private.len() >= 7 => Ok(video(Codec::Avc { avcc: t.private.clone() })),
        "V_MPEGH/ISO/HEVC" if t.private.len() >= 23 => Ok(video(Codec::Hevc { hvcc: t.private.clone() })),
        c if c == "A_AAC" || c.starts_with("A_AAC/") => {
            let asc = if t.private.len() >= 2 { t.private.clone() } else { aac_config(c, rate, t.channels) };
            Ok(audio(Codec::Aac { asc }, rate))
        }
        "A_AC3" => {
            let f = first_frame.ok_or("no AC-3 frame")?;
            let (dac3, sr) = ac3_info(f).ok_or("not an AC-3 frame")?;
            Ok(audio(Codec::Ac3 { dac3 }, sr))
        }
        "A_EAC3" => {
            let f = first_frame.ok_or("no E-AC-3 frame")?;
            let (dec3, sr, samples) = eac3_info(f).ok_or("not a supported E-AC-3 frame")?;
            Ok(audio(Codec::Eac3 { dec3, samples }, sr))
        }
        "A_OPUS" if t.private.len() >= 19 && t.private.starts_with(b"OpusHead") => {
            let p = &t.private;
            let pre_skip = u16::from_le_bytes([p[10], p[11]]);
            // dOps is OpusHead without the magic, big-endian, version 0.
            let mut d = vec![0, p[9]];
            d.extend_from_slice(&pre_skip.to_be_bytes());
            d.extend_from_slice(&u32::from_le_bytes([p[12], p[13], p[14], p[15]]).to_be_bytes());
            d.extend_from_slice(&i16::from_le_bytes([p[16], p[17]]).to_be_bytes());
            d.push(p[18]);
            if p[18] != 0 {
                d.extend_from_slice(p.get(19..21 + p[9] as usize).ok_or("short Opus header")?);
            }
            Ok(audio(Codec::Opus { dops: d, pre_skip }, 48_000))
        }
        "A_MPEG/L3" => {
            // MPEG-1 frames hold 1152 samples, MPEG-2/2.5 (low sample rates) 576.
            let samples = match first_frame {
                Some(f) if f.len() >= 4 && f[0] == 0xFF && f[1] & 0xE0 == 0xE0 => {
                    if (f[1] >> 3) & 3 == 3 {
                        1152
                    } else {
                        576
                    }
                }
                _ if rate >= 32000 => 1152,
                _ => 576,
            };
            Ok(audio(Codec::Mp3 { samples }, rate))
        }
        c => Err(format!("codec {c} isn't copied")),
    }
}

/// An AudioSpecificConfig for old `A_AAC/MPEG4/LC`-style ids without codec data.
fn aac_config(id: &str, rate: u32, channels: u32) -> Vec<u8> {
    let object: u8 = if id.ends_with("/MAIN") {
        1
    } else if id.ends_with("/SSR") {
        3
    } else {
        2
    };
    const RATES: [u32; 12] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000];
    let idx = RATES.iter().position(|r| *r == rate).unwrap_or(3) as u8;
    let ch = channels.clamp(1, 7) as u8;
    vec![(object << 3) | (idx >> 1), ((idx & 1) << 7) | (ch << 3)]
}

struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn get(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.b.get(self.pos / 8)?;
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        Some(v)
    }
}

/// dac3 and the sample rate from an AC-3 sync frame.
pub fn ac3_info(f: &[u8]) -> Option<([u8; 3], u32)> {
    if f.len() < 8 || f[0] != 0x0B || f[1] != 0x77 {
        return None;
    }
    let mut b = Bits { b: &f[4..], pos: 0 };
    let fscod = b.get(2)?;
    let frmsizecod = b.get(6)?;
    let bsid = b.get(5)?;
    let bsmod = b.get(3)?;
    let acmod = b.get(3)?;
    if acmod & 1 != 0 && acmod != 1 {
        b.get(2)?;
    }
    if acmod & 4 != 0 {
        b.get(2)?;
    }
    if acmod == 2 {
        b.get(2)?;
    }
    let lfeon = b.get(1)?;
    let sr = [48000, 44100, 32000].get(fscod as usize).copied()?;
    let v: u32 =
        (fscod << 22) | (bsid << 17) | (bsmod << 14) | (acmod << 11) | (lfeon << 10) | ((frmsizecod >> 1) << 5);
    let x = v.to_be_bytes();
    Some(([x[1], x[2], x[3]], sr))
}

/// dec3, the sample rate and samples per frame from an E-AC-3 frame (one independent
/// substream; dependent substreams aren't described).
pub fn eac3_info(f: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if f.len() < 8 || f[0] != 0x0B || f[1] != 0x77 {
        return None;
    }
    let mut b = Bits { b: &f[2..], pos: 0 };
    let strmtyp = b.get(2)?;
    let _substreamid = b.get(3)?;
    let frmsiz = b.get(11)?;
    let fscod = b.get(2)?;
    let (sr, blocks) = if fscod == 3 {
        let fscod2 = b.get(2)?;
        ([24000, 22050, 16000].get(fscod2 as usize).copied()?, 6)
    } else {
        let numblkscod = b.get(2)?;
        ([48000, 44100, 32000][fscod as usize], [1, 2, 3, 6][numblkscod as usize])
    };
    let acmod = b.get(3)?;
    let lfeon = b.get(1)?;
    let bsid = b.get(5)?;
    if strmtyp == 1 || bsid <= 10 {
        return None;
    }
    let samples = 256 * blocks as u32;
    let frame_bytes = (frmsiz + 1) * 2;
    let data_rate = (frame_bytes as u64 * 8 * sr as u64 / samples as u64 / 1000) as u32;
    // data_rate(13) num_ind_sub(3) | fscod(2) bsid(5) reserved(1) asvc(1) bsmod(3) acmod(3)
    // lfeon(1) reserved(3) num_dep_sub(4) reserved(1)
    let mut out = Vec::new();
    let head = (data_rate.min(8191) << 3) as u16;
    out.extend_from_slice(&head.to_be_bytes());
    let fs = if fscod == 3 { 3 } else { fscod };
    out.push(((fs << 6) | (bsid << 1)) as u8);
    out.push(((acmod << 1) | lfeon) as u8);
    out.push(0);
    Some((out, sr, samples))
}

/// Samples in an Opus packet (48 kHz), from its TOC byte.
pub fn opus_samples(p: &[u8]) -> u32 {
    let Some(&toc) = p.first() else { return 960 };
    let config = toc >> 3;
    let frame = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => p.get(1).map_or(1, |b| (b & 0x3f) as u32).max(1),
    };
    frame * count
}

fn sample_entry(t: &OutTrack) -> Vec<u8> {
    let mut e = vec![0u8; 6];
    e.extend_from_slice(&1u16.to_be_bytes());
    if t.is_video() {
        e.extend_from_slice(&[0u8; 16]);
        e.extend_from_slice(&(t.width as u16).to_be_bytes());
        e.extend_from_slice(&(t.height as u16).to_be_bytes());
        e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        e.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        e.extend_from_slice(&0u32.to_be_bytes());
        e.extend_from_slice(&1u16.to_be_bytes());
        e.extend_from_slice(&[0u8; 32]);
        e.extend_from_slice(&0x0018u16.to_be_bytes());
        e.extend_from_slice(&0xFFFFu16.to_be_bytes());
        match &t.codec {
            Codec::Avc { avcc } => bx(b"avc1", &[e, bx(b"avcC", avcc)].concat()),
            Codec::Hevc { hvcc } => bx(b"hvc1", &[e, bx(b"hvcC", hvcc)].concat()),
            _ => unreachable!(),
        }
    } else {
        e.extend_from_slice(&[0u8; 8]);
        e.extend_from_slice(&(t.channels as u16).to_be_bytes());
        e.extend_from_slice(&16u16.to_be_bytes());
        e.extend_from_slice(&[0u8; 4]);
        let rate = if t.sample_rate > 0xFFFF { 0 } else { t.sample_rate << 16 };
        e.extend_from_slice(&rate.to_be_bytes());
        match &t.codec {
            Codec::Aac { .. } | Codec::Mp3 { .. } => {
                // ES_Descriptor(3) > DecoderConfigDescriptor(4) > DecoderSpecificInfo(5) + SLConfig(6);
                // MP3 (object type 0x6B) has no decoder-specific info.
                let (oti, dsi) = match &t.codec {
                    Codec::Aac { asc } => (0x40, [vec![5, asc.len() as u8], asc.clone()].concat()),
                    _ => (0x6B, Vec::new()),
                };
                let mut dcd = vec![oti, 0x15, 0, 0, 0];
                dcd.extend_from_slice(&0u32.to_be_bytes());
                dcd.extend_from_slice(&0u32.to_be_bytes());
                dcd.extend_from_slice(&dsi);
                let dcd = [vec![4, dcd.len() as u8], dcd].concat();
                let mut es = vec![0, 1, 0];
                es.extend_from_slice(&dcd);
                es.extend_from_slice(&[6, 1, 2]);
                let es = [vec![3, es.len() as u8], es].concat();
                bx(b"mp4a", &[e, full(b"esds", 0, 0, &es)].concat())
            }
            Codec::Ac3 { dac3 } => bx(b"ac-3", &[e, bx(b"dac3", dac3)].concat()),
            Codec::Eac3 { dec3, .. } => bx(b"ec-3", &[e, bx(b"dec3", dec3)].concat()),
            Codec::Opus { dops, .. } => bx(b"Opus", &[e, bx(b"dOps", dops)].concat()),
            _ => unreachable!(),
        }
    }
}

/// `ftyp` + `moov` describing `tracks`.
pub fn init_segment(tracks: &[OutTrack]) -> Vec<u8> {
    let ftyp = bx(b"ftyp", &[b"iso6".as_slice(), &0u32.to_be_bytes(), b"iso6", b"mp41", b"cmfc"].concat());
    let mut mvhd = Vec::new();
    mvhd.extend_from_slice(&[0u8; 8]);
    mvhd.extend_from_slice(&1000u32.to_be_bytes());
    mvhd.extend_from_slice(&0u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0u8; 10]);
    for m in MATRIX {
        mvhd.extend_from_slice(&m.to_be_bytes());
    }
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend_from_slice(&(tracks.len() as u32 + 1).to_be_bytes());
    let mut moov = full(b"mvhd", 0, 0, &mvhd);
    let mut mvex = Vec::new();
    for t in tracks {
        let mut tkhd = Vec::new();
        tkhd.extend_from_slice(&[0u8; 8]);
        tkhd.extend_from_slice(&t.id.to_be_bytes());
        tkhd.extend_from_slice(&[0u8; 4]);
        tkhd.extend_from_slice(&0u32.to_be_bytes());
        tkhd.extend_from_slice(&[0u8; 8]);
        // layer, alternate group, volume, reserved
        tkhd.extend_from_slice(&[0u8; 4]);
        tkhd.extend_from_slice(&(if t.is_video() { 0u16 } else { 0x0100 }).to_be_bytes());
        tkhd.extend_from_slice(&[0u8; 2]);
        for m in MATRIX {
            tkhd.extend_from_slice(&m.to_be_bytes());
        }
        tkhd.extend_from_slice(&(t.width << 16).to_be_bytes());
        tkhd.extend_from_slice(&(t.height << 16).to_be_bytes());
        let tkhd = full(b"tkhd", 0, 3, &tkhd);
        let mut mdhd = Vec::new();
        mdhd.extend_from_slice(&[0u8; 8]);
        mdhd.extend_from_slice(&t.timescale.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&0x55C4u16.to_be_bytes());
        mdhd.extend_from_slice(&0u16.to_be_bytes());
        let mdhd = full(b"mdhd", 0, 0, &mdhd);
        let (handler, name): (&[u8; 4], &[u8]) =
            if t.is_video() { (b"vide", b"Video\0") } else { (b"soun", b"Audio\0") };
        let hdlr = full(b"hdlr", 0, 0, &[&[0u8; 4][..], handler, &[0u8; 12], name].concat());
        let media_header = if t.is_video() { full(b"vmhd", 0, 1, &[0u8; 8]) } else { full(b"smhd", 0, 0, &[0u8; 4]) };
        let dinf = bx(b"dinf", &full(b"dref", 0, 0, &[&1u32.to_be_bytes()[..], &full(b"url ", 0, 1, &[])].concat()));
        let stsd = full(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &sample_entry(t)].concat());
        let empty = |k: &[u8; 4]| full(k, 0, 0, &0u32.to_be_bytes());
        let stsz = full(b"stsz", 0, 0, &[0u8; 8]);
        let stbl = bx(b"stbl", &[stsd, empty(b"stts"), empty(b"stsc"), stsz, empty(b"stco")].concat());
        let minf = bx(b"minf", &[media_header, dinf, stbl].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
        moov.extend(bx(b"trak", &[tkhd, mdia].concat()));
        let mut trex = Vec::new();
        trex.extend_from_slice(&t.id.to_be_bytes());
        trex.extend_from_slice(&1u32.to_be_bytes());
        trex.extend_from_slice(&[0u8; 12]);
        mvex.extend(full(b"trex", 0, 0, &trex));
    }
    moov.extend(bx(b"mvex", &mvex));
    [ftyp, bx(b"moov", &moov)].concat()
}

struct Sample {
    duration: u32,
    size: u32,
    key: bool,
    cto: i32,
}

/// One track's part of a segment.
struct Traf<'a> {
    id: u32,
    base: u64,
    samples: Vec<Sample>,
    frames: Vec<&'a Frame>,
    video: bool,
}

fn clamp_u32(v: i64) -> u32 {
    v.clamp(0, u32::MAX as i64) as u32
}

/// One `moof` + `mdat` with every frame of `frames` that belongs to `tracks`. Timestamps move
/// by `shift_ns` (so none is negative); the segment ends at `end_ns` (before the shift), where
/// its last video frame ends.
pub fn media_segment(seq: u32, tracks: &[OutTrack], frames: &[Frame], shift_ns: i64, end_ns: i64) -> Vec<u8> {
    let mut trafs: Vec<Traf> = Vec::new();
    for t in tracks {
        let mine: Vec<&Frame> = frames.iter().filter(|f| f.track == t.source).collect();
        if mine.is_empty() {
            continue;
        }
        let to_ts = |ns: i64| -> i64 {
            let v = ns.saturating_add(shift_ns) as i128 * t.timescale as i128 / 1_000_000_000;
            v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
        };
        let mut samples = Vec::with_capacity(mine.len());
        let base;
        if t.is_video() {
            // Decode times are the presentation times in order (signed composition offsets).
            let pts: Vec<i64> = mine.iter().map(|f| to_ts(f.pts_ns)).collect();
            let mut dts = pts.clone();
            dts.sort_unstable();
            base = dts[0];
            let end = to_ts(end_ns);
            for (i, f) in mine.iter().enumerate() {
                let duration = match dts.get(i + 1) {
                    Some(n) => n.saturating_sub(dts[i]),
                    // The last frame lasts until the next segment starts.
                    None if end > dts[i] => end - dts[i],
                    None => t.frame_duration as i64,
                };
                samples.push(Sample {
                    duration: clamp_u32(duration).max(1),
                    size: f.data.len() as u32,
                    key: f.key,
                    cto: pts[i].saturating_sub(dts[i]).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                });
            }
        } else {
            base = to_ts(mine[0].pts_ns);
            // Frames run back to back by their codec length; a gap in the source (a frame's own
            // timestamp more than half a frame past the running time) stretches the sample
            // before it. Matroska's millisecond timestamps jitter by far less than that.
            let mut running = base;
            let mut last_pts = i64::MIN;
            for f in &mine {
                let pts = to_ts(f.pts_ns);
                if pts != last_pts
                    && let Some(prev) = samples.last_mut()
                {
                    let gap = pts.saturating_sub(running);
                    if gap > prev.duration as i64 / 2 {
                        prev.duration = prev.duration.saturating_add(clamp_u32(gap));
                        running = running.saturating_add(gap);
                    }
                }
                last_pts = pts;
                let duration = match &t.codec {
                    Codec::Aac { .. } => 1024,
                    Codec::Ac3 { .. } => 1536,
                    Codec::Eac3 { samples, .. } => *samples,
                    Codec::Opus { .. } => opus_samples(&f.data),
                    Codec::Mp3 { samples } => *samples,
                    _ => 1024,
                };
                samples.push(Sample { duration, size: f.data.len() as u32, key: true, cto: 0 });
                running = running.saturating_add(duration as i64);
            }
        }
        trafs.push(Traf { id: t.id, base: base.max(0) as u64, samples, frames: mine, video: t.is_video() });
    }
    // moof size first (it is fixed by the sample counts), then each trun's data offset.
    let traf_len = |n: usize, video: bool| 8 + 16 + 20 + 12 + 8 + n * if video { 16 } else { 8 };
    let moof_len = 8 + 16 + trafs.iter().map(|t| traf_len(t.samples.len(), t.video)).sum::<usize>();
    let mut moof = full(b"mfhd", 0, 0, &seq.to_be_bytes());
    let mut offset = moof_len + 8;
    for Traf { id, base, samples, video, .. } in &trafs {
        let tfhd = full(b"tfhd", 0, 0x02_0000, &id.to_be_bytes());
        let tfdt = full(b"tfdt", 1, 0, &base.to_be_bytes());
        let flags = 0x1 | 0x100 | 0x200 | if *video { 0x400 | 0x800 } else { 0 };
        let mut trun = Vec::new();
        trun.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        trun.extend_from_slice(&(offset as u32).to_be_bytes());
        for s in samples {
            trun.extend_from_slice(&s.duration.to_be_bytes());
            trun.extend_from_slice(&s.size.to_be_bytes());
            if *video {
                let f: u32 = if s.key { 0x0200_0000 } else { 0x0101_0000 };
                trun.extend_from_slice(&f.to_be_bytes());
                trun.extend_from_slice(&s.cto.to_be_bytes());
            }
        }
        let trun = full(b"trun", 1, flags, &trun);
        moof.extend(bx(b"traf", &[tfhd, tfdt, trun].concat()));
        offset += samples.iter().map(|s| s.size as usize).sum::<usize>();
    }
    let moof = bx(b"moof", &moof);
    debug_assert_eq!(moof.len(), moof_len);
    let mdat_len: usize = trafs.iter().flat_map(|t| &t.samples).map(|s| s.size as usize).sum();
    let mut out = Vec::with_capacity(moof.len() + 8 + mdat_len);
    out.extend(moof);
    out.extend_from_slice(&(8 + mdat_len as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    for f in trafs.iter().flat_map(|t| &t.frames) {
        out.extend_from_slice(&f.data);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Box (kind, body) pairs at one level.
    pub(crate) fn boxes(mut b: &[u8]) -> Vec<(String, &[u8])> {
        let mut v = Vec::new();
        while b.len() >= 8 {
            let size = u32::from_be_bytes(b[..4].try_into().unwrap()) as usize;
            assert!(size >= 8 && size <= b.len(), "box size {size} of {}", b.len());
            v.push((String::from_utf8_lossy(&b[4..8]).into_owned(), &b[8..size]));
            b = &b[size..];
        }
        assert!(b.is_empty());
        v
    }

    #[test]
    fn ac3_and_eac3_headers() {
        // AC-3: 48 kHz, frmsizecod 0x14 (192 kbps), bsid 8, bsmod 0, acmod 7 (3/2), lfeon 1.
        let mut f = vec![0x0B, 0x77, 0, 0];
        // fscod 00 frmsizecod 010100 | bsid 01000 bsmod 000 | acmod 111 cmix 00 surmix 00 lfe 1
        f.extend_from_slice(&[0b0001_0100, 0b0100_0000, 0b1110_0001, 0]);
        let (dac3, sr) = ac3_info(&f).unwrap();
        assert_eq!(sr, 48000);
        let v = u32::from_be_bytes([0, dac3[0], dac3[1], dac3[2]]);
        assert_eq!((v >> 17) & 0x1f, 8, "bsid");
        assert_eq!((v >> 11) & 7, 7, "acmod");
        assert_eq!((v >> 10) & 1, 1, "lfe");
        assert_eq!((v >> 5) & 0x1f, 10, "bit rate code");
        // E-AC-3: strmtyp 0, frmsiz 383 (768 bytes), 48 kHz, 6 blocks, acmod 7, lfe, bsid 16.
        let mut g = vec![0x0B, 0x77];
        // strmtyp 00 substreamid 000 frmsiz 00101111111 | fscod 00 numblkscod 11 acmod 111 lfeon 1 | bsid 10000 ...
        g.extend_from_slice(&[0b0000_0001, 0b0111_1111, 0b0011_1111, 0b1000_0000, 0, 0]);
        let (dec3, sr, samples) = eac3_info(&g).unwrap();
        assert_eq!((sr, samples), (48000, 1536));
        assert_eq!(u16::from_be_bytes([dec3[0], dec3[1]]) >> 3, 192, "data rate kbps");
        assert!(ac3_info(b"nope").is_none() && eac3_info(&f).is_none());
    }

    #[test]
    fn opus_frame_lengths() {
        assert_eq!(opus_samples(&[19 << 3]), 960); // CELT 20 ms, one frame
        assert_eq!(opus_samples(&[(1 << 3) | 1]), 1920); // SILK 20 ms × 2
        assert_eq!(opus_samples(&[(16 << 3) | 3, 3]), 360); // CELT 2.5 ms × 3
    }

    #[test]
    fn segments_are_well_formed() {
        let video = OutTrack {
            source: 1,
            id: 1,
            codec: Codec::Avc { avcc: vec![1, 0x64, 0, 0x1f, 0xff, 0xe1] },
            timescale: VIDEO_TIMESCALE,
            width: 320,
            height: 240,
            channels: 0,
            sample_rate: 0,
            frame_duration: 3600,
        };
        let audio = OutTrack {
            source: 2,
            id: 2,
            codec: Codec::Aac { asc: vec![0x11, 0x90] },
            timescale: 48000,
            width: 0,
            height: 0,
            channels: 2,
            sample_rate: 48000,
            frame_duration: 0,
        };
        assert_eq!(video.codec_string(), "avc1.64001f");
        assert_eq!(audio.codec_string(), "mp4a.40.2");
        let init = init_segment(&[video.clone(), audio.clone()]);
        let top = boxes(&init);
        assert_eq!(top.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), ["ftyp", "moov"]);
        let moov = boxes(top[1].1);
        assert_eq!(moov.iter().filter(|b| b.0 == "trak").count(), 2);
        let ms = |n| n * 1_000_000i64;
        let frame = |track, pts_ns, key, data: Vec<u8>| Frame { track, pts_ns, key, data: data.into() };
        let frames = vec![
            frame(1, ms(0), true, vec![1; 10]),
            frame(2, ms(0), true, vec![2; 5]),
            frame(1, ms(80), false, vec![3; 7]),
            frame(1, ms(40), false, vec![4; 6]),
        ];
        let seg = media_segment(1, &[video.clone(), audio.clone()], &frames, 0, 0);
        let top = boxes(&seg);
        assert_eq!(top.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), ["moof", "mdat"]);
        assert_eq!(top[1].1.len(), 28);
        // The video trun: offsets point at its data, composition offsets 0, +40 ms, -40 ms.
        let moof = boxes(top[0].1);
        let traf = boxes(moof[1].1);
        let trun = traf[2].1;
        let count = u32::from_be_bytes(trun[4..8].try_into().unwrap());
        let offset = u32::from_be_bytes(trun[8..12].try_into().unwrap()) as usize;
        assert_eq!(count, 3);
        assert_eq!(&seg[offset..offset + 10], &[1; 10]);
        let cto = |i: usize| i32::from_be_bytes(trun[12 + i * 16 + 12..12 + i * 16 + 16].try_into().unwrap());
        assert_eq!((cto(0), cto(1), cto(2)), (0, 3600, -3600));
        // An audio gap in the source stretches the sample before it (to the next timestamp).
        let trun_of = |seg: &[u8], traf: usize| -> Vec<u8> { boxes(boxes(boxes(seg)[0].1)[1 + traf].1)[2].1.to_vec() };
        let gap = vec![
            frame(2, ms(0), true, vec![1]),
            frame(2, ms(1021), true, vec![2]),
            frame(2, ms(1043), true, vec![3]),
            // One AAC frame (21.3 ms) missing: also filled.
            frame(2, ms(1086), true, vec![4]),
        ];
        let g = media_segment(2, std::slice::from_ref(&audio), &gap, 0, 0);
        let gtrun = trun_of(&g, 0);
        let dur = |i: usize| u32::from_be_bytes(gtrun[12 + i * 8..16 + i * 8].try_into().unwrap());
        // The third frame runs on to 1086 ms: 1024 samples plus the 1072-sample gap.
        assert_eq!((dur(0), dur(1), dur(2)), (1021 * 48, 1024, 2096), "{gtrun:?}");
        // Audio data follows the video's.
        let atrun = boxes(moof[2].1)[2].1;
        let aoff = u32::from_be_bytes(atrun[8..12].try_into().unwrap()) as usize;
        assert_eq!(&seg[aoff..aoff + 5], &[2; 5]);
        // A negative first timestamp moves everything by the shift; the last video frame lasts
        // until the segment's end.
        let neg = vec![frame(1, ms(-40), true, vec![1]), frame(1, ms(0), false, vec![2])];
        let n = media_segment(3, std::slice::from_ref(&video), &neg, ms(40), ms(1000));
        let ntraf = boxes(boxes(boxes(&n)[0].1)[1].1);
        assert_eq!(u64::from_be_bytes(ntraf[1].1[4..12].try_into().unwrap()), 0, "tfdt");
        let ntrun = ntraf[2].1;
        let vdur = |i: usize| u32::from_be_bytes(ntrun[12 + i * 16..16 + i * 16].try_into().unwrap());
        assert_eq!((vdur(0), vdur(1)), (3600, 90_000));
    }
}
