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
    let identity = Identity::new(Db::open(&dir.join("corrent.db")).unwrap(), HashCost::FAST);
    let settings = Arc::new(ArcSwap::from_pointee(testkit::settings(&dir)));
    let st = build_state(ServerParts {
        engine: l.engine.clone(),
        settings,
        paths: l.paths.clone(),
        identity,
        cfg,
        hub: Default::default(),
        tls_fingerprint: None,
    })
    .await
    .unwrap();
    H { app: router(st.clone()), st, l, dir }
}

impl H {
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
