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

/// Starts the in-process server on the app's engine and profile.
pub async fn start(
    engine: EngineHandle,
    settings: Arc<ArcSwap<Settings>>,
    paths: Paths,
) -> anyhow::Result<LocalServer> {
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
    crate::start_wanted_scheduler(state.clone());
    Ok(LocalServer {
        base_url: format!("http://127.0.0.1:{}", listener.addr.port()),
        token: issued.token,
        user: user.name,
        state,
        _listener: listener,
    })
}
