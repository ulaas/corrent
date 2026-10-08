//! Local HTTP streaming of torrent files to media players (watch while downloading).
//!
//! Listens on 127.0.0.1 only. Every URL carries a random per-run token, so other local programs
//! can't enumerate or read torrents. Supports GET/HEAD and single byte ranges (seeking).
//! Reading a librqbit `FileStream` makes the engine prioritize the pieces being played.

use std::sync::{Arc, Mutex};

use librqbit::Session;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::Id20;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::types::InfoHash;

pub struct StreamServer {
    pub port: u16,
    pub token: String,
    session: Mutex<Option<Arc<Session>>>,
    streams: Arc<tokio::sync::Semaphore>,
}

fn random_token() -> String {
    crate::util::random_hex(16)
}

/// Each open stream holds one of librqbit's blocking-I/O permits (8 by default, shared with
/// piece writes); keep enough free for the engine.
const MAX_STREAMS: usize = 3;
const HEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

impl StreamServer {
    pub async fn start() -> anyhow::Result<Arc<StreamServer>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = Arc::new(StreamServer {
            port,
            token: random_token(),
            session: Mutex::new(None),
            streams: Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS)),
        });
        let s = server.clone();
        tokio::spawn(async move {
            loop {
                let conn = match listener.accept().await {
                    Ok((conn, _)) => conn,
                    Err(e) => {
                        // e.g. out of file handles: back off instead of spinning.
                        tracing::debug!("stream accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                };
                let s = s.clone();
                tokio::spawn(async move {
                    if let Err(e) = s.handle(conn).await {
                        tracing::debug!("stream request failed: {e:#}");
                    }
                });
            }
        });
        Ok(server)
    }

    pub fn set_session(&self, s: Option<Arc<Session>>) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    async fn handle(&self, mut conn: TcpStream) -> anyhow::Result<()> {
        let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut conn)).await??;
        let req = parse_request(&head).ok_or_else(|| anyhow::anyhow!("bad request"))?;
        let Some((hash, file)) = parse_path(&req.path, &self.token) else {
            return respond(&mut conn, "404 Not Found", &[], b"not found").await;
        };
        let session = self.session.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(handle) = session.and_then(|s| s.get(TorrentIdOrHash::Hash(Id20::new(hash.0)))) else {
            return respond(&mut conn, "404 Not Found", &[], b"torrent not found").await;
        };
        let info = handle
            .with_metadata(|m| {
                m.file_infos.get(file).map(|f| (f.relative_filename.to_string_lossy().into_owned(), f.len))
            })
            .ok()
            .flatten();
        let Some((name, len)) = info else {
            return respond(&mut conn, "404 Not Found", &[], b"file not found").await;
        };
        let ctype = content_type(&name);
        let (start, end, status) = match req.range {
            Some((s, e)) => {
                let e = e.unwrap_or(len.saturating_sub(1)).min(len.saturating_sub(1));
                if len == 0 || s > e {
                    let cr = format!("bytes */{len}");
                    return respond(&mut conn, "416 Range Not Satisfiable", &[("Content-Range", &cr)], b"").await;
                }
                (s, e, "206 Partial Content")
            }
            None => (0, len.saturating_sub(1), "200 OK"),
        };
        let body_len = if len == 0 { 0 } else { end - start + 1 };
        let permit = if req.head || body_len == 0 {
            None
        } else {
            match self.streams.clone().try_acquire_owned() {
                Ok(p) => Some(p),
                Err(_) => {
                    let busy: &[u8] = b"too many streams open; close a player and try again";
                    return respond(&mut conn, "503 Service Unavailable", &[("Retry-After", "2")], busy).await;
                }
            }
        };
        let mut headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {body_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n"
        );
        if status.starts_with("206") {
            headers.push_str(&format!("Content-Range: bytes {start}-{end}/{len}\r\n"));
        }
        headers.push_str("\r\n");
        conn.write_all(headers.as_bytes()).await?;
        if req.head || body_len == 0 {
            return Ok(());
        }
        let _permit = permit;
        let mut stream = handle.stream(file).await?;
        stream.seek(std::io::SeekFrom::Start(start)).await?;
        let mut limited = stream.take(body_len);
        tokio::io::copy(&mut limited, &mut conn).await?;
        conn.flush().await?;
        Ok(())
    }
}

pub fn stream_url(port: u16, token: &str, hash: InfoHash, file: usize, name: &str) -> String {
    let file_name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    format!("http://127.0.0.1:{port}/{token}/{}/{file}/{}", hash.to_hex(), super::types::url_encode(file_name))
}

async fn read_head(conn: &mut TcpStream) -> anyhow::Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 512];
    loop {
        let n = conn.read(&mut byte).await?;
        anyhow::ensure!(n > 0, "connection closed");
        buf.extend_from_slice(&byte[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        anyhow::ensure!(buf.len() < 16 * 1024, "request head too large");
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[derive(Debug, PartialEq)]
struct Request {
    path: String,
    head: bool,
    range: Option<(u64, Option<u64>)>,
}

fn parse_request(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?;
    let path = first.next()?.to_string();
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return None,
    };
    let mut range = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':')
            && k.trim().eq_ignore_ascii_case("range")
        {
            range = parse_range(v.trim());
        }
    }
    Some(Request { path, head: head_only, range })
}

/// `bytes=START-[END]` (single range only).
fn parse_range(v: &str) -> Option<(u64, Option<u64>)> {
    let r = v.strip_prefix("bytes=")?;
    if r.contains(',') {
        return None;
    }
    let (s, e) = r.split_once('-')?;
    let start = s.trim().parse().ok()?;
    let end = if e.trim().is_empty() { None } else { Some(e.trim().parse().ok()?) };
    Some((start, end))
}

/// `/<token>/<hash>/<file>/<name>` → (hash, file), only with the right token.
fn parse_path(path: &str, token: &str) -> Option<(InfoHash, usize)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != token {
        return None;
    }
    let hash = InfoHash::from_hex(parts.next()?)?;
    let file = parts.next()?.parse().ok()?;
    Some((hash, file))
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "m4a" | "aac" => "audio/aac",
        "wav" => "audio/wav",
        "srt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub fn is_media(name: &str) -> bool {
    let ct = content_type(name);
    ct.starts_with("video/") || ct.starts_with("audio/")
}

async fn respond(conn: &mut TcpStream, status: &str, extra: &[(&str, &str)], body: &[u8]) -> anyhow::Result<()> {
    let mut h = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in extra {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    conn.write_all(h.as_bytes()).await?;
    conn.write_all(body).await?;
    Ok(())
}

/// Write an .m3u playlist for `url` and open it with the default media player.
pub fn open_in_player(dir: &std::path::Path, title: &str, url: &str) -> std::io::Result<()> {
    let path = dir.join("corrent-stream.m3u");
    std::fs::write(&path, format!("#EXTM3U\n#EXTINF:-1,{title}\n{url}\n"))?;
    open::that_detached(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_parsing() {
        let r = parse_request("GET /t/abc/0/x.mkv HTTP/1.1\r\nHost: x\r\nRange: bytes=100-199\r\n\r\n").unwrap();
        assert_eq!(r.range, Some((100, Some(199))));
        assert!(!r.head);
        let r = parse_request("HEAD / HTTP/1.1\r\nrange: bytes=5-\r\n\r\n").unwrap();
        assert!(r.head);
        assert_eq!(r.range, Some((5, None)));
        assert!(parse_request("POST / HTTP/1.1\r\n\r\n").is_none());
        assert_eq!(parse_range("bytes=0-1,5-6"), None);
    }

    #[test]
    fn path_requires_token() {
        let h = InfoHash([0xab; 20]);
        let url = stream_url(1234, "secret", h, 3, "dir/My Movie.mkv");
        assert!(url.ends_with("/3/My%20Movie.mkv"));
        let path = url.trim_start_matches("http://127.0.0.1:1234");
        assert_eq!(parse_path(path, "secret"), Some((h, 3)));
        assert_eq!(parse_path(path, "other"), None);
        assert!(is_media("a.MKV") && !is_media("a.nfo"));
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 32);
    }
}
