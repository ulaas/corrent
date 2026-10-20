//! Translate [`Settings`] into librqbit [`SessionOptions`] and create sessions.

use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

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
    cancel: &CancellationToken,
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
        // Torrent URLs and HTTP trackers: a server that trickles bytes can't hold an add forever.
        http_timeout: Some(crate::faults::http_timeout(&paths.data_dir).unwrap_or(Duration::from_secs(60))),
        // A child per attempt: a session that fails half-way cancels its own token when dropped.
        cancellation_token: Some(cancel.child_token()),
        ..Default::default()
    }
}

/// Create a session. If the configured port is taken (or not allowed), fall back to a random
/// port so the client still starts (the status bar shows the real port). Other failures, such
/// as a damaged session database, are reported as themselves.
/// Cancelling `cancel` stops every task of the session, also when no one calls `stop()` (an
/// engine task that panicked).
pub async fn create(
    paths: &Paths,
    s: &Settings,
    alt: bool,
    cancel: &CancellationToken,
    on_phase: &(dyn Fn(super::types::StartPhase) + Send + Sync),
    deadline: Duration,
) -> anyhow::Result<(Arc<Session>, Option<String>)> {
    std::fs::create_dir_all(paths.session_dir())?;
    let out = s.downloads.save_path.clone();
    let _ = std::fs::create_dir_all(&out);
    on_phase(super::types::StartPhase::IpFilter);
    // Outside the deadline: each list download has its own timeouts, and a first start with a
    // dead list URL (nothing cached yet) must still start once they have given up.
    let lists = super::ipfilter::prepare(paths, &s.connection).await;
    on_phase(super::types::StartPhase::Torrents);
    let list_warning = (!lists.warnings.is_empty()).then(|| lists.warnings.join(" "));
    let delay = crate::faults::start_delay(&paths.data_dir);
    let start = |opts| new_session(out.clone(), opts, deadline, cancel, delay);
    match start(options(paths, s, None, alt, &lists, cancel)).await {
        Ok(sess) => Ok((sess, list_warning)),
        Err(e) if !s.connection.random_port && is_port_error(&e) => {
            tracing::warn!(
                "could not start session on port {}: {e:#}; retrying on a random port",
                s.connection.listen_port
            );
            let sess = start(options(paths, s, Some(0), alt, &lists, cancel)).await?;
            Ok((
                sess,
                Some(t!(
                    "Port {port} is unavailable ({e}). Using a random port instead.",
                    port = s.connection.listen_port,
                    e = e
                )),
            ))
        }
        Err(e) => Err(e),
    }
}

/// librqbit's own start (listeners, persistence, every torrent), at most `deadline`. On timeout
/// the half-built session's tasks are cancelled, so a retry finds its ports and files free.
async fn new_session(
    out: std::path::PathBuf,
    opts: SessionOptions,
    deadline: Duration,
    cancel: &CancellationToken,
    test_delay: Option<Duration>,
) -> anyhow::Result<Arc<Session>> {
    let start = async {
        if let Some(d) = test_delay {
            tokio::time::sleep(d).await;
        }
        Session::new_with_opts(out, opts).await
    };
    match tokio::time::timeout(deadline, start).await {
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err(anyhow::anyhow!(t!("the engine didn't start within {secs} seconds", secs = deadline.as_secs())))
        }
    }
}

/// True when starting the peer listeners failed because the port is in use or not allowed.
///
/// The OS error is only in the message: librqbit's bind error doesn't expose it as a source.
/// A DHT port clash isn't a listener error (retrying the peer port wouldn't help), and an
/// address that doesn't exist or isn't supported (a vanished adapter, IPv6 turned off) is
/// reported as itself.
fn is_port_error(e: &anyhow::Error) -> bool {
    let frames: Vec<String> = e.chain().map(|c| c.to_string().to_ascii_lowercase()).collect();
    let listeners = frames.iter().any(|f| f.contains("error starting listeners"));
    const TAKEN: [&str; 8] = [
        "os error 10048", // WSAEADDRINUSE
        "os error 10013", // WSAEACCES (a port reserved by Hyper-V/WinNAT, or excluded)
        "os error 98)",   // Linux EADDRINUSE
        "os error 48)",   // macOS EADDRINUSE
        "os error 13)",   // EACCES (a privileged port)
        "already in use",
        "access is denied",
        "permission denied",
    ];
    let taken = frames.iter().any(|f| f.contains("error binding") && TAKEN.iter().any(|m| f.contains(m)));
    listeners && taken
}

#[cfg(test)]
mod port_tests {
    use super::is_port_error;

    fn listener_error(os: &str) -> anyhow::Error {
        anyhow::anyhow!("error binding: {os}")
            .context("error starting TCP listener")
            .context("error starting listeners")
    }

    #[test]
    fn only_a_taken_listener_port_falls_back() {
        for os in [
            "Only one usage of each socket address is normally permitted. (os error 10048)",
            "An attempt was made to access a socket in a way forbidden by its access permissions. (os error 10013)",
            "Address already in use (os error 98)",
            "Permission denied (os error 13)",
        ] {
            assert!(is_port_error(&listener_error(os)), "{os}");
        }
        for os in [
            "The requested address is not valid in its context. (os error 10049)",
            "Cannot assign requested address (os error 99)",
            "An address incompatible with the requested protocol was used. (os error 10047)",
        ] {
            assert!(!is_port_error(&listener_error(os)), "{os}");
        }
        let db = anyhow::anyhow!("expected value at line 1 column 1")
            .context("error deserializing session database")
            .context("error initializing JsonSessionPersistenceStore");
        assert!(!is_port_error(&db));
        // The DHT's UDP port is not the peer port.
        let dht = anyhow::anyhow!("Address already in use (os error 98)")
            .context("error binding")
            .context("error starting DHT");
        assert!(!is_port_error(&dht));
    }
}

#[cfg(test)]
mod limit_tests {
    /// Limits below one 16 KiB chunk used to make every transfer fail (governor rejects requests
    /// larger than the burst); the vendored limiter allows a one-chunk burst.
    #[tokio::test]
    async fn tiny_limits_still_transfer() {
        let l = librqbit::limits::Limits::new(super::limits(1, 1));
        let chunk = std::num::NonZeroU32::new(16 * 1024).unwrap();
        let r = tokio::time::timeout(std::time::Duration::from_secs(2), l.prepare_for_download(chunk)).await;
        assert!(matches!(r, Ok(Ok(()))), "a full chunk passes a 1 KiB/s limit");
        assert!(l.prepare_for_upload(chunk).await.is_ok());
    }
}
