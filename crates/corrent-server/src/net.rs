//! Who is calling: client addresses behind trusted proxies, and the LAN-only guard.

use std::net::IpAddr;
use std::str::FromStr;

/// An IPv4 or IPv6 network in CIDR form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    prefix: u8,
}

impl FromStr for Cidr {
    type Err = String;
    fn from_str(s: &str) -> Result<Cidr, String> {
        let (ip, prefix) = match s.trim().split_once('/') {
            Some((ip, p)) => (ip, Some(p)),
            None => (s.trim(), None),
        };
        let net: IpAddr = ip.parse().map_err(|_| format!("not an address: {ip}"))?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| format!("bad prefix: {p}"))?,
            None => max,
        };
        Ok(Cidr { net: normalize(net), prefix })
    }
}

impl Cidr {
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.net, normalize(ip)) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix as u32) };
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = if self.prefix == 0 { 0 } else { u128::MAX << (128 - self.prefix as u32) };
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

/// `::ffff:a.b.c.d` → `a.b.c.d`, so IPv4 rules apply to dual-stack sockets.
pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

pub fn parse_cidrs(list: &[String]) -> Vec<Cidr> {
    list.iter().filter_map(|s| s.parse().map_err(|e| tracing::warn!("ignoring network {s:?}: {e}")).ok()).collect()
}

/// Loopback, private (RFC 1918 / ULA), link-local and CGNAT (100.64.0.0/10, where Tailscale
/// addresses live), plus `extra`.
pub fn is_local(ip: IpAddr, extra: &[Cidr]) -> bool {
    let ip = normalize(ip);
    let builtin = match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments()[0];
            v6.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    };
    builtin || extra.iter().any(|c| c.contains(ip))
}

/// The client's address: the socket peer, or — only when the peer is a trusted proxy — the
/// right-most address in `X-Forwarded-For` that isn't a trusted proxy itself.
pub fn client_ip(peer: IpAddr, forwarded_for: Option<&str>, trusted: &[Cidr]) -> IpAddr {
    let peer = normalize(peer);
    if !trusted.iter().any(|c| c.contains(peer)) {
        return peer;
    }
    let Some(xff) = forwarded_for else { return peer };
    for hop in xff.rsplit(',') {
        match hop.trim().parse::<IpAddr>() {
            Ok(ip) if trusted.iter().any(|c| c.contains(ip)) => continue,
            Ok(ip) => return normalize(ip),
            Err(_) => return peer,
        }
    }
    peer
}

/// Whether an address is on the public internet (not loopback, private, link-local, CGNAT,
/// multicast, unspecified or documentation space).
pub fn is_public(ip: IpAddr) -> bool {
    let ip = normalize(ip);
    if is_local(ip, &[]) || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => !(v4.is_broadcast() || v4.is_documentation() || v4.octets()[0] == 0),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xe000) == 0x2000,
    }
}

/// Resolves the host of an http(s) URL and refuses it unless every address is public (users
/// without admin rights must not make the server fetch from its own network).
pub async fn check_public_url(url: &str) -> Result<(), String> {
    let u = url::Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err("only http(s) URLs".into());
    }
    let host = u.host_str().ok_or("URL without host")?.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = u.port_or_known_default().unwrap_or(80);
    let addrs: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => {
            tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| e.to_string())?.map(|a| a.ip()).collect()
        }
    };
    if addrs.is_empty() || !addrs.iter().all(|a| is_public(*a)) {
        return Err("that address is on the server's own network".into());
    }
    Ok(())
}

/// Host header check against DNS rebinding: IP literals, `localhost`, and known names.
pub fn host_allowed(host_header: &str, names: &[String]) -> bool {
    let host = if let Some(rest) = host_header.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host_header.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_header)
    };
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.parse::<IpAddr>().is_ok() || names.iter().any(|n| n.eq_ignore_ascii_case(&h))
}

/// A GET to the public internet only: the host must resolve to public addresses (not this
/// server, its network, loopback or link-local), the connection goes to the address checked,
/// and every redirect is checked again. For URLs people type (radio stations, podcast feeds),
/// so they can't reach what only the server can.
pub async fn get_public(
    url: &str,
    read_timeout: std::time::Duration,
    total: Option<std::time::Duration>,
) -> Result<reqwest::Response, String> {
    let mut u = url::Url::parse(url).map_err(|e| e.to_string())?;
    for _ in 0..6 {
        if !matches!(u.scheme(), "http" | "https") {
            return Err("only http and https addresses".into());
        }
        let host = u.host_str().ok_or("no host")?.trim_start_matches('[').trim_end_matches(']').to_string();
        let port = u.port_or_known_default().ok_or("no port")?;
        let addrs: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| format!("{host}: {e}"))?.collect();
        let Some(addr) = addrs.first().copied() else { return Err(format!("{host} has no address")) };
        // Tests serve their feeds on loopback.
        let allowed = |ip: std::net::IpAddr| corrent_plugins::is_public(ip) || (cfg!(test) && ip.is_loopback());
        if addrs.iter().any(|a| !allowed(a.ip())) {
            return Err(format!("{host} is on a private network"));
        }
        let mut b = reqwest::Client::builder()
            .user_agent(concat!("Corrent/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(read_timeout);
        if u.host().is_some_and(|h| matches!(h, url::Host::Domain(_))) {
            b = b.resolve(&host, addr);
        }
        let client = b.build().map_err(|e| e.to_string())?;
        let mut req = client.get(u.clone());
        if let Some(t) = total {
            req = req.timeout(t);
        }
        let res = req.send().await.map_err(|e| e.to_string())?;
        if res.status().is_redirection() {
            let loc = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("redirect without a location")?;
            u = u.join(loc).map_err(|e| e.to_string())?;
            continue;
        }
        if !res.status().is_success() {
            return Err(res.status().to_string());
        }
        return Ok(res);
    }
    Err("too many redirects".into())
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn public_fetches_refuse_private_networks() {
        let t = std::time::Duration::from_secs(2);
        for u in [
            "http://10.1.2.3/x",
            "http://169.254.169.254/latest/meta-data",
            "http://192.168.1.1:8096/",
            "http://[fd00::1]/",
            "ftp://example.org/",
        ] {
            let e = get_public(u, t, Some(t)).await.unwrap_err();
            assert!(e.contains("private") || e.contains("only http"), "{u}: {e}");
        }
    }

    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn local_networks() {
        for a in [
            "127.0.0.1",
            "10.1.2.3",
            "172.20.0.1",
            "192.168.1.9",
            "169.254.3.3",
            "100.101.102.103",
            "::1",
            "fd00::5",
            "fe80::1",
            "::ffff:192.168.0.2",
        ] {
            assert!(is_local(ip(a), &[]), "{a}");
        }
        for a in ["8.8.8.8", "100.128.0.1", "2001:db8::1", "::ffff:8.8.8.8", "172.32.0.1"] {
            assert!(!is_local(ip(a), &[]), "{a}");
        }
        let extra = parse_cidrs(&["203.0.113.0/24".into(), "garbage".into()]);
        assert!(is_local(ip("203.0.113.77"), &extra));
        assert!(!is_local(ip("203.0.114.1"), &extra));
    }

    #[test]
    fn forwarded_for_only_from_trusted_proxies() {
        let trusted = parse_cidrs(&["10.0.0.2".into()]);
        assert_eq!(client_ip(ip("8.8.8.8"), Some("1.2.3.4"), &trusted), ip("8.8.8.8"), "spoofed header ignored");
        assert_eq!(client_ip(ip("10.0.0.2"), Some("1.2.3.4, 10.0.0.2"), &trusted), ip("1.2.3.4"));
        assert_eq!(client_ip(ip("10.0.0.2"), None, &trusted), ip("10.0.0.2"));
    }

    #[test]
    fn public_addresses() {
        assert!(is_public(ip("8.8.8.8")));
        assert!(is_public(ip("2606:4700::1111")));
        for a in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "::ffff:192.168.1.1",
            "224.0.0.1",
        ] {
            assert!(!is_public(ip(a)), "{a}");
        }
    }

    #[tokio::test]
    async fn urls_to_the_own_network_are_refused() {
        assert!(check_public_url("http://127.0.0.1:8080/x.torrent").await.is_err());
        assert!(check_public_url("http://169.254.169.254/latest").await.is_err());
        assert!(check_public_url("http://localhost/x").await.is_err());
        assert!(check_public_url("ftp://8.8.8.8/x").await.is_err());
        assert!(check_public_url("https://8.8.8.8/x").await.is_ok());
    }

    #[test]
    fn hosts() {
        let names = vec!["media.example".to_string()];
        assert!(host_allowed("localhost:8443", &names));
        assert!(host_allowed("192.168.1.4:8443", &names));
        assert!(host_allowed("[::1]:8443", &names));
        assert!(host_allowed("Media.Example", &names));
        assert!(!host_allowed("evil.example:8443", &names));
    }
}
