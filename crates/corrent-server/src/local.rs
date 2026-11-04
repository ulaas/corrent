//! The server hosted inside the desktop app ("local mode"): same engine, same profile,
//! listening on loopback only, with a device token for the app itself.

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use corrent_core::config::{Paths, Settings};
use corrent_core::engine::EngineHandle;

use crate::identity::{HashCost, Identity};
use crate::state::AppState;
use crate::{Listener, ServerConfig, ServerParts};

pub struct LocalServer {
    pub state: Arc<AppState>,
    /// `http://127.0.0.1:<port>`
    pub base_url: String,
    pub token: String,
    pub user: String,
    _listener: Listener,
}

impl LocalServer {
    pub fn addr(&self) -> &str {
        &self.base_url
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.state.stop();
        // Its key dies with it.
        let _ = std::fs::remove_file(agent_link_file(&self.state.paths.data_dir));
    }
}

/// The file `corrent --mcp` (the stdio bridge for AI agents) reads: where this server listens
/// and an agent token of its own (revocable under Devices; kept while it stays valid).
pub fn agent_link_file(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("mcp.json")
}

async fn agent_link(state: &Arc<AppState>, user: i64, url: &str) {
    // A key of this run (not a sign-in token): the MCP endpoint takes it, nothing else does, and
    // it only reaches what managing the library allows. Earlier versions issued real device
    // tokens for this: those go.
    let _ = state.identity.db().write(|t| t.execute("DELETE FROM devices WHERE client = 'corrent --mcp'", [])).await;
    let key = format!("cm_{}", crate::util::random_hex(32));
    *state.agent_key.lock().unwrap_or_else(|e| e.into_inner()) = Some((key.clone(), user));
    let file = agent_link_file(&state.paths.data_dir);
    let body = serde_json::json!({ "url": url, "token": key }).to_string();
    let written = tokio::task::spawn_blocking(move || write_owner_only(&file, &body).map_err(|e| (file, e))).await;
    if let Ok(Err((file, e))) = written {
        tracing::warn!("{}: {e:#}", file.display());
    }
}

/// Writes a secret only this account can read, from before it holds anything: 0600 on Unix, on
/// Windows an ACL without inherited entries (the profile may be on a shared drive in portable
/// mode).
fn write_owner_only(path: &std::path::Path, data: &str) -> anyhow::Result<()> {
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
    #[cfg(windows)]
    limit_to_this_account(&tmp);
    f.write_all(data.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(windows)]
fn limit_to_this_account(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(user) = std::env::var("USERNAME") else { return };
    let out = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::warn!("icacls: {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => tracing::warn!("icacls: {e}"),
    }
}

/// Starts the in-process server on the app's engine and profile.
pub async fn start(
    engine: EngineHandle,
    settings: Arc<ArcSwap<Settings>>,
    paths: Paths,
) -> anyhow::Result<LocalServer> {
    // A staged restore was applied by the app at startup, before its settings were read.
    let db = corrent_db::Db::open(&paths.data_dir.join("corrent.db"))?;
    let identity = Identity::new(db.clone(), HashCost::STANDARD);
    let library = corrent_library::Library::new(
        db,
        &paths.data_dir,
        corrent_library::probe::Prober::new(corrent_library::probe::Prober::find_worker(), 2),
    );
    // Loopback only, no TLS, no qBittorrent API (the app has its own setting for that).
    let cfg = ServerConfig {
        bind: "127.0.0.1".into(),
        tls: false,
        qbt_api: false,
        ..ServerConfig::load(&paths.data_dir).unwrap_or_default()
    };
    let state = crate::build_state(ServerParts {
        engine,
        settings,
        paths,
        identity,
        cfg,
        hub: Default::default(),
        tls_fingerprint: None,
        ca_pem: None,
        library,
    })
    .await?;
    let (user, issued) = state.identity.local_device_token().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let listener = crate::serve_http(crate::router(state.clone()), SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    crate::restart_watching(&state).await;
    crate::start_tasks(state.clone());
    let base_url = format!("http://127.0.0.1:{}", listener.addr.port());
    agent_link(&state, user.id, &base_url).await;
    Ok(LocalServer { base_url, token: issued.token, user: user.name, state, _listener: listener })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_agent_key_is_readable_after_writing_it_privately() {
        let d = std::env::temp_dir().join(format!("corrent-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("mcp.json");
        std::fs::write(&f, "old").unwrap();
        super::write_owner_only(&f, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
