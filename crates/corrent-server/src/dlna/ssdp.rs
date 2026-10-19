//! SSDP: answering "who is a media server?" (M-SEARCH) and announcing ourselves (NOTIFY) on
//! 239.255.255.250:1900 and its IPv6 site-local twin, on every interface.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use librqbit_dualstack_sockets::{MulticastOpts, MulticastUdpSocket};

const PORT: u16 = 1900;
const V4: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), PORT);
const V6_SITE: SocketAddrV6 = SocketAddrV6::new(Ipv6Addr::new(0xff05, 0, 0, 0, 0, 0, 0, 0xc), PORT, 0, 0);
const SERVER: &str = concat!("Corrent/", env!("CARGO_PKG_VERSION"), " UPnP/1.0 DLNADOC/1.50");

/// The search targets we answer, besides our own UDN.
const TARGETS: [&str; 4] = [
    "upnp:rootdevice",
    super::DEVICE,
    "urn:schemas-upnp-org:service:ContentDirectory:1",
    "urn:schemas-upnp-org:service:ConnectionManager:1",
];

pub struct Ssdp {
    udn: String,
    port: u16,
    socket: MulticastUdpSocket,
}

/// A search request's target, if it is a well-formed M-SEARCH.
pub fn search_target(msg: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(msg).ok()?;
    let mut lines = text.split("\r\n");
    if !lines.next()?.starts_with("M-SEARCH ") {
        return None;
    }
    let (mut man, mut st) = (false, None);
    for l in lines {
        let Some((k, v)) = l.split_once(':') else { continue };
        match k.trim().to_ascii_lowercase().as_str() {
            "man" => man = v.trim() == "\"ssdp:discover\"",
            "st" => st = Some(v.trim().to_string()),
            _ => {}
        }
    }
    st.filter(|_| man)
}

/// The targets to answer for a search (`ssdp:all` gets one answer per target).
pub fn answers(st: &str, udn: &str) -> Vec<String> {
    if st == "ssdp:all" {
        let mut v = vec![udn.to_string()];
        v.extend(TARGETS.iter().map(|t| t.to_string()));
        v
    } else if st == udn || TARGETS.contains(&st) {
        vec![st.to_string()]
    } else {
        Vec::new()
    }
}

fn usn(udn: &str, nt: &str) -> String {
    if nt == udn { udn.to_string() } else { format!("{udn}::{nt}") }
}

fn no_scope(a: SocketAddr) -> SocketAddr {
    match a {
        SocketAddr::V6(mut v) => {
            v.set_scope_id(0);
            SocketAddr::V6(v)
        }
        v4 => v4,
    }
}

fn location(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v) => format!("http://{v}:{port}/dlna/description.xml"),
        IpAddr::V6(v) => format!("http://[{v}]:{port}/dlna/description.xml"),
    }
}

impl Ssdp {
    pub async fn new(udn: String, port: u16) -> anyhow::Result<Ssdp> {
        let socket = MulticastUdpSocket::new((Ipv6Addr::UNSPECIFIED, PORT).into(), V4, V6_SITE, None, None).await?;
        Ok(Ssdp { udn, port, socket })
    }

    fn notify(&self, nt: &str, nts: &str, o: &MulticastOpts) -> String {
        format!(
            "NOTIFY * HTTP/1.1\r\nHOST: {}\r\nCACHE-CONTROL: max-age=1800\r\nLOCATION: {}\r\nNT: {nt}\r\nNTS: {nts}\r\nSERVER: {SERVER}\r\nUSN: {}\r\n\r\n",
            no_scope(o.mcast_addr()),
            location(o.iface_ip(), self.port),
            usn(&self.udn, nt),
        )
    }

    async fn announce(&self, nts: &str) {
        let mut nts_list = vec![self.udn.clone()];
        nts_list.extend(TARGETS.iter().map(|t| t.to_string()));
        for nt in &nts_list {
            self.socket.try_send_mcast_everywhere(&|o| Some(self.notify(nt, nts, o))).await;
        }
    }

    async fn reply(&self, msg: &[u8], from: SocketAddr) {
        let Some(st) = search_target(msg) else { return };
        // Link-local IPv6 locations can't carry the scope a client would need.
        if matches!(from.ip(), IpAddr::V6(a) if a.is_unicast_link_local()) {
            return;
        }
        let Ok(ip) = librqbit_upnp::get_local_ip_relative_to(from, self.socket.nics()) else { return };
        for t in answers(&st, &self.udn) {
            let r = format!(
                "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nEXT:\r\nLOCATION: {}\r\nSERVER: {SERVER}\r\nST: {t}\r\nUSN: {}\r\nContent-Length: 0\r\n\r\n",
                location(ip, self.port),
                usn(&self.udn, &t),
            );
            let _ = self.socket.send_to(r.as_bytes(), from).await;
        }
    }

    pub async fn run(&self) {
        self.announce("ssdp:alive").await;
        let answer = async {
            let mut buf = vec![0u8; 8192];
            loop {
                match self.socket.recv_from(&mut buf).await {
                    Ok((n, from)) => self.reply(&buf[..n], from).await,
                    Err(e) => {
                        tracing::debug!("SSDP: {e}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        };
        let alive = async {
            let mut every = tokio::time::interval(Duration::from_secs(600));
            every.tick().await;
            loop {
                every.tick().await;
                self.announce("ssdp:alive").await;
            }
        };
        tokio::join!(answer, alive);
    }
}
