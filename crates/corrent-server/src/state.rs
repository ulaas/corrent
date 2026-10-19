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
    /// Files whose trickplay is being made, and the one-at-a-time slot.
    pub trickplay_busy: Mutex<std::collections::HashSet<i64>>,
    pub trickplay_slot: tokio::sync::Mutex<()>,
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
    pub remote_status: ArcSwap<corrent_proto::server::RemoteStatus>,
    /// Set when a hosted server stops: its background tasks end (they hold the state).
    pub stopping: tokio::sync::watch::Sender<bool>,
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
        hostnames.push(format!("{host}.local"));
        hostnames.push(host);
        Nets {
            allowed: crate::net::parse_cidrs(&cfg.allowed_networks),
            proxies: crate::net::parse_cidrs(&cfg.trusted_proxies),
            hostnames,
        }
    }
}

/// Failed sign-ins per address (10 in 15 minutes → refused) and a global token bucket for the
/// unauthenticated endpoints (sign-in, pairing, setup), so a flood can't keep argon2 busy.
pub struct Limiter {
    fails: HashMap<IpAddr, (u32, Instant)>,
    /// Wrong pairing codes per address: five lock that address out of pairing for a while.
    pair_fails: HashMap<IpAddr, (u32, Instant)>,
    tokens: f64,
    last: Instant,
}

const IP_FAILS: u32 = 10;
const PAIR_FAILS: u32 = 5;
const IP_WINDOW: Duration = Duration::from_secs(15 * 60);
const BUCKET: f64 = 30.0;
const REFILL_PER_SEC: f64 = 1.0;

impl Default for Limiter {
    fn default() -> Self {
        Limiter { fails: HashMap::new(), pair_fails: HashMap::new(), tokens: BUCKET, last: Instant::now() }
    }
}

impl Limiter {
    /// May this address try to authenticate now? Takes a token from the global bucket.
    pub fn allow(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * REFILL_PER_SEC).min(BUCKET);
        self.last = now;
        self.fails.retain(|_, (_, t)| now.duration_since(*t) < IP_WINDOW);
        if self.fails.get(&ip).is_some_and(|(n, _)| *n >= IP_FAILS) || self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }

    pub fn failed(&mut self, ip: IpAddr) {
        let e = self.fails.entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
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

    pub fn succeeded(&mut self, ip: IpAddr) {
        self.fails.remove(&ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter() {
        let mut l = Limiter::default();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..10 {
            assert!(l.allow(a));
            l.failed(a);
        }
        assert!(!l.allow(a), "address blocked after 10 failures");
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        assert!(l.allow(b));
        l.succeeded(a);
        // The global bucket empties under a flood from many addresses.
        let mut refused = 0;
        for i in 0..100u8 {
            if !l.allow(IpAddr::from([10, 1, 0, i])) {
                refused += 1;
            }
        }
        assert!(refused > 50);
    }
}
