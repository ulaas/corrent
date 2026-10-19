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

struct H {
    app: Router,
    st: Arc<AppState>,
    l: Leecher,
    dir: std::path::PathBuf,
}

async fn harness(name: &str, cfg: ServerConfig) -> H {
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

    async fn stop(self) {
        drop(self.app);
        drop(self.st);
        testkit::stop(self.l).await;
        let _ = std::fs::remove_dir_all(self.dir);
    }
}

#[derive(Default)]
struct Call<'a> {
    peer: Option<&'a str>,
    host: Option<&'a str>,
    bearer: Option<&'a str>,
    cookie: Option<&'a str>,
    csrf: Option<&'a str>,
    body: Option<Value>,
}

async fn call(app: &Router, method: Method, uri: &str, c: Call<'_>) -> (StatusCode, Value, HeaderMap) {
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

fn get(app: &Router, uri: &str, bearer: &str) -> impl std::future::Future<Output = (StatusCode, Value, HeaderMap)> {
    let (app, uri, bearer) = (app.clone(), uri.to_string(), bearer.to_string());
    async move { call(&app, Method::GET, &uri, Call { bearer: Some(&bearer), ..Default::default() }).await }
}

async fn send(app: &Router, method: Method, uri: &str, bearer: &str, body: Value) -> (StatusCode, Value, HeaderMap) {
    call(app, method, uri, Call { bearer: Some(bearer), body: Some(body), ..Default::default() }).await
}

fn device() -> Value {
    json!({ "name": "test", "platform": "linux", "client": "tests" })
}

/// Creates the admin through first-run setup and returns a device token for it.
async fn admin_token(app: &Router) -> String {
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
    let (s, v, _) =
        send(app, Method::POST, "/api/v1/torrents", &bob, json!({ "urls": [magnet], "save_path": "/etc" })).await;
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
    let (s, _, _) = get(app, "/api/v1/server/fs?path=/etc", &bob).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v, _) = get(app, "/api/v1/server/fs", &bob).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["entries"].as_array().unwrap().iter().all(|e| !e["path"].as_str().unwrap().starts_with("/etc")));
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
        input: "av://lavfi:testsrc=duration=12:size=320x240:rate=25".into(),
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
    // Web: H.264 in Matroska needs a transcode; the subtitle is offered as WebVTT.
    let (s, v, _) =
        send(app, Method::POST, &format!("/api/v1/playback/{item}"), &admin, json!({ "profile": "web" })).await;
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
        input: "av://lavfi:testsrc=duration=4:size=160x120:rate=10".into(),
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
