//! Small helpers: atomic file writes, human-readable formatting, time.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::config::SpeedUnits;

/// Write a file atomically: write to a temp sibling, fsync, then rename over the target.
/// The previous version is kept as `<name>.bak` so a crash mid-write never loses data.
pub fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    if path.exists() {
        let bak = path.with_extension(format!("{}.bak", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
        let _ = std::fs::copy(path, bak);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What kind of I/O problem an error message describes (works across OSes and wrappers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoKind {
    DiskFull,
    Permission,
    Locked,
    NotFound,
    Other,
}

pub fn classify_io(msg: &str) -> IoKind {
    let m = msg.to_ascii_lowercase();
    if m.contains("os error 28")
        || m.contains("os error 112")
        || m.contains("os error 39")
        || m.contains("no space left")
        || m.contains("not enough space")
        || m.contains("disk is full")
    {
        IoKind::DiskFull
    } else if m.contains("os error 32") || m.contains("os error 33") || m.contains("being used by another process") {
        IoKind::Locked
    } else if m.contains("os error 5)")
        || m.contains("os error 13")
        || m.contains("access is denied")
        || m.contains("permission denied")
        || m.contains("read-only file system")
        || m.contains("os error 30")
    {
        IoKind::Permission
    } else if m.contains("os error 2)")
        || m.contains("os error 3)")
        || m.contains("no such file")
        || m.contains("cannot find the path")
    {
        IoKind::NotFound
    } else {
        IoKind::Other
    }
}

/// A plain-language explanation for an I/O error, followed by the original message.
pub fn explain_io(msg: &str) -> String {
    let hint = match classify_io(msg) {
        IoKind::DiskFull => "The disk is full. Free up space or move the download to another drive.",
        IoKind::Permission => {
            "Access was denied. The file may be blocked or quarantined by antivirus software, read-only, or in a \
             folder Corrent may not write to."
        }
        IoKind::Locked => "A file is in use by another program. Close it and retry.",
        IoKind::NotFound => "A file or folder is missing (for example, a disconnected drive).",
        IoKind::Other => return msg.to_string(),
    };
    format!("{hint}\n\n{msg}")
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

pub fn fmt_speed(bps: u64, units: SpeedUnits) -> String {
    if bps == 0 {
        return "—".into();
    }
    let (base, names): (f64, [&str; 5]) = match units {
        SpeedUnits::Binary => (1024.0, ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"]),
        SpeedUnits::Decimal => (1000.0, ["B/s", "kB/s", "MB/s", "GB/s", "TB/s"]),
        SpeedUnits::Bits => (1000.0, ["bit/s", "kbit/s", "Mbit/s", "Gbit/s", "Tbit/s"]),
    };
    let mut v = if units == SpeedUnits::Bits { bps as f64 * 8.0 } else { bps as f64 };
    let mut i = 0;
    while v >= base && i < names.len() - 1 {
        v /= base;
        i += 1;
    }
    if i == 0 || v >= 100.0 { format!("{v:.0} {}", names[i]) } else { format!("{v:.1} {}", names[i]) }
}

pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 100 * 86400 {
        return "∞".into();
    }
    let (days, h, m, sec) = (s / 86400, (s / 3600) % 24, (s / 60) % 60, s % 60);
    if days > 0 {
        format!("{days}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {sec:02}s")
    } else {
        format!("{sec}s")
    }
}

pub fn fmt_date(ts: i64) -> String {
    if ts <= 0 {
        return "—".into();
    }
    // Civil-from-days (Howard Hinnant) in local-agnostic UTC; good enough for a list column.
    let offset = local_offset_secs();
    let t = ts + offset;
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {:02}:{:02}", secs / 3600, (secs / 60) % 60)
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Local UTC offset in seconds, computed once.
pub fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        #[cfg(windows)]
        {
            windows_offset().unwrap_or(0)
        }
        #[cfg(not(windows))]
        {
            unix_offset().unwrap_or(0)
        }
    })
}

#[cfg(not(windows))]
fn unix_offset() -> Option<i64> {
    // `date +%z` is available on Linux and macOS and avoids pulling a tz database.
    let out = std::process::Command::new("date").arg("+%z").output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    if s.len() != 5 {
        return None;
    }
    let sign = if s.starts_with('-') { -1 } else { 1 };
    let h: i64 = s[1..3].parse().ok()?;
    let m: i64 = s[3..5].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

#[cfg(windows)]
fn windows_offset() -> Option<i64> {
    #[repr(C)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: [u16; 8],
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: [u16; 8],
        daylight_bias: i32,
    }
    unsafe extern "system" {
        fn GetTimeZoneInformation(tzi: *mut TimeZoneInformation) -> u32;
    }
    // SAFETY: plain C struct filled by the OS.
    let mut tzi: TimeZoneInformation = unsafe { std::mem::zeroed() };
    let r = unsafe { GetTimeZoneInformation(&mut tzi) };
    if r == u32::MAX {
        return None;
    }
    // TIME_ZONE_ID_DAYLIGHT == 2; bias is minutes *west* of UTC.
    let bias = tzi.bias + if r == 2 { tzi.daylight_bias } else { tzi.standard_bias };
    Some(-(bias as i64) * 60)
}

/// Minutes since local midnight and weekday (0 = Monday).
pub fn local_minutes_and_weekday() -> (u16, u8) {
    let t = now_unix() + local_offset_secs();
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // 1970-01-01 was a Thursday (weekday 3 with Monday = 0).
    let weekday = ((days + 3).rem_euclid(7)) as u8;
    ((secs / 60) as u16, weekday)
}

pub fn fmt_ratio(r: f32) -> String {
    if !r.is_finite() { "∞".into() } else { format!("{r:.2}") }
}

/// Encode RGBA8 pixels as a PNG (uncompressed deflate blocks; small icons only).
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width * height * 4 + height) as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: none
        raw.extend_from_slice(&rgba[y * width as usize * 4..(y + 1) * width as usize * 4]);
    }
    // zlib stream with stored blocks
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(block) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.50 KiB");
        assert_eq!(fmt_speed(0, SpeedUnits::Binary), "—");
        assert_eq!(fmt_speed(2048, SpeedUnits::Binary), "2.0 KiB/s");
        assert_eq!(fmt_duration(Duration::from_secs(3725)), "1h 02m");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20000), (2024, 10, 4));
    }

    #[test]
    fn png_header() {
        let png = encode_png(&[255u8; 4 * 4], 2, 2);
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn io_classification() {
        assert_eq!(
            classify_io("error writing: There is not enough space on the disk. (os error 112)"),
            IoKind::DiskFull
        );
        assert_eq!(classify_io("No space left on device (os error 28)"), IoKind::DiskFull);
        assert_eq!(classify_io("Access is denied. (os error 5)"), IoKind::Permission);
        assert_eq!(classify_io("Permission denied (os error 13)"), IoKind::Permission);
        assert_eq!(classify_io("being used by another process. (os error 32)"), IoKind::Locked);
        assert_eq!(classify_io("The system cannot find the path specified. (os error 3)"), IoKind::NotFound);
        assert_eq!(classify_io("os error 50"), IoKind::Other, "not confused with 5");
        assert_eq!(explain_io("weird"), "weird");
        assert!(explain_io("os error 112").starts_with("The disk is full"));
    }

    #[test]
    fn atomic_write() {
        let dir = std::env::temp_dir().join(format!("corrent-test-{}", std::process::id()));
        let f = dir.join("x.json");
        write_atomic(&f, b"one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert_eq!(std::fs::read(dir.join("x.json.bak")).unwrap(), b"one");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn bytes_units() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(10 * 1024 * 1024), "10.0 MiB");
        assert_eq!(fmt_bytes(150 * 1024 * 1024 * 1024), "150 GiB");
    }

    #[test]
    fn speed_units() {
        assert_eq!(fmt_speed(1000, SpeedUnits::Decimal), "1.0 kB/s");
        assert_eq!(fmt_speed(125, SpeedUnits::Bits), "1.0 kbit/s");
        assert_eq!(fmt_speed(500, SpeedUnits::Binary), "500 B/s");
        assert_eq!(fmt_speed(200 * 1024 * 1024, SpeedUnits::Binary), "200 MiB/s");
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(Duration::from_secs(5)), "5s");
        assert_eq!(fmt_duration(Duration::from_secs(65)), "1m 05s");
        assert_eq!(fmt_duration(Duration::from_secs(2 * 86400 + 3600)), "2d 1h");
        assert_eq!(fmt_duration(Duration::from_secs(200 * 86400)), "∞");
    }

    #[test]
    fn ratio_and_dates() {
        assert_eq!(fmt_ratio(f32::INFINITY), "∞");
        assert_eq!(fmt_ratio(1.234), "1.23");
        assert_eq!(fmt_date(0), "—");
        assert_eq!(civil_from_days(19782), (2024, 2, 29), "leap day");
    }

    #[test]
    fn minutes_and_weekday_in_range() {
        let (m, d) = local_minutes_and_weekday();
        assert!(m < 24 * 60);
        assert!(d < 7);
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let dir = std::env::temp_dir().join(format!("corrent-aw-{}", std::process::id())).join("a/b");
        write_atomic(&dir.join("f.txt"), b"x").unwrap();
        assert_eq!(std::fs::read(dir.join("f.txt")).unwrap(), b"x");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }
}
