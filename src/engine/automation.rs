//! Watch folders and "run a program on completion".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{AddSource, TorrentView};
use crate::config::{AfterAdd, WatchFolder};

/// Tracks candidate files until their size is stable (fully written).
#[derive(Default)]
pub struct WatchState {
    sizes: HashMap<PathBuf, (u64, u8)>,
}

pub struct WatchHit {
    pub source: AddSource,
    pub folder: WatchFolder,
}

impl WatchState {
    /// Scan all enabled folders once. Files are returned after two polls with an unchanged size;
    /// they are read into memory and then renamed/deleted so they are never added twice.
    pub fn poll(&mut self, folders: &[WatchFolder]) -> Vec<WatchHit> {
        let mut hits = Vec::new();
        let mut seen = HashMap::new();
        for f in folders.iter().filter(|f| f.enabled && !f.path.as_os_str().is_empty()) {
            let Ok(entries) = std::fs::read_dir(&f.path) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let is_torrent = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("torrent"));
                if !is_torrent || !path.is_file() {
                    continue;
                }
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let (prev, stable) = self.sizes.get(&path).copied().unwrap_or((u64::MAX, 0));
                let stable = if prev == size && size > 0 { stable.saturating_add(1) } else { 0 };
                seen.insert(path.clone(), (size, stable));
                if stable >= 1 {
                    match std::fs::read(&path) {
                        Ok(bytes) => {
                            finish_file(&path, f.after_add);
                            seen.remove(&path);
                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            hits.push(WatchHit { source: AddSource::Bytes { name, bytes }, folder: f.clone() });
                        }
                        Err(e) => tracing::warn!("watch folder: cannot read {}: {e}", path.display()),
                    }
                }
            }
        }
        self.sizes = seen;
        hits
    }
}

fn finish_file(path: &Path, after: AfterAdd) {
    let result = match after {
        AfterAdd::Delete => std::fs::remove_file(path),
        AfterAdd::Rename | AfterAdd::Keep => {
            let mut target = path.as_os_str().to_owned();
            target.push(".added");
            std::fs::rename(path, PathBuf::from(target))
        }
    };
    if let Err(e) = result {
        tracing::warn!("watch folder: could not move {} out of the way: {e}", path.display());
    }
}

/// Substitute placeholders in a user command line.
pub fn expand_command(template: &str, t: &TorrentView) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let value = match chars.peek() {
            Some('N') => t.name.clone(),
            Some('F') => t.content_path.display().to_string(),
            Some('D') => t.save_path.display().to_string(),
            Some('C') => t.category.clone().unwrap_or_default(),
            Some('T') => t.tags.join(","),
            Some('Z') => t.size.to_string(),
            Some('I') => t.hash.to_hex(),
            Some('%') => "%".to_string(),
            _ => {
                out.push('%');
                continue;
            }
        };
        chars.next();
        out.push_str(&value);
    }
    out
}

/// Run the completion command detached, logging its outcome.
pub fn run_on_complete(template: &str, t: &TorrentView) {
    let cmdline = expand_command(template, t);
    if cmdline.trim().is_empty() {
        return;
    }
    std::thread::spawn(move || {
        #[cfg(windows)]
        let result = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("cmd").arg("/C").raw_arg(&cmdline).creation_flags(0x0800_0000).output()
        };
        #[cfg(not(windows))]
        let result = std::process::Command::new("sh").arg("-c").arg(&cmdline).output();
        match result {
            Ok(o) if o.status.success() => tracing::info!("completion command finished: {cmdline}"),
            Ok(o) => tracing::warn!(
                "completion command exited with {}: {} {}",
                o.status,
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => tracing::error!("could not run completion command `{cmdline}`: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_requires_stable_size() {
        let dir = std::env::temp_dir().join(format!("corrent-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.torrent");
        std::fs::write(&f, b"d4:infod4:name1:aee").unwrap();
        let folder = WatchFolder { path: dir.clone(), ..Default::default() };
        let mut w = WatchState::default();
        assert!(w.poll(std::slice::from_ref(&folder)).is_empty(), "first sighting is not stable yet");
        let hits = w.poll(std::slice::from_ref(&folder));
        assert_eq!(hits.len(), 1);
        assert!(!f.exists());
        assert!(dir.join("a.torrent.added").exists());
        assert!(w.poll(&[folder]).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
