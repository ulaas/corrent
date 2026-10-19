//! A test task: counts its runs in the plugin's key-value store.

wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "scheduled-task" });

use corrent::plugin::host;
use exports::corrent::plugin::task::Guest;

struct Example;

impl Guest for Example {
    fn run() -> Result<String, String> {
        let n: u32 = host::kv_get("runs").and_then(|v| v.parse().ok()).unwrap_or(0) + 1;
        host::kv_set("runs", &n.to_string())?;
        Ok(format!("run {n}"))
    }
}

export!(Example);
