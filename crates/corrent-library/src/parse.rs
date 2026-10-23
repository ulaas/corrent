//! Names → structure: what a file name and its folders say about the media (title, year,
//! season/episode, quality). Pure functions; the scanner combines them with tags and probes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Release-name facts that quality profiles and matching use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quality {
    /// 2160, 1080, 720, 576, 480.
    pub resolution: Option<u32>,
    /// bluray, remux, webdl, webrip, hdtv, dvd, …
    pub source: Option<String>,
    pub codec: Option<String>,
    pub hdr: bool,
    pub group: Option<String>,
    pub proper: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parsed {
    pub title: String,
    pub year: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// Last episode of a multi-episode file (`S01E01E02`, `S01E01-E03`).
    pub episode_end: Option<u32>,
    /// Daily shows (`2024-05-01`).
    pub date: Option<String>,
    /// Anime-style absolute number (`Show - 012`).
    pub absolute: Option<u32>,
    pub edition: Option<String>,
    /// Part of a split movie (`cd1`, `part2`).
    pub part: Option<u32>,
    pub quality: Quality,
}

impl Parsed {
    pub fn is_episode(&self) -> bool {
        self.episode.is_some() || self.date.is_some() || self.absolute.is_some()
    }
}

/// Words that end the title part of a release name.
const STOP_WORDS: &[&str] = &[
    "2160p",
    "1080p",
    "1080i",
    "720p",
    "576p",
    "480p",
    "4k",
    "uhd",
    "bluray",
    "blu-ray",
    "bdrip",
    "brrip",
    "remux",
    "web-dl",
    "webdl",
    "webrip",
    "web",
    "hdtv",
    "dvdrip",
    "dvd",
    "hdrip",
    "x264",
    "x265",
    "h264",
    "h265",
    "hevc",
    "avc",
    "av1",
    "xvid",
    "divx",
    "10bit",
    "hdr",
    "hdr10",
    "dv",
    "dovi",
    "proper",
    "repack",
    "extended",
    "unrated",
    "remastered",
    "directors",
    "imax",
    "multi",
    "dual",
    "aac",
    "ac3",
    "dts",
    "ddp5",
    "dd5",
    "atmos",
    "truehd",
    "flac",
    "complete",
    "internal",
    "limited",
    "criterion",
];

const EDITIONS: &[(&str, &str)] = &[
    ("directors cut", "Director's Cut"),
    ("director's cut", "Director's Cut"),
    ("extended", "Extended"),
    ("unrated", "Unrated"),
    ("remastered", "Remastered"),
    ("theatrical", "Theatrical"),
    ("imax", "IMAX"),
    ("criterion", "Criterion"),
    ("final cut", "Final Cut"),
];

fn is_year(w: &str) -> Option<u32> {
    let w = w.trim_matches(|c: char| c == '(' || c == ')' || c == '[' || c == ']');
    if w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()) {
        let y: u32 = w.parse().ok()?;
        (1900..=2100).contains(&y).then_some(y)
    } else {
        None
    }
}

/// `S01E02`, `S01E02E03`, `S01E02-E04`, `s1e2`, `1x02`, `S01 E02`.
fn season_episode(lower: &str) -> Option<(usize, u32, u32, Option<u32>)> {
    let b = lower.as_bytes();
    let digits = |i: usize| -> (u32, usize) {
        let mut j = i;
        while j < b.len() && b[j].is_ascii_digit() && j - i < 4 {
            j += 1;
        }
        (lower[i..j].parse().unwrap_or(0), j)
    };
    for i in 0..b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if !boundary {
            continue;
        }
        // SxxEyy
        if b[i] == b's' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            let (s, j) = digits(i + 1);
            let mut k = j;
            if k < b.len() && (b[k] == b' ' || b[k] == b'.') && k + 1 < b.len() && b[k + 1] == b'e' {
                k += 1;
            }
            if k < b.len() && b[k] == b'e' && k + 1 < b.len() && b[k + 1].is_ascii_digit() {
                let (e, mut m) = digits(k + 1);
                let mut end = None;
                loop {
                    let mut n = m;
                    if n < b.len() && b[n] == b'-' {
                        n += 1;
                    }
                    if n < b.len() && b[n] == b'e' && n + 1 < b.len() && b[n + 1].is_ascii_digit() {
                        let (e2, m2) = digits(n + 1);
                        end = Some(e2);
                        m = m2;
                    } else {
                        break;
                    }
                }
                return Some((i, s, e, end));
            }
        }
        // 1x02
        if b[i].is_ascii_digit() {
            let (s, j) = digits(i);
            if j < b.len() && b[j] == b'x' && j + 1 < b.len() && b[j + 1].is_ascii_digit() && s < 100 {
                let (e, m) = digits(j + 1);
                if m == b.len() || !b[m].is_ascii_alphanumeric() {
                    return Some((i, s, e, None));
                }
            }
        }
    }
    None
}

/// Season packs: a bare `S01` token or `Season 1`.
fn season_pack(lower: &str) -> Option<(usize, u32)> {
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in lower.char_indices() {
        let sep = !c.is_ascii_alphanumeric();
        match (sep, start) {
            (true, Some(s)) => {
                words.push((s, &lower[s..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push((s, &lower[s..]));
    }
    for (k, (at, w)) in words.iter().enumerate() {
        if *at == 0 {
            continue;
        }
        if let Some(n) = w.strip_prefix('s')
            && (1..=2).contains(&n.len())
            && n.chars().all(|c| c.is_ascii_digit())
        {
            return Some((*at, n.parse().ok()?));
        }
        if *w == "season"
            && let Some((_, n)) = words.get(k + 1)
            && (1..=2).contains(&n.len())
            && n.chars().all(|c| c.is_ascii_digit())
        {
            return Some((*at, n.parse().ok()?));
        }
    }
    None
}

/// `2024-05-01`, `2024.05.01`.
fn air_date(lower: &str) -> Option<(usize, String)> {
    let b = lower.as_bytes();
    for i in 0..b.len().saturating_sub(9) {
        if !lower.is_char_boundary(i) || !lower.is_char_boundary(i + 10) {
            continue;
        }
        let w = &lower[i..i + 10];
        let wb = w.as_bytes();
        let sep = wb[4];
        if (sep == b'-' || sep == b'.' || sep == b' ')
            && wb[7] == sep
            && wb[..4].iter().all(u8::is_ascii_digit)
            && wb[5..7].iter().all(u8::is_ascii_digit)
            && wb[8..10].iter().all(u8::is_ascii_digit)
            && (i == 0 || !b[i - 1].is_ascii_digit())
            && (i + 10 == b.len() || !b[i + 10].is_ascii_digit())
        {
            let (y, m, d): (u32, u32, u32) = (w[..4].parse().ok()?, w[5..7].parse().ok()?, w[8..10].parse().ok()?);
            if (1950..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d) {
                return Some((i, format!("{y:04}-{m:02}-{d:02}")));
            }
        }
    }
    None
}

fn clean_title(raw: &str) -> String {
    let mut t: String = raw
        .chars()
        .map(|c| if c == '.' || c == '_' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Leading bracketed release group: "[Group] Title".
    while t.starts_with('[') {
        match t.find(']') {
            Some(i) => t = t[i + 1..].trim().to_string(),
            None => break,
        }
    }
    t.trim_matches(|c: char| c == '-' || c == ' ' || c == '(' || c == '[').trim().to_string()
}

fn quality_of(lower: &str, stem: &str) -> Quality {
    // Both "web-dl" and the pieces of "x264-group".
    let mut words: Vec<&str> =
        lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    let pieces: Vec<&str> = words.iter().flat_map(|w| w.split('-')).filter(|w| !w.is_empty()).collect();
    words.extend(pieces);
    let has = |w: &str| words.contains(&w);
    let resolution = if has("2160p") || has("4k") || has("uhd") {
        Some(2160)
    } else if has("1080p") || has("1080i") {
        Some(1080)
    } else if has("720p") {
        Some(720)
    } else if has("576p") {
        Some(576)
    } else if has("480p") {
        Some(480)
    } else {
        None
    };
    let source = if has("remux") {
        Some("remux")
    } else if has("bluray") || has("blu-ray") || has("bdrip") || has("brrip") {
        Some("bluray")
    } else if has("web-dl") || has("webdl") {
        Some("webdl")
    } else if has("webrip") || has("web") {
        Some("webrip")
    } else if has("hdtv") {
        Some("hdtv")
    } else if has("dvdrip") || has("dvd") {
        Some("dvd")
    } else {
        None
    };
    let codec = if has("x265") || has("h265") || has("hevc") {
        Some("hevc")
    } else if has("x264") || has("h264") || has("avc") {
        Some("h264")
    } else if has("av1") {
        Some("av1")
    } else if has("xvid") || has("divx") {
        Some("xvid")
    } else {
        None
    };
    // "-GROUP" at the very end of the stem.
    let group = stem
        .rsplit_once('-')
        .map(|(_, g)| g.trim())
        .filter(|g| {
            !g.is_empty()
                && g.len() <= 20
                && !g.contains(' ')
                && !g.contains('.')
                && g.chars().any(|c| c.is_ascii_alphabetic())
        })
        .filter(|g| !STOP_WORDS.contains(&g.to_ascii_lowercase().as_str()))
        .map(str::to_string);
    Quality {
        resolution,
        source: source.map(str::to_string),
        codec: codec.map(str::to_string),
        hdr: has("hdr") || has("hdr10") || has("dv") || has("dovi"),
        group,
        proper: has("proper") || has("repack"),
    }
}

/// Parses a file or folder name (without extension).
pub fn parse_name(stem: &str) -> Parsed {
    let lower = stem.to_ascii_lowercase();
    let mut p = Parsed { quality: quality_of(&lower, stem), ..Default::default() };
    let mut cut = stem.len();

    if let Some((at, s, e, end)) = season_episode(&lower) {
        p.season = Some(s);
        p.episode = Some(e);
        p.episode_end = end.filter(|x| *x > e);
        cut = cut.min(at);
    } else if let Some((at, date)) = air_date(&lower) {
        p.date = Some(date);
        cut = cut.min(at);
    } else if let Some((at, season)) = season_pack(&lower) {
        p.season = Some(season);
        cut = cut.min(at);
    }

    // Year: the last plausible year before the first stop word (titles like "2001 A Space
    // Odyssey" keep their leading number).
    let words: Vec<(usize, &str)> = {
        let mut v = Vec::new();
        let mut start = None;
        for (i, c) in stem.char_indices() {
            let sep = c == '.' || c == ' ' || c == '_' || c == '[' || c == ']' || c == '(' || c == ')';
            match (sep, start) {
                (true, Some(s)) => {
                    v.push((s, &stem[s..i]));
                    start = None;
                }
                (false, None) => start = Some(i),
                _ => {}
            }
        }
        if let Some(s) = start {
            v.push((s, &stem[s..]));
        }
        v
    };
    let stop_at = words
        .iter()
        .find(|(i, w)| *i > 0 && STOP_WORDS.contains(&w.to_ascii_lowercase().as_str()))
        .map(|(i, _)| *i)
        .unwrap_or(stem.len());
    cut = cut.min(stop_at);
    if let Some((i, y)) = words
        .iter()
        .filter(|(i, _)| *i > 0 && *i < cut.max(1))
        .filter_map(|(i, w)| is_year(w).map(|y| (*i, y)))
        .next_back()
    {
        p.year = Some(y);
        cut = cut.min(i);
    }
    let lower_cut = &lower[..cut.min(lower.len())];
    for (needle, name) in EDITIONS {
        if lower.contains(needle) {
            p.edition = Some((*name).to_string());
            break;
        }
    }
    // Parts: "cd1", "part2", "pt.3", "disc 2".
    for w in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        for pre in ["cd", "part", "pt", "disc", "disk"] {
            if let Some(n) = w.strip_prefix(pre)
                && !n.is_empty()
                && n.len() <= 2
                && let Ok(n) = n.parse::<u32>()
            {
                p.part = Some(n);
            }
        }
    }
    let mut title = clean_title(&stem[..lower_cut.len()]);
    // Anime: "[Group] Show - 012 [1080p]".
    if !p.is_episode()
        && let Some((head, tail)) = title.rsplit_once(" - ")
    {
        let num: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !num.is_empty() && num.len() <= 4 && tail.trim().len() == num.len() {
            p.absolute = num.parse().ok();
            title = head.trim().to_string();
        }
    }
    // A trailing part marker isn't part of the title.
    if p.part.is_some() {
        let lw = title.to_ascii_lowercase();
        for pre in [" cd", " part", " pt", " disc", " disk"] {
            if let Some(i) = lw.rfind(pre)
                && lw[i + pre.len()..].trim().chars().all(|c| c.is_ascii_digit())
            {
                title.truncate(i);
            }
        }
    }
    p.title = title.trim().trim_end_matches(['-', ',']).trim().to_string();
    p
}

/// `Season 2`, `S02`, `Staffel 3`, `Specials`, `Season 2024` (a daily show's year) → season number.
pub fn season_folder(name: &str) -> Option<u32> {
    let l = name.trim().to_lowercase();
    if l == "specials" || l == "extras specials" {
        return Some(0);
    }
    for pre in [
        "season",
        "series",
        "staffel",
        "saison",
        "temporada",
        "temporadas",
        "stagione",
        "seizoen",
        "sæson",
        "säsong",
        "sesong",
        "kausi",
        "sezon",
        "sezona",
        "sezóna",
        "évad",
        "сезон",
        "book",
        "s",
    ] {
        if let Some(rest) = l.strip_prefix(pre) {
            let n = rest.trim_start_matches([' ', '.', '_', '-']);
            if !n.is_empty() && n.len() <= 4 && n.chars().all(|c| c.is_ascii_digit()) {
                return n.parse().ok();
            }
        }
    }
    None
}

/// A title by its letters and digits only, without articles ("The Matrix" = "matrix").
pub fn norm_title(s: &str) -> String {
    sort_title(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The folder a movie file stands for: its own, or the one above a disc structure
/// (`Film (2005)/VIDEO_TS/VTS_01_1.VOB`, `Film/BDMV/STREAM/00000.m2ts`).
pub fn movie_dir(file: &Path) -> Option<&Path> {
    let mut d = file.parent()?;
    while let Some(n) = d.file_name().and_then(|n| n.to_str()) {
        if !matches!(
            n.to_ascii_lowercase().as_str(),
            "video_ts" | "bdmv" | "stream" | "playlist" | "clipinf" | "backup"
        ) {
            break;
        }
        d = d.parent()?;
    }
    Some(d)
}

/// For a file in a shows library: the show folder's name (the folder directly under the
/// library root, so `Show/Season 1/x.mkv` and `Show/x.mkv` both give `Show`).
pub fn show_folder<'a>(root: &Path, file: &'a Path) -> Option<&'a str> {
    let rel = file.strip_prefix(root).ok()?;
    let mut comps = rel.components();
    let first = comps.next()?;
    // A file directly in the root has no show folder.
    comps.next()?;
    first.as_os_str().to_str()
}

/// For a file in a shows library: the show's folder, relative to the root. The deepest folder
/// above the file that isn't a season folder (`Season 1`, `Specials`), a release folder that
/// names a season or episode (`Show.S01.1080p`), or an extras folder, so a grouping folder
/// (`Anime/Show A/Season 1/x.mkv`) is not taken for the show. `None` when every folder is one of
/// those (a season pack straight under the root) or the file is in the root.
pub fn show_dir(root: &Path, file: &Path) -> Option<PathBuf> {
    let rel = file.strip_prefix(root).ok()?;
    let dirs: Vec<&std::ffi::OsStr> = rel.parent()?.iter().collect();
    let idx = dirs.iter().enumerate().rposition(|(i, c)| {
        let Some(name) = c.to_str() else { return false };
        let p = parse_name(name);
        // Inside another folder, a plain number is a year or a season ("Show/2024"); at the top
        // it is the show ("24", "1883"). Likewise a name with a season in it is a release or
        // season folder inside a show's, but a season pack's own folder at the top.
        let inner = i > 0;
        season_folder(name).is_none()
            && !is_extras_folder(name)
            && !p.title.is_empty()
            && !(inner && (p.season.is_some() || p.episode.is_some()))
            && !(inner && !name.chars().any(char::is_alphabetic))
    })?;
    Some(dirs[..=idx].iter().collect())
}

/// Where a show's own files are (`tvshow.nfo`, `poster.jpg`): its folder, else the folder
/// directly under the root (a season pack's folder).
pub fn show_home(root: &Path, file: &Path) -> Option<PathBuf> {
    show_dir(root, file).or_else(|| show_folder(root, file).map(PathBuf::from))
}

pub fn is_extras_folder(name: &str) -> bool {
    let l = name.trim().to_ascii_lowercase();
    EXTRAS_FOLDERS.contains(&l.as_str())
        || matches!(l.as_str(), "sample" | "samples" | "trailers" | "subs" | "subtitles")
}

const EXTRAS_FOLDERS: [&str; 8] =
    ["extras", "featurettes", "behind the scenes", "deleted scenes", "interviews", "scenes", "shorts", "other"];

/// A file name that says nothing about the movie (`movie.mkv`, `VTS_01_1`, `title_t00`): the
/// folder's name is used instead.
pub fn is_generic_name(stem: &str) -> bool {
    let l = stem.trim().to_ascii_lowercase();
    let p = parse_name(stem);
    let alnum: String = p.title.chars().filter(|c| c.is_alphanumeric()).collect();
    // One character is no title ("Up" and "It" are); five digits are a disc's stream number.
    alnum.chars().count() < 2
        || alnum.len() >= 5 && alnum.chars().all(|c| c.is_ascii_digit())
        || matches!(
            p.title.to_ascii_lowercase().as_str(),
            "movie" | "film" | "video" | "feature" | "main" | "title" | "video ts"
        )
        || l.starts_with("vts_")
        || l.starts_with("title_t")
}

/// `01 - Title`, `1-03 Title`, `03. Title`, `Artist - 05 - Title` → (disc, track, title).
pub fn track_name(stem: &str) -> (Option<u32>, Option<u32>, String) {
    let s = stem.trim();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        // "Artist - 05 - Title"
        let parts: Vec<&str> = s.split(" - ").collect();
        if parts.len() >= 3 && parts[1].trim().chars().all(|c| c.is_ascii_digit()) {
            return (None, parts[1].trim().parse().ok(), parts[2..].join(" - ").trim().to_string());
        }
        return (None, None, s.to_string());
    }
    let mut rest = &s[digits.len()..];
    let (mut disc, mut track) = (None, digits.parse().ok());
    // "1-03 Title": disc 1, track 3.
    if let Some(r) = rest.strip_prefix('-') {
        let t: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !t.is_empty() && digits.len() == 1 {
            disc = digits.parse().ok();
            track = t.parse().ok();
            rest = &r[t.len()..];
        }
    }
    let title = rest.trim_start_matches([' ', '.', '-', '_']).trim().to_string();
    if title.is_empty() {
        return (disc, track, s.to_string());
    }
    (disc, track, title)
}

/// The kind of file by extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Video,
    Audio,
    Image,
    Subtitle,
    Nfo,
    Other,
}

pub fn file_kind(path: &Path) -> FileKind {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "mkv" | "mp4" | "m4v" | "avi" | "mov" | "wmv" | "webm" | "ts" | "m2ts" | "mts" | "mpg" | "mpeg" | "flv"
        | "ogv" | "3gp" | "vob" | "divx" => FileKind::Video,
        "mp3" | "flac" | "m4a" | "m4b" | "aac" | "ogg" | "oga" | "opus" | "wav" | "wv" | "ape" | "alac" | "aiff"
        | "aif" | "dsf" | "dff" | "mka" | "wma" => FileKind::Audio,
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "avif" => FileKind::Image,
        "srt" | "ass" | "ssa" | "vtt" | "sub" | "idx" | "sup" => FileKind::Subtitle,
        "nfo" => FileKind::Nfo,
        _ => FileKind::Other,
    }
}

/// Samples, trailers and other extras that must not become their own movie.
pub fn is_extra(path: &Path) -> Option<&'static str> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let parent = path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let in_dir = |names: &[&str]| names.contains(&parent.as_str());
    if stem == "sample" || stem.ends_with("-sample") || stem.ends_with(".sample") || in_dir(&["sample", "samples"]) {
        return Some("sample");
    }
    if stem.ends_with("-trailer") || stem == "trailer" || in_dir(&["trailers"]) {
        return Some("trailer");
    }
    if in_dir(&EXTRAS_FOLDERS) {
        return Some("extra");
    }
    if stem == "theme" || stem.ends_with("-theme") {
        return Some("theme");
    }
    None
}

/// A sort key: lower case, leading English/Turkish articles dropped.
pub fn sort_title(title: &str) -> String {
    let l = title.trim().to_lowercase();
    for a in ["the ", "a ", "an "] {
        if let Some(r) = l.strip_prefix(a)
            && !r.is_empty()
        {
            return r.to_string();
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Parsed {
        parse_name(s)
    }

    #[test]
    fn movies() {
        let x = p("The.Matrix.1999.1080p.BluRay.x264-GROUP");
        assert_eq!((x.title.as_str(), x.year), ("The Matrix", Some(1999)));
        assert_eq!(x.quality.resolution, Some(1080));
        assert_eq!(x.quality.source.as_deref(), Some("bluray"));
        assert_eq!(x.quality.codec.as_deref(), Some("h264"));
        assert_eq!(x.quality.group.as_deref(), Some("GROUP"));
        let x = p("Blade Runner (1982) Final Cut");
        assert_eq!((x.title.as_str(), x.year, x.edition.as_deref()), ("Blade Runner", Some(1982), Some("Final Cut")));
        let x = p("2001 A Space Odyssey (1968)");
        assert_eq!((x.title.as_str(), x.year), ("2001 A Space Odyssey", Some(1968)));
        let x = p("Sintel");
        assert_eq!((x.title.as_str(), x.year), ("Sintel", None));
        let x = p("Big.Buck.Bunny.2008.2160p.WEB-DL.HEVC.HDR");
        assert_eq!(
            (x.title.as_str(), x.year, x.quality.resolution, x.quality.hdr),
            ("Big Buck Bunny", Some(2008), Some(2160), true)
        );
        let x = p("Kill Bill Vol 1 (2003) cd2");
        assert_eq!((x.title.as_str(), x.part), ("Kill Bill Vol 1", Some(2)));
        let x = p("Blade Runner 2049 2017");
        assert_eq!((x.title.as_str(), x.year), ("Blade Runner 2049", Some(2017)));
    }

    #[test]
    fn episodes() {
        let x = p("Show.Name.S01E02.720p.HDTV.x264-LOL");
        assert_eq!((x.title.as_str(), x.season, x.episode), ("Show Name", Some(1), Some(2)));
        let x = p("Show Name - 3x07 - Title");
        assert_eq!((x.season, x.episode), (Some(3), Some(7)));
        let x = p("show.s02e01e02");
        assert_eq!((x.season, x.episode, x.episode_end), (Some(2), Some(1), Some(2)));
        let x = p("Show S01E01-E03");
        assert_eq!(x.episode_end, Some(3));
        let x = p("Show S01 E05");
        assert_eq!((x.season, x.episode), (Some(1), Some(5)));
        let x = p("The Daily Show 2024.05.01 Guest");
        assert_eq!((x.title.as_str(), x.date.as_deref()), ("The Daily Show", Some("2024-05-01")));
        let x = p("[SubsPlease] Frieren - 012 (1080p) [ABCD1234]");
        assert_eq!((x.title.as_str(), x.absolute), ("Frieren", Some(12)));
        let x = p("Show.Name.S02.1080p.WEB-DL");
        assert_eq!((x.title.as_str(), x.season, x.episode), ("Show Name", Some(2), None));
        let x = p("Show Name Season 3 Complete");
        assert_eq!((x.title.as_str(), x.season), ("Show Name", Some(3)));
        let x = p("Doctor Who (2005) S01E01");
        assert_eq!((x.title.as_str(), x.year, x.episode), ("Doctor Who", Some(2005), Some(1)));
    }

    #[test]
    fn folders_tracks_and_kinds() {
        assert_eq!(season_folder("Season 02"), Some(2));
        assert_eq!(season_folder("S3"), Some(3));
        assert_eq!(season_folder("Specials"), Some(0));
        assert_eq!(season_folder("Seasoned"), None);
        assert_eq!(track_name("01 - Intro"), (None, Some(1), "Intro".into()));
        assert_eq!(track_name("1-03 Song"), (Some(1), Some(3), "Song".into()));
        assert_eq!(track_name("07. Title"), (None, Some(7), "Title".into()));
        assert_eq!(track_name("Artist - 05 - Title"), (None, Some(5), "Title".into()));
        assert_eq!(track_name("Just a Title"), (None, None, "Just a Title".into()));
        assert_eq!(file_kind(Path::new("a/b.MKV")), FileKind::Video);
        assert_eq!(file_kind(Path::new("a/b.flac")), FileKind::Audio);
        assert_eq!(file_kind(Path::new("a/b.en.srt")), FileKind::Subtitle);
        assert_eq!(is_extra(Path::new("M/Movie-sample.mkv")), Some("sample"));
        assert_eq!(is_extra(Path::new("M/Trailers/x.mkv")), Some("trailer"));
        assert_eq!(is_extra(Path::new("M/Movie.mkv")), None);
        assert_eq!(show_folder(Path::new("/tv"), Path::new("/tv/Show/Season 1/e.mkv")), Some("Show"));
        assert_eq!(show_folder(Path::new("/tv"), Path::new("/tv/e.mkv")), None);
        assert_eq!(sort_title("The Matrix"), "matrix");
    }

    #[test]
    fn never_panics_on_odd_names() {
        for s in [
            "",
            "-",
            "S",
            "s1",
            "s01e",
            "1x",
            "(((",
            "[]",
            "2024-13-40",
            "ü.Ş.ğ 2020",
            "a - ",
            "S99999E99999",
            "x".repeat(500).as_str(),
        ] {
            let _ = parse_name(s);
            let _ = track_name(s);
            let _ = season_folder(s);
        }
    }

    #[test]
    fn random_names_never_panic() {
        let alphabet: Vec<char> = "Ss0123456789xXeE.-_ ()[]üŞğé漢😀pP1080i2160".chars().collect();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let len = (x % 40) as usize;
            let s: String =
                (0..len).map(|i| alphabet[((x >> (i % 50)) as usize).wrapping_add(i * 7) % alphabet.len()]).collect();
            let _ = parse_name(&s);
            let _ = track_name(&s);
            let _ = season_folder(&s);
        }
    }
}
