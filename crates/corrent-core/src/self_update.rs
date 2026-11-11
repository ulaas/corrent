//! Updating Corrent in place, on the user's request. `updates` finds a newer release; this module
//! knows how the running copy was installed (`InstallKind`), which file of the release replaces
//! it, how that file is checked and how it is put in place. Nothing here runs before the user
//! clicks "Update and restart".
//!
//! What is trusted: GitHub over HTTPS. The download is installed only if its SHA-256 matches the
//! release's `SHA256SUMS.txt` and the digest GitHub's API reports for the file; every source the
//! release has must agree, and it must have at least one. Only addresses under the project's own
//! releases are fetched. Releases are not signed, so this protects against a damaged or swapped
//! download, not against someone who can publish a release.
//!
//! How each kind is updated (docs/PACKAGING.md has the long version):
//! - Windows portable zip, Linux tarball: the new files are unpacked beside the old ones, the old
//!   ones are set aside as `*.old` (a running program can be renamed, not overwritten) and the new
//!   ones take their names; any failure puts everything back. The next start deletes the leftovers.
//! - AppImage: the new file is renamed over the old one, which is kept as `*.old` until the next start.
//! - Windows MSI: the installer is downloaded and checked here; the app hands it to a copy of
//!   itself that waits for the app to quit, runs `msiexec /i … /passive` and starts the new version.
//! - macOS: the disk image is downloaded and checked, then opened for the user to drag across.
//! - Scoop, Homebrew, system packages, Flatpak, Snap, read-only folders: never touched; the app
//!   says which command updates it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::updates::{Asset, Release};

/// Corrent's packages are 30–80 MiB; anything far bigger is not one.
pub const MAX_DOWNLOAD: u64 = 600 << 20;
/// `SHA256SUMS.txt` is a few hundred bytes.
const MAX_SUMS: usize = 1 << 20;
/// The release's checksum list.
pub const SUMS: &str = "SHA256SUMS.txt";
/// What a file that was replaced is called until the next start removes it.
pub const OLD_SUFFIX: &str = ".old";
/// The folder made beside an installation while it is being updated starts with this.
pub const WORK_PREFIX: &str = ".corrent-update-";
/// Downloads that don't go beside the installation (the MSI) go into this folder of the profile.
pub const DATA_SUBDIR: &str = "update";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Mac,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
}

/// What keeps a copy of Corrent up to date instead of Corrent itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Scoop,
    Homebrew,
    /// A distribution package (the AUR's `corrent-bin`, or anything else under `/usr`).
    SystemPackage,
    Flatpak,
    Snap,
}

impl Manager {
    /// The command that updates Corrent there.
    pub fn command(self) -> &'static str {
        match self {
            Manager::Scoop => "scoop update corrent",
            Manager::Homebrew => "brew upgrade --cask corrent",
            Manager::SystemPackage => "yay -S corrent-bin",
            Manager::Flatpak => "flatpak update io.github.ulaas.Corrent",
            Manager::Snap => "snap refresh corrent",
        }
    }

    /// One sentence for the user.
    pub fn advice(self) -> String {
        match self {
            Manager::Scoop => t!("This copy was installed with Scoop, which also updates it."),
            Manager::Homebrew => t!("This copy was installed with Homebrew, which also updates it."),
            Manager::SystemPackage => {
                t!("This copy was installed by your system's package manager, which also updates it. From the AUR, for example:")
            }
            Manager::Flatpak => t!("This copy is a Flatpak, which Flatpak updates."),
            Manager::Snap => t!("This copy is a Snap, which snapd updates."),
        }
        .to_string()
    }
}

/// How the running copy of Corrent got onto this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// The portable zip, unpacked into `dir` (with its `portable` marker).
    WindowsPortable { dir: PathBuf },
    /// The per-user MSI, installed into `dir`.
    WindowsMsi { dir: PathBuf },
    /// An AppImage the user keeps at `file`.
    AppImage { file: PathBuf },
    /// The Linux tarball's two programs in a folder the user can write to.
    LinuxTarball { dir: PathBuf },
    /// `Corrent.app` from the disk image.
    MacApp { bundle: PathBuf },
    /// A package manager owns this copy.
    Managed(Manager),
    /// A known kind in a place Corrent can't write to.
    ReadOnly,
    /// Not a release package (a development build, a copy moved out of its package).
    Unknown,
}

impl InstallKind {
    /// Whether "Update and restart" replaces this copy (`false`: the disk image is only opened).
    pub fn installs_itself(&self) -> bool {
        matches!(
            self,
            InstallKind::WindowsPortable { .. }
                | InstallKind::WindowsMsi { .. }
                | InstallKind::AppImage { .. }
                | InstallKind::LinuxTarball { .. }
        )
    }

    /// Whether Corrent downloads anything for this copy.
    pub fn downloads(&self) -> bool {
        self.installs_itself() || matches!(self, InstallKind::MacApp { .. })
    }

    /// The exact name of the release file for this copy.
    pub fn asset_name(&self, version: &str) -> Option<String> {
        Some(match self {
            InstallKind::WindowsPortable { .. } => format!("Corrent-{version}-windows-portable.zip"),
            InstallKind::WindowsMsi { .. } => format!("Corrent-{version}-x86_64.msi"),
            InstallKind::AppImage { .. } => format!("Corrent-{version}-x86_64.AppImage"),
            InstallKind::LinuxTarball { .. } => format!("Corrent-{version}-linux-x86_64.tar.gz"),
            InstallKind::MacApp { .. } => format!("Corrent-{version}-macos.dmg"),
            InstallKind::Managed(_) | InstallKind::ReadOnly | InstallKind::Unknown => return None,
        })
    }
}

/// What is known about the running copy; `classify` turns it into an `InstallKind`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The running executable.
    pub exe: PathBuf,
    /// The releases are built for x86-64 (Windows, Linux).
    pub x86_64: bool,
    /// A `portable` file sits beside the executable.
    pub portable_marker: bool,
    /// `HKCU\Software\Corrent\Installer`'s `InstallDir` (the MSI writes it).
    pub msi_dir: Option<PathBuf>,
    /// `$APPIMAGE`.
    pub appimage: Option<PathBuf>,
    pub flatpak: bool,
    pub snap: bool,
    /// Homebrew has a `corrent` cask installed.
    pub homebrew_cask: bool,
}

/// Lower case with forward slashes and no trailing one, to compare folders.
fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/").to_lowercase();
    let s = s.strip_prefix("//?/").unwrap_or(&s);
    s.trim_end_matches('/').to_string()
}

/// A cargo target folder: a build someone made, not a package.
fn dev_build(dir: &Path, exists: &dyn Fn(&Path) -> bool) -> bool {
    exists(&dir.join("deps")) && exists(&dir.join("build"))
}

/// The kind of installation `f` describes. `exists` and `writable` look at the disk (tests answer
/// for them).
pub fn classify(os: Os, f: &Facts, exists: &dyn Fn(&Path) -> bool, writable: &dyn Fn(&Path) -> bool) -> InstallKind {
    let Some(dir) = f.exe.parent().map(Path::to_path_buf) else { return InstallKind::Unknown };
    let exe = norm(&f.exe);
    match os {
        Os::Windows => {
            if exe.contains("/scoop/apps/") {
                return InstallKind::Managed(Manager::Scoop);
            }
            if !f.x86_64 || !exe.ends_with("/corrent.exe") {
                return InstallKind::Unknown;
            }
            if f.msi_dir.as_deref().is_some_and(|m| norm(m) == norm(&dir)) {
                return InstallKind::WindowsMsi { dir };
            }
            if !f.portable_marker || dev_build(&dir, exists) || !exists(&dir.join("corrent-mpvw.exe")) {
                return InstallKind::Unknown;
            }
            if writable(&dir) { InstallKind::WindowsPortable { dir } } else { InstallKind::ReadOnly }
        }
        Os::Linux => {
            if f.flatpak {
                return InstallKind::Managed(Manager::Flatpak);
            }
            if f.snap {
                return InstallKind::Managed(Manager::Snap);
            }
            if let Some(file) = f.appimage.clone().filter(|p| !p.as_os_str().is_empty()) {
                if !f.x86_64 || !exists(&file) {
                    return InstallKind::Unknown;
                }
                let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
                return if writable(&folder) { InstallKind::AppImage { file } } else { InstallKind::ReadOnly };
            }
            if ["/usr/", "/opt/", "/bin/", "/sbin/", "/nix/", "/gnu/", "/snap/", "/app/"]
                .iter()
                .any(|p| exe.starts_with(p))
            {
                return InstallKind::Managed(Manager::SystemPackage);
            }
            if !f.x86_64 || !exe.ends_with("/corrent") || dev_build(&dir, exists) || !exists(&dir.join("corrent-mpvw"))
            {
                return InstallKind::Unknown;
            }
            if writable(&dir) { InstallKind::LinuxTarball { dir } } else { InstallKind::ReadOnly }
        }
        Os::Mac => {
            if !exe.contains(".app/contents/macos/") {
                return InstallKind::Unknown;
            }
            if f.homebrew_cask {
                return InstallKind::Managed(Manager::Homebrew);
            }
            // Run from the mounted disk image, or from wherever Gatekeeper moved a quarantined
            // app: there is no installed copy to speak of yet.
            if exe.starts_with("/volumes/") || exe.contains("/apptranslocation/") {
                return InstallKind::ReadOnly;
            }
            match f.exe.ancestors().nth(3) {
                Some(bundle) => InstallKind::MacApp { bundle: bundle.to_path_buf() },
                None => InstallKind::Unknown,
            }
        }
        Os::Other => InstallKind::Unknown,
    }
}

/// Whether a file can be made in `dir` (by making one).
pub fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!("{WORK_PREFIX}probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

// ------------------------------------------------------------------------------------ the plan

/// What "Update and restart" will fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub version: String,
    /// The release's page.
    pub page: String,
    pub asset: Asset,
    /// The release's `SHA256SUMS.txt`, when it has one.
    pub sums: Option<Asset>,
}

/// The file of `release` that updates an installation of `kind`, matched by its exact name.
pub fn plan(release: &Release, kind: &InstallKind) -> Result<Plan, String> {
    let name = kind.asset_name(&release.version).ok_or_else(|| t!("This copy of Corrent can't update itself."))?;
    let find = |n: &str| release.assets.iter().find(|a| a.name == n).cloned();
    let asset = find(&name).ok_or_else(|| {
        t!(
            "Corrent {version} has no download for this kind of installation ({file}).",
            version = release.version,
            file = name
        )
    })?;
    Ok(Plan { version: release.version.clone(), page: release.url.clone(), asset, sums: find(SUMS) })
}

/// The SHA-256 a `sha256sum` list gives for `name` ("<hash>  <name>", or "<hash> *<name>").
/// `None` when the file isn't listed, or is listed with different hashes.
pub fn parse_sums(text: &str, name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in text.lines() {
        let Some((hash, file)) = line.trim().split_once(char::is_whitespace) else { continue };
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        if file != name {
            continue;
        }
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let hash = hash.to_ascii_lowercase();
        if found.as_ref().is_some_and(|f| *f != hash) {
            return None;
        }
        found = Some(hash);
    }
    found
}

/// What is happening, for a progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Reading the release's checksums.
    Checksums,
    Download {
        done: u64,
        total: Option<u64>,
    },
    /// Putting the new version in place (no way back by cancelling from here).
    Install,
}

/// A client for the download: no limit on the whole transfer, only on connecting and on silence;
/// HTTPS only, redirects included. `Err` when it can't be built that way.
pub fn client(proxy: Option<String>) -> Result<reqwest::Client, String> {
    crate::mpv_get::client_builder(proxy).https_only(true).build().map_err(|e| e.to_string())
}

fn cancelled() -> String {
    t!("Cancelled.").into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a file on disk, in lower-case hex.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hash = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(buf.get(..n).unwrap_or_default());
    }
    Ok(hex(hash.finish().as_ref()))
}

/// Waits for `fut`, giving up as soon as `cancel` is set (a server that went silent would
/// otherwise hold the Cancel button for the client's whole timeout).
async fn or_cancel<T>(fut: impl Future<Output = Result<T, reqwest::Error>>, cancel: &AtomicBool) -> Result<T, String> {
    tokio::pin!(fut);
    loop {
        match tokio::time::timeout(Duration::from_millis(200), &mut fut).await {
            Ok(r) => return r.map_err(|e| e.without_url().to_string()),
            Err(_) if cancel.load(Ordering::Relaxed) => return Err(cancelled()),
            Err(_) => {}
        }
    }
}

/// The hash the download must have: every source the release has, all agreeing.
async fn expected_sha256(client: &reqwest::Client, plan: &Plan, cancel: &AtomicBool) -> Result<String, String> {
    let mut sources: Vec<String> = plan.asset.sha256.iter().cloned().collect();
    if let Some(sums) = &plan.sums {
        let mut resp = or_cancel(client.get(&sums.url).send(), cancel).await?;
        if !resp.status().is_success() {
            return Err(t!("GitHub answered {status}", status = resp.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = or_cancel(resp.chunk(), cancel).await? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_SUMS {
                return Err(t!("the answer is too large").into());
            }
        }
        let got = hex(aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &body).as_ref());
        if sums.sha256.as_ref().is_some_and(|want| *want != got) {
            return Err(t!("The release's checksums disagree with each other. Nothing was installed.").into());
        }
        let listed = parse_sums(&String::from_utf8_lossy(&body), &plan.asset.name)
            .ok_or_else(|| t!("The release's checksum list doesn't name {file}.", file = plan.asset.name))?;
        sources.push(listed);
    }
    let Some(first) = sources.first().cloned() else {
        return Err(t!("This release has no checksums to check the download against.").into());
    };
    if sources.iter().any(|s| *s != first) {
        return Err(t!("The release's checksums disagree with each other. Nothing was installed.").into());
    }
    Ok(first)
}

/// Downloads the plan's file into `dir` (which exists) and returns its path, once its size and
/// SHA-256 are what the release says. Nothing with the file's name is left behind otherwise.
pub async fn fetch(
    client: &reqwest::Client,
    plan: &Plan,
    dir: &Path,
    mut progress: impl FnMut(Progress) + Send,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let too_big = || t!("The download is much larger than a Corrent release.").to_string();
    let want = plan.asset.size;
    if want > MAX_DOWNLOAD {
        return Err(too_big());
    }
    // The name is one of `InstallKind::asset_name`'s; all the same, never a path.
    if plan.asset.name.contains(['/', '\\']) || plan.asset.name.starts_with('.') {
        return Err(t!("unexpected answer ({e})", e = "file name"));
    }
    progress(Progress::Checksums);
    let sha256 = expected_sha256(client, plan, cancel).await?;
    let dest = dir.join(&plan.asset.name);
    let part = dir.join(format!("{}.part", plan.asset.name));
    let r = download(client, plan, &part, &sha256, &mut progress, cancel).await;
    let r = r.and_then(|()| {
        std::fs::rename(&part, &dest).map_err(|e| t!("Can't write to {folder}: {e}", folder = dir.display(), e = e))
    });
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r.map(|()| dest)
}

async fn download(
    client: &reqwest::Client,
    plan: &Plan,
    to: &Path,
    sha256: &str,
    progress: &mut (impl FnMut(Progress) + Send),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let want = plan.asset.size;
    let wrong_size = |got: u64| {
        t!(
            "The download isn't the size the release says ({got} instead of {want}). Try again.",
            got = crate::util::fmt_bytes(got),
            want = crate::util::fmt_bytes(want)
        )
    };
    let mut resp = or_cancel(client.get(&plan.asset.url).send(), cancel).await?;
    if !resp.status().is_success() {
        return Err(t!("GitHub answered {status}", status = resp.status()));
    }
    let total = resp.content_length();
    if total.is_some_and(|t| t > MAX_DOWNLOAD) {
        return Err(t!("The download is much larger than a Corrent release.").into());
    }
    // GitHub says how large the file is; an answer of another length is not that file.
    if let Some(t) = total
        && want > 0
        && t != want
    {
        return Err(wrong_size(t));
    }
    let limit = if want > 0 { want } else { MAX_DOWNLOAD };
    let cant_write = |e: std::io::Error| t!("Can't write to {folder}: {e}", folder = to.display(), e = e);
    let mut file = tokio::fs::File::create(to).await.map_err(cant_write)?;
    let mut hash = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
    let mut done = 0u64;
    let total = total.or((want > 0).then_some(want));
    progress(Progress::Download { done, total });
    while let Some(chunk) = or_cancel(resp.chunk(), cancel).await? {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        done += chunk.len() as u64;
        if done > limit {
            return Err(if want > 0 {
                wrong_size(done)
            } else {
                t!("The download is much larger than a Corrent release.").into()
            });
        }
        hash.update(&chunk);
        file.write_all(&chunk).await.map_err(cant_write)?;
        progress(Progress::Download { done, total });
    }
    file.flush().await.map_err(cant_write)?;
    file.sync_all().await.map_err(cant_write)?;
    drop(file);
    if want > 0 && done != want {
        return Err(wrong_size(done));
    }
    if hex(hash.finish().as_ref()) != sha256 {
        return Err(t!("The download is damaged (its checksum doesn't match). Try again.").into());
    }
    Ok(())
}

// ------------------------------------------------------------------------------------ unpacking

fn cant_unpack(e: impl std::fmt::Display) -> String {
    t!("Couldn't unpack the download: {e}", e = e)
}

/// Copies at most `MAX_DOWNLOAD` bytes of `from` into a new file `to`.
fn copy_out(from: &mut impl Read, to: &Path) -> Result<u64, String> {
    let mut out = std::fs::File::create(to).map_err(cant_unpack)?;
    let n = std::io::copy(&mut from.take(MAX_DOWNLOAD + 1), &mut out).map_err(cant_unpack)?;
    if n > MAX_DOWNLOAD {
        return Err(cant_unpack("too large"));
    }
    out.sync_all().map_err(cant_unpack)?;
    Ok(n)
}

fn starts_with(path: &Path, magic: &[u8]) -> bool {
    let mut head = vec![0u8; magic.len()];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == magic
}

/// Takes the files named `wanted` out of the zip's `folder/` into `out`, under the same names.
/// Returns the names found. Nothing else in the archive is written anywhere.
fn unzip(archive: &Path, folder: &str, wanted: &[&str], out: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(archive).map_err(cant_unpack)?;
    let mut zip = zip::ZipArchive::new(file).map_err(cant_unpack)?;
    let mut found = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(cant_unpack)?;
        // Windows PowerShell's Compress-Archive writes back slashes.
        let name = entry.name().map_err(cant_unpack)?.replace('\\', "/");
        let Some(name) = name.strip_prefix(folder).and_then(|n| n.strip_prefix('/')) else { continue };
        let Some(name) = wanted.iter().copied().find(|w| *w == name) else { continue };
        if !entry.is_file() || found.iter().any(|f| f == name) {
            return Err(cant_unpack(name));
        }
        copy_out(&mut entry, &out.join(name))?;
        found.push(name.to_string());
    }
    Ok(found)
}

/// Takes the regular files named `wanted` out of a `.tar.gz`'s top level into `out`.
fn untar(archive: &Path, wanted: &[&str], out: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(archive).map_err(cant_unpack)?;
    let mut tar = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut found = Vec::new();
    let mut header = [0u8; 512];
    loop {
        // The archive ends with empty blocks (or just ends).
        if tar.read_exact(&mut header).is_err() || header.iter().all(|b| *b == 0) {
            break;
        }
        let text = |range: std::ops::Range<usize>| {
            let field = header.get(range).unwrap_or_default();
            let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
            String::from_utf8_lossy(field.get(..end).unwrap_or_default()).trim().to_string()
        };
        let name = text(0..100);
        let name = name.strip_prefix("./").unwrap_or(&name).to_string();
        let size = u64::from_str_radix(&text(124..136), 8).map_err(|_| cant_unpack("size"))?;
        let kind = header.get(156).copied().unwrap_or(0);
        let padded = size.div_ceil(512) * 512;
        let wanted_name = wanted.iter().copied().find(|w| *w == name).filter(|_| kind == b'0' || kind == 0);
        let Some(name) = wanted_name else {
            // Something else (a longer name's extra header, a folder): passed over.
            let skipped = std::io::copy(&mut (&mut tar).take(padded), &mut std::io::sink()).map_err(cant_unpack)?;
            if skipped != padded {
                return Err(cant_unpack("cut off"));
            }
            continue;
        };
        if found.iter().any(|f| f == name) || size > MAX_DOWNLOAD {
            return Err(cant_unpack(name));
        }
        if copy_out(&mut (&mut tar).take(size), &out.join(name))? != size {
            return Err(cant_unpack("cut off"));
        }
        std::io::copy(&mut (&mut tar).take(padded - size), &mut std::io::sink()).map_err(cant_unpack)?;
        found.push(name.to_string());
    }
    Ok(found)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(cant_unpack)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

const ELF: &[u8] = b"\x7fELF";

/// Unpacks the checked download `file` into `work` (a fresh folder on the installation's own
/// disk) and says which new file replaces which installed one. Nothing installed is touched.
pub fn stage(kind: &InstallKind, file: &Path, work: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let missing = |name: &str| cant_unpack(format!("{name}?"));
    match kind {
        InstallKind::WindowsPortable { dir } => {
            let (exe, worker, license) = ("corrent.exe", "corrent-mpvw.exe", "LICENSE.txt");
            let found = unzip(file, "Corrent", &[exe, worker, license], work)?;
            for name in [exe, worker] {
                if !found.iter().any(|f| f == name) {
                    return Err(missing(name));
                }
                // A Windows program starts with "MZ".
                if !starts_with(&work.join(name), b"MZ") {
                    return Err(cant_unpack(name));
                }
            }
            Ok(found.iter().map(|n| (work.join(n), dir.join(n))).collect())
        }
        InstallKind::LinuxTarball { dir } => {
            let names = ["corrent", "corrent-mpvw"];
            let found = untar(file, &names, work)?;
            for name in names {
                if !found.iter().any(|f| f == name) {
                    return Err(missing(name));
                }
                if !starts_with(&work.join(name), ELF) {
                    return Err(cant_unpack(name));
                }
                make_executable(&work.join(name))?;
            }
            Ok(names.iter().map(|n| (work.join(n), dir.join(n))).collect())
        }
        InstallKind::AppImage { file: installed } => {
            if !starts_with(file, ELF) {
                return Err(cant_unpack("AppImage"));
            }
            make_executable(file)?;
            Ok(vec![(file.to_path_buf(), installed.clone())])
        }
        _ => Err(t!("This copy of Corrent can't update itself.").into()),
    }
}

// ------------------------------------------------------------------------------------ the swap

/// `corrent.exe` → `corrent.exe.old`.
pub fn old_path(installed: &Path) -> PathBuf {
    let mut name = installed.file_name().unwrap_or_default().to_os_string();
    name.push(OLD_SUFFIX);
    installed.with_file_name(name)
}

/// How an installed file gets out of the new one's way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aside {
    /// Renamed to `*.old`: the only way for a program that is running on Windows. For a moment
    /// the name is free.
    Rename,
    /// Hard-linked (or copied) to `*.old`, so the new file replaces the old one in one step and
    /// the name is never free (Linux, macOS).
    Link,
}

impl Aside {
    pub fn for_os(os: Os) -> Aside {
        if os == Os::Windows { Aside::Rename } else { Aside::Link }
    }
}

/// Installed files that were replaced, until the update is known to have worked.
#[derive(Debug)]
pub struct Swap {
    /// (installed path, whether there was a file there before).
    replaced: Vec<(PathBuf, bool)>,
}

impl Swap {
    /// Puts the previous files back under their names; what replaced them is removed.
    pub fn rollback(self) -> Result<(), String> {
        let mut first_error = None;
        for (installed, had_old) in self.replaced.iter().rev() {
            let old = old_path(installed);
            let r = if *had_old {
                std::fs::rename(&old, installed).or_else(|_| {
                    // The new file can't be replaced (it is running after all): out of the way with it.
                    let mut failed = installed.as_os_str().to_os_string();
                    failed.push(".failed");
                    std::fs::rename(installed, PathBuf::from(failed)).and_then(|()| std::fs::rename(&old, installed))
                })
            } else {
                match std::fs::remove_file(installed) {
                    // The new file never got there.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    r => r,
                }
            };
            if r.is_ok() {
                // Still there when it is a link to the file that is back under its name. Never
                // removed otherwise: it may be the only copy of the previous version.
                let _ = std::fs::remove_file(&old);
            }
            if let Err(e) = r {
                first_error.get_or_insert(t!(
                    "Couldn't put the previous version back ({path}: {e}). Download Corrent again from the release page.",
                    path = installed.display(),
                    e = e
                ));
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// The files that were replaced.
    pub fn installed(&self) -> impl Iterator<Item = &Path> {
        self.replaced.iter().map(|(p, _)| p.as_path())
    }
}

/// Replaces each installed file with its new one (`pairs`: new file, installed path; both on one
/// disk). All or nothing: if any step fails, the files already replaced are put back and the
/// error is returned. The old files stay beside the new ones as `*.old` (`cleanup` removes them
/// at the next start), so `Swap::rollback` can still undo everything.
pub fn swap(pairs: &[(PathBuf, PathBuf)], aside: Aside) -> Result<Swap, String> {
    swap_with(pairs, aside, |_| Ok(()))
}

/// `swap` with a step before each file is put in place (tests make it fail).
fn swap_with(
    pairs: &[(PathBuf, PathBuf)],
    aside: Aside,
    mut before_put: impl FnMut(usize) -> std::io::Result<()>,
) -> Result<Swap, String> {
    let in_the_way =
        |path: &Path, e: std::io::Error| t!("{path} is in use or can't be replaced: {e}", path = path.display(), e = e);
    // Before anything changes: every new file is there, and no leftover of an earlier update is
    // in the way.
    for (new, installed) in pairs {
        if !new.is_file() {
            return Err(cant_unpack(new.display()));
        }
        let old = old_path(installed);
        if old.exists() {
            std::fs::remove_file(&old).map_err(|e| in_the_way(&old, e))?;
        }
    }
    let mut done = Swap { replaced: Vec::new() };
    let undo = |done: Swap, e: std::io::Error, installed: &Path| {
        let why = in_the_way(installed, e);
        match done.rollback() {
            Ok(()) => Err(why),
            Err(worse) => Err(format!("{why} {worse}")),
        }
    };
    for (i, (new, installed)) in pairs.iter().enumerate() {
        let old = old_path(installed);
        let had_old = installed.exists();
        if had_old {
            let set_aside = match aside {
                Aside::Rename => crate::util::rename_retrying(installed, &old),
                Aside::Link => {
                    std::fs::hard_link(installed, &old).or_else(|_| std::fs::copy(installed, &old).map(|_| ()))
                }
            };
            if let Err(e) = set_aside {
                // This file is as it was; the ones before it are put back.
                let _ = std::fs::remove_file(&old);
                return undo(done, e, installed);
            }
        }
        // From here the file counts as touched: a failure puts `old` back.
        done.replaced.push((installed.clone(), had_old));
        if let Err(e) = before_put(i).and_then(|()| crate::util::rename_retrying(new, installed)) {
            return undo(done, e, installed);
        }
    }
    Ok(done)
}

/// Removes what an earlier update left: `*.old` files beside the installed ones and the update's
/// work folders, in `folders` (the executable's folder, an AppImage's), and the profile's
/// download folder. Files still in use stay for the next start. Returns how many things went.
pub fn cleanup(folders: &[PathBuf], appimage: Option<&Path>, data_dir: &Path) -> usize {
    const INSTALLED: [&str; 5] = ["corrent.exe", "corrent-mpvw.exe", "LICENSE.txt", "corrent", "corrent-mpvw"];
    let mut gone = 0;
    let mut remove_file = |p: PathBuf| {
        if p.is_file() && std::fs::remove_file(&p).is_ok() {
            gone += 1;
        }
    };
    for dir in folders {
        for name in INSTALLED {
            remove_file(old_path(&dir.join(name)));
            remove_file(dir.join(format!("{name}{OLD_SUFFIX}.failed")));
            remove_file(dir.join(format!("{name}.failed")));
        }
    }
    if let Some(a) = appimage {
        remove_file(old_path(a));
    }
    let mut remove_dir = |p: PathBuf| {
        if p.is_dir() && std::fs::remove_dir_all(&p).is_ok() {
            gone += 1;
        }
    };
    for dir in folders.iter().cloned().chain(appimage.and_then(Path::parent).map(Path::to_path_buf)) {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().starts_with(WORK_PREFIX) {
                remove_dir(e.path());
            }
        }
    }
    remove_dir(data_dir.join(DATA_SUBDIR));
    gone
}

/// A fresh folder for one update: beside the installation for the kinds that swap files (the
/// new files must be on the same disk to be renamed into place), in the profile for the MSI.
pub fn work_dir(kind: &InstallKind, data_dir: &Path) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = match kind {
        InstallKind::WindowsPortable { dir } | InstallKind::LinuxTarball { dir } => {
            dir.join(format!("{WORK_PREFIX}{nanos}"))
        }
        InstallKind::AppImage { file } => {
            file.parent().map(Path::to_path_buf).unwrap_or_default().join(format!("{WORK_PREFIX}{nanos}"))
        }
        _ => data_dir.join(DATA_SUBDIR).join(nanos.to_string()),
    };
    std::fs::create_dir_all(&dir).map_err(|e| t!("Can't write to {folder}: {e}", folder = dir.display(), e = e))?;
    Ok(dir)
}

// ------------------------------------------------------------------------------------ the MSI

/// `msiexec`'s arguments for a non-interactive install with its progress window: no questions,
/// no restart of the computer.
pub fn msiexec_args(msi: &Path) -> Vec<std::ffi::OsString> {
    vec!["/i".into(), msi.as_os_str().to_os_string(), "/passive".into(), "/norestart".into()]
}

/// Whether `msiexec`'s exit code means the new version is installed (3010 and 1641: installed,
/// and Windows wants a restart to finish replacing a file that was in use).
pub fn msi_succeeded(code: Option<i32>) -> bool {
    matches!(code, Some(0 | 3010 | 1641))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-selfupdate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(b: &[u8]) -> String {
        hex(aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, b).as_ref())
    }

    fn facts(exe: &str) -> Facts {
        Facts { exe: exe.into(), x86_64: true, ..Default::default() }
    }

    fn kind_of(os: Os, f: &Facts, files: &[&str], read_only: bool) -> InstallKind {
        let files: Vec<String> = files.iter().map(|p| norm(Path::new(p))).collect();
        classify(os, f, &|p| files.contains(&norm(p)), &|_| !read_only)
    }

    // Windows paths only come apart on Windows.
    #[cfg(windows)]
    #[test]
    fn windows_installations_are_told_apart() {
        let dir = r"C:\Tools\Corrent";
        let exe = r"C:\Tools\Corrent\corrent.exe";
        let worker = r"C:\Tools\Corrent\corrent-mpvw.exe";
        let portable = Facts { portable_marker: true, ..facts(exe) };
        assert_eq!(kind_of(Os::Windows, &portable, &[worker], false), InstallKind::WindowsPortable { dir: dir.into() });
        assert_eq!(kind_of(Os::Windows, &portable, &[worker], true), InstallKind::ReadOnly);
        assert_eq!(kind_of(Os::Windows, &portable, &[], false), InstallKind::Unknown, "no worker: not the zip");
        // `--portable` on a copy without the marker (a build someone made) is not the zip.
        assert_eq!(kind_of(Os::Windows, &facts(exe), &[worker], false), InstallKind::Unknown);
        let target = [worker, r"C:\Tools\Corrent\deps", r"C:\Tools\Corrent\build"];
        assert_eq!(kind_of(Os::Windows, &portable, &target, false), InstallKind::Unknown, "a cargo target folder");

        // The MSI's folder, however the registry spells it; also when run with --portable.
        let msi = r"C:\Users\u\AppData\Local\Programs\Corrent\corrent.exe";
        let installed = Facts {
            msi_dir: Some(r"c:\users\u\appdata\local\programs\corrent\".into()),
            portable_marker: true,
            ..facts(msi)
        };
        assert_eq!(
            kind_of(Os::Windows, &installed, &[], false),
            InstallKind::WindowsMsi { dir: r"C:\Users\u\AppData\Local\Programs\Corrent".into() }
        );
        // A portable copy elsewhere on a computer that also has the MSI installed.
        let both = Facts { msi_dir: installed.msi_dir.clone(), ..portable.clone() };
        assert_eq!(kind_of(Os::Windows, &both, &[worker], false), InstallKind::WindowsPortable { dir: dir.into() });

        let scoop = r"C:\Users\u\scoop\apps\corrent\current\corrent.exe";
        let scooped = Facts { portable_marker: true, ..facts(scoop) };
        assert_eq!(kind_of(Os::Windows, &scooped, &[], false), InstallKind::Managed(Manager::Scoop));

        let arm = Facts { x86_64: false, ..portable };
        assert_eq!(kind_of(Os::Windows, &arm, &[worker], false), InstallKind::Unknown, "the releases are x86-64");
    }

    #[test]
    fn linux_installations_are_told_apart() {
        let appimage = Facts {
            appimage: Some("/home/u/Apps/Corrent.AppImage".into()),
            ..facts("/tmp/.mount_Corr1/usr/bin/corrent")
        };
        let file = "/home/u/Apps/Corrent.AppImage";
        assert_eq!(kind_of(Os::Linux, &appimage, &[file], false), InstallKind::AppImage { file: file.into() });
        assert_eq!(kind_of(Os::Linux, &appimage, &[file], true), InstallKind::ReadOnly);
        assert_eq!(kind_of(Os::Linux, &appimage, &[], false), InstallKind::Unknown, "the file is gone");

        let tarball = facts("/home/u/corrent/corrent");
        let worker = "/home/u/corrent/corrent-mpvw";
        assert_eq!(
            kind_of(Os::Linux, &tarball, &[worker], false),
            InstallKind::LinuxTarball { dir: "/home/u/corrent".into() }
        );
        assert_eq!(kind_of(Os::Linux, &tarball, &[worker], true), InstallKind::ReadOnly);
        assert_eq!(kind_of(Os::Linux, &tarball, &[], false), InstallKind::Unknown);
        let target = [worker, "/home/u/corrent/deps", "/home/u/corrent/build"];
        assert_eq!(kind_of(Os::Linux, &tarball, &target, false), InstallKind::Unknown, "a cargo target folder");

        for exe in ["/usr/bin/corrent", "/opt/corrent/corrent", "/nix/store/x-corrent/bin/corrent"] {
            let k = kind_of(Os::Linux, &facts(exe), &["/usr/bin/corrent-mpvw", "/opt/corrent/corrent-mpvw"], false);
            assert_eq!(k, InstallKind::Managed(Manager::SystemPackage), "{exe}");
        }
        let flatpak = Facts { flatpak: true, ..facts("/app/bin/corrent") };
        assert_eq!(kind_of(Os::Linux, &flatpak, &[], false), InstallKind::Managed(Manager::Flatpak));
        // Snap sets $SNAP; an AppImage variable beside it changes nothing.
        let snap = Facts { snap: true, appimage: Some(file.into()), ..facts("/snap/corrent/1/bin/corrent") };
        assert_eq!(kind_of(Os::Linux, &snap, &[file], false), InstallKind::Managed(Manager::Snap));
    }

    #[test]
    fn mac_installations_are_told_apart() {
        let app = facts("/Applications/Corrent.app/Contents/MacOS/corrent");
        assert_eq!(
            kind_of(Os::Mac, &app, &[], false),
            InstallKind::MacApp { bundle: "/Applications/Corrent.app".into() }
        );
        let brewed = Facts { homebrew_cask: true, ..app };
        assert_eq!(kind_of(Os::Mac, &brewed, &[], false), InstallKind::Managed(Manager::Homebrew));
        let mounted = facts("/Volumes/Corrent/Corrent.app/Contents/MacOS/corrent");
        assert_eq!(kind_of(Os::Mac, &mounted, &[], false), InstallKind::ReadOnly);
        let moved = facts("/private/var/folders/x/AppTranslocation/ABC/d/Corrent.app/Contents/MacOS/corrent");
        assert_eq!(kind_of(Os::Mac, &moved, &[], false), InstallKind::ReadOnly);
        assert_eq!(
            kind_of(Os::Mac, &facts("/Users/u/corrent/target/release/corrent"), &[], false),
            InstallKind::Unknown
        );
        assert_eq!(kind_of(Os::Other, &facts("/usr/local/bin/corrent"), &[], false), InstallKind::Unknown);
    }

    fn release(version: &str, names: &[&str]) -> Release {
        let base = format!("https://github.com/ulaas/corrent/releases/download/v{version}/");
        Release {
            version: version.into(),
            url: format!("https://github.com/ulaas/corrent/releases/tag/v{version}"),
            assets: names
                .iter()
                .map(|n| Asset { name: n.to_string(), url: format!("{base}{n}"), size: 10, sha256: None })
                .collect(),
        }
    }

    /// Every file of a real release.
    const FILES: [&str; 12] = [
        "Corrent-1.2.3-windows-portable.zip",
        "Corrent-1.2.3-x86_64.msi",
        "Corrent-1.2.3-x86_64.AppImage",
        "Corrent-1.2.3-linux-x86_64.tar.gz",
        "Corrent-1.2.3-macos.dmg",
        "Corrent-Server-1.2.3-windows-x86_64.zip",
        "Corrent-Server-1.2.3-linux-x86_64.tar.gz",
        "Corrent-Server-1.2.3-darwin-arm64.tar.gz",
        "SHA256SUMS.txt",
        "corrent.json",
        "corrent.rb",
        "PKGBUILD",
    ];

    #[test]
    fn each_kind_gets_its_own_file_by_exact_name() {
        let r = release("1.2.3", &FILES);
        let cases = [
            (InstallKind::WindowsPortable { dir: "d".into() }, "Corrent-1.2.3-windows-portable.zip"),
            (InstallKind::WindowsMsi { dir: "d".into() }, "Corrent-1.2.3-x86_64.msi"),
            (InstallKind::AppImage { file: "f".into() }, "Corrent-1.2.3-x86_64.AppImage"),
            (InstallKind::LinuxTarball { dir: "d".into() }, "Corrent-1.2.3-linux-x86_64.tar.gz"),
            (InstallKind::MacApp { bundle: "b".into() }, "Corrent-1.2.3-macos.dmg"),
        ];
        for (kind, name) in cases {
            let p = plan(&r, &kind).unwrap();
            assert_eq!(p.asset.name, name, "{kind:?}");
            assert!(p.asset.url.ends_with(name));
            assert_eq!(p.sums.as_ref().map(|s| s.name.as_str()), Some(SUMS));
            assert!(kind.downloads());
            assert_eq!(kind.installs_itself(), !matches!(kind, InstallKind::MacApp { .. }));
        }
        // A release without that file (or with a near miss) gives nothing rather than a guess.
        let other =
            release("1.2.3", &["Corrent-Server-1.2.3-linux-x86_64.tar.gz", "Corrent-1.2.3-windows-portable.zip.sig"]);
        assert!(plan(&other, &InstallKind::LinuxTarball { dir: "d".into() }).is_err());
        assert!(plan(&other, &InstallKind::WindowsPortable { dir: "d".into() }).is_err());
        assert_eq!(
            plan(&release("1.2.3", &["Corrent-1.2.3-x86_64.msi"]), &InstallKind::WindowsMsi { dir: "d".into() })
                .unwrap()
                .sums,
            None
        );
    }

    #[test]
    fn package_managed_and_unwritable_copies_refuse() {
        let r = release("1.2.3", &FILES);
        let managers = [Manager::Scoop, Manager::Homebrew, Manager::SystemPackage, Manager::Flatpak, Manager::Snap];
        let kinds = managers.into_iter().map(InstallKind::Managed).chain([InstallKind::ReadOnly, InstallKind::Unknown]);
        for kind in kinds {
            assert!(!kind.installs_itself() && !kind.downloads(), "{kind:?}");
            assert_eq!(kind.asset_name("1.2.3"), None, "{kind:?}");
            assert!(plan(&r, &kind).is_err(), "{kind:?}");
            let work = scratch("refuse");
            std::fs::write(work.join("x"), b"MZ").unwrap();
            assert!(stage(&kind, &work.join("x"), &work).is_err(), "{kind:?}");
            let _ = std::fs::remove_dir_all(work);
        }
        for m in managers {
            assert!(m.command().contains("corrent") || m.command().contains("Corrent"));
            assert!(!m.advice().is_empty());
        }
        // The MSI and the disk image aren't swapped file by file either.
        let work = scratch("refuse-msi");
        std::fs::write(work.join("x"), b"MZ").unwrap();
        assert!(stage(&InstallKind::WindowsMsi { dir: work.clone() }, &work.join("x"), &work).is_err());
        assert!(stage(&InstallKind::MacApp { bundle: work.clone() }, &work.join("x"), &work).is_err());
        let _ = std::fs::remove_dir_all(work);
    }

    #[test]
    fn checksum_lists_are_read_strictly() {
        let (a, b) = ("ab".repeat(32), "cd".repeat(32));
        let text = format!("{a}  one.zip\n{} *two.zip\n\nnot a line\n{b}  three zip\n", b.to_uppercase());
        assert_eq!(parse_sums(&text, "one.zip"), Some(a.clone()));
        assert_eq!(parse_sums(&text, "two.zip"), Some(b.clone()), "binary mark, upper case");
        assert_eq!(parse_sums(&text, "three zip"), Some(b.clone()));
        assert_eq!(parse_sums(&text, "zip"), None, "only whole names");
        assert_eq!(parse_sums(&text, "four.zip"), None);
        assert_eq!(parse_sums(&format!("{a}  x\n{b}  x\n"), "x"), None, "listed twice, differently");
        assert_eq!(parse_sums(&format!("{a}  x\n{a}  x\n"), "x"), Some(a));
        assert_eq!(parse_sums("abcd  x\n", "x"), None, "not a SHA-256");
    }

    // ---------------------------------------------------------------- the download (loopback)

    use crate::engine::faultserver::{FaultServer, Route, start};

    fn quick_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .read_timeout(Duration::from_secs(2))
            .build()
            .unwrap()
    }

    const NAME: &str = "Corrent-9.0.0-windows-portable.zip";

    fn body() -> Vec<u8> {
        (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect()
    }

    /// A plan for `NAME` served at `path`, with the checksum list at `/sums` when `sums`.
    fn plan_at(srv: &FaultServer, path: &str, size: u64, digest: Option<String>, sums: bool) -> Plan {
        Plan {
            version: "9.0.0".into(),
            page: "https://github.com/ulaas/corrent/releases/tag/v9.0.0".into(),
            asset: Asset { name: NAME.into(), url: srv.url(path), size, sha256: digest },
            sums: sums.then(|| Asset { name: SUMS.into(), url: srv.url("/sums"), size: 0, sha256: None }),
        }
    }

    fn answer(body: Vec<u8>) -> Route {
        Route::Answer { status: 200, content_type: "application/octet-stream", body }
    }

    async fn get(plan: &Plan, dir: &Path) -> Result<PathBuf, String> {
        fetch(&quick_client(), plan, dir, |_| {}, &AtomicBool::new(false)).await
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_download_is_kept_only_when_every_checksum_agrees() {
        let dir = scratch("fetch-ok");
        let srv = start().await;
        let data = body();
        let good = sha(&data);
        srv.route("/file", answer(data.clone()));
        srv.route("/sums", answer(format!("{}  other.zip\n{good}  {NAME}\n", "00".repeat(32)).into_bytes()));
        let size = data.len() as u64;

        // Both sources, agreeing.
        let mut seen = Vec::new();
        let p = plan_at(&srv, "/file", size, Some(good.clone()), true);
        let file = fetch(&quick_client(), &p, &dir, |pr| seen.push(pr), &AtomicBool::new(false)).await.unwrap();
        assert_eq!(file, dir.join(NAME));
        assert_eq!(std::fs::read(&file).unwrap(), data);
        assert_eq!(seen.first(), Some(&Progress::Checksums));
        assert_eq!(seen.last(), Some(&Progress::Download { done: size, total: Some(size) }));
        assert_eq!(leftovers(&dir), [NAME], "no .part file");
        std::fs::remove_file(&file).unwrap();

        // One source is enough when it is the only one: the list alone, GitHub's digest alone.
        assert!(get(&plan_at(&srv, "/file", size, None, true), &dir).await.is_ok());
        assert!(get(&plan_at(&srv, "/file", size, Some(good.clone()), false), &dir).await.is_ok());
        std::fs::remove_file(&file).unwrap();

        // None at all: nothing is even downloaded.
        let before = srv.hits("/file");
        let e = get(&plan_at(&srv, "/file", size, None, false), &dir).await.unwrap_err();
        assert!(e.contains("no checksums"), "{e}");
        // The sources disagree (GitHub's digest is for another file).
        let e = get(&plan_at(&srv, "/file", size, Some("11".repeat(32)), true), &dir).await.unwrap_err();
        assert!(e.contains("disagree"), "{e}");
        // The list is there but doesn't name the file; it can't be fetched; it isn't the file
        // GitHub says it is.
        srv.route("/sums", answer(format!("{good}  something-else.zip\n").into_bytes()));
        let e = get(&plan_at(&srv, "/file", size, Some(good.clone()), true), &dir).await.unwrap_err();
        assert!(e.contains("doesn't name"), "{e}");
        srv.route("/sums", Route::Answer { status: 500, content_type: "text/plain", body: vec![] });
        assert!(get(&plan_at(&srv, "/file", size, Some(good.clone()), true), &dir).await.is_err());
        srv.route("/sums", answer(format!("{good}  {NAME}\n").into_bytes()));
        let mut p = plan_at(&srv, "/file", size, Some(good.clone()), true);
        p.sums.as_mut().unwrap().sha256 = Some("22".repeat(32));
        assert!(get(&p, &dir).await.unwrap_err().contains("disagree"));
        srv.route("/sums", Route::Huge { len: 3 << 20 });
        assert!(get(&plan_at(&srv, "/file", size, Some(good), true), &dir).await.is_err(), "an endless list");
        assert_eq!(srv.hits("/file"), before, "nothing was downloaded without a checksum to hold it to");
        assert!(leftovers(&dir).is_empty(), "{:?}", leftovers(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_real_client_is_https_only() {
        let c = client(None).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        // Refused before any connection is made (nothing listens there anyway).
        let e = rt.block_on(c.get("http://127.0.0.1:9/x").send()).unwrap_err();
        assert!(!e.is_connect(), "{e:?}");
    }

    // ---------------------------------------------------------------- unpacking and the swap

    /// A portable zip as the release workflow makes it, with stand-in programs.
    fn portable_zip(path: &Path, exe: &[u8], back_slashes: bool) {
        let sep = if back_slashes { "\\" } else { "/" };
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in [
            ("corrent.exe", exe),
            ("corrent-mpvw.exe", b"MZ new worker".as_slice()),
            ("portable", b"marker".as_slice()),
            ("LICENSE.txt", b"new license".as_slice()),
        ] {
            z.start_file(format!("Corrent{sep}{name}"), opts).unwrap();
            z.write_all(data).unwrap();
        }
        // Not in the package's folder: never unpacked.
        z.start_file("corrent.exe", opts).unwrap();
        z.write_all(b"MZ wrong place").unwrap();
        z.start_file("Corrent/corrent-data/settings.toml", opts).unwrap();
        z.write_all(b"evil").unwrap();
        z.finish().unwrap();
    }

    /// A portable installation with a profile and a downloaded libmpv.
    fn portable_install(name: &str) -> PathBuf {
        let dir = scratch(name);
        for (file, data) in [
            ("corrent.exe", "MZ old app"),
            ("corrent-mpvw.exe", "MZ old worker"),
            ("LICENSE.txt", "old license"),
            ("portable", "marker"),
            ("corrent-data/settings.toml", "mine"),
            ("corrent-lib/libmpv-2.dll", "MZ mpv"),
        ] {
            let p = dir.join(file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, data).unwrap();
        }
        dir
    }

    fn read(p: PathBuf) -> String {
        String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned()
    }

    fn untouched(dir: &Path) {
        assert_eq!(read(dir.join("corrent-data/settings.toml")), "mine");
        assert_eq!(read(dir.join("corrent-lib/libmpv-2.dll")), "MZ mpv");
        assert_eq!(read(dir.join("portable")), "marker");
    }

    #[test]
    fn a_portable_copy_is_swapped_in_place_and_cleaned_up_at_the_next_start() {
        for back_slashes in [false, true] {
            let dir = portable_install("swap-ok");
            let kind = InstallKind::WindowsPortable { dir: dir.clone() };
            let work = work_dir(&kind, &dir.join("corrent-data")).unwrap();
            assert_eq!(work.parent(), Some(dir.as_path()), "beside the installation: the same disk");
            let zip = work.join("Corrent-9.0.0-windows-portable.zip");
            portable_zip(&zip, b"MZ new app", back_slashes);
            let pairs = stage(&kind, &zip, &work).unwrap();
            assert_eq!(pairs.len(), 3);
            assert_eq!(read(dir.join("corrent.exe")), "MZ old app", "staging touches nothing installed");
            assert!(!work.join("portable").exists() && !work.join("corrent-data").exists(), "only the three files");

            // The running program holds its file open; it can still be set aside.
            let running = std::fs::File::open(dir.join("corrent.exe")).unwrap();
            let done = swap(&pairs, Aside::Rename).unwrap();
            assert_eq!(done.installed().count(), 3);
            assert_eq!(read(dir.join("corrent.exe")), "MZ new app");
            assert_eq!(read(dir.join("corrent-mpvw.exe")), "MZ new worker");
            assert_eq!(read(dir.join("LICENSE.txt")), "new license");
            assert_eq!(read(dir.join("corrent.exe.old")), "MZ old app");
            untouched(&dir);
            drop(running);

            // The next start.
            let gone = cleanup(std::slice::from_ref(&dir), None, &dir.join("corrent-data"));
            assert_eq!(gone, 4, "three old files and the work folder");
            let mut names = leftovers(&dir);
            names.sort();
            assert_eq!(
                names,
                ["LICENSE.txt", "corrent-data", "corrent-lib", "corrent-mpvw.exe", "corrent.exe", "portable"]
            );
            assert_eq!(read(dir.join("corrent.exe")), "MZ new app");
            untouched(&dir);
            assert_eq!(cleanup(std::slice::from_ref(&dir), None, &dir.join("corrent-data")), 0, "nothing more to do");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_failed_swap_puts_every_file_back() {
        for aside in [Aside::Rename, Aside::Link] {
            for fail_at in 0..3 {
                let dir = portable_install("swap-fail");
                let kind = InstallKind::WindowsPortable { dir: dir.clone() };
                let work = work_dir(&kind, &dir).unwrap();
                let zip = work.join("x.zip");
                portable_zip(&zip, b"MZ new app", false);
                let pairs = stage(&kind, &zip, &work).unwrap();
                let r = swap_with(&pairs, aside, |i| {
                    if i == fail_at { Err(std::io::Error::other("disk full")) } else { Ok(()) }
                });
                let e = r.unwrap_err();
                assert!(e.contains("disk full"), "{e}");
                assert_eq!(read(dir.join("corrent.exe")), "MZ old app", "{aside:?} failing at {fail_at}");
                assert_eq!(read(dir.join("corrent-mpvw.exe")), "MZ old worker", "{aside:?} failing at {fail_at}");
                assert_eq!(read(dir.join("LICENSE.txt")), "old license", "{aside:?} failing at {fail_at}");
                untouched(&dir);
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn a_swap_that_worked_can_still_be_undone() {
        // The new program didn't start: the old one comes back.
        for aside in [Aside::Rename, Aside::Link] {
            let dir = portable_install("swap-undo");
            // No license in this installation: the new one is simply removed again.
            std::fs::remove_file(dir.join("LICENSE.txt")).unwrap();
            let kind = InstallKind::WindowsPortable { dir: dir.clone() };
            let work = work_dir(&kind, &dir).unwrap();
            let zip = work.join("x.zip");
            portable_zip(&zip, b"MZ new app", false);
            let done = swap(&stage(&kind, &zip, &work).unwrap(), aside).unwrap();
            assert_eq!(read(dir.join("corrent.exe")), "MZ new app");
            done.rollback().unwrap();
            assert_eq!(read(dir.join("corrent.exe")), "MZ old app");
            assert_eq!(read(dir.join("corrent-mpvw.exe")), "MZ old worker");
            assert!(!dir.join("LICENSE.txt").exists());
            assert!(!dir.join("corrent.exe.old").exists());
            untouched(&dir);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_package_that_is_not_one_is_refused_before_anything_changes() {
        let dir = portable_install("swap-bad");
        let kind = InstallKind::WindowsPortable { dir: dir.clone() };
        let work = work_dir(&kind, &dir).unwrap();
        // Not a zip at all.
        std::fs::write(work.join("a.zip"), b"hello").unwrap();
        assert!(stage(&kind, &work.join("a.zip"), &work).is_err());
        // A zip whose program isn't one.
        portable_zip(&work.join("b.zip"), b"#!/bin/sh", false);
        assert!(stage(&kind, &work.join("b.zip"), &work).is_err());
        // A zip without the worker.
        let mut z = zip::ZipWriter::new(std::fs::File::create(work.join("c.zip")).unwrap());
        z.start_file("Corrent/corrent.exe", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"MZ").unwrap();
        z.finish().unwrap();
        assert!(stage(&kind, &work.join("c.zip"), &work).is_err());
        // A new file that went missing between staging and the swap.
        let pairs =
            vec![(work.join("corrent.exe"), dir.join("corrent.exe")), (work.join("nope"), dir.join("LICENSE.txt"))];
        std::fs::write(work.join("corrent.exe"), b"MZ new").unwrap();
        assert!(swap(&pairs, Aside::Rename).is_err());
        assert_eq!(read(dir.join("corrent.exe")), "MZ old app");
        assert_eq!(read(dir.join("LICENSE.txt")), "old license");
        assert!(!dir.join("corrent.exe.old").exists());
        untouched(&dir);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// One tar entry: a 512-byte header and the data padded to 512.
    fn tar_entry(name: &str, data: &[u8], kind: u8) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000755");
        let size = format!("{:011o}", data.len());
        h[124..135].copy_from_slice(size.as_bytes());
        h[156] = kind;
        h[257..262].copy_from_slice(b"ustar");
        let mut out = h;
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
        out
    }

    fn tar_gz(path: &Path, entries: &[(&str, &[u8], u8)]) {
        let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
        for (name, data, kind) in entries {
            gz.write_all(&tar_entry(name, data, *kind)).unwrap();
        }
        gz.write_all(&[0u8; 1024]).unwrap();
        gz.finish().unwrap();
    }

    #[test]
    fn a_tarball_copy_is_swapped_and_an_appimage_replaced() {
        let dir = scratch("tarball");
        std::fs::write(dir.join("corrent"), b"\x7fELF old app").unwrap();
        std::fs::write(dir.join("corrent-mpvw"), b"\x7fELF old worker").unwrap();
        std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
        let kind = InstallKind::LinuxTarball { dir: dir.clone() };
        let work = work_dir(&kind, &dir).unwrap();
        let big = vec![7u8; 5000];
        let new_app = [b"\x7fELF new app".as_slice(), &big].concat();
        tar_gz(
            &work.join("t.tar.gz"),
            &[
                ("./", b"", b'5'),
                ("notes.txt", b"theirs", b'0'),
                ("corrent", &new_app, b'0'),
                ("sub/corrent-mpvw", b"\x7fELF wrong place", b'0'),
                ("corrent-mpvw", b"\x7fELF new worker", 0),
            ],
        );
        let pairs = stage(&kind, &work.join("t.tar.gz"), &work).unwrap();
        assert_eq!(pairs.len(), 2);
        swap(&pairs, Aside::Link).unwrap();
        assert_eq!(std::fs::read(dir.join("corrent")).unwrap(), new_app);
        assert_eq!(read(dir.join("corrent-mpvw")), "\x7fELF new worker");
        assert_eq!(read(dir.join("corrent.old")), "\x7fELF old app");
        assert_eq!(read(dir.join("notes.txt")), "mine", "only the two programs");
        assert_eq!(cleanup(std::slice::from_ref(&dir), None, &dir.join("no-profile")), 3);
        assert_eq!(leftovers(&dir).len(), 3);

        // A tarball without the worker, or one that is cut off.
        let work = work_dir(&kind, &dir).unwrap();
        tar_gz(&work.join("a.tar.gz"), &[("corrent", b"\x7fELF x", b'0')]);
        assert!(stage(&kind, &work.join("a.tar.gz"), &work).is_err());
        std::fs::write(work.join("b.tar.gz"), b"\x1f\x8b not really").unwrap();
        assert!(stage(&kind, &work.join("b.tar.gz"), &work).is_err());
        // A program that is a link somewhere is not a program.
        tar_gz(&work.join("c.tar.gz"), &[("corrent", b"", b'2'), ("corrent-mpvw", b"\x7fELF w", b'0')]);
        assert!(stage(&kind, &work.join("c.tar.gz"), &work).is_err());
        let _ = std::fs::remove_dir_all(&work);

        // The AppImage: one file, replaced; the old one kept until the next start.
        let appimage = dir.join("Corrent.AppImage");
        std::fs::write(&appimage, b"\x7fELF old image").unwrap();
        let kind = InstallKind::AppImage { file: appimage.clone() };
        let work = work_dir(&kind, &dir).unwrap();
        assert_eq!(work.parent(), Some(dir.as_path()));
        let new = work.join("Corrent-9.0.0-x86_64.AppImage");
        std::fs::write(&new, b"not an image").unwrap();
        assert!(stage(&kind, &new, &work).is_err());
        std::fs::write(&new, b"\x7fELF new image").unwrap();
        let done = swap(&stage(&kind, &new, &work).unwrap(), Aside::Link).unwrap();
        assert_eq!(read(appimage.clone()), "\x7fELF new image");
        assert_eq!(read(old_path(&appimage)), "\x7fELF old image");
        done.rollback().unwrap();
        assert_eq!(read(appimage.clone()), "\x7fELF old image");
        std::fs::write(old_path(&appimage), b"left over").unwrap();
        assert_eq!(cleanup(&[], Some(&appimage), &dir.join("no-profile")), 2, "the old image and the work folder");
        assert!(appimage.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_installer_runs_without_questions_and_its_download_lives_in_the_profile() {
        let args = msiexec_args(Path::new(r"C:\x\Corrent-9.0.0-x86_64.msi"));
        assert_eq!(args, ["/i", r"C:\x\Corrent-9.0.0-x86_64.msi", "/passive", "/norestart"]);
        assert!(msi_succeeded(Some(0)) && msi_succeeded(Some(3010)));
        assert!(!msi_succeeded(Some(1603)) && !msi_succeeded(Some(1602)) && !msi_succeeded(None));
        let data = scratch("msi-work");
        let work = work_dir(&InstallKind::WindowsMsi { dir: "C:/nowhere".into() }, &data).unwrap();
        assert!(work.starts_with(data.join(DATA_SUBDIR)));
        std::fs::write(work.join("x.msi"), b"abc").unwrap();
        assert_eq!(file_sha256(&work.join("x.msi")).unwrap(), sha(b"abc"));
        assert_eq!(cleanup(&[], None, &data), 1);
        assert!(!data.join(DATA_SUBDIR).exists());
        let _ = std::fs::remove_dir_all(data);
    }
}
