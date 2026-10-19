//! Scanning real (generated) media end to end: movies, shows, music; queries, search, home,
//! watch state, moves and deletions. Skips when libmpv isn't installed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use corrent_db::Db;
use corrent_proto::library::{ItemKind, ItemQuery, LibraryKind, LibrarySettings, NewLibrary, SortBy, StateUpdate};

use crate::Library;
use crate::probe::Prober;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("corrent-lib-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn encode(out: &Path, secs: u32, video: bool, meta: &str) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let ext = out.extension().unwrap().to_str().unwrap();
    let mut options = vec![("oset-metadata".to_string(), meta.to_string())];
    match ext {
        "mkv" => options.extend([
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=ultrafast".into()),
            ("oac".into(), "aac".into()),
        ]),
        "flac" => options.extend([("of".into(), "flac".into()), ("oac".into(), "flac".into())]),
        _ => unreachable!(),
    }
    let job = corrent_mpv::encode::EncodeJob {
        input: if video {
            format!("av://lavfi:testsrc=duration={secs}:size=160x120:rate=10")
        } else {
            format!("av://lavfi:sine=frequency=440:duration={secs}")
        },
        audio_file: video.then(|| format!("av://lavfi:sine=duration={secs}")),
        output: out.display().to_string(),
        options,
    };
    corrent_mpv::encode::run(&job, Duration::from_secs(60), &mut |_| true).unwrap();
}

fn png(path: &Path) {
    let img = image::RgbImage::from_fn(300, 450, |x, y| image::Rgb([x as u8, y as u8, 90]));
    img.save(path).unwrap();
}

async fn setup(name: &str) -> (PathBuf, Library, i64) {
    let dir = scratch(name);
    let db = Db::open(&dir.join("corrent.db")).unwrap();
    db.write(|t| {
        t.execute("INSERT INTO users(id, name, pw_hash, role, created_at) VALUES (1, 'u', 'x', 'user', 0)", [])
    })
    .await
    .unwrap();
    let lib = Library::new(db, &dir, Prober::new(Prober::find_worker(), 2));
    (dir, lib, 1)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scans_movies_shows_and_music() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let (dir, lib, user) = setup("all").await;

    // Movies: one in its own folder with a poster and an .nfo, one with a release name.
    let movies = dir.join("Movies");
    encode(&movies.join("Sintel (2010)/movie.mkv"), 3, true, "title=\"x\"");
    png(&movies.join("Sintel (2010)/poster.png"));
    std::fs::write(
        movies.join("Sintel (2010)/movie.nfo"),
        "<movie><title>Sintel</title><year>2010</year><plot>A girl and her dragon.</plot><genre>Fantasy</genre>\
         <director>Colin Levy</director><uniqueid type=\"tmdb\">45745</uniqueid></movie>",
    )
    .unwrap();
    encode(&movies.join("Big.Buck.Bunny.2008.1080p.BluRay.x264-GRP.mkv"), 2, true, "title=\"x\"");
    std::fs::write(
        movies.join("Big.Buck.Bunny.2008.1080p.BluRay.x264-GRP.en.srt"),
        "1\n00:00:00,000 --> 00:00:01,000\nHi\n",
    )
    .unwrap();

    // Shows.
    let shows = dir.join("Shows");
    encode(&shows.join("Test Show (2020)/Season 1/Test.Show.S01E01.mkv"), 2, true, "title=\"x\"");
    encode(&shows.join("Test Show (2020)/Season 1/Test.Show.S01E02.mkv"), 2, true, "title=\"x\"");

    // Music with tags.
    let music = dir.join("Music");
    encode(
        &music.join("Band/Record/01 - One.flac"),
        2,
        false,
        "title=\"One\",artist=\"Band\",album=\"Record\",track=\"1\",date=\"2021\",genre=\"Rock\",work=\"Suite\"",
    );
    encode(
        &music.join("Band/Record/02 - Two.flac"),
        2,
        false,
        "title=\"Two\",artist=\"Band\",album=\"Record\",track=\"2\",date=\"2021\",genre=\"Rock\"",
    );

    let mk = |name: &str, kind, folder: &Path| NewLibrary {
        name: name.into(),
        kind,
        folders: vec![folder.display().to_string()],
        settings: LibrarySettings::defaults(),
    };
    let lm = lib.create(mk("Movies", LibraryKind::Movies, &movies)).await.unwrap();
    let ls = lib.create(mk("Shows", LibraryKind::Shows, &shows)).await.unwrap();
    let lu = lib.create(mk("Music", LibraryKind::Music, &music)).await.unwrap();
    for l in [&lm, &ls, &lu] {
        let s = lib.scan_now(l.id).await.unwrap();
        assert!(!s.running && s.error.is_none(), "{s:?}");
    }
    assert_eq!(lib.scan_status(lm.id).unwrap().added, 2);

    // Movies.
    let page = lib.items(user, ItemQuery { library_id: Some(lm.id), ..Default::default() }).await.unwrap();
    let titles: Vec<_> = page.items.iter().map(|i| (i.title.clone(), i.year)).collect();
    assert_eq!(titles, vec![("Big Buck Bunny".into(), Some(2008)), ("Sintel".into(), Some(2010))]);
    let sintel = page.items.iter().find(|i| i.title == "Sintel").unwrap();
    assert!(sintel.images.iter().any(|i| i.kind == "poster" && !i.blurhash.is_empty()), "{:?}", sintel.images);
    let d = lib.item(user, sintel.id).await.unwrap();
    assert_eq!(d.overview.as_deref(), Some("A girl and her dragon."));
    assert_eq!(d.genres, vec!["Fantasy"]);
    assert_eq!(d.provider_ids.get("tmdb").map(String::as_str), Some("45745"));
    assert!(d.credits.iter().any(|c| c.name == "Colin Levy" && c.role == "director"));
    let f = &d.files[0];
    assert!(f.streams.iter().any(|s| s.kind == "video" && s.width == Some(160)));
    assert!(f.duration_ms.unwrap() > 2000);
    let bbb = page.items.iter().find(|i| i.title == "Big Buck Bunny").unwrap();
    let d = lib.item(user, bbb.id).await.unwrap();
    assert!(
        d.files[0].streams.iter().any(|s| s.external && s.lang.as_deref() == Some("en")),
        "external subtitle found"
    );

    // Shows: show > season > episodes, ordered.
    let shows_page = lib.items(user, ItemQuery { library_id: Some(ls.id), ..Default::default() }).await.unwrap();
    assert_eq!(shows_page.items.len(), 1);
    let show = &shows_page.items[0];
    assert_eq!((show.title.as_str(), show.year, show.state.unplayed), ("Test Show", Some(2020), Some(2)));
    let seasons = lib.items(user, ItemQuery { parent_id: Some(show.id), ..Default::default() }).await.unwrap();
    assert_eq!(seasons.items[0].index, Some(1));
    let eps = lib
        .items(user, ItemQuery { parent_id: Some(seasons.items[0].id), sort: SortBy::Index, ..Default::default() })
        .await
        .unwrap();
    assert_eq!(eps.items.iter().map(|e| e.index).collect::<Vec<_>>(), vec![Some(1), Some(2)]);
    assert_eq!(eps.items[0].context, vec!["Test Show".to_string(), "Season 1".to_string()]);

    // Music: artist > album > tracks.
    let albums = lib
        .items(user, ItemQuery { library_id: Some(lu.id), kind: Some(ItemKind::Album), ..Default::default() })
        .await
        .unwrap();
    assert_eq!((albums.items[0].title.as_str(), albums.items[0].year), ("Record", Some(2021)));
    let tracks =
        lib.items(user, ItemQuery { parent_id: Some(albums.items[0].id), ..Default::default() }).await.unwrap();
    assert_eq!(tracks.items.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["One", "Two"]);
    assert_eq!(tracks.items[0].work.as_deref(), Some("Suite"), "the work tag groups classical movements");

    // Query filters: kinds, everything below a show, ids, titles, parent fields.
    let below = lib
        .items(
            user,
            ItemQuery {
                ancestor_id: Some(show.id),
                kinds: vec![ItemKind::Episode],
                sort: SortBy::Index,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(below.total, 2);
    assert_eq!((below.items[0].grandparent_id, below.items[0].parent_index), (Some(show.id), Some(1)));
    assert!(below.items[0].images.iter().all(|i| i.inherited || !i.sha.is_empty()));
    let some = lib.items(user, ItemQuery { ids: Some(vec![sintel.id, bbb.id]), ..Default::default() }).await.unwrap();
    assert_eq!(some.total, 2);
    let none = lib.items(user, ItemQuery { ids: Some(vec![]), ..Default::default() }).await.unwrap();
    assert_eq!(none.total, 0);
    let named = lib
        .items(
            user,
            ItemQuery { title_contains: Some("bunny".into()), kinds: vec![ItemKind::Movie], ..Default::default() },
        )
        .await
        .unwrap();
    assert_eq!(named.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), vec!["Big Buck Bunny"]);
    let weird =
        lib.items(user, ItemQuery { title_contains: Some("50%_'".into()), ..Default::default() }).await.unwrap();
    assert_eq!(weird.total, 0);

    // Search.
    let r = lib.search(user, "sint").await.unwrap();
    assert_eq!(r.items.first().map(|i| i.title.as_str()), Some("Sintel"));
    let r = lib.search(user, "levy").await.unwrap();
    assert!(r.items.iter().any(|i| i.title == "Sintel"), "people are searchable");

    // Watch state: progress → continue watching; finishing episode 1 → next up is episode 2.
    lib.set_state(user, sintel.id, StateUpdate { position_ms: Some(1000), ..Default::default() }).await.unwrap();
    let st =
        lib.set_state(user, eps.items[0].id, StateUpdate { played: Some(true), ..Default::default() }).await.unwrap();
    assert!(st.played && st.play_count == 1);
    let home = lib.home(user).await.unwrap();
    let row = |id: &str| home.iter().find(|r| r.id == id).map(|r| r.items.iter().map(|i| i.id).collect::<Vec<_>>());
    assert_eq!(row("continue"), Some(vec![sintel.id]));
    assert_eq!(row("next_up"), Some(vec![eps.items[1].id]));
    // Past the threshold counts as played.
    let st =
        lib.set_state(user, sintel.id, StateUpdate { position_ms: Some(2900), ..Default::default() }).await.unwrap();
    assert!(st.played, "{st:?}");
    // Marking the show played marks every episode.
    lib.set_state(user, show.id, StateUpdate { played: Some(true), ..Default::default() }).await.unwrap();
    let show_now = lib.items(user, ItemQuery { library_id: Some(ls.id), ..Default::default() }).await.unwrap();
    assert_eq!(show_now.items[0].state.unplayed, Some(0));

    // A moved file keeps its row (and the item's state).
    std::fs::rename(movies.join("Sintel (2010)"), movies.join("Sintel Renamed (2010)")).unwrap();
    lib.scan_now(lm.id).await.unwrap();
    let again = lib.items(user, ItemQuery { library_id: Some(lm.id), ..Default::default() }).await.unwrap();
    let s2 = again.items.iter().find(|i| i.title == "Sintel").expect("still there");
    assert_eq!(s2.id, sintel.id, "same item after a move");
    assert!(s2.state.played);

    // A deleted file is only marked missing (grace period).
    std::fs::remove_file(movies.join("Big.Buck.Bunny.2008.1080p.BluRay.x264-GRP.mkv")).unwrap();
    lib.scan_now(lm.id).await.unwrap();
    let d = lib.item(user, bbb.id).await.unwrap();
    assert!(d.files[0].missing);

    // An unavailable folder empties nothing.
    let gone = dir.join("Elsewhere");
    lib.update(
        lm.id,
        corrent_proto::library::LibraryUpdate { folders: Some(vec![gone.display().to_string()]), ..Default::default() },
    )
    .await
    .unwrap();
    lib.scan_now(lm.id).await.unwrap();
    assert_eq!(lib.items(user, ItemQuery { library_id: Some(lm.id), ..Default::default() }).await.unwrap().total, 2);

    // Library allowlists hide other libraries.
    lib.set_user_libraries(user, vec![lu.id]).await.unwrap();
    assert!(lib.item(user, sintel.id).await.is_err());
    assert_eq!(lib.search(user, "sintel").await.unwrap().items.len(), 0);
    let _ = std::fs::remove_dir_all(dir);
}
