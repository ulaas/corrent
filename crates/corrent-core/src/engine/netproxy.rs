//! STABILITY.md S8: a TCP proxy between a test seeder and the engine that misbehaves on
//! purpose: latency, a bandwidth cap, a reset after some bytes, a black hole (connections stay
//! open, nothing passes) and a network that is down (new connections refused, live ones cut).
//! The mode can change while connections are open.
// Test tooling: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Transparent,
    /// Each chunk (up to 16 KiB) waits this long before it is passed on, in both directions
    /// (one at a time: a slow link more than a long one).
    Latency(Duration),
    /// At most this many bytes per second, each direction.
    Bandwidth(u64),
    /// Connections are reset (RST) once this many bytes passed (then the proxy is transparent
    /// again: one reset).
    ResetAfter(u64),
    /// Connections stay open but nothing passes.
    Blackhole,
    /// New connections are refused and open ones cut.
    Down,
}

pub struct Proxy {
    pub addr: SocketAddr,
    mode: watch::Sender<Mode>,
    accepted: Arc<AtomicUsize>,
    resets: Arc<AtomicUsize>,
    _task: tokio::task::JoinHandle<()>,
}

impl Proxy {
    pub fn set(&self, m: Mode) {
        self.mode.send_replace(m);
    }

    /// Connections accepted so far.
    pub fn accepted(&self) -> usize {
        self.accepted.load(Ordering::Relaxed)
    }

    /// Connections reset by `ResetAfter`.
    pub fn resets(&self) -> usize {
        self.resets.load(Ordering::Relaxed)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self._task.abort();
    }
}

/// A proxy on 127.0.0.1 in front of `upstream`.
pub async fn start(upstream: SocketAddr) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (mode, rx) = watch::channel(Mode::Transparent);
    let accepted = Arc::new(AtomicUsize::new(0));
    let resets = Arc::new(AtomicUsize::new(0));
    let (acc, rs, tx) = (accepted.clone(), resets.clone(), mode.clone());
    let task = tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else { break };
            if *rx.borrow() == Mode::Down {
                drop(client);
                continue;
            }
            acc.fetch_add(1, Ordering::Relaxed);
            let (rx, rs, tx) = (rx.clone(), rs.clone(), tx.clone());
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(upstream).await else { return };
                connection(client, server, rx, rs, tx).await;
            });
        }
    });
    Proxy { addr, mode, accepted, resets, _task: task }
}

async fn connection(
    client: TcpStream,
    server: TcpStream,
    rx: watch::Receiver<Mode>,
    resets: Arc<AtomicUsize>,
    tx: watch::Sender<Mode>,
) {
    // Closing resets (RST) rather than ending politely: a cut connection looks like one. (A
    // zero linger never blocks on drop, which is what the deprecation is about.)
    #[allow(deprecated)]
    {
        let _ = client.set_linger(Some(Duration::ZERO));
        let _ = server.set_linger(Some(Duration::ZERO));
    }
    let passed = Arc::new(AtomicU64::new(0));
    let (cr, cw) = client.into_split();
    let (sr, sw) = server.into_split();
    let up = pump(cr, sw, rx.clone(), passed.clone());
    let down = pump(sr, cw, rx.clone(), passed.clone());
    // Either side ending (or a reset) ends both.
    let reset = tokio::select! {
        r = up => r,
        r = down => r,
    };
    if reset {
        resets.fetch_add(1, Ordering::Relaxed);
        // One reset: transparent again.
        tx.send_replace(Mode::Transparent);
    }
    // Both halves of both sockets drop here.
}

/// Copies one direction under the current mode. True when `ResetAfter` cut it.
async fn pump(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    mut rx: watch::Receiver<Mode>,
    passed: Arc<AtomicU64>,
) -> bool {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let mode = *rx.borrow_and_update();
        match mode {
            Mode::Down => return false,
            Mode::Blackhole => {
                // Hold everything until the mode changes.
                if rx.changed().await.is_err() {
                    return false;
                }
                continue;
            }
            _ => {}
        }
        let n = tokio::select! {
            r = from.read(&mut buf) => match r {
                Ok(0) | Err(_) => return false,
                Ok(n) => n,
            },
            c = rx.changed() => {
                if c.is_err() {
                    return false;
                }
                continue;
            }
        };
        match mode {
            Mode::Latency(d) => tokio::time::sleep(d).await,
            Mode::Bandwidth(bps) if bps > 0 => {
                tokio::time::sleep(Duration::from_secs_f64(n as f64 / bps as f64)).await;
            }
            _ => {}
        }
        // A mode that changed meanwhile (a black hole, the network going down) holds the data.
        if matches!(*rx.borrow(), Mode::Down) {
            return false;
        }
        let total = passed.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        if let Mode::ResetAfter(limit) = mode
            && total > limit
        {
            return true;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return false;
        }
    }
}
