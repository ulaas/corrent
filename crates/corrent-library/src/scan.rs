//! Scanning a library: walk the folders, diff against the database, recognise moved files by
//! a quick content hash, read what the files are (probe, tags, .nfo, names), place them in the
//! item tree, attach local artwork, and keep the full-text index current.
//!
//! Unchanged files (same size and mtime) cost one map lookup. Files that disappear are marked
//! missing and only removed after a grace period — an unplugged drive or an offline share
//! (its folder can't be read at all) never empties a library.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use corrent_db::rusqlite::{self, OptionalExtension, Transaction, params};
use corrent_mpv::{ProbeResult, TrackKind};
use corrent_proto::library::{ItemKind, LibraryKind, ScanStatus};

use crate::nfo::Nfo;
use crate::parse::{self, FileKind};
use crate::tags::AudioTags;
use crate::{LibError, Library, Result, artwork, now_unix};

/// Files missing longer than this are removed with their items.
pub const MISSING_GRACE_SECS: i64 = 30 * 86_400;
const QUICK_HASH_CHUNK: u64 = 64 * 1024;

#[derive(Clone, Debug)]
struct Found {
    path: PathBuf,
    size: u64,
    mtime: i64,
}

/// Walks `root` (no symlinked folders, no hidden entries, skips folders with `.nomedia` or
/// `.ignore`). `Err` when the root itself can't be read (drive unplugged, share offline).
fn walk(root: &Path, kind: LibraryKind) -> std::io::Result<Vec<Found>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) if first => return Err(e),
            Err(e) => {
                tracing::debug!("skipping {}: {e}", dir.display());
                continue;
            }
        };
        first = false;
        if dir.join(".nomedia").exists() || dir.join(".ignore").exists() {
            continue;
        }
        for e in rd.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
            } else if wanted(&p, kind)
                && let Ok(m) = e.metadata()
            {
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.push(Found { path: p, size: m.len(), mtime });
            }
        }
    }
    Ok(out)
}

fn wanted(p: &Path, kind: LibraryKind) -> bool {
    let k = parse::file_kind(p);
    match kind {
        LibraryKind::Movies | LibraryKind::Shows | LibraryKind::Videos => k == FileKind::Video,
        LibraryKind::Music => k == FileKind::Audio,
        LibraryKind::Audiobooks => k == FileKind::Audio,
        LibraryKind::Photos => k == FileKind::Image,
        LibraryKind::Podcasts => matches!(k, FileKind::Audio | FileKind::Video),
    }
}

/// SHA-256 over the size and the first and last 64 KiB: recognises a moved or renamed file
/// without reading it whole.
pub fn quick_hash(path: &Path, size: u64) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut ctx = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
    ctx.update(&size.to_le_bytes());
    let mut buf = vec![0u8; QUICK_HASH_CHUNK as usize];
    let n = f.read(&mut buf)?;
    ctx.update(&buf[..n]);
    if size > 2 * QUICK_HASH_CHUNK {
        f.seek(SeekFrom::Start(size - QUICK_HASH_CHUNK))?;
        let n = f.read(&mut buf)?;
        ctx.update(&buf[..n]);
    }
    Ok(ctx.finish().as_ref().iter().take(16).map(|b| format!("{b:02x}")).collect())
}

// ------------------------------------------------------------------ placement

/// Placement rules this build applies. Libraries placed by an older version are repaired once
/// (`repair`): files the old rules put in the wrong item move to the right one.
pub const PLACEMENT_VERSION: i64 = 1;

/// What a file's placement may use besides its own name.
#[derive(Clone, Copy, Debug, Default)]
struct Context {
    /// The only main video in its folder: the folder's name, `movie.nfo` and `poster.jpg`
    /// describe it (in a folder of several movies they describe none of them).
    alone: bool,
    /// The rules before placement version 1, only to find what the repair must move.
    legacy: bool,
}

/// Movies per folder, for `Context::alone`: the different titles of its main videos, so the
/// parts of a movie (`cd1`, `cd2`) and its versions (`1080p`, `4K`) count once and extras not
/// at all.
fn videos_per_folder<'a>(paths: impl Iterator<Item = &'a Path>) -> HashMap<PathBuf, usize> {
    let mut titles: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    for p in paths.filter(|p| parse::is_extra(p).is_none()) {
        if let Some(d) = p.parent() {
            titles.entry(d.to_path_buf()).or_default().insert(parse::norm_title(&parse::parse_name(&stem(p)).title));
        }
    }
    titles.into_iter().map(|(d, t)| (d, t.len())).collect()
}

fn context(per_folder: &HashMap<PathBuf, usize>, f: &Path) -> Context {
    Context { alone: f.parent().and_then(|d| per_folder.get(d)).is_none_or(|n| *n <= 1), legacy: false }
}

/// Where a file goes in the item tree.
#[derive(Clone, Debug, PartialEq)]
enum Placement {
    Movie {
        title: String,
        year: Option<i32>,
        role: &'static str,
    },
    Episode {
        show: String,
        show_year: Option<i32>,
        season: i64,
        episode: i64,
        episode_end: Option<i64>,
        title: Option<String>,
    },
    Track {
        artist: String,
        album: String,
        album_year: Option<i32>,
        disc: Option<i64>,
        track: Option<i64>,
        title: String,
    },
    Book {
        title: String,
        author: Option<String>,
    },
    Video {
        title: String,
        folder: String,
    },
    Photo {
        title: String,
        folder: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ArtTarget {
    Main,
    Show,
    Season,
    Album,
    Artist,
}

/// Everything learned about one file before it is written.
struct Analyzed {
    path: PathBuf,
    size: u64,
    mtime: i64,
    quick_hash: String,
    probe: Option<ProbeResult>,
    tags: Option<AudioTags>,
    nfo: Option<Nfo>,
    parent_nfo: Option<Nfo>,
    placement: Placement,
    art: Vec<(ArtTarget, &'static str, artwork::Stored, String)>,
    external_subs: Vec<(PathBuf, Option<String>, bool)>,
    /// Re-placed by the repair: an existing episode takes the file's title again.
    retitle: bool,
}

fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

fn folder_name(p: &Path) -> String {
    p.parent().and_then(|d| d.file_name()).and_then(|s| s.to_str()).unwrap_or("").to_string()
}

fn root_of<'a>(roots: &'a [PathBuf], p: &Path) -> &'a Path {
    roots
        .iter()
        .filter(|r| p.starts_with(r))
        .max_by_key(|r| r.as_os_str().len())
        .map(|r| r.as_path())
        .unwrap_or(Path::new("/"))
}

fn place(
    kind: LibraryKind,
    roots: &[PathBuf],
    f: &Path,
    tags: Option<&AudioTags>,
    nfo: Option<&Nfo>,
    cx: Context,
) -> Placement {
    let st = stem(f);
    match kind {
        LibraryKind::Movies => {
            let role = parse::is_extra(f).unwrap_or("main");
            let from_file = parse::parse_name(&st);
            let root = root_of(roots, f);
            // The movie's folder: above a disc structure (`Film/VIDEO_TS/VTS_01_1.VOB`).
            let dir = if cx.legacy { f.parent() } else { parse::movie_dir(f) };
            let dir_name = dir.and_then(|d| d.file_name()).and_then(|n| n.to_str()).unwrap_or("").to_string();
            let in_folder = !dir_name.is_empty() && dir != Some(root) && dir.is_some_and(|d| d.starts_with(root));
            // Generic file names ("movie.mkv", "VTS_01_1") take the folder's name, and so does a
            // file without a year alone in a folder that has one ("Dune (2021)/dune.mkv") or
            // whose name holds the file's ("The Shawshank Redemption/shawshank.mkv"). A folder
            // that groups movies ("Marvel/Thor.mkv", "4K/Avatar.mkv") names none of them.
            let weak = if cx.legacy {
                from_file.title.len() < 3 || from_file.year.is_none() && in_folder
            } else {
                let d = parse::parse_name(&dir_name);
                let (ft, dt) = (parse::norm_title(&from_file.title), parse::norm_title(&d.title));
                in_folder
                    && (parse::is_generic_name(&st)
                        || cx.alone
                            && from_file.year.is_none()
                            && (d.year.is_some() || !ft.is_empty() && dt.contains(ft.as_str())))
            };
            let p = if weak && role == "main" {
                let from_dir = parse::parse_name(&dir_name);
                if from_dir.title.is_empty() { from_file } else { from_dir }
            } else {
                from_file
            };
            let title = nfo.and_then(|n| n.title.clone()).unwrap_or(p.title);
            let year = nfo.and_then(|n| n.year).or(p.year.map(|y| y as i32));
            Placement::Movie { title, year, role }
        }
        LibraryKind::Shows => {
            let root = root_of(roots, f);
            let p = parse::parse_name(&st);
            let show_dir = if cx.legacy {
                parse::show_folder(root, f).map(parse::parse_name)
            } else {
                parse::show_dir(root, f).and_then(|d| d.file_name().and_then(|n| n.to_str()).map(parse::parse_name))
            };
            let (show, show_year) = match &show_dir {
                Some(sd) if !sd.title.is_empty() => (sd.title.clone(), sd.year.map(|y| y as i32)),
                _ => (p.title.clone(), p.year.map(|y| y as i32)),
            };
            let season = nfo
                .and_then(|n| n.season)
                .or(p.season.map(i64::from))
                .or_else(|| parse::season_folder(&folder_name(f)).map(i64::from))
                .unwrap_or(if p.absolute.is_some() || p.date.is_some() { 1 } else { 0 });
            let episode = nfo
                .and_then(|n| n.episode)
                .or(p.episode.map(i64::from))
                .or(p.absolute.map(i64::from))
                .or_else(|| p.date.as_ref().map(|d| d.replace('-', "").parse::<i64>().unwrap_or(0)))
                .unwrap_or(0);
            let title = nfo.and_then(|n| n.title.clone()).or_else(|| {
                // "Show - S01E02 - Episode Title" → the part after the numbering.
                st.split(" - ").nth(2).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
            });
            Placement::Episode { show, show_year, season, episode, episode_end: p.episode_end.map(i64::from), title }
        }
        LibraryKind::Music => {
            let t = tags.cloned().unwrap_or_default();
            let (disc_n, track_n, title_n) = parse::track_name(&st);
            let album_dir = folder_name(f);
            let artist_dir = f
                .parent()
                .and_then(|d| d.parent())
                .filter(|d| *d != root_of(roots, f))
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .map(str::to_string);
            let artist = t
                .album_artist
                .clone()
                .or_else(|| if t.compilation { Some("Various Artists".into()) } else { None })
                .or(t.artist.clone())
                .or(artist_dir)
                .unwrap_or_else(|| "Unknown artist".into());
            let album = t
                .album
                .clone()
                .unwrap_or_else(|| if album_dir.is_empty() { "Unknown album".into() } else { album_dir });
            Placement::Track {
                artist,
                album,
                album_year: t.year.map(|y| y as i32),
                disc: t.disc.map(i64::from).or(disc_n.map(i64::from)),
                track: t.track.map(i64::from).or(track_n.map(i64::from)),
                title: t.title.clone().unwrap_or(title_n),
            }
        }
        LibraryKind::Audiobooks => {
            // A folder is a book; a single file in the root is a book of its own.
            let in_root = f.parent() == Some(root_of(roots, f));
            let title = if in_root { st } else { folder_name(f) };
            Placement::Book { title, author: tags.and_then(|t| t.album_artist.clone().or(t.artist.clone())) }
        }
        LibraryKind::Videos => Placement::Video { title: st, folder: folder_name(f) },
        LibraryKind::Photos => Placement::Photo { title: st, folder: folder_name(f) },
        LibraryKind::Podcasts => {
            // "<root>/<Podcast>/2026-10-01 Episode title.mp3": the date orders the episodes.
            let root = root_of(roots, f);
            let show = parse::show_folder(root, f).map(str::to_string).unwrap_or_else(|| folder_name(f));
            let (episode, title) = podcast_episode(&st);
            Placement::Episode { show, show_year: None, season: 1, episode, episode_end: None, title: Some(title) }
        }
    }
}

/// "2026-10-01 Title" → (20261001, "Title"); undated names keep their whole stem.
fn podcast_episode(stem: &str) -> (i64, String) {
    let b = stem.as_bytes();
    let dated = b.len() >= 10
        && b[..10].iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() });
    if dated {
        let n = stem[..10].replace('-', "").parse().unwrap_or(0);
        let rest = stem[10..].trim_start_matches([' ', '-', '_', '.']).trim();
        (n, if rest.is_empty() { stem.to_string() } else { rest.to_string() })
    } else {
        (0, stem.to_string())
    }
}

/// Local artwork next to a file or in its folders.
fn local_art(kind: LibraryKind, roots: &[PathBuf], f: &Path, cx: Context) -> Vec<(ArtTarget, &'static str, PathBuf)> {
    let dir = f.parent().unwrap_or(Path::new("/"));
    let st = stem(f);
    let first =
        |d: &Path, names: &[String]| -> Option<PathBuf> { names.iter().map(|n| d.join(n)).find(|p| p.is_file()) };
    let imgs = |base: &[&str]| -> Vec<String> {
        base.iter().flat_map(|b| ["jpg", "jpeg", "png", "webp"].map(|e| format!("{b}.{e}"))).collect()
    };
    let mut out = Vec::new();
    match kind {
        LibraryKind::Movies => {
            let sp = format!("{st}-poster");
            let sf = format!("{st}-fanart");
            let sl = format!("{st}-logo");
            // Folder-wide images only for the folder's one movie.
            let (posters, backdrops, logos): (Vec<&str>, Vec<&str>, Vec<&str>) = if cx.alone {
                (
                    vec![&sp, "poster", "folder", "cover", "movie"],
                    vec![&sf, "fanart", "backdrop", "background"],
                    vec![&sl, "logo", "clearlogo"],
                )
            } else {
                (vec![&sp], vec![&sf], vec![&sl])
            };
            if let Some(p) = first(dir, &imgs(&posters)) {
                out.push((ArtTarget::Main, "poster", p));
            }
            if let Some(p) = first(dir, &imgs(&backdrops)) {
                out.push((ArtTarget::Main, "backdrop", p));
            }
            if let Some(p) = first(dir, &imgs(&logos)) {
                out.push((ArtTarget::Main, "logo", p));
            }
        }
        LibraryKind::Shows => {
            let root = root_of(roots, f);
            if let Some(show) = parse::show_home(root, f).map(|s| root.join(s)) {
                if let Some(p) = first(&show, &imgs(&["poster", "folder", "cover"])) {
                    out.push((ArtTarget::Show, "poster", p));
                }
                if let Some(p) = first(&show, &imgs(&["fanart", "backdrop", "background"])) {
                    out.push((ArtTarget::Show, "backdrop", p));
                }
                if let Some(p) = first(&show, &imgs(&["logo", "clearlogo"])) {
                    out.push((ArtTarget::Show, "logo", p));
                }
                if let Some(n) = parse::season_folder(&folder_name(f)) {
                    let named = [format!("season{n:02}-poster"), format!("season{n}-poster")];
                    let named: Vec<&str> = named.iter().map(String::as_str).collect();
                    if let Some(p) = first(&show, &imgs(&named))
                        .or_else(|| (dir != show).then(|| first(dir, &imgs(&["poster", "folder"]))).flatten())
                    {
                        out.push((ArtTarget::Season, "poster", p));
                    }
                }
            }
            if let Some(p) = first(dir, &imgs(&[&format!("{st}-thumb"), &st])) {
                out.push((ArtTarget::Main, "thumb", p));
            }
        }
        LibraryKind::Music => {
            if let Some(p) = first(dir, &imgs(&["cover", "folder", "front", "album", "albumart"])) {
                out.push((ArtTarget::Album, "poster", p));
            }
            if let Some(artist) = dir.parent()
                && artist != root_of(roots, f)
                && let Some(p) = first(artist, &imgs(&["artist", "folder", "poster"]))
            {
                out.push((ArtTarget::Artist, "poster", p));
            }
        }
        LibraryKind::Audiobooks => {
            if let Some(p) = first(dir, &imgs(&["cover", "folder", "front"])) {
                out.push((ArtTarget::Main, "poster", p));
            }
        }
        LibraryKind::Videos => {
            if let Some(p) = first(dir, &imgs(&[&format!("{st}-thumb"), &st])) {
                out.push((ArtTarget::Main, "thumb", p));
            }
        }
        LibraryKind::Photos => out.push((ArtTarget::Main, "thumb", f.to_path_buf())),
        LibraryKind::Podcasts => {
            let root = root_of(roots, f);
            if let Some(show) = parse::show_folder(root, f).map(|s| root.join(s))
                && let Some(p) = first(&show, &imgs(&["cover", "poster", "folder"]))
            {
                out.push((ArtTarget::Show, "poster", p));
            }
        }
    }
    out
}

/// `Movie.en.forced.srt`, `Movie.tr.srt`, `Subs/…`: subtitles that belong to a video file.
fn external_subs(f: &Path) -> Vec<(PathBuf, Option<String>, bool)> {
    let dir = f.parent().unwrap_or(Path::new("/"));
    let st = stem(f).to_lowercase();
    let mut out = Vec::new();
    for d in [dir.to_path_buf(), dir.join("Subs"), dir.join("subs"), dir.join("Subtitles")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if parse::file_kind(&p) != FileKind::Subtitle || p.extension().is_some_and(|x| x == "idx") {
                continue;
            }
            let s = stem(&p).to_lowercase();
            let in_subdir = d != dir;
            if !(s == st || s.starts_with(&format!("{st}.")) || in_subdir) {
                continue;
            }
            let rest: Vec<&str> = s.strip_prefix(&st).unwrap_or(&s).split('.').filter(|x| !x.is_empty()).collect();
            let forced = rest.contains(&"forced");
            let lang = rest
                .iter()
                .find(|x| (2..=3).contains(&x.len()) && x.chars().all(|c| c.is_ascii_alphabetic()))
                .map(|x| x.to_string());
            out.push((p, lang, forced));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A `.nfo` someone else wrote. Ours only repeat what the library already has, and reading them
/// back would keep a wrong match forever (its title would place the file).
fn read_nfo(p: &Path) -> Option<Nfo> {
    let text = std::fs::read(p).ok()?;
    let text = String::from_utf8_lossy(&text);
    if text.contains(crate::nfo::MARK) {
        return None;
    }
    crate::nfo::parse(&text)
}

/// The .nfo for a file, and for its container (tvshow.nfo).
fn nfos(kind: LibraryKind, roots: &[PathBuf], f: &Path, cx: Context) -> (Option<Nfo>, Option<Nfo>) {
    let dir = f.parent().unwrap_or(Path::new("/"));
    let own = read_nfo(&f.with_extension("nfo"));
    match kind {
        // `movie.nfo` describes the folder's one movie, not each of several.
        LibraryKind::Movies => {
            (own.or_else(|| (cx.alone || cx.legacy).then(|| read_nfo(&dir.join("movie.nfo"))).flatten()), None)
        }
        LibraryKind::Shows => {
            let root = root_of(roots, f);
            let home =
                if cx.legacy { parse::show_folder(root, f).map(PathBuf::from) } else { parse::show_home(root, f) };
            let show = home.and_then(|s| read_nfo(&root.join(s).join("tvshow.nfo")));
            (own, show)
        }
        _ => (None, None),
    }
}

// ------------------------------------------------------------------ database

fn find_or_create(
    t: &Transaction,
    library_id: i64,
    kind: ItemKind,
    parent: Option<i64>,
    title: &str,
    year: Option<i32>,
    index: Option<i64>,
    now: i64,
) -> rusqlite::Result<i64> {
    let sort = parse::sort_title(title);
    // Containers with an index (seasons) match on it; everything else on the title the scanner
    // gave it (`placed`: a provider may have renamed it since) or its current one.
    let existing: Option<i64> = if kind == ItemKind::Season {
        t.query_row(
            "SELECT id FROM items WHERE library_id = ?1 AND kind = ?2 AND parent_id IS ?3 AND index_no IS ?4",
            params![library_id, kind.as_str(), parent, index],
            |r| r.get(0),
        )
        .optional()?
    } else {
        // Two indexed lookups (items_library_kind, items_placed) rather than an OR over both.
        let mut st = t.prepare_cached(
            "SELECT id, year FROM items WHERE library_id = ?1 AND kind = ?2 AND sort_title = ?4 AND parent_id IS ?3 \
             UNION SELECT id, year FROM items WHERE library_id = ?1 AND kind = ?2 \
               AND json_extract(extra, '$.placed') = ?4 AND parent_id IS ?3 ORDER BY id",
        )?;
        let same: Vec<(i64, Option<i32>)> = st
            .query_map(params![library_id, kind.as_str(), parent, sort], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        same_item(&same, year)
    };
    if let Some(id) = existing {
        if year.is_some() {
            t.execute("UPDATE items SET year = coalesce(year, ?1) WHERE id = ?2", params![year, id])?;
        }
        if kind != ItemKind::Season {
            t.execute(
                "UPDATE items SET extra = json_set(extra, '$.placed', ?1) \
                 WHERE id = ?2 AND json_extract(extra, '$.placed') IS NULL",
                params![sort, id],
            )?;
        }
        return Ok(id);
    }
    // Made by the current rules: a repair has nothing to look at in it.
    let extra = if kind == ItemKind::Season {
        "{}".to_string()
    } else {
        serde_json::json!({ "placed": sort, "placement": PLACEMENT_VERSION }).to_string()
    };
    t.execute(
        "INSERT INTO items(library_id, kind, parent_id, title, sort_title, year, index_no, added_at, updated_at, extra) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
        params![library_id, kind.as_str(), parent, title, sort, year, index, now, extra],
    )?;
    Ok(t.last_insert_rowid())
}

/// Which of the items with the same title, as (id, year), a file with `year` belongs to: the one
/// with that year, else the only one if it has no year. Without a year: the only one, else the
/// only one without a year. Never a guess between remakes ("Dune" 1984 and 2021).
fn same_item(same: &[(i64, Option<i32>)], year: Option<i32>) -> Option<i64> {
    let yearless: Vec<i64> = same.iter().filter(|(_, y)| y.is_none()).map(|(id, _)| *id).collect();
    match year {
        Some(y) => same
            .iter()
            .find(|(_, iy)| *iy == Some(y))
            .map(|(id, _)| *id)
            .or_else(|| (same.len() == 1 && yearless.len() == 1).then(|| yearless[0])),
        None => match (same, yearless.as_slice()) {
            ([(id, _)], _) => Some(*id),
            (_, [id]) => Some(*id),
            _ => None,
        },
    }
}

/// Extras near a newly placed movie file that found no movie when they were scanned (a scan
/// can see them first, or the movie came later): attached now.
fn adopt_extras(t: &Transaction, library_id: i64, movie_file: &Path) -> rusqlite::Result<()> {
    let Some(dir) = parse::movie_dir(movie_file) else { return Ok(()) };
    let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
    // Every path under the folder sorts between the prefix and the prefix with its separator
    // raised by one (an index range on files.path, not a scan).
    let mut end = prefix.clone();
    end.pop();
    end.push(char::from(std::path::MAIN_SEPARATOR as u8 + 1));
    let mut st = t.prepare_cached(
        "SELECT f.id, f.path FROM files f WHERE f.path >= ?2 AND f.path < ?3 AND f.library_id = ?1 \
         AND f.missing_since IS NULL AND NOT EXISTS (SELECT 1 FROM item_files x WHERE x.file_id = f.id)",
    )?;
    let loose: Vec<(i64, String)> = st
        .query_map(params![library_id, prefix, end], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (file, path) in loose {
        let path = PathBuf::from(path);
        if let Some(role) = parse::is_extra(&path)
            && let Some(movie) = movie_of_extra(t, library_id, &path)?
        {
            link(t, movie, file, role)?;
        }
    }
    Ok(())
}

/// The movie an extra (trailer, sample, featurette) belongs to: the one whose main file is in
/// the extra's folder (or the folder above `Trailers/`, `Extras/`…). Several there: the one
/// whose file name starts the extra's ("Movie-trailer.mkv" → "Movie.mkv"); else none.
fn movie_of_extra(t: &Transaction, library_id: i64, path: &Path) -> rusqlite::Result<Option<i64>> {
    let Some(mut dir) = path.parent() else { return Ok(None) };
    while dir.file_name().and_then(|n| n.to_str()).is_some_and(parse::is_extras_folder)
        && let Some(up) = dir.parent()
    {
        dir = up;
    }
    let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
    let mut st = t.prepare_cached(
        "SELECT i.id, x.path FROM items i JOIN item_files f ON f.item_id = i.id AND f.role = 'main' \
         JOIN files x ON x.id = f.file_id \
         WHERE i.library_id = ?1 AND i.kind = 'movie' AND substr(x.path, 1, length(?2)) = ?2 \
         AND instr(substr(x.path, length(?2) + 1), '/') = 0 AND instr(substr(x.path, length(?2) + 1), '\\') = 0 \
         ORDER BY i.id",
    )?;
    let mains: Vec<(i64, String)> =
        st.query_map(params![library_id, prefix], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut items: Vec<i64> = mains.iter().map(|(id, _)| *id).collect();
    items.dedup();
    if let [one] = items.as_slice() {
        return Ok(Some(*one));
    }
    let extra = stem(path).to_lowercase();
    Ok(mains
        .iter()
        .map(|(id, p)| (*id, stem(Path::new(p)).to_lowercase()))
        .filter(|(_, s)| !s.is_empty() && extra.starts_with(s.as_str()))
        .max_by_key(|(_, s)| s.len())
        .map(|(id, _)| id))
}

fn locked(t: &Transaction, item: i64) -> rusqlite::Result<HashSet<String>> {
    let s: String = t.query_row("SELECT locked_fields FROM items WHERE id = ?1", [item], |r| r.get(0))?;
    Ok(s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
}

fn apply_nfo(t: &Transaction, item: i64, n: &Nfo, now: i64) -> rusqlite::Result<()> {
    let lock = locked(t, item)?;
    let set = |field: &str, sql: &str, v: &dyn rusqlite::ToSql| -> rusqlite::Result<()> {
        if !lock.contains(field) {
            t.execute(sql, params![v, item])?;
        }
        Ok(())
    };
    if let Some(v) = &n.title {
        set("title", "UPDATE items SET title = ?1 WHERE id = ?2", v)?;
        let sort = n.sort_title.clone().unwrap_or_else(|| parse::sort_title(v));
        set("title", "UPDATE items SET sort_title = ?1 WHERE id = ?2", &sort)?;
    }
    if let Some(v) = &n.original_title {
        set("original_title", "UPDATE items SET original_title = ?1 WHERE id = ?2", v)?;
    }
    if let Some(v) = n.year {
        set("year", "UPDATE items SET year = ?1 WHERE id = ?2", &v)?;
    }
    if let Some(v) = &n.plot {
        set("overview", "UPDATE items SET overview = ?1 WHERE id = ?2", v)?;
    }
    if let Some(v) = n.rating {
        set("rating", "UPDATE items SET rating = ?1 WHERE id = ?2", &v)?;
    }
    if let Some(v) = &n.mpaa {
        set("official_rating", "UPDATE items SET official_rating = ?1 WHERE id = ?2", v)?;
    }
    if let Some(v) = &n.premiered {
        set("premiered", "UPDATE items SET premiered = ?1 WHERE id = ?2", v)?;
    }
    if !n.ids.is_empty() {
        let cur: String = t.query_row("SELECT provider_ids FROM items WHERE id = ?1", [item], |r| r.get(0))?;
        let mut ids: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&cur).unwrap_or_default();
        for (k, v) in &n.ids {
            ids.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        t.execute(
            "UPDATE items SET provider_ids = ?1 WHERE id = ?2",
            params![serde_json::Value::Object(ids).to_string(), item],
        )?;
    }
    if !lock.contains("genres") && !n.genres.is_empty() {
        set_genres(t, item, &n.genres)?;
    }
    if !lock.contains("people") {
        let mut people: Vec<(String, &str, Option<String>)> = Vec::new();
        people.extend(n.actors.iter().map(|(name, role)| (name.clone(), "actor", role.clone())));
        people.extend(n.directors.iter().map(|d| (d.clone(), "director", None)));
        people.extend(n.writers.iter().map(|d| (d.clone(), "writer", None)));
        if !people.is_empty() {
            set_people(t, item, &people)?;
        }
    }
    t.execute("UPDATE items SET updated_at = ?1 WHERE id = ?2", params![now, item])?;
    Ok(())
}

fn set_genres(t: &Transaction, item: i64, genres: &[String]) -> rusqlite::Result<()> {
    t.execute("DELETE FROM item_genres WHERE item_id = ?1", [item])?;
    for g in genres.iter().take(20) {
        t.execute("INSERT OR IGNORE INTO genres(name) VALUES (?1)", [g])?;
        let gid: i64 = t.query_row("SELECT id FROM genres WHERE name = ?1", [g], |r| r.get(0))?;
        t.execute("INSERT OR IGNORE INTO item_genres(item_id, genre_id) VALUES (?1, ?2)", params![item, gid])?;
    }
    Ok(())
}

fn set_people(t: &Transaction, item: i64, people: &[(String, &str, Option<String>)]) -> rusqlite::Result<()> {
    t.execute("DELETE FROM item_people WHERE item_id = ?1", [item])?;
    for (ord, (name, role, character)) in people.iter().take(60).enumerate() {
        t.execute("INSERT OR IGNORE INTO people(name) VALUES (?1)", [name])?;
        let pid: i64 = t.query_row("SELECT id FROM people WHERE name = ?1", [name], |r| r.get(0))?;
        t.execute(
            "INSERT OR IGNORE INTO item_people(item_id, person_id, role, character, ord) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![item, pid, role, character, ord as i64],
        )?;
    }
    Ok(())
}

fn set_image(t: &Transaction, owner: i64, kind: &str, s: &artwork::Stored, source: &str) -> rusqlite::Result<()> {
    t.execute(
        "INSERT INTO images(owner_kind, owner_id, kind, sha256, width, height, blurhash, source) \
         VALUES ('item', ?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(owner_kind, owner_id, kind) DO UPDATE SET sha256 = excluded.sha256, width = excluded.width, \
         height = excluded.height, blurhash = excluded.blurhash, source = excluded.source \
         WHERE images.locked = 0",
        params![owner, kind, s.sha, s.width, s.height, s.blurhash, source],
    )?;
    Ok(())
}

/// Writes a file row with its streams and chapters; returns the file id.
fn upsert_file(t: &Transaction, library_id: i64, a: &Analyzed) -> rusqlite::Result<i64> {
    let path = a.path.display().to_string();
    let probe = a.probe.as_ref().map(|p| serde_json::to_string(p).unwrap_or_default());
    let now = now_unix();
    t.execute(
        "INSERT INTO files(library_id, path, size, mtime, quick_hash, probe, probed_at, missing_since) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL) \
         ON CONFLICT(path) DO UPDATE SET library_id = excluded.library_id, size = excluded.size, mtime = excluded.mtime, \
         quick_hash = excluded.quick_hash, probe = coalesce(excluded.probe, probe), probed_at = coalesce(excluded.probed_at, probed_at), \
         missing_since = NULL",
        params![library_id, path, a.size as i64, a.mtime, a.quick_hash, probe, a.probe.as_ref().map(|_| now)],
    )?;
    let file_id: i64 = t.query_row("SELECT id FROM files WHERE path = ?1", [&path], |r| r.get(0))?;
    if let Some(p) = &a.probe {
        t.execute("DELETE FROM streams WHERE file_id = ?1", [file_id])?;
        t.execute("DELETE FROM chapters WHERE file_id = ?1", [file_id])?;
        for tr in &p.tracks {
            let kind = match tr.kind {
                Some(TrackKind::Video) if tr.albumart => continue,
                Some(TrackKind::Video) => "video",
                Some(TrackKind::Audio) => "audio",
                Some(TrackKind::Sub) => "subtitle",
                None => continue,
            };
            t.execute(
                "INSERT OR REPLACE INTO streams(file_id, idx, kind, codec, profile, lang, title, is_default, forced, width, height, fps, \
                 bitrate, channels, sample_rate, external_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL)",
                params![
                    file_id, tr.id, kind, tr.codec, tr.codec_profile, tr.lang, tr.title, tr.default, tr.forced, tr.width, tr.height,
                    tr.fps, tr.bitrate, tr.channels, tr.sample_rate
                ],
            )?;
        }
        for (i, c) in p.chapters.iter().enumerate() {
            t.execute(
                "INSERT OR REPLACE INTO chapters(file_id, idx, start_ms, title) VALUES (?1, ?2, ?3, ?4)",
                params![file_id, i as i64, (c.start * 1000.0) as i64, c.title],
            )?;
        }
    }
    // External subtitles: numbered after the embedded ones.
    t.execute("DELETE FROM streams WHERE file_id = ?1 AND external_path IS NOT NULL", [file_id])?;
    for (i, (p, lang, forced)) in a.external_subs.iter().enumerate() {
        let codec = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        t.execute(
            "INSERT OR REPLACE INTO streams(file_id, idx, kind, codec, lang, forced, external_path) VALUES (?1, ?2, 'subtitle', ?3, ?4, ?5, ?6)",
            params![file_id, 1000 + i as i64, codec, lang, forced, p.display().to_string()],
        )?;
    }
    Ok(file_id)
}

fn link(t: &Transaction, item: i64, file: i64, role: &str) -> rusqlite::Result<()> {
    // A file belongs to one item.
    t.execute("DELETE FROM item_files WHERE file_id = ?1 AND item_id != ?2", params![file, item])?;
    t.execute(
        "INSERT OR IGNORE INTO item_files(item_id, file_id, role) VALUES (?1, ?2, ?3)",
        params![item, file, role],
    )?;
    Ok(())
}

/// Places one analyzed file; returns the items whose search entries need refreshing.
fn apply(t: &Transaction, library_id: i64, a: &Analyzed) -> rusqlite::Result<Vec<i64>> {
    let now = now_unix();
    let file_id = upsert_file(t, library_id, a)?;
    let runtime = a.probe.as_ref().and_then(|p| p.duration).map(|d| (d * 1000.0) as i64).or(a
        .tags
        .as_ref()
        .and_then(|t| t.duration_ms)
        .map(|d| d as i64));
    let mut touched = Vec::new();
    let mut targets: HashMap<ArtTarget, i64> = HashMap::new();
    match &a.placement {
        Placement::Movie { title, year, role } => {
            // Extras attach to their movie, never to a movie of their own or a guessed one.
            let item = if *role != "main" {
                match movie_of_extra(t, library_id, &a.path)? {
                    Some(i) => i,
                    None => {
                        t.execute("DELETE FROM item_files WHERE file_id = ?1", [file_id])?;
                        return Ok(touched);
                    }
                }
            } else {
                find_or_create(t, library_id, ItemKind::Movie, None, title, *year, None, now)?
            };
            link(t, item, file_id, role)?;
            if *role == "main" {
                adopt_extras(t, library_id, &a.path)?;
            }
            if *role == "main" {
                t.execute(
                    "UPDATE items SET runtime_ms = coalesce(?1, runtime_ms) WHERE id = ?2",
                    params![runtime, item],
                )?;
                if let Some(n) = &a.nfo {
                    apply_nfo(t, item, n, now)?;
                }
            }
            targets.insert(ArtTarget::Main, item);
            touched.push(item);
        }
        Placement::Episode { show, show_year, season, episode, episode_end, title } => {
            let show_id = find_or_create(t, library_id, ItemKind::Show, None, show, *show_year, None, now)?;
            if let Some(n) = &a.parent_nfo {
                apply_nfo(t, show_id, n, now)?;
            }
            let season_title = if *season == 0 { "Specials".to_string() } else { format!("Season {season}") };
            let season_id = find_or_create(
                t,
                library_id,
                ItemKind::Season,
                Some(show_id),
                &season_title,
                None,
                Some(*season),
                now,
            )?;
            let ep_title = title.clone().unwrap_or_else(|| format!("Episode {episode}"));
            let ep: Option<i64> = t
                .query_row(
                    "SELECT id FROM items WHERE parent_id = ?1 AND kind = 'episode' AND index_no = ?2",
                    params![season_id, episode],
                    |r| r.get(0),
                )
                .optional()?;
            let ep = match ep {
                Some(id) => id,
                None => {
                    t.execute(
                        "INSERT INTO items(library_id, kind, parent_id, title, sort_title, index_no, index2, added_at, updated_at) \
                         VALUES (?1, 'episode', ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                        params![library_id, season_id, ep_title, format!("{episode:05}"), episode, episode_end, now],
                    )?;
                    t.last_insert_rowid()
                }
            };
            t.execute(
                "UPDATE items SET runtime_ms = coalesce(?1, runtime_ms), index2 = coalesce(?2, index2) WHERE id = ?3",
                params![runtime, episode_end, ep],
            )?;
            // Re-placed by the repair: the title a wrong show's provider gave it goes.
            if a.retitle && !locked(t, ep)?.contains("title") {
                t.execute("UPDATE items SET title = ?1 WHERE id = ?2", params![ep_title, ep])?;
            }
            if let Some(n) = &a.nfo {
                apply_nfo(t, ep, n, now)?;
            }
            link(t, ep, file_id, "main")?;
            targets.insert(ArtTarget::Main, ep);
            targets.insert(ArtTarget::Show, show_id);
            targets.insert(ArtTarget::Season, season_id);
            touched.extend([ep, show_id]);
        }
        Placement::Track { artist, album, album_year, disc, track, title } => {
            let artist_id = find_or_create(t, library_id, ItemKind::Artist, None, artist, None, None, now)?;
            let album_id =
                find_or_create(t, library_id, ItemKind::Album, Some(artist_id), album, *album_year, None, now)?;
            let existing: Option<i64> =
                t.query_row("SELECT item_id FROM item_files WHERE file_id = ?1", [file_id], |r| r.get(0)).optional()?;
            let tr = match existing {
                Some(id) => {
                    t.execute(
                        "UPDATE items SET parent_id = ?1, title = ?2, sort_title = ?3, index_no = ?4, index2 = ?5, updated_at = ?6 WHERE id = ?7",
                        params![album_id, title, format!("{:03}{:04}", disc.unwrap_or(1), track.unwrap_or(0)), track, disc, now, id],
                    )?;
                    id
                }
                None => {
                    t.execute(
                        "INSERT INTO items(library_id, kind, parent_id, title, sort_title, index_no, index2, added_at, updated_at) \
                         VALUES (?1, 'track', ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                        params![library_id, album_id, title, format!("{:03}{:04}", disc.unwrap_or(1), track.unwrap_or(0)), track, disc, now],
                    )?;
                    t.last_insert_rowid()
                }
            };
            t.execute("UPDATE items SET runtime_ms = ?1 WHERE id = ?2", params![runtime, tr])?;
            if let Some(tags) = &a.tags {
                let extra = serde_json::json!({
                    "artist": tags.artist, "album_artist": tags.album_artist, "composer": tags.composer, "work": tags.work,
                    "rg_track_gain": tags.rg_track_gain, "rg_album_gain": tags.rg_album_gain,
                    "sample_rate": tags.sample_rate, "channels": tags.channels, "bitrate_kbps": tags.bitrate_kbps,
                });
                t.execute("UPDATE items SET extra = ?1 WHERE id = ?2", params![extra.to_string(), tr])?;
                let mut ids = serde_json::Map::new();
                if let Some(v) = &tags.mb_track {
                    ids.insert("musicbrainz".into(), v.clone().into());
                }
                t.execute(
                    "UPDATE items SET provider_ids = ?1 WHERE id = ?2",
                    params![serde_json::Value::Object(ids).to_string(), tr],
                )?;
                if let Some(v) = &tags.mb_album {
                    t.execute(
                        "UPDATE items SET provider_ids = json_set(provider_ids, '$.musicbrainz', ?1) WHERE id = ?2",
                        params![v, album_id],
                    )?;
                }
                if !tags.genres.is_empty() {
                    set_genres(t, tr, &tags.genres)?;
                    set_genres(t, album_id, &tags.genres)?;
                }
                if let Some(a) = &tags.artist {
                    set_people(t, tr, &[(a.clone(), "artist", None)])?;
                }
            }
            link(t, tr, file_id, "main")?;
            targets.insert(ArtTarget::Main, album_id);
            targets.insert(ArtTarget::Album, album_id);
            targets.insert(ArtTarget::Artist, artist_id);
            touched.extend([tr, album_id, artist_id]);
        }
        Placement::Book { title, author } => {
            let book = find_or_create(t, library_id, ItemKind::Book, None, title, None, None, now)?;
            if let Some(a) = author {
                set_people(t, book, &[(a.clone(), "author", None)])?;
            }
            link(t, book, file_id, "main")?;
            let total: Option<i64> = t.query_row(
                "SELECT sum(json_extract(f.probe, '$.duration') * 1000) FROM item_files x JOIN files f ON f.id = x.file_id WHERE x.item_id = ?1",
                [book],
                |r| r.get(0),
            )?;
            t.execute("UPDATE items SET runtime_ms = ?1 WHERE id = ?2", params![total, book])?;
            targets.insert(ArtTarget::Main, book);
            touched.push(book);
        }
        Placement::Video { title, folder } | Placement::Photo { title, folder } => {
            let kind = if matches!(a.placement, Placement::Video { .. }) { ItemKind::Video } else { ItemKind::Photo };
            let existing: Option<i64> =
                t.query_row("SELECT item_id FROM item_files WHERE file_id = ?1", [file_id], |r| r.get(0)).optional()?;
            let item = match existing {
                Some(id) => id,
                None => {
                    t.execute(
                        "INSERT INTO items(library_id, kind, title, sort_title, added_at, updated_at, extra) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                        params![library_id, kind.as_str(), title, parse::sort_title(title), now, serde_json::json!({ "folder": folder }).to_string()],
                    )?;
                    t.last_insert_rowid()
                }
            };
            t.execute("UPDATE items SET runtime_ms = ?1 WHERE id = ?2", params![runtime, item])?;
            link(t, item, file_id, "main")?;
            targets.insert(ArtTarget::Main, item);
            touched.push(item);
        }
    }
    for (target, kind, stored, source) in &a.art {
        if let Some(owner) = targets.get(target) {
            set_image(t, *owner, kind, stored, source)?;
        }
    }
    Ok(touched)
}

/// Title, original title, overview and people of an item, for its search entry.
type FtsRow = (String, Option<String>, Option<String>, Option<String>);

/// Rebuilds the search entries of `items`.
pub(crate) fn refresh_fts(t: &Transaction, items: &[i64]) -> rusqlite::Result<()> {
    for id in items {
        t.execute("DELETE FROM items_fts WHERE rowid = ?1", [id])?;
        let row: Option<FtsRow> = t
            .query_row(
                "SELECT i.title, i.original_title, i.overview, \
                 (SELECT group_concat(p.name, ' ') FROM item_people ip JOIN people p ON p.id = ip.person_id WHERE ip.item_id = i.id) \
                 FROM items i WHERE i.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((title, original, overview, people)) = row {
            t.execute(
                "INSERT INTO items_fts(rowid, title, original_title, people, overview) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    title,
                    original.unwrap_or_default(),
                    people.unwrap_or_default(),
                    overview.unwrap_or_default()
                ],
            )?;
        }
    }
    Ok(())
}

/// Deletes items that no longer have files, and containers left empty.
fn prune(t: &Transaction, library_id: i64) -> rusqlite::Result<usize> {
    let mut removed = t.execute(
        "DELETE FROM items WHERE library_id = ?1 AND kind IN ('movie','episode','track','book','video','photo') \
         AND NOT EXISTS (SELECT 1 FROM item_files f WHERE f.item_id = items.id)",
        [library_id],
    )?;
    // Seasons, then shows/albums, then artists.
    for _ in 0..3 {
        removed += t.execute(
            "DELETE FROM items WHERE library_id = ?1 AND kind IN ('show','season','album','artist') \
             AND NOT EXISTS (SELECT 1 FROM items c WHERE c.parent_id = items.id)",
            [library_id],
        )?;
    }
    t.execute("DELETE FROM items_fts WHERE rowid NOT IN (SELECT id FROM items)", [])?;
    t.execute("DELETE FROM images WHERE owner_kind = 'item' AND owner_id NOT IN (SELECT id FROM items)", [])?;
    Ok(removed)
}

// ------------------------------------------------------------------ the scan

struct Known {
    id: i64,
    size: i64,
    mtime: i64,
    quick_hash: String,
    missing_since: Option<i64>,
}

pub(crate) async fn scan(lib: &Library, library_id: i64, cancel: &AtomicBool) -> Result<()> {
    let (kind, roots, _settings) = lib.library_row(library_id).await?;
    let mut status = ScanStatus { library_id, running: true, phase: "walk".into(), ..Default::default() };
    lib.set_status(status.clone());

    // 1. Walk every folder (offline ones are remembered, not emptied).
    let r2 = roots.clone();
    let (found, offline) = tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        let mut offline = Vec::new();
        for r in &r2 {
            match walk(r, kind) {
                Ok(mut f) => found.append(&mut f),
                Err(e) => {
                    tracing::warn!("library folder {} is unavailable: {e}", r.display());
                    offline.push(r.clone());
                }
            }
        }
        (found, offline)
    })
    .await
    .map_err(|e| LibError::Db(e.to_string()))?;

    // 2. What the database knows.
    let known: HashMap<String, Known> = lib
        .db()
        .read(move |c| {
            let mut st =
                c.prepare("SELECT id, path, size, mtime, quick_hash, missing_since FROM files WHERE library_id = ?1")?;
            let rows = st.query_map([library_id], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    Known {
                        id: r.get(0)?,
                        size: r.get(2)?,
                        mtime: r.get(3)?,
                        quick_hash: r.get(4)?,
                        missing_since: r.get(5)?,
                    },
                ))
            })?;
            rows.collect()
        })
        .await?;

    let seen: HashSet<String> = found.iter().map(|f| f.path.display().to_string()).collect();
    let per_folder = videos_per_folder(found.iter().map(|f| f.path.as_path()));

    // 2b. Libraries placed by older rules: move what they put in the wrong item (once).
    let version: i64 = lib
        .db()
        .read(move |c| c.query_row("SELECT placement FROM libraries WHERE id = ?1", [library_id], |r| r.get(0)))
        .await?;
    let mut repaired = version >= PLACEMENT_VERSION;
    let mut touched_all: Vec<i64> = Vec::new();
    if !repaired {
        if matches!(kind, LibraryKind::Movies | LibraryKind::Shows) {
            status.phase = "repair".into();
            lib.set_status(status.clone());
            match repair(lib, library_id, kind, &roots, &per_folder, &seen, cancel, &mut status).await {
                Ok(t) => {
                    touched_all.extend(t);
                    repaired = offline.is_empty();
                }
                Err(e) => tracing::warn!("library {library_id}: repairing placements failed: {e}"),
            }
        } else {
            repaired = true;
        }
    }

    let mut todo: Vec<Found> = found
        .into_iter()
        .filter(|f| match known.get(&f.path.display().to_string()) {
            Some(k) => k.size != f.size as i64 || k.mtime != f.mtime || k.missing_since.is_some(),
            None => true,
        })
        .collect();
    // Main files first: extras look for their movie.
    todo.sort_by_key(|f| (parse::is_extra(&f.path).is_some(), f.path.clone()));
    // Gone: known, not seen, and not under an offline folder.
    let gone: Vec<(&String, &Known)> = known
        .iter()
        .filter(|(p, _)| !seen.contains(*p) && !offline.iter().any(|o| Path::new(p).starts_with(o)))
        .collect();

    status.phase = "read".into();
    status.total = todo.len() as u64;
    lib.set_status(status.clone());

    // 3. Moves: a new path whose quick hash matches a gone file takes over its row (keeps
    // watch state, artwork and metadata).
    let gone_by_hash: HashMap<(String, i64), i64> = gone
        .iter()
        .filter(|(_, k)| !k.quick_hash.is_empty())
        .map(|(_, k)| ((k.quick_hash.clone(), k.size), k.id))
        .collect();
    let mut moved_ids = HashSet::new();

    for (n, f) in todo.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            status.error = Some("cancelled".into());
            break;
        }
        let p2 = f.path.clone();
        let qh = tokio::task::spawn_blocking(move || quick_hash(&p2, f.size))
            .await
            .map_err(|e| LibError::Db(e.to_string()))?
            .unwrap_or_default();
        if !known.contains_key(&f.path.display().to_string())
            && let Some(old) = gone_by_hash.get(&(qh.clone(), f.size as i64)).copied()
            && moved_ids.insert(old)
        {
            let (path, mtime) = (f.path.display().to_string(), f.mtime);
            lib.db()
                .write(move |t| {
                    t.execute(
                        "UPDATE files SET path = ?1, mtime = ?2, missing_since = NULL WHERE id = ?3",
                        params![path, mtime, old],
                    )
                    .map(|_| ())
                })
                .await?;
            tracing::info!("library: {} was moved here", f.path.display());
            status.updated += 1;
            status.done = n as u64 + 1;
            continue;
        }
        let cx = context(&per_folder, &f.path);
        let analyzed = analyze(lib, kind, &roots, f, qh, cx, true).await;
        let is_new = !known.contains_key(&analyzed.path.display().to_string());
        let touched = lib.db().write(move |t| {
            let touched = apply(t, library_id, &analyzed)?;
            refresh_fts(t, &touched)?;
            Ok(touched)
        });
        match touched.await {
            Ok(tch) => {
                touched_all.extend(tch);
                if is_new {
                    status.added += 1;
                } else {
                    status.updated += 1;
                }
            }
            Err(e) => tracing::warn!("library: couldn't add a file: {e}"),
        }
        status.done = n as u64 + 1;
        if n % 25 == 0 {
            lib.set_status(status.clone());
        }
    }

    // 4. Missing files: mark now, remove after the grace period.
    status.phase = "cleanup".into();
    lib.set_status(status.clone());
    let now = now_unix();
    let gone_ids: Vec<(i64, Option<i64>)> =
        gone.iter().filter(|(_, k)| !moved_ids.contains(&k.id)).map(|(_, k)| (k.id, k.missing_since)).collect();
    let removed = lib
        .db()
        .write(move |t| {
            let mut removed = 0u64;
            for (id, since) in &gone_ids {
                match since {
                    None => {
                        t.execute("UPDATE files SET missing_since = ?1 WHERE id = ?2", params![now, id])?;
                    }
                    Some(s) if now - s > MISSING_GRACE_SECS => {
                        t.execute("DELETE FROM files WHERE id = ?1", [id])?;
                        removed += 1;
                    }
                    Some(_) => {}
                }
            }
            prune(t, library_id)?;
            t.execute("UPDATE libraries SET scanned_at = ?1 WHERE id = ?2", params![now, library_id])?;
            if repaired {
                t.execute("UPDATE libraries SET placement = ?1 WHERE id = ?2", params![PLACEMENT_VERSION, library_id])?;
            }
            Ok(removed)
        })
        .await?;
    status.removed = removed;
    status.running = false;
    status.phase = "done".into();
    lib.set_status(status);
    Ok(())
}

/// What a name-parser plugin makes of a file without an .nfo (movies, shows, videos).
async fn hook_nfo(lib: &Library, kind: LibraryKind, path: &Path) -> Option<Nfo> {
    if !matches!(kind, LibraryKind::Movies | LibraryKind::Shows | LibraryKind::Videos) {
        return None;
    }
    let hook = lib.name_hook()?;
    hook(stem(path)).await.map(|h| {
        let episodic = kind == LibraryKind::Shows;
        Nfo {
            kind: "plugin".into(),
            // For episodes the title an .nfo carries is the episode's: the show comes from the
            // folders.
            title: (!episodic && !h.title.is_empty()).then_some(h.title),
            year: h.year,
            season: h.season,
            episode: h.episode,
            ..Default::default()
        }
    })
}

/// Reads everything about one file (blocking work on the pool, probing in the worker; `probe`
/// false keeps what the file row has, for the repair).
async fn analyze(
    lib: &Library,
    kind: LibraryKind,
    roots: &[PathBuf],
    f: Found,
    quick_hash: String,
    cx: Context,
    probe: bool,
) -> Analyzed {
    let fk = parse::file_kind(&f.path);
    let probe = match fk {
        _ if !probe => None,
        FileKind::Video | FileKind::Audio => match lib.prober().probe(&f.path).await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::debug!("probe failed for {}: {e}", f.path.display());
                None
            }
        },
        _ => None,
    };
    let (path, roots2) = (f.path.clone(), roots.to_vec());
    let (tags, nfo, parent_nfo, art_files, subs) = tokio::task::spawn_blocking(move || {
        let tags = (fk == FileKind::Audio).then(|| crate::tags::read(&path).ok()).flatten();
        let (nfo, parent) = nfos(kind, &roots2, &path, cx);
        let art = local_art(kind, &roots2, &path, cx);
        let subs = if fk == FileKind::Video { external_subs(&path) } else { Vec::new() };
        (tags, nfo, parent, art, subs)
    })
    .await
    .unwrap_or_default();
    // No .nfo: a name-parser plugin may know the naming scheme better than the built-in parser.
    let nfo = match nfo {
        Some(n) => Some(n),
        None => hook_nfo(lib, kind, &f.path).await,
    };
    let placement = place(kind, roots, &f.path, tags.as_ref(), nfo.as_ref(), cx);

    // Artwork: local files, else the embedded cover of a track.
    let mut art = Vec::new();
    let _permit = lib.image_permit().await;
    let root = lib.artwork_root().to_path_buf();
    let cover = tags.as_ref().and_then(|t| t.cover.clone());
    let stored = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut has_album = false;
        for (target, k, p) in art_files {
            let Ok(data) = std::fs::read(&p) else { continue };
            match artwork::store(&root, &data) {
                Ok(s) => {
                    has_album |= target == ArtTarget::Album;
                    out.push((target, k, s, p.display().to_string()));
                }
                Err(e) => tracing::debug!("artwork {}: {e}", p.display()),
            }
        }
        if !has_album
            && let Some(c) = cover
            && let Ok(s) = artwork::store(&root, &c)
        {
            out.push((ArtTarget::Album, "poster", s, "embedded".to_string()));
        }
        out
    })
    .await
    .unwrap_or_default();
    art.extend(stored);
    Analyzed {
        path: f.path,
        size: f.size,
        mtime: f.mtime,
        quick_hash,
        probe,
        tags,
        nfo,
        parent_nfo,
        placement,
        art,
        external_subs: subs,
        retitle: false,
    }
}

// ------------------------------------------------------------------ repair

/// A file of the library as the repair sees it.
struct Row {
    file: i64,
    path: PathBuf,
    size: i64,
    mtime: i64,
    quick_hash: String,
    /// The item the file is in now (the episode, for a show).
    item: Option<i64>,
    /// The movie, or the show of the episode, the file is in now.
    group: Option<i64>,
    /// A provider matched the group (automatically or the user's pick).
    matched: bool,
    /// The group was looked at by a repair of this version already.
    done: bool,
}

/// A title and its year.
type Titled = (String, Option<i32>);

/// What a placement puts a file under: the movie or the show, by title (letters and digits
/// only) and year. Extras have none.
fn group_key(p: &Placement) -> Option<Titled> {
    match p {
        Placement::Movie { title, year, role: "main" } => Some((parse::norm_title(title), *year)),
        Placement::Episode { show, show_year, .. } => Some((parse::norm_title(show), *show_year)),
        _ => None,
    }
}

fn group_title(p: &Placement) -> Option<Titled> {
    match p {
        Placement::Movie { title, year, role: "main" } => Some((title.clone(), *year)),
        Placement::Episode { show, show_year, .. } => Some((show.clone(), *show_year)),
        _ => None,
    }
}

/// Why a file is placed again.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Redo {
    /// Its item held several movies or shows, or was named by older rules and never matched:
    /// the item is reset and every file of it placed again.
    Item,
    /// An extra: attached again to its movie (its movie keeps everything).
    Extra,
    /// Not in any item.
    Loose,
}

/// Moves the files earlier placement rules put in the wrong movie or show and repairs what that
/// did, without losing what is right:
///
/// - an item whose files the current rules put under different titles held several movies or
///   shows (a grouping folder, two shows under one folder, a shared `movie.nfo`): it is reset
///   (fields the user locked stay), keeps the files of its main title, and the others move to
///   items of their own, taking watch state, playlist places and following with them;
/// - an item the old rules named differently (`4K/Avatar.mkv` was "4K") is renamed, but only
///   if no provider matched it: a match, maybe the user's own pick, is never undone;
/// - extras are attached to their movie again (never making their movie "changed"), and a
///   folder's poster given to each of several movies is taken back from them.
///
/// Every movie and show looked at is marked, so running again (a folder was offline) changes
/// nothing. Returns the items to refresh in the index.
#[allow(clippy::too_many_arguments)]
async fn repair(
    lib: &Library,
    library_id: i64,
    kind: LibraryKind,
    roots: &[PathBuf],
    per_folder: &HashMap<PathBuf, usize>,
    on_disk: &HashSet<String>,
    cancel: &AtomicBool,
    status: &mut ScanStatus,
) -> Result<Vec<i64>> {
    let rows: Vec<Row> = lib
        .db()
        .read(move |c| {
            let mut st = c.prepare(
                "WITH r AS (SELECT f.id, f.path, f.size, f.mtime, f.quick_hash, x.item_id, \
                   CASE i.kind WHEN 'episode' THEN (SELECT s.parent_id FROM items s WHERE s.id = i.parent_id) \
                   ELSE i.id END AS grp \
                   FROM files f LEFT JOIN item_files x ON x.file_id = f.id LEFT JOIN items i ON i.id = x.item_id \
                   WHERE f.library_id = ?1 AND f.missing_since IS NULL) \
                 SELECT r.*, coalesce(json_extract(g.extra, '$.meta_provider'), '') != '', \
                   coalesce(json_extract(g.extra, '$.placement'), 0) >= ?2 \
                 FROM r LEFT JOIN items g ON g.id = r.grp ORDER BY r.path",
            )?;
            st.query_map(params![library_id, PLACEMENT_VERSION], |r| {
                Ok(Row {
                    file: r.get(0)?,
                    path: PathBuf::from(r.get::<_, String>(1)?),
                    size: r.get(2)?,
                    mtime: r.get(3)?,
                    quick_hash: r.get(4)?,
                    item: r.get(5)?,
                    group: r.get(6)?,
                    matched: r.get::<_, Option<bool>>(7)?.unwrap_or(false),
                    done: r.get::<_, Option<bool>>(8)?.unwrap_or(false),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    let rows: Vec<Row> = rows.into_iter().filter(|r| on_disk.contains(&r.path.display().to_string())).collect();
    let cancelled = || LibError::Invalid("cancelled".into());

    // Where the old and the current rules put each file.
    let legacy = Context { alone: true, legacy: true };
    let mut new_place: HashMap<i64, Placement> = HashMap::new();
    let mut renamed: HashSet<i64> = HashSet::new();
    // Folder images a movie got only because the old rules gave them to every movie there.
    let mut shared_art: Vec<(i64, Vec<String>)> = Vec::new();
    for r in &rows {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        let cx = context(per_folder, &r.path);
        let (path, roots2) = (r.path.clone(), roots.to_vec());
        let (nfo_new, nfo_old, art) = tokio::task::spawn_blocking(move || {
            let (n, _) = nfos(kind, &roots2, &path, cx);
            let (o, _) = nfos(kind, &roots2, &path, legacy);
            let now: HashSet<PathBuf> = local_art(kind, &roots2, &path, cx).into_iter().map(|(.., p)| p).collect();
            let art: Vec<String> = local_art(kind, &roots2, &path, legacy)
                .into_iter()
                .filter(|(.., p)| !now.contains(p))
                .map(|(.., p)| p.display().to_string())
                .collect();
            (n, o, art)
        })
        .await
        .map_err(|e| LibError::Db(e.to_string()))?;
        let hooked = if nfo_new.is_none() || nfo_old.is_none() { hook_nfo(lib, kind, &r.path).await } else { None };
        let new = place(kind, roots, &r.path, None, nfo_new.or(hooked.clone()).as_ref(), cx);
        let old = place(kind, roots, &r.path, None, nfo_old.or(hooked).as_ref(), legacy);
        if let Some(g) = r.group.filter(|_| !r.done) {
            if group_key(&new).is_some() && group_key(&new) != group_key(&old) && !r.matched {
                renamed.insert(g);
            }
            if !art.is_empty() {
                shared_art.push((g, art));
            }
        }
        new_place.insert(r.file, new);
    }

    let mut by_group: HashMap<i64, Vec<&Row>> = HashMap::new();
    for r in &rows {
        if let Some(g) = r.group {
            by_group.entry(g).or_default().push(r);
        }
    }
    // Items that held several movies or shows, and unmatched ones the old rules misnamed.
    let mut tainted: HashSet<i64> = renamed;
    for (g, files) in &by_group {
        let keys: HashSet<_> = files.iter().filter_map(|r| new_place.get(&r.file).and_then(group_key)).collect();
        if keys.len() > 1 {
            tainted.insert(*g);
        }
    }
    let mut redo: HashMap<i64, Redo> = HashMap::new();
    for r in &rows {
        let is_extra = matches!(new_place.get(&r.file), Some(Placement::Movie { role, .. }) if *role != "main");
        let why = match r.group {
            Some(g) if tainted.contains(&g) => Some(Redo::Item),
            _ if is_extra => Some(Redo::Extra),
            None => Some(Redo::Loose),
            Some(_) => None,
        };
        if let Some(w) = why {
            redo.insert(r.file, w);
        }
    }

    // The title most of an item's files give it: a reset item keeps it, so they find it again.
    let dominant = |files: &[&Row]| -> Option<Titled> {
        let mut n: HashMap<Titled, (usize, String)> = HashMap::new();
        for r in files {
            if let Some(p) = new_place.get(&r.file)
                && let (Some(k), Some((title, _))) = (group_key(p), group_title(p))
            {
                n.entry(k).or_insert((0, title)).0 += 1;
            }
        }
        n.into_iter().max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.0.cmp(&a.0))).map(|((_, y), (_, t))| (t, y))
    };
    if !tainted.is_empty() {
        tracing::info!(
            "library {library_id}: placing the files of {} items again, re-attaching {} extras",
            tainted.len(),
            redo.values().filter(|w| **w == Redo::Extra).count()
        );
    }
    status.total = redo.len() as u64;
    status.done = 0;
    lib.set_status(status.clone());

    // Items left as they are: what the scanner calls them, a folder's poster taken back, and the
    // mark that they were looked at.
    let keep: Vec<(i64, Option<String>)> = by_group
        .iter()
        .filter(|(g, files)| !tainted.contains(g) && !files.iter().all(|r| r.done))
        .map(|(g, files)| (*g, dominant(files).map(|(t, _)| parse::sort_title(&t))))
        .collect();
    lib.db()
        .write(move |t| {
            for (g, sort) in &keep {
                if let Some(s) = sort {
                    t.execute(
                        "UPDATE items SET extra = json_set(extra, '$.placed', coalesce(json_extract(extra, '$.placed'), ?1)) \
                         WHERE id = ?2",
                        params![s, g],
                    )?;
                }
                mark_repaired(t, *g)?;
            }
            for (g, paths) in &shared_art {
                for p in paths {
                    let n = t.execute(
                        "DELETE FROM images WHERE owner_kind = 'item' AND owner_id = ?1 AND locked = 0 AND source = ?2",
                        params![g, p],
                    )?;
                    if n > 0 {
                        t.execute("UPDATE items SET extra = json_remove(extra, '$.thumb_tried') WHERE id = ?1", [g])?;
                    }
                }
            }
            Ok(())
        })
        .await?;

    // Each reset item with all its files in one transaction: a cancelled repair leaves every
    // item either repaired or as it was.
    let mut touched = Vec::new();
    let mut groups: Vec<i64> = tainted.iter().copied().collect();
    groups.sort_unstable();
    for g in groups {
        let files = by_group.get(&g).cloned().unwrap_or_default();
        let title = dominant(&files);
        let mut analyzed = Vec::new();
        let mut files = files;
        // Main files first: extras look for their movie.
        files.sort_by_key(|r| (parse::is_extra(&r.path).is_some(), r.path.clone()));
        for r in &files {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            let f = Found { path: r.path.clone(), size: r.size as u64, mtime: r.mtime };
            let mut a = analyze(lib, kind, roots, f, r.quick_hash.clone(), context(per_folder, &r.path), false).await;
            a.retitle = true;
            analyzed.push((r.file, r.item, a));
        }
        let n = analyzed.len() as u64;
        touched.extend(
            lib.db()
                .write(move |t| {
                    reset_item(t, g, title.as_ref())?;
                    let mut touched = Vec::new();
                    for (file, old_item, a) in &analyzed {
                        touched.extend(apply(t, library_id, a)?);
                        carry_over(t, *file, *old_item, g)?;
                    }
                    mark_repaired(t, g)?;
                    refresh_fts(t, &touched)?;
                    Ok(touched)
                })
                .await?,
        );
        status.done += n;
        lib.set_status(status.clone());
    }

    // Extras and files in no item: attached again on their own.
    let mut rest: Vec<&Row> =
        rows.iter().filter(|r| matches!(redo.get(&r.file), Some(Redo::Extra | Redo::Loose))).collect();
    rest.sort_by_key(|r| (parse::is_extra(&r.path).is_some(), r.path.clone()));
    for r in rest {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        let f = Found { path: r.path.clone(), size: r.size as u64, mtime: r.mtime };
        let a = analyze(lib, kind, roots, f, r.quick_hash.clone(), context(per_folder, &r.path), false).await;
        touched.extend(
            lib.db()
                .write(move |t| {
                    let touched = apply(t, library_id, &a)?;
                    refresh_fts(t, &touched)?;
                    Ok(touched)
                })
                .await?,
        );
        status.done += 1;
        if status.done.is_multiple_of(25) {
            lib.set_status(status.clone());
        }
    }
    Ok(touched)
}

fn mark_repaired(t: &Transaction, item: i64) -> rusqlite::Result<()> {
    t.execute(
        "UPDATE items SET extra = json_set(extra, '$.placement', ?1) WHERE id = ?2",
        params![PLACEMENT_VERSION, item],
    )?;
    Ok(())
}

/// A file moved from `old_item` (in the movie or show `old_group`) to another item: what users
/// kept on the old one goes along (watch state, favourites, ratings; a show being followed), and
/// playlists that pointed at an old item left without files point at the new one.
fn carry_over(t: &Transaction, file: i64, old_item: Option<i64>, old_group: i64) -> rusqlite::Result<()> {
    let Some(old) = old_item else { return Ok(()) };
    let new: Option<(i64, Option<i64>)> = t
        .query_row(
            "SELECT i.id, CASE i.kind WHEN 'episode' THEN (SELECT s.parent_id FROM items s WHERE s.id = i.parent_id) \
             ELSE i.id END FROM item_files x JOIN items i ON i.id = x.item_id WHERE x.file_id = ?1",
            [file],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((new, new_group)) = new else { return Ok(()) };
    if new == old {
        return Ok(());
    }
    t.execute(
        "INSERT OR IGNORE INTO watch_state(user_id, item_id, position_ms, played, play_count, last_played_at, favorite, rating) \
         SELECT user_id, ?1, position_ms, played, play_count, last_played_at, favorite, rating FROM watch_state WHERE item_id = ?2",
        params![new, old],
    )?;
    t.execute(
        "UPDATE playlist_items SET item_id = ?1 WHERE item_id = ?2 \
         AND NOT EXISTS (SELECT 1 FROM item_files WHERE item_id = ?2)",
        params![new, old],
    )?;
    if let Some(ng) = new_group.filter(|ng| *ng != old_group) {
        t.execute(
            "INSERT OR IGNORE INTO monitored_shows(item_id, profile_id, from_season, added_by, created_at, checked_at) \
             SELECT ?1, profile_id, from_season, added_by, created_at, checked_at FROM monitored_shows WHERE item_id = ?2",
            params![ng, old_group],
        )?;
    }
    Ok(())
}

/// Forgets what was fetched or guessed for an item that held files of several movies or shows
/// (fields the user locked stay), names it after the files it keeps, and lets it be identified
/// again. A show's seasons and episodes lose their fetched images, overviews and dates too.
fn reset_item(t: &Transaction, item: i64, title: Option<&Titled>) -> rusqlite::Result<()> {
    let lock = locked(t, item)?;
    let free = |f: &str| !lock.contains(f);
    if let Some((title, year)) = title {
        if free("title") {
            t.execute(
                "UPDATE items SET title = ?1, sort_title = ?2 WHERE id = ?3",
                params![title, parse::sort_title(title), item],
            )?;
        }
        if free("year") {
            t.execute("UPDATE items SET year = ?1 WHERE id = ?2", params![year, item])?;
        }
    }
    for field in ["original_title", "overview", "rating", "official_rating", "premiered"] {
        if free(field) {
            t.execute(&format!("UPDATE items SET {field} = NULL WHERE id = ?1"), [item])?;
        }
    }
    if free("genres") {
        t.execute("DELETE FROM item_genres WHERE item_id = ?1", [item])?;
    }
    if free("people") {
        t.execute("DELETE FROM item_people WHERE item_id = ?1", [item])?;
    }
    t.execute("DELETE FROM collection_items WHERE item_id = ?1", [item])?;
    let placed = title.map(|(t, _)| parse::sort_title(t));
    t.execute(
        "UPDATE items SET provider_ids = '{}', \
         extra = json_remove(extra, '$.meta_checked', '$.meta_provider', '$.ratings', '$.thumb_tried', '$.placed') \
         WHERE id = ?1",
        [item],
    )?;
    if let Some(p) = placed {
        t.execute("UPDATE items SET extra = json_set(extra, '$.placed', ?1) WHERE id = ?2", params![p, item])?;
    }
    t.execute(
        "WITH RECURSIVE tree(id) AS (SELECT ?1 UNION ALL SELECT i.id FROM items i JOIN tree ON i.parent_id = tree.id) \
         DELETE FROM images WHERE owner_kind = 'item' AND locked = 0 AND owner_id IN (SELECT id FROM tree)",
        [item],
    )?;
    t.execute(
        "WITH RECURSIVE tree(id) AS (SELECT id FROM items WHERE parent_id = ?1 \
           UNION ALL SELECT i.id FROM items i JOIN tree ON i.parent_id = tree.id) \
         UPDATE items SET \
           overview = CASE WHEN instr(locked_fields, 'overview') > 0 THEN overview END, \
           premiered = CASE WHEN instr(locked_fields, 'premiered') > 0 THEN premiered END, \
           extra = json_remove(extra, '$.thumb_tried') \
         WHERE id IN (SELECT id FROM tree)",
        [item],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Context = Context { alone: true, legacy: false };
    const MANY: Context = Context { alone: false, legacy: false };

    fn movie(f: &str, cx: Context) -> (String, Option<i32>) {
        match place(LibraryKind::Movies, &[PathBuf::from("/m")], Path::new(f), None, None, cx) {
            Placement::Movie { title, year, .. } => (title, year),
            p => panic!("{p:?}"),
        }
    }

    fn show(f: &str) -> String {
        match place(LibraryKind::Shows, &[PathBuf::from("/tv")], Path::new(f), None, None, ONE) {
            Placement::Episode { show, .. } => show,
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn grouping_folders_name_no_movie() {
        // A folder of several movies names none of them.
        assert_eq!(movie("/m/Marvel/Iron Man.mkv", MANY).0, "Iron Man");
        assert_eq!(movie("/m/Marvel/Thor.mkv", MANY).0, "Thor");
        // Nor does a folder without a year that holds one.
        assert_eq!(movie("/m/4K/Avatar.mkv", ONE).0, "Avatar");
        // A folder with a year names its one movie; a release name keeps its own.
        assert_eq!(movie("/m/Dune (2021)/dune.mkv", ONE), ("Dune".into(), Some(2021)));
        assert_eq!(movie("/m/Dune (2021)/abc-dune-1080p.mkv", ONE).1, Some(2021));
        assert_eq!(movie("/m/Collection/Heat.1995.mkv", MANY), ("Heat".into(), Some(1995)));
        // Generic names always take the folder's.
        assert_eq!(movie("/m/Sintel (2010)/movie.mkv", MANY), ("Sintel".into(), Some(2010)));
        assert_eq!(movie("/m/Some Film/VTS_01_1.vob", ONE).0, "Some Film");
        // The old rules (only for the repair) took the grouping folder.
        assert_eq!(movie("/m/Marvel/Thor.mkv", Context { alone: false, legacy: true }).0, "Marvel");
    }

    #[test]
    fn the_show_is_the_folder_above_its_seasons() {
        assert_eq!(show("/tv/Anime/Show A/Season 1/Show.A.S01E01.mkv"), "Show A");
        assert_eq!(show("/tv/Anime/Show B/Season 1/Show.B.S01E01.mkv"), "Show B");
        assert_eq!(show("/tv/Anime/Show C/Show.C.S01E01.mkv"), "Show C");
        assert_eq!(show("/tv/The Office (2005)/Season 1/The.Office.S01E01.mkv"), "The Office");
        // Release folders inside the show's are not the show.
        assert_eq!(show("/tv/Show/Show.S01.1080p.WEB/Show.S01E02.mkv"), "Show");
        assert_eq!(show("/tv/Show/Season 1/Show.S01E03.720p/Show.S01E03.mkv"), "Show");
        assert_eq!(show("/tv/Show/Specials/Show.S00E01.mkv"), "Show");
        // A season pack straight under the root: the file's name.
        assert_eq!(show("/tv/Great.Show.S01.1080p/Great.Show.S01E01.mkv"), "Great Show");
        assert_eq!(
            parse::show_home(Path::new("/tv"), Path::new("/tv/Great.Show.S01.1080p/Great.Show.S01E01.mkv")),
            Some(PathBuf::from("Great.Show.S01.1080p"))
        );
    }

    #[test]
    fn real_layouts_keep_their_names() {
        // Parts and versions of one movie count once: the folder's year stays.
        let per = videos_per_folder(
            ["/m/Kill Bill Vol 1 (2003)/Kill Bill Vol 1 cd1.avi", "/m/Kill Bill Vol 1 (2003)/Kill Bill Vol 1 cd2.avi"]
                .iter()
                .map(Path::new),
        );
        let cx = context(&per, Path::new("/m/Kill Bill Vol 1 (2003)/Kill Bill Vol 1 cd1.avi"));
        assert!(cx.alone);
        assert_eq!(
            movie("/m/Kill Bill Vol 1 (2003)/Kill Bill Vol 1 cd1.avi", cx),
            ("Kill Bill Vol 1".into(), Some(2003))
        );
        let per = videos_per_folder(
            ["/m/Dune (2021)/Dune - 1080p.mkv", "/m/Dune (2021)/Dune - 4K.mkv"].iter().map(Path::new),
        );
        assert_eq!(per.get(Path::new("/m/Dune (2021)")), Some(&1));
        // A folder named after its one movie names it, also without a year.
        assert_eq!(movie("/m/The Shawshank Redemption/shawshank.mkv", ONE).0, "The Shawshank Redemption");
        assert_eq!(movie("/m/The.Matrix.1080p.BluRay.x264-GRP/matrix.mkv", ONE).0, "The Matrix");
        // Disc structures take the movie's folder.
        assert_eq!(movie("/m/Some Film (2005)/VIDEO_TS/VTS_01_1.vob", MANY), ("Some Film".into(), Some(2005)));
        assert_eq!(movie("/m/Other Film (2007)/BDMV/STREAM/00000.m2ts", MANY), ("Other Film".into(), Some(2007)));
        // Two-letter titles are titles.
        assert_eq!(movie("/m/Pixar/Up.2009.1080p.mkv", MANY), ("Up".into(), Some(2009)));
        assert_eq!(movie("/m/Horror/It (2017).mkv", MANY), ("It".into(), Some(2017)));
    }

    #[test]
    fn season_and_year_folders_are_not_the_show() {
        assert_eq!(show("/tv/The Daily Show/Season 2024/The Daily Show - 2024-05-01 - Guest.mkv"), "The Daily Show");
        assert_eq!(show("/tv/The Daily Show/2024/The Daily Show - 2024-05-01 - Guest.mkv"), "The Daily Show");
        assert_eq!(show("/tv/Legend of Korra/Book 1/Legend.of.Korra.S01E01.mkv"), "Legend of Korra");
        assert_eq!(show("/tv/Gomorra/Stagione 1/Gomorra.S01E01.mkv"), "Gomorra");
        assert_eq!(show("/tv/Show/Season 1/1/Show.S01E01.mkv"), "Show");
        // Numbers at the top are shows.
        assert_eq!(show("/tv/24/Season 1/24.S01E01.mkv"), "24");
        assert_eq!(show("/tv/1883/1883.S01E01.mkv"), "1883");
        // A season pack's folder at the top is the show (by its name without the release).
        assert_eq!(show("/tv/Show.Name.S01-S05.Complete/Season 1/Show.Name.S01E01.mkv"), "Show Name");
        assert_eq!(parse::season_folder("Season 2024"), Some(2024));
        assert_eq!(parse::season_folder("Book of Boba Fett"), None);
    }

    #[test]
    fn same_titles_match_only_without_doubt() {
        let remakes = [(1, Some(1984)), (2, Some(2021))];
        assert_eq!(same_item(&remakes, Some(2021)), Some(2));
        assert_eq!(same_item(&remakes, None), None, "no guess between remakes");
        assert_eq!(same_item(&remakes, Some(2000)), None);
        assert_eq!(same_item(&[(1, None)], Some(2021)), Some(1));
        assert_eq!(same_item(&[(1, Some(1984))], None), Some(1));
        assert_eq!(same_item(&[(1, Some(1984)), (3, None)], None), Some(3));
        assert_eq!(same_item(&[(1, Some(1984)), (3, None)], Some(2021)), None);
        assert_eq!(same_item(&[], None), None);
    }

    #[test]
    fn placements() {
        let roots = vec![PathBuf::from("/m")];
        assert_eq!(
            place(
                LibraryKind::Movies,
                &roots,
                Path::new("/m/The Matrix (1999)/The.Matrix.1999.1080p.mkv"),
                None,
                None,
                ONE
            ),
            Placement::Movie { title: "The Matrix".into(), year: Some(1999), role: "main" }
        );
        // A generic file name takes the folder's.
        assert_eq!(
            place(LibraryKind::Movies, &roots, Path::new("/m/Sintel (2010)/movie.mkv"), None, None, ONE),
            Placement::Movie { title: "Sintel".into(), year: Some(2010), role: "main" }
        );
        assert!(matches!(
            place(LibraryKind::Movies, &roots, Path::new("/m/Sintel (2010)/sample.mkv"), None, None, ONE),
            Placement::Movie { role: "sample", .. }
        ));
        let roots = vec![PathBuf::from("/tv")];
        assert_eq!(
            place(
                LibraryKind::Shows,
                &roots,
                Path::new("/tv/Doctor Who (2005)/Season 1/Doctor.Who.S01E02.mkv"),
                None,
                None,
                ONE
            ),
            Placement::Episode {
                show: "Doctor Who".into(),
                show_year: Some(2005),
                season: 1,
                episode: 2,
                episode_end: None,
                title: None
            }
        );
        assert_eq!(
            place(LibraryKind::Shows, &roots, Path::new("/tv/Show/Season 2/05 - Name.mkv"), None, None, ONE),
            Placement::Episode {
                show: "Show".into(),
                show_year: None,
                season: 2,
                episode: 0,
                episode_end: None,
                title: None
            }
        );
        let roots = vec![PathBuf::from("/music")];
        let tags = AudioTags {
            album: Some("Album".into()),
            artist: Some("A".into()),
            track: Some(4),
            title: Some("Song".into()),
            ..Default::default()
        };
        assert_eq!(
            place(LibraryKind::Music, &roots, Path::new("/music/A/Album/04 - Song.flac"), Some(&tags), None, ONE),
            Placement::Track {
                artist: "A".into(),
                album: "Album".into(),
                album_year: None,
                disc: None,
                track: Some(4),
                title: "Song".into()
            }
        );
        // No tags: folders and the file name.
        assert_eq!(
            place(LibraryKind::Music, &roots, Path::new("/music/Artist X/Best Of/1-03 Hit.mp3"), None, None, ONE),
            Placement::Track {
                artist: "Artist X".into(),
                album: "Best Of".into(),
                album_year: None,
                disc: Some(1),
                track: Some(3),
                title: "Hit".into()
            }
        );
    }
}
