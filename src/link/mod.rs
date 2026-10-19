//! Where the torrent engine runs: in this app (the default) or on a Corrent server the app is
//! paired with. A remote server stands in for the local engine behind the same
//! [`EngineHandle`](crate::engine::EngineHandle), so the torrent UI is the same either way.

pub mod pair;
pub mod remote;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Paths;

/// A server this app was paired with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerEntry {
    /// The server's stable id.
    pub id: String,
    pub name: String,
    /// `https://host:port` (no trailing slash).
    pub url: String,
    pub user: String,
    /// Device token (a secret: `servers.toml` is readable by its owner only).
    pub token: String,
    /// Hex SHA-256 of the server CA's public key (empty: a publicly trusted certificate).
    pub fingerprint: String,
    /// The server's CA (PEM), trusted for this server only.
    pub ca_pem: String,
}

impl ServerEntry {
    pub fn ca(&self) -> Option<&str> {
        (!self.ca_pem.is_empty()).then_some(self.ca_pem.as_str())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Servers {
    /// Id of the server the app uses (None: the engine runs in this app).
    pub active: Option<String>,
    #[serde(rename = "server")]
    pub list: Vec<ServerEntry>,
}

impl Servers {
    fn path(paths: &Paths) -> PathBuf {
        paths.data_dir.join("servers.toml")
    }

    /// The saved servers (an unreadable file counts as none: the app then runs locally).
    pub fn load(paths: &Paths) -> Servers {
        match std::fs::read_to_string(Self::path(paths)) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("servers.toml is not valid ({e}); using the local engine");
                Servers::default()
            }),
            Err(_) => Servers::default(),
        }
    }

    /// Writes the list atomically, readable by the owner only (it holds device tokens).
    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        use std::io::Write;
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let path = Self::path(paths);
        let tmp = path.with_extension("toml.tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)
    }

    pub fn active_entry(&self) -> Option<&ServerEntry> {
        let id = self.active.as_ref()?;
        self.list.iter().find(|s| &s.id == id)
    }

    /// Adds or replaces (same server id) an entry.
    pub fn upsert(&mut self, e: ServerEntry) {
        match self.list.iter_mut().find(|s| s.id == e.id) {
            Some(s) => *s = e,
            None => self.list.push(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_round_trip() {
        let dir = std::env::temp_dir().join(format!("corrent-servers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths { data_dir: dir.clone(), portable: true };
        assert!(Servers::load(&paths).active_entry().is_none());
        let mut s = Servers::default();
        s.upsert(ServerEntry { id: "a".into(), name: "Home".into(), token: "t".into(), ..Default::default() });
        s.upsert(ServerEntry { id: "a".into(), name: "Home 2".into(), ..Default::default() });
        s.active = Some("a".into());
        s.save(&paths).unwrap();
        let back = Servers::load(&paths);
        assert_eq!(back.list.len(), 1);
        assert_eq!(back.active_entry().map(|e| e.name.as_str()), Some("Home 2"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("servers.toml")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "owner only");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
