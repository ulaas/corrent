//! Handshake and the endpoints apps call before (and right after) signing in.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::dto::Ctx;
use super::{JfAuth, empty_result, fail, json};
use crate::state::AppState;
use crate::v1::{ClientIp, RequestHost};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/system/info/public", get(info_public))
        .route("/system/info", get(info))
        .route("/system/ping", get(ping).post(ping))
        .route("/system/endpoint", get(endpoint))
        .route("/system/configuration", get(|_a: JfAuth| async { json(json!({})) }))
        .route("/system/configuration/{key}", get(|_a: JfAuth| async { json(json!({})) }))
        .route(
            "/branding/configuration",
            get(|| async { json(json!({ "LoginDisclaimer": "", "CustomCss": "", "SplashscreenEnabled": false })) }),
        )
        .route("/branding/css", get(css))
        .route("/branding/css.css", get(css))
        .route("/quickconnect/enabled", get(|| async { json(json!(false)) }))
        .route("/quickconnect/initiate", post(quick_off).get(quick_off))
        .route("/quickconnect/connect", get(quick_off))
        .route("/quickconnect/authorize", post(quick_off))
        .route(
            "/localization/options",
            get(|_a: JfAuth| async {
                json(json!([{ "Name": "English", "Value": "en-US" }, { "Name": "Türkçe", "Value": "tr" }]))
            }),
        )
        .route("/localization/cultures", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/localization/countries", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/localization/parentalratings", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/plugins", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/packages", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/scheduledtasks", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/library/virtualfolders", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/library/mediafolders", get(|_a: JfAuth| async { empty_result() }))
}

pub(crate) fn ctx(st: &AppState) -> Ctx {
    Ctx {
        server_id: super::ids::server(&st.server_id),
        server_name: format!("Corrent ({})", st.cfg.load().display_name()),
    }
}

/// `http://<host>/…` as the client addressed us (the listener it came through).
fn local_address(st: &AppState, headers: &HeaderMap, host: Option<&RequestHost>) -> String {
    let host = host
        .map(|h| h.0.clone())
        .or_else(|| headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(str::to_string));
    let Some(h) = host else { return String::new() };
    // Through the main listener (HTTPS, under /jellyfin) or the plain one.
    let cfg = st.cfg.load();
    let port: Option<u16> = h.rsplit_once(':').and_then(|(_, p)| p.parse().ok());
    if cfg.tls && port == Some(cfg.https_port) { format!("https://{h}/jellyfin") } else { format!("http://{h}") }
}

pub(crate) fn public_info(st: &AppState, headers: &HeaderMap, host: Option<&RequestHost>) -> Value {
    let c = ctx(st);
    json!({
        "LocalAddress": local_address(st, headers, host),
        "ServerName": c.server_name,
        "Version": st.cfg.load().jellyfin_version,
        "ProductName": "Jellyfin Server",
        "Id": c.server_id,
        "StartupWizardCompleted": true,
    })
}

async fn info_public(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    host: Option<axum::Extension<RequestHost>>,
) -> Response {
    json(public_info(&st, &headers, host.as_deref()))
}

async fn info(
    State(st): State<Arc<AppState>>,
    _a: JfAuth,
    headers: HeaderMap,
    host: Option<axum::Extension<RequestHost>>,
) -> Response {
    let mut v = public_info(&st, &headers, host.as_deref());
    // Never real paths.
    for (k, val) in [
        ("OperatingSystemDisplayName", json!("")),
        ("PackageName", json!("")),
        ("HasPendingRestart", json!(false)),
        ("IsShuttingDown", json!(false)),
        ("SupportsLibraryMonitor", json!(true)),
        ("WebSocketPortNumber", json!(0)),
        ("CompletedInstallations", json!([])),
        ("CanSelfRestart", json!(false)),
        ("CanLaunchWebBrowser", json!(false)),
        ("ProgramDataPath", json!("")),
        ("WebPath", json!("")),
        ("ItemsByNamePath", json!("")),
        ("CachePath", json!("")),
        ("LogPath", json!("")),
        ("InternalMetadataPath", json!("")),
        ("TranscodingTempPath", json!("")),
        ("CastReceiverApplications", json!([])),
        ("HasUpdateAvailable", json!(false)),
        ("EncoderLocation", json!("NotFound")),
        ("SystemArchitecture", json!(std::env::consts::ARCH)),
    ] {
        v[k] = val;
    }
    json(v)
}

async fn ping(State(st): State<Arc<AppState>>) -> Response {
    json(json!(ctx(&st).server_name))
}

async fn endpoint(_a: JfAuth, ip: ClientIp, State(st): State<Arc<AppState>>) -> Response {
    json(json!({ "IsLocal": ip.0.is_loopback(), "IsInNetwork": crate::net::is_local(ip.0, &st.nets.load().allowed) }))
}

async fn css() -> Response {
    ([(header::CONTENT_TYPE, "text/css")], "").into_response()
}

async fn quick_off() -> Response {
    fail(StatusCode::UNAUTHORIZED, "Quick connect is disabled")
}
