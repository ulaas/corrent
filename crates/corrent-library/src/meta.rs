//! Applying metadata from providers to items: never over locked fields, local artwork wins,
//! and collections are created as providers name them.

use std::collections::BTreeMap;

use corrent_db::rusqlite::{self, OptionalExtension, params};
use corrent_proto::library::ItemKind;

use crate::scan::refresh_fts;
use crate::{Library, Result, artwork, now_unix};

/// Provider details in the library's terms (independent of the plugin crate).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaDetails {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub premiered: Option<String>,
    pub runtime_minutes: Option<u32>,
    pub rating: Option<f32>,
    pub official_rating: Option<String>,
    pub genres: Vec<String>,
    /// (name, role, character)
    pub credits: Vec<(String, String, Option<String>)>,
    pub ids: BTreeMap<String, String>,
    pub collection: Option<String>,
}

/// (episode number, title, overview, air date) from a provider.
pub type EpisodeMeta = (i64, Option<String>, Option<String>, Option<String>);

/// An item waiting for metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: i64,
    pub library_id: i64,
    pub kind: ItemKind,
    pub title: String,
    pub year: Option<i32>,
    pub ids: BTreeMap<String, String>,
    /// Artist name (albums).
    pub artist: Option<String>,
}

impl Library {
    /// Movies, shows, albums and artists never checked against providers (or asked again).
    pub async fn items_needing_metadata(&self, library_id: i64, limit: i64) -> Result<Vec<Pending>> {
        Ok(self
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT i.id, i.library_id, i.kind, i.title, i.year, i.provider_ids, p.title FROM items i \
                     LEFT JOIN items p ON p.id = i.parent_id \
                     WHERE i.library_id = ?1 AND i.kind IN ('movie','show','album','artist') \
                     AND json_extract(i.extra, '$.meta_checked') IS NULL ORDER BY i.added_at DESC LIMIT ?2",
                )?;
                st.query_map(params![library_id, limit], |r| {
                    let kind: String = r.get(2)?;
                    let kind = ItemKind::parse(&kind).unwrap_or(ItemKind::Movie);
                    Ok(Pending {
                        id: r.get(0)?,
                        library_id: r.get(1)?,
                        kind,
                        title: r.get(3)?,
                        year: r.get(4)?,
                        ids: serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&r.get::<_, String>(5)?)
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                            .collect(),
                        artist: (kind == ItemKind::Album).then(|| r.get::<_, Option<String>>(6)).transpose()?.flatten(),
                    })
                })?
                .collect()
            })
            .await?)
    }

    pub async fn pending_item(&self, id: i64) -> Result<Pending> {
        let lib = self
            .db()
            .read(move |c| {
                c.query_row("SELECT library_id FROM items WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).optional()
            })
            .await?;
        let lib = lib.ok_or(crate::LibError::NotFound)?;
        // Reset the checked flag so the item is listed, then fetch it.
        self.mark_meta_checked(id, None).await?;
        self.items_needing_metadata(lib, 100_000)
            .await?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or(crate::LibError::NotFound)
    }

    /// Remembers that an item was checked (`Some(provider)`), or asks for a new check (`None`).
    pub async fn mark_meta_checked(&self, id: i64, provider: Option<String>) -> Result<()> {
        let now = now_unix();
        self.db()
            .write(move |t| match provider {
                Some(p) => t.execute(
                    "UPDATE items SET extra = json_set(extra, '$.meta_checked', ?1, '$.meta_provider', ?2) WHERE id = ?3",
                    params![now, p, id],
                ),
                None => t.execute("UPDATE items SET extra = json_remove(extra, '$.meta_checked') WHERE id = ?1", [id]),
            })
            .await?;
        Ok(())
    }

    /// Applies provider details to an item (locked fields stay as they are).
    pub async fn apply_details(&self, id: i64, d: MetaDetails) -> Result<()> {
        let now = now_unix();
        self.db()
            .write(move |t| {
                let (lock_s, library_id): (String, i64) =
                    t.query_row("SELECT locked_fields, library_id FROM items WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let lock: Vec<&str> = lock_s.split(',').map(str::trim).collect();
                let free = |f: &str| !lock.contains(&f);
                macro_rules! set {
                    ($field:literal, $col:literal, $v:expr) => {
                        if free($field)
                            && let Some(v) = $v
                        {
                            t.execute(concat!("UPDATE items SET ", $col, " = ?1 WHERE id = ?2"), params![v, id])?;
                        }
                    };
                }
                if free("title")
                    && let Some(title) = d.title.as_ref().filter(|t| !t.trim().is_empty())
                {
                    t.execute(
                        "UPDATE items SET title = ?1, sort_title = ?2 WHERE id = ?3",
                        params![title, crate::parse::sort_title(title), id],
                    )?;
                }
                set!("original_title", "original_title", d.original_title.as_ref());
                set!("overview", "overview", d.overview.as_ref());
                set!("year", "year", d.year);
                set!("premiered", "premiered", d.premiered.as_ref());
                set!("rating", "rating", d.rating);
                set!("official_rating", "official_rating", d.official_rating.as_ref());
                if free("runtime")
                    && let Some(m) = d.runtime_minutes
                {
                    t.execute("UPDATE items SET runtime_ms = coalesce(runtime_ms, ?1) WHERE id = ?2", params![m as i64 * 60_000, id])?;
                }
                let cur: String = t.query_row("SELECT provider_ids FROM items WHERE id = ?1", [id], |r| r.get(0))?;
                let mut ids: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&cur).unwrap_or_default();
                for (k, v) in &d.ids {
                    ids.insert(k.clone(), v.clone().into());
                }
                t.execute("UPDATE items SET provider_ids = ?1, updated_at = ?2 WHERE id = ?3", params![serde_json::Value::Object(ids).to_string(), now, id])?;
                if free("genres") && !d.genres.is_empty() {
                    t.execute("DELETE FROM item_genres WHERE item_id = ?1", [id])?;
                    for g in d.genres.iter().take(20) {
                        t.execute("INSERT OR IGNORE INTO genres(name) VALUES (?1)", [g])?;
                        let gid: i64 = t.query_row("SELECT id FROM genres WHERE name = ?1", [g], |r| r.get(0))?;
                        t.execute("INSERT OR IGNORE INTO item_genres(item_id, genre_id) VALUES (?1, ?2)", params![id, gid])?;
                    }
                }
                if free("people") && !d.credits.is_empty() {
                    t.execute("DELETE FROM item_people WHERE item_id = ?1", [id])?;
                    for (ord, (name, role, character)) in d.credits.iter().take(60).enumerate() {
                        t.execute("INSERT OR IGNORE INTO people(name) VALUES (?1)", [name])?;
                        let pid: i64 = t.query_row("SELECT id FROM people WHERE name = ?1", [name], |r| r.get(0))?;
                        t.execute(
                            "INSERT OR IGNORE INTO item_people(item_id, person_id, role, character, ord) VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![id, pid, role, character, ord as i64],
                        )?;
                    }
                }
                if let Some(c) = d.collection.as_ref().filter(|c| !c.trim().is_empty()) {
                    let cid: Option<i64> = t
                        .query_row("SELECT id FROM collections WHERE library_id = ?1 AND name = ?2", params![library_id, c], |r| r.get(0))
                        .optional()?;
                    let cid = match cid {
                        Some(x) => x,
                        None => {
                            t.execute("INSERT INTO collections(library_id, name) VALUES (?1, ?2)", params![library_id, c])?;
                            t.last_insert_rowid()
                        }
                    };
                    t.execute("INSERT OR IGNORE INTO collection_items(collection_id, item_id) VALUES (?1, ?2)", params![cid, id])?;
                }
                refresh_fts(t, &[id])?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Stores image bytes as an item's artwork of `kind`; with `only_if_missing`, an existing
    /// image (e.g. a local poster) is kept.
    pub async fn set_artwork(
        &self,
        item: i64,
        kind: &str,
        data: Vec<u8>,
        source: &str,
        only_if_missing: bool,
    ) -> Result<bool> {
        let k = kind.to_string();
        if only_if_missing {
            let has = self
                .db()
                .read(move |c| {
                    c.query_row(
                        "SELECT count(*) FROM images WHERE owner_kind = 'item' AND owner_id = ?1 AND kind = ?2",
                        params![item, k],
                        |r| r.get::<_, i64>(0),
                    )
                })
                .await?;
            if has > 0 {
                return Ok(false);
            }
        }
        let _permit = self.image_permit().await;
        let root = self.artwork_root().to_path_buf();
        let stored = tokio::task::spawn_blocking(move || artwork::store(&root, &data))
            .await
            .map_err(|e| crate::LibError::Db(e.to_string()))?
            .map_err(crate::LibError::Invalid)?;
        let (kind, source) = (kind.to_string(), source.to_string());
        self.db()
            .write(move |t| {
                t.execute(
                    "INSERT INTO images(owner_kind, owner_id, kind, sha256, width, height, blurhash, source) \
                     VALUES ('item', ?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                     ON CONFLICT(owner_kind, owner_id, kind) DO UPDATE SET sha256 = excluded.sha256, width = excluded.width, \
                     height = excluded.height, blurhash = excluded.blurhash, source = excluded.source",
                    params![item, kind, stored.sha, stored.width, stored.height, stored.blurhash, source],
                )
            })
            .await?;
        Ok(true)
    }

    /// Updates episodes of a season from a provider's episode list (by episode number).
    pub async fn apply_episodes(&self, season_item: i64, episodes: Vec<EpisodeMeta>) -> Result<Vec<(i64, i64)>> {
        Ok(self
            .db()
            .write(move |t| {
                let mut touched = Vec::new();
                for (num, title, overview, aired) in &episodes {
                    let row: Option<(i64, String)> = t
                        .query_row(
                            "SELECT id, locked_fields FROM items WHERE parent_id = ?1 AND kind = 'episode' AND index_no = ?2",
                            params![season_item, num],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?;
                    let Some((id, lock)) = row else { continue };
                    if !lock.contains("title")
                        && let Some(tl) = title
                    {
                        t.execute("UPDATE items SET title = ?1 WHERE id = ?2", params![tl, id])?;
                    }
                    if !lock.contains("overview")
                        && let Some(o) = overview
                    {
                        t.execute("UPDATE items SET overview = ?1 WHERE id = ?2", params![o, id])?;
                    }
                    if let Some(a) = aired {
                        t.execute("UPDATE items SET premiered = ?1 WHERE id = ?2", params![a, id])?;
                    }
                    touched.push((*num, id));
                }
                let ids: Vec<i64> = touched.iter().map(|(_, id)| *id).collect();
                refresh_fts(t, &ids)?;
                Ok(touched)
            })
            .await?)
    }

    /// Seasons of a show: (season item id, season number).
    pub async fn seasons(&self, show: i64) -> Result<Vec<(i64, i64)>> {
        Ok(self
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT id, index_no FROM items WHERE parent_id = ?1 AND kind = 'season' ORDER BY index_no",
                )?;
                st.query_map([show], |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0))))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .await?)
    }

    /// The episode numbers a season already has.
    pub async fn episode_numbers(&self, season_item: i64) -> Result<Vec<i64>> {
        Ok(self
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT index_no FROM items WHERE parent_id = ?1 AND kind = 'episode' AND index_no IS NOT NULL",
                )?;
                st.query_map([season_item], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()
            })
            .await?)
    }
}
