-- An image the user chose (a frame from playback, an upload) is locked: providers and frame
-- thumbnails never replace it.
ALTER TABLE images ADD COLUMN locked INTEGER NOT NULL DEFAULT 0;
