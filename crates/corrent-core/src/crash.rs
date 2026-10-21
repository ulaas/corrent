//! Crash reports and unclean-shutdown detection.

use std::io::Write;
use std::path::{Path, PathBuf};

const KEEP_REPORTS: usize = 5;

/// What the previous run left behind.
#[derive(Clone, Debug, Default)]
pub struct PreviousRun {
    /// The previous run did not shut down cleanly.
    pub unclean: bool,
    /// Crash report written by the previous run, if it panicked.
    pub report: Option<PathBuf>,
    /// The computer was restarted since the previous run started (with `unclean`: a power loss,
    /// a hard reset or a system crash, so recently written data may not have reached the disk).
    pub rebooted: bool,
}

fn marker(log_dir: &Path) -> PathBuf {
    log_dir.join("running.marker")
}

/// Set by the panic hook; survives a normal quit (e.g. after an engine-task panic the UI kept
/// running) and is cleared when the user dismisses the banner.
fn pending(log_dir: &Path) -> PathBuf {
    log_dir.join("crash.pending")
}

pub fn dismiss(log_dir: &Path) {
    let _ = std::fs::remove_file(pending(log_dir));
}

/// Touched about once a minute while the engine runs: when the previous run was last alive.
fn alive(log_dir: &Path) -> PathBuf {
    log_dir.join("alive")
}

/// Written at startup after an unclean exit and a restart of the computer; holds when the
/// previous run was last known alive (unix time). The engine consumes it (see `power_loss`).
fn power_loss_flag(log_dir: &Path) -> PathBuf {
    log_dir.join("power-loss.pending")
}

/// Mark the running engine as alive (called about once a minute).
pub fn heartbeat(log_dir: &Path) {
    let _ = std::fs::write(alive(log_dir), crate::util::now_unix().to_string());
}

/// After a power loss (see `PreviousRun::rebooted`): when the previous run was last known alive.
/// Taken once: the flag is removed.
pub fn take_power_loss(log_dir: &Path) -> Option<i64> {
    let f = power_loss_flag(log_dir);
    let at = std::fs::read_to_string(&f).ok()?;
    let _ = std::fs::remove_file(&f);
    Some(at.trim().parse().unwrap_or(0))
}

/// Something that changes with every boot: the boot time on Windows (to the minute it can drift
/// with the wall clock), the kernel's boot id on Linux. None elsewhere (macOS: never treated as
/// a reboot).
fn boot_token() -> Option<String> {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetTickCount64() -> u64;
        }
        // SAFETY: no arguments, no state.
        let up = unsafe { GetTickCount64() } / 1000;
        Some(format!("t{}", crate::util::now_unix() - up as i64))
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok().map(|s| s.trim().to_string())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// Whether two boot tokens name different boots. Boot times within a minute are one boot (the
/// wall clock may have been corrected since); unknown is never a reboot.
fn other_boot(before: Option<&str>, now: Option<&str>) -> bool {
    match (before, now) {
        (Some(a), Some(b)) => match (a.strip_prefix('t'), b.strip_prefix('t')) {
            (Some(x), Some(y)) => match (x.parse::<i64>(), y.parse::<i64>()) {
                (Ok(x), Ok(y)) => (x - y).abs() > 60,
                _ => false,
            },
            _ => a != b,
        },
        _ => false,
    }
}

/// Call once at startup: inspects the previous run and marks this one as running.
pub fn start(log_dir: &Path) -> PreviousRun {
    start_at(log_dir, boot_token())
}

fn start_at(log_dir: &Path, boot: Option<String>) -> PreviousRun {
    let _ = std::fs::create_dir_all(log_dir);
    let m = marker(log_dir);
    let since = std::fs::metadata(&m).and_then(|md| md.modified()).ok();
    // "pid\nboot token" (older versions wrote the pid only).
    let previous_boot = std::fs::read_to_string(&m).ok().and_then(|t| t.lines().nth(1).map(str::to_string));
    let crashed = pending(log_dir).exists();
    let unclean = since.is_some() || crashed;
    let report = if crashed {
        reports(log_dir).into_iter().next()
    } else {
        since.and_then(|t| {
            reports(log_dir)
                .into_iter()
                .find(|p| std::fs::metadata(p).and_then(|md| md.modified()).map(|mt| mt >= t).unwrap_or(false))
        })
    };
    let rebooted = since.is_some() && other_boot(previous_boot.as_deref(), boot.as_deref());
    // A flag the engine didn't take yet (it never started) stays: one extra check is harmless,
    // a missed one isn't.
    if rebooted {
        // Last sign of life: the heartbeat, else the start of that run.
        let at = std::fs::read_to_string(alive(log_dir))
            .ok()
            .and_then(|t| t.trim().parse::<i64>().ok())
            .or_else(|| since.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64))
            .unwrap_or(0);
        let _ = std::fs::write(power_loss_flag(log_dir), at.to_string());
    }
    let _ = std::fs::remove_file(alive(log_dir));
    let _ = std::fs::write(&m, format!("{}\n{}", std::process::id(), boot.unwrap_or_default()));
    PreviousRun { unclean, report, rebooted }
}

/// Call on clean shutdown.
pub fn finish(log_dir: &Path) {
    let _ = std::fs::remove_file(marker(log_dir));
}

/// Crash reports, newest first.
pub fn reports(log_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        })
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    // Newest first; report names carry a timestamp, which breaks ties on coarse-mtime filesystems.
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    v.into_iter().map(|(_, p)| p).collect()
}

/// Install a panic hook that writes a crash report before the default hook runs.
pub fn install_hook(log_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = write_report(&log_dir, info);
        previous(info);
        // The lines before the panic are queued for the log thread (a no-op on that thread).
        crate::logfile::flush(std::time::Duration::from_millis(500));
    }));
}

fn write_report(log_dir: &Path, info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    let ts = crate::util::now_unix();
    let path = log_dir.join(format!("crash-{ts}.txt"));
    let mut f = std::fs::File::create(&path)?;
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-string panic payload)".into());
    let thread = std::thread::current();
    writeln!(f, "Corrent {} crash report", env!("CARGO_PKG_VERSION"))?;
    writeln!(f, "time: {} (unix {ts})", crate::util::fmt_date(ts))?;
    writeln!(f, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH)?;
    writeln!(f, "thread: {}", thread.name().unwrap_or("<unnamed>"))?;
    writeln!(f, "message: {message}")?;
    if let Some(loc) = info.location() {
        writeln!(f, "location: {}:{}:{}", loc.file(), loc.line(), loc.column())?;
    }
    // Offsets into the image, resolvable later by the same build (`--symbolize`, symbols.rs).
    writeln!(f, "\nbacktrace:\n{}", crate::symbols::backtrace_text())?;
    // Never block inside a panic: skip the log tail if the lock is held. A poisoned buffer (a
    // panic while logging) is still worth reading.
    let lines = match crate::logbuf::LINES.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    if let Some(lines) = lines {
        writeln!(f, "\nrecent log:")?;
        let skip = lines.len().saturating_sub(200);
        for l in lines.iter().skip(skip) {
            writeln!(f, "{} {} {}: {}", l.time, l.level, l.target, l.message)?;
        }
    }
    f.flush()?;
    let _ = std::fs::write(pending(log_dir), path.to_string_lossy().as_bytes());
    for old in reports(log_dir).into_iter().skip(KEEP_REPORTS) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclean_detection() {
        let dir = std::env::temp_dir().join(format!("corrent-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!start(&dir).unclean);
        // Simulated crash: marker left behind plus a report (written after it, so mtime >= marker's).
        std::fs::write(dir.join("crash-1.txt"), "x").unwrap();
        let prev = start(&dir);
        assert!(prev.unclean);
        assert!(prev.report.is_some());
        finish(&dir);
        assert!(!start(&dir).unclean);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn reports_are_newest_first_and_filtered() {
        let dir = std::env::temp_dir().join(format!("corrent-crash2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("crash-1.txt"), "a").unwrap();
        std::fs::write(dir.join("crash-2.txt"), "b").unwrap();
        std::fs::write(dir.join("corrent.log"), "c").unwrap();
        let r = reports(&dir);
        assert_eq!(r.len(), 2);
        assert!(r[0].ends_with("crash-2.txt"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod power_loss_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-power-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_unclean_exit_and_a_restart_flag_a_power_loss() {
        let dir = scratch("flag");
        let boot1 = Some("t1000000".to_string());
        assert!(!start_at(&dir, boot1.clone()).unclean);
        heartbeat(&dir);
        let alive_at = crate::util::now_unix();
        // No clean finish, and the computer restarted.
        let run = start_at(&dir, Some("t1000500".into()));
        assert!(run.unclean && run.rebooted);
        let at = take_power_loss(&dir).unwrap();
        assert!((at - alive_at).abs() <= 1, "{at} vs {alive_at}");
        assert_eq!(take_power_loss(&dir), None, "taken once");
        // Crashed but the computer kept running: the OS still writes the cached data.
        let run = start_at(&dir, Some("t1000530".into()));
        assert!(run.unclean && !run.rebooted, "within a minute: the same boot");
        assert_eq!(take_power_loss(&dir), None);
        // A clean exit is never a power loss; a flag nobody took yet stays for the engine.
        finish(&dir);
        let run = start_at(&dir, Some("t9999999".into()));
        assert!(!run.unclean && !run.rebooted);
        assert_eq!(take_power_loss(&dir), None);
        std::fs::write(power_loss_flag(&dir), "5").unwrap();
        finish(&dir);
        start_at(&dir, Some("t9999999".into()));
        assert_eq!(take_power_loss(&dir), Some(5));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn boots_are_compared_with_slack_and_unknown_is_not_a_reboot() {
        assert!(!other_boot(Some("t100"), Some("t150")));
        assert!(other_boot(Some("t100"), Some("t200")));
        assert!(other_boot(Some("a-b-c"), Some("d-e-f")));
        assert!(!other_boot(Some("a-b-c"), Some("a-b-c")));
        assert!(!other_boot(None, Some("t100")), "a marker from an older version");
        assert!(!other_boot(Some("t100"), None));
        assert!(boot_token().is_some() || !cfg!(any(windows, target_os = "linux")));
    }
}
