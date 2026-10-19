-- Media library: libraries and their folders, files with probe results, the item tree
-- (movie / show > season > episode / artist > album > track / book / video / photo),
-- streams, chapters, people, genres, artwork, per-user watch state, playlists, collections,
-- and a full-text index.

CREATE TABLE libraries (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,              -- movies | shows | music | audiobooks | videos | photos
    settings   TEXT NOT NULL DEFAULT '{}', -- JSON: import categories, metadata policy, …
    created_at INTEGER NOT NULL,
    scanned_at INTEGER
);

CREATE TABLE library_folders (
    id         INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    watch      INTEGER NOT NULL DEFAULT 1,
    UNIQUE (library_id, path)
);

CREATE TABLE files (
    id            INTEGER PRIMARY KEY,
    library_id    INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    path          TEXT NOT NULL UNIQUE,
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    quick_hash    TEXT NOT NULL DEFAULT '',
    probe         TEXT,                    -- JSON from the prober
    probed_at     INTEGER,
    missing_since INTEGER,
    source        TEXT                     -- JSON: {"torrent": hash, "file": index}
);
CREATE INDEX files_library ON files(library_id);
CREATE INDEX files_hash ON files(quick_hash);

CREATE TABLE items (
    id             INTEGER PRIMARY KEY,
    library_id     INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL,          -- movie show season episode artist album track book video photo
    parent_id      INTEGER REFERENCES items(id) ON DELETE CASCADE,
    title          TEXT NOT NULL,
    sort_title     TEXT NOT NULL,
    original_title TEXT,
    year           INTEGER,
    index_no       INTEGER,                -- season / episode / track / disc number
    index2         INTEGER,                -- last episode of a multi-episode file, disc of a track
    overview       TEXT,
    runtime_ms     INTEGER,
    rating         REAL,
    official_rating TEXT,
    premiered      TEXT,
    added_at       INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    locked_fields  TEXT NOT NULL DEFAULT '',  -- comma-separated field names metadata must not change
    provider_ids   TEXT NOT NULL DEFAULT '{}', -- JSON {"tmdb": "…", "imdb": "…", "musicbrainz": "…"}
    extra          TEXT NOT NULL DEFAULT '{}'  -- JSON: artist names, album artist, replaygain, …
);
CREATE INDEX items_library_kind ON items(library_id, kind, sort_title);
CREATE INDEX items_parent ON items(parent_id, index_no);
CREATE INDEX items_added ON items(added_at);

CREATE TABLE item_files (
    item_id INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    role    TEXT NOT NULL DEFAULT 'main',   -- main | extra | trailer | theme
    PRIMARY KEY (item_id, file_id)
);
CREATE INDEX item_files_file ON item_files(file_id);

CREATE TABLE streams (
    file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    idx         INTEGER NOT NULL,
    kind        TEXT NOT NULL,              -- video | audio | subtitle
    codec       TEXT,
    profile     TEXT,
    lang        TEXT,
    title       TEXT,
    is_default  INTEGER NOT NULL DEFAULT 0,
    forced      INTEGER NOT NULL DEFAULT 0,
    width       INTEGER,
    height      INTEGER,
    fps         REAL,
    bitrate     INTEGER,
    channels    INTEGER,
    sample_rate INTEGER,
    external_path TEXT,
    PRIMARY KEY (file_id, kind, idx)
);

CREATE TABLE chapters (
    file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    idx      INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    title    TEXT,
    PRIMARY KEY (file_id, idx)
);

CREATE TABLE people (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    provider_ids TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE item_people (
    item_id   INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    role      TEXT NOT NULL,                -- actor | director | writer | artist | author | composer
    character TEXT,
    ord       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (item_id, person_id, role)
);
CREATE INDEX item_people_person ON item_people(person_id);

CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
CREATE TABLE item_genres (
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (item_id, genre_id)
);

-- Content-addressed artwork: artwork/<sha[0..2]>/<sha>.jpg plus .w200/.w400/.w800 variants.
CREATE TABLE images (
    id         INTEGER PRIMARY KEY,
    owner_kind TEXT NOT NULL,               -- item | person | collection | library | user
    owner_id   INTEGER NOT NULL,
    kind       TEXT NOT NULL,               -- poster | backdrop | logo | thumb | banner | art
    sha256     TEXT NOT NULL,
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    blurhash   TEXT NOT NULL DEFAULT '',
    source     TEXT NOT NULL DEFAULT '',    -- local path, "embedded", or provider URL
    UNIQUE (owner_kind, owner_id, kind)
);

CREATE TABLE watch_state (
    user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id        INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    position_ms    INTEGER NOT NULL DEFAULT 0,
    played         INTEGER NOT NULL DEFAULT 0,
    play_count     INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER,
    favorite       INTEGER NOT NULL DEFAULT 0,
    rating         REAL,
    PRIMARY KEY (user_id, item_id)
);
CREATE INDEX watch_state_recent ON watch_state(user_id, last_played_at);

CREATE TABLE playlists (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    public     INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE TABLE playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    pos         INTEGER NOT NULL,
    item_id     INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY (playlist_id, pos)
);

CREATE TABLE collections (
    id           INTEGER PRIMARY KEY,
    library_id   INTEGER REFERENCES libraries(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    provider_ids TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE collection_items (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    item_id       INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY (collection_id, item_id)
);

-- Which libraries a user may see (no rows = all libraries).
CREATE TABLE user_libraries (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, library_id)
);

-- Full-text search over titles, people and overviews (rebuilt per item by the library).
CREATE VIRTUAL TABLE items_fts USING fts5(
    title, original_title, people, overview,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);
