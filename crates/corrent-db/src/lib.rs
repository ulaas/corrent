//! Corrent's SQLite database (`corrent.db`): one writer thread that batches write jobs into
//! transactions, a small pool of read connections, and numbered migrations applied under
//! `PRAGMA user_version` with a backup before each upgrade.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use rusqlite;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

/// Every migration, in order. Version N = the first N applied.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("identity", include_str!("../migrations/0001_identity.sql")),
    ("library", include_str!("../migrations/0002_library.sql")),
    ("wanted", include_str!("../migrations/0003_wanted.sql")),
    ("plugins", include_str!("../migrations/0004_plugins.sql")),
    ("tasks", include_str!("../migrations/0005_tasks.sql")),
    ("profiles", include_str!("../migrations/0006_profiles.sql")),
    ("monitor", include_str!("../migrations/0007_monitor.sql")),
    ("prefs", include_str!("../migrations/0008_prefs.sql")),
    ("jellyfin", include_str!("../migrations/0009_jellyfin.sql")),
    ("segments", include_str!("../migrations/0010_segments.sql")),
    ("image_locks", include_str!("../migrations/0011_image_locks.sql")),
    ("agent", include_str!("../migrations/0012_agent.sql")),
];

pub fn schema_version() -> u32 {
    MIGRATIONS.len() as u32
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("the database was made by a newer Corrent (schema {found}, this one knows {known})")]
    TooNew { found: u32, known: u32 },
    #[error("the database is damaged: {0}")]
    Corrupt(String),
    #[error("the database writer stopped")]
    Closed,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Runs inside the batch transaction and returns whether it succeeded, plus a reply that is
/// delivered after the commit (with the commit's error, if any).
type Reply = Box<dyn FnOnce(Option<String>) + Send>;
/// Gets the batch transaction, or the reason none could be started.
type Job = Box<dyn FnOnce(std::result::Result<&Transaction, &str>) -> (bool, Reply) + Send>;

struct Inner {
    path: PathBuf,
    writer: std::sync::mpsc::Sender<Job>,
    readers: Mutex<Vec<Connection>>,
    max_readers: usize,
}

/// A cheap handle to the database; clone it freely.
#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
}

fn configure(c: &Connection) -> rusqlite::Result<()> {
    c.busy_timeout(Duration::from_secs(5))?;
    c.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY; \
         PRAGMA cache_size=-65536; PRAGMA mmap_size=268435456;",
    )
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Applies pending migrations; backs up an existing database first (`VACUUM INTO`).
fn migrate(c: &mut Connection, path: &Path) -> Result<()> {
    let found: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let known = schema_version();
    if found > known {
        return Err(DbError::TooNew { found, known });
    }
    if found == known {
        return Ok(());
    }
    if found > 0 && path.exists() {
        let bak = path.with_extension(format!("db.v{found}-{}.bak", now_unix()));
        c.execute("VACUUM INTO ?1", [bak.to_string_lossy()])?;
        tracing::info!("backed up {} before migrating to schema {known}", bak.display());
    }
    for (i, (name, sql)) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let tx = c.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as u32)?;
        tx.commit()?;
        tracing::info!("database migrated to {} ({name})", i + 1);
    }
    Ok(())
}

impl Db {
    /// Opens (creating if needed) and migrates the database at `path`.
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DbError::Other(format!("{}: {e}", dir.display())))?;
        }
        let mut c = Connection::open(path)?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        configure(&c)?;
        let check: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(DbError::Corrupt(check));
        }
        migrate(&mut c, path)?;
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("corrent-db-writer".into())
            .spawn(move || writer_loop(c, rx))
            .map_err(|e| DbError::Other(e.to_string()))?;
        Ok(Db {
            inner: Arc::new(Inner {
                path: path.to_path_buf(),
                writer: tx,
                readers: Mutex::new(Vec::new()),
                max_readers: 8,
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    fn take_reader(&self) -> Result<Connection> {
        if let Some(c) = self.inner.readers.lock().unwrap_or_else(|e| e.into_inner()).pop() {
            return Ok(c);
        }
        let c = Connection::open_with_flags(
            &self.inner.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
        )?;
        configure(&c)?;
        Ok(c)
    }

    fn put_reader(&self, c: Connection) {
        let mut pool = self.inner.readers.lock().unwrap_or_else(|e| e.into_inner());
        if pool.len() < self.inner.max_readers {
            pool.push(c);
        }
    }

    /// Runs a read on a pooled read-only connection (blocking; call from a blocking context).
    pub fn read_blocking<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let c = self.take_reader()?;
        let r = f(&c);
        self.put_reader(c);
        Ok(r?)
    }

    /// Runs a read off the async runtime.
    pub async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.read_blocking(f)).await.map_err(|e| DbError::Other(e.to_string()))?
    }

    fn submit<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<T>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job: Job = Box::new(move |t| {
            let t = match t {
                Ok(t) => t,
                Err(why) => {
                    let why = why.to_string();
                    let reply: Reply = Box::new(move |_| {
                        let _ = tx.send(Err(DbError::Other(why)));
                    });
                    return (false, reply);
                }
            };
            let r = f(t);
            let ok = r.is_ok();
            let reply: Reply = Box::new(move |commit_err| {
                let _ = tx.send(match (r, commit_err) {
                    (Err(e), _) => Err(e.into()),
                    (Ok(_), Some(e)) => Err(DbError::Other(format!("commit failed: {e}"))),
                    (Ok(v), None) => Ok(v),
                });
            });
            (ok, reply)
        });
        self.inner.writer.send(job).map_err(|_| DbError::Closed)?;
        Ok(rx)
    }

    /// Runs `f` inside a write transaction on the writer thread. Jobs queued together share one
    /// transaction; each runs in its own savepoint, so a failing job rolls back only itself.
    pub async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        self.submit(f)?.await.map_err(|_| DbError::Closed)?
    }

    /// Blocking variant of [`Db::write`] (never call it from an async task).
    pub fn write_blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        self.submit(f)?.blocking_recv().map_err(|_| DbError::Closed)?
    }

    /// A consistent copy of the whole database (backups).
    pub async fn backup_to(&self, dest: PathBuf) -> Result<()> {
        self.write(move |t| t.execute("VACUUM INTO ?1", [dest.to_string_lossy()]).map(|_| ())).await
    }
}

fn writer_loop(mut c: Connection, rx: std::sync::mpsc::Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while batch.len() < 256 {
            match rx.try_recv() {
                Ok(j) => batch.push(j),
                Err(_) => break,
            }
        }
        let tx = match c.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("database: can't start a write transaction: {e}");
                // Each caller hears the real reason (their job never ran).
                let msg = format!("couldn't start a transaction: {e}");
                for job in batch {
                    let (_, reply) = job(Err(&msg));
                    reply(None);
                }
                continue;
            }
        };
        let mut replies = Vec::with_capacity(batch.len());
        for (i, job) in batch.into_iter().enumerate() {
            let name = format!("j{i}");
            if let Err(e) = tx.execute_batch(&format!("SAVEPOINT {name}")) {
                tracing::error!("database: savepoint failed: {e}");
                continue;
            }
            // A panicking job must not take the writer (and every later write) down with it.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(Ok(&tx)))) {
                Ok((true, reply)) => {
                    let _ = tx.execute_batch(&format!("RELEASE {name}"));
                    replies.push(reply);
                }
                Ok((false, reply)) => {
                    let _ = tx.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"));
                    replies.push(reply);
                }
                Err(_) => {
                    tracing::error!("database: a write job panicked");
                    let _ = tx.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"));
                }
            }
        }
        let commit_err = tx.commit().err().map(|e| {
            tracing::error!("database: commit failed: {e}");
            e.to_string()
        });
        // Callers hear back only after the commit (or its failure). With synchronous=NORMAL in
        // WAL mode a committed write survives a crash of Corrent, but the very last
        // transactions can be lost on power failure — acceptable for this data.
        for reply in replies {
            reply(commit_err.clone());
        }
    }
    let _ = c.execute_batch("PRAGMA optimize");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("corrent-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("corrent.db")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn migrates_writes_and_reads() {
        let p = scratch("basic");
        let db = Db::open(&p).unwrap();
        let v: u32 = db.read(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0))).await.unwrap();
        assert_eq!(v, schema_version());
        db.write(|t| t.execute("INSERT INTO kv(key, value) VALUES ('a', '1')", [])).await.unwrap();
        let got: String =
            db.read(|c| c.query_row("SELECT value FROM kv WHERE key='a'", [], |r| r.get(0))).await.unwrap();
        assert_eq!(got, "1");
        drop(db);
        // Re-open: no migration runs again.
        let db = Db::open(&p).unwrap();
        let n: i64 = db.read(|c| c.query_row("SELECT count(*) FROM kv", [], |r| r.get(0))).await.unwrap();
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_job_rolls_back_only_itself() {
        let p = scratch("savepoint");
        let db = Db::open(&p).unwrap();
        let mut waits = Vec::new();
        for i in 0..50 {
            let db = db.clone();
            waits.push(tokio::spawn(async move {
                db.write(move |t| {
                    t.execute("INSERT INTO kv(key, value) VALUES (?1, 'x')", [format!("k{i}")])?;
                    if i % 10 == 0 {
                        // Duplicate key: this job fails after its first insert.
                        t.execute("INSERT INTO kv(key, value) VALUES (?1, 'y')", [format!("k{i}")])?;
                    }
                    Ok(())
                })
                .await
            }));
        }
        let mut failed = 0;
        for w in waits {
            if w.await.unwrap().is_err() {
                failed += 1;
            }
        }
        assert_eq!(failed, 5);
        let n: i64 = db.read(|c| c.query_row("SELECT count(*) FROM kv", [], |r| r.get(0))).await.unwrap();
        assert_eq!(n, 45, "failed jobs left nothing behind");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn newer_schema_is_refused() {
        let p = scratch("newer");
        {
            let c = Connection::open(&p).unwrap();
            c.pragma_update(None, "user_version", 999).unwrap();
        }
        assert!(matches!(Db::open(&p), Err(DbError::TooNew { found: 999, .. })));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_write_is_visible_to_the_next_read() {
        let p = scratch("visible");
        let db = Db::open(&p).unwrap();
        // Concurrent writers keep batches large while one caller checks read-after-write.
        let noise: Vec<_> = (0..4)
            .map(|w| {
                let db = db.clone();
                tokio::spawn(async move {
                    for i in 0..100 {
                        let _ = db
                            .write(move |t| t.execute("INSERT INTO kv VALUES (?1, 'n')", [format!("n{w}-{i}")]))
                            .await;
                    }
                })
            })
            .collect();
        for i in 0..100 {
            db.write(move |t| t.execute("INSERT INTO kv VALUES (?1, 'v')", [format!("v{i}")])).await.unwrap();
            let n: i64 =
                db.read(|c| c.query_row("SELECT count(*) FROM kv WHERE value = 'v'", [], |r| r.get(0))).await.unwrap();
            assert_eq!(n, i + 1);
        }
        for n in noise {
            n.await.unwrap();
        }
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
