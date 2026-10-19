//! TheTVDB (API v4): shows with their seasons and episodes.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "metadata-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::metadata::{Artwork, Candidate, Credit, Episode, Guest, ItemDetails, MediaKind, SearchQuery};
use serde_json::{Value, json};

const API: &str = "https://api4.thetvdb.com/v4";

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A bearer token (valid a month; kept for three weeks).
fn token(fresh: bool) -> Result<String, String> {
    let key = host::setting("api_key").filter(|k| !k.is_empty()).ok_or("the TheTVDB API key is not set")?;
    if !fresh
        && let Some(saved) = host::kv_get("token")
        && let Some((t, k)) = saved.split_once('|')
        && k == key.get(..8).unwrap_or(&key)
    {
        return Ok(t.to_string());
    }
    let mut body = json!({ "apikey": key });
    if let Some(pin) = host::setting("pin").filter(|p| !p.is_empty()) {
        body["pin"] = json!(pin);
    }
    let res = host::http(&HttpRequest {
        method: "POST".into(),
        url: format!("{API}/login"),
        headers: vec![("content-type".into(), "application/json".into())],
        body: Some(body.to_string().into_bytes()),
    })?;
    if res.status != 200 {
        return Err(format!("TheTVDB refused the key ({})", res.status));
    }
    let v: Value = serde_json::from_slice(&res.body).map_err(|e| e.to_string())?;
    let t = v["data"]["token"].as_str().ok_or("no token from TheTVDB")?.to_string();
    let _ = host::kv_set("token", &format!("{t}|{}", key.get(..8).unwrap_or(&key)));
    Ok(t)
}

fn get(path: &str) -> Result<Value, String> {
    for fresh in [false, true] {
        let res = host::http(&HttpRequest {
            method: "GET".into(),
            url: format!("{API}{path}"),
            headers: vec![("authorization".into(), format!("Bearer {}", token(fresh)?)), ("accept".into(), "application/json".into())],
            body: None,
        })?;
        match res.status {
            200 => return serde_json::from_slice::<Value>(&res.body).map(|v| v["data"].clone()).map_err(|e| e.to_string()),
            401 if !fresh => continue,
            404 => return Err("not on TheTVDB".into()),
            s => return Err(format!("TheTVDB answered {s}")),
        }
    }
    Err("TheTVDB refused the key".into())
}

/// TheTVDB uses three-letter language codes.
fn lang3(l: &str) -> &'static str {
    match l.get(..2).unwrap_or("en") {
        "tr" => "tur",
        "de" => "deu",
        "fr" => "fra",
        "es" => "spa",
        "it" => "ita",
        "nl" => "nld",
        "pt" => "por",
        "ru" => "rus",
        "ja" => "jpn",
        "ko" => "kor",
        "zh" => "zho",
        "pl" => "pol",
        "sv" => "swe",
        _ => "eng",
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).filter(|x| !x.is_empty())
}

fn year(v: &Value) -> Option<u32> {
    s(v, "year").and_then(|y| y.parse().ok()).or_else(|| s(v, "firstAired").and_then(|d| d.get(..4)?.parse().ok()))
}

struct Tvdb;

impl Guest for Tvdb {
    fn kinds() -> Vec<MediaKind> {
        vec![MediaKind::Show]
    }

    fn search(q: SearchQuery) -> Result<Vec<Candidate>, String> {
        if q.kind != MediaKind::Show {
            return Ok(vec![]);
        }
        if let Some((_, id)) = q.ids.iter().find(|(k, _)| k == "tvdb") {
            let v = get(&format!("/series/{}", enc(id)))?;
            return Ok(vec![Candidate {
                id: id.clone(),
                title: s(&v, "name").unwrap_or_else(|| q.title.clone()),
                year: year(&v),
                overview: s(&v, "overview"),
                poster_url: s(&v, "image"),
            }]);
        }
        let mut path = format!("/search?type=series&query={}", enc(&q.title));
        if let Some(y) = q.year {
            path.push_str(&format!("&year={y}"));
        }
        let v = get(&path)?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .take(10)
            .filter_map(|r| {
                Some(Candidate {
                    id: s(r, "tvdb_id")?,
                    title: s(r, "name")?,
                    year: year(r),
                    overview: s(r, "overview"),
                    poster_url: s(r, "image_url"),
                })
            })
            .collect())
    }

    fn details(_kind: MediaKind, id: String, language: String) -> Result<ItemDetails, String> {
        let v = get(&format!("/series/{}/extended?meta=translations&short=false", enc(&id)))?;
        let l3 = lang3(&language);
        // Translated name and overview when there are some.
        let tr = |kind: &str| {
            v["translations"][kind].as_array().and_then(|a| a.iter().find(|t| t["language"].as_str() == Some(l3))).cloned()
        };
        let title = tr("nameTranslations").and_then(|t| s(&t, "name")).or_else(|| s(&v, "name"));
        let overview = tr("overviewTranslations").and_then(|t| s(&t, "overview")).or_else(|| s(&v, "overview"));
        let genres = v["genres"].as_array().into_iter().flatten().filter_map(|g| s(g, "name")).collect();
        let credits = v["characters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["peopleType"].as_str() == Some("Actor"))
            .take(30)
            .filter_map(|c| {
                Some(Credit { name: s(c, "personName")?, role: "actor".into(), character: s(c, "name"), image_url: s(c, "personImgURL") })
            })
            .collect();
        // Artwork types: 1 banner, 2 poster, 3 background, 23 clear logo.
        let mut artwork = Vec::new();
        for (t, kind) in [(2, "poster"), (3, "backdrop"), (1, "banner"), (23, "logo")] {
            let best = v["artworks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["type"].as_i64() == Some(t))
                .max_by_key(|a| (a["language"].as_str() == Some(l3), a["score"].as_i64().unwrap_or(0)));
            if let Some(a) = best.and_then(|a| s(a, "image")) {
                artwork.push(Artwork { kind: kind.into(), url: a, language: None });
            }
        }
        let mut ids = vec![("tvdb".to_string(), id.clone())];
        for r in v["remoteIds"].as_array().into_iter().flatten() {
            let key = match r["sourceName"].as_str() {
                Some("IMDB") => "imdb",
                Some("TheMovieDB.com") => "tmdb",
                _ => continue,
            };
            if let Some(i) = s(r, "id") {
                ids.push((key.into(), i));
            }
        }
        Ok(ItemDetails {
            title,
            original_title: s(&v, "name"),
            overview,
            year: year(&v),
            premiered: s(&v, "firstAired"),
            runtime_minutes: v["averageRuntime"].as_u64().map(|m| m as u32),
            // TheTVDB's "score" is popularity, not a rating.
            rating: None,
            official_rating: None,
            genres,
            credits,
            artwork,
            ids,
            collection: None,
        })
    }

    fn episodes(show: String, season: u32, language: String) -> Result<Vec<Episode>, String> {
        let l3 = lang3(&language);
        let mut out = Vec::new();
        for page in 0..10 {
            let v = get(&format!("/series/{}/episodes/default/{l3}?season={season}&page={page}", enc(&show)))?;
            let eps = v["episodes"].as_array().cloned().unwrap_or_default();
            if eps.is_empty() {
                break;
            }
            for e in eps {
                if e["seasonNumber"].as_u64() != Some(season as u64) {
                    continue;
                }
                let Some(n) = e["number"].as_u64() else { continue };
                out.push(Episode {
                    season,
                    episode: n as u32,
                    title: s(&e, "name"),
                    overview: s(&e, "overview"),
                    aired: s(&e, "aired"),
                    still_url: s(&e, "image"),
                    runtime_minutes: e["runtime"].as_u64().map(|m| m as u32),
                });
            }
        }
        Ok(out)
    }
}

export!(Tvdb);
