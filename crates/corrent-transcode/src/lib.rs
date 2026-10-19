//! Playback for clients that can't play a file as it is: the direct-play/transcode decision,
//! HLS sessions encoded by libmpv (in the `corrent-mpvw` worker), WebVTT subtitles and trickplay
//! thumbnails.

pub mod decide;
pub mod hls;
pub mod hw;
pub mod trickplay;
pub mod vtt;

#[cfg(test)]
mod tests;
