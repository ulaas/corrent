#![recursion_limit = "256"]

//! Corrent's server: the HTTP APIs over the engine (and, as the media server grows, the library,
//! playback and remote access). Hosted in-process by the GUI or by the headless `correntd`.

#[allow(unused_imports)]
use corrent_core::{config, engine, rss, store, util};

pub mod config_file;
pub mod events;
/// Wire event types, re-exported for hosts.
pub use corrent_proto::event as events_api;
pub mod identity;
pub mod net;
pub mod state;
pub mod tls;
pub mod v1;
pub mod webapi;

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::Router;
use corrent_core::config::{Paths, Settings};
use corrent_core::engine::EngineHandle;

pub use config_file::ServerConfig;
use events::Hub;
use identity::Identity;
use state::{AppState, Nets};

/// Everything a media server needs besides its listeners.
pub struct ServerParts {
    pub engine: EngineHandle,
    pub settings: Arc<ArcSwap<Settings>>,
    pub paths: Paths,
    pub identity: Identity,
    pub cfg: ServerConfig,
    pub hub: Arc<Hub>,
    pub tls_fingerprint: Option<String>,
}

/// Builds the shared state (server id, adopted qBittorrent-API admin).
pub async fn build_state(p: ServerParts) -> anyhow::Result<Arc<AppState>> {
    let server_id = p.identity.server_id().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let legacy = p.settings.load().web_api.clone();
    if let Some(u) = p
        .identity
        .adopt_legacy_admin(&legacy.username, &legacy.password_hash)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
    {
        tracing::info!("the Web API account {:?} is now the server administrator", u.name);
    }
    let proxy = p.settings.load().connection.proxy_url();
    Ok(Arc::new(AppState {
        nets: ArcSwap::from_pointee(Nets::from(&p.cfg)),
        tls: p.cfg.tls,
        cfg: ArcSwap::from_pointee(p.cfg),
        engine: p.engine,
        settings: p.settings,
        paths: p.paths,
        identity: p.identity,
        server_id,
        tls_fingerprint: p.tls_fingerprint,
        http: corrent_core::rss::feed::client(proxy),
        limiter: Default::default(),
        settings_lock: Default::default(),
        hub: p.hub,
    }))
}

/// The whole HTTP surface: `/api/v1`, and (when enabled) the qBittorrent API and classic web UI.
pub fn router(st: Arc<AppState>) -> Router {
    let mut app = Router::new().nest("/api/v1", v1::router().with_state(st.clone()));
    if st.cfg.load().qbt_api {
        app = app.merge(webapi::media_server_router(st.engine.clone(), st.settings.clone(), st.identity.clone()));
    }
    app.layer(axum::middleware::from_fn_with_state(st.clone(), v1::guard))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024))
}

/// Forwards the engine's events to connected clients (for hosts that own the engine's event
/// channel, like `correntd`). Runs until the channel closes.
pub fn forward_engine_events(rx: std::sync::mpsc::Receiver<corrent_core::engine::EngineEvent>, hub: Arc<Hub>) {
    let _ = std::thread::Builder::new().name("corrent-events".into()).spawn(move || {
        while let Ok(ev) = rx.recv() {
            hub.engine_event(ev);
        }
    });
}

/// A running listener; stops when dropped.
pub struct Listener {
    pub addr: SocketAddr,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            s();
        }
    }
}

/// Serves `app` over plain HTTP on `addr`.
pub async fn serve_http(app: Router, addr: SocketAddr) -> anyhow::Result<Listener> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let serve = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async {
                let _ = rx.await;
            });
        if let Err(e) = serve.await {
            tracing::warn!("HTTP listener stopped: {e}");
        }
    });
    tracing::info!("listening on http://{addr}");
    Ok(Listener {
        addr,
        stop: Some(Box::new(move || {
            let _ = tx.send(());
        })),
    })
}

/// Serves `app` over HTTPS on `addr` with the given certificate chain and key.
pub async fn serve_https(app: Router, addr: SocketAddr, tls: &tls::TlsMaterial) -> anyhow::Result<Listener> {
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        tls.cert_chain_pem.as_bytes().to_vec(),
        tls.key_pem.as_bytes().to_vec(),
    )
    .await?;
    let std_listener = std::net::TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;
    let addr = std_listener.local_addr()?;
    let handle = axum_server::Handle::new();
    let server = axum_server::from_tcp_rustls(std_listener, config)?.handle(handle.clone());
    tokio::spawn(async move {
        if let Err(e) = server.serve(app.into_make_service_with_connect_info::<SocketAddr>()).await {
            tracing::warn!("HTTPS listener stopped: {e}");
        }
    });
    tracing::info!("listening on https://{addr}");
    Ok(Listener {
        addr,
        stop: Some(Box::new(move || handle.graceful_shutdown(Some(std::time::Duration::from_secs(5))))),
    })
}

#[cfg(test)]
mod api_tests;
