//! STABILITY.md Z4: the qBittorrent Web API's input parsing (Host and Origin checks, the session
//! cookie, forms, hash lists), shared by the stable runner below, `fuzz_corpus_replay` and the
//! cargo-fuzz crate in `fuzz/`. Test builds and the `fuzz` feature only.
// Test tooling: a broken invariant panics on purpose.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, header};

/// Every target of this crate, by its corpus folder name.
pub const TARGETS: &[(&str, corrent_core::fuzz::Target)] = &[("z4", z4)];

thread_local! {
    static RT: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
}

pub fn z4(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    let _ = super::host_and_port(&s);
    for lan in [false, true] {
        if super::host_allowed(&s, lan) {
            let h = super::host_of(&s).to_ascii_lowercase();
            assert!(h == "localhost" || h.parse::<std::net::IpAddr>().is_ok(), "{s:?} allowed");
        }
    }
    if let Ok(v) = HeaderValue::from_bytes(data) {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, v);
        let _ = super::sid_from(&headers);
    }
    let _ = super::qbt::parse_hashes(&s);
    // A form: the query string up to the first NUL, the body after it; the first byte picks the
    // content type.
    let (query, body) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    let ctype = match data.first().map_or(0, |b| b % 3) {
        0 => "application/x-www-form-urlencoded",
        1 => "multipart/form-data; boundary=X",
        _ => "text/plain",
    };
    let uri = format!("/api/v2/torrents/add?{}", String::from_utf8_lossy(query));
    let Ok(req) = Request::builder().uri(uri).header(header::CONTENT_TYPE, ctype).body(Body::from(body.to_vec()))
    else {
        return;
    };
    let form = RT.with(|rt| rt.block_on(super::qbt::read_form(req)));
    let _ = super::qbt::truthy(form.get("paused"));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn corpus_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fuzz-corpus")
    }

    fn corpus(target: &str) -> Vec<Vec<u8>> {
        let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(corpus_dir().join(target))
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        out.sort();
        out.into_iter().map(|(_, b)| b).collect()
    }

    #[test]
    fn fuzz_corpus_replay() {
        for (name, f) in TARGETS {
            for b in corpus(name) {
                f(&b);
            }
        }
        // Every corpus folder belongs to a target (a renamed target would stop being replayed).
        let known: Vec<&str> = TARGETS.iter().chain(corrent_core::fuzz::TARGETS).map(|(n, _)| *n).collect();
        for e in std::fs::read_dir(corpus_dir()).unwrap().flatten().filter(|e| e.path().is_dir()) {
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(known.contains(&name.as_str()), "tests/fuzz-corpus/{name} has no target");
        }
    }

    #[test]
    fn z4_web_api_inputs() {
        let seeds = corpus("z4");
        let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
        corrent_fuzz::run_budget("z4", corrent_fuzz::budget(3_000), |r| z4(&r.bytes_case(&seeds, 1024)));
    }
}
