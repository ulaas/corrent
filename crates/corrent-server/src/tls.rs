//! The server's TLS identity: a private CA and a leaf certificate it signs, both made on first
//! start (`<data>/tls/`). Clients pin the CA's public key (its SHA-256 is the fingerprint shown
//! everywhere), so the leaf can be re-issued when the computer's names or addresses change
//! without breaking paired devices.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use aws_lc_rs::digest;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose,
    PublicKeyData, SanType,
};

pub struct TlsMaterial {
    /// Leaf followed by the CA (PEM).
    pub cert_chain_pem: String,
    pub key_pem: String,
    pub ca_pem: String,
    /// Hex SHA-256 of the CA's SubjectPublicKeyInfo.
    pub fingerprint: String,
}

fn ca_params(server_id: &str) -> CertificateParams {
    let mut p = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("Corrent Server CA {server_id}"));
    dn.push(DnType::OrganizationName, "Corrent");
    p.distinguished_name = dn;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p
}

pub fn fingerprint_of(key: &impl PublicKeyData) -> String {
    digest::digest(&digest::SHA256, &key.subject_public_key_info())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fingerprint shown to people: `AB:CD:…` groups of the first 16 bytes.
pub fn short_fingerprint(hex: &str) -> String {
    hex.as_bytes().chunks(2).take(16).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join(":")
}

/// Writes a private key: created owner-only from the start (no window where it is readable),
/// then renamed into place.
pub(crate) fn write_private(path: &Path, data: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp)?;
    f.write_all(data.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The pinning fingerprint of an existing CA in `dir`, without creating or changing anything.
pub fn existing_fingerprint(dir: &Path) -> Option<String> {
    let pem = std::fs::read_to_string(dir.join("ca.key")).ok()?;
    KeyPair::from_pem(&pem).ok().map(|k| fingerprint_of(&k))
}

/// Names and addresses the leaf certificate should cover.
pub fn local_names(extra: &[String]) -> (Vec<String>, Vec<IpAddr>) {
    let host = crate::config_file::hostname();
    let mut names = vec!["localhost".to_string(), host.clone(), format!("{host}.local")];
    names.extend(extra.iter().cloned());
    names.sort();
    names.dedup();
    let mut ips: Vec<IpAddr> = vec!["127.0.0.1".parse().expect("static"), "::1".parse().expect("static")];
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        ips.extend(ifs.into_iter().map(|i| i.ip()).filter(|ip| !ip.is_loopback()));
    }
    ips.sort();
    ips.dedup();
    (names, ips)
}

/// Loads the TLS material from `dir`, creating the CA once and (re)issuing the leaf when the
/// names/addresses it must cover changed.
pub fn ensure(dir: &Path, server_id: &str, names: &[String], ips: &[IpAddr]) -> anyhow::Result<TlsMaterial> {
    std::fs::create_dir_all(dir)?;
    let (ca_key_path, ca_path) = (dir.join("ca.key"), dir.join("ca.pem"));
    let (key_path, cert_path, sans_path) = (dir.join("server.key"), dir.join("server.pem"), dir.join("server.sans"));
    let ca_key = match std::fs::read_to_string(&ca_key_path) {
        Ok(pem) if ca_path.exists() => KeyPair::from_pem(&pem)?,
        _ => {
            let key = KeyPair::generate()?;
            let cert = ca_params(server_id).self_signed(&key)?;
            write_private(&ca_key_path, &key.serialize_pem())?;
            corrent_core::util::write_atomic(&ca_path, cert.pem().as_bytes())?;
            // A new CA invalidates any old leaf.
            let _ = std::fs::remove_file(&sans_path);
            tracing::info!("created the server's certificate authority in {}", dir.display());
            key
        }
    };
    let ca_pem = std::fs::read_to_string(&ca_path)?;
    let wanted = format!("{}\n{}", names.join(","), ips.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    // The sidecar holds the covered names/addresses and the issue time (unix seconds).
    let now_unix = time::OffsetDateTime::now_utc().unix_timestamp();
    let current = std::fs::read_to_string(&sans_path).unwrap_or_default();
    let (covered, issued) = current.rsplit_once('\n').unwrap_or(("", ""));
    let fresh = issued.parse::<i64>().is_ok_and(|t| now_unix - t < 700 * 86_400);
    if covered != wanted || !fresh || !key_path.exists() || !cert_path.exists() {
        let key = KeyPair::generate()?;
        let mut p = CertificateParams::new(names.to_vec())?;
        p.subject_alt_names.extend(ips.iter().map(|ip| SanType::IpAddress(*ip)));
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, names.get(1).cloned().unwrap_or_else(|| "corrent".into()));
        p.distinguished_name = dn;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
        p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        // 825 days: the longest Apple platforms accept for TLS server certificates. A day of
        // slack before now covers clocks that are a little behind.
        let now = time::OffsetDateTime::now_utc();
        p.not_before = now - time::Duration::days(1);
        p.not_after = now + time::Duration::days(825);
        let issuer = Issuer::new(ca_params(server_id), &ca_key);
        let cert = p.signed_by(&key, &issuer)?;
        write_private(&key_path, &key.serialize_pem())?;
        corrent_core::util::write_atomic(&cert_path, cert.pem().as_bytes())?;
        corrent_core::util::write_atomic(&sans_path, format!("{wanted}\n{now_unix}").as_bytes())?;
        tracing::info!("issued the server certificate for {} and {} addresses", names.join(", "), ips.len());
    }
    let leaf = std::fs::read_to_string(&cert_path)?;
    Ok(TlsMaterial {
        cert_chain_pem: format!("{leaf}{ca_pem}"),
        key_pem: std::fs::read_to_string(&key_path)?,
        fingerprint: fingerprint_of(&ca_key),
        ca_pem,
    })
}

/// User-provided certificate and key (e.g. from ACME): no pinning fingerprint.
pub fn load_custom(cert: &PathBuf, key: &PathBuf) -> anyhow::Result<TlsMaterial> {
    Ok(TlsMaterial {
        cert_chain_pem: std::fs::read_to_string(cert)?,
        key_pem: std::fs::read_to_string(key)?,
        ca_pem: String::new(),
        fingerprint: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_once_leaf_when_names_change() {
        let d = std::env::temp_dir().join(format!("corrent-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ips: Vec<IpAddr> = vec!["127.0.0.1".parse().unwrap()];
        let a = ensure(&d, "abc", &["localhost".into()], &ips).unwrap();
        assert_eq!(a.fingerprint.len(), 64);
        assert!(a.cert_chain_pem.matches("BEGIN CERTIFICATE").count() == 2);
        let b = ensure(&d, "abc", &["localhost".into()], &ips).unwrap();
        assert_eq!(a.cert_chain_pem, b.cert_chain_pem, "reused");
        let c = ensure(&d, "abc", &["localhost".into(), "media.example".into()], &ips).unwrap();
        assert_ne!(a.cert_chain_pem, c.cert_chain_pem, "re-issued for a new name");
        assert_eq!(a.fingerprint, c.fingerprint, "same CA, same pin");
        assert_eq!(short_fingerprint(&a.fingerprint).len(), 16 * 3 - 1);
        assert_eq!(existing_fingerprint(&d).as_deref(), Some(a.fingerprint.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(d.join("server.key")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(d);
    }
}
