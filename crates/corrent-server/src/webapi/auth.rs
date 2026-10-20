//! Password hashing and session ids for the web API.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use aws_lc_rs::{pbkdf2, rand};

const ITERATIONS: u32 = 100_000;
const ITERS: NonZeroU32 = NonZeroU32::new(ITERATIONS).unwrap();
const SESSION_TTL: Duration = Duration::from_secs(60 * 60);

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
    #[expect(clippy::expect_used, reason = "the OS generator doesn't fail; nothing is safe without it")]
    rand::fill(&mut salt).expect("OS random generator");
    let mut out = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, ITERS, &salt, password.as_bytes(), &mut out);
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
}

impl Sessions {
    pub fn create(&mut self) -> String {
        self.sids.retain(|_, at| at.elapsed() < SESSION_TTL);
        // Logins only add sessions; past the cap the least recently used one goes.
        while self.sids.len() >= MAX_SESSIONS {
            let Some(oldest) = self.sids.iter().min_by_key(|(_, at)| **at).map(|(k, _)| k.clone()) else { break };
            self.sids.remove(&oldest);
        }
        let sid = random_hex(16);
        self.sids.insert(sid.clone(), Instant::now());
        sid
    }

    /// Valid sessions are refreshed on use.
    pub fn check(&mut self, sid: &str) -> bool {
        // Expired sessions go here too, not only when someone logs in.
        self.sids.retain(|_, at| at.elapsed() < SESSION_TTL);
        match self.sids.get_mut(sid) {
            Some(at) => {
                *at = Instant::now();
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, sid: &str) {
        self.sids.remove(sid);
    }
}

/// Sessions kept at once (each login adds one; clients reuse theirs).
const MAX_SESSIONS: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_bounded() {
        let mut s = Sessions::default();
        let first = s.create();
        let mut sids = vec![first.clone()];
        for _ in 0..300 {
            sids.push(s.create());
            // Keep the first one in use: it is recent, so it survives the cap.
            assert!(s.check(&first));
        }
        assert!(s.sids.len() <= MAX_SESSIONS);
        assert!(s.check(&first), "a session in use stays");
        assert!(s.check(sids.last().unwrap()));
        assert!(!s.check(&sids[1]), "the least recently used went");
    }

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
    fn sessions_check_and_end() {
        let mut s = Sessions::default();
        let sid = s.create();
        assert!(s.check(&sid));
        assert!(!s.check("nope"));
        s.remove(&sid);
        assert!(!s.check(&sid));
    }
}
