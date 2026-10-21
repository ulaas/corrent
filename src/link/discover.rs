//! Corrent servers announcing themselves on this network, as the app sees them: a server it
//! isn't paired with is only offered (pairing stays the trust step: anyone on the network can
//! announce), and a paired one is believed only after its address answers with its pinned
//! certificate and id. Announcements are hints; nothing here sends a token.

use std::net::IpAddr;
use std::time::Duration;

use corrent_server::discovery::Found;

use super::pair::{fingerprint_matches, valid_id};
use super::{ServerEntry, Servers};

/// What an announcement is to this app.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not paired: may be offered.
    New,
    /// The paired server with this key (same id, and a fingerprint that doesn't contradict it).
    Paired(String),
    /// Unusable (no id, no address) or contradicting a paired server's fingerprint.
    Skip,
}

pub fn classify(f: &Found, servers: &Servers) -> Verdict {
    if !valid_id(&f.advert.id) || f.url().is_none() {
        return Verdict::Skip;
    }
    let same_id: Vec<&ServerEntry> = servers.list.iter().filter(|e| e.id == f.advert.id).collect();
    if same_id.is_empty() {
        return Verdict::New;
    }
    // Someone announcing a paired server's id with another fingerprint isn't that server (the
    // address check would refuse it anyway).
    let agrees = |e: &&&ServerEntry| {
        e.fingerprint.is_empty()
            || f.advert.fingerprint.is_empty()
            || (fingerprint_matches(&e.fingerprint, &f.advert.fingerprint)
                || fingerprint_matches(&f.advert.fingerprint, &e.fingerprint))
    };
    let pick = same_id
        .iter()
        .filter(agrees)
        .find(|e| {
            servers.active.as_deref() == Some(e.key().as_str()) || servers.fallback.as_deref() == Some(e.key().as_str())
        })
        .or_else(|| same_id.iter().find(agrees));
    match pick {
        Some(e) => Verdict::Paired(e.key()),
        None => Verdict::Skip,
    }
}

/// The host of an `https://host:port` address.
fn host_of(url: &str) -> Option<IpAddr> {
    let u = url::Url::parse(url).ok()?;
    match u.host()? {
        url::Host::Ipv4(v) => Some(IpAddr::V4(v)),
        url::Host::Ipv6(v) => Some(IpAddr::V6(v)),
        url::Host::Domain(_) => None,
    }
}

/// Where a paired server may be now, most likely first: its saved address when the server
/// still announces that address, else each announced address on the saved port scheme.
/// Always `https` (taken from the saved address, never from the announcement: a plain-HTTP
/// address would be checked without its certificate). Empty for servers saved without https.
pub fn candidates(entry: &ServerEntry, f: &Found) -> Vec<String> {
    if !entry.url.starts_with("https://") {
        return Vec::new();
    }
    if host_of(&entry.url).is_some_and(|h| f.addrs.contains(&h)) {
        return vec![entry.url.clone()];
    }
    let mut addrs: Vec<IpAddr> = f
        .addrs
        .iter()
        .copied()
        .filter(|a| !a.is_loopback() && !a.is_unspecified() && !matches!(a, IpAddr::V6(v) if v.is_unicast_link_local()))
        .collect();
    // Private IPv4 first (home networks), then other IPv4, then IPv6.
    addrs.sort_by_key(|a| match a {
        IpAddr::V4(v) if v.is_private() => 0,
        IpAddr::V4(_) => 1,
        IpAddr::V6(_) => 2,
    });
    addrs
        .into_iter()
        .map(|a| match a {
            IpAddr::V4(v) => format!("https://{v}:{}", f.advert.port),
            IpAddr::V6(v) => format!("https://[{v}]:{}", f.advert.port),
        })
        .collect()
}

/// The first of `urls` where `entry`'s server answers: its pinned certificate and its id,
/// asked without the device token (the server's info needs none).
pub async fn probe(entry: &ServerEntry, urls: &[String]) -> Option<String> {
    for url in urls {
        let Ok(c) = crate::app::media::client::MediaClient::new(url, "", entry.ca()) else { continue };
        let info = tokio::time::timeout(
            Duration::from_secs(5),
            c.get::<corrent_proto::server::ServerInfo>("/api/v1/server/info"),
        )
        .await;
        match info {
            Ok(Ok(i)) if i.id == entry.id => return Some(url.clone()),
            Ok(Ok(_)) => tracing::debug!("{url}: another server answers"),
            Ok(Err(e)) => tracing::debug!("{url}: {e}"),
            Err(_) => tracing::debug!("{url}: no answer"),
        }
    }
    None
}

/// Whether the app should be listening: the setting is on, the welcome screens are done, and
/// the first frame was a few seconds ago (the first network use may ask for a permission).
pub fn wanted(find_servers: bool, onboarding: bool, since_first_frame: Duration) -> bool {
    find_servers && !onboarding && since_first_frame >= Duration::from_secs(5)
}

#[cfg(test)]
mod tests {
    use corrent_server::discovery::Advert;

    use super::*;

    const ID: &str = "ab12cd34ef56";
    const FP: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn found(id: &str, fp: &str, addrs: &[&str]) -> Found {
        Found {
            advert: Advert {
                id: id.into(),
                name: "Home".into(),
                fingerprint: fp.into(),
                https: true,
                port: 8443,
                ..Default::default()
            },
            addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    fn paired(url: &str) -> ServerEntry {
        ServerEntry {
            id: ID.into(),
            name: "Home".into(),
            url: url.into(),
            fingerprint: FP.into(),
            token: "secret".into(),
            ..Default::default()
        }
    }

    #[test]
    fn announcements_are_classified() {
        let mut s = Servers::default();
        assert_eq!(classify(&found(ID, FP, &["192.168.1.5"]), &s), Verdict::New);
        assert_eq!(classify(&found("x", FP, &["192.168.1.5"]), &s), Verdict::Skip, "not a server id");
        assert_eq!(classify(&found(ID, FP, &["fe80::1"]), &s), Verdict::Skip, "no usable address");
        s.upsert(paired("https://192.168.1.5:8443"));
        let key = s.list[0].key();
        assert_eq!(classify(&found(ID, FP, &["192.168.1.9"]), &s), Verdict::Paired(key.clone()));
        assert_eq!(classify(&found(ID, &FP[..32], &["192.168.1.9"]), &s), Verdict::Paired(key.clone()), "short form");
        assert_eq!(classify(&found(ID, "", &["192.168.1.9"]), &s), Verdict::Paired(key), "no fingerprint announced");
        assert_eq!(classify(&found(ID, &"f".repeat(64), &["192.168.1.9"]), &s), Verdict::Skip, "another fingerprint");
    }

    #[test]
    fn candidate_addresses() {
        let e = paired("https://192.168.1.5:8443");
        assert_eq!(candidates(&e, &found(ID, FP, &["10.8.0.2", "192.168.1.5"])), vec!["https://192.168.1.5:8443"]);
        assert_eq!(
            candidates(&e, &found(ID, FP, &["fd00::7", "203.0.113.4", "192.168.1.20", "127.0.0.1", "fe80::1"])),
            vec!["https://192.168.1.20:8443", "https://203.0.113.4:8443", "https://[fd00::7]:8443"]
        );
        // The announcement can't ask for plain HTTP, and a server saved without https isn't probed.
        let mut plain = found(ID, FP, &["192.168.1.20"]);
        plain.advert.https = false;
        assert!(candidates(&e, &plain).iter().all(|u| u.starts_with("https://")));
        assert!(candidates(&paired("http://192.168.1.5:8080"), &plain).is_empty());
    }

    #[test]
    fn listening_waits_for_the_app() {
        let s5 = Duration::from_secs(5);
        assert!(wanted(true, false, s5));
        assert!(!wanted(false, false, s5));
        assert!(!wanted(true, true, s5));
        assert!(!wanted(true, false, Duration::from_secs(2)));
    }

    /// A probe never sends the token, and accepts only the pinned certificate with the right id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probes_check_certificate_and_id() {
        use std::sync::{Arc, Mutex};
        let dir = std::env::temp_dir().join(format!("corrent-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tls = corrent_server::tls::ensure(&dir.join("tls"), "test", &[], &["127.0.0.1".parse().unwrap()]).unwrap();
        let auth: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = auth.clone();
        let app = axum::Router::new().route(
            "/api/v1/server/info",
            axum::routing::get(move |h: axum::http::HeaderMap| {
                let seen = seen.clone();
                async move {
                    if let Some(v) = h.get("authorization") {
                        seen.lock().unwrap().push(v.to_str().unwrap_or("").to_string());
                    }
                    axum::Json(serde_json::json!({ "id": ID, "name": "Home", "version": "0", "api_version": 1 }))
                }
            }),
        );
        let listener = corrent_server::serve_https(app, "127.0.0.1:0".parse().unwrap(), &tls).await.unwrap();
        let url = format!("https://127.0.0.1:{}", listener.addr.port());
        let entry = ServerEntry { ca_pem: tls.ca_pem.clone(), ..paired(&url) };
        assert_eq!(probe(&entry, std::slice::from_ref(&url)).await.as_deref(), Some(url.as_str()));
        assert!(auth.lock().unwrap().iter().all(|a| !a.contains("secret")), "the token stays home");
        let other = ServerEntry { id: "ffffffffffff".into(), ..entry.clone() };
        assert_eq!(probe(&other, std::slice::from_ref(&url)).await, None, "another id");
        let other_tls =
            corrent_server::tls::ensure(&dir.join("tls2"), "other", &[], &["127.0.0.1".parse().unwrap()]).unwrap();
        let wrong_ca = ServerEntry { ca_pem: other_tls.ca_pem.clone(), ..entry };
        assert_eq!(probe(&wrong_ca, &[url]).await, None, "another certificate");
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
