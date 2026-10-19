# Spike: Corrent Connect over iroh (MS12)

**Question.** Can the server's existing axum router be served over iroh streams, and can the
desktop app (and mpv, which only speaks TCP/HTTP) use it through a loopback bridge — directly on
a LAN and relayed through NATs?

**Setup.** `iroh` 1.3 (`default-features = false`, `tls-aws-lc-rs` + `portmapper`, so rustls keeps a
single crypto provider). ALPN `corrent/1`. The server accepts bidirectional streams and runs
`hyper::server::conn::http1` with upgrades on each (`tokio::io::join(recv, send)` →
`hyper_util::rt::TokioIo`), inserting `ConnectInfo` (a stable ULA address derived from the
client's key, `fd63:6f6e::/32`) and a `P2pPeer` extension. The client bridge listens on
`127.0.0.1:0`; every accepted TCP connection opens one stream on a shared QUIC connection
(re-dialled when it drops) and is copied both ways.

**Results** (Fedora, this machine; `crates/corrent-server/src/p2p.rs` tests):

| Case | Numbers |
|---|---|
| Direct (relays off, loopback addresses) | connected in 22 ms; 6 HTTP requests (6 streams) in 37 ms total |
| Relayed (n0 relays + DNS address lookup, dialled by key only) | server online after 0.69 s; first HTTP response through the bridge after 1.18 s |
| Remote backend falls back to Connect when the address is unreachable | WS + REST work through the bridge (`src/link/remote.rs` test) |

**Decision: go.** WebSocket upgrades, multipart uploads and Range requests are ordinary HTTP/1.1
on the stream, so nothing in the API changes. Cost: ~180 more crates to build (behind the `p2p`
feature, on by default for `correntd` and the app). With relays on, the server publishes its
addresses to n0's DNS (`iroh.link`) — only while Connect is turned on; the Remote access page
says so.

**Not done here:** a self-hosted relay (`iroh-relay` crate) and choosing relays in the settings.
