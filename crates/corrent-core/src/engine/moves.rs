//! Moving a torrent's files to another folder, safely across volumes and across crashes.
//!
//! A move within one volume is a rename. Across volumes each file is copied to `<name>.part`,
//! synced, renamed into place and only then removed at the source, so a file at the
//! destination is always complete. The caller records each finished file (`on_moved`), and
//! after a crash `settle` ends with every file in one folder: it finishes the move, or moves
//! the files back, or reports that they are split.

use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;

#[derive(Debug)]
pub enum MoveError {
    /// The move failed and every file is back in the old folder.
    Back(anyhow::Error),
    /// The files are in two folders and couldn't be brought together.
    Split(anyhow::Error),
}

/// `<path>.part`, keeping the real extension in the name.
fn part_of(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// Move one file; `copy` skips the rename (tests, to exercise the cross-volume path).
fn move_one(src: &Path, dst: &Path, copy: bool) -> anyhow::Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    if dst.exists() {
        anyhow::bail!("{} already exists at the destination", dst.display());
    }
    if !copy && std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    // Different volume: copy to a .part file, sync it, then put it in place.
    let part = part_of(dst);
    let copied = std::fs::copy(src, &part)
        .with_context(|| format!("copying {}", src.display()))
        // Opened for writing: Windows refuses to flush a read-only handle.
        .and_then(|_| std::fs::OpenOptions::new().write(true).open(&part)?.sync_all().context("syncing the copy"))
        .and_then(|()| std::fs::rename(&part, dst).context("putting the copy in place"));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if let Err(e) = std::fs::remove_file(src) {
        // As before the move: otherwise the complete copy would block every later move there.
        let _ = std::fs::remove_file(dst);
        return Err(anyhow::Error::from(e).context(t!("removing {path} after copying it", path = src.display())));
    }
    Ok(())
}

/// Move `files` (relative paths) from `from` to `to`. `on_moved` records each finished file; if
/// it fails, the move stops as if that file had failed. On failure the moved files go back.
pub(super) fn move_files(
    from: &Path,
    to: &Path,
    files: &[PathBuf],
    on_moved: &mut dyn FnMut(&Path) -> anyhow::Result<()>,
    copy: bool,
) -> Result<(), MoveError> {
    // Torrents from before the name checks (engine::paths) could carry "C:x"-style names, which
    // `join` would turn into paths outside both folders.
    if let Some(bad) = files.iter().find(|rel| !rel.components().all(|c| matches!(c, Component::Normal(_)))) {
        return Err(MoveError::Back(anyhow::anyhow!("refusing to move {}: not a plain relative path", bad.display())));
    }
    let mut moved: Vec<&PathBuf> = Vec::new();
    for rel in files {
        let src = from.join(rel);
        if !src.is_file() {
            continue;
        }
        let result = move_one(&src, &to.join(rel), copy).and_then(|()| {
            moved.push(rel);
            on_moved(rel)
        });
        if let Err(e) = result {
            let stuck: Vec<String> = moved
                .iter()
                .rev()
                .filter(|rel| {
                    let r = move_one(&to.join(rel), &from.join(rel), copy);
                    if let Err(e2) = &r {
                        tracing::error!("could not move {} back: {e2:#}", to.join(rel).display());
                    }
                    r.is_err()
                })
                .map(|rel| rel.display().to_string())
                .collect();
            return Err(if stuck.is_empty() {
                MoveError::Back(e)
            } else {
                MoveError::Split(
                    e.context(t!("and these files couldn't be moved back: {files}", files = stuck.join(", "))),
                )
            });
        }
    }
    remove_empty_dirs(from, files);
    Ok(())
}

/// Remove the folders a move left empty (best effort).
fn remove_empty_dirs(from: &Path, files: &[PathBuf]) {
    let mut dirs: Vec<PathBuf> = files.iter().filter_map(|f| f.parent().map(|p| from.join(p))).collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
    let _ = std::fs::remove_dir(from);
}

/// After a crash in the middle of a move from `old` to `new`: bring every file into one folder
/// and return it. `moved` are the files the note recorded as finished. An error means the files
/// are split and were left as they are.
pub(super) fn settle(
    old: &Path,
    new: &Path,
    files: &[PathBuf],
    moved: &[PathBuf],
    copy: bool,
) -> anyhow::Result<PathBuf> {
    if old == new {
        return Ok(old.to_path_buf());
    }
    // A .part file is an unfinished copy; its original is still at the source.
    for rel in files {
        let _ = std::fs::remove_file(part_of(&new.join(rel)));
    }
    if !files.iter().any(|rel| new.join(rel).is_file()) {
        return Ok(old.to_path_buf());
    }
    // In both folders: the file the crash hit between "in place" and "source removed". Ours if
    // the note recorded it, or (crash before the note was updated) if it has the same size.
    for rel in files {
        let (o, n) = (old.join(rel), new.join(rel));
        if o.is_file() && n.is_file() {
            let same_size =
                matches!((std::fs::metadata(&o), std::fs::metadata(&n)), (Ok(a), Ok(b)) if a.len() == b.len());
            anyhow::ensure!(
                moved.contains(rel) || same_size,
                "{} is in both folders and the copies differ",
                rel.display()
            );
            std::fs::remove_file(&o).with_context(|| format!("removing {}", o.display()))?;
        }
    }
    // Finish the move: what is still in the old folder goes to the new one.
    let rest: Vec<PathBuf> = files.iter().filter(|rel| old.join(rel).is_file()).cloned().collect();
    match move_files(old, new, &rest, &mut |_| Ok(()), copy) {
        Ok(()) => Ok(new.to_path_buf()),
        Err(MoveError::Back(forward)) => {
            // Couldn't finish it: move everything back instead.
            let at_new: Vec<PathBuf> = files.iter().filter(|rel| new.join(rel).is_file()).cloned().collect();
            match move_files(new, old, &at_new, &mut |_| Ok(()), copy) {
                Ok(()) => Ok(old.to_path_buf()),
                Err(MoveError::Back(e) | MoveError::Split(e)) => {
                    Err(e.context(t!("finishing the move failed too: {e}", e = format!("{forward:#}"))))
                }
            }
        }
        Err(MoveError::Split(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: [&str; 4] = ["a.bin", "sub/b.bin", "sub/c.bin", "d.bin"];

    struct Dirs {
        base: PathBuf,
        old: PathBuf,
        new: PathBuf,
    }

    impl Dirs {
        fn new(name: &str) -> Dirs {
            let base = std::env::temp_dir().join(format!("corrent-moves-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let (old, new) = (base.join("old"), base.join("vol2").join("new"));
            for f in FILES {
                let p = old.join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, f.as_bytes()).unwrap();
            }
            Dirs { base, old, new }
        }

        fn files() -> Vec<PathBuf> {
            FILES.iter().map(PathBuf::from).collect()
        }

        /// Every file is in `dir` with its contents, none anywhere else, and no .part is left.
        fn all_in(&self, dir: &Path) {
            let other = if dir == self.old { &self.new } else { &self.old };
            for f in FILES {
                assert_eq!(std::fs::read(dir.join(f)).unwrap(), f.as_bytes(), "{f} in {}", dir.display());
                assert!(!other.join(f).exists(), "{f} also in {}", other.display());
                assert!(!part_of(&self.new.join(f)).exists() && !part_of(&self.old.join(f)).exists());
            }
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn names_that_leave_the_folder_are_refused() {
        let d = Dirs::new("escape");
        for bad in ["../x", "/etc/x", "C:x"] {
            let bad = PathBuf::from(bad);
            if bad.components().all(|c| matches!(c, Component::Normal(_))) {
                continue; // "C:x" is a plain name on Unix
            }
            let r = move_files(&d.old, &d.new, std::slice::from_ref(&bad), &mut |_| Ok(()), false);
            assert!(matches!(r, Err(MoveError::Back(_))), "{bad:?}");
        }
        d.all_in(&d.old);
    }

    #[cfg(windows)]
    #[test]
    fn a_source_that_cannot_be_removed_leaves_no_copy_behind() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = Dirs::new("held-src");
        // Readable, but can't be deleted while this handle is open (no FILE_SHARE_DELETE).
        let held = std::fs::File::options().read(true).share_mode(1).open(d.old.join("d.bin")).unwrap();
        let r = move_files(&d.old, &d.new, &Dirs::files(), &mut |_| Ok(()), true);
        assert!(matches!(r, Err(MoveError::Back(_))), "{r:?}");
        drop(held);
        d.all_in(&d.old);
        // And the next try is not blocked by a leftover copy.
        move_files(&d.old, &d.new, &Dirs::files(), &mut |_| Ok(()), true).unwrap();
        d.all_in(&d.new);
    }

    #[test]
    fn moves_by_copy_and_records_each_file() {
        for copy in [false, true] {
            let d = Dirs::new(&format!("plain-{copy}"));
            let mut log = Vec::new();
            move_files(
                &d.old,
                &d.new,
                &Dirs::files(),
                &mut |f| {
                    log.push(f.to_path_buf());
                    Ok(())
                },
                copy,
            )
            .unwrap();
            d.all_in(&d.new);
            assert_eq!(log, Dirs::files());
            assert!(!d.old.exists(), "the emptied folder is removed");
        }
    }

    #[test]
    fn a_failure_after_any_file_moves_everything_back() {
        for k in 0..FILES.len() {
            for copy in [false, true] {
                let d = Dirs::new(&format!("abort-{k}-{copy}"));
                let mut n = 0;
                let r = move_files(
                    &d.old,
                    &d.new,
                    &Dirs::files(),
                    &mut |_| {
                        n += 1;
                        if n > k { anyhow::bail!("crash after file {k}") } else { Ok(()) }
                    },
                    copy,
                );
                assert!(matches!(r, Err(MoveError::Back(_))), "{k}: {r:?}");
                d.all_in(&d.old);
            }
        }
    }

    #[test]
    fn recovery_finishes_an_interrupted_move() {
        for k in 0..=FILES.len() {
            let d = Dirs::new(&format!("settle-{k}"));
            // The first k files made it before the crash.
            let moved: Vec<PathBuf> = Dirs::files().into_iter().take(k).collect();
            move_files(&d.old, &d.new, &moved, &mut |_| Ok(()), false).unwrap();
            let at = settle(&d.old, &d.new, &Dirs::files(), &moved, true).unwrap();
            let expect = if k == 0 { &d.old } else { &d.new };
            assert_eq!(&at, expect, "{k}");
            d.all_in(expect);
        }
    }

    #[test]
    fn recovery_after_a_crash_between_copy_and_note() {
        let d = Dirs::new("both");
        // a.bin was copied and put in place, but the source wasn't removed and the note not updated;
        // sub/b.bin was half-copied.
        let new_a = d.new.join("a.bin");
        std::fs::create_dir_all(new_a.parent().unwrap()).unwrap();
        std::fs::copy(d.old.join("a.bin"), &new_a).unwrap();
        std::fs::create_dir_all(d.new.join("sub")).unwrap();
        std::fs::write(part_of(&d.new.join("sub/b.bin")), b"sub/").unwrap();
        assert_eq!(settle(&d.old, &d.new, &Dirs::files(), &[], false).unwrap(), d.new);
        d.all_in(&d.new);
    }

    #[test]
    fn recovery_moves_back_when_it_cannot_finish() {
        let d = Dirs::new("back");
        move_files(&d.old, &d.new, &Dirs::files()[..1], &mut |_| Ok(()), false).unwrap();
        // Something else now sits where sub/b.bin must go.
        std::fs::create_dir_all(d.new.join("sub/b.bin")).unwrap();
        let at = settle(&d.old, &d.new, &Dirs::files(), &Dirs::files()[..1], false).unwrap();
        assert_eq!(at, d.old);
        for f in FILES {
            assert_eq!(std::fs::read(d.old.join(f)).unwrap(), f.as_bytes());
        }
        assert!(!d.new.join("a.bin").exists());
    }

    #[test]
    fn a_failed_move_back_is_reported_as_split() {
        let d = Dirs::new("split");
        move_files(&d.old, &d.new, &Dirs::files()[..2], &mut |_| Ok(()), false).unwrap();
        // Neither direction works: blocked at the new folder for c.bin, at the old one for a.bin.
        std::fs::create_dir_all(d.new.join("sub/c.bin")).unwrap();
        std::fs::create_dir_all(d.old.join("a.bin")).unwrap();
        let moved = &Dirs::files()[..2];
        let err = settle(&d.old, &d.new, &Dirs::files(), moved, false).unwrap_err();
        assert!(format!("{err:#}").contains("finishing the move failed too"), "{err:#}");
        // Nothing was lost: each file is in one of the two folders.
        for f in FILES {
            let here = |dir: &Path| std::fs::read(dir.join(f)).is_ok_and(|b| b == f.as_bytes());
            assert!(here(&d.old) || here(&d.new), "{f}");
        }
        // The live move reports it the same way.
        let d = Dirs::new("split-live");
        std::fs::create_dir_all(d.new.join("d.bin")).unwrap();
        let mut n = 0;
        let r = move_files(
            &d.old,
            &d.new,
            &Dirs::files(),
            &mut |f| {
                n += 1;
                if n == 1 {
                    // Block a.bin's way back before the failure.
                    std::fs::create_dir_all(d.old.join(f)).ok();
                }
                Ok(())
            },
            false,
        );
        assert!(matches!(r, Err(MoveError::Split(_))), "{r:?}");
    }
}
