//! Hang detection (STABILITY.md O6). The engine actor says what it is doing (a breadcrumb: the
//! command or phase, and since when) and beats on every tick; the GUI beats every frame while
//! its window is shown. A `corrent-watchdog` thread writes `logs/hang-<time>.txt` once per
//! episode when the UI misses 10 s of frames or the engine is stuck on one thing for 30 s
//! (2 minutes while it stops a session; never while it starts one, which has its own deadline,
//! or shuts down). It only reports: nothing is killed. Time the computer spent asleep is never
//! a hang (I1).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;

/// What the engine actor is busy with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Doing {
    /// Waiting for a command or the next tick.
    Idle,
    /// The periodic update (snapshot, queue, limits).
    Tick,
    StartingSession,
    StoppingSession,
    Shutdown,
    /// A command, by its name (`ApplySettings`).
    Command(Arc<str>),
}

impl std::fmt::Display for Doing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Doing::Idle => f.write_str("idle"),
            Doing::Tick => f.write_str("tick"),
            Doing::StartingSession => f.write_str("starting the session"),
            Doing::StoppingSession => f.write_str("stopping the session"),
            Doing::Shutdown => f.write_str("shutting down"),
            Doing::Command(name) => write!(f, "command {name}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub doing: Doing,
    /// `now_ms()` when it started.
    pub since: u64,
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Milliseconds since the first call in this process (monotonic; includes time asleep).
pub fn now_ms() -> u64 {
    START.elapsed().as_millis() as u64
}

static CRUMB: LazyLock<ArcSwap<Crumb>> =
    LazyLock::new(|| ArcSwap::from_pointee(Crumb { doing: Doing::Idle, since: now_ms() }));
static ACTOR_BEAT: AtomicU64 = AtomicU64::new(0);
static UI_BEAT: AtomicU64 = AtomicU64::new(0);
static UI_SHOWN: AtomicBool = AtomicBool::new(false);
static COMMANDS: Mutex<VecDeque<Arc<str>>> = Mutex::new(VecDeque::new());
const KEEP_COMMANDS: usize = 20;

/// The engine actor starts on something (the crumb stays until the next one).
pub fn doing(what: Doing) {
    let now = now_ms();
    if let Doing::Command(name) = &what {
        let mut c = crate::util::lock(&COMMANDS);
        if c.len() == KEEP_COMMANDS {
            c.pop_front();
        }
        c.push_back(name.clone());
    }
    CRUMB.store(Arc::new(Crumb { doing: what, since: now }));
    ACTOR_BEAT.store(now, Ordering::Relaxed);
}

/// The engine actor is alive (each tick, and while a long phase reports progress).
pub fn actor_beat() {
    ACTOR_BEAT.store(now_ms(), Ordering::Relaxed);
}

pub fn crumb() -> Arc<Crumb> {
    CRUMB.load_full()
}

/// The last commands the engine took, oldest first (names only). Never blocks: empty if busy.
pub fn recent_commands() -> Vec<Arc<str>> {
    COMMANDS.try_lock().map(|c| c.iter().cloned().collect()).unwrap_or_default()
}

/// A command's name: its `Debug` form up to the first character that isn't part of a name,
/// so payloads are never formatted (`Add(Box<AddRequest>)` is `Add`).
pub fn command_name(cmd: &impl std::fmt::Debug) -> Arc<str> {
    struct Name(String);
    impl std::fmt::Write for Name {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if !(c.is_ascii_alphanumeric() || c == '_') {
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut n = Name(String::new());
    let _ = write!(n, "{cmd:?}");
    n.0.into()
}

/// A frame of the GUI; `shown` is false while the window is hidden or minimized (no frames then).
pub fn ui_frame(shown: bool) {
    UI_BEAT.store(now_ms(), Ordering::Relaxed);
    UI_SHOWN.store(shown, Ordering::Relaxed);
}

/// The GUI is about to hide its window: frames stop, which isn't a hang.
pub fn ui_hidden() {
    UI_SHOWN.store(false, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// None: no GUI (correntd).
    pub ui: Option<Duration>,
    pub actor: Duration,
    /// While the actor stops a session (it waits for librqbit, then drains the engine calls).
    pub actor_stopping: Duration,
}

impl Limits {
    pub const GUI: Limits = Limits {
        ui: Some(Duration::from_secs(10)),
        actor: Duration::from_secs(30),
        actor_stopping: Duration::from_secs(120),
    };
    pub const SERVER: Limits = Limits { ui: None, ..Limits::GUI };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Ui,
    Engine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hang {
    pub side: Side,
    pub secs: u64,
    pub crumb: Crumb,
}

/// What the watchdog sees at one moment.
#[derive(Clone, Debug)]
pub struct Seen {
    pub now: u64,
    pub ui_beat: u64,
    pub ui_shown: bool,
    pub actor_beat: u64,
    pub crumb: Crumb,
    /// Beats older than this don't count (the watchdog itself was not running: asleep).
    pub awake_since: u64,
}

/// The hangs in `seen`, at most one per side.
pub fn check(seen: &Seen, limits: &Limits) -> Vec<Hang> {
    let mut v = Vec::new();
    let age = |beat: u64| seen.now.saturating_sub(beat.max(seen.awake_since));
    if let Some(limit) = limits.ui
        && seen.ui_shown
        && seen.ui_beat > 0
        && age(seen.ui_beat) > limit.as_millis() as u64
    {
        v.push(Hang { side: Side::Ui, secs: age(seen.ui_beat) / 1000, crumb: seen.crumb.clone() });
    }
    let limit = match seen.crumb.doing {
        // Starting has its own deadline (which grows with the library); shutting down is bounded.
        Doing::Shutdown | Doing::StartingSession => None,
        Doing::StoppingSession => Some(limits.actor_stopping),
        _ => Some(limits.actor),
    };
    if let Some(limit) = limit
        && seen.actor_beat > 0
    {
        // Busy on one thing too long, or not run at all (an idle actor still ticks).
        let busy = if seen.crumb.doing == Doing::Idle { 0 } else { age(seen.crumb.since) };
        let stuck = busy.max(age(seen.actor_beat));
        if stuck > limit.as_millis() as u64 {
            v.push(Hang { side: Side::Engine, secs: stuck / 1000, crumb: seen.crumb.clone() });
        }
    }
    v
}

/// The hang report's text (English, like crash reports).
pub fn report(hang: &Hang, commands: &[Arc<str>], log: &[String]) -> String {
    let mut out = String::new();
    let side = match hang.side {
        Side::Ui => "the window (UI thread)",
        Side::Engine => "the engine",
    };
    let ts = crate::util::now_unix();
    let _ = writeln!(out, "Corrent {} hang report", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "time: {} (unix {ts})", crate::util::fmt_date(ts));
    let _ = writeln!(out, "os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(out, "not responding: {side}, for {} s", hang.secs);
    let _ = writeln!(out, "engine busy with: {}", hang.crumb.doing);
    let _ = writeln!(out, "recent commands: {}", commands.join(", "));
    let _ = writeln!(out, "\nrecent log:");
    for l in log {
        let _ = writeln!(out, "{l}");
    }
    out
}

fn log_tail(n: usize) -> Vec<String> {
    let Ok(lines) = crate::logbuf::LINES.try_lock() else { return Vec::new() };
    let skip = lines.len().saturating_sub(n);
    lines.iter().skip(skip).map(|l| format!("{} {} {}: {}", l.time, l.level, l.target, l.message)).collect()
}

fn write_report(log_dir: &Path, hang: &Hang) -> Option<PathBuf> {
    let side = match hang.side {
        Side::Ui => "ui",
        Side::Engine => "engine",
    };
    let path = log_dir.join(format!("hang-{}-{side}.txt", crate::util::now_unix()));
    std::fs::write(&path, report(hang, &recent_commands(), &log_tail(100))).ok()?;
    Some(path)
}

/// Hang reports kept in the log folder.
const KEEP_REPORTS: usize = 5;

/// Starts the watchdog thread (once per process).
pub fn start(log_dir: PathBuf, limits: Limits) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("corrent-watchdog".into()).spawn(move || {
        let mut last = now_ms();
        let mut awake_since = 0;
        let mut reported = [(Side::Ui, false), (Side::Engine, false)];
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = now_ms();
            // A long gap in this loop: the computer slept. Beats from before don't count.
            if now - last > 5_000 {
                awake_since = now;
            }
            last = now;
            let seen = Seen {
                now,
                ui_beat: UI_BEAT.load(Ordering::Relaxed),
                ui_shown: UI_SHOWN.load(Ordering::Relaxed),
                actor_beat: ACTOR_BEAT.load(Ordering::Relaxed),
                crumb: (*crumb()).clone(),
                awake_since,
            };
            let hangs = check(&seen, &limits);
            for (side, reported) in &mut reported {
                let side = *side;
                match hangs.iter().find(|h| h.side == side) {
                    // Once per episode; the episode ends when the side responds again.
                    Some(h) if !*reported => {
                        *reported = true;
                        let path = write_report(&log_dir, h);
                        tracing::error!(
                            "{} not responding for {} s (engine busy with: {}); report: {}",
                            if side == Side::Ui { "the window is" } else { "the engine is" },
                            h.secs,
                            h.crumb.doing,
                            path.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
                        );
                        prune(&log_dir);
                    }
                    Some(_) => {}
                    None => *reported = false,
                }
            }
        }
    });
}

fn prune(log_dir: &Path) {
    let mut v: Vec<PathBuf> = std::fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("hang-") && n.ends_with(".txt"))
        })
        .collect();
    // Names start with the time: newest last.
    v.sort();
    let n = v.len().saturating_sub(KEEP_REPORTS);
    for p in v.into_iter().take(n) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(now: u64, doing: Doing, since: u64) -> Seen {
        Seen { now, ui_beat: now, ui_shown: true, actor_beat: now, crumb: Crumb { doing, since }, awake_since: 0 }
    }

    fn cmd(name: &str) -> Doing {
        Doing::Command(name.into())
    }

    #[test]
    fn hangs_are_found_and_named() {
        let l = Limits::GUI;
        assert!(check(&seen(100_000, Doing::Idle, 0), &l).is_empty(), "idle and ticking");
        assert!(check(&seen(100_000, cmd("Pause"), 90_000), &l).is_empty(), "busy for 10 s");
        let h = check(&seen(100_000, cmd("ApplySettings"), 60_000), &l);
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].side, h[0].secs), (Side::Engine, 40));
        assert_eq!(h[0].crumb.doing, cmd("ApplySettings"));
        // Not run at all: the beat stops even though nothing is in progress.
        let s = Seen { actor_beat: 50_000, ..seen(100_000, Doing::Idle, 0) };
        assert_eq!(check(&s, &l)[0].side, Side::Engine);
        // A session that stops slowly gets longer; starting (its own deadline) and shutting down never.
        assert!(check(&seen(100_000, Doing::StoppingSession, 40_000), &l).is_empty());
        assert_eq!(check(&seen(200_000, Doing::StoppingSession, 40_000), &l).len(), 1);
        let long = Seen { actor_beat: 1, ..seen(900_000, Doing::StartingSession, 1) };
        assert!(check(&long, &l).is_empty());
        assert!(check(&Seen { crumb: Crumb { doing: Doing::Shutdown, since: 1 }, ..long }, &l).is_empty());
        // The window: stale frames while shown; never while hidden, before the first frame, or in correntd.
        let ui = Seen { ui_beat: 85_000, ..seen(100_000, Doing::Idle, 0) };
        assert_eq!(check(&ui, &l)[0].side, Side::Ui);
        assert_eq!(check(&ui, &l)[0].secs, 15);
        assert!(check(&Seen { ui_shown: false, ..ui.clone() }, &l).is_empty());
        assert!(check(&Seen { ui_beat: 0, ..ui.clone() }, &l).is_empty());
        assert!(check(&ui, &Limits::SERVER).is_empty());
    }

    #[test]
    fn time_asleep_is_not_a_hang() {
        // Everything stopped at 10 s; the computer woke at 1000 s and the beats haven't come yet.
        let s = Seen {
            now: 1_002_000,
            ui_beat: 10_000,
            ui_shown: true,
            actor_beat: 10_000,
            crumb: Crumb { doing: cmd("Pause"), since: 9_000 },
            awake_since: 1_000_000,
        };
        assert!(check(&s, &Limits::GUI).is_empty());
        // Still stuck 40 s after waking: that is a hang.
        let later = Seen { now: 1_040_000, ..s };
        assert_eq!(check(&later, &Limits::GUI).len(), 2);
    }

    #[test]
    fn command_names_leave_out_the_payload() {
        #[derive(Debug)]
        #[allow(dead_code)]
        enum C {
            Pause(Vec<u8>),
            Flush,
            Move { to: String },
        }
        assert_eq!(&*command_name(&C::Pause(vec![1])), "Pause");
        assert_eq!(&*command_name(&C::Flush), "Flush");
        assert_eq!(&*command_name(&C::Move { to: "secret".into() }), "Move");
        let real = crate::engine::EngineCommand::SetDetailTarget(None);
        assert_eq!(&*command_name(&real), "SetDetailTarget");
    }

    #[test]
    fn a_hang_report_says_what_was_going_on() {
        let h = Hang { side: Side::Engine, secs: 42, crumb: Crumb { doing: cmd("DevHang"), since: 0 } };
        let text = report(&h, &["Pause".into(), "DevHang".into()], &["12:00 INFO x: y".into()]);
        assert!(text.contains("not responding: the engine, for 42 s"), "{text}");
        assert!(text.contains("engine busy with: command DevHang"), "{text}");
        assert!(text.contains("recent commands: Pause, DevHang"), "{text}");
        assert!(text.ends_with("recent log:\n12:00 INFO x: y\n"), "{text}");
        let dir = std::env::temp_dir().join(format!("corrent-hang-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..8 {
            std::fs::write(dir.join(format!("hang-100{i}-ui.txt")), "x").unwrap();
        }
        let p = write_report(&dir, &h).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("DevHang"));
        prune(&dir);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), KEEP_REPORTS);
        assert!(p.exists(), "the newest stays");
        let _ = std::fs::remove_dir_all(dir);
    }
}
