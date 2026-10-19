//! Lyrics of a track: a sidecar `<track>.lrc` (or `.txt`) next to the file, else the file's own
//! lyrics tag (ID3 USLT, Vorbis LYRICS, MP4 ©lyr). LRC timestamps make them synced.

use std::path::Path;

use corrent_proto::library::{LyricLine, Lyrics};
use lofty::prelude::*;
use lofty::tag::ItemKey;

/// Larger files aren't lyrics.
const MAX: u64 = 512 * 1024;

/// `[mm:ss.xx]` (also `[mm:ss]`, `[mm:ss:xx]`, `[h:mm:ss.xx]`) → milliseconds.
fn stamp(s: &str) -> Option<i64> {
    // Short digit runs only (nothing can overflow).
    let num = |x: &str| -> Option<i64> {
        (!x.is_empty() && x.len() <= 4 && x.bytes().all(|b| b.is_ascii_digit())).then(|| x.parse().ok()).flatten()
    };
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [m, s] => (0, num(m)?, *s),
        // mm:ss:xx (centiseconds after a colon) or h:mm:ss.xx
        [a, b, c] if c.contains('.') => (num(a)?, num(b)?, *c),
        [m, s, cs] => {
            let (m, s, cs) = (num(m)?, num(s)?, num(cs)?);
            if m >= 600 || s >= 60 || cs >= 100 {
                return None;
            }
            return Some((m * 60 + s) * 1000 + cs * 10);
        }
        _ => return None,
    };
    let (whole, frac) = sec.split_once('.').unwrap_or((sec, ""));
    let whole = num(whole)?;
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let frac_ms: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 100,
        2 => frac.parse::<i64>().ok()? * 10,
        _ => frac.get(..3)?.parse().ok()?,
    };
    if m >= 600 || whole >= 60 {
        return None;
    }
    Some(((h * 60 + m) * 60 + whole) * 1000 + frac_ms)
}

/// Parses LRC (or plain text, which becomes unsynced lines).
pub fn parse(text: &str) -> Lyrics {
    let mut offset = 0i64;
    let mut timed: Vec<LyricLine> = Vec::new();
    let mut plain: Vec<LyricLine> = Vec::new();
    for raw in text.lines().take(5000) {
        let mut line = raw.trim_start_matches('\u{feff}').trim();
        let mut times = Vec::new();
        while let Some(rest) = line.strip_prefix('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[..end];
            if let Some(t) = stamp(tag) {
                times.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset = v.trim().parse().unwrap_or(0);
            } else if tag.contains(':') && times.is_empty() {
                // [ar:…], [ti:…] and other headers.
                line = "";
                break;
            }
            line = rest[end + 1..].trim_start();
        }
        let text: String = line.chars().take(500).collect();
        if times.is_empty() {
            if !raw.trim().starts_with('[') {
                plain.push(LyricLine { time_ms: None, text });
            }
        } else {
            for t in times {
                // A positive offset shows lines earlier.
                timed.push(LyricLine { time_ms: Some((t - offset).max(0)), text: text.clone() });
            }
        }
    }
    if timed.is_empty() {
        // Drop leading/trailing blank lines of plain lyrics.
        while plain.first().is_some_and(|l| l.text.is_empty()) {
            plain.remove(0);
        }
        while plain.last().is_some_and(|l| l.text.is_empty()) {
            plain.pop();
        }
        return Lyrics { synced: false, lines: plain };
    }
    timed.sort_by_key(|l| l.time_ms);
    Lyrics { synced: true, lines: timed }
}

fn read_small(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() || m.len() > MAX {
        return None;
    }
    let b = std::fs::read(p).ok()?;
    Some(String::from_utf8(b.clone()).unwrap_or_else(|_| b.iter().map(|&c| c as char).collect()))
}

/// The lyrics of an audio file (blocking), if it has any.
pub fn find(track: &Path) -> Option<Lyrics> {
    for ext in ["lrc", "LRC", "txt"] {
        if let Some(t) = read_small(&track.with_extension(ext)) {
            let l = parse(&t);
            if !l.lines.is_empty() {
                return Some(l);
            }
        }
    }
    let file = lofty::read_from_path(track).ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let text = tag.get_string(ItemKey::Lyrics).or_else(|| tag.get_string(ItemKey::UnsyncLyrics))?;
    let l = parse(text);
    (!l.lines.is_empty()).then_some(l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc() {
        let l = parse(
            "[ar:Someone]\n[ti:Song]\n[offset:+500]\n[00:12.00]First\n[00:17.20][01:02.5]Again\n\n[00:20.123]Third\n",
        );
        assert!(l.synced);
        let got: Vec<(i64, &str)> = l.lines.iter().map(|x| (x.time_ms.unwrap(), x.text.as_str())).collect();
        assert_eq!(got, vec![(11500, "First"), (16700, "Again"), (19623, "Third"), (62000, "Again")]);
        assert_eq!(stamp("01:02:50"), Some(62500));
        assert_eq!(stamp("1:00:00.00"), Some(3_600_000));
        assert_eq!(stamp("aa:bb"), None);
        assert_eq!(stamp("00:99"), None);
    }

    #[test]
    fn plain_text() {
        let l = parse("\nLine one\n\nLine two\n\n");
        assert!(!l.synced);
        assert_eq!(l.lines.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), vec!["Line one", "", "Line two"]);
    }

    #[test]
    fn sidecar_file() {
        let d = std::env::temp_dir().join(format!("corrent-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("01 Song.flac"), b"not audio").unwrap();
        std::fs::write(d.join("01 Song.lrc"), "[00:01.00]Hello").unwrap();
        let l = find(&d.join("01 Song.flac")).unwrap();
        assert_eq!(l.lines[0].text, "Hello");
        assert!(find(&d.join("missing.flac")).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn hostile_input_doesnt_panic() {
        let mut rng = corrent_fuzz::Rng::new(3);
        for _ in 0..3000 {
            let s = rng.mutate("[00:12.00]First\n[ar:x]\n[offset:-200]\n[1:2:3]x");
            let _ = parse(&s);
        }
    }
}
