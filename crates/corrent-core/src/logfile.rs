//! The log file, written by one background thread (STABILITY.md O1). A log line is handed over
//! without waiting, so a slow or full disk never stalls the UI or the engine: lines that don't
//! fit in the queue are counted, and the file says how many were lost. The file is rotated by
//! size (`<stem>.log`, `<stem>.log.1` … `.5`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Lines waiting for the disk; more are dropped (and counted).
const QUEUE: usize = 8192;
const MAX_BYTES: u64 = 10 * 1024 * 1024;
const KEEP: usize = 5;
const LOST: &str = "log lines were lost (the disk couldn't keep up)";

enum Msg {
    Line(Vec<u8>),
    Flush(std::sync::mpsc::Sender<()>),
}

/// A `MakeWriter` for tracing's fmt layer.
#[derive(Clone)]
pub struct Writer {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
}

/// One event's text, sent as a whole when the layer is done with it.
pub struct Line {
    buf: Vec<u8>,
    w: Writer,
}

impl Write for Line {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.w.tx.try_send(Msg::Line(std::mem::take(&mut self.buf))) {
            self.w.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Writer {
    type Writer = Line;
    fn make_writer(&'a self) -> Line {
        Line { buf: Vec::new(), w: self.clone() }
    }
}

impl Writer {
    /// Wait until every line handed over so far is written (at most `timeout`).
    pub fn flush(&self, timeout: Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        // Blocking send: a full queue drains by itself, the timeout bounds the wait.
        let tx2 = self.tx.clone();
        let sent = std::thread::Builder::new()
            .name("corrent-log-flush".into())
            .spawn(move || tx2.send(Msg::Flush(tx)).is_ok())
            .is_ok();
        sent && rx.recv_timeout(timeout).is_ok()
    }
}

static GLOBAL: OnceLock<Writer> = OnceLock::new();

/// Start the log file `<log_dir>/<stem>.log` (once per process). None if it can't be opened.
pub fn init(log_dir: &Path, stem: &str) -> Option<Writer> {
    if let Some(w) = GLOBAL.get() {
        return Some(w.clone());
    }
    let _ = std::fs::create_dir_all(log_dir);
    let w = start(Sink::file(log_dir.join(format!("{stem}.log")), MAX_BYTES, KEEP).ok()?)?;
    Some(GLOBAL.get_or_init(|| w).clone())
}

/// Wait until the log is on disk (exit paths; a no-op without a log file, or on the log thread).
pub fn flush(timeout: Duration) -> bool {
    if std::thread::current().name() == Some("corrent-log") {
        return false;
    }
    GLOBAL.get().is_some_and(|w| w.flush(timeout))
}

/// Flushes the log when dropped: keep one alive for as long as `main` runs, so an early return
/// doesn't lose the last lines.
pub struct FlushGuard;

impl Drop for FlushGuard {
    fn drop(&mut self) {
        flush(Duration::from_secs(1));
    }
}

fn start(sink: Sink) -> Option<Writer> {
    let (tx, rx) = sync_channel(QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let d = dropped.clone();
    std::thread::Builder::new().name("corrent-log".into()).spawn(move || run(rx, sink, d)).ok()?;
    Some(Writer { tx, dropped })
}

fn run(rx: Receiver<Msg>, mut sink: Sink, dropped: Arc<AtomicU64>) {
    while let Ok(first) = rx.recv() {
        // A burst at a time, then onto the disk: the file is never more than a burst behind
        // (a crash, or someone reading the log while Corrent runs).
        let mut next = Some(first);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Line(b) => sink.write(&b),
                Msg::Flush(done) => {
                    sink.flush();
                    let _ = done.send(());
                }
            }
            // Where the lines went missing, not after the whole burst.
            let n = dropped.swap(0, Ordering::Relaxed);
            if n > 0 {
                // English, like every other log line.
                sink.write(format!("[corrent-log] {n} {LOST}\n").as_bytes());
            }
            next = rx.try_recv().ok();
        }
        sink.flush();
    }
    sink.flush();
}

/// Where the lines go: the rotated file (or, in tests, anything).
struct Sink {
    out: Box<dyn Write + Send>,
    path: Option<PathBuf>,
    size: u64,
    max: u64,
    keep: usize,
}

impl Sink {
    fn file(path: PathBuf, max: u64, keep: usize) -> std::io::Result<Sink> {
        let f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Sink { out: Box::new(std::io::BufWriter::new(f)), path: Some(path), size, max, keep })
    }

    fn write(&mut self, b: &[u8]) {
        if self.path.is_some() && self.size > 0 && self.size + b.len() as u64 > self.max {
            self.rotate();
        }
        if self.out.write_all(b).is_ok() {
            self.size += b.len() as u64;
        }
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }

    /// `<stem>.log` becomes `.log.1`, `.1` becomes `.2` … the oldest beyond `keep` goes.
    fn rotate(&mut self) {
        let Some(path) = self.path.clone() else { return };
        let _ = self.out.flush();
        let numbered = |n: usize| {
            let mut p = path.as_os_str().to_owned();
            p.push(format!(".{n}"));
            PathBuf::from(p)
        };
        let _ = std::fs::remove_file(numbered(self.keep));
        for n in (1..self.keep).rev() {
            let _ = std::fs::rename(numbered(n), numbered(n + 1));
        }
        // On Windows the open file can't be renamed: switch to a sink first.
        self.out = Box::new(std::io::sink());
        let _ = std::fs::rename(&path, numbered(1));
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => {
                self.out = Box::new(std::io::BufWriter::new(f));
                self.size = 0;
            }
            Err(e) => {
                eprintln!("the log file couldn't be reopened: {e}");
                // Don't try again with every line.
                self.size = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::fmt::MakeWriter;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("corrent-logfile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn send(w: &Writer, text: &[u8]) {
        let mut l = w.make_writer();
        l.write_all(text).unwrap();
    }

    #[test]
    fn a_flood_is_rotated_and_bounded() {
        let dir = scratch("rotate");
        let path = dir.join("corrent.log");
        let w = start(Sink::file(path.clone(), 1 << 20, 5).unwrap()).unwrap();
        // 20 MiB in 1 KiB lines, faster than the disk might take them: some may be dropped.
        let line = [b'x'; 1023];
        for _ in 0..20 * 1024 {
            let mut l = w.make_writer();
            l.write_all(&line).unwrap();
            l.write_all(b"\n").unwrap();
        }
        assert!(w.flush(Duration::from_secs(30)));
        let total: u64 = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum();
        assert!(total <= 6 << 20, "{total} bytes kept");
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n == "corrent.log.5") && !names.iter().any(|n| n == "corrent.log.6"), "{names:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A disk that takes 200 ms per write.
    struct Slow(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Slow {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(200));
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_slow_disk_never_holds_up_the_logging_thread() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Sink { out: Box::new(Slow(seen.clone())), path: None, size: 0, max: u64::MAX, keep: 0 };
        let w = start(sink).unwrap();
        let t = std::time::Instant::now();
        for i in 0..(QUEUE + 2000) {
            send(&w, format!("line {i}\n").as_bytes());
        }
        assert!(t.elapsed() < Duration::from_secs(2), "logging waited {:?}", t.elapsed());
        // The overflow is counted and reported in the log itself.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !String::from_utf8_lossy(&seen.lock().unwrap()).contains("log lines were lost") {
            assert!(std::time::Instant::now() < deadline, "no note about the lost lines");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn flush_waits_for_the_lines() {
        let dir = scratch("flush");
        let path = dir.join("x.log");
        let w = start(Sink::file(path.clone(), MAX_BYTES, KEEP).unwrap()).unwrap();
        send(&w, b"hello\n");
        assert!(w.flush(Duration::from_secs(5)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
