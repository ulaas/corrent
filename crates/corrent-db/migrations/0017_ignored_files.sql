-- Files a library leaves out although they are in one of its folders: media someone removed from
-- the library without deleting it. The scan skips them until they are restored.
CREATE TABLE ignored_files (
    library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    title      TEXT NOT NULL DEFAULT '',   -- what its item was called, for the list
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (library_id, path)
);
