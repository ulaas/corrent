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
    // And through a torrent's typed parse: an unknown key holding nearly the limit.
    let mut torrent = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:".to_vec();
    torrent.extend_from_slice(&[7u8; 20]);
    torrent.extend_from_slice(b"5:extra");
    torrent.extend_from_slice(&lists(MAX_DEPTH - 2));
    torrent.extend_from_slice(b"ee");
    on_stack(1 << 20, move || {
        assert!(librqbit::torrent_from_bytes(&torrent).is_ok());
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
