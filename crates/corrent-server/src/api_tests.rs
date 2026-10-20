//! In-process tests of the whole HTTP surface (`oneshot` through the real router, guard
//! included) against a real engine on loopback.

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use corrent_core::engine::testkit::{self, Leecher};
use corrent_db::Db;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::identity::{HashCost, Identity};
use crate::state::AppState;
use crate::{ServerConfig, ServerParts, build_state, router};

pub(crate) struct H {
    pub(crate) app: Router,
    pub(crate) st: Arc<AppState>,
    l: Leecher,
    pub(crate) dir: std::path::PathBuf,
}

pub(crate) async fn harness(name: &str, cfg: ServerConfig) -> H {
    let dir = testkit::temp_dir(&format!("api-{name}"));
    let l = testkit::start(&dir);
    let db = Db::open(&dir.join("corrent.db")).unwrap();
    let identity = Identity::new(db.clone(), HashCost::FAST);
    let library = corrent_library::Library::new(db, &dir, corrent_library::probe::Prober::new(None, 2));
    let settings = Arc::new(ArcSwap::from_pointee(testkit::settings(&dir)));
    let st = build_state(ServerParts {
        engine: l.engine.clone(),
        settings,
        paths: l.paths.clone(),
        identity,
        cfg,
        hub: Default::default(),
        tls_fingerprint: None,
        ca_pem: None,
        library,
    })
    .await
    .unwrap();
    H { app: router(st.clone()), st, l, dir }
}

impl H {
    /// Takes the engine's event receiver (so the server's integration can consume it).
    fn l_events(&mut self) -> std::sync::mpsc::Receiver<corrent_core::engine::EngineEvent> {
        std::mem::replace(&mut self.l.events, std::sync::mpsc::channel().1)
    }

    pub(crate) async fn stop(self) {
        drop(self.app);
        drop(self.st);
        testkit::stop(self.l).await;
        let _ = std::fs::remove_dir_all(self.dir);
    }
}

#[derive(Default)]
pub(crate) struct Call<'a> {
    pub(crate) peer: Option<&'a str>,
    pub(crate) host: Option<&'a str>,
    pub(crate) bearer: Option<&'a str>,
    pub(crate) cookie: Option<&'a str>,
    pub(crate) csrf: Option<&'a str>,
    pub(crate) origin: Option<&'a str>,
    pub(crate) body: Option<Value>,
}

pub(crate) async fn call(app: &Router, method: Method, uri: &str, c: Call<'_>) -> (StatusCode, Value, HeaderMap) {
    let peer: SocketAddr = format!("{}:50000", c.peer.unwrap_or("127.0.0.1")).parse().unwrap();
    let mut b = Request::builder().method(method).uri(uri).header(header::HOST, c.host.unwrap_or("127.0.0.1:8443"));
    if let Some(t) = c.bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(t) = c.cookie {
        b = b.header(header::COOKIE, format!("corrent_session={t}"));
    }
    if let Some(t) = c.csrf {
        b = b.header("x-corrent-csrf", t);
    }
    if let Some(o) = c.origin {
        b = b.header(header::ORIGIN, o);
    }
    let body = match c.body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = b.body(body).unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v, headers)
}

pub(crate) fn get(
    app: &Router,
    uri: &str,
    bearer: &str,
) -> impl std::future::Future<Output = (StatusCode, Value, HeaderMap)> {
    let (app, uri, bearer) = (app.clone(), uri.to_string(), bearer.to_string());
    async move { call(&app, Method::GET, &uri, Call { bearer: Some(&bearer), ..Default::default() }).await }
}

pub(crate) async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    bearer: &str,
    body: Value,
) -> (StatusCode, Value, HeaderMap) {
    call(app, method, uri, Call { bearer: Some(bearer), body: Some(body), ..Default::default() }).await
}

pub(crate) fn device() -> Value {
    json!({ "name": "test", "platform": "linux", "client": "tests" })
}

/// Creates the admin through first-run setup and returns a device token for it.
pub(crate) async fn admin_token(app: &Router) -> String {
    let (s, v, h) = call(
        app,
        Method::POST,
        "/api/v1/auth/setup",
        Call {
            body: Some(json!({ "name": "admin", "password": "correct horse", "role": "admin" })),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(h.get(header::SET_COOKIE).is_some(), "setup signs the browser in");
    let (s, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "admin", "password": "correct horse", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["token"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setup_login_and_permissions() {
    let h = harness("auth", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let (s, v, _) = call(app, Method::GET, "/api/v1/server/info", Call::default()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["setup_required"], true);
    // Setup only from this computer.
    let (s, _, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/setup",
        Call {
            peer: Some("192.168.1.20"),
            body: Some(json!({ "name": "x", "password": "correct horse", "role": "admin" })),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let admin = admin_token(app).await;
    let (_, v, _) = call(app, Method::GET, "/api/v1/server/info", Call::default()).await;
    assert_eq!(v["setup_required"], false);
    let (s, _, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/setup",
        Call { body: Some(json!({ "name": "y", "password": "correct horse", "role": "admin" })), ..Default::default() },
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "setup works once");

    // Wrong password, no token, bad token.
    let (s, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call { body: Some(json!({ "username": "admin", "password": "nope" })), ..Default::default() },
    )
    .await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("invalid_credentials")));
    assert_eq!(call(app, Method::GET, "/api/v1/torrents", Call::default()).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get(app, "/api/v1/torrents", "ct_bad").await.0, StatusCode::UNAUTHORIZED);

    // A guest can list but not add or manage users.
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "kid", "password": "kid password", "role": "guest" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "kid", "password": "kid password", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let guest = v["token"].as_str().unwrap().to_string();
    assert_eq!(get(app, "/api/v1/torrents", &guest).await.0, StatusCode::OK);
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/torrents",
        &guest,
        json!({ "urls": ["magnet:?xt=urn:btih:0101010101010101010101010101010101010101"] }),
    )
    .await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    assert_eq!(get(app, "/api/v1/users", &guest).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get(app, "/api/v1/users", &admin).await.1.as_array().unwrap().len(), 2);
    // Guests may change their own display name, not their role.
    let me = get(app, "/api/v1/auth/me", &guest).await.1["user"]["id"].as_i64().unwrap();
    let (s, _, _) = send(app, Method::PATCH, &format!("/api/v1/users/{me}"), &guest, json!({ "role": "admin" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v, _) =
        send(app, Method::PATCH, &format!("/api/v1/users/{me}"), &guest, json!({ "display_name": "Kiddo" })).await;
    assert_eq!((s, v["display_name"].as_str()), (StatusCode::OK, Some("Kiddo")));
    // A plain user is confined to the download folders and public URLs.
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "bob", "password": "bob password", "role": "user" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "bob", "password": "bob password", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let bob = v["token"].as_str().unwrap().to_string();
    let magnet = "magnet:?xt=urn:btih:0202020202020202020202020202020202020202";
    let outside = if cfg!(windows) { r"C:\Windows" } else { "/etc" };
    let (s, v, _) =
        send(app, Method::POST, "/api/v1/torrents", &bob, json!({ "urls": [magnet], "save_path": outside })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let inside = h.dir.join("dl").join("bob");
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/torrents",
        &bob,
        json!({ "urls": [magnet], "save_path": inside, "paused": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _, _) =
        send(app, Method::POST, "/api/v1/torrents", &bob, json!({ "urls": ["http://127.0.0.1:9/x.torrent"] })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no fetching from the server's own network");
    let (s, _, _) = get(app, &format!("/api/v1/server/fs?path={}", outside.replace('\\', "%5C")), &bob).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v, _) = get(app, "/api/v1/server/fs", &bob).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["entries"].as_array().unwrap().iter().all(|e| !e["path"].as_str().unwrap().starts_with(outside)));
    // Changing one's own password needs the current one.
    let bob_id = get(app, "/api/v1/auth/me", &bob).await.1["user"]["id"].as_i64().unwrap();
    let (s, _, _) =
        send(app, Method::PATCH, &format!("/api/v1/users/{bob_id}"), &bob, json!({ "password": "new password 1" }))
            .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = send(
        app,
        Method::PATCH,
        &format!("/api/v1/users/{bob_id}"),
        &bob,
        json!({ "password": "new password 1", "current_password": "bob password" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // The audit log saw the sign-ins.
    let (_, v, _) = get(app, "/api/v1/server/audit", &admin).await;
    assert!(v.as_array().unwrap().iter().any(|e| e["action"] == "login" && e["ok"] == false));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cookies_need_csrf_and_the_guard_refuses_strangers() {
    let h = harness("csrf", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    admin_token(app).await;
    let (_, v, hd) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call { body: Some(json!({ "username": "admin", "password": "correct horse" })), ..Default::default() },
    )
    .await;
    let csrf = v["csrf"].as_str().unwrap().to_string();
    assert!(v["token"].is_null(), "browsers never see the token");
    let cookie = hd.get(header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let token = cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_string();
    // Reads work with the cookie alone; writes need the CSRF header.
    let (s, _, _) =
        call(app, Method::GET, "/api/v1/torrents", Call { cookie: Some(&token), ..Default::default() }).await;
    assert_eq!(s, StatusCode::OK);
    let body = json!({ "action": "pause_all" });
    let (s, _, _) = call(
        app,
        Method::POST,
        "/api/v1/torrents/actions",
        Call { cookie: Some(&token), body: Some(body.clone()), ..Default::default() },
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        app,
        Method::POST,
        "/api/v1/torrents/actions",
        Call { cookie: Some(&token), csrf: Some(&csrf), body: Some(body), ..Default::default() },
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Internet addresses are refused while remote access is off; Tailscale (CGNAT) is local.
    let (s, v, _) =
        call(app, Method::GET, "/api/v1/server/info", Call { peer: Some("8.8.8.8"), ..Default::default() }).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("remote_access_disabled")));
    let (s, _, _) =
        call(app, Method::GET, "/api/v1/server/info", Call { peer: Some("100.100.1.2"), ..Default::default() }).await;
    assert_eq!(s, StatusCode::OK);
    // DNS rebinding: unknown host names are refused.
    let (s, _, _) =
        call(app, Method::GET, "/api/v1/server/info", Call { host: Some("evil.example"), ..Default::default() }).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Logout ends the session.
    let (s, _, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/logout",
        Call { cookie: Some(&token), csrf: Some(&csrf), ..Default::default() },
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) =
        call(app, Method::GET, "/api/v1/torrents", Call { cookie: Some(&token), ..Default::default() }).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torrents_settings_pairing_and_qbittorrent_login() {
    let h = harness("torrents", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let seeder = testkit::seeder(&h.dir).await;
    // Upload the seeder's .torrent (multipart: options JSON + file part).
    let boundary = "corrent-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"options\"\r\n\r\n{}\r\n",
            json!({ "category": "Movies", "paused": true })
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"data.torrent\"\r\nContent-Type: application/x-bittorrent\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(&seeder.bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/torrents")
        .header(header::HOST, "127.0.0.1:8443")
        .header(header::AUTHORIZATION, format!("Bearer {admin}"))
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let hash = seeder.hash.to_hex();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, v, _) = get(app, "/api/v1/torrents", &admin).await;
        let t = v["torrents"].as_array().unwrap().iter().find(|t| t["hash"] == hash.as_str()).cloned();
        if let Some(t) = t
            && t["added_by"] == "admin"
        {
            assert_eq!(t["category"], "Movies");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "torrent listed with its owner: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(get(app, &format!("/api/v1/torrents/{hash}"), &admin).await.0, StatusCode::OK);
    // Other users don't see the admin's trackers or folders.
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "viewer", "password": "viewer password", "role": "guest" }),
    )
    .await;
    assert!(v["id"].is_i64());
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "viewer", "password": "viewer password", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let viewer = v["token"].as_str().unwrap().to_string();
    let (_, v, _) = get(app, &format!("/api/v1/torrents/{hash}"), &viewer).await;
    assert_eq!(v["save_path"], "", "{v}");
    assert_eq!(get(app, "/api/v1/torrents/zz", &admin).await.0, StatusCode::BAD_REQUEST);
    let (s, _, _) =
        send(app, Method::POST, "/api/v1/torrents", &admin, json!({ "urls": ["file:///etc/passwd"] })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Settings: secrets are masked and kept when the mask comes back.
    let (_, mut st, _) = get(app, "/api/v1/server/settings", &admin).await;
    st["connection"]["proxy_password"] = json!("s3cret");
    let (s, v, _) = send(app, Method::PUT, "/api/v1/server/settings", &admin, st).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["connection"]["proxy_password"], crate::v1::server_mask());
    let (s, _, _) = send(app, Method::PUT, "/api/v1/server/settings", &admin, v).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.st.settings.load().connection.proxy_password, "s3cret");

    // Pairing: a code from a signed-in user gives a new device its own token.
    let (s, code, _) = send(app, Method::POST, "/api/v1/auth/pair/code", &admin, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{code}");
    assert!(code["link"].as_str().unwrap().starts_with("corrent://pair?"));
    let (s, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/pair",
        Call {
            peer: Some("192.168.1.30"),
            body: Some(json!({ "code": code["code"], "device": device() })),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let tv = v["token"].as_str().unwrap().to_string();
    // The admin now has two devices (sign-in and pairing); the caller's is marked current.
    let devs = get(app, "/api/v1/devices", &tv).await.1;
    let devs = devs.as_array().unwrap();
    assert_eq!(devs.len(), 2);
    assert_eq!(devs.iter().filter(|d| d["current"] == true).count(), 1);

    // Folder browser.
    let (s, v, _) = get(app, &format!("/api/v1/server/fs?path={}", h.dir.display()), &admin).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["entries"].as_array().unwrap().iter().all(|e| e["dir"] == true));

    // The qBittorrent API takes server accounts.
    let res = h
        .app
        .clone()
        .oneshot({
            let mut r = Request::builder()
                .method(Method::POST)
                .uri("/api/v2/auth/login")
                .header(header::HOST, "127.0.0.1:8443")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=correct%20horse"))
                .unwrap();
            r.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
            r
        })
        .await
        .unwrap();
    assert!(res.headers().get(header::SET_COOKIE).is_some_and(|c| c.to_str().unwrap().starts_with("SID=")));

    // Remove it again.
    let (s, _, _) =
        send(app, Method::POST, "/api/v1/torrents/actions", &admin, json!({ "action": "remove", "hashes": [hash] }))
            .await;
    assert_eq!(s, StatusCode::OK);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_stream_full_list_then_deltas() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

    let h = harness("events", ServerConfig { tls: false, ..Default::default() }).await;
    let token = admin_token(&h.app).await;
    let listener = crate::serve_http(h.app.clone(), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let mut req = format!("ws://{}/api/v1/events?rate_ms=250", listener.addr).into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let mut next = async || -> Value {
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(20), ws.next())
                .await
                .expect("event in time")
                .unwrap()
                .unwrap();
            match m {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Binary(b) => {
                    let mut out = Vec::new();
                    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&b[..]), &mut out).unwrap();
                    return serde_json::from_slice(&out).unwrap();
                }
                _ => continue,
            }
        }
    };
    assert_eq!(next().await["t"], "hello");
    drop(next);
    ws.send(Message::Text(r#"{"t":"subscribe","topics":[{"topic":"torrents"},{"topic":"session"}]}"#.into()))
        .await
        .unwrap();
    let mut seen_full = false;
    let mut seen_session = false;
    let seeder = testkit::seeder(&h.dir).await;
    let hash = seeder.hash.to_hex();
    let mut seen_new = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !(seen_full && seen_session && seen_new) {
        assert!(std::time::Instant::now() < deadline, "full={seen_full} session={seen_session} new={seen_new}");
        let m = tokio::time::timeout(std::time::Duration::from_secs(20), ws.next()).await.unwrap().unwrap().unwrap();
        let v: Value = match m {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            Message::Binary(b) => {
                let mut out = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&b[..]), &mut out).unwrap();
                serde_json::from_slice(&out).unwrap()
            }
            _ => continue,
        };
        match v["t"].as_str() {
            Some("torrents_full") => {
                seen_full = true;
                // Add a torrent once the client has the (empty) list: it must arrive as a delta.
                testkit::add(&h.l, &seeder, &h.dir);
            }
            Some("session") => seen_session = true,
            Some("torrents_delta") => {
                assert!(seen_full, "full list comes first");
                if v["delta"]["upserted"].as_array().is_some_and(|a| a.iter().any(|t| t["hash"] == hash.as_str())) {
                    seen_new = true;
                }
            }
            _ => {}
        }
    }
    drop(ws);
    drop(listener);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_websockets_need_our_origin() {
    let h = harness("wsorigin", ServerConfig { tls: false, ..Default::default() }).await;
    admin_token(&h.app).await;
    let (_, _, hd) = call(
        &h.app,
        Method::POST,
        "/api/v1/auth/login",
        Call { body: Some(json!({ "username": "admin", "password": "correct horse" })), ..Default::default() },
    )
    .await;
    let cookie = hd.get(header::SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let listener = crate::serve_http(h.app.clone(), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{}/api/v1/events", listener.addr).into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut().insert("origin", "http://evil.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err(), "cross-site socket refused");
    let mut req = format!("ws://{}/api/v1/events", listener.addr).into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut().insert("origin", format!("http://{}", listener.addr).parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_ok(), "same-origin socket accepted");
    drop(listener);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_access_settings() {
    let h = harness("remote", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let (s, v, _) = send(app, Method::GET, "/api/v1/server/remote", &admin, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["config"]["discovery"], true);
    assert_eq!(v["config"]["connect"], false);
    let (s, _, _) = send(
        app,
        Method::PUT,
        "/api/v1/server/remote",
        &admin,
        json!({ "discovery": false, "allowed_networks": ["not a network"] }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v, _) = send(
        app,
        Method::PUT,
        "/api/v1/server/remote",
        &admin,
        json!({ "discovery": false, "connect_relay": true, "allowed_networks": ["203.0.113.0/24"] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["config"]["allowed_networks"][0], "203.0.113.0/24");
    let saved = ServerConfig::load(&h.st.paths.data_dir).unwrap();
    assert!(!saved.discovery && saved.allowed_networks == vec!["203.0.113.0/24".to_string()]);
    // The new network counts as local right away.
    assert!(crate::net::is_local("203.0.113.9".parse().unwrap(), &h.st.nets.load().allowed));
    // Not for regular users.
    let (s, _, _) = send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "viewer", "password": "viewer password", "role": "user" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "viewer", "password": "viewer password", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let user = v["token"].as_str().unwrap().to_string();
    let (s, _, _) = send(app, Method::GET, "/api/v1/server/remote", &user, Value::Null).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    h.stop().await;
}

/// Starts a task and waits for its run to finish; returns the finished run.
async fn run_task(app: &Router, token: &str, id: &str) -> Value {
    let (s, started, _) = send(app, Method::POST, &format!("/api/v1/tasks/{id}/run"), token, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{started}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let (_, h, _) = get(app, &format!("/api/v1/tasks/{id}/history"), token).await;
        if let Some(r) =
            h.as_array().and_then(|a| a.iter().find(|r| r["id"] == started["id"])).filter(|r| r["finished"].is_i64())
        {
            return r.clone();
        }
        assert!(std::time::Instant::now() < deadline, "task {id} finished: {h}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_notifications_backups_metrics() {
    let h = harness("admin", ServerConfig { tls: false, metrics: true, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;

    // Tasks: listed with defaults, settings stick, a run is recorded.
    let (s, v, _) = get(app, "/api/v1/tasks", &admin).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v.as_array().unwrap().iter().any(|t| t["id"] == "backup" && t["every_min"] == 1440));
    let (s, v, _) = send(app, Method::PUT, "/api/v1/tasks/backup", &admin, json!({ "every_min": 60 })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v.as_array().unwrap().iter().any(|t| t["id"] == "backup" && t["every_min"] == 60));
    let v = run_task(app, &admin, "cleanup_cache").await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, v, _) = get(app, "/api/v1/tasks/cleanup_cache/history", &admin).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["trigger"], "admin");
    let (s, _, _) = send(app, Method::POST, "/api/v1/tasks/nope/run", &admin, Value::Null).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Backups: a zip with the database and the configuration, downloadable by admins.
    let v = run_task(app, &admin, "backup").await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, list, _) = get(app, "/api/v1/backups", &admin).await;
    let name = list[0]["name"].as_str().unwrap().to_string();
    let zip_path = crate::backups::path_of(&h.st.paths.data_dir, &name).unwrap();
    let mut z = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
    let names: Vec<String> = z.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    assert!(names.contains(&"corrent.db".to_string()), "{names:?}");
    let mut db = Vec::new();
    std::io::Read::read_to_end(&mut z.by_name("corrent.db").unwrap(), &mut db).unwrap();
    assert!(db.starts_with(b"SQLite format 3"));
    let (s, _, hd) = get(app, &format!("/api/v1/backups/{name}"), &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(hd[header::CONTENT_TYPE], "application/zip");
    let (s, _, _) = get(app, "/api/v1/backups/..%2Fcorrent.db", &admin).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Restore: staged for the next start, cancellable; uploads must be backups.
    let (s, v, _) = send(app, Method::POST, &format!("/api/v1/backups/{name}/restore"), &admin, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(get(app, "/api/v1/backups/restore", &admin).await.1["pending"], name.as_str());
    let (s, v, _) = send(app, Method::DELETE, "/api/v1/backups/restore", &admin, json!({})).await;
    assert_eq!((s, v["pending"].clone()), (StatusCode::OK, Value::Null));
    let (s, _, _) = send(app, Method::POST, "/api/v1/backups/nope.zip/restore", &admin, json!({})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = send(app, Method::POST, "/api/v1/backups/upload", &admin, json!({ "not": "a zip" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Notifications: a webhook receives the test message and events.
    let got: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    let g2 = got.clone();
    let hook = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
            let g2 = g2.clone();
            async move {
                g2.lock().unwrap().push(v);
                "ok"
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, hook).await });
    let (s, _, _) = send(
        app,
        Method::POST,
        "/api/v1/notifications",
        &admin,
        json!({ "name": "x", "kind": "webhook", "url": "ftp://x" }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, t, _) = send(
        app,
        Method::POST,
        "/api/v1/notifications",
        &admin,
        json!({ "name": "Hook", "kind": "webhook", "url": url, "events": ["download_done"], "enabled": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{t}");
    let id = t["id"].as_i64().unwrap();
    let (s, v, _) = send(app, Method::POST, &format!("/api/v1/notifications/{id}/test"), &admin, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    crate::notify::send(&h.st, "device_paired", "ignored".into(), String::new());
    crate::notify::send(&h.st, "download_done", "Download complete".into(), "Sintel".into());
    for _ in 0..100 {
        if got.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    {
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 2, "test + the subscribed event only: {got:?}");
        assert_eq!(got[0]["event"], "test");
        assert_eq!(got[1]["body"], "Sintel");
    }

    // Metrics (enabled in this server's config).
    let (s, v, _) = call(app, Method::GET, "/metrics", Call::default()).await;
    assert_eq!(s, StatusCode::OK);
    let text = v.as_str().unwrap();
    assert!(text.contains("# TYPE corrent_download_bytes_per_second gauge"), "{text}");
    assert!(text.contains("corrent_users 1"), "{text}");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profiles_and_pins() {
    let h = harness("profiles", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let mk = |name: &str, role: &str| json!({ "name": name, "password": "long enough pw", "role": role });
    let (_, kid, _) = send(app, Method::POST, "/api/v1/users", &admin, mk("kid", "kids")).await;
    let (_, mom, _) = send(app, Method::POST, "/api/v1/users", &admin, mk("mom", "user")).await;
    let (kid, mom) = (kid["id"].as_i64().unwrap(), mom["id"].as_i64().unwrap());
    let (s, v, _) = get(app, "/api/v1/auth/profiles", &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 3);
    let switch = |user: i64, pin: Option<&str>| json!({ "user_id": user, "pin": pin, "device": device() });
    // The admin may enter the kid's profile without a PIN; the kid can't enter mom's.
    let (s, v, _) = send(app, Method::POST, "/api/v1/auth/profile", &admin, switch(kid, None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let kid_token = v["token"].as_str().unwrap().to_string();
    let (s, _, _) = send(app, Method::POST, "/api/v1/auth/profile", &kid_token, switch(mom, None)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // With a PIN on mom's profile: the right PIN opens it, a wrong one doesn't.
    let (s, _, _) = send(app, Method::PUT, &format!("/api/v1/users/{mom}/pin"), &admin, json!({ "pin": "12a4" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = send(app, Method::PUT, &format!("/api/v1/users/{mom}/pin"), &admin, json!({ "pin": "2468" })).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = send(app, Method::POST, "/api/v1/auth/profile", &kid_token, switch(mom, Some("1111"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v, _) = send(app, Method::POST, "/api/v1/auth/profile", &kid_token, switch(mom, Some("2468"))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["user"]["name"], "mom");
    // A kid can't set someone else's PIN.
    let (s, _, _) =
        send(app, Method::PUT, &format!("/api/v1/users/{mom}/pin"), &kid_token, json!({ "pin": null })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Switching again replaces this device's previous token for that profile.
    let (_, v1, _) = send(app, Method::POST, "/api/v1/auth/profile", &admin, switch(kid, None)).await;
    assert_ne!(v1["token"], kid_token);
    let (s, _, _) = get(app, "/api/v1/auth/me", &kid_token).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "the old profile token was replaced");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_app_claims_its_owner_account() {
    let h = harness("claim", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let (_, issued) = h.st.identity.local_device_token().await.unwrap();
    let (_, v, _) = call(app, Method::GET, "/api/v1/server/info", Call::default()).await;
    assert_eq!(v["setup_required"], false, "browsers aren't offered the setup page");
    let body = json!({ "name": "me", "password": "correct horse", "role": "admin" });
    // Another local program (loopback, no token) can't take the account.
    let (s, _, _) =
        call(app, Method::POST, "/api/v1/auth/setup", Call { body: Some(body.clone()), ..Default::default() }).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // The app itself can.
    let (s, v, _) = send(app, Method::POST, "/api/v1/auth/setup", &issued.token, body).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["user"]["name"], "me");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preferences_follow_the_user() {
    let h = harness("prefs", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let (_, v, _) = get(app, "/api/v1/me/prefs", &admin).await;
    assert_eq!(v, json!({}));
    let p = json!({ "homeOrder": ["favorites", "continue"], "homeHidden": ["next_up"], "cardSize": "large" });
    let (s, _, _) = send(app, Method::PUT, "/api/v1/me/prefs", &admin, p.clone()).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v, _) = get(app, "/api/v1/me/prefs", &admin).await;
    assert_eq!(v, p);
    let (s, _, _) = send(app, Method::PUT, "/api/v1/me/prefs", &admin, json!([1, 2])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, t, _) = get(app, "/api/v1/theme", &admin).await;
    assert_eq!(t["preset"], "corrent");
    assert_eq!(t["accent"], "blue");
    h.stop().await;
}

/// Robustness: mangled request bodies are refused cleanly (4xx), never with a 500 or a
/// crashed handler, and the server keeps answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_inputs_are_refused_cleanly() {
    use corrent_fuzz::{Rng, iterations};
    let mut h = harness("fuzz", ServerConfig { tls: false, ..Default::default() }).await;
    // Previews are answered through the engine's events.
    crate::forward_engine_events(h.l_events(), h.st.clone());
    let app = &h.app;
    let admin = admin_token(app).await;
    let hash = "0101010101010101010101010101010101010101";
    let targets: Vec<(Method, String, &str)> = vec![
        (
            Method::POST,
            "/api/v1/torrents".into(),
            r#"{"urls":["magnet:?xt=urn:btih:0101010101010101010101010101010101010101"],"paused":true}"#,
        ),
        (
            Method::POST,
            "/api/v1/torrents/actions".into(),
            r#"{"action":"pause","hashes":["0101010101010101010101010101010101010101"]}"#,
        ),
        (Method::PATCH, format!("/api/v1/torrents/{hash}"), r#"{"name":"x","tags":["a"],"seed":{"ratio":1.5}}"#),
        (Method::POST, "/api/v1/torrents/preview".into(), r#"{"torrent":"ZDQ6aW5mb2Rl"}"#),
        (Method::POST, "/api/v1/torrents/queue".into(), r#"{"hashes":[],"before":null}"#),
        (
            Method::PUT,
            "/api/v1/torrents/categories".into(),
            r#"{"Movies":{"save_path":null,"down_limit_kib":0,"up_limit_kib":0}}"#,
        ),
        (Method::POST, "/api/v1/libraries".into(), r#"{"name":"L","kind":"photos","folders":["/nonexistent"]}"#),
        (Method::POST, "/api/v1/wanted".into(), r#"{"title":"Sintel","year":2010}"#),
        (Method::POST, "/api/v1/users".into(), r#"{"name":"u","password":"long enough pw","role":"user"}"#),
        (Method::PUT, "/api/v1/me/prefs".into(), r#"{"homeOrder":["continue"]}"#),
        (Method::POST, "/api/v1/notifications".into(), r#"{"name":"n","kind":"ntfy","url":"https://ntfy.sh/x"}"#),
        (Method::PUT, "/api/v1/tasks/backup".into(), r#"{"every_min":60}"#),
        (
            Method::POST,
            "/api/v1/auth/profile".into(),
            r#"{"user_id":1,"pin":"1234","device":{"name":"d","platform":"linux"}}"#,
        ),
        (Method::PUT, "/api/v1/auth/pin".into(), r#"{"pin":"1234"}"#),
        (Method::POST, "/api/v1/auth/pair/code".into(), r#"{"host":"x"}"#),
    ];
    let n = iterations(600);
    for i in 0..n {
        let mut r = Rng::new(i as u64 + 1);
        let (method, uri, seed) = r.pick(&targets).clone();
        let body = r.case(&[seed], 200);
        let mut req = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .header(header::HOST, "127.0.0.1:8443")
            .header(header::AUTHORIZATION, format!("Bearer {admin}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
        let t0 = std::time::Instant::now();
        let res = app.clone().oneshot(req).await.expect("the handler answered");
        assert_ne!(res.status(), StatusCode::INTERNAL_SERVER_ERROR, "case {i}: {method} {uri} with {body:?}");
        // Garbage must be refused quickly too (nothing waits on it).
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(10),
            "case {i} took {:?}: {method} {uri} with {body:?} → {}",
            t0.elapsed(),
            res.status()
        );
    }
    // Still healthy.
    let (s, _, _) = get(app, "/api/v1/auth/me", &admin).await;
    assert_eq!(s, StatusCode::OK);
    h.stop().await;
}

/// A torrent added some other way (an RSS rule, by hand) that is what the wanted list waits for
/// marks the entry as grabbed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn added_torrents_claim_wanted_entries() {
    let mut h = harness("claim-wanted", ServerConfig { tls: false, ..Default::default() }).await;
    crate::forward_engine_events(h.l_events(), h.st.clone());
    let app = &h.app;
    let admin = admin_token(app).await;
    let (_, w, _) = send(app, Method::POST, "/api/v1/wanted", &admin, json!({ "title": "Sintel", "year": 2010 })).await;
    let (_, other, _) = send(app, Method::POST, "/api/v1/wanted", &admin, json!({ "title": "Tears of Steel" })).await;
    let seeder = testkit::seeder_with(&h.dir, "Sintel.2010.1080p.mkv", vec![7u8; 100_000]).await;
    h.st.engine.send(corrent_core::engine::EngineCommand::Add(Box::new(corrent_core::engine::AddRequest {
        source: corrent_core::engine::types::AddSource::Bytes { name: "s.torrent".into(), bytes: seeder.bytes.clone() },
        torrent_bytes: None,
        save_path: h.dir.join("dl"),
        create_subfolder: false,
        category: None,
        tags: vec![],
        paused: true,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: None,
    })));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, v, _) = get(app, "/api/v1/wanted", &admin).await;
        let by_id = |id: &Value| v.as_array().unwrap().iter().find(|x| x["id"] == *id).cloned().unwrap();
        if by_id(&w["id"])["status"] == "grabbed" {
            assert_eq!(by_id(&w["id"])["torrent"], seeder.hash.to_hex());
            assert_eq!(by_id(&other["id"])["status"], "wanted");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "claimed: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    h.stop().await;
}

/// Installs a plugin through the API (multipart: manifest + wasm).
async fn install_plugin(app: &Router, admin: &str, manifest: &str, wasm: &[u8]) {
    let boundary = "corrent-plugin-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(manifest.as_bytes());
    body.extend_from_slice(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"wasm\"; filename=\"plugin.wasm\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(wasm);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/plugins")
        .header(header::HOST, "127.0.0.1:8443")
        .header(header::AUTHORIZATION, format!("Bearer {admin}"))
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

fn wav(path: &std::path::Path, samples: &[i16]) {
    let rate = corrent_transcode::intro::RATE;
    let mut b = b"RIFF".to_vec();
    b.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&[1, 0, 1, 0]);
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&[2, 0, 16, 0]);
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn noise(seed: u64, secs: f64) -> Vec<i16> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..(secs * corrent_transcode::intro::RATE as f64) as usize)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let env = 0.5 + 0.5 * ((i as f64 / 8000.0) * 1.7 + seed as f64).sin();
            (((x % 20000) as f64 - 10000.0) * env) as i16
        })
        .collect()
}

/// Two episodes sharing a theme at different times: the task finds it in both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn intros_are_found_by_their_sound() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let h = harness("intros", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let season = h.dir.join("Shows/Theme Show/Season 1");
    std::fs::create_dir_all(&season).unwrap();
    let theme = noise(42, 30.0);
    for (n, (before, after)) in [(1, (10.0, 50.0)), (2, (25.0, 35.0))] {
        let audio = [noise(n * 10, before), theme.clone(), noise(n * 10 + 1, after)].concat();
        let w = h.dir.join(format!("ep{n}.wav"));
        wav(&w, &audio);
        let secs = (before + 30.0 + after) as u32;
        let job = corrent_mpv::encode::EncodeJob {
            input: format!("av://lavfi:testsrc=duration={secs}:size=160x120:rate=5,format=yuv420p"),
            audio_file: Some(w.display().to_string()),
            output: season.join(format!("Theme Show - S01E0{n}.mkv")).display().to_string(),
            options: vec![
                ("of".into(), "matroska".into()),
                ("ovc".into(), "libx264".into()),
                ("ovcopts".into(), "preset=ultrafast".into()),
                ("oac".into(), "aac".into()),
            ],
        };
        corrent_mpv::encode::run(&job, std::time::Duration::from_secs(120), &mut |_| true).unwrap();
    }
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
    let eps = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}&kind=episode&sort=index"), &admin).await;
        if v["total"] == 2 {
            break v["items"].as_array().unwrap().iter().map(|e| e["id"].as_i64().unwrap()).collect::<Vec<_>>();
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let r = run_task(app, &admin, "detect_intros").await;
    assert_eq!(r["ok"], true, "{r}");
    for (id, start) in eps.iter().zip([10_000, 25_000]) {
        let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
        let seg = &d["files"][0]["segments"][0];
        assert_eq!(seg["kind"], "intro", "{d}");
        let (s, e) = (seg["start_ms"].as_i64().unwrap(), seg["end_ms"].as_i64().unwrap());
        assert!((s - start).abs() < 2500 && (e - (start + 30_000)).abs() < 2500, "{s}–{e}");
        let (_, p, _) =
            send(app, Method::POST, &format!("/api/v1/playback/{id}"), &admin, json!({ "profile": "native" })).await;
        assert_eq!(p["segments"][0]["kind"], "intro");
    }
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn playlists_and_collections() {
    let h = harness("lists", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let dir = h.dir.join("Vids");
    std::fs::create_dir_all(&dir).unwrap();
    for n in ["a", "b", "c"] {
        let ok = std::process::Command::new("mpv")
            .args([
                "--really-quiet",
                "av://lavfi:testsrc=duration=1:size=64x48:rate=5,format=yuv420p",
                "--of=matroska",
                "--ovc=libx264",
                "--ovcopts=preset=ultrafast",
            ])
            .arg(format!("--o={}", dir.join(format!("{n}.mkv")).display()))
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("mpv not available; skipping");
            h.stop().await;
            return;
        }
    }
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Vids", "kind": "videos", "folders": [dir] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let ids: Vec<i64> = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}&kind=video"), &admin).await;
        if v["total"] == 3 {
            break v["items"].as_array().unwrap().iter().map(|i| i["id"].as_i64().unwrap()).collect();
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let (s, p, _) =
        send(app, Method::POST, "/api/v1/playlists", &admin, json!({ "name": "Mix", "items": [ids[2], ids[0]] })).await;
    assert_eq!((s, p["count"].clone(), p["mine"].clone()), (StatusCode::OK, json!(2), json!(true)), "{p}");
    let pid = p["id"].as_i64().unwrap();
    send(app, Method::POST, &format!("/api/v1/playlists/{pid}/items"), &admin, json!({ "items": [ids[1]] })).await;
    let order = |v: &Value| v.as_array().unwrap().iter().map(|i| i["id"].as_i64().unwrap()).collect::<Vec<_>>();
    let (_, v, _) = get(app, &format!("/api/v1/playlists/{pid}/items"), &admin).await;
    assert_eq!(order(&v), vec![ids[2], ids[0], ids[1]]);
    send(app, Method::POST, &format!("/api/v1/playlists/{pid}/move"), &admin, json!({ "from": 2, "to": 0 })).await;
    send(app, Method::DELETE, &format!("/api/v1/playlists/{pid}/items/1"), &admin, json!({})).await;
    let (_, v, _) = get(app, &format!("/api/v1/playlists/{pid}/items"), &admin).await;
    assert_eq!(order(&v), vec![ids[1], ids[0]]);
    // Others see it only when public, and can't change it.
    send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "pal", "password": "long enough pw", "role": "user" }),
    )
    .await;
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "pal", "password": "long enough pw", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let pal = v["token"].as_str().unwrap().to_string();
    assert_eq!(get(app, "/api/v1/playlists", &pal).await.1, json!([]));
    assert_eq!(get(app, &format!("/api/v1/playlists/{pid}/items"), &pal).await.0, StatusCode::NOT_FOUND);
    send(
        app,
        Method::PATCH,
        &format!("/api/v1/playlists/{pid}"),
        &admin,
        json!({ "public": true, "name": "Shared mix" }),
    )
    .await;
    let (_, v, _) = get(app, "/api/v1/playlists", &pal).await;
    assert_eq!(
        (v[0]["name"].clone(), v[0]["mine"].clone(), v[0]["owner"].clone()),
        (json!("Shared mix"), json!(false), json!("admin"))
    );
    assert_eq!(
        send(app, Method::DELETE, &format!("/api/v1/playlists/{pid}"), &pal, json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    // Collections (as a metadata provider creates them).
    let (a, b) = (ids[0], ids[1]);
    h.st.library
        .db()
        .write(move |t| {
            t.execute("INSERT INTO collections(id, library_id, name) VALUES (7, ?1, 'Trilogy')", [lib])?;
            t.execute("INSERT INTO collection_items(collection_id, item_id) VALUES (7, ?1), (7, ?2)", [a, b])
        })
        .await
        .unwrap();
    let (_, v, _) = get(app, "/api/v1/collections", &admin).await;
    assert_eq!((v[0]["name"].clone(), v[0]["count"].clone()), (json!("Trilogy"), json!(2)));
    assert_eq!(get(app, "/api/v1/collections/7/items", &admin).await.1.as_array().unwrap().len(), 2);
    assert_eq!(get(app, &format!("/api/v1/items/{a}"), &admin).await.1["collections"], json!([[7, "Trilogy"]]));
    h.stop().await;
}

/// Web plugins: listed for the web client, their files served, nothing outside `web/`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web_plugins_serve_their_files() {
    let h = harness("web-plugin", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let dir = h.dir.join("hello");
    std::fs::create_dir_all(dir.join("web")).unwrap();
    std::fs::write(dir.join("plugin.toml"), "id = \"hello\"\nname = \"Hello\"\nversion = \"1.0.0\"\nhost_api = \"1\"\nkinds = [\"web\"]\nruntime = \"web\"\n").unwrap();
    std::fs::write(dir.join("web/index.js"), "corrent.toast('hello');").unwrap();
    let m = corrent_plugins::Manifest::parse(&std::fs::read_to_string(dir.join("plugin.toml")).unwrap()).unwrap();
    h.st.plugins.as_ref().unwrap().load_web(m, dir.clone(), true).unwrap();
    let (_, v, _) = get(app, "/api/v1/plugins/web", &admin).await;
    assert_eq!(v, json!([{ "id": "hello", "name": "Hello", "entry": "/api/v1/plugins/hello/web/index.js" }]));
    let (s, body, hd) = get(app, "/api/v1/plugins/hello/web/index.js", &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert!(hd[header::CONTENT_TYPE].to_str().unwrap().contains("javascript"), "{hd:?}");
    assert_eq!(body, "corrent.toast('hello');");
    for bad in [
        "/api/v1/plugins/hello/web/..%2Fplugin.toml",
        "/api/v1/plugins/hello/web/.hidden",
        "/api/v1/plugins/nope/web/index.js",
    ] {
        assert_eq!(get(app, bad, &admin).await.0, StatusCode::NOT_FOUND, "{bad}");
    }
    assert!(corrent_plugins::Manifest::parse("id = \"x\"\nname = \"X\"\nversion = \"1\"\nhost_api = \"1\"\nkinds = [\"web\"]\nruntime = \"web\"\nweb_entry = \"../x.js\"\n").is_err());
    h.stop().await;
}

/// Event plugins hear what notifications do and may read the library.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn event_plugins_read_the_library() {
    let h = harness("events-plugin", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let pics = h.dir.join("Pics");
    std::fs::create_dir_all(&pics).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(pics.join("forest.png")).unwrap();
    send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Pics", "kind": "photos", "folders": [pics] }),
    )
    .await;
    install_plugin(
        app,
        &admin,
        include_str!("../../corrent-plugins/builtin/example-events.toml"),
        include_bytes!("../../corrent-plugins/builtin/example-events.wasm"),
    )
    .await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, v, _) = get(app, "/api/v1/items?kind=photo", &admin).await;
        if v["total"] == 1 {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    crate::notify::send(&h.st, "download_done", "forest".into(), "done".into());
    let read = || {
        let db = h.st.identity.db().clone();
        async move {
            db.read(|c| {
                use corrent_db::rusqlite::OptionalExtension;
                c.query_row("SELECT value FROM plugin_kv WHERE plugin = 'example-events' AND key = 'last'", [], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
            })
            .await
            .unwrap()
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let last = loop {
        if let Some(v) = read().await {
            break v;
        }
        assert!(std::time::Instant::now() < deadline, "the plugin heard the event");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(last, "download_done | forest | photo:forest");
    h.stop().await;
}

/// Subtitle providers, name parsers and task plugins at work in the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subtitle_parser_and_task_plugins() {
    let h = harness("plugin-kinds", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let b = |n: &str| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../corrent-plugins/builtin").join(n);
    for n in ["example-subtitles", "example-parser", "example-task"] {
        let toml = std::fs::read_to_string(b(&format!("{n}.toml"))).unwrap();
        install_plugin(app, &admin, &toml, &std::fs::read(b(&format!("{n}.wasm"))).unwrap()).await;
    }
    // Metadata lookups would rename things: keep the built-in providers out of it.
    for id in ["musicbrainz", "tmdb"] {
        send(app, Method::PUT, &format!("/api/v1/plugins/{id}/enabled"), &admin, json!({ "enabled": false })).await;
    }
    // A name only the parser plugin understands.
    let movies = h.dir.join("Films");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::write(movies.join("Home Movie {2019} #3.mkv"), vec![1u8; 200_000]).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Films", "kind": "movies", "folders": [movies] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if v["total"] == 1 {
            break v["items"][0].clone();
        }
        assert!(std::time::Instant::now() < deadline, "scanned: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(
        (item["title"].as_str(), item["year"].as_i64()),
        (Some("Home Movie"), Some(2019)),
        "the parser plugin named it"
    );
    let id = item["id"].as_i64().unwrap();

    // Subtitles: found by the provider, saved next to the video, listed with the file.
    let (s, found, _) = get(app, &format!("/api/v1/items/{id}/subtitles?languages=tr,en"), &admin).await;
    assert_eq!(s, StatusCode::OK, "{found}");
    assert_eq!(found.as_array().unwrap().len(), 2);
    assert_eq!(found[0]["provider"], "example-subtitles");
    assert_eq!(found[0]["exact"], true, "the file hash was sent");
    let (s, saved, _) = send(
        app,
        Method::POST,
        &format!("/api/v1/items/{id}/subtitles"),
        &admin,
        json!({ "provider": "example-subtitles", "id": found[0]["id"], "language": "tr" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{saved}");
    let path = saved["path"].as_str().unwrap();
    assert!(path.ends_with("Home Movie {2019} #3.tr.srt"), "{path}");
    assert!(std::fs::read_to_string(path).unwrap().contains("[tr]"));
    let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
    let subs: Vec<&Value> =
        d["files"][0]["streams"].as_array().unwrap().iter().filter(|s| s["kind"] == "subtitle").collect();
    assert_eq!(subs.len(), 1, "{d}");

    // The task plugin is a task like the others.
    let (_, tasks, _) = get(app, "/api/v1/tasks", &admin).await;
    assert!(tasks.as_array().unwrap().iter().any(|t| t["id"] == "plugin:example-task" && t["every_min"] == 60));
    let r = run_task(app, &admin, "plugin:example-task").await;
    assert_eq!((r["ok"].as_bool(), r["message"].as_str()), (Some(true), Some("run 1")));
    h.stop().await;
}

/// Catalog installs: only archives that match their hash and carry a trusted signature.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_installs_signed_plugins_only() {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let b = |n: &str| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../corrent-plugins/builtin").join(n);
    // The archive: plugin.toml + plugin.wasm.
    let mut zip_bytes = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("plugin.toml", o).unwrap();
        std::io::Write::write_all(&mut z, &std::fs::read(b("example-task.toml")).unwrap()).unwrap();
        z.start_file("plugin.wasm", o).unwrap();
        std::io::Write::write_all(&mut z, &std::fs::read(b("example-task.wasm")).unwrap()).unwrap();
        z.finish().unwrap();
    }
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &zip_bytes);
    let sha: String = digest.as_ref().iter().map(|x| format!("{x:02x}")).collect();
    let key = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let sig = base64::engine::general_purpose::STANDARD.encode(key.sign(digest.as_ref()).to_bytes());
    let pubkey: String = key.verifying_key().to_bytes().iter().map(|x| format!("{x:02x}")).collect();
    let other = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
    let bad_sig = base64::engine::general_purpose::STANDARD.encode(other.sign(digest.as_ref()).to_bytes());
    // A tiny catalog server.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let index = json!({ "plugins": [
        { "id": "example-task", "name": "Example task", "version": "1.0.0", "url": format!("{base}/task.zip"), "sha256": sha, "signature": sig },
        { "id": "forged", "name": "Forged", "version": "1.0.0", "url": format!("{base}/task.zip"), "sha256": sha, "signature": bad_sig },
        { "id": "tampered", "name": "Tampered", "version": "1.0.0", "url": format!("{base}/other.zip"), "sha256": sha, "signature": sig },
    ]});
    let (z1, z2) = (zip_bytes.clone(), {
        let mut t = zip_bytes.clone();
        let n = t.len() / 2;
        t[n] ^= 0xff;
        t
    });
    let srv = axum::Router::new()
        .route(
            "/index.json",
            axum::routing::get(move || {
                let i = index.clone();
                async move { axum::Json(i) }
            }),
        )
        .route(
            "/task.zip",
            axum::routing::get(move || {
                let z = z1.clone();
                async move { z }
            }),
        )
        .route(
            "/other.zip",
            axum::routing::get(move || {
                let z = z2.clone();
                async move { z }
            }),
        );
    tokio::spawn(async move { axum::serve(l, srv).await });
    let h = harness(
        "catalog",
        ServerConfig {
            tls: false,
            plugin_catalog: format!("{base}/index.json"),
            plugin_keys: vec![pubkey],
            ..Default::default()
        },
    )
    .await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let (s, v, _) = get(app, "/api/v1/plugins/catalog", &admin).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["entries"].as_array().unwrap().len(), 3);
    assert!(v["entries"][0]["installed"].is_null());
    let install = |id: &str| json!({ "id": id });
    let (s, _, _) = send(app, Method::POST, "/api/v1/plugins/catalog/install", &admin, install("forged")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "an untrusted signature is refused");
    let (s, _, _) = send(app, Method::POST, "/api/v1/plugins/catalog/install", &admin, install("tampered")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a download that doesn't match is refused");
    let (s, v, _) = send(app, Method::POST, "/api/v1/plugins/catalog/install", &admin, install("example-task")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["manifest"]["id"], "example-task");
    let (_, v, _) = get(app, "/api/v1/plugins/catalog", &admin).await;
    assert_eq!(v["entries"][0]["installed"], "1.0.0");
    h.stop().await;
}

/// Watching together: rooms share play/pause/seek among their members only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn syncplay_rooms() {
    let h = harness("syncplay", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let photos = h.dir.join("Pics");
    std::fs::create_dir_all(&photos).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(photos.join("a.png")).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Pics", "kind": "photos", "folders": [photos] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let mk = |name: &str| json!({ "name": name, "password": "long enough pw", "role": "user" });
    send(app, Method::POST, "/api/v1/users", &admin, mk("friend")).await;
    send(app, Method::POST, "/api/v1/users", &admin, mk("other")).await;
    let login = |u: &'static str| async move {
        let (_, v, _) = call(
            app,
            Method::POST,
            "/api/v1/auth/login",
            Call {
                body: Some(json!({ "username": u, "password": "long enough pw", "device": device() })),
                ..Default::default()
            },
        )
        .await;
        v["token"].as_str().unwrap().to_string()
    };
    let (friend, other) = (login("friend").await, login("other").await);

    let (s, room, _) = send(app, Method::POST, "/api/v1/syncplay", &admin, json!({ "item_id": item })).await;
    assert_eq!(s, StatusCode::OK, "{room}");
    let id = room["id"].as_str().unwrap().to_string();
    assert_eq!(room["paused"], true);
    // Not a member yet: nothing to see or do.
    let (s, _, _) = get(app, &format!("/api/v1/syncplay/{id}"), &friend).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, _) = send(
        app,
        Method::POST,
        &format!("/api/v1/syncplay/{id}/command"),
        &other,
        json!({ "action": "play", "position_ms": 0 }),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, v, _) = send(app, Method::POST, &format!("/api/v1/syncplay/{id}/join"), &friend, Value::Null).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["members"], json!(["admin", "friend"]));
    // The friend presses play at 10 s; the admin sees it running.
    let (_, v, _) = send(
        app,
        Method::POST,
        &format!("/api/v1/syncplay/{id}/command"),
        &friend,
        json!({ "action": "play", "position_ms": 10_000 }),
    )
    .await;
    assert_eq!((v["paused"].as_bool(), v["by"].as_str()), (Some(false), Some("friend")));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, v, _) = get(app, &format!("/api/v1/syncplay/{id}"), &admin).await;
    let st: corrent_proto::library::RoomState = serde_json::from_value(v).unwrap();
    let pos = st.position_at(st.server_now_ms);
    assert!((10_250..12_000).contains(&pos), "advanced while playing: {pos}");
    let (_, v, _) = send(
        app,
        Method::POST,
        &format!("/api/v1/syncplay/{id}/command"),
        &admin,
        json!({ "action": "pause", "position_ms": 10_500 }),
    )
    .await;
    assert_eq!(v["paused"], true);
    // Leaving empties the room; the last one out removes it.
    send(app, Method::POST, &format!("/api/v1/syncplay/{id}/leave"), &friend, Value::Null).await;
    send(app, Method::POST, &format!("/api/v1/syncplay/{id}/leave"), &admin, Value::Null).await;
    let (s, _, _) = send(app, Method::POST, &format!("/api/v1/syncplay/{id}/join"), &friend, Value::Null).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    h.stop().await;
}

async fn soap(app: &Router, peer: &str, action: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/dlna/control/ContentDirectory")
        .header(header::HOST, "192.168.1.5:8201")
        .header("soapaction", format!("\"urn:schemas-upnp-org:service:ContentDirectory:1#{action}\""))
        .body(Body::from(body.to_string()))
        .unwrap();
    let (s, v) = raw(app, peer, req).await;
    (s, String::from_utf8_lossy(&v).into_owned())
}

async fn raw(app: &Router, peer: &str, mut req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let peer: SocketAddr = format!("{peer}:50000").parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    (status, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

fn browse(object: &str, flag: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
         <u:Browse xmlns:u=\"urn:schemas-upnp-org:service:ContentDirectory:1\"><ObjectID>{object}</ObjectID>\
         <BrowseFlag>{flag}</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex>\
         <RequestedCount>0</RequestedCount><SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dlna_browses_and_streams() {
    let h = harness("dlna", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let photos = h.dir.join("Pics");
    std::fs::create_dir_all(&photos).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(photos.join("a & b.png")).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Pics & more", "kind": "photos", "folders": [photos] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let (dlna, _) = crate::dlna::test_router(h.st.clone(), true).await.unwrap();

    // The description, from the local network only.
    let desc = || Request::builder().uri("/dlna/description.xml").body(Body::empty()).unwrap();
    let (s, d) = raw(&dlna, "192.168.1.9", desc()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8_lossy(&d).contains("urn:schemas-upnp-org:device:MediaServer:1"));
    assert_eq!(raw(&dlna, "8.8.8.8", desc()).await.0, StatusCode::FORBIDDEN);

    // Root → the library → the photo with a signed URL.
    let (s, root) = soap(&dlna, "192.168.1.9", "Browse", &browse("0", "BrowseDirectChildren")).await;
    assert_eq!(s, StatusCode::OK, "{root}");
    assert!(root.contains(&format!("id=&quot;l{lib}&quot;")) && root.contains("Pics &amp;amp; more"), "{root}");
    let (_, items) = soap(&dlna, "192.168.1.9", "Browse", &browse(&format!("l{lib}"), "BrowseDirectChildren")).await;
    assert!(items.contains("object.item.imageItem.photo"), "{items}");
    assert!(items.contains("<TotalMatches>1</TotalMatches>"), "{items}");
    let start = items.find("http://192.168.1.5:8201/dlna/media/").expect("a media URL");
    let url = &items[start..];
    let url = &url[..url.find("&lt;").unwrap()];
    let path = url.trim_start_matches("http://192.168.1.5:8201");
    let (s, bytes) = raw(&dlna, "192.168.1.9", Request::builder().uri(path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(bytes.starts_with(b"\x89PNG"));
    // A forged signature, an unknown object, an unknown action.
    let forged = format!("/dlna/media/{item}/000000000000000000000000.png");
    assert_eq!(
        raw(&dlna, "192.168.1.9", Request::builder().uri(forged).body(Body::empty()).unwrap()).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, meta) = soap(&dlna, "192.168.1.9", "Browse", &browse(&format!("i{item}"), "BrowseMetadata")).await;
    assert!(meta.contains("<NumberReturned>1</NumberReturned>"), "{meta}");
    let (s, nope) = soap(&dlna, "192.168.1.9", "Browse", &browse("i999999", "BrowseMetadata")).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(nope.contains("<errorCode>701</errorCode>"));
    assert!(soap(&dlna, "192.168.1.9", "GetSystemUpdateID", "").await.1.contains("<Id>"));
    assert_eq!(soap(&dlna, "192.168.1.9", "Explode", "").await.0, StatusCode::INTERNAL_SERVER_ERROR);
    h.stop().await;
}

/// A pretend Chromecast: TLS on a random port, the Default Media Receiver's messages. Sends
/// every LOAD's URL and every control message type to `seen`.
async fn fake_receiver(seen: tokio::sync::mpsc::UnboundedSender<String>) -> SocketAddr {
    use crate::cast::wire::CastMessage;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let key = rcgen::generate_simple_self_signed(vec!["chromecast".into()]).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![key.cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.signing_key.serialize_der().into()),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut s = acceptor.accept(tcp).await.unwrap();
        let send = |ns: &str, src: &str, payload: Value| {
            CastMessage {
                source: src.into(),
                destination: "sender-corrent".into(),
                namespace: ns.into(),
                payload: payload.to_string(),
            }
            .encode()
        };
        let mut state = "IDLE";
        loop {
            let mut len = [0u8; 4];
            if s.read_exact(&mut len).await.is_err() {
                return;
            }
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            s.read_exact(&mut body).await.unwrap();
            let m = CastMessage::decode(&body).unwrap();
            let v: Value = serde_json::from_str(&m.payload).unwrap();
            let ty = v["type"].as_str().unwrap_or("").to_string();
            let out = match ty.as_str() {
                "LAUNCH" => Some(send(
                    "urn:x-cast:com.google.cast.receiver",
                    "receiver-0",
                    json!({ "type": "RECEIVER_STATUS", "status": { "applications": [{ "appId": "CC1AD845", "transportId": "t-1", "sessionId": "s-1" }] } }),
                )),
                "LOAD" => {
                    let _ = seen.send(format!("LOAD {}", v["media"]["contentId"].as_str().unwrap()));
                    state = "PLAYING";
                    Some(send(
                        "urn:x-cast:com.google.cast.media",
                        "t-1",
                        json!({ "type": "MEDIA_STATUS", "status": [{ "mediaSessionId": 9, "playerState": state, "currentTime": 0.0 }] }),
                    ))
                }
                "PAUSE" | "PLAY" | "SEEK" | "STOP" => {
                    let _ = seen.send(ty.clone());
                    state = if ty == "PAUSE" { "PAUSED" } else { "PLAYING" };
                    Some(send(
                        "urn:x-cast:com.google.cast.media",
                        "t-1",
                        json!({ "type": "MEDIA_STATUS", "status": [{ "mediaSessionId": 9, "playerState": state, "currentTime": 3.0 }] }),
                    ))
                }
                "GET_STATUS" => Some(send(
                    "urn:x-cast:com.google.cast.media",
                    "t-1",
                    json!({ "type": "MEDIA_STATUS", "status": [{ "mediaSessionId": 9, "playerState": state, "currentTime": 3.0 }] }),
                )),
                _ => None,
            };
            if let Some(o) = out {
                s.write_all(&o).await.unwrap();
            }
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn casts_to_a_receiver() {
    let h = harness("cast", ServerConfig { tls: false, dlna_port: 0, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let photos = h.dir.join("Pics");
    std::fs::create_dir_all(&photos).unwrap();
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(photos.join("a.png")).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Pics", "kind": "photos", "folders": [photos] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let (tx, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let addr = fake_receiver(tx).await;
    h.st.casts.inject(crate::cast::Found {
        device: corrent_proto::library::CastDevice {
            id: "tv".into(),
            name: "Living room".into(),
            model: "Chromecast".into(),
        },
        addr,
    });
    let (_, v, _) = get(app, "/api/v1/cast/devices", &admin).await;
    assert_eq!(v[0]["name"], "Living room");
    let (s, v, _) = send(app, Method::POST, "/api/v1/cast/tv/play", &admin, json!({ "item_id": item })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let next = async |seen: &mut tokio::sync::mpsc::UnboundedReceiver<String>| {
        tokio::time::timeout(std::time::Duration::from_secs(10), seen.recv()).await.unwrap().unwrap()
    };
    let load = match tokio::time::timeout(std::time::Duration::from_secs(10), seen.recv()).await {
        Ok(Some(l)) => l,
        _ => panic!("no LOAD; status {:?}", h.st.casts.status("tv")),
    };
    let url = load.strip_prefix("LOAD ").unwrap();
    assert!(url.starts_with("http://127.0.0.1:") && url.contains("/dlna/media/"), "{url}");
    // The device fetches the file itself, over the network.
    let bytes = reqwest::get(url).await.unwrap().bytes().await.unwrap();
    assert!(bytes.starts_with(b"\x89PNG"));
    let forged = url.replace("/dlna/media/1/", "/dlna/media/2/");
    assert_eq!(reqwest::get(&forged).await.unwrap().status(), reqwest::StatusCode::NOT_FOUND);
    // Status follows the device; commands reach it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (_, v, _) = get(app, "/api/v1/cast/tv", &admin).await;
        if v["state"] == "playing" {
            assert_eq!(v["item_id"], item);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    send(app, Method::POST, "/api/v1/cast/tv/command", &admin, json!({ "action": "pause" })).await;
    assert_eq!(next(&mut seen).await, "PAUSE");
    send(app, Method::POST, "/api/v1/cast/tv/command", &admin, json!({ "action": "seek", "position_ms": 3000 })).await;
    assert_eq!(next(&mut seen).await, "SEEK");
    let (s, _, _) = send(app, Method::POST, "/api/v1/cast/nope/command", &admin, json!({ "action": "play" })).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Someone else's running cast isn't theirs to drive.
    send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "guest1", "password": "long enough pw", "role": "user" }),
    )
    .await;
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "guest1", "password": "long enough pw", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let other = v["token"].as_str().unwrap().to_string();
    let (s, _, _) = send(app, Method::POST, "/api/v1/cast/tv/command", &other, json!({ "action": "pause" })).await;
    assert_eq!(s, StatusCode::CONFLICT);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn podcasts_download_into_their_library() {
    let h = harness("podcasts", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    // A feed and its episodes on loopback.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let rss = format!(
        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Test Cast</title>
<item><title>Third</title><pubDate>Sat, 03 Oct 2026 10:00:00 GMT</pubDate><enclosure url="{base}/e3.mp3" type="audio/mpeg"/></item>
<item><title>Second</title><pubDate>Fri, 02 Oct 2026 10:00:00 GMT</pubDate><enclosure url="{base}/e2.mp3" type="audio/mpeg"/></item>
<item><title>First</title><pubDate>Thu, 01 Oct 2026 10:00:00 GMT</pubDate><enclosure url="{base}/e1.mp3" type="audio/mpeg"/></item>
</channel></rss>"#
    );
    let feed = axum::Router::new().route("/feed.xml", axum::routing::get(move || async move { rss })).route(
        "/{ep}",
        axum::routing::get(
            |axum::extract::Path(ep): axum::extract::Path<String>| async move { format!("audio of {ep}") },
        ),
    );
    tokio::spawn(async move { axum::serve(listener, feed).await.unwrap() });
    let folder = h.dir.join("Podcasts");
    std::fs::create_dir_all(&folder).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Podcasts", "kind": "podcasts", "folders": [folder] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    // Only podcast libraries, only real feeds.
    let (s, _, _) = send(
        app,
        Method::POST,
        "/api/v1/podcasts",
        &admin,
        json!({ "url": format!("{base}/e1.mp3"), "library_id": lib }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/podcasts",
        &admin,
        json!({ "url": format!("{base}/feed.xml"), "library_id": lib, "keep": 2 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["title"], "Test Cast");
    let (s, _, _) = send(
        app,
        Method::POST,
        "/api/v1/podcasts",
        &admin,
        json!({ "url": format!("{base}/feed.xml"), "library_id": lib }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let dir = folder.join("Test Cast");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, v, _) = get(app, "/api/v1/podcasts", &admin).await;
        if v[0]["episodes"] == 2 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let mut names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, vec!["2026-10-02 Second.mp3", "2026-10-03 Third.mp3"]);
    assert_eq!(std::fs::read_to_string(dir.join("2026-10-03 Third.mp3")).unwrap(), "audio of e3.mp3");
    let id = v_id(app, &admin).await;
    let (s, _, _) = send(app, Method::DELETE, &format!("/api/v1/podcasts/{id}"), &admin, json!({})).await;
    assert_eq!(s, StatusCode::OK);
    assert!(dir.join("2026-10-03 Third.mp3").exists(), "unsubscribing keeps the episodes");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn radio_stations_relay_their_stream() {
    let h = harness("radio", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let pls = format!("[playlist]\nFile1={base}/live\n");
    let radio = axum::Router::new()
        .route(
            "/listen.pls",
            axum::routing::get(move || async move { ([(header::CONTENT_TYPE, "audio/x-scpls")], pls) }),
        )
        .route("/live", axum::routing::get(|| async { ([(header::CONTENT_TYPE, "audio/mpeg")], "ID3 stream bytes") }));
    tokio::spawn(async move { axum::serve(listener, radio).await.unwrap() });
    let (s, _, _) = send(app, Method::POST, "/api/v1/radio", &admin, json!({ "name": "", "url": "ftp://x" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/radio",
        &admin,
        json!({ "name": "Test FM", "url": format!("{base}/listen.pls") }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(get(app, "/api/v1/radio", &admin).await.1[0]["name"], "Test FM");
    let (s, body, hd) = get(app, &format!("/api/v1/radio/{id}/stream"), &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(hd[header::CONTENT_TYPE], "audio/mpeg");
    assert_eq!(body, "ID3 stream bytes");
    send(app, Method::DELETE, &format!("/api/v1/radio/{id}"), &admin, json!({})).await;
    assert_eq!(get(app, "/api/v1/radio", &admin).await.1, json!([]));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watched_by_everyone_counts_every_account() {
    let h = harness("watched", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let pics = h.dir.join("Vids").join("Batch");
    std::fs::create_dir_all(&pics).unwrap();
    let made = std::process::Command::new("mpv")
        .args([
            "--really-quiet",
            "av://lavfi:testsrc=duration=2:size=160x120:rate=10,format=yuv420p",
            "--of=matroska",
            "--ovc=libx264",
            "--ovcopts=preset=ultrafast",
        ])
        .arg(format!("--o={}", pics.join("clip.mkv").display()))
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("mpv not available; skipping");
        h.stop().await;
        return;
    }
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Vids", "kind": "videos", "folders": [h.dir.join("Vids")] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}&kind=video"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let w = |p: std::path::PathBuf| {
        let st = h.st.clone();
        async move { st.library.watched_by_everyone(lib, &p).await.unwrap() }
    };
    assert_eq!(w(h.dir.join("elsewhere")).await, None);
    assert_eq!(w(pics.clone()).await, Some(false));
    send(app, Method::PUT, &format!("/api/v1/items/{item}/state"), &admin, json!({ "played": true })).await;
    assert_eq!(w(pics.clone()).await, Some(true));
    // A second account that hasn't watched it yet.
    send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "viewer", "password": "long enough pw", "role": "user" }),
    )
    .await;
    assert_eq!(w(pics.clone()).await, Some(false));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nfo_files_are_written_and_foreign_ones_kept() {
    let h = harness("nfo", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let dir = h.dir.join("Movies").join("Clip (2020)");
    std::fs::create_dir_all(&dir).unwrap();
    let made = std::process::Command::new("mpv")
        .args([
            "--really-quiet",
            "av://lavfi:testsrc=duration=2:size=160x120:rate=10,format=yuv420p",
            "--of=matroska",
            "--ovc=libx264",
            "--ovcopts=preset=ultrafast",
        ])
        .arg(format!("--o={}", dir.join("Clip (2020).mkv").display()))
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("mpv not available; skipping");
        h.stop().await;
        return;
    }
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Movies", "kind": "movies", "folders": [h.dir.join("Movies")], "settings": { "write_nfo": true, "watch": false } }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let (s, v, _) = send(
        app,
        Method::PATCH,
        &format!("/api/v1/items/{item}"),
        &admin,
        json!({ "title": "Clip & Co", "overview": "Test pattern." }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let nfo = std::fs::read_to_string(dir.join("Clip (2020).nfo")).unwrap();
    assert!(
        nfo.contains(corrent_library::nfo::MARK)
            && nfo.contains("<title>Clip &amp; Co</title>")
            && nfo.contains("<plot>Test pattern.</plot>"),
        "{nfo}"
    );
    // Someone else's .nfo stays as it is.
    std::fs::write(dir.join("Clip (2020).nfo"), "<movie><title>Mine</title></movie>").unwrap();
    send(app, Method::PATCH, &format!("/api/v1/items/{item}"), &admin, json!({ "title": "Other" })).await;
    assert_eq!(std::fs::read_to_string(dir.join("Clip (2020).nfo")).unwrap(), "<movie><title>Mine</title></movie>");
    h.stop().await;
}

async fn v_id(app: &Router, admin: &str) -> String {
    get(app, "/api/v1/podcasts", admin).await.1[0]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn https_with_the_servers_own_ca() {
    let h = harness("https", ServerConfig::default()).await;
    let tls =
        crate::tls::ensure(&h.dir.join("tls"), &h.st.server_id, &["localhost".into()], &["127.0.0.1".parse().unwrap()])
            .unwrap();
    let listener = crate::serve_https(h.app.clone(), "127.0.0.1:0".parse().unwrap(), &tls).await.unwrap();
    let ca = reqwest::Certificate::from_pem(tls.ca_pem.as_bytes()).unwrap();
    let client = reqwest::Client::builder().tls_certs_only([ca]).build().unwrap();
    let url = format!("https://localhost:{}/api/v1/server/info", listener.addr.port());
    let v: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["api_version"], 1);
    // Without the CA the connection is refused (self-signed).
    assert!(reqwest::Client::new().get(&url).send().await.is_err());
    // A client that never finishes its request headers is dropped after HEADER_TIMEOUT.
    {
        use rustls::pki_types::pem::PemObject;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from_pem_slice(tls.ca_pem.as_bytes()).unwrap()).unwrap();
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect(("127.0.0.1", listener.addr.port())).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut s = tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp).await.unwrap();
        s.write_all(b"GET /api/v1/server/info HTTP/1.1\r\nHost: localhost\r\n").await.unwrap();
        let started = std::time::Instant::now();
        let mut buf = [0u8; 512];
        let r = tokio::time::timeout(crate::HEADER_TIMEOUT + std::time::Duration::from_secs(5), s.read(&mut buf)).await;
        assert!(r.is_ok(), "the half-sent request was still open after {:?}", started.elapsed());
        assert!(started.elapsed() >= crate::HEADER_TIMEOUT - std::time::Duration::from_secs(1));
    }
    drop(listener);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn library_endpoints() {
    let h = harness("library", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    // Photos need no probing, so this runs without libmpv.
    let photos = h.dir.join("Photos/Trip");
    std::fs::create_dir_all(&photos).unwrap();
    image::RgbImage::from_fn(64, 48, |x, _| image::Rgb([x as u8, 0, 0])).save(photos.join("beach.png")).unwrap();
    image::RgbImage::from_fn(64, 48, |_, y| image::Rgb([0, y as u8, 0])).save(photos.join("forest.png")).unwrap();
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Photos", "kind": "photos", "folders": [h.dir.join("Photos")] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let items = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if v["total"] == 2 {
            break v;
        }
        assert!(std::time::Instant::now() < deadline, "scan finished: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let first = &items["items"][0];
    assert_eq!(first["title"], "beach");
    let sha = first["images"][0]["sha"].as_str().unwrap().to_string();
    // Artwork is served with an immutable cache policy; Range works on files.
    let (s, _, hd) = get(app, &format!("/api/v1/images/{sha}?w=200"), &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert!(hd.get(header::CACHE_CONTROL).unwrap().to_str().unwrap().contains("immutable"));
    assert_eq!(get(app, "/api/v1/images/../../etc/passwd", &admin).await.0, StatusCode::NOT_FOUND);
    let id = first["id"].as_i64().unwrap();
    let res = h
        .app
        .clone()
        .oneshot({
            let mut r = Request::builder()
                .uri(format!("/api/v1/items/{id}/file"))
                .header(header::HOST, "127.0.0.1:8443")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .header(header::RANGE, "bytes=0-9")
                .body(Body::empty())
                .unwrap();
            r.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
            r
        })
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.into_body().collect().await.unwrap().to_bytes().len(), 10);
    // Watch state, search, home.
    let (s, v, _) =
        send(app, Method::PUT, &format!("/api/v1/items/{id}/state"), &admin, json!({ "favorite": true })).await;
    assert_eq!((s, v["favorite"].as_bool()), (StatusCode::OK, Some(true)));
    let (_, v, _) = get(app, "/api/v1/library/search?q=fore", &admin).await;
    assert_eq!(v["items"][0]["title"], "forest");
    let (_, v, _) = get(app, "/api/v1/home", &admin).await;
    assert!(v.as_array().unwrap().iter().any(|r| r["id"] == "favorites"));
    // A guest limited to no libraries sees nothing of this one; library management needs the permission.
    let (_, guest, _) = send(
        app,
        Method::POST,
        "/api/v1/users",
        &admin,
        json!({ "name": "g", "password": "guest password", "role": "guest" }),
    )
    .await;
    let gid = guest["id"].as_i64().unwrap();
    let (s, _, _) = send(app, Method::PUT, &format!("/api/v1/users/{gid}/libraries"), &admin, json!([999])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unknown library");
    let other = h.dir.join("dl/videos");
    std::fs::create_dir_all(&other).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Other", "kind": "videos", "folders": [other] }),
    )
    .await;
    let other_id = v["id"].as_i64().unwrap();
    let (s, _, _) = send(app, Method::PUT, &format!("/api/v1/users/{gid}/libraries"), &admin, json!([other_id])).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v, _) = call(
        app,
        Method::POST,
        "/api/v1/auth/login",
        Call {
            body: Some(json!({ "username": "g", "password": "guest password", "device": device() })),
            ..Default::default()
        },
    )
    .await;
    let g = v["token"].as_str().unwrap().to_string();
    assert_eq!(get(app, &format!("/api/v1/items/{id}"), &g).await.0, StatusCode::NOT_FOUND);
    assert_eq!(get(app, "/api/v1/libraries", &g).await.1.as_array().unwrap().len(), 1, "only the allowed one");
    let (s, _, _) =
        send(app, Method::POST, "/api/v1/libraries", &g, json!({ "name": "x", "kind": "videos", "folders": ["/"] }))
            .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn playback_direct_and_hls() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let h = harness("playback", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let movies = h.dir.join("dl/Movies/Clip (2024)");
    std::fs::create_dir_all(&movies).unwrap();
    let job = corrent_mpv::encode::EncodeJob {
        input: "av://lavfi:testsrc=duration=12:size=320x240:rate=25,format=yuv420p".into(),
        audio_file: Some("av://lavfi:sine=duration=12".into()),
        output: movies.join("Clip.2024.mkv").display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=ultrafast".into()),
            ("oac".into(), "aac".into()),
        ],
    };
    corrent_mpv::encode::run(&job, std::time::Duration::from_secs(60), &mut |_| true).unwrap();
    std::fs::write(movies.join("Clip.2024.en.srt"), "1\n00:00:01,000 --> 00:00:02,000\nHello\n").unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Movies", "kind": "movies", "folders": [h.dir.join("dl/Movies")] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if v["total"] == 1 && h.st.hls_encoder.lock().unwrap().is_some() {
            break v["items"][0]["id"].as_i64().unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "scan and encoder detection finished: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    // Native: direct.
    let (s, v, _) =
        send(app, Method::POST, &format!("/api/v1/playback/{item}"), &admin, json!({ "profile": "native" })).await;
    assert_eq!((s, v["method"].as_str()), (StatusCode::OK, Some("direct")), "{v}");
    // Web: H.264 + AAC in Matroska is copied into HLS/fMP4 (direct stream), no session.
    let (s, v, _) =
        send(app, Method::POST, &format!("/api/v1/playback/{item}"), &admin, json!({ "profile": "web" })).await;
    assert_eq!((s, v["method"].as_str(), v["session_id"].as_str()), (StatusCode::OK, Some("remux"), None), "{v}");
    let rurl = v["url"].as_str().unwrap().to_string();
    let (s, pl, _) = get(app, &rurl, &admin).await;
    assert_eq!(s, StatusCode::OK);
    let pl = pl.as_str().unwrap().to_string();
    // Segment URLs carry the file's version, which is also their ETag.
    let version = pl.split("init.mp4?v=").nth(1).and_then(|r| r.split('"').next()).unwrap_or_default().to_string();
    assert!(!version.is_empty() && pl.contains(&format!("\n0.m4s?v={version}\n")), "{pl}");
    let fetch = |name: String, etag: Option<String>| {
        let mut r = Request::builder()
            .uri(rurl.replace("index.m3u8", &name))
            .header(header::HOST, "127.0.0.1:8443")
            .header(header::AUTHORIZATION, format!("Bearer {admin}"));
        if let Some(e) = etag {
            r = r.header(header::IF_NONE_MATCH, e);
        }
        let mut r = r.body(Body::empty()).unwrap();
        r.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
        h.app.clone().oneshot(r)
    };
    for name in ["init.mp4", "0.m4s"] {
        let res = fetch(format!("{name}?v={version}"), None).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{name}");
        assert_eq!(res.headers()[header::ETAG], format!("\"{version}\"").as_str());
        let b = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&b[4..8], if name == "init.mp4" { b"ftyp" } else { b"moof" }, "{name}");
        let res = fetch(name.to_string(), Some(format!("\"{version}\""))).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED, "{name}");
        let res = fetch(name.to_string(), Some("\"stale\"".into())).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{name}");
    }
    let (s, _, _) = get(app, &rurl.replace("index.m3u8", "999.m4s"), &admin).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Forced: a transcode; the subtitle is offered as WebVTT.
    let (s, v, _) = send(
        app,
        Method::POST,
        &format!("/api/v1/playback/{item}"),
        &admin,
        json!({ "profile": "web", "force_transcode": true }),
    )
    .await;
    assert_eq!((s, v["method"].as_str()), (StatusCode::OK, Some("hls")), "{v}");
    let sub_url = v["subtitles"][0]["url"].as_str().unwrap_or_else(|| panic!("{v}")).to_string();
    let (s, vtt, _) = get(app, &sub_url, &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert!(vtt.as_str().unwrap().contains("00:00:01.000 --> 00:00:02.000\nHello"));
    let url = v["url"].as_str().unwrap().to_string();
    let (s, pl, _) = get(app, &url, &admin).await;
    assert_eq!(s, StatusCode::OK);
    assert!(pl.as_str().unwrap().contains("0.ts"), "{pl}");
    let seg = url.replace("index.m3u8", "1.ts");
    let res = h
        .app
        .clone()
        .oneshot({
            let mut r = Request::builder()
                .uri(&seg)
                .header(header::HOST, "127.0.0.1:8443")
                .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                .body(Body::empty())
                .unwrap();
            r.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
            r
        })
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.into_body().collect().await.unwrap().to_bytes().len() > 1000);
    let (_, v, _) = get(app, "/api/v1/playback/sessions", &admin).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    let sid = v[0]["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        app,
        Method::DELETE,
        &format!("/api/v1/playback/sessions/{sid}"),
        Call { bearer: Some(&admin), ..Default::default() },
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_downloads_import_into_libraries() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let mut h = harness("import", ServerConfig { tls: false, ..Default::default() }).await;
    // Engine events reach the integration (the test harness owns the receiver).
    let events = h.l_events();
    crate::forward_engine_events(events, h.st.clone());
    let app = &h.app;
    let admin = admin_token(app).await;
    // A movie to seed.
    let src = h.dir.join("src.mkv");
    let job = corrent_mpv::encode::EncodeJob {
        input: "av://lavfi:testsrc=duration=4:size=160x120:rate=10,format=yuv420p".into(),
        audio_file: None,
        output: src.display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=ultrafast".into()),
        ],
    };
    corrent_mpv::encode::run(&job, std::time::Duration::from_secs(60), &mut |_| true).unwrap();
    let seeder = testkit::seeder_with(&h.dir, "Big.Buck.Bunny.2008.mkv", std::fs::read(&src).unwrap()).await;

    // A library fed by the "Movies" category gets that category's folder.
    let folder = h.dir.join("dl/Movies");
    std::fs::create_dir_all(&folder).unwrap();
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({
            "name": "Movies", "kind": "movies", "folders": [folder],
            "settings": { "import_categories": ["Movies"], "watch": false, "seed_ratio": 2.5 }
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let (_, v, _) = get(app, "/api/v1/torrents/categories", &admin).await;
        if v["categories"]["Movies"]["save_path"].as_str().is_some_and(|p| p.ends_with("Movies")) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "category folder set: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    h.st.engine.send(corrent_core::engine::EngineCommand::Add(Box::new(corrent_core::engine::AddRequest {
        source: corrent_core::engine::types::AddSource::Bytes { name: "x.torrent".into(), bytes: seeder.bytes.clone() },
        torrent_bytes: None,
        save_path: folder.clone(),
        create_subfolder: false,
        category: Some("Movies".into()),
        tags: vec![],
        paused: false,
        only_files: None,
        add_to_top: false,
        initial_peers: Some(vec![seeder.addr]),
        added_by: Some("admin".into()),
    })));
    // Completed → imported in place → scanned → the movie is in the library.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if v["total"] == 1 {
            break v["items"][0].clone();
        }
        assert!(std::time::Instant::now() < deadline, "imported: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    assert_eq!((item["title"].as_str(), item["year"].as_i64()), (Some("Big Buck Bunny"), Some(2008)));
    // The library's seeding goal was applied when the torrent was added.
    let hash = seeder.hash.to_hex();
    // Removing it with its data needs confirmation, because library files are inside.
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/torrents/actions",
        &admin,
        json!({ "action": "remove_with_data", "hashes": [hash] }),
    )
    .await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::CONFLICT, Some("conflict")), "{v}");
    assert_eq!(v["params"]["files"], "1");
    let (s, _, _) =
        send(app, Method::POST, "/api/v1/torrents/actions", &admin, json!({ "action": "remove", "hashes": [hash] }))
            .await;
    assert_eq!(s, StatusCode::OK, "removing without data is fine");
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wanted_list_and_profiles() {
    let h = harness("wanted", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let (_, v, _) = get(app, "/api/v1/quality-profiles", &admin).await;
    assert!(v.as_array().unwrap().len() >= 3, "default profiles: {v}");
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/quality-profiles",
        &admin,
        json!({ "name": "Small", "rules": { "max_resolution": 720 } }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v, _) = send(
        app,
        Method::POST,
        "/api/v1/wanted",
        &admin,
        json!({ "title": "Sintel", "year": 2010, "profile_id": v["id"] }),
    )
    .await;
    assert_eq!((s, v["kind"].as_str(), v["status"].as_str()), (StatusCode::OK, Some("movie"), Some("wanted")), "{v}");
    let id = v["id"].as_i64().unwrap();
    // No providers configured: searching finds nothing (and doesn't fail).
    let (s, v, _) = send(app, Method::POST, &format!("/api/v1/wanted/{id}/search"), &admin, json!({})).await;
    assert_eq!((s, v.as_array().map(Vec::len)), (StatusCode::OK, Some(0)));
    let (_, v, _) = get(app, "/api/v1/wanted?status=wanted", &admin).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    let (s, _, _) =
        call(app, Method::DELETE, &format!("/api/v1/wanted/{id}"), Call { bearer: Some(&admin), ..Default::default() })
            .await;
    assert_eq!(s, StatusCode::OK);
    // A file of an unfinished (paused, empty) download is found for streaming.
    let seeder = testkit::seeder(&h.dir).await;
    h.st.engine.send(corrent_core::engine::EngineCommand::Add(Box::new(corrent_core::engine::AddRequest {
        source: corrent_core::engine::types::AddSource::Bytes { name: "x.torrent".into(), bytes: seeder.bytes.clone() },
        torrent_bytes: None,
        save_path: h.dir.join("dl"),
        create_subfolder: false,
        category: None,
        tags: vec![],
        paused: true,
        only_files: None,
        add_to_top: false,
        initial_peers: None,
        added_by: None,
    })));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some((hash, idx, name)) = crate::integrate::downloading_file(&h.st, &h.dir.join("dl/data.bin")).await {
            assert_eq!((hash, idx, name.as_str()), (seeder.hash, 0, "data.bin"));
            break;
        }
        assert!(std::time::Instant::now() < deadline, "downloading file found");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plugins_identify_library_items() {
    let h = harness("plugins", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    // Built-in providers are listed; TMDB needs its key first.
    let (_, v, _) = get(app, "/api/v1/plugins", &admin).await;
    let tmdb = v.as_array().unwrap().iter().find(|p| p["manifest"]["id"] == "tmdb").cloned().unwrap();
    assert_eq!(tmdb["missing"], json!(["api_key"]));
    // Install the example provider (multipart: manifest + wasm).
    let boundary = "plugin-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(include_bytes!("../../corrent-plugins/builtin/example.toml"));
    body.extend_from_slice(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"wasm\"; filename=\"plugin.wasm\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(include_bytes!("../../corrent-plugins/builtin/example.wasm"));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/plugins")
        .header(header::HOST, "127.0.0.1:8443")
        .header(header::AUTHORIZATION, format!("Bearer {admin}"))
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Only the example answers in tests (no network, no keys): turn MusicBrainz off.
    let (s, _, _) =
        send(app, Method::PUT, "/api/v1/plugins/musicbrainz/enabled", &admin, json!({ "enabled": false })).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v, _) =
        send(app, Method::PUT, "/api/v1/plugins/example/settings", &admin, json!({ "greeting": "Merhaba" })).await;
    assert_eq!((s, v["settings"]["greeting"].as_str()), (StatusCode::OK, Some("Merhaba")));

    // A movie (the file doesn't need to be real media for naming and metadata).
    let movies = h.dir.join("dl/Movies");
    std::fs::create_dir_all(movies.join("Sintel (2010)")).unwrap();
    std::fs::write(movies.join("Sintel (2010)/Sintel.2010.mkv"), b"not really a movie").unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Movies", "kind": "movies", "folders": [movies] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let id = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if v["items"][0]["title"].as_str().is_some_and(|t| t.starts_with("Example")) {
            break v["items"][0]["id"].as_i64().unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "identified: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
    assert_eq!(d["overview"], "Merhaba (en)", "{d}");
    assert_eq!(d["genres"], json!(["Animation", "Fantasy"]));
    assert_eq!(d["provider_ids"]["example"], "ex-sintel");
    assert!(d["credits"].as_array().unwrap().iter().any(|c| c["name"] == "Colin Levy"));
    // Identify by hand: candidates, then apply another one.
    let (s, c, _) = get(app, &format!("/api/v1/items/{id}/identify?title=Big%20Buck%20Bunny&year=2008"), &admin).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c[0]["provider"], "example");
    let (s, _, _) = send(
        app,
        Method::POST,
        &format!("/api/v1/items/{id}/identify"),
        &admin,
        json!({ "provider": "example", "id": c[0]["id"] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
    assert_eq!(d["provider_ids"]["example"], "ex-big-buck-bunny");

    // A followed show: the aired episodes it lacks become wanted (the example lists two
    // seasons of three episodes, aired in 2020).
    let shows = h.dir.join("dl/Shows");
    std::fs::create_dir_all(shows.join("Test Show/Season 1")).unwrap();
    std::fs::write(shows.join("Test Show/Season 1/Test.Show.S01E01.mkv"), b"not really").unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Shows", "kind": "shows", "folders": [shows] }),
    )
    .await;
    let shows_lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let show = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={shows_lib}&kind=show"), &admin).await;
        let id = v["items"][0]["id"].as_i64();
        if let Some(id) = id {
            let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
            if d["provider_ids"]["example"].is_string() {
                break id;
            }
        }
        assert!(std::time::Instant::now() < deadline, "show identified: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    let (s, v, _) = send(app, Method::PUT, &format!("/api/v1/items/{show}/monitor"), &admin, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let wanted = loop {
        let (_, w, _) = get(app, "/api/v1/wanted", &admin).await;
        if w.as_array().is_some_and(|a| a.len() >= 5) {
            break w;
        }
        assert!(std::time::Instant::now() < deadline, "episodes wanted: {w}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let eps: Vec<(i64, i64)> = wanted
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["season"].as_i64().unwrap(), w["episode"].as_i64().unwrap()))
        .collect();
    assert_eq!(eps.len(), 5, "S01E01 is in the library: {eps:?}");
    assert!(!eps.contains(&(1, 1)));
    // Checking again adds nothing new.
    let r = run_task(app, &admin, "monitor_shows").await;
    assert_eq!(r["ok"], true, "{r}");
    let (_, w, _) = get(app, "/api/v1/wanted", &admin).await;
    assert_eq!(w.as_array().unwrap().len(), 5);
    let (_, m, _) = get(app, "/api/v1/monitored", &admin).await;
    assert_eq!(m[0]["item_id"], show);

    // Built-in plugins can't be removed; installed ones can.
    let (s, _, _) =
        call(app, Method::DELETE, "/api/v1/plugins/tmdb", Call { bearer: Some(&admin), ..Default::default() }).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _, _) =
        call(app, Method::DELETE, "/api/v1/plugins/example", Call { bearer: Some(&admin), ..Default::default() }).await;
    assert_eq!(s, StatusCode::OK);
    h.stop().await;
}

/// docs/API.md lists every route of `/api/v1` (fails when one is added without documenting it).
#[test]
fn api_doc_lists_every_route() {
    let doc = include_str!("../../../docs/API.md");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/v1");
    let mut missing = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let src = std::fs::read_to_string(e.path()).unwrap();
        for part in src.split(".route(").skip(1) {
            let Some(path) = part.trim_start().strip_prefix('"').and_then(|p| p.split('"').next()) else { continue };
            if !doc.contains(&format!("`/api/v1{path}`")) {
                missing.push(path.to_string());
            }
        }
    }
    assert!(missing.is_empty(), "add these to docs/API.md: {missing:?}");
}

/// A movie without artwork gets a frame as its thumbnail and poster after the scan; a frame the
/// user picks is locked against providers and later frames.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn videos_without_artwork_get_a_frame() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let h = harness("frames", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let dir = h.dir.join("Movies").join("Clip (2020)");
    std::fs::create_dir_all(&dir).unwrap();
    let job = corrent_mpv::encode::EncodeJob {
        input: "av://lavfi:testsrc=duration=8:size=320x180:rate=10,format=yuv420p".into(),
        output: dir.join("Clip (2020).mkv").display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=ultrafast".into()),
        ],
        audio_file: None,
    };
    corrent_mpv::encode::run(&job, std::time::Duration::from_secs(60), &mut |_| true).unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Movies", "kind": "movies", "folders": [h.dir.join("Movies")], "settings": { "watch": false } }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    // The scan's end starts a thumbnail pass; the task does the same.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let (item, images) = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        let kinds: Vec<String> = v["items"][0]["images"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["kind"].as_str().map(String::from))
            .collect();
        if kinds.contains(&"thumb".to_string()) {
            break (v["items"][0]["id"].as_i64().unwrap(), v["items"][0]["images"].clone());
        }
        assert!(std::time::Instant::now() < deadline, "{v}");
        run_task(app, &admin, "video_thumbnails").await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };
    let kinds: Vec<&str> = images.as_array().unwrap().iter().filter_map(|i| i["kind"].as_str()).collect();
    assert!(kinds.contains(&"poster"), "a movie without a poster shows the frame: {images}");
    let sha = |imgs: &Value, k: &str| {
        imgs.as_array().unwrap().iter().find(|i| i["kind"] == k).and_then(|i| i["sha"].as_str()).unwrap().to_string()
    };
    let auto = sha(&images, "thumb");
    // The user picks another moment: stored and locked.
    let (s, r, _) =
        send(app, Method::POST, &format!("/api/v1/items/{item}/thumbnail"), &admin, json!({ "at_ms": 6500 })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["stored"], true);
    let (_, d, _) = get(app, &format!("/api/v1/items/{item}"), &admin).await;
    let picked = sha(&d["images"], "thumb");
    assert_ne!(picked, auto, "a different frame");
    // Neither provider artwork nor a later frame replaces it.
    let png = {
        let mut b = Vec::new();
        image::RgbImage::from_pixel(64, 36, image::Rgb([1, 2, 3]))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    use corrent_library::meta::Replace;
    let lib_ = &h.st.library;
    assert!(
        !lib_
            .set_artwork(item, "thumb", png.clone(), "https://provider/x.jpg", Replace::IfMissingOrFrame, false)
            .await
            .unwrap()
    );
    assert!(!lib_.set_artwork(item, "thumb", png.clone(), "frame", Replace::Always, false).await.unwrap());
    // The automatic poster frame does give way to provider artwork.
    assert!(
        lib_.set_artwork(item, "poster", png, "https://provider/p.jpg", Replace::IfMissingOrFrame, false)
            .await
            .unwrap()
    );
    // An item that isn't there.
    let (s, _, _) = send(app, Method::POST, "/api/v1/items/999999/thumbnail", &admin, json!({})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    h.stop().await;
}

/// "Add to library…": a finished download goes into a movie library with clean names, hard-linked
/// (still seeding) or moved out (the torrent is removed, its folder goes).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_download_goes_into_a_library_with_clean_names() {
    let h = harness("add-to-library", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let release = "Sintel.2010.1080p.BluRay.x264-GRP";
    let data = testkit::content();
    let s = testkit::seeder_files(
        &h.dir,
        release,
        &[
            (&format!("{release}.mkv"), data[..200_000].to_vec()),
            (&format!("{release}.en.srt"), b"1\n00:00:01,000 --> 00:00:02,000\nHi\n".to_vec()),
        ],
    )
    .await;
    testkit::add(&h.l, &s, &h.dir);
    testkit::wait_for(&h.l, "the download", 60, |sn| testkit::complete(sn, s.hash)).await;
    let mut libs = Vec::new();
    for (name, kind) in [("Linked", "movies"), ("Moved", "movies"), ("Songs", "music")] {
        let folder = h.dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let (st, v, _) = send(
            app,
            Method::POST,
            "/api/v1/libraries",
            &admin,
            json!({ "name": name, "kind": kind, "folders": [folder], "settings": { "watch": false } }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        libs.push((v["id"].as_i64().unwrap(), folder));
    }
    let url = format!("/api/v1/torrents/{}/import", s.hash.to_hex());
    let movie = std::path::Path::new("Sintel (2010)");
    // A dry run says where things would go and changes nothing.
    let (st, plan, _) = send(
        app,
        Method::POST,
        &url,
        &admin,
        json!({ "library_id": libs[0].0, "action": "hardlink", "dry_run": true }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    let to: Vec<String> =
        plan["files"].as_array().unwrap().iter().map(|f| f["to"].as_str().unwrap().to_string()).collect();
    let linked_movie = libs[0].1.join(movie).join("Sintel (2010).mkv");
    assert!(to.contains(&linked_movie.display().to_string()), "{plan}");
    assert!(to.contains(&libs[0].1.join(movie).join("Sintel (2010).en.srt").display().to_string()), "{plan}");
    assert!(!linked_movie.exists());
    // Hard links: the files are there and the torrent keeps seeding.
    let (st, plan, _) =
        send(app, Method::POST, &url, &admin, json!({ "library_id": libs[0].0, "action": "hardlink" })).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    assert_eq!(std::fs::read(&linked_movie).unwrap(), data[..200_000]);
    assert!(testkit::find(&h.l.engine.snapshot(), s.hash).is_some(), "still seeding");
    // Not a video library.
    let (st, _, _) = send(app, Method::POST, &url, &admin, json!({ "library_id": libs[2].0, "action": "copy" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Moved out: into the library, the torrent removed, its folder gone.
    let content = testkit::find(&h.l.engine.snapshot(), s.hash).unwrap().content_path.clone();
    let (st, plan, _) =
        send(app, Method::POST, &url, &admin, json!({ "library_id": libs[1].0, "action": "move_out" })).await;
    assert_eq!(st, StatusCode::OK, "{plan}");
    let moved = libs[1].1.join(movie).join("Sintel (2010).mkv");
    assert_eq!(std::fs::read(&moved).unwrap(), data[..200_000]);
    assert!(libs[1].1.join(movie).join("Sintel (2010).en.srt").exists());
    assert!(testkit::find(&h.l.engine.snapshot(), s.hash).is_none(), "no longer a torrent");
    assert!(!content.exists(), "the download's folder is gone: {}", content.display());
    assert!(linked_movie.exists(), "the earlier hard link is untouched");
    h.stop().await;
}

/// Items scanned while no provider could answer (no key yet) are identified as soon as one is
/// ready, not left "checked" for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn items_wait_for_a_provider_and_are_identified_once_one_is_ready() {
    let h = harness("meta-later", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let (s, _, _) =
        send(app, Method::PUT, "/api/v1/plugins/musicbrainz/enabled", &admin, json!({ "enabled": false })).await;
    assert_eq!(s, StatusCode::OK);
    let movies = h.dir.join("dl/Movies");
    std::fs::create_dir_all(movies.join("Sintel (2010)")).unwrap();
    std::fs::write(movies.join("Sintel (2010)/Sintel.2010.mkv"), b"not really a movie").unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Movies", "kind": "movies", "folders": [movies] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let id = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline, "scanned: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    // No provider is ready (TMDB has no key): nothing to match against, yet.
    run_task(app, &admin, "refresh_metadata").await;
    // Then one is (here: installing the example provider).
    let boundary = "plugin-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(include_bytes!("../../corrent-plugins/builtin/example.toml"));
    body.extend_from_slice(
        format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"wasm\"; filename=\"plugin.wasm\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(include_bytes!("../../corrent-plugins/builtin/example.wasm"));
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/plugins")
        .header(header::HOST, "127.0.0.1:8443")
        .header(header::AUTHORIZATION, format!("Bearer {admin}"))
        .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
        if d["provider_ids"]["example"].is_string() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "identified after the provider came: {d}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    // Rating sites (what OMDb adds) are kept with the item; a key that isn't a plain name isn't.
    let ratings: std::collections::BTreeMap<String, String> =
        [("imdb", "7.4"), ("rotten_tomatoes", "85%"), ("x') OR 1=1 --", "0")]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
    h.st.library.set_ratings(id, ratings).await.unwrap();
    let (_, d, _) = get(app, &format!("/api/v1/items/{id}"), &admin).await;
    assert_eq!(d["extra"]["ratings"], json!({ "imdb": "7.4", "rotten_tomatoes": "85%" }), "{d}");
    h.stop().await;
}

/// An AI agent over MCP: it reads the library, proposes a change, and nothing changes until a
/// person approves it (then the fields stay as approved); a rejected proposal changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_proposes_and_a_person_decides() {
    let h = harness("agent", ServerConfig { tls: false, ..Default::default() }).await;
    let app = &h.app;
    let admin = admin_token(app).await;
    let movies = h.dir.join("dl/Movies");
    std::fs::create_dir_all(movies.join("Sintl (2010)")).unwrap();
    std::fs::write(movies.join("Sintl (2010)/Sintl.2010.mkv"), b"not really a movie").unwrap();
    let (_, v, _) = send(
        app,
        Method::POST,
        "/api/v1/libraries",
        &admin,
        json!({ "name": "Movies", "kind": "movies", "folders": [movies] }),
    )
    .await;
    let lib = v["id"].as_i64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let item = loop {
        let (_, v, _) = get(app, &format!("/api/v1/items?library_id={lib}"), &admin).await;
        if let Some(id) = v["items"][0]["id"].as_i64() {
            break id;
        }
        assert!(std::time::Instant::now() < deadline, "scanned: {v}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let rpc = |id: i64, method: &str, params: Value| json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    // Handshake.
    let (s, r, headers) = send(app, Method::POST, "/api/v1/mcp", &admin, rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }))).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert!(headers.get("mcp-session-id").is_some());
    let (s, _, _) = send(
        app,
        Method::POST,
        "/api/v1/mcp",
        &admin,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "a notification has no answer");
    // Tools.
    let (_, r, _) = send(app, Method::POST, "/api/v1/mcp", &admin, rpc(2, "tools/list", json!({}))).await;
    let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    for n in ["list_libraries", "search_items", "get_item", "propose_details", "propose_import"] {
        assert!(names.contains(&n), "{names:?}");
    }
    let call = |id: i64, name: &str, args: Value| rpc(id, "tools/call", json!({ "name": name, "arguments": args }));
    let (_, r, _) =
        send(app, Method::POST, "/api/v1/mcp", &admin, call(3, "search_items", json!({ "library_id": lib }))).await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("Sintl"), "{r}");
    let (_, r, _) = send(app, Method::POST, "/api/v1/mcp", &admin, call(4, "list_unmatched", json!({}))).await;
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains(&format!("\"id\": {item}")), "{r}");
    // A proposal: nothing changes yet.
    let (_, r, _) = send(
        app,
        Method::POST,
        "/api/v1/mcp",
        &admin,
        call(5, "propose_details", json!({ "item_id": item, "title": "Sintel", "reason": "typo in the folder name" })),
    )
    .await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    let (_, d, _) = get(app, &format!("/api/v1/items/{item}"), &admin).await;
    assert_eq!(d["title"], "Sintl");
    let (_, list, _) = get(app, "/api/v1/agent/proposals?pending=true", &admin).await;
    let p = &list[0];
    assert_eq!(p["status"], "pending");
    assert!(
        p["summary"].as_str().unwrap().contains("Sintel") && p["summary"].as_str().unwrap().contains("typo"),
        "{p}"
    );
    // Approved: applied, and the title is kept from now on.
    let (s, done, _) =
        send(app, Method::POST, &format!("/api/v1/agent/proposals/{}/approve", p["id"]), &admin, json!({})).await;
    assert_eq!((s, done["status"].as_str()), (StatusCode::OK, Some("applied")), "{done}");
    let (_, d, _) = get(app, &format!("/api/v1/items/{item}"), &admin).await;
    assert_eq!(d["title"], "Sintel");
    assert!(d["locked_fields"].as_array().unwrap().iter().any(|f| f == "title"), "{d}");
    let (s, _, _) =
        send(app, Method::POST, &format!("/api/v1/agent/proposals/{}/approve", p["id"]), &admin, json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "decided once");
    // Rejected: nothing changes.
    let (_, r, _) = send(
        app,
        Method::POST,
        "/api/v1/mcp",
        &admin,
        call(6, "propose_details", json!({ "item_id": item, "year": 1999 })),
    )
    .await;
    let id =
        serde_json::from_str::<Value>(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap()["proposal"].clone();
    let (_, done, _) =
        send(app, Method::POST, &format!("/api/v1/agent/proposals/{id}/reject"), &admin, json!({})).await;
    assert_eq!(done["status"], "rejected");
    let (_, d, _) = get(app, &format!("/api/v1/items/{item}"), &admin).await;
    assert_eq!(d["year"], 2010);
    // Bad input is an error the agent can read, not a failure of the protocol.
    let (_, r, _) = send(app, Method::POST, "/api/v1/mcp", &admin, call(7, "get_item", json!({}))).await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    let (_, r, _) = send(app, Method::POST, "/api/v1/mcp", &admin, rpc(8, "no/such", json!({}))).await;
    assert_eq!(r["error"]["code"], -32601);
    // Without a token: nothing.
    let (s, _, _) = call_anon(app, Method::POST, "/api/v1/mcp", rpc(9, "tools/list", json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // The desktop app's agent key: the MCP endpoint takes it, nothing else does.
    let owner = h.st.identity.users().await.unwrap().into_iter().next().unwrap().id;
    *h.st.agent_key.lock().unwrap() = Some(("cm_test-key".into(), owner));
    let (s, r, _) = send(app, Method::POST, "/api/v1/mcp", "cm_test-key", call(10, "list_libraries", json!({}))).await;
    assert_eq!((s, &r["result"]["isError"]), (StatusCode::OK, &json!(false)), "{r}");
    let (s, _, _) = get(app, "/api/v1/libraries", "cm_test-key").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "only the MCP endpoint");
    let (s, _, _) = send(app, Method::POST, "/api/v1/mcp", "cm_wrong", call(11, "list_libraries", json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Two approvals at once: it runs once.
    let (_, r, _) = send(
        app,
        Method::POST,
        "/api/v1/mcp",
        "cm_test-key",
        call(12, "propose_details", json!({ "item_id": item, "overview": "A girl and a dragon." })),
    )
    .await;
    let id =
        serde_json::from_str::<Value>(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap()["proposal"].clone();
    let url = format!("/api/v1/agent/proposals/{id}/approve");
    let (a, b) = tokio::join!(
        send(app, Method::POST, &url, &admin, json!({})),
        send(app, Method::POST, &url, &admin, json!({}))
    );
    let mut codes = [a.0, b.0];
    codes.sort();
    assert_eq!(codes, [StatusCode::OK, StatusCode::CONFLICT]);
    // A page from another site may not drive it.
    let (s, _, _) = crate::api_tests::call(
        app,
        Method::POST,
        "/api/v1/mcp",
        Call {
            bearer: Some(&admin),
            body: Some(rpc(13, "tools/list", json!({}))),
            origin: Some("http://localhost.evil.example"),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    h.stop().await;
}

async fn call_anon(app: &Router, method: Method, uri: &str, body: Value) -> (StatusCode, Value, HeaderMap) {
    call(app, method, uri, Call { body: Some(body), ..Default::default() }).await
}
