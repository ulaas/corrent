//! Fetching and parsing feeds (RSS 0.x/1.0/2.0, Atom, JSON Feed).

use serde::{Deserialize, Serialize};

/// Feeds larger than this are rejected (a feed is a few hundred KiB at most).
const MAX_FEED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Article {
    pub guid: String,
    pub title: String,
    /// .torrent URL or magnet link, if the item has one.
    pub torrent: Option<String>,
    /// Web page of the item.
    pub page: Option<String>,
    pub published: Option<i64>,
    pub size: Option<u64>,
}

pub struct ParsedFeed {
    pub title: Option<String>,
    pub articles: Vec<Article>,
}

pub fn parse(bytes: &[u8], url: &str) -> Result<ParsedFeed, String> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(url))
        .build()
        .parse(bytes)
        .map_err(|e| t!("not a valid feed ({e})", e = e))?;
    let articles = feed
        .entries
        .into_iter()
        .map(|e| {
            let title = e.title.map(|t| t.content.trim().to_string()).unwrap_or_default();
            let (torrent, size) = torrent_link(&e.links, &e.media);
            let page = e.links.iter().map(|l| l.href.clone()).find(|h| Some(h) != torrent.as_ref() && is_web(h));
            Article {
                guid: e.id,
                title,
                torrent,
                page,
                published: e.published.or(e.updated).map(|d| d.timestamp()),
                size,
            }
        })
        .collect();
    Ok(ParsedFeed { title: feed.title.map(|t| t.content.trim().to_string()).filter(|t| !t.is_empty()), articles })
}

fn is_web(href: &str) -> bool {
    href.starts_with("http://") || href.starts_with("https://")
}

fn is_torrent_url(href: &str, mime: Option<&str>) -> bool {
    href.starts_with("magnet:")
        || mime.is_some_and(|m| m.eq_ignore_ascii_case("application/x-bittorrent"))
        || href.split(['?', '#']).next().is_some_and(|p| p.to_ascii_lowercase().ends_with(".torrent"))
}

/// The best download link: an enclosure / media item or link that is a torrent or magnet; else
/// the only link if it's http(s) (many trackers' feeds link straight to the download).
fn torrent_link(
    links: &[feed_rs::model::Link],
    media: &[feed_rs::model::MediaObject],
) -> (Option<String>, Option<u64>) {
    for m in media {
        for c in &m.content {
            if let Some(u) = &c.url {
                let mime = c.content_type.as_ref().map(|t| t.to_string());
                if is_torrent_url(u.as_str(), mime.as_deref()) {
                    return (Some(u.to_string()), c.size);
                }
            }
        }
    }
    for l in links {
        if is_torrent_url(&l.href, l.media_type.as_deref()) {
            return (Some(l.href.clone()), l.length);
        }
    }
    // Fallback: an enclosure without a type, or a single link.
    let enclosure = media.iter().flat_map(|m| &m.content).find_map(|c| c.url.as_ref().map(|u| (u.to_string(), c.size)));
    if let Some((u, size)) = enclosure {
        return (Some(u), size);
    }
    match links {
        [only] if is_web(&only.href) => (Some(only.href.clone()), only.length),
        _ => (None, None),
    }
}

pub fn client(proxy: Option<String>) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("Corrent/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(15));
    if let Some(p) = proxy.and_then(|p| reqwest::Proxy::all(p).ok()) {
        b = b.proxy(p);
    }
    b.build().unwrap_or_default()
}

pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<ParsedFeed, String> {
    let mut resp = client.get(url).send().await.map_err(describe)?;
    if !resp.status().is_success() {
        return Err(t!("the server answered {status}", status = resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(describe)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_FEED_BYTES {
            return Err(t!("the feed is too large").into());
        }
    }
    let url = url.to_string();
    tokio::task::spawn_blocking(move || parse(&body, &url)).await.map_err(|e| e.to_string())?
}

fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        t!("the server didn't answer in time").into()
    } else if e.is_connect() {
        t!("couldn't connect to the server").into()
    } else {
        // Feed URLs may contain passkeys: keep them out of messages and logs.
        let e = e.without_url();
        let mut s = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(inner) = src {
            s = format!("{s}: {inner}");
            src = inner.source();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Test feed</title>
<item><title>Sintel 2010 1080p</title><guid>sintel-1</guid>
  <link>https://example.org/sintel</link>
  <enclosure url="https://example.org/sintel.torrent" length="1234" type="application/x-bittorrent"/>
  <pubDate>Mon, 01 Jan 2024 10:00:00 GMT</pubDate></item>
<item><title>Big Buck Bunny</title><link>magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&amp;dn=bbb</link></item>
<item><title>No link</title></item>
</channel></rss>"#;

    #[test]
    fn parses_rss_enclosures_and_magnets() {
        let f = parse(RSS.as_bytes(), "https://example.org/feed").unwrap();
        assert_eq!(f.title.as_deref(), Some("Test feed"));
        assert_eq!(f.articles.len(), 3);
        let a = &f.articles[0];
        assert_eq!(a.guid, "sintel-1");
        assert_eq!(a.torrent.as_deref(), Some("https://example.org/sintel.torrent"));
        assert_eq!(a.page.as_deref(), Some("https://example.org/sintel"));
        assert_eq!(a.size, Some(1234));
        assert_eq!(a.published, Some(1_704_103_200));
        assert!(f.articles[1].torrent.as_deref().unwrap().starts_with("magnet:?xt=urn:btih:dd8255"));
        assert_eq!(f.articles[2].torrent, None);
        assert!(!f.articles[2].guid.is_empty(), "items without a guid still get a stable id");
        let again = parse(RSS.as_bytes(), "https://example.org/feed").unwrap();
        assert_eq!(again.articles[2].guid, f.articles[2].guid);
    }

    #[test]
    fn parses_atom() {
        let atom = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom</title><id>x</id><updated>2024-01-01T00:00:00Z</updated>
<entry><title>Debian 12 netinst</title><id>urn:debian-12</id><updated>2024-01-02T00:00:00Z</updated>
<link rel="enclosure" type="application/x-bittorrent" href="/debian.torrent"/></entry></feed>"#;
        let f = parse(atom.as_bytes(), "https://cdimage.example/feed.atom").unwrap();
        assert_eq!(f.articles[0].torrent.as_deref(), Some("https://cdimage.example/debian.torrent"));
        assert!(parse(b"<html>nope</html>", "https://x").is_err());
    }
}
