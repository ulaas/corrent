//! `plugin.toml`: what a plugin is, what it may reach, and its settings.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Metadata,
    Indexer,
    Notifier,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// Host names the plugin may call over HTTP(S); `*.example.com` matches subdomains.
    pub hosts: Vec<String>,
    /// Allow those hosts to resolve to the server's own network (off: public addresses only).
    pub allow_local_network: bool,
    /// Requests per second (burst of twice that).
    pub rate_per_sec: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingDef {
    pub key: String,
    pub label: String,
    pub help: String,
    /// Stored, given to the plugin, never shown back to people.
    pub secret: bool,
    pub required: bool,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// The host API version the plugin was built for ("1").
    pub host_api: String,
    pub kinds: Vec<Kind>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub settings: Vec<SettingDef>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| format!("plugin.toml: {e}"))?;
        if m.id.is_empty()
            || m.id.len() > 64
            || !m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err("plugin id must be lower-case letters, digits and dashes".into());
        }
        if m.host_api != "1" {
            return Err(format!("plugin needs host API {}, this server has 1", m.host_api));
        }
        if m.kinds.is_empty() {
            return Err("plugin declares no kind".into());
        }
        Ok(m)
    }

    /// Whether `host` is one the plugin declared.
    pub fn allows_host(&self, host: &str) -> bool {
        let h = host.trim_end_matches('.').to_ascii_lowercase();
        self.permissions.hosts.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            match p.strip_prefix("*.") {
                Some(suffix) => h.ends_with(&format!(".{suffix}")),
                None => h == p,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_hosts() {
        let m = Manifest::parse(
            r#"id = "tmdb"
name = "TMDB"
version = "1.0.0"
host_api = "1"
kinds = ["metadata"]
[permissions]
hosts = ["api.themoviedb.org", "*.tmdb.org"]
[[settings]]
key = "api_key"
label = "API key"
secret = true
required = true
"#,
        )
        .unwrap();
        assert!(m.allows_host("api.themoviedb.org"));
        assert!(m.allows_host("image.tmdb.org"));
        assert!(!m.allows_host("tmdb.org.evil.com"));
        assert!(!m.allows_host("evil.com"));
        assert!(
            Manifest::parse("id = \"Bad Id\"\nname=\"x\"\nversion=\"1\"\nhost_api=\"1\"\nkinds=[\"metadata\"]")
                .is_err()
        );
        assert!(
            Manifest::parse("id = \"x\"\nname=\"x\"\nversion=\"1\"\nhost_api=\"2\"\nkinds=[\"metadata\"]").is_err()
        );
    }
}
