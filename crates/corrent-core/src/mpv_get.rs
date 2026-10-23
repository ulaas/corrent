//! Getting libmpv where no package manager provides it (Windows): on the user's request Corrent
//! downloads a pinned mpv build from the mpv-winbuild-cmake project's GitHub releases, checks its
//! SHA-256, and keeps only `libmpv-2.dll`, in the folder the caller names (the app uses
//! `corrent-lib` next to its executable, where it and its `corrent-mpvw` worker look first).
//!
//! libmpv is not part of Corrent's releases (docs/PACKAGING.md): it is free software under the
//! GPL from its own makers, fetched from them only when the user asks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;

/// Where the builds come from, for the user to read.
pub const PROJECT_URL: &str = "https://github.com/shinchiro/mpv-winbuild-cmake";
const RELEASES: &str = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/";
const LATEST_API: &str = "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest";
/// The build Corrent was checked with (keep in step with scripts/fetch-libmpv.ps1).
const TAG: &str = "20261019";
const ASSET: &str = "mpv-dev-x86_64-20261019-git-6a262d11f6.7z";
const SHA256: &str = "ca8604754170db6f1b23139d8449782b1ae294a883211131954d30085985f77b";
/// The library's file name in the archive and on disk.
pub const DLL: &str = "libmpv-2.dll";
/// The archives are about 30 MiB; anything far bigger is not one.
const MAX_ARCHIVE: u64 = 400 << 20;

/// One downloadable build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

/// The build Corrent was checked with.
pub fn pinned() -> Build {
    Build { name: ASSET.into(), url: format!("{RELEASES}{TAG}/{ASSET}"), sha256: SHA256.into() }
}

/// What is happening, for a progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Download { done: u64, total: Option<u64> },
    Unpack,
}

/// A client for the download: no limit on the whole transfer (it may take minutes on a slow
/// line), only on connecting and on silence.
pub fn client(proxy: Option<String>) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .user_agent(concat!("Corrent/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60));
    if let Some(p) = proxy.and_then(|p| reqwest::Proxy::all(p).ok()) {
        b = b.proxy(p);
    }
    b.build().unwrap_or_default()
}

/// The newest build in a GitHub "latest release" answer, with the SHA-256 GitHub computed for
/// it. Only an x86-64 development archive from the project's own releases is accepted.
pub fn parse_latest(json: &str) -> Result<Build, String> {
    #[derive(serde::Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        digest: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Release {
        assets: Vec<Asset>,
    }
    let r: Release = serde_json::from_str(json).map_err(|e| t!("unexpected answer ({e})", e = e))?;
    let a = r
        .assets
        .into_iter()
        .find(|a| a.name.starts_with("mpv-dev-x86_64-2") && a.name.ends_with(".7z"))
        .ok_or_else(|| t!("The newest mpv release has no Windows library."))?;
    let sha = a
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| t!("The newest mpv release has no checksum."))?
        .to_ascii_lowercase();
    if !a.browser_download_url.starts_with(RELEASES) {
        return Err(t!("unexpected answer ({e})", e = "download address"));
    }
    Ok(Build { name: a.name, url: a.browser_download_url, sha256: sha })
}

async fn latest(client: &reqwest::Client, api: &str) -> Result<Build, String> {
    let mut resp = client
        .get(api)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(t!("GitHub answered {status}", status = resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.without_url().to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > 4 << 20 {
            return Err(t!("the answer is too large").into());
        }
    }
    parse_latest(&String::from_utf8_lossy(&body))
}

enum Fetch {
    /// The release is gone (the project prunes old ones).
    Gone,
    Failed(String),
}

fn cancelled() -> String {
    t!("Cancelled.").into()
}

/// Downloads `b` to `to`, checking its size and SHA-256 as it arrives.
async fn download(
    client: &reqwest::Client,
    b: &Build,
    to: &Path,
    progress: &mut (impl FnMut(Progress) + Send),
    cancel: &AtomicBool,
) -> Result<(), Fetch> {
    let mut resp = client.get(&b.url).send().await.map_err(|e| Fetch::Failed(e.without_url().to_string()))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Fetch::Gone);
    }
    if !resp.status().is_success() {
        return Err(Fetch::Failed(t!("GitHub answered {status}", status = resp.status())));
    }
    let total = resp.content_length();
    let too_big = || Fetch::Failed(t!("The download is much larger than an mpv library.").into());
    if total.is_some_and(|t| t > MAX_ARCHIVE) {
        return Err(too_big());
    }
    let mut file = tokio::fs::File::create(to).await.map_err(|e| Fetch::Failed(e.to_string()))?;
    let mut hash = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
    let mut done = 0u64;
    progress(Progress::Download { done, total });
    while let Some(chunk) = resp.chunk().await.map_err(|e| Fetch::Failed(e.without_url().to_string()))? {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fetch::Failed(cancelled()));
        }
        done += chunk.len() as u64;
        if done > MAX_ARCHIVE {
            return Err(too_big());
        }
        hash.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| Fetch::Failed(e.to_string()))?;
        progress(Progress::Download { done, total });
    }
    file.flush().await.map_err(|e| Fetch::Failed(e.to_string()))?;
    let got: String = hash.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    if got != b.sha256.to_ascii_lowercase() {
        return Err(Fetch::Failed(t!("The download is damaged (its checksum doesn't match). Try again.").into()));
    }
    Ok(())
}

/// Takes `DLL` out of the archive into `out` (Windows' own tar reads 7z).
fn unpack(archive: &Path, out: &Path) -> Result<PathBuf, String> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let tar = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let mut c = std::process::Command::new(tar.join("System32").join("tar.exe"));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    };
    // NO_WINDOW_OK: not Windows (bsdtar reads 7z on macOS; Linux has libmpv in its packages).
    #[cfg(not(windows))]
    let mut cmd = std::process::Command::new("tar");
    let o = cmd
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(out)
        .arg(DLL)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| t!("Couldn't unpack the download: {e}", e = e))?;
    let dll = out.join(DLL);
    if !o.status.success() || !dll.is_file() {
        let why = String::from_utf8_lossy(&o.stderr).trim().chars().take(300).collect::<String>();
        return Err(t!("Couldn't unpack the download: {e}", e = why));
    }
    // A Windows library starts with "MZ".
    let head = std::fs::read(&dll).map(|b| b.get(..2) == Some(b"MZ")).unwrap_or(false);
    if !head {
        return Err(t!("Couldn't unpack the download: {e}", e = "not a library"));
    }
    Ok(dll)
}

/// Downloads libmpv into `folder` (created if needed): the pinned build, or the newest one when
/// the pinned one is no longer published. Returns the library's path and the build's name.
pub async fn install(
    client: &reqwest::Client,
    folder: &Path,
    progress: impl FnMut(Progress) + Send,
    cancel: &AtomicBool,
) -> Result<(PathBuf, String), String> {
    install_from(client, folder, pinned(), LATEST_API, progress, cancel).await
}

/// `install` with the build and the "latest release" address given (tests).
pub async fn install_from(
    client: &reqwest::Client,
    folder: &Path,
    first: Build,
    latest_api: &str,
    mut progress: impl FnMut(Progress) + Send,
    cancel: &AtomicBool,
) -> Result<(PathBuf, String), String> {
    let cant_write = |e: std::io::Error| t!("Can't write to {folder}: {e}", folder = folder.display(), e = e);
    std::fs::create_dir_all(folder).map_err(cant_write)?;
    // A fresh folder for the download: nothing in it is trusted until the checksum matched.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let work = folder.join(format!(".download-{nanos}"));
    std::fs::create_dir_all(&work).map_err(cant_write)?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(work.clone());

    let archive = work.join("mpv.7z");
    let build = match download(client, &first, &archive, &mut progress, cancel).await {
        Ok(()) => first,
        Err(Fetch::Failed(e)) => return Err(e),
        Err(Fetch::Gone) => {
            let b = latest(client, latest_api).await?;
            match download(client, &b, &archive, &mut progress, cancel).await {
                Ok(()) => b,
                Err(Fetch::Failed(e)) => return Err(e),
                Err(Fetch::Gone) => return Err(t!("GitHub answered {status}", status = "404")),
            }
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    progress(Progress::Unpack);
    let out = work.join("out");
    std::fs::create_dir_all(&out).map_err(cant_write)?;
    let (a, o) = (archive.clone(), out.clone());
    let dll = tokio::task::spawn_blocking(move || unpack(&a, &o)).await.map_err(|e| e.to_string())??;
    let dest = folder.join(DLL);
    if dest.exists() {
        std::fs::remove_file(&dest)
            .map_err(|e| t!("{path} is in use or can't be replaced: {e}", path = dest.display(), e = e))?;
    }
    std::fs::rename(&dll, &dest).map_err(cant_write)?;
    let _ = std::fs::write(folder.join("VERSION"), format!("{}\n", build.name));
    Ok((dest, build.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[cfg(windows)]
    fn sha(b: &[u8]) -> String {
        aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, b).as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Serves `routes` (path → (status, body)) on loopback.
    async fn serve(routes: Vec<(String, u16, Vec<u8>)>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (status, body) = routes
                        .iter()
                        .find(|(p, ..)| *p == path)
                        .map(|(_, s, b)| (*s, b.clone()))
                        .unwrap_or((404, vec![]));
                    let head =
                        format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        base
    }

    #[test]
    fn the_newest_release_needs_its_checksum_and_the_projects_address() {
        let ok = format!(
            r#"{{"assets":[{{"name":"mpv-x86_64-20261101.7z","browser_download_url":"{RELEASES}x/a.7z","digest":"sha256:{z}"}},
                {{"name":"mpv-dev-x86_64-20261101-git-1.7z","browser_download_url":"{RELEASES}20261101/mpv-dev-x86_64-20261101-git-1.7z","digest":"sha256:{z}"}}]}}"#,
            z = "AB".repeat(32)
        );
        let b = parse_latest(&ok).unwrap();
        assert_eq!(b.name, "mpv-dev-x86_64-20261101-git-1.7z");
        assert_eq!(b.sha256, "ab".repeat(32));
        assert!(parse_latest(&ok.replace("sha256:", "md5:")).is_err(), "no checksum");
        assert!(parse_latest(&ok.replace(RELEASES, "https://example.com/")).is_err(), "elsewhere");
        assert!(parse_latest(r#"{"assets":[]}"#).is_err());
        assert!(parse_latest("not json").is_err());
        assert!(pinned().url.starts_with(RELEASES) && pinned().sha256.len() == 64);
    }

    #[tokio::test]
    async fn a_damaged_download_is_refused_and_leaves_nothing() {
        let body = b"not the archive".to_vec();
        let base = serve(vec![("/a.7z".into(), 200, body)]).await;
        let dir = std::env::temp_dir().join(format!("corrent-mpvget-bad-{}", std::process::id()));
        let b = Build { name: "a.7z".into(), url: format!("{base}/a.7z"), sha256: "00".repeat(32) };
        let r = install_from(&client(None), &dir, b, &format!("{base}/latest"), |_| {}, &AtomicBool::new(false)).await;
        assert!(r.unwrap_err().contains("checksum"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the download folder is gone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_pruned_build_falls_back_to_the_newest_and_installs_the_library() {
        // A 7z with a stand-in library, made by Windows' tar.
        let src = std::env::temp_dir().join(format!("corrent-mpvget-src-{}", std::process::id()));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(DLL), b"MZ stand-in").unwrap();
        let mut c = std::process::Command::new("tar");
        c.args(["--format", "7zip", "-cf", "a.7z", DLL]).current_dir(&src);
        assert!(c.status().unwrap().success());
        let archive = std::fs::read(src.join("a.7z")).unwrap();
        // The pinned build is gone; "latest" names one on this server (its address is checked
        // against the project's, so the answer is rewritten after parsing: see below).
        let base = serve(vec![("/new.7z".into(), 200, archive.clone())]).await;
        let latest = format!(
            r#"{{"assets":[{{"name":"mpv-dev-x86_64-2-new.7z","browser_download_url":"{RELEASES}new.7z","digest":"sha256:{}"}}]}}"#,
            sha(&archive)
        );
        let parsed = parse_latest(&latest).unwrap();
        assert_eq!(parsed.url, format!("{RELEASES}new.7z"));
        // Serve the parsed build from loopback for the rest of the test.
        let local = Build { url: format!("{base}/new.7z"), ..parsed };
        let dir = std::env::temp_dir().join(format!("corrent-mpvget-ok-{}", std::process::id()));
        let mut seen = Vec::new();
        let gone = Build { name: "old".into(), url: format!("{base}/old.7z"), sha256: "00".repeat(32) };
        let r = install_from(
            &client(None),
            &dir,
            gone,
            &format!("{base}/latest"),
            |p| seen.push(p),
            &AtomicBool::new(false),
        )
        .await;
        assert!(r.is_err(), "no latest answer on this server: {r:?}");
        let (dll, name) = install_from(&client(None), &dir, local, "unused", |p| seen.push(p), &AtomicBool::new(false))
            .await
            .unwrap();
        assert_eq!(dll, dir.join(DLL));
        assert_eq!(std::fs::read(&dll).unwrap(), b"MZ stand-in");
        assert_eq!(name, "mpv-dev-x86_64-2-new.7z");
        assert!(seen.contains(&Progress::Unpack));
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "the library and VERSION, no download folder: {names:?}");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(src);
    }

    /// The real pinned build from GitHub (about 30 MiB): `cargo test -p corrent-core
    /// mpv_get::tests::live -- --ignored`.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "downloads mpv from GitHub"]
    async fn live() {
        let dir = std::env::temp_dir().join(format!("corrent-mpvget-live-{}", std::process::id()));
        let (dll, name) = install(&client(None), &dir, |_| {}, &AtomicBool::new(false)).await.unwrap();
        println!("{name}: {} bytes", std::fs::metadata(&dll).unwrap().len());
        let _ = std::fs::remove_dir_all(dir);
    }
}
