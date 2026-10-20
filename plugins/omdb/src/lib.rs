//! OMDb: IMDb ratings (and Rotten Tomatoes / Metacritic scores) for items another provider
//! identified, by their IMDb id. Marked `supplements`: it never identifies items itself (OMDb's
//! title search is weaker than TMDB's). The scores come back as `rating` (IMDb, out of 10) and
//! as `rating:<source>` entries in `ids`, which the host keeps with the item.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "metadata-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::metadata::{Candidate, Episode, Guest, ItemDetails, MediaKind, SearchQuery};
use serde_json::Value;

fn get(imdb: &str) -> Result<Value, String> {
    let key = host::setting("api_key").filter(|k| !k.is_empty()).ok_or("the OMDb API key is not set")?;
    let url = format!("https://www.omdbapi.com/?i={imdb}&apikey={key}");
    let res = host::http(&HttpRequest { method: "GET".into(), url, headers: vec![], body: None })?;
    let v: Value = match res.status {
        200 => serde_json::from_slice(&res.body).map_err(|e| e.to_string())?,
        401 => return Err("OMDb refused the API key".into()),
        s => return Err(format!("OMDb answered {s}")),
    };
    // OMDb answers 200 with {"Response":"False","Error":"…"} for problems.
    if v["Response"].as_str() == Some("False") {
        return Err(v["Error"].as_str().unwrap_or("OMDb found nothing").to_string());
    }
    Ok(v)
}

/// "tt0111161" and nothing else (the id goes into a URL).
fn valid_imdb(id: &str) -> bool {
    id.len() >= 7 && id.len() <= 12 && id.starts_with("tt") && id[2..].bytes().all(|b| b.is_ascii_digit())
}

struct Omdb;

impl Guest for Omdb {
    fn kinds() -> Vec<MediaKind> {
        vec![MediaKind::Movie, MediaKind::Show]
    }

    /// Only items whose IMDb id is known.
    fn search(q: SearchQuery) -> Result<Vec<Candidate>, String> {
        Ok(q.ids
            .iter()
            .filter(|(k, v)| k == "imdb" && valid_imdb(v))
            .map(|(_, v)| Candidate { id: v.clone(), title: q.title.clone(), year: q.year, overview: None, poster_url: None })
            .take(1)
            .collect())
    }

    fn details(_kind: MediaKind, id: String, _language: String) -> Result<ItemDetails, String> {
        if !valid_imdb(&id) {
            return Err("not an IMDb id".into());
        }
        let v = get(&id)?;
        let num = |s: &str| s.replace(',', "").parse::<f32>().ok();
        let imdb = v["imdbRating"].as_str().and_then(num);
        let mut ids = vec![("imdb".to_string(), id.clone())];
        if let Some(r) = imdb {
            ids.push(("rating:imdb".into(), format!("{r:.1}")));
        }
        if let Some(votes) = v["imdbVotes"].as_str().filter(|s| *s != "N/A") {
            ids.push(("rating:imdb_votes".into(), votes.replace(',', "")));
        }
        for r in v["Ratings"].as_array().into_iter().flatten() {
            let (Some(src), Some(val)) = (r["Source"].as_str(), r["Value"].as_str()) else { continue };
            let key = match src {
                "Rotten Tomatoes" => "rating:rotten_tomatoes",
                "Metacritic" => "rating:metacritic",
                _ => continue,
            };
            ids.push((key.into(), val.to_string()));
        }
        Ok(ItemDetails {
            title: None,
            original_title: None,
            overview: None,
            year: None,
            premiered: None,
            runtime_minutes: None,
            rating: imdb,
            official_rating: None,
            genres: vec![],
            credits: vec![],
            artwork: vec![],
            ids,
            collection: None,
        })
    }

    fn episodes(_show: String, _season: u32, _language: String) -> Result<Vec<Episode>, String> {
        Ok(vec![])
    }
}

export!(Omdb);
