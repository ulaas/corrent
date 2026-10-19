//! Where on the server's disks a user may read folder listings and write downloads.
//!
//! Administrators (`settings.write`) may use any path. Everyone else is confined to the
//! download folder, category folders, library folders and the admin's `download_roots`.
//! Paths are resolved (the deepest existing ancestor canonicalized, `..` refused), so neither
//! `..` nor symlinks can escape a root.

use std::path::{Component, Path, PathBuf};

use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;

use crate::state::AppState;
use crate::v1::{ApiErr, Auth, err};

/// Canonical form of `p`, which may not exist yet. `None` for relative paths and `..`.
pub fn resolve(p: &Path) -> Option<PathBuf> {
    if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let mut existing = p.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&existing) {
            let mut out = plain(c);
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Some(out);
        }
        let name = existing.file_name()?.to_os_string();
        rest.push(name);
        if !existing.pop() {
            return None;
        }
    }
}

/// Windows canonicalizes to verbatim paths (`\\?\C:\x`, `\\?\UNC\server\share\x`); settings,
/// torrent folders and the library's file paths use the plain form, so return that unless the
/// path needs the verbatim form (a component Windows would otherwise alter).
#[cfg(windows)]
fn plain(p: PathBuf) -> PathBuf {
    use std::path::Prefix;
    let mut parts = p.components();
    let Some(Component::Prefix(prefix)) = parts.next() else { return p };
    let rest = parts.as_path();
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let odd = rest.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        let stem = s.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        s.ends_with('.')
            || s.ends_with(' ')
            || RESERVED.contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit())
    });
    if odd {
        return p;
    }
    let base = match prefix.kind() {
        Prefix::VerbatimDisk(d) => format!("{}:\\", d as char),
        Prefix::VerbatimUNC(server, share) => {
            format!(r"\\{}\{}\", server.to_string_lossy(), share.to_string_lossy())
        }
        _ => return p,
    };
    let rest = rest.strip_prefix(std::path::MAIN_SEPARATOR_STR).unwrap_or(rest);
    PathBuf::from(base).join(rest)
}

#[cfg(not(windows))]
fn plain(p: PathBuf) -> PathBuf {
    p
}

/// The roots a confined user may use (resolved; missing ones dropped).
pub fn roots(st: &AppState) -> Vec<PathBuf> {
    let settings = st.settings.load();
    let snap = st.engine.snapshot();
    let mut v: Vec<PathBuf> = vec![settings.downloads.save_path.clone()];
    v.extend(snap.categories.values().filter_map(|c| c.save_path.clone()));
    v.extend(st.cfg.load().download_roots.iter().map(PathBuf::from));
    v.extend(st.library_roots.load().iter().cloned());
    let mut out: Vec<PathBuf> = v.iter().filter_map(|p| resolve(p)).collect();
    out.sort();
    out.dedup();
    out
}

pub fn unconfined(auth: &Auth) -> bool {
    auth.0.can(Perms::SETTINGS_WRITE)
}

/// Resolves `p` and checks that `auth` may use it.
pub fn check(st: &AppState, auth: &Auth, p: &Path) -> Result<PathBuf, ApiErr> {
    let r = resolve(p).ok_or_else(|| err(ErrorCode::BadRequest, "absolute path without '..' required"))?;
    if unconfined(auth) || roots(st).iter().any(|root| r.starts_with(root)) {
        Ok(r)
    } else {
        Err(err(ErrorCode::Forbidden, "that folder is outside the folders you may use"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_missing_tails_and_refuses_dotdot() {
        let d = std::env::temp_dir().join(format!("corrent-paths-{}", std::process::id()));
        std::fs::create_dir_all(d.join("a")).unwrap();
        let r = resolve(&d.join("a/new/deeper")).unwrap();
        assert!(r.ends_with("a/new/deeper"));
        assert!(resolve(&d.join("a/../../etc")).is_none());
        assert!(resolve(Path::new("relative")).is_none());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", d.join("a/link")).unwrap();
            let r = resolve(&d.join("a/link/passwd")).unwrap();
            assert!(r.starts_with("/etc"), "symlinks resolve to their target: {}", r.display());
        }
        #[cfg(windows)]
        {
            // Plain form, comparable with what users and the engine store.
            assert!(!r.to_string_lossy().starts_with(r"\\?\"), "{}", r.display());
            assert_eq!(plain(PathBuf::from(r"\\?\C:\Videos\a b")), PathBuf::from(r"C:\Videos\a b"));
            assert_eq!(plain(PathBuf::from(r"\\?\UNC\nas\media\x")), PathBuf::from(r"\\nas\media\x"));
            // Names Windows would alter keep the verbatim form.
            assert_eq!(plain(PathBuf::from(r"\\?\C:\x\con")), PathBuf::from(r"\\?\C:\x\con"));
            assert_eq!(plain(PathBuf::from(r"\\?\C:\x\dots.")), PathBuf::from(r"\\?\C:\x\dots."));
        }
        let _ = std::fs::remove_dir_all(d);
    }
}
