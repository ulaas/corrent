//! Views (libraries), item lists and pages, home rows, shows, images and the many lists apps ask
//! for that Corrent doesn't have (answered empty, never with an error).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use corrent_proto::library::{Item, ItemKind, ItemQuery, Library, LibraryKind, SortBy};
use serde_json::{Value, json};

use super::dto::{self, Ctx};
use super::system::ctx;
use super::{JfAuth, Q, empty_result, fail, ids, json, no_content, not_found};
use crate::state::AppState;

pub(super) fn routes() -> Router<Arc<AppState>> {
    let mut r = Router::new()
        .route("/userviews", get(views))
        .route("/users/{u}/views", get(views_of))
        .route("/items", get(items))
        .route("/users/{u}/items", get(items_of))
        .route("/items/root", get(root))
        .route("/users/{u}/items/root", get(root))
        .route("/items/latest", get(latest))
        .route("/users/{u}/items/latest", get(latest_of))
        .route("/useritems/resume", get(resume))
        .route("/users/{u}/items/resume", get(resume_of))
        .route("/items/{id}", get(item))
        .route("/users/{u}/items/{id}", get(item_of))
        .route("/shows/nextup", get(next_up))
        .route("/shows/{id}/seasons", get(seasons))
        .route("/shows/{id}/episodes", get(episodes))
        .route("/items/{id}/similar", get(similar))
        .route("/items/{id}/ancestors", get(ancestors))
        .route(
            "/items/filters",
            get(|_a: JfAuth| async { json(json!({ "Genres": [], "Tags": [], "OfficialRatings": [], "Years": [] })) }),
        )
        .route("/items/filters2", get(|_a: JfAuth| async { json(json!({ "Genres": [], "Tags": [] })) }))
        .route(
            "/items/{id}/thememedia",
            get(|_a: JfAuth| async {
                let e = json!({ "Items": [], "TotalRecordCount": 0, "StartIndex": 0 });
                json(json!({ "ThemeVideosResult": e, "ThemeSongsResult": e, "SoundtrackSongsResult": e }))
            }),
        )
        .route("/items/{id}/refresh", post(|_a: JfAuth| async { no_content() }))
        .route("/items/{id}/images", get(|_a: JfAuth| async { json(json!([])) }))
        .route("/items/{id}/images/{ty}", get(image))
        .route("/items/{id}/images/{ty}/{index}", get(image_indexed));
    // Lists Corrent has nothing for.
    for p in [
        "/items/{id}/intros",
        "/items/{id}/localtrailers",
        "/items/{id}/specialfeatures",
        "/videos/{id}/additionalparts",
        "/users/{u}/items/{id}/intros",
        "/users/{u}/items/{id}/localtrailers",
        "/users/{u}/items/{id}/specialfeatures",
        "/movies/recommendations",
    ] {
        r = r.route(p, get(|_a: JfAuth| async { json(json!([])) }));
    }
    for p in [
        "/shows/upcoming",
        "/users/{u}/suggestions",
        "/items/suggestions",
        "/artists",
        "/artists/albumartists",
        "/persons",
        "/genres",
        "/studios",
        "/musicgenres",
        "/years",
        "/playlists",
        "/mediasegments/{id}",
        "/livetv/channels",
        "/livetv/programs/recommended",
        "/channels",
    ] {
        r = r.route(p, get(|_a: JfAuth| async { empty_result() }));
    }
    r
}

/// Libraries the user sees through Jellyfin (podcast libraries are left out).
async fn visible_libraries(st: &AppState, user: i64) -> Vec<Library> {
    let all = st.library.list().await.unwrap_or_default();
    let allowed = st.library.user_libraries(user).await.unwrap_or_default();
    all.into_iter()
        .filter(|l| allowed.is_empty() || allowed.contains(&l.id))
        .filter(|l| dto::collection_type(l.kind).is_some())
        .collect()
}

fn kinds_of_libraries(libs: &[Library]) -> HashMap<i64, LibraryKind> {
    libs.iter().map(|l| (l.id, l.kind)).collect()
}

fn qr(items: Vec<Value>, total: i64, start: i64) -> Response {
    json(json!({ "Items": items, "TotalRecordCount": total, "StartIndex": start }))
}

async fn views(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    let uid = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let c = ctx(&st);
    let libs = visible_libraries(&st, uid).await;
    let items: Vec<Value> = libs.iter().map(|l| dto::view_dto(&c, l)).collect();
    let n = items.len() as i64;
    qr(items, n, 0)
}

async fn views_of(State(st): State<Arc<AppState>>, a: JfAuth, Path(u): Path<String>, q: Q) -> Response {
    if let Err(r) = a.check_user(Some(&u)) {
        return r;
    }
    views(State(st), a, q).await
}

async fn root(State(st): State<Arc<AppState>>, _a: JfAuth) -> Response {
    json(dto::root_folder(&ctx(&st)))
}

/// Which fields ask for the expensive parts of an item.
fn wants_detail(q: &Q) -> bool {
    q.list("fields").iter().any(|f| {
        matches!(f.to_ascii_lowercase().as_str(), "mediasources" | "mediastreams" | "people" | "chapters" | "overview")
    })
}

/// Runs a query over as many of our pages as the client's `Limit` asks for (up to 2000).
async fn run(st: &AppState, user: i64, mut q: ItemQuery, limit: i64) -> (Vec<Item>, i64) {
    let mut out = Vec::new();
    let mut total = 0;
    let want = limit.clamp(0, 2000);
    loop {
        q.limit = Some((want - out.len() as i64).clamp(1, 500));
        let Ok(page) = st.library.items(user, q.clone()).await else { break };
        total = page.total;
        let got = page.items.len() as i64;
        out.extend(page.items);
        if got == 0 || out.len() as i64 >= want || page.next.is_none() {
            break;
        }
        q.offset += got;
    }
    (out, total)
}

pub(crate) async fn dtos(st: &AppState, c: &Ctx, user: i64, items: &[Item], detail: bool) -> Vec<Value> {
    let libs = kinds_of_libraries(&st.library.list().await.unwrap_or_default());
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        if libs.get(&it.library_id).is_some_and(|k| dto::collection_type(*k).is_none()) {
            continue;
        }
        let d = if detail { st.library.item(user, it.id).await.ok() } else { None };
        out.push(dto::item_dto(c, it, libs.get(&it.library_id).copied(), d.as_ref()));
    }
    out
}

fn sort_of(q: &Q) -> (SortBy, bool) {
    let first = q.list("sortby").into_iter().next().unwrap_or_default().to_ascii_lowercase();
    let sort = match first.as_str() {
        "datecreated" => SortBy::Added,
        "premieredate" | "productionyear" => SortBy::Year,
        "dateplayed" | "datelastcontentadded" => SortBy::Played,
        "indexnumber" | "parentindexnumber" | "airedepisodeorder" => SortBy::Index,
        "random" => SortBy::Random,
        _ => SortBy::Title,
    };
    let desc = q.list("sortorder").first().is_some_and(|o| o.eq_ignore_ascii_case("descending"));
    (sort, desc)
}

/// `IncludeItemTypes` and `MediaTypes` → kinds; `None` when every named type is one we lack.
fn requested_kinds(q: &Q) -> Option<Vec<ItemKind>> {
    let mut kinds = Vec::new();
    let types = q.list("includeitemtypes");
    let mut known = types.is_empty();
    for t in &types {
        if let Some(k) = dto::kinds_of(t) {
            kinds.extend(k);
            known = true;
        }
    }
    for m in q.list("mediatypes") {
        let k: &[ItemKind] = match m.to_ascii_lowercase().as_str() {
            "video" => &[ItemKind::Movie, ItemKind::Episode, ItemKind::Video],
            "audio" => &[ItemKind::Track, ItemKind::Book],
            "book" => &[ItemKind::Book],
            "photo" => &[ItemKind::Photo],
            _ => &[],
        };
        if types.is_empty() {
            kinds.extend_from_slice(k);
        } else {
            kinds.retain(|x| k.contains(x));
        }
    }
    let exclude: Vec<ItemKind> = q.list("excludeitemtypes").iter().filter_map(|t| dto::kinds_of(t)).flatten().collect();
    if !exclude.is_empty() {
        if kinds.is_empty() {
            kinds = ItemKind::ALL.into_iter().filter(|k| !exclude.contains(k)).collect();
        } else {
            kinds.retain(|k| !exclude.contains(k));
        }
    }
    for f in q.list("filters") {
        match f.to_ascii_lowercase().as_str() {
            "isfolder" => {
                let folders = [ItemKind::Show, ItemKind::Season, ItemKind::Artist, ItemKind::Album, ItemKind::Folder];
                if kinds.is_empty() {
                    kinds = folders.to_vec();
                } else {
                    kinds.retain(|k| folders.contains(k));
                }
            }
            "isnotfolder" => {
                if kinds.is_empty() {
                    kinds = ItemKind::ALL.into_iter().filter(|k| k.playable() || *k == ItemKind::Photo).collect();
                } else {
                    kinds.retain(|k| k.playable() || *k == ItemKind::Photo);
                }
            }
            _ => {}
        }
    }
    kinds.sort_by_key(|k| k.as_str());
    kinds.dedup();
    known.then_some(kinds)
}

/// `/Items` → our query (see the translation table in the plan).
async fn list(st: &Arc<AppState>, a: &JfAuth, user: i64, q: &Q) -> Response {
    let c = ctx(st);
    let start = q.i64("startindex").unwrap_or(0).max(0);
    let limit = q.i64("limit").unwrap_or(500).clamp(0, 2000);
    let Some(kinds) = requested_kinds(q) else { return qr(Vec::new(), 0, start) };
    let recursive = q.bool("recursive").unwrap_or(false);
    let (sort, desc) = sort_of(q);
    let mut iq = ItemQuery { sort, desc, offset: start, kinds, ..Default::default() };
    let filters: Vec<String> = q.list("filters").iter().map(|f| f.to_ascii_lowercase()).collect();
    if filters.iter().any(|f| f == "isfavorite") || q.bool("isfavorite") == Some(true) {
        iq.favorite = Some(true);
    }
    if filters.iter().any(|f| f == "isplayed") {
        iq.played = Some(true);
    }
    if filters.iter().any(|f| f == "isunplayed") {
        iq.played = Some(false);
    }
    if let Some(p) = q.bool("isplayed") {
        iq.played = Some(p);
    }
    if filters.iter().any(|f| f == "isresumable") {
        iq.resumable = Some(true);
    }
    if let Some(t) = q.get("searchterm").or_else(|| q.get("namestartswith")) {
        iq.title_contains = Some(t.chars().take(100).collect());
    }
    if let Some(g) = q.list("genres").into_iter().next() {
        iq.genre = Some(g);
    }
    if let Some(p) = q.list("personids").first().and_then(|p| ids::dec_kind(p, ids::PERSON)) {
        iq.person_id = Some(p);
    }
    let id_list = q.list("ids");
    if !id_list.is_empty() {
        iq.ids = Some(id_list.iter().filter_map(|i| ids::dec_kind(i, ids::ITEM)).collect());
        let views: Vec<i64> = id_list.iter().filter_map(|i| ids::dec_kind(i, ids::VIEW)).collect();
        if !views.is_empty() {
            let libs = visible_libraries(st, user).await;
            let v: Vec<Value> = libs.iter().filter(|l| views.contains(&l.id)).map(|l| dto::view_dto(&c, l)).collect();
            let n = v.len() as i64;
            return qr(v, n, 0);
        }
    }
    let parent = q.get("parentid").map(str::to_string);
    let artist = q
        .list("artistids")
        .into_iter()
        .chain(q.list("albumartistids"))
        .next()
        .and_then(|a| ids::dec_kind(&a, ids::ITEM));
    let plain = iq.kinds.is_empty()
        && iq.ids.is_none()
        && iq.title_contains.is_none()
        && iq.favorite.is_none()
        && iq.played.is_none()
        && iq.resumable.is_none()
        && iq.genre.is_none()
        && iq.person_id.is_none();
    match parent.as_deref().filter(|p| !p.is_empty()).map(ids::dec) {
        Some(Some((ids::VIEW, 0))) | None if artist.is_none() => {
            if plain && !recursive {
                // The root: the views.
                let libs = visible_libraries(st, user).await;
                let v: Vec<Value> = libs.iter().map(|l| dto::view_dto(&c, l)).collect();
                let n = v.len() as i64;
                return qr(v, n, 0);
            }
        }
        Some(Some((ids::VIEW, l))) => {
            if !visible_libraries(st, user).await.iter().any(|x| x.id == l) {
                return not_found();
            }
            iq.library_id = Some(l);
        }
        Some(Some((ids::ITEM, i))) => {
            if recursive {
                iq.ancestor_id = Some(i);
            } else {
                iq.parent_id = Some(i);
            }
        }
        Some(_) => return qr(Vec::new(), 0, start),
        None => {}
    }
    if let Some(a) = artist {
        // An artist's albums (or everything below it).
        iq.ancestor_id = Some(a);
        if iq.kinds.is_empty() {
            iq.kinds = vec![ItemKind::Album];
        }
    }
    if limit == 0 {
        let (_, total) = run(st, user, ItemQuery { limit: Some(1), ..iq }, 1).await;
        return qr(Vec::new(), total, start);
    }
    let (items, total) = run(st, user, iq, limit).await;
    let detail = wants_detail(q) && limit <= 200;
    let _ = a;
    qr(dtos(st, &c, user, &items, detail).await, total, start)
}

async fn items(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => list(&st, &a, u, &q).await,
        Err(r) => r,
    }
}

async fn items_of(State(st): State<Arc<AppState>>, a: JfAuth, Path(u): Path<String>, q: Q) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => list(&st, &a, u, &q).await,
        Err(r) => r,
    }
}

async fn one(st: &Arc<AppState>, user: i64, id: &str) -> Response {
    let c = ctx(st);
    if id.eq_ignore_ascii_case("root") {
        return json(dto::root_folder(&c));
    }
    match ids::dec(id) {
        Some((ids::VIEW, 0)) => json(dto::root_folder(&c)),
        Some((ids::VIEW, l)) => match visible_libraries(st, user).await.into_iter().find(|x| x.id == l) {
            Some(lib) => json(dto::view_dto(&c, &lib)),
            None => not_found(),
        },
        Some((ids::PERSON, p)) => json(
            json!({ "Name": "", "Id": ids::enc(ids::PERSON, p), "ServerId": c.server_id, "Type": "Person", "IsFolder": false, "ImageTags": {} }),
        ),
        Some((ids::GENRE, g)) => json(
            json!({ "Name": "", "Id": ids::enc(ids::GENRE, g), "ServerId": c.server_id, "Type": "Genre", "IsFolder": true, "ImageTags": {} }),
        ),
        Some((ids::ITEM, i)) => {
            let Ok(d) = st.library.item(user, i).await else { return not_found() };
            let libs = kinds_of_libraries(&st.library.list().await.unwrap_or_default());
            let kind = libs.get(&d.item.library_id).copied();
            if kind.is_some_and(|k| dto::collection_type(k).is_none()) {
                return not_found();
            }
            // Images of the parents too (episodes show their show's).
            let mut it = d.item.clone();
            if let Ok(page) = st.library.items(user, ItemQuery { ids: Some(vec![i]), ..Default::default() }).await
                && let Some(full) = page.items.into_iter().next()
            {
                it = full;
            }
            json(dto::item_dto(&c, &it, kind, Some(&d)))
        }
        _ => not_found(),
    }
}

async fn item(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => one(&st, u, &id).await,
        Err(r) => r,
    }
}

async fn item_of(State(st): State<Arc<AppState>>, a: JfAuth, Path((u, id)): Path<(String, String)>) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => one(&st, u, &id).await,
        Err(r) => r,
    }
}

/// What the "recently added" row of a library lists.
fn latest_kind(k: LibraryKind) -> ItemKind {
    match k {
        LibraryKind::Movies => ItemKind::Movie,
        LibraryKind::Shows | LibraryKind::Podcasts => ItemKind::Episode,
        LibraryKind::Music => ItemKind::Album,
        LibraryKind::Audiobooks => ItemKind::Book,
        LibraryKind::Videos => ItemKind::Video,
        LibraryKind::Photos => ItemKind::Photo,
    }
}

async fn latest_for(st: &Arc<AppState>, user: i64, q: &Q) -> Response {
    let c = ctx(st);
    let limit = q.i64("limit").unwrap_or(20).clamp(1, 100);
    let libs = visible_libraries(st, user).await;
    let only = q.get("parentid").and_then(|p| ids::dec_kind(p, ids::VIEW));
    let mut out = Vec::new();
    for l in libs.iter().filter(|l| only.is_none_or(|o| o == l.id)) {
        let kinds = requested_kinds(q).filter(|k| !k.is_empty()).unwrap_or_else(|| vec![latest_kind(l.kind)]);
        let mut iq = ItemQuery { library_id: Some(l.id), kinds, sort: SortBy::Added, desc: true, ..Default::default() };
        if q.bool("isplayed") == Some(false) {
            iq.played = Some(false);
        }
        let (items, _) = run(st, user, iq, limit).await;
        out.extend(dtos(st, &c, user, &items, false).await);
    }
    out.truncate(limit as usize * libs.len().max(1));
    json(Value::Array(out))
}

async fn latest(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => latest_for(&st, u, &q).await,
        Err(r) => r,
    }
}

async fn latest_of(State(st): State<Arc<AppState>>, a: JfAuth, Path(u): Path<String>, q: Q) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => latest_for(&st, u, &q).await,
        Err(r) => r,
    }
}

async fn resume_for(st: &Arc<AppState>, user: i64, q: &Q) -> Response {
    let c = ctx(st);
    let start = q.i64("startindex").unwrap_or(0).max(0);
    let limit = q.i64("limit").unwrap_or(12).clamp(1, 200);
    let kinds = requested_kinds(q).unwrap_or_default();
    let mut iq = ItemQuery {
        resumable: Some(true),
        sort: SortBy::Played,
        desc: true,
        offset: start,
        kinds,
        ..Default::default()
    };
    if let Some(l) = q.get("parentid").and_then(|p| ids::dec_kind(p, ids::VIEW)) {
        iq.library_id = Some(l);
    }
    let (items, total) = run(st, user, iq, limit).await;
    qr(dtos(st, &c, user, &items, false).await, total, start)
}

async fn resume(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    match a.check_user(q.get("userid")) {
        Ok(u) => resume_for(&st, u, &q).await,
        Err(r) => r,
    }
}

async fn resume_of(State(st): State<Arc<AppState>>, a: JfAuth, Path(u): Path<String>, q: Q) -> Response {
    match a.check_user(Some(&u)) {
        Ok(u) => resume_for(&st, u, &q).await,
        Err(r) => r,
    }
}

async fn next_up(State(st): State<Arc<AppState>>, a: JfAuth, q: Q) -> Response {
    let user = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let c = ctx(&st);
    let series = q.get("seriesid").and_then(|s| ids::dec_kind(s, ids::ITEM));
    let first = q.get("seriesid").is_some() && q.bool("disablefirstepisode") != Some(true);
    let mut items = st.library.next_up(user, series, first).await.unwrap_or_default();
    if let Some(l) = q.get("parentid").and_then(|p| ids::dec_kind(p, ids::VIEW)) {
        items.retain(|i| i.library_id == l);
    }
    let total = items.len() as i64;
    let start = q.i64("startindex").unwrap_or(0).max(0) as usize;
    let limit = q.i64("limit").unwrap_or(20).clamp(1, 200) as usize;
    let page: Vec<Item> = items.into_iter().skip(start).take(limit).collect();
    qr(dtos(&st, &c, user, &page, false).await, total, start as i64)
}

async fn seasons(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    let user = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(show) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let c = ctx(&st);
    let iq =
        ItemQuery { parent_id: Some(show), kinds: vec![ItemKind::Season], sort: SortBy::Index, ..Default::default() };
    let (items, total) = run(&st, user, iq, 500).await;
    qr(dtos(&st, &c, user, &items, false).await, total, 0)
}

async fn episodes(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    let user = match a.check_user(q.get("userid")) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(show) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let c = ctx(&st);
    let start = q.i64("startindex").unwrap_or(0).max(0);
    let limit = q.i64("limit").unwrap_or(2000).clamp(1, 2000);
    let mut iq = ItemQuery { kinds: vec![ItemKind::Episode], sort: SortBy::Index, ..Default::default() };
    if let Some(season) = q.get("seasonid").and_then(|s| ids::dec_kind(s, ids::ITEM)) {
        iq.parent_id = Some(season);
    } else if let Some(n) = q.i64("season") {
        let (seasons, _) = run(
            &st,
            user,
            ItemQuery { parent_id: Some(show), kinds: vec![ItemKind::Season], ..Default::default() },
            500,
        )
        .await;
        match seasons.iter().find(|s| s.index == Some(n)) {
            Some(s) => iq.parent_id = Some(s.id),
            None => return qr(Vec::new(), 0, start),
        }
    } else {
        iq.ancestor_id = Some(show);
    }
    // All of a show: by season, then episode.
    let (mut items, _) = run(&st, user, iq, 2000).await;
    items.sort_by_key(|e| (e.parent_index.unwrap_or(0), e.index.unwrap_or(0)));
    if let Some(from) = q.get("startitemid").and_then(|s| ids::dec_kind(s, ids::ITEM))
        && let Some(pos) = items.iter().position(|e| e.id == from)
    {
        items.drain(..pos);
    }
    let total = items.len() as i64;
    let page: Vec<Item> = items.into_iter().skip(start as usize).take(limit as usize).collect();
    let detail = wants_detail(&q) && page.len() <= 200;
    qr(dtos(&st, &c, user, &page, detail).await, total, start)
}

async fn similar(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>, q: Q) -> Response {
    let user = a.user();
    let Some(i) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let Ok(d) = st.library.item(user, i).await else { return not_found() };
    let c = ctx(&st);
    let limit = q.i64("limit").unwrap_or(12).clamp(1, 50);
    let mut iq = ItemQuery {
        library_id: Some(d.item.library_id),
        kinds: vec![d.item.kind],
        sort: SortBy::Random,
        ..Default::default()
    };
    if let Some(g) = d.genres.first() {
        iq.genre = Some(g.clone());
    }
    let (mut items, _) = run(&st, user, iq, limit + 1).await;
    items.retain(|x| x.id != i);
    items.truncate(limit as usize);
    let n = items.len() as i64;
    qr(dtos(&st, &c, user, &items, false).await, n, 0)
}

async fn ancestors(State(st): State<Arc<AppState>>, a: JfAuth, Path(id): Path<String>) -> Response {
    let user = a.user();
    let Some(mut i) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
    let c = ctx(&st);
    let mut out = Vec::new();
    let mut lib = None;
    for _ in 0..4 {
        let Ok(d) = st.library.item(user, i).await else { break };
        lib = Some(d.item.library_id);
        match d.item.parent_id {
            Some(p) => {
                if let Ok(pd) = st.library.item(user, p).await {
                    out.push(dto::item_dto(&c, &pd.item, None, None));
                }
                i = p;
            }
            None => break,
        }
    }
    if let Some(l) = lib
        && let Some(v) = visible_libraries(&st, user).await.iter().find(|x| x.id == l)
    {
        out.push(dto::view_dto(&c, v));
    }
    json(Value::Array(out))
}

// ---- images

async fn image(state: State<Arc<AppState>>, Path((id, ty)): Path<(String, String)>, q: Q, req: Request) -> Response {
    serve_image(state, id, ty, q, req).await
}

async fn image_indexed(
    state: State<Arc<AppState>>,
    Path((id, ty, _i)): Path<(String, String, String)>,
    q: Q,
    req: Request,
) -> Response {
    serve_image(state, id, ty, q, req).await
}

/// Artwork: with `tag` (the image's hash, unguessable) anyone on the network may load it, as
/// apps load art without credentials; without one the caller must be signed in.
async fn serve_image(State(st): State<Arc<AppState>>, id: String, ty: String, q: Q, req: Request) -> Response {
    let width = q
        .i64("maxwidth")
        .or(q.i64("width"))
        .or(q.i64("fillwidth"))
        .or_else(|| q.i64("maxheight").or(q.i64("height")).or(q.i64("fillheight")).map(|h| h * 7 / 10))
        .unwrap_or(400)
        .clamp(50, 4000) as u32;
    let tag =
        q.get("tag").filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())).map(str::to_ascii_lowercase);
    let sha = match tag {
        Some(t) => t,
        None => {
            let (mut parts, body) = req.into_parts();
            let public = st.cfg.load().jellyfin_public_images;
            let user =
                match <JfAuth as axum::extract::FromRequestParts<Arc<AppState>>>::from_request_parts(&mut parts, &st)
                    .await
                {
                    Ok(a) => Some(a.user()),
                    Err(r) if !public => return r,
                    Err(_) => None,
                };
            let req = Request::from_parts(parts, body);
            let Some(i) = ids::dec_kind(&id, ids::ITEM) else { return not_found() };
            // Without a user (public images) any item's art; with one, what they may see.
            let uid = user.unwrap_or(0);
            let Ok(page) = st.library.items(uid, ItemQuery { ids: Some(vec![i]), ..Default::default() }).await else {
                return not_found();
            };
            let Some(it) = page.items.first() else { return not_found() };
            let Some(img) = dto::image_for(it, &ty) else { return fail(StatusCode::NOT_FOUND, "Image not found") };
            return send_image(&st, &img.sha, width, req, false).await;
        }
    };
    send_image(&st, &sha, width, req, true).await
}

async fn send_image(st: &AppState, sha: &str, width: u32, req: Request, immutable: bool) -> Response {
    let w = [200, 400, 800].into_iter().find(|v| *v >= width);
    let Some(path) = corrent_library::artwork::existing(st.library.artwork_root(), sha, w)
        .or_else(|| corrent_library::artwork::existing(st.library.artwork_root(), sha, None))
    else {
        return fail(StatusCode::NOT_FOUND, "Image not found");
    };
    if req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_matches('"') == sha)
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    match crate::v1::library::serve_path(path, req).await {
        Ok(mut r) => {
            let h = r.headers_mut();
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(if immutable {
                    "public, max-age=31536000, immutable"
                } else {
                    "private, max-age=3600"
                }),
            );
            if let Ok(v) = HeaderValue::from_str(&format!("\"{sha}\"")) {
                h.insert(header::ETAG, v);
            }
            r
        }
        Err(_) => not_found(),
    }
}
