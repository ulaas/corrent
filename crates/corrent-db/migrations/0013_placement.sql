-- The placement rules a library's items were made with (corrent-library scan::PLACEMENT_VERSION):
-- a library made with older ones is repaired once on its next scan.
ALTER TABLE libraries ADD COLUMN placement INTEGER NOT NULL DEFAULT 0;
