//! Fault injection for tests (STABILITY.md workstream I): slow disks, a stalled engine and
//! similar, scoped to one test's data folder (or torrent) because the tests of a crate run in one
//! process. Compiled out of normal builds; the `testkit` feature (only enabled by
//! dev-dependencies) keeps it for other crates' tests.

use std::path::Path;
use std::time::Duration;

#[cfg(any(test, feature = "testkit"))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    /// Rules by path prefix.
    pub struct Rules(Mutex<Vec<(PathBuf, Duration)>>);

    impl Rules {
        pub const fn new() -> Rules {
            Rules(Mutex::new(Vec::new()))
        }
        pub fn set(&self, prefix: &Path, d: Duration) {
            let mut v = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            v.retain(|(p, _)| p != prefix);
            v.push((prefix.to_path_buf(), d));
        }
        pub fn clear(&self, prefix: &Path) {
            self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(p, _)| !p.starts_with(prefix));
        }
        pub fn get(&self, path: &Path) -> Option<Duration> {
            let v = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            v.iter().find(|(p, _)| path.starts_with(p)).map(|(_, d)| *d)
        }
    }

    pub static FS_DELAY: Rules = Rules::new();
    pub static START_DELAY: Rules = Rules::new();
    pub static START_DEADLINE: Rules = Rules::new();
    pub static HTTP_TIMEOUT: Rules = Rules::new();
    static CALL_DELAY: Mutex<Vec<([u8; 20], Duration)>> = Mutex::new(Vec::new());

    pub fn set_call_delay(hash: [u8; 20], delay: Duration) {
        let mut v = CALL_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|(h, _)| *h != hash);
        v.push((hash, delay));
    }

    pub fn clear_call_delay(hash: [u8; 20]) {
        CALL_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(h, _)| *h != hash);
    }

    pub fn call_delay(hash: [u8; 20]) -> Option<Duration> {
        let v = CALL_DELAY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.iter().find(|(h, _)| *h == hash).map(|(_, d)| *d)
    }

    /// Write errors for profile files by path prefix: (prefix, Windows/Unix error codes, times
    /// left; None = until cleared).
    /// (prefix, (Windows code, Unix code), times left).
    type FileError = (PathBuf, (i32, i32), Option<u32>);
    static FILE_ERROR: Mutex<Vec<FileError>> = Mutex::new(Vec::new());

    pub fn set_file_error(prefix: &Path, codes: (i32, i32), times: Option<u32>) {
        let mut v = FILE_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|(p, _, _)| p != prefix);
        v.push((prefix.to_path_buf(), codes, times));
    }

    pub fn clear_file_errors(prefix: &Path) {
        FILE_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(p, _, _)| !p.starts_with(prefix));
    }

    static FREE_SPACE: Mutex<Vec<(PathBuf, u64)>> = Mutex::new(Vec::new());

    pub fn set_free_space(prefix: &Path, bytes: Option<u64>) {
        let mut v = FREE_SPACE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|(p, _)| p != prefix);
        if let Some(b) = bytes {
            v.push((prefix.to_path_buf(), b));
        }
    }

    pub fn free_space(path: &Path) -> Option<u64> {
        let v = FREE_SPACE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.iter().find(|(p, _)| path.starts_with(p)).map(|(_, b)| *b)
    }

    pub fn file_error(path: &Path) -> Option<std::io::Error> {
        let mut v = FILE_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (_, (win, unix), times) = v.iter_mut().find(|(p, _, t)| path.starts_with(p.as_path()) && *t != Some(0))?;
        if let Some(n) = times {
            *n -= 1;
        }
        Some(std::io::Error::from_raw_os_error(if cfg!(windows) { *win } else { *unix }))
    }

    /// A pretend sleep, by profile folder: added to the next tick's gap, once.
    static TICK_JUMP: Mutex<Vec<(PathBuf, Duration)>> = Mutex::new(Vec::new());

    pub fn jump_ticks(scope: &Path, d: Duration) {
        TICK_JUMP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((scope.to_path_buf(), d));
    }

    pub fn take_tick_jump(data_dir: &Path) -> Duration {
        let mut v = TICK_JUMP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (now, later): (Vec<_>, Vec<_>) = v.drain(..).partition(|(p, _)| data_dir.starts_with(p.as_path()));
        *v = later;
        now.into_iter().map(|(_, d)| d).sum()
    }

    /// Storage faults by profile folder, with the bytes written since each was set.
    static STORAGE: Mutex<Vec<(PathBuf, super::StorageFault, u64)>> = Mutex::new(Vec::new());

    pub fn set_storage_fault(scope: &Path, f: super::StorageFault) {
        STORAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((scope.to_path_buf(), f, 0));
    }

    pub fn clear_storage_faults(scope: &Path) {
        STORAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(p, _, _)| !p.starts_with(scope));
    }

    pub fn storage_check(scope: &Path, op: super::StorageOp, bytes: u64) -> std::io::Result<()> {
        use super::StorageFaultKind as K;
        let mut delay = None;
        let mut fail = None;
        {
            let mut v = STORAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (p, f, written) in v.iter_mut() {
                if !scope.starts_with(p.as_path()) || f.op != op {
                    continue;
                }
                match f.kind {
                    K::DiskFull { after_bytes } => {
                        if *written + bytes > after_bytes {
                            fail = Some(std::io::Error::from_raw_os_error(if cfg!(windows) { 112 } else { 28 }));
                        } else {
                            *written += bytes;
                        }
                    }
                    K::Permission => fail = Some(std::io::Error::from_raw_os_error(if cfg!(windows) { 5 } else { 13 })),
                    K::Eio => fail = Some(std::io::Error::from_raw_os_error(if cfg!(windows) { 1117 } else { 5 })),
                    K::Missing => fail = Some(std::io::Error::from_raw_os_error(if cfg!(windows) { 3 } else { 2 })),
                    K::Delay(d) => delay = Some(d),
                }
            }
        }
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        fail.map_or(Ok(()), Err)
    }
}

#[cfg(any(test, feature = "testkit"))]
pub use imp::{clear_call_delay, clear_storage_faults, set_call_delay, set_storage_fault, storage_check};

/// Tests: writes of profile files under `prefix` (store.json, settings, feeds…) fail with a full
/// disk, `times` times (None: until cleared).
#[cfg(any(test, feature = "testkit"))]
pub fn set_disk_full(prefix: &Path, times: Option<u32>) {
    imp::set_file_error(prefix, (112, 28), times);
}

/// Tests: the same with access denied.
#[cfg(any(test, feature = "testkit"))]
pub fn set_access_denied(prefix: &Path, times: Option<u32>) {
    imp::set_file_error(prefix, (5, 13), times);
}

#[cfg(any(test, feature = "testkit"))]
pub fn clear_file_errors(prefix: &Path) {
    imp::clear_file_errors(prefix);
}

/// Tests: the next engine tick of profiles under `scope` sees `d` more time passed (as after the
/// computer slept).
#[cfg(any(test, feature = "testkit"))]
pub fn jump_ticks(scope: &Path, d: Duration) {
    imp::jump_ticks(scope, d);
}

/// The pretend sleep for this profile's next tick (zero in release builds).
pub fn take_tick_jump(data_dir: &Path) -> Duration {
    #[cfg(any(test, feature = "testkit"))]
    return imp::take_tick_jump(data_dir);
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = data_dir;
        Duration::ZERO
    }
}

/// Tests: the free space reported for folders under `prefix` (None: the real value again).
#[cfg(any(test, feature = "testkit"))]
pub fn set_free_space(prefix: &Path, bytes: Option<u64>) {
    imp::set_free_space(prefix, bytes);
}

/// A test's free space for `path` (None in release builds: measure it).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(any(test, feature = "testkit"))]
    return imp::free_space(path);
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = path;
        None
    }
}

/// An injected error for writing `path` (test builds; never in release).
pub fn file_error(path: &Path) -> Option<std::io::Error> {
    #[cfg(any(test, feature = "testkit"))]
    return imp::file_error(path);
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = path;
        None
    }
}

/// Where in the engine's storage a fault strikes (S8, tests only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageOp {
    /// Opening the files (adding, resuming).
    Init,
    Write,
    Read,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageFaultKind {
    /// Writes fail once this many more bytes were written (ENOSPC / ERROR_DISK_FULL).
    DiskFull { after_bytes: u64 },
    /// Access denied (EACCES / ERROR_ACCESS_DENIED).
    Permission,
    /// An I/O error (EIO / ERROR_IO_DEVICE).
    Eio,
    /// The folder is gone (ENOENT / ERROR_PATH_NOT_FOUND).
    Missing,
    /// Every matching operation waits this long first.
    Delay(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageFault {
    pub op: StorageOp,
    pub kind: StorageFaultKind,
}

/// The engine's storage for a profile: with injected faults in test builds, librqbit's own
/// otherwise.
pub fn storage_factory(data_dir: &Path) -> Option<librqbit::storage::BoxStorageFactory> {
    #[cfg(any(test, feature = "testkit"))]
    return Some(crate::engine::faulty_storage::FaultyStorageFactory::boxed(data_dir));
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = data_dir;
        None
    }
}

/// A note when fault injection is compiled in (only test builds and the `testkit` or `fuzz`
/// feature).
/// The binaries log it at start, so `scripts/release-hygiene.ps1` finds the text in a binary
/// that got the feature by mistake.
pub fn compiled_in() -> Option<&'static str> {
    #[cfg(feature = "fuzz")]
    return Some(
        "CORRENT_FAULTS_COMPILED_IN, CORRENT_FUZZ_COMPILED_IN: fault injection, the test kit and the fuzz targets are in this build",
    );
    #[cfg(all(any(test, feature = "testkit"), not(feature = "fuzz")))]
    return Some("CORRENT_FAULTS_COMPILED_IN: fault injection and the test kit are in this build");
    #[cfg(not(any(test, feature = "testkit")))]
    None
}

/// Tests: file system work under `prefix` waits `d` first.
#[cfg(any(test, feature = "testkit"))]
pub fn set_fs_delay(prefix: &Path, d: Duration) {
    imp::FS_DELAY.set(prefix, d);
}

/// Tests: starting an engine whose data folder is under `prefix` takes `d` longer.
#[cfg(any(test, feature = "testkit"))]
pub fn set_start_delay(prefix: &Path, d: Duration) {
    imp::START_DELAY.set(prefix, d);
}

/// Tests: an engine whose data folder is under `prefix` must start within `d`.
#[cfg(any(test, feature = "testkit"))]
pub fn set_start_deadline(prefix: &Path, d: Duration) {
    imp::START_DEADLINE.set(prefix, d);
}

/// Tests: the engine HTTP requests of a data folder under `prefix` time out after `d`.
#[cfg(any(test, feature = "testkit"))]
pub fn set_http_timeout(prefix: &Path, d: Duration) {
    imp::HTTP_TIMEOUT.set(prefix, d);
}

/// Tests: drop every rule for paths under `prefix`.
#[cfg(any(test, feature = "testkit"))]
pub fn clear(prefix: &Path) {
    imp::FS_DELAY.clear(prefix);
    imp::START_DELAY.clear(prefix);
    imp::START_DEADLINE.clear(prefix);
    imp::HTTP_TIMEOUT.clear(prefix);
}

/// A test's slow disk: file system work on `path` waits this long first (blocking).
pub fn fs_delay(path: &Path) {
    if let Some(d) = lookup(Kind::Fs, path) {
        std::thread::sleep(d);
    }
}

/// A test's slow engine start for the data folder `path`.
pub fn start_delay(path: &Path) -> Option<Duration> {
    lookup(Kind::StartDelay, path)
}

/// A test's start deadline for the data folder `path`.
pub fn start_deadline(path: &Path) -> Option<Duration> {
    lookup(Kind::StartDeadline, path)
}

/// A test's HTTP timeout for the engine of the data folder `path`.
pub fn http_timeout(path: &Path) -> Option<Duration> {
    lookup(Kind::HttpTimeout, path)
}

/// A test's stalled engine: librqbit calls for the torrent `hash` hang this long first.
pub fn call_delay(hash: [u8; 20]) -> Option<Duration> {
    #[cfg(any(test, feature = "testkit"))]
    {
        imp::call_delay(hash)
    }
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = hash;
        None
    }
}

/// STABILITY.md D8: a named point of a durable write. `op` says what is written, `phase` how far
/// the write got. With fault injection compiled in, the harness (`corrent-harness`) uses it to
/// end the process right there (`CORRENT_DEV_KILL_AT=<op>:<phase>:<n>`, the n-th time) or to
/// list the points a run reaches (`CORRENT_DEV_KILL_LOG=<file>`). Otherwise it does nothing.
pub fn kill_point(op: &str, phase: &str) {
    #[cfg(any(test, feature = "testkit"))]
    kill::reached(op, phase);
    #[cfg(not(any(test, feature = "testkit")))]
    let _ = (op, phase);
}

/// [`kill_point`] for a write of `path`, named after the file.
pub fn kill_point_at(path: &Path, phase: &str) {
    #[cfg(any(test, feature = "testkit"))]
    kill::reached(&op_of(path), phase);
    #[cfg(not(any(test, feature = "testkit")))]
    let _ = (path, phase);
}

/// The kill-point name of a file Corrent writes: the same in every run, whatever the folder or
/// the time in the name.
#[cfg(any(test, feature = "testkit"))]
pub fn op_of(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = |n: &str| match n {
        "store.json" => "store",
        "settings.toml" => "settings",
        "rss.json" => "rss",
        "session.json" => "session-recover",
        _ => "other",
    };
    if dir == "torrents" {
        return if name.ends_with(".torrent") { "readd-backup" } else { "readd-note" }.into();
    }
    if dir == "ipfilter" {
        return "ipfilter".into();
    }
    for tag in ["daily", "good", "bad", "pre-prune"] {
        if let Some((file, _)) = name.split_once(&format!(".{tag}-")) {
            return format!("gen-{tag}-{}", base(file));
        }
    }
    // Migration backups: `<file>.v<N>-<time>`.
    for file in ["settings.toml", "store.json"] {
        if name.strip_prefix(file).is_some_and(|r| r.starts_with(".v")) {
            return format!("migrate-{}", base(file));
        }
    }
    base(&name).into()
}

#[cfg(any(test, feature = "testkit"))]
mod kill {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};

    struct Plan {
        op: String,
        phase: String,
        nth: u64,
    }

    struct State {
        plan: Option<Plan>,
        log: Option<std::fs::File>,
        counts: HashMap<(String, String), u64>,
    }

    fn state() -> &'static Mutex<State> {
        static S: OnceLock<Mutex<State>> = OnceLock::new();
        S.get_or_init(|| {
            let plan = std::env::var("CORRENT_DEV_KILL_AT").ok().and_then(|v| {
                let mut it = v.splitn(3, ':');
                let (op, phase, nth) = (it.next()?, it.next()?, it.next()?.parse().ok()?);
                Some(Plan { op: op.into(), phase: phase.into(), nth })
            });
            let log = std::env::var_os("CORRENT_DEV_KILL_LOG")
                .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
            Mutex::new(State { plan, log, counts: HashMap::new() })
        })
    }

    thread_local! {
        /// Unit tests: the points this thread reaches, instead of the process-wide plan.
        static RECORD: std::cell::RefCell<Option<Vec<(String, String)>>> = const { std::cell::RefCell::new(None) };
    }

    pub fn reached(op: &str, phase: &str) {
        let recorded = RECORD.with(|r| r.borrow_mut().as_mut().map(|v| v.push((op.into(), phase.into()))).is_some());
        if recorded {
            return;
        }
        let mut st = crate::util::lock(state());
        if st.plan.is_none() && st.log.is_none() {
            return;
        }
        let n = {
            let c = st.counts.entry((op.into(), phase.into())).or_default();
            *c += 1;
            *c
        };
        if let Some(f) = st.log.as_mut() {
            // Unbuffered: the line is the OS's once written, even if the process ends next.
            let _ = writeln!(f, "{op}:{phase}");
        }
        if st.plan.as_ref().is_some_and(|p| p.op == op && p.phase == phase && p.nth == n) {
            terminate();
        }
    }

    /// End the process at once: no destructors, no flushing, like a power cut for the process
    /// (the OS still writes what it already has).
    fn terminate() -> ! {
        #[cfg(windows)]
        {
            unsafe extern "system" {
                fn GetCurrentProcess() -> isize;
                fn TerminateProcess(process: isize, code: u32) -> i32;
            }
            // SAFETY: the pseudo handle of this process; TerminateProcess doesn't return.
            unsafe {
                TerminateProcess(GetCurrentProcess(), super::KILLED_EXIT_CODE);
            }
        }
        std::process::abort()
    }

    /// Run `f` and return the kill points it reached on this thread (nothing is killed).
    pub fn record(f: impl FnOnce()) -> Vec<(String, String)> {
        RECORD.with(|r| *r.borrow_mut() = Some(Vec::new()));
        f();
        RECORD.with(|r| r.borrow_mut().take()).unwrap_or_default()
    }
}

/// Also stop at librqbit's kill points (its persistence writes). Call before starting an engine.
#[cfg(any(test, feature = "testkit"))]
pub fn init_kill_points() {
    librqbit::corrent_hooks::set_kill_hook(kill::reached);
}

/// STABILITY.md D10: a step that a test can make fail (on this thread, see [`fail_nth`]).
pub fn fail_point(op: &str) -> std::io::Result<()> {
    #[cfg(any(test, feature = "testkit"))]
    if FAIL.with(|f| {
        let mut f = f.borrow_mut();
        let fire = match f.as_mut() {
            Some((o, n)) if o == op => {
                *n = n.saturating_sub(1);
                *n == 0
            }
            _ => false,
        };
        if fire {
            *f = None; // once
        }
        fire
    }) {
        return Err(std::io::Error::other("injected failure"));
    }
    #[cfg(not(any(test, feature = "testkit")))]
    let _ = op;
    Ok(())
}

#[cfg(any(test, feature = "testkit"))]
thread_local! {
    static FAIL: std::cell::RefCell<Option<(String, u32)>> = const { std::cell::RefCell::new(None) };
}

/// Tests: the `n`-th [`fail_point`] `op` on this thread fails (once); 0 disarms.
#[cfg(any(test, feature = "testkit"))]
pub fn fail_nth(op: &str, n: u32) {
    FAIL.with(|f| *f.borrow_mut() = (n > 0).then(|| (op.to_string(), n)));
}

/// The exit code of a process ended at a kill point (Windows; Unix ends with SIGABRT).
#[cfg(any(test, feature = "testkit"))]
pub const KILLED_EXIT_CODE: u32 = 75;

/// Tests: the kill points `f` reaches on this thread, in order.
#[cfg(any(test, feature = "testkit"))]
pub fn record_kill_points(f: impl FnOnce()) -> Vec<(String, String)> {
    kill::record(f)
}

enum Kind {
    Fs,
    StartDelay,
    StartDeadline,
    HttpTimeout,
}

fn lookup(kind: Kind, path: &Path) -> Option<Duration> {
    #[cfg(any(test, feature = "testkit"))]
    {
        match kind {
            Kind::Fs => imp::FS_DELAY.get(path),
            Kind::StartDelay => imp::START_DELAY.get(path),
            Kind::StartDeadline => imp::START_DEADLINE.get(path),
            Kind::HttpTimeout => imp::HTTP_TIMEOUT.get(path),
        }
    }
    #[cfg(not(any(test, feature = "testkit")))]
    {
        let _ = (kind, path);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reach_their_kill_points_in_order() {
        let d = std::env::temp_dir().join(format!("corrent-killpoints-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let store = d.join("store.json");
        let first = record_kill_points(|| crate::util::write_atomic(&store, b"1").unwrap());
        let names = |v: &[(String, String)]| v.iter().map(|(o, p)| format!("{o}:{p}")).collect::<Vec<_>>();
        assert_eq!(names(&first), ["store:before_tmp", "store:after_tmp", "store:after_rename"]);
        // The second save refreshes the missing .bak first.
        let second = record_kill_points(|| crate::util::write_atomic(&store, b"2").unwrap());
        assert_eq!(
            names(&second),
            [
                "store:before_tmp",
                "store:after_tmp",
                "store:bak_after_tmp",
                "store:bak_after_rename",
                "store:after_bak",
                "store:after_rename"
            ]
        );
        let generation = record_kill_points(|| {
            crate::util::keep_generations(&store, "daily", b"3", 3).unwrap();
        });
        assert_eq!(
            names(&generation),
            ["gen-daily-store:after_tmp", "gen-daily-store:after_rename", "gen-daily-store:after_prune"]
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn kill_point_names_dont_depend_on_folders_or_times() {
        let p = |s: &str| op_of(Path::new(s));
        assert_eq!(p("C:/x/data/settings.toml"), "settings");
        assert_eq!(p("/d/data/rss.json"), "rss");
        assert_eq!(p("/d/data/store.json.pre-prune-1700000000"), "gen-pre-prune-store");
        assert_eq!(p("/d/data/session/session.json.good-17"), "gen-good-session-recover");
        assert_eq!(p("/d/data/session/session.json"), "session-recover");
        assert_eq!(p("/d/data/settings.toml.v0-1700000000"), "migrate-settings");
        assert_eq!(p("/d/data/torrents/ab.torrent"), "readd-backup");
        assert_eq!(p("/d/data/torrents/ab.json"), "readd-note");
        assert_eq!(p("/d/data/ipfilter/blocklist-download.txt"), "ipfilter");
    }
}
