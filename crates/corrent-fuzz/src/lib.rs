//! Seeded random inputs for robustness tests: parsers, converters and API inputs must never
//! panic or hang, whatever they get. Each run starts from valid examples ("seeds") and mutates
//! them, mixed with plain random text. Deterministic per seed; `CORRENT_FUZZ_ITERS` raises the
//! number of cases (the default keeps `cargo test` fast; soak runs use millions), and
//! `CORRENT_FUZZ_SECS` runs [`run_budget`] targets for that long instead (`scripts/fuzz-long.ps1`).
// A dev-dependency for tests: a failed case must panic with its seed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::time::{Duration, Instant};

/// Cases per target: `CORRENT_FUZZ_ITERS` or `default`.
pub fn iterations(default: usize) -> usize {
    std::env::var("CORRENT_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// xorshift64*: tiny, fast, reproducible.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A character from a mix that finds bugs: separators, digits, brackets, control
    /// characters, combining marks, emoji, right-to-left text.
    pub fn char(&mut self) -> char {
        const SPECIAL: &[char] = &[
            '.', '-', '_', ' ', '[', ']', '(', ')', '{', '}', ':', ',', '\n', '\r', '\t', '\0', '\\', '/', '"', '\'',
            '#', '%', '&', '<', '>', '=', '@', 'é', 'İ', 'ı', 'ß', 'ﬁ', '\u{301}', '\u{200B}', '\u{202E}', '日', 'の',
            '😀', '\u{FEFF}', '\u{7F}', '×', '…', '→',
        ];
        match self.below(10) {
            0..=3 => (b'a' + self.below(26) as u8) as char,
            4 => (b'A' + self.below(26) as u8) as char,
            5 | 6 => (b'0' + self.below(10) as u8) as char,
            _ => *self.pick(SPECIAL),
        }
    }

    pub fn text(&mut self, max_len: usize) -> String {
        let n = self.below(max_len + 1);
        (0..n).map(|_| self.char()).collect()
    }

    /// `seed` with a few random edits (insert, delete, replace, duplicate a slice, truncate).
    pub fn mutate(&mut self, seed: &str) -> String {
        let mut chars: Vec<char> = seed.chars().collect();
        for _ in 0..=self.below(4) {
            let len = chars.len();
            match self.below(6) {
                0 => {
                    let at = self.below(len + 1);
                    let c = self.char();
                    chars.insert(at, c);
                }
                1 if len > 0 => {
                    chars.remove(self.below(len));
                }
                2 if len > 0 => {
                    let at = self.below(len);
                    chars[at] = self.char();
                }
                3 if len > 0 => {
                    let a = self.below(len);
                    let b = (a + self.below(16)).min(len);
                    let slice: Vec<char> = chars[a..b].to_vec();
                    let at = self.below(chars.len() + 1);
                    chars.splice(at..at, slice);
                }
                4 => chars.truncate(self.below(len + 1)),
                _ => {
                    let at = self.below(len + 1);
                    let digits: Vec<char> = self.next_u64().to_string().chars().take(1 + self.below(12)).collect();
                    chars.splice(at..at, digits);
                }
            }
        }
        chars.into_iter().collect()
    }

    /// A case for a text target: mostly mutated seeds, sometimes pure noise.
    pub fn case(&mut self, seeds: &[&str], max_noise: usize) -> String {
        if seeds.is_empty() || self.chance(20) {
            return self.text(max_noise);
        }
        let seed = *self.pick(seeds);
        self.mutate(seed)
    }

    pub fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let n = self.below(max_len + 1);
        (0..n).map(|_| self.next_u64() as u8).collect()
    }

    /// Byte-level edits that matter for bencode and binary protocols: digit runs (lengths and
    /// integers that overflow), duplicated slices (extra nesting), stray structure bytes,
    /// deletions and truncation.
    pub fn mutate_bytes(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut b = seed.to_vec();
        for _ in 0..=self.below(4) {
            let len = b.len();
            match self.below(7) {
                0 => {
                    let at = self.below(len + 1);
                    let digits = self.next_u64().to_string().repeat(1 + self.below(3));
                    b.splice(at..at, digits.into_bytes());
                }
                1 if len > 0 => {
                    b.remove(self.below(len));
                }
                2 if len > 0 => {
                    let at = self.below(len);
                    b[at] = *self.pick(b"lde:i-0123456789");
                }
                3 if len > 0 => {
                    let a = self.below(len);
                    let end = (a + self.below(64)).min(len);
                    let slice = b[a..end].to_vec();
                    for _ in 0..=self.below(50) {
                        let at = self.below(b.len() + 1);
                        b.splice(at..at, slice.iter().copied());
                    }
                }
                4 => b.truncate(self.below(len + 1)),
                5 if len > 0 => {
                    // Any byte, anywhere (binary lengths and ids).
                    let at = self.below(len);
                    b[at] = self.next_u64() as u8;
                }
                _ => {
                    let at = self.below(len + 1);
                    let n = self.below(2000);
                    b.splice(at..at, std::iter::repeat_n(*self.pick(b"ld"), n));
                }
            }
        }
        b
    }

    /// A case for a byte target: mostly mutated seeds, sometimes pure noise.
    pub fn bytes_case(&mut self, seeds: &[&[u8]], max_noise: usize) -> Vec<u8> {
        if seeds.is_empty() || self.chance(10) {
            return self.bytes(max_noise);
        }
        let seed = *self.pick(seeds);
        self.mutate_bytes(seed)
    }
}

/// How many cases a [`run_budget`] target gets: a count (`CORRENT_FUZZ_ITERS` or the default),
/// or a wall-clock time (`CORRENT_FUZZ_SECS`, for long runs).
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub cases: usize,
    pub time: Option<Duration>,
}

pub fn budget(default: usize) -> Budget {
    match std::env::var("CORRENT_FUZZ_SECS").ok().and_then(|v| v.parse::<u64>().ok()) {
        Some(secs) => Budget { cases: usize::MAX, time: Some(Duration::from_secs(secs)) },
        None => Budget { cases: iterations(default), time: None },
    }
}

fn case_seed(name: &str, i: usize) -> u64 {
    (i as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407) ^ name.len() as u64
}

fn one(name: &str, i: usize, f: &mut impl FnMut(&mut Rng)) {
    let seed = case_seed(name, i);
    let mut rng = Rng::new(seed);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut rng)));
    if let Err(e) = r {
        let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
        panic!("{name}: case {i} (seed {seed:#x}) panicked: {}", msg.unwrap_or_default());
    }
}

/// Runs `f` on `n` cases from seeded generators; a panic names the case's index and seed so it
/// can be replayed with `Rng::new(seed)`.
pub fn run(name: &str, n: usize, mut f: impl FnMut(&mut Rng)) {
    for i in 0..n {
        one(name, i, &mut f);
    }
}

/// [`run`] within a [`Budget`]. Returns the number of cases run; a timed run prints it (with the
/// rate) for the long-run table. Replay a failure by its seed: a timed run's index depends on speed.
pub fn run_budget(name: &str, budget: Budget, mut f: impl FnMut(&mut Rng)) -> usize {
    let start = Instant::now();
    let mut i = 0;
    while i < budget.cases {
        if let Some(t) = budget.time
            && i % 64 == 0
            && start.elapsed() >= t
        {
            break;
        }
        one(name, i, &mut f);
        i += 1;
    }
    if budget.time.is_some() {
        let secs = start.elapsed().as_secs_f64();
        println!("FUZZ {name}: {i} cases in {secs:.0} s ({:.0}/s)", i as f64 / secs.max(0.001));
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_varied() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        let xa: Vec<u64> = (0..10).map(|_| a.next_u64()).collect();
        let xb: Vec<u64> = (0..10).map(|_| b.next_u64()).collect();
        assert_eq!(xa, xb);
        let mut r = Rng::new(3);
        let m: std::collections::HashSet<String> = (0..50).map(|_| r.mutate("Show.S01E02.1080p")).collect();
        assert!(m.len() > 30);
    }
}
