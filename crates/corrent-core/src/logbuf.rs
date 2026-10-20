//! A tracing layer that keeps the most recent log lines in memory (log viewers, crash reports).

use std::collections::VecDeque;
use std::sync::Mutex;

use tracing::Level;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

#[derive(Clone, Debug)]
pub struct LogLine {
    /// Increases by one per captured line (lets readers continue exactly where they stopped).
    pub seq: u64,
    pub time: i64,
    pub level: Level,
    pub target: String,
    pub message: String,
}

const MAX_LINES: usize = 4000;

pub static LINES: Mutex<VecDeque<LogLine>> = Mutex::new(VecDeque::new());

pub struct LogCapture;

struct MsgVisitor(String);

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.insert_str(0, &format!("{value:?}"));
        } else {
            self.0.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.insert_str(0, value);
        } else {
            self.0.push_str(&format!(" {}={value}", field.name()));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for LogCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > Level::INFO {
            return;
        }
        let mut v = MsgVisitor(String::new());
        event.record(&mut v);
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let line = LogLine {
            seq: SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            time: crate::util::now_unix(),
            level: *meta.level(),
            target: meta.target().to_string(),
            message: v.0,
        };
        let mut l = crate::util::lock(&LINES);
        if l.len() >= MAX_LINES {
            l.pop_front();
        }
        l.push_back(line);
    }
}
