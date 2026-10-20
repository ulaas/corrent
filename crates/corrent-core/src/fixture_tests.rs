//! STABILITY.md D9: profiles written by each schema version (`tests/fixtures/profile-v*`) load
//! without losing anything: settings, store, feed history and the engine's torrents.
//!
//! Add a fixture with every release that changes a format:
//! `cargo test -p corrent-core --lib write_current_fixture -- --ignored` writes
//! `profile-v<current>/`; check it in.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::config::{Paths, Settings};
use crate::engine::testkit::{self, wait_for};
use crate::engine::types::{AddSource, InfoHash};
use crate::engine::{AddRequest, EngineCommand};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The download folder, as written into the fixture files (forward slashes are valid paths on
/// every OS and need no escaping in TOML or JSON).
const DL: &str = "{{DL}}";

fn slashes(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn is_text(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "json" || e == "toml")
}

/// Copy `from` into `to`, replacing `a` with `b` in the text files.
fn copy_tree(from: &Path, to: &Path, a: &str, b: &str) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst, a, b);
        } else if is_text(&src) {
            std::fs::write(&dst, std::fs::read_to_string(&src).unwrap().replace(a, b)).unwrap();
        } else if src.extension().is_none_or(|e| e != "md") {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

/// A tiny paused single-file torrent (made-up piece hash: nothing to download).
fn tiny(i: u8) -> Vec<u8> {
    let name = format!("fixture-{i}.bin");
    let mut b = b"d4:infod6:lengthi16384e4:name".to_vec();
    b.extend_from_slice(format!("{}:{name}", name.len()).as_bytes());
    b.extend_from_slice(b"12:piece lengthi16384e6:pieces20:");
    b.extend_from_slice(&[i.wrapping_add(40); 20]);
    b.extend_from_slice(b"ee");
    b
}

/// Writes `profile-v<SETTINGS_VERSION>` from a real run: non-default settings, a category with
/// its own folder, three torrents with categories, tags and one paused, a feed with a rule and a
/// downloaded article.
#[test]
#[ignore = "writes tests/fixtures; run by hand when a format changes"]
fn write_current_fixture() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = testkit::temp_dir("fixture-write");
    let dl = dir.join("dl");
    let mut s = testkit::settings(&dir);
    s.speed.download_limit = 512;
    s.behavior.skip_add_dialog = true;
    s.queue.max_active_downloads = 4;
    s.rss.feeds.push(crate::rss::FeedConfig {
        url: "https://feeds.example/tv.xml".into(),
        name: "TV".into(),
        enabled: true,
    });
    s.rss.rules.push(crate::rss::rules::RssRule {
        name: "Show".into(),
        must_contain: "Show".into(),
        ..Default::default()
    });
    let paths = Paths { data_dir: dir.join("data"), portable: true };
    s.save(&paths).unwrap();
    rt.block_on(async {
        let l = testkit::start_with(&dir, s.clone());
        let mut cats = std::collections::BTreeMap::new();
        cats.insert(
            "Movies".to_string(),
            crate::store::CategoryInfo { save_path: Some(dl.join("movies")), down_limit_kib: 300, up_limit_kib: 0 },
        );
        l.engine.send(EngineCommand::SetCategories(cats));
        for (i, (cat, tags, paused)) in
            [("Movies", vec!["hd"], false), ("TV", vec!["hd", "new"], true), ("TV", vec![], true)]
                .into_iter()
                .enumerate()
        {
            l.engine.send(EngineCommand::Add(Box::new(AddRequest {
                source: AddSource::Bytes { name: "f.torrent".into(), bytes: tiny(i as u8) },
                torrent_bytes: None,
                save_path: dl.clone(),
                create_subfolder: false,
                category: Some(cat.into()),
                tags: tags.into_iter().map(String::from).collect(),
                paused,
                only_files: None,
                add_to_top: false,
                initial_peers: None,
                added_by: None,
            })));
        }
        wait_for(&l, "the fixture torrents", 30, |sn| {
            sn.torrents.len() == 3 && sn.torrents.iter().all(|t| !t.pending && t.category.is_some())
        })
        .await;
        testkit::stop(l).await;
    });
    // Feed history: a fetched feed and an article marked downloaded.
    {
        let mut rss = crate::rss::Rss::load(paths.rss_file());
        let feed = crate::rss::feed::ParsedFeed {
            title: Some("TV".into()),
            articles: vec![crate::rss::feed::Article {
                guid: "show-s01e01".into(),
                title: "Show S01E01 1080p".into(),
                torrent: Some("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into()),
                page: None,
                published: Some(1_700_000_000),
                size: Some(1_000_000),
            }],
        };
        let _ = rss.fetched("https://feeds.example/tv.xml", Ok(feed), &s.rss);
        rss.mark_downloaded("https://feeds.example/tv.xml", "show-s01e01", &s.rss);
        rss.flush();
    }
    let out = fixtures().join(format!("profile-v{}", crate::migrate::SETTINGS_VERSION));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(out.join("session")).unwrap();
    let dl_text = slashes(&dl);
    let dl_escaped = dl.display().to_string().replace('\\', "\\\\");
    let put = |rel: &str| {
        let src = paths.data_dir.join(rel);
        if is_text(&src) {
            let t = std::fs::read_to_string(&src).unwrap();
            let t = t.replace(&dl_escaped, DL).replace(&dl.display().to_string(), DL).replace(&dl_text, DL);
            // Paths under the folder in the fixtures' form (forward slashes, valid on every OS).
            let t = t.replace(&format!("{DL}\\\\"), &format!("{DL}/")).replace(&format!("{DL}\\"), &format!("{DL}/"));
            std::fs::write(out.join(rel), t).unwrap();
        } else {
            std::fs::copy(&src, out.join(rel)).unwrap();
        }
    };
    for f in ["settings.toml", "store.json", "rss.json", "session/session.json"] {
        put(f);
    }
    for e in std::fs::read_dir(paths.session_dir()).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.ends_with(".torrent") || n.ends_with(".bitv") {
            put(&format!("session/{n}"));
        }
    }
    std::fs::write(
        out.join("README.md"),
        "Written by `fixture_tests::write_current_fixture` (crates/corrent-core). `{{DL}}` stands for \
         the download folder.\n",
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// Where `a` (a fixture file's value) has something `b` (what Corrent loaded and would save)
/// doesn't, or a different value.
fn missing(a: &Value, b: &Value, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => a.iter().find_map(|(k, v)| match b.get(k) {
            Some(w) => missing(v, w, &format!("{at}.{k}")),
            None => Some(format!("{at}.{k}")),
        }),
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            a.iter().zip(b).enumerate().find_map(|(i, (v, w))| missing(v, w, &format!("{at}[{i}]")))
        }
        // Paths may come back with the OS's separator.
        (Value::String(x), Value::String(y)) if x.replace('\\', "/") == y.replace('\\', "/") => None,
        (Value::Number(x), Value::Number(y)) if x.as_f64() == y.as_f64() => None,
        _ if a == b => None,
        _ => Some(format!("{at}: {a} became {b}")),
    }
}

fn without_version(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("schema_version");
    }
    v
}

#[test]
fn every_fixture_loads_losslessly() {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(fixtures())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("profile-v")))
        .collect();
    dirs.sort();
    assert!(dirs.len() >= 2, "fixtures: {dirs:?}");
    let rt = tokio::runtime::Runtime::new().unwrap();
    for fixture in dirs {
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let dir = testkit::temp_dir(&format!("fixture-{name}"));
        let dl = dir.join("dl");
        let paths = Paths { data_dir: dir.join("data"), portable: true };
        copy_tree(&fixture, &paths.data_dir, DL, &slashes(&dl));
        let read = |f: &str| std::fs::read_to_string(paths.data_dir.join(f)).unwrap();
        let settings_in: Value = serde_json::to_value(read("settings.toml").parse::<toml::Table>().unwrap()).unwrap();
        let store_in: Value = serde_json::from_str(&read("store.json")).unwrap();
        let rss_in: Value = serde_json::from_str(&read("rss.json")).unwrap();

        let (settings, _, warning) = Settings::load(&paths);
        assert_eq!(settings.schema_version, crate::migrate::SETTINGS_VERSION, "{name}");
        let settings_out = serde_json::to_value(&settings).unwrap();
        if let Some(m) = missing(&without_version(settings_in), &settings_out, "settings") {
            panic!("{name}: {m} (warning {warning:?})");
        }
        let store = crate::store::Store::load(paths.store_file());
        assert!(store.warning.is_none(), "{name}: {:?}", store.warning);
        let store_out = serde_json::to_value(&store.data).unwrap();
        if let Some(m) = missing(&without_version(store_in.clone()), &store_out, "store") {
            panic!("{name}: {m}");
        }
        drop(store);
        let mut rss = crate::rss::Rss::load(paths.rss_file());
        assert!(rss.warning.is_none(), "{name}: {:?}", rss.warning);
        rss.rebuild(&settings.rss);
        let view = rss.view();
        let article = view.feeds.iter().flat_map(|f| f.articles.iter()).find(|a| a.article.guid == "show-s01e01");
        assert!(article.is_some_and(|a| a.downloaded), "{name}: the downloaded article");
        drop(rss);
        assert!(rss_in.get("feeds").is_some(), "{name}: rss.json has feeds");
        if name == "profile-v0" {
            let names: Vec<String> = std::fs::read_dir(&paths.data_dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            for f in ["settings.toml.v0-", "store.json.v0-"] {
                assert!(names.iter().any(|n| n.starts_with(f)), "{name}: a {f}* backup in {names:?}");
            }
        }

        // The engine has every torrent of the store back, with its category.
        let hashes: Vec<InfoHash> =
            store_in["torrents"].as_object().unwrap().keys().map(|h| InfoHash::from_hex(h).unwrap()).collect();
        rt.block_on(async {
            let l = testkit::start_with(&dir, settings);
            let snap = wait_for(&l, "the fixture's torrents", 30, |sn| {
                hashes.iter().all(|h| sn.torrents.iter().any(|t| t.hash == *h && !t.pending && t.busy.is_none()))
            })
            .await;
            for t in &snap.torrents {
                assert_ne!(t.state, crate::engine::types::TorrentState::Error, "{name}: {} {:?}", t.name, t.error);
                let want = store_in["torrents"][t.hash.to_hex()]["category"].as_str();
                assert_eq!(t.category.as_deref(), want, "{name}: {}", t.name);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            testkit::stop(l).await;
        });
        let _ = std::fs::remove_dir_all(dir);
    }
}
