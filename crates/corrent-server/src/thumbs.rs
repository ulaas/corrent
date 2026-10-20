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

/// Takes the frame at `at` seconds (default: the automatic position) of an item's video as its
/// `kind` image (`thumb` or `poster`), `lock`ed or not; a movie without a real poster shows a new
/// thumbnail there too. Returns whether it was stored.
pub async fn frame_as_image(
    st: &AppState,
    user: i64,
    item: i64,
    at: Option<f64>,
    kind: &str,
    lock: bool,
) -> Result<bool, String> {
    if !matches!(kind, "thumb" | "poster") {
        return Err("kind must be thumb or poster".into());
    }
    let (_, path) = st.library.main_file(user, item).await.map_err(|e| e.to_string())?;
    let d = st.library.item(user, item).await.map_err(|e| e.to_string())?;
    let episode = d.item.kind == corrent_proto::library::ItemKind::Episode;
    let at = at.unwrap_or_else(|| corrent_transcode::frame::position_secs(d.item.runtime_ms, episode));
    let img = match frame(st, &path, at).await {
        Err(_) if d.item.runtime_ms.is_none() => frame(st, &path, 1.0).await,
        r => r,
    }?;
    if kind == "thumb" && d.item.kind == corrent_proto::library::ItemKind::Movie {
        let _ = st.library.set_artwork(item, "poster", img.clone(), FRAME, Replace::IfMissingOrFrame, false).await;
    }
    st.library.set_artwork(item, kind, img, FRAME, Replace::Always, lock).await.map_err(|e| e.to_string())
}

/// Passes over the videos that have no thumbnail yet (one at a time, server-wide), until none
/// is left.
pub async fn run(st: &Arc<AppState>) -> Result<String, String> {
    let _one = st.thumb_slot.lock().await;
    let (mut made, mut failed) = (0, 0);
    loop {
        st.thumb_again.store(false, std::sync::atomic::Ordering::Relaxed);
        let wanted = st.library.items_needing_thumbs(PER_RUN).await.map_err(|e| e.to_string())?;
        let full = wanted.len() as i64 == PER_RUN;
        pass(st, wanted, &mut made, &mut failed).await;
        let again = st.thumb_again.load(std::sync::atomic::Ordering::Relaxed);
        if *st.stopping.borrow() || !(full || again) {
            break;
        }
    }
    Ok(format!("{made} thumbnails, {failed} failed"))
}

async fn pass(st: &Arc<AppState>, wanted: Vec<corrent_library::meta::ThumbWanted>, made: &mut u32, failed: &mut u32) {
    for w in wanted {
        if *st.stopping.borrow() {
            break;
        }
        let at = corrent_transcode::frame::position_secs(w.runtime_ms, w.kind == "episode");
        // Length unknown (a probe that failed): a short clip has no frame at 30 s, try 1 s in.
        let got = match frame(st, &w.path, at).await {
            Err(_) if w.runtime_ms.is_none() => frame(st, &w.path, 1.0).await,
            r => r,
        };
        match got {
            Ok(jpg) => {
                // A movie without a poster shows the frame there too (cropped to a poster).
                if w.kind == "movie" && !w.has_poster {
                    let _ = st
                        .library
                        .set_artwork(w.item, "poster", jpg.clone(), FRAME, Replace::IfMissingOrFrame, false)
                        .await;
                }
                match st.library.set_artwork(w.item, "thumb", jpg, FRAME, Replace::IfMissingOrFrame, false).await {
                    Ok(_) => *made += 1,
                    Err(e) => {
                        tracing::debug!("thumbnail of item {}: {e}", w.item);
                        *failed += 1;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("thumbnail of item {} ({}): {e}", w.item, w.path);
                *failed += 1;
            }
        }
        // Tried either way: a file without a picture isn't decoded again on every run.
        let _ = st.library.mark_thumb_tried(w.item).await;
    }
}

/// Runs passes in the background (after a scan). When one is running already, it is asked to
/// go round once more instead of a second one queueing up.
pub fn start(st: &Arc<AppState>) {
    if st.thumb_slot.try_lock().is_err() {
        st.thumb_again.store(true, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    let st = st.clone();
    tokio::spawn(async move {
        if let Err(e) = run(&st).await {
            tracing::warn!("video thumbnails: {e}");
        }
    });
}
