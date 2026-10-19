//! Podcasts: feeds the server follows, downloading new episodes into a podcast library
//! (`<library folder>/<Podcast>/<YYYY-MM-DD> <Episode>.<ext>` plus `cover.jpg`), keeping the
//! newest few. The `podcasts` task checks them; the library scan then lists the episodes.
//! Subscriptions live in `<data>/podcasts.json`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Feeds larger than this aren't podcasts.
const MAX_FEED: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Podcast {
    pub id: String,
    pub url: String,
    pub title: String,
    pub library_id: i64,
    /// Episodes kept (the newest); 0 = all.
    pub keep: u32,
    pub added_at: i64,
    pub checked_at: Option<i64>,
    pub error: Option<String>,
    /// Episodes on disk after the last check.
    pub episodes: u32,
}

/// The JSON file, one writer at a time.
#[derive(Default)]
pub struct Podcasts(tokio::sync::Mutex<()>);

fn file(data: &Path) -> PathBuf {
    data.join("podcasts.json")
}

pub fn load(data: &Path) -> Vec<Podcast> {
    std::fs::read(file(data)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(data: &Path, list: &[Podcast]) -> Result<(), String> {
    let b = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    corrent_core::util::write_atomic(&file(data), &b).map_err(|e| e.to_string())
}

/// Changes the list under the lock.
pub async fn edit<T>(st: &AppState, f: impl FnOnce(&mut Vec<Podcast>) -> T) -> Result<T, String> {
    let _one = st.podcasts.0.lock().await;
    let data = st.paths.data_dir.clone();
    let mut list = load(&data);
    let r = f(&mut list);
    save(&data, &list)?;
    Ok(r)
}

/// A name safe on every file system.
pub fn safe_name(s: &str) -> String {
    let c: String = s
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(100)
        .collect();
    if c.is_empty() { "Podcast".into() } else { c }
}

#[derive(Debug, PartialEq)]
pub struct Episode {
    pub title: String,
    pub url: String,
    pub published: Option<i64>,
    pub ext: String,
}

pub struct Feed {
    pub title: String,
    pub image: Option<String>,
    pub episodes: Vec<Episode>,
}

fn ext_of(url: &str, mime: Option<&str>) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let from_url = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()));
    from_url.unwrap_or_else(|| {
        match mime.unwrap_or("") {
            "audio/mpeg" => "mp3",
            "audio/mp4" | "audio/x-m4a" | "audio/aac" => "m4a",
            "audio/ogg" | "audio/opus" => "ogg",
            "video/mp4" => "mp4",
            _ => "mp3",
        }
        .into()
    })
}

/// Episodes of a feed, newest first (only ones with an audio/video enclosure).
pub fn parse(bytes: &[u8], url: &str) -> Result<Feed, String> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(url))
        .build()
        .parse(bytes)
        .map_err(|e| format!("not a podcast feed ({e})"))?;
    let mut episodes: Vec<Episode> = feed
        .entries
        .into_iter()
        .filter_map(|e| {
            let content = e.media.iter().flat_map(|m| &m.content).find(|c| {
                c.url.is_some()
                    && c.content_type.as_ref().is_none_or(|t| {
                        let t = t.to_string();
                        t.starts_with("audio/") || t.starts_with("video/")
                    })
            })?;
            let u = content.url.as_ref()?.to_string();
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return None;
            }
            let mime = content.content_type.as_ref().map(|t| t.to_string());
            Some(Episode {
                title: e
                    .title
                    .map(|t| t.content.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "Episode".into()),
                ext: ext_of(&u, mime.as_deref()),
                url: u,
                published: e.published.or(e.updated).map(|d| d.timestamp()),
            })
        })
        .collect();
    episodes.sort_by_key(|e| std::cmp::Reverse(e.published.unwrap_or(0)));
    let image = feed.logo.map(|i| i.uri).or(feed.icon.map(|i| i.uri)).filter(|u| u.starts_with("http"));
    Ok(Feed {
        title: feed
            .title
            .map(|t| t.content.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Podcast".into()),
        image,
        episodes,
    })
}

/// `2026-10-01 Title.mp3`.
pub fn episode_file(e: &Episode) -> String {
    let date = e
        .published
        .map(|t| {
            let (y, m, d) = corrent_core::util::civil_from_days(t.div_euclid(86_400));
            format!("{y:04}-{m:02}-{d:02}")
        })
        .unwrap_or_else(|| "0000-00-00".into());
    format!("{date} {}.{}", safe_name(&e.title), e.ext)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Corrent/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_default()
}

pub async fn fetch(url: &str) -> Result<Feed, String> {
    let mut res = http().get(url).timeout(Duration::from_secs(60)).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(res.status().to_string());
    }
    let mut body = Vec::new();
    while let Some(c) = res.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&c);
        if body.len() > MAX_FEED {
            return Err("the feed is too large".into());
        }
    }
    parse(&body, url)
}

async fn download(url: &str, to: &Path) -> Result<(), String> {
    let mut res = http().get(url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(res.status().to_string());
    }
    let part = to.with_extension("part");
    let mut f = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
    use tokio::io::AsyncWriteExt;
    let r: Result<(), String> = async {
        while let Some(c) = res.chunk().await.map_err(|e| e.to_string())? {
            f.write_all(&c).await.map_err(|e| e.to_string())?;
        }
        f.flush().await.map_err(|e| e.to_string())
    }
    .await;
    drop(f);
    match r {
        Ok(()) => tokio::fs::rename(&part, to).await.map_err(|e| e.to_string()),
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

/// Files this module wrote in a podcast folder (dated names), newest first.
fn ours(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let b = n.as_bytes();
            p.is_file()
                && !n.ends_with(".part")
                && b.len() > 11
                && b[..10]
                    .iter()
                    .enumerate()
                    .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
        })
        .collect();
    v.sort();
    v.reverse();
    v
}

/// Checks one podcast: new episodes are downloaded, old ones beyond `keep` removed.
pub async fn check(st: &Arc<AppState>, p: &Podcast) -> Result<(String, u32, u32), String> {
    let lib = st.library.get(p.library_id).await.map_err(|e| e.to_string())?;
    let root = lib.folders.first().ok_or("the library has no folder")?;
    let feed = fetch(&p.url).await?;
    let dir = Path::new(root).join(safe_name(&feed.title));
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    if let Some(img) = &feed.image
        && !dir.join("cover.jpg").exists()
        && let Ok(r) = http().get(img).timeout(Duration::from_secs(30)).send().await
        && r.status().is_success()
        && let Ok(b) = r.bytes().await
        && b.len() < 10 * 1024 * 1024
    {
        let _ = tokio::fs::write(dir.join("cover.jpg"), &b).await;
    }
    let wanted: Vec<&Episode> =
        if p.keep == 0 { feed.episodes.iter().collect() } else { feed.episodes.iter().take(p.keep as usize).collect() };
    let mut added = 0;
    for e in &wanted {
        let path = dir.join(episode_file(e));
        if path.exists() {
            continue;
        }
        download(&e.url, &path).await?;
        added += 1;
    }
    // Keep the newest `keep` files of ours.
    if p.keep > 0 {
        let d = dir.clone();
        let keep = p.keep as usize;
        tokio::task::spawn_blocking(move || {
            for old in ours(&d).into_iter().skip(keep) {
                let _ = std::fs::remove_file(old);
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    let d = dir.clone();
    let on_disk = tokio::task::spawn_blocking(move || ours(&d).len() as u32).await.unwrap_or(0);
    if added > 0 {
        st.library.start_scan(p.library_id);
    }
    Ok((feed.title, added, on_disk))
}

/// The `podcasts` task.
pub async fn check_all(st: &Arc<AppState>) -> Result<String, String> {
    let list = load(&st.paths.data_dir);
    let (mut added, mut failed) = (0, 0);
    for p in &list {
        let r = check(st, p).await;
        let id = p.id.clone();
        let _ = edit(st, |l| {
            if let Some(x) = l.iter_mut().find(|x| x.id == id) {
                x.checked_at = Some(corrent_core::util::now_unix());
                match &r {
                    Ok((title, _, n)) => {
                        x.title = title.clone();
                        x.episodes = *n;
                        x.error = None;
                    }
                    Err(e) => x.error = Some(e.clone()),
                }
            }
        })
        .await;
        match r {
            Ok((_, n, _)) => added += n,
            Err(_) => failed += 1,
        }
    }
    if failed > 0 && failed == list.len() {
        return Err(format!("{failed} feeds failed"));
    }
    Ok(format!(
        "{} podcasts, {added} new episodes{}",
        list.len(),
        if failed > 0 { format!(", {failed} failed") } else { String::new() }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"><channel>
<title>Open Source Hour</title><image><url>https://example.org/cover.jpg</url></image>
<item><title>Episode 2: Rust</title><pubDate>Thu, 01 Oct 2026 10:00:00 GMT</pubDate><enclosure url="https://cdn.example.org/ep2.mp3?x=1" type="audio/mpeg" length="100"/></item>
<item><title>Episode 1</title><pubDate>Tue, 01 Sep 2026 10:00:00 GMT</pubDate><enclosure url="https://cdn.example.org/ep1" type="audio/mp4" length="100"/></item>
<item><title>Show notes only</title></item>
</channel></rss>"#;

    #[test]
    fn parses_and_names_episodes() {
        let f = parse(RSS.as_bytes(), "https://example.org/feed").unwrap();
        assert_eq!(f.title, "Open Source Hour");
        assert_eq!(f.episodes.len(), 2);
        assert_eq!(f.episodes[0].title, "Episode 2: Rust");
        assert_eq!(episode_file(&f.episodes[0]), "2026-10-01 Episode 2_ Rust.mp3");
        assert_eq!(episode_file(&f.episodes[1]), "2026-09-01 Episode 1.m4a");
        assert!(parse(b"<html></html>", "https://x").is_err());
    }

    #[test]
    fn keeps_only_dated_files() {
        let d = std::env::temp_dir().join(format!("corrent-pod-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for n in ["2026-01-01 a.mp3", "2026-02-01 b.mp3", "cover.jpg", "notes.txt", "2026-03-01 c.part"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let names: Vec<String> =
            ours(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["2026-02-01 b.mp3", "2026-01-01 a.mp3"]);
        let _ = std::fs::remove_dir_all(d);
    }
}
