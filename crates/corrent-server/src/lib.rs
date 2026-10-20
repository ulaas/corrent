#![recursion_limit = "256"]

//! Corrent's server: the HTTP APIs over the engine (and, as the media server grows, the library,
//! playback and remote access). Hosted in-process by the GUI or by the headless `correntd`.

#[macro_use]
extern crate corrent_i18n;

#[allow(unused_imports)]
use corrent_core::{config, engine, rss, store, util};

pub mod cast;
pub mod config_file;
pub mod discovery;
pub mod dlna;
pub mod events;
/// Wire event types, re-exported for hosts.
pub use corrent_proto::event as events_api;
pub mod acme;
pub mod agent;
pub mod backups;
pub mod identity;
pub mod integrate;
pub mod intros;
pub mod jellyfin;
pub mod local;
pub mod metadata;
pub mod metrics;
pub mod monitor;
pub mod net;
pub mod notify;
#[cfg(feature = "p2p")]
pub mod p2p;
pub mod paths;
pub mod plugins;
pub mod podcasts;
pub mod radio;
pub mod remote;
pub mod state;
pub mod tasks;
pub mod thumbs;
pub mod tls;
pub mod v1;
pub mod webapi;
pub mod webclient;

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
    pub ca_pem: Option<String>,
    pub library: corrent_library::Library,
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
    // Library changes and scan progress go to realtime clients.
    let (mut rx, hub) = (p.library.subscribe(), p.hub.clone());
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(corrent_library::LibraryEvent::Changed { library_id }) => {
                    hub.publish(corrent_proto::event::Event::LibraryChanged { library_id })
                }
                Ok(corrent_library::LibraryEvent::Scan(status)) => {
                    hub.publish(corrent_proto::event::Event::ScanStatus { status })
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
    let transcode_root = p.paths.data_dir.join("transcode");
    let plugin_host = plugins::start(p.identity.db(), &p.paths.data_dir).await;
    // Plugins with the `library` permission read the libraries (as an account that sees all).
    if let Some(host) = &plugin_host {
        host.set_library(Arc::new(plugins::LibraryView {
            lib: p.library.clone(),
            rt: tokio::runtime::Handle::current(),
        }));
    }
    // Name-parser plugins get a say when scans read file names.
    if let Some(host) = &plugin_host {
        let host = host.clone();
        p.library.set_name_hook(Some(Arc::new(move |name: String| {
            let host = host.clone();
            Box::pin(async move {
                for id in host.ready(corrent_plugins::Kind::Parser) {
                    if let Ok(Some(n)) = host.parse_name(&id, name.clone()).await {
                        return Some(corrent_library::NameHint {
                            title: n.title,
                            year: n.year.map(|y| y as i32),
                            season: n.season.map(i64::from),
                            episode: n.episode.map(i64::from),
                        });
                    }
                }
                None
            })
        })));
    }
    let state = Arc::new(AppState {
        nets: ArcSwap::from_pointee(Nets::from(&p.cfg)),
        tls: p.cfg.tls,
        cfg: ArcSwap::from_pointee(p.cfg),
        engine: p.engine,
        settings: p.settings,
        paths: p.paths,
        identity: p.identity,
        server_id,
        tls_fingerprint: p.tls_fingerprint,
        ca_pem: p.ca_pem,
        previews: Default::default(),
        http: corrent_core::rss::feed::client(proxy),
        limiter: Default::default(),
        settings_lock: Default::default(),
        hub: p.hub.clone(),
        library_roots: Default::default(),
        library: p.library.clone(),
        watching: Default::default(),
        hls: corrent_transcode::hls::HlsManager::new(corrent_transcode::hls::HlsConfig {
            root: transcode_root,
            ..Default::default()
        }),
        hls_encoder: Default::default(),
        remux: Default::default(),
        trickplay_busy: Default::default(),
        trickplay_slot: Default::default(),
        thumb_slot: Default::default(),
        plugins: plugin_host,
        metadata_slot: Default::default(),
        stopping: tokio::sync::watch::Sender::new(false),
        agent_changed: tokio::sync::watch::Sender::new(0),
        remote: Default::default(),
        tasks: Default::default(),
        rooms: Default::default(),
        casts: Default::default(),
        podcasts: Default::default(),
        radio: Default::default(),
        jellyfin: Default::default(),
        remote_status: Default::default(),
    });
    metadata::start(state.clone());
    // Find a working H.264 encoder in the background (hardware first).
    let st2 = state.clone();
    tokio::spawn(async move {
        let scratch = st2.paths.data_dir.join("transcode-probe");
        let found = corrent_transcode::hw::detect(None, corrent_mpv::worker_path(), &scratch).await;
        let _ = std::fs::remove_dir_all(&scratch);
        if let Some(e) = &found {
            st2.hls.set_encoder(e);
        }
        *st2.hls_encoder.lock().unwrap_or_else(|e| e.into_inner()) = found;
    });
    v1::library::refresh_roots(&state).await;
    integrate::ensure_category_folders(&state).await;
    Ok(state)
}

/// (Re)starts watching the library folders (after libraries change).
pub async fn restart_watching(st: &AppState) {
    let mut w = st.watching.lock().await;
    *w = None;
    match corrent_library::watch::start(&st.library, std::time::Duration::from_secs(3)).await {
        Ok(new) => *w = Some(new),
        Err(e) => tracing::warn!("library watching: {e}"),
    }
}

/// The whole HTTP surface: `/api/v1`, and (when enabled) the qBittorrent API and classic web UI.
pub fn router(st: Arc<AppState>) -> Router {
    let mut app = Router::new().nest("/api/v1", v1::router().with_state(st.clone()));
    if st.cfg.load().qbt_api {
        app =
            app.merge(webapi::media_server_router(st.engine.clone(), st.settings.clone(), st.identity.clone(), st.tls));
    }
    if st.cfg.load().metrics {
        app = app.route("/metrics", axum::routing::get(metrics::serve).with_state(st.clone()));
    }
    app = app.nest_service("/jellyfin", jellyfin::service(st.clone()));
    app.fallback(webclient::serve)
        .layer(axum::middleware::from_fn_with_state(st.clone(), v1::guard))
        // Small by default; the routes that take uploads raise it for themselves.
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

pub use integrate::forward_engine_events;
pub use tasks::start as start_tasks;

/// A running listener; stops when dropped.
pub struct Listener {
    pub addr: SocketAddr,
    stop: Option<Box<dyn FnOnce() + Send + Sync>>,
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
    serve_https_config(app, addr, config).await
}

/// How long a client may take to send its request headers (HTTPS).
pub(crate) const HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Serves `app` over HTTPS with ready TLS settings.
pub(crate) async fn serve_https_config(
    app: Router,
    addr: SocketAddr,
    config: axum_server::tls_rustls::RustlsConfig,
) -> anyhow::Result<Listener> {
    let std_listener = std::net::TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;
    let addr = std_listener.local_addr()?;
    let handle = axum_server::Handle::new();
    let mut server = axum_server::from_tcp_rustls(std_listener, config)?.handle(handle.clone());
    // A client that opens a connection and never finishes its request headers (slow loris) is
    // dropped instead of holding a connection forever; also ends idle keep-alive connections.
    // hyper needs a timer for that, and has none by default.
    server.http_builder().http1().timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT);
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

/// A media type written in the code (an unparsable one would be a bug: served as bytes).
pub(crate) fn static_mime(s: &'static str) -> mime_guess::mime::Mime {
    s.parse().unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM)
}
