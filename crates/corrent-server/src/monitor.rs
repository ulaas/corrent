//! Followed shows: episodes that have aired (per the show's metadata provider) and that the
//! library doesn't have go on the wanted list, where the wanted search picks them up.

use std::sync::Arc;

use corrent_plugins::MediaKind;
use corrent_proto::library::{MonitoredShow, NewWanted, WantedKind};

use crate::state::AppState;

/// Today as `YYYY-MM-DD` (UTC; air dates are compared as text).
fn today() -> String {
    let days = corrent_core::util::now_unix().div_euclid(86_400);
    let (y, m, d) = corrent_core::util::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Adds the missing aired episodes of one show; returns how many were added.
pub async fn check_show(st: &AppState, show: &MonitoredShow) -> Result<usize, String> {
    let host = st.plugins.clone().ok_or("plugins are unavailable")?;
    let (library_id, title, ids) = st.library.show_info(show.item_id).await.map_err(|e| e.to_string())?;
    let providers = crate::metadata::providers_for(&host, MediaKind::Show).await;
    let (provider, pid) = providers
        .iter()
        .find_map(|p| ids.get(p).map(|id| (p.clone(), id.clone())))
        .ok_or("the show isn't identified with a metadata provider yet")?;
    let language = st
        .library
        .get(library_id)
        .await
        .ok()
        .map(|l| l.settings.language)
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en".into());
    let have = st.library.show_episodes(show.item_id).await.map_err(|e| e.to_string())?;
    let today = today();
    let mut added = 0;
    for season in show.from_season.max(1)..=99 {
        let eps = match host.metadata_episodes(&provider, pid.clone(), season as u32, language.clone()).await {
            Ok(e) if !e.is_empty() => e,
            _ => break,
        };
        for e in eps {
            let ep = e.episode as i64;
            // Only what has aired (unknown dates are skipped: not out yet, or not listed).
            if e.aired.as_deref().is_none_or(|a| a > today.as_str()) || have.contains(&(season, ep)) {
                continue;
            }
            if st.library.episode_wanted(&title, season, ep).await.map_err(|e| e.to_string())? {
                continue;
            }
            let w = NewWanted {
                library_id: Some(library_id),
                kind: Some(WantedKind::Episode),
                title: title.clone(),
                season: Some(season),
                episode: Some(ep),
                profile_id: show.profile_id,
                ..Default::default()
            };
            st.library.add_wanted(w, "monitor").await.map_err(|e| e.to_string())?;
            added += 1;
        }
    }
    let _ = st.library.mark_monitor_checked(show.item_id).await;
    Ok(added)
}

/// Every followed show (the `monitor_shows` task).
pub async fn check_all(st: &Arc<AppState>) -> Result<String, String> {
    let shows = st.library.monitored().await.map_err(|e| e.to_string())?;
    let (mut added, mut failed) = (0, 0);
    for s in &shows {
        match check_show(st, s).await {
            Ok(n) => added += n,
            Err(e) => {
                failed += 1;
                tracing::info!("monitor {}: {e}", s.title);
            }
        }
    }
    Ok(format!("{} shows, {added} episodes wanted, {failed} not checked", shows.len()))
}
