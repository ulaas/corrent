//! Single-instance enforcement with argument forwarding over a per-user local socket.
//!
//! The first instance listens; later launches (e.g. a browser opening a magnet link) connect,
//! send their arguments and exit. Protocol: UTF-8 lines `ADD <item>` and `FOCUS`.

use std::io::{BufRead, BufReader, Write};
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

fn try_forward(name: &Name<'_>, items: &[String]) -> std::io::Result<()> {
    let mut conn = Stream::connect(name.borrow())?;
    let mut msg = String::new();
    for it in items {
        msg.push_str("ADD ");
        msg.push_str(&it.replace(['\r', '\n'], ""));
        msg.push('\n');
    }
    msg.push_str("FOCUS\n");
    conn.write_all(msg.as_bytes())?;
    conn.flush()?;
    Ok(())
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
        if try_forward(&name, &items).is_ok() {
            tracing::info!("forwarded {} item(s) to the running instance", items.len());
            return Acquire::Forwarded;
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
) {
    std::thread::Builder::new()
        .name("corrent-ipc".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let (tx, ctx) = (tx.clone(), ctx.clone());
                // One thread per client so a stalled client cannot block later launches.
                let _ = std::thread::Builder::new().name("corrent-ipc-client".into()).spawn(move || {
                    // Bound the amount of data a client may send.
                    let reader = BufReader::new(std::io::Read::take(conn, 256 * 1024));
                    for line in reader.lines().take(512) {
                        let Ok(line) = line else { break };
                        if let Some(msg) = parse_line(&line) {
                            let _ = tx.send(msg);
                        }
                    }
                    if let Some(c) = ctx.get() {
                        c.request_repaint();
                    }
                });
            }
        })
        .expect("spawn ipc thread");
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

    #[test]
    fn parse() {
        assert!(matches!(parse_line("FOCUS"), Some(IpcMessage::Focus)));
        assert!(matches!(parse_line("ADD magnet:?xt=urn:btih:abc"), Some(IpcMessage::Add(AddSource::Magnet(_)))));
        assert!(parse_line("ADD C:/does/not/exist.torrent").is_none());
        assert!(parse_line("RM -rf").is_none());
    }
}
