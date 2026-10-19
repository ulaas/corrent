//! Users, roles, permissions, sessions, devices and pairing.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    #[default]
    User,
    /// Read and play only.
    Guest,
    /// Like `user` for playback, limited by a rating ceiling and allowed libraries.
    Kids,
}

impl Role {
    pub const ALL: [Role; 4] = [Role::Admin, Role::User, Role::Guest, Role::Kids];

    pub fn label(self) -> &'static str {
        match self {
            Role::Admin => t!("Administrator"),
            Role::User => t!("User"),
            Role::Guest => t!("Guest"),
            Role::Kids => t!("Kids"),
        }
    }

    pub fn default_perms(self) -> Perms {
        match self {
            Role::Admin => Perms::ALL,
            Role::User => Perms(
                Perms::TORRENTS_ADD.0
                    | Perms::TORRENTS_MANAGE_OWN.0
                    | Perms::PLAYBACK_TRANSCODE.0
                    | Perms::SETTINGS_READ.0,
            ),
            Role::Guest | Role::Kids => Perms(Perms::PLAYBACK_TRANSCODE.0),
        }
    }
}

/// Permission bits. A user's effective permissions are their role's defaults, unless the admin
/// stored an override.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Perms(pub u32);

impl Perms {
    pub const TORRENTS_ADD: Perms = Perms(1);
    pub const TORRENTS_MANAGE_OWN: Perms = Perms(1 << 1);
    pub const TORRENTS_MANAGE_ALL: Perms = Perms(1 << 2);
    pub const LIBRARY_MANAGE: Perms = Perms(1 << 3);
    pub const PLAYBACK_TRANSCODE: Perms = Perms(1 << 4);
    pub const SETTINGS_READ: Perms = Perms(1 << 5);
    pub const SETTINGS_WRITE: Perms = Perms(1 << 6);
    pub const USERS_MANAGE: Perms = Perms(1 << 7);
    pub const PLUGINS_MANAGE: Perms = Perms(1 << 8);
    /// The qBittorrent-compatible API (Sonarr/Radarr).
    pub const DOWNLOADER: Perms = Perms(1 << 9);
    pub const ALL: Perms = Perms((1 << 10) - 1);

    /// Every named bit with its stable wire name.
    pub const NAMED: [(Perms, &'static str); 10] = [
        (Perms::TORRENTS_ADD, "torrents.add"),
        (Perms::TORRENTS_MANAGE_OWN, "torrents.manage_own"),
        (Perms::TORRENTS_MANAGE_ALL, "torrents.manage_all"),
        (Perms::LIBRARY_MANAGE, "library.manage"),
        (Perms::PLAYBACK_TRANSCODE, "playback.transcode"),
        (Perms::SETTINGS_READ, "settings.read"),
        (Perms::SETTINGS_WRITE, "settings.write"),
        (Perms::USERS_MANAGE, "users.manage"),
        (Perms::PLUGINS_MANAGE, "plugins.manage"),
        (Perms::DOWNLOADER, "downloader"),
    ];

    pub fn contains(self, other: Perms) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn names(self) -> Vec<&'static str> {
        Perms::NAMED.iter().filter(|(p, _)| self.contains(*p)).map(|(_, n)| *n).collect()
    }
}

impl std::ops::BitOr for Perms {
    type Output = Perms;
    fn bitor(self, rhs: Perms) -> Perms {
        Perms(self.0 | rhs.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub role: Role,
    pub perms: Perms,
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rating: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// When set, a long-lived device token is issued instead of a browser session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    /// "linux", "windows", "macos", "web", "tv", …
    pub platform: String,
    #[serde(default)]
    pub client: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoginResponse {
    pub user: User,
    /// Bearer token (device logins). Browser logins get an HttpOnly cookie instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Double-submit CSRF token for cookie sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csrf: Option<String>,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub platform: String,
    pub client: String,
    pub created_at: i64,
    pub last_seen: i64,
    pub last_ip: String,
    /// The device making this request.
    #[serde(default)]
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: i64,
    /// `corrent://pair?…` link carrying the address, server id, certificate fingerprint and code.
    pub link: String,
    /// The link as a QR code (SVG), for apps on phones and TVs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub qr_svg: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device: DeviceInfo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewUser {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    pub password: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perms: Option<Perms>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserUpdate {
    pub display_name: Option<String>,
    pub password: Option<String>,
    /// Required when people change their own password (not when an admin resets it).
    pub current_password: Option<String>,
    pub role: Option<Role>,
    pub perms: Option<Perms>,
    pub disabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub ip: String,
    pub action: String,
    pub target: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_perms() {
        assert!(Role::Admin.default_perms().contains(Perms::USERS_MANAGE));
        assert!(!Role::User.default_perms().contains(Perms::USERS_MANAGE));
        assert!(Role::User.default_perms().contains(Perms::TORRENTS_ADD));
        assert!(!Role::Guest.default_perms().contains(Perms::TORRENTS_ADD));
        assert_eq!(Perms::ALL.names().len(), Perms::NAMED.len());
        assert_eq!(serde_json::to_string(&Role::Kids).unwrap(), "\"kids\"");
    }
}
