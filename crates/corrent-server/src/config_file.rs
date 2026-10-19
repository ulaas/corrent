//! `server.toml`: how the media server listens and who may reach it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    /// Shown to clients; empty = the computer's name.
    pub name: String,
    /// Address for the HTTPS listener ("0.0.0.0" = every interface; the LAN guard still applies).
    pub bind: String,
    pub https_port: u16,
    /// Plain HTTP, on loopback only (local tools and the first-run setup page).
    pub http_port: u16,
    /// Serve HTTPS (self-signed unless `cert_path`/`key_path` are set).
    pub tls: bool,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    /// Accept clients from outside the local network. Off: only loopback, private, link-local,
    /// CGNAT (Tailscale) and `allowed_networks` addresses get in.
    pub remote_access: bool,
    /// Extra networks treated as local, in CIDR form ("203.0.113.0/24").
    pub allowed_networks: Vec<String>,
    /// Reverse proxies whose `X-Forwarded-For` is believed (CIDRs). Empty = never.
    pub trusted_proxies: Vec<String>,
    /// Host names browsers may use for this server besides IP addresses, `localhost`, the
    /// computer's name and `<name>.local` (DNS-rebinding protection).
    pub hostnames: Vec<String>,
    /// Serve the qBittorrent-compatible API at /api/v2 (Sonarr, Radarr).
    pub qbt_api: bool,
    /// Folders non-admin users may download to and browse, besides the download folder,
    /// category folders and library folders.
    pub download_roots: Vec<String>,
    /// Announce the server on the local network (mDNS).
    pub discovery: bool,
    /// Corrent Connect (iroh): reachable through NATs by the server's key.
    pub connect: bool,
    /// Use the public relays for Connect.
    pub connect_relay: bool,
    /// Ask the router to forward the HTTPS port.
    pub port_mapping: bool,
    /// Serve Prometheus metrics at /metrics (local network only, never over Corrent Connect).
    pub metrics: bool,
    /// When set, /metrics needs `Authorization: Bearer <metrics_token>`.
    pub metrics_token: String,
    /// A plugin catalog (JSON index) to install signed plugins from; empty: none.
    pub plugin_catalog: String,
    /// Ed25519 public keys (hex) whose signatures on catalog plugins are trusted.
    pub plugin_keys: Vec<String>,
    /// DLNA/UPnP media server for TVs and players on the local network (plain HTTP).
    pub dlna: bool,
    pub dlna_port: u16,
    /// The account whose libraries DLNA shows; empty = the first administrator.
    pub dlna_user: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            name: String::new(),
            bind: "0.0.0.0".into(),
            https_port: 8443,
            http_port: 8088,
            tls: true,
            cert_path: None,
            key_path: None,
            remote_access: false,
            allowed_networks: Vec::new(),
            trusted_proxies: Vec::new(),
            hostnames: Vec::new(),
            qbt_api: true,
            download_roots: Vec::new(),
            discovery: true,
            connect: false,
            connect_relay: true,
            port_mapping: false,
            metrics: false,
            metrics_token: String::new(),
            plugin_catalog: String::new(),
            plugin_keys: Vec::new(),
            dlna: false,
            dlna_port: 8201,
            dlna_user: String::new(),
        }
    }
}

impl ServerConfig {
    pub fn file(data_dir: &Path) -> PathBuf {
        data_dir.join("server.toml")
    }

    /// Missing file → defaults. A damaged file is an error (never silently replaced).
    pub fn load(data_dir: &Path) -> anyhow::Result<ServerConfig> {
        match std::fs::read_to_string(Self::file(data_dir)) {
            Ok(t) => Ok(toml::from_str(&t)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ServerConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, data_dir: &Path) -> anyhow::Result<()> {
        corrent_core::util::write_atomic(&Self::file(data_dir), toml::to_string_pretty(self)?.as_bytes())
    }

    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_string();
        }
        hostname()
    }
}

/// The computer's host name (best effort).
pub fn hostname() -> String {
    for var in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(h) = std::env::var(var)
            && !h.trim().is_empty()
        {
            return h.trim().to_string();
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Corrent".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let d = std::env::temp_dir().join(format!("corrent-srvcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(ServerConfig::load(&d).unwrap(), ServerConfig::default());
        let c = ServerConfig { remote_access: true, hostnames: vec!["media.example".into()], ..Default::default() };
        c.save(&d).unwrap();
        assert_eq!(ServerConfig::load(&d).unwrap(), c);
        std::fs::write(ServerConfig::file(&d), "https_port = \"x\"").unwrap();
        assert!(ServerConfig::load(&d).is_err());
        let _ = std::fs::remove_dir_all(d);
    }
}
