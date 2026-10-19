//! Corrent's core: the torrent engine actor, settings and paths, the per-torrent store, RSS,
//! Torznab search, import from other clients, backups, migrations, crash reports and the
//! in-memory log. No UI code: the GUI, the server and the headless daemon all build on this.

#[macro_use]
extern crate corrent_i18n;

pub mod backup;
pub mod config;
pub mod crash;
pub mod engine;
pub mod import;
pub mod logbuf;
pub mod migrate;
pub mod rss;
pub mod search;
pub mod store;
pub mod updates;
pub mod util;
