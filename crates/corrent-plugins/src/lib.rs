//! Corrent's plugin host: WebAssembly components (wasmtime, component model) implementing the
//! worlds in `wit/corrent.wit`.
//!
//! Plugins are sandboxed: no filesystem, no sockets, no environment. They reach the outside
//! only through the host API — HTTP to hosts their manifest declares (public addresses only
//! unless the admin allowed the local network; connections are pinned to the checked address,
//! redirects re-checked), a per-plugin key-value store and their settings. Each call runs in a
//! fresh instance with a 64 MiB memory cap and a 30 s deadline; three traps disable a plugin.

pub mod manifest;
pub mod process;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub use manifest::{Kind, Manifest, Runtime, SettingDef};

// Records derive serde too: process plugins exchange the same shapes as JSON.
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "metadata-provider",
        additional_derives: [serde::Serialize, serde::Deserialize],
    });
}
macro_rules! world_bindings {
    ($m:ident, $world:literal) => {
        mod $m {
            wasmtime::component::bindgen!({
                path: "wit",
                world: $world,
                additional_derives: [serde::Serialize, serde::Deserialize],
                with: {
                    "corrent:plugin/host": super::bindings::corrent::plugin::host,
                    "corrent:plugin/types": super::bindings::corrent::plugin::types,
                },
            });
        }
    };
}
world_bindings!(indexer_bindings, "indexer-plugin");
world_bindings!(notifier_bindings, "notifier-plugin");
world_bindings!(subtitle_bindings, "subtitle-provider");
world_bindings!(parser_bindings, "name-parser");
world_bindings!(task_bindings, "scheduled-task");
world_bindings!(event_bindings, "event-handler");

pub use event_bindings::corrent::plugin::library::LibraryItem;
pub use event_bindings::exports::corrent::plugin::events::Event;

/// The server's libraries as plugins may read them (implemented by the server). Called on a
/// blocking thread.
pub trait LibraryAccess: Send + Sync {
    fn search(&self, query: &str, limit: u32) -> Result<Vec<LibraryItem>, String>;
    fn item(&self, id: i64) -> Result<Option<LibraryItem>, String>;
}

pub use bindings::corrent::plugin::types::{HttpRequest, HttpResponse};
pub use bindings::exports::corrent::plugin::metadata::{
    Artwork, Candidate, Credit, Episode, ItemDetails, MediaKind, SearchQuery,
};
pub use indexer_bindings::exports::corrent::plugin::indexer::Release;
pub use parser_bindings::exports::corrent::plugin::parser::ParsedName;
pub use subtitle_bindings::exports::corrent::plugin::subtitles::{Subtitle, SubtitleFile, SubtitleQuery};

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
    /// Only for plugins whose manifest asks for `library`.
    library: Option<Arc<dyn LibraryAccess>>,
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

impl event_bindings::corrent::plugin::library::Host for HostState {
    fn search(&mut self, query: String, limit: u32) -> Result<Vec<LibraryItem>, String> {
        let l = self.library.clone().ok_or("library access isn't in this plugin's permissions")?;
        l.search(&query.chars().take(200).collect::<String>(), limit.min(100))
    }

    fn item(&mut self, id: i64) -> Result<Option<LibraryItem>, String> {
        let l = self.library.clone().ok_or("library access isn't in this plugin's permissions")?;
        l.item(id)
    }
}

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

enum Code {
    Wasm(Component),
    Process(Arc<process::ProcessPlugin>),
}

struct Loaded {
    ctx: Arc<PluginCtx>,
    code: Arc<Code>,
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
        (include_str!("../builtin/tvdb.toml"), &include_bytes!("../builtin/tvdb.wasm")[..]),
        (include_str!("../builtin/fanart.toml"), &include_bytes!("../builtin/fanart.wasm")[..]),
        (include_str!("../builtin/opensubtitles.toml"), &include_bytes!("../builtin/opensubtitles.wasm")[..]),
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
    library: RwLock<Option<Arc<dyn LibraryAccess>>>,
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
            library: RwLock::new(None),
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
        self.insert(manifest, Code::Wasm(component), settings, enabled, builtin);
        Ok(())
    }

    /// Registers a process plugin living in `dir` (its manifest names the program).
    pub fn load_process(
        &self,
        manifest: Manifest,
        dir: std::path::PathBuf,
        settings: HashMap<String, String>,
        enabled: bool,
    ) -> Result<(), String> {
        if manifest.runtime != Runtime::Process {
            return Err("not a process plugin".into());
        }
        let program = dir.join(manifest.command.first().ok_or("no command")?);
        if !program.is_file() {
            return Err(format!("{} is missing", program.display()));
        }
        let p = process::ProcessPlugin::new(&manifest.id, dir, manifest.command.clone());
        self.insert(manifest, Code::Process(Arc::new(p)), settings, enabled, false);
        Ok(())
    }

    fn insert(&self, manifest: Manifest, code: Code, settings: HashMap<String, String>, enabled: bool, builtin: bool) {
        let ctx = Arc::new(PluginCtx {
            manifest: manifest.clone(),
            settings: RwLock::new(settings),
            kv: self.kv.clone(),
            rt: self.rt.clone(),
            bucket: Mutex::new(Bucket { tokens: 10.0, last: Instant::now() }),
        });
        let loaded = Arc::new(Loaded { ctx, code: Arc::new(code), enabled, builtin, failures: AtomicU32::new(0) });
        self.plugins.write().unwrap_or_else(|e| e.into_inner()).insert(manifest.id.clone(), loaded);
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
        if let Code::Process(p) = &*l.code {
            let p = p.clone();
            self.rt.spawn(async move { p.reconfigure().await });
        }
        Ok(merged)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), PluginError> {
        let l = self.get(id)?;
        let new = Arc::new(Loaded {
            ctx: l.ctx.clone(),
            code: l.code.clone(),
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
            library: if l.ctx.manifest.permissions.library {
                self.library.read().unwrap_or_else(|e| e.into_inner()).clone()
            } else {
                None
            },
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
        event_bindings::corrent::plugin::library::add_to_linker::<_, HasSelf<_>>(&mut linker, |s| s)
            .map_err(|e| PluginError::Trap(e.to_string()))?;
        Ok(linker)
    }

    fn count_failure(&self, id: &str, l: &Loaded, e: &str) {
        let n = l.failures.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!("plugin {id} failed ({n}/{BREAKER}): {e}");
        if n >= BREAKER {
            tracing::error!("plugin {id} is disabled after {n} failures");
            let _ = self.set_enabled(id, false);
        }
    }

    /// Runs `f` against a fresh instance of a WebAssembly plugin on a blocking thread,
    /// counting traps.
    async fn call<R: Send + 'static>(
        self: &Arc<Self>,
        id: &str,
        f: impl FnOnce(&PluginHost, &Loaded, &Component) -> Result<Result<R, String>, wasmtime::Error> + Send + 'static,
    ) -> Result<R, PluginError> {
        let l = self.get(id)?;
        if !l.enabled {
            return Err(PluginError::Disabled);
        }
        let me = self.clone();
        let l2 = l.clone();
        let r = tokio::task::spawn_blocking(move || match &*l2.code {
            Code::Wasm(c) => f(&me, &l2, c),
            Code::Process(_) => Err(wasmtime::Error::msg("not a WebAssembly plugin")),
        })
        .await
        .map_err(|e| PluginError::Trap(e.to_string()))?;
        match r {
            Ok(Ok(v)) => {
                l.failures.store(0, Ordering::Relaxed);
                Ok(v)
            }
            Ok(Err(e)) => Err(PluginError::Plugin(e)),
            Err(e) => {
                self.count_failure(id, &l, &format!("{e:#}"));
                Err(PluginError::Trap(format!("{e:#}")))
            }
        }
    }

    fn is_process(&self, id: &str) -> bool {
        self.get(id).is_ok_and(|l| matches!(&*l.code, Code::Process(_)))
    }

    /// A JSON-RPC call to a process plugin.
    async fn rpc<R: serde::de::DeserializeOwned>(
        self: &Arc<Self>,
        id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<R, PluginError> {
        let l = self.get(id)?;
        if !l.enabled {
            return Err(PluginError::Disabled);
        }
        let Code::Process(p) = &*l.code else { return Err(PluginError::NotFound) };
        let mut settings: HashMap<String, String> =
            l.ctx.manifest.settings.iter().filter_map(|d| d.default.clone().map(|v| (d.key.clone(), v))).collect();
        settings.extend(l.ctx.settings.read().unwrap_or_else(|e| e.into_inner()).clone());
        let deadline = TICK * self.deadline.load(Ordering::Relaxed) as u32;
        match p.call(&settings, method, params, deadline).await {
            Ok(v) => {
                l.failures.store(0, Ordering::Relaxed);
                serde_json::from_value(v)
                    .map_err(|e| PluginError::Plugin(format!("unexpected answer to {method}: {e}")))
            }
            Err(process::CallError::Plugin(m)) => Err(PluginError::Plugin(m)),
            Err(process::CallError::Broken(m)) => {
                self.count_failure(id, &l, &m);
                Err(PluginError::Trap(m))
            }
        }
    }

    pub async fn metadata_search(self: &Arc<Self>, id: &str, q: SearchQuery) -> Result<Vec<Candidate>, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "metadata.search", serde_json::json!({ "query": q })).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(&mut store, c, &h.linker_or_err()?)?;
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
        if self.is_process(id) {
            let params = serde_json::json!({ "kind": kind, "id": item, "language": language });
            return self.rpc(id, "metadata.details", params).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(&mut store, c, &h.linker_or_err()?)?;
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
        if self.is_process(id) {
            let params = serde_json::json!({ "show": show, "season": season, "language": language });
            return self.rpc(id, "metadata.episodes", params).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_metadata().call_episodes(&mut store, &show, season, &language)
        })
        .await
    }

    pub async fn metadata_kinds(self: &Arc<Self>, id: &str) -> Result<Vec<MediaKind>, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "metadata.kinds", serde_json::json!({})).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = bindings::MetadataProvider::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_metadata().call_kinds(&mut store).map(Ok)
        })
        .await
    }

    pub async fn indexer_search(self: &Arc<Self>, id: &str, query: String) -> Result<Vec<Release>, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "indexer.search", serde_json::json!({ "query": query })).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = indexer_bindings::IndexerPlugin::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_indexer().call_search(&mut store, &query)
        })
        .await
    }

    pub async fn notify(self: &Arc<Self>, id: &str, title: String, body: String) -> Result<(), PluginError> {
        if self.is_process(id) {
            let _: serde_json::Value =
                self.rpc(id, "notifier.notify", serde_json::json!({ "title": title, "body": body })).await?;
            return Ok(());
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = notifier_bindings::NotifierPlugin::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_notifier().call_notify(&mut store, &title, &body)
        })
        .await
    }

    pub async fn subtitles_search(self: &Arc<Self>, id: &str, q: SubtitleQuery) -> Result<Vec<Subtitle>, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "subtitles.search", serde_json::json!({ "query": q })).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = subtitle_bindings::SubtitleProvider::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_subtitles().call_search(&mut store, &q)
        })
        .await
    }

    pub async fn subtitles_download(self: &Arc<Self>, id: &str, sub: String) -> Result<SubtitleFile, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "subtitles.download", serde_json::json!({ "id": sub })).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = subtitle_bindings::SubtitleProvider::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_subtitles().call_download(&mut store, &sub)
        })
        .await
    }

    /// A name parser's reading of `name` (`None`: no opinion).
    pub async fn parse_name(self: &Arc<Self>, id: &str, name: String) -> Result<Option<ParsedName>, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "parser.parse", serde_json::json!({ "name": name })).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = parser_bindings::NameParser::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_parser().call_parse(&mut store, &name).map(Ok)
        })
        .await
    }

    /// What plugins with the `library` permission read (set by the server).
    pub fn set_library(&self, l: Arc<dyn LibraryAccess>) {
        *self.library.write().unwrap_or_else(|e| e.into_inner()) = Some(l);
    }

    /// Tells an event plugin about something that happened.
    pub async fn on_event(self: &Arc<Self>, id: &str, e: Event) -> Result<(), PluginError> {
        if self.is_process(id) {
            let _: serde_json::Value = self.rpc(id, "events.on_event", serde_json::json!({ "event": e })).await?;
            return Ok(());
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = event_bindings::EventHandler::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_events().call_on_event(&mut store, &e).map(Ok)
        })
        .await
    }

    /// One run of a task plugin.
    pub async fn run_task(self: &Arc<Self>, id: &str) -> Result<String, PluginError> {
        if self.is_process(id) {
            return self.rpc(id, "task.run", serde_json::json!({})).await;
        }
        self.call(id, move |h, l, c| {
            let mut store = h.store(l);
            let p = task_bindings::ScheduledTask::instantiate(&mut store, c, &h.linker_or_err()?)?;
            p.corrent_plugin_task().call_run(&mut store)
        })
        .await
    }

    fn linker_or_err(&self) -> Result<Linker<HostState>, wasmtime::Error> {
        self.linker().map_err(|e| wasmtime::Error::msg(e.to_string()))
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
        assert_eq!(
            b.iter().map(|(m, _)| m.id.as_str()).collect::<Vec<_>>(),
            vec!["tmdb", "musicbrainz", "tvdb", "fanart", "opensubtitles"]
        );
        assert!(b.iter().find(|(m, _)| m.id == "fanart").is_some_and(|(m, _)| m.supplements));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let h = PluginHost::new(Arc::new(MemoryKv::default()), rt.handle().clone()).unwrap();
        for (m, w) in b {
            h.load(m, w, HashMap::new(), true, true).unwrap();
        }
        let tmdb = h.list().into_iter().find(|p| p.manifest.id == "tmdb").unwrap();
        assert_eq!(tmdb.missing, vec!["api_key".to_string()], "needs a key before it is used");
        assert_eq!(h.ready(Kind::Metadata), vec!["musicbrainz".to_string()], "the others need keys");
        assert!(h.ready(Kind::Subtitles).is_empty());
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

    #[tokio::test(flavor = "multi_thread")]
    async fn subtitle_parser_and_task_plugins() {
        let h = host();
        for (toml, wasm) in [
            (
                include_str!("../builtin/example-subtitles.toml"),
                &include_bytes!("../builtin/example-subtitles.wasm")[..],
            ),
            (include_str!("../builtin/example-parser.toml"), &include_bytes!("../builtin/example-parser.wasm")[..]),
            (include_str!("../builtin/example-task.toml"), &include_bytes!("../builtin/example-task.wasm")[..]),
        ] {
            h.load(Manifest::parse(toml).unwrap(), wasm, HashMap::new(), true, false).unwrap();
        }
        let q = SubtitleQuery {
            title: "Sintel".into(),
            year: Some(2010),
            season: None,
            episode: None,
            languages: vec!["tr".into(), "en".into()],
            ids: vec![],
            file_hash: Some("8e245d9679d31e12".into()),
            file_size: Some(1000),
        };
        let found = h.subtitles_search("example-subtitles", q).await.unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[0].exact);
        let f = h.subtitles_download("example-subtitles", found[0].id.clone()).await.unwrap();
        assert!(String::from_utf8(f.data).unwrap().contains("Sintel [tr]"));
        let p = h.parse_name("example-parser", "Home Movie {2019} #3".into()).await.unwrap().unwrap();
        assert_eq!((p.title.as_str(), p.year, p.episode), ("Home Movie", Some(2019), Some(3)));
        assert!(h.parse_name("example-parser", "Some.Movie.2010".into()).await.unwrap().is_none());
        assert_eq!(h.run_task("example-task").await.unwrap(), "run 1");
        assert_eq!(h.run_task("example-task").await.unwrap(), "run 2");
        assert_eq!(h.ready(Kind::Task), vec!["example-task".to_string()]);
    }

    /// A process plugin (a Python script) answers the same calls over JSON-RPC; a crash counts
    /// toward the breaker and the process is restarted.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn process_plugins_speak_json_rpc() {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("corrent-process-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("plugin.py");
        std::fs::write(
            &script,
            r#"#!/usr/bin/env python3
import json, sys
settings = {}
for line in sys.stdin:
    req = json.loads(line)
    m, p = req["method"], req.get("params", {})
    if m == "configure":
        settings = p["settings"]; res = None
    elif m == "metadata.kinds":
        res = ["Movie"]
    elif m == "metadata.search":
        if p["query"]["title"] == "crash":
            sys.exit(3)
        if p["query"]["title"] == "fail":
            print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "error": {"code": 1, "message": "nope"}}), flush=True)
            continue
        res = [{"id": "py-1", "title": settings.get("greeting", "?") + " " + p["query"]["title"], "year": 2010, "overview": None, "poster_url": None}]
    elif m == "task.run":
        res = "ran"
    else:
        res = None
    print("stray output", flush=True)
    print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": res}), flush=True)
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let m = Manifest::parse(
            r#"id = "py"
name = "Python"
version = "1"
host_api = "1"
kinds = ["metadata", "task"]
runtime = "process"
command = ["plugin.py"]
[[settings]]
key = "greeting"
default = "Hi"
"#,
        )
        .unwrap();
        assert!(Manifest::parse("command = [\"../evil\"]\nid = \"x\"\nname = \"x\"\nversion = \"1\"\nhost_api = \"1\"\nkinds = [\"task\"]\nruntime = \"process\"").is_err());
        let h = PluginHost::new(Arc::new(MemoryKv::default()), tokio::runtime::Handle::current()).unwrap();
        h.set_deadline(Duration::from_secs(5));
        h.load_process(m, dir.clone(), HashMap::new(), true).unwrap();
        assert_eq!(h.metadata_kinds("py").await.unwrap(), vec![MediaKind::Movie]);
        let c = h.metadata_search("py", q("Sintel")).await.unwrap();
        assert_eq!(c[0].title, "Hi Sintel", "the setting's default reached the process");
        h.set_settings("py", HashMap::from([("greeting".to_string(), "Merhaba".to_string())])).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let c = h.metadata_search("py", q("Sintel")).await.unwrap();
        assert_eq!(c[0].title, "Merhaba Sintel", "restarted with the new settings");
        assert_eq!(h.metadata_search("py", q("fail")).await.unwrap_err(), PluginError::Plugin("nope".into()));
        assert!(matches!(h.metadata_search("py", q("crash")).await.unwrap_err(), PluginError::Trap(_)));
        assert_eq!(h.run_task("py").await.unwrap(), "ran", "restarted after the crash");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
