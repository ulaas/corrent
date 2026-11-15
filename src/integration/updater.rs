//! The parts of updating in place that touch this process and the system: how this copy was
//! installed, starting the new version, and the helper that runs the MSI once the app has quit.
//! What is downloaded, how it is checked and how files are swapped is `corrent_core::self_update`.
//!
//! Getting out of the way and coming back:
//! - The data-folder lock (`corrent_core::lock`) is held for the life of a process, so "the lock
//!   is free" means "the previous Corrent is gone": its engine stopped and saved (up to 8 s), its
//!   single-instance pipe closed, its files no longer in use.
//! - After a file swap the app starts the new program with `--after-update` and quits. The new
//!   one waits for the lock before it does anything else; without the wait it would hand its
//!   launch to the old instance and exit.
//! - For the MSI the app starts a copy of itself from the profile's `update` folder with
//!   `--run-installer …` and quits. The copy waits for the lock, runs
//!   `msiexec /i <msi> /passive /norestart`, then starts the installed program. It never registers
//!   anything: the installer's own actions decide that, as they do for an upgrade by hand.
//! - macOS: once the new `Corrent.app` is in place, a detached `sh` waits for this process to be
//!   gone and starts the app with `open -n … --args --after-update` (through LaunchServices, as
//!   the Finder would; no Terminal).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use corrent_core::self_update::{self, Facts, InstallKind, Os};

/// The new version waits this long for the old one to quit.
const WAIT_FOR_PREVIOUS: Duration = Duration::from_secs(90);

/// The option a restarted Corrent is started with.
pub const AFTER_UPDATE: &str = "--after-update";
/// The option that turns a copy of Corrent into the MSI's runner.
pub const RUN_INSTALLER: &str = "--run-installer";

#[cfg(windows)]
fn msi_dir() -> Option<PathBuf> {
    windows_registry::CURRENT_USER
        .open(r"Software\Corrent\Installer")
        .and_then(|k| k.get_string("InstallDir"))
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn msi_dir() -> Option<PathBuf> {
    None
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// The app bundle this program runs from, with links resolved (`None`: not in a bundle).
#[cfg(target_os = "macos")]
fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    self_update::mac::bundle_of(&std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// Whether `path` belongs to the user this process runs as (a file made in the temporary folder
/// tells which user that is).
#[cfg(target_os = "macos")]
fn owned_by_me(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let probe = std::env::temp_dir().join(format!(".corrent-owner-{}", std::process::id()));
    let me = std::fs::write(&probe, b"").and_then(|()| std::fs::metadata(&probe)).map(|m| m.uid());
    let _ = std::fs::remove_file(&probe);
    matches!((me, std::fs::metadata(path)), (Ok(me), Ok(m)) if m.uid() == me)
}

/// What is known about the running copy.
pub fn facts() -> Facts {
    // As started, not resolved: the MSI's folder is compared with the registry's spelling. On
    // macOS resolved: the app that is replaced is the real one, wherever links point.
    let exe = std::env::current_exe().unwrap_or_default();
    #[cfg(target_os = "macos")]
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let portable_marker = exe.parent().is_some_and(|d| d.join("portable").exists());
    #[cfg(target_os = "macos")]
    let bundle = self_update::mac::bundle_of(&exe);
    Facts {
        portable_marker,
        portable_data: portable_marker || std::env::args().skip(1).any(|a| a == "--portable"),
        #[cfg(target_os = "macos")]
        mac_team: bundle.as_deref().and_then(self_update::mac::team_of),
        #[cfg(target_os = "macos")]
        mac_bundle_mine: bundle.as_deref().is_some_and(owned_by_me),
        #[cfg(not(target_os = "macos"))]
        mac_team: None,
        #[cfg(not(target_os = "macos"))]
        mac_bundle_mine: false,
        x86_64: cfg!(target_arch = "x86_64"),
        msi_dir: msi_dir(),
        appimage: std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()).map(PathBuf::from),
        flatpak: env_set("FLATPAK_ID"),
        snap: env_set("SNAP"),
        homebrew_cask: cfg!(target_os = "macos")
            && ["/opt/homebrew/Caskroom/corrent", "/usr/local/Caskroom/corrent"].iter().any(|p| Path::new(p).exists()),
        exe,
    }
}

/// How this copy was installed. Looks at the disk (and makes a probe file to see whether the
/// folder can be written): not for the UI thread.
pub fn detect() -> InstallKind {
    self_update::classify(Os::current(), &facts(), &|p| p.exists(), &self_update::dir_writable)
}

/// Removes what the last update left beside the program and in the profile (one pass; a file
/// still in use goes at the next start).
pub fn cleanup(data_dir: &Path) {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let appimage = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()).map(PathBuf::from);
    let folders: Vec<PathBuf> = exe_dir.into_iter().collect();
    #[allow(unused_mut, reason = "macOS adds to it")]
    let mut gone = self_update::cleanup(&folders, appimage.as_deref(), data_dir);
    #[cfg(target_os = "macos")]
    if let Some(bundle) = running_bundle() {
        gone += self_update::mac::cleanup(&bundle);
    }
    if gone > 0 {
        tracing::info!("removed {gone} leftover(s) of the last update");
    }
}

/// Waits until no other process holds the data folder (the previous version quitting), at most
/// `WAIT_FOR_PREVIOUS`. Returns whether it is free.
pub fn wait_for_previous(data_dir: &Path) -> bool {
    let started = Instant::now();
    let mut waited = false;
    loop {
        match corrent_core::lock::lock_data_dir(data_dir) {
            Ok(lock) => {
                drop(lock);
                if waited {
                    // The process is gone a moment after its lock; only then can its program
                    // file (set aside as `*.old`) be deleted, or replaced by an installer.
                    std::thread::sleep(Duration::from_millis(700));
                }
                return true;
            }
            Err(corrent_core::lock::LockError::Held) if started.elapsed() < WAIT_FOR_PREVIOUS => {
                waited = true;
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(_) => return false,
        }
    }
}

/// A command for a program that outlives this process: no console window, no inherited pipes.
fn detached(program: &Path) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c
}

/// The options this process was started with that the next one needs too.
fn kept_options() -> Vec<String> {
    std::env::args().skip(1).take_while(|a| a.starts_with("--")).filter(|a| a == "--portable" || a == "--tv").collect()
}

/// Starts the updated program; it waits for this process to quit (`AFTER_UPDATE`).
pub fn start_new_version(program: &Path) -> std::io::Result<()> {
    let mut c = detached(program);
    c.arg(AFTER_UPDATE).args(kept_options());
    if let Some(dir) = program.parent() {
        c.current_dir(dir);
    }
    c.spawn().map(|_| ())
}

/// macOS: starts `bundle` again once this process is gone, through LaunchServices
/// (`open -n <bundle> --args --after-update …`), from a detached `sh` in its own process group
/// with no terminal. Debug builds pass on the test hooks' environment (`open` starts apps with
/// the session's own).
#[cfg(target_os = "macos")]
pub fn relaunch_when_gone(bundle: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    // A process that ended but wasn't reaped yet (a zombie) counts as gone.
    const WAIT_THEN_OPEN: &str = r#"pid=$1; shift
while kill -0 "$pid" 2>/dev/null; do
  case $(/bin/ps -o stat= -p "$pid" 2>/dev/null) in Z*) break ;; esac
  sleep 0.2
done
exec /usr/bin/open "$@""#;
    let mut c = detached(Path::new("/bin/sh"));
    c.arg("-c").arg(WAIT_THEN_OPEN).arg("sh").arg(std::process::id().to_string()).arg("-n");
    #[cfg(debug_assertions)]
    for var in ["HOME", "CORRENT_DEV_UPDATE"] {
        if let Some(value) = std::env::var_os(var) {
            let mut pair = std::ffi::OsString::from(format!("{var}="));
            pair.push(value);
            c.arg("--env").arg(pair);
        }
    }
    c.arg(bundle).arg("--args").arg(AFTER_UPDATE).args(kept_options());
    c.current_dir("/").process_group(0);
    c.spawn().map(|_| ())
}

/// The program "Update and restart" starts once the files are swapped.
pub fn program_of(kind: &InstallKind) -> Option<PathBuf> {
    match kind {
        InstallKind::WindowsPortable { dir } => Some(dir.join("corrent.exe")),
        InstallKind::LinuxTarball { dir } => Some(dir.join("corrent")),
        InstallKind::AppImage { file } => Some(file.clone()),
        _ => None,
    }
}

/// Starts the MSI's runner: a copy of this program in `work` (outside the installation, which
/// the installer replaces), told what to install and what to start afterwards.
pub fn start_installer(msi: &Path, work: &Path, data_dir: &Path) -> std::io::Result<()> {
    let me = std::env::current_exe()?;
    let helper = work.join("corrent-update.exe");
    std::fs::copy(&me, &helper)?;
    let sha256 = self_update::file_sha256(msi)?;
    let mut c = detached(&helper);
    c.arg(RUN_INSTALLER)
        .arg(msi)
        .arg(sha256)
        .arg(data_dir)
        .arg(&me)
        .arg(corrent_i18n::current().code())
        .current_dir(work);
    c.spawn().map(|_| ())
}

/// `corrent --run-installer <msi> <sha256> <data folder> <program> <language>`: waits for the
/// app that started it to quit, installs the MSI with its progress window and starts the
/// installed program. On any failure it says so and starts the program that is there (the
/// installer rolls a failed upgrade back). Returns the exit code.
pub fn run_installer(args: &[String]) -> i32 {
    let [msi, sha256, data_dir, program, lang] = args else { return 2 };
    corrent_i18n::set_language(lang);
    let (msi, data_dir, program) = (Path::new(msi), Path::new(data_dir), Path::new(program));
    let fail = |text: String| {
        rfd::MessageDialog::new()
            .set_title("Corrent")
            .set_description(&text)
            .set_level(rfd::MessageLevel::Error)
            .show();
        if program.is_file() {
            let _ = detached(program).spawn();
        }
        1
    };
    if !wait_for_previous(data_dir) {
        // Still running: it has the files the installer would replace. Nothing to start either.
        rfd::MessageDialog::new()
            .set_title("Corrent")
            .set_description(t!("Corrent didn't close, so the update wasn't installed. Quit Corrent and update again."))
            .set_level(rfd::MessageLevel::Error)
            .show();
        return 1;
    }
    // The file that was checked is the file that runs.
    if self_update::file_sha256(msi).ok().as_deref() != Some(sha256.as_str()) {
        return fail(
            t!("The downloaded installer changed after it was checked, so it wasn't run. Update again.").into(),
        );
    }
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let status = detached(&system.join("System32").join("msiexec.exe")).args(self_update::msiexec_args(msi)).status();
    let code = status.as_ref().ok().and_then(|s| s.code());
    if !self_update::msi_succeeded(code) {
        let why = match (&status, code) {
            (Err(e), _) => e.to_string(),
            (_, Some(c)) => c.to_string(),
            _ => "?".into(),
        };
        return fail(t!(
            "The update wasn't installed (the installer ended with {code}). The version you had is still there.",
            code = why
        ));
    }
    match detached(program).spawn() {
        Ok(_) => 0,
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("Corrent")
                .set_description(t!(
                    "The update is installed, but Corrent couldn't be started ({e}). Start it from the Start menu.",
                    e = e
                ))
                .set_level(rfd::MessageLevel::Warning)
                .show();
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_version_waits_for_the_old_ones_lock() {
        let dir = std::env::temp_dir().join(format!("corrent-updater-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Nobody there: at once.
        let started = Instant::now();
        assert!(wait_for_previous(&dir));
        assert!(started.elapsed() < Duration::from_secs(2));
        // The old version quits a moment later.
        let lock = corrent_core::lock::lock_data_dir(&dir).unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            drop(lock);
        });
        let started = Instant::now();
        assert!(wait_for_previous(&dir));
        assert!(started.elapsed() >= Duration::from_millis(400), "it waited: {:?}", started.elapsed());
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_test_binary_is_not_a_package() {
        // target/debug/deps: nothing "Update and restart" would replace.
        let kind = detect();
        assert!(!kind.installs_itself(), "{kind:?}");
        assert!(program_of(&kind).is_none());
        // The runner refuses a command line that isn't its own.
        assert_eq!(run_installer(&["only".into(), "three".into(), "values".into()]), 2);
    }
}
