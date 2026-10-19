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
    wait_for(&l, "added via API", 20, |sn| find(sn, s.hash).is_some()).await;

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
