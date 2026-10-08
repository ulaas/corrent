//! Translate [`Settings`] into librqbit [`SessionOptions`] and create sessions.

use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use librqbit::dht::DhtPersistenceConfig;
use librqbit::limits::LimitsConfig;
use librqbit::{
    ConnectionOptions, DhtSessionConfig, ListenerMode, ListenerOptions, PeerConnectionOptions, Session, SessionOptions,
    SessionPersistenceConfig,
};

use crate::config::{ListenMode, Paths, Settings};

pub fn kib_limit(kib: u32) -> Option<NonZeroU32> {
    NonZeroU32::new(kib.saturating_mul(1024))
}

pub fn limits(down_kib: u32, up_kib: u32) -> LimitsConfig {
    LimitsConfig { download_bps: kib_limit(down_kib), upload_bps: kib_limit(up_kib) }
}

fn options(
    paths: &Paths,
    s: &Settings,
    port_override: Option<u16>,
    alt: bool,
    lists: &super::ipfilter::Prepared,
) -> SessionOptions {
    let c = &s.connection;
    let bt = &s.bittorrent;
    let port = port_override.unwrap_or(if c.random_port { 0 } else { c.listen_port });
    let listen_addr: SocketAddr = match c.listen_address.trim().parse::<std::net::IpAddr>() {
        Ok(ip) => (ip, port).into(),
        Err(_) if !c.listen_address.trim().is_empty() => {
            tracing::warn!("listen address {:?} is not an IP address; listening on all", c.listen_address);
            if c.ipv4_only { (Ipv4Addr::UNSPECIFIED, port).into() } else { (Ipv6Addr::UNSPECIFIED, port).into() }
        }
        Err(_) if c.ipv4_only => (Ipv4Addr::UNSPECIFIED, port).into(),
        Err(_) => (Ipv6Addr::UNSPECIFIED, port).into(),
    };
    let (down, up) = s.speed.effective(alt);

    let trackers: HashSet<url::Url> =
        bt.default_trackers.iter().filter_map(|t| url::Url::parse(t.trim()).ok()).collect();

    SessionOptions {
        dht: bt.dht.then(|| DhtSessionConfig {
            bootstrap_addrs: None,
            port: None,
            persistence: Some(DhtPersistenceConfig {
                dump_interval: Some(Duration::from_secs(60)),
                config_filename: Some(paths.dht_file()),
            }),
        }),
        bind_device_name: Some(c.bind_device.trim().to_string()).filter(|d| !d.is_empty()),
        disable_trackers: !bt.trackers_enabled,
        fastresume: bt.fastresume,
        persistence: Some(SessionPersistenceConfig::Json { folder: Some(paths.session_dir()) }),
        listen: Some(ListenerOptions {
            mode: match c.listen_mode {
                ListenMode::Tcp => ListenerMode::TcpOnly,
                ListenMode::Utp => ListenerMode::UtpOnly,
                ListenMode::TcpAndUtp => ListenerMode::TcpAndUtp,
            },
            listen_addr,
            enable_upnp_port_forwarding: c.upnp,
            ipv4_only: c.ipv4_only,
            ..Default::default()
        }),
        connect: Some(ConnectionOptions {
            proxy_url: c.proxy_url(),
            enable_tcp: true,
            peer_opts: Some(PeerConnectionOptions {
                connect_timeout: Some(Duration::from_secs(c.connect_timeout_secs.clamp(2, 120))),
                ..Default::default()
            }),
        }),
        concurrent_init_limit: Some(s.advanced.concurrent_init_limit.max(1)),
        ratelimits: limits(down, up),
        // Prepared local files (see ipfilter.rs); a URL here would make a dead server fatal.
        blocklist_url: lists.blocklist.as_deref().and_then(super::ipfilter::file_url),
        allowlist_url: lists.allowlist.as_deref().and_then(super::ipfilter::file_url),
        trackers,
        peer_limit: (c.max_peers_global > 0).then_some(c.max_peers_global),
        disable_local_service_discovery: !bt.lsd,
        ipv4_only: c.ipv4_only,
        client_name_and_version: Some(format!("Corrent {}", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
    }
}

/// Create a session. If the configured port is unavailable, fall back to a random port so the
/// client still starts (the status bar shows the real port).
pub async fn create(paths: &Paths, s: &Settings, alt: bool) -> anyhow::Result<(Arc<Session>, Option<String>)> {
    std::fs::create_dir_all(paths.session_dir())?;
    let out = s.downloads.save_path.clone();
    let _ = std::fs::create_dir_all(&out);
    let lists = super::ipfilter::prepare(paths, &s.connection).await;
    let list_warning = (!lists.warnings.is_empty()).then(|| lists.warnings.join(" "));
    match Session::new_with_opts(out.clone(), options(paths, s, None, alt, &lists)).await {
        Ok(sess) => Ok((sess, list_warning)),
        Err(e) if !s.connection.random_port => {
            tracing::warn!(
                "could not start session on port {}: {e:#}; retrying on a random port",
                s.connection.listen_port
            );
            let sess = Session::new_with_opts(out, options(paths, s, Some(0), alt, &lists)).await?;
            Ok((
                sess,
                Some(format!("Port {} is unavailable ({e}). Using a random port instead.", s.connection.listen_port)),
            ))
        }
        Err(e) => Err(e),
    }
}
