//! Local HTTP streaming of torrent files to media players (watch while downloading).
//!
//! Listens on 127.0.0.1 only. Every URL carries a random per-run token, so other local programs
//! can't enumerate or read torrents. Supports GET/HEAD and single byte ranges (seeking).
//! Reading a librqbit `FileStream` makes the engine prioritize the pieces being played.

//!
//! Players open many short connections (probing, seeking). Connections from the same client
//! (address + User-Agent) for the same file are grouped into one *stream session*, which the
//! Streaming window shows with its state, position and throughput.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use librqbit::Session;
use librqbit::api::TorrentIdOrHash;
use librqbit::dht::Id20;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::types::{InfoHash, StreamState};

pub struct StreamServer {
    pub port: u16,
    pub token: String,
    session: Mutex<Option<Arc<Session>>>,
    streams: Arc<tokio::sync::Semaphore>,
    registry: Mutex<Registry>,
    epoch: Instant,
}

/// How long an ended session stays listed.
const LINGER: Duration = Duration::from_secs(30);
/// After "Stop", requests from that player for that file are refused this long (players retry).
const STOP_BLOCK: Duration = Duration::from_secs(10);
/// A read waiting this long for data means the player is buffering.
const STALL: Duration = Duration::from_secs(1);
/// A write blocked this long means the player isn't reading (paused or its buffer is full).
const BACKPRESSURE: Duration = Duration::from_secs(2);
const MAX_SESSIONS: usize = 32;

const PHASE_IDLE: u8 = 0;
const PHASE_READ: u8 = 1;
const PHASE_WRITE: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    ip: IpAddr,
    user_agent: String,
    hash: InfoHash,
    file: usize,
}

/// One open HTTP connection that is sending a body.
struct Conn {
    start: u64,
    position: AtomicU64,
    phase: AtomicU8,
    /// Milliseconds since the server epoch when `phase` last changed or, while writing, when
    /// the player last took data.
    phase_since: AtomicU64,
}

impl Conn {
    fn sent(&self) -> u64 {
        self.position.load(Ordering::Relaxed).saturating_sub(self.start)
    }
}

struct StreamSession {
    id: u64,
    key: Key,
    file_name: String,
    file_len: u64,
    started: Instant,
    last_seen: Instant,
    served: Arc<AtomicU64>,
    conns: Vec<Arc<Conn>>,
    last_position: u64,
    /// Bytes sent by the connection `last_position` came from.
    last_sent: u64,
    requests: u32,
    refused: u32,
    stopped_at: Option<Instant>,
    cancel: tokio::sync::watch::Sender<bool>,
}

#[derive(Default)]
struct Registry {
    next_id: u64,
    sessions: Vec<StreamSession>,
}

/// What the actor reads each tick (the engine adds torrent names, rates and availability).
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub id: u64,
    pub hash: InfoHash,
    pub file: usize,
    pub file_name: String,
    pub file_len: u64,
    pub client: IpAddr,
    pub user_agent: String,
    pub state: StreamState,
    pub connections: usize,
    pub requests: u32,
    pub refused: u32,
    pub elapsed: Duration,
    pub served: u64,
    pub position: u64,
}

/// Combine the open connections of a session into one state.
fn derive_state(stopped: bool, conns: &[(u8, Duration)]) -> StreamState {
    if stopped {
        return StreamState::Stopped;
    }
    if conns.is_empty() {
        return StreamState::Ended;
    }
    if conns.iter().any(|&(ph, d)| ph == PHASE_READ && d >= STALL) {
        return StreamState::Buffering;
    }
    if conns.iter().all(|&(ph, d)| ph == PHASE_WRITE && d >= BACKPRESSURE) {
        return StreamState::Paused;
    }
    StreamState::Playing
}

impl Registry {
    fn prune(&mut self, now: Instant) {
        self.sessions.retain(|s| !s.conns.is_empty() || now.duration_since(s.last_seen) < LINGER);
        // Many different clients: forget the oldest ended ones first (but keep Stop blocks).
        while self.sessions.len() > MAX_SESSIONS {
            let blocked = |s: &StreamSession| s.stopped_at.is_some_and(|at| now.duration_since(at) < STOP_BLOCK);
            let Some(i) = self.sessions.iter().position(|s| s.conns.is_empty() && !blocked(s)) else { break };
            self.sessions.remove(i);
        }
    }

    /// The session for `key`, created (or restarted after a Stop) if needed. `None` while the
    /// player is blocked after a Stop.
    fn session_for(&mut self, key: &Key, file_name: &str, file_len: u64, now: Instant) -> Option<&mut StreamSession> {
        self.prune(now);
        let found = self.sessions.iter().enumerate().find(|(_, s)| &s.key == key);
        let i = match found {
            Some((i, s)) => {
                if let Some(at) = s.stopped_at {
                    if now.duration_since(at) < STOP_BLOCK || !s.conns.is_empty() {
                        return None;
                    }
                    self.sessions.remove(i);
                    self.insert(key, file_name, file_len, now)
                } else {
                    i
                }
            }
            None => self.insert(key, file_name, file_len, now),
        };
        let s = self.sessions.get_mut(i)?;
        s.last_seen = now;
        Some(s)
    }

    fn insert(&mut self, key: &Key, file_name: &str, file_len: u64, now: Instant) -> usize {
        self.next_id += 1;
        self.sessions.push(StreamSession {
            id: self.next_id,
            key: key.clone(),
            file_name: file_name.to_string(),
            file_len,
            started: now,
            last_seen: now,
            served: Arc::new(AtomicU64::new(0)),
            conns: Vec::new(),
            last_position: 0,
            last_sent: 0,
            requests: 0,
            refused: 0,
            stopped_at: None,
            cancel: tokio::sync::watch::channel(false).0,
        });
        self.sessions.len() - 1
    }

    /// Lift a Stop block for a file (the user asked to play it again).
    fn unblock(&mut self, hash: InfoHash, file: usize) {
        self.sessions.retain(|s| !(s.stopped_at.is_some() && s.key.hash == hash && s.key.file == file));
    }

    fn stop(&mut self, id: u64, now: Instant) -> bool {
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) else { return false };
        s.stopped_at = Some(now);
        s.last_seen = now;
        s.cancel.send_replace(true);
        true
    }

    fn infos(&mut self, now: Instant, epoch: Instant) -> Vec<StreamInfo> {
        self.prune(now);
        let now_ms = now.duration_since(epoch).as_millis() as u64;
        self.sessions
            .iter()
            .map(|s| {
                let phases: Vec<(u8, Duration)> = s
                    .conns
                    .iter()
                    .map(|c| {
                        let since = c.phase_since.load(Ordering::Relaxed);
                        (c.phase.load(Ordering::Relaxed), Duration::from_millis(now_ms.saturating_sub(since)))
                    })
                    .collect();
                // The connection that has sent the most is what's playing; others are probes
                // (players read the end of a file for its index, for example).
                let position = s
                    .conns
                    .iter()
                    .max_by_key(|c| c.sent())
                    .filter(|c| c.sent() >= s.last_sent)
                    .map(|c| c.position.load(Ordering::Relaxed))
                    .unwrap_or(s.last_position);
                StreamInfo {
                    id: s.id,
                    hash: s.key.hash,
                    file: s.key.file,
                    file_name: s.file_name.clone(),
                    file_len: s.file_len,
                    client: s.key.ip,
                    user_agent: s.key.user_agent.clone(),
                    state: derive_state(s.stopped_at.is_some(), &phases),
                    connections: s.conns.len(),
                    requests: s.requests,
                    refused: s.refused,
                    elapsed: if s.conns.is_empty() { s.last_seen } else { now }.duration_since(s.started),
                    served: s.served.load(Ordering::Relaxed),
                    position,
                }
            })
            .collect()
    }
}

/// Removes its connection from the session when the request ends (however it ends).
struct ConnGuard<'a> {
    server: &'a StreamServer,
    id: u64,
    conn: Arc<Conn>,
}

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        let mut reg = self.server.registry.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some(s) = reg.sessions.iter_mut().find(|s| s.id == self.id) {
            s.conns.retain(|c| !Arc::ptr_eq(c, &self.conn));
            let sent = self.conn.sent();
            if sent >= s.last_sent {
                s.last_sent = sent;
                s.last_position = self.conn.position.load(Ordering::Relaxed);
            }
            s.last_seen = now;
        }
    }
}

/// Friendly name of the media player from its User-Agent.
pub fn player_name(ua: &str) -> String {
    let l = ua.to_ascii_lowercase();
    // (needle, name, product token whose version to show)
    const KNOWN: &[(&str, &str, &str)] = &[
        ("vlc", "VLC", "vlc/"),
        ("mpv", "mpv", "mpv "),
        ("mpc-hc", "MPC-HC", "mpc-hc/"),
        ("mpc-be", "MPC-BE", "mpc-be/"),
        ("potplayer", "PotPlayer", "potplayer/"),
        ("kodi", "Kodi", "kodi/"),
        ("iina", "IINA", "iina/"),
        ("infuse", "Infuse", "infuse/"),
        ("nsplayer", "Windows Media Player", "nsplayer/"),
        ("windows-media-player", "Windows Media Player", "windows-media-player/"),
        ("wmfsdk", "Windows Media Player", "wmfsdk/"),
        ("applecoremedia", "QuickTime", "applecoremedia/"),
        ("quicktime", "QuickTime", "quicktime/"),
        ("gstreamer", "GStreamer", "gstreamer "),
        ("kaffeine", "Kaffeine", "kaffeine/"),
        ("smplayer", "SMPlayer", "smplayer/"),
        ("lavf", "FFmpeg", "lavf/"),
        ("edg/", "Edge", "edg/"),
        ("opr/", "Opera", "opr/"),
        ("firefox", "Firefox", "firefox/"),
        ("chrome", "Chrome", "chrome/"),
        ("safari", "Safari", "version/"),
        ("curl", "curl", "curl/"),
        ("wget", "Wget", "wget/"),
    ];
    for (needle, name, token) in KNOWN {
        if l.contains(needle) {
            let version = l.find(token).map(|i| {
                ua[i + token.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect::<String>()
                    .trim_end_matches('.')
                    .to_string()
            });
            return match version.filter(|v| !v.is_empty()) {
                Some(v) => format!("{name} {v}"),
                None => (*name).to_string(),
            };
        }
    }
    // Unknown: the first product token, e.g. "MyPlayer/1.2 (…)" → "MyPlayer/1.2".
    let first: String = ua.split_whitespace().next().unwrap_or("").chars().filter(|&c| printable(c)).take(40).collect();
    first
}

/// Bytes downloaded contiguously from `position` in a file, and per-bucket availability
/// (0–255) of the whole file, from the torrent's MSB-first piece bitfield.
pub fn file_availability(
    bits: &[u8],
    piece_count: u32,
    piece_len: u64,
    file_offset: u64,
    file_len: u64,
    position: u64,
    buckets: usize,
) -> (u64, Vec<u8>) {
    let have =
        |p: u64| p < piece_count as u64 && bits.get((p / 8) as usize).is_some_and(|b| b & (0x80 >> (p % 8)) != 0);
    if piece_len == 0 || file_len == 0 {
        return (0, vec![0; buckets]);
    }
    let file_end = file_offset + file_len;
    let mut ahead = 0;
    let abs = file_offset + position.min(file_len);
    if abs < file_end {
        let mut p = abs / piece_len;
        while have(p) && p * piece_len < file_end {
            ahead = ((p + 1) * piece_len).min(file_end) - abs;
            p += 1;
        }
    }
    let strip = (0..buckets as u64)
        .map(|b| {
            let s = file_offset + file_len * b / buckets as u64;
            let e = (file_offset + file_len * (b + 1) / buckets as u64).max(s + 1);
            let (first, last) = (s / piece_len, (e - 1) / piece_len);
            let had = (first..=last).filter(|&p| have(p)).count() as u64;
            (had * 255 / (last - first + 1)) as u8
        })
        .collect();
    (ahead, strip)
}

fn random_token() -> String {
    crate::util::random_hex(16)
}

/// Each open stream holds one of librqbit's blocking-I/O permits (8 by default, shared with
/// piece writes); keep enough free for the engine.
const MAX_STREAMS: usize = 3;
const HEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// A player that hasn't taken any data for this long has stopped reading (hung, or closed without
/// closing the connection): its slot goes to the next player. Pausing in a player for this long
/// costs a reconnect, which players do by themselves.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// Tests: a shorter write timeout (ms; 0 = the default).
#[cfg(test)]
pub(crate) static TEST_WRITE_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_timeout() -> Duration {
    #[cfg(test)]
    {
        let ms = TEST_WRITE_TIMEOUT_MS.load(Ordering::Relaxed);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    WRITE_TIMEOUT
}

impl StreamServer {
    pub async fn start() -> anyhow::Result<Arc<StreamServer>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = Arc::new(StreamServer {
            port,
            token: random_token(),
            session: Mutex::new(None),
            streams: Arc::new(tokio::sync::Semaphore::new(MAX_STREAMS)),
            registry: Mutex::new(Registry::default()),
            epoch: Instant::now(),
        });
        // Weak: the server (and the session it points to) must not outlive the engine that owns
        // it, or a restarted engine would run next to the old session.
        let weak = Arc::downgrade(&server);
        tokio::spawn(async move {
            loop {
                let (conn, addr) = match listener.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        // e.g. out of file handles: back off instead of spinning.
                        tracing::debug!("stream accept failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                };
                let Some(s) = weak.upgrade() else { break };
                tokio::spawn(async move {
                    if let Err(e) = s.handle(conn, addr.ip()).await {
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

    /// Active and recently ended stream sessions.
    pub fn sessions(&self) -> Vec<StreamInfo> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner()).infos(Instant::now(), self.epoch)
    }

    /// Close a session's connections; the player is refused for a few seconds so it doesn't
    /// just reconnect.
    pub fn unblock(&self, hash: InfoHash, file: usize) {
        self.registry.lock().unwrap_or_else(|e| e.into_inner()).unblock(hash, file);
    }

    pub fn stop(&self, id: u64) -> bool {
        self.registry.lock().unwrap_or_else(|e| e.into_inner()).stop(id, Instant::now())
    }

    /// (bodies being sent, maximum).
    pub fn slots(&self) -> (usize, usize) {
        (MAX_STREAMS - self.streams.available_permits(), MAX_STREAMS)
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    async fn handle(&self, mut conn: TcpStream, ip: IpAddr) -> anyhow::Result<()> {
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
        let key = Key { ip, user_agent: req.user_agent.clone().unwrap_or_default(), hash, file };
        let sending = !req.head && body_len > 0;
        let mut permit = None;
        // Register the request with its session (and the connection, if it sends a body).
        let registered = {
            let mut reg = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            match reg.session_for(&key, &name, len, Instant::now()) {
                None => None,
                Some(s) => {
                    s.requests = s.requests.saturating_add(1);
                    // Only after the Stop check, so a refused player doesn't hold a slot.
                    if sending {
                        permit = self.streams.clone().try_acquire_owned().ok();
                    }
                    if sending && permit.is_none() {
                        s.refused = s.refused.saturating_add(1);
                        Some(None)
                    } else if sending {
                        let c = Arc::new(Conn {
                            start,
                            position: AtomicU64::new(start),
                            phase: AtomicU8::new(PHASE_IDLE),
                            phase_since: AtomicU64::new(self.now_ms()),
                        });
                        s.conns.push(c.clone());
                        Some(Some((s.id, c, s.served.clone(), s.cancel.subscribe())))
                    } else {
                        Some(None)
                    }
                }
            }
        };
        let Some(registered) = registered else {
            let stopped: &[u8] = b"stopped from Corrent";
            return respond(&mut conn, "410 Gone", &[], stopped).await;
        };
        if sending && permit.is_none() {
            let busy: &[u8] = b"too many streams open; close a player and try again";
            return respond(&mut conn, "503 Service Unavailable", &[("Retry-After", "2")], busy).await;
        }
        let guard = registered.map(|(id, c, served, cancel)| (ConnGuard { server: self, id, conn: c }, served, cancel));
        let mut headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {body_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n"
        );
        if status.starts_with("206") {
            headers.push_str(&format!("Content-Range: bytes {start}-{end}/{len}\r\n"));
        }
        headers.push_str("\r\n");
        let Some((guard, served, mut cancel)) = guard else {
            conn.write_all(headers.as_bytes()).await?;
            return Ok(());
        };
        let _permit = permit;
        let c = &guard.conn;
        let set_phase = |ph: u8| {
            c.phase.store(ph, Ordering::Relaxed);
            c.phase_since.store(self.now_ms(), Ordering::Relaxed);
        };
        // Opening the stream can wait for an I/O permit (e.g. during a recheck): that's buffering,
        // and Stop must work there too.
        set_phase(PHASE_READ);
        let setup = async {
            tokio::time::timeout(write_timeout(), conn.write_all(headers.as_bytes()))
                .await
                .map_err(|_| anyhow::anyhow!("the player stopped reading"))??;
            let mut stream = handle.stream(file).await?;
            stream.seek(std::io::SeekFrom::Start(start)).await?;
            anyhow::Ok(stream)
        };
        let stream = tokio::select! {
            r = setup => r?,
            _ = cancel.wait_for(|stop| *stop) => return Ok(()),
        };
        let mut limited = stream.take(body_len);
        // Copy by hand to track position and what the connection is waiting on. Writes go in
        // small pieces so a slow (low-bitrate) player still shows progress, not "paused".
        const PIECE: usize = 16 * 1024;
        let mut buf = vec![0u8; 64 * 1024];
        'copy: loop {
            set_phase(PHASE_READ);
            let n = tokio::select! {
                r = limited.read(&mut buf) => r?,
                _ = cancel.wait_for(|stop| *stop) => break,
            };
            if n == 0 {
                break;
            }
            set_phase(PHASE_WRITE);
            for piece in buf.get(..n).unwrap_or_default().chunks(PIECE) {
                tokio::select! {
                    r = tokio::time::timeout(write_timeout(), conn.write_all(piece)) => {
                        r.map_err(|_| anyhow::anyhow!("the player stopped reading"))??
                    }
                    _ = cancel.wait_for(|stop| *stop) => break 'copy,
                }
                c.phase_since.store(self.now_ms(), Ordering::Relaxed);
                c.position.fetch_add(piece.len() as u64, Ordering::Relaxed);
                served.fetch_add(piece.len() as u64, Ordering::Relaxed);
            }
        }
        set_phase(PHASE_IDLE);
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
        buf.extend_from_slice(byte.get(..n).unwrap_or_default());
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
    user_agent: Option<String>,
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
    let mut user_agent = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let k = k.trim();
            if k.eq_ignore_ascii_case("range") {
                range = parse_range(v.trim());
            } else if k.eq_ignore_ascii_case("user-agent") {
                user_agent = Some(v.trim().chars().filter(|&c| printable(c)).take(200).collect());
            }
        }
    }
    Some(Request { path, head: head_only, range, user_agent })
}

/// Not a control or invisible formatting character (bidi overrides could disguise a name).
fn printable(c: char) -> bool {
    !c.is_control()
        && !matches!(
            c,
            '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
        )
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
    let title: String = title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
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
    fn user_agent_is_parsed() {
        let r = parse_request("GET / HTTP/1.1\r\nUser-Agent: VLC/3.0.20 LibVLC/3.0.20\r\n\r\n").unwrap();
        assert_eq!(r.user_agent.as_deref(), Some("VLC/3.0.20 LibVLC/3.0.20"));
        assert_eq!(player_name("VLC/3.0.20 LibVLC/3.0.20"), "VLC 3.0.20");
        assert_eq!(player_name("mpv 0.38.0"), "mpv 0.38.0");
        assert_eq!(player_name("Lavf/60.16.100"), "FFmpeg 60.16.100");
        assert_eq!(
            player_name("NSPlayer/12.00.22621.2506 WMFSDK/12.00.22621.2506"),
            "Windows Media Player 12.00.22621.2506"
        );
        assert_eq!(
            player_name(
                "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/128.0.0.0 Safari/537.36 Edg/128.0.0.0"
            ),
            "Edge 128.0.0.0"
        );
        assert_eq!(player_name("Kodi/21.0 (Windows NT 10.0)"), "Kodi 21.0");
        assert_eq!(player_name("MyPlayer/1.2 (x)"), "MyPlayer/1.2");
        assert_eq!(player_name(""), "");
    }

    #[test]
    fn state_from_connections() {
        let s = Duration::from_secs;
        assert_eq!(derive_state(false, &[]), StreamState::Ended);
        assert_eq!(derive_state(true, &[(PHASE_READ, s(5))]), StreamState::Stopped);
        assert_eq!(derive_state(false, &[(PHASE_READ, Duration::from_millis(100))]), StreamState::Playing);
        assert_eq!(derive_state(false, &[(PHASE_READ, s(2))]), StreamState::Buffering);
        assert_eq!(derive_state(false, &[(PHASE_WRITE, s(3))]), StreamState::Paused);
        // A probe connection that's blocked doesn't hide the one that's playing.
        assert_eq!(derive_state(false, &[(PHASE_WRITE, s(3)), (PHASE_WRITE, Duration::ZERO)]), StreamState::Playing);
        assert_eq!(derive_state(false, &[(PHASE_WRITE, s(3)), (PHASE_READ, s(3))]), StreamState::Buffering);
    }

    #[test]
    fn sessions_group_linger_and_stop() {
        let mut reg = Registry::default();
        let t0 = Instant::now();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let key = Key { ip, user_agent: "VLC/3".into(), hash: InfoHash([1; 20]), file: 0 };
        let id = reg.session_for(&key, "a.mkv", 100, t0).unwrap().id;
        // A second request from the same player joins the session; another file doesn't.
        assert_eq!(reg.session_for(&key, "a.mkv", 100, t0).unwrap().id, id);
        let other = Key { file: 1, ..key.clone() };
        assert_ne!(reg.session_for(&other, "b.mkv", 100, t0).unwrap().id, id);
        assert_eq!(reg.infos(t0, t0).len(), 2);
        assert!(reg.infos(t0, t0).iter().all(|i| i.state == StreamState::Ended));
        // Ended sessions are forgotten after the linger time.
        assert!(reg.infos(t0 + LINGER + Duration::from_secs(1), t0).is_empty());

        let id = reg.session_for(&key, "a.mkv", 100, t0).unwrap().id;
        assert!(reg.stop(id, t0));
        assert!(reg.session_for(&key, "a.mkv", 100, t0 + Duration::from_secs(1)).is_none(), "blocked after Stop");
        assert_eq!(reg.infos(t0, t0)[0].state, StreamState::Stopped);
        let again = reg.session_for(&key, "a.mkv", 100, t0 + STOP_BLOCK + Duration::from_secs(1)).unwrap().id;
        assert_ne!(again, id, "a new session once the block expires");
        assert!(!reg.stop(9999, t0));

        // Playing again from Corrent lifts the block at once.
        let id = reg.session_for(&key, "a.mkv", 100, t0).unwrap().id;
        reg.stop(id, t0);
        reg.unblock(key.hash, key.file);
        assert!(reg.session_for(&key, "a.mkv", 100, t0).is_some());
    }

    #[test]
    fn position_follows_the_playing_connection_not_probes() {
        let mut reg = Registry::default();
        let t0 = Instant::now();
        let key = Key { ip: "127.0.0.1".parse().unwrap(), user_agent: String::new(), hash: InfoHash([2; 20]), file: 0 };
        let conn = |start: u64, sent: u64| {
            Arc::new(Conn {
                start,
                position: AtomicU64::new(start + sent),
                phase: AtomicU8::new(PHASE_READ),
                phase_since: AtomicU64::new(0),
            })
        };
        let s = reg.session_for(&key, "a.mkv", 1000, t0).unwrap();
        s.conns.push(conn(100, 500)); // playing
        s.conns.push(conn(990, 5)); // index probe at the end
        assert_eq!(reg.infos(t0, t0)[0].position, 600);
    }

    #[test]
    fn eviction_keeps_stop_blocks() {
        let mut reg = Registry::default();
        let t0 = Instant::now();
        let key = |n: u8| Key {
            ip: "127.0.0.1".parse().unwrap(),
            user_agent: n.to_string(),
            hash: InfoHash([3; 20]),
            file: 0,
        };
        let id = reg.session_for(&key(0), "a", 1, t0).unwrap().id;
        reg.stop(id, t0);
        for n in 1..=40 {
            reg.session_for(&key(n), "a", 1, t0);
        }
        assert!(reg.infos(t0, t0).len() <= MAX_SESSIONS);
        assert!(reg.session_for(&key(0), "a", 1, t0).is_none(), "still blocked");
    }

    #[test]
    fn invisible_characters_are_dropped_from_user_agents() {
        let r = parse_request("GET / HTTP/1.1\r\nUser-Agent: VLC\u{202E}evil\u{200B}/1\r\n\r\n").unwrap();
        assert_eq!(r.user_agent.as_deref(), Some("VLCevil/1"));
    }

    #[test]
    fn availability_and_buffer_ahead() {
        // 8 pieces of 10 bytes; have 0,1,2 and 5. File 1 covers bytes 15..65 (pieces 1–6).
        let bits = [0b1110_0100u8];
        let (ahead, strip) = file_availability(&bits, 8, 10, 15, 50, 0, 5);
        assert_eq!(ahead, 15, "pieces 1–2 end at byte 30 = 15 into the file");
        assert_eq!(strip, vec![255, 127, 0, 127, 127]);
        let (ahead, _) = file_availability(&bits, 8, 10, 15, 50, 20, 5);
        assert_eq!(ahead, 0, "piece 3 is missing");
        let (ahead, _) = file_availability(&bits, 8, 10, 15, 50, 40, 5);
        assert_eq!(ahead, 5, "piece 5 ends at 60 = 45 into the file");
        // Positions past the end and an empty file are safe.
        assert_eq!(file_availability(&bits, 8, 10, 15, 50, 999, 5).0, 0);
        assert_eq!(file_availability(&bits, 8, 10, 15, 0, 0, 3), (0, vec![0, 0, 0]));
        let full = [0xff, 0xff];
        assert_eq!(file_availability(&full, 16, 10, 0, 160, 0, 4), (160, vec![255; 4]));
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 32);
    }
}
