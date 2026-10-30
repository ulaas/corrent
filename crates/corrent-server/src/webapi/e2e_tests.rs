//! The qBittorrent-compatible API against a real engine over loopback.

use std::sync::Arc;

use corrent_core::engine::testkit::*;
use corrent_core::engine::types::TorrentState;

/// What Sonarr/Radarr do with a qBittorrent client, against Corrent's web API.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webapi_behaves_like_qbittorrent_for_sonarr() {
    use crate::webapi::{WebApiServer, WebApiSettings, hash_password};
    let dir = temp_dir("webapi");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let cfg = WebApiSettings {
        enabled: true,
        port: 0,
        allow_lan: false,
        username: "sonarr".into(),
        password_hash: hash_password("correct horse"),
    };
    let shared = Arc::new(arc_swap::ArcSwap::from_pointee(settings(&dir)));
    let server = WebApiServer::start(cfg, l.engine.clone(), shared).await.unwrap();
    let base = format!("http://127.0.0.1:{}/api/v2", server.addr.port());
    let http = reqwest::Client::new();

    // The web UI is served without a session, with protective headers.
    let root = format!("http://127.0.0.1:{}", server.addr.port());
    let r = http.get(format!("{root}/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'none'"));
    assert_eq!(r.headers()["x-frame-options"], "DENY");
    assert!(r.text().await.unwrap().contains("/webui/app.js"));
    let r = http.get(format!("{root}/webui/app.js")).send().await.unwrap();
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/javascript"));
    assert!(r.text().await.unwrap().contains("textContent"));

    // Wrong password, then no session.
    let r = http
        .post(format!("{base}/auth/login"))
        .form(&[("username", "sonarr"), ("password", "nope")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "Fails.");
    let r = http.get(format!("{base}/app/version")).send().await.unwrap();
    assert_eq!(r.status(), 403);

    let r = http
        .post(format!("{base}/auth/login"))
        .form(&[("username", "sonarr"), ("password", "correct horse")])
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    assert!(cookie.starts_with("SID="));
    assert_eq!(r.text().await.unwrap(), "Ok.");
    let get = |path: &str| http.get(format!("{base}/{path}")).header("Cookie", &cookie).send();
    let post = |path: &str, form: Vec<(&str, String)>| {
        http.post(format!("{base}/{path}")).header("Cookie", &cookie).form(&form).send()
    };

    assert_eq!(get("app/webapiVersion").await.unwrap().text().await.unwrap(), "2.9.3");
    assert!(get("app/version").await.unwrap().text().await.unwrap().starts_with("v4."));
    let prefs: serde_json::Value = get("app/preferences").await.unwrap().json().await.unwrap();
    assert!(prefs["save_path"].as_str().unwrap().ends_with("dl"));
    assert_eq!(prefs["queueing_enabled"], false);

    post("torrents/createCategory", vec![("category", "tv-sonarr".into()), ("savePath", "".into())]).await.unwrap();
    wait_for(&l, "category", 10, |sn| sn.categories.contains_key("tv-sonarr")).await;
    let cats: serde_json::Value = get("torrents/categories").await.unwrap().json().await.unwrap();
    assert_eq!(cats["tv-sonarr"]["name"], "tv-sonarr");

    // Upload a .torrent as multipart, like Sonarr does.
    let boundary = "corrent-test-boundary";
    let mut body = Vec::new();
    for (k, v) in [("category", "tv-sonarr"), ("paused", "false")] {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"data.torrent\"\r\nContent-Type: application/x-bittorrent\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend(&s.bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    let r = http
        .post(format!("{base}/torrents/add"))
        .header("Cookie", &cookie)
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "Ok.");
    // Listed before the add finishes: wait for the category too.
    wait_for(&l, "added via API", 20, |sn| {
        find(sn, s.hash).is_some_and(|t| t.category.as_deref() == Some("tv-sonarr"))
    })
    .await;

    let list: Vec<serde_json::Value> = get("torrents/info?category=tv-sonarr").await.unwrap().json().await.unwrap();
    assert_eq!(list.len(), 1);
    let t = &list[0];
    assert_eq!(t["hash"], s.hash.to_hex());
    assert_eq!(t["name"], "data.bin");
    assert_eq!(t["category"], "tv-sonarr");
    assert_eq!(t["size"], SIZE as u64);
    assert!(t["content_path"].as_str().unwrap().contains("data.bin"));
    assert!(
        ["downloading", "stalledDL", "metaDL", "checkingDL", "queuedDL", "uploading", "stalledUP", "checkingUP"]
            .contains(&t["state"].as_str().unwrap()),
        "{t}"
    );
    let empty: Vec<serde_json::Value> = get("torrents/info?category=movies").await.unwrap().json().await.unwrap();
    assert!(empty.is_empty());
    let props: serde_json::Value =
        get(&format!("torrents/properties?hash={}", s.hash.to_hex())).await.unwrap().json().await.unwrap();
    assert!(props["save_path"].as_str().unwrap().ends_with("dl"));
    let main: serde_json::Value = get("sync/maindata").await.unwrap().json().await.unwrap();
    assert!(main["torrents"][s.hash.to_hex()].is_object());

    let h = s.hash.to_hex();
    let r = post(
        "torrents/setShareLimits",
        vec![("hashes", h.clone()), ("ratioLimit", "2".into()), ("seedingTimeLimit", "-2".into())],
    )
    .await
    .unwrap();
    assert_eq!(r.status(), 200);
    post("torrents/pause", vec![("hashes", h.clone())]).await.unwrap();
    wait_for(&l, "paused via API", 20, |sn| find(sn, s.hash).is_some_and(|t| t.state == TorrentState::Paused)).await;
    let list: Vec<serde_json::Value> = get("torrents/info?filter=paused").await.unwrap().json().await.unwrap();
    assert_eq!(list[0]["state"], "pausedDL");
    post("torrents/delete", vec![("hashes", h.clone()), ("deleteFiles", "true".into())]).await.unwrap();
    wait_for(&l, "deleted via API", 20, |sn| find(sn, s.hash).is_none()).await;

    // Cross-site requests and foreign Host names are refused even with a session.
    let r = http
        .get(format!("{base}/app/version"))
        .header("Cookie", &cookie)
        .header("Origin", "http://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .get(format!("{base}/app/version"))
        .header("Cookie", &cookie)
        .header("Host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    drop(server);
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A stuck or flooded engine: adds are refused with 503 instead of queueing without end
/// (STABILITY.md F11).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_answers_503_when_the_engine_is_busy() {
    use corrent_core::engine::{API_ADD_QUEUE, Backend, BackendIo, EngineCommand, EngineHandle, Wake};

    use crate::webapi::{WebApiServer, WebApiSettings, hash_password};
    let dir = temp_dir("webapi-busy");
    let stuck: Backend = Arc::new(|io: BackendIo| {
        Box::pin(async move {
            let _commands = io.commands;
            std::future::pending::<()>().await
        })
    });
    let (events, _rx) = std::sync::mpsc::channel();
    let paths = corrent_core::config::Paths { data_dir: dir.join("data"), portable: true };
    let engine = EngineHandle::with_backend(&tokio::runtime::Handle::current(), paths, Wake::noop(), events, stuck);
    let cfg = WebApiSettings {
        enabled: true,
        port: 0,
        allow_lan: false,
        username: "u".into(),
        password_hash: hash_password("pw"),
    };
    let shared = Arc::new(arc_swap::ArcSwap::from_pointee(settings(&dir)));
    let server = WebApiServer::start(cfg, engine.clone(), shared).await.unwrap();
    let base = format!("http://127.0.0.1:{}/api/v2", server.addr.port());
    let http = reqwest::Client::new();
    let r =
        http.post(format!("{base}/auth/login")).form(&[("username", "u"), ("password", "pw")]).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let add = || {
        http.post(format!("{base}/torrents/add"))
            .header("Cookie", &cookie)
            .form(&[("urls", "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567")])
            .send()
    };
    for _ in 0..API_ADD_QUEUE {
        assert_eq!(add().await.unwrap().status(), 200);
    }
    assert_eq!(add().await.unwrap().status(), 503);
    assert_eq!(engine.queued_adds(), API_ADD_QUEUE);
    // Other commands still fill the queue up to its end, then are refused too.
    while engine.send(EngineCommand::PauseAll) {}
    assert!(engine.refused() >= 1);
    assert_eq!(add().await.unwrap().status(), 503);
    let _ = std::fs::remove_dir_all(dir);
}

/// STABILITY.md Z4: mutated requests to every route, against a real engine. No answer is a server
/// error (5xx) other than 503 (busy), and each comes within 5 s. Fields that would reach the
/// network (`urls`) or folders outside the test (`savepath`…) are left out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn webapi_inputs_are_refused_cleanly() {
    use crate::webapi::{WebApiServer, WebApiSettings, hash_password};
    const ROUTES: &[(&str, bool)] = &[
        ("app/version", false),
        ("app/preferences", false),
        ("transfer/info", false),
        ("sync/maindata", false),
        ("torrents/info", false),
        ("torrents/properties", false),
        ("torrents/files", false),
        ("torrents/categories", false),
        ("torrents/tags", false),
        ("torrents/add", true),
        ("torrents/pause", true),
        ("torrents/resume", true),
        ("torrents/delete", true),
        ("torrents/recheck", true),
        ("torrents/setCategory", true),
        ("torrents/createCategory", true),
        ("torrents/editCategory", true),
        ("torrents/removeCategories", true),
        ("torrents/addTags", true),
        ("torrents/removeTags", true),
        ("torrents/setShareLimits", true),
        ("torrents/setForceStart", true),
        ("torrents/setDownloadLimit", true),
        ("torrents/setUploadLimit", true),
        ("torrents/topPrio", true),
        ("torrents/increasePrio", true),
        ("auth/login", true),
    ];
    const KEYS: &[&str] = &[
        "hashes",
        "hash",
        "category",
        "categories",
        "tags",
        "paused",
        "stopped",
        "deleteFiles",
        "ratioLimit",
        "seedingTimeLimit",
        "inactiveSeedingTimeLimit",
        "limit",
        "value",
        "filter",
        "sort",
        "reverse",
        "offset",
        "rid",
        "skip_checking",
        "root_folder",
        "contentLayout",
        "upLimit",
        "dlLimit",
        "sequentialDownload",
        "firstLastPiecePrio",
        "autoTMM",
        "rename",
        "username",
        "password",
    ];
    const VALUES: &[&str] = &[
        "all",
        "true",
        "false",
        "1",
        "0",
        "-1",
        "-2",
        "1.5",
        "NaN",
        "inf",
        "1e308",
        "",
        "tv",
        "a,b",
        "a|b",
        "08ada5a7a6183aae1e09d831df6748d566095a10",
        "name",
        "size",
        "Subfolder",
    ];
    let dir = temp_dir("webapi-fuzz");
    let s = seeder(&dir).await;
    let l = start(&dir);
    let cfg = WebApiSettings {
        enabled: true,
        port: 0,
        allow_lan: false,
        username: "u".into(),
        password_hash: hash_password("p"),
    };
    let shared = Arc::new(arc_swap::ArcSwap::from_pointee(settings(&dir)));
    let server = WebApiServer::start(cfg, l.engine.clone(), shared).await.unwrap();
    let base = format!("http://127.0.0.1:{}/api/v2", server.addr.port());
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap();
    let r = http.post(format!("{base}/auth/login")).form(&[("username", "u"), ("password", "p")]).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();

    // Built up front (the generator isn't Send across awaits), then sent.
    let mut cases = Vec::new();
    corrent_fuzz::run("webapi_requests", corrent_fuzz::iterations(300), |r| {
        let (route, post) = *r.pick(ROUTES);
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for _ in 0..r.below(5) {
            let v = if r.chance(70) { r.pick(VALUES).to_string() } else { r.case(VALUES, 40) };
            form.append_pair(r.pick(KEYS), &v);
        }
        let raw = if r.chance(50) { r.mutate(&form.finish()) } else { form.finish() };
        // What the server will decode, without the fields this test must not send.
        let safe: String = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(url::form_urlencoded::parse(raw.as_bytes()).filter(|(k, _)| {
                let k = k.to_ascii_lowercase();
                !k.contains("url") && !k.contains("path") && k != "savepath"
            }))
            .finish();
        let torrent = (route == "torrents/add" && r.chance(70)).then(|| r.mutate_bytes(&s.bytes));
        cases.push((route, post, safe, torrent));
    });
    for (route, post, form, torrent) in cases {
        let url = format!("{base}/{route}");
        let req = match (post, torrent) {
            (true, Some(t)) => {
                let mut body = Vec::new();
                for (k, v) in url::form_urlencoded::parse(form.as_bytes()) {
                    body.extend(
                        format!("--B\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes(),
                    );
                }
                body.extend(
                    b"--B\r\nContent-Disposition: form-data; name=\"torrents\"; filename=\"t.torrent\"\r\n\r\n",
                );
                body.extend(t);
                body.extend(b"\r\n--B--\r\n");
                http.post(&url).header("Content-Type", "multipart/form-data; boundary=B").body(body)
            }
            (true, None) => {
                http.post(&url).header("Content-Type", "application/x-www-form-urlencoded").body(form.clone())
            }
            (false, _) => http.get(format!("{url}?{form}")),
        };
        let resp = req.header("Cookie", &cookie).send().await.unwrap_or_else(|e| panic!("{route} {form:?}: {e}"));
        let status = resp.status().as_u16();
        assert!(status < 500 || status == 503, "{route} {form:?}: {status}");
    }
    assert!(l.engine.is_alive());
    stop(l).await;
    let _ = std::fs::remove_dir_all(dir);
}
