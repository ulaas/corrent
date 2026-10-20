//! Process plugins: a program in the plugin's folder that answers JSON-RPC 2.0 requests, one per
//! line, on stdin/stdout (logs go to stderr). For things WebAssembly can't do (native libraries,
//! Python scrapers, an ffmpeg wrapper). They are not sandboxed: only administrators install
//! them, by putting the folder into `<data>/plugins/<id>/`.
//!
//! Methods (params → result), with the same shapes as the WIT records:
//! `configure` ({settings}) → null, sent after start and when settings change;
//! `metadata.kinds` → [kind]; `metadata.search` ({query}) → [candidate];
//! `metadata.details` ({kind, id, language}) → item-details;
//! `metadata.episodes` ({show, season, language}) → [episode];
//! `indexer.search` ({query}) → [release]; `notifier.notify` ({title, body}) → null;
//! `subtitles.search` ({query}) → [subtitle]; `subtitles.download` ({id}) → subtitle-file;
//! `parser.parse` ({name}) → parsed-name | null; `task.run` → string.
//! Errors: `{"error": {"code": …, "message": …}}`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Why a call failed: the plugin said no, or the process misbehaved (counts toward the breaker).
pub enum CallError {
    Plugin(String),
    Broken(String),
}

struct Proc {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

pub struct ProcessPlugin {
    id: String,
    dir: PathBuf,
    command: Vec<String>,
    proc: tokio::sync::Mutex<Option<Proc>>,
    next: AtomicU64,
}

/// Lines longer than this (16 MiB) are refused.
const MAX_LINE: usize = 16 * 1024 * 1024;

impl ProcessPlugin {
    pub fn new(id: &str, dir: PathBuf, command: Vec<String>) -> ProcessPlugin {
        ProcessPlugin { id: id.to_string(), dir, command, proc: tokio::sync::Mutex::new(None), next: AtomicU64::new(1) }
    }

    fn spawn(&self) -> Result<Proc, String> {
        let program = self.dir.join(self.command.first().ok_or("no command")?);
        let mut cmd = tokio::process::Command::new(&program);
        cmd.args(&self.command[1..])
            .current_dir(&self.dir)
            .env_clear()
            .envs(
                std::env::vars().filter(|(k, _)| matches!(k.as_str(), "PATH" | "LANG" | "SYSTEMROOT" | "TEMP" | "TMP")),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", program.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        if let Some(err) = child.stderr.take() {
            let id = self.id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tracing::info!("plugin {id}: {}", l.chars().take(2000).collect::<String>());
                }
            });
        }
        Ok(Proc { child, stdin, stdout })
    }

    async fn exchange(p: &mut Proc, id: u64, method: &str, params: Value) -> Result<Value, CallError> {
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let mut line = serde_json::to_string(&req).map_err(|e| CallError::Broken(e.to_string()))?;
        line.push('\n');
        p.stdin.write_all(line.as_bytes()).await.map_err(|e| CallError::Broken(e.to_string()))?;
        p.stdin.flush().await.map_err(|e| CallError::Broken(e.to_string()))?;
        loop {
            let mut buf = String::new();
            let n = (&mut p.stdout)
                .take(MAX_LINE as u64 + 1)
                .read_line(&mut buf)
                .await
                .map_err(|e| CallError::Broken(e.to_string()))?;
            if n == 0 {
                return Err(CallError::Broken("the plugin exited".into()));
            }
            if buf.len() > MAX_LINE {
                return Err(CallError::Broken("answer too large".into()));
            }
            // Anything that isn't our answer (stray output) is skipped.
            let Ok(v) = serde_json::from_str::<Value>(&buf) else { continue };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                let msg = e.get("message").and_then(Value::as_str).unwrap_or("plugin error").to_string();
                return Err(CallError::Plugin(msg));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// One call (requests are sent one at a time); the process starts on first use and again
    /// after it died or timed out.
    pub async fn call(
        &self,
        settings: &HashMap<String, String>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        let mut slot = self.proc.lock().await;
        if slot.as_mut().is_some_and(|p| p.child.try_wait().ok().flatten().is_some()) {
            *slot = None;
        }
        let p = match &mut *slot {
            Some(p) => p,
            None => {
                let mut p = self.spawn().map_err(CallError::Broken)?;
                let id = self.next.fetch_add(1, Ordering::Relaxed);
                tokio::time::timeout(timeout, Self::exchange(&mut p, id, "configure", json!({ "settings": settings })))
                    .await
                    .map_err(|_| CallError::Broken("no answer to configure".into()))??;
                slot.insert(p)
            }
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        match tokio::time::timeout(timeout, Self::exchange(p, id, method, params)).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(CallError::Plugin(m))) => Err(CallError::Plugin(m)),
            Ok(Err(CallError::Broken(m))) => {
                *slot = None;
                Err(CallError::Broken(m))
            }
            Err(_) => {
                // A stuck process is killed (kill_on_drop) and restarted next time.
                *slot = None;
                Err(CallError::Broken("timed out".into()))
            }
        }
    }

    /// New settings: the running process is told, or restarted with them.
    pub async fn reconfigure(&self) {
        *self.proc.lock().await = None;
    }
}
