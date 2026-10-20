//! Single-instance enforcement with argument forwarding over a per-user local socket.
//!
//! The first instance listens; later launches (e.g. a browser opening a magnet link) connect,
//! send their arguments and exit. Protocol: UTF-8 lines `ADD <item>` and a final `FOCUS`; the
//! primary answers `OK` once it has queued them for the UI. Without that answer the items count
//! as not delivered (a hung primary), and `main` decides with the data-folder lock.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream};

use crate::config::Paths;
use crate::engine::types::AddSource;

#[derive(Debug, Clone)]
pub enum IpcMessage {
    Add(AddSource),
    Focus,
}

pub enum Acquire {
    /// We are the primary instance (listener may be None if the socket could not be created).
    Primary(Option<interprocess::local_socket::Listener>),
    /// Another instance received our arguments; this process should exit.
    Forwarded,
    /// Another instance took the connection but never confirmed (hung, or a version without
    /// confirmations). It must not be joined by a second engine.
    Unresponsive,
}

/// How long a launch waits for the primary's `OK`.
const ACK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
enum ForwardError {
    /// Nothing listening (no primary yet).
    NoPrimary,
    /// Connected and sent, but no `OK` in time.
    NoAck,
}

fn socket_name(paths: &Paths) -> std::io::Result<Name<'static>> {
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "user".into());
    let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    // Portable installs get their own instance so two copies can coexist.
    let suffix = if paths.portable {
        let mut h: u32 = 2166136261;
        for b in paths.data_dir.to_string_lossy().bytes() {
            h = (h ^ b as u32).wrapping_mul(16777619);
        }
        format!("-p{h:08x}")
    } else {
        String::new()
    };
    // Windows: a named pipe (default DACL: other users cannot write to it).
    // Unix: a socket file inside the private (0700) data directory. Linux abstract sockets are
    // avoided on purpose because any local user could connect to them.
    if cfg!(windows) && GenericNamespaced::is_supported() {
        format!("corrent-{user}{suffix}.sock").to_ns_name::<GenericNamespaced>()
    } else {
        paths.data_dir.join("corrent.sock").to_fs_name::<GenericFilePath>()
    }
}

/// Canonicalize file arguments so the primary instance (with a different working directory)
/// can open them.
pub fn normalize_arg(arg: &str) -> String {
    match AddSource::parse(arg) {
        Some(AddSource::File(p)) => std::fs::canonicalize(&p)
            .map(|c| strip_verbatim(&c.to_string_lossy()))
            .unwrap_or_else(|_| p.to_string_lossy().into_owned()),
        _ => arg.to_string(),
    }
}

fn strip_verbatim(s: &str) -> String {
    s.strip_prefix(r"\\?\").unwrap_or(s).to_string()
}

fn try_forward(name: &Name<'_>, items: &[String], timeout: Duration) -> Result<(), ForwardError> {
    let mut conn = Stream::connect(name.borrow()).map_err(|_| ForwardError::NoPrimary)?;
    let mut msg = String::new();
    for it in items {
        msg.push_str("ADD ");
        msg.push_str(&it.replace(['\r', '\n'], ""));
        msg.push('\n');
    }
    msg.push_str("FOCUS\n");
    if conn.write_all(msg.as_bytes()).and_then(|()| conn.flush()).is_err() {
        // The listener went away between connect and write: as if nothing listened.
        return Err(ForwardError::NoPrimary);
    }
    if await_ack(conn, timeout) { Ok(()) } else { Err(ForwardError::NoAck) }
}

/// Wait up to `timeout` for the `OK` line. Pipes have no read timeout, so the read runs on a
/// helper thread; a primary that never answers leaves it blocked until this process exits.
fn await_ack(conn: Stream, timeout: Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("corrent-ipc-ack".into()).spawn(move || {
        let mut line = String::new();
        let ok = BufReader::new(&conn).read_line(&mut line).is_ok() && line.trim() == "OK";
        let _ = tx.send(ok);
    });
    spawned.is_ok() && rx.recv_timeout(timeout).unwrap_or(false)
}

/// Become the primary instance or forward `items` to the running one.
pub fn acquire(paths: &Paths, items: &[String]) -> Acquire {
    let name = match socket_name(paths) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("single-instance socket name unavailable: {e}");
            return Acquire::Primary(None);
        }
    };
    let items: Vec<String> = items.iter().map(|i| normalize_arg(i)).collect();
    for attempt in 0..4u64 {
        match try_forward(&name, &items, ACK_TIMEOUT) {
            Ok(()) => {
                tracing::info!("forwarded {} item(s) to the running instance", items.len());
                return Acquire::Forwarded;
            }
            // Not "still starting": retrying would only wait longer for the same silence.
            Err(ForwardError::NoAck) => return Acquire::Unresponsive,
            Err(ForwardError::NoPrimary) => {}
        }
        match ListenerOptions::new().name(name.borrow()).create_sync() {
            Ok(l) => return Acquire::Primary(Some(l)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                // Another instance is starting right now; give it a moment and try to forward.
                std::thread::sleep(Duration::from_millis(150 * (attempt + 1)));
            }
            Err(e) => {
                tracing::warn!("could not create single-instance listener: {e}");
                return Acquire::Primary(None);
            }
        }
    }
    tracing::warn!("single-instance negotiation failed; continuing as an independent instance");
    Acquire::Primary(None)
}

/// Serve forwarded launches on a background thread.
pub fn serve(
    listener: interprocess::local_socket::Listener,
    tx: std::sync::mpsc::Sender<IpcMessage>,
    ctx: Arc<OnceLock<egui::Context>>,
) -> Arc<AtomicUsize> {
    let live = Arc::new(AtomicUsize::new(0));
    let count = live.clone();
    let spawned = std::thread::Builder::new().name("corrent-ipc".into()).spawn(move || {
        for conn in listener.incoming() {
            let conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    // Never a busy loop on a listener that keeps failing.
                    tracing::debug!("single-instance accept failed: {e}");
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
            };
            // A flood of connections can't pile up threads: the launch that is turned away
            // gets no `OK` and says so.
            if count.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
                count.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let (tx, ctx, slot) = (tx.clone(), ctx.clone(), count.clone());
            // One thread per client so a stalled client cannot block later launches.
            let spawned = std::thread::Builder::new().name("corrent-ipc-client".into()).spawn(move || {
                // Frees the slot however the client ends (also if handling it panics).
                struct Slot(Arc<AtomicUsize>);
                impl Drop for Slot {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _slot = Slot(slot);
                handle_client(&conn, &tx);
                if let Some(c) = ctx.get() {
                    c.request_repaint();
                }
            });
            if spawned.is_err() {
                count.fetch_sub(1, Ordering::SeqCst);
            }
        }
    });
    // Without the thread, later launches can't forward to this one; the data-folder lock still
    // keeps them out.
    if let Err(e) = spawned {
        tracing::warn!("no IPC thread ({e}); running without single-instance forwarding");
    }
    live
}

/// Clients served at once.
const MAX_CLIENTS: usize = 8;
/// A client has this long to send its lines (named pipes have no read timeout, so the reads
/// don't block and are polled).
const CLIENT_DEADLINE: Duration = Duration::from_secs(2);

fn handle_client(conn: &Stream, tx: &std::sync::mpsc::Sender<IpcMessage>) {
    #[cfg(debug_assertions)]
    if std::env::var("CORRENT_DEV_HANG").is_ok_and(|v| v == "ipc") {
        // Debug hook: a primary that takes connections but never queues them.
        std::thread::sleep(Duration::from_secs(3600));
        return;
    }
    let polled = conn.set_nonblocking(true).is_ok();
    let deadline = std::time::Instant::now() + CLIENT_DEADLINE;
    let (mut buf, mut total, mut lines) = (Vec::new(), 0usize, 0usize);
    let mut chunk = [0u8; 4096];
    // One line; true when the client is done with (FOCUS answered, the UI gone, too many lines).
    let mut handle = |line: &[u8]| -> bool {
        lines += 1;
        let msg = parse_line(&String::from_utf8_lossy(line));
        let focus = matches!(msg, Some(IpcMessage::Focus));
        // A closed queue means the UI is gone: then no `OK`.
        if let Some(msg) = msg
            && tx.send(msg).is_err()
        {
            return true;
        }
        if focus {
            // Everything before FOCUS is queued.
            let _ = conn.set_nonblocking(false);
            let _ = (&mut &*conn).write_all(b"OK\n").and_then(|()| (&mut &*conn).flush());
            return true;
        }
        lines >= 512
    };
    loop {
        match std::io::Read::read(&mut &*conn, &mut chunk) {
            // A non-blocking named pipe reads 0 bytes both when nothing has arrived yet and at
            // the end, so on Windows it means "nothing yet" until the deadline.
            Ok(0) if cfg!(windows) && polled => {
                if std::time::Instant::now() > deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(0) => break,
            Ok(n) => {
                total += n;
                buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
                while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    if handle(line.strip_suffix(b"\n").unwrap_or(&line)) {
                        return;
                    }
                }
                if total > 256 * 1024 {
                    return;
                }
            }
            Err(e) if polled && e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    tracing::debug!("a forwarding launch sent nothing for {CLIENT_DEADLINE:?}");
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }
    // The last line may have no newline (the stream ended after it).
    if !buf.is_empty() {
        handle(&buf);
    }
}

fn parse_line(line: &str) -> Option<IpcMessage> {
    let line = line.trim();
    if line == "FOCUS" {
        return Some(IpcMessage::Focus);
    }
    let item = line.strip_prefix("ADD ")?;
    match AddSource::parse(item)? {
        AddSource::File(p) => {
            // Only accept existing .torrent files from other processes.
            let ok = p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent"));
            ok.then_some(IpcMessage::Add(AddSource::File(p)))
        }
        other => Some(IpcMessage::Add(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private pipe/socket name for one test.
    fn test_name(tag: &str) -> (Paths, Name<'static>) {
        let dir = std::env::temp_dir().join(format!("corrent-ipc-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths { data_dir: dir, portable: true };
        let name = socket_name(&paths).unwrap();
        (paths, name)
    }

    #[test]
    fn forwarding_waits_for_the_primary_to_queue() {
        let (paths, name) = test_name("ack");
        let listener = ListenerOptions::new().name(name.borrow()).create_sync().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        serve(listener, tx, Arc::new(OnceLock::new()));
        let items = vec!["magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_string()];
        try_forward(&name, &items, Duration::from_secs(5)).unwrap();
        // The OK came after both lines were queued.
        assert!(matches!(rx.try_recv(), Ok(IpcMessage::Add(AddSource::Magnet(_)))));
        assert!(matches!(rx.try_recv(), Ok(IpcMessage::Focus)));
        let _ = std::fs::remove_dir_all(paths.data_dir);
    }

    #[test]
    fn a_silent_primary_is_reported_not_trusted() {
        let (paths, name) = test_name("silent");
        let listener = ListenerOptions::new().name(name.borrow()).create_sync().unwrap();
        // Takes the connection and reads, but never answers (like a hung UI or an old version).
        std::thread::spawn(move || {
            let conn = listener.incoming().next().unwrap().unwrap();
            let _ = BufReader::new(&conn).lines().count();
        });
        let started = std::time::Instant::now();
        let r = try_forward(&name, &[], Duration::from_millis(500));
        assert!(matches!(r, Err(ForwardError::NoAck)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        let _ = std::fs::remove_dir_all(paths.data_dir);
    }

    #[test]
    fn silent_clients_are_let_go_and_capped() {
        let (paths, name) = test_name("silent-clients");
        let listener = ListenerOptions::new().name(name.borrow()).create_sync().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let live = serve(listener, tx, Arc::new(OnceLock::new()));
        // Twelve launches that connect and then say nothing.
        let silent: Vec<Stream> = (0..12).filter_map(|_| Stream::connect(name.borrow()).ok()).collect();
        assert!(!silent.is_empty());
        let mut most = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            most = most.max(live.load(Ordering::SeqCst));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(most <= MAX_CLIENTS, "{most} clients at once");
        let deadline = std::time::Instant::now() + CLIENT_DEADLINE + Duration::from_secs(2);
        while live.load(Ordering::SeqCst) > 0 {
            assert!(std::time::Instant::now() < deadline, "silent clients still held");
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(silent);
        // And a real launch still gets through, also one that writes a moment after connecting.
        try_forward(&name, &[], Duration::from_secs(5)).unwrap();
        let mut late = Stream::connect(name.borrow()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        late.write_all(b"FOCUS\n").unwrap();
        late.flush().unwrap();
        assert!(await_ack(late, Duration::from_secs(5)), "a late writer still gets its OK");
        let _ = std::fs::remove_dir_all(paths.data_dir);
    }

    #[test]
    fn nobody_listening_is_not_a_hang() {
        let (paths, name) = test_name("none");
        assert!(matches!(try_forward(&name, &[], Duration::from_millis(500)), Err(ForwardError::NoPrimary)));
        let _ = std::fs::remove_dir_all(paths.data_dir);
    }

    #[test]
    fn parse() {
        assert!(matches!(parse_line("FOCUS"), Some(IpcMessage::Focus)));
        assert!(matches!(parse_line("ADD magnet:?xt=urn:btih:abc"), Some(IpcMessage::Add(AddSource::Magnet(_)))));
        assert!(parse_line("ADD C:/does/not/exist.torrent").is_none());
        assert!(parse_line("RM -rf").is_none());
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    #[test]
    fn non_file_args_pass_through() {
        assert_eq!(normalize_arg("magnet:?xt=urn:btih:abc"), "magnet:?xt=urn:btih:abc");
        assert_eq!(normalize_arg("https://x/y.torrent"), "https://x/y.torrent");
    }

    #[test]
    fn existing_non_torrent_files_are_rejected() {
        let f = std::env::temp_dir().join(format!("corrent-ipc-{}.txt", std::process::id()));
        std::fs::write(&f, "x").unwrap();
        assert!(parse_line(&format!("ADD {}", f.display())).is_none());
        let t = f.with_extension("torrent");
        std::fs::write(&t, "x").unwrap();
        assert!(matches!(parse_line(&format!("ADD {}", t.display())), Some(IpcMessage::Add(AddSource::File(_)))));
        let _ = std::fs::remove_file(f);
        let _ = std::fs::remove_file(t);
    }
}
