fn main() {
    println!("cargo:rerun-if-changed=assets/icon/corrent.ico");
    copy_libmpv();
    git_sha();
    // Embed the icon and version info so Explorer, the taskbar and file associations show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/corrent.ico")
            .set("FileDescription", "Corrent BitTorrent client")
            .set("ProductName", "Corrent")
            .set("OriginalFilename", "corrent.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}

/// Put the libmpv fetched by `scripts/fetch-libmpv.ps1` (`.cache/libmpv/libmpv-2.dll`) next to the
/// executables in `target/<profile>/`, where Corrent and `corrent-mpvw` look for it. Nothing is
/// downloaded here; without the cached file this does nothing.
fn copy_libmpv() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let src = std::path::Path::new(".cache/libmpv/libmpv-2.dll");
    if !src.is_file() {
        // Watching a missing file would rerun this script on every build; the fetch script
        // copies into existing target folders itself.
        return;
    }
    println!("cargo:rerun-if-changed=.cache/libmpv/libmpv-2.dll");
    // OUT_DIR is target/<profile>/build/<crate>-<hash>/out.
    let Some(profile_dir) = std::env::var_os("OUT_DIR")
        .map(std::path::PathBuf::from)
        .and_then(|p| p.ancestors().nth(3).map(|p| p.to_path_buf()))
    else {
        return;
    };
    let dst = profile_dir.join("libmpv-2.dll");
    let same = std::fs::metadata(&dst).ok().map(|m| m.len()) == std::fs::metadata(src).ok().map(|m| m.len());
    if !same && let Err(e) = std::fs::copy(src, &dst) {
        println!("cargo:warning=could not copy libmpv-2.dll to {}: {e}", dst.display());
    }
}

/// The commit the build is made from, as `CORRENT_GIT_SHA` (shown in About and `--version`):
/// `CORRENT_BUILD_SHA` if set (a release script that knows about uncommitted changes), else
/// `git rev-parse --short=9 HEAD`. Nothing when neither is available (a source archive).
fn git_sha() {
    println!("cargo:rerun-if-env-changed=CORRENT_BUILD_SHA");
    // Rerun on a new commit or a branch switch (HEAD, the branch's ref, packed refs).
    let git = std::path::Path::new(".git");
    if git.is_dir() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        println!("cargo:rerun-if-changed=.git/packed-refs");
        if let Ok(head) = std::fs::read_to_string(git.join("HEAD"))
            && let Some(r) = head.trim().strip_prefix("ref: ")
        {
            println!("cargo:rerun-if-changed=.git/{r}");
        }
    }
    let sha = std::env::var("CORRENT_BUILD_SHA").ok().filter(|s| !s.trim().is_empty()).or_else(|| {
        let out = std::process::Command::new("git").args(["rev-parse", "--short=9", "HEAD"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    if let Some(sha) = sha.filter(|s| !s.is_empty()) {
        println!("cargo:rustc-env=CORRENT_GIT_SHA={}", sha.trim());
    }
}
