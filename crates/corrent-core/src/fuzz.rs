//! STABILITY.md workstream Z: fuzz target bodies, one function per target, each taking raw bytes
//! from outside the machine. Three runners share them so they can't drift: the stable seeded runs
//! below (in every `cargo test`, longer with `CORRENT_FUZZ_SECS`, see `scripts/fuzz-long.ps1`),
//! `fuzz_corpus_replay` (every file under `tests/fuzz-corpus/<target>/`), and the cargo-fuzz
//! crate in `fuzz/` (nightly, [O]). A body asserts the target's invariants and must never panic
//! otherwise. Compiled into test builds and the `fuzz` feature only (release hygiene checks).
// Test tooling: a broken invariant panics on purpose.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::HashSet;

use crate::engine::types::AddSource;

/// A target's body.
pub type Target = fn(&[u8]);

/// Every target, by its corpus folder name.
pub const TARGETS: &[(&str, Target)] = &[("z1", z1), ("z6", z6), ("z7", z7)];

/// Z1: torrent bytes (a file, a URL, a feed, the watch folder, an import, the Web API) through
/// the bencode parsers, librqbit's validation and Corrent's path checks. Accepted paths stay inside
/// the save folder and are distinct files on the strictest rules.
pub fn z1(data: &[u8]) {
    use crate::engine::paths::{MAX_COMPONENTS, MAX_PATH_BYTES, Rules, check_torrent_paths, is_reserved};
    let _ = librqbit_bencode::dyn_from_bytes::<librqbit_bencode::ByteBufOwned>(data);
    let _ = crate::import::dict(data);
    let _ = crate::import::parse_torrent(data.to_vec());
    let src = AddSource::Bytes { name: String::new(), bytes: data.to_vec() };
    let _ = (src.info_hash(), src.describe());
    let Ok(t) = librqbit::torrent_from_bytes(data) else { return };
    // The engine checks the sizes before it validates (vendored librqbit, `corrent_sizes`).
    if librqbit::corrent_sizes::check(&t.info.data).is_err() {
        return;
    }
    let Ok(info) = t.info.data.validate() else { return };
    // What the add preview shows.
    let l = info.lengths();
    let _ = (l.total_length(), l.total_pieces(), l.default_piece_length());
    let files: Vec<Vec<String>> = info.iter_file_details().map(|f| f.filename.to_vec()).collect();
    let rules = [
        Rules { windows_names: true, case_insensitive: true },
        Rules { windows_names: false, case_insensitive: false },
        Rules::HOST,
    ];
    for r in rules {
        if check_torrent_paths(files.clone(), r).is_err() {
            continue;
        }
        let mut seen = HashSet::new();
        for f in &files {
            assert!(f.len() <= MAX_COMPONENTS, "{} components", f.len());
            assert!(f.iter().map(|c| c.len() + 1).sum::<usize>() <= MAX_PATH_BYTES, "a long path");
            for c in f {
                assert!(c != ".." && !c.contains(['/', '\\']) && !c.chars().any(char::is_control), "{f:?}");
                if r.windows_names {
                    assert!(!c.contains(':') && !is_reserved(c) && !c.ends_with(['.', ' ']), "{f:?}");
                }
            }
            if r.case_insensitive {
                let key: Vec<String> = f
                    .iter()
                    .map(|c| if r.windows_names { c.trim_end_matches(['.', ' ']) } else { c.as_str() }.to_lowercase())
                    .collect();
                assert!(seen.insert(key), "case-only duplicate {f:?}");
            }
        }
    }
}

/// Z6: what users paste and launches pass on (CLI arguments, IPC lines, the Add dialog, the Web
/// API's `urls`, feed links): classification, display, redaction and the magnet parser.
pub fn z6(data: &[u8]) {
    use crate::engine::types::{magnet_display_name, url_decode};
    let s = String::from_utf8_lossy(data);
    for exists in [false, true] {
        if let Some(src) = AddSource::parse_with(&s, |_| exists) {
            let _ = (src.describe(), src.info_hash());
        }
    }
    let _ = (magnet_display_name(&s), url_decode(&s), crate::util::redact_url(&s));
    if let Ok(m) = librqbit::corrent_sizes::parse_magnet(&s) {
        let n = m.get_select_only().map_or(0, |v| v.len());
        assert!(n as u64 <= librqbit::corrent_sizes::MAX_SELECT_ONLY, "{n} select-only indices");
    }
}

/// A single-file and a multi-file torrent for the import plans.
fn import_torrents() -> [crate::import::Parsed; 2] {
    let single = b"d4:infod6:lengthi5e4:name5:a.mkv12:piece lengthi16384e6:pieces20:AAAAAAAAAAAAAAAAAAAAee";
    let multi =
        b"d4:infod5:filesld6:lengthi5e4:pathl1:aeee4:name1:x12:piece lengthi16384e6:pieces20:AAAAAAAAAAAAAAAAAAAAee";
    [crate::import::parse_torrent(single.to_vec()).unwrap(), crate::import::parse_torrent(multi.to_vec()).unwrap()]
}

/// Z7: another client's resume data (qBittorrent `.fastresume`, Transmission `.resume`) read on
/// import. Only the pure part: the folders it names are never touched here.
pub fn z7(data: &[u8]) {
    let Some(d) = crate::import::dict(data) else { return };
    for t in &import_torrents() {
        let _ = crate::import::qbt_plan(t, Some(&d));
    }
    let plan = crate::import::transmission_plan(Some(&d));
    assert!(plan.candidates.len() <= 2);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn corpus_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fuzz-corpus")
    }

    /// A target's corpus files: its seeds and every input that once failed.
    fn corpus(target: &str) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(corpus_dir().join(target)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().is_file() && name != "README.md" {
                    out.push((name, std::fs::read(e.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// On a 2 MiB stack, like the tokio threads where Corrent parses these.
    fn on_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new().stack_size(2 << 20).spawn(f).unwrap().join().unwrap();
    }

    fn target(name: &str) -> Target {
        TARGETS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f).unwrap()
    }

    /// Seeded mutations of the target's corpus within the budget (`CORRENT_FUZZ_SECS` for long runs).
    fn fuzz(name: &'static str, default_cases: usize, max_noise: usize) {
        let seeds = corpus(name);
        assert!(!seeds.is_empty(), "no seeds in tests/fuzz-corpus/{name}");
        let f = target(name);
        on_stack(move || {
            let seeds: Vec<&[u8]> = seeds.iter().map(|(_, b)| b.as_slice()).collect();
            corrent_fuzz::run_budget(
                name,
                corrent_fuzz::budget(default_cases),
                |r| f(&r.bytes_case(&seeds, max_noise)),
            );
        });
    }

    #[test]
    fn fuzz_corpus_replay() {
        let mut failed = Vec::new();
        for &(name, f) in TARGETS {
            for (file, bytes) in corpus(name) {
                let r = std::thread::Builder::new().stack_size(2 << 20).spawn(move || f(&bytes)).unwrap().join();
                if r.is_err() {
                    failed.push(format!("tests/fuzz-corpus/{name}/{file}"));
                }
            }
        }
        assert!(failed.is_empty(), "these fail again: {failed:#?}");
    }

    #[test]
    fn z1_torrent_bytes() {
        fuzz("z1", 3_000, 4096);
    }

    #[test]
    fn z6_magnets_urls_and_launch_items() {
        fuzz("z6", 3_000, 512);
    }

    #[test]
    fn z7_import_resume_data() {
        fuzz("z7", 3_000, 2048);
    }
}
