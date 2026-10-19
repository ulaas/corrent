//! Pairing with a server: trusting its certificate (pinned by fingerprint) and getting a device
//! token with a pairing code or a user name and password.

use corrent_proto::auth::{DeviceInfo, LoginRequest, LoginResponse, PairRequest};
use corrent_proto::server::ServerInfo;

use super::ServerEntry;
use crate::app::media::client::MediaClient;

/// What a pairing link (`corrent://pair?host=…&id=…&code=…&fp=…`) holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PairLink {
    pub host: String,
    pub id: String,
    pub code: String,
    pub fingerprint: String,
}

pub fn parse_link(s: &str) -> Option<PairLink> {
    let u = url::Url::parse(s.trim()).ok()?;
    if u.scheme() != "corrent" || u.host_str() != Some("pair") {
        return None;
    }
    let mut l = PairLink::default();
    for (k, v) in u.query_pairs() {
        match &*k {
            "host" => l.host = v.into_owned(),
            "id" => l.id = v.into_owned(),
            "code" => l.code = v.into_owned(),
            "fp" => l.fingerprint = v.into_owned(),
            _ => {}
        }
    }
    (!l.host.is_empty()).then_some(l)
}

/// `host`, `host:port` or a URL → the server's base URL (HTTPS on 8443 unless given).
pub fn base_url(address: &str) -> Option<String> {
    let a = address.trim().trim_end_matches('/');
    if a.is_empty() {
        return None;
    }
    let with_scheme = if a.contains("://") { a.to_string() } else { format!("https://{a}") };
    let mut u = url::Url::parse(&with_scheme).ok()?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return None;
    }
    if u.port().is_none() && !a.contains("://") {
        u.set_port(Some(8443)).ok()?;
    }
    Some(u.origin().ascii_serialization())
}

/// Normalizes a fingerprint as typed or shown (`AB:CD:…`, any case) to lower-case hex.
pub fn normalize_fingerprint(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_lowercase()).collect()
}

/// A fingerprint as servers show it: `AB:CD:…`, the first 16 bytes.
pub fn short(hex: &str) -> String {
    hex.as_bytes().chunks(2).take(16).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join(":")
}

/// Hex SHA-256 of the public key of the first certificate in `pem`.
pub fn ca_fingerprint(pem: &str) -> Result<String, String> {
    let (_, p) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).map_err(|e| e.to_string())?;
    let cert = p.parse_x509().map_err(|e| e.to_string())?;
    let spki = cert.tbs_certificate.subject_pki.raw;
    Ok(aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, spki)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A pinned fingerprint matches when it is the whole hash or at least its first 16 bytes (the
/// short form servers display).
pub fn fingerprint_matches(expected: &str, actual: &str) -> bool {
    let e = normalize_fingerprint(expected);
    e.len() >= 32 && actual.starts_with(&e)
}

/// A server we are about to trust.
#[derive(Clone, Debug)]
pub struct Trusted {
    pub url: String,
    pub info: ServerInfo,
    /// Empty for a publicly trusted certificate (or plain HTTP).
    pub ca_pem: String,
    pub fingerprint: String,
}

/// Fetches the server's CA, checks it against `expected` (when given) and reads the server's
/// info through it. Without `expected` the caller shows the fingerprint for the person to
/// compare with the one the server displays.
pub async fn trust(url: &str, expected: Option<&str>) -> Result<Trusted, String> {
    if url.starts_with("http://") {
        let info = MediaClient::new(url, "", None)?.get::<ServerInfo>("/api/v1/server/info").await?;
        return Ok(Trusted { url: url.to_string(), info, ca_pem: String::new(), fingerprint: String::new() });
    }
    // The CA is fetched without verification: it is trusted only after its fingerprint checks.
    let insecure = reqwest::Client::builder()
        .tls_danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let res = insecure.get(format!("{url}/api/v1/server/ca")).send().await.map_err(|e| e.to_string())?;
    let (ca_pem, fingerprint) = if res.status().is_success() {
        let pem = res.text().await.map_err(|e| e.to_string())?;
        let fp = ca_fingerprint(&pem)?;
        if let Some(e) = expected.filter(|e| !e.trim().is_empty())
            && !fingerprint_matches(e, &fp)
        {
            return Err(t!(
                "The server's certificate doesn't match the fingerprint. Don't connect unless you know why it changed."
            )
            .to_string());
        }
        (pem, fp)
    } else {
        // A certificate from a public authority: ordinary verification.
        (String::new(), String::new())
    };
    let client = MediaClient::new(url, "", (!ca_pem.is_empty()).then_some(ca_pem.as_str()))?;
    let info = client.get::<ServerInfo>("/api/v1/server/info").await?;
    if !fingerprint.is_empty() && info.tls_fingerprint.as_deref().is_some_and(|f| f != fingerprint) {
        return Err(t!(
            "The server's certificate doesn't match the fingerprint. Don't connect unless you know why it changed."
        )
        .to_string());
    }
    Ok(Trusted { url: url.to_string(), info, ca_pem, fingerprint })
}

pub fn this_device() -> DeviceInfo {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Corrent".into());
    DeviceInfo {
        name: host,
        platform: std::env::consts::OS.into(),
        client: format!("corrent {}", env!("CARGO_PKG_VERSION")),
    }
}

/// How to sign in.
#[derive(Clone, Debug)]
pub enum Credentials {
    Code(String),
    Password { user: String, password: String },
}

/// Gets a device token for this app.
pub async fn pair(t: &Trusted, creds: Credentials) -> Result<ServerEntry, String> {
    let client = MediaClient::new(&t.url, "", (!t.ca_pem.is_empty()).then_some(t.ca_pem.as_str()))?;
    let res: LoginResponse = match creds {
        Credentials::Code(code) => {
            let req = PairRequest { code: code.trim().to_string(), device: this_device() };
            client.send(reqwest::Method::POST, "/api/v1/auth/pair", &req).await?
        }
        Credentials::Password { user, password } => {
            let req = LoginRequest { username: user.trim().to_string(), password, device: Some(this_device()) };
            client.send(reqwest::Method::POST, "/api/v1/auth/login", &req).await?
        }
    };
    let token = res.token.ok_or_else(|| t!("The server didn't issue a device token.").to_string())?;
    Ok(ServerEntry {
        id: t.info.id.clone(),
        name: t.info.name.clone(),
        url: t.url.clone(),
        user: res.user.name,
        token,
        fingerprint: t.fingerprint.clone(),
        ca_pem: t.ca_pem.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_addresses() {
        let l = parse_link("corrent://pair?host=192.168.1.5%3A8443&id=abc&code=123-456&fp=00ff").unwrap();
        assert_eq!((l.host.as_str(), l.code.as_str(), l.fingerprint.as_str()), ("192.168.1.5:8443", "123-456", "00ff"));
        assert!(parse_link("https://x/pair").is_none());
        assert_eq!(base_url("nas.local").as_deref(), Some("https://nas.local:8443"));
        assert_eq!(base_url("10.0.0.2:9000/").as_deref(), Some("https://10.0.0.2:9000"));
        assert_eq!(base_url("http://127.0.0.1:8080").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(base_url("https://media.example.org").as_deref(), Some("https://media.example.org"));
        assert_eq!(base_url("[fd00::1]:8443").as_deref(), Some("https://[fd00::1]:8443"));
        assert!(base_url("").is_none());
        assert!(fingerprint_matches(
            "AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89",
            "abcdef0123456789abcdef0123456789ffff"
        ));
        assert!(!fingerprint_matches("AB:CD", "abcd"), "too short to pin");
    }

    /// Pairing over HTTPS with the server's own CA: the pin is checked, a wrong pin refused, and
    /// the realtime channel then runs over TLS trusting only that CA.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pairs_over_https_with_a_pinned_ca() {
        use corrent_core::engine::testkit;
        use corrent_server::identity::{HashCost, Identity, new_admin};
        let dir = testkit::temp_dir("pair-https");
        let leecher = testkit::start(&dir);
        let db = corrent_db::Db::open(&dir.join("corrent.db")).unwrap();
        let identity = Identity::new(db.clone(), HashCost::FAST);
        let tls = corrent_server::tls::ensure(&dir.join("tls"), "test", &[], &["127.0.0.1".parse().unwrap()]).unwrap();
        let state = corrent_server::build_state(corrent_server::ServerParts {
            engine: leecher.engine.clone(),
            settings: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(testkit::settings(&dir))),
            paths: leecher.paths.clone(),
            identity,
            cfg: corrent_server::ServerConfig { bind: "127.0.0.1".into(), tls: true, ..Default::default() },
            hub: Default::default(),
            tls_fingerprint: Some(tls.fingerprint.clone()),
            ca_pem: Some(tls.ca_pem.clone()),
            library: corrent_library::Library::new(
                db,
                &dir,
                corrent_library::probe::Prober::new(corrent_library::probe::Prober::find_worker(), 1),
            ),
        })
        .await
        .unwrap();
        let listener =
            corrent_server::serve_https(corrent_server::router(state.clone()), "127.0.0.1:0".parse().unwrap(), &tls)
                .await
                .unwrap();
        let url = format!("https://127.0.0.1:{}", listener.addr.port());
        let admin = state.identity.create_user(new_admin("admin", "correct horse battery")).await.unwrap();
        let (code, _) = state.identity.pairing_code(admin.id).await.unwrap();

        assert_eq!(ca_fingerprint(&tls.ca_pem).unwrap(), tls.fingerprint);
        let wrong = "00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF";
        assert!(trust(&url, Some(wrong)).await.is_err(), "a wrong pin is refused");
        let t = trust(&url, Some(&short(&tls.fingerprint))).await.unwrap();
        assert_eq!(t.fingerprint, tls.fingerprint);
        assert_eq!(t.info.id, state.server_id);
        assert!(pair(&t, Credentials::Code("000-000".into())).await.is_err());
        let entry = pair(&t, Credentials::Code(code)).await.unwrap();
        assert_eq!(entry.user, "admin");
        assert!(!entry.token.is_empty() && !entry.ca_pem.is_empty());

        let (backend, status) = crate::link::remote::backend(entry);
        let (tx, _events) = std::sync::mpsc::channel();
        let client = corrent_core::engine::EngineHandle::with_backend(
            &tokio::runtime::Handle::current(),
            corrent_core::config::Paths { data_dir: dir.join("client"), portable: true },
            corrent_core::engine::Wake::noop(),
            tx,
            backend,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !status.connected.load(std::sync::atomic::Ordering::Relaxed) {
            assert!(std::time::Instant::now() < deadline, "{:?}", status.problem.load_full());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let c2 = client.clone();
        tokio::task::spawn_blocking(move || c2.shutdown_blocking(std::time::Duration::from_secs(5))).await.unwrap();
        drop(listener);
        testkit::stop(leecher).await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
