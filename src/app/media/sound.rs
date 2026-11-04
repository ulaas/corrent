//! The player's sound: what it hands over to be saved.

use std::sync::Mutex;

/// A closing player has no way to the settings, so what is left to save waits here for the next
/// frame.
static VOLUME: Mutex<Option<(f32, bool)>> = Mutex::new(None);

pub(super) fn post_volume(volume: f32, muted: bool) {
    *crate::util::lock(&VOLUME) = Some((volume, muted));
}

pub(super) fn take_volume() -> Option<(f32, bool)> {
    crate::util::lock(&VOLUME).take()
}
