//! The `detect_intros` task: per season, the stretch of audio its episodes share near their
//! start (the opening theme) becomes an "intro" segment players offer to skip.

use std::sync::Arc;

use corrent_transcode::intro;

use crate::state::AppState;

/// Seasons looked at per run (decoding takes a few seconds per episode).
const SEASONS_PER_RUN: i64 = 20;
/// Intros start within the first five minutes.
const LATEST_START_MS: i64 = 5 * 60 * 1000;

pub async fn run(st: &Arc<AppState>) -> Result<String, String> {
    let seasons = st.library.intro_candidates(SEASONS_PER_RUN).await.map_err(|e| e.to_string())?;
    let tmp = st.paths.data_dir.join("transcode").join("intro");
    let worker = corrent_mpv::worker_path();
    let (mut found, mut looked) = (0, 0);
    for eps in seasons {
        // Each episode's start, fingerprinted once.
        let mut prints = Vec::with_capacity(eps.len());
        for (_, _, path, dur) in &eps {
            let secs = ((*dur as f64 / 1000.0) * 0.35).clamp(90.0, 480.0) as u32;
            let p = match intro::decode(path, secs, &tmp, worker.clone()).await {
                Ok(samples) => {
                    tokio::task::spawn_blocking(move || intro::fingerprint(&samples)).await.unwrap_or_default()
                }
                Err(e) => {
                    tracing::debug!("intro detection, {path}: {e}");
                    Vec::new()
                }
            };
            prints.push(Arc::new(p));
        }
        for (i, (_, file, _, _)) in eps.iter().enumerate() {
            looked += 1;
            // The neighbours (and the one after next, in case a neighbour is a special).
            let others: Vec<usize> =
                [i.wrapping_sub(1), i + 1, i + 2].into_iter().filter(|j| *j != i && *j < eps.len()).collect();
            let mut best: Option<(i64, i64)> = None;
            for j in others {
                let (a, b) = (prints[i].clone(), prints[j].clone());
                let r = tokio::task::spawn_blocking(move || intro::common(&a, &b)).await.unwrap_or(None);
                if let Some((s, e, _, _)) = r {
                    let t = intro::frame_secs() * 1000.0;
                    let (s, e) = ((s as f64 * t) as i64, (e as f64 * t) as i64);
                    if s <= LATEST_START_MS && best.is_none_or(|(bs, be)| e - s > be - bs) {
                        best = Some((s, e));
                    }
                }
            }
            if best.is_some() {
                found += 1;
            }
            st.library.set_intro(*file, best).await.map_err(|e| e.to_string())?;
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(format!("{found} intros in {looked} episodes"))
}
