//! File names inside a torrent that this OS can't store safely.
//!
//! librqbit only refuses `..`, separators and exact duplicates. On Windows a component like
//! `C:evil` replaces the whole path when joined (a write outside the save folder), `CON.txt` is a
//! device, `name.` and `name` are the same file, and `A.txt`/`a.txt` collide (also on macOS).

use std::collections::HashMap;

/// Which rules apply. `HOST` is the current OS; tests pick their own.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Win32 file name rules: no `:` or `<>"|?*`, no device names, no trailing dot or space,
    /// at most 255 UTF-16 units per component.
    pub windows_names: bool,
    /// Paths that differ only in letter case are one file.
    pub case_insensitive: bool,
}

impl Rules {
    pub const HOST: Rules =
        Rules { windows_names: cfg!(windows), case_insensitive: cfg!(any(windows, target_os = "macos")) };
}

/// Device names Windows reserves, with or without an extension.
const RESERVED: [&str; 24] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// True if Windows treats `name` as a device (`CON`, `nul.txt`, `COM1 .log`, …).
pub fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// Folders a file may be nested in, and the length of its path inside the torrent.
pub const MAX_COMPONENTS: usize = 64;
pub const MAX_PATH_BYTES: usize = 4096;

/// Check every file of a torrent (each as its path components) against `rules`.
pub fn check_torrent_paths<I>(files: I, rules: Rules) -> Result<(), String>
where
    I: IntoIterator<Item = Vec<String>>,
{
    let mut seen: HashMap<String, String> = HashMap::new();
    for components in files {
        for c in &components {
            check_component(c, rules)?;
        }
        // Everywhere: no file system takes paths this long (Linux: 4096 bytes), and the folder
        // tree in the UI would nest that deep.
        let joined: usize = components.iter().map(|c| c.len() + 1).sum();
        if components.len() > MAX_COMPONENTS || joined > MAX_PATH_BYTES {
            let shown: String = components.join("/").chars().take(120).collect();
            return Err(t!("The torrent contains a path that is too deep or too long: {path}…", path = shown));
        }
        if rules.case_insensitive {
            let key = components
                .iter()
                .map(|c| {
                    let c = if rules.windows_names { c.trim_end_matches(['.', ' ']) } else { c.as_str() };
                    c.to_lowercase()
                })
                .collect::<Vec<_>>()
                .join("/");
            let shown = components.join("/");
            if let Some(first) = seen.insert(key, shown.clone()) {
                return Err(t!(
                    "Two files in the torrent differ only in letter case, which this system stores as one file: {a} and {b}",
                    a = first,
                    b = shown
                ));
            }
        }
    }
    Ok(())
}

fn check_component(c: &str, rules: Rules) -> Result<(), String> {
    let bad = c.chars().any(char::is_control)
        || rules.windows_names
            && (c.chars().any(|ch| matches!(ch, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
                || is_reserved(c)
                || c.ends_with(['.', ' '])
                || c.encode_utf16().count() > 255);
    if bad {
        let shown: String = c.chars().map(|ch| if ch.is_control() { '\u{FFFD}' } else { ch }).take(80).collect();
        return Err(t!("The torrent contains a file name this system can't store safely: {name}", name = shown));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: Rules = Rules { windows_names: true, case_insensitive: true };
    const LINUX: Rules = Rules { windows_names: false, case_insensitive: false };
    const MAC: Rules = Rules { windows_names: false, case_insensitive: true };

    fn files(list: &[&[&str]]) -> Vec<Vec<String>> {
        list.iter().map(|f| f.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn windows_refuses_each_unsafe_class() {
        let long = "x".repeat(256);
        for bad in [
            &["C:evil"][..],
            &["dir", "file.txt:stream"],
            &["CON.txt"],
            &["sub", "nul"],
            &["com1 .log"],
            &["CONOUT$"],
            &["name."],
            &["name "],
            &["a\u{1}b"],
            &["what?"],
            &[long.as_str()],
        ] {
            assert!(check_torrent_paths(files(&[bad]), WIN).is_err(), "{bad:?}");
        }
        let ok = files(&[
            &["Movie (2020)", "Movie.mkv"],
            &["CONTENT.txt"],
            &["con-fig"],
            &["COM10"],
            &["x".repeat(255).as_str()],
        ]);
        assert_eq!(check_torrent_paths(ok, WIN), Ok(()));
    }

    #[test]
    fn case_only_duplicates_where_names_ignore_case() {
        let pair = files(&[&["A.txt"], &["a.txt"]]);
        let err = check_torrent_paths(pair.clone(), WIN).unwrap_err();
        assert!(err.contains("A.txt and a.txt"), "{err}");
        assert!(check_torrent_paths(pair.clone(), MAC).is_err());
        assert_eq!(check_torrent_paths(pair, LINUX), Ok(()));
        // Folders count too.
        assert!(check_torrent_paths(files(&[&["d", "x"], &["D", "X"]]), WIN).is_err());
        assert!(check_torrent_paths(files(&[&["É.txt"], &["é.txt"]]), WIN).is_err(), "Unicode case");
        assert_eq!(check_torrent_paths(files(&[&["a", "x"], &["b", "x"]]), WIN), Ok(()));
    }

    #[test]
    fn other_systems_keep_their_legal_names() {
        for name in ["Title: Subtitle.mkv", "CON.txt", "name.", "what?"] {
            assert_eq!(check_torrent_paths(files(&[&[name]]), LINUX), Ok(()), "{name}");
            assert_eq!(check_torrent_paths(files(&[&[name]]), MAC), Ok(()), "{name}");
        }
        assert!(check_torrent_paths(files(&[&["a\nb"]]), LINUX).is_err(), "control characters everywhere");
    }

    #[test]
    fn paths_too_deep_or_too_long_are_refused() {
        let deep: Vec<String> = (0..=MAX_COMPONENTS).map(|i| format!("d{i}")).collect();
        let err = check_torrent_paths(vec![deep], LINUX).unwrap_err();
        assert!(err.contains("too deep or too long"), "{err}");
        let ok: Vec<String> = (0..MAX_COMPONENTS).map(|i| format!("d{i}")).collect();
        assert_eq!(check_torrent_paths(vec![ok], LINUX), Ok(()));
        let long = vec!["a".repeat(200); 30];
        assert!(check_torrent_paths(vec![long], LINUX).is_err(), "6 KB of path");
    }

    #[test]
    fn reserved_names() {
        assert!(is_reserved("con") && is_reserved("Lpt9.tar.gz") && is_reserved("AUX "));
        assert!(!is_reserved("console") && !is_reserved("") && !is_reserved("COM0"));
    }
}
