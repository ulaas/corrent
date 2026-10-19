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
        assert!(!f.exists());
        assert!(dir.join("a.torrent.added").exists());
        assert!(w.poll(&[folder]).is_empty());
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
