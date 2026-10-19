//! Robustness: the name parser, folder rules and the `.nfo` reader take anything a download
//! can contain without panicking, hanging, or producing nonsense numbers.

use std::path::Path;
use std::time::{Duration, Instant};

use corrent_fuzz::{iterations, run};

const NAMES: &[&str] = &[
    "The.Matrix.1999.1080p.BluRay.x264-GROUP",
    "Breaking.Bad.S05E14.Ozymandias.720p.WEB-DL",
    "Show Name - 1x02 - Title",
    "Series.S01.COMPLETE.2160p",
    "Movie (2010) [1080p] {imdb-tt0123456}",
    "[SubGroup] Anime Title - 12 (1080p) [ABCD1234]",
    "01 - Artist - Track Name (feat. Someone)",
    "Documentary.2019.Part.2.HDTV",
    "Some.Show.2021.03.15.Episode.Name",
    "S01E01E02E03",
    "Title.S1E1-E3",
];

#[test]
fn name_parser_never_panics_and_stays_sane() {
    let started = Instant::now();
    run("parse_name", iterations(20_000), |r| {
        let s = r.case(NAMES, 120);
        let p = crate::parse::parse_name(&s);
        assert!(p.title.len() <= s.len() * 4 + 16, "title grew: {s:?} → {:?}", p.title);
        if let Some(y) = p.year {
            assert!((1870..=2200).contains(&y), "year {y} from {s:?}");
        }
        let _ = crate::parse::track_name(&s);
        let _ = crate::parse::season_folder(&s);
        let _ = crate::parse::sort_title(&s);
    });
    assert!(started.elapsed() < Duration::from_secs(60), "parser too slow: {:?}", started.elapsed());
}

#[test]
fn path_rules_never_panic() {
    run("paths", iterations(5_000), |r| {
        let root = format!("/media/{}", r.text(8));
        let rel: Vec<String> = (0..r.below(5)).map(|_| r.case(NAMES, 30)).collect();
        let file = Path::new(&root).join(rel.join("/")).join(format!(
            "{}.{}",
            r.case(NAMES, 20),
            r.pick(&["mkv", "mp4", "srt", "", "flac", "jpg", "nfo"])
        ));
        let _ = crate::parse::show_folder(Path::new(&root), &file);
        let _ = crate::parse::is_extra(&file);
        let _ = crate::parse::file_kind(&file);
    });
}

const NFOS: &[&str] = &[
    "<?xml version=\"1.0\"?><movie><title>Sintel</title><year>2010</year><uniqueid type=\"tmdb\">45745</uniqueid><genre>Animation</genre><actor><name>A</name><role>B</role></actor></movie>",
    "<tvshow><title>Show</title><plot>Text &amp; more</plot></tvshow>",
    "<episodedetails><title>Ep</title><season>1</season><episode>2</episode></episodedetails>",
    "https://www.themoviedb.org/movie/45745",
];

#[test]
fn nfo_reader_never_panics() {
    run("nfo", iterations(10_000), |r| {
        let s = r.case(NFOS, 300);
        let _ = crate::nfo::parse(&s);
    });
}
