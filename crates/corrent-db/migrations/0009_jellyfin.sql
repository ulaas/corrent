-- Jellyfin-compatible API: a Jellyfin app's own device id (re-logins replace the device), and
-- the documents those apps store per user (UserConfiguration, DisplayPreferences).
ALTER TABLE devices ADD COLUMN external_id TEXT;
CREATE INDEX devices_external ON devices(user_id, external_id);

CREATE TABLE jf_prefs (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    json       TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, key)
);
