-- Scheduled tasks: settings per task and the history of runs.
CREATE TABLE tasks (
    id        TEXT PRIMARY KEY,
    enabled   INTEGER NOT NULL DEFAULT 1,
    -- Minutes between runs (0 = only by hand).
    every_min INTEGER NOT NULL
);

CREATE TABLE task_runs (
    id       INTEGER PRIMARY KEY,
    task     TEXT NOT NULL,
    started  INTEGER NOT NULL,
    finished INTEGER,
    ok       INTEGER,
    message  TEXT NOT NULL DEFAULT '',
    -- "schedule" or the user who started it.
    trigger  TEXT NOT NULL DEFAULT 'schedule'
);
CREATE INDEX task_runs_by_task ON task_runs(task, started DESC);

-- Where notifications go.
CREATE TABLE notify_targets (
    id       INTEGER PRIMARY KEY,
    name     TEXT NOT NULL,
    -- "webhook" (JSON), "discord", "ntfy" or "plugin".
    kind     TEXT NOT NULL,
    url      TEXT NOT NULL DEFAULT '',
    plugin   TEXT NOT NULL DEFAULT '',
    -- Comma-separated event names; empty = all.
    events   TEXT NOT NULL DEFAULT '',
    enabled  INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
);
