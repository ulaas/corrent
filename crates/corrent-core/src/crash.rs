//! Crash reports and unclean-shutdown detection.

use std::cell::Cell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Unclean without a report (an abort, a native crash, the process killed): the end of the
    /// previous run's log, saved as `crash-<time>-noreport.txt`.
    pub note: Option<PathBuf>,
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
    // Written before this run's marker, so it can never pass for a report of this run.
    let note = (unclean && report.is_none()).then(|| write_note(log_dir)).flatten();
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
    PreviousRun { unclean, report, rebooted, note }
}

/// Bytes of the previous log kept in a no-report note.
const NOTE_TAIL: u64 = 16 * 1024;

fn is_note(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("-noreport.txt"))
}

/// The newest log's last lines, after a run that ended without a report.
fn write_note(log_dir: &Path) -> Option<PathBuf> {
    use std::io::{Read, Seek, SeekFrom};
    let log = std::fs::read_dir(log_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p);
    let mut tail = String::new();
    if let Some(log) = &log
        && let Ok(mut f) = std::fs::File::open(log)
    {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(NOTE_TAIL)));
        let mut bytes = Vec::new();
        let _ = f.read_to_end(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);
        // From the first whole line.
        tail = match text.split_once('\n') {
            Some((_, rest)) if len > NOTE_TAIL => rest.to_string(),
            _ => text.into_owned(),
        };
    }
    let ts = crate::util::now_unix();
    let mut out = String::new();
    let _ = writeln!(out, "Corrent {} note: the previous run ended without a crash report", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "time: {} (unix {ts})", crate::util::fmt_date(ts));
    let _ = writeln!(out, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let _ =
        writeln!(out, "It was ended from outside (killed, the system shut down) or by a crash that writes no report.");
    let log_name = log.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
    let _ = writeln!(out, "\nend of {} (may begin with this run's first lines):", log_name.unwrap_or_default());
    out.push_str(&tail);
    let path = log_dir.join(format!("crash-{ts}-noreport.txt"));
    std::fs::write(&path, out).ok()?;
    for old in newest_first(log_dir, |n| n.starts_with("crash-") && n.ends_with("-noreport.txt"))
        .into_iter()
        .skip(KEEP_REPORTS)
    {
        let _ = std::fs::remove_file(old);
    }
    Some(path)
}

/// Call on clean shutdown.
pub fn finish(log_dir: &Path) {
    let _ = std::fs::remove_file(marker(log_dir));
}

/// Crash reports, newest first (not the no-report notes).
pub fn reports(log_dir: &Path) -> Vec<PathBuf> {
    newest_first(log_dir, |n| n.starts_with("crash-") && n.ends_with(".txt"))
        .into_iter()
        .filter(|p| !is_note(p))
        .collect()
}

fn newest_first(log_dir: &Path, name: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(&name))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    // Newest first; report names carry a timestamp, which breaks ties on coarse-mtime filesystems.
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    v.into_iter().map(|(_, p)| p).collect()
}

/// What the app was doing, for crash reports (O3). Updated by the binaries and the engine; the
/// panic hook only reads it, never waiting for the lock.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// "0.1.0 (abc123)".
    pub build: String,
    pub portable: bool,
    /// The engine's state (`Running`) and its torrents by state ("downloading 2, seeding 5").
    pub engine: String,
    pub torrents: String,
    /// Settings sections that differ from the defaults (names only, never values).
    pub changed_settings: Vec<String>,
    /// Free space in the save folder.
    pub free_space: Option<u64>,
}

static CONTEXT: Mutex<Option<Context>> = Mutex::new(None);
static STARTED: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);

/// Change the crash-report context.
pub fn note(f: impl FnOnce(&mut Context)) {
    f(crate::util::lock(&CONTEXT).get_or_insert_with(Default::default));
}

fn context() -> Option<Context> {
    match CONTEXT.try_lock() {
        Ok(g) => g.clone(),
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().clone(),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// E7: whether a panic stops the app (the UI thread, the engine actor) or only one of the many
/// tasks (librqbit's peers and trackers, a request, a helper thread), which the app survives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanicKind {
    Fatal,
    Task,
}

pub fn panic_kind(thread: Option<&str>, in_engine: bool) -> PanicKind {
    if in_engine || thread == Some("main") { PanicKind::Fatal } else { PanicKind::Task }
}

tokio::task_local! {
    static ENGINE_TASK: ();
}

thread_local! {
    static ENGINE_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Runs the engine actor (or the backend standing in for it): its panics are fatal.
pub async fn engine_scope<F: std::future::Future>(f: F) -> F::Output {
    ENGINE_TASK.scope((), f).await
}

/// [`engine_scope`] for blocking work on the actor's behalf (loading it on a blocking thread).
pub fn engine_thread<R>(f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        // Runs after the hook, so a panic in `f` is still seen as the engine's.
        fn drop(&mut self) {
            ENGINE_THREAD.with(|c| c.set(false));
        }
    }
    ENGINE_THREAD.with(|c| c.set(true));
    let _reset = Reset;
    f()
}

fn in_engine() -> bool {
    ENGINE_TASK.try_with(|_| ()).is_ok() || ENGINE_THREAD.try_with(Cell::get).unwrap_or(false)
}

static TASK_PANICS: AtomicU64 = AtomicU64::new(0);
/// When the last task-panic report was written (unix seconds): at most one a minute, since a
/// panicking peer task could fire many times a second.
static LAST_TASK_REPORT_AT: AtomicU64 = AtomicU64::new(0);
static LAST_TASK_REPORT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Task panics so far, and the newest one's report (the GUI tells the user once per panic).
pub fn task_panics() -> (u64, Option<PathBuf>) {
    let report = LAST_TASK_REPORT.try_lock().ok().and_then(|g| g.clone());
    (TASK_PANICS.load(Ordering::Relaxed), report)
}

/// Install a panic hook that writes a crash report before the default hook runs.
pub fn install_hook(log_dir: PathBuf) {
    let _ = *STARTED;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = write_report(&log_dir, info);
        previous(info);
        // The lines before the panic are queued for the log thread (a no-op on that thread).
        crate::logfile::flush(std::time::Duration::from_millis(500));
    }));
}

/// A report's contents, gathered in the hook.
struct Report {
    ts: i64,
    kind: PanicKind,
    thread: String,
    message: String,
    location: Option<String>,
    backtrace: String,
    context: Option<Context>,
    uptime: std::time::Duration,
    crumb: String,
    commands: Vec<std::sync::Arc<str>>,
    log: Option<Vec<String>>,
}

/// The report's text (English: it is for bug reports).
fn render(r: &Report) -> String {
    let mut f = String::new();
    let _ = writeln!(f, "Corrent {} crash report", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(f, "time: {} (unix {})", crate::util::fmt_date(r.ts), r.ts);
    let _ = writeln!(f, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let kind = match r.kind {
        PanicKind::Fatal => "fatal (the app or the engine stopped)",
        PanicKind::Task => "task panic (the app kept running)",
    };
    let _ = writeln!(f, "kind: {kind}");
    let _ = writeln!(f, "thread: {}", r.thread);
    let _ = writeln!(f, "message: {}", r.message);
    if let Some(loc) = &r.location {
        let _ = writeln!(f, "location: {loc}");
    }
    let _ = writeln!(f, "\ncontext:");
    let _ = writeln!(f, "uptime: {} s", r.uptime.as_secs());
    if let Some(c) = &r.context {
        let _ = writeln!(f, "build: {}", c.build);
        let _ = writeln!(f, "portable: {}", c.portable);
        let _ = writeln!(f, "engine: {}", c.engine);
        let _ = writeln!(f, "torrents: {}", c.torrents);
        let _ = writeln!(f, "changed settings: {}", c.changed_settings.join(", "));
        if let Some(free) = c.free_space {
            let _ = writeln!(f, "free space (save folder): {free} bytes");
        }
    }
    let _ = writeln!(f, "engine busy with: {}", r.crumb);
    let names: Vec<&str> = r.commands.iter().map(|c| &**c).collect();
    let _ = writeln!(f, "recent commands: {}", names.join(", "));
    // Offsets into the image, resolvable later by the same build (`--symbolize`, symbols.rs).
    let _ = writeln!(f, "\nbacktrace:\n{}", r.backtrace);
    if let Some(lines) = &r.log {
        let _ = writeln!(f, "\nrecent log:");
        for l in lines {
            let _ = writeln!(f, "{l}");
        }
    }
    f
}

fn write_report(log_dir: &Path, info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    let ts = crate::util::now_unix();
    let path = log_dir.join(format!("crash-{ts}.txt"));
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-string panic payload)".into());
    let thread = std::thread::current();
    let kind = panic_kind(thread.name(), in_engine());
    if kind == PanicKind::Task {
        TASK_PANICS.fetch_add(1, Ordering::Relaxed);
        let last = LAST_TASK_REPORT_AT.load(Ordering::Relaxed);
        if (ts as u64).saturating_sub(last) < 60 {
            return Ok(());
        }
        LAST_TASK_REPORT_AT.store(ts as u64, Ordering::Relaxed);
    } else if in_engine() {
        crate::watchdog::actor_stopped();
    }
    // Never block inside a panic: skip the log tail if the lock is held. A poisoned buffer (a
    // panic while logging) is still worth reading.
    let lines = match crate::logbuf::LINES.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    let log = lines.map(|lines| {
        let skip = lines.len().saturating_sub(200);
        lines.iter().skip(skip).map(|l| format!("{} {} {}: {}", l.time, l.level, l.target, l.message)).collect()
    });
    let report = Report {
        ts,
        kind,
        thread: thread.name().unwrap_or("<unnamed>").to_string(),
        message,
        location: info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())),
        backtrace: crate::symbols::backtrace_text(),
        context: context(),
        uptime: STARTED.elapsed(),
        crumb: crate::watchdog::crumb().doing.to_string(),
        commands: crate::watchdog::recent_commands(),
        log,
    };
    std::fs::write(&path, render(&report))?;
    match kind {
        PanicKind::Fatal => {
            let _ = std::fs::write(pending(log_dir), path.to_string_lossy().as_bytes());
        }
        // The app goes on: no "closed unexpectedly" at the next start, a notice now instead.
        PanicKind::Task => {
            if let Ok(mut last) = LAST_TASK_REPORT.try_lock() {
                *last = Some(path.clone());
            }
        }
    }
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

#[cfg(test)]
mod s10_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn panics_outside_the_ui_and_the_engine_are_not_fatal() {
        assert_eq!(panic_kind(Some("main"), false), PanicKind::Fatal);
        assert_eq!(panic_kind(Some("corrent-rt"), true), PanicKind::Fatal);
        assert_eq!(panic_kind(Some("corrent-rt"), false), PanicKind::Task);
        assert_eq!(panic_kind(None, false), PanicKind::Task);
        assert!(!in_engine());
        assert!(engine_thread(in_engine));
        assert!(!in_engine(), "reset afterwards");
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert!(rt.block_on(engine_scope(async { in_engine() })));
        // Tasks the engine spawns aren't the engine (librqbit's peers, E7).
        let spawned = rt.block_on(engine_scope(async { tokio::spawn(async { in_engine() }).await.unwrap() }));
        assert!(!spawned);
    }

    #[test]
    fn a_report_has_its_context() {
        let r = Report {
            ts: 1_000,
            kind: PanicKind::Task,
            thread: "corrent-rt".into(),
            message: "boom".into(),
            location: Some("a.rs:1:2".into()),
            backtrace: "image: x\n   0: x+0x1".into(),
            context: Some(Context {
                build: "0.1.0 (abc)".into(),
                portable: true,
                engine: "Running".into(),
                torrents: "Seeding 3, Paused 1".into(),
                changed_settings: vec!["connection".into(), "speed".into()],
                free_space: Some(5),
            }),
            uptime: std::time::Duration::from_secs(61),
            crumb: "command Pause".into(),
            commands: vec!["Add".into(), "Pause".into()],
            log: Some(vec!["l1".into(), "l2".into()]),
        };
        let text = render(&r);
        for want in [
            "kind: task panic",
            "thread: corrent-rt",
            "message: boom",
            "location: a.rs:1:2",
            "uptime: 61 s",
            "build: 0.1.0 (abc)",
            "portable: true",
            "engine: Running",
            "torrents: Seeding 3, Paused 1",
            "changed settings: connection, speed",
            "free space (save folder): 5 bytes",
            "engine busy with: command Pause",
            "recent commands: Add, Pause",
            "backtrace:\nimage: x\n   0: x+0x1",
            "recent log:\nl1\nl2\n",
        ] {
            assert!(text.contains(want), "{want:?} missing from:\n{text}");
        }
        let fatal = render(&Report { kind: PanicKind::Fatal, context: None, log: None, ..r });
        assert!(fatal.contains("kind: fatal") && !fatal.contains("build:") && !fatal.contains("recent log"));
    }

    #[test]
    fn a_simulated_abort_leaves_a_noreport_note() {
        let dir = scratch("abort");
        assert!(start(&dir).note.is_none());
        // The run logs, then is ended from outside: no finish, no report.
        let mut log = String::new();
        for i in 0..2000 {
            log.push_str(&format!("line {i}\n"));
        }
        std::fs::write(dir.join("corrent.log"), &log).unwrap();
        let run = start(&dir);
        assert!(run.unclean && run.report.is_none());
        let note = run.note.expect("a note");
        let text = std::fs::read_to_string(&note).unwrap();
        assert!(text.contains("ended without a crash report"), "{text}");
        assert!(text.ends_with("line 1999\n"), "the end of the log");
        assert!(!text.contains("line 0\n") && text.len() < 17 * 1024, "only the tail");
        assert!(reports(&dir).is_empty(), "a note isn't a report");
        // The next unclean start doesn't take the old note for its report.
        let again = start(&dir);
        assert!(again.report.is_none() && again.note.is_some());
        // A clean run leaves nothing.
        finish(&dir);
        assert!(start(&dir).note.is_none());
        // Notes are pruned like reports.
        for i in 0..8 {
            std::fs::write(dir.join(format!("crash-{i}-noreport.txt")), "x").unwrap();
        }
        write_note(&dir).unwrap();
        let notes = newest_first(&dir, |n| n.ends_with("-noreport.txt"));
        assert_eq!(notes.len(), KEEP_REPORTS, "{notes:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
