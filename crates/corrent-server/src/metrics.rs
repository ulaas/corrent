//! `GET /metrics`: Prometheus text format (opt-in with `metrics = true` in server.toml; the LAN
//! guard applies like everywhere else).

use std::fmt::Write;
use std::sync::Arc;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;

use crate::state::AppState;

fn metric(out: &mut String, name: &str, kind: &str, help: &str, rows: &[(String, f64)]) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
    for (labels, v) in rows {
        let _ = writeln!(out, "{name}{labels} {v}");
    }
}

fn one(v: impl Into<f64>) -> Vec<(String, f64)> {
    vec![(String::new(), v.into())]
}

fn label(k: &str, v: &str) -> String {
    format!("{{{k}=\"{}\"}}", v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " "))
}

pub async fn serve(State(st): State<Arc<AppState>>, req: axum::extract::Request) -> axum::response::Response {
    #[cfg(feature = "p2p")]
    if req.extensions().get::<crate::p2p::P2pPeer>().is_some() {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let token = st.cfg.load().metrics_token.clone();
    if !token.is_empty() {
        let given = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");
        let expected = format!("Bearer {token}");
        // Constant-time comparison (the token is a secret).
        let same = given.len() == expected.len()
            && given.bytes().zip(expected.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
        if !same {
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let snap = st.engine.snapshot();
    let s = &snap.session;
    let mut out = String::new();
    let mut by_state: std::collections::BTreeMap<String, f64> = Default::default();
    for t in &snap.torrents {
        *by_state.entry(format!("{:?}", t.state).to_lowercase()).or_default() += 1.0;
    }
    metric(
        &mut out,
        "corrent_torrents",
        "gauge",
        "Torrents by state.",
        &by_state.into_iter().map(|(k, v)| (label("state", &k), v)).collect::<Vec<_>>(),
    );
    metric(&mut out, "corrent_download_bytes_per_second", "gauge", "Download rate.", &one(s.down_bps as f64));
    metric(&mut out, "corrent_upload_bytes_per_second", "gauge", "Upload rate.", &one(s.up_bps as f64));
    metric(
        &mut out,
        "corrent_downloaded_bytes_total",
        "counter",
        "Downloaded since install.",
        &one(snap.all_time_downloaded as f64),
    );
    metric(
        &mut out,
        "corrent_uploaded_bytes_total",
        "counter",
        "Uploaded since install.",
        &one(snap.all_time_uploaded as f64),
    );
    metric(&mut out, "corrent_peers", "gauge", "Connected peers.", &one(s.peers_live));
    metric(&mut out, "corrent_dht_nodes", "gauge", "DHT routing table size.", &one(s.dht_nodes.unwrap_or(0) as f64));
    metric(&mut out, "corrent_uptime_seconds", "gauge", "Engine uptime.", &one(s.uptime_secs as f64));
    if let Some(f) = snap.free_space {
        metric(&mut out, "corrent_free_space_bytes", "gauge", "Free space for downloads.", &one(f as f64));
    }
    let libs = st.library.list().await.unwrap_or_default();
    metric(
        &mut out,
        "corrent_library_items",
        "gauge",
        "Items per library.",
        &libs.iter().map(|l| (label("library", &l.name), l.item_count as f64)).collect::<Vec<_>>(),
    );
    metric(
        &mut out,
        "corrent_transcode_sessions",
        "gauge",
        "Running HLS sessions.",
        &one(st.hls.sessions().len() as f64),
    );
    metric(&mut out, "corrent_realtime_clients", "gauge", "Open WebSocket connections.", &one(st.hub.clients() as f64));
    metric(&mut out, "corrent_users", "gauge", "Accounts.", &one(st.identity.user_count().await.unwrap_or(0) as f64));
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out).into_response()
}
