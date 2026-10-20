//! Small helpers: atomic file writes, human-readable formatting, time.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::SpeedUnits;

/// How long `<name>.bak` is kept before a save refreshes it. A backup that is only one save old
/// would take on a logical corruption within seconds; dated generations (`keep_generations`) go
/// further back.
const BAK_MAX_AGE: Duration = Duration::from_secs(3600);

/// Write a file atomically: write to a temp sibling, fsync, then rename over the target.
/// The previous version is kept as `<name>.bak` (refreshed at most once an hour) so a crash
/// mid-write never loses data. A rename refused because another program (antivirus, indexer,
/// sync client) holds the target is retried for about 1.5 s.
pub fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = sibling(path, "tmp");
    if let Err(e) = write_synced(&tmp, data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if path.exists() {
        refresh_bak(path);
    }
    if let Err(e) = rename_retrying(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    sync_parent(path);
    Ok(())
}

/// `<name>.<ext>.<suffix>` next to `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    path.with_extension(format!("{}.{suffix}", path.extension().and_then(|e| e.to_str()).unwrap_or("")))
}

fn write_synced(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Copy the current file to `.bak` when the backup is missing or older than `BAK_MAX_AGE`.
/// Written fresh (not `fs::copy`, which keeps the source's mtime on Windows) so its mtime says
/// when the backup was taken.
fn refresh_bak(path: &Path) {
    let bak = sibling(path, "bak");
    let fresh = std::fs::metadata(&bak)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < BAK_MAX_AGE);
    if !fresh && let Ok(current) = std::fs::read(path) {
        // Through a temp file too: a crash mid-refresh must not leave a cut-off backup.
        let tmp = sibling(&bak, "tmp");
        if write_synced(&tmp, &current).and_then(|()| std::fs::rename(&tmp, &bak)).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Like `write_atomic`, but without a `.bak` (for short-lived files rewritten often, such as a
/// move's progress note).
pub fn write_replace(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = sibling(path, "tmp");
    if let Err(e) = write_synced(&tmp, data).and_then(|()| rename_retrying(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    sync_parent(path);
    Ok(())
}

fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut delay = Duration::from_millis(50);
    loop {
        match std::fs::rename(from, to) {
            Err(e)
                if delay <= Duration::from_millis(800)
                    && matches!(classify_io(&e.to_string()), IoKind::Locked | IoKind::Permission) =>
            {
                std::thread::sleep(delay);
                delay *= 2;
            }
            r => return r,
        }
    }
}

/// Unix: a rename is durable only once its directory is synced.
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = std::fs::File::open(if dir.as_os_str().is_empty() { Path::new(".") } else { dir })
    {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Write `data` as a dated generation of `path` (`<name>.<tag>-<unix time>`, fsynced) and delete
/// all but the newest `keep` generations with that tag. Returns the new file.
pub fn keep_generations(path: &Path, tag: &str, data: &[u8], keep: usize) -> anyhow::Result<PathBuf> {
    keep_generation_at(path, tag, data, keep, now_unix())
}

fn keep_generation_at(path: &Path, tag: &str, data: &[u8], keep: usize, ts: i64) -> anyhow::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| anyhow::anyhow!("no file name: {}", path.display()))?;
    let file = path.with_file_name(format!("{}.{tag}-{ts}", name.to_string_lossy()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = sibling(&file, "tmp");
    if let Err(e) = write_synced(&tmp, data).and_then(|()| std::fs::rename(&tmp, &file)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    sync_parent(&file);
    for (_, old) in generations(path, tag).into_iter().skip(keep) {
        let _ = std::fs::remove_file(old);
    }
    Ok(file)
}

/// The generations of `path` with `tag`, newest first, with their timestamps.
pub fn generations(path: &Path, tag: &str) -> Vec<(i64, PathBuf)> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return Vec::new() };
    let prefix = format!("{}.{tag}-", name.to_string_lossy());
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let mut found: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let ts = n.strip_prefix(&prefix)?.parse().ok()?;
            Some((ts, e.path()))
        })
        .collect();
    found.sort_by_key(|g| std::cmp::Reverse(g.0));
    found
}

/// The largest .torrent file Corrent reads (real ones are a few MB at most).
pub const MAX_TORRENT_BYTES: u64 = 64 << 20;

/// Read a file of at most `max` bytes. A bigger one is refused before anything is read (a
/// multi-gigabyte file named `.torrent` must not be loaded into memory).
pub fn read_capped(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let len = std::fs::metadata(path)?.len();
    too_large(path, len, max)?;
    let mut buf = Vec::with_capacity(len as usize);
    // The file may grow while it is read.
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    too_large(path, buf.len() as u64, max)?;
    Ok(buf)
}

/// `read_capped` for async code.
pub async fn read_capped_async(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let len = tokio::fs::metadata(path).await?.len();
    too_large(path, len, max)?;
    let mut buf = Vec::with_capacity(len as usize);
    tokio::fs::File::open(path).await?.take(max + 1).read_to_end(&mut buf).await?;
    too_large(path, buf.len() as u64, max)?;
    Ok(buf)
}

fn too_large(path: &Path, len: u64, max: u64) -> std::io::Result<()> {
    if len <= max {
        return Ok(());
    }
    // The callers name the file already.
    let _ = path;
    Err(std::io::Error::other(t!(
        "the file is too large ({size}; at most {max} is read)",
        size = fmt_bytes(len),
        max = fmt_bytes(max)
    )))
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
        || (cfg!(windows) && m.contains("os error 39"))
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
        IoKind::DiskFull => t!("The disk is full. Free up space or move the download to another drive."),
        IoKind::Permission => {
            t!("Access was denied. The file may be blocked or quarantined by antivirus software, read-only, or in a \
             folder Corrent may not write to.")
        }
        IoKind::Locked => t!("A file is in use by another program. Close it and retry."),
        IoKind::NotFound => t!("A file or folder is missing (for example, a disconnected drive)."),
        IoKind::Other => return msg.to_string(),
    };
    format!("{hint}\n\n{msg}")
}

/// `bytes` random bytes from the OS generator, hex-encoded.
pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    #[expect(clippy::expect_used, reason = "the OS generator doesn't fail; nothing is safe without it")]
    aws_lc_rs::rand::fill(&mut b).expect("OS random generator");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A URL without credentials, query or fragment (private feeds carry passkeys there), for logs.
pub fn redact_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            let had_query = u.query().is_some();
            u.set_query(None);
            u.set_fragment(None);
            let mut s = u.to_string();
            if had_query {
                s.push_str("?…");
            }
            s
        }
        Err(_) => "(invalid address)".into(),
    }
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
    let unit = UNITS.get(i).copied().unwrap_or_default();
    if v >= 100.0 {
        format!("{v:.0} {unit}")
    } else if v >= 10.0 {
        format!("{v:.1} {unit}")
    } else {
        format!("{v:.2} {unit}")
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
    let unit = names.get(i).copied().unwrap_or_default();
    if i == 0 || v >= 100.0 { format!("{v:.0} {unit}") } else { format!("{v:.1} {unit}") }
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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    // Re-read every 10 minutes so a daylight-saving change while Corrent runs is picked up.
    static OFF: Mutex<Option<(Instant, i64)>> = Mutex::new(None);
    let cached = *OFF.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = cached
        && at.elapsed() < Duration::from_secs(600)
    {
        return v;
    }
    // Computed without the lock: on Unix it runs `date`, and every date shown in the UI would wait
    // behind a slow process start. Two threads may compute it at once; both get the same value.
    #[cfg(windows)]
    let v = windows_offset().unwrap_or(0);
    #[cfg(not(windows))]
    let v = unix_offset().unwrap_or(0);
    *OFF.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v));
    v
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
        raw.extend_from_slice(rgba.get(y * width as usize * 4..(y + 1) * width as usize * 4).unwrap_or_default());
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

/// Lock `m` even after a thread panicked while holding it (STABILITY.md F18). The data may be
/// half-updated, but every lock in Corrent guards state that stays usable (caches, queues,
/// status); a lock that silently stops working once poisoned is worse.
pub fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run `f` on the data behind `m` ([`lock`]).
pub fn with_lock<T: ?Sized, R>(m: &std::sync::Mutex<T>, f: impl FnOnce(&mut T) -> R) -> R {
    f(&mut lock(m))
}

/// [`lock`] for reading an `RwLock`.
pub fn read<T: ?Sized>(l: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`lock`] for writing an `RwLock`.
pub fn write<T: ?Sized>(l: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(std::sync::PoisonError::into_inner)
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
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bak_is_refreshed_at_most_hourly() {
        let dir = scratch("bak-age");
        let f = dir.join("store.json");
        let bak = dir.join("store.json.bak");
        write_atomic(&f, b"0").unwrap();
        for i in 1..=100 {
            write_atomic(&f, format!("{i}").as_bytes()).unwrap();
        }
        assert_eq!(std::fs::read(&f).unwrap(), b"100");
        assert_eq!(std::fs::read(&bak).unwrap(), b"0", "the backup survives 100 saves within the hour");
        // An hour later the next save takes a new backup of the current file.
        let old = std::time::SystemTime::now() - BAK_MAX_AGE - Duration::from_secs(60);
        std::fs::File::options().write(true).open(&bak).unwrap().set_modified(old).unwrap();
        write_atomic(&f, b"101").unwrap();
        assert_eq!(std::fs::read(&bak).unwrap(), b"100");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn rename_waits_for_a_handle_that_blocks_it() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("held");
        let f = dir.join("settings.toml");
        write_atomic(&f, b"one").unwrap();
        // Like an antivirus scan: the file is open without FILE_SHARE_DELETE, so it can't be
        // replaced until the handle closes.
        const FILE_SHARE_READ: u32 = 1;
        let held = std::fs::File::options().read(true).share_mode(FILE_SHARE_READ).open(&f).unwrap();
        std::fs::write(dir.join("probe"), b"").unwrap();
        assert!(std::fs::rename(dir.join("probe"), &f).is_err(), "the handle blocks replacing the file");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        write_atomic(&f, b"two").unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert!(!dir.join("settings.toml.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replace_keeps_no_backup() {
        let dir = scratch("replace");
        let f = dir.join("note.json");
        write_replace(&f, b"one").unwrap();
        write_replace(&f, b"two").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no .bak or .tmp left");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn big_files_are_refused_before_reading() {
        let dir = scratch("capped");
        let f = dir.join("huge.torrent");
        // 65 MiB without writing it (sparse; instant).
        std::fs::File::create(&f).unwrap().set_len(65 << 20).unwrap();
        let t = std::time::Instant::now();
        let err = read_capped(&f, MAX_TORRENT_BYTES).unwrap_err();
        assert!(t.elapsed() < Duration::from_millis(500), "it was read: {:?}", t.elapsed());
        assert!(err.to_string().contains("too large"), "{err}");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(rt.block_on(read_capped_async(&f, MAX_TORRENT_BYTES)).is_err());
        std::fs::write(&f, b"d4:infoe").unwrap();
        assert_eq!(read_capped(&f, MAX_TORRENT_BYTES).unwrap(), b"d4:infoe");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn generations_keep_the_newest() {
        let dir = scratch("gens");
        let f = dir.join("store.json");
        std::fs::write(dir.join("store.json.bak"), b"bak").unwrap();
        std::fs::write(dir.join("store.json.daily-x"), b"not ours").unwrap();
        std::fs::write(dir.join("other.json.daily-5"), b"other").unwrap();
        for ts in [10, 30, 20, 40] {
            keep_generation_at(&f, "daily", ts.to_string().as_bytes(), 3, ts).unwrap();
        }
        let kept = generations(&f, "daily");
        assert_eq!(kept.iter().map(|g| g.0).collect::<Vec<_>>(), [40, 30, 20]);
        assert_eq!(std::fs::read(&kept[0].1).unwrap(), b"40");
        assert!(!dir.join("store.json.daily-10").exists());
        for other in ["store.json.bak", "store.json.daily-x", "other.json.daily-5"] {
            assert!(dir.join(other).exists(), "{other} is left alone");
        }
        assert!(generations(&f, "pre-prune").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod more_tests {

    #[test]
    fn redacts_secrets_from_urls() {
        assert_eq!(
            crate::util::redact_url("https://user:pw@tracker.example/rss?passkey=abc#x"),
            "https://tracker.example/rss?…"
        );
        assert_eq!(crate::util::redact_url("https://a.example/feed.xml"), "https://a.example/feed.xml");
        assert_eq!(crate::util::random_hex(16).len(), 32);
    }

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
    fn offset_is_shared_by_concurrent_callers() {
        let all: Vec<i64> = (0..8)
            .map(|_| std::thread::spawn(local_offset_secs))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert!(all.windows(2).all(|w| w[0] == w[1]), "{all:?}");
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

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn lock_recovers_after_a_panicking_holder() {
        let m = std::sync::Arc::new(std::sync::Mutex::new(vec![1]));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let mut g = m2.lock().unwrap();
            g.push(2);
            panic!("while holding the lock");
        })
        .join();
        assert!(m.is_poisoned());
        assert_eq!(*lock(&m), vec![1, 2]);
        let l = std::sync::RwLock::new(1);
        *write(&l) += 1;
        assert_eq!(*read(&l), 2);
    }

    /// Every std `lock()`, `read()` or `write()` in shipped code recovers from poisoning
    /// (`util::lock` or `.unwrap_or_else(..)`), or is a tokio lock (`.await`). `.ok()`,
    /// `if let Ok(..) = m.lock()` and `.map(..)` silently stop working once a thread panicked
    /// while holding the lock (STABILITY.md F18).
    #[test]
    fn locks_never_go_silent_when_poisoned() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        let mut stack = vec![root.join("src")];
        for c in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
            stack.push(c.path().join("src"));
        }
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        let mut found = Vec::new();
        for p in files {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with("_tests.rs") || name == "tests.rs" || name == "testkit.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
            let code = match text.find("#[cfg(test)]\nmod tests {") {
                Some(i) => &text[..i],
                None => text.as_str(),
            };
            for (n, line) in code.lines().enumerate() {
                // Chains continue on the next lines: look at what follows, without whitespace.
                let rest: String = code.lines().skip(n).take(3).collect::<Vec<_>>().join("");
                let rest: String = rest.chars().filter(|c| !c.is_whitespace()).collect();
                let skip = line.chars().filter(|c| !c.is_whitespace()).count();
                let here = rest.get(..skip).unwrap_or(&rest);
                for call in [".lock()", ".read()", ".write()"] {
                    let mut from = 0;
                    while let Some(i) = here.get(from..).and_then(|h| h.find(call)) {
                        let after = rest.get(from + i + call.len()..).unwrap_or("");
                        // `self.lock()`: a type's own helper (it recovers, or this test finds it).
                        let own = here.get(..from + i).is_some_and(|b| b.ends_with("self"));
                        let ok = own || after.starts_with(".await") || after.starts_with(".unwrap_or_else(");
                        if !ok {
                            found.push(format!("{}:{}: {}", p.display(), n + 1, line.trim()));
                        }
                        from += i + call.len();
                    }
                }
            }
        }
        assert!(found.is_empty(), "use corrent_core::util::lock (or read/write) for these:\n{}", found.join("\n"));
    }
}
