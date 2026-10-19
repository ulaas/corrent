//! A test name parser: understands "Title {Year} #Episode" names, leaves the rest alone.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "name-parser" });

use exports::corrent::plugin::parser::{Guest, ParsedName};

struct Example;

impl Guest for Example {
    fn parse(name: String) -> Option<ParsedName> {
        let (title, rest) = name.split_once(" {")?;
        let (year, rest) = rest.split_once('}')?;
        let episode = rest.trim().strip_prefix('#').and_then(|e| e.parse().ok());
        Some(ParsedName { title: title.trim().to_string(), year: year.parse().ok(), season: episode.map(|_| 1), episode })
    }
}

export!(Example);
