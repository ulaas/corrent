//! Casting from the native player: the server finds Chromecasts and plays the item on one;
//! this side lists them, starts the cast, follows its status and sends play/pause/stop.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_proto::library::{CastCommand, CastDevice, CastStatus};

use super::client::MediaClient;

#[derive(Default)]
struct Shared {
    devices: Option<Result<Vec<CastDevice>, String>>,
    status: Option<CastStatus>,
    polling: bool,
}

#[derive(Clone)]
pub struct CastCtl {
    shared: Arc<Mutex<Shared>>,
    client: MediaClient,
    rt: tokio::runtime::Handle,
}

/// A cast in progress.
pub struct Casting {
    pub device: CastDevice,
    last_poll: Instant,
}

impl CastCtl {
    pub fn new(client: MediaClient, rt: tokio::runtime::Handle) -> CastCtl {
        CastCtl { shared: Arc::default(), client, rt }
    }

    /// Looks for devices (the answer arrives in `devices`).
    pub fn find(&self, ctx: &egui::Context, refresh: bool) {
        let (c, s, ctx) = (self.client.clone(), self.shared.clone(), ctx.clone());
        if let Ok(mut g) = s.lock() {
            g.devices = None;
        }
        self.rt.spawn(async move {
            let path = if refresh { "/api/v1/cast/devices?refresh=true" } else { "/api/v1/cast/devices" };
            let r = c.get::<Vec<CastDevice>>(path).await;
            if let Ok(mut g) = s.lock() {
                g.devices = Some(r);
            }
            ctx.request_repaint();
        });
    }

    /// None while looking.
    pub fn devices(&self) -> Option<Result<Vec<CastDevice>, String>> {
        self.shared.lock().ok()?.devices.clone()
    }

    pub fn status(&self) -> Option<CastStatus> {
        self.shared.lock().ok()?.status.clone()
    }

    pub fn start(&self, device: CastDevice, item_id: i64, pos: f64) -> Casting {
        let (c, s) = (self.client.clone(), self.shared.clone());
        if let Ok(mut g) = s.lock() {
            g.status = None;
        }
        let id = device.id.clone();
        self.rt.spawn(async move {
            let body = serde_json::json!({ "item_id": item_id, "position_ms": (pos * 1000.0) as i64 });
            let path = format!("/api/v1/cast/{id}/play");
            let r = c.send::<CastStatus, _>(reqwest::Method::POST, &path, &body).await;
            if let Ok(mut g) = s.lock() {
                g.status = Some(r.unwrap_or_else(|e| CastStatus {
                    state: "error".into(),
                    error: Some(e),
                    ..Default::default()
                }));
            }
        });
        Casting { device, last_poll: Instant::now() }
    }

    pub fn poll(&self, casting: &mut Casting, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_secs(1));
        if casting.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        {
            let Ok(mut g) = self.shared.lock() else { return };
            if g.polling {
                return;
            }
            g.polling = true;
        }
        casting.last_poll = Instant::now();
        let (c, s, id) = (self.client.clone(), self.shared.clone(), casting.device.id.clone());
        self.rt.spawn(async move {
            let r = c.get::<CastStatus>(&format!("/api/v1/cast/{id}")).await;
            if let Ok(mut g) = s.lock() {
                g.polling = false;
                if let Ok(st) = r {
                    g.status = Some(st);
                }
            }
        });
    }

    pub fn command(&self, casting: &Casting, cmd: CastCommand) {
        let (c, id) = (self.client.clone(), casting.device.id.clone());
        self.rt.spawn(async move {
            let path = format!("/api/v1/cast/{id}/command");
            if let Err(e) = c.send::<serde_json::Value, _>(reqwest::Method::POST, &path, &cmd).await {
                tracing::warn!("cast: {e}");
            }
        });
    }
}
