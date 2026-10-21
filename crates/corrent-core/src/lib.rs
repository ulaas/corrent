//! Corrent's core: the torrent engine actor, settings and paths, the per-torrent store, RSS,
//! Torznab search, import from other clients, backups, migrations, crash reports and the
//! in-memory log. No UI code: the GUI, the server and the headless daemon all build on this.

// STABILITY.md F17: shipped code doesn't index (tests may; clippy.toml).
#![deny(clippy::indexing_slicing)]

#[macro_use]
extern crate corrent_i18n;

pub mod backup;
pub mod config;
pub mod crash;
pub mod diagnostics;
pub mod engine;
pub mod faults;
#[cfg(any(test, feature = "fuzz"))]
pub mod fuzz;
pub mod health;
pub mod import;
pub mod lock;
pub mod logbuf;
pub mod logfile;
pub mod migrate;
pub mod retention;
pub mod rss;
pub mod search;
pub mod store;
pub mod symbols;
pub mod updates;
pub mod util;
pub mod watchdog;
pub mod writer;

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod prop_tests;
