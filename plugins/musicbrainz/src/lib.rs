//! MusicBrainz (albums, artists) with covers from the Cover Art Archive.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "metadata-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::metadata::{Artwork, Candidate, Credit, Episode, Guest, ItemDetails, MediaKind, SearchQuery};
use serde_json::Value;

const API: &str = "https://musicbrainz.org/ws/2";

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Lucene phrase with quotes and backslashes escaped.
fn phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn get(path_and_query: &str) -> Result<Value, String> {
    let res = host::http(&HttpRequest {
        method: "GET".into(),
        url: format!("{API}{path_and_query}"),
        headers: vec![("accept".into(), "application/json".into())],
        body: None,
    })?;
    if res.status == 503 {
        return Err("MusicBrainz is busy, try again later".into());
    }
    if res.status != 200 {
        return Err(format!("MusicBrainz answered {}", res.status));
    }
    serde_json::from_slice(&res.body).map_err(|e| e.to_string())
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).filter(|x| !x.is_empty())
}

fn artist_credit(v: &Value) -> Option<String> {
    let parts: Vec<String> = v["artist-credit"]
        .as_array()?
        .iter()
        .map(|c| format!("{}{}", c["name"].as_str().unwrap_or(""), c["joinphrase"].as_str().unwrap_or("")))
        .collect();
    Some(parts.concat()).filter(|x| !x.is_empty())
}

struct MusicBrainz;

impl Guest for MusicBrainz {
    fn kinds() -> Vec<MediaKind> {
        vec![MediaKind::Album, MediaKind::Artist]
    }

    fn search(q: SearchQuery) -> Result<Vec<Candidate>, String> {
        match q.kind {
            MediaKind::Artist => {
                let v = get(&format!("/artist/?fmt=json&limit=10&query={}", enc(&format!("artist:{}", phrase(&q.title)))))?;
                Ok(v["artists"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| Candidate {
                        id: s(a, "id").unwrap_or_default(),
                        title: s(a, "name").unwrap_or_default(),
                        year: s(&a["life-span"], "begin").and_then(|d| d.get(..4).and_then(|y| y.parse().ok())),
                        overview: s(a, "disambiguation"),
                        poster_url: None,
                    })
                    .collect())
            }
            MediaKind::Album => {
                let mut query = format!("releasegroup:{}", phrase(&q.title));
                if let Some(a) = &q.artist {
                    query.push_str(&format!(" AND artist:{}", phrase(a)));
                }
                if let Some((_, id)) = q.ids.iter().find(|(k, _)| k == "musicbrainz") {
                    // A release id from tags: look up its release group directly.
                    let v = get(&format!("/release/{}?fmt=json&inc=release-groups", enc(id)))?;
                    if let Some(rg) = s(&v["release-group"], "id") {
                        return Ok(vec![Candidate {
                            id: rg,
                            title: s(&v["release-group"], "title").unwrap_or_default(),
                            year: s(&v["release-group"], "first-release-date").and_then(|d| d.get(..4).and_then(|y| y.parse().ok())),
                            overview: None,
                            poster_url: None,
                        }]);
                    }
                }
                let v = get(&format!("/release-group/?fmt=json&limit=10&query={}", enc(&query)))?;
                Ok(v["release-groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|g| Candidate {
                        id: s(g, "id").unwrap_or_default(),
                        title: s(g, "title").unwrap_or_default(),
                        year: s(g, "first-release-date").and_then(|d| d.get(..4).and_then(|y| y.parse().ok())),
                        overview: artist_credit(g),
                        poster_url: None,
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn details(kind: MediaKind, id: String, _language: String) -> Result<ItemDetails, String> {
        match kind {
            MediaKind::Artist => {
                let v = get(&format!("/artist/{}?fmt=json&inc=genres", enc(&id)))?;
                Ok(ItemDetails {
                    title: s(&v, "name"),
                    original_title: None,
                    overview: s(&v, "disambiguation"),
                    year: None,
                    premiered: s(&v["life-span"], "begin"),
                    runtime_minutes: None,
                    rating: None,
                    official_rating: None,
                    genres: v["genres"].as_array().into_iter().flatten().take(5).filter_map(|g| s(g, "name")).collect(),
                    credits: Vec::new(),
                    artwork: Vec::new(),
                    ids: vec![("musicbrainz".into(), id)],
                    collection: None,
                })
            }
            _ => {
                let v = get(&format!("/release-group/{}?fmt=json&inc=artists+genres", enc(&id)))?;
                let date = s(&v, "first-release-date");
                Ok(ItemDetails {
                    title: s(&v, "title"),
                    original_title: None,
                    overview: None,
                    year: date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
                    premiered: date,
                    runtime_minutes: None,
                    rating: None,
                    official_rating: None,
                    genres: v["genres"].as_array().into_iter().flatten().take(5).filter_map(|g| s(g, "name")).collect(),
                    credits: artist_credit(&v)
                        .map(|a| vec![Credit { name: a, role: "artist".into(), character: None, image_url: None }])
                        .unwrap_or_default(),
                    artwork: vec![Artwork { kind: "poster".into(), url: format!("https://coverartarchive.org/release-group/{id}/front-500"), language: None }],
                    ids: vec![("musicbrainz".into(), id)],
                    collection: None,
                })
            }
        }
    }

    fn episodes(_show: String, _season: u32, _language: String) -> Result<Vec<Episode>, String> {
        Ok(Vec::new())
    }
}

export!(MusicBrainz);
