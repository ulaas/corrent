//! On Windows every child process of a GUI program gets its own console window unless it is
//! started with `CREATE_NO_WINDOW`: a probe or an encode would flash a terminal up. Every
//! `Command::new` in the workspace's code sets it within a few lines, or says why it doesn't
//! need it with a `NO_WINDOW_OK` comment just above (Linux/macOS-only code, a console program).
// Test tooling: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "tests" && name != "target" && name != "examples" {
                sources(&p, out);
            }
        } else if name.ends_with(".rs") && !name.contains("test") {
            out.push(p);
        }
    }
}

#[test]
fn child_processes_never_open_a_console_window() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    for c in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        sources(&c.path().join("src"), &mut files);
    }
    assert!(files.len() > 50, "found the sources: {}", files.len());
    let mut bad = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Unit tests at the end of a file are left out.
        let end = lines.iter().position(|l| l.trim() == "#[cfg(test)]").unwrap_or(lines.len());
        for (i, l) in lines[..end].iter().enumerate() {
            if !l.contains("Command::new(") || l.trim_start().starts_with("//") {
                continue;
            }
            let before = lines[i.saturating_sub(2)..i].iter().any(|l| l.contains("NO_WINDOW_OK"));
            let after = lines[i..(i + 10).min(end)].iter().any(|l| l.contains("creation_flags("));
            if !before && !after {
                bad.push(format!("{}:{}", f.display(), i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "Command::new without CREATE_NO_WINDOW (or a NO_WINDOW_OK note):\n{}", bad.join("\n"));
}
