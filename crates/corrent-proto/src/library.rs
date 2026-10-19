//! Media library wire types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryKind {
    Movies,
    Shows,
    Music,
    Audiobooks,
    /// Home videos and anything else: one item per file, organised by folder.
    Videos,
    Photos,
}

impl LibraryKind {
    pub const ALL: [LibraryKind; 6] = [
        LibraryKind::Movies,
        LibraryKind::Shows,
        LibraryKind::Music,
        LibraryKind::Audiobooks,
        LibraryKind::Videos,
        LibraryKind::Photos,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LibraryKind::Movies => "movies",
            LibraryKind::Shows => "shows",
            LibraryKind::Music => "music",
            LibraryKind::Audiobooks => "audiobooks",
            LibraryKind::Videos => "videos",
            LibraryKind::Photos => "photos",
        }
    }

    pub fn parse(s: &str) -> Option<LibraryKind> {
        LibraryKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            LibraryKind::Movies => t!("Movies"),
            LibraryKind::Shows => t!("TV shows"),
            LibraryKind::Music => t!("Music"),
            LibraryKind::Audiobooks => t!("Audiobooks"),
            LibraryKind::Videos => t!("Videos"),
            LibraryKind::Photos => t!("Photos"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Movie,
    Show,
    Season,
    Episode,
    Artist,
    Album,
    Track,
    Book,
    Video,
    Photo,
    Folder,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Movie => "movie",
            ItemKind::Show => "show",
            ItemKind::Season => "season",
            ItemKind::Episode => "episode",
            ItemKind::Artist => "artist",
            ItemKind::Album => "album",
            ItemKind::Track => "track",
            ItemKind::Book => "book",
            ItemKind::Video => "video",
            ItemKind::Photo => "photo",
            ItemKind::Folder => "folder",
        }
    }

    pub fn parse(s: &str) -> Option<ItemKind> {
        use ItemKind::*;
        [Movie, Show, Season, Episode, Artist, Album, Track, Book, Video, Photo, Folder]
            .into_iter()
            .find(|k| k.as_str() == s)
    }

    /// Has its own media file (as opposed to a container like a show or an album).
    pub fn playable(self) -> bool {
        matches!(self, ItemKind::Movie | ItemKind::Episode | ItemKind::Track | ItemKind::Book | ItemKind::Video)
    }
}

/// What to do with torrents of a category when they complete (Track T1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Scan the download folder; the files stay where they are and keep seeding.
    #[default]
    InPlace,
    Hardlink,
    Copy,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    /// Torrent categories whose completed downloads feed this library.
    pub import_categories: Vec<String>,
    pub import_mode: ImportMode,
    /// Watch the folders for changes.
    pub watch: bool,
    /// Fetch metadata and artwork from providers (plugins).
    pub fetch_metadata: bool,
    /// Preferred metadata language (e.g. "tr").
    pub language: String,
    /// Percentage watched after which an item counts as played.
    pub played_threshold: u8,
}

impl LibrarySettings {
    pub fn defaults() -> LibrarySettings {
        LibrarySettings { watch: true, fetch_metadata: true, played_threshold: 90, ..Default::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub name: String,
    pub kind: LibraryKind,
    pub folders: Vec<String>,
    pub settings: LibrarySettings,
    pub item_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<i64>,
    /// Scan in progress (0–100), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanning: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewLibrary {
    pub name: String,
    pub kind: LibraryKind,
    pub folders: Vec<String>,
    #[serde(default = "LibrarySettings::defaults")]
    pub settings: LibrarySettings,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryUpdate {
    pub name: Option<String>,
    pub folders: Option<Vec<String>>,
    pub settings: Option<LibrarySettings>,
}

/// An image attached to an item: the hash is the cache key (`/api/v1/images/{sha}?w=400`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub kind: String,
    pub sha: String,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blurhash: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserState {
    pub position_ms: i64,
    pub played: bool,
    pub play_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played_at: Option<i64>,
    pub favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    /// Containers: how many playable children are not played yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unplayed: Option<i64>,
}

/// An item in lists and grids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    pub library_id: i64,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ms: Option<i64>,
    /// Parent's and grandparent's titles (episode: show + season; track: album artist + album).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
    #[serde(default)]
    pub state: UserState,
    pub added_at: i64,
    /// Children count for containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_count: Option<i64>,
    /// Some files still downloading (torrent not complete).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stream {
    pub index: i64,
    pub kind: String,
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub bitrate: Option<i64>,
    pub channels: Option<i64>,
    pub sample_rate: Option<i64>,
    /// External subtitle file next to the media.
    pub external: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaFile {
    pub id: i64,
    pub path: String,
    pub size: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    pub role: String,
    pub streams: Vec<Stream>,
    pub chapters: Vec<Chapter>,
    /// The torrent this file comes from, when it was downloaded by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Credit {
    pub person_id: i64,
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemDetail {
    #[serde(flatten)]
    pub item: Item,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub premiered: Option<String>,
    pub genres: Vec<String>,
    pub credits: Vec<Credit>,
    pub files: Vec<MediaFile>,
    pub provider_ids: BTreeMap<String, String>,
    pub locked_fields: Vec<String>,
    /// Free-form facts (album artist, ReplayGain, …).
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One page of a list; `next` continues it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    #[default]
    Title,
    Added,
    Year,
    Played,
    Index,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemQuery {
    pub library_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub kind: Option<ItemKind>,
    pub genre: Option<String>,
    pub person_id: Option<i64>,
    pub favorite: Option<bool>,
    pub played: Option<bool>,
    pub sort: SortBy,
    pub desc: bool,
    pub offset: i64,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeRow {
    /// "continue", "next_up", "recent:<library id>", "favorites".
    pub id: String,
    pub title: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub items: Vec<Item>,
    pub people: Vec<Credit>,
}

/// `PUT /items/{id}/state`: fields present are changed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateUpdate {
    pub position_ms: Option<i64>,
    pub played: Option<bool>,
    pub favorite: Option<bool>,
    pub rating: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanStatus {
    pub library_id: i64,
    pub running: bool,
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
