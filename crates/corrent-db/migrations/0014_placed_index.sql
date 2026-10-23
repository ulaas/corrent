-- Items by the title the scanner gave them (extra.placed): matching a file to its movie or show
-- looks it up for every file scanned.
CREATE INDEX items_placed ON items(library_id, kind, json_extract(extra, '$.placed'));
