//! A connection to an AirPlay receiver: HTTP/1.1 or RTSP/1.0 requests one after another on one
//! TCP stream, in the clear until pairing is verified and inside HomeKit's framing afterwards
//! (`length (2, little-endian) | ChaCha20-Poly1305 ciphertext | tag`, at most 1024 bytes of text a
//! frame, a counter per direction as the nonce, the length as associated data).

use std::net::SocketAddr;
use std::time::Duration;

use aws_lc_rs::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const FRAME: usize = 1024;
const TAG: usize = 16;
const MAX_HEAD: usize = 32 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

struct Sealed {
    write: LessSafeKey,
    read: LessSafeKey,
    sent: u64,
    received: u64,
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    Nonce::assume_unique_for_key(n)
}

fn key(bytes: &[u8; 32]) -> anyhow::Result<LessSafeKey> {
    Ok(LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, bytes).map_err(|_| anyhow::anyhow!("bad key"))?))
}

pub struct Conn {
    stream: TcpStream,
    sealed: Option<Sealed>,
    /// Bytes from the socket not yet taken apart (sealed) or parsed (clear).
    raw: Vec<u8>,
    /// Text ready to parse.
    text: Vec<u8>,
}

impl Conn {
    pub async fn connect(addr: SocketAddr) -> anyhow::Result<Conn> {
        if !addr.is_ipv4() || !crate::net::is_local(addr.ip(), &[]) {
            anyhow::bail!("not a device on this network");
        }
        let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr)).await??;
        let _ = stream.set_nodelay(true);
        Ok(Conn { stream, sealed: None, raw: Vec::new(), text: Vec::new() })
    }

    /// From the next request on, everything goes inside the framing. `write`: what we send with.
    pub fn secure(&mut self, write: &[u8; 32], read: &[u8; 32]) -> anyhow::Result<()> {
        self.sealed = Some(Sealed { write: key(write)?, read: key(read)?, sent: 0, received: 0 });
        Ok(())
    }

    async fn send(&mut self, text: &[u8]) -> anyhow::Result<()> {
        let Some(s) = &mut self.sealed else {
            self.stream.write_all(text).await?;
            return Ok(());
        };
        let mut out = Vec::with_capacity(text.len() + text.len() / FRAME * 18 + 18);
        for part in text.chunks(FRAME) {
            let len = (part.len() as u16).to_le_bytes();
            let mut frame = part.to_vec();
            s.write
                .seal_in_place_append_tag(nonce(s.sent), Aad::from(len), &mut frame)
                .map_err(|_| anyhow::anyhow!("sealing failed"))?;
            s.sent += 1;
            out.extend_from_slice(&len);
            out.extend_from_slice(&frame);
        }
        self.stream.write_all(&out).await?;
        Ok(())
    }

    /// More text into `self.text`: one read from the socket, opened if the channel is sealed.
    async fn more(&mut self) -> anyhow::Result<()> {
        let mut buf = [0u8; 8192];
        let n = self.stream.read(&mut buf).await?;
        if n == 0 {
            anyhow::bail!("the device closed the connection");
        }
        let Some(s) = &mut self.sealed else {
            self.text.extend_from_slice(buf.get(..n).unwrap_or_default());
            return Ok(());
        };
        self.raw.extend_from_slice(buf.get(..n).unwrap_or_default());
        while let (Some(a), Some(b)) = (self.raw.first().copied(), self.raw.get(1).copied()) {
            let len = usize::from(u16::from_le_bytes([a, b]));
            if len > FRAME {
                anyhow::bail!("a frame larger than the protocol allows");
            }
            let Some(frame) = self.raw.get(2..2 + len + TAG) else { break };
            let mut frame = frame.to_vec();
            let aad = (len as u16).to_le_bytes();
            let plain = s
                .read
                .open_in_place(nonce(s.received), Aad::from(aad), &mut frame)
                .map_err(|_| anyhow::anyhow!("the device's answer doesn't open with the session's key"))?;
            s.received += 1;
            self.text.extend_from_slice(plain);
            self.raw.drain(..2 + len + TAG);
        }
        Ok(())
    }

    /// One request and its answer. `protocol`: `HTTP/1.1` or `RTSP/1.0`.
    pub async fn request(
        &mut self,
        protocol: &str,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> anyhow::Result<Response> {
        let mut text = format!("{method} {path} {protocol}\r\n");
        if !body.is_empty() {
            text.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        for (k, v) in headers {
            // Header values are ours, but nothing with a line break may ever go out.
            if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
                anyhow::bail!("a line break in a header");
            }
            text.push_str(&format!("{k}: {v}\r\n"));
        }
        text.push_str("\r\n");
        let mut bytes = text.into_bytes();
        bytes.extend_from_slice(body);
        tokio::time::timeout(TIMEOUT, async {
            self.send(&bytes).await?;
            self.answer().await
        })
        .await
        .map_err(|_| anyhow::anyhow!("the device didn't answer {method} {path}"))?
    }

    async fn answer(&mut self) -> anyhow::Result<Response> {
        let (first, headers, body) = self.message().await?;
        let status: u16 = first
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("not an HTTP answer"))?;
        Ok(Response { status, headers, body })
    }

    /// The other side of a connection, for a receiver made for tests.
    #[cfg(test)]
    pub(crate) fn accepted(stream: TcpStream) -> Conn {
        Conn { stream, sealed: None, raw: Vec::new(), text: Vec::new() }
    }

    /// Tests: the next request as (first line, body).
    #[cfg(test)]
    pub(crate) async fn next_request(&mut self) -> anyhow::Result<(String, Vec<u8>)> {
        let (first, _, body) = self.message().await?;
        Ok((first, body))
    }

    /// The next message the other side sends on its own (the event channel: there the device
    /// asks and we answer): its first line, headers and body. Waits as long as it takes.
    pub async fn incoming(&mut self) -> anyhow::Result<(String, Vec<(String, String)>, Vec<u8>)> {
        self.message().await
    }

    /// Text sent as it is (an answer on the event channel).
    pub async fn send_text(&mut self, text: &[u8]) -> anyhow::Result<()> {
        self.send(text).await
    }

    /// Tests: an answer to it.
    #[cfg(test)]
    pub(crate) async fn respond(&mut self, status: u16, body: &[u8]) -> anyhow::Result<()> {
        let mut text = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        text.extend_from_slice(body);
        self.send(&text).await
    }

    /// One message: its first line, headers and body.
    async fn message(&mut self) -> anyhow::Result<(String, Vec<(String, String)>, Vec<u8>)> {
        let head_end = loop {
            if let Some(i) = self.text.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            if self.text.len() > MAX_HEAD {
                anyhow::bail!("the device's answer has no end");
            }
            self.more().await?;
        };
        let head = String::from_utf8_lossy(self.text.get(..head_end).unwrap_or_default()).into_owned();
        let mut lines = head.split("\r\n");
        let first = lines.next().unwrap_or_default().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let length: usize = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        if length > MAX_BODY {
            anyhow::bail!("the device's answer is too large");
        }
        let start = head_end + 4;
        while self.text.len() < start + length {
            self.more().await?;
        }
        let body = self.text.get(start..start + length).unwrap_or_default().to_vec();
        self.text.drain(..start + length);
        Ok((first, headers, body))
    }
}
