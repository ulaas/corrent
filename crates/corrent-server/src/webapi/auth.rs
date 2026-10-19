//! Password hashing, session ids and login throttling for the web API.

use std::collections::HashMap;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use aws_lc_rs::{pbkdf2, rand};

const ITERATIONS: u32 = 100_000;
const SESSION_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_FAILS: u32 = 5;
const BAN: Duration = Duration::from_secs(5 * 60);

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

use crate::util::random_hex;

/// `pbkdf2-sha256$<iterations>$<salt>$<hash>` (hex).
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    rand::fill(&mut salt).expect("OS random generator");
    let mut out = [0u8; 32];
    let iters = NonZeroU32::new(ITERATIONS).expect("non-zero");
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, iters, &salt, password.as_bytes(), &mut out);
    format!("pbkdf2-sha256${ITERATIONS}${}${}", hex(&salt), hex(&out))
}

pub fn verify_password(stored: &str, password: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some("pbkdf2-sha256"), Some(iters), Some(salt), Some(hash), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (Some(iters), Some(salt), Some(hash)) =
        (iters.parse::<u32>().ok().and_then(NonZeroU32::new), unhex(salt), unhex(hash))
    else {
        return false;
    };
    pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256, iters, &salt, password.as_bytes(), &hash).is_ok()
}

#[derive(Default)]
pub struct Sessions {
    sids: HashMap<String, Instant>,
    fails: HashMap<IpAddr, (u32, Instant)>,
}

impl Sessions {
    pub fn banned(&mut self, ip: IpAddr) -> bool {
        match self.fails.get(&ip) {
            Some((n, at)) if *n >= MAX_FAILS && at.elapsed() < BAN => true,
            Some((_, at)) if at.elapsed() >= BAN => {
                self.fails.remove(&ip);
                false
            }
            _ => false,
        }
    }

    pub fn failed(&mut self, ip: IpAddr) {
        self.fails.retain(|_, (_, at)| at.elapsed() < BAN);
        let e = self.fails.entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
    }

    pub fn create(&mut self, ip: IpAddr) -> String {
        self.fails.remove(&ip);
        self.sids.retain(|_, at| at.elapsed() < SESSION_TTL);
        let sid = random_hex(16);
        self.sids.insert(sid.clone(), Instant::now());
        sid
    }

    /// Valid sessions are refreshed on use.
    pub fn check(&mut self, sid: &str) -> bool {
        match self.sids.get_mut(sid) {
            Some(at) if at.elapsed() < SESSION_TTL => {
                *at = Instant::now();
                true
            }
            Some(_) => {
                self.sids.remove(sid);
                false
            }
            None => false,
        }
    }

    pub fn remove(&mut self, sid: &str) {
        self.sids.remove(sid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("hunter2");
        assert!(h.starts_with("pbkdf2-sha256$100000$"));
        assert!(verify_password(&h, "hunter2"));
        assert!(!verify_password(&h, "hunter3"));
        assert!(!verify_password("", "x"));
        assert!(!verify_password("plain", "plain"));
        assert_ne!(hash_password("a"), hash_password("a"), "salted");
    }

    #[test]
    fn throttles_and_expires() {
        let mut s = Sessions::default();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..MAX_FAILS {
            assert!(!s.banned(ip));
            s.failed(ip);
        }
        assert!(s.banned(ip));
        let sid = s.create(ip);
        assert!(!s.banned(ip), "a successful login clears failures");
        assert!(s.check(&sid));
        assert!(!s.check("nope"));
        s.remove(&sid);
        assert!(!s.check(&sid));
    }
}
