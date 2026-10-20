//! The media library over HTTP: libraries, scans, items, watch state, home rows, search,
//! artwork and direct file serving (with Range, for players and the web client).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use corrent_library::LibError;
use corrent_proto::ErrorCode;
use corrent_proto::auth::Perms;
use corrent_proto::library::{
    HomeRow, Item, ItemDetail, ItemKind, ItemQuery, Library, LibraryUpdate, NewLibrary, Page, ScanStatus,
    SearchResults, SortBy, StateUpdate, UserState,
};
use serde::Deserialize;
use tower::ServiceExt;

use super::{ApiErr, ApiResult, Auth, ClientIp, err};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/libraries", get(list).post(create))
        .route("/libraries/{id}", get(one).patch(update).delete(remove))
        .route("/libraries/{id}/scan", get(scan_status).post(scan).delete(cancel_scan))
        .route("/items", get(items))
        .route("/items/{id}", get(item).patch(edit))
        .route("/items/{id}/state", put(set_state))
        .route("/items/{id}/file", get(item_file))
        .route("/items/{id}/thumbnail", post(thumbnail_from_frame))
        .route("/items/{id}/lyrics", get(lyrics))
        .route("/files/{id}", get(file))
        .route("/images/{sha}", get(image))
        .route("/home", get(home))
        .route("/library/search", get(search))
        .route("/users/{id}/libraries", get(user_libraries).put(set_user_libraries))
}

trait WithId {
    fn with_id(self, id: i64) -> Self;
}

impl WithId for ApiErr {
    fn with_id(self, id: i64) -> Self {
        ApiErr(self.0.with("id", id))
    }
}

pub fn lib_err(e: LibError) -> ApiErr {
    match e {
        LibError::NotFound => err(ErrorCode::NotFound, "not found"),
        LibError::Invalid(m) => err(ErrorCode::BadRequest, m),
        LibError::Db(m) => err(ErrorCode::Internal, m),
    }
}

/// Refreshes the folders confined users may use (library folders are among them).
pub async fn refresh_roots(st: &AppState) {
    if let Ok(libs) = st.library.list().await {
        st.library_roots.store(Arc::new(libs.iter().flat_map(|l| l.folders.iter().map(PathBuf::from)).collect()));
    }
}

async fn visible_libraries(st: &AppState, auth: &Auth) -> Result<Vec<Library>, ApiErr> {
    let all = st.library.list().await.map_err(lib_err)?;
    let allowed = st.library.user_libraries(auth.0.user.id).await.map_err(lib_err)?;
    Ok(if allowed.is_empty() { all } else { all.into_iter().filter(|l| allowed.contains(&l.id)).collect() })
}

async fn list(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<Library>> {
    Ok(Json(visible_libraries(&st, &auth).await?))
}

async fn one(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Library> {
    visible_libraries(&st, &auth)
        .await?
        .into_iter()
        .find(|l| l.id == id)
        .map(Json)
        .ok_or_else(|| err(ErrorCode::NotFound, "no such library"))
}

/// Library folders are read by the server: same confinement as download folders.
fn check_folders(st: &AppState, auth: &Auth, folders: &[String]) -> Result<Vec<String>, ApiErr> {
    folders
        .iter()
        .filter(|f| !f.trim().is_empty())
        .map(|f| crate::paths::check(st, auth, &PathBuf::from(f)).map(|p| p.display().to_string()))
        .collect()
}

async fn create(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Json(mut l): Json<NewLibrary>,
) -> ApiResult<Library> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    l.folders = check_folders(&st, &auth, &l.folders)?;
    let r = st.library.create(l).await.map_err(lib_err);
    st.identity
        .audit(
            Some(auth.name()),
            &ip.0.to_string(),
            "library_create",
            &r.as_ref().map(|l| l.name.clone()).unwrap_or_default(),
            r.is_ok(),
            "",
        )
        .await;
    let lib = r?;
    refresh_roots(&st).await;
    crate::integrate::ensure_category_folders(&st).await;
    st.library.start_scan(lib.id);
    crate::restart_watching(&st).await;
    Ok(Json(lib))
}

async fn update(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(mut u): Json<LibraryUpdate>,
) -> ApiResult<Library> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    if let Some(f) = &u.folders {
        u.folders = Some(check_folders(&st, &auth, f)?);
    }
    let rescan = u.folders.is_some();
    let r = st.library.update(id, u).await.map_err(lib_err);
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "library_update", &id.to_string(), r.is_ok(), "").await;
    let lib = r?;
    refresh_roots(&st).await;
    crate::integrate::ensure_category_folders(&st).await;
    if rescan {
        st.library.start_scan(id);
    }
    crate::restart_watching(&st).await;
    Ok(Json(lib))
}

async fn remove(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let r = st.library.delete(id).await.map_err(lib_err);
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "library_delete", &id.to_string(), r.is_ok(), "").await;
    r?;
    refresh_roots(&st).await;
    crate::restart_watching(&st).await;
    Ok(Json(serde_json::json!({})))
}

async fn scan(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    st.library.get(id).await.map_err(lib_err)?;
    let started = st.library.start_scan(id);
    Ok(Json(serde_json::json!({ "started": started })))
}

async fn cancel_scan(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    st.library.cancel_scan(id);
    Ok(Json(serde_json::json!({})))
}

async fn scan_status(State(st): State<Arc<AppState>>, _auth: Auth, Path(id): Path<i64>) -> ApiResult<ScanStatus> {
    Ok(Json(st.library.scan_status(id).unwrap_or(ScanStatus { library_id: id, ..Default::default() })))
}

/// `GET /items?library_id=&parent_id=&kind=&genre=&person_id=&favorite=&played=&sort=&desc=&offset=&limit=`
#[derive(Deserialize, Default)]
#[serde(default)]
struct ItemsQuery {
    library_id: Option<i64>,
    parent_id: Option<i64>,
    kind: Option<String>,
    genre: Option<String>,
    person_id: Option<i64>,
    favorite: Option<bool>,
    played: Option<bool>,
    sort: Option<String>,
    desc: bool,
    offset: i64,
    limit: Option<i64>,
}

async fn items(State(st): State<Arc<AppState>>, auth: Auth, Query(q): Query<ItemsQuery>) -> ApiResult<Page<Item>> {
    let kind = match q.kind.as_deref() {
        Some(k) => Some(ItemKind::parse(k).ok_or_else(|| err(ErrorCode::BadRequest, "unknown kind"))?),
        None => None,
    };
    let sort = match q.sort.as_deref().unwrap_or("title") {
        "title" => SortBy::Title,
        "added" => SortBy::Added,
        "year" => SortBy::Year,
        "played" => SortBy::Played,
        "index" => SortBy::Index,
        "random" => SortBy::Random,
        _ => return Err(err(ErrorCode::BadRequest, "unknown sort")),
    };
    let query = ItemQuery {
        library_id: q.library_id,
        parent_id: q.parent_id,
        kind,
        genre: q.genre,
        person_id: q.person_id,
        favorite: q.favorite,
        played: q.played,
        sort,
        desc: q.desc,
        offset: q.offset,
        limit: q.limit,
        ..Default::default()
    };
    Ok(Json(st.library.items(auth.0.user.id, query).await.map_err(lib_err)?))
}

async fn item(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<ItemDetail> {
    let mut d = st.library.item(auth.0.user.id, id).await.map_err(lib_err)?;
    if !auth.0.can(Perms::LIBRARY_MANAGE) {
        // File locations are for people who manage the library.
        for f in &mut d.files {
            f.path = PathBuf::from(&f.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
    }
    Ok(Json(d))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EditItem {
    title: Option<String>,
    overview: Option<String>,
    year: Option<i32>,
    /// Fields metadata refreshes must leave alone.
    locked_fields: Vec<String>,
}

async fn edit(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(e): Json<EditItem>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    st.library.edit_item(id, e.title, e.overview, e.year, e.locked_fields).await.map_err(lib_err)?;
    crate::metadata::write_nfo(&st, id).await;
    Ok(Json(serde_json::json!({})))
}

async fn set_state(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    Json(u): Json<StateUpdate>,
) -> ApiResult<UserState> {
    Ok(Json(st.library.set_state(auth.0.user.id, id, u).await.map_err(lib_err)?))
}

async fn home(State(st): State<Arc<AppState>>, auth: Auth) -> ApiResult<Vec<HomeRow>> {
    Ok(Json(st.library.home(auth.0.user.id).await.map_err(lib_err)?))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(State(st): State<Arc<AppState>>, auth: Auth, Query(q): Query<SearchQuery>) -> ApiResult<SearchResults> {
    if q.q.len() > 200 {
        return Err(err(ErrorCode::BadRequest, "search text too long"));
    }
    Ok(Json(st.library.search(auth.0.user.id, &q.q).await.map_err(lib_err)?))
}

/// Serves a file with Range, conditional requests and the right type (direct play).
pub(crate) async fn serve_path(path: PathBuf, req: Request) -> Result<Response, ApiErr> {
    let mime = mime_for(&path);
    let res = tower_http::services::ServeFile::new_with_mime(&path, &mime)
        .oneshot(req)
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    let mut res = res.map(Body::new);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(res)
}

pub(crate) fn mime_for(p: &std::path::Path) -> mime_guess::Mime {
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    // Types browsers need exactly right for <video>/<audio>.
    let t = match ext.as_str() {
        "mkv" => "video/x-matroska",
        "mka" => "audio/x-matroska",
        "m4b" => "audio/mp4",
        "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "srt" => "application/x-subrip",
        "ass" | "ssa" => "text/x-ssa",
        "vtt" => "text/vtt",
        _ => return mime_guess::from_path(p).first_or_octet_stream(),
    };
    t.parse().unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM)
}

async fn item_file(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    req: Request,
) -> Result<Response, ApiErr> {
    let (_, path) = st.library.main_file(auth.0.user.id, id).await.map_err(lib_err)?;
    serve_path(PathBuf::from(path), req).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FrameRequest {
    /// Where in the video (default: the automatic position, 15–20 % in).
    at_ms: Option<i64>,
    /// `thumb` (default) or `poster`.
    kind: Option<String>,
    /// Keep it over provider artwork and automatic frames (default true: someone chose it).
    lock: Option<bool>,
}

/// Takes a frame of the item's video as its thumbnail (or poster).
async fn thumbnail_from_frame(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    body: Option<Json<FrameRequest>>,
) -> ApiResult<serde_json::Value> {
    auth.require(Perms::LIBRARY_MANAGE)?;
    let req = body.map(|b| b.0).unwrap_or_default();
    let kind = req.kind.as_deref().unwrap_or("thumb");
    if !matches!(kind, "thumb" | "poster") {
        return Err(err(ErrorCode::BadRequest, "kind must be thumb or poster"));
    }
    let (_, path) = st.library.main_file(auth.0.user.id, id).await.map_err(lib_err)?;
    let d = st.library.item(auth.0.user.id, id).await.map_err(lib_err)?;
    let at = match req.at_ms {
        Some(ms) => ms.max(0) as f64 / 1000.0,
        None => corrent_transcode::frame::position_secs(d.item.runtime_ms, d.item.kind == ItemKind::Episode),
    };
    let jpg = crate::thumbs::frame(&st, &path, at).await.map_err(|e| err(ErrorCode::Internal, e))?;
    let lock = req.lock.unwrap_or(true);
    let stored = st
        .library
        .set_artwork(id, kind, jpg, corrent_library::meta::FRAME, corrent_library::meta::Replace::Always, lock)
        .await
        .map_err(lib_err)?;
    Ok(Json(serde_json::json!({ "stored": stored })))
}

async fn file(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
    req: Request,
) -> Result<Response, ApiErr> {
    let path = st.library.file_path(auth.0.user.id, id).await.map_err(lib_err)?;
    serve_path(PathBuf::from(path), req).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ImageQuery {
    w: Option<u32>,
}

/// Content-addressed artwork: immutable, so cached for a year.
async fn image(
    State(st): State<Arc<AppState>>,
    _auth: Auth,
    Path(sha): Path<String>,
    Query(q): Query<ImageQuery>,
    req: Request,
) -> Result<Response, ApiErr> {
    let Some(path) = corrent_library::artwork::existing(st.library.artwork_root(), &sha, q.w) else {
        return Err(err(ErrorCode::NotFound, "no such image"));
    };
    if req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_matches('"') == sha)
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let mut res = serve_path(path, req).await?;
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    if let Ok(v) = HeaderValue::from_str(&format!("\"{sha}\"")) {
        h.insert(header::ETAG, v);
    }
    Ok(res)
}

async fn user_libraries(State(st): State<Arc<AppState>>, auth: Auth, Path(id): Path<i64>) -> ApiResult<Vec<i64>> {
    if id != auth.0.user.id {
        auth.require(Perms::USERS_MANAGE)?;
    }
    Ok(Json(st.library.user_libraries(id).await.map_err(lib_err)?))
}

async fn set_user_libraries(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    ip: ClientIp,
    Path(id): Path<i64>,
    Json(libs): Json<Vec<i64>>,
) -> ApiResult<Vec<i64>> {
    auth.require(Perms::USERS_MANAGE)?;
    let known: Vec<i64> = st.library.list().await.map_err(lib_err)?.iter().map(|l| l.id).collect();
    if let Some(bad) = libs.iter().find(|l| !known.contains(l)) {
        return Err(err(ErrorCode::BadRequest, "no such library").with_id(*bad));
    }
    st.library.set_user_libraries(id, libs.clone()).await.map_err(lib_err)?;
    st.identity.audit(Some(auth.name()), &ip.0.to_string(), "user_libraries", &id.to_string(), true, "").await;
    Ok(Json(libs))
}

/// A track's lyrics (sidecar `.lrc`/`.txt` or the file's tag).
async fn lyrics(
    State(st): State<Arc<AppState>>,
    auth: Auth,
    Path(id): Path<i64>,
) -> ApiResult<corrent_proto::library::Lyrics> {
    let (_, path) = st.library.main_file(auth.0.user.id, id).await.map_err(lib_err)?;
    let found = tokio::task::spawn_blocking(move || corrent_library::lyrics::find(std::path::Path::new(&path)))
        .await
        .map_err(|e| err(ErrorCode::Internal, e.to_string()))?;
    found.map(Json).ok_or_else(|| err(ErrorCode::NotFound, "no lyrics"))
}
