//! TMDB (themoviedb.org) metadata for movies and TV shows.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "metadata-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::metadata::{Artwork, Candidate, Credit, Episode, Guest, ItemDetails, MediaKind, SearchQuery};
use serde_json::Value;

const API: &str = "https://api.themoviedb.org/3";
const IMG: &str = "https://image.tmdb.org/t/p/";

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn get(path: &str, query: &[(&str, String)]) -> Result<Value, String> {
    let key = host::setting("api_key").filter(|k| !k.is_empty()).ok_or("the TMDB API key is not set")?;
    let mut url = format!("{API}{path}?");
    let mut headers = vec![("accept".to_string(), "application/json".to_string())];
    // A v4 read access token is a long JWT; the v3 key is 32 hex digits.
    if key.len() > 40 {
        headers.push(("authorization".into(), format!("Bearer {key}")));
    } else {
        url.push_str(&format!("api_key={}&", enc(&key)));
    }
    for (k, v) in query {
        url.push_str(&format!("{k}={}&", enc(v)));
    }
    let res = host::http(&HttpRequest { method: "GET".into(), url, headers, body: None })?;
    if res.status == 401 {
        return Err("TMDB refused the API key".into());
    }
    if res.status == 404 {
        return Err("not found on TMDB".into());
    }
    if res.status != 200 {
        return Err(format!("TMDB answered {}", res.status));
    }
    serde_json::from_slice(&res.body).map_err(|e| e.to_string())
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).filter(|x| !x.is_empty())
}

fn year_of(date: Option<String>) -> Option<u32> {
    date.and_then(|d| d.get(..4).and_then(|y| y.parse().ok()))
}

fn image(path: Option<String>, size: &str) -> Option<String> {
    path.map(|p| format!("{IMG}{size}{p}"))
}

fn tv(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Show | MediaKind::Season | MediaKind::Episode)
}

struct Tmdb;

impl Guest for Tmdb {
    fn kinds() -> Vec<MediaKind> {
        vec![MediaKind::Movie, MediaKind::Show]
    }

    fn search(q: SearchQuery) -> Result<Vec<Candidate>, String> {
        // A known IMDb id finds the exact title.
        if let Some((_, imdb)) = q.ids.iter().find(|(k, _)| k == "imdb") {
            let v = get(&format!("/find/{}", enc(imdb)), &[("external_source", "imdb_id".into()), ("language", q.language.clone())])?;
            let key = if tv(q.kind) { "tv_results" } else { "movie_results" };
            if let Some(found) = v[key].as_array().filter(|a| !a.is_empty()) {
                return Ok(found.iter().map(|r| candidate(r, q.kind)).collect());
            }
        }
        let (path, year_key) = if tv(q.kind) { ("/search/tv", "first_air_date_year") } else { ("/search/movie", "year") };
        let mut query = vec![("query", q.title.clone()), ("language", q.language.clone()), ("include_adult", "false".to_string())];
        if let Some(y) = q.year {
            query.push((year_key, y.to_string()));
        }
        let v = get(path, &query)?;
        let mut out: Vec<Candidate> = v["results"].as_array().into_iter().flatten().take(10).map(|r| candidate(r, q.kind)).collect();
        // A wrong year in the file name: try again without it.
        if out.is_empty() && q.year.is_some() {
            query.retain(|(k, _)| *k != year_key);
            let v = get(path, &query)?;
            out = v["results"].as_array().into_iter().flatten().take(10).map(|r| candidate(r, q.kind)).collect();
        }
        Ok(out)
    }

    fn details(kind: MediaKind, id: String, language: String) -> Result<ItemDetails, String> {
        if tv(kind) {
            let v = get(&format!("/tv/{}", enc(&id)), &[("language", language), ("append_to_response", "credits,external_ids,content_ratings".into())])?;
            let rating = v["content_ratings"]["results"].as_array().into_iter().flatten().find(|r| r["iso_3166_1"] == "US").and_then(|r| s(r, "rating"));
            let mut ids = vec![("tmdb".to_string(), id)];
            if let Some(i) = s(&v["external_ids"], "imdb_id") {
                ids.push(("imdb".into(), i));
            }
            if let Some(i) = v["external_ids"]["tvdb_id"].as_i64() {
                ids.push(("tvdb".into(), i.to_string()));
            }
            Ok(ItemDetails {
                title: s(&v, "name"),
                original_title: s(&v, "original_name"),
                overview: s(&v, "overview"),
                year: year_of(s(&v, "first_air_date")),
                premiered: s(&v, "first_air_date"),
                runtime_minutes: v["episode_run_time"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).map(|m| m as u32),
                rating: v["vote_average"].as_f64().map(|r| r as f32),
                official_rating: rating,
                genres: genres(&v),
                credits: credits(&v["credits"]),
                artwork: artwork(&v),
                ids,
                collection: None,
            })
        } else {
            let v = get(&format!("/movie/{}", enc(&id)), &[("language", language), ("append_to_response", "credits,release_dates".into())])?;
            let rating = v["release_dates"]["results"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["iso_3166_1"] == "US")
                .and_then(|r| r["release_dates"].as_array().and_then(|d| d.iter().find_map(|x| s(x, "certification"))));
            let mut ids = vec![("tmdb".to_string(), id)];
            if let Some(i) = s(&v, "imdb_id") {
                ids.push(("imdb".into(), i));
            }
            Ok(ItemDetails {
                title: s(&v, "title"),
                original_title: s(&v, "original_title"),
                overview: s(&v, "overview"),
                year: year_of(s(&v, "release_date")),
                premiered: s(&v, "release_date"),
                runtime_minutes: v["runtime"].as_u64().map(|m| m as u32),
                rating: v["vote_average"].as_f64().map(|r| r as f32),
                official_rating: rating,
                genres: genres(&v),
                credits: credits(&v["credits"]),
                artwork: artwork(&v),
                ids,
                collection: s(&v["belongs_to_collection"], "name"),
            })
        }
    }

    fn episodes(show: String, season: u32, language: String) -> Result<Vec<Episode>, String> {
        let v = get(&format!("/tv/{}/season/{season}", enc(&show)), &[("language", language)])?;
        Ok(v["episodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                Some(Episode {
                    season,
                    episode: e["episode_number"].as_u64()? as u32,
                    title: s(e, "name"),
                    overview: s(e, "overview"),
                    aired: s(e, "air_date"),
                    still_url: image(s(e, "still_path"), "w780"),
                    runtime_minutes: e["runtime"].as_u64().map(|m| m as u32),
                })
            })
            .collect())
    }
}

fn candidate(r: &Value, kind: MediaKind) -> Candidate {
    let (title, date) = if tv(kind) { (s(r, "name"), s(r, "first_air_date")) } else { (s(r, "title"), s(r, "release_date")) };
    Candidate {
        id: r["id"].as_i64().map(|i| i.to_string()).unwrap_or_default(),
        title: title.unwrap_or_default(),
        year: year_of(date),
        overview: s(r, "overview"),
        poster_url: image(s(r, "poster_path"), "w342"),
    }
}

fn genres(v: &Value) -> Vec<String> {
    v["genres"].as_array().into_iter().flatten().filter_map(|g| s(g, "name")).collect()
}

fn credits(c: &Value) -> Vec<Credit> {
    let mut out: Vec<Credit> = c["cast"]
        .as_array()
        .into_iter()
        .flatten()
        .take(25)
        .filter_map(|p| {
            Some(Credit { name: s(p, "name")?, role: "actor".into(), character: s(p, "character"), image_url: image(s(p, "profile_path"), "w185") })
        })
        .collect();
    for p in c["crew"].as_array().into_iter().flatten() {
        let role = match p["job"].as_str() {
            Some("Director") => "director",
            Some("Screenplay" | "Writer") => "writer",
            Some("Original Music Composer") => "composer",
            _ => continue,
        };
        if let Some(name) = s(p, "name") {
            out.push(Credit { name, role: role.into(), character: None, image_url: image(s(p, "profile_path"), "w185") });
        }
    }
    out
}

fn artwork(v: &Value) -> Vec<Artwork> {
    let mut out = Vec::new();
    if let Some(u) = image(s(v, "poster_path"), "w780") {
        out.push(Artwork { kind: "poster".into(), url: u, language: None });
    }
    if let Some(u) = image(s(v, "backdrop_path"), "w1280") {
        out.push(Artwork { kind: "backdrop".into(), url: u, language: None });
    }
    out
}

export!(Tmdb);
