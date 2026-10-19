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
    /// The files Corrent downloaded (only these are ever removed).
    pub files: Vec<String>,
}

/// The JSON file, one writer at a time; and the podcasts being checked right now.
#[derive(Default)]
pub struct Podcasts(tokio::sync::Mutex<()>, std::sync::Mutex<std::collections::HashSet<String>>);

/// Largest episode (4 GiB) and cover (10 MiB) accepted; downloads stop below 2 GiB free.
const MAX_EPISODE: u64 = 4 << 30;
const MAX_COVER: usize = 10 * 1024 * 1024;
const MIN_FREE: u64 = 2 << 30;

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

pub async fn fetch(url: &str) -> Result<Feed, String> {
    let mut res = crate::net::get_public(url, Duration::from_secs(60), Some(Duration::from_secs(60))).await?;
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
    let mut res = crate::net::get_public(url, Duration::from_secs(60), None).await?;
    if res.content_length().is_some_and(|l| l > MAX_EPISODE) {
        return Err("the episode is too large".into());
    }
    let part = to.with_extension("part");
    let mut f = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
    use tokio::io::AsyncWriteExt;
    let r: Result<(), String> = async {
        let mut n = 0u64;
        while let Some(c) = res.chunk().await.map_err(|e| e.to_string())? {
            n += c.len() as u64;
            if n > MAX_EPISODE {
                return Err("the episode is too large".into());
            }
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

/// The feed's picture as `cover.jpg` (at most 10 MiB).
async fn cover(url: &str, to: &Path) {
    let Ok(mut res) = crate::net::get_public(url, Duration::from_secs(30), Some(Duration::from_secs(30))).await else {
        return;
    };
    let mut body = Vec::new();
    while let Ok(Some(c)) = res.chunk().await {
        body.extend_from_slice(&c);
        if body.len() > MAX_COVER {
            return;
        }
    }
    // Only an image.
    if image_kind(&body) {
        let _ = tokio::fs::write(to, &body).await;
    }
}

fn image_kind(b: &[u8]) -> bool {
    b.starts_with(&[0xff, 0xd8, 0xff]) || b.starts_with(b"\x89PNG") || (b.len() > 12 && &b[8..12] == b"WEBP")
}

/// Marks a podcast as being checked until dropped.
struct Running<'a>(&'a Podcasts, String);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.1.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.1);
    }
}

/// Checks one podcast and records the outcome: new episodes are downloaded, old ones beyond
/// `keep` removed (only files Corrent downloaded). Returns the episodes added.
pub async fn check(st: &Arc<AppState>, id: &str) -> Result<u32, String> {
    if !st.podcasts.1.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string()) {
        return Err("this podcast is being checked already".into());
    }
    let _running = Running(&st.podcasts, id.to_string());
    let p = load(&st.paths.data_dir).into_iter().find(|x| x.id == id).ok_or("no such podcast")?;
    let mut files = p.files.clone();
    let r = check_one(st, &p, &mut files).await;
    let _ = edit(st, |l| {
        if let Some(x) = l.iter_mut().find(|x| x.id == id) {
            x.checked_at = Some(corrent_core::util::now_unix());
            x.files = files.clone();
            x.episodes = files.len() as u32;
            match &r {
                Ok((title, _)) => {
                    x.title = title.clone();
                    x.error = None;
                }
                Err(e) => x.error = Some(e.clone()),
            }
        }
    })
    .await;
    r.map(|(_, n)| n)
}

/// `files` (paths Corrent wrote) is kept up to date even when the check fails halfway.
async fn check_one(st: &Arc<AppState>, p: &Podcast, files: &mut Vec<String>) -> Result<(String, u32), String> {
    let lib = st.library.get(p.library_id).await.map_err(|e| e.to_string())?;
    let root = lib.folders.first().ok_or("the library has no folder")?;
    let feed = fetch(&p.url).await?;
    let dir = Path::new(root).join(safe_name(&feed.title));
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    if let Some(img) = &feed.image
        && !tokio::fs::try_exists(dir.join("cover.jpg")).await.unwrap_or(true)
    {
        cover(img, &dir.join("cover.jpg")).await;
    }
    let wanted: Vec<&Episode> =
        if p.keep == 0 { feed.episodes.iter().collect() } else { feed.episodes.iter().take(p.keep as usize).collect() };
    let mut keep: Vec<String> = Vec::new();
    let mut added = 0;
    let mut result = Ok(());
    for e in &wanted {
        let path = dir.join(episode_file(e));
        let name = path.display().to_string();
        keep.push(name.clone());
        if tokio::fs::try_exists(&path).await.unwrap_or(true) {
            continue;
        }
        let d = dir.clone();
        let free = tokio::task::spawn_blocking(move || corrent_core::engine::free_space_for(&d)).await.ok().flatten();
        if free.is_some_and(|f| f < MIN_FREE) {
            result = Err("not enough free space for new episodes".to_string());
            break;
        }
        if let Err(err) = download(&e.url, &path).await {
            result = Err(err);
            break;
        }
        files.push(name);
        added += 1;
    }
    // Files we wrote that are gone, or (with a limit) no longer among the newest, go.
    let limit = p.keep > 0 && result.is_ok();
    let list = std::mem::take(files);
    *files = tokio::task::spawn_blocking(move || {
        let mut left = Vec::new();
        for f in list {
            if !Path::new(&f).exists() {
                continue;
            }
            if limit && !keep.contains(&f) {
                if std::fs::remove_file(&f).is_err() {
                    left.push(f);
                }
                continue;
            }
            left.push(f);
        }
        left
    })
    .await
    .map_err(|e| e.to_string())?;
    if added > 0 {
        st.library.start_scan(p.library_id);
    }
    result.map(|()| (feed.title, added))
}

/// The `podcasts` task.
pub async fn check_all(st: &Arc<AppState>) -> Result<String, String> {
    let list = load(&st.paths.data_dir);
    let (mut added, mut failed) = (0, 0);
    for p in &list {
        match check(st, &p.id).await {
            Ok(n) => added += n,
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
}
