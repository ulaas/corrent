//! STABILITY.md S8: an HTTP server that misbehaves on purpose, for the clients Corrent talks to
//! (feeds, Torznab search, the update check, torrent URLs): error statuses, cut-off and
//! oversized bodies, slow answers, redirect loops, connections that hang or reset. Plain
//! HTTP/1.1, one answer per connection (`Connection: close`).
// Test tooling: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub enum Route {
    /// A normal answer.
    Answer { status: u16, content_type: &'static str, body: Vec<u8> },
    /// The body a few bytes at a time with a pause between.
    Slow { body: Vec<u8>, chunk: usize, pause: Duration },
    /// Content-Length says the whole body, the connection closes after `sent` bytes.
    Truncated { body: Vec<u8>, sent: usize },
    /// `len` bytes of zeros (no Content-Length): a body that never seems to end.
    Huge { len: usize },
    /// 302 to itself.
    RedirectLoop,
    /// Reads the request and never answers (the connection stays open).
    Hang,
    /// Reads the request and resets the connection.
    Reset,
}

pub struct FaultServer {
    pub base: String,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl FaultServer {
    pub fn route(&self, path: &str, r: Route) {
        crate::util::lock(&self.routes).insert(path.to_string(), r);
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn hits(&self, path: &str) -> usize {
        crate::util::lock(&self.hits).get(path).copied().unwrap_or(0)
    }
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        self._task.abort();
    }
}

pub async fn start() -> FaultServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
    let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let (r2, h2) = (routes.clone(), hits.clone());
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((mut c, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = c.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
            *crate::util::lock(&h2).entry(path.clone()).or_default() += 1;
            let route = crate::util::lock(&r2).get(&path).cloned();
            let Some(route) = route else {
                let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            };
            if let Route::Hang = route {
                held.push(c);
                continue;
            }
            tokio::spawn(async move {
                let head = |status: u16, ctype: &str, len: Option<usize>| {
                    let reason = if status < 400 { "OK" } else { "Error" };
                    let len = len.map(|l| format!("Content-Length: {l}\r\n")).unwrap_or_default();
                    format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\n{len}Connection: close\r\n\r\n")
                };
                match route {
                    Route::Answer { status, content_type, body } => {
                        let _ = c.write_all(head(status, content_type, Some(body.len())).as_bytes()).await;
                        let _ = c.write_all(&body).await;
                    }
                    Route::Slow { body, chunk, pause } => {
                        let _ = c.write_all(head(200, "application/octet-stream", Some(body.len())).as_bytes()).await;
                        for piece in body.chunks(chunk.max(1)) {
                            if c.write_all(piece).await.is_err() {
                                return;
                            }
                            tokio::time::sleep(pause).await;
                        }
                    }
                    Route::Truncated { body, sent } => {
                        let _ = c.write_all(head(200, "application/octet-stream", Some(body.len())).as_bytes()).await;
                        let _ = c.write_all(&body[..sent.min(body.len())]).await;
                    }
                    Route::Huge { len } => {
                        let _ = c.write_all(head(200, "application/octet-stream", None).as_bytes()).await;
                        let zeros = vec![b'0'; 64 * 1024];
                        let mut left = len;
                        while left > 0 {
                            let n = left.min(zeros.len());
                            if c.write_all(&zeros[..n]).await.is_err() {
                                return;
                            }
                            left -= n;
                        }
                    }
                    Route::RedirectLoop => {
                        let loc = format!(
                            "HTTP/1.1 302 Found\r\nLocation: {path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = c.write_all(loc.as_bytes()).await;
                    }
                    Route::Reset => {
                        #[allow(deprecated)]
                        let _ = c.set_linger(Some(Duration::ZERO));
                    }
                    Route::Hang => {}
                }
                let _ = c.shutdown().await;
            });
        }
    });
    FaultServer { base, routes, hits, _task: task }
}
