//! Casting from the native player: the server finds Chromecasts and plays the item on one;
//! this side lists them, starts the cast, follows its status and drives the device.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_proto::library::{CastCommand, CastDevice, CastStatus};

use super::client::MediaClient;

/// Volume changes sent while dragging, at most this often.
const VOLUME_EVERY: Duration = Duration::from_millis(150);
/// How long a volume set here is shown before the device's own report takes over.
const VOLUME_SHOWN: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct Shared {
    devices: Option<Result<Vec<CastDevice>, String>>,
    status: Option<CastStatus>,
    polling: bool,
    /// A play request on its way: no status means anything until it is answered.
    starting: bool,
    /// The play request itself failed: the device's status is about something older.
    refused: bool,
    /// Bumped by every play request; answers to polls from before it are dropped.
    generation: u64,
    /// The casts running when last asked, until one is attached to.
    active: Option<Vec<CastStatus>>,
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
    volume_sent: Option<Instant>,
    /// The volume last set here, until the device has caught up.
    volume_set: Option<(f64, Instant)>,
}

impl Casting {
    fn new(device: CastDevice) -> Casting {
        Casting { device, last_poll: Instant::now(), volume_sent: None, volume_set: None }
    }
}

/// A reason from the server, for people: translated when it's one we know.
pub fn reason_text(reason: &str) -> String {
    match reason {
        "casting is turned off in the settings" => t!("Casting is turned off in the settings.").to_string(),
        "cant_play" => t!("The device couldn’t play this.").to_string(),
        "cant_reach" => {
            t!("The device couldn’t reach this computer. Allow Corrent through the firewall for private networks, then try again.")
                .to_string()
        }
        other => other.to_string(),
    }
}

/// What went wrong, for people.
pub fn error_text(s: &CastStatus) -> String {
    reason_text(s.error.as_deref().unwrap_or("cant_play"))
}

impl CastCtl {
    pub fn new(client: MediaClient, rt: tokio::runtime::Handle) -> CastCtl {
        CastCtl { shared: Arc::default(), client, rt }
    }

    /// Looks for devices (the answer arrives in `devices`).
    pub fn find(&self, ctx: &egui::Context, refresh: bool) {
        let (c, s, ctx) = (self.client.clone(), self.shared.clone(), ctx.clone());
        {
            let mut g = crate::util::lock(&s);
            g.devices = None;
        }
        self.rt.spawn(async move {
            let path = if refresh { "/api/v1/cast/devices?refresh=true" } else { "/api/v1/cast/devices" };
            let r = c.get_reason::<Vec<CastDevice>>(path).await.map_err(|e| reason_text(&e));
            {
                let mut g = crate::util::lock(&s);
                g.devices = Some(r);
            }
            ctx.request_repaint();
        });
    }

    /// None while looking.
    pub fn devices(&self) -> Option<Result<Vec<CastDevice>, String>> {
        crate::util::lock(&self.shared).devices.clone()
    }

    pub fn status(&self) -> Option<CastStatus> {
        crate::util::lock(&self.shared).status.clone()
    }

    /// Waiting for the answer to a play request.
    pub fn starting(&self) -> bool {
        crate::util::lock(&self.shared).starting
    }

    /// Asks what is casting, to attach to it (the answer arrives in `take_active`).
    pub fn find_active(&self, ctx: &egui::Context) {
        let (c, s, ctx) = (self.client.clone(), self.shared.clone(), ctx.clone());
        crate::util::lock(&s).active = None;
        self.rt.spawn(async move {
            if let Ok(list) = c.get::<Vec<CastStatus>>("/api/v1/cast/active").await {
                crate::util::lock(&s).active = Some(list);
                ctx.request_repaint();
            }
        });
    }

    /// A cast of `item` found by `find_active`: it is followed from now on.
    pub fn attach(&self, item: i64) -> Option<Casting> {
        let mut g = crate::util::lock(&self.shared);
        let found = g.active.as_ref()?.iter().find(|s| s.item_id == Some(item)).cloned();
        g.active = None;
        let status = found?;
        let device = status.device.clone();
        g.status = Some(status);
        g.refused = false;
        Some(Casting::new(device))
    }

    /// Plays `item` on `device` (again, with other tracks, when it already does). The status stays
    /// as it was until the answer, unless it is another device's.
    pub fn start(
        &self,
        device: CastDevice,
        item_id: i64,
        pos: f64,
        audio_track: Option<i64>,
        subtitle: Option<i64>,
    ) -> Casting {
        let (c, s) = (self.client.clone(), self.shared.clone());
        {
            let mut g = crate::util::lock(&s);
            g.starting = true;
            g.refused = false;
            g.generation += 1;
            if g.status.as_ref().is_none_or(|st| st.device.id != device.id) {
                g.status = None;
            }
        }
        let id = device.id.clone();
        let for_error = device.clone();
        self.rt.spawn(async move {
            let body = serde_json::json!({
                "item_id": item_id, "position_ms": (pos * 1000.0) as i64, "audio_track": audio_track, "subtitle": subtitle,
            });
            let path = format!("/api/v1/cast/{id}/play");
            let r = c.send_reason::<CastStatus, _>(reqwest::Method::POST, &path, &body).await;
            let mut g = crate::util::lock(&s);
            g.starting = false;
            g.refused = r.is_err();
            g.status = Some(r.unwrap_or_else(|e| CastStatus {
                device: for_error,
                state: "error".into(),
                error: Some(e),
                ..Default::default()
            }));
        });
        Casting::new(device)
    }

    pub fn poll(&self, casting: &mut Casting, ctx: &egui::Context) {
        ctx.request_repaint_after(Duration::from_secs(1));
        if casting.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        let generation = {
            let mut g = crate::util::lock(&self.shared);
            if g.polling || g.starting || g.refused {
                return;
            }
            g.polling = true;
            g.generation
        };
        casting.last_poll = Instant::now();
        let (c, s, id) = (self.client.clone(), self.shared.clone(), casting.device.id.clone());
        self.rt.spawn(async move {
            let r = c.get::<CastStatus>(&format!("/api/v1/cast/{id}")).await;
            let mut g = crate::util::lock(&s);
            g.polling = false;
            if let Ok(st) = r
                && g.generation == generation
            {
                g.status = Some(st);
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

    /// The device's volume (0 to 1) and mute: what was just set here, else what it reports.
    pub fn volume(&self, casting: &Casting) -> (Option<f64>, bool) {
        let s = self.status();
        let reported = s.as_ref().and_then(|s| s.volume);
        let muted = s.as_ref().and_then(|s| s.muted).unwrap_or(false);
        let set = casting.volume_set.filter(|(_, at)| at.elapsed() < VOLUME_SHOWN).map(|(v, _)| v);
        (set.or(reported), muted)
    }

    /// Sets the device's volume. `done`: the last of a drag, which always goes out.
    pub fn set_volume(&self, casting: &mut Casting, level: f64, done: bool) {
        // Its level isn't ours to change: the slider would only jump back.
        if self.status().is_some_and(|s| s.volume_fixed) {
            return;
        }
        let level = level.clamp(0.0, 1.0);
        casting.volume_set = Some((level, Instant::now()));
        if done || casting.volume_sent.is_none_or(|t| t.elapsed() >= VOLUME_EVERY) {
            casting.volume_sent = Some(Instant::now());
            self.command(casting, CastCommand::Volume { level });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_sentences() {
        let e = |code: &str| error_text(&CastStatus { error: Some(code.into()), ..Default::default() });
        assert!(e("cant_play").contains("couldn’t play"));
        assert!(e("cant_reach").contains("firewall"));
        assert_eq!(e("the device stopped answering"), "the device stopped answering");
        assert!(error_text(&CastStatus::default()).contains("couldn’t play"));
        assert!(reason_text("casting is turned off in the settings").starts_with("Casting is turned off"));
    }
}
