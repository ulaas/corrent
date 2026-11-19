//! Taking media out of a library: either its files are deleted, or they stay where they are and
//! the library leaves them out from then on (`ignored_files`, which the scan skips).
//!
//! An item is removed with everything inside it (a show with its seasons and episodes). Only
//! files inside the library's folders are ever deleted: the media files, and what lies next to
//! one under its name (`Movie.en.srt`, `Movie.nfo`, `Movie-poster.jpg`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use corrent_db::rusqlite::{OptionalExtension, params};
use corrent_proto::library::{IgnoredFile, RemovalFailure, RemovalFile, RemovalPlan, Removed};

use crate::parse::{self, FileKind};
use crate::{LibError, Library, Result, now_unix};

/// What the database says about an item to remove.
struct Target {
    library_id: i64,
    title: String,
    /// The media files of the item and of everything inside it: (file id, path).
    files: Vec<(i64, String)>,
    /// Every other file of the library: never taken for something that goes with a removed one.
    others: HashSet<String>,
}

impl Library {
    async fn removal_target(&self, user: i64, item: i64) -> Result<Target> {
        self.db()
            .read(move |c| {
                if !crate::query::visible(c, user, item)? {
                    return Ok(None);
                }
                let head: Option<(i64, String)> = c
                    .query_row("SELECT library_id, title FROM items WHERE id = ?1", [item], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                let Some((library_id, title)) = head else { return Ok(None) };
                let files: Vec<(i64, String)> = c
                    .prepare(
                        "WITH RECURSIVE d(id) AS (SELECT ?1 UNION ALL SELECT i.id FROM items i JOIN d ON i.parent_id = d.id) \
                         SELECT DISTINCT f.id, f.path FROM d JOIN item_files x ON x.item_id = d.id JOIN files f ON f.id = x.file_id \
                         ORDER BY f.path",
                    )?
                    .query_map([item], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<corrent_db::rusqlite::Result<_>>()?;
                let mine: HashSet<i64> = files.iter().map(|f| f.0).collect();
                let others = c
                    .prepare("SELECT id, path FROM files WHERE library_id = ?1")?
                    .query_map([library_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .filter_map(|r| r.ok())
                    .filter(|(id, _)| !mine.contains(id))
                    .map(|(_, p)| p)
                    .collect();
                Ok(Some(Target { library_id, title, files, others }))
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    /// What removing `item` is about: its files and what goes with them, for the confirmation.
    pub async fn removal_plan(&self, user: i64, item: i64) -> Result<RemovalPlan> {
        let t = self.removal_target(user, item).await?;
        let (_, roots, _) = self.library_row(t.library_id).await?;
        let (library_id, title) = (t.library_id, t.title.clone());
        let files = tokio::task::spawn_blocking(move || files_to_delete(&t, &roots))
            .await
            .map_err(|e| LibError::Db(e.to_string()))?;
        Ok(RemovalPlan {
            item_id: item,
            library_id,
            title,
            size: files.iter().map(|f| f.size).sum(),
            files,
            torrents: Vec::new(),
        })
    }

    /// Removes `item` and everything inside it from its library. With `delete_files` its files
    /// are deleted from the disk; without, they stay and the library ignores them from now on.
    /// A file that can't be deleted stays in the library, with its item.
    pub async fn remove_item(&self, user: i64, item: i64, delete_files: bool) -> Result<Removed> {
        let t = self.removal_target(user, item).await?;
        let library_id = t.library_id;
        let title = t.title.clone();
        let media = t.files.clone();
        let mut out = Removed::default();
        // Media files that are gone from the library: all of them, or those that were deleted.
        let mut gone: Vec<(i64, String)> = media.clone();
        if delete_files {
            let (_, roots, _) = self.library_row(library_id).await?;
            let (deleted, failed) = tokio::task::spawn_blocking(move || {
                let plan = files_to_delete(&t, &roots);
                delete(&plan, &roots)
            })
            .await
            .map_err(|e| LibError::Db(e.to_string()))?;
            out.deleted = deleted;
            gone.retain(|(_, p)| !failed.iter().any(|f| &f.path == p));
            out.failed = failed;
        }
        out.files = gone.len();
        let whole = gone.len() == media.len();
        let now = now_unix();
        self.db()
            .write(move |tx| {
                for (id, path) in &gone {
                    if !delete_files {
                        tx.execute(
                            "INSERT OR REPLACE INTO ignored_files(library_id, path, title, added_at) VALUES (?1, ?2, ?3, ?4)",
                            params![library_id, path, title, now],
                        )?;
                    }
                    tx.execute("DELETE FROM files WHERE id = ?1", [id])?;
                }
                // Everything inside goes with it; what a file couldn't be deleted of stays.
                if whole {
                    tx.execute("DELETE FROM items WHERE id = ?1", [item])?;
                }
                crate::scan::prune(tx, library_id)?;
                Ok(())
            })
            .await?;
        self.emit_changed(library_id);
        Ok(out)
    }

    /// The files a library leaves out, newest first.
    pub async fn ignored_files(&self, library_id: i64) -> Result<Vec<IgnoredFile>> {
        Ok(self
            .db()
            .read(move |c| {
                c.prepare("SELECT path, title, added_at FROM ignored_files WHERE library_id = ?1 ORDER BY added_at DESC, path")?
                    .query_map([library_id], |r| Ok(IgnoredFile { path: r.get(0)?, title: r.get(1)?, added_at: r.get(2)? }))?
                    .collect()
            })
            .await?)
    }

    /// Lets the next scan see these files again. Returns how many were on the list.
    pub async fn restore_ignored(&self, library_id: i64, paths: Vec<String>) -> Result<usize> {
        Ok(self
            .db()
            .write(move |tx| {
                let mut n = 0;
                for p in &paths {
                    n += tx.execute(
                        "DELETE FROM ignored_files WHERE library_id = ?1 AND path = ?2",
                        params![library_id, p],
                    )?;
                }
                Ok(n)
            })
            .await?)
    }

    /// The paths a library's scan skips.
    pub(crate) async fn ignored_paths(&self, library_id: i64) -> Result<HashSet<String>> {
        Ok(self
            .db()
            .read(move |c| {
                c.prepare("SELECT path FROM ignored_files WHERE library_id = ?1")?
                    .query_map([library_id], |r| r.get(0))?
                    .collect()
            })
            .await?)
    }
}

fn inside(roots: &[PathBuf], p: &Path) -> bool {
    roots.iter().any(|r| p != r && p.starts_with(r))
}

/// The media files of a removal and the files that go with each: in the same folder, named as
/// the media file with something after a `.` or `-` (`Movie.en.srt`, `Movie.mkv.nfo`,
/// `Movie-poster.jpg`), and not media themselves. Blocking.
fn files_to_delete(t: &Target, roots: &[PathBuf]) -> Vec<RemovalFile> {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mine: HashSet<&str> = t.files.iter().map(|f| f.1.as_str()).collect();
    let mut out: Vec<RemovalFile> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for (_, path) in &t.files {
        let p = PathBuf::from(path);
        if !inside(roots, &p) || !seen.insert(p.clone()) {
            continue;
        }
        out.push(RemovalFile { path: path.clone(), size: size(&p), media: true });
        let (Some(dir), Some(stem)) = (p.parent(), p.file_stem().and_then(|s| s.to_str())) else { continue };
        let stem = stem.to_lowercase();
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut with: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .filter(|q| {
                let name = q.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
                let named_after = name.strip_prefix(&stem).is_some_and(|rest| rest.starts_with(['.', '-']));
                let shown = q.display().to_string();
                named_after
                    && !mine.contains(shown.as_str())
                    && !t.others.contains(&shown)
                    && !matches!(parse::file_kind(q), FileKind::Video | FileKind::Audio)
            })
            .collect();
        with.sort();
        for q in with {
            if seen.insert(q.clone()) {
                out.push(RemovalFile { path: q.display().to_string(), size: size(&q), media: false });
            }
        }
    }
    out
}

/// Deletes the files, then the folders that this left empty (never a library's own folder).
/// Returns how many files were deleted and which weren't. Blocking.
fn delete(files: &[RemovalFile], roots: &[PathBuf]) -> (usize, Vec<RemovalFailure>) {
    let mut deleted = 0;
    let mut failed = Vec::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    for f in files {
        let p = Path::new(&f.path);
        if !inside(roots, p) {
            continue;
        }
        match std::fs::remove_file(p) {
            Ok(()) => deleted += 1,
            // Gone already: that is what was asked for.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                failed.push(RemovalFailure { path: f.path.clone(), error: e.to_string() });
                continue;
            }
        }
        if let Some(d) = p.parent()
            && !folders.iter().any(|x| x == d)
        {
            folders.push(d.to_path_buf());
        }
    }
    for d in folders {
        let mut d = d.as_path();
        // `remove_dir` only removes an empty folder.
        while inside(roots, d) && std::fs::remove_dir(d).is_ok() {
            let Some(up) = d.parent() else { break };
            d = up;
        }
    }
    (deleted, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(files: &[&Path], others: &[&Path]) -> Target {
        Target {
            library_id: 1,
            title: "t".into(),
            files: files.iter().enumerate().map(|(i, p)| (i as i64, p.display().to_string())).collect(),
            others: others.iter().map(|p| p.display().to_string()).collect(),
        }
    }

    #[test]
    fn only_what_is_named_after_the_media_file_goes_with_it() {
        let root = std::env::temp_dir().join(format!("corrent-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("Movie (2001)");
        std::fs::create_dir_all(&dir).unwrap();
        let names = [
            "Movie.mkv",
            "Movie.en.srt",
            "Movie.mkv.nfo",
            "Movie-poster.jpg",
            // Another movie whose name begins the same way, known to the library or not.
            "Movie.2.mkv",
            "Movie-trailer.mp4",
            "Movies.srt",
            "Other.srt",
            "readme.txt",
        ];
        for n in names {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let (movie, other) = (dir.join("Movie.mkv"), dir.join("Movie.2.mkv"));
        let roots = vec![root.clone()];
        let plan = files_to_delete(&target(&[&movie], &[&other]), &roots);
        let got: Vec<(String, bool)> = plan
            .iter()
            .map(|f| (Path::new(&f.path).file_name().unwrap().to_string_lossy().into_owned(), f.media))
            .collect();
        let want =
            [("Movie.mkv", true), ("Movie-poster.jpg", false), ("Movie.en.srt", false), ("Movie.mkv.nfo", false)];
        assert_eq!(got, want.map(|(n, m)| (n.to_string(), m)));

        // Outside the library's folders nothing is listed, and nothing deleted.
        let elsewhere = vec![root.join("elsewhere")];
        assert!(files_to_delete(&target(&[&movie], &[]), &elsewhere).is_empty());
        assert_eq!(delete(&plan, &elsewhere), (0, Vec::new()));
        assert!(movie.exists());

        let (deleted, failed) = delete(&plan, &roots);
        assert_eq!((deleted, failed.len()), (4, 0));
        assert!(!movie.exists() && other.exists() && dir.join("readme.txt").exists());
        // The folder still has files: it stays. Emptied, it goes; the library's folder never does.
        let lone = root.join("Lone (1999)").join("Lone.mkv");
        std::fs::create_dir_all(lone.parent().unwrap()).unwrap();
        std::fs::write(&lone, b"x").unwrap();
        let plan = files_to_delete(&target(&[&lone], &[]), &roots);
        assert_eq!(delete(&plan, &roots).0, 1);
        assert!(!lone.parent().unwrap().exists() && root.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
