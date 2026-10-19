//! Scheduled tasks, notification targets and backups (admin pages).

use serde::{Deserialize, Serialize};

/// One run of a task.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: i64,
    pub task: String,
    pub started: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default)]
    pub message: String,
    /// "schedule" or the user who started it.
    pub trigger: String,
}

/// A task (`GET /api/v1/tasks`). Clients name tasks by `id` (translated on their side).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub enabled: bool,
    /// Minutes between runs (0 = only by hand).
    pub every_min: u32,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<TaskRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskUpdate {
    pub enabled: Option<bool>,
    pub every_min: Option<u32>,
}

/// Events a notification target can subscribe to.
pub const NOTIFY_EVENTS: [&str; 5] =
    ["download_done", "torrent_added", "release_grabbed", "device_paired", "task_failed"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    /// POST of a JSON object `{event, title, body, time}`.
    #[default]
    Webhook,
    /// A Discord webhook URL.
    Discord,
    /// An ntfy topic URL (`https://ntfy.sh/<topic>` or self-hosted).
    Ntfy,
    /// A notifier plugin.
    Plugin,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyTarget {
    pub id: i64,
    pub name: String,
    pub kind: NotifyKind,
    pub url: String,
    pub plugin: String,
    /// Empty = every event.
    pub events: Vec<String>,
    pub enabled: bool,
}

/// A backup file on the server (`GET /api/v1/backups`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BackupFile {
    pub name: String,
    pub size: u64,
    pub created: i64,
}
