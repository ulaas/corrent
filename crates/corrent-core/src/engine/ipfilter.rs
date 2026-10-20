//! IP filter lists: a blocklist (URL or local file, plain or gzip, plus the user's own entries)
//! and an optional allowlist.
//!
//! Corrent prepares the lists itself and hands librqbit local files: librqbit fails to start the
//! whole session when a list URL can't be loaded, so a dead blocklist server would otherwise stop
//! all downloads. A downloaded list is cached and reused when the server is unreachable.

use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::config::{Connection, Paths};

const MAX_LIST_BYTES: usize = 128 * 1024 * 1024;

#[derive(Default, Debug)]
pub struct Prepared {
    pub blocklist: Option<PathBuf>,
    pub allowlist: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// One user entry → an inclusive range: `1.2.3.4`, `10.0.0.0/8`, `1.2.3.4-1.2.3.9`,
/// `name:1.2.3.4-1.2.3.9` (P2P format) or the IPv6 equivalents. Comments start with `#`.
pub fn parse_entry(line: &str) -> Result<Option<(IpAddr, IpAddr)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let bad = || t!("“{line}” is not an IP address, CIDR block or range", line = line);
    if let Ok(ip) = line.parse::<IpAddr>() {
        return Ok(Some((ip, ip)));
    }
    if let Some((ip, len)) = line.split_once('/') {
        let ip: IpAddr = ip.trim().parse().map_err(|_| bad())?;
        let len: u32 = len.trim().parse().map_err(|_| bad())?;
        return match ip {
            IpAddr::V4(v4) if len <= 32 => {
                let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
                let start = v4.to_bits() & mask;
                Ok(Some((IpAddr::V4(start.into()), IpAddr::V4((start | !mask).into()))))
            }
            IpAddr::V6(v6) if len <= 128 => {
                let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
                let start = v6.to_bits() & mask;
                Ok(Some((IpAddr::V6(start.into()), IpAddr::V6((start | !mask).into()))))
            }
            _ => Err(bad()),
        };
    }
    // `start-end`, optionally prefixed by `name:` (P2P format). IPv6 addresses contain colons,
    // so try the plain range first.
    let range = |s: &str| -> Option<(IpAddr, IpAddr)> {
        let (a, b) = s.split_once('-')?;
        let (a, b): (IpAddr, IpAddr) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        (a.is_ipv4() == b.is_ipv4() && a <= b).then_some((a, b))
    };
    if let Some(r) = range(line) {
        return Ok(Some(r));
    }
    let named = if line.matches(':').count() > 2 { line.split_once(':') } else { line.rsplit_once(':') };
    named.and_then(|(_, r)| range(r)).map(Some).ok_or_else(bad)
}

/// Problems in the user's entries (shown under the text box).
pub fn check_entries(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| parse_entry(l).err()).collect()
}

fn p2p_lines(text: &str) -> String {
    let mut out = String::new();
    for (a, b) in text.lines().filter_map(|l| parse_entry(l).ok().flatten()) {
        out.push_str(&format!("corrent:{a}-{b}\n"));
    }
    out
}

fn gunzip_if_needed(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(&bytes[..])
            .take(MAX_LIST_BYTES as u64)
            .read_to_end(&mut out)
            .map_err(|e| t!("the list is not valid gzip ({e})", e = e))?;
        Ok(out)
    } else {
        Ok(bytes)
    }
}

/// Read a list from an http(s) URL, a `file://` URL or a local path.
async fn fetch(client: &reqwest::Client, source: &str) -> Result<Vec<u8>, String> {
    let source = source.trim();
    let path = if let Ok(u) = url::Url::parse(source)
        && u.scheme() == "file"
    {
        Some(u.to_file_path().map_err(|_| "invalid file address".to_string())?)
    } else if source.starts_with("http://") || source.starts_with("https://") {
        None
    } else {
        Some(PathBuf::from(source))
    };
    let bytes = match path {
        Some(p) => tokio::fs::read(&p).await.map_err(|e| format!("can't read {}: {e}", p.display()))?,
        None => {
            let mut resp = client.get(source).send().await.map_err(|e| e.without_url().to_string())?;
            if !resp.status().is_success() {
                return Err(t!("the server answered {status}", status = resp.status()));
            }
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|e| e.without_url().to_string())? {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_LIST_BYTES {
                    return Err(t!("the list is too large").into());
                }
            }
            body
        }
    };
    gunzip_if_needed(bytes)
}

/// How old the cached copy of a downloaded list may get before it is refreshed.
const REFRESH_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Download (or read) a list. A downloaded list's copy from an earlier start is used at once (a
/// slow or dead server must not hold up the start); when it is older than a day it is refreshed
/// in the background for the next start. Only the download's own copy is rewritten then, never
/// the merged list. Without a copy the list is fetched now.
async fn load(client: &reqwest::Client, source: &str, cache: &Path, allow: bool, warnings: &mut Vec<String>) -> String {
    let remote = source.trim().starts_with("http://") || source.trim().starts_with("https://");
    // The copy is of this URL (a changed URL fetches the new list now, not a day later).
    let url_file = cache.with_extension("url");
    let same_url = std::fs::read_to_string(&url_file).is_ok_and(|u| u.trim() == source.trim());
    if remote
        && same_url
        && let Ok(b) = std::fs::read(cache)
    {
        let age = std::fs::metadata(cache).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if age.is_none_or(|a| a > REFRESH_AFTER) {
            let (client, source, cache) = (client.clone(), source.to_string(), cache.to_path_buf());
            tokio::spawn(async move {
                match fetch(&client, &source).await {
                    Ok(bytes) => {
                        if let Err(e) = crate::util::write_atomic(&cache, &bytes) {
                            tracing::warn!("could not keep the refreshed IP filter list: {e:#}");
                        }
                    }
                    // Unchanged URL: its file is already right.
                    Err(e) => tracing::warn!("the IP filter list couldn't be refreshed: {e}"),
                }
            });
        }
        return String::from_utf8_lossy(&b).into_owned();
    }
    match fetch(client, source).await {
        Ok(bytes) => {
            if crate::util::write_atomic(cache, &bytes).is_ok() {
                let _ = crate::util::write_replace(&url_file, source.trim().as_bytes());
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(e) => match std::fs::read(cache) {
            Ok(b) => {
                warnings.push(if allow {
                    t!("The allowlist couldn't be updated ({e}); using the copy from an earlier start.", e = e)
                } else {
                    t!("The blocklist couldn't be updated ({e}); using the copy from an earlier start.", e = e)
                });
                String::from_utf8_lossy(&b).into_owned()
            }
            Err(_) => {
                warnings.push(if allow {
                    t!("The allowlist couldn't be loaded ({e}); continuing without it.", e = e)
                } else {
                    t!("The blocklist couldn't be loaded ({e}); continuing without it.", e = e)
                });
                String::new()
            }
        },
    }
}

pub async fn prepare(paths: &Paths, c: &Connection) -> Prepared {
    let mut out = Prepared::default();
    let custom = p2p_lines(&c.custom_blocklist);
    if c.blocklist_url.trim().is_empty() && c.allowlist_url.trim().is_empty() && custom.is_empty() {
        return out;
    }
    let dir = paths.data_dir.join("ipfilter");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        out.warnings.push(t!("The IP filter couldn't be prepared: {e}", e = e));
        return out;
    }
    let client = crate::rss::feed::client(c.proxy_url());
    let mut block = String::new();
    if !c.blocklist_url.trim().is_empty() {
        block = load(&client, &c.blocklist_url, &dir.join("blocklist-download.txt"), false, &mut out.warnings).await;
    }
    if !block.is_empty() && !block.ends_with('\n') {
        block.push('\n');
    }
    block.push_str(&custom);
    if !block.trim().is_empty() {
        let f = dir.join("blocklist.txt");
        match crate::util::write_atomic(&f, block.as_bytes()) {
            Ok(()) => out.blocklist = Some(f),
            Err(e) => out.warnings.push(t!("The blocklist couldn't be saved: {e}", e = format!("{e:#}"))),
        }
    }
    if !c.allowlist_url.trim().is_empty() {
        let allow = load(&client, &c.allowlist_url, &dir.join("allowlist-download.txt"), true, &mut out.warnings).await;
        // An empty allowlist would block every peer: only use it when it has entries.
        if allow.lines().any(|l| !l.trim().is_empty() && !l.trim().starts_with('#')) {
            let f = dir.join("allowlist.txt");
            if crate::util::write_atomic(&f, allow.as_bytes()).is_ok() {
                out.allowlist = Some(f);
            }
        }
    }
    out
}

pub fn file_url(p: &Path) -> Option<String> {
    url::Url::from_file_path(p).ok().map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Option<(IpAddr, IpAddr)> {
        parse_entry(s).unwrap()
    }

    #[test]
    fn parses_entries() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(r("1.2.3.4"), Some((ip("1.2.3.4"), ip("1.2.3.4"))));
        assert_eq!(r("10.0.0.0/8"), Some((ip("10.0.0.0"), ip("10.255.255.255"))));
        assert_eq!(r("10.1.2.3/24"), Some((ip("10.1.2.0"), ip("10.1.2.255"))));
        assert_eq!(r(" 1.2.3.4 - 1.2.3.9 "), Some((ip("1.2.3.4"), ip("1.2.3.9"))));
        assert_eq!(r("Some org:1.2.3.4-1.2.3.9"), Some((ip("1.2.3.4"), ip("1.2.3.9"))));
        assert_eq!(r("2001:db8::/32"), Some((ip("2001:db8::"), ip("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff"))));
        assert_eq!(r("2001:db8::1-2001:db8::9"), Some((ip("2001:db8::1"), ip("2001:db8::9"))));
        assert_eq!(r("# comment"), None);
        assert_eq!(r(""), None);
        assert!(parse_entry("1.2.3.9-1.2.3.4").is_err(), "reversed range");
        assert!(parse_entry("1.2.3.4/33").is_err());
        assert!(parse_entry("example.com").is_err());
        assert_eq!(check_entries("1.2.3.4\nnope\n# fine").len(), 1);
        assert_eq!(p2p_lines("10.0.0.0/8\nbad"), "corrent:10.0.0.0-10.255.255.255\n");
    }

    #[test]
    fn gzip_lists_are_unpacked() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"x:1.2.3.4-1.2.3.5\n").unwrap();
        let packed = gz.finish().unwrap();
        assert_eq!(gunzip_if_needed(packed).unwrap(), b"x:1.2.3.4-1.2.3.5\n");
        assert_eq!(gunzip_if_needed(b"plain".to_vec()).unwrap(), b"plain");
    }

    #[tokio::test]
    async fn unreachable_list_falls_back_to_cache_or_nothing() {
        let dir = std::env::temp_dir().join(format!("corrent-ipf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths { data_dir: dir.clone(), portable: true };
        let c = Connection {
            blocklist_url: "http://127.0.0.1:9/list.gz".into(),
            custom_blocklist: "10.0.0.0/8".into(),
            ..Default::default()
        };
        let p = prepare(&paths, &c).await;
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        let text = std::fs::read_to_string(p.blocklist.unwrap()).unwrap();
        assert_eq!(text, "corrent:10.0.0.0-10.255.255.255\n", "custom entries still apply");
        std::fs::write(dir.join("ipfilter/blocklist-download.txt"), "old:1.1.1.1-1.1.1.2\n").unwrap();
        std::fs::write(dir.join("ipfilter/blocklist-download.url"), &c.blocklist_url).unwrap();
        // A copy from an earlier start is used at once, without asking the (dead) server.
        let p = prepare(&paths, &c).await;
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let text = std::fs::read_to_string(p.blocklist.unwrap()).unwrap();
        assert!(text.starts_with("old:1.1.1.1-1.1.1.2\n") && text.contains("corrent:10.0.0.0"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
