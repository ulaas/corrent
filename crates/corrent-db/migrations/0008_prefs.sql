-- Each user's client preferences (theme, home layout, card size…), one JSON document,
-- shared by their devices.
CREATE TABLE user_prefs (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    json       TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
