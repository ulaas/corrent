//! Runtime-loaded libmpv.
//!
//! libmpv is opened with `dlopen` (libloading) instead of being linked, so Corrent builds on
//! machines without mpv development files and still starts when mpv is missing: playback,
//! probing and transcoding then report [`Error::NotAvailable`] instead of the process failing.

pub mod encode;
mod ffi;
pub mod probe;
pub mod render;

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub use probe::{Chapter, ProbeResult, Track, TrackKind, probe};

/// The client API major version this crate is written against.
pub const API_MAJOR: u64 = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// No usable libmpv was found (message lists what was tried).
    NotAvailable(String),
    /// An mpv call failed: the function or property, and mpv's error text.
    Mpv {
        what: String,
        code: i32,
        message: String,
    },
    /// A string argument contained a NUL byte.
    Nul,
    Timeout(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotAvailable(m) => write!(f, "libmpv is not available: {m}"),
            Error::Mpv { what, message, .. } => write!(f, "mpv: {what}: {message}"),
            Error::Nul => write!(f, "string contains a NUL byte"),
            Error::Timeout(m) => write!(f, "mpv timed out: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The `corrent-mpvw` worker next to the running executable (or one folder up, for test
/// binaries in `target/*/deps`).
pub fn worker_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) { "corrent-mpvw.exe" } else { "corrent-mpvw" };
    let dir = exe.parent()?;
    [dir.join(name), dir.parent()?.join(name)].into_iter().find(|p| p.is_file())
}

/// A loaded libmpv: the library stays mapped for the life of the process.
pub struct Library {
    api: ffi::Api,
    path: PathBuf,
    version: u64,
    _lib: libloading::Library,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library").field("path", &self.path).field("version", &self.version_string()).finish()
    }
}

/// File names tried, in order, when no explicit path is configured.
pub fn candidate_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"]
    } else if cfg!(target_os = "macos") {
        &[
            "libmpv.2.dylib",
            "libmpv.dylib",
            "/opt/homebrew/lib/libmpv.2.dylib",
            "/opt/homebrew/lib/libmpv.dylib",
            "/usr/local/lib/libmpv.2.dylib",
            "/usr/local/lib/libmpv.dylib",
        ]
    } else {
        &["libmpv.so.2", "libmpv.so"]
    }
}

static GLOBAL: OnceLock<Mutex<Option<Arc<Library>>>> = OnceLock::new();
static CONFIGURED: OnceLock<Option<PathBuf>> = OnceLock::new();

impl Library {
    /// Opens libmpv from `path`, or from the platform's search list when `None`.
    pub fn open(path: Option<&Path>) -> Result<Library> {
        let tries: Vec<PathBuf> = match path {
            Some(p) => vec![p.to_path_buf()],
            None => candidate_names().iter().map(PathBuf::from).collect(),
        };
        let mut errors = Vec::new();
        for p in tries {
            // SAFETY: loading libmpv runs its library constructors, which have no preconditions.
            let lib = match unsafe { libloading::Library::new(&p) } {
                Ok(l) => l,
                Err(e) => {
                    errors.push(format!("{}: {e}", p.display()));
                    continue;
                }
            };
            // SAFETY: the symbols are libmpv's; signatures follow its public headers.
            let api = match unsafe { ffi::Api::load(&lib) } {
                Ok(a) => a,
                Err(e) => {
                    errors.push(format!("{}: {e}", p.display()));
                    continue;
                }
            };
            // SAFETY: no arguments, returns a constant.
            let version = unsafe { (api.client_api_version)() } as u64;
            if version >> 16 != API_MAJOR {
                errors.push(format!(
                    "{}: client API {}.{} (need {API_MAJOR}.x)",
                    p.display(),
                    version >> 16,
                    version & 0xffff
                ));
                continue;
            }
            return Ok(Library { api, path: p, version, _lib: lib });
        }
        Err(Error::NotAvailable(errors.join("; ")))
    }

    /// Sets the library path used by [`Library::get`]. Only the first call has an effect.
    pub fn configure(path: Option<PathBuf>) {
        let _ = CONFIGURED.set(path);
    }

    /// The process-wide library, loaded on first use. A failed load is retried on the next call
    /// (the user may install mpv while Corrent runs).
    pub fn get() -> Result<Arc<Library>> {
        let slot = GLOBAL.get_or_init(|| Mutex::new(None));
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = guard.as_ref() {
            return Ok(l.clone());
        }
        let configured = CONFIGURED.get().and_then(|p| p.as_deref());
        let lib = Arc::new(Library::open(configured)?);
        tracing::info!("loaded {} (client API {})", lib.path.display(), lib.version_string());
        *guard = Some(lib.clone());
        Ok(lib)
    }

    /// Whether libmpv can be loaded (loads it as a side effect).
    pub fn available() -> bool {
        Library::get().is_ok()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version_string(&self) -> String {
        format!("{}.{}", self.version >> 16, self.version & 0xffff)
    }

    pub(crate) fn error_text(&self, code: c_int) -> String {
        // SAFETY: mpv_error_string returns a static string for any code.
        unsafe { cstr_to_string((self.api.error_string)(code)) }
    }
}

/// A value of mpv's node type, copied into Rust memory.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Node {
    None,
    Flag(bool),
    Int(i64),
    Double(f64),
    String(String),
    Array(Vec<Node>),
    Map(BTreeMap<String, Node>),
    Bytes(Vec<u8>),
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(m) => m.get(key),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Node::Int(i) => Some(*i),
            Node::Double(d) => Some(*d as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Node::Double(d) => Some(*d),
            Node::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Node::Flag(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_array(&self) -> &[Node] {
        match self {
            Node::Array(a) => a,
            _ => &[],
        }
    }

    /// # Safety
    /// `n` must point to a valid, initialized mpv_node.
    unsafe fn from_raw(n: *const ffi::mpv_node) -> Node {
        // SAFETY: guaranteed by the caller; each union field is read only for its format.
        unsafe {
            let n = &*n;
            match n.format {
                ffi::MPV_FORMAT_STRING | ffi::MPV_FORMAT_OSD_STRING => Node::String(cstr_to_string(n.u.string)),
                ffi::MPV_FORMAT_FLAG => Node::Flag(n.u.flag != 0),
                ffi::MPV_FORMAT_INT64 => Node::Int(n.u.int64),
                ffi::MPV_FORMAT_DOUBLE => Node::Double(n.u.double_),
                ffi::MPV_FORMAT_NODE_ARRAY | ffi::MPV_FORMAT_NODE_MAP => {
                    let list = &*n.u.list;
                    let len = list.num.max(0) as usize;
                    if n.format == ffi::MPV_FORMAT_NODE_ARRAY {
                        Node::Array((0..len).map(|i| Node::from_raw(list.values.add(i))).collect())
                    } else {
                        Node::Map(
                            (0..len)
                                .map(|i| (cstr_to_string(*list.keys.add(i)), Node::from_raw(list.values.add(i))))
                                .collect(),
                        )
                    }
                }
                ffi::MPV_FORMAT_BYTE_ARRAY => {
                    let ba = &*n.u.ba;
                    Node::Bytes(std::slice::from_raw_parts(ba.data as *const u8, ba.size).to_vec())
                }
                _ => Node::None,
            }
        }
    }
}

/// Why playback of a file ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Quit,
    Error(i32),
    Redirect,
    Other(i32),
}

/// An mpv event, copied out of mpv's memory.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Shutdown,
    Log { prefix: String, level: String, text: String },
    CommandReply { userdata: u64, error: i32 },
    StartFile,
    EndFile(EndReason),
    FileLoaded,
    VideoReconfig,
    AudioReconfig,
    Seek,
    PlaybackRestart,
    PropertyChange { userdata: u64, name: String, value: Node },
    QueueOverflow,
    Other(i32),
}

pub(crate) type WakeBox = Box<dyn Fn() + Send + Sync>;

/// An mpv instance (`mpv_handle`). All methods are thread-safe; events are read by one caller
/// at a time.
pub struct Mpv {
    lib: Arc<Library>,
    handle: *mut ffi::mpv_handle,
    events: Mutex<()>,
    wake: Mutex<Option<Box<WakeBox>>>,
}

// SAFETY: the libmpv client API is thread-safe for a handle; `wait_event` is serialized.
unsafe impl Send for Mpv {}
// SAFETY: see above.
unsafe impl Sync for Mpv {}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::Nul)
}

/// # Safety
/// `p` must be NULL or a valid NUL-terminated string.
unsafe fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// Options for a headless instance that never reads user config, scripts or the terminal.
pub const HEADLESS: &[(&str, &str)] = &[
    ("config", "no"),
    ("load-scripts", "no"),
    ("ytdl", "no"),
    ("osc", "no"),
    ("input-default-bindings", "no"),
    ("input-vo-keyboard", "no"),
    ("terminal", "no"),
    ("idle", "yes"),
];

impl Mpv {
    /// Creates and initializes an instance with `options` (set before `mpv_initialize`, which
    /// is required for encoding options such as `o`, `ovc`, `of`).
    pub fn new(lib: Arc<Library>, options: &[(&str, &str)]) -> Result<Mpv> {
        // SAFETY: no preconditions.
        let handle = unsafe { (lib.api.create)() };
        if handle.is_null() {
            return Err(Error::Mpv { what: "mpv_create".into(), code: -2, message: "out of memory".into() });
        }
        let mpv = Mpv { lib, handle, events: Mutex::new(()), wake: Mutex::new(None) };
        for (k, v) in options {
            mpv.set_option(k, v)?;
        }
        // SAFETY: handle is valid and not yet initialized.
        let rc = unsafe { (mpv.lib.api.initialize)(mpv.handle) };
        mpv.check("mpv_initialize", rc)?;
        Ok(mpv)
    }

    /// A headless instance with the default library ([`HEADLESS`] plus `extra`).
    pub fn headless(extra: &[(&str, &str)]) -> Result<Mpv> {
        let mut opts: Vec<(&str, &str)> = HEADLESS.to_vec();
        opts.extend_from_slice(extra);
        Mpv::new(Library::get()?, &opts)
    }

    pub fn library(&self) -> &Arc<Library> {
        &self.lib
    }

    pub(crate) fn raw(&self) -> *mut ffi::mpv_handle {
        self.handle
    }

    pub(crate) fn api(&self) -> &ffi::Api {
        &self.lib.api
    }

    fn check(&self, what: &str, rc: c_int) -> Result<()> {
        if rc >= 0 { Ok(()) } else { Err(Error::Mpv { what: what.into(), code: rc, message: self.lib.error_text(rc) }) }
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (cstring(name)?, cstring(value)?);
        // SAFETY: valid handle and NUL-terminated strings.
        let rc = unsafe { (self.lib.api.set_option_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        self.check(&format!("option {name}={value}"), rc)
    }

    pub fn set_property(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (cstring(name)?, cstring(value)?);
        // SAFETY: valid handle and NUL-terminated strings.
        let rc = unsafe { (self.lib.api.set_property_string)(self.handle, n.as_ptr(), v.as_ptr()) };
        self.check(&format!("set {name}"), rc)
    }

    pub fn set_flag(&self, name: &str, value: bool) -> Result<()> {
        let n = cstring(name)?;
        let mut v: c_int = value.into();
        // SAFETY: valid handle; FLAG takes a pointer to int.
        let rc = unsafe {
            (self.lib.api.set_property)(
                self.handle,
                n.as_ptr(),
                ffi::MPV_FORMAT_FLAG,
                &mut v as *mut c_int as *mut c_void,
            )
        };
        self.check(&format!("set {name}"), rc)
    }

    pub fn set_double(&self, name: &str, value: f64) -> Result<()> {
        let n = cstring(name)?;
        let mut v = value;
        // SAFETY: valid handle; DOUBLE takes a pointer to double.
        let rc = unsafe {
            (self.lib.api.set_property)(
                self.handle,
                n.as_ptr(),
                ffi::MPV_FORMAT_DOUBLE,
                &mut v as *mut f64 as *mut c_void,
            )
        };
        self.check(&format!("set {name}"), rc)
    }

    /// Reads a property as a node (every type can be read this way).
    pub fn get(&self, name: &str) -> Result<Node> {
        let n = cstring(name)?;
        let mut node = std::mem::MaybeUninit::<ffi::mpv_node>::zeroed();
        // SAFETY: valid handle; NODE writes an mpv_node we free with mpv_free_node_contents.
        let rc = unsafe {
            (self.lib.api.get_property)(self.handle, n.as_ptr(), ffi::MPV_FORMAT_NODE, node.as_mut_ptr() as *mut c_void)
        };
        self.check(&format!("get {name}"), rc)?;
        // SAFETY: on success mpv initialized the node.
        unsafe {
            let v = Node::from_raw(node.as_ptr());
            (self.lib.api.free_node_contents)(node.as_mut_ptr());
            Ok(v)
        }
    }

    pub fn get_string(&self, name: &str) -> Result<String> {
        match self.get(name)? {
            Node::String(s) => Ok(s),
            other => Ok(format!("{other:?}")),
        }
    }

    pub fn get_f64(&self, name: &str) -> Result<Option<f64>> {
        Ok(self.get(name)?.as_f64())
    }

    /// Runs a command synchronously (`["loadfile", path]`).
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned = args.iter().map(|a| cstring(a)).collect::<Result<Vec<_>>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: NULL-terminated array of valid strings that outlive the call.
        let rc = unsafe { (self.lib.api.command)(self.handle, ptrs.as_mut_ptr()) };
        self.check(&format!("command {}", args.first().unwrap_or(&"")), rc)
    }

    /// Runs a command asynchronously; completion arrives as [`Event::CommandReply`].
    pub fn command_async(&self, userdata: u64, args: &[&str]) -> Result<()> {
        let owned = args.iter().map(|a| cstring(a)).collect::<Result<Vec<_>>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: mpv copies the arguments before returning.
        let rc = unsafe { (self.lib.api.command_async)(self.handle, userdata, ptrs.as_mut_ptr()) };
        self.check(&format!("command {}", args.first().unwrap_or(&"")), rc)
    }

    /// Observes a property; changes arrive as [`Event::PropertyChange`] with `userdata`.
    pub fn observe(&self, userdata: u64, name: &str) -> Result<()> {
        let n = cstring(name)?;
        // SAFETY: valid handle and string.
        let rc = unsafe { (self.lib.api.observe_property)(self.handle, userdata, n.as_ptr(), ffi::MPV_FORMAT_NODE) };
        self.check(&format!("observe {name}"), rc)
    }

    /// Enables log messages at `level` ("error", "warn", "info", "v", "debug", "no").
    pub fn request_log_messages(&self, level: &str) -> Result<()> {
        let l = cstring(level)?;
        // SAFETY: valid handle and string.
        let rc = unsafe { (self.lib.api.request_log_messages)(self.handle, l.as_ptr()) };
        self.check("request_log_messages", rc)
    }

    /// Waits up to `timeout` seconds (negative = forever) for the next event.
    /// Returns `None` on timeout.
    pub fn wait_event(&self, timeout: f64) -> Option<Event> {
        let _g = self.events.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: valid handle; only one thread waits at a time (the mutex). The returned event
        // stays valid until the next wait_event, and we copy everything out before unlocking.
        unsafe {
            let ev = &*(self.lib.api.wait_event)(self.handle, timeout);
            Some(match ev.event_id {
                ffi::MPV_EVENT_NONE => return None,
                ffi::MPV_EVENT_SHUTDOWN => Event::Shutdown,
                ffi::MPV_EVENT_LOG_MESSAGE => {
                    let m = &*(ev.data as *const ffi::mpv_event_log_message);
                    Event::Log {
                        prefix: cstr_to_string(m.prefix),
                        level: cstr_to_string(m.level),
                        text: cstr_to_string(m.text).trim_end().to_string(),
                    }
                }
                ffi::MPV_EVENT_COMMAND_REPLY => Event::CommandReply { userdata: ev.reply_userdata, error: ev.error },
                ffi::MPV_EVENT_START_FILE => Event::StartFile,
                ffi::MPV_EVENT_END_FILE => {
                    let e = &*(ev.data as *const ffi::mpv_event_end_file);
                    Event::EndFile(match e.reason {
                        ffi::MPV_END_FILE_REASON_EOF => EndReason::Eof,
                        ffi::MPV_END_FILE_REASON_STOP => EndReason::Stop,
                        ffi::MPV_END_FILE_REASON_QUIT => EndReason::Quit,
                        ffi::MPV_END_FILE_REASON_ERROR => EndReason::Error(e.error),
                        ffi::MPV_END_FILE_REASON_REDIRECT => EndReason::Redirect,
                        r => EndReason::Other(r),
                    })
                }
                ffi::MPV_EVENT_FILE_LOADED => Event::FileLoaded,
                ffi::MPV_EVENT_VIDEO_RECONFIG => Event::VideoReconfig,
                ffi::MPV_EVENT_AUDIO_RECONFIG => Event::AudioReconfig,
                ffi::MPV_EVENT_SEEK => Event::Seek,
                ffi::MPV_EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
                ffi::MPV_EVENT_PROPERTY_CHANGE => {
                    let p = &*(ev.data as *const ffi::mpv_event_property);
                    let value = if p.format == ffi::MPV_FORMAT_NODE && !p.data.is_null() {
                        Node::from_raw(p.data as *const ffi::mpv_node)
                    } else {
                        Node::None
                    };
                    Event::PropertyChange { userdata: ev.reply_userdata, name: cstr_to_string(p.name), value }
                }
                ffi::MPV_EVENT_QUEUE_OVERFLOW => Event::QueueOverflow,
                other => Event::Other(other),
            })
        }
    }

    /// Interrupts a [`Mpv::wait_event`] in progress.
    pub fn wakeup(&self) {
        // SAFETY: valid handle.
        unsafe { (self.lib.api.wakeup)(self.handle) }
    }

    /// Calls `f` (from an mpv thread) whenever new events are queued. `f` must not call back
    /// into mpv; it should only signal another thread.
    pub fn set_wakeup_callback(&self, f: impl Fn() + Send + Sync + 'static) {
        unsafe extern "C" fn trampoline(d: *mut c_void) {
            // SAFETY: `d` is the boxed closure registered below, alive until replaced or drop.
            let f = unsafe { &*(d as *const WakeBox) };
            f();
        }
        let boxed: Box<WakeBox> = Box::new(Box::new(f));
        let ptr = &*boxed as *const WakeBox as *mut c_void;
        let mut slot = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: valid handle; the closure outlives its registration (kept in `self.wake`,
        // and the old one is only dropped after mpv switched to the new pointer).
        unsafe { (self.lib.api.set_wakeup_callback)(self.handle, Some(trampoline), ptr) };
        *slot = Some(boxed);
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: valid handle; after this call mpv no longer invokes the wakeup callback.
        unsafe {
            (self.lib.api.set_wakeup_callback)(self.handle, None, std::ptr::null_mut());
            (self.lib.api.terminate_destroy)(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_is_reported_not_fatal() {
        let err = Library::open(Some(Path::new("/nonexistent/libmpv-test.so"))).unwrap_err();
        assert!(matches!(err, Error::NotAvailable(_)));
    }

    #[test]
    fn version_and_properties_when_available() {
        let Ok(lib) = Library::get() else {
            eprintln!("libmpv not found; skipping");
            return;
        };
        assert!(lib.version_string().starts_with("2."));
        let mpv = Mpv::new(lib, &[("config", "no"), ("vo", "null"), ("ao", "null"), ("idle", "yes")]).unwrap();
        assert!(mpv.get_string("mpv-version").unwrap().starts_with("mpv"));
        mpv.set_property("volume", "50").unwrap();
        assert_eq!(mpv.get_f64("volume").unwrap(), Some(50.0));
        assert!(mpv.get("no-such-property").is_err());
        assert!(!mpv.get("encoder-list").unwrap().as_array().is_empty());
    }
}
