//! fanart.tv: artwork only (logos, clear art, banners, backdrops), for movies by TMDB/IMDb id
//! and shows by TVDB id. Marked `supplements` in its manifest: it never identifies items, it
//! adds pictures to items another provider identified.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "metadata-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::metadata::{Artwork, Candidate, Episode, Guest, ItemDetails, MediaKind, SearchQuery};
use serde_json::Value;

fn get(path: &str) -> Result<Value, String> {
    let key = host::setting("api_key").filter(|k| !k.is_empty()).ok_or("the fanart.tv API key is not set")?;
    let mut url = format!("https://webservice.fanart.tv/v3{path}?api_key={key}");
    if let Some(c) = host::setting("client_key").filter(|k| !k.is_empty()) {
        url.push_str(&format!("&client_key={c}"));
    }
    let res = host::http(&HttpRequest { method: "GET".into(), url, headers: vec![], body: None })?;
    match res.status {
        200 => serde_json::from_slice(&res.body).map_err(|e| e.to_string()),
        404 => Err("not on fanart.tv".into()),
        401 => Err("fanart.tv refused the API key".into()),
        s => Err(format!("fanart.tv answered {s}")),
    }
}

/// The best image of a list: the wanted language, then English, then text-free ("00").
fn best(v: &Value, key: &str, language: &str) -> Option<(String, Option<String>)> {
    let list = v.get(key)?.as_array()?;
    let likes = |x: &Value| x["likes"].as_str().and_then(|l| l.parse::<i64>().ok()).unwrap_or(0);
    for lang in [language, "en", "00", ""] {
        let pick = list
            .iter()
            .filter(|x| lang.is_empty() || x["lang"].as_str() == Some(lang))
            .max_by_key(|x| likes(x));
        if let Some(x) = pick
            && let Some(url) = x["url"].as_str()
        {
            return Some((url.to_string(), x["lang"].as_str().map(str::to_string)));
        }
    }
    None
}

struct Fanart;

impl Guest for Fanart {
    fn kinds() -> Vec<MediaKind> {
        vec![MediaKind::Movie, MediaKind::Show]
    }

    /// Only items whose ids are known: fanart.tv has no title search.
    fn search(q: SearchQuery) -> Result<Vec<Candidate>, String> {
        let id = |k: &str| q.ids.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let found = match q.kind {
            MediaKind::Movie => id("tmdb").or_else(|| id("imdb")).map(|i| format!("movie:{i}")),
            MediaKind::Show => id("tvdb").map(|i| format!("tv:{i}")),
            _ => None,
        };
        Ok(found
            .into_iter()
            .map(|id| Candidate { id, title: q.title.clone(), year: q.year, overview: None, poster_url: None })
            .collect())
    }

    fn details(_kind: MediaKind, id: String, language: String) -> Result<ItemDetails, String> {
        let (kind, num) = id.split_once(':').ok_or("unknown id")?;
        let (path, keys): (String, &[(&str, &str)]) = if kind == "tv" {
            (
                format!("/tv/{num}"),
                &[("tvposter", "poster"), ("showbackground", "backdrop"), ("hdtvlogo", "logo"), ("tvbanner", "banner"), ("tvthumb", "thumb")],
            )
        } else {
            (
                format!("/movies/{num}"),
                &[("movieposter", "poster"), ("moviebackground", "backdrop"), ("hdmovielogo", "logo"), ("moviebanner", "banner"), ("moviethumb", "thumb")],
            )
        };
        let v = get(&path)?;
        let mut artwork = Vec::new();
        for (src, kind) in keys {
            // Backdrops look best without text.
            let lang = if *kind == "backdrop" { "00" } else { language.as_str() };
            if let Some((url, l)) = best(&v, src, lang) {
                artwork.push(Artwork { kind: kind.to_string(), url, language: l });
            }
        }
        Ok(ItemDetails {
            title: None,
            original_title: None,
            overview: None,
            year: None,
            premiered: None,
            runtime_minutes: None,
            rating: None,
            official_rating: None,
            genres: vec![],
            credits: vec![],
            artwork,
            ids: vec![],
            collection: None,
        })
    }

    fn episodes(_show: String, _season: u32, _language: String) -> Result<Vec<Episode>, String> {
        Ok(vec![])
    }
}

export!(Fanart);
