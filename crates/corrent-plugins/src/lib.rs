//! Corrent's plugin host: WebAssembly components (wasmtime, component model) implementing the
//! worlds in `wit/corrent.wit`.
//!
//! Plugins are sandboxed: no filesystem, no sockets, no environment. They reach the outside
//! only through the host API — HTTP to hosts their manifest declares (public addresses only
//! unless the admin allowed the local network; connections are pinned to the checked address,
//! redirects re-checked), a per-plugin key-value store and their settings. Each call runs in a
//! fresh instance with a 64 MiB memory cap and a 30 s deadline; three traps disable a plugin.

pub mod manifest;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub use manifest::{Kind, Manifest, SettingDef};

mod bindings {
    wasmtime::component::bindgen!({ path: "wit", world: "metadata-provider" });
}
mod indexer_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "indexer-plugin",
        with: { "corrent:plugin/host": super::bindings::corrent::plugin::host, "corrent:plugin/types": super::bindings::corrent::plugin::types },
    });
}
mod notifier_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "notifier-plugin",
        with: { "corrent:plugin/host": super::bindings::corrent::plugin::host, "corrent:plugin/types": super::bindings::corrent::plugin::types },
    });
}

pub use bindings::corrent::plugin::types::{HttpRequest, HttpResponse};
pub use bindings::exports::corrent::plugin::metadata::{
    Artwork, Candidate, Credit, Episode, ItemDetails, MediaKind, SearchQuery,
};
pub use indexer_bindings::exports::corrent::plugin::indexer::Release;

const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const TICK: Duration = Duration::from_millis(100);
/// 30 s at one epoch per 100 ms.
const DEADLINE_TICKS: u64 = 300;
const MAX_BODY: usize = 16 * 1024 * 1024;
const BREAKER: u32 = 3;

/// Where plugins keep data (implemented by the server on top of `corrent.db`).
pub trait KvStore: Send + Sync {
    fn get(&self, plugin: &str, key: &str) -> Option<String>;
    fn set(&self, plugin: &str, key: &str, value: &str) -> Result<(), String>;
}

/// An in-memory store (tests).
#[derive(Default)]
pub struct MemoryKv(Mutex<HashMap<(String, String), String>>);

impl KvStore for MemoryKv {
    fn get(&self, plugin: &str, key: &str) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&(plugin.to_string(), key.to_string())).cloned()
    }
    fn set(&self, plugin: &str, key: &str, value: &str) -> Result<(), String> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if value.len() > 1024 * 1024 || m.len() > 10_000 {
            return Err("quota exceeded".into());
        }
        m.insert((plugin.to_string(), key.to_string()), value.to_string());
        Ok(())
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Per-plugin context shared by its calls.
struct PluginCtx {
    manifest: Manifest,
    settings: RwLock<HashMap<String, String>>,
    kv: Arc<dyn KvStore>,
    rt: tokio::runtime::Handle,
    bucket: Mutex<Bucket>,
}

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    plugin: Arc<PluginCtx>,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Public internet addresses only (no loopback, private, link-local, CGNAT, multicast…).
pub fn is_public(ip: IpAddr) -> bool {
    match normalize(ip) {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_multicast()
                || v.is_broadcast()
                || v.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64))
        }
        IpAddr::V6(v) => (v.segments()[0] & 0xe000) == 0x2000,
    }
}

impl PluginCtx {
    fn allow_request(&self) -> bool {
        let rate = self.manifest.permissions.rate_per_sec.unwrap_or(10).max(1) as f64;
        let mut b = self.bucket.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(rate * 2.0);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// One request (no redirects) to a checked, pinned address.
    async fn fetch_once(&self, req: &HttpRequest, url: &url::Url) -> Result<reqwest::Response, String> {
        if !matches!(url.scheme(), "https" | "http") {
            return Err("only http(s)".into());
        }
        let host = url.host_str().ok_or("no host")?.trim_start_matches('[').trim_end_matches(']').to_string();
        if !self.manifest.allows_host(&host) {
            return Err(format!("host {host} is not declared in the plugin's manifest"));
        }
        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::SocketAddr> = match host.parse::<IpAddr>() {
            Ok(ip) => vec![std::net::SocketAddr::new(ip, port)],
            Err(_) => tokio::net::lookup_host((host.as_str(), port)).await.map_err(|e| e.to_string())?.collect(),
        };
        let addr = addrs.first().copied().ok_or("host did not resolve")?;
        if !self.manifest.permissions.allow_local_network && !addrs.iter().all(|a| is_public(a.ip())) {
            return Err(format!("{host} resolves to the server's own network"));
        }
        // Pin the connection to the address that was checked (no DNS rebinding).
        let client = reqwest::Client::builder()
            .resolve(&host, addr)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("Corrent/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        let method =
            reqwest::Method::from_bytes(req.method.to_ascii_uppercase().as_bytes()).map_err(|e| e.to_string())?;
        let mut rb = client.request(method, url.clone());
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(b) = &req.body {
            rb = rb.body(b.clone());
        }
        rb.send().await.map_err(|e| e.to_string())
    }

    async fn http(&self, req: HttpRequest) -> Result<HttpResponse, String> {
        if !self.allow_request() {
            return Err("rate limited".into());
        }
        let mut url = url::Url::parse(&req.url).map_err(|e| e.to_string())?;
        for _ in 0..6 {
            let res = self.fetch_once(&req, &url).await?;
            if res.status().is_redirection()
                && let Some(loc) = res.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok())
            {
                url = url.join(loc).map_err(|e| e.to_string())?;
                continue;
            }
            let status = res.status().as_u16();
            let headers = res
                .headers()
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
                .collect();
            if res.content_length().is_some_and(|l| l as usize > MAX_BODY) {
                return Err("response too large".into());
            }
            let body = res.bytes().await.map_err(|e| e.to_string())?;
            if body.len() > MAX_BODY {
                return Err("response too large".into());
            }
            return Ok(HttpResponse { status, headers, body: body.to_vec() });
        }
        Err("too many redirects".into())
    }
}

impl bindings::corrent::plugin::types::Host for HostState {}

impl bindings::corrent::plugin::host::Host for HostState {
    fn http(&mut self, req: HttpRequest) -> Result<HttpResponse, String> {
        let p = self.plugin.clone();
        // Calls run on a blocking thread, where waiting on the runtime is allowed.
        p.rt.block_on(async { p.http(req).await })
    }

    fn log(&mut self, level: String, message: String) {
        let id = &self.plugin.manifest.id;
        let message: String = message.chars().take(2000).collect();
        match level.as_str() {
            "error" => tracing::error!("plugin {id}: {message}"),
            "warn" => tracing::warn!("plugin {id}: {message}"),
            "info" => tracing::info!("plugin {id}: {message}"),
            _ => tracing::debug!("plugin {id}: {message}"),
        }
    }

    fn setting(&mut self, key: String) -> Option<String> {
        let s = self.plugin.settings.read().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
        s.or_else(|| self.plugin.manifest.settings.iter().find(|d| d.key == key).and_then(|d| d.default.clone()))
    }

    fn kv_get(&mut self, key: String) -> Option<String> {
        self.plugin.kv.get(&self.plugin.manifest.id, &key)
    }

    fn kv_set(&mut self, key: String, value: String) -> Result<(), String> {
        if key.len() > 256 || value.len() > 1024 * 1024 {
            return Err("too large".into());
        }
        self.plugin.kv.set(&self.plugin.manifest.id, &key, &value)
    }
}

struct Loaded {
    ctx: Arc<PluginCtx>,
    component: Component,
    enabled: bool,
    builtin: bool,
    failures: AtomicU32,
}

/// What clients see of a plugin.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PluginInfo {
    pub manifest: Manifest,
    pub enabled: bool,
    pub builtin: bool,
    /// Settings with secrets masked; missing required settings listed.
    pub settings: HashMap<String, String>,
    pub missing: Vec<String>,
    pub failures: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginError {
    NotFound,
    Disabled,
    /// The plugin returned an error.
    Plugin(String),
    /// The plugin trapped, ran out of time or memory.
    Trap(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::NotFound => write!(f, "no such plugin"),
            PluginError::Disabled => write!(f, "plugin is disabled"),
            PluginError::Plugin(m) => write!(f, "{m}"),
            PluginError::Trap(m) => write!(f, "plugin crashed: {m}"),
        }
    }
}

impl std::error::Error for PluginError {}

pub const SECRET_MASK: &str = "••••••••";

/// The first-party plugins shipped inside the server (`scripts/build-plugins.sh` refreshes them).
pub fn builtins() -> Vec<(Manifest, &'static [u8])> {
    [
        (include_str!("../builtin/tmdb.toml"), &include_bytes!("../builtin/tmdb.wasm")[..]),
        (include_str!("../builtin/musicbrainz.toml"), &include_bytes!("../builtin/musicbrainz.wasm")[..]),
    ]
    .into_iter()
    .filter_map(|(m, w)| {
        Manifest::parse(m).map_err(|e| tracing::error!("built-in plugin manifest: {e}")).ok().map(|m| (m, w))
    })
    .collect()
}

pub struct PluginHost {
    /// Epoch ticks (100 ms each) a call may run.
    deadline: std::sync::atomic::AtomicU64,
    engine: Engine,
    plugins: RwLock<HashMap<String, Arc<Loaded>>>,
    kv: Arc<dyn KvStore>,
    rt: tokio::runtime::Handle,
}

impl PluginHost {
    pub fn new(kv: Arc<dyn KvStore>, rt: tokio::runtime::Handle) -> anyhow::Result<Arc<PluginHost>> {
        let mut cfg = Config::new();
        cfg.wasm_component_model(true);
        cfg.epoch_interruption(true);
        let engine = Engine::new(&cfg)?;
        let e2 = engine.clone();
        std::thread::Builder::new().name("corrent-plugin-epoch".into()).spawn(move || {
            loop {
                std::thread::sleep(TICK);
                e2.increment_epoch();
            }
        })?;
        Ok(Arc::new(PluginHost {
            deadline: std::sync::atomic::AtomicU64::new(DEADLINE_TICKS),
            engine,
            plugins: RwLock::new(HashMap::new()),
            kv,
            rt,
        }))
    }

    /// Changes the per-call time limit.
    pub fn set_deadline(&self, d: Duration) {
        self.deadline.store((d.as_millis() / TICK.as_millis()).max(1) as u64, Ordering::Relaxed);
    }

    /// Compiles and registers a plugin (replacing one with the same id).
    pub fn load(
        &self,
        manifest: Manifest,
        wasm: &[u8],
        settings: HashMap<String, String>,
        enabled: bool,
        builtin: bool,
    ) -> Result<(), String> {
        let component = Component::new(&self.engine, wasm).map_err(|e| format!("not a valid plugin: {e}"))?;
        let ctx = Arc::new(PluginCtx {
            manifest: manifest.clone(),
            settings: RwLock::new(settings),
            kv: self.kv.clone(),
            rt: self.rt.clone(),
            bucket: Mutex::new(Bucket { tokens: 10.0, last: Instant::now() }),
        });
        let loaded = Arc::new(Loaded { ctx, component, enabled, builtin, failures: AtomicU32::new(0) });
        self.plugins.write().unwrap_or_else(|e| e.into_inner()).insert(manifest.id.clone(), loaded);
        Ok(())
    }

    pub fn unload(&self, id: &str) -> bool {
        self.plugins.write().unwrap_or_else(|e| e.into_inner()).remove(id).is_some()
    }

    fn get(&self, id: &str) -> Result<Arc<Loaded>, PluginError> {
        self.plugins.read().unwrap_or_else(|e| e.into_inner()).get(id).cloned().ok_or(PluginError::NotFound)
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        let mut v: Vec<PluginInfo> = self
            .plugins
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|l| {
                let settings = l.ctx.settings.read().unwrap_or_else(|e| e.into_inner());
                let shown = settings
                    .iter()
                    .map(|(k, v)| {
                        let secret = l.ctx.manifest.settings.iter().any(|d| d.key == *k && d.secret);
                        (k.clone(), if secret && !v.is_empty() { SECRET_MASK.to_string() } else { v.clone() })
                    })
                    .collect();
                let missing = l
                    .ctx
                    .manifest
                    .settings
                    .iter()
                    .filter(|d| d.required && settings.get(&d.key).is_none_or(|v| v.is_empty()) && d.default.is_none())
                    .map(|d| d.key.clone())
                    .collect();
                PluginInfo {
                    manifest: l.ctx.manifest.clone(),
                    enabled: l.enabled,
                    builtin: l.builtin,
                    settings: shown,
                    missing,
                    failures: l.failures.load(Ordering::Relaxed),
                }
            })
            .collect();
        v.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        v
    }

    /// The stored settings with secrets (for persisting them; never send these to clients).
    pub fn settings_raw(&self, id: &str) -> Option<HashMap<String, String>> {
        self.get(id).ok().map(|l| l.ctx.settings.read().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Replaces settings (masked secrets sent back keep their stored value).
    pub fn set_settings(&self, id: &str, new: HashMap<String, String>) -> Result<HashMap<String, String>, PluginError> {
        let l = self.get(id)?;
        let mut s = l.ctx.settings.write().unwrap_or_else(|e| e.into_inner());
        let mut merged = HashMap::new();
        for d in &l.ctx.manifest.settings {
            if let Some(v) = new.get(&d.key) {
                let v = if v == SECRET_MASK { s.get(&d.key).cloned().unwrap_or_default() } else { v.clone() };
                merged.insert(d.key.clone(), v);
            }
        }
        *s = merged.clone();
        Ok(merged)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), PluginError> {
        let l = self.get(id)?;
        let new = Arc::new(Loaded {
            ctx: l.ctx.clone(),
            component: l.component.clone(),
            enabled,
            builtin: l.builtin,
            failures: AtomicU32::new(0),
        });
        self.plugins.write().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), new);
        Ok(())
    }

    /// Enabled plugins of a kind whose required settings are filled in.
    pub fn ready(&self, kind: Kind) -> Vec<String> {
        self.list()
            .into_iter()
            .filter(|p| p.enabled && p.missing.is_empty() && p.manifest.kinds.contains(&kind))
            .map(|p| p.manifest.id)
            .collect()
    }

    fn store(&self, l: &Loaded) -> Store<HostState> {
        let wasi = WasiCtxBuilder::new().build();
        let state = HostState {
            wasi,
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).instances(16).tables(64).build(),
            plugin: l.ctx.clone(),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(self.deadline.load(Ordering::Relaxed));
        store.epoch_deadline_trap();
        store
    }

    fn linker(&self) -> Result<Linker<HostState>, PluginError> {
        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| PluginError::Trap(e.to_string()))?;
        bindings::corrent::plugin::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)
            .map_err(|e| PluginError::Trap(e.to_string()))?;
        bindings::corrent::plugin::types::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)
            .map_err(|e| PluginError::Trap(e.to_string()))?;
        Ok(linker)
    }

    /// Runs `f` against a fresh instance on a blocking thread, counting traps.
    async fn call<R: Send + 'static>(
        self: &Arc<Self>,
        id: &str,
        f: impl FnOnce(&PluginHost, &Loaded) -> Result<Result<R, String>, wasmtime::Error> + Send + 'static,
    ) -> Result<R, PluginError> {
        let l = self.get(id)?;
        if !l.enabled {
            return Err(PluginError::Disabled);
        }
        let me = self.clone();
        let l2 = l.clone();
        let r = tokio::task::spawn_blocking(move || f(&me, &l2)).await.map_err(|e| PluginError::Trap(e.to_string()))?;
        match r {
            Ok(Ok(v)) => {
                l.failures.store(0, Ordering::Relaxed);
                Ok(v)
            }
            Ok(Err(e)) => Err(PluginError::Plugin(e)),
            Err(e) => {
                let n = l.failures.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!("plugin {id} failed ({n}/{BREAKER}): {e:#}");
                if n >= BREAKER {
                    tracing::error!("plugin {id} is disabled after {n} failures");
                    let _ = self.set_enabled(id, false);
                }
                Err(PluginError::Trap(format!("{e:#}")))
            }
        }
    }

    pub async fn metadata_search(self: &Arc<Self>, id: &str, q: SearchQuery) -> Result<Vec<Candidate>, PluginError> {
        self.call(id, move |h, l| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(
                &mut store,
                &l.component,
                &h.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))?,
            )?;
            p.corrent_plugin_metadata().call_search(&mut store, &q)
        })
        .await
    }

    pub async fn metadata_details(
        self: &Arc<Self>,
        id: &str,
        kind: MediaKind,
        item: String,
        language: String,
    ) -> Result<ItemDetails, PluginError> {
        self.call(id, move |h, l| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(
                &mut store,
                &l.component,
                &h.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))?,
            )?;
            p.corrent_plugin_metadata().call_details(&mut store, kind, &item, &language)
        })
        .await
    }

    pub async fn metadata_episodes(
        self: &Arc<Self>,
        id: &str,
        show: String,
        season: u32,
        language: String,
    ) -> Result<Vec<Episode>, PluginError> {
        self.call(id, move |h, l| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(
                &mut store,
                &l.component,
                &h.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))?,
            )?;
            p.corrent_plugin_metadata().call_episodes(&mut store, &show, season, &language)
        })
        .await
    }

    pub async fn metadata_kinds(self: &Arc<Self>, id: &str) -> Result<Vec<MediaKind>, PluginError> {
        self.call(id, move |h, l| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(
                &mut store,
                &l.component,
                &h.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))?,
            )?;
            p.corrent_plugin_metadata().call_kinds(&mut store).map(Ok)
        })
        .await
    }

    pub async fn indexer_search(self: &Arc<Self>, id: &str, query: String) -> Result<Vec<Release>, PluginError> {
        self.call(id, move |h, l| {
            let mut store = h.store(l);
            let p = indexer_bindings::IndexerPlugin::instantiate(
                &mut store,
                &l.component,
                &h.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))?,
            )?;
            p.corrent_plugin_indexer().call_search(&mut store, &query)
        })
        .await
    }

    pub async fn notify(self: &Arc<Self>, id: &str, title: String, body: String) -> Result<(), PluginError> {
        self.call(id, move |h, l| {
            let mut store = h.store(l);
            let p = notifier_bindings::NotifierPlugin::instantiate(
                &mut store,
                &l.component,
                &h.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))?,
            )?;
            p.corrent_plugin_notifier().call_notify(&mut store, &title, &body)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Arc<PluginHost> {
        let h = PluginHost::new(Arc::new(MemoryKv::default()), tokio::runtime::Handle::current()).unwrap();
        h.set_deadline(Duration::from_secs(2));
        let m = Manifest::parse(include_str!("../builtin/example.toml")).unwrap();
        h.load(m, include_bytes!("../builtin/example.wasm"), HashMap::new(), true, false).unwrap();
        h
    }

    fn q(title: &str) -> SearchQuery {
        SearchQuery {
            kind: MediaKind::Movie,
            title: title.into(),
            year: Some(2010),
            language: "en".into(),
            ids: vec![],
            artist: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn calls_settings_and_storage() {
        let h = host();
        assert_eq!(h.metadata_kinds("example").await.unwrap(), vec![MediaKind::Movie, MediaKind::Show]);
        let c = h.metadata_search("example", q("Sintel")).await.unwrap();
        assert_eq!((c[0].id.as_str(), c[0].overview.as_deref()), ("ex-sintel", Some("call 1")));
        let c = h.metadata_search("example", q("Sintel")).await.unwrap();
        assert_eq!(c[0].overview.as_deref(), Some("call 2"), "kv survives between calls");
        let d = h.metadata_details("example", MediaKind::Movie, "ex-1".into(), "tr".into()).await.unwrap();
        assert_eq!(d.overview.as_deref(), Some("Hello (tr)"), "setting default");
        h.set_settings("example", HashMap::from([("greeting".to_string(), "Merhaba".to_string())])).unwrap();
        let d = h.metadata_details("example", MediaKind::Movie, "ex-1".into(), "tr".into()).await.unwrap();
        assert_eq!(d.overview.as_deref(), Some("Merhaba (tr)"));
        assert_eq!(h.metadata_episodes("example", "s".into(), 2, "en".into()).await.unwrap().len(), 3);
        assert_eq!(
            h.metadata_search("example", q("fail")).await.unwrap_err(),
            PluginError::Plugin("no such title".into())
        );
        assert_eq!(h.ready(Kind::Metadata), vec!["example".to_string()]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sandbox_contains_bad_plugins() {
        let h = host();
        // Undeclared host and the server's own network are refused by the host.
        let e = h.metadata_search("example", q("undeclared")).await.unwrap_err();
        assert!(matches!(&e, PluginError::Plugin(m) if m.contains("not declared")), "{e:?}");
        let e = h.metadata_search("example", q("local")).await.unwrap_err();
        assert!(matches!(&e, PluginError::Plugin(m) if m.contains("own network")), "{e:?}");
        // An endless loop hits the deadline; a huge allocation hits the memory cap.
        let t = Instant::now();
        assert!(matches!(h.metadata_search("example", q("loop")).await, Err(PluginError::Trap(_))));
        assert!(t.elapsed() < Duration::from_secs(10), "stopped by the deadline");
        assert!(matches!(h.metadata_search("example", q("alloc")).await, Err(PluginError::Trap(_))));
        // A third trap disables the plugin.
        assert!(matches!(h.metadata_search("example", q("loop")).await, Err(PluginError::Trap(_))));
        assert_eq!(h.metadata_search("example", q("Sintel")).await.unwrap_err(), PluginError::Disabled);
        assert!(h.ready(Kind::Metadata).is_empty());
    }

    #[test]
    fn builtins_load() {
        let b = builtins();
        assert_eq!(b.iter().map(|(m, _)| m.id.as_str()).collect::<Vec<_>>(), vec!["tmdb", "musicbrainz"]);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let h = PluginHost::new(Arc::new(MemoryKv::default()), rt.handle().clone()).unwrap();
        for (m, w) in b {
            h.load(m, w, HashMap::new(), true, true).unwrap();
        }
        let tmdb = h.list().into_iter().find(|p| p.manifest.id == "tmdb").unwrap();
        assert_eq!(tmdb.missing, vec!["api_key".to_string()], "needs a key before it is used");
        assert_eq!(h.ready(Kind::Metadata), vec!["musicbrainz".to_string()]);
    }

    /// Talks to musicbrainz.org (run by hand: `cargo test -p corrent-plugins -- --ignored`).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_musicbrainz() {
        let h = PluginHost::new(Arc::new(MemoryKv::default()), tokio::runtime::Handle::current()).unwrap();
        let (m, w) = builtins().into_iter().find(|(m, _)| m.id == "musicbrainz").unwrap();
        h.load(m, w, HashMap::new(), true, true).unwrap();
        let q = SearchQuery {
            kind: MediaKind::Album,
            title: "Abbey Road".into(),
            year: None,
            language: "en".into(),
            ids: vec![],
            artist: Some("The Beatles".into()),
        };
        let found = h.metadata_search("musicbrainz", q).await.unwrap();
        assert!(found.iter().any(|c| c.title == "Abbey Road" && c.year == Some(1969)), "{found:?}");
        let d = h.metadata_details("musicbrainz", MediaKind::Album, found[0].id.clone(), "en".into()).await.unwrap();
        assert!(d.credits.iter().any(|c| c.name.contains("Beatles")), "{d:?}");
        assert!(d.artwork[0].url.contains("coverartarchive.org"));
    }
}
