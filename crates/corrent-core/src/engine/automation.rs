//! Watch folders and "run a program on completion".

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::types::{AddSource, TorrentView};
use crate::config::{AfterAdd, WatchFolder};

/// Tracks candidate files until their size is stable (fully written).
#[derive(Default)]
pub struct WatchState {
    sizes: HashMap<PathBuf, (u64, u8)>,
    /// Files handed out by a scan whose add (and the move out of the way after it) hasn't
    /// finished: left in place, never handed out twice.
    in_flight: HashSet<PathBuf>,
    /// A scan runs off the engine's thread (a folder on a sleeping disk or an offline share
    /// can take long); one at a time.
    scanning: bool,
}

/// What a scan needs, taken from the state so it can run on another thread.
pub struct ScanInput {
    sizes: HashMap<PathBuf, (u64, u8)>,
    in_flight: HashSet<PathBuf>,
    folders: Vec<WatchFolder>,
}

/// A finished scan, handed back to `WatchState::finish_scan`.
pub struct Scan {
    sizes: HashMap<PathBuf, (u64, u8)>,
    hits: Vec<WatchHit>,
}

impl Scan {
    /// A scan that found nothing (one that failed): sizes start over.
    pub fn empty() -> Scan {
        Scan { sizes: HashMap::new(), hits: Vec::new() }
    }
}

pub struct WatchHit {
    pub source: AddSource,
    pub folder: WatchFolder,
    pub path: PathBuf,
}

impl WatchState {
    /// Start a scan of the enabled folders; None while one is still running.
    pub fn begin_scan(&mut self, folders: &[WatchFolder]) -> Option<ScanInput> {
        if self.scanning {
            return None;
        }
        self.scanning = true;
        Some(ScanInput {
            sizes: std::mem::take(&mut self.sizes),
            in_flight: self.in_flight.clone(),
            folders: folders.to_vec(),
        })
    }

    /// Take a scan's results: its hits are now in flight.
    pub fn finish_scan(&mut self, scan: Scan) -> Vec<WatchHit> {
        self.scanning = false;
        self.sizes = scan.sizes;
        let hits: Vec<WatchHit> = scan.hits.into_iter().filter(|h| !self.in_flight.contains(&h.path)).collect();
        self.in_flight.extend(hits.iter().map(|h| h.path.clone()));
        hits
    }

    /// Scan and take the results on this thread (tests).
    #[cfg(test)]
    pub fn poll(&mut self, folders: &[WatchFolder]) -> Vec<WatchHit> {
        let input = self.begin_scan(folders).expect("no scan running");
        self.finish_scan(scan(input))
    }
}

/// Scan the folders once (blocking file system work). Files are returned after two scans with an
/// unchanged size. They stay where they are until `finish` reports how their add went, so a
/// failed add never costs the user their only copy.
pub fn scan(input: ScanInput) -> Scan {
    let ScanInput { sizes, in_flight, folders } = input;
    let mut hits = Vec::new();
    let mut seen = HashMap::new();
    {
        for f in folders.iter().filter(|f| f.enabled && !f.path.as_os_str().is_empty()) {
            crate::faults::fs_delay(&f.path);
            let Ok(entries) = std::fs::read_dir(&f.path) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                let is_torrent = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("torrent"));
                if !is_torrent || !path.is_file() || in_flight.contains(&path) {
                    continue;
                }
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let (prev, stable) = sizes.get(&path).copied().unwrap_or((u64::MAX, 0));
                let stable = if prev == size && size > 0 { stable.saturating_add(1) } else { 0 };
                seen.insert(path.clone(), (size, stable));
                if stable >= 1 {
                    // The add reads it (at most MAX_TORRENT_BYTES); one that fails, also for being
                    // too large, is renamed to .failed (`finish_file`).
                    seen.remove(&path);
                    hits.push(WatchHit { source: AddSource::File(path.clone()), folder: f.clone(), path });
                }
            }
        }
    }
    Scan { sizes: seen, hits }
}

/// The name a file whose add failed is given (`<name>.failed`), so it isn't tried again.
pub fn failed_name(path: &Path) -> PathBuf {
    let mut failed = path.as_os_str().to_owned();
    failed.push(".failed");
    PathBuf::from(failed)
}

impl WatchState {
    /// The add of a file from a scan has ended. Added (or already in the list): the folder's
    /// after-add action runs. Otherwise the file is renamed to `failed_name`. Blocking file work;
    /// the file stays in flight until `release`.
    pub fn finish_file(path: &Path, after: AfterAdd, added: bool) {
        crate::faults::fs_delay(path);
        if added {
            finish_file(path, after);
        } else if let Err(e) = std::fs::rename(path, failed_name(path)) {
            tracing::warn!("watch folder: could not rename {}: {e}", path.display());
        }
    }

    /// Tests: `finish_file` and `release` on this thread; the `.failed` path when renamed.
    #[cfg(test)]
    pub fn finish(&mut self, path: &Path, after: AfterAdd, added: bool) -> Option<PathBuf> {
        WatchState::finish_file(path, after, added);
        self.release(path);
        (!added && failed_name(path).exists()).then(|| failed_name(path))
    }

    /// The file may be picked up again (its add never started, or it was moved out of the way).
    pub fn release(&mut self, path: &Path) {
        self.in_flight.remove(path);
    }
}

fn finish_file(path: &Path, after: AfterAdd) {
    let result = match after {
        AfterAdd::Delete => std::fs::remove_file(path),
        AfterAdd::Rename => {
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

/// Split a command line into arguments, honouring "double" and 'single' quotes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_arg = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_arg = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(cur);
    }
    args
}

/// Run the completion program detached, logging its outcome.
///
/// The template is split into arguments *before* placeholders are substituted and the program is
/// started directly (no shell), so torrent names can never inject commands. Users who need shell
/// features can call the shell explicitly, e.g. `sh -c '...'` or `cmd /C ...`.
pub fn run_on_complete(template: &str, t: &TorrentView) {
    let argv: Vec<String> = split_args(template).iter().map(|a| expand_command(a, t)).collect();
    let Some((program, args)) = argv.split_first() else { return };
    let (program, args) = (program.clone(), args.to_vec());
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new(&program);
        cmd.args(&args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.output() {
            Ok(o) if o.status.success() => tracing::info!("completion program finished: {program}"),
            Ok(o) => tracing::warn!(
                "completion program exited with {}: {} {}",
                o.status,
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => tracing::error!("could not run completion program `{program}`: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_and_no_injection() {
        assert_eq!(split_args(r#"notify-send "Done" '%N' x"#), vec!["notify-send", "Done", "%N", "x"]);
        assert_eq!(split_args(r#"a "" b"#), vec!["a", "", "b"]);
        // A hostile name stays inside one argument after substitution.
        let args: Vec<String> = split_args(r#"echo "%N""#).iter().map(|a| a.replace("%N", "x\" & del *")).collect();
        assert_eq!(args.len(), 2);
    }

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
        assert_eq!(hits[0].path, f);
        assert!(f.exists(), "kept until the add has finished");
        for _ in 0..3 {
            assert!(w.poll(std::slice::from_ref(&folder)).is_empty(), "never handed out twice");
        }
        assert_eq!(w.finish(&f, AfterAdd::Rename, true), None);
        assert!(!f.exists());
        assert!(dir.join("a.torrent.added").exists());
        assert!(w.poll(&[folder]).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn watch_keeps_files_whose_add_failed() {
        let dir = std::env::temp_dir().join(format!("corrent-watch-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("b.torrent");
        std::fs::write(&f, b"not a torrent").unwrap();
        let folder = WatchFolder { path: dir.clone(), after_add: AfterAdd::Delete, ..Default::default() };
        let mut w = WatchState::default();
        w.poll(std::slice::from_ref(&folder));
        let hits = w.poll(std::slice::from_ref(&folder));
        assert_eq!(hits.len(), 1);
        let failed = w.finish(&f, AfterAdd::Delete, false).unwrap();
        assert_eq!(failed, dir.join("b.torrent.failed"));
        assert_eq!(std::fs::read(&failed).unwrap(), b"not a torrent", "the user's copy survives");
        for _ in 0..3 {
            assert!(w.poll(std::slice::from_ref(&folder)).is_empty(), "a failed file isn't retried");
        }
        // An add that never started leaves the file to be picked up again.
        let g = dir.join("c.torrent");
        std::fs::write(&g, b"d4:infod4:name1:cee").unwrap();
        w.poll(std::slice::from_ref(&folder));
        assert_eq!(w.poll(std::slice::from_ref(&folder)).len(), 1);
        w.release(&g);
        w.poll(std::slice::from_ref(&folder));
        assert_eq!(w.poll(std::slice::from_ref(&folder)).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::engine::types::InfoHash;

    #[test]
    fn placeholders_expand() {
        let t = TorrentView {
            name: "N".into(),
            hash: InfoHash([0xab; 20]),
            size: 42,
            category: Some("C".into()),
            tags: vec!["a".into(), "b".into()],
            save_path: "D".into(),
            content_path: "F".into(),
            ..Default::default()
        };
        assert_eq!(expand_command("%N|%F|%D|%C|%T|%Z|%%|%X", &t), "N|F|D|C|a,b|42|%|%X");
        assert!(expand_command("%I", &t).starts_with("abab"));
    }

    #[test]
    fn split_handles_quotes_inside_words() {
        assert_eq!(split_args(r#"prog --name="a b" x"#), vec!["prog", "--name=a b", "x"]);
        assert!(split_args("   ").is_empty());
    }
}
