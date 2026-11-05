-- Foreign keys are checked from the parent's side: without an index on the child column, every
-- deleted item or library scans the whole child table on the writer thread.
CREATE INDEX watch_state_item ON watch_state(item_id);
CREATE INDEX playlist_items_item ON playlist_items(item_id);
CREATE INDEX collection_items_item ON collection_items(item_id);
CREATE INDEX grabs_item ON grabs(item_id);
CREATE INDEX grabs_wanted ON grabs(wanted_id);
CREATE INDEX wanted_library ON wanted(library_id);
CREATE INDEX wanted_profile ON wanted(profile_id);
CREATE INDEX monitored_profile ON monitored_shows(profile_id);
CREATE INDEX collections_library ON collections(library_id);
