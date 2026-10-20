//! The server hosted inside the desktop app ("local mode"): same engine, same profile,
//! listening on loopback only, with a device token for the app itself.

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use corrent_core::config::{Paths, Settings};
use corrent_core::engine::EngineHandle;

use crate::identity::{HashCost, Identity};
use crate::state::AppState;
use crate::{Listener, ServerConfig, ServerParts};

pub struct LocalServer {
    pub state: Arc<AppState>,
    /// `http://127.0.0.1:<port>`
    pub base_url: String,
    pub token: String,
    pub user: String,
    _listener: Listener,
}

impl LocalServer {
    pub fn addr(&self) -> &str {
        &self.base_url
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.state.stop();
    }
}

/// The file `corrent --mcp` (the stdio bridge for AI agents) reads: where this server listens
/// and an agent token of its own (revocable under Devices; kept while it stays valid).
pub fn agent_link_file(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("mcp.json")
}

async fn agent_link(state: &Arc<AppState>, user: i64, url: &str) {
    let file = agent_link_file(&state.paths.data_dir);
    let old = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from));
    let token = match old {
        Some(t) if state.identity.verify_token(&t, "127.0.0.1").await.ok().flatten().is_some() => t,
        _ => {
            let device = corrent_proto::auth::DeviceInfo {
                name: "AI agents (MCP)".into(),
                platform: "mcp".into(),
                client: "corrent --mcp".into(),
            };
            match state.identity.issue(user, Some(device), "127.0.0.1".into(), "corrent --mcp".into()).await {
                Ok(i) => i.token,
                Err(e) => {
                    tracing::warn!("AI agent token: {e}");
                    return;
                }
            }
        }
    };
    let body = serde_json::json!({ "url": url, "token": token }).to_string();
    if let Err(e) = corrent_core::util::write_replace(&file, body.as_bytes()) {
        tracing::warn!("{}: {e:#}", file.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
}

/// Starts the in-process server on the app's engine and profile.
pub async fn start(
    engine: EngineHandle,
    settings: Arc<ArcSwap<Settings>>,
    paths: Paths,
) -> anyhow::Result<LocalServer> {
    // A staged restore was applied by the app at startup, before its settings were read.
    let db = corrent_db::Db::open(&paths.data_dir.join("corrent.db"))?;
    let identity = Identity::new(db.clone(), HashCost::STANDARD);
    let library = corrent_library::Library::new(
        db,
        &paths.data_dir,
        corrent_library::probe::Prober::new(corrent_library::probe::Prober::find_worker(), 2),
    );
    // Loopback only, no TLS, no qBittorrent API (the app has its own setting for that).
    let cfg = ServerConfig {
        bind: "127.0.0.1".into(),
        tls: false,
        qbt_api: false,
        ..ServerConfig::load(&paths.data_dir).unwrap_or_default()
    };
    let state = crate::build_state(ServerParts {
        engine,
        settings,
        paths,
        identity,
        cfg,
        hub: Default::default(),
        tls_fingerprint: None,
        ca_pem: None,
        library,
    })
    .await?;
    let (user, issued) = state.identity.local_device_token().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let listener = crate::serve_http(crate::router(state.clone()), SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    crate::restart_watching(&state).await;
    crate::start_tasks(state.clone());
    let base_url = format!("http://127.0.0.1:{}", listener.addr.port());
    agent_link(&state, user.id, &base_url).await;
    Ok(LocalServer { base_url, token: issued.token, user: user.name, state, _listener: listener })
}
