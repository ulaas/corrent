//! OpenSubtitles (opensubtitles.com REST API v1): search by file hash, ids or title; download.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "subtitle-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::subtitles::{Guest, Subtitle, SubtitleFile, SubtitleQuery};
use serde_json::{Value, json};

const API: &str = "https://api.opensubtitles.com/api/v1";
const AGENT: &str = "Corrent v1.0";

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn headers(token: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let key = host::setting("api_key").filter(|k| !k.is_empty()).ok_or("the OpenSubtitles API key is not set")?;
    let mut h = vec![
        ("api-key".to_string(), key),
        ("user-agent".to_string(), AGENT.to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ];
    if let Some(t) = token {
        h.push(("authorization".into(), format!("Bearer {t}")));
    }
    Ok(h)
}

fn call(method: &str, url: String, body: Option<Value>, token: Option<&str>) -> Result<Value, String> {
    let body = body.map(|b| b.to_string().into_bytes());
    let res = host::http(&HttpRequest { method: method.into(), url, headers: headers(token)?, body })?;
    match res.status {
        200 => serde_json::from_slice(&res.body).map_err(|e| e.to_string()),
        401 | 403 => Err("OpenSubtitles refused the key or the sign-in".into()),
        406 => Err("the OpenSubtitles download quota for today is used up".into()),
        429 => Err("OpenSubtitles asks to slow down".into()),
        s => Err(format!("OpenSubtitles answered {s}")),
    }
}

/// A sign-in token when a user name is set (kept for a day).
fn token() -> Option<String> {
    let user = host::setting("username").filter(|u| !u.is_empty())?;
    let pass = host::setting("password").unwrap_or_default();
    if let Some(saved) = host::kv_get("token")
        && let Some((t, who)) = saved.split_once('|')
        && who == user
    {
        return Some(t.to_string());
    }
    let v = call("POST", format!("{API}/login"), Some(json!({ "username": user, "password": pass })), None).ok()?;
    let t = v["token"].as_str()?.to_string();
    let _ = host::kv_set("token", &format!("{t}|{user}"));
    Some(t)
}

struct OpenSubtitles;

impl Guest for OpenSubtitles {
    fn search(q: SubtitleQuery) -> Result<Vec<Subtitle>, String> {
        let mut params = vec![("languages", q.languages.join(","))];
        if let Some(h) = &q.file_hash {
            params.push(("moviehash", h.clone()));
        }
        let id = |k: &str| q.ids.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        if let Some(imdb) = id("imdb") {
            let key = if q.episode.is_some() { "parent_imdb_id" } else { "imdb_id" };
            params.push((key, imdb.trim_start_matches("tt").to_string()));
        } else if let Some(tmdb) = id("tmdb") {
            let key = if q.episode.is_some() { "parent_tmdb_id" } else { "tmdb_id" };
            params.push((key, tmdb));
        } else {
            params.push(("query", q.title.clone()));
            if let Some(y) = q.year {
                params.push(("year", y.to_string()));
            }
        }
        if let Some(s) = q.season {
            params.push(("season_number", s.to_string()));
        }
        if let Some(e) = q.episode {
            params.push(("episode_number", e.to_string()));
        }
        params.sort();
        let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", enc(v))).collect();
        let v = call("GET", format!("{API}/subtitles?{}", query.join("&")), None, None)?;
        let mut out = Vec::new();
        for d in v["data"].as_array().into_iter().flatten() {
            let a = &d["attributes"];
            let Some(file) = a["files"].as_array().and_then(|f| f.first()) else { continue };
            let Some(file_id) = file["file_id"].as_i64() else { continue };
            let name = file["file_name"].as_str().or(a["release"].as_str()).unwrap_or("").to_string();
            let format = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| e.len() <= 4).unwrap_or_else(|| "srt".into());
            out.push(Subtitle {
                id: file_id.to_string(),
                language: a["language"].as_str().unwrap_or("").to_string(),
                name: a["release"].as_str().map(str::to_string).unwrap_or(name),
                format,
                downloads: a["download_count"].as_u64().map(|n| n.min(u32::MAX as u64) as u32),
                hearing_impaired: a["hearing_impaired"].as_bool().unwrap_or(false),
                exact: a["moviehash_match"].as_bool().unwrap_or(false),
            });
        }
        Ok(out)
    }

    fn download(id: String) -> Result<SubtitleFile, String> {
        let file_id: i64 = id.parse().map_err(|_| "unknown subtitle")?;
        let token = token();
        let v = call("POST", format!("{API}/download"), Some(json!({ "file_id": file_id })), token.as_deref())?;
        let link = v["link"].as_str().ok_or("OpenSubtitles gave no download link")?;
        let res = host::http(&HttpRequest { method: "GET".into(), url: link.to_string(), headers: vec![], body: None })?;
        if res.status != 200 {
            return Err(format!("the download answered {}", res.status));
        }
        let format = v["file_name"].as_str().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_else(|| "srt".into());
        Ok(SubtitleFile { format, data: res.body })
    }
}

export!(OpenSubtitles);
