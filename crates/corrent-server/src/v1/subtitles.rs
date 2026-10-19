//! Finding subtitles with subtitle-provider plugins and saving them next to the video.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use corrent_plugins::{Kind, Subtitle, SubtitleQuery};
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use serde::{Deserialize, Serialize};

use super::library::lib_err;
use super::{ApiResult, Auth, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/items/{id}/subtitles", get(search).post(download))
}

/// The OpenSubtitles file hash: the size plus the sums of the first and last 64 KiB as 64-bit
/// little-endian words.
pub fn opensubtitles_hash(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut hash = size;
    let mut sum = |f: &mut std::fs::File, at: u64| -> std::io::Result<()> {
        f.seek(SeekFrom::Start(at))?;
        let mut buf = vec![0u8; CHUNK.min(size) as usize];
        f.read_exact(&mut buf)?;
        for w in buf.as_chunks::<8>().0 {
            hash = hash.wrapping_add(u64::from_le_bytes(*w));
        }
        Ok(())
    };
    sum(&mut f, 0)?;
    sum(&mut f, size.saturating_sub(CHUNK))?;
    Ok(format!("{hash:016x}"))
}

#[derive(Deserialize)]
struct SearchParams {
    /// Comma-separated ISO 639-1 codes, most wanted first.
    #[serde(default)]
    languages: String,
}

#[derive(Serialize, Deserialize)]
pub struct Found {
    pub provider: String,
    #[serde(flatten)]
    pub subtitle: Subtitle,
}

/// Asks every ready subtitle provider (exact file matches first, then by downloads).
async fn search(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Query(p): Query<SearchParams>,
) -> ApiResult<Vec<Found>> {
    let host = st.plugins.clone().ok_or_else(|| err(ErrorCode::EngineUnavailable, "plugins are unavailable"))?;
    let t = st.library.subtitle_target(auth.0.user.id, id).await.map_err(lib_err)?;
    let path = std::path::PathBuf::from(&t.path);
    let hash = tokio::task::spawn_blocking(move || opensubtitles_hash(&path).ok()).await.ok().flatten();
    let mut languages: Vec<String> =
        p.languages.split(',').map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).take(5).collect();
    if languages.is_empty() {
        languages.push("en".into());
    }
    let q = SubtitleQuery {
        title: t.title.clone(),
        year: t.year.map(|y| y as u32),
        season: t.season.map(|s| s as u32),
        episode: t.episode.map(|e| e as u32),
        languages,
        ids: t.ids.clone().into_iter().collect(),
        file_hash: hash,
        file_size: Some(t.size.max(0) as u64),
    };
    let mut out = Vec::new();
    for provider in host.ready(Kind::Subtitles) {
        match host.subtitles_search(&provider, q.clone()).await {
            Ok(list) => out.extend(list.into_iter().map(|s| Found { provider: provider.clone(), subtitle: s })),
            Err(e) => tracing::info!("subtitles from {provider}: {e}"),
        }
    }
    out.sort_by(|a, b| b.subtitle.exact.cmp(&a.subtitle.exact).then(b.subtitle.downloads.cmp(&a.subtitle.downloads)));
    Ok(Json(out))
}

#[derive(Deserialize)]
struct DownloadRequest {
    provider: String,
    id: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Saved {
    pub path: String,
}

fn safe_part(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(12).collect()
}

/// Downloads one subtitle and saves it as `<video>.<lang>.<ext>` beside the video.
async fn download(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(r): Json<DownloadRequest>,
) -> ApiResult<Saved> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let host = st.plugins.clone().ok_or_else(|| err(ErrorCode::EngineUnavailable, "plugins are unavailable"))?;
    let t = st.library.subtitle_target(auth.0.user.id, id).await.map_err(lib_err)?;
    let f = host.subtitles_download(&r.provider, r.id).await.map_err(|e| err(ErrorCode::BadRequest, e.to_string()))?;
    if f.data.len() > 10 * 1024 * 1024 {
        return Err(err(ErrorCode::PayloadTooLarge, "subtitle too large"));
    }
    let ext = match safe_part(&f.format).to_ascii_lowercase().as_str() {
        e @ ("srt" | "ass" | "ssa" | "vtt" | "sub") => e.to_string(),
        _ => "srt".to_string(),
    };
    let lang = r.language.as_deref().map(safe_part).filter(|l| !l.is_empty());
    let video = std::path::PathBuf::from(&t.path);
    let stem = video.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = video.parent().ok_or_else(|| err(ErrorCode::Internal, "no folder"))?.to_path_buf();
    let mut name = match &lang {
        Some(l) => format!("{stem}.{l}.{ext}"),
        None => format!("{stem}.{ext}"),
    };
    // Never overwrite a subtitle that is already there.
    for n in 2..100 {
        if !dir.join(&name).exists() {
            break;
        }
        name = match &lang {
            Some(l) => format!("{stem}.{l}.{n}.{ext}"),
            None => format!("{stem}.{n}.{ext}"),
        };
    }
    let target = dir.join(&name);
    let (t2, data) = (target.clone(), f.data);
    tokio::task::spawn_blocking(move || corrent_core::util::write_atomic(&t2, &data))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?
        .map_err(|e| err(ErrorCode::Forbidden, format!("can't write next to the video: {e}")))?;
    let path = target.display().to_string();
    st.library.add_external_subtitle(t.file_id, path.clone(), lang).await.map_err(lib_err)?;
    Ok(Json(Saved { path }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn hash_matches_the_reference() {
        // The documented example: a file of 131072 bytes with zero content hashes to its size.
        let d = std::env::temp_dir().join(format!("corrent-oshash-{}", std::process::id()));
        std::fs::write(&d, vec![0u8; 131_072]).unwrap();
        assert_eq!(super::opensubtitles_hash(&d).unwrap(), format!("{:016x}", 131_072u64));
        let _ = std::fs::remove_file(&d);
    }
}
