//! The Jellyfin surface driven the way the apps drive it (requests modelled on Jellyfin Web,
//! the Kotlin SDK and the Kodi add-on).

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::ids;
use crate::ServerConfig;
use crate::api_tests::{admin_token, harness, send};

const WEB: &str = r#"MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="dev-abc", Version="10.10.7""#;

async fn jf(
    app: &Router,
    method: Method,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, HeaderMap) {
    let mut b = Request::builder().method(method).uri(uri).header(header::HOST, "127.0.0.1:8443");
    if let Some(a) = auth {
        b = b.header(header::AUTHORIZATION, a);
    }
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = b.body(body).unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:50000".parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v, headers)
}

fn with_token(t: &str) -> String {
    format!(r#"{WEB}, Token="{t}""#)
}

async fn login(app: &Router) -> (String, String) {
    let (s, v, _) = jf(
        app,
        Method::POST,
        "/jellyfin/Users/AuthenticateByName",
        Some(WEB),
        Some(json!({ "Username": "admin", "Pw": "correct horse" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    (v["AccessToken"].as_str().unwrap().to_string(), v["User"]["Id"].as_str().unwrap().to_string())
}

#[test]
fn authorization_header() {
    let d = super::parse_authorization(r#"MediaBrowser Client="Kodi, the add-on", Device="Living%20room", DeviceId="x1", Version="1.0", Token="ct_abc""#).unwrap();
    assert_eq!((d.client.as_str(), d.device.as_str(), d.device_id.as_str()), ("Kodi, the add-on", "Living room", "x1"));
    assert_eq!(d.token.as_deref(), Some("ct_abc"));
    assert!(super::parse_authorization("Bearer ct_abc").is_none());
    assert!(super::parse_authorization(r#"emby Token=ct_x"#).is_some_and(|d| d.token.as_deref() == Some("ct_x")));
    let q = super::Q::parse(Some("ParentId=1&parentid=2&IncludeItemTypes=Movie,Series&Limit=5"));
    assert_eq!((q.get("parentid"), q.i64("limit")), (Some("1"), Some(5)));
    assert_eq!(q.list("includeitemtypes"), vec!["Movie", "Series"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_and_login() {
    let h = harness("jf-login", ServerConfig { tls: false, jellyfin_api: true, ..Default::default() }).await;
    let app = &h.app;
    let _ = admin_token(app).await;
    for path in ["/jellyfin/System/Info/Public", "/jellyfin/system/info/public"] {
        let (s, v, _) = jf(app, Method::GET, path, None, None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["ProductName"], "Jellyfin Server");
        assert_eq!(v["Id"].as_str().unwrap().len(), 32);
        let parts: Vec<u32> = v["Version"].as_str().unwrap().split('.').map(|p| p.parse().unwrap()).collect();
        assert!(parts.len() == 3 && parts[0] >= 12, "{v}");
    }
    // The sign-in name, even with a display name set.
    let admin_id = h.st.identity.users().await.unwrap()[0].id;
    let upd = corrent_proto::auth::UserUpdate { display_name: Some("The Admin".into()), ..Default::default() };
    h.st.identity.update_user(admin_id, upd).await.unwrap();
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/Users/Public", None, None).await;
    assert_eq!(v[0]["Name"], "admin");
    assert_eq!(jf(app, Method::GET, "/jellyfin/QuickConnect/Enabled", None, None).await.1, json!(false));
    assert_eq!(jf(app, Method::GET, "/jellyfin/System/Info", None, None).await.0, StatusCode::UNAUTHORIZED);
    let (s, _, _) = jf(
        app,
        Method::POST,
        "/jellyfin/Users/AuthenticateByName",
        Some(WEB),
        Some(json!({ "Username": "admin", "Pw": "nope" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (token, uid) = login(app).await;
    assert_eq!(ids::dec(&uid).map(|(k, _)| k), Some(ids::USER));
    // Every way apps send the token.
    let me =
        |auth: Option<String>, uri: &'static str| async move { jf(app, Method::GET, uri, auth.as_deref(), None).await };
    assert_eq!(me(Some(with_token(&token)), "/jellyfin/Users/Me").await.1["Name"], "admin");
    assert_eq!(
        me(None, Box::leak(format!("/jellyfin/Users/Me?api_key={token}").into_boxed_str())).await.0,
        StatusCode::OK
    );
    assert_eq!(
        me(None, Box::leak(format!("/jellyfin/users/me?ApiKey={token}").into_boxed_str())).await.0,
        StatusCode::OK
    );
    assert_eq!(me(Some(format!("Bearer {token}")), "/jellyfin/Users/Me").await.0, StatusCode::UNAUTHORIZED);
    let (s, v, _) = jf(app, Method::GET, "/jellyfin/Sessions?DeviceId=dev-abc", Some(&with_token(&token)), None).await;
    assert_eq!((s, v[0]["DeviceId"].clone()), (StatusCode::OK, json!("dev-abc")));
    // Signing in again from the same app replaces its device.
    let (token2, _) = login(app).await;
    let devices = h.st.identity.devices(None).await.unwrap();
    assert_eq!(devices.iter().filter(|d| d.client == "Jellyfin Web 10.10.7").count(), 1, "{devices:?}");
    assert_eq!(me(Some(with_token(&token)), "/jellyfin/Users/Me").await.0, StatusCode::UNAUTHORIZED);
    // Preferences round-trip.
    let a = with_token(&token2);
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/DisplayPreferences/usersettings?client=emby", Some(&a), None).await;
    assert_eq!(v["SortBy"], "SortName");
    let doc = json!({ "Id": "usersettings", "CustomPrefs": { "homesection0": "resume" } });
    assert_eq!(
        jf(app, Method::POST, "/jellyfin/DisplayPreferences/usersettings?client=emby", Some(&a), Some(doc)).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/DisplayPreferences/usersettings?client=emby", Some(&a), None).await;
    assert_eq!(v["CustomPrefs"]["homesection0"], "resume");
    let cfg = json!({ "SubtitleMode": "Always" });
    assert_eq!(
        jf(app, Method::POST, "/jellyfin/Users/Configuration", Some(&a), Some(cfg)).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        jf(app, Method::GET, "/jellyfin/Users/Me", Some(&a), None).await.1["Configuration"]["SubtitleMode"],
        "Always"
    );
    // Signing out ends the token.
    assert_eq!(jf(app, Method::POST, "/jellyfin/Sessions/Logout", Some(&a), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(jf(app, Method::GET, "/jellyfin/Users/Me", Some(&a), None).await.0, StatusCode::UNAUTHORIZED);
    // Unknown endpoints answer softly.
    let (token3, _) = login(app).await;
    assert_eq!(jf(app, Method::GET, "/jellyfin/Plugins", Some(&with_token(&token3)), None).await.1, json!([]));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_by_default() {
    let h = harness("jf-off", ServerConfig { tls: false, ..Default::default() }).await;
    assert_eq!(jf(&h.app, Method::GET, "/jellyfin/System/Info/Public", None, None).await.0, StatusCode::NOT_FOUND);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browse_photos_and_user_data() {
    let h = harness("jf-browse", ServerConfig { tls: false, jellyfin_api: true, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let pics = h.dir.join("Pics");
    std::fs::create_dir_all(&pics).unwrap();
    for (n, c) in [("forest", [10, 120, 30]), ("sea", [10, 40, 200])] {
        image::RgbImage::from_pixel(64, 48, image::Rgb(c)).save(pics.join(format!("{n}.png"))).unwrap();
    }
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Pics", "kind": "photos", "folders": [pics] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, v, _) = crate::api_tests::get(app, &format!("/api/v1/items?library_id={lib}&kind=photo"), &admin).await;
        if v["total"] == 2 {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (token, uid) = login(app).await;
    let a = with_token(&token);
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/UserViews", Some(&a), None).await;
    assert_eq!(v["Items"][0]["CollectionType"], "photos");
    let view = v["Items"][0]["Id"].as_str().unwrap().to_string();
    let (_, legacy, _) = jf(app, Method::GET, &format!("/jellyfin/Users/{uid}/Views"), Some(&a), None).await;
    assert_eq!(legacy["TotalRecordCount"], 1);
    let (s, v, _) = jf(
        app,
        Method::GET,
        &format!("/jellyfin/Items?ParentId={view}&Recursive=true&IncludeItemTypes=Photo&SortBy=SortName&SortOrder=Ascending&StartIndex=0&Limit=1"),
        Some(&a),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        (v["TotalRecordCount"].clone(), v["Items"][0]["Type"].clone(), v["Items"][0]["Name"].clone()),
        (json!(2), json!("Photo"), json!("forest"))
    );
    let item = v["Items"][0]["Id"].as_str().unwrap().to_string();
    let tag = v["Items"][0]["ImageTags"]["Primary"].as_str().unwrap().to_string();
    // Kotlin SDK style (camelCase) and Kodi's legacy path.
    let (_, v2, _) = jf(
        app,
        Method::GET,
        &format!("/jellyfin/Items?parentId={view}&recursive=true&includeItemTypes=Photo"),
        Some(&a),
        None,
    )
    .await;
    assert_eq!(v2["TotalRecordCount"], 2);
    let (_, d, _) = jf(app, Method::GET, &format!("/jellyfin/Users/{uid}/Items/{item}"), Some(&a), None).await;
    assert_eq!((d["Id"].as_str(), d["MediaType"].as_str()), (Some(item.as_str()), Some("Photo")));
    // Art by tag without credentials; without the tag only signed in.
    let (s, _, hd) =
        jf(app, Method::GET, &format!("/jellyfin/Items/{item}/Images/Primary?tag={tag}&maxWidth=200"), None, None)
            .await;
    assert_eq!(s, StatusCode::OK);
    assert!(hd[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));
    assert_eq!(
        jf(app, Method::GET, &format!("/jellyfin/Items/{item}/Images/Primary"), None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        jf(app, Method::GET, &format!("/jellyfin/Items/{item}/Images/Primary"), Some(&a), None).await.0,
        StatusCode::OK
    );
    // Favorites and played marks.
    let (s, v, _) = jf(app, Method::POST, &format!("/jellyfin/UserFavoriteItems/{item}"), Some(&a), None).await;
    assert_eq!((s, v["IsFavorite"].clone()), (StatusCode::OK, json!(true)));
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/Items?Recursive=true&Filters=IsFavorite", Some(&a), None).await;
    assert_eq!(v["TotalRecordCount"], 1);
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/Items?Recursive=true&SearchTerm=se", Some(&a), None).await;
    assert_eq!(v["Items"][0]["Name"], "sea");
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/Items?Ids={item}"), Some(&a), None).await;
    assert_eq!(v["TotalRecordCount"], 1);
    // The root folder lists the views; garbage ids are 404, never 500.
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/Users/{uid}/Items"), Some(&a), None).await;
    assert_eq!(v["Items"][0]["Id"], view);
    for bad in ["zzz", "0100000000000000ffffffffffffffff", "f1d2d2f924e986ac86fdf7b36c94bcdf"] {
        assert_eq!(
            jf(app, Method::GET, &format!("/jellyfin/Items/{bad}"), Some(&a), None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    // Administrators may read other users' lists.
    assert_eq!(
        jf(app, Method::GET, &format!("/jellyfin/Users/{}/Items", ids::enc(ids::USER, 999)), Some(&a), None).await.0,
        StatusCode::OK
    );
    h.stop().await;
}

fn encode(path: &std::path::Path, secs: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let job = corrent_mpv::encode::EncodeJob {
        input: format!("av://lavfi:testsrc=duration={secs}:size=320x240:rate=25"),
        audio_file: Some(format!("av://lavfi:sine=duration={secs}")),
        output: path.display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=ultrafast".into()),
            ("oac".into(), "aac".into()),
        ],
    };
    corrent_mpv::encode::run(&job, std::time::Duration::from_secs(60), &mut |_| true).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shows_next_up_and_playback() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let h = harness("jf-shows", ServerConfig { tls: false, jellyfin_api: true, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let season = h.dir.join("Shows/Test Show (2020)/Season 1");
    encode(&season.join("Test Show - S01E01.mkv"), 12);
    encode(&season.join("Test Show - S01E02.mkv"), 4);
    std::fs::write(season.join("Test Show - S01E01.en.srt"), "1\n00:00:01,000 --> 00:00:02,000\nHello\n").unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Shows", "kind": "shows", "folders": [h.dir.join("Shows")] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        let (_, v, _) =
            crate::api_tests::get(app, &format!("/api/v1/items?library_id={lib}&kind=episode"), &admin).await;
        if v["total"] == 2 && h.st.hls_encoder.lock().unwrap().is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "scan: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (token, _) = login(app).await;
    let a = with_token(&token);
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/UserViews", Some(&a), None).await;
    assert_eq!(v["Items"][0]["CollectionType"], "tvshows");
    let view = v["Items"][0]["Id"].as_str().unwrap().to_string();
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/Items?ParentId={view}"), Some(&a), None).await;
    assert_eq!((v["Items"][0]["Type"].clone(), v["Items"][0]["Name"].clone()), (json!("Series"), json!("Test Show")));
    let show = v["Items"][0]["Id"].as_str().unwrap().to_string();
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/Shows/{show}/Seasons"), Some(&a), None).await;
    assert_eq!((v["TotalRecordCount"].clone(), v["Items"][0]["SeriesId"].clone()), (json!(1), json!(show)));
    let season_id = v["Items"][0]["Id"].as_str().unwrap().to_string();
    let (_, v, _) = jf(
        app,
        Method::GET,
        &format!("/jellyfin/Shows/{show}/Episodes?SeasonId={season_id}&Fields=MediaSources"),
        Some(&a),
        None,
    )
    .await;
    assert_eq!(v["TotalRecordCount"], 2);
    let e1 = &v["Items"][0];
    assert_eq!(
        (e1["IndexNumber"].clone(), e1["ParentIndexNumber"].clone(), e1["SeriesId"].clone()),
        (json!(1), json!(1), json!(show))
    );
    assert!(e1["MediaSources"][0]["Id"].is_string(), "{e1}");
    let (ep1, ep2) = (e1["Id"].as_str().unwrap().to_string(), v["Items"][1]["Id"].as_str().unwrap().to_string());
    let (_, v, _) = jf(
        app,
        Method::GET,
        &format!("/jellyfin/Items?ParentId={show}&Recursive=true&IncludeItemTypes=Episode"),
        Some(&a),
        None,
    )
    .await;
    assert_eq!(v["TotalRecordCount"], 2);
    // Nothing watched: S01E01 when asked for this show; after E01, E02.
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/Shows/NextUp?SeriesId={show}"), Some(&a), None).await;
    assert_eq!(v["Items"][0]["Id"], ep1);
    jf(app, Method::POST, &format!("/jellyfin/UserPlayedItems/{ep1}"), Some(&a), None).await;
    let (_, v, _) = jf(app, Method::GET, "/jellyfin/Shows/NextUp", Some(&a), None).await;
    assert_eq!(v["Items"][0]["Id"], ep2);
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/Items/Latest?ParentId={view}"), Some(&a), None).await;
    assert!(v.as_array().is_some_and(|l| l.len() == 2), "{v}");

    // Playback: a browser profile can't play Matroska → HLS, started only when fetched.
    let web = json!({ "DeviceProfile": {
        "DirectPlayProfiles": [{ "Container": "mp4,m4v", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac" }],
        "TranscodingProfiles": [{ "Container": "ts", "Type": "Video", "AudioCodec": "aac", "VideoCodec": "h264", "Protocol": "hls" }]
    }});
    let (s, v, _) = jf(app, Method::POST, &format!("/jellyfin/Items/{ep1}/PlaybackInfo"), Some(&a), Some(web)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let src = &v["MediaSources"][0];
    assert_eq!(src["SupportsDirectPlay"], false, "{v}");
    let turl = src["TranscodingUrl"].as_str().unwrap().to_string();
    assert!(turl.starts_with(&format!("/Videos/{ep1}/master.m3u8?")) && turl.contains("PlaySessionId="), "{turl}");
    let sub = src["MediaStreams"].as_array().unwrap().iter().find(|s| s["Type"] == "Subtitle").unwrap().clone();
    assert_eq!(sub["DeliveryMethod"], "External");
    let indices: Vec<i64> =
        src["MediaStreams"].as_array().unwrap().iter().map(|s| s["Index"].as_i64().unwrap()).collect();
    let mut uniq = indices.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), indices.len(), "stream indices are unique: {indices:?}");
    assert!(h.st.hls.sessions().is_empty(), "no transcode yet");
    let (s, master, _) = jf(app, Method::GET, &format!("/jellyfin{turl}"), None, None).await;
    assert_eq!(s, StatusCode::OK, "{master}");
    let variant = master.as_str().unwrap().lines().find(|l| l.starts_with("main.m3u8?")).unwrap().to_string();
    let (s, media, _) = jf(app, Method::GET, &format!("/jellyfin/Videos/{ep1}/{variant}"), None, None).await;
    assert_eq!(s, StatusCode::OK);
    let seg = media.as_str().unwrap().lines().find(|l| l.starts_with("hls1/main/0.ts?")).unwrap().to_string();
    let (s, _, hd) = jf(app, Method::GET, &format!("/jellyfin/Videos/{ep1}/{seg}"), None, None).await;
    assert_eq!((s, hd[header::CONTENT_TYPE].to_str().unwrap()), (StatusCode::OK, "video/mp2t"));
    let durl = sub["DeliveryUrl"].as_str().unwrap();
    let (s, vtt, _) = jf(app, Method::GET, &format!("/jellyfin{durl}"), None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(vtt.as_str().unwrap().starts_with("WEBVTT"), "{vtt}");
    let play = turl.split("PlaySessionId=").nth(1).unwrap().split('&').next().unwrap().to_string();
    let prog = json!({ "ItemId": ep1, "PositionTicks": 50_000_000, "PlaySessionId": play });
    assert_eq!(
        jf(app, Method::POST, "/jellyfin/Sessions/Playing/Progress", Some(&a), Some(prog.clone())).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, v, _) = jf(app, Method::GET, &format!("/jellyfin/UserItems/{ep1}/UserData"), Some(&a), None).await;
    assert_eq!(v["PlaybackPositionTicks"], 50_000_000);
    assert_eq!(
        jf(app, Method::POST, "/jellyfin/Sessions/Playing/Stopped", Some(&a), Some(prog)).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(h.st.hls.sessions().is_empty(), "stopping ends the transcode");
    // Kodi plays Matroska itself: the file, with Range.
    let kodi = json!({ "DeviceProfile": { "DirectPlayProfiles": [{ "Container": "mkv", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac" }] } });
    let (_, v, _) = jf(app, Method::POST, &format!("/jellyfin/Items/{ep2}/PlaybackInfo"), Some(&a), Some(kodi)).await;
    assert_eq!(v["MediaSources"][0]["SupportsDirectPlay"], true, "{v}");
    let src = v["MediaSources"][0]["Id"].as_str().unwrap().to_string();
    let mut req = Request::builder()
        .uri(format!("/jellyfin/Videos/{ep2}/stream.mkv?Static=true&MediaSourceId={src}&api_key={token}"))
        .header(header::HOST, "127.0.0.1:8443")
        .header(header::RANGE, "bytes=0-99")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:50000".parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_http_listener_and_discovery() {
    let h =
        harness("jf-listen", ServerConfig { tls: false, jellyfin_api: true, jellyfin_port: 0, ..Default::default() })
            .await;
    let _ = admin_token(&h.app).await;
    let l = super::listen(h.st.clone()).await.unwrap();
    let base = format!("http://127.0.0.1:{}", l.addr.port());
    for path in ["/System/Info/Public", "/jellyfin/system/info/public"] {
        let v: Value = reqwest::get(format!("{base}{path}")).await.unwrap().json().await.unwrap();
        assert_eq!(v["ProductName"], "Jellyfin Server");
        assert!(v["LocalAddress"].as_str().unwrap().starts_with("http://127.0.0.1:"), "{v}");
    }
    // Discovery answers the apps' question with the address to use.
    let port = 20000 + (std::process::id() % 20000) as u16;
    let task = super::discovery::serve(h.st.clone(), port).await.unwrap();
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sock.send_to(b"Who is JellyfinServer?", ("127.0.0.1", port)).await.unwrap();
    let mut buf = [0u8; 2048];
    let (n, _) =
        tokio::time::timeout(std::time::Duration::from_secs(5), sock.recv_from(&mut buf)).await.unwrap().unwrap();
    let v: Value = serde_json::from_slice(&buf[..n]).unwrap();
    assert!(v["Address"].as_str().unwrap().starts_with("http://127.0.0.1:"), "{v}");
    assert_eq!(v["Id"].as_str().unwrap().len(), 32);
    task.abort();
    drop(l);
    h.stop().await;
}

/// Garbage in (ids, numbers, headers, bodies) never gets a server error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inputs_never_break_it() {
    let h = harness("jf-fuzz", ServerConfig { tls: false, jellyfin_api: true, ..Default::default() }).await;
    let app = &h.app;
    let _ = admin_token(app).await;
    let (token, _) = login(app).await;
    let a = with_token(&token);
    let mut rng = corrent_fuzz::Rng::new(11);
    let paths = [
        "/jellyfin/Items?ParentId={x}&Limit={n}&StartIndex={n}&IncludeItemTypes={x}&SortBy={x}",
        "/jellyfin/Items/{x}",
        "/jellyfin/Users/{x}/Items/{x}",
        "/jellyfin/Shows/{x}/Episodes?Season={n}&SeasonId={x}",
        "/jellyfin/Items/{x}/Images/{x}?tag={x}&maxWidth={n}",
        "/jellyfin/Videos/{x}/{x}?PlaySessionId={x}&api_key=",
        "/jellyfin/Videos/{x}/{x}/Subtitles/{n}/0/Stream.{x}",
        "/jellyfin/Shows/NextUp?SeriesId={x}&Limit={n}",
        "/jellyfin/DisplayPreferences/{x}?client={x}",
        "/jellyfin/UserItems/{x}/UserData",
    ];
    for i in 0..400 {
        let x: String = url::form_urlencoded::byte_serialize(rng.text(40).as_bytes()).collect();
        let n = match i % 4 {
            0 => "-1".to_string(),
            1 => "999999999999".into(),
            2 => "1e9".into(),
            _ => (rng.next_u64() % 100).to_string(),
        };
        let p = paths[i % paths.len()].replace("{x}", &x).replace("{n}", &n);
        let p = format!("{p}{}", if p.ends_with('=') { token.clone() } else { String::new() });
        let (s, v, _) = jf(app, Method::GET, &p, Some(&a), None).await;
        assert!(!s.is_server_error(), "{p} → {s} {v}");
    }
    for body in [
        json!(null),
        json!({ "DeviceProfile": "x" }),
        json!({ "DeviceProfile": { "DirectPlayProfiles": "x" } }),
        json!([1, 2]),
    ] {
        let (s, _, _) = jf(
            app,
            Method::POST,
            &format!("/jellyfin/Items/{}/PlaybackInfo", ids::enc(ids::ITEM, 1)),
            Some(&a),
            Some(body),
        )
        .await;
        assert!(!s.is_server_error());
        let (s, _, _) = jf(
            app,
            Method::POST,
            "/jellyfin/Sessions/Playing/Progress",
            Some(&a),
            Some(json!({ "ItemId": 5, "PositionTicks": "x" })),
        )
        .await;
        assert!(!s.is_server_error());
    }
    for auth in [
        "MediaBrowser",
        "MediaBrowser Token",
        r#"MediaBrowser Token=""""#,
        "Emby ,,,,=",
        "MediaBrowser Token=\"ct_\u{1F600}\"",
    ] {
        let (s, _, _) = jf(app, Method::GET, "/jellyfin/Users/Me", Some(auth), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{auth}");
    }
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_universal_transcodes_with_the_apps_session_id() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let h = harness("jf-audio", ServerConfig { tls: false, jellyfin_api: true, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let album = h.dir.join("Music/Artist/Album");
    std::fs::create_dir_all(&album).unwrap();
    let job = corrent_mpv::encode::EncodeJob {
        input: "av://lavfi:sine=frequency=440:duration=8".into(),
        audio_file: None,
        output: album.join("01 Song.flac").display().to_string(),
        options: vec![("of".into(), "flac".into()), ("oac".into(), "flac".into())],
    };
    corrent_mpv::encode::run(&job, std::time::Duration::from_secs(60), &mut |_| true).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Music", "kind": "music", "folders": [h.dir.join("Music")] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let track = loop {
        let (_, v, _) = crate::api_tests::get(app, &format!("/api/v1/items?library_id={lib}&kind=track"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let (token, _) = login(app).await;
    let t = ids::enc(ids::ITEM, track);
    // Jellyfin Web asks for MP3 only, with its own PlaySessionId: FLAC → HLS.
    let uri = format!(
        "/jellyfin/Audio/{t}/universal?UserId=x&DeviceId=dev-abc&MaxStreamingBitrate=140000000&Container=mp3&TranscodingContainer=ts&TranscodingProtocol=hls&AudioCodec=aac&api_key={token}&PlaySessionId=1729000000001&StartTimeTicks=0"
    );
    let (s, master, _) = jf(app, Method::GET, &uri, None, None).await;
    assert_eq!(s, StatusCode::OK, "{master}");
    let variant = master.as_str().unwrap().lines().find(|l| l.starts_with("main.m3u8?")).unwrap().to_string();
    assert!(
        variant.contains("PlaySessionId=1729000000001") && !variant.matches("PlaySessionId").nth(1).is_some(),
        "{variant}"
    );
    let (s, media, _) = jf(app, Method::GET, &format!("/jellyfin/Audio/{t}/{variant}"), None, None).await;
    assert_eq!(s, StatusCode::OK, "{media}");
    let seg = media.as_str().unwrap().lines().find(|l| l.starts_with("hls1/main/0.ts?")).unwrap().to_string();
    assert_eq!(jf(app, Method::GET, &format!("/jellyfin/Audio/{t}/{seg}"), None, None).await.0, StatusCode::OK);
    // A FLAC-capable app gets the file.
    let (s, _, hd) =
        jf(app, Method::GET, &format!("/jellyfin/Audio/{t}/universal?Container=flac,mp3&api_key={token}"), None, None)
            .await;
    assert_eq!(s, StatusCode::OK);
    assert!(hd[header::CONTENT_TYPE].to_str().unwrap().contains("flac"), "{hd:?}");
    h.stop().await;
}
