//! Subtitles for browsers: SubRip and SubStation Alpha (ASS/SSA) converted to WebVTT. Styling
//! is reduced to italics/bold/underline; positioning and effects are dropped.

/// `00:01:02,345` / `0:01:02.34` → milliseconds.
fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', ".");
    let mut parts = s.split(':').collect::<Vec<_>>();
    if parts.len() == 2 {
        parts.insert(0, "0");
    }
    if parts.len() != 3 {
        return None;
    }
    let h: u64 = parts[0].trim().parse().ok()?;
    let m: u64 = parts[1].trim().parse().ok()?;
    let (sec, frac) = parts[2].split_once('.').unwrap_or((parts[2], "0"));
    let sec: u64 = sec.trim().parse().ok()?;
    let frac = frac.trim();
    let ms = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 100,
        2 => frac.parse::<u64>().ok()? * 10,
        _ => frac.get(..3)?.parse::<u64>().ok()?,
    };
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

fn fmt_time(ms: u64) -> String {
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Escapes text for a cue, keeping the `<i>`, `<b>`, `<u>` tags SRT files use.
fn cue_text(s: &str) -> String {
    let mut out = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    for t in ["i", "b", "u"] {
        out = out
            .replace(&format!("&lt;{t}&gt;"), &format!("<{t}>"))
            .replace(&format!("&lt;/{t}&gt;"), &format!("</{t}>"));
    }
    // Cue text must not contain "-->".
    out.replace("--&gt;", "→")
}

pub fn srt_to_vtt(srt: &str) -> String {
    let text = srt.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::from("WEBVTT\n\n");
    for block in text.split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty()).peekable();
        // Optional numeric id.
        if lines.peek().is_some_and(|l| l.trim().chars().all(|c| c.is_ascii_digit())) {
            lines.next();
        }
        let Some(timing) = lines.next() else { continue };
        let Some((a, b)) = timing.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (parse_time(a), parse_time(b.split_whitespace().next().unwrap_or(""))) else {
            continue;
        };
        let body: Vec<String> = lines.map(cue_text).collect();
        if body.is_empty() {
            continue;
        }
        out.push_str(&format!("{} --> {}\n{}\n\n", fmt_time(start), fmt_time(end), body.join("\n")));
    }
    out
}

/// ASS/SSA dialogue lines → WebVTT (override blocks like `{\i1}` become tags or are dropped).
pub fn ass_to_vtt(ass: &str) -> String {
    let text = ass.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut format: Vec<String> = Vec::new();
    let mut in_events = false;
    let mut cues: Vec<(u64, u64, String)> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_events = l.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(f) = l.strip_prefix("Format:") {
            format = f.split(',').map(|x| x.trim().to_ascii_lowercase()).collect();
            continue;
        }
        let Some(d) = l.strip_prefix("Dialogue:") else { continue };
        let n = format.len().max(10);
        let fields: Vec<&str> = d.splitn(n, ',').collect();
        let idx = |name: &str, default: usize| format.iter().position(|f| f == name).unwrap_or(default);
        let (si, ei, ti) = (idx("start", 1), idx("end", 2), idx("text", n - 1));
        let (Some(s), Some(e), Some(t)) = (fields.get(si), fields.get(ei), fields.get(ti)) else { continue };
        let (Some(s), Some(e)) = (parse_time(s), parse_time(e)) else { continue };
        // Override blocks: keep italics/bold/underline, drop the rest.
        let mut body = String::new();
        let mut rest = *t;
        let mut open: Vec<char> = Vec::new();
        while let Some(i) = rest.find('{') {
            body.push_str(&cue_text(&rest[..i].replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ")));
            let Some(j) = rest[i..].find('}') else { break };
            let block = &rest[i + 1..i + j];
            for (tag, ch) in [("\\i1", 'i'), ("\\b1", 'b'), ("\\u1", 'u')] {
                if block.contains(tag) {
                    body.push_str(&format!("<{ch}>"));
                    open.push(ch);
                }
            }
            for (tag, ch) in [("\\i0", 'i'), ("\\b0", 'b'), ("\\u0", 'u')] {
                if block.contains(tag) && open.contains(&ch) {
                    body.push_str(&format!("</{ch}>"));
                    open.retain(|c| *c != ch);
                }
            }
            rest = &rest[i + j + 1..];
        }
        body.push_str(&cue_text(&rest.replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ")));
        for ch in open.iter().rev() {
            body.push_str(&format!("</{ch}>"));
        }
        if !body.trim().is_empty() {
            cues.push((s, e, body.trim().to_string()));
        }
    }
    cues.sort_by_key(|c| (c.0, c.1));
    let mut out = String::from("WEBVTT\n\n");
    for (s, e, b) in cues {
        out.push_str(&format!("{} --> {}\n{}\n\n", fmt_time(s), fmt_time(e), b));
    }
    out
}

/// Converts by file extension (`srt`, `ass`, `ssa`, `vtt`); `None` for image subtitles.
pub fn to_vtt(ext: &str, bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8(bytes.to_vec()).unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
    match ext.to_ascii_lowercase().as_str() {
        "srt" => Some(srt_to_vtt(&text)),
        "ass" | "ssa" => Some(ass_to_vtt(&text)),
        "vtt" => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt() {
        let v = srt_to_vtt(
            "\u{feff}1\r\n00:00:01,500 --> 00:00:03,000\r\n<i>Hello</i> & <script>\r\n\r\n2\r\n00:01:00,000 --> 00:01:02,250 X1:0\r\nTwo --> lines\r\nhere\r\n",
        );
        assert_eq!(
            v,
            "WEBVTT\n\n00:00:01.500 --> 00:00:03.000\n<i>Hello</i> &amp; &lt;script&gt;\n\n00:01:00.000 --> 00:01:02.250\nTwo → lines\nhere\n\n"
        );
    }

    #[test]
    fn ass() {
        let a = "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
                 Dialogue: 0,0:00:02.00,0:00:04.50,Default,,0,0,0,,{\\pos(10,10)}{\\i1}Hi{\\i0}, there\\Nline two\n\
                 Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,First\n";
        assert_eq!(
            ass_to_vtt(a),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nFirst\n\n00:00:02.000 --> 00:00:04.500\n<i>Hi</i>, there\nline two\n\n"
        );
    }

    #[test]
    fn garbage_is_safe() {
        for s in ["", "1\nnot a time\n", "00:00 --> zz\n", "[Events]\nDialogue: x"] {
            let _ = srt_to_vtt(s);
            let _ = ass_to_vtt(s);
        }
        assert!(to_vtt("sup", b"x").is_none());
    }
}
