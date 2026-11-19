//! Taking media out of a library: either its files are deleted, or they stay where they are and
//! the library leaves them out from then on (`ignored_files`, which the scan skips).
//!
//! An item is removed with everything inside it (a show with its seasons and episodes). Only
//! files inside the library's folders are ever deleted: the media files, and what lies next to
//! one under its name (`Movie.en.srt`, `Movie.nfo`, `Movie-poster.jpg`) and under no other media
//! file's name. What goes with a media file is deleted only once that file is gone.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use corrent_db::rusqlite::{OptionalExtension, params};
use corrent_proto::library::{IgnoredFile, LibraryKind, RemovalFailure, RemovalFile, RemovalPlan, Removed};

use crate::parse::{self, FileKind};
use crate::{LibError, Library, Result, now_unix};

/// What the database says about an item to remove.
struct Target {
    library_id: i64,
    title: String,
    /// The media files of the item and of everything inside it: (file id, path).
    files: Vec<(i64, String)>,
    /// Paths that are never taken for something that goes with a removed file: the library's
    /// other files, and the ones it ignores.
    protected: HashSet<String>,
    /// Episodes inside it that belong to a followed show (show title, season, episode), unless
    /// the show itself is removed: the monitor must not get them again.
    followed: Vec<(String, i64, i64)>,
}

/// One media file of a removal with the files that go with it.
struct Group {
    media: RemovalFile,
    with: Vec<RemovalFile>,
    /// Inside one of the library's folders: only then is anything deleted.
    inside: bool,
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
                const INSIDE: &str =
                    "WITH RECURSIVE d(id) AS (SELECT ?1 UNION ALL SELECT i.id FROM items i JOIN d ON i.parent_id = d.id) ";
                let files: Vec<(i64, String)> = c
                    .prepare(&format!(
                        "{INSIDE} SELECT DISTINCT f.id, f.path FROM d JOIN item_files x ON x.item_id = d.id \
                         JOIN files f ON f.id = x.file_id ORDER BY f.path"
                    ))?
                    .query_map([item], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<corrent_db::rusqlite::Result<_>>()?;
                let mine: HashSet<i64> = files.iter().map(|f| f.0).collect();
                let mut protected: HashSet<String> = c
                    .prepare("SELECT id, path FROM files WHERE library_id = ?1")?
                    .query_map([library_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .filter_map(|r| r.ok())
                    .filter(|(id, _)| !mine.contains(id))
                    .map(|(_, p)| p)
                    .collect();
                protected.extend(
                    c.prepare("SELECT path FROM ignored_files WHERE library_id = ?1")?
                        .query_map([library_id], |r| r.get::<_, String>(0))?
                        .filter_map(|r| r.ok()),
                );
                let followed = c
                    .prepare(&format!(
                        "{INSIDE} SELECT sh.title, s.index_no, e.index_no FROM d JOIN items e ON e.id = d.id AND e.kind = 'episode' \
                         JOIN items s ON s.id = e.parent_id JOIN items sh ON sh.id = s.parent_id \
                         JOIN monitored_shows m ON m.item_id = sh.id \
                         WHERE sh.id != ?1 AND s.index_no IS NOT NULL AND e.index_no IS NOT NULL"
                    ))?
                    .query_map([item], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<corrent_db::rusqlite::Result<_>>()?;
                Ok(Some(Target { library_id, title, files, protected, followed }))
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    /// What removing `item` is about: its files and what goes with them, for the confirmation.
    pub async fn removal_plan(&self, user: i64, item: i64) -> Result<RemovalPlan> {
        let t = self.removal_target(user, item).await?;
        let (kind, roots, _) = self.library_row(t.library_id).await?;
        let (library_id, title) = (t.library_id, t.title.clone());
        let groups = tokio::task::spawn_blocking(move || groups(&t, &roots, kind))
            .await
            .map_err(|e| LibError::Db(e.to_string()))?;
        let files: Vec<RemovalFile> = groups.into_iter().flat_map(|g| std::iter::once(g.media).chain(g.with)).collect();
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
    /// A media file that can't be deleted stays in the library, with its item. Also returns the
    /// ids of the file rows that are gone (what was kept about them elsewhere can go).
    pub async fn remove_item(&self, user: i64, item: i64, delete_files: bool) -> Result<(Removed, Vec<i64>)> {
        let t = self.removal_target(user, item).await?;
        let library_id = t.library_id;
        let title = t.title.clone();
        let media = t.files.clone();
        let followed = t.followed.clone();
        let mut out = Removed::default();
        // Media files that are gone from the library: all of them, or those that were deleted.
        let mut gone: Vec<(i64, String)> = media.clone();
        if delete_files {
            let (kind, roots, _) = self.library_row(library_id).await?;
            let (deleted, failed, kept) = tokio::task::spawn_blocking(move || {
                let plan = groups(&t, &roots, kind);
                delete(&plan, &roots)
            })
            .await
            .map_err(|e| LibError::Db(e.to_string()))?;
            out.deleted = deleted;
            gone.retain(|(_, p)| !kept.contains(p));
            out.failed = failed;
        }
        out.files = gone.len();
        let whole = gone.len() == media.len();
        let ids: Vec<i64> = gone.iter().map(|g| g.0).collect();
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
                    // An episode of a followed show isn't looked for again: to the monitor it
                    // counts as dealt with.
                    for (show, season, episode) in &followed {
                        tx.execute(
                            "INSERT INTO wanted(library_id, kind, title, season, episode, status, added_by, created_at) \
                             SELECT ?1, 'episode', ?2, ?3, ?4, 'done', 'removed', ?5 WHERE NOT EXISTS \
                             (SELECT 1 FROM wanted WHERE title = ?2 COLLATE NOCASE AND season = ?3 AND episode = ?4)",
                            params![library_id, show, season, episode, now],
                        )?;
                    }
                }
                crate::scan::prune(tx, library_id)?;
                Ok(())
            })
            .await?;
        self.emit_changed(library_id);
        Ok((out, ids))
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

    /// The paths a library's scan skips. Rows a scan wrote for one of them meanwhile (it ran
    /// while the file was being removed) go as well.
    pub(crate) async fn ignored_paths(&self, library_id: i64) -> Result<HashSet<String>> {
        Ok(self
            .db()
            .write(move |tx| {
                let paths: HashSet<String> = tx
                    .prepare("SELECT path FROM ignored_files WHERE library_id = ?1")?
                    .query_map([library_id], |r| r.get(0))?
                    .collect::<corrent_db::rusqlite::Result<_>>()?;
                if !paths.is_empty() {
                    tx.execute(
                        "DELETE FROM files WHERE library_id = ?1 AND path IN (SELECT path FROM ignored_files WHERE library_id = ?1)",
                        [library_id],
                    )?;
                }
                Ok(paths)
            })
            .await?)
    }
}

fn inside(roots: &[PathBuf], p: &Path) -> bool {
    roots.iter().any(|r| p != r && p.starts_with(r))
}

/// Whether a file named `name` is named after one whose stem is `stem` (both lowercase):
/// `movie.en.srt`, `movie.mkv.nfo`, `movie-poster.jpg` after `movie`.
fn named_after(name: &str, stem: &str) -> bool {
    name.strip_prefix(stem).is_some_and(|rest| rest.starts_with(['.', '-']))
}

/// The media files of a removal, each with the files that go with it: in the same folder, named
/// after it and after no other media file there with a longer name (`Rocky.II.en.srt` belongs to
/// `Rocky.II.mkv`, not to `Rocky.mkv`). Media files are never what goes with another. Blocking.
fn groups(t: &Target, roots: &[PathBuf], kind: LibraryKind) -> Vec<Group> {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let lower_stem = |p: &Path| p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_lowercase();
    let mine: HashSet<&str> = t.files.iter().map(|f| f.1.as_str()).collect();
    let mut out: Vec<Group> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for (_, path) in &t.files {
        let p = PathBuf::from(path);
        if !seen.insert(p.clone()) {
            continue;
        }
        let mut g = Group {
            media: RemovalFile { path: path.clone(), size: size(&p), media: true },
            with: Vec::new(),
            inside: inside(roots, &p),
        };
        let listing = if g.inside { p.parent().and_then(|d| std::fs::read_dir(d).ok()) } else { None };
        if let Some(rd) = listing {
            let all: Vec<PathBuf> =
                rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.path()).collect();
            // Media: by its kind, what this library scans, what it knows and what it ignores.
            let is_media = |q: &Path| {
                let shown = q.display().to_string();
                matches!(parse::file_kind(q), FileKind::Video | FileKind::Audio)
                    || crate::scan::wanted(q, kind)
                    || mine.contains(shown.as_str())
                    || t.protected.contains(&shown)
            };
            let stems: Vec<String> = all.iter().filter(|q| is_media(q)).map(|q| lower_stem(q)).collect();
            let my_stem = lower_stem(&p);
            let mut with: Vec<PathBuf> = all
                .iter()
                .filter(|q| {
                    let name = q.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_lowercase();
                    !is_media(q)
                        && named_after(&name, &my_stem)
                        && !stems.iter().any(|s| s.len() > my_stem.len() && named_after(&name, s))
                })
                .cloned()
                .collect();
            with.sort();
            for q in with {
                if seen.insert(q.clone()) {
                    g.with.push(RemovalFile { path: q.display().to_string(), size: size(&q), media: false });
                }
            }
        }
        out.push(g);
    }
    out
}

/// Deletes each media file and then what goes with it, and the folders this left empty (never
/// a library's own folder). Returns how many files were deleted, what couldn't be, and the media
/// files that are therefore still there. Blocking.
fn delete(groups: &[Group], roots: &[PathBuf]) -> (usize, Vec<RemovalFailure>, HashSet<String>) {
    let mut deleted = 0;
    let mut failed = Vec::new();
    let mut kept = HashSet::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    let fail = |failed: &mut Vec<RemovalFailure>, path: &str, error: String| {
        failed.push(RemovalFailure { path: path.to_string(), error });
    };
    for g in groups {
        let p = Path::new(&g.media.path);
        if !g.inside {
            fail(&mut failed, &g.media.path, "not inside one of the library's folders".into());
            kept.insert(g.media.path.clone());
            continue;
        }
        let gone = match std::fs::remove_file(p) {
            Ok(()) => {
                deleted += 1;
                Ok(())
            }
            // Gone already, which is what was asked for: but only where the library's folder can
            // be read (an unplugged drive says "not found" for everything on it).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let reachable = roots.iter().filter(|r| p.starts_with(r)).any(|r| std::fs::read_dir(r).is_ok());
                if reachable { Ok(()) } else { Err("the library's folder can't be reached".to_string()) }
            }
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = gone {
            fail(&mut failed, &g.media.path, e);
            kept.insert(g.media.path.clone());
            continue;
        }
        for w in &g.with {
            match std::fs::remove_file(&w.path) {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => fail(&mut failed, &w.path, e.to_string()),
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
    (deleted, failed, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(files: &[&Path], protected: &[&Path]) -> Target {
        Target {
            library_id: 1,
            title: "t".into(),
            files: files.iter().enumerate().map(|(i, p)| (i as i64, p.display().to_string())).collect(),
            protected: protected.iter().map(|p| p.display().to_string()).collect(),
            followed: Vec::new(),
        }
    }

    fn names(groups: &[Group]) -> Vec<(String, bool)> {
        groups
            .iter()
            .flat_map(|g| std::iter::once(&g.media).chain(&g.with))
            .map(|f| (Path::new(&f.path).file_name().unwrap().to_string_lossy().into_owned(), f.media))
            .collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("corrent-remove-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn only_what_is_named_after_the_media_file_goes_with_it() {
        let root = scratch("named");
        let dir = root.join("Movie (2001)");
        std::fs::create_dir_all(&dir).unwrap();
        let names_there = [
            "Movie.mkv",
            "Movie.en.srt",
            "Movie.mkv.nfo",
            "Movie-poster.jpg",
            // Another movie whose name begins the same way, known to the library or not, and
            // what goes with it.
            "Movie.2.mkv",
            "Movie.2.en.srt",
            "Movie.2-poster.jpg",
            "Movie-trailer.mp4",
            "Movie-trailer.srt",
            "Movies.srt",
            "Other.srt",
            "readme.txt",
        ];
        for n in names_there {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let (movie, other) = (dir.join("Movie.mkv"), dir.join("Movie.2.mkv"));
        let roots = vec![root.clone()];
        let plan = groups(&target(&[&movie], &[&other]), &roots, LibraryKind::Movies);
        let want =
            [("Movie.mkv", true), ("Movie-poster.jpg", false), ("Movie.en.srt", false), ("Movie.mkv.nfo", false)];
        assert_eq!(names(&plan), want.map(|(n, m)| (n.to_string(), m)));

        // Outside the library's folders nothing goes with it, and nothing is deleted.
        let elsewhere = vec![root.join("elsewhere")];
        let outside = groups(&target(&[&movie], &[]), &elsewhere, LibraryKind::Movies);
        assert_eq!(names(&outside), [("Movie.mkv".to_string(), true)]);
        let (deleted, failed, kept) = delete(&outside, &elsewhere);
        assert_eq!((deleted, failed.len(), kept.len()), (0, 1, 1));
        assert!(movie.exists());

        let (deleted, failed, kept) = delete(&plan, &roots);
        assert_eq!((deleted, failed.len(), kept.len()), (4, 0, 0));
        assert!(!movie.exists() && other.exists() && dir.join("readme.txt").exists());
        for n in ["Movie.2.en.srt", "Movie.2-poster.jpg", "Movie-trailer.mp4", "Movie-trailer.srt"] {
            assert!(dir.join(n).exists(), "{n}");
        }
        // The folder still has files: it stays. Emptied, it goes; the library's folder never does.
        let lone = root.join("Lone (1999)").join("Lone.mkv");
        std::fs::create_dir_all(lone.parent().unwrap()).unwrap();
        std::fs::write(&lone, b"x").unwrap();
        let plan = groups(&target(&[&lone], &[]), &roots, LibraryKind::Movies);
        assert_eq!(delete(&plan, &roots).0, 1);
        assert!(!lone.parent().unwrap().exists() && root.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pictures_of_a_photo_library_and_ignored_files_are_never_what_goes_with_another() {
        let root = scratch("photos");
        let dir = root.join("a");
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["IMG_1.jpg", "IMG_1-edit.jpg", "IMG_1.xmp", "Clip.mkv", "Clip-kept.nfo"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let roots = vec![root.clone()];
        // A photo that was never scanned is still a photo, not a part of `IMG_1.jpg`.
        let plan = groups(&target(&[&dir.join("IMG_1.jpg")], &[]), &roots, LibraryKind::Photos);
        assert_eq!(names(&plan), [("IMG_1.jpg".to_string(), true), ("IMG_1.xmp".to_string(), false)]);
        // A file the library ignores is somebody's kept media, whatever its kind.
        let plan =
            groups(&target(&[&dir.join("Clip.mkv")], &[&dir.join("Clip-kept.nfo")]), &roots, LibraryKind::Movies);
        assert_eq!(names(&plan), [("Clip.mkv".to_string(), true)]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_goes_with_a_file_that_cant_be_deleted_stays_too() {
        let root = scratch("locked");
        let dir = root.join("Movie");
        // A folder where the video should be: deleting it as a file fails.
        std::fs::create_dir_all(dir.join("Movie.mkv")).unwrap();
        std::fs::write(dir.join("Movie.en.srt"), b"x").unwrap();
        let movie = dir.join("Movie.mkv");
        let plan = vec![Group {
            media: RemovalFile { path: movie.display().to_string(), size: 0, media: true },
            with: vec![RemovalFile { path: dir.join("Movie.en.srt").display().to_string(), size: 1, media: false }],
            inside: true,
        }];
        let (deleted, failed, kept) = delete(&plan, std::slice::from_ref(&root));
        assert_eq!((deleted, failed.len()), (0, 1));
        assert!(kept.contains(&movie.display().to_string()) && dir.join("Movie.en.srt").exists());

        // Not found under a folder that can't be read (an unplugged drive): not "deleted".
        let away = root.join("unplugged");
        let lost = away.join("Film").join("Film.mkv");
        let plan = vec![Group {
            media: RemovalFile { path: lost.display().to_string(), size: 0, media: true },
            with: Vec::new(),
            inside: true,
        }];
        let (_, failed, kept) = delete(&plan, std::slice::from_ref(&away));
        assert_eq!((failed.len(), kept.len()), (1, 1));
        // Not found where the library's folder is there: gone already.
        let plan = vec![Group {
            media: RemovalFile { path: root.join("Gone.mkv").display().to_string(), size: 0, media: true },
            with: Vec::new(),
            inside: true,
        }];
        assert_eq!(delete(&plan, std::slice::from_ref(&root)), (0, Vec::new(), HashSet::new()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
