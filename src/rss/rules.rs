//! Download rules: which feed articles to add automatically.
//!
//! Matching follows the qBittorrent conventions people know: without regex, words separated by
//! spaces must all appear (case-insensitive, `*`/`?` wildcards) and `|` separates alternatives.
//! The episode filter (`1x1-5;8;10-`) limits a show to a season and episode ranges.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RssRule {
    pub name: String,
    pub enabled: bool,
    /// Feeds this rule applies to (URLs); empty = all feeds.
    pub feeds: Vec<String>,
    pub must_contain: String,
    pub must_not_contain: String,
    pub use_regex: bool,
    /// e.g. `1x1-5;8;10-` (season 1, episodes 1–5, 8 and 10 onwards). Empty = any.
    pub episode_filter: String,
    /// Download each episode only once, even if several releases match.
    pub smart_episodes: bool,
    pub category: Option<String>,
    pub save_path: Option<std::path::PathBuf>,
    pub start_paused: bool,
}

impl Default for RssRule {
    fn default() -> Self {
        RssRule {
            name: String::new(),
            enabled: true,
            feeds: Vec::new(),
            must_contain: String::new(),
            must_not_contain: String::new(),
            use_regex: false,
            episode_filter: String::new(),
            smart_episodes: true,
            category: None,
            save_path: None,
            start_paused: false,
        }
    }
}

/// All of these must match (the words of one `|` alternative, or a single regex).
type Alternative = Vec<Regex>;

/// A rule compiled for matching.
pub struct Matcher {
    contain: Option<Vec<Alternative>>,
    exclude: Option<Vec<Alternative>>,
    episodes: Option<EpisodeFilter>,
}

impl Matcher {
    pub fn new(rule: &RssRule) -> Result<Matcher, String> {
        let contain = compile(&rule.must_contain, rule.use_regex).map_err(|e| format!("“Must contain”: {e}"))?;
        let exclude =
            compile(&rule.must_not_contain, rule.use_regex).map_err(|e| format!("“Must not contain”: {e}"))?;
        let episodes = if rule.episode_filter.trim().is_empty() {
            None
        } else {
            Some(EpisodeFilter::parse(&rule.episode_filter).map_err(|e| format!("Episode filter: {e}"))?)
        };
        Ok(Matcher { contain, exclude, episodes })
    }

    pub fn matches(&self, title: &str) -> bool {
        if let Some(c) = &self.contain
            && !c.iter().any(|alt| alt.iter().all(|r| r.is_match(title)))
        {
            return false;
        }
        if let Some(x) = &self.exclude
            && x.iter().any(|alt| alt.iter().all(|r| r.is_match(title)))
        {
            return false;
        }
        match &self.episodes {
            Some(f) => episode_of(title).is_some_and(|(s, e)| f.allows(s, e)),
            None => true,
        }
    }
}

/// Alternatives (`|`), each a list of words that must all appear (any order).
fn compile(text: &str, regex: bool) -> Result<Option<Vec<Alternative>>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let build = |pattern: &str| {
        RegexBuilder::new(pattern).case_insensitive(true).size_limit(1 << 20).build().map_err(|e| e.to_string())
    };
    if regex {
        return Ok(Some(vec![vec![build(text)?]]));
    }
    let mut out = Vec::new();
    for alt in text.split('|').map(str::trim).filter(|a| !a.is_empty()) {
        out.push(alt.split_whitespace().map(|w| build(&wildcard_to_regex(w))).collect::<Result<Vec<_>, _>>()?);
    }
    Ok(if out.is_empty() { None } else { Some(out) })
}

fn wildcard_to_regex(word: &str) -> String {
    let mut s = String::new();
    for c in word.chars() {
        match c {
            '*' => s.push_str(".*"),
            '?' => s.push('.'),
            c => s.push_str(&regex::escape(&c.to_string())),
        }
    }
    s
}

/// Season + episode ranges: `1x1-5;8;10-`.
#[derive(Debug, PartialEq)]
pub struct EpisodeFilter {
    season: u32,
    ranges: Vec<(u32, Option<u32>)>,
}

impl EpisodeFilter {
    pub fn parse(text: &str) -> Result<EpisodeFilter, String> {
        let t = text.trim().to_ascii_lowercase();
        let (season, rest) = t.split_once('x').ok_or("expected SEASONxEPISODES, e.g. 1x1-5;8")?;
        let season: u32 = season.trim().trim_start_matches('s').parse().map_err(|_| "invalid season number")?;
        let mut ranges = Vec::new();
        for part in rest.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let r = match part.split_once('-') {
                Some((a, b)) => {
                    let a: u32 = a.trim().parse().map_err(|_| format!("invalid range “{part}”"))?;
                    let b = b.trim();
                    let b = if b.is_empty() {
                        None
                    } else {
                        Some(b.parse().map_err(|_| format!("invalid range “{part}”"))?)
                    };
                    if b.is_some_and(|b| b < a) {
                        return Err(format!("range “{part}” ends before it starts"));
                    }
                    (a, b)
                }
                None => {
                    let n: u32 = part.parse().map_err(|_| format!("invalid episode “{part}”"))?;
                    (n, Some(n))
                }
            };
            ranges.push(r);
        }
        if ranges.is_empty() {
            return Err("no episodes given".into());
        }
        Ok(EpisodeFilter { season, ranges })
    }

    pub fn allows(&self, season: u32, episode: u32) -> bool {
        season == self.season && self.ranges.iter().any(|(a, b)| episode >= *a && b.is_none_or(|b| episode <= b))
    }
}

/// Season and episode from a release title (`S02E05`, `s2e5`, `2x05`).
pub fn episode_of(title: &str) -> Option<(u32, u32)> {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:\bs(\d{1,3})[ ._-]?e(\d{1,4})|\b(\d{1,2})x(\d{1,3})\b)").expect("valid regex")
    });
    let c = re.captures(title)?;
    let num = |i: usize| c.get(i).and_then(|m| m.as_str().parse().ok());
    num(1).zip(num(2)).or_else(|| num(3).zip(num(4)))
}

/// Key used by the smart episode filter.
pub fn episode_key(title: &str) -> Option<String> {
    episode_of(title).map(|(s, e)| format!("S{s:02}E{e:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(contain: &str, exclude: &str, regex: bool, episodes: &str) -> Matcher {
        Matcher::new(&RssRule {
            must_contain: contain.into(),
            must_not_contain: exclude.into(),
            use_regex: regex,
            episode_filter: episodes.into(),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn wildcard_words_any_order() {
        let m = rule("sintel 1080p", "", false, "");
        assert!(m.matches("Sintel.2010.1080p.mkv"));
        assert!(m.matches("1080p release of SINTEL"));
        assert!(!m.matches("Sintel 720p"));
        let m = rule("big*bunny | sintel", "cam", false, "");
        assert!(m.matches("Big Buck Bunny 4K"));
        assert!(m.matches("Sintel"));
        assert!(!m.matches("Sintel CAM"));
        assert!(rule("", "", false, "").matches("anything"));
        assert!(rule("a?c", "", false, "").matches("xabcx"));
        assert!(rule("(x)", "", false, "").matches("has (x) parens"), "wildcard mode escapes regex chars");
    }

    #[test]
    fn regex_mode() {
        let m = rule(r"^show\.s\d+e\d+", "", true, "");
        assert!(m.matches("Show.S01E02.720p"));
        assert!(!m.matches("Other.Show.S01E02"));
        assert!(Matcher::new(&RssRule { must_contain: "(".into(), use_regex: true, ..Default::default() }).is_err());
    }

    #[test]
    fn episodes() {
        assert_eq!(episode_of("Show.S02E05.1080p"), Some((2, 5)));
        assert_eq!(episode_of("show s2 e5"), Some((2, 5)));
        assert_eq!(episode_of("dress2e5"), None);
        assert_eq!(episode_of("Show 3x07 HDTV"), Some((3, 7)));
        assert_eq!(episode_of("Show.2010.1080p"), None);
        assert_eq!(episode_key("show.s1e2"), Some("S01E02".into()));
        let f = EpisodeFilter::parse("1x1-3;8;10-").unwrap();
        assert!(f.allows(1, 2) && f.allows(1, 8) && f.allows(1, 99));
        assert!(!f.allows(1, 5) && !f.allows(2, 2));
        assert!(EpisodeFilter::parse("1x5-2").is_err());
        assert!(EpisodeFilter::parse("15").is_err());
        let m = rule("show", "", false, "2x1-2");
        assert!(m.matches("Show.S02E01"));
        assert!(!m.matches("Show.S02E03"));
        assert!(!m.matches("Show special"), "titles without an episode don't pass an episode filter");
    }
}
