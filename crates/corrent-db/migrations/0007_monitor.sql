-- Followed shows: newly aired episodes that the library doesn't have become wanted entries.
CREATE TABLE monitored_shows (
    item_id     INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    profile_id  INTEGER REFERENCES quality_profiles(id) ON DELETE SET NULL,
    from_season INTEGER NOT NULL DEFAULT 1,
    added_by    TEXT,
    created_at  INTEGER NOT NULL,
    checked_at  INTEGER
);
