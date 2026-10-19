//! Corrent Connect: the server reachable from anywhere through iroh (QUIC with hole punching,
//! relayed when that fails; end-to-end encrypted and authenticated by the server's key).
//!
//! The server accepts bidirectional streams and serves **the same router** on each one as an
//! HTTP/1.1 connection (WebSocket upgrades included), so every API works unchanged. Clients
//! (the desktop app; mpv) can't dial iroh, so the app runs a loopback **bridge**: every TCP
//! connection to `127.0.0.1:<port>` becomes one stream to the server.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::ConnectInfo;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey, TransportAddr};

pub use iroh::EndpointId;

pub const ALPN: &[u8] = b"corrent/1";
/// Concurrent streams (HTTP connections) per Connect client.
const MAX_STREAMS: usize = 64;

/// Marks requests that came through Corrent Connect (with the client's key).
#[derive(Clone, Copy, Debug)]
pub struct P2pPeer(pub EndpointId);

/// The server's Connect key (`connect.key`, created owner-only on first use). Its public half is
/// the address clients dial.
pub fn load_or_create_key(dir: &Path) -> anyhow::Result<SecretKey> {
    let path = dir.join("connect.key");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let bytes: Vec<u8> = (0..text.trim().len() / 2)
            .filter_map(|i| u8::from_str_radix(text.trim().get(i * 2..i * 2 + 2)?, 16).ok())
            .collect();
        let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow::anyhow!("{} is damaged", path.display()))?;
        return Ok(SecretKey::from_bytes(&arr));
    }
    let key = SecretKey::generate();
    let hex: String = key.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    crate::tls::write_private(&path, &hex)?;
    Ok(key)
}

/// A stable per-client address in a private range, so per-address limits and logs tell Connect
/// clients apart (and the LAN guard admits them: Connect is remote access the owner turned on).
pub fn peer_ip(id: &EndpointId) -> IpAddr {
    let b = id.as_bytes();
    let mut a = [0u8; 16];
    a[0] = 0xfd;
    a[1] = 0x63; // "c"
    a[2] = 0x6f; // "o"
    a[3] = 0x6e; // "n"
    a[4..].copy_from_slice(&b[..12]);
    IpAddr::V6(Ipv6Addr::from(a))
}

fn builder(relay: bool) -> iroh::endpoint::Builder {
    if relay {
        Endpoint::builder(presets::N0)
    } else {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
    }
}

/// The listening side; stops when dropped.
pub struct Listener {
    pub endpoint: Endpoint,
    task: tokio::task::JoinHandle<()>,
}

impl Listener {
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// The endpoint's current addresses (direct ones and its home relay).
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
        let ep = self.endpoint.clone();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { ep.close().await });
        }
    }
}

/// Serves `router` over Connect. `relay`: use n0's relays and address lookup (needed to be
/// reachable from outside the LAN); off: direct addresses only (tests, LAN).
pub async fn serve(router: Router, key: SecretKey, relay: bool, bind: Option<SocketAddr>) -> anyhow::Result<Listener> {
    let mut b = builder(relay).secret_key(key).alpns(vec![ALPN.to_vec()]);
    if let Some(addr) = bind {
        b = b.clear_ip_transports().bind_addr(addr)?;
    }
    let endpoint = b.bind().await?;
    let ep = endpoint.clone();
    let task = tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let router = router.clone();
            tokio::spawn(async move {
                let conn = match incoming.await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("connect: handshake failed: {e}");
                        return;
                    }
                };
                let peer = conn.remote_id();
                tracing::debug!("connect: {} connected", peer.fmt_short());
                // A bounded number of streams per connection at once (each is an HTTP connection).
                let slots = Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS));
                while let Ok((send, recv)) = conn.accept_bi().await {
                    let Ok(slot) = slots.clone().acquire_owned().await else { break };
                    let router = router.clone();
                    tokio::spawn(async move {
                        serve_stream(send, recv, router, peer).await;
                        drop(slot);
                    });
                }
            });
        }
    });
    Ok(Listener { endpoint, task })
}

async fn serve_stream(
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    router: Router,
    peer: EndpointId,
) {
    let io = hyper_util::rt::TokioIo::new(tokio::io::join(recv, send));
    let addr = SocketAddr::new(peer_ip(&peer), 0);
    let svc = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
        req.extensions_mut().insert(ConnectInfo(addr));
        req.extensions_mut().insert(P2pPeer(peer));
        tower::ServiceExt::oneshot(router.clone(), req.map(axum::body::Body::new))
    });
    if let Err(e) = hyper::server::conn::http1::Builder::new().serve_connection(io, svc).with_upgrades().await {
        tracing::debug!("connect: stream ended: {e}");
    }
}

/// The client side: a loopback port whose connections are carried to the server.
pub struct Bridge {
    pub addr: SocketAddr,
    endpoint: Endpoint,
    task: tokio::task::JoinHandle<()>,
}

impl Bridge {
    /// `http://127.0.0.1:<port>`
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
        let ep = self.endpoint.clone();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { ep.close().await });
        }
    }
}

/// Opens the bridge to `server` (dialled by id through address lookup when `relay`, plus any
/// `direct` addresses known, e.g. from the LAN). Connects once up front so a wrong id or an
/// unreachable server is reported here.
pub async fn bridge(server: EndpointId, relay: bool, direct: Vec<SocketAddr>) -> anyhow::Result<Bridge> {
    let endpoint = builder(relay).bind().await?;
    let target = EndpointAddr { id: server, addrs: direct.into_iter().map(TransportAddr::Ip).collect() };
    let first = tokio::time::timeout(std::time::Duration::from_secs(20), endpoint.connect(target.clone(), ALPN))
        .await
        .map_err(|_| anyhow::anyhow!("the server didn't answer through Corrent Connect"))??;
    let conn: Arc<tokio::sync::Mutex<Option<Connection>>> = Arc::new(tokio::sync::Mutex::new(Some(first)));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let ep = endpoint.clone();
    let task = tokio::spawn(async move {
        loop {
            let (tcp, peer) = match listener.accept().await {
                Ok(c) => c,
                // Out of file descriptors and the like: wait a moment, keep the bridge alive.
                Err(e) => {
                    tracing::debug!("connect bridge: accept: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                }
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let (ep, conn, target) = (ep.clone(), conn.clone(), target.clone());
            tokio::spawn(async move {
                // One QUIC connection for all streams; re-dialled when it dropped.
                let c = {
                    let mut slot = conn.lock().await;
                    match slot.as_ref().filter(|c| c.close_reason().is_none()) {
                        Some(c) => c.clone(),
                        None => match ep.connect(target, ALPN).await {
                            Ok(c) => {
                                *slot = Some(c.clone());
                                c
                            }
                            Err(e) => {
                                tracing::debug!("connect: dial failed: {e}");
                                return;
                            }
                        },
                    }
                };
                let Ok((send, recv)) = c.open_bi().await else { return };
                let mut tcp = tcp;
                let mut quic = tokio::io::join(recv, send);
                let _ = tokio::io::copy_bidirectional(&mut tcp, &mut quic).await;
            });
        }
    });
    Ok(Bridge { addr, endpoint, task })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_addresses_are_private_and_stable() {
        let id = SecretKey::from_bytes(&[7; 32]).public();
        let ip = peer_ip(&id);
        assert_eq!(ip, peer_ip(&id));
        assert!(crate::net::is_local(ip, &[]), "{ip}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn key_is_kept() {
        let dir = std::env::temp_dir().join(format!("corrent-connect-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = load_or_create_key(&dir).unwrap();
        let b = load_or_create_key(&dir).unwrap();
        assert_eq!(a.public(), b.public());
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn echo_router() -> Router {
        Router::new().route(
            "/hello",
            axum::routing::get(|ConnectInfo(a): ConnectInfo<SocketAddr>, p: axum::Extension<P2pPeer>| async move {
                format!("hello {} {}", a.ip(), p.0.0.fmt_short())
            }),
        )
    }

    /// Direct (no relay): an HTTP request through the bridge reaches the router with the
    /// client's key attached.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn http_through_the_bridge_direct() {
        let t0 = std::time::Instant::now();
        let server = serve(echo_router().await, SecretKey::generate(), false, Some("127.0.0.1:0".parse().unwrap()))
            .await
            .unwrap();
        let direct: Vec<SocketAddr> = server.endpoint.bound_sockets();
        let bridge = bridge(server.id(), false, direct).await.unwrap();
        let connected = t0.elapsed();
        let body = reqwest::get(format!("{}/hello", bridge.url())).await.unwrap().text().await.unwrap();
        assert!(body.starts_with("hello fd63:6f6e:"), "{body}");
        // Several requests (each its own stream).
        for _ in 0..5 {
            assert_eq!(reqwest::get(format!("{}/hello", bridge.url())).await.unwrap().status(), 200);
        }
        eprintln!("connect direct: connected in {connected:?}, 6 requests in {:?}", t0.elapsed());
    }

    /// Through n0's public relays and address lookup (needs the internet):
    /// `CORRENT_TEST_RELAY=1 cargo test -p corrent-server --features p2p relayed -- --ignored`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "uses the public n0 relays"]
    async fn http_through_the_bridge_relayed() {
        if std::env::var_os("CORRENT_TEST_RELAY").is_none() {
            return;
        }
        let t0 = std::time::Instant::now();
        let server = serve(echo_router().await, SecretKey::generate(), true, None).await.unwrap();
        server.endpoint.online().await;
        let online = t0.elapsed();
        let bridge = bridge(server.id(), true, Vec::new()).await.unwrap();
        let body = reqwest::get(format!("{}/hello", bridge.url())).await.unwrap().text().await.unwrap();
        assert!(body.starts_with("hello "), "{body}");
        eprintln!("connect relayed: online in {online:?}, first response after {:?}", t0.elapsed());
    }
}
