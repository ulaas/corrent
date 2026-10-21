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
pub const TARGETS: &[(&str, Target)] =
    &[("z1", z1), ("z2_peer", z2_peer), ("z2_dht", z2_dht), ("z3", z3), ("z5", z5), ("z6", z6), ("z7", z7)];

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

/// Z2: what a peer sends: the handshake, wire messages (split anywhere between the two halves
/// of the receive buffer), extended messages (handshake, ut_metadata, ut_pex) and the torrent info
/// assembled from ut_metadata. What parses must serialize back without a panic.
pub fn z2_peer(data: &[u8]) {
    use librqbit_bencode::{ByteBuf, from_bytes};
    use librqbit_peer_protocol::extended::{
        ExtendedMessage, PeerExtendedMessageIds, handshake::ExtendedHandshake, ut_metadata::UtMetadata, ut_pex::UtPex,
    };
    use librqbit_peer_protocol::{DoubleBufHelper, Handshake, Message};
    let split = data.first().map_or(0, |&b| usize::from(b)).min(data.len());
    let (a, b) = data.split_at(split);
    // Mutations mostly break the length prefix: also try the message with it fixed up.
    let mut framed = data.to_vec();
    if let Some(prefix) = framed.get_mut(..4) {
        prefix.copy_from_slice(&u32::try_from(data.len() - 4).unwrap_or(u32::MAX).to_be_bytes());
    }
    let ids = || PeerExtendedMessageIds { ut_metadata: Some(3), ut_pex: Some(1) };
    for (x, y) in [(data, &[][..]), (a, b), (&framed[..], &[][..])] {
        if let Ok((m, _)) = Message::deserialize(x, y) {
            let mut out = vec![0u8; data.len() + 64];
            let _ = m.serialize(&mut out, &ids);
        }
        let _ = ExtendedMessage::<ByteBuf>::deserialize(DoubleBufHelper::new(x, y));
        let _ = UtMetadata::<ByteBuf>::deserialize(DoubleBufHelper::new(x, y));
    }
    let _ = Handshake::deserialize(data);
    let _ = from_bytes::<ExtendedHandshake<ByteBuf>>(data);
    let _ = from_bytes::<UtPex<ByteBuf>>(data);
    // The info dictionary of a magnet, as librqbit checks it after ut_metadata.
    if let Ok(info) = from_bytes::<librqbit_core::torrent_metainfo::TorrentMetaV1Info<ByteBuf>>(data)
        && librqbit::corrent_sizes::check(&info).is_ok()
    {
        let _ = info.validate();
    }
}

/// Z2: a DHT packet from anyone on the internet. What parses must serialize back.
pub fn z2_dht(data: &[u8]) {
    use librqbit_bencode::{ByteBuf, ByteBufOwned};
    use librqbit_dht::bprotocol::{deserialize_message, serialize_message};
    // The DHT worker parses into owned buffers.
    if let Ok(m) = deserialize_message::<ByteBufOwned>(data) {
        let _ = m.get_our_transaction_id();
    }
    if let Ok(m) = deserialize_message::<ByteBuf>(data) {
        let mut out = Vec::new();
        let _ = serialize_message(&mut out, m.transaction_id, m.version, m.ip, m.kind);
    }
}

/// Z3: the head of a request to the streaming server (any local program, or the LAN when it is
/// shared). The bytes sent are always inside the file.
pub fn z3(data: &[u8]) {
    use crate::engine::stream::{parse_path, parse_range, parse_request, plan_range, printable};
    let head = String::from_utf8_lossy(data);
    let _ = parse_range(&head);
    let Some(req) = parse_request(&head) else { return };
    let _ = parse_path(&req.path, "0123456789abcdef");
    if let Some(ua) = &req.user_agent {
        assert!(ua.chars().count() <= 200 && ua.chars().all(printable), "{ua:?}");
    }
    let from_input = data.get(..8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()));
    for len in [0, 1, 7, 65_536, u64::MAX, from_input] {
        match plan_range(req.range, len) {
            None => assert!(req.range.is_some(), "416 without a range"),
            Some((start, end, _)) if len == 0 => assert!(req.range.is_none() && start == 0 && end == 0),
            Some((start, end, _)) => {
                assert!(start <= end && end < len, "{start}-{end} of {len}");
                assert!((end - start).checked_add(1).is_some());
            }
        }
    }
}

/// Z5: feeds and Torznab answers from the network (`rss::feed::parse`, `search::parse`).
pub fn z5(data: &[u8]) {
    let _ = crate::rss::feed::parse(data, "https://example.invalid/feed");
    let _ = crate::search::parse(data, "p");
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
            corrent_fuzz::run_budget(name, corrent_fuzz::budget(default_cases), |r| {
                let input = r.bytes_case(&seeds, max_noise);
                if let Err(e) = std::panic::catch_unwind(|| f(&input)) {
                    // Kept for the corpus (tests/fuzz-corpus/README.md).
                    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fuzz-failures");
                    let _ = std::fs::create_dir_all(&dir);
                    let file = dir.join(format!(
                        "{name}-{:016x}",
                        corrent_fuzz::Rng::new(input.len() as u64 + 1).next_u64() ^ fnv(&input)
                    ));
                    let _ = std::fs::write(&file, &input);
                    eprintln!("{name}: failing input saved to {}", file.display());
                    std::panic::resume_unwind(e);
                }
            });
        });
    }

    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3))
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

    /// The mutations start from valid inputs: every seed parses.
    #[test]
    fn seeds_are_valid() {
        use librqbit_bencode::ByteBuf;
        for (f, b) in corpus("z1").iter().filter(|(f, _)| f.starts_with("seed-")) {
            assert!(librqbit::torrent_from_bytes(b).is_ok(), "z1/{f}");
        }
        for (f, b) in corpus("z2_dht").iter().filter(|(f, _)| f.starts_with("seed-")) {
            let r = librqbit_dht::bprotocol::deserialize_message::<ByteBuf>(b);
            assert!(r.is_ok(), "z2_dht/{f}: {r:?}");
        }
        for (f, b) in corpus("z2_peer")
            .iter()
            .filter(|(f, _)| f.starts_with("seed-") && !f.contains("handshake") && !f.contains("info"))
        {
            let r = librqbit_peer_protocol::Message::deserialize(b, &[]);
            assert!(r.is_ok(), "z2_peer/{f}: {r:?}");
        }
        let hs = corpus("z2_peer").into_iter().find(|(f, _)| f == "seed-handshake").unwrap().1;
        assert!(librqbit_peer_protocol::Handshake::deserialize(&hs).is_ok());
        for (f, b) in corpus("z5").iter().filter(|(f, _)| f.starts_with("seed-") && !f.contains("error")) {
            assert!(
                crate::rss::feed::parse(b, "https://e.org/").is_ok() || crate::search::parse(b, "p").is_ok(),
                "z5/{f}"
            );
        }
        for (f, b) in corpus("z7") {
            assert!(crate::import::dict(&b).is_some(), "z7/{f}");
        }
    }

    #[test]
    fn z1_torrent_bytes() {
        fuzz("z1", 3_000, 4096);
    }

    #[test]
    fn z2_peer_wire_messages() {
        fuzz("z2_peer", 5_000, 2048);
    }

    #[test]
    fn z2_dht_packets() {
        fuzz("z2_dht", 5_000, 1500);
    }

    #[test]
    fn z3_stream_request_heads() {
        fuzz("z3", 5_000, 1024);
    }

    #[test]
    fn z5_feeds_and_torznab() {
        fuzz("z5", 2_000, 8192);
    }

    /// Feeds of `mib` MiB three ways: many items, one huge title, deep unknown elements.
    fn big_feeds(mib: usize) -> Vec<(&'static str, Vec<u8>)> {
        let size = mib << 20;
        let head = "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>t</title>";
        let tail = "</channel></rss>";
        let item = "<item><title>Sintel 1080p</title><guid>g</guid><enclosure url=\"https://e.org/a.torrent\" length=\"5\"/></item>";
        let items = format!("{head}{}{tail}", item.repeat(size / item.len()));
        let title = format!("{head}<item><title>{}</title></item>{tail}", "x".repeat(size));
        let n = size / 7;
        let deep = format!("{head}<item>{}{}</item>{tail}", "<a><b>".repeat(n / 2), "</b></a>".repeat(n / 2));
        vec![("items", items.into_bytes()), ("title", title.into_bytes()), ("deep", deep.into_bytes())]
    }

    fn feeds_parse_in_time(mib: usize, limit: std::time::Duration) {
        for (what, feed) in big_feeds(mib) {
            let t = std::time::Instant::now();
            z5(&feed);
            assert!(t.elapsed() < limit, "{what}: {mib} MiB took {:?}", t.elapsed());
        }
    }

    #[test]
    fn z5_a_big_feed_parses_in_time() {
        // Only catches blow-ups here (a loaded gate, a debug build); the 8 MiB check below runs
        // optimized in the long run.
        feeds_parse_in_time(1, std::time::Duration::from_secs(if cfg!(debug_assertions) { 15 } else { 1 }));
    }

    /// STABILITY.md Z5: an 8 MiB feed (the download cap) parses in under a second (optimized).
    #[test]
    #[ignore = "long run (scripts/fuzz-long.ps1)"]
    fn z5_an_8_mib_feed_parses_in_a_second() {
        feeds_parse_in_time(8, std::time::Duration::from_secs(1));
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
