//! Corrent's media library: libraries and folders, scanning (walk → diff → probe/tags/.nfo →
//! item tree), artwork, full-text search, watch state and the queries clients browse with.
//! Everything lives in `corrent.db` (see `corrent-db` migration 0002).

#[macro_use]
extern crate corrent_i18n;

pub mod artwork;
pub mod nfo;
pub mod parse;
pub mod probe;
pub mod query;
pub mod scan;
pub mod tags;
pub mod wanted;
pub mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use corrent_db::Db;
use corrent_db::rusqlite::{self, OptionalExtension, params};
use corrent_proto::library::{
    Library as LibraryInfo, LibraryKind, LibrarySettings, LibraryUpdate, NewLibrary, ScanStatus,
};

/// Something in the library changed (for realtime clients).
#[derive(Clone, Debug, PartialEq)]
pub enum LibraryEvent {
    Scan(ScanStatus),
    Changed { library_id: i64 },
}

struct Inner {
    db: Db,
    artwork: PathBuf,
    prober: probe::Prober,
    status: Mutex<HashMap<i64, ScanStatus>>,
    cancel: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    events: tokio::sync::broadcast::Sender<LibraryEvent>,
    /// Image work is CPU-bound: at most this many at once.
    images: tokio::sync::Semaphore,
}

/// The media library. Cheap to clone.
#[derive(Clone)]
pub struct Library {
    inner: Arc<Inner>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
pub enum LibError {
    NotFound,
    Invalid(String),
    Db(String),
}

impl std::fmt::Display for LibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibError::NotFound => write!(f, "not found"),
            LibError::Invalid(m) => write!(f, "{m}"),
            LibError::Db(m) => write!(f, "database: {m}"),
        }
    }
}

impl std::error::Error for LibError {}

impl From<corrent_db::DbError> for LibError {
    fn from(e: corrent_db::DbError) -> Self {
        LibError::Db(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LibError>;

impl Library {
    /// `data_dir` holds `artwork/`. The prober uses the worker process when found.
    pub fn new(db: Db, data_dir: &Path, prober: probe::Prober) -> Library {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        Library {
            inner: Arc::new(Inner {
                db,
                artwork: data_dir.join("artwork"),
                prober,
                status: Mutex::new(HashMap::new()),
                cancel: Mutex::new(HashMap::new()),
                events: tokio::sync::broadcast::channel(256).0,
                images: tokio::sync::Semaphore::new((cpus / 2).max(1)),
            }),
        }
    }

    pub fn db(&self) -> &Db {
        &self.inner.db
    }

    pub fn artwork_root(&self) -> &Path {
        &self.inner.artwork
    }

    pub fn prober(&self) -> &probe::Prober {
        &self.inner.prober
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<LibraryEvent> {
        self.inner.events.subscribe()
    }

    pub(crate) fn emit(&self, e: LibraryEvent) {
        let _ = self.inner.events.send(e);
    }

    pub(crate) async fn image_permit(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.inner.images.acquire().await.expect("semaphore never closed")
    }

    pub fn scan_status(&self, library_id: i64) -> Option<ScanStatus> {
        self.inner.status.lock().unwrap_or_else(|e| e.into_inner()).get(&library_id).cloned()
    }

    pub(crate) fn set_status(&self, s: ScanStatus) {
        self.inner.status.lock().unwrap_or_else(|e| e.into_inner()).insert(s.library_id, s.clone());
        self.emit(LibraryEvent::Scan(s));
    }

    /// Asks a running scan to stop at the next file.
    pub fn cancel_scan(&self, library_id: i64) {
        if let Some(c) = self.inner.cancel.lock().unwrap_or_else(|e| e.into_inner()).get(&library_id) {
            c.store(true, Ordering::Relaxed);
        }
    }

    /// Starts a scan in the background unless one is running. Returns false when one was.
    pub fn start_scan(&self, library_id: i64) -> bool {
        let flag = {
            let mut c = self.inner.cancel.lock().unwrap_or_else(|e| e.into_inner());
            if c.contains_key(&library_id) {
                return false;
            }
            let f = Arc::new(AtomicBool::new(false));
            c.insert(library_id, f.clone());
            f
        };
        let lib = self.clone();
        tokio::spawn(async move {
            let r = scan::scan(&lib, library_id, &flag).await;
            if let Err(e) = r {
                tracing::warn!("scan of library {library_id} failed: {e}");
                let mut s = lib.scan_status(library_id).unwrap_or(ScanStatus { library_id, ..Default::default() });
                s.running = false;
                s.error = Some(e.to_string());
                lib.set_status(s);
            }
            lib.inner.cancel.lock().unwrap_or_else(|e| e.into_inner()).remove(&library_id);
            lib.emit(LibraryEvent::Changed { library_id });
        });
        true
    }

    /// Runs a scan to completion (tests, CLI).
    pub async fn scan_now(&self, library_id: i64) -> Result<ScanStatus> {
        let flag = Arc::new(AtomicBool::new(false));
        scan::scan(self, library_id, &flag).await?;
        self.emit(LibraryEvent::Changed { library_id });
        Ok(self.scan_status(library_id).unwrap_or_default())
    }

    // ------------------------------------------------------------ libraries

    pub async fn create(&self, l: NewLibrary) -> Result<LibraryInfo> {
        let name = l.name.trim().to_string();
        if name.is_empty() || name.len() > 100 {
            return Err(LibError::Invalid("library name required".into()));
        }
        let folders = clean_folders(&l.folders)?;
        let settings = serde_json::to_string(&l.settings).unwrap_or_else(|_| "{}".into());
        let now = now_unix();
        let id = self
            .inner
            .db
            .write(move |t| {
                t.execute(
                    "INSERT INTO libraries(name, kind, settings, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![name, l.kind.as_str(), settings, now],
                )?;
                let id = t.last_insert_rowid();
                for f in &folders {
                    t.execute("INSERT INTO library_folders(library_id, path) VALUES (?1, ?2)", params![id, f])?;
                }
                Ok(id)
            })
            .await?;
        self.get(id).await
    }

    pub async fn update(&self, id: i64, u: LibraryUpdate) -> Result<LibraryInfo> {
        let folders = match &u.folders {
            Some(f) => Some(clean_folders(f)?),
            None => None,
        };
        let n = self
            .inner
            .db
            .write(move |t| {
                let exists: bool =
                    t.query_row("SELECT count(*) FROM libraries WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? > 0;
                if !exists {
                    return Ok(0);
                }
                if let Some(name) = u.name.as_ref().map(|n| n.trim()).filter(|n| !n.is_empty()) {
                    t.execute("UPDATE libraries SET name = ?1 WHERE id = ?2", params![name, id])?;
                }
                if let Some(s) = &u.settings {
                    t.execute(
                        "UPDATE libraries SET settings = ?1 WHERE id = ?2",
                        params![serde_json::to_string(s).unwrap_or_else(|_| "{}".into()), id],
                    )?;
                }
                if let Some(fs) = &folders {
                    t.execute("DELETE FROM library_folders WHERE library_id = ?1", [id])?;
                    for f in fs {
                        t.execute("INSERT INTO library_folders(library_id, path) VALUES (?1, ?2)", params![id, f])?;
                    }
                }
                Ok(1)
            })
            .await?;
        if n == 0 {
            return Err(LibError::NotFound);
        }
        self.get(id).await
    }

    /// Removes a library from the database (never touches the files).
    pub async fn delete(&self, id: i64) -> Result<()> {
        self.cancel_scan(id);
        let n = self.inner.db.write(move |t| t.execute("DELETE FROM libraries WHERE id = ?1", [id])).await?;
        if n == 0 {
            return Err(LibError::NotFound);
        }
        // Orphaned FTS rows of deleted items.
        self.inner
            .db
            .write(|t| t.execute("DELETE FROM items_fts WHERE rowid NOT IN (SELECT id FROM items)", []).map(|_| ()))
            .await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<LibraryInfo>> {
        let rows = self
            .inner
            .db
            .read(|c| {
                let mut st = c.prepare(
                    "SELECT l.id, l.name, l.kind, l.settings, l.scanned_at, \
                     (SELECT count(*) FROM items i WHERE i.library_id = l.id AND i.parent_id IS NULL) \
                     FROM libraries l ORDER BY l.name",
                )?;
                let libs = st
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<i64>>(4)?,
                            r.get::<_, i64>(5)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut out = Vec::new();
                for (id, name, kind, settings, scanned_at, count) in libs {
                    let mut fs =
                        c.prepare_cached("SELECT path FROM library_folders WHERE library_id = ?1 ORDER BY path")?;
                    let folders =
                        fs.query_map([id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    out.push((id, name, kind, settings, scanned_at, count, folders));
                }
                Ok(out)
            })
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, kind, settings, scanned_at, item_count, folders)| LibraryInfo {
                id,
                name,
                kind: LibraryKind::parse(&kind).unwrap_or(LibraryKind::Videos),
                folders,
                settings: serde_json::from_str(&settings).unwrap_or_else(|_| LibrarySettings::defaults()),
                item_count,
                scanned_at,
                scanning: self
                    .scan_status(id)
                    .filter(|s| s.running)
                    .map(|s| (s.done * 100).checked_div(s.total).unwrap_or(0).min(100) as u8),
            })
            .collect())
    }

    pub async fn get(&self, id: i64) -> Result<LibraryInfo> {
        self.list().await?.into_iter().find(|l| l.id == id).ok_or(LibError::NotFound)
    }

    /// The library whose folders contain `path` (deepest folder wins).
    pub async fn library_for_path(&self, path: &Path) -> Result<Option<i64>> {
        let p = path.to_path_buf();
        Ok(self
            .inner
            .db
            .read(move |c| {
                let mut st = c.prepare("SELECT library_id, path FROM library_folders")?;
                let rows = st
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows.into_iter().filter(|(_, f)| p.starts_with(f)).max_by_key(|(_, f)| f.len()).map(|(id, _)| id))
            })
            .await?)
    }

    /// Libraries fed by a torrent category.
    pub async fn libraries_for_category(&self, category: &str) -> Result<Vec<LibraryInfo>> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .filter(|l| l.settings.import_categories.iter().any(|c| c == category))
            .collect())
    }

    pub(crate) async fn library_row(&self, id: i64) -> Result<(LibraryKind, Vec<PathBuf>, LibrarySettings)> {
        let r = self
            .inner
            .db
            .read(move |c| {
                let row = c
                    .query_row("SELECT kind, settings FROM libraries WHERE id = ?1", [id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .optional()?;
                let mut st = c.prepare("SELECT path FROM library_folders WHERE library_id = ?1")?;
                let folders = st.query_map([id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(row.map(|(k, s)| (k, s, folders)))
            })
            .await?;
        let (kind, settings, folders) = r.ok_or(LibError::NotFound)?;
        Ok((
            LibraryKind::parse(&kind).unwrap_or(LibraryKind::Videos),
            folders.into_iter().map(PathBuf::from).collect(),
            serde_json::from_str(&settings).unwrap_or_else(|_| LibrarySettings::defaults()),
        ))
    }
}

fn clean_folders(folders: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for f in folders.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
        let p = Path::new(f);
        if !p.is_absolute() {
            return Err(LibError::Invalid(format!("not an absolute path: {f}")));
        }
        let s = p.components().as_path().display().to_string();
        let s = if s.len() > 1 { s.trim_end_matches(['/', '\\']).to_string() } else { s };
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
