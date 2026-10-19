//! Desktop notifications through one worker thread with a bounded queue. Showing one can block
//! (a hung notification service); a burst of finished downloads must not start a thread each.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

/// Notifications waiting to be shown; more than this are counted and summarised.
const QUEUE: usize = 32;

type Show = Box<dyn Fn(&str, &str) + Send>;

pub struct Notifier {
    tx: Mutex<Option<SyncSender<(String, String)>>>,
    show: Mutex<Option<Show>>,
    dropped: Arc<AtomicUsize>,
}

impl Notifier {
    pub fn new(show: impl Fn(&str, &str) + Send + 'static) -> Notifier {
        Notifier { tx: Mutex::new(None), show: Mutex::new(Some(Box::new(show))), dropped: Arc::default() }
    }

    /// Queue a notification; never blocks. The worker starts with the first one.
    pub fn push(&self, title: &str, body: &str) {
        let mut tx = self.tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if tx.is_none() {
            let Some(show) = self.show.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else {
                return;
            };
            let (sender, rx) = sync_channel(QUEUE);
            let dropped = self.dropped.clone();
            let started =
                std::thread::Builder::new().name("corrent-notify".into()).spawn(move || run(rx, show, dropped));
            if let Err(e) = started {
                tracing::debug!("no notification thread: {e}");
                return;
            }
            *tx = Some(sender);
        }
        if let Some(sender) = tx.as_ref()
            && let Err(TrySendError::Full(_)) = sender.try_send((title.to_string(), body.to_string()))
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn run(rx: Receiver<(String, String)>, show: Show, dropped: Arc<AtomicUsize>) {
    while let Ok(first) = rx.recv() {
        // Everything queued, then one summary of what didn't fit.
        let mut next = Some(first);
        while let Some((title, body)) = next.take() {
            show(&title, &body);
            next = rx.try_recv().ok();
        }
        let n = dropped.swap(0, Ordering::Relaxed);
        if n > 0 {
            let text = if n == 1 { t!("1 more notification").to_string() } else { t!("{n} more notifications", n = n) };
            show("Corrent", &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_burst_uses_one_thread_and_is_summarised() {
        let seen: Arc<Mutex<Vec<(String, std::thread::ThreadId)>>> = Arc::default();
        let log = seen.clone();
        let n = Notifier::new(move |title, body| {
            std::thread::sleep(Duration::from_millis(5));
            log.lock().unwrap().push((format!("{title}: {body}"), std::thread::current().id()));
        });
        for i in 0..1000 {
            n.push("Download complete", &format!("file {i}"));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let v = seen.lock().unwrap();
            if v.iter().any(|(t, _)| t.contains("more notifications")) || std::time::Instant::now() > deadline {
                break;
            }
            drop(v);
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        let v = seen.lock().unwrap();
        let threads: std::collections::HashSet<_> = v.iter().map(|(_, id)| *id).collect();
        assert_eq!(threads.len(), 1, "one worker");
        assert_ne!(threads.into_iter().next(), Some(std::thread::current().id()));
        let shown = v.iter().filter(|(t, _)| t.starts_with("Download complete")).count();
        let summary: usize =
            v.iter().filter_map(|(t, _)| t.strip_prefix("Corrent: ")?.split(' ').next()?.parse::<usize>().ok()).sum();
        assert!(shown <= QUEUE + 2, "{shown} shown");
        assert_eq!(shown + summary, 1000, "every notification is shown or counted");
    }
}
