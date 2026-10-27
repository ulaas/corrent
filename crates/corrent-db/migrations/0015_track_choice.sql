-- The audio and subtitle track each user last chose for an item (stream index; subtitle 0 = off),
-- so playback starts with them again on every client.
ALTER TABLE watch_state ADD COLUMN audio_track INTEGER;
ALTER TABLE watch_state ADD COLUMN subtitle_track INTEGER;
