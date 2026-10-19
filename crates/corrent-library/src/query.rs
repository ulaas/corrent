//! Reading the library for clients: item lists, details, home rows, search and watch state.
//! Every query is per user (watch state, allowed libraries).

use std::collections::{BTreeMap, HashMap};

use corrent_db::rusqlite::{self, Connection, OptionalExtension, Row, params, params_from_iter};
use corrent_proto::library::{
    Chapter, Credit, HomeRow, ImageRef, Item, ItemDetail, ItemKind, ItemQuery, MediaFile, Page, SearchResults, SortBy,
    StateUpdate, Stream, UserState,
};

use crate::{LibError, Library, Result, now_unix};

const ITEM_SELECT: &str = "SELECT i.id, i.library_id, i.kind, i.parent_id, i.title, i.year, i.index_no, i.runtime_ms, i.added_at, \
     p.title, gp.title, ws.position_ms, ws.played, ws.play_count, ws.last_played_at, ws.favorite, ws.rating, \
     (SELECT count(*) FROM items c WHERE c.parent_id = i.id), \
     CASE i.kind \
       WHEN 'season' THEN (SELECT count(*) FROM items e WHERE e.parent_id = i.id AND NOT EXISTS \
            (SELECT 1 FROM watch_state w WHERE w.item_id = e.id AND w.user_id = ?1 AND w.played = 1)) \
       WHEN 'show' THEN (SELECT count(*) FROM items e JOIN items s ON e.parent_id = s.id WHERE s.parent_id = i.id AND NOT EXISTS \
            (SELECT 1 FROM watch_state w WHERE w.item_id = e.id AND w.user_id = ?1 AND w.played = 1)) \
       ELSE NULL END, \
     p.parent_id, p.index_no \
     FROM items i LEFT JOIN items p ON p.id = i.parent_id LEFT JOIN items gp ON gp.id = p.parent_id \
     LEFT JOIN watch_state ws ON ws.item_id = i.id AND ws.user_id = ?1";

struct RawItem {
    item: Item,
    parent: Option<i64>,
    grandparent: Option<i64>,
}

fn item_row(r: &Row) -> rusqlite::Result<RawItem> {
    let kind: String = r.get(2)?;
    let mut context = Vec::new();
    if let Some(gp) = r.get::<_, Option<String>>(10)? {
        context.push(gp);
    }
    if let Some(p) = r.get::<_, Option<String>>(9)? {
        context.push(p);
    }
    let parent: Option<i64> = r.get(3)?;
    Ok(RawItem {
        item: Item {
            id: r.get(0)?,
            library_id: r.get(1)?,
            kind: ItemKind::parse(&kind).unwrap_or(ItemKind::Video),
            parent_id: parent,
            title: r.get(4)?,
            year: r.get(5)?,
            index: r.get(6)?,
            runtime_ms: r.get(7)?,
            added_at: r.get(8)?,
            context,
            images: Vec::new(),
            state: UserState {
                position_ms: r.get::<_, Option<i64>>(11)?.unwrap_or(0),
                played: r.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
                play_count: r.get::<_, Option<i64>>(13)?.unwrap_or(0),
                last_played_at: r.get(14)?,
                favorite: r.get::<_, Option<i64>>(15)?.unwrap_or(0) != 0,
                rating: r.get(16)?,
                unplayed: r.get(18)?,
            },
            child_count: Some(r.get::<_, i64>(17)?).filter(|n| *n > 0),
            partial: false,
            grandparent_id: r.get(19)?,
            parent_index: r.get(20)?,
        },
        parent,
        grandparent: r.get(19)?,
    })
}

/// Fills images: the item's own, else its parent's/grandparent's (episode → season → show).
fn attach_images(c: &Connection, raws: Vec<RawItem>) -> rusqlite::Result<Vec<Item>> {
    if raws.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<i64> = raws.iter().flat_map(|r| [Some(r.item.id), r.parent, r.grandparent]).flatten().collect();
    ids.sort_unstable();
    ids.dedup();
    let mut by_owner: HashMap<i64, Vec<ImageRef>> = HashMap::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let mut st = c.prepare(&format!(
            "SELECT owner_id, kind, sha256, width, height, blurhash FROM images WHERE owner_kind = 'item' AND owner_id IN ({marks})"
        ))?;
        let rows = st.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                ImageRef {
                    kind: r.get(1)?,
                    sha: r.get(2)?,
                    width: r.get(3)?,
                    height: r.get(4)?,
                    blurhash: r.get(5)?,
                    inherited: false,
                },
            ))
        })?;
        for row in rows {
            let (owner, img) = row?;
            by_owner.entry(owner).or_default().push(img);
        }
    }
    Ok(raws
        .into_iter()
        .map(|r| {
            let mut item = r.item;
            item.images = by_owner.get(&item.id).cloned().unwrap_or_default();
            for fallback in [r.parent, r.grandparent].into_iter().flatten() {
                for img in by_owner.get(&fallback).into_iter().flatten() {
                    if !item.images.iter().any(|i| i.kind == img.kind) {
                        item.images.push(ImageRef { inherited: true, ..img.clone() });
                    }
                }
            }
            item
        })
        .collect())
}

/// Libraries the user may see (`None` = all).
fn allowed(c: &Connection, user: i64) -> rusqlite::Result<Option<Vec<i64>>> {
    let mut st = c.prepare_cached("SELECT library_id FROM user_libraries WHERE user_id = ?1")?;
    let v: Vec<i64> = st.query_map([user], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok((!v.is_empty()).then_some(v))
}

fn allowed_clause(allowed: &Option<Vec<i64>>) -> String {
    match allowed {
        None => String::new(),
        Some(v) => format!(" AND i.library_id IN ({})", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
    }
}

fn query_items(c: &Connection, user: i64, q: &ItemQuery) -> rusqlite::Result<Page<Item>> {
    let allowed = allowed(c, user)?;
    let mut where_ = vec!["1 = 1".to_string()];
    let mut args: Vec<rusqlite::types::Value> = vec![user.into()];
    let mut arg = |v: rusqlite::types::Value| {
        args.push(v);
        format!("?{}", args.len())
    };
    if let Some(l) = q.library_id {
        where_.push(format!("i.library_id = {}", arg(l.into())));
    }
    match q.parent_id {
        Some(p) => where_.push(format!("i.parent_id = {}", arg(p.into()))),
        None if q.kind.is_none()
            && q.kinds.is_empty()
            && q.ids.is_none()
            && q.ancestor_id.is_none()
            && q.resumable.is_none()
            && q.title_contains.is_none()
            && q.genre.is_none()
            && q.person_id.is_none()
            && q.favorite.is_none() =>
        {
            where_.push("i.parent_id IS NULL".into())
        }
        None => {}
    }
    if let Some(k) = q.kind {
        where_.push(format!("i.kind = {}", arg(k.as_str().to_string().into())));
    }
    if !q.kinds.is_empty() {
        let list: Vec<String> = q.kinds.iter().map(|k| format!("'{}'", k.as_str())).collect();
        where_.push(format!("i.kind IN ({})", list.join(",")));
    }
    if let Some(ids) = &q.ids {
        let list: Vec<String> = ids.iter().take(1000).map(|i| i.to_string()).collect();
        where_.push(if list.is_empty() { "0 = 1".into() } else { format!("i.id IN ({})", list.join(",")) });
    }
    if let Some(a) = q.ancestor_id {
        where_.push(format!(
            "i.id IN (WITH RECURSIVE d(id) AS (SELECT id FROM items WHERE parent_id = {a} \
             UNION ALL SELECT x.id FROM items x JOIN d ON x.parent_id = d.id) SELECT id FROM d)",
            a = arg(a.into())
        ));
    }
    if let Some(r) = q.resumable {
        where_.push(if r {
            "coalesce(ws.position_ms, 0) > 0 AND coalesce(ws.played, 0) = 0".into()
        } else {
            "NOT (coalesce(ws.position_ms, 0) > 0 AND coalesce(ws.played, 0) = 0)".into()
        });
    }
    if let Some(t) = q.title_contains.as_ref().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        let esc = t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        where_.push(format!("i.title LIKE '%' || {} || '%' ESCAPE '\\'", arg(esc.into())));
    }
    if let Some(g) = &q.genre {
        where_.push(format!(
            "EXISTS (SELECT 1 FROM item_genres ig JOIN genres g ON g.id = ig.genre_id WHERE ig.item_id = i.id AND g.name = {})",
            arg(g.clone().into())
        ));
    }
    if let Some(pid) = q.person_id {
        where_.push(format!(
            "EXISTS (SELECT 1 FROM item_people ip WHERE ip.item_id = i.id AND ip.person_id = {})",
            arg(pid.into())
        ));
    }
    if let Some(f) = q.favorite {
        where_.push(format!("coalesce(ws.favorite, 0) = {}", if f { 1 } else { 0 }));
    }
    if let Some(p) = q.played {
        where_.push(format!("coalesce(ws.played, 0) = {}", if p { 1 } else { 0 }));
    }
    let filter = format!("{}{}", where_.join(" AND "), allowed_clause(&allowed));
    let dir = if q.desc { "DESC" } else { "ASC" };
    let order = match q.sort {
        SortBy::Title => format!("i.sort_title {dir}, i.id"),
        SortBy::Added => format!("i.added_at {dir}, i.id"),
        SortBy::Year => format!("i.year {dir}, i.sort_title"),
        SortBy::Played => format!("ws.last_played_at {dir}, i.id"),
        SortBy::Index => format!("i.index2 {dir}, i.index_no {dir}, i.sort_title"),
        SortBy::Random => "random()".into(),
    };
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.max(0);
    let total: i64 = c.query_row(
        &format!(
            "SELECT count(*) FROM items i LEFT JOIN watch_state ws ON ws.item_id = i.id AND ws.user_id = ?1 WHERE {filter}"
        ),
        params_from_iter(args.iter()),
        |r| r.get(0),
    )?;
    let sql = format!("{ITEM_SELECT} WHERE {filter} ORDER BY {order} LIMIT {limit} OFFSET {offset}");
    let mut st = c.prepare(&sql)?;
    let raws = st.query_map(params_from_iter(args.iter()), item_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let items = attach_images(c, raws)?;
    let next = (offset + (items.len() as i64) < total).then_some(offset + items.len() as i64);
    Ok(Page { items, total, next })
}

fn items_by_sql(
    c: &Connection,
    user: i64,
    filter: &str,
    order: &str,
    limit: i64,
    extra: &[rusqlite::types::Value],
) -> rusqlite::Result<Vec<Item>> {
    let allowed = allowed(c, user)?;
    let sql = format!("{ITEM_SELECT} WHERE {filter}{} ORDER BY {order} LIMIT {limit}", allowed_clause(&allowed));
    let mut args: Vec<rusqlite::types::Value> = vec![user.into()];
    args.extend_from_slice(extra);
    let mut st = c.prepare(&sql)?;
    let raws = st.query_map(params_from_iter(args.iter()), item_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    attach_images(c, raws)
}

fn visible(c: &Connection, user: i64, item: i64) -> rusqlite::Result<bool> {
    let lib: Option<i64> =
        c.query_row("SELECT library_id FROM items WHERE id = ?1", [item], |r| r.get(0)).optional()?;
    Ok(match (lib, allowed(c, user)?) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(l), Some(a)) => a.contains(&l),
    })
}

/// The descriptive columns of an item.
type DetailRow = (Option<String>, Option<String>, Option<f64>, Option<String>, Option<String>, String, String, String);

fn detail(c: &Connection, user: i64, id: i64) -> rusqlite::Result<Option<ItemDetail>> {
    if !visible(c, user, id)? {
        return Ok(None);
    }
    let Some(item) = items_by_sql(c, user, "i.id = ?2", "i.id", 1, &[id.into()])?.pop() else { return Ok(None) };
    let (original_title, overview, rating, official_rating, premiered, provider_ids, locked, extra): DetailRow = c.query_row(
        "SELECT original_title, overview, rating, official_rating, premiered, provider_ids, locked_fields, extra FROM items WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
    )?;
    let mut st = c.prepare_cached(
        "SELECT g.name FROM item_genres ig JOIN genres g ON g.id = ig.genre_id WHERE ig.item_id = ?1 ORDER BY g.name",
    )?;
    let genres = st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    let mut st = c.prepare_cached(
        "SELECT p.id, p.name, ip.role, ip.character FROM item_people ip JOIN people p ON p.id = ip.person_id WHERE ip.item_id = ?1 ORDER BY ip.ord",
    )?;
    let credits = st
        .query_map([id], |r| {
            Ok(Credit { person_id: r.get(0)?, name: r.get(1)?, role: r.get(2)?, character: r.get(3)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut st = c.prepare_cached(
        "SELECT f.id, f.path, f.size, json_extract(f.probe, '$.format'), json_extract(f.probe, '$.duration'), x.role, \
         json_extract(f.source, '$.torrent'), f.missing_since \
         FROM item_files x JOIN files f ON f.id = x.file_id WHERE x.item_id = ?1 ORDER BY x.role = 'main' DESC, f.path",
    )?;
    let rows = st
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<i64>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut files = Vec::new();
    for (fid, path, size, container, duration, role, torrent, missing) in rows {
        let mut st = c.prepare_cached(
            "SELECT idx, kind, codec, profile, lang, title, is_default, forced, width, height, fps, bitrate, channels, sample_rate, \
             external_path IS NOT NULL FROM streams WHERE file_id = ?1 ORDER BY kind, idx",
        )?;
        let streams = st
            .query_map([fid], |r| {
                Ok(Stream {
                    index: r.get(0)?,
                    kind: r.get(1)?,
                    codec: r.get(2)?,
                    profile: r.get(3)?,
                    lang: r.get(4)?,
                    title: r.get(5)?,
                    default: r.get::<_, i64>(6)? != 0,
                    forced: r.get::<_, i64>(7)? != 0,
                    width: r.get(8)?,
                    height: r.get(9)?,
                    fps: r.get(10)?,
                    bitrate: r.get(11)?,
                    channels: r.get(12)?,
                    sample_rate: r.get(13)?,
                    external: r.get(14)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut st = c.prepare_cached("SELECT start_ms, title FROM chapters WHERE file_id = ?1 ORDER BY idx")?;
        let chapters = st
            .query_map([fid], |r| Ok(Chapter { start_ms: r.get(0)?, title: r.get(1)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        files.push(MediaFile {
            id: fid,
            path,
            size,
            container,
            duration_ms: duration.map(|d| (d * 1000.0) as i64),
            role,
            streams,
            chapters,
            torrent,
            missing: missing.is_some(),
        });
    }
    let to_map = |s: &str| -> BTreeMap<String, serde_json::Value> {
        serde_json::from_str::<BTreeMap<String, serde_json::Value>>(s)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, v)| !v.is_null())
            .collect()
    };
    Ok(Some(ItemDetail {
        item,
        original_title,
        overview,
        rating,
        official_rating,
        premiered,
        genres,
        credits,
        files,
        provider_ids: to_map(&provider_ids)
            .into_iter()
            .map(|(k, v)| (k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
            .collect(),
        locked_fields: locked.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        extra: to_map(&extra),
    }))
}

/// Builds an FTS5 query: every word as a quoted prefix term.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(8)
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

impl Library {
    pub async fn items(&self, user: i64, q: ItemQuery) -> Result<Page<Item>> {
        Ok(self.db().read(move |c| query_items(c, user, &q)).await?)
    }

    pub async fn item(&self, user: i64, id: i64) -> Result<ItemDetail> {
        self.db().read(move |c| detail(c, user, id)).await?.ok_or(LibError::NotFound)
    }

    /// The file to play for an item (its main file; for containers, nothing).
    pub async fn main_file(&self, user: i64, id: i64) -> Result<(i64, String)> {
        self.db()
            .read(move |c| {
                if !visible(c, user, id)? {
                    return Ok(None);
                }
                c.query_row(
                    "SELECT f.id, f.path FROM item_files x JOIN files f ON f.id = x.file_id WHERE x.item_id = ?1 AND x.role = 'main' \
                     AND f.missing_since IS NULL ORDER BY f.path LIMIT 1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    /// A file's path, if the user may see an item that uses it.
    pub async fn file_path(&self, user: i64, file_id: i64) -> Result<String> {
        self.db()
            .read(move |c| {
                let row: Option<(String, i64)> = c
                    .query_row(
                        "SELECT f.path, x.item_id FROM files f JOIN item_files x ON x.file_id = f.id WHERE f.id = ?1 LIMIT 1",
                        [file_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                match row {
                    Some((p, item)) if visible(c, user, item)? => Ok(Some(p)),
                    _ => Ok(None),
                }
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    /// How many library files lie under `prefix` (the delete guard for torrents).
    pub async fn files_under(&self, prefix: &std::path::Path) -> Result<i64> {
        let p = prefix.display().to_string();
        Ok(self
            .db()
            .read(move |c| {
                c.query_row(
                    "SELECT count(*) FROM files WHERE missing_since IS NULL AND (path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || ?2)",
                    params![p, std::path::MAIN_SEPARATOR.to_string()],
                    |r| r.get(0),
                )
            })
            .await?)
    }

    /// Whether every enabled account that can see `library` has played every item of it whose
    /// main file lies under `prefix` (`None`: no such items, or nobody can see the library).
    pub async fn watched_by_everyone(&self, library: i64, prefix: &std::path::Path) -> Result<Option<bool>> {
        let p = prefix.display().to_string();
        Ok(self
            .db()
            .read(move |c| {
                let sep = std::path::MAIN_SEPARATOR.to_string();
                let items: Vec<i64> = c
                    .prepare(
                        "SELECT DISTINCT x.item_id FROM files f JOIN item_files x ON x.file_id = f.id JOIN items i ON i.id = x.item_id \
                         WHERE x.role = 'main' AND i.library_id = ?1 AND f.missing_since IS NULL AND i.kind IN ('movie','episode','track','book','video') \
                         AND (f.path = ?2 OR substr(f.path, 1, length(?2) + 1) = ?2 || ?3)",
                    )?
                    .query_map(params![library, p, sep], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let users: Vec<i64> = c
                    .prepare(
                        "SELECT u.id FROM users u WHERE u.disabled = 0 AND (NOT EXISTS (SELECT 1 FROM user_libraries a WHERE a.user_id = u.id) \
                         OR EXISTS (SELECT 1 FROM user_libraries a WHERE a.user_id = u.id AND a.library_id = ?1))",
                    )?
                    .query_map([library], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                if items.is_empty() || users.is_empty() {
                    return Ok(None);
                }
                let mut q = c.prepare_cached("SELECT played FROM watch_state WHERE user_id = ?1 AND item_id = ?2")?;
                for u in &users {
                    for i in &items {
                        let played: Option<bool> = q.query_row([u, i], |r| r.get(0)).optional()?;
                        if played != Some(true) {
                            return Ok(Some(false));
                        }
                    }
                }
                Ok(Some(true))
            })
            .await?)
    }

    /// Some main file inside an item (itself or any descendant): where its folder is.
    pub async fn any_file(&self, item: i64) -> Result<Option<String>> {
        Ok(self
            .db()
            .read(move |c| {
                c.query_row(
                    "WITH RECURSIVE d(id) AS (SELECT ?1 UNION ALL SELECT i.id FROM items i JOIN d ON i.parent_id = d.id) \
                     SELECT f.path FROM d JOIN item_files x ON x.item_id = d.id AND x.role = 'main' JOIN files f ON f.id = x.file_id \
                     WHERE f.missing_since IS NULL ORDER BY f.path LIMIT 1",
                    [item],
                    |r| r.get(0),
                )
                .optional()
            })
            .await?)
    }

    /// The file of an external subtitle stream, if the user may see the media file.
    pub async fn subtitle_path(&self, user: i64, file_id: i64, idx: i64) -> Result<String> {
        self.file_path(user, file_id).await?;
        self.db()
            .read(move |c| {
                c.query_row(
                    "SELECT external_path FROM streams WHERE file_id = ?1 AND idx = ?2 AND kind = 'subtitle' AND external_path IS NOT NULL",
                    params![file_id, idx],
                    |r| r.get(0),
                )
                .optional()
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    pub async fn search(&self, user: i64, q: &str) -> Result<SearchResults> {
        let Some(fts) = fts_query(q) else { return Ok(SearchResults::default()) };
        let like = format!("%{}%", q.trim().replace(['%', '_'], ""));
        Ok(self
            .db()
            .read(move |c| {
                let items = items_by_sql(
                    c,
                    user,
                    "i.id IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?2 ORDER BY bm25(items_fts, 10.0, 5.0, 2.0, 1.0) LIMIT 200)",
                    "CASE i.kind WHEN 'movie' THEN 0 WHEN 'show' THEN 0 WHEN 'artist' THEN 1 WHEN 'album' THEN 1 WHEN 'book' THEN 1 ELSE 2 END, i.sort_title",
                    60,
                    &[fts.into()],
                )?;
                let mut st = c.prepare("SELECT id, name FROM people WHERE name LIKE ?1 ORDER BY name LIMIT 20")?;
                let people = st
                    .query_map([like], |r| Ok(Credit { person_id: r.get(0)?, name: r.get(1)?, role: String::new(), character: None }))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(SearchResults { items, people })
            })
            .await?)
    }

    /// Home screen rows for a user.
    pub async fn home(&self, user: i64) -> Result<Vec<HomeRow>> {
        let libs = self.list().await?;
        Ok(self
            .db()
            .read(move |c| {
                let mut rows = Vec::new();
                let cont = items_by_sql(
                    c,
                    user,
                    "ws.position_ms > 0 AND coalesce(ws.played, 0) = 0 AND i.kind IN ('movie','episode','book','video')",
                    "ws.last_played_at DESC",
                    20,
                    &[],
                )?;
                if !cont.is_empty() {
                    rows.push(HomeRow { id: "continue".into(), title: t!("Continue watching").into(), items: cont });
                }
                let next = next_up(c, user)?;
                if !next.is_empty() {
                    rows.push(HomeRow { id: "next_up".into(), title: t!("Next up").into(), items: next });
                }
                for l in libs {
                    let kind = match l.kind {
                        corrent_proto::library::LibraryKind::Shows => "episode",
                        corrent_proto::library::LibraryKind::Music => "album",
                        corrent_proto::library::LibraryKind::Movies => "movie",
                        corrent_proto::library::LibraryKind::Audiobooks => "book",
                        corrent_proto::library::LibraryKind::Videos => "video",
                        corrent_proto::library::LibraryKind::Photos => "photo",
                        corrent_proto::library::LibraryKind::Podcasts => "episode",
                    };
                    let items = items_by_sql(
                        c,
                        user,
                        "i.library_id = ?2 AND i.kind = ?3",
                        "i.added_at DESC, i.id DESC",
                        20,
                        &[l.id.into(), kind.to_string().into()],
                    )?;
                    if !items.is_empty() {
                        rows.push(HomeRow {
                            id: format!("recent:{}", l.id),
                            title: t!("Recently added in {library}", library = l.name),
                            items,
                        });
                    }
                }
                let fav = items_by_sql(c, user, "coalesce(ws.favorite, 0) = 1", "i.sort_title", 30, &[])?;
                if !fav.is_empty() {
                    rows.push(HomeRow { id: "favorites".into(), title: t!("Favorites").into(), items: fav });
                }
                Ok(rows)
            })
            .await?)
    }

    /// Changes a user's state for an item. Marking a container played/unplayed applies to every
    /// playable item in it. A position past the library's threshold marks the item played.
    pub async fn set_state(&self, user: i64, id: i64, u: StateUpdate) -> Result<UserState> {
        let ok = self.db().read(move |c| visible(c, user, id)).await?;
        if !ok {
            return Err(LibError::NotFound);
        }
        let now = now_unix();
        self.db()
            .write(move |t| {
                let (kind, runtime, threshold): (String, Option<i64>, Option<i64>) = t.query_row(
                    "SELECT i.kind, i.runtime_ms, json_extract(l.settings, '$.played_threshold') FROM items i JOIN libraries l ON l.id = i.library_id WHERE i.id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                let ensure = |item: i64| {
                    t.execute("INSERT OR IGNORE INTO watch_state(user_id, item_id) VALUES (?1, ?2)", params![user, item])
                };
                ensure(id)?;
                if let Some(f) = u.favorite {
                    t.execute("UPDATE watch_state SET favorite = ?1 WHERE user_id = ?2 AND item_id = ?3", params![f, user, id])?;
                }
                if let Some(r) = u.rating {
                    t.execute("UPDATE watch_state SET rating = ?1 WHERE user_id = ?2 AND item_id = ?3", params![r, user, id])?;
                }
                let mut played = u.played;
                if let Some(pos) = u.position_ms {
                    let pos = pos.max(0);
                    t.execute(
                        "UPDATE watch_state SET position_ms = ?1, last_played_at = ?2 WHERE user_id = ?3 AND item_id = ?4",
                        params![pos, now, user, id],
                    )?;
                    let th = threshold.unwrap_or(90).clamp(50, 100);
                    if played.is_none() && runtime.is_some_and(|r| r > 0 && pos * 100 >= r * th) {
                        played = Some(true);
                    }
                }
                if let Some(p) = played {
                    let kind = ItemKind::parse(&kind).unwrap_or(ItemKind::Video);
                    let targets: Vec<i64> = if kind.playable() {
                        vec![id]
                    } else {
                        let mut st = t.prepare(
                            "WITH RECURSIVE d(id) AS (SELECT ?1 UNION ALL SELECT i.id FROM items i JOIN d ON i.parent_id = d.id) \
                             SELECT i.id FROM items i JOIN d ON d.id = i.id WHERE i.kind IN ('movie','episode','track','book','video')",
                        )?;
                        st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
                    };
                    for item in targets {
                        ensure(item)?;
                        if p {
                            t.execute(
                                "UPDATE watch_state SET played = 1, position_ms = 0, play_count = play_count + 1, last_played_at = ?1 \
                                 WHERE user_id = ?2 AND item_id = ?3",
                                params![now, user, item],
                            )?;
                        } else {
                            t.execute(
                                "UPDATE watch_state SET played = 0, position_ms = 0 WHERE user_id = ?1 AND item_id = ?2",
                                params![user, item],
                            )?;
                        }
                    }
                }
                t.query_row(
                    "SELECT position_ms, played, play_count, last_played_at, favorite, rating FROM watch_state WHERE user_id = ?1 AND item_id = ?2",
                    params![user, id],
                    |r| {
                        Ok(UserState {
                            position_ms: r.get(0)?,
                            played: r.get::<_, i64>(1)? != 0,
                            play_count: r.get(2)?,
                            last_played_at: r.get(3)?,
                            favorite: r.get::<_, i64>(4)? != 0,
                            rating: r.get(5)?,
                            unplayed: None,
                        })
                    },
                )
            })
            .await
            .map_err(Into::into)
    }

    /// Limits which libraries a user sees (empty = all).
    pub async fn set_user_libraries(&self, user: i64, libraries: Vec<i64>) -> Result<()> {
        self.db()
            .write(move |t| {
                t.execute("DELETE FROM user_libraries WHERE user_id = ?1", [user])?;
                for l in &libraries {
                    t.execute(
                        "INSERT OR IGNORE INTO user_libraries(user_id, library_id) VALUES (?1, ?2)",
                        params![user, l],
                    )?;
                }
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn user_libraries(&self, user: i64) -> Result<Vec<i64>> {
        Ok(self.db().read(move |c| allowed(c, user).map(Option::unwrap_or_default)).await?)
    }

    /// Locks fields against metadata refreshes (and edits them).
    pub async fn edit_item(
        &self,
        id: i64,
        title: Option<String>,
        overview: Option<String>,
        year: Option<i32>,
        lock: Vec<String>,
    ) -> Result<()> {
        let n = self
            .db()
            .write(move |t| {
                let n = t.execute("UPDATE items SET updated_at = ?1 WHERE id = ?2", params![now_unix(), id])?;
                if let Some(v) = &title {
                    t.execute(
                        "UPDATE items SET title = ?1, sort_title = ?2 WHERE id = ?3",
                        params![v, crate::parse::sort_title(v), id],
                    )?;
                }
                if let Some(v) = &overview {
                    t.execute("UPDATE items SET overview = ?1 WHERE id = ?2", params![v, id])?;
                }
                if let Some(v) = year {
                    t.execute("UPDATE items SET year = ?1 WHERE id = ?2", params![v, id])?;
                }
                t.execute("UPDATE items SET locked_fields = ?1 WHERE id = ?2", params![lock.join(","), id])?;
                crate::scan::refresh_fts(t, &[id])?;
                Ok(n)
            })
            .await?;
        if n == 0 { Err(LibError::NotFound) } else { Ok(()) }
    }
}

/// For each show the user is watching, the first unplayed episode after the last played one.
fn next_up(c: &Connection, user: i64) -> rusqlite::Result<Vec<Item>> {
    let mut st = c.prepare(
        "SELECT s.parent_id, max(w.last_played_at) FROM watch_state w JOIN items e ON e.id = w.item_id AND e.kind = 'episode' \
         JOIN items s ON s.id = e.parent_id WHERE w.user_id = ?1 AND w.played = 1 GROUP BY s.parent_id ORDER BY 2 DESC LIMIT 20",
    )?;
    let shows: Vec<i64> = st.query_map([user], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for show in shows {
        out.extend(next_in_show(c, user, show, false)?);
    }
    Ok(out)
}

/// The next unwatched episode of a show after the last watched one (`first`: S01E01 when
/// nothing was watched).
fn next_in_show(c: &Connection, user: i64, show: i64, first: bool) -> rusqlite::Result<Vec<Item>> {
    {
        // Last played (season, episode) in the show.
        let last: Option<(i64, i64)> = c
            .query_row(
                "SELECT s.index_no, e.index_no FROM watch_state w JOIN items e ON e.id = w.item_id JOIN items s ON s.id = e.parent_id \
                 WHERE w.user_id = ?1 AND w.played = 1 AND s.parent_id = ?2 AND s.index_no > 0 ORDER BY s.index_no DESC, e.index_no DESC LIMIT 1",
                params![user, show],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (sn, en) = match last {
            Some(l) => l,
            None if first => (0, 0),
            None => return Ok(Vec::new()),
        };
        items_by_sql(
            c,
            user,
            "i.kind = 'episode' AND p.parent_id = ?2 AND coalesce(ws.played, 0) = 0 AND p.index_no > 0 \
             AND (p.index_no > ?3 OR (p.index_no = ?3 AND i.index_no > ?4))",
            "p.index_no, i.index_no",
            1,
            &[show.into(), sn.into(), en.into()],
        )
    }
}

impl Library {
    /// "Next up": per recently watched show its next episode, or for one show (`series`) its
    /// next one (S01E01 when nothing was watched and `first`).
    pub async fn next_up(&self, user: i64, series: Option<i64>, first: bool) -> Result<Vec<Item>> {
        Ok(self
            .db()
            .read(move |c| match series {
                Some(s) => next_in_show(c, user, s, first),
                None => next_up(c, user),
            })
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_terms() {
        assert_eq!(fts_query("matrix rel").as_deref(), Some("\"matrix\"* \"rel\"*"));
        assert_eq!(fts_query("  \"; DROP --").as_deref(), Some("\"DROP\"*"));
        assert_eq!(fts_query("!!"), None);
    }
}

/// What a subtitle search needs to know about an item and its main video file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubtitleTarget {
    /// The movie's or the show's title.
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    /// Provider ids of the movie or show (`tmdb`, `imdb`, …).
    pub ids: BTreeMap<String, String>,
    pub file_id: i64,
    pub path: String,
    pub size: i64,
}

impl Library {
    /// The movie or episode to find subtitles for (`user` must see it).
    pub async fn subtitle_target(&self, user: i64, item: i64) -> Result<SubtitleTarget> {
        let (file_id, path) = self.main_file(user, item).await?;
        self.db()
            .read(move |c| {
                let row = c
                    .query_row(
                        "SELECT i.kind, i.title, i.year, i.index_no, i.provider_ids, s.index_no, sh.title, sh.year, sh.provider_ids, \
                         (SELECT size FROM files WHERE id = ?2) \
                         FROM items i LEFT JOIN items s ON s.id = i.parent_id LEFT JOIN items sh ON sh.id = s.parent_id WHERE i.id = ?1",
                        params![item, file_id],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, Option<i32>>(2)?,
                                r.get::<_, Option<i64>>(3)?,
                                r.get::<_, String>(4)?,
                                r.get::<_, Option<i64>>(5)?,
                                r.get::<_, Option<String>>(6)?,
                                r.get::<_, Option<i32>>(7)?,
                                r.get::<_, Option<String>>(8)?,
                                r.get::<_, Option<i64>>(9)?,
                            ))
                        },
                    )
                    .optional()?;
                Ok(row.map(|(kind, title, year, idx, ids, season, show, show_year, show_ids, size)| {
                    let parse = |j: &str| serde_json::from_str::<BTreeMap<String, String>>(j).unwrap_or_default();
                    if kind == "episode" {
                        SubtitleTarget {
                            title: show.unwrap_or(title),
                            year: show_year.or(year),
                            season,
                            episode: idx,
                            ids: show_ids.as_deref().map(parse).unwrap_or_default(),
                            file_id,
                            path: path.clone(),
                            size: size.unwrap_or(0),
                        }
                    } else {
                        SubtitleTarget {
                            title,
                            year,
                            ids: parse(&ids),
                            file_id,
                            path: path.clone(),
                            size: size.unwrap_or(0),
                            ..Default::default()
                        }
                    }
                }))
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    /// Records a subtitle file that was just saved next to a video (a later scan finds it too).
    pub async fn add_external_subtitle(&self, file_id: i64, path: String, lang: Option<String>) -> Result<()> {
        let codec = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        self.db()
            .write(move |t| {
                let next: i64 = t.query_row(
                    "SELECT coalesce(max(idx), 999) + 1 FROM streams WHERE file_id = ?1 AND idx >= 1000",
                    [file_id],
                    |r| r.get(0),
                )?;
                t.execute(
                    "INSERT INTO streams(file_id, idx, kind, codec, lang, forced, external_path) VALUES (?1, ?2, 'subtitle', ?3, ?4, 0, ?5)",
                    params![file_id, next, codec, lang, path],
                )
                .map(drop)
            })
            .await?;
        Ok(())
    }
}
