//! Untrusted bencode (STABILITY.md E2): the vendored librqbit-bencode bounds nesting and checks
//! number sizes for every parser in the process (torrents, ut_metadata, the DHT, peer and tracker
//! messages, imports). Its own tests don't run in the gate, so they live here.

use librqbit_bencode::{ByteBufOwned, MAX_DEPTH, dyn_from_bytes, from_bytes};

/// `n` nested lists, closed.
fn lists(n: usize) -> Vec<u8> {
    let mut b = vec![b'l'; n];
    b.extend(std::iter::repeat_n(b'e', n));
    b
}

/// Run `f` on a thread with `stack` bytes of stack (the Windows UI thread has 1 MiB).
fn on_stack<T: Send + 'static>(stack: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(stack).spawn(f).unwrap().join().unwrap()
}

#[test]
fn deep_nesting_is_refused_instead_of_overflowing_the_stack() {
    // Before the patch each of these overflowed a 2 MiB stack and aborted the process.
    let deep = vec![b'l'; 10_000_000];
    on_stack(2 << 20, move || {
        assert!(librqbit::torrent_from_bytes(&deep).is_err(), "a .torrent");
        assert!(dyn_from_bytes::<ByteBufOwned>(&deep).is_err(), "a dynamic value");
        // Unknown keys of a known struct are skipped through the same recursion.
        let mut info = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces0:5:extra".to_vec();
        info.extend_from_slice(&deep[..100_000]);
        assert!(librqbit::torrent_from_bytes(&info).is_err(), "an unknown key");
    });
}

#[test]
fn the_deepest_accepted_value_fits_a_small_stack() {
    // At the limit, with the debug build's big frames, on the smallest stack Corrent runs on.
    let ok = lists(MAX_DEPTH);
    let too_deep = lists(MAX_DEPTH + 1);
    on_stack(1 << 20, move || {
        assert!(dyn_from_bytes::<ByteBufOwned>(&ok).is_ok());
        assert!(dyn_from_bytes::<ByteBufOwned>(&too_deep).is_err());
        let msg = format!("{:#}", dyn_from_bytes::<ByteBufOwned>(&too_deep).unwrap_err());
        assert!(msg.contains("nested more than"), "{msg}");
    });
}

#[test]
fn real_torrents_nest_far_below_the_limit() {
    // A hybrid (v1 + v2) torrent's "file tree" nests two levels per folder: 100 folders deep is
    // still accepted (librqbit uses the v1 part).
    let mut tree = Vec::new();
    for _ in 0..100 {
        tree.extend_from_slice(b"d3:dir");
    }
    tree.extend_from_slice(b"d0:d6:lengthi1eee");
    tree.extend(std::iter::repeat_n(b'e', 100));
    let mut t = b"d4:infod9:file tree".to_vec();
    t.extend_from_slice(&tree);
    t.extend_from_slice(b"6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:");
    t.extend_from_slice(&[7u8; 20]);
    t.extend_from_slice(b"ee");
    assert!(librqbit::torrent_from_bytes(&t).is_ok(), "{:?}", librqbit::torrent_from_bytes(&t).err());
}

#[test]
fn numbers_too_big_for_their_type_are_invalid_not_a_panic() {
    // A string length or an integer past u64 used to overflow (a panic with overflow checks, a
    // wrong length without).
    for bad in [&b"99999999999999999999999:x"[..], b"i99999999999999999999999e", b"l99999999999999999999999:xe"] {
        assert!(dyn_from_bytes::<ByteBufOwned>(bad).is_err(), "{}", String::from_utf8_lossy(bad));
    }
    #[derive(serde::Deserialize, Debug)]
    struct Small {
        #[allow(dead_code)]
        n: u8,
    }
    assert!(from_bytes::<Small>(b"d1:ni300ee").is_err(), "300 doesn't fit a u8");
    assert!(from_bytes::<Small>(b"d1:ni200ee").is_ok());
}

/// A small valid multi-file torrent to mutate.
fn seed_torrent() -> Vec<u8> {
    let mut b = b"d8:announce20:http://t.example/ann4:infod5:filesld6:lengthi16384e4:pathl3:dir5:a.txteed6:lengthi5e4:pathl5:b.mkveee4:name4:seed12:piece lengthi16384e6:pieces40:".to_vec();
    b.extend_from_slice(&[7u8; 40]);
    b.extend_from_slice(b"ee");
    b
}

/// Byte-level edits that matter for bencode: digit runs (lengths and integers that overflow),
/// duplicated slices (extra nesting), stray structure bytes, deletions and truncation.
fn mutate_bytes(r: &mut corrent_fuzz::Rng, seed: &[u8]) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..=r.below(4) {
        let len = b.len();
        match r.below(6) {
            0 => {
                let at = r.below(len + 1);
                let digits = r.next_u64().to_string().repeat(1 + r.below(3));
                b.splice(at..at, digits.into_bytes());
            }
            1 if len > 0 => {
                b.remove(r.below(len));
            }
            2 if len > 0 => {
                let at = r.below(len);
                b[at] = *r.pick(b"lde:i-0123456789");
            }
            3 if len > 0 => {
                let a = r.below(len);
                let end = (a + r.below(64)).min(len);
                let slice = b[a..end].to_vec();
                for _ in 0..=r.below(50) {
                    let at = r.below(b.len() + 1);
                    b.splice(at..at, slice.iter().copied());
                }
            }
            4 => b.truncate(r.below(len + 1)),
            _ => {
                let at = r.below(len + 1);
                let n = r.below(2000);
                b.splice(at..at, std::iter::repeat_n(*r.pick(b"ld"), n));
            }
        }
    }
    b
}

#[test]
fn torrent_bytes_never_panic_or_escape() {
    use crate::engine::paths::{Rules, check_torrent_paths};
    let seed = seed_torrent();
    assert!(librqbit::torrent_from_bytes(&seed).is_ok(), "the seed is valid");
    // A 2 MiB stack, like the tokio threads where Corrent parses torrents.
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            corrent_fuzz::run("torrent_bytes", corrent_fuzz::iterations(3_000), |r| {
                let bytes = if r.chance(10) { r.bytes(4096) } else { mutate_bytes(r, &seed) };
                if let Ok(t) = librqbit::torrent_from_bytes(&bytes)
                    && let Ok(info) = t.info.data.validate()
                {
                    let files: Vec<Vec<String>> = info.iter_file_details().map(|f| f.filename.to_vec()).collect();
                    for strict in [Rules { windows_names: true, case_insensitive: true }, Rules::HOST] {
                        if check_torrent_paths(files.clone(), strict).is_ok() {
                            // Accepted paths stay inside the save folder.
                            for f in &files {
                                assert!(f.iter().all(|c| c != ".." && !c.contains(['/', '\\'])), "{f:?}");
                            }
                        }
                    }
                }
                let _ = librqbit_bencode::dyn_from_bytes::<ByteBufOwned>(&bytes);
                let _ = crate::import::dict(&bytes);
                let _ = crate::import::parse_torrent(bytes);
            });
        })
        .unwrap()
        .join()
        .unwrap();
}
