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
