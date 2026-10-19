//! Certificates from Let's Encrypt for the public domains in `acme_domains` (feature `acme`).
//! The HTTPS port answers TLS-ALPN-01 challenges itself, so the internet's port 443 must reach
//! it (a router forward 443 → `https_port`). Only connections asking for one of those names get
//! the domain certificate; everything else — apps pinning the server's CA, LAN addresses,
//! Corrent Connect — keeps the server's own certificate, so turning this on breaks nothing.
//! The account key and certificate are cached in `<data>/tls/acme/` (owner-only).

use std::net::SocketAddr;
use std::path::Path;

use axum::Router;

use crate::config_file::ServerConfig;
use crate::{Listener, tls};

/// The domains to get a certificate for (trimmed, lowercase, plausible host names only).
pub fn domains(cfg: &ServerConfig) -> Vec<String> {
    let mut v: Vec<String> = cfg
        .acme_domains
        .iter()
        .map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| valid_domain(d))
        .collect();
    // Sorted: the certificate cache is keyed by the list, so reordering the config mustn't
    // order a new one.
    v.sort();
    v.dedup();
    v
}

/// A contact address the authority accepts (or none).
pub fn valid_email(e: &str) -> bool {
    e.is_empty() || (e.contains('@') && !e.contains(|c: char| c.is_whitespace() || c == ',') && e.len() <= 254)
}

/// A public DNS name: dotted labels of letters, digits and hyphens (no IP addresses, no
/// wildcards — TLS-ALPN-01 can't prove those).
pub fn valid_domain(d: &str) -> bool {
    d.len() <= 253
        && d.contains('.')
        && d.parse::<std::net::IpAddr>().is_err()
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// The address a pairing link should name for a request to `host`: apps pin the server's own
/// CA, which the ACME names don't present, so for those a LAN address of the server instead
/// (kept as is when there is none; the link's Connect id still works).
pub fn pairing_host(cfg: &ServerConfig, host: &str, ips: &[std::net::IpAddr]) -> String {
    let name = host
        .rsplit_once(':')
        .filter(|(h, p)| !h.contains(']') && p.bytes().all(|b| b.is_ascii_digit()))
        .map_or(host, |(h, _)| h);
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if !domains(cfg).contains(&name) {
        return host.to_string();
    }
    let lan = ips.iter().filter(|ip| !ip.is_loopback() && !ip.is_unspecified()).min_by_key(|ip| (!ip.is_ipv4(), **ip));
    match lan {
        Some(std::net::IpAddr::V6(v6)) => format!("[{v6}]:{}", cfg.https_port),
        Some(ip) => format!("{ip}:{}", cfg.https_port),
        None => host.to_string(),
    }
}

/// Serves `app` over HTTPS like [`crate::serve_https`], adding the domain certificate when
/// `acme_domains` is set (and the server was built with the `acme` feature).
pub async fn serve_https(
    app: Router,
    addr: SocketAddr,
    own: &tls::TlsMaterial,
    cfg: &ServerConfig,
    data_dir: &Path,
) -> anyhow::Result<Listener> {
    let domains = domains(cfg);
    if domains.is_empty() {
        if !cfg.acme_domains.is_empty() {
            tracing::warn!(
                "acme_domains has no usable domain name (public names only; international names in their punycode form, xn--…); using the server's own certificate"
            );
        }
        return crate::serve_https(app, addr, own).await;
    }
    if crate::remote::loopback_only(&cfg.bind) {
        tracing::warn!(
            "acme_domains is set, but bind = {:?} can't be reached from the internet; no certificate can be issued",
            cfg.bind
        );
    }
    if !valid_email(cfg.acme_email.trim()) {
        tracing::warn!("acme_email {:?} isn't a single e-mail address; ordering without a contact", cfg.acme_email);
    }
    imp::serve(app, addr, own, cfg, &domains, data_dir).await
}

#[cfg(not(feature = "acme"))]
mod imp {
    use super::*;

    pub async fn serve(
        app: Router,
        addr: SocketAddr,
        own: &tls::TlsMaterial,
        _cfg: &ServerConfig,
        _domains: &[String],
        _data_dir: &Path,
    ) -> anyhow::Result<Listener> {
        tracing::warn!("acme_domains is set, but this build has no ACME support; using the server's own certificate");
        crate::serve_https(app, addr, own).await
    }
}

#[cfg(feature = "acme")]
mod imp {
    use std::sync::Arc;

    use futures_util::StreamExt;
    use rustls::server::{ClientHello, ResolvesServerCert};
    use rustls::sign::CertifiedKey;
    use rustls_acme::{AcmeConfig, caches::DirCache};

    use super::*;

    const ACME_ALPN: &[u8] = b"acme-tls/1";

    /// The domain certificate for its names and challenges, the server's own for the rest.
    #[derive(Debug)]
    pub(super) struct Either {
        pub acme: Arc<dyn ResolvesServerCert>,
        pub domains: Vec<String>,
        pub own: Arc<CertifiedKey>,
    }

    impl ResolvesServerCert for Either {
        fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            // Exactly `acme-tls/1` (what the CA sends), as rustls-acme checks it.
            let challenge = rustls_acme::is_tls_alpn_challenge(&hello);
            let ours = hello.server_name().is_some_and(|n| self.domains.iter().any(|d| d.eq_ignore_ascii_case(n)));
            if challenge {
                // Only the CA asks for this protocol; it gets the challenge certificate or nothing.
                return if ours { self.acme.resolve(hello) } else { None };
            }
            if ours {
                // Until the first certificate arrives, the own one (browsers warn as before).
                return self.acme.resolve(hello).or_else(|| Some(self.own.clone()));
            }
            Some(self.own.clone())
        }
    }

    /// The listener's TLS settings: HTTP/2, HTTP/1.1 and the challenge protocol.
    pub(super) fn server_config(resolver: Either) -> anyhow::Result<rustls::ServerConfig> {
        let mut config =
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(resolver));
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), ACME_ALPN.to_vec()];
        Ok(config)
    }

    pub(super) fn certified_key(m: &tls::TlsMaterial) -> anyhow::Result<CertifiedKey> {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let chain = CertificateDer::pem_slice_iter(m.cert_chain_pem.as_bytes()).collect::<Result<Vec<_>, _>>()?;
        let key = PrivateKeyDer::from_pem_slice(m.key_pem.as_bytes())?;
        let signer = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)?;
        Ok(CertifiedKey::new(chain, signer))
    }

    pub async fn serve(
        app: Router,
        addr: SocketAddr,
        own: &tls::TlsMaterial,
        cfg: &ServerConfig,
        domains: &[String],
        data_dir: &Path,
    ) -> anyhow::Result<Listener> {
        let cache = data_dir.join("tls").join("acme");
        let mut dir = std::fs::DirBuilder::new();
        dir.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            dir.mode(0o700);
            dir.create(&cache)?;
            // An existing folder, too.
            std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        dir.create(&cache)?;
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut acme = AcmeConfig::new_with_provider(domains.iter(), provider)
            .cache(DirCache::new(cache))
            .directory_lets_encrypt(!cfg.acme_staging);
        let email = cfg.acme_email.trim();
        if !email.is_empty() && valid_email(email) {
            acme = acme.contact_push(format!("mailto:{email}"));
        }
        let mut state = acme.state();
        let resolver = Either { acme: state.resolver(), domains: domains.to_vec(), own: Arc::new(certified_key(own)?) };
        let rustls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(server_config(resolver)?));
        let mut listener = crate::serve_https_config(app, addr, rustls).await?;
        let names = domains.join(", ");
        // Orders and renews (a third of the lifetime before expiry). rustls-acme retries a failed
        // order within seconds, which would run into Let's Encrypt's limits (5 failed validations
        // per name and hour): the stream is only polled again after a pause.
        let task = tokio::spawn(async move {
            while let Some(event) = state.next().await {
                use rustls_acme::EventError;
                match event {
                    Ok(ok) => tracing::info!("certificate for {names}: {ok:?}"),
                    Err(EventError::Order(e)) => {
                        let text = e.to_string();
                        let pause = if text.contains("rateLimited") { 3600 } else { 900 };
                        tracing::warn!("certificate for {names}: {text}; trying again in {} minutes", pause / 60);
                        tokio::time::sleep(std::time::Duration::from_secs(pause)).await;
                    }
                    Err(e @ (EventError::CertCacheStore(_) | EventError::AccountCacheStore(_))) => {
                        // Not fatal now, but every restart would then order again.
                        tracing::error!("certificate for {names}: {e} (check that tls/acme is writable)");
                    }
                    Err(e) => tracing::warn!("certificate for {names}: {e}"),
                }
            }
        });
        let stop = listener.stop.take();
        listener.stop = Some(Box::new(move || {
            task.abort();
            if let Some(s) = stop {
                s();
            }
        }));
        Ok(listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_names() {
        assert!(valid_domain("media.example.org"));
        assert!(valid_domain("a-b.example"));
        for bad in ["localhost", "192.168.1.2", "*.example.org", "-a.example", "a..example", "a_b.example", ""] {
            assert!(!valid_domain(bad), "{bad}");
        }
        let cfg = ServerConfig {
            acme_domains: vec![" Media.Example.org. ".into(), "media.example.org".into(), "10.0.0.1".into()],
            ..Default::default()
        };
        assert_eq!(domains(&cfg), vec!["media.example.org"]);
    }

    #[test]
    fn pairing_links_avoid_the_acme_names() {
        let cfg = ServerConfig { acme_domains: vec!["media.example.org".into()], ..Default::default() };
        let ips: Vec<std::net::IpAddr> =
            vec!["127.0.0.1".parse().unwrap(), "fd00::5".parse().unwrap(), "192.168.1.20".parse().unwrap()];
        assert_eq!(pairing_host(&cfg, "media.example.org", &ips), "192.168.1.20:8443");
        assert_eq!(pairing_host(&cfg, "Media.Example.org:8443", &ips), "192.168.1.20:8443");
        assert_eq!(pairing_host(&cfg, "nas.local:8443", &ips), "nas.local:8443");
        assert_eq!(pairing_host(&cfg, "media.example.org", &[]), "media.example.org");
        assert!(valid_email("me@example.org") && valid_email(""));
        assert!(!valid_email("a@b.org, c@d.org") && !valid_email("nobody"));
    }

    /// Stands in for rustls-acme's resolver (its certificate can't be set from outside).
    #[cfg(feature = "acme")]
    #[derive(Debug)]
    struct Stub {
        cert: Option<std::sync::Arc<rustls::sign::CertifiedKey>>,
        challenge: std::sync::Arc<rustls::sign::CertifiedKey>,
    }

    #[cfg(feature = "acme")]
    impl rustls::server::ResolvesServerCert for Stub {
        fn resolve(
            &self,
            hello: rustls::server::ClientHello<'_>,
        ) -> Option<std::sync::Arc<rustls::sign::CertifiedKey>> {
            if rustls_acme::is_tls_alpn_challenge(&hello) { Some(self.challenge.clone()) } else { self.cert.clone() }
        }
    }

    #[cfg(feature = "acme")]
    #[test]
    fn domain_certificate_only_for_the_domain() {
        use std::sync::Arc;
        let base = std::env::temp_dir().join(format!("corrent-acme-{}", std::process::id()));
        let key = |n: &str| {
            Arc::new(imp::certified_key(&tls::ensure(&base.join(n), n, &["localhost".into()], &[]).unwrap()).unwrap())
        };
        let (own, domain, challenge) = (key("own"), key("domain"), key("challenge"));
        let _ = std::fs::remove_dir_all(&base);
        let server = |cert: Option<Arc<rustls::sign::CertifiedKey>>| {
            Arc::new(
                imp::server_config(imp::Either {
                    acme: Arc::new(Stub { cert, challenge: challenge.clone() }),
                    domains: vec!["media.example.org".into()],
                    own: own.clone(),
                })
                .unwrap(),
            )
        };
        // The leaf certificate a handshake for this name and these protocols gets (an in-memory
        // handshake: a ClientHello can't be built by hand).
        let shake = |server: &Arc<rustls::ServerConfig>, sni: &str, alpn: &[&[u8]]| -> Option<Vec<u8>> {
            let mut cc =
                rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(AcceptAll))
                    .with_no_client_auth();
            cc.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
            let mut c = rustls::ClientConnection::new(Arc::new(cc), sni.to_string().try_into().unwrap()).unwrap();
            let mut s = rustls::ServerConnection::new(server.clone()).unwrap();
            let mut buf = Vec::new();
            for _ in 0..4 {
                buf.clear();
                c.write_tls(&mut buf).unwrap();
                s.read_tls(&mut buf.as_slice()).unwrap();
                s.process_new_packets().ok()?;
                buf.clear();
                s.write_tls(&mut buf).unwrap();
                c.read_tls(&mut buf.as_slice()).unwrap();
                c.process_new_packets().ok()?;
            }
            Some(c.peer_certificates()?.first()?.to_vec())
        };
        let leaf = |k: &rustls::sign::CertifiedKey| Some(k.cert[0].to_vec());
        let issued = server(Some(domain.clone()));
        assert_eq!(shake(&issued, "localhost", &[b"h2"]), leaf(&own), "other names: the own certificate");
        assert_eq!(shake(&issued, "media.example.org", &[b"h2", b"http/1.1"]), leaf(&domain));
        assert_eq!(shake(&issued, "media.example.org", &[b"acme-tls/1"]), leaf(&challenge));
        assert_eq!(shake(&issued, "media.example.org", &[b"h2", b"acme-tls/1"]), leaf(&domain), "not a challenge");
        assert_eq!(shake(&issued, "localhost", &[b"acme-tls/1"]), None, "challenges only for the domains");
        let pending = server(None);
        assert_eq!(
            shake(&pending, "media.example.org", &[b"http/1.1"]),
            leaf(&own),
            "the own one until the CA's arrives"
        );
    }

    #[cfg(feature = "acme")]
    #[derive(Debug)]
    struct AcceptAll;

    #[cfg(feature = "acme")]
    impl rustls::client::danger::ServerCertVerifier for AcceptAll {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms.supported_schemes()
        }
    }
}
