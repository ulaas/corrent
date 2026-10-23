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

/// Older placement rules merged movies under a grouping folder, two shows under one folder, and
/// made extras into movies of their own; a library scanned with them is repaired on its next
/// scan, and items both rules agree on keep everything (a match the user chose included).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repair_splits_what_older_rules_merged() {
    use corrent_db::rusqlite::params;
    let (dir, lib, user) = setup("repair").await;
    let movies = dir.join("M");
    let shows = dir.join("S");
    for f in [
        movies.join("Marvel/Iron Man.mkv"),
        movies.join("Marvel/Thor.mkv"),
        movies.join("Amelie.2001.mkv"),
        movies.join("Heat (1995)/Heat.mkv"),
        movies.join("Heat (1995)/Trailers/teaser.mkv"),
        shows.join("Anime/Show A/Season 1/Show.A.S01E01.mkv"),
        shows.join("Anime/Show B/Season 1/Show.B.S01E01.mkv"),
    ] {
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"not really a video").unwrap();
    }
    png(&movies.join("Marvel/poster.png"));
    let mk = |name: &str, kind, folder: &Path| NewLibrary {
        name: name.into(),
        kind,
        folders: vec![folder.display().to_string()],
        settings: LibrarySettings::defaults(),
    };
    let lm = lib.create(mk("Movies", LibraryKind::Movies, &movies)).await.unwrap().id;
    let ls = lib.create(mk("Shows", LibraryKind::Shows, &shows)).await.unwrap().id;
    lib.scan_now(lm).await.unwrap();
    lib.scan_now(ls).await.unwrap();

    let titles = |lib_id: i64| {
        let lib = lib.clone();
        async move {
            let q = ItemQuery { library_id: Some(lib_id), ..Default::default() };
            let mut t: Vec<String> = lib.items(user, q).await.unwrap().items.into_iter().map(|i| i.title).collect();
            t.sort();
            t
        }
    };
    assert_eq!(titles(lm).await, ["Amelie", "Heat", "Iron Man", "Thor"]);
    assert_eq!(titles(ls).await, ["Show A", "Show B"]);

    // What the old rules and a wrong match made of it.
    let (iron, amelie) = lib
        .db()
        .write(move |t| {
            let id = |sql: &str| t.query_row(sql, [], |r| r.get::<_, i64>(0));
            let iron = id("SELECT id FROM items WHERE title = 'Iron Man'")?;
            let thor = id("SELECT id FROM items WHERE title = 'Thor'")?;
            let heat = id("SELECT id FROM items WHERE title = 'Heat'")?;
            let amelie = id("SELECT id FROM items WHERE title = 'Amelie'")?;
            t.execute("UPDATE item_files SET item_id = ?1 WHERE item_id = ?2", params![iron, thor])?;
            t.execute("DELETE FROM items WHERE id = ?1", [thor])?;
            t.execute(
                "UPDATE items SET title = 'Marvel', sort_title = 'marvel', overview = 'wrong', \
                 provider_ids = '{\"tmdb\":\"1\"}', extra = json_set(extra, '$.meta_checked', 1, '$.placed', 'marvel') \
                 WHERE id = ?1",
                [iron],
            )?;
            t.execute(
                "INSERT INTO images(owner_kind, owner_id, kind, sha256, width, height, source) \
                 VALUES ('item', ?1, 'poster', 'x', 1, 1, 'https://image.tmdb.org/wrong.jpg')",
                [iron],
            )?;
            // The trailer made a movie of its own.
            t.execute(
                "INSERT INTO items(library_id, kind, title, sort_title, added_at, updated_at) \
                 VALUES (?1, 'movie', 'teaser', 'teaser', 0, 0)",
                [lm],
            )?;
            let junk = t.last_insert_rowid();
            t.execute("UPDATE item_files SET item_id = ?1 WHERE item_id = ?2 AND role = 'trailer'", params![junk, heat])?;
            // A match the user chose: kept.
            t.execute(
                "UPDATE items SET title = 'Amélie', overview = 'kept', extra = json_set(extra, '$.meta_checked', 1) \
                 WHERE id = ?1",
                [amelie],
            )?;
            // Show B's episode in show A's, named after the grouping folder.
            let a = id("SELECT id FROM items WHERE title = 'Show A'")?;
            let b = id("SELECT id FROM items WHERE title = 'Show B'")?;
            let ep_a = id("SELECT e.id FROM items e JOIN items s ON s.id = e.parent_id JOIN items sh ON sh.id = s.parent_id \
                           WHERE sh.title = 'Show A' AND e.kind = 'episode'")?;
            t.execute(
                "UPDATE item_files SET item_id = ?1 WHERE item_id IN (SELECT e.id FROM items e JOIN items s ON s.id = e.parent_id \
                 WHERE s.parent_id = ?2)",
                params![ep_a, b],
            )?;
            t.execute("DELETE FROM items WHERE id = ?1", [b])?;
            t.execute(
                "UPDATE items SET title = 'Anime', sort_title = 'anime', overview = 'wrong', \
                 extra = json_set(extra, '$.meta_checked', 1, '$.placed', 'anime') WHERE id = ?1",
                [a],
            )?;
            t.execute("UPDATE items SET title = 'Wrong episode', overview = 'wrong' WHERE id = ?1", [ep_a])?;
            // Items an older version made carry no mark of the current rules.
            t.execute("UPDATE items SET extra = json_remove(extra, '$.placement')", [])?;
            t.execute("UPDATE libraries SET placement = 0", [])?;
            Ok((iron, amelie))
        })
        .await
        .unwrap();
    assert_eq!(titles(lm).await, ["Amélie", "Heat", "Marvel", "teaser"]);

    lib.scan_now(lm).await.unwrap();
    lib.scan_now(ls).await.unwrap();

    assert_eq!(titles(lm).await, ["Amélie", "Heat", "Iron Man", "Thor"], "split, junk gone, the match kept");
    assert_eq!(titles(ls).await, ["Show A", "Show B"]);
    let d = lib.item(user, iron).await.unwrap();
    assert_eq!(d.item.title, "Iron Man", "the item keeps its id and the files it still has");
    assert_eq!(d.overview, None, "what was fetched for the merged item is gone");
    assert!(d.provider_ids.is_empty(), "{:?}", d.provider_ids);
    assert!(d.item.images.is_empty(), "no wrong poster, no folder poster: {:?}", d.item.images);
    let pending: Vec<i64> = lib.items_needing_metadata(lm, 10).await.unwrap().into_iter().map(|p| p.id).collect();
    assert!(pending.contains(&iron), "identified again");
    assert!(!pending.contains(&amelie), "the user's match stays");
    assert_eq!(lib.item(user, amelie).await.unwrap().overview.as_deref(), Some("kept"));
    let (trailer_owner, placement, ep_titles): (String, i64, Vec<String>) = lib
        .db()
        .read(|c| {
            let owner = c.query_row(
                "SELECT i.title FROM item_files x JOIN items i ON i.id = x.item_id WHERE x.role = 'trailer'",
                [],
                |r| r.get(0),
            )?;
            let placement = c.query_row("SELECT min(placement) FROM libraries", [], |r| r.get(0))?;
            let mut st = c.prepare("SELECT title FROM items WHERE kind = 'episode' ORDER BY title")?;
            let eps = st.query_map([], |r| r.get(0))?.collect::<corrent_db::rusqlite::Result<Vec<String>>>()?;
            Ok((owner, placement, eps))
        })
        .await
        .unwrap();
    assert_eq!(trailer_owner, "Heat");
    assert_eq!(placement, crate::scan::PLACEMENT_VERSION);
    assert_eq!(ep_titles, ["Episode 1", "Episode 1"], "the wrong show's episode title is gone");
    let _ = std::fs::remove_dir_all(dir);
}

/// What the repair must never do: reset a movie because of its trailer or sample, undo a match
/// (maybe the user's own), or reset again on the next scan while a folder is offline. A file
/// that moves to an item of its own takes its watch state along; extras scanned before their
/// movie, or nested in extras folders, find it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repair_keeps_what_is_right() {
    use corrent_db::rusqlite::params;
    let (dir, lib, user) = setup("repair-keeps").await;
    let movies = dir.join("M");
    for f in [
        "Heat (1995)/Heat.mkv",
        "Heat (1995)/Sample/sample.mkv",
        "Heat (1995)/Heat-trailer.mkv",
        "Heat (1995)/Extras/Deleted Scenes/cut.mkv",
        "Picked/some.release.mkv",
        "Marvel/Iron Man.mkv",
        "Marvel/Thor.mkv",
    ] {
        let f = movies.join(f);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"not really a video").unwrap();
    }
    // A second folder that is offline: the library can't be marked repaired.
    let offline = dir.join("unplugged");
    let l = lib
        .create(NewLibrary {
            name: "Movies".into(),
            kind: LibraryKind::Movies,
            folders: vec![movies.display().to_string(), offline.display().to_string()],
            settings: LibrarySettings::defaults(),
        })
        .await
        .unwrap()
        .id;
    lib.scan_now(l).await.unwrap();
    let extras: i64 = lib
        .db()
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM item_files x JOIN items i ON i.id = x.item_id WHERE i.title = 'Heat' AND x.role != 'main'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(extras, 3, "the sample, the trailer and the nested deleted scene belong to Heat");

    // As an older version left it: matches, one picked by hand under a name the old rules gave,
    // Iron Man and Thor merged with someone's progress on the merged item.
    let (heat, picked, merged) = lib
        .db()
        .write(move |t| {
            let id = |sql: &str| t.query_row(sql, [], |r| r.get::<_, i64>(0));
            let heat = id("SELECT id FROM items WHERE title = 'Heat'")?;
            let picked = id("SELECT id FROM items WHERE title = 'some release'")?;
            let iron = id("SELECT id FROM items WHERE title = 'Iron Man'")?;
            let thor = id("SELECT id FROM items WHERE title = 'Thor'")?;
            let matched = "json_set(extra, '$.meta_checked', 1, '$.meta_provider', 'tmdb', '$.placement', 0)";
            t.execute(
                &format!("UPDATE items SET overview = 'fetched', provider_ids = '{{\"tmdb\":\"949\"}}', extra = {matched} WHERE id = ?1"),
                [heat],
            )?;
            t.execute(
                &format!("UPDATE items SET title = 'Picked', sort_title = 'picked', overview = 'by hand', extra = {matched} WHERE id = ?1"),
                [picked],
            )?;
            t.execute("UPDATE item_files SET item_id = ?1 WHERE item_id = ?2", params![iron, thor])?;
            t.execute("DELETE FROM items WHERE id = ?1", [thor])?;
            t.execute("UPDATE items SET title = 'Marvel', sort_title = 'marvel' WHERE id = ?1", [iron])?;
            t.execute(
                "INSERT INTO watch_state(user_id, item_id, position_ms, played, play_count) VALUES (1, ?1, 60000, 0, 1)",
                [iron],
            )?;
            t.execute("UPDATE items SET extra = json_remove(extra, '$.placement')", [])?;
            t.execute("UPDATE libraries SET placement = 0", [])?;
            Ok((heat, picked, iron))
        })
        .await
        .unwrap();

    lib.scan_now(l).await.unwrap();
    let d = lib.item(user, heat).await.unwrap();
    assert_eq!(d.overview.as_deref(), Some("fetched"), "a movie with extras keeps its match");
    assert_eq!(d.provider_ids.get("tmdb").map(String::as_str), Some("949"));
    let d = lib.item(user, picked).await.unwrap();
    assert_eq!((d.item.title.as_str(), d.overview.as_deref()), ("Picked", Some("by hand")), "a match is never undone");
    let q = ItemQuery { library_id: Some(l), ..Default::default() };
    let items = lib.items(user, q.clone()).await.unwrap().items;
    let thor = items.iter().find(|i| i.title == "Thor").expect("Thor split out");
    let iron = items.iter().find(|i| i.title == "Iron Man").expect("Iron Man keeps the item");
    assert_eq!(iron.id, merged);
    assert_eq!(thor.state.position_ms, 60000, "progress on the merged item goes along");
    let placement: i64 = lib
        .db()
        .read(move |c| c.query_row("SELECT placement FROM libraries WHERE id = ?1", [l], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(placement, 0, "a folder was offline: not marked repaired");

    // The next scan repairs again (the folder is still offline), and changes nothing.
    lib.db()
        .write(|t| t.execute("UPDATE items SET overview = 'fetched again' WHERE kind = 'movie'", []))
        .await
        .unwrap();
    lib.scan_now(l).await.unwrap();
    let after = lib.items(user, q).await.unwrap().items;
    assert_eq!(after.len(), items.len());
    for i in &after {
        assert_eq!(lib.item(user, i.id).await.unwrap().overview.as_deref(), Some("fetched again"), "{}", i.title);
    }

    // A trailer scanned before its movie finds it once the movie comes.
    let late = movies.join("Late (2020)");
    std::fs::create_dir_all(&late).unwrap();
    std::fs::write(late.join("Late-trailer.mkv"), b"x").unwrap();
    lib.scan_now(l).await.unwrap();
    std::fs::write(late.join("Late.mkv"), b"x").unwrap();
    lib.scan_now(l).await.unwrap();
    let owner: String = lib
        .db()
        .read(|c| {
            c.query_row(
                "SELECT i.title FROM files f JOIN item_files x ON x.file_id = f.id JOIN items i ON i.id = x.item_id \
                 WHERE f.path LIKE '%Late-trailer.mkv'",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(owner, "Late");
    let _ = std::fs::remove_dir_all(dir);
}
