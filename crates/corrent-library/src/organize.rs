//! Clean names for files that go into a library: `Title (Year)/Title (Year).mkv` for movies,
//! `Show/Season 01/Show - S01E02.mkv` for episodes. Samples, NFOs and pictures stay behind; extras
//! go to the title's `extras` folder and subtitles keep their language next to their video.

use std::path::{Path, PathBuf};

use corrent_proto::library::LibraryKind;

use crate::parse::{FileKind, Parsed, file_kind, is_extra, parse_name};

/// A file name or folder that is valid on every system (no `:` `/` `\` `?` `*` `"` `<` `>` `|`,
/// no trailing dots or spaces).
pub fn clean(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            ':' => out.push_str(" -"),
            '/' | '\\' | '|' => out.push('-'),
            '?' | '*' | '"' | '<' | '>' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        return "Untitled".into();
    }
    // Windows device names (CON, NUL, COM1…, also with an extension) open the device.
    let stem = out.split('.').next().unwrap_or_default().trim().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.chars().last().is_some_and(|c| c.is_ascii_digit() && c != '0'));
    if device { format!("_{out}") } else { out }
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase())).unwrap_or_default()
}

/// The movie's folder name: the file's own name when it says more than the torrent's (a year),
/// else the torrent's.
fn movie_title(file: &Parsed, torrent: &Parsed) -> String {
    let best = if file.year.is_some() || torrent.title.trim().is_empty() { file } else { torrent };
    let title = clean(if best.title.trim().is_empty() { &torrent.title } else { &best.title });
    match best.year.or(torrent.year) {
        Some(y) => format!("{title} ({y})"),
        None => title,
    }
}

/// Where each file of a download goes, relative to a library folder of `kind`: (path in the
/// download, path in the library). `files` are relative to the download's content folder (a
/// single-file torrent: just its name).
pub fn plan_names(kind: LibraryKind, torrent_name: &str, files: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let torrent = parse_name(torrent_name);
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    let videos: Vec<&PathBuf> =
        files.iter().filter(|f| file_kind(f) == FileKind::Video && is_extra(f) != Some("sample")).collect();
    let main: Vec<&PathBuf> = videos.iter().copied().filter(|f| is_extra(f).is_none()).collect();
    // Destination of each main video (without extension), for its extras and subtitles.
    let mut placed: Vec<(&PathBuf, PathBuf)> = Vec::new();
    for f in &main {
        let parsed = parse_name(&stem(f));
        let dest = match kind {
            LibraryKind::Movies => {
                let title = movie_title(&parsed, &torrent);
                let part = parsed.part.map(|n| format!(" - part{n}")).unwrap_or_default();
                PathBuf::from(&title).join(format!("{title}{part}"))
            }
            LibraryKind::Shows => {
                let show = clean(if parsed.title.trim().is_empty() { &torrent.title } else { &parsed.title });
                match (parsed.season.or(torrent.season), parsed.episode) {
                    (Some(s), Some(e)) => {
                        let end = parsed.episode_end.map(|x| format!("-E{x:02}")).unwrap_or_default();
                        PathBuf::from(&show)
                            .join(format!("Season {s:02}"))
                            .join(format!("{show} - S{s:02}E{e:02}{end}"))
                    }
                    // No episode number: kept as named, in the show's folder.
                    _ => PathBuf::from(&show).join(clean(&stem(f))),
                }
            }
            // Home videos and the rest: the download's own names (each part made valid), in a
            // folder of its name.
            _ => {
                let mut p = PathBuf::from(clean(torrent_name));
                for c in f.with_extension("").components() {
                    p.push(clean(&c.as_os_str().to_string_lossy()));
                }
                p
            }
        };
        // Two videos that clean up to the same name (Movie.mkv and Movie.1080p.mkv): the second
        // gets " - 2".
        let mut dest = dest;
        let taken = |d: &Path, out: &[(PathBuf, PathBuf)]| out.iter().any(|(_, o)| o.with_extension("") == d);
        if taken(&dest, &out) {
            let base = file_name(&dest);
            let mut n = 2;
            while taken(&dest.with_file_name(format!("{base} - {n}")), &out) {
                n += 1;
            }
            dest = dest.with_file_name(format!("{base} - {n}"));
        }
        out.push(((*f).clone(), dest.with_file_name(format!("{}{}", file_name(&dest), ext(f)))));
        placed.push((f, dest));
    }
    // Extras (trailers, featurettes): into the folder of the title they belong to.
    for f in videos.iter().filter(|f| is_extra(f).is_some()) {
        let Some((_, dest)) = placed.first() else { break };
        let folder = match kind {
            LibraryKind::Shows => dest.parent().and_then(Path::parent).unwrap_or(Path::new("")).to_path_buf(),
            _ => dest.parent().unwrap_or(Path::new("")).to_path_buf(),
        };
        out.push(((*f).clone(), folder.join("extras").join(format!("{}{}", clean(&stem(f)), ext(f)))));
    }
    // Subtitles: next to the video whose name they start with (or the only video), keeping what
    // follows that name (`.en`, `.en.forced`).
    for f in files.iter().filter(|f| file_kind(f) == FileKind::Subtitle) {
        let s = stem(f);
        let owner = placed
            .iter()
            .filter(|(v, _)| s.starts_with(&stem(v)))
            .max_by_key(|(v, _)| stem(v).len())
            .or(if placed.len() == 1 { placed.first() } else { None });
        let Some((v, dest)) = owner else { continue };
        let suffix = match s.strip_prefix(&stem(v)) {
            Some(rest) => rest.to_string(),
            // Named after nothing in particular (`English.srt`): its name becomes the language.
            None => format!(".{}", clean(&s)),
        };
        let name = format!("{}{suffix}{}", file_name(dest), ext(f));
        out.push((f.clone(), dest.with_file_name(name)));
    }
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kind: LibraryKind, torrent: &str, files: &[&str]) -> Vec<(String, String)> {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        plan_names(kind, torrent, &files)
            .into_iter()
            .map(|(a, b)| (a.display().to_string().replace('\\', "/"), b.display().to_string().replace('\\', "/")))
            .collect()
    }

    #[test]
    fn a_movie_release_gets_its_title_and_year() {
        let p = plan(
            LibraryKind::Movies,
            "Sintel.2010.1080p.BluRay.x264-GRP",
            &[
                "Sintel.2010.1080p.BluRay.x264-GRP.mkv",
                "Sintel.2010.1080p.BluRay.x264-GRP.en.srt",
                "sample/sample.mkv",
                "GRP.nfo",
                "Featurettes/Making of.mkv",
            ],
        );
        assert!(
            p.contains(&("Sintel.2010.1080p.BluRay.x264-GRP.mkv".into(), "Sintel (2010)/Sintel (2010).mkv".into())),
            "{p:?}"
        );
        assert!(
            p.contains(&(
                "Sintel.2010.1080p.BluRay.x264-GRP.en.srt".into(),
                "Sintel (2010)/Sintel (2010).en.srt".into()
            )),
            "{p:?}"
        );
        assert!(
            p.contains(&("Featurettes/Making of.mkv".into(), "Sintel (2010)/extras/Making of.mkv".into())),
            "{p:?}"
        );
        assert_eq!(p.len(), 3, "no sample, no nfo: {p:?}");
    }

    #[test]
    fn a_single_file_torrent_and_a_title_with_a_colon() {
        let p = plan(LibraryKind::Movies, "Star Wars: Episode IV (1977).mkv", &["Star Wars: Episode IV (1977).mkv"]);
        assert_eq!(
            p,
            vec![(
                "Star Wars: Episode IV (1977).mkv".into(),
                "Star Wars - Episode IV (1977)/Star Wars - Episode IV (1977).mkv".into()
            )]
        );
    }

    #[test]
    fn a_season_pack_becomes_season_folders() {
        let p = plan(
            LibraryKind::Shows,
            "Theme.Show.S01.1080p.WEB",
            &[
                "Theme.Show.S01E01.1080p.WEB.mkv",
                "Theme.Show.S01E02E03.1080p.WEB.mkv",
                "Theme.Show.S01E01.1080p.WEB.srt",
            ],
        );
        assert!(
            p.contains(&(
                "Theme.Show.S01E01.1080p.WEB.mkv".into(),
                "Theme Show/Season 01/Theme Show - S01E01.mkv".into()
            )),
            "{p:?}"
        );
        assert!(
            p.contains(&(
                "Theme.Show.S01E02E03.1080p.WEB.mkv".into(),
                "Theme Show/Season 01/Theme Show - S01E02-E03.mkv".into()
            )),
            "{p:?}"
        );
        assert!(
            p.contains(&(
                "Theme.Show.S01E01.1080p.WEB.srt".into(),
                "Theme Show/Season 01/Theme Show - S01E01.srt".into()
            )),
            "{p:?}"
        );
    }

    #[test]
    fn home_videos_keep_their_names() {
        let p = plan(LibraryKind::Videos, "Holiday 2024", &["day1/beach.mp4", "day2/hike.MP4", "notes.txt"]);
        assert_eq!(
            p,
            vec![
                ("day1/beach.mp4".into(), "Holiday 2024/day1/beach.mp4".into()),
                ("day2/hike.MP4".into(), "Holiday 2024/day2/hike.mp4".into()),
            ]
        );
    }

    #[test]
    fn names_are_valid_everywhere() {
        assert_eq!(clean("A: B / C?*\"<>|"), "A - B - C-");
        assert_eq!(clean("  "), "Untitled");
        assert_eq!(clean("Title..."), "Title");
        assert_eq!(clean("nul.mkv"), "_nul.mkv");
        assert_eq!(clean("COM3"), "_COM3");
        assert_eq!(clean("Console"), "Console");
    }

    #[test]
    fn nothing_leaves_the_library_folder() {
        // A Windows drive prefix or a parent step inside a torrent's names stays a plain name.
        let p = plan(LibraryKind::Videos, "Trip", &["C:/x.mkv", "C:x.mkv", "../y.mkv"]);
        for (_, to) in &p {
            assert!(to.starts_with("Trip/"), "{to}");
            assert!(!to.contains("..") && !to.contains(':'), "{to}");
        }
    }

    #[test]
    fn two_videos_with_the_same_clean_name_both_get_a_place() {
        let p = plan(LibraryKind::Movies, "Sintel (2010)", &["Sintel.2010.mkv", "Sintel.2010.1080p.mkv"]);
        let to: Vec<&str> = p.iter().map(|(_, t)| t.as_str()).collect();
        assert!(to.contains(&"Sintel (2010)/Sintel (2010).mkv"), "{to:?}");
        assert!(to.contains(&"Sintel (2010)/Sintel (2010) - 2.mkv"), "{to:?}");
    }
}
