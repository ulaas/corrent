-- A light Sonarr/Radarr: quality profiles, things people want, and what was grabbed for them.
CREATE TABLE quality_profiles (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL UNIQUE,
    rules TEXT NOT NULL DEFAULT '{}'   -- JSON QualityRules
);

CREATE TABLE wanted (
    id          INTEGER PRIMARY KEY,
    library_id  INTEGER REFERENCES libraries(id) ON DELETE SET NULL,
    kind        TEXT NOT NULL,         -- movie | episode | season | show | album | other
    title       TEXT NOT NULL,
    year        INTEGER,
    season      INTEGER,
    episode     INTEGER,
    query       TEXT,                  -- overrides the search text built from the fields above
    profile_id  INTEGER REFERENCES quality_profiles(id) ON DELETE SET NULL,
    status      TEXT NOT NULL DEFAULT 'wanted',  -- wanted | grabbed | done | failed
    torrent     TEXT,                  -- info hash of the grab
    added_by    TEXT,
    created_at  INTEGER NOT NULL,
    searched_at INTEGER
);
CREATE INDEX wanted_status ON wanted(status, searched_at);

CREATE TABLE grabs (
    id        INTEGER PRIMARY KEY,
    wanted_id INTEGER REFERENCES wanted(id) ON DELETE SET NULL,
    item_id   INTEGER REFERENCES items(id) ON DELETE SET NULL,
    torrent   TEXT NOT NULL,
    title     TEXT NOT NULL,
    score     INTEGER NOT NULL DEFAULT 0,
    at        INTEGER NOT NULL
);

INSERT INTO quality_profiles(name, rules) VALUES
  ('Any', '{"auto_grab":true}'),
  ('HD 1080p', '{"min_resolution":720,"max_resolution":1080,"preferred_resolution":1080,"min_seeders":2,"auto_grab":true}'),
  ('Ultra HD', '{"min_resolution":2160,"preferred_resolution":2160,"min_seeders":2,"auto_grab":true}');
