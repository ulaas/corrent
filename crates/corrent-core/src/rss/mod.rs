//! RSS/Atom feeds and automatic downloads.
//!
//! The engine actor owns an [`Rss`]: it says which feeds are due, gets the fetched results back
//! and returns the articles that rules want downloaded. Downloaded article ids (and episodes for
//! the smart filter) are persisted in `rss.json`, so every item is added at most once, also
//! across restarts.

pub mod feed;
pub mod rules;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub use feed::{Article, ParsedFeed};
pub use rules::{Matcher, RssRule};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RssSettings {
    /// Apply download rules (feeds are still refreshed when off).
    pub auto_download: bool,
    pub refresh_minutes: u32,
    /// Articles kept per feed.
    pub max_articles: usize,
    pub feeds: Vec<FeedConfig>,
    pub rules: Vec<RssRule>,
}

impl Default for RssSettings {
    fn default() -> Self {
        RssSettings {
            auto_download: true,
            refresh_minutes: 30,
            max_articles: 200,
            feeds: Vec::new(),
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FeedConfig {
    pub url: String,
    /// Display name; empty = the feed's own title.
    pub name: String,
    pub enabled: bool,
}

impl Default for FeedConfig {
    fn default() -> Self {
        FeedConfig { url: String::new(), name: String::new(), enabled: true }
    }
}

/// A rule is only active with something to match: an empty rule would download whole feeds.
pub fn rule_is_active(r: &RssRule) -> bool {
    r.enabled && (!r.must_contain.trim().is_empty() || !r.episode_filter.trim().is_empty())
}

pub fn rule_applies_to(r: &RssRule, feed_url: &str) -> bool {
    r.feeds.is_empty() || r.feeds.iter().any(|f| f == feed_url)
}

pub(crate) const STATE_VERSION: u32 = 1;
const MAX_REMEMBERED: usize = 10_000;
const MIN_REFRESH: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
struct StateFile {
    schema_version: u32,
    feeds: BTreeMap<String, FeedCache>,
    /// Article ids already downloaded (oldest first).
    downloaded: VecDeque<String>,
    /// Rule name → episodes downloaded by it (smart episode filter).
    episodes: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct FeedCache {
    pub title: Option<String>,
    pub articles: Vec<Article>,
    pub last_ok: Option<i64>,
    pub error: Option<String>,
}

/// What the UI shows.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RssView {
    pub feeds: Vec<FeedView>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FeedView {
    pub url: String,
    pub title: String,
    pub articles: Vec<ArticleView>,
    pub last_ok: Option<i64>,
    pub error: Option<String>,
    pub loading: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ArticleView {
    pub article: Article,
    pub downloaded: bool,
}

/// An article a rule wants added.
#[derive(Clone, Debug)]
pub struct Download {
    pub link: String,
    pub title: String,
    pub rule: RssRule,
    /// Identifies the article in the downloaded set (see [`Rss::forget`]).
    pub key: String,
    pub episode: Option<String>,
}

/// Article ids are only unique within a feed (many trackers use plain numbers).
fn article_key(feed: &str, guid: &str) -> String {
    format!("{feed}\u{1f}{guid}")
}

pub struct Rss {
    path: PathBuf,
    data: StateFile,
    downloaded: HashSet<String>,
    next_due: HashMap<String, Instant>,
    in_flight: HashSet<String>,
    view: Arc<RssView>,
    /// The state file couldn't be read: don't overwrite it and don't auto-download (the record of
    /// what was already downloaded is unknown).
    read_only: bool,
    /// Shown to the user once.
    pub warning: Option<String>,
}

impl Rss {
    pub fn load(path: PathBuf) -> Rss {
        let parse = |b: &[u8]| serde_json::from_slice::<StateFile>(b).ok();
        let mut warning = None;
        let mut read_only = false;
        // A transient read failure (lock, antivirus) must not be mistaken for "no history".
        let mut read = std::fs::read(&path);
        for _ in 0..5 {
            match &read {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    std::thread::sleep(Duration::from_millis(100));
                    read = std::fs::read(&path);
                }
                _ => break,
            }
        }
        let data = match read {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StateFile::default(),
            Err(e) => {
                tracing::error!("rss.json can't be read ({e}); feeds are read-only this session");
                read_only = true;
                warning = Some(t!(
                    "Corrent couldn't read its feed history ({e}). Automatic feed downloads are paused until the \
                     next start, so nothing is downloaded twice.",
                    e = e
                ));
                StateFile::default()
            }
            Ok(b) => match parse(&b) {
                Some(d) => d,
                None => {
                    let bad = path.with_extension(format!("json.bad-{}", crate::util::now_unix()));
                    let _ = std::fs::copy(&path, &bad);
                    match std::fs::read(path.with_extension("json.bak")).ok().and_then(|b| parse(&b)) {
                        Some(d) => {
                            tracing::error!("rss.json is invalid; restored from its backup");
                            d
                        }
                        None => {
                            tracing::error!("rss.json is invalid and has no usable backup");
                            read_only = true;
                            warning = Some(t!(
                                "Corrent's feed history was damaged (kept as {path}). Automatic feed downloads are \
                                 paused for this session so nothing is downloaded twice.",
                                path = bad.display()
                            ));
                            StateFile::default()
                        }
                    }
                }
            },
        };
        let downloaded = data.downloaded.iter().cloned().collect();
        Rss {
            path,
            data,
            downloaded,
            next_due: HashMap::new(),
            in_flight: HashSet::new(),
            view: Default::default(),
            read_only,
            warning,
        }
    }

    fn is_downloaded(&self, feed: &str, guid: &str) -> bool {
        // Bare guids were written before ids were scoped per feed.
        self.downloaded.contains(&article_key(feed, guid)) || self.downloaded.contains(guid)
    }

    pub fn view(&self) -> Arc<RssView> {
        self.view.clone()
    }

    /// Feeds to fetch now; they count as in flight until [`Rss::fetched`].
    pub fn due(&mut self, s: &RssSettings) -> Vec<String> {
        let now = Instant::now();
        let mut out = Vec::new();
        for f in s.feeds.iter().filter(|f| f.enabled && !f.url.trim().is_empty()) {
            if self.in_flight.contains(&f.url) || self.next_due.get(&f.url).is_some_and(|t| *t > now) {
                continue;
            }
            self.in_flight.insert(f.url.clone());
            out.push(f.url.clone());
        }
        if !out.is_empty() {
            self.rebuild(s);
        }
        out
    }

    /// Fetch `url` (or all feeds) on the next tick.
    pub fn refresh_now(&mut self, url: Option<&str>) {
        match url {
            Some(u) => {
                self.next_due.remove(u);
            }
            None => self.next_due.clear(),
        }
    }

    pub fn fetched(&mut self, url: &str, result: Result<ParsedFeed, String>, s: &RssSettings) -> Vec<Download> {
        self.in_flight.remove(url);
        let every = Duration::from_secs(u64::from(s.refresh_minutes) * 60).max(MIN_REFRESH);
        self.next_due.insert(url.to_string(), Instant::now() + every);
        if !s.feeds.iter().any(|f| f.url == url) {
            return Vec::new(); // removed while loading
        }
        let cache = self.data.feeds.entry(url.to_string()).or_default();
        match result {
            Err(e) => {
                tracing::info!("feed {}: {e}", crate::util::redact_url(url));
                cache.error = Some(e);
            }
            Ok(parsed) => {
                cache.error = None;
                cache.last_ok = Some(crate::util::now_unix());
                if parsed.title.is_some() {
                    cache.title = parsed.title;
                }
                // Newest feed items first, then older cached ones the feed no longer lists.
                let fresh: HashSet<&str> = parsed.articles.iter().map(|a| a.guid.as_str()).collect();
                let older: Vec<Article> =
                    cache.articles.iter().filter(|a| !fresh.contains(a.guid.as_str())).cloned().collect();
                let mut merged = parsed.articles;
                merged.extend(older);
                merged.truncate(s.max_articles.max(10));
                cache.articles = merged;
            }
        }
        let downloads = if s.auto_download && !self.read_only { self.match_feed(url, s) } else { Vec::new() };
        self.save();
        self.rebuild(s);
        downloads
    }

    /// Re-run the rules over every cached article (after rules changed).
    pub fn apply_rules(&mut self, s: &RssSettings) -> Vec<Download> {
        if !s.auto_download || self.read_only {
            return Vec::new();
        }
        let urls: Vec<String> = self.data.feeds.keys().cloned().collect();
        let mut out = Vec::new();
        for u in urls {
            out.extend(self.match_feed(&u, s));
        }
        if !out.is_empty() {
            self.save();
            self.rebuild(s);
        }
        out
    }

    fn match_feed(&mut self, url: &str, s: &RssSettings) -> Vec<Download> {
        let rules: Vec<(&RssRule, Matcher)> = s
            .rules
            .iter()
            .filter(|r| rule_is_active(r) && rule_applies_to(r, url))
            .filter_map(|r| Matcher::new(r).ok().map(|m| (r, m)))
            .collect();
        if rules.is_empty() {
            return Vec::new();
        }
        let Some(cache) = self.data.feeds.get(url) else { return Vec::new() };
        let mut out = Vec::new();
        let mut marks: Vec<(String, String, Option<String>)> = Vec::new();
        // Oldest first, so the first release of an episode wins.
        for a in cache.articles.iter().rev() {
            let Some(link) = &a.torrent else { continue };
            let key = article_key(url, &a.guid);
            if self.is_downloaded(url, &a.guid) || marks.iter().any(|(k, _, _)| *k == key) {
                continue;
            }
            for (rule, m) in &rules {
                if !m.matches(&a.title) {
                    continue;
                }
                let episode = rule.smart_episodes.then(|| rules::episode_key(&a.title)).flatten();
                if let Some(ep) = &episode {
                    let seen = self.data.episodes.get(&rule.name).is_some_and(|v| v.contains(ep))
                        || marks.iter().any(|(_, r, e)| r == &rule.name && e.as_ref() == Some(ep));
                    if seen {
                        continue;
                    }
                }
                out.push(Download {
                    link: link.clone(),
                    title: a.title.clone(),
                    rule: (*rule).clone(),
                    key: key.clone(),
                    episode: episode.clone(),
                });
                marks.push((key, rule.name.clone(), episode));
                break;
            }
        }
        for (key, rule, episode) in marks {
            self.remember(key);
            if let Some(ep) = episode {
                self.data.episodes.entry(rule).or_default().push(ep);
            }
        }
        out
    }

    /// Mark an article as downloaded (e.g. added by hand), so rules skip it.
    pub fn mark_downloaded(&mut self, feed: &str, guid: &str, s: &RssSettings) {
        if !self.is_downloaded(feed, guid) {
            self.remember(article_key(feed, guid));
            self.save();
            self.rebuild(s);
        }
    }

    /// Adding a rule's download failed: let a later refresh try again.
    pub fn forget(&mut self, d: &Download, s: &RssSettings) {
        if self.downloaded.remove(&d.key) {
            self.data.downloaded.retain(|k| *k != d.key);
        }
        if let Some(ep) = &d.episode
            && let Some(v) = self.data.episodes.get_mut(&d.rule.name)
        {
            v.retain(|e| e != ep);
        }
        self.save();
        self.rebuild(s);
    }

    fn remember(&mut self, guid: String) {
        if self.downloaded.insert(guid.clone()) {
            self.data.downloaded.push_back(guid);
            while self.data.downloaded.len() > MAX_REMEMBERED {
                if let Some(old) = self.data.downloaded.pop_front() {
                    self.downloaded.remove(&old);
                }
            }
        }
    }

    /// Settings changed: forget removed feeds, refresh new ones.
    pub fn settings_changed(&mut self, s: &RssSettings) {
        let urls: HashSet<&str> = s.feeds.iter().map(|f| f.url.as_str()).collect();
        let before = self.data.feeds.len();
        self.data.feeds.retain(|u, _| urls.contains(u.as_str()));
        self.next_due.retain(|u, _| urls.contains(u.as_str()));
        let names: HashSet<&str> = s.rules.iter().map(|r| r.name.as_str()).collect();
        self.data.episodes.retain(|r, _| names.contains(r.as_str()));
        if self.data.feeds.len() != before {
            self.save();
        }
        self.rebuild(s);
    }

    pub fn rebuild(&mut self, s: &RssSettings) {
        let feeds = s
            .feeds
            .iter()
            .filter(|f| !f.url.trim().is_empty())
            .map(|f| {
                let cache = self.data.feeds.get(&f.url);
                let title = if !f.name.trim().is_empty() {
                    f.name.clone()
                } else {
                    cache.and_then(|c| c.title.clone()).unwrap_or_else(|| f.url.clone())
                };
                FeedView {
                    url: f.url.clone(),
                    title,
                    articles: cache
                        .map(|c| {
                            c.articles
                                .iter()
                                .map(|a| ArticleView {
                                    article: a.clone(),
                                    downloaded: self.is_downloaded(&f.url, &a.guid),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    last_ok: cache.and_then(|c| c.last_ok),
                    error: cache.and_then(|c| c.error.clone()),
                    loading: self.in_flight.contains(&f.url),
                    enabled: f.enabled,
                }
            })
            .collect();
        self.view = Arc::new(RssView { feeds });
    }

    fn save(&mut self) {
        if self.read_only {
            return;
        }
        self.data.schema_version = STATE_VERSION;
        match serde_json::to_vec(&self.data) {
            // On the writer thread (it logs failures); see `flush`.
            Ok(b) => crate::writer::global().submit(&self.path, b, None),
            Err(e) => tracing::error!("failed to encode rss state: {e}"),
        }
    }

    /// Wait until the state is on disk (shutdown).
    pub fn flush(&self) {
        if !crate::writer::global().flush(&self.path, std::time::Duration::from_secs(5)) {
            tracing::warn!("rss.json is still being saved");
        }
        let _ = crate::writer::global().take_result(&self.path);
    }
}

impl Drop for Rss {
    /// Also when the engine task unwinds after a panic.
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(guid: &str, title: &str) -> Article {
        Article {
            guid: guid.into(),
            title: title.into(),
            torrent: Some(format!("https://x/{guid}.torrent")),
            ..Default::default()
        }
    }

    fn settings(rules: Vec<RssRule>) -> RssSettings {
        RssSettings {
            feeds: vec![FeedConfig { url: "https://feed".into(), ..Default::default() }],
            rules,
            ..Default::default()
        }
    }

    fn parsed(items: Vec<Article>) -> Result<ParsedFeed, String> {
        Ok(ParsedFeed { title: Some("Feed".into()), articles: items })
    }

    #[test]
    fn matching_items_are_added_once_also_after_restart() {
        let path = std::env::temp_dir().join(format!("corrent-rss-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = settings(vec![RssRule { name: "Show".into(), must_contain: "show".into(), ..Default::default() }]);
        let mut rss = Rss::load(path.clone());
        assert_eq!(rss.due(&s), vec!["https://feed".to_string()]);
        assert!(rss.due(&s).is_empty(), "in flight");
        let items = vec![article("3", "Show S01E02 720p"), article("2", "Show S01E02 1080p"), article("1", "Other")];
        let d = rss.fetched("https://feed", parsed(items.clone()), &s);
        assert_eq!(d.len(), 1, "smart filter: one release per episode");
        assert_eq!(d[0].title, "Show S01E02 1080p", "the oldest release wins");
        assert!(rss.fetched("https://feed", parsed(items.clone()), &s).is_empty(), "nothing twice");
        drop(rss);
        let mut rss = Rss::load(path.clone());
        assert!(rss.fetched("https://feed", parsed(items), &s).is_empty(), "nothing twice after a restart");
        let v = rss.view();
        assert_eq!(v.feeds[0].articles.len(), 3);
        assert!(v.feeds[0].articles.iter().any(|a| a.downloaded));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn errors_keep_cached_articles_and_empty_rules_do_nothing() {
        let path = std::env::temp_dir().join(format!("corrent-rss-err-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = settings(vec![RssRule { name: "Everything".into(), ..Default::default() }]);
        let mut rss = Rss::load(path.clone());
        assert!(
            rss.fetched("https://feed", parsed(vec![article("1", "A")]), &s).is_empty(),
            "an empty rule is inactive"
        );
        rss.fetched("https://feed", Err("offline".into()), &s);
        let v = rss.view();
        assert_eq!(v.feeds[0].articles.len(), 1);
        assert_eq!(v.feeds[0].error.as_deref(), Some("offline"));
        // A new rule picks up cached articles.
        let s2 = settings(vec![RssRule { name: "A".into(), must_contain: "a".into(), ..Default::default() }]);
        assert_eq!(rss.apply_rules(&s2).len(), 1);
        // Removing the feed forgets its cache.
        rss.settings_changed(&RssSettings::default());
        assert!(rss.view().feeds.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ids_are_per_feed_failed_adds_retry_and_damage_is_read_only() {
        let path = std::env::temp_dir().join(format!("corrent-rss-keys-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut s = settings(vec![RssRule { name: "A".into(), must_contain: "a".into(), ..Default::default() }]);
        s.feeds.push(FeedConfig { url: "https://feed2".into(), ..Default::default() });
        let mut rss = Rss::load(path.clone());
        let d = rss.fetched("https://feed", parsed(vec![article("1", "A")]), &s);
        assert_eq!(d.len(), 1);
        // Same guid in another feed is a different article.
        assert_eq!(rss.fetched("https://feed2", parsed(vec![article("1", "A")]), &s).len(), 1);
        // A failed add is forgotten and matched again.
        rss.forget(&d[0], &s);
        assert_eq!(rss.fetched("https://feed", parsed(vec![article("1", "A")]), &s).len(), 1);
        // Legacy bare ids still count as downloaded.
        rss.remember("legacy".into());
        assert!(rss.fetched("https://feed", parsed(vec![article("legacy", "A")]), &s).is_empty());
        drop(rss);
        // Damaged file without backup: read-only, nothing auto-downloaded, file untouched.
        let _ = std::fs::remove_file(path.with_extension("json.bak"));
        std::fs::write(&path, b"{ broken").unwrap();
        let mut rss = Rss::load(path.clone());
        assert!(rss.warning.is_some());
        assert!(rss.fetched("https://feed", parsed(vec![article("9", "A")]), &s).is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ broken");
        for f in std::fs::read_dir(std::env::temp_dir()).unwrap().flatten() {
            if f.file_name().to_string_lossy().starts_with(&format!("corrent-rss-keys-{}", std::process::id())) {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }

    #[test]
    fn rules_only_apply_to_their_feeds() {
        let r = RssRule { feeds: vec!["https://a".into()], must_contain: "x".into(), ..Default::default() };
        assert!(rule_applies_to(&r, "https://a") && !rule_applies_to(&r, "https://b"));
        assert!(rule_is_active(&r));
        assert!(!rule_is_active(&RssRule { enabled: false, ..r }));
    }
}
