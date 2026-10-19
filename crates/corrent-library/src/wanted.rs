//! The wanted list and quality profiles (a light Sonarr/Radarr): judging releases against what
//! was wanted and how good it must be, and keeping track of what was grabbed.

use corrent_db::rusqlite::{self, OptionalExtension, Row, params};
use corrent_proto::library::{NewWanted, QualityProfile, QualityRules, Release, Wanted, WantedKind};

use crate::parse::{parse_name, sort_title};
use crate::{LibError, Library, Result, now_unix};

/// Comparable form of a title: lower case, letters and digits only.
fn norm(s: &str) -> String {
    sort_title(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Does a release name fit what is wanted (title, year, season/episode)?
pub fn fits(release: &str, w: &Wanted) -> std::result::Result<(), String> {
    let p = parse_name(release);
    if w.kind == WantedKind::Other {
        return Ok(());
    }
    if norm(&p.title) != norm(&w.title) {
        return Err(format!("title is “{}”", p.title));
    }
    match w.kind {
        WantedKind::Movie => {
            if let (Some(a), Some(b)) = (p.year, w.year)
                && (a as i32 - b).abs() > 1
            {
                return Err(format!("year {a}"));
            }
            if p.is_episode() {
                return Err("an episode".into());
            }
        }
        WantedKind::Episode => {
            if p.season.map(i64::from) != w.season || p.episode.map(i64::from) != w.episode {
                return Err("another episode".into());
            }
        }
        WantedKind::Season => {
            if p.season.map(i64::from) != w.season || p.episode.is_some() {
                return Err("not the whole season".into());
            }
        }
        WantedKind::Show | WantedKind::Album | WantedKind::Other => {}
    }
    Ok(())
}

/// Scores a release under `rules`: `Err(reason)` when it is not acceptable.
pub fn score(
    title: &str,
    size: Option<u64>,
    seeders: Option<u32>,
    rules: &QualityRules,
) -> std::result::Result<i64, String> {
    let q = parse_name(title).quality;
    let lower = title.to_lowercase();
    if let (Some(min), Some(r)) = (rules.min_resolution, q.resolution)
        && r < min
    {
        return Err(format!("{r}p is below {min}p"));
    }
    if rules.min_resolution.is_some() && q.resolution.is_none() {
        return Err("unknown resolution".into());
    }
    if let (Some(max), Some(r)) = (rules.max_resolution, q.resolution)
        && r > max
    {
        return Err(format!("{r}p is above {max}p"));
    }
    if !rules.sources.is_empty() && !q.source.as_ref().is_some_and(|s| rules.sources.contains(s)) {
        return Err(format!("source {}", q.source.clone().unwrap_or_else(|| "unknown".into())));
    }
    for w in &rules.required_words {
        if !lower.contains(&w.to_lowercase()) {
            return Err(format!("missing “{w}”"));
        }
    }
    for w in &rules.forbidden_words {
        if lower.contains(&w.to_lowercase()) {
            return Err(format!("contains “{w}”"));
        }
    }
    if let (Some(min), Some(s)) = (rules.min_size_mb, size)
        && s < min * 1_000_000
    {
        return Err("too small".into());
    }
    if let (Some(max), Some(s)) = (rules.max_size_mb, size)
        && s > max * 1_000_000
    {
        return Err("too large".into());
    }
    if let Some(min) = rules.min_seeders
        && seeders.unwrap_or(0) < min
    {
        return Err(format!("{} seeders", seeders.unwrap_or(0)));
    }
    let mut score = 0i64;
    if let Some(r) = q.resolution {
        score += match rules.preferred_resolution {
            Some(p) if p == r => 1000,
            Some(p) => 600 - (p as i64 - r as i64).abs() / 4,
            None => r as i64 / 4,
        };
    }
    score += match q.source.as_deref() {
        Some("remux") => 300,
        Some("bluray") => 250,
        Some("webdl") => 200,
        Some("webrip") => 150,
        Some("hdtv") => 80,
        Some("dvd") => 40,
        _ => 0,
    };
    if let Some(c) = &q.codec
        && let Some(i) = rules.codecs.iter().position(|x| x == c)
    {
        score += 100 - 20 * i as i64;
    }
    for w in &rules.preferred_words {
        if lower.contains(&w.to_lowercase()) {
            score += 50;
        }
    }
    if q.proper {
        score += 20;
    }
    // Healthy swarms download sooner (log-scaled, capped).
    score += (seeders.unwrap_or(0) as f64 + 1.0).log2().min(10.0) as i64 * 10;
    Ok(score)
}

/// The search text for a wanted entry.
pub fn query_for(w: &Wanted) -> String {
    if let Some(q) = w.query.as_ref().filter(|q| !q.trim().is_empty()) {
        return q.trim().to_string();
    }
    match w.kind {
        WantedKind::Movie => match w.year {
            Some(y) => format!("{} {y}", w.title),
            None => w.title.clone(),
        },
        WantedKind::Episode => format!("{} S{:02}E{:02}", w.title, w.season.unwrap_or(1), w.episode.unwrap_or(1)),
        WantedKind::Season => format!("{} S{:02}", w.title, w.season.unwrap_or(1)),
        _ => w.title.clone(),
    }
}

/// Judges search results for a wanted entry, best first.
pub fn judge(results: &[Candidate], w: Option<&Wanted>, rules: &QualityRules) -> Vec<Release> {
    let mut out: Vec<Release> = results
        .iter()
        .map(|r| {
            let q = parse_name(&r.title).quality;
            let verdict =
                w.map(|w| fits(&r.title, w)).unwrap_or(Ok(())).and_then(|()| score(&r.title, r.size, r.seeders, rules));
            Release {
                title: r.title.clone(),
                provider: r.provider.clone(),
                link: r.link.clone(),
                size: r.size,
                seeders: r.seeders,
                resolution: q.resolution,
                source: q.source,
                codec: q.codec,
                score: verdict.as_ref().ok().copied(),
                rejected: verdict.err(),
            }
        })
        .collect();
    out.sort_by_key(|r| (r.score.is_none(), std::cmp::Reverse(r.score.unwrap_or(0))));
    out
}

/// A search result as the judge needs it (decoupled from the Torznab client).
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub title: String,
    pub provider: String,
    pub link: Option<String>,
    pub size: Option<u64>,
    pub seeders: Option<u32>,
}

fn kind_str(k: WantedKind) -> &'static str {
    match k {
        WantedKind::Movie => "movie",
        WantedKind::Episode => "episode",
        WantedKind::Season => "season",
        WantedKind::Show => "show",
        WantedKind::Album => "album",
        WantedKind::Other => "other",
    }
}

fn kind_from(s: &str) -> WantedKind {
    match s {
        "movie" => WantedKind::Movie,
        "episode" => WantedKind::Episode,
        "season" => WantedKind::Season,
        "show" => WantedKind::Show,
        "album" => WantedKind::Album,
        _ => WantedKind::Other,
    }
}

const WANTED_COLS: &str = "id, library_id, kind, title, year, season, episode, query, profile_id, status, torrent, added_by, created_at, searched_at";

fn wanted_row(r: &Row) -> rusqlite::Result<Wanted> {
    Ok(Wanted {
        id: r.get(0)?,
        library_id: r.get(1)?,
        kind: kind_from(&r.get::<_, String>(2)?),
        title: r.get(3)?,
        year: r.get(4)?,
        season: r.get(5)?,
        episode: r.get(6)?,
        query: r.get(7)?,
        profile_id: r.get(8)?,
        status: r.get(9)?,
        torrent: r.get(10)?,
        added_by: r.get(11)?,
        created_at: r.get(12)?,
        searched_at: r.get(13)?,
    })
}

impl Library {
    pub async fn profiles(&self) -> Result<Vec<QualityProfile>> {
        Ok(self
            .db()
            .read(|c| {
                let mut st = c.prepare("SELECT id, name, rules FROM quality_profiles ORDER BY id")?;
                st.query_map([], |r| {
                    Ok(QualityProfile {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        rules: serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default(),
                    })
                })?
                .collect()
            })
            .await?)
    }

    pub async fn profile(&self, id: Option<i64>) -> Result<QualityRules> {
        let all = self.profiles().await?;
        Ok(id
            .and_then(|id| all.iter().find(|p| p.id == id))
            .or(all.first())
            .map(|p| p.rules.clone())
            .unwrap_or_default())
    }

    pub async fn save_profile(&self, id: Option<i64>, name: String, rules: QualityRules) -> Result<QualityProfile> {
        if name.trim().is_empty() {
            return Err(LibError::Invalid("profile name required".into()));
        }
        let json = serde_json::to_string(&rules).unwrap_or_else(|_| "{}".into());
        let (n2, r2) = (name.trim().to_string(), rules.clone());
        let id = self
            .db()
            .write(move |t| match id {
                Some(id) => {
                    let n = t.execute(
                        "UPDATE quality_profiles SET name = ?1, rules = ?2 WHERE id = ?3",
                        params![n2, json, id],
                    )?;
                    Ok((n > 0).then_some(id))
                }
                None => {
                    t.execute("INSERT INTO quality_profiles(name, rules) VALUES (?1, ?2)", params![n2, json])?;
                    Ok(Some(t.last_insert_rowid()))
                }
            })
            .await
            .map_err(|e| match e {
                corrent_db::DbError::Sqlite(rusqlite::Error::SqliteFailure(f, _))
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    LibError::Invalid("a profile with that name exists".into())
                }
                e => e.into(),
            })?
            .ok_or(LibError::NotFound)?;
        Ok(QualityProfile { id, name, rules: r2 })
    }

    pub async fn delete_profile(&self, id: i64) -> Result<()> {
        let n = self.db().write(move |t| t.execute("DELETE FROM quality_profiles WHERE id = ?1", [id])).await?;
        if n == 0 { Err(LibError::NotFound) } else { Ok(()) }
    }

    pub async fn wanted(&self, status: Option<String>) -> Result<Vec<Wanted>> {
        Ok(self
            .db()
            .read(move |c| {
                let mut st = c.prepare(&format!(
                    "SELECT {WANTED_COLS} FROM wanted WHERE ?1 IS NULL OR status = ?1 ORDER BY created_at DESC LIMIT 1000"
                ))?;
                st.query_map([status], wanted_row)?.collect()
            })
            .await?)
    }

    pub async fn wanted_one(&self, id: i64) -> Result<Wanted> {
        self.db()
            .read(move |c| {
                c.query_row(&format!("SELECT {WANTED_COLS} FROM wanted WHERE id = ?1"), [id], wanted_row).optional()
            })
            .await?
            .ok_or(LibError::NotFound)
    }

    pub async fn add_wanted(&self, w: NewWanted, by: &str) -> Result<Wanted> {
        if w.title.trim().is_empty() {
            return Err(LibError::Invalid("title required".into()));
        }
        let kind = w.kind.unwrap_or(if w.episode.is_some() {
            WantedKind::Episode
        } else if w.season.is_some() {
            WantedKind::Season
        } else {
            WantedKind::Movie
        });
        let (by, now) = (by.to_string(), now_unix());
        let id = self
            .db()
            .write(move |t| {
                t.execute(
                    "INSERT INTO wanted(library_id, kind, title, year, season, episode, query, profile_id, added_by, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![w.library_id, kind_str(kind), w.title.trim(), w.year, w.season, w.episode, w.query, w.profile_id, by, now],
                )?;
                Ok(t.last_insert_rowid())
            })
            .await?;
        self.wanted_one(id).await
    }

    pub async fn delete_wanted(&self, id: i64) -> Result<()> {
        let n = self.db().write(move |t| t.execute("DELETE FROM wanted WHERE id = ?1", [id])).await?;
        if n == 0 { Err(LibError::NotFound) } else { Ok(()) }
    }

    pub async fn set_wanted_status(&self, id: i64, status: &str, torrent: Option<String>) -> Result<()> {
        let status = status.to_string();
        self.db()
            .write(move |t| {
                t.execute(
                    "UPDATE wanted SET status = ?1, torrent = coalesce(?2, torrent) WHERE id = ?3",
                    params![status, torrent, id],
                )
            })
            .await?;
        Ok(())
    }

    pub async fn mark_searched(&self, id: i64) -> Result<()> {
        let now = now_unix();
        self.db()
            .write(move |t| t.execute("UPDATE wanted SET searched_at = ?1 WHERE id = ?2", params![now, id]))
            .await?;
        Ok(())
    }

    /// Wanted entries not searched within `every_secs`.
    pub async fn due_wanted(&self, every_secs: i64) -> Result<Vec<Wanted>> {
        let before = now_unix() - every_secs;
        Ok(self
            .db()
            .read(move |c| {
                let mut st = c.prepare(&format!(
                    "SELECT {WANTED_COLS} FROM wanted WHERE status = 'wanted' AND (searched_at IS NULL OR searched_at < ?1) \
                     ORDER BY searched_at IS NOT NULL, searched_at LIMIT 50"
                ))?;
                st.query_map([before], wanted_row)?.collect()
            })
            .await?)
    }

    /// A finished download completes the wanted entries that grabbed it.
    pub async fn complete_torrent(&self, hash: &str) -> Result<usize> {
        let h = hash.to_string();
        Ok(self
            .db()
            .write(move |t| {
                t.execute("UPDATE wanted SET status = 'done' WHERE torrent = ?1 AND status = 'grabbed'", [h])
            })
            .await?)
    }

    pub async fn record_grab(
        &self,
        wanted: Option<i64>,
        item: Option<i64>,
        torrent: &str,
        title: &str,
        score: i64,
    ) -> Result<()> {
        let (torrent, title, now) = (torrent.to_string(), title.to_string(), now_unix());
        self.db()
            .write(move |t| {
                t.execute(
                    "INSERT INTO grabs(wanted_id, item_id, torrent, title, score, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![wanted, item, torrent, title, score, now],
                )
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(kind: WantedKind, title: &str, year: Option<i32>, s: Option<i64>, e: Option<i64>) -> Wanted {
        Wanted {
            id: 1,
            library_id: None,
            kind,
            title: title.into(),
            year,
            season: s,
            episode: e,
            query: None,
            profile_id: None,
            status: "wanted".into(),
            torrent: None,
            added_by: None,
            created_at: 0,
            searched_at: None,
        }
    }

    #[test]
    fn fitting() {
        let m = w(WantedKind::Movie, "The Matrix", Some(1999), None, None);
        assert!(fits("The.Matrix.1999.1080p.BluRay.x264-GRP", &m).is_ok());
        assert!(fits("The.Matrix.Reloaded.2003.1080p", &m).is_err());
        assert!(fits("Matrix 1999 2160p", &m).is_ok(), "articles don't matter");
        let e = w(WantedKind::Episode, "Show Name", None, Some(1), Some(2));
        assert!(fits("Show.Name.S01E02.720p", &e).is_ok());
        assert!(fits("Show.Name.S01E03.720p", &e).is_err());
        let s = w(WantedKind::Season, "Show Name", None, Some(1), None);
        assert!(fits("Show.Name.S01.1080p.WEB-DL", &s).is_ok());
        assert_eq!(query_for(&e), "Show Name S01E02");
        assert_eq!(query_for(&m), "The Matrix 1999");
    }

    #[test]
    fn scoring() {
        let hd = QualityRules {
            min_resolution: Some(720),
            max_resolution: Some(1080),
            preferred_resolution: Some(1080),
            min_seeders: Some(2),
            forbidden_words: vec!["cam".into()],
            codecs: vec!["hevc".into(), "h264".into()],
            ..Default::default()
        };
        let a = score("Movie.2020.1080p.BluRay.x265-G", Some(4_000_000_000), Some(50), &hd).unwrap();
        let b = score("Movie.2020.720p.WEBRip.x264-G", Some(1_000_000_000), Some(50), &hd).unwrap();
        assert!(a > b);
        assert!(score("Movie.2020.2160p.WEB-DL", None, Some(50), &hd).is_err());
        assert!(score("Movie.2020.1080p.CAM", None, Some(50), &hd).is_err());
        assert!(score("Movie.2020.1080p.BluRay", None, Some(0), &hd).is_err());
        assert!(score("Movie.2020.BluRay", None, Some(10), &hd).is_err(), "unknown resolution with a minimum");
    }
}
