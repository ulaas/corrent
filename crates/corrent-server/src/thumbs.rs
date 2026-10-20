//! Frame thumbnails: every video, movie and episode without artwork gets a frame from itself
//! (15–20 % in), so no card is left blank. Provider and local artwork replace a frame later; an
//! image the user chose is never replaced.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use corrent_library::meta::{FRAME, Replace};

use crate::state::AppState;

/// Frames are this wide (the artwork store makes the smaller sizes).
pub const WIDTH: u32 = 640;
/// Videos handled per run (the next run, or the next scan, does the rest).
const PER_RUN: i64 = 300;

/// The frame at `at_secs` of `path` as JPEG bytes.
pub async fn frame(st: &AppState, path: &str, at_secs: f64) -> Result<Vec<u8>, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = st.paths.data_dir.join("cache").join(format!("frame-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
    corrent_transcode::frame::extract(path, at_secs, WIDTH, corrent_mpv::worker_path(), &tmp).await
}

/// One pass over the videos that have no thumbnail yet (one at a time, server-wide).
pub async fn run(st: &Arc<AppState>) -> Result<String, String> {
    let _one = st.thumb_slot.lock().await;
    let wanted = st.library.items_needing_thumbs(PER_RUN).await.map_err(|e| e.to_string())?;
    let (mut made, mut failed) = (0, 0);
    for w in wanted {
        if *st.stopping.borrow() {
            break;
        }
        let at = corrent_transcode::frame::position_secs(w.runtime_ms, w.kind == "episode");
        match frame(st, &w.path, at).await {
            Ok(jpg) => {
                // A movie without a poster shows the frame there too (cropped to a poster).
                if w.kind == "movie" && !w.has_poster {
                    let _ = st
                        .library
                        .set_artwork(w.item, "poster", jpg.clone(), FRAME, Replace::IfMissingOrFrame, false)
                        .await;
                }
                match st.library.set_artwork(w.item, "thumb", jpg, FRAME, Replace::IfMissingOrFrame, false).await {
                    Ok(_) => made += 1,
                    Err(e) => {
                        tracing::debug!("thumbnail of item {}: {e}", w.item);
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("thumbnail of item {} ({}): {e}", w.item, w.path);
                failed += 1;
            }
        }
        // Tried either way: a file without a picture isn't decoded again on every run.
        let _ = st.library.mark_thumb_tried(w.item).await;
    }
    Ok(format!("{made} thumbnails, {failed} failed"))
}

/// Runs a pass in the background (after a scan); a pass already running picks up new items next
/// time.
pub fn start(st: &Arc<AppState>) {
    let st = st.clone();
    tokio::spawn(async move {
        if let Err(e) = run(&st).await {
            tracing::warn!("video thumbnails: {e}");
        }
    });
}
