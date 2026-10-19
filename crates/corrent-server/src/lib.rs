#![recursion_limit = "256"]

//! Corrent's server: the HTTP APIs over the engine (and, as the media server grows, the library,
//! playback and remote access). Hosted in-process by the GUI or by the headless `correntd`.

#[allow(unused_imports)]
use corrent_core::{config, engine, rss, store, util};

pub mod webapi;
