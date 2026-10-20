//! Shared state of a running media server.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use corrent_core::config::{Paths, Settings};
use corrent_core::engine::EngineHandle;

use crate::config_file::ServerConfig;
use crate::events::Hub;
use crate::identity::Identity;
use crate::net::Cidr;

/// Where an API preview's answer goes.
pub type PreviewReply = tokio::sync::oneshot::Sender<Result<Box<corrent_core::engine::Preview>, String>>;

pub struct AppState {
    pub engine: EngineHandle,
    pub settings: Arc<ArcSwap<Settings>>,
    pub paths: Paths,
    pub identity: Identity,
    pub cfg: ArcSwap<ServerConfig>,
    /// Parsed `allowed_networks` and `trusted_proxies`, and the accepted host names.
    pub nets: ArcSwap<Nets>,
    pub server_id: String,
    pub tls_fingerprint: Option<String>,
    /// The server's own CA (PEM), served to clients that pin `tls_fingerprint`.
    pub ca_pem: Option<String>,
    /// Torrent previews asked for through the API, by engine request id.
    pub previews: Mutex<HashMap<u64, PreviewReply>>,
    /// Cookies get `Secure` when the browser talks HTTPS to us.
    pub tls: bool,
    pub http: reqwest::Client,
    pub limiter: Mutex<Limiter>,
    /// Serializes settings writes (read-modify-write from several clients).
    pub settings_lock: tokio::sync::Mutex<()>,
    pub hub: Arc<Hub>,
    /// Library folders (confined users may save and browse there).
    pub library_roots: ArcSwap<Vec<std::path::PathBuf>>,
    pub library: corrent_library::Library,
    /// Folder watchers of the libraries (replaced when libraries change).
    pub watching: tokio::sync::Mutex<Option<corrent_library::watch::Watching>>,
    pub hls: Arc<corrent_transcode::hls::HlsManager>,
    /// The working H.264 encoder (None until detected, or when there is none).
    pub hls_encoder: Mutex<Option<String>>,
    /// Matroska files indexed for direct stream (copying into HLS/fMP4).
    pub remux: corrent_transcode::remux::Cache,
    /// Files whose trickplay is being made, and the one-at-a-time slot.
    pub trickplay_busy: Mutex<std::collections::HashSet<i64>>,
    pub trickplay_slot: tokio::sync::Mutex<()>,
    /// Frame thumbnails are made one at a time.
    pub thumb_slot: tokio::sync::Mutex<()>,
    /// Plugin host (None when wasmtime couldn't start).
    pub plugins: Option<Arc<corrent_plugins::PluginHost>>,
    /// One metadata refresh at a time.
    pub metadata_slot: tokio::sync::Mutex<()>,
    /// LAN discovery, Corrent Connect and port mapping (see `remote.rs`).
    pub remote: tokio::sync::Mutex<crate::remote::Running>,
    /// Scheduled tasks running right now.
    pub tasks: crate::tasks::Running,
    /// Watch-together rooms.
    pub rooms: crate::v1::syncplay::Rooms,
    /// Chromecasts found and the casts running on them.
    pub casts: crate::cast::Casts,
    /// Podcast subscriptions (`podcasts.json`).
    pub podcasts: crate::podcasts::Podcasts,
    /// Internet radio stations (`radio.json`).
    pub radio: crate::radio::Stations,
    /// The Jellyfin-compatible API's play sessions and device profiles.
    pub jellyfin: crate::jellyfin::JfState,
    pub remote_status: ArcSwap<corrent_proto::server::RemoteStatus>,
    /// Set when a hosted server stops: its background tasks end (they hold the state).
    pub stopping: tokio::sync::watch::Sender<bool>,
    /// Bumped when an AI agent proposes something or a proposal is decided.
    pub agent_changed: tokio::sync::watch::Sender<u64>,
}

impl AppState {
    /// Ends the background tasks (scheduler, metadata) so the state can be dropped.
    pub fn stop(&self) {
        self.stopping.send_replace(true);
    }

    /// Resolves when [`Self::stop`] was called.
    pub fn stopped(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.stopping.subscribe();
        async move {
            let _ = rx.wait_for(|s| *s).await;
        }
    }
}

#[derive(Default)]
pub struct Nets {
    pub allowed: Vec<Cidr>,
    pub proxies: Vec<Cidr>,
    pub hostnames: Vec<String>,
}

impl Nets {
    pub fn from(cfg: &ServerConfig) -> Nets {
        let host = crate::config_file::hostname().to_ascii_lowercase();
        let mut hostnames: Vec<String> = cfg.hostnames.iter().map(|h| h.to_ascii_lowercase()).collect();
        hostnames.extend(crate::acme::domains(cfg));
        hostnames.push(format!("{host}.local"));
        hostnames.push(host);
        Nets {
            allowed: crate::net::parse_cidrs(&cfg.allowed_networks),
            proxies: crate::net::parse_cidrs(&cfg.trusted_proxies),
            hostnames,
        }
    }
}

/// A global token bucket for the unauthenticated endpoints (sign-in, pairing, setup), so a flood
/// can't keep argon2 busy. Wrong passwords block no address (the owner's choice).
pub struct Limiter {
    /// Wrong pairing codes per address: five lock that address out of pairing for a while.
    pair_fails: HashMap<IpAddr, (u32, Instant)>,
    tokens: f64,
    last: Instant,
}

const PAIR_FAILS: u32 = 5;
const IP_WINDOW: Duration = Duration::from_secs(15 * 60);
const BUCKET: f64 = 30.0;
const REFILL_PER_SEC: f64 = 1.0;

impl Default for Limiter {
    fn default() -> Self {
        Limiter { pair_fails: HashMap::new(), tokens: BUCKET, last: Instant::now() }
    }
}

impl Limiter {
    /// May anyone try to authenticate now? Takes a token from the global bucket.
    pub fn allow(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * REFILL_PER_SEC).min(BUCKET);
        self.last = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }

    pub fn may_pair(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        self.pair_fails.retain(|_, (_, t)| now.duration_since(*t) < IP_WINDOW);
        self.pair_fails.get(&ip).is_none_or(|(n, _)| *n < PAIR_FAILS)
    }

    pub fn pair_failed(&mut self, ip: IpAddr) {
        let e = self.pair_fails.entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter() {
        let mut l = Limiter::default();
        // The global bucket empties under a flood.
        let refused = (0..100).filter(|_| !l.allow()).count();
        assert!(refused > 50);
        // Wrong pairing codes do block an address (six digits are quick to guess otherwise).
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..PAIR_FAILS {
            assert!(l.may_pair(a));
            l.pair_failed(a);
        }
        assert!(!l.may_pair(a));
        assert!(l.may_pair("10.0.0.2".parse().unwrap()));
    }
}
