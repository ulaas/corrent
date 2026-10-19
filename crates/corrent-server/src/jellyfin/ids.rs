//! Jellyfin's ids are 32-hex GUIDs; ours are numbers of a few kinds. The codec is reversible:
//! 16 bytes = kind ‖ seven zero bytes ‖ the number big-endian. Ticks are 100 ns.

pub const ITEM: u8 = 1;
pub const USER: u8 = 2;
/// A library (Jellyfin "view"); number 0 is the user's root folder.
pub const VIEW: u8 = 3;
/// A media file (`MediaSourceId`).
pub const FILE: u8 = 4;
pub const PERSON: u8 = 5;
pub const GENRE: u8 = 6;

pub fn enc(kind: u8, id: i64) -> String {
    format!("{kind:02x}00000000000000{:016x}", id as u64)
}

/// Dashed or not, any case; `None` for anything that isn't one of ours.
pub fn dec(s: &str) -> Option<(u8, i64)> {
    let h: String = s.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if h.len() != 32 || !h.bytes().all(|b| b.is_ascii_hexdigit()) || &h[2..16] != "00000000000000" {
        return None;
    }
    let kind = u8::from_str_radix(&h[..2], 16).ok()?;
    let id = u64::from_str_radix(&h[16..], 16).ok()? as i64;
    Some((kind, id))
}

/// One of ours of this kind.
pub fn dec_kind(s: &str, kind: u8) -> Option<i64> {
    dec(s).filter(|(k, _)| *k == kind).map(|(_, i)| i)
}

/// The server id (24 hex) as a GUID.
pub fn server(id: &str) -> String {
    let mut h: String = id.chars().filter(|c| c.is_ascii_hexdigit()).take(32).collect::<String>().to_ascii_lowercase();
    while h.len() < 32 {
        h.push('0');
    }
    h
}

pub fn ticks(ms: i64) -> i64 {
    ms.saturating_mul(10_000)
}

pub fn ms(ticks: i64) -> i64 {
    ticks / 10_000
}

/// Unix seconds → `2026-10-19T12:00:00.0000000Z`.
pub fn date(secs: i64) -> String {
    let (y, m, d) = corrent_core::util::civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.0000000Z", s / 3600, s / 60 % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids() {
        let g = enc(ITEM, 42);
        assert_eq!(g, "0100000000000000000000000000002a");
        assert_eq!(dec(&g), Some((ITEM, 42)));
        assert_eq!(dec("01000000-0000-0000-0000-00000000002A"), Some((ITEM, 42)));
        assert_eq!(dec_kind(&g, USER), None);
        assert_eq!(dec("nope"), None);
        assert_eq!(dec("f1d2d2f924e986ac86fdf7b36c94bcdf"), None);
        assert_eq!(dec(&enc(VIEW, 0)), Some((VIEW, 0)));
        assert_eq!(server("a1b2c3").len(), 32);
        assert_eq!(date(0), "1970-01-01T00:00:00.0000000Z");
        assert_eq!(ticks(1500), 15_000_000);
    }
}
