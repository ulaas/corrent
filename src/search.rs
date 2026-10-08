//! Torrent search through Torznab providers the user configures (Jackett, Prowlarr, …).
//! Corrent never scrapes sites itself: it only queries the endpoints given in Settings › Search.

use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SearchSettings {
    pub providers: Vec<TorznabProvider>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TorznabProvider {
    pub name: String,
    /// Torznab API endpoint, e.g. `http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab`.
    pub url: String,
    pub api_key: String,
    pub enabled: bool,
}

impl Default for TorznabProvider {
    fn default() -> Self {
        TorznabProvider { name: String::new(), url: String::new(), api_key: String::new(), enabled: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResult {
    pub provider: String,
    pub title: String,
    /// Magnet link if the indexer gave one, else the .torrent download URL.
    pub link: Option<String>,
    pub page: Option<String>,
    pub size: Option<u64>,
    pub seeders: Option<u32>,
    pub peers: Option<u32>,
    pub published: Option<i64>,
    pub info_hash: Option<String>,
    pub category: Option<String>,
}

/// The request URL for a free-text search (the API key is a query parameter, as Torznab requires).
pub fn search_url(p: &TorznabProvider, query: &str) -> Result<url::Url, String> {
    let mut u = url::Url::parse(p.url.trim()).map_err(|_| "the provider address is not a valid URL".to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("the provider address must start with http:// or https://".into());
    }
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("t", "search");
        q.append_pair("q", query);
        if !p.api_key.trim().is_empty() {
            q.append_pair("apikey", p.api_key.trim());
        }
    }
    Ok(u)
}

pub async fn search(client: &reqwest::Client, p: &TorznabProvider, query: &str) -> Result<Vec<SearchResult>, String> {
    let url = search_url(p, query)?;
    let describe = |e: reqwest::Error| {
        if e.is_timeout() {
            "the provider didn't answer in time".to_string()
        } else if e.is_connect() {
            "couldn't connect to the provider".to_string()
        } else {
            // The URL carries the API key: never show it.
            e.without_url().to_string()
        }
    };
    let mut resp = client.get(url).send().await.map_err(describe)?;
    let status = resp.status();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(describe)? {
        body.extend_from_slice(&chunk);
        if body.len() > 16 * 1024 * 1024 {
            return Err("the answer is too large".into());
        }
    }
    // Torznab reports errors as XML, sometimes with an HTTP error status.
    match parse(&body, &p.name) {
        Ok(r) => Ok(r),
        Err(e) if !status.is_success() => Err(format!("the provider answered {status} ({e})")),
        Err(e) => Err(e),
    }
}

fn unescape_ref(name: &str) -> String {
    quick_xml::escape::unescape(&format!("&{name};")).map(|s| s.into_owned()).unwrap_or_default()
}

/// Parse a Torznab (RSS 2.0 + `torznab:attr`) response.
pub fn parse(xml: &[u8], provider: &str) -> Result<Vec<SearchResult>, String> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut item: Option<SearchResult> = None;
    let mut field: Option<String> = None;
    let mut text = String::new();
    let mut seen_rss = false;
    loop {
        let ev = reader.read_event_into(&mut buf).map_err(|e| format!("not a valid Torznab answer ({e})"))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name().as_ref().to_string();
                let attr = |key: &str| {
                    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == key).and_then(|a| {
                        a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned())
                    })
                };
                match name.as_str() {
                    "rss" | "channel" => seen_rss = true,
                    "error" => {
                        let code = attr("code").unwrap_or_default();
                        let desc = attr("description").unwrap_or_else(|| "unknown error".into());
                        return Err(match code.as_str() {
                            "100" | "101" | "102" => format!("the API key was rejected ({desc})"),
                            _ => desc,
                        });
                    }
                    "item" => item = Some(SearchResult { provider: provider.to_string(), ..Default::default() }),
                    "enclosure" => {
                        if let Some(it) = &mut item {
                            if it.link.is_none() {
                                it.link = attr("url");
                            }
                            if it.size.is_none() {
                                it.size = attr("length").and_then(|l| l.parse().ok()).filter(|n| *n > 0);
                            }
                        }
                    }
                    "attr" => {
                        if let (Some(it), Some(k), Some(v)) = (&mut item, attr("name"), attr("value")) {
                            match k.as_str() {
                                "seeders" => it.seeders = v.parse().ok(),
                                "peers" => it.peers = v.parse().ok(),
                                "size" => it.size = v.parse().ok().or(it.size),
                                "magneturl" if v.starts_with("magnet:") => it.link = Some(v),
                                "infohash" => it.info_hash = Some(v.to_ascii_lowercase()),
                                _ => {}
                            }
                        }
                    }
                    _ if item.is_some() => {
                        field = Some(name);
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) => text.push_str(&t.xml10_content()),
            Event::CData(t) => text.push_str(&t.xml10_content()),
            Event::GeneralRef(r) => text.push_str(&unescape_ref(&r.xml10_content())),
            Event::End(e) => {
                let name = e.local_name().as_ref().to_string();
                if name == "item" {
                    if let Some(it) = item.take()
                        && !it.title.is_empty()
                    {
                        out.push(it);
                    }
                } else if let (Some(it), Some(f)) = (&mut item, &field)
                    && *f == name
                {
                    let v = text.trim().to_string();
                    match f.as_str() {
                        "title" => it.title = v,
                        "link" if !v.is_empty() => {
                            if it.link.is_none() {
                                it.link = Some(v);
                            }
                        }
                        "comments" | "guid" if v.starts_with("http") => {
                            if it.page.is_none() {
                                it.page = Some(v);
                            }
                        }
                        "size" => it.size = v.parse().ok().or(it.size),
                        "pubDate" => it.published = parse_rfc2822(&v),
                        "category" if it.category.is_none() => it.category = Some(v),
                        _ => {}
                    }
                    field = None;
                }
            }
            _ => {}
        }
        buf.clear();
    }
    if !seen_rss {
        return Err("not a Torznab answer (is the address the indexer's Torznab feed?)".into());
    }
    Ok(out)
}

fn parse_rfc2822(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(s.trim()).ok().map(|d| d.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><title>Indexer</title>
<item>
  <title>Sintel 2010 1080p &amp; extras</title>
  <guid>https://indexer.example/details/1</guid>
  <link>https://indexer.example/dl/1.torrent?apikey=k</link>
  <pubDate>Mon, 05 Oct 2026 10:00:00 +0000</pubDate>
  <size>1234567</size>
  <category>Movies</category>
  <enclosure url="https://indexer.example/dl/1.torrent?apikey=k" length="1234567" type="application/x-bittorrent"/>
  <torznab:attr name="seeders" value="42"/>
  <torznab:attr name="peers" value="50"/>
  <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&amp;dn=Sintel"/>
  <torznab:attr name="infohash" value="08ADA5A7A6183AAE1E09D831DF6748D566095A10"/>
</item>
<item><title><![CDATA[Big Buck Bunny]]></title><link>https://indexer.example/dl/2.torrent</link></item>
</channel></rss>"#;

    #[test]
    fn parses_torznab_items() {
        let r = parse(XML.as_bytes(), "Local").unwrap();
        assert_eq!(r.len(), 2);
        let a = &r[0];
        assert_eq!(a.title, "Sintel 2010 1080p & extras");
        assert_eq!(a.seeders, Some(42));
        assert_eq!(a.peers, Some(50));
        assert_eq!(a.size, Some(1_234_567));
        assert!(a.link.as_deref().unwrap().starts_with("magnet:?xt=urn:btih:08ada5"), "magnet preferred");
        assert!(a.link.as_deref().unwrap().contains("&dn=Sintel"));
        assert_eq!(a.page.as_deref(), Some("https://indexer.example/details/1"));
        assert_eq!(a.published, Some(1_791_194_400));
        assert_eq!(a.info_hash.as_deref(), Some("08ada5a7a6183aae1e09d831df6748d566095a10"));
        assert_eq!(a.category.as_deref(), Some("Movies"));
        assert_eq!(r[1].title, "Big Buck Bunny");
        assert_eq!(r[1].link.as_deref(), Some("https://indexer.example/dl/2.torrent"));
    }

    #[test]
    fn reports_errors() {
        let e = parse(br#"<?xml version="1.0"?><error code="100" description="Invalid API Key"/>"#, "x").unwrap_err();
        assert!(e.contains("API key"));
        assert!(parse(b"<html><body>login</body></html>", "x").is_err());
    }

    #[test]
    fn builds_urls() {
        let p =
            TorznabProvider { url: "http://127.0.0.1:9117/api?x=1".into(), api_key: "k".into(), ..Default::default() };
        let u = search_url(&p, "big buck & bunny").unwrap();
        assert_eq!(u.as_str(), "http://127.0.0.1:9117/api?x=1&t=search&q=big+buck+%26+bunny&apikey=k");
        assert!(search_url(&TorznabProvider { url: "ftp://x".into(), ..Default::default() }, "a").is_err());
    }
}
