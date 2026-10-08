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

/// Call once at startup: inspects the previous run and marks this one as running.
pub fn start(log_dir: &Path) -> PreviousRun {
    let _ = std::fs::create_dir_all(log_dir);
    let m = marker(log_dir);
    let since = std::fs::metadata(&m).and_then(|md| md.modified()).ok();
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
    let _ = std::fs::write(&m, std::process::id().to_string());
    PreviousRun { unclean, report }
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
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt")))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// Install a panic hook that writes a crash report before the default hook runs.
pub fn install_hook(log_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = write_report(&log_dir, info);
        previous(info);
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
    writeln!(f, "\nbacktrace:\n{}", std::backtrace::Backtrace::force_capture())?;
    // Never block inside a panic: skip the log tail if the lock is held.
    if let Ok(lines) = crate::app::log::LINES.try_lock() {
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
        // Simulated crash: marker left behind plus a report.
        std::thread::sleep(std::time::Duration::from_millis(20));
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
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("crash-2.txt"), "b").unwrap();
        std::fs::write(dir.join("corrent.log"), "c").unwrap();
        let r = reports(&dir);
        assert_eq!(r.len(), 2);
        assert!(r[0].ends_with("crash-2.txt"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
