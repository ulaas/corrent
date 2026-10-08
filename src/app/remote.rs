//! Starts/stops the qBittorrent-compatible web API to match the settings.

use std::net::SocketAddr;
use std::sync::{Arc, mpsc};

use arc_swap::ArcSwap;

use super::CorrentApp;
use crate::config::Settings;
use crate::webapi::{WebApiServer, WebApiSettings};

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteStatus {
    Off,
    Starting,
    Running(SocketAddr),
    Failed(String),
}

type Started = (WebApiSettings, Result<WebApiServer, String>);

pub struct RemoteCtl {
    /// Latest saved settings, read by the API handlers.
    pub shared: Arc<ArcSwap<Settings>>,
    server: Option<WebApiServer>,
    pending: Option<WebApiSettings>,
    tx: mpsc::Sender<Started>,
    rx: mpsc::Receiver<Started>,
    pub status: RemoteStatus,
    synced: bool,
}

impl RemoteCtl {
    pub fn new(settings: &Settings) -> Self {
        let (tx, rx) = mpsc::channel();
        RemoteCtl {
            shared: Arc::new(ArcSwap::from_pointee(settings.clone())),
            server: None,
            pending: None,
            tx,
            rx,
            status: RemoteStatus::Off,
            synced: false,
        }
    }
}

impl CorrentApp {
    /// Bring the server in line with `settings.web_api` (start, restart or stop).
    pub fn sync_remote(&mut self) {
        let r = &mut self.remote;
        r.synced = true;
        r.shared.store(Arc::new(self.settings.clone()));
        let cfg = &self.settings.web_api;
        let wanted = (cfg.enabled && !cfg.password_hash.is_empty()).then(|| cfg.clone());
        let running = r.server.as_ref().map(|s| &s.cfg);
        if wanted.is_some() && (wanted == r.pending || (r.pending.is_none() && wanted.as_ref() == running)) {
            return;
        }
        r.server = None; // stops it
        r.pending = wanted.clone();
        let Some(cfg) = wanted else {
            r.status = if self.settings.web_api.enabled {
                RemoteStatus::Failed(t!("Set a password to turn it on.").into())
            } else {
                RemoteStatus::Off
            };
            return;
        };
        r.status = RemoteStatus::Starting;
        let (tx, engine, shared, ctx) = (r.tx.clone(), self.engine.clone(), r.shared.clone(), self.ctx.clone());
        self.rt.spawn(async move {
            let result = WebApiServer::start(cfg.clone(), engine, shared).await.map_err(|e| {
                if e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse) {
                    t!("port {port} is already in use by another program", port = cfg.port)
                } else {
                    format!("{e:#}")
                }
            });
            let _ = tx.send((cfg, result));
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_remote(&mut self) {
        if !self.remote.synced {
            self.sync_remote();
        }
        while let Ok((cfg, result)) = self.remote.rx.try_recv() {
            let r = &mut self.remote;
            if r.pending.as_ref() != Some(&cfg) {
                continue; // superseded; dropping a server stops it
            }
            r.pending = None;
            match result {
                Ok(server) => {
                    r.status = RemoteStatus::Running(server.addr);
                    r.server = Some(server);
                }
                Err(e) => {
                    tracing::warn!("web API: {e}");
                    r.status = RemoteStatus::Failed(e.clone());
                    self.toasts.push(crate::engine::ToastKind::Error, t!("Remote control couldn't start: {e}", e = e));
                }
            }
        }
    }
}
