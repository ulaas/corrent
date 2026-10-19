//! Internet radio: stations (name, stream URL, optional logo) the administrator adds, kept in
//! `<data>/radio.json`. Players fetch the stream through the server (`/radio/{id}/stream`) so a
//! browser on HTTPS can play an HTTP stream; the desktop app may play the URL itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub url: String,
    pub image: Option<String>,
    pub added_at: i64,
}

#[derive(Default)]
pub struct Stations(tokio::sync::Mutex<()>);

fn file(data: &Path) -> PathBuf {
    data.join("radio.json")
}

pub fn load(data: &Path) -> Vec<Station> {
    std::fs::read(file(data)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub async fn edit<T>(st: &crate::state::AppState, f: impl FnOnce(&mut Vec<Station>) -> T) -> Result<T, String> {
    let _one = st.radio.0.lock().await;
    let data = st.paths.data_dir.clone();
    let mut list = load(&data);
    let r = f(&mut list);
    let b = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
    corrent_core::util::write_atomic(&file(&data), &b).map_err(|e| e.to_string())?;
    Ok(r)
}

/// Playlist files (`.pls`, `.m3u`) name the real stream: the first http(s) URL in them.
pub fn playlist_target(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .filter_map(|l| {
            if l.starts_with("http") {
                Some(l)
            } else if let Some((k, v)) = l.split_once('=') {
                k.to_ascii_lowercase().starts_with("file").then_some(v.trim())
            } else {
                Some(l)
            }
        })
        .find(|u| u.starts_with("http://") || u.starts_with("https://"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    #[test]
    fn playlists() {
        assert_eq!(
            super::playlist_target("[playlist]\nNumberOfEntries=1\nFile1=http://radio.example:8000/live\nTitle1=X"),
            Some("http://radio.example:8000/live".into())
        );
        assert_eq!(
            super::playlist_target("#EXTM3U\n#EXTINF:-1,X\nhttps://s.example/a.mp3\n"),
            Some("https://s.example/a.mp3".into())
        );
        assert_eq!(super::playlist_target("https://s.example/live?sid=1\n"), Some("https://s.example/live?sid=1".into()));
        assert_eq!(super::playlist_target("nothing here"), None);
    }
}
