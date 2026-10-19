//! Corrent API v1 wire types, shared by the server and every Rust client. Plain serde data and
//! pure functions (deltas) only: no async runtime, no UI.

#[macro_use]
extern crate corrent_i18n;

pub mod auth;
pub mod error;
pub mod event;
pub mod library;
pub mod server;
pub mod torrent;

pub use error::{ApiError, ErrorCode};
pub use event::Event;
pub use torrent::{InfoHash, Torrent, TorrentPatch, TorrentState, TorrentsDelta};

/// Major version of the HTTP API (`/api/v1`).
pub const API_VERSION: u32 = 1;
