-- Parts of a file players can skip, found by analysis (intros shared by a season's episodes).
CREATE TABLE media_segments (
    file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    kind     TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms   INTEGER NOT NULL,
    PRIMARY KEY (file_id, kind)
);

-- Files the intro detection looked at (found something or not).
CREATE TABLE intro_checked (
    file_id    INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    checked_at INTEGER NOT NULL
);
