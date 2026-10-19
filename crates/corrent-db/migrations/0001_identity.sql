-- Identity: users, sessions/device tokens, devices, pairing codes, audit log, server key-values.
CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT NOT NULL DEFAULT '',
    pw_hash       TEXT NOT NULL,
    role          TEXT NOT NULL,
    perms         INTEGER,            -- NULL = the role's defaults
    max_rating    TEXT,
    disabled      INTEGER NOT NULL DEFAULT 0,
    failed_logins INTEGER NOT NULL DEFAULT 0,
    locked_until  INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

CREATE TABLE devices (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    platform   TEXT NOT NULL DEFAULT '',
    client     TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    last_ip    TEXT NOT NULL DEFAULT ''
);

-- Tokens are stored as SHA-256 of the secret; the secret is only ever shown to its owner.
CREATE TABLE sessions (
    id         INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id  INTEGER REFERENCES devices(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,          -- 'web' | 'device'
    csrf       TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    ip         TEXT NOT NULL DEFAULT '',
    user_agent TEXT NOT NULL DEFAULT ''
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX sessions_expiry ON sessions(expires_at);

CREATE TABLE pairing_codes (
    code       TEXT PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE audit (
    id      INTEGER PRIMARY KEY,
    ts      INTEGER NOT NULL,
    user    TEXT,
    ip      TEXT NOT NULL DEFAULT '',
    action  TEXT NOT NULL,
    target  TEXT NOT NULL DEFAULT '',
    ok      INTEGER NOT NULL,
    details TEXT NOT NULL DEFAULT ''
);
CREATE INDEX audit_ts ON audit(ts);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
