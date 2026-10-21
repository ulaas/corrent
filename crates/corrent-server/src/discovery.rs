//! LAN discovery over mDNS/DNS-SD: servers advertise `_corrent._tcp` with their id, name,
//! version and certificate fingerprint; the desktop app lists them in its Connect dialog.
//! The fingerprint in an advertisement is only a hint (anyone on the network can advertise):
//! the app still shows it for comparison unless it came from a pairing link.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

pub const SERVICE: &str = "_corrent._tcp.local.";

/// What a server advertises.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Advert {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Hex SHA-256 of the CA's public key (empty: no own CA).
    pub fingerprint: String,
    pub https: bool,
    pub port: u16,
}

/// A server found on the network.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub advert: Advert,
    pub addrs: Vec<IpAddr>,
}

impl Found {
    /// The address to connect to: IPv4 first (link-local IPv6 needs a scope the URL can't hold).
    pub fn url(&self) -> Option<String> {
        let ip = self
            .addrs
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| self.addrs.iter().find(|a| matches!(a, IpAddr::V6(v) if !v.is_unicast_link_local())))?;
        let scheme = if self.advert.https { "https" } else { "http" };
        Some(match ip {
            IpAddr::V4(v) => format!("{scheme}://{v}:{}", self.advert.port),
            IpAddr::V6(v) => format!("{scheme}://[{v}]:{}", self.advert.port),
        })
    }
}

/// Keeps the advertisement up; withdrawn when dropped.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Instance names are limited to 63 bytes and must be unique on the network: the name plus the
/// start of the id.
fn instance_name(a: &Advert) -> String {
    let id: String = a.id.chars().take(6).collect();
    let mut name: String = a.name.chars().filter(|c| !c.is_control() && *c != '.').collect();
    while name.len() > 50 {
        name.pop();
    }
    format!("{name} ({id})")
}

pub fn advertise(a: &Advert) -> anyhow::Result<Advertiser> {
    let daemon = ServiceDaemon::new()?;
    let host = format!("{}.local.", crate::config_file::hostname().to_ascii_lowercase());
    let props = [
        ("id", a.id.as_str()),
        ("name", a.name.as_str()),
        ("ver", a.version.as_str()),
        ("fp", a.fingerprint.as_str()),
        ("https", if a.https { "1" } else { "0" }),
    ];
    let info = ServiceInfo::new(SERVICE, &instance_name(a), &host, "", a.port, &props[..])?.enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info)?;
    Ok(Advertiser { daemon, fullname })
}

/// A server from a resolved advertisement (None for other events and adverts without an id).
/// The name is cleaned for display; loopback and unspecified addresses are dropped.
fn found_from(ev: ServiceEvent) -> Option<Found> {
    let ServiceEvent::ServiceResolved(s) = ev else { return None };
    let p = s.get_properties();
    let get = |k: &str| p.get_property_val_str(k).unwrap_or_default().to_string();
    let found = Found {
        advert: Advert {
            id: get("id"),
            name: get("name").chars().filter(|c| !c.is_control()).take(40).collect(),
            version: get("ver"),
            fingerprint: get("fp"),
            https: get("https") == "1",
            port: s.get_port(),
        },
        addrs: s
            .get_addresses()
            .iter()
            .map(|a| a.to_ip_addr())
            .filter(|a| !a.is_loopback() && !a.is_unspecified())
            .collect(),
    };
    (!found.advert.id.is_empty()).then_some(found)
}

/// Adds `f` to `list`, merging the addresses of a server already in it (same id).
pub fn merge(list: &mut Vec<Found>, f: Found) {
    match list.iter_mut().find(|x| x.advert.id == f.advert.id) {
        Some(x) => {
            for a in f.addrs {
                if !x.addrs.contains(&a) {
                    x.addrs.push(a);
                }
            }
            x.advert = f.advert;
        }
        None => list.push(f),
    }
}

/// Lists servers that answer within `wait` (blocking: run it off the UI thread).
pub fn browse(wait: Duration) -> anyhow::Result<Vec<Found>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE)?;
    let deadline = Instant::now() + wait;
    let mut out: Vec<Found> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ev) => {
                if let Some(f) = found_from(ev) {
                    merge(&mut out, f);
                }
            }
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    out.sort_by(|a, b| a.advert.name.cmp(&b.advert.name));
    Ok(out)
}

/// A search that keeps running: servers are reported as they announce themselves (when they
/// start, and again when the daemon re-asks). Ends when dropped.
pub struct Browser {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
}

impl Browser {
    pub fn start() -> anyhow::Result<Browser> {
        let daemon = ServiceDaemon::new()?;
        // Network interfaces are listed every 5 s by default; a minute is plenty here.
        let _ = daemon.set_ip_check_interval(60);
        let events = daemon.browse(SERVICE)?;
        Ok(Browser { daemon, events })
    }

    /// Ask the network again now (after a sleep: the daemon's own re-asking backs off to an hour).
    pub fn requery(&mut self) -> anyhow::Result<()> {
        let _ = self.daemon.stop_browse(SERVICE);
        self.events = self.daemon.browse(SERVICE)?;
        Ok(())
    }

    /// The servers resolved since the last call (doesn't block).
    pub fn poll(&self) -> Vec<Found> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            if let Some(f) = found_from(ev) {
                merge(&mut out, f);
            }
        }
        out
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE);
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_names() {
        let mut f = Found {
            advert: Advert {
                id: "abcdef123".into(),
                name: "Home".into(),
                https: true,
                port: 8443,
                ..Default::default()
            },
            addrs: vec!["fe80::1".parse().unwrap(), "192.168.1.5".parse().unwrap()],
        };
        assert_eq!(f.url().as_deref(), Some("https://192.168.1.5:8443"));
        f.addrs = vec!["fe80::1".parse().unwrap()];
        assert_eq!(f.url(), None, "link-local only");
        f.addrs = vec!["fd00::5".parse().unwrap()];
        assert_eq!(f.url().as_deref(), Some("https://[fd00::5]:8443"));
        assert_eq!(instance_name(&f.advert), "Home (abcdef)");
    }

    #[test]
    fn merging_keeps_one_entry_per_server() {
        let found = |id: &str, ip: &str| Found {
            advert: Advert { id: id.into(), name: id.into(), ..Default::default() },
            addrs: vec![ip.parse().unwrap()],
        };
        let mut list = Vec::new();
        merge(&mut list, found("a", "192.168.1.5"));
        merge(&mut list, found("a", "10.0.0.5"));
        merge(&mut list, found("a", "192.168.1.5"));
        merge(&mut list, found("b", "192.168.1.6"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].addrs.len(), 2);
    }

    /// Advertise and find ourselves (needs a multicast-capable interface; skipped when the
    /// machine has none).
    #[test]
    #[ignore = "needs multicast on the local network"]
    fn finds_its_own_advertisement() {
        let a = Advert {
            id: format!("test{}", std::process::id()),
            name: "Discovery test".into(),
            version: "0".into(),
            fingerprint: "00ff".into(),
            https: true,
            port: 18_443,
        };
        let _ad = advertise(&a).unwrap();
        let found = browse(Duration::from_secs(4)).unwrap();
        let me = found.iter().find(|f| f.advert.id == a.id).expect("found");
        assert_eq!(me.advert, a);
    }
}
