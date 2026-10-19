//! Server identity and capabilities.

use serde::{Deserialize, Serialize};

/// `GET /api/v1/server/info` (public: lets a client recognise the server before signing in).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Stable random id generated on first start.
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    /// SHA-256 of the TLS certificate's public key (hex), when served over TLS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_fingerprint: Option<String>,
    /// No admin exists yet: the first sign-in creates one (loopback only).
    #[serde(default)]
    pub setup_required: bool,
    #[serde(default)]
    pub features: Vec<String>,
    /// The Corrent Connect address (iroh endpoint id), when Connect is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_id: Option<String>,
}

/// How the server can be reached (`GET/PUT /api/v1/server/remote`, admins).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    /// Accept clients from outside the local network (needs a reachable address: port mapping,
    /// a reverse proxy or a VPN).
    pub remote_access: bool,
    /// Announce the server on the local network (mDNS).
    pub discovery: bool,
    /// Corrent Connect: reachable from anywhere by its key, through NATs (relayed when needed).
    pub connect: bool,
    /// Use the public relays for Connect (off: direct connections only).
    pub connect_relay: bool,
    /// A self-hosted relay instead of the public ones (empty: the public ones).
    pub connect_relay_url: String,
    /// Ask the router to forward the HTTPS port (UPnP / NAT-PMP / PCP).
    pub port_mapping: bool,
    /// DLNA/UPnP media server for TVs and players on this network.
    pub dlna: bool,
    /// The account whose libraries DLNA shows (empty: the first administrator).
    pub dlna_user: String,
    /// The Jellyfin-compatible API for Jellyfin apps.
    pub jellyfin_api: bool,
    /// Its plain-HTTP listener on this network (0: off).
    pub jellyfin_port: u16,
    /// Answer Jellyfin apps looking for servers (UDP 7359).
    pub jellyfin_discovery: bool,
    /// Extra networks treated as local (CIDR).
    pub allowed_networks: Vec<String>,
    pub trusted_proxies: Vec<String>,
    pub hostnames: Vec<String>,
}

/// What is running right now.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteStatus {
    pub https_port: u16,
    pub discovery: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_error: Option<String>,
    /// The Connect address (endpoint id) when listening.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_id: Option<String>,
    /// The relay Connect is reachable through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_relay: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_error: Option<String>,
    /// The public address the router forwards to us ("ip:port").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapped_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_mapping_error: Option<String>,
    /// The DLNA port while DLNA runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dlna_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dlna_error: Option<String>,
    /// The Jellyfin listener's port while it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jellyfin_port: Option<u16>,
    pub jellyfin_discovery: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jellyfin_error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoteAccess {
    pub config: RemoteConfig,
    pub status: RemoteStatus,
}

/// One entry of the server's folder browser (`GET /api/v1/server/fs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FsListing {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_space: Option<u64>,
}
