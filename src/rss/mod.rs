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

const STATE_VERSION: u32 = 1;
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
#[derive(Clone, Debug, Default)]
pub struct RssView {
    pub feeds: Vec<FeedView>,
}

#[derive(Clone, Debug, Default)]
pub struct FeedView {
    pub url: String,
    pub title: String,
    pub articles: Vec<ArticleView>,
    pub last_ok: Option<i64>,
    pub error: Option<String>,
    pub loading: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default)]
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
}

pub struct Rss {
    path: PathBuf,
    data: StateFile,
    downloaded: HashSet<String>,
    next_due: HashMap<String, Instant>,
    in_flight: HashSet<String>,
    view: Arc<RssView>,
}

impl Rss {
    pub fn load(path: PathBuf) -> Rss {
        let data: StateFile = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::error!("rss.json is invalid ({e}); starting with an empty feed cache");
                let _ = std::fs::copy(&path, path.with_extension(format!("json.bad-{}", crate::util::now_unix())));
                StateFile::default()
            }),
            Err(_) => StateFile::default(),
        };
        let downloaded = data.downloaded.iter().cloned().collect();
        Rss { path, data, downloaded, next_due: HashMap::new(), in_flight: HashSet::new(), view: Default::default() }
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
                tracing::info!("feed {url}: {e}");
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
        let downloads = if s.auto_download { self.match_feed(url, s) } else { Vec::new() };
        self.save();
        self.rebuild(s);
        downloads
    }

    /// Re-run the rules over every cached article (after rules changed).
    pub fn apply_rules(&mut self, s: &RssSettings) -> Vec<Download> {
        if !s.auto_download {
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
            if self.downloaded.contains(&a.guid) || marks.iter().any(|(g, _, _)| g == &a.guid) {
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
                out.push(Download { link: link.clone(), title: a.title.clone(), rule: (*rule).clone() });
                marks.push((a.guid.clone(), rule.name.clone(), episode));
                break;
            }
        }
        for (guid, rule, episode) in marks {
            self.remember(guid);
            if let Some(ep) = episode {
                self.data.episodes.entry(rule).or_default().push(ep);
            }
        }
        out
    }

    /// Mark an article as downloaded (e.g. added by hand), so rules skip it.
    pub fn mark_downloaded(&mut self, guid: String, s: &RssSettings) {
        if !self.downloaded.contains(&guid) {
            self.remember(guid);
            self.save();
            self.rebuild(s);
        }
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

    fn rebuild(&mut self, s: &RssSettings) {
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
                                    downloaded: self.downloaded.contains(&a.guid),
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
        self.data.schema_version = STATE_VERSION;
        match serde_json::to_vec(&self.data) {
            Ok(b) => {
                if let Err(e) = crate::util::write_atomic(&self.path, &b) {
                    tracing::error!("failed to save rss.json: {e:#}");
                }
            }
            Err(e) => tracing::error!("failed to encode rss state: {e}"),
        }
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
    fn rules_only_apply_to_their_feeds() {
        let r = RssRule { feeds: vec!["https://a".into()], must_contain: "x".into(), ..Default::default() };
        assert!(rule_applies_to(&r, "https://a") && !rule_applies_to(&r, "https://b"));
        assert!(rule_is_active(&r));
        assert!(!rule_is_active(&RssRule { enabled: false, ..r }));
    }
}
