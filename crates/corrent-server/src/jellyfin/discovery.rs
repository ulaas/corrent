//! Jellyfin apps find servers by broadcasting "who is JellyfinServer?" to UDP 7359; the answer
//! names the address to use (the plain-HTTP listener, as the app's network sees us).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::state::AppState;

pub const PORT: u16 = 7359;

/// Whether a datagram is a discovery question.
pub fn is_question(msg: &[u8]) -> bool {
    std::str::from_utf8(msg).is_ok_and(|s| s.to_ascii_lowercase().contains("who is jellyfinserver?"))
}

/// Answers on `port` until dropped (the task is aborted by its owner).
pub async fn serve(st: Arc<AppState>, port: u16) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let sock = tokio::net::UdpSocket::bind(("0.0.0.0", port)).await?;
    Ok(tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        let mut window = (Instant::now(), 0u32);
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            };
            if !is_question(&buf[..n]) || !crate::net::is_local(from.ip(), &st.nets.load().allowed) {
                continue;
            }
            // At most 5 answers a second.
            if window.0.elapsed() > Duration::from_secs(1) {
                window = (Instant::now(), 0);
            }
            window.1 += 1;
            if window.1 > 5 {
                continue;
            }
            if let Some(reply) = answer(&st, from) {
                let _ = sock.send_to(reply.as_bytes(), from).await;
            }
        }
    }))
}

fn answer(st: &AppState, from: SocketAddr) -> Option<String> {
    let cfg = st.cfg.load();
    let ip = crate::cast::local_ip_towards(from)?;
    let host = match ip {
        std::net::IpAddr::V6(v) => format!("[{v}]"),
        v4 => v4.to_string(),
    };
    let address = if cfg.jellyfin_port != 0 {
        format!("http://{host}:{}", cfg.jellyfin_port)
    } else {
        format!("{}://{host}:{}/jellyfin", if cfg.tls { "https" } else { "http" }, cfg.https_port)
    };
    let c = super::system::ctx(st);
    Some(json!({ "Address": address, "Id": c.server_id, "Name": c.server_name, "EndpointAddress": null }).to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn questions() {
        assert!(super::is_question(b"Who is JellyfinServer?"));
        assert!(super::is_question(b"who is jellyfinserver?"));
        assert!(!super::is_question(b"who is EmbyServer?"));
        assert!(!super::is_question(&[0xff, 0xfe]));
    }
}
