//! A test event handler: remembers the last event and how many library items match its title.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "event-handler" });

use corrent::plugin::{host, library};
use exports::corrent::plugin::events::{Event, Guest};

struct Example;

impl Guest for Example {
    fn on_event(e: Event) {
        let found = match library::search(&e.title, 10) {
            Ok(items) => items.iter().map(|i| format!("{}:{}", i.kind, i.title)).collect::<Vec<_>>().join(","),
            Err(err) => format!("error {err}"),
        };
        let _ = host::kv_set("last", &format!("{} | {} | {found}", e.kind, e.title));
    }
}

export!(Example);
