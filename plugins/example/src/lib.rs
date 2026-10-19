//! A test provider: deterministic answers, plus deliberately bad behaviour on request so the
//! host's sandbox can be tested (endless loop, huge allocation, undeclared host).

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "metadata-provider" });

use corrent::plugin::host;
use corrent::plugin::types::HttpRequest;
use exports::corrent::plugin::metadata::{Artwork, Candidate, Credit, Episode, Guest, ItemDetails, MediaKind, SearchQuery};

struct Example;

impl Guest for Example {
    fn kinds() -> Vec<MediaKind> {
        vec![MediaKind::Movie, MediaKind::Show]
    }

    fn search(q: SearchQuery) -> Result<Vec<Candidate>, String> {
        match q.title.as_str() {
            "loop" => {
                let mut x: u64 = 1;
                loop {
                    x = x.wrapping_mul(31).wrapping_add(7);
                    if x == 0 {
                        return Err(String::new());
                    }
                }
            }
            "alloc" => {
                let v = std::hint::black_box(vec![1u8; 512 * 1024 * 1024]);
                Err(format!("allocated {}", std::hint::black_box(&v)[v.len() / 2]))
            }
            "undeclared" => host::http(&HttpRequest { method: "GET".into(), url: "https://evil.example/".into(), headers: vec![], body: None })
                .map(|_| vec![]),
            "local" => host::http(&HttpRequest { method: "GET".into(), url: "http://127.0.0.1:9/".into(), headers: vec![], body: None })
                .map(|_| vec![]),
            "fail" => Err("no such title".into()),
            _ => {
                let n: u32 = host::kv_get("calls").and_then(|v| v.parse().ok()).unwrap_or(0) + 1;
                host::kv_set("calls", &n.to_string())?;
                host::log("info", &format!("search #{n} for {}", q.title));
                Ok(vec![Candidate {
                    id: format!("ex-{}", q.title.to_lowercase().replace(' ', "-")),
                    title: q.title.clone(),
                    year: q.year,
                    overview: Some(format!("call {n}")),
                    poster_url: None,
                }])
            }
        }
    }

    fn details(_kind: MediaKind, id: String, language: String) -> Result<ItemDetails, String> {
        Ok(ItemDetails {
            title: Some(format!("Example {id}")),
            original_title: None,
            overview: Some(format!("{} ({language})", host::setting("greeting").unwrap_or_default())),
            year: Some(2010),
            premiered: Some("2010-09-27".into()),
            runtime_minutes: Some(15),
            rating: Some(7.5),
            official_rating: Some("PG".into()),
            genres: vec!["Animation".into(), "Fantasy".into()],
            credits: vec![Credit { name: "Colin Levy".into(), role: "director".into(), character: None, image_url: None }],
            artwork: vec![Artwork { kind: "poster".into(), url: "https://example.com/poster.jpg".into(), language: None }],
            ids: vec![("example".into(), id)],
            collection: None,
        })
    }

    fn episodes(_show: String, season: u32, _language: String) -> Result<Vec<Episode>, String> {
        // Two seasons of three episodes (an empty season ends the list).
        if season > 2 {
            return Ok(Vec::new());
        }
        Ok((1..=3)
            .map(|e| Episode {
                season,
                episode: e,
                title: Some(format!("Episode title {e}")),
                overview: Some("An episode.".into()),
                aired: Some(format!("2020-01-0{e}")),
                still_url: None,
                runtime_minutes: Some(22),
            })
            .collect())
    }
}

export!(Example);
