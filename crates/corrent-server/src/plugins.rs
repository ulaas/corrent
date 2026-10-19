//! Plugins in the server: loading built-in and installed plugins with their stored settings,
//! persisting changes, installing and removing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use corrent_db::Db;
use corrent_db::rusqlite::{OptionalExtension, params};
use corrent_plugins::{KvStore, Manifest, PluginHost};

/// Plugin key-value storage in `corrent.db` (called from plugin threads, so blocking is fine).
pub struct DbKv(pub Db);

const KV_QUOTA: i64 = 10_000;

impl KvStore for DbKv {
    fn get(&self, plugin: &str, key: &str) -> Option<String> {
        let (p, k) = (plugin.to_string(), key.to_string());
        self.0
            .read_blocking(move |c| {
                c.query_row("SELECT value FROM plugin_kv WHERE plugin = ?1 AND key = ?2", params![p, k], |r| r.get(0))
                    .optional()
            })
            .ok()
            .flatten()
    }

    fn set(&self, plugin: &str, key: &str, value: &str) -> Result<(), String> {
        let (p, k, v) = (plugin.to_string(), key.to_string(), value.to_string());
        self.0
            .write_blocking(move |t| {
                let n: i64 = t.query_row("SELECT count(*) FROM plugin_kv WHERE plugin = ?1", [&p], |r| r.get(0))?;
                if n >= KV_QUOTA {
                    return Ok(Err("storage quota reached".to_string()));
                }
                t.execute(
                    "INSERT INTO plugin_kv(plugin, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(plugin, key) DO UPDATE SET value = excluded.value",
                    params![p, k, v],
                )?;
                Ok(Ok(()))
            })
            .map_err(|e| e.to_string())?
    }
}

/// `<data>/plugins/<id>/` holds installed (non-built-in) plugins.
pub fn plugins_dir(data: &Path) -> PathBuf {
    data.join("plugins")
}

struct Stored {
    enabled: bool,
    settings: HashMap<String, String>,
}

async fn stored(db: &Db) -> HashMap<String, Stored> {
    db.read(|c| {
        let mut st = c.prepare("SELECT id, enabled, settings FROM plugins")?;
        st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Stored {
                    enabled: r.get::<_, i64>(1)? != 0,
                    settings: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                },
            ))
        })?
        .collect()
    })
    .await
    .unwrap_or_default()
}

/// Creates the host and loads every plugin.
pub async fn start(db: &Db, data: &Path) -> Option<Arc<PluginHost>> {
    let host = match PluginHost::new(Arc::new(DbKv(db.clone())), tokio::runtime::Handle::current()) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("plugins are unavailable: {e:#}");
            return None;
        }
    };
    let saved = stored(db).await;
    let mut all: Vec<(Manifest, Vec<u8>, bool)> =
        corrent_plugins::builtins().into_iter().map(|(m, w)| (m, w.to_vec(), true)).collect();
    for e in std::fs::read_dir(plugins_dir(data)).into_iter().flatten().flatten() {
        let (m, w) = (e.path().join("plugin.toml"), e.path().join("plugin.wasm"));
        match (
            std::fs::read_to_string(&m).map_err(|e| e.to_string()).and_then(|t| Manifest::parse(&t)),
            std::fs::read(&w),
        ) {
            (Ok(m), Ok(w)) => all.push((m, w, false)),
            (Err(err), _) => tracing::warn!("plugin in {}: {err}", m.display()),
            (_, Err(e)) => tracing::warn!("plugin in {}: {e}", w.display()),
        }
    }
    let h2 = host.clone();
    tokio::task::spawn_blocking(move || {
        for (m, w, builtin) in all {
            let s = saved.get(&m.id);
            let id = m.id.clone();
            if let Err(e) =
                h2.load(m, &w, s.map(|s| s.settings.clone()).unwrap_or_default(), s.is_none_or(|s| s.enabled), builtin)
            {
                tracing::warn!("plugin {id}: {e}");
            }
        }
    })
    .await
    .ok()?;
    tracing::info!("{} plugins loaded", host.list().len());
    Some(host)
}

/// Saves a plugin's state (enabled and settings).
pub async fn persist(db: &Db, host: &PluginHost, id: &str) {
    let Some(info) = host.list().into_iter().find(|p| p.manifest.id == id) else { return };
    // The real (unmasked) settings come from the host.
    let settings = host.settings_raw(id).unwrap_or_default();
    let (id, enabled, json, now) = (
        id.to_string(),
        info.enabled,
        serde_json::to_string(&settings).unwrap_or_default(),
        corrent_core::util::now_unix(),
    );
    let r = db
        .write(move |t| {
            t.execute(
                "INSERT INTO plugins(id, enabled, settings, installed_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, settings = excluded.settings",
                params![id, enabled as i64, json, now],
            )
        })
        .await;
    if let Err(e) = r {
        tracing::error!("saving plugin state: {e}");
    }
}
