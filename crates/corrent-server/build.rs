//! Builds the web client (`web/` into `web/dist`, which `src/webclient.rs` embeds) when its
//! sources are newer than the last build. Needs Node 20.19+ with npm on `PATH`.
//!
//! `CORRENT_SKIP_WEB_BUILD=1` uses `web/dist` as it is (a build made elsewhere, no Node here).

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::SystemTime;

/// What the bundle is made from, relative to the repository root. The plugins in `web/plugins`
/// read the translation catalogs and the design tokens.
const INPUTS: &[&str] = &[
    "web/src",
    "web/public",
    "web/plugins",
    "web/scripts/i18n-keys.mjs",
    "web/index.html",
    "web/package.json",
    "web/package-lock.json",
    "web/vite.config.ts",
    "web/svelte.config.js",
    "web/tsconfig.json",
    "assets/i18n/tr.toml",
    "assets/i18n/web",
    "design/tokens.json",
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: the web client could not be built: {e}");
            eprintln!(
                "Install Node 20.19 or newer (with npm), or set CORRENT_SKIP_WEB_BUILD=1 to use a web/dist built elsewhere."
            );
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").ok_or("CARGO_MANIFEST_DIR is not set")?;
    let root = PathBuf::from(manifest).join("../..");
    let web = root.join("web");
    let index = web.join("dist/index.html");

    println!("cargo:rerun-if-env-changed=CORRENT_SKIP_WEB_BUILD");
    for input in INPUTS {
        println!("cargo:rerun-if-changed={}", root.join(input).display());
    }
    // Missing: cargo runs this script again. Rebuilt by hand (`npm run build`): the crate is
    // compiled again, so a release build embeds the new files.
    println!("cargo:rerun-if-changed={}", index.display());

    if std::env::var_os("CORRENT_SKIP_WEB_BUILD").is_some_and(|v| !v.is_empty() && v != "0") {
        return if index.exists() {
            Ok(())
        } else {
            Err(format!("CORRENT_SKIP_WEB_BUILD is set, but {} is missing", index.display()))
        };
    }

    // One build at a time: cargo and an editor's `cargo check` (another target folder) may both
    // get here, and Vite empties `web/dist` first.
    let lock_path = web.join(".build.lock");
    let lock = File::create(&lock_path).map_err(|e| format!("{}: {e}", lock_path.display()))?;
    lock.lock().map_err(|e| format!("{}: {e}", lock_path.display()))?;

    let mut newest = SystemTime::UNIX_EPOCH;
    for input in INPUTS {
        let path = root.join(input);
        newest = newest.max(newest_change(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    if modified(&index).is_ok_and(|built| built >= newest) {
        return Ok(());
    }

    // `npm ci` writes node_modules/.package-lock.json: older than the lock file means other
    // dependencies than the ones installed.
    let installed = modified(&web.join("node_modules/.package-lock.json"));
    let wanted = modified(&web.join("package-lock.json")).map_err(|e| format!("web/package-lock.json: {e}"))?;
    if !installed.is_ok_and(|t| t >= wanted) {
        npm(&web, &["ci", "--no-audit", "--no-fund"])?;
    }
    npm(&web, &["run", "build"])?;
    if !index.exists() {
        return Err(format!("the build left no {}", index.display()));
    }
    Ok(())
}

fn modified(path: &Path) -> io::Result<SystemTime> {
    fs::metadata(path)?.modified()
}

/// The latest modification time of a file, or of anything in a folder.
fn newest_change(path: &Path) -> io::Result<SystemTime> {
    let meta = fs::metadata(path)?;
    let mut newest = meta.modified()?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            newest = newest.max(newest_change(&entry?.path())?);
        }
    }
    Ok(newest)
}

fn npm(web: &Path, args: &[&str]) -> Result<(), String> {
    // npm is a .cmd file on Windows.
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "npm"]);
        c
    } else {
        Command::new("npm")
    };
    // Cargo reads this script's stdout for instructions: npm's output goes to stderr.
    let status = cmd
        .args(args)
        .current_dir(web)
        .stdin(Stdio::null())
        .stdout(Stdio::from(io::stderr()))
        .status()
        .map_err(|e| format!("npm could not be started: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("`npm {}` failed ({status})", args.join(" "))) }
}
