//! Playlists (a person's ordered items; others see them when public) and collections (film
//! series named by metadata providers). Items are only listed when the reader may see them.

use corrent_db::rusqlite::{OptionalExtension, params};
use corrent_proto::library::{Collection, Item, ItemQuery, Playlist};

use crate::{LibError, Library, Result, now_unix};

/// Longest playlist.
const MAX_ITEMS: i64 = 5000;

impl Library {
    async fn visible_items(&self, user: i64, ids: Vec<i64>) -> Result<Vec<Item>> {
        let mut out = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(500) {
            let page = self
                .items(user, ItemQuery { ids: Some(chunk.to_vec()), limit: Some(500), ..Default::default() })
                .await?;
            out.extend(page.items);
        }
        // In the list's order (a query sorts by title).
        let pos: std::collections::HashMap<i64, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        out.sort_by_key(|i| pos.get(&i.id).copied().unwrap_or(usize::MAX));
        Ok(out)
    }

    /// The caller's playlists and others' public ones.
    pub async fn playlists(&self, user: i64) -> Result<Vec<Playlist>> {
        let rows: Vec<(Playlist, Vec<i64>)> = self
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT p.id, p.name, p.public, u.name, p.user_id, p.created_at FROM playlists p JOIN users u ON u.id = p.user_id \
                     WHERE p.user_id = ?1 OR p.public = 1 ORDER BY p.user_id != ?1, p.name COLLATE NOCASE",
                )?;
                let lists = st
                    .query_map([user], |r| {
                        Ok(Playlist {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            public: r.get::<_, i64>(2)? != 0,
                            owner: r.get(3)?,
                            mine: r.get::<_, i64>(4)? == user,
                            created_at: r.get(5)?,
                            ..Default::default()
                        })
                    })?
                    .collect::<corrent_db::rusqlite::Result<Vec<_>>>()?;
                let mut out = Vec::new();
                for p in lists {
                    let mut st = c.prepare_cached("SELECT item_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY pos")?;
                    let ids: Vec<i64> = st.query_map([p.id], |r| r.get(0))?.collect::<corrent_db::rusqlite::Result<_>>()?;
                    out.push((p, ids));
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::new();
        for (mut p, ids) in rows {
            let items = self.visible_items(user, ids).await?;
            p.count = items.len() as i64;
            p.runtime_ms = items.iter().filter_map(|i| i.runtime_ms).sum();
            p.images = items.iter().filter_map(|i| i.images.first().cloned()).take(4).collect();
            out.push(p);
        }
        Ok(out)
    }

    /// Who owns a playlist and whether it's public (`None`: no such playlist).
    async fn playlist_owner(&self, id: i64) -> Result<Option<(i64, bool)>> {
        Ok(self
            .db()
            .read(move |c| {
                c.query_row("SELECT user_id, public FROM playlists WHERE id = ?1", [id], |r| {
                    Ok((r.get(0)?, r.get::<_, i64>(1)? != 0))
                })
                .optional()
            })
            .await?)
    }

    async fn own(&self, user: i64, id: i64) -> Result<()> {
        match self.playlist_owner(id).await? {
            Some((owner, _)) if owner == user => Ok(()),
            _ => Err(LibError::NotFound),
        }
    }

    pub async fn create_playlist(&self, user: i64, name: String, public: bool) -> Result<i64> {
        let name: String = name.trim().chars().take(120).collect();
        if name.is_empty() {
            return Err(LibError::Invalid("a playlist needs a name".into()));
        }
        let now = now_unix();
        Ok(self
            .db()
            .write(move |t| {
                t.execute(
                    "INSERT INTO playlists(user_id, name, public, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![user, name, public, now],
                )?;
                Ok(t.last_insert_rowid())
            })
            .await?)
    }

    pub async fn update_playlist(&self, user: i64, id: i64, name: Option<String>, public: Option<bool>) -> Result<()> {
        self.own(user, id).await?;
        let name = name.map(|n| n.trim().chars().take(120).collect::<String>()).filter(|n| !n.is_empty());
        self.db()
            .write(move |t| {
                if let Some(n) = name {
                    t.execute("UPDATE playlists SET name = ?1 WHERE id = ?2", params![n, id])?;
                }
                if let Some(p) = public {
                    t.execute("UPDATE playlists SET public = ?1 WHERE id = ?2", params![p, id])?;
                }
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn delete_playlist(&self, user: i64, id: i64) -> Result<()> {
        self.own(user, id).await?;
        self.db().write(move |t| t.execute("DELETE FROM playlists WHERE id = ?1", [id]).map(drop)).await?;
        Ok(())
    }

    /// A playlist's items the caller may see, in order.
    pub async fn playlist_items(&self, user: i64, id: i64) -> Result<Vec<Item>> {
        match self.playlist_owner(id).await? {
            Some((owner, public)) if owner == user || public => {}
            _ => return Err(LibError::NotFound),
        }
        let ids: Vec<i64> = self
            .db()
            .read(move |c| {
                let mut st = c.prepare("SELECT item_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY pos")?;
                st.query_map([id], |r| r.get(0))?.collect::<corrent_db::rusqlite::Result<_>>()
            })
            .await?;
        self.visible_items(user, ids).await
    }

    /// Appends items (ones the caller can see; containers add their playable items).
    pub async fn add_to_playlist(&self, user: i64, id: i64, items: Vec<i64>) -> Result<i64> {
        self.own(user, id).await?;
        let mut add = Vec::new();
        for i in items.into_iter().take(1000) {
            let d = self.item(user, i).await?;
            if d.item.kind.playable() {
                add.push(i);
            } else {
                let q = ItemQuery {
                    ancestor_id: Some(i),
                    kinds: corrent_proto::library::ItemKind::ALL.into_iter().filter(|k| k.playable()).collect(),
                    sort: corrent_proto::library::SortBy::Index,
                    limit: Some(500),
                    ..Default::default()
                };
                let mut below = self.items(user, q).await?.items;
                below.sort_by_key(|x| (x.parent_index.unwrap_or(0), x.index.unwrap_or(0)));
                add.extend(below.into_iter().map(|x| x.id));
            }
        }
        Ok(self
            .db()
            .write(move |t| {
                let (n, last): (i64, Option<i64>) =
                    t.query_row("SELECT count(*), max(pos) FROM playlist_items WHERE playlist_id = ?1", [id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?;
                let mut pos = last.unwrap_or(-1);
                let room = (MAX_ITEMS - n).max(0) as usize;
                for i in add.iter().take(room) {
                    pos += 1;
                    t.execute(
                        "INSERT INTO playlist_items(playlist_id, pos, item_id) VALUES (?1, ?2, ?3)",
                        params![id, pos, i],
                    )?;
                }
                Ok(add.len().min(room) as i64)
            })
            .await?)
    }

    /// Removes the item at `index` (0-based, in order) or moves it to `to`.
    pub async fn edit_playlist_entry(&self, user: i64, id: i64, index: usize, to: Option<usize>) -> Result<()> {
        self.own(user, id).await?;
        self.db()
            .write(move |t| {
                let mut st = t.prepare("SELECT item_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY pos")?;
                let mut ids: Vec<i64> =
                    st.query_map([id], |r| r.get(0))?.collect::<corrent_db::rusqlite::Result<_>>()?;
                drop(st);
                if index >= ids.len() {
                    return Ok(());
                }
                let it = ids.remove(index);
                if let Some(to) = to {
                    ids.insert(to.min(ids.len()), it);
                }
                t.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
                for (pos, i) in ids.iter().enumerate() {
                    t.execute(
                        "INSERT INTO playlist_items(playlist_id, pos, item_id) VALUES (?1, ?2, ?3)",
                        params![id, pos as i64, i],
                    )?;
                }
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Collections with at least one item the caller may see (in `library` when given).
    pub async fn collections(&self, user: i64, library: Option<i64>) -> Result<Vec<Collection>> {
        let rows: Vec<(Collection, Vec<i64>)> = self
            .db()
            .read(move |c| {
                let mut st = c.prepare("SELECT id, name, library_id FROM collections WHERE ?1 IS NULL OR library_id = ?1 ORDER BY name COLLATE NOCASE")?;
                let cols = st
                    .query_map([library], |r| Ok(Collection { id: r.get(0)?, name: r.get(1)?, library_id: r.get::<_, Option<i64>>(2)?.unwrap_or(0), ..Default::default() }))?
                    .collect::<corrent_db::rusqlite::Result<Vec<_>>>()?;
                let mut out = Vec::new();
                for col in cols {
                    let mut st = c.prepare_cached("SELECT item_id FROM collection_items WHERE collection_id = ?1")?;
                    let ids: Vec<i64> = st.query_map([col.id], |r| r.get(0))?.collect::<corrent_db::rusqlite::Result<_>>()?;
                    out.push((col, ids));
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::new();
        for (mut col, ids) in rows {
            let mut items = self.visible_items(user, ids).await?;
            if items.is_empty() {
                continue;
            }
            items.sort_by_key(|i| i.year.unwrap_or(9999));
            col.count = items.len() as i64;
            col.images =
                items.iter().filter_map(|i| i.images.iter().find(|im| im.kind == "poster").cloned()).take(1).collect();
            out.push(col);
        }
        Ok(out)
    }

    /// A collection's items, by year.
    pub async fn collection_items(&self, user: i64, id: i64) -> Result<Vec<Item>> {
        let ids: Vec<i64> = self
            .db()
            .read(move |c| {
                let mut st = c.prepare("SELECT item_id FROM collection_items WHERE collection_id = ?1")?;
                st.query_map([id], |r| r.get(0))?.collect::<corrent_db::rusqlite::Result<_>>()
            })
            .await?;
        let mut items = self.visible_items(user, ids).await?;
        if items.is_empty() {
            return Err(LibError::NotFound);
        }
        items.sort_by_key(|i| (i.year.unwrap_or(9999), i.title.to_lowercase()));
        Ok(items)
    }

    /// The collections an item is in.
    pub async fn collections_of(&self, item: i64) -> Result<Vec<(i64, String)>> {
        Ok(self
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT c.id, c.name FROM collections c JOIN collection_items x ON x.collection_id = c.id WHERE x.item_id = ?1 ORDER BY c.name",
                )?;
                st.query_map([item], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<corrent_db::rusqlite::Result<_>>()
            })
            .await?)
    }
}
