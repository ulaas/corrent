//! Seeded random inputs for robustness tests: parsers, converters and API inputs must never
//! panic or hang, whatever they get. Each run starts from valid examples ("seeds") and mutates
//! them, mixed with plain random text. Deterministic per seed; `CORRENT_FUZZ_ITERS` raises the
//! number of cases (the default keeps `cargo test` fast; soak runs use millions).
// A dev-dependency for tests: a failed case must panic with its seed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
}

/// Runs `f` on `n` cases from seeded generators; a panic names the case's index and seed so it
/// can be replayed with `Rng::new(seed)`.
pub fn run(name: &str, n: usize, mut f: impl FnMut(&mut Rng)) {
    for i in 0..n {
        let seed = (i as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407) ^ name.len() as u64;
        let mut rng = Rng::new(seed);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut rng)));
        if let Err(e) = r {
            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
            panic!("{name}: case {i} (seed {seed:#x}) panicked: {}", msg.unwrap_or_default());
        }
    }
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
