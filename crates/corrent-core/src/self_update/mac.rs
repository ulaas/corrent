//! Replacing `Corrent.app` in place on macOS (`InstallKind::MacApp`).
//!
//! The checked disk image is mounted read-only and hidden. The app in it must be the only app at
//! its root, have the running app's bundle identifier and the release's version, pass a deep,
//! strict signature check, be signed with a Developer ID of the running app's own team (read from
//! the running app's signature, so a fork signed by another team keeps updating itself) and be
//! accepted by Gatekeeper as notarized. Only then is it copied (`ditto`) to a hidden folder beside
//! the installed app, checked once more there, and the two are swapped by renaming on that one
//! volume: installed → `.Corrent.app.old-<pid>`, new → `Corrent.app`. Any failure renames the old
//! app back and deletes the copy; the installed path never holds a half-copied app. The old app
//! is deleted once the new one is set to start (the app quits, a small `sh` waits for it and runs
//! `open -n`); what a crash leaves behind goes at the next start (`cleanup`).
//!
//! The parts that decide (requirement, parsing, the swap) are plain functions that the tests run
//! on any platform; the parts that run Apple's tools are macOS only.

use std::path::{Path, PathBuf};

/// `Corrent.app` of `…/Corrent.app/Contents/MacOS/corrent`.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |p: &Path, n: &str| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(n));
    let is_app = bundle.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (named(macos, "MacOS") && named(contents, "Contents") && is_app && bundle.parent().is_some())
        .then(|| bundle.to_path_buf())
}

/// An Apple team identifier: ten upper-case letters and digits.
pub fn valid_team(team: &str) -> bool {
    team.len() == 10 && team.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// The code requirement the new app must satisfy: Apple's anchor, a Developer ID intermediate,
/// a Developer ID Application leaf, and the running app's team.
pub fn requirement(team: &str) -> String {
    // A code requirement, not text for people.
    const DEVELOPER_ID: &str = "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and \
         certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = ";
    [DEVELOPER_ID, "\"", team, "\""].concat()
}

/// The team in `codesign -dv --verbose=2` output; `None` for an ad hoc signature, "not set", or
/// anything that isn't a team identifier.
pub fn parse_team(codesign_output: &str) -> Option<String> {
    let lines = || codesign_output.lines().map(str::trim);
    if lines().any(|l| l == "Signature=adhoc") {
        return None;
    }
    let team = lines().find_map(|l| l.strip_prefix("TeamIdentifier="))?.trim();
    valid_team(team).then(|| team.to_string())
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The first `mount-point` in `hdiutil attach -plist` output.
pub fn parse_mount_point(plist: &str) -> Option<PathBuf> {
    let after = plist.split("<key>mount-point</key>").nth(1)?;
    let value = after.trim_start().strip_prefix("<string>")?;
    let end = value.find("</string>")?;
    let path = xml_unescape(value.get(..end)?);
    path.starts_with('/').then(|| PathBuf::from(path))
}

fn sibling(bundle: &Path, what: &str, pid: u32) -> PathBuf {
    let name = bundle.file_name().unwrap_or_default().to_string_lossy();
    bundle.with_file_name(format!(".{name}.{what}-{pid}"))
}

/// Where the new version is copied before the swap: `<folder>/.Corrent.app.update-<pid>`.
pub fn staging_path(bundle: &Path, pid: u32) -> PathBuf {
    sibling(bundle, "update", pid)
}

/// Where the installed version goes during the swap: `<folder>/.Corrent.app.old-<pid>`.
pub fn backup_path(bundle: &Path, pid: u32) -> PathBuf {
    sibling(bundle, "old", pid)
}

/// Whether `name` (in the bundle's folder) is a staging copy or a backup of `bundle_name`.
pub fn is_leftover(name: &str, bundle_name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_prefix(bundle_name)) else { return false };
    let Some(pid) = rest.strip_prefix(".update-").or_else(|| rest.strip_prefix(".old-")) else { return false };
    !pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit())
}

/// Removes staging copies and backups an earlier update of `bundle` left in its folder (a crash
/// in the middle, a backup that couldn't be deleted). Never this process's own. Returns how many.
pub fn cleanup(bundle: &Path) -> usize {
    let (Some(folder), Some(name)) = (bundle.parent(), bundle.file_name()) else { return 0 };
    let name = name.to_string_lossy();
    let mine = [staging_path(bundle, std::process::id()), backup_path(bundle, std::process::id())];
    let mut gone = 0;
    for e in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = e.path();
        if is_leftover(&e.file_name().to_string_lossy(), &name)
            && !mine.contains(&path)
            && std::fs::remove_dir_all(&path).is_ok()
        {
            gone += 1;
        }
    }
    gone
}

/// The one app at the root of a mounted disk image (the `Applications` link beside it is not one).
pub fn the_app(root: &Path) -> Result<PathBuf, String> {
    let apps: Vec<PathBuf> = std::fs::read_dir(root)
        .map_err(|e| t!("The disk image couldn't be read: {e}", e = e))?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("app")))
        .map(|e| e.path())
        .collect();
    match apps.as_slice() {
        [app] if std::fs::symlink_metadata(app).is_ok_and(|m| m.is_dir()) => Ok(app.clone()),
        _ => Err(t!("The disk image doesn't hold exactly one app. Nothing was installed.").into()),
    }
}

/// The new app is this app (same bundle identifier) in the release's version.
pub fn check_identity(
    id: Option<&str>,
    version: Option<&str>,
    want_id: &str,
    want_version: &str,
) -> Result<(), String> {
    if id == Some(want_id) && version == Some(want_version) {
        return Ok(());
    }
    Err(t!(
        "The app in the disk image is {id} {version}, not {want_id} {want_version}. Nothing was installed.",
        id = id.unwrap_or("?"),
        version = version.unwrap_or("?"),
        want_id = want_id,
        want_version = want_version
    ))
}

/// An installed app that was swapped for a new one, until the new one is known to start.
#[derive(Debug)]
pub struct BundleSwap {
    installed: PathBuf,
    backup: PathBuf,
}

impl BundleSwap {
    /// Puts the previous app back; the new one is deleted.
    pub fn rollback(self) -> Result<(), String> {
        let aside = staging_path(&self.installed, std::process::id());
        let _ = std::fs::remove_dir_all(&aside);
        let r = std::fs::rename(&self.installed, &aside).and_then(|()| {
            std::fs::rename(&self.backup, &self.installed).inspect_err(|_| {
                // Something stays under the name: the new version, then.
                let _ = std::fs::rename(&aside, &self.installed);
            })
        });
        let _ = std::fs::remove_dir_all(&aside);
        r.map_err(|e| {
            t!(
                "Couldn't put the previous version back ({path}: {e}). Download Corrent again from the release page.",
                path = self.installed.display(),
                e = e
            )
        })
    }

    /// The new version stays: the previous one is deleted (what can't be goes at the next start).
    pub fn finish(self) -> bool {
        std::fs::remove_dir_all(&self.backup).is_ok()
    }
}

/// Swaps the installed app for `staged` (a complete, checked copy in the same folder): the
/// installed one is renamed to `backup`, the staged one takes its name. If the second step fails,
/// the installed app is renamed back. `staged` is the caller's to delete on an error.
pub fn swap_bundle(installed: &Path, staged: &Path, backup: &Path) -> Result<BundleSwap, String> {
    swap_bundle_with(installed, staged, backup, || Ok(()))
}

/// `swap_bundle` with a step before the new app is put in place (tests make it fail).
fn swap_bundle_with(
    installed: &Path,
    staged: &Path,
    backup: &Path,
    before_put: impl FnOnce() -> std::io::Result<()>,
) -> Result<BundleSwap, String> {
    let failed = |e: std::io::Error| {
        t!(
            "Couldn't put the new version in place ({path}: {e}). The version you had is still there.",
            path = installed.display(),
            e = e
        )
    };
    if !staged.join("Contents").is_dir() || !installed.join("Contents").is_dir() {
        return Err(failed(std::io::ErrorKind::NotFound.into()));
    }
    // A leftover of an earlier attempt with this process id.
    if std::fs::symlink_metadata(backup).is_ok() {
        std::fs::remove_dir_all(backup).map_err(failed)?;
    }
    std::fs::rename(installed, backup).map_err(failed)?;
    if let Err(e) = before_put().and_then(|()| std::fs::rename(staged, installed)) {
        return Err(match std::fs::rename(backup, installed) {
            Ok(()) => failed(e),
            Err(worse) => t!(
                "Couldn't put the previous version back ({path}: {e}). Download Corrent again from the release page.",
                path = installed.display(),
                e = worse
            ),
        });
    }
    Ok(BundleSwap { installed: installed.to_path_buf(), backup: backup.to_path_buf() })
}

// ------------------------------------------------------------------------------- Apple's tools

#[cfg(target_os = "macos")]
pub use tools::{install, team_of};

#[cfg(target_os = "macos")]
mod tools {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::BundleSwap;

    // Full paths: a GUI app gets a minimal PATH.
    const HDIUTIL: &str = "/usr/bin/hdiutil";
    const CODESIGN: &str = "/usr/bin/codesign";
    const SPCTL: &str = "/usr/sbin/spctl";
    const DITTO: &str = "/usr/bin/ditto";
    const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";

    /// Runs a tool; `Ok` with its output (stdout, then stderr) when it exits with 0, `Err` with
    /// the last lines it printed otherwise.
    fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<String, String> {
        let out =
            Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{program}: {e}"))?;
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        if out.status.success() {
            return Ok(text);
        }
        let tail: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let tail = tail.get(tail.len().saturating_sub(2)..).unwrap_or_default().join(" ");
        let name = Path::new(program).file_name().unwrap_or_default().to_string_lossy();
        Err(format!("{name}: {} {tail}", out.status).trim().to_string())
    }

    fn os(s: &str) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(s)
    }

    /// The Developer ID team `bundle` is signed with (`None`: ad hoc, unsigned, unreadable).
    pub fn team_of(bundle: &Path) -> Option<String> {
        let out = run(CODESIGN, &[os("-dv"), os("--verbose=2"), bundle.as_os_str()]).ok()?;
        super::parse_team(&out)
    }

    /// A string from the bundle's Info.plist.
    fn info(bundle: &Path, key: &str) -> Option<String> {
        let plist = bundle.join("Contents/Info.plist");
        let print = format!("Print :{key}");
        run(PLIST_BUDDY, &[os("-c"), os(&print), plist.as_os_str()]).ok().map(|s| s.trim().to_string())
    }

    /// A disk image mounted read-only and hidden; detached when dropped.
    struct Mount(PathBuf);

    impl Mount {
        fn attach(dmg: &Path, under: &Path) -> Result<Mount, String> {
            let args = [
                os("attach"),
                os("-nobrowse"),
                os("-readonly"),
                os("-noautoopen"),
                os("-plist"),
                os("-mountrandom"),
                under.as_os_str(),
                dmg.as_os_str(),
            ];
            let out = run(HDIUTIL, &args).map_err(|e| t!("The disk image couldn't be opened: {e}", e = e))?;
            match super::parse_mount_point(&out) {
                Some(p) => Ok(Mount(p)),
                None => {
                    // Mounted somewhere unexpected, or not at all: whatever it attached goes.
                    for line in out.lines() {
                        if let Some(dev) = line.trim().strip_prefix("<string>/dev/disk") {
                            let dev = format!("/dev/disk{}", dev.trim_end_matches("</string>"));
                            let _ = run(HDIUTIL, &[os("detach"), os("-force"), os(&dev)]);
                        }
                    }
                    Err(t!("The disk image couldn't be opened: {e}", e = "mount-point?"))
                }
            }
        }
    }

    impl Drop for Mount {
        fn drop(&mut self) {
            if run(HDIUTIL, &[os("detach"), os("-quiet"), self.0.as_os_str()]).is_err()
                && let Err(e) = run(HDIUTIL, &[os("detach"), os("-quiet"), os("-force"), self.0.as_os_str()])
            {
                tracing::warn!("couldn't detach the update's disk image at {}: {e}", self.0.display());
            }
        }
    }

    /// The signature is complete and intact, and from a Developer ID of `team`.
    fn check_signature(app: &Path, team: &str) -> Result<(), String> {
        let not_signed = |e: String| {
            t!(
                "The app in the update isn't signed by the same developer as this one ({e}). Nothing was installed.",
                e = e
            )
        };
        run(CODESIGN, &[os("--verify"), os("--deep"), os("--strict"), app.as_os_str()]).map_err(not_signed)?;
        let requirement = format!("-R={}", super::requirement(team));
        run(CODESIGN, &[os("--verify"), os(&requirement), app.as_os_str()]).map_err(not_signed)?;
        Ok(())
    }

    /// Downloads checked, signature checked: mounts `dmg`, checks the app in it and swaps it for
    /// `bundle`. `work` is a folder for the mount point. On an error nothing installed has changed
    /// and nothing of the update is left beside it.
    pub fn install(bundle: &Path, team: &str, dmg: &Path, version: &str, work: &Path) -> Result<BundleSwap, String> {
        let want_id = info(bundle, "CFBundleIdentifier")
            .ok_or_else(|| t!("This copy of Corrent can't update itself.").to_string())?;
        let under = work.join("mount");
        std::fs::create_dir_all(&under)
            .map_err(|e| t!("Can't write to {folder}: {e}", folder = under.display(), e = e))?;
        let mount = Mount::attach(dmg, &under)?;
        let app = super::the_app(&mount.0)?;
        super::check_identity(
            info(&app, "CFBundleIdentifier").as_deref(),
            info(&app, "CFBundleShortVersionString").as_deref(),
            &want_id,
            version,
        )?;
        check_signature(&app, team)?;
        run(SPCTL, &[os("--assess"), os("--type"), os("execute"), app.as_os_str()]).map_err(|e| {
            t!("macOS doesn't accept the app in the update as notarized ({e}). Nothing was installed.", e = e)
        })?;

        // Beside the installed app, so the swap is a rename on one volume.
        let staged = super::staging_path(bundle, std::process::id());
        let backup = super::backup_path(bundle, std::process::id());
        let _ = std::fs::remove_dir_all(&staged);
        let copied = run(DITTO, &[os("--noqtn"), app.as_os_str(), staged.as_os_str()])
            .map_err(|e| t!("Couldn't copy the new version next to the installed one: {e}", e = e))
            // The copy is what runs: the same checks again (a full disk shows up here).
            .and_then(|_| check_signature(&staged, team));
        drop(mount);
        let swapped = copied.and_then(|()| super::swap_bundle(bundle, &staged, &backup));
        if swapped.is_err() {
            let _ = std::fs::remove_dir_all(&staged);
        }
        swapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-macupdate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in app: an Info.plist with `version`, a program and a library.
    fn fake_app(at: &Path, version: &str) {
        std::fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        std::fs::create_dir_all(at.join("Contents/Frameworks")).unwrap();
        std::fs::write(at.join("Contents/Info.plist"), version).unwrap();
        std::fs::write(at.join("Contents/MacOS/corrent"), format!("program {version}")).unwrap();
        std::fs::write(at.join("Contents/Frameworks/libmpv.2.dylib"), format!("mpv {version}")).unwrap();
    }

    fn version_of(app: &Path) -> String {
        std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn the_bundle_is_found_from_its_program() {
        let b = |p: &str| bundle_of(Path::new(p));
        assert_eq!(b("/Applications/Corrent.app/Contents/MacOS/corrent"), Some("/Applications/Corrent.app".into()));
        assert_eq!(
            b("/Users/u/Apps/My Corrent.app/Contents/MacOS/corrent"),
            Some("/Users/u/Apps/My Corrent.app".into())
        );
        assert_eq!(b("/Users/u/corrent/target/release/corrent"), None);
        assert_eq!(b("/Applications/Corrent/Contents/MacOS/corrent"), None, "not an .app");
        assert_eq!(b("/Corrent.app/Resources/MacOS/corrent"), None);
    }

    #[test]
    fn the_requirement_pins_developer_id_and_the_team() {
        let r = requirement("N6FA79GV97");
        assert!(r.starts_with("anchor apple generic and "), "{r}");
        assert!(r.contains("certificate 1[field.1.2.840.113635.100.6.2.6] exists"), "Developer ID intermediate");
        assert!(r.contains("certificate leaf[field.1.2.840.113635.100.6.1.13] exists"), "Developer ID Application");
        assert!(r.ends_with("certificate leaf[subject.OU] = \"N6FA79GV97\""), "{r}");
        assert!(valid_team("N6FA79GV97"));
        for bad in ["", "not set", "N6FA79GV9", "N6FA79GV977", "n6fa79gv97", "N6FA79GV9\"", "N6FA79 V97"] {
            assert!(!valid_team(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_team_comes_from_the_running_apps_signature() {
        let developer_id = "Executable=/Applications/Corrent.app/Contents/MacOS/corrent\n\
            Identifier=io.github.ulaas.Corrent\nFormat=app bundle with Mach-O thin (arm64)\n\
            Authority=Developer ID Application: Aydemir Sahin (N6FA79GV97)\n\
            Authority=Developer ID Certification Authority\nAuthority=Apple Root CA\n\
            TeamIdentifier=N6FA79GV97\nRuntime Version=15.0.0\n";
        assert_eq!(parse_team(developer_id).as_deref(), Some("N6FA79GV97"));
        let adhoc = "Executable=/x/Corrent.app/Contents/MacOS/corrent\nSignature=adhoc\nTeamIdentifier=not set\n";
        assert_eq!(parse_team(adhoc), None);
        assert_eq!(parse_team("TeamIdentifier=not set\n"), None);
        assert_eq!(parse_team("/x/Corrent.app: code object is not signed at all\n"), None);
        // Nothing that could change the requirement it goes into.
        assert_eq!(parse_team("TeamIdentifier=AB\" or true\n"), None);
    }

    #[test]
    fn the_mount_point_is_read_from_hdiutils_plist() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>system-entities</key><array>
<dict><key>content-hint</key><string>GUID_partition_scheme</string><key>dev-entry</key><string>/dev/disk9</string></dict>
<dict><key>content-hint</key><string>Apple_HFS</string><key>dev-entry</key><string>/dev/disk9s1</string>
<key>mount-point</key>
	<string>/private/tmp/x/dmg.Ab &amp; c</string><key>volume-kind</key><string>hfs</string></dict>
</array></dict></plist>"#;
        assert_eq!(parse_mount_point(plist), Some("/private/tmp/x/dmg.Ab & c".into()));
        assert_eq!(parse_mount_point("<key>dev-entry</key><string>/dev/disk9</string>"), None);
        assert_eq!(parse_mount_point("<key>mount-point</key><string>relative</string>"), None);
    }

    #[test]
    fn the_new_app_must_be_the_only_one_and_this_app_in_the_new_version() {
        let image = scratch("image");
        assert!(the_app(&image).is_err(), "empty");
        fake_app(&image.join("Corrent.app"), "2.0.0");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/Applications", image.join("Applications")).unwrap();
        std::fs::write(image.join(".VolumeIcon.icns"), b"icon").unwrap();
        assert_eq!(the_app(&image).unwrap(), image.join("Corrent.app"));
        fake_app(&image.join("Other.app"), "2.0.0");
        assert!(the_app(&image).is_err(), "two apps");
        let _ = std::fs::remove_dir_all(image);

        let id = "io.github.ulaas.Corrent";
        assert!(check_identity(Some(id), Some("2.0.0"), id, "2.0.0").is_ok());
        assert!(check_identity(Some("com.example.Other"), Some("2.0.0"), id, "2.0.0").is_err());
        assert!(check_identity(Some(id), Some("1.9.0"), id, "2.0.0").is_err(), "an older or other version");
        assert!(check_identity(None, Some("2.0.0"), id, "2.0.0").is_err());
        assert!(check_identity(Some(id), None, id, "2.0.0").is_err());
    }

    #[test]
    fn the_app_is_swapped_whole_and_put_back_on_any_failure() {
        let dir = scratch("swap");
        let installed = dir.join("Corrent.app");
        let (staged, backup) = (staging_path(&installed, 7), backup_path(&installed, 7));
        assert_eq!(staged, dir.join(".Corrent.app.update-7"));
        assert_eq!(backup, dir.join(".Corrent.app.old-7"));

        // Putting the new one in place fails: the old one is back under its name, untouched.
        fake_app(&installed, "1.0.0");
        fake_app(&staged, "2.0.0");
        let e = swap_bundle_with(&installed, &staged, &backup, || Err(std::io::ErrorKind::Other.into())).unwrap_err();
        assert!(e.contains("still there"), "{e}");
        assert_eq!(version_of(&installed), "1.0.0");
        assert!(!backup.exists());
        assert_eq!(version_of(&staged), "2.0.0", "the caller deletes the copy");

        // It works; the backup holds the old one until `finish`.
        let swap = swap_bundle(&installed, &staged, &backup).unwrap();
        assert_eq!(version_of(&installed), "2.0.0");
        assert_eq!(version_of(&backup), "1.0.0");
        assert!(!staged.exists());
        // The new version couldn't be started: everything as before.
        swap.rollback().unwrap();
        assert_eq!(version_of(&installed), "1.0.0");
        assert_eq!(names(&dir), ["Corrent.app"]);

        // Again, and this time it stays.
        fake_app(&staged, "2.0.0");
        let swap = swap_bundle(&installed, &staged, &backup).unwrap();
        assert!(swap.finish());
        assert_eq!(version_of(&installed), "2.0.0");
        assert_eq!(std::fs::read_to_string(installed.join("Contents/Frameworks/libmpv.2.dylib")).unwrap(), "mpv 2.0.0");
        assert_eq!(names(&dir), ["Corrent.app"]);

        // A half-made copy (no Contents) is never put in place.
        std::fs::create_dir_all(&staged).unwrap();
        assert!(swap_bundle(&installed, &staged, &backup).is_err());
        assert_eq!(version_of(&installed), "2.0.0");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn leftovers_of_an_earlier_update_go_but_nothing_else() {
        let dir = scratch("leftovers");
        let installed = dir.join("Corrent.app");
        fake_app(&installed, "2.0.0");
        for other in
            [".Corrent.app.update-123", ".Corrent.app.old-9", "Other.app", ".Other.app.old-9", ".Corrent.app.old-x"]
        {
            std::fs::create_dir_all(dir.join(other)).unwrap();
        }
        // This process's own (an update in progress) stays.
        std::fs::create_dir_all(staging_path(&installed, std::process::id())).unwrap();
        assert_eq!(cleanup(&installed), 2);
        let mut want = vec![
            ".Corrent.app.old-x".to_string(),
            format!(".Corrent.app.update-{}", std::process::id()),
            ".Other.app.old-9".into(),
            "Corrent.app".into(),
            "Other.app".into(),
        ];
        want.sort();
        assert_eq!(names(&dir), want);
        assert!(is_leftover(".Corrent.app.update-1", "Corrent.app"));
        assert!(!is_leftover(".Corrent.app.update-", "Corrent.app"));
        assert!(!is_leftover("Corrent.app", "Corrent.app"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The real tools on an app signed ad hoc: the requirement turns it away.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_ad_hoc_app_has_no_team_and_fails_the_requirement() {
        let dir = scratch("adhoc");
        let app = dir.join("Fake.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/fake")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>fake</string><key>CFBundleIdentifier</key><string>test.corrent.fake</string>
<key>CFBundleShortVersionString</key><string>1.0.0</string></dict></plist>"#,
        )
        .unwrap();
        let signed = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .output()
            .unwrap();
        assert!(signed.status.success(), "{signed:?}");
        assert_eq!(team_of(&app), None, "ad hoc");
        let r = std::process::Command::new("/usr/bin/codesign")
            .arg("--verify")
            .arg(format!("-R={}", requirement("N6FA79GV97")))
            .arg(&app)
            .output()
            .unwrap();
        assert!(!r.status.success(), "the requirement is checked: {r:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
