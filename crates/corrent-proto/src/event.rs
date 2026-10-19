//! Realtime messages on the WebSocket `/api/v1/events`.

use serde::{Deserialize, Serialize};

use crate::torrent::{InfoHash, Labels, SessionStats, Torrent, TorrentDetail, TorrentsDelta};

/// Topics a client can subscribe to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "topic", content = "arg")]
pub enum Topic {
    Torrents,
    /// Files, peers and pieces of one torrent.
    Torrent(InfoHash),
    Session,
    Log,
    /// Library changes and scan progress.
    Library,
}

/// Client → server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t")]
pub enum ClientMessage {
    Subscribe { topics: Vec<Topic> },
    Unsubscribe { topics: Vec<Topic> },
    Ping { id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

/// Server → client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t")]
pub enum Event {
    Hello {
        server_id: String,
        user: String,
    },
    Pong {
        id: u64,
    },
    /// The whole list (after subscribing or when the client fell behind).
    TorrentsFull {
        torrents: Vec<Torrent>,
        labels: Labels,
    },
    TorrentsDelta {
        delta: TorrentsDelta,
    },
    Labels {
        labels: Labels,
    },
    Session {
        stats: SessionStats,
    },
    TorrentDetail {
        detail: TorrentDetail,
    },
    /// A notification for people (already translated into the server's language; clients may
    /// prefer `code` when they know it).
    Notice {
        severity: Severity,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    Added {
        hash: InfoHash,
        name: String,
    },
    Completed {
        hash: InfoHash,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<String>,
    },
    Removed {
        hash: InfoHash,
        name: String,
        undoable: bool,
    },
    Log {
        level: String,
        target: String,
        message: String,
        time: i64,
    },
    /// A library's items changed (refetch what is shown).
    LibraryChanged {
        library_id: i64,
    },
    ScanStatus {
        status: crate::library::ScanStatus,
    },
    /// The server is going away (restart or shutdown).
    Bye,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_json() {
        let e = Event::Added { hash: InfoHash([1; 20]), name: "x".into() };
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["t"], "added");
        let m: ClientMessage =
            serde_json::from_str(r#"{"t":"subscribe","topics":[{"topic":"torrents"},{"topic":"torrent","arg":"0101010101010101010101010101010101010101"}]}"#).unwrap();
        assert_eq!(m, ClientMessage::Subscribe { topics: vec![Topic::Torrents, Topic::Torrent(InfoHash([1; 20]))] });
    }
}
