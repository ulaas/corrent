//! Scheduled tasks: library scans, metadata, the wanted-list search, cache cleanup and backups,
//! each on its own interval (changeable, or off), runnable by hand, with a history of runs.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use corrent_db::rusqlite::{OptionalExtension, params};
use corrent_proto::tasks::{TaskInfo, TaskRun, TaskUpdate};

use crate::state::AppState;

/// Task ids and their default intervals in minutes.
pub const TASKS: [(&str, u32); 8] = [
    ("scan_libraries", 6 * 60),
    ("monitor_shows", 6 * 60),
    ("refresh_metadata", 24 * 60),
    ("wanted_search", 15),
    ("cleanup_cache", 24 * 60),
    ("backup", 24 * 60),
    ("podcasts", 60),
    ("stop_watched_seeding", 60),
];

/// Tasks running right now.
#[derive(Default)]
pub struct Running(Mutex<HashSet<String>>);

impl Running {
    fn start(&self, id: &str) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string())
    }
    fn end(&self, id: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
    fn is(&self, id: &str) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).contains(id)
    }
}

fn run_row(r: &corrent_db::rusqlite::Row) -> corrent_db::rusqlite::Result<TaskRun> {
    Ok(TaskRun {
        id: r.get(0)?,
        task: r.get(1)?,
        started: r.get(2)?,
        finished: r.get(3)?,
        ok: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
        message: r.get(5)?,
        trigger: r.get(6)?,
    })
}

const RUN_COLS: &str = "id, task, started, finished, ok, message, trigger";

/// Settings of every task (defaults filled in).
async fn settings(st: &AppState) -> Vec<(String, bool, u32)> {
    let stored: Vec<(String, bool, u32)> = st
        .identity
        .db()
        .read(|c| {
            let mut s = c.prepare("SELECT id, enabled, every_min FROM tasks")?;
            s.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, i64>(2)? as u32)))?.collect()
        })
        .await
        .unwrap_or_default();
    all_tasks(st)
        .into_iter()
        .map(|(id, every)| stored.iter().find(|s| s.0 == id).cloned().unwrap_or((id, true, every)))
        .collect()
}

/// The built-in tasks and those of task plugins (`plugin:<id>`), with default intervals.
fn all_tasks(st: &AppState) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = TASKS.iter().map(|(id, e)| (id.to_string(), *e)).collect();
    if let Some(host) = &st.plugins {
        for p in host.list().into_iter().filter(|p| p.manifest.kinds.contains(&corrent_plugins::Kind::Task)) {
            v.push((format!("plugin:{}", p.manifest.id), p.manifest.task_every_min.unwrap_or(0)));
        }
    }
    v
}

async fn last_run(st: &AppState, id: &str) -> Option<TaskRun> {
    let id = id.to_string();
    st.identity
        .db()
        .read(move |c| {
            c.query_row(
                &format!("SELECT {RUN_COLS} FROM task_runs WHERE task = ?1 ORDER BY started DESC, id DESC LIMIT 1"),
                [id],
                run_row,
            )
            .optional()
        })
        .await
        .ok()
        .flatten()
}

pub async fn list(st: &AppState) -> Vec<TaskInfo> {
    let mut out = Vec::new();
    for (id, enabled, every_min) in settings(st).await {
        let raw = last_run(st, &id).await;
        let next_at = (enabled && every_min > 0).then(|| {
            raw.as_ref().map(|l| l.started + every_min as i64 * 60).unwrap_or_else(corrent_core::util::now_unix)
        });
        // An anchor (the interval's starting point) isn't a run.
        let last = raw.filter(|r| r.trigger != "anchor");
        out.push(TaskInfo { running: st.tasks.is(&id), id, enabled, every_min, last, next_at });
    }
    out
}

pub async fn history(st: &AppState, id: &str, limit: u32) -> Vec<TaskRun> {
    let id = id.to_string();
    st.identity
        .db()
        .read(move |c| {
            let mut s = c.prepare(&format!(
                "SELECT {RUN_COLS} FROM task_runs WHERE task = ?1 AND trigger != 'anchor' ORDER BY started DESC, id DESC LIMIT ?2"
            ))?;
            s.query_map(params![id, limit], run_row)?.collect()
        })
        .await
        .unwrap_or_default()
}

pub async fn update(st: &AppState, id: &str, u: TaskUpdate) -> Result<(), String> {
    let (_, enabled, every) = settings(st).await.into_iter().find(|s| s.0 == id).ok_or("no such task")?;
    let (enabled, every) = (u.enabled.unwrap_or(enabled), u.every_min.unwrap_or(every).min(60 * 24 * 30));
    let id = id.to_string();
    st.identity
        .db()
        .write(move |t| {
            t.execute(
                "INSERT INTO tasks(id, enabled, every_min) VALUES (?1, ?2, ?3)                  ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, every_min = excluded.every_min",
                params![id, enabled as i64, every],
            )
            .map(drop)
        })
        .await
        .map_err(|e| e.to_string())
}

/// What a task does; the message is shown in the history.
async fn execute(st: &Arc<AppState>, id: &str) -> Result<String, String> {
    match id {
        "scan_libraries" => {
            let libs = st.library.list().await.map_err(|e| e.to_string())?;
            for l in &libs {
                st.library.start_scan(l.id);
            }
            Ok(format!("{} libraries", libs.len()))
        }
        "refresh_metadata" => {
            let libs = st.library.list().await.map_err(|e| e.to_string())?;
            for l in &libs {
                crate::metadata::refresh_library(st.clone(), l.id).await;
            }
            Ok(format!("{} libraries", libs.len()))
        }
        "monitor_shows" => crate::monitor::check_all(st).await,
        "podcasts" => crate::podcasts::check_all(st).await,
        "stop_watched_seeding" => crate::integrate::stop_watched_seeding(st).await,
        plugin if plugin.starts_with("plugin:") => {
            let host = st.plugins.clone().ok_or("plugins are unavailable")?;
            host.run_task(&plugin["plugin:".len()..]).await.map_err(|e| e.to_string())
        }
        "wanted_search" => {
            if !st.settings.load().search.providers.iter().any(|p| p.enabled) {
                return Ok("no search providers".into());
            }
            let n = crate::integrate::run_wanted(st).await;
            if n > 0 {
                crate::notify::send(
                    st,
                    "release_grabbed",
                    t!("Releases found for your wanted list").to_string(),
                    t!("{n} added to the downloads.", n = n),
                );
            }
            Ok(format!("{n} grabbed"))
        }
        "cleanup_cache" => {
            let root = st.paths.data_dir.join("transcode");
            let active: HashSet<String> = st.hls.sessions().into_iter().map(|s| s.id).collect();
            let n = tokio::task::spawn_blocking(move || cleanup(&root, &active)).await.map_err(|e| e.to_string())?;
            Ok(format!("{n} removed"))
        }
        "backup" => {
            let b = crate::backups::create(st).await?;
            Ok(b.name)
        }
        _ => Err("no such task".into()),
    }
}

/// Removes transcode leftovers older than a day that no session uses.
fn cleanup(root: &std::path::Path, active: &HashSet<String>) -> usize {
    let day = Duration::from_secs(86_400);
    let mut n = 0;
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > day);
        if old && !active.contains(&name) {
            let r = if e.path().is_dir() { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
            n += r.is_ok() as usize;
        }
    }
    n
}

/// Starts a run: checks, marks the task running and records the row.
async fn begin(st: &Arc<AppState>, id: &str, trigger: &str) -> Result<TaskRun, String> {
    if !all_tasks(st).iter().any(|(t, _)| t == id) {
        return Err("no such task".into());
    }
    if !st.tasks.start(id) {
        return Err("already running".into());
    }
    let (task, trig, now) = (id.to_string(), trigger.to_string(), corrent_core::util::now_unix());
    let run_id = st
        .identity
        .db()
        .write(move |t| {
            t.execute("INSERT INTO task_runs(task, started, trigger) VALUES (?1, ?2, ?3)", params![task, now, trig])?;
            Ok(t.last_insert_rowid())
        })
        .await;
    match run_id {
        Ok(run_id) => Ok(TaskRun {
            id: run_id,
            task: id.to_string(),
            started: now,
            finished: None,
            ok: None,
            message: String::new(),
            trigger: trigger.to_string(),
        }),
        Err(e) => {
            st.tasks.end(id);
            Err(e.to_string())
        }
    }
}

/// Does the work of a begun run and records the outcome (a panic counts as a failure and
/// never leaves the task marked running).
async fn finish(st: Arc<AppState>, mut run: TaskRun) -> TaskRun {
    let (st2, id) = (st.clone(), run.task.clone());
    let result = match tokio::spawn(async move { execute(&st2, &id).await }).await {
        Ok(r) => r,
        Err(e) => Err(format!("the task crashed: {e}")),
    };
    st.tasks.end(&run.task);
    let (ok, message) = match result {
        Ok(m) => (true, m),
        Err(e) => (false, e),
    };
    if !ok {
        tracing::warn!("task {}: {message}", run.task);
        crate::notify::send(&st, "task_failed", t!("A server task failed: {task}", task = run.task), message.clone());
    }
    let (m, fin, run_id) = (message.clone(), corrent_core::util::now_unix(), run.id);
    let _ = st
        .identity
        .db()
        .write(move |t| {
            t.execute(
                "UPDATE task_runs SET finished = ?1, ok = ?2, message = ?3 WHERE id = ?4",
                params![fin, ok as i64, m, run_id],
            )?;
            // Keep the last 50 runs per task.
            t.execute(
                "DELETE FROM task_runs WHERE id IN (SELECT id FROM task_runs WHERE task = (SELECT task FROM task_runs WHERE id = ?1) \
                 ORDER BY started DESC, id DESC LIMIT -1 OFFSET 50)",
                [run_id],
            )
            .map(drop)
        })
        .await;
    run.finished = Some(fin);
    run.ok = Some(ok);
    run.message = message;
    run
}

/// Runs a task to the end (unless it is running already); `trigger` is "schedule" or a user.
pub async fn run(st: Arc<AppState>, id: &str, trigger: &str) -> Result<TaskRun, String> {
    let r = begin(&st, id, trigger).await?;
    Ok(finish(st, r).await)
}

/// Starts a task and answers right away (the run row, not finished yet).
pub async fn start_run(st: Arc<AppState>, id: &str, trigger: &str) -> Result<TaskRun, String> {
    let r = begin(&st, id, trigger).await?;
    tokio::spawn(finish(st, r.clone()));
    Ok(r)
}

/// Checks every minute which tasks are due (stops with the server state).
pub fn start(st: Arc<AppState>) {
    tokio::spawn(async move {
        let stopped = st.stopped();
        tokio::pin!(stopped);
        // Let the server settle before the first round.
        let mut tick =
            tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_secs(30), Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = &mut stopped => break,
            }
            for t in list(&st).await {
                if !t.enabled || t.every_min == 0 || t.running {
                    continue;
                }
                let now = corrent_core::util::now_unix();
                match last_run(&st, &t.id).await {
                    Some(r) if r.started + t.every_min as i64 * 60 > now => {}
                    // Never run: some tasks wait one interval (counted from an "anchor" row).
                    None if !matches!(
                        t.id.as_str(),
                        "wanted_search"
                            | "cleanup_cache"
                            | "monitor_shows"
                            | "scan_libraries"
                            | "podcasts"
                            | "stop_watched_seeding"
                    ) =>
                    {
                        let id = t.id.clone();
                        let _ = st
                            .identity
                            .db()
                            .write(move |tx| {
                                tx.execute(
                                    "INSERT INTO task_runs(task, started, trigger) VALUES (?1, ?2, 'anchor')",
                                    params![id, now],
                                )
                                .map(drop)
                            })
                            .await;
                    }
                    _ => {
                        let st = st.clone();
                        tokio::spawn(async move {
                            let _ = run(st, &t.id, "schedule").await;
                        });
                    }
                }
            }
        }
    });
}
