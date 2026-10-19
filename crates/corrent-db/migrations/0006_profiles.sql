-- Profile PINs: switching to a profile on a shared device (TV, family computer) asks for it.
ALTER TABLE users ADD COLUMN pin_hash TEXT;
