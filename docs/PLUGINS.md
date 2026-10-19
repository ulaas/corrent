# Writing Corrent plugins

Plugins add metadata providers, torrent indexers, subtitle providers, notifiers, file-name
parsers and scheduled tasks to a Corrent server. Most are **WebAssembly components** (sandboxed:
no files, no sockets; HTTP only to the hosts the manifest declares); heavy or native ones can be
**process plugins** (a program speaking JSON-RPC over stdio, not sandboxed, installed by an
administrator).

## The interface

`crates/corrent-plugins/wit/corrent.wit` (package `corrent:plugin@1.0.0`) is the contract. Every
plugin imports `host`:

| Function | What |
|---|---|
| `http(req)` | HTTP(S) to a host listed in `permissions.hosts` (public addresses only unless the admin allows the local network; redirects are re-checked; rate-limited) |
| `log(level, message)` | the server log |
| `setting(key)` | a value from the manifest's `settings` as the admin entered it (secrets included; never shown back) |
| `kv-get` / `kv-set` | the plugin's own key-value store (10,000 keys) |
| `library.search(query, limit)`, `library.item(id)` | read the libraries (`event-handler` world; needs `library = true` under `[permissions]`; never writes) |

and exports one interface, chosen by its world:

| World | Exports | Used for |
|---|---|---|
| `metadata-provider` | `kinds`, `search`, `details`, `episodes` | identifying library items, artwork, cast, episode lists |
| `indexer-plugin` | `search` | finding releases (wanted list, "find releases") |
| `notifier-plugin` | `notify` | notification destinations |
| `subtitle-provider` | `search`, `download` | subtitles for an item (file hash, ids, title) |
| `name-parser` | `parse` | reading file names before the built-in parser (after `.nfo` files) |
| `scheduled-task` | `run` | a task in Settings → Tasks (`task_every_min`) |
| `event-handler` | `on-event` | everything notifications hear (download done, torrent added, release grabbed, device paired, task failed) |

Each call runs in a fresh instance with a 64 MiB memory limit and a 30 s deadline; three crashes
or timeouts in a row disable the plugin.

## A 20-line example

A name parser for a home-video folder whose files are called `Title {Year} #Episode`:

```rust
// plugins/my-parser/src/lib.rs  (Cargo.toml: crate-type = ["cdylib"], wit-bindgen = "0.57")
wit_bindgen::generate!({ path: "../../crates/corrent-plugins/wit", world: "name-parser" });

use exports::corrent::plugin::parser::{Guest, ParsedName};

struct MyParser;

impl Guest for MyParser {
    fn parse(name: String) -> Option<ParsedName> {
        let (title, rest) = name.split_once(" {")?;
        let (year, rest) = rest.split_once('}')?;
        let episode = rest.trim().strip_prefix('#').and_then(|e| e.parse().ok());
        Some(ParsedName { title: title.trim().into(), year: year.parse().ok(), season: episode.map(|_| 1), episode })
    }
}

export!(MyParser);
```

```toml
# plugins/my-parser/plugin.toml
id = "my-parser"
name = "My parser"
version = "1.0.0"
host_api = "1"
kinds = ["parser"]
```

Build with `rustup target add wasm32-wasip2` once, then
`cargo build --release --target wasm32-wasip2`, and install `plugin.toml` with
`target/wasm32-wasip2/release/my_parser.wasm` (renamed `plugin.wasm`) in the web client
(Settings → Plugins → Install a plugin…). The first-party plugins in `plugins/` (TMDB,
MusicBrainz, TheTVDB, fanart.tv, OpenSubtitles and the `example-*` test plugins) are complete
examples of every kind; `scripts/build-plugins.sh` builds them.

## Manifest

```toml
id = "lower-case-id"          # letters, digits, dashes
name = "Shown name"
version = "1.0.0"
description = "One sentence."
author = "You"
host_api = "1"
kinds = ["metadata"]          # metadata | indexer | notifier | subtitles | parser | task | events
supplements = false           # metadata: only adds artwork/ids to another provider's match
task_every_min = 60           # task: default interval
runtime = "wasm"              # or "process"
command = ["my-plugin"]       # process: the program in the plugin's folder, and arguments

[permissions]
hosts = ["api.example.org", "*.cdn.example.org"]
allow_local_network = false
rate_per_sec = 4
library = false               # events: may read the libraries

[[settings]]
key = "api_key"
label = "API key"
help = "Where to get one."
secret = true
required = true
```

## Process plugins

Put the folder (with `plugin.toml` and the program) into `<server data>/plugins/<id>/` and
restart the server. The program reads one JSON-RPC 2.0 request per line on stdin and answers on
stdout (log to stderr). Methods: `configure` (settings), `metadata.kinds|search|details|episodes`,
`indexer.search`, `notifier.notify`, `subtitles.search|download`, `parser.parse`, `task.run`, `events.on_event` —
parameters and results have the WIT records' shapes, field names in snake_case. The process is
started on first use, restarted after it exits or times out, and told about new settings by a
restart. See `crates/corrent-plugins/src/process.rs` and its Python test plugin.

## Web plugins

`runtime = "web"` with `kinds = ["web"]`: a folder in `<server data>/plugins/<id>/` holding
`plugin.toml` and `web/` (the module `web_entry`, default `index.js`, plus anything it loads).
The web client imports the module for everyone signed in; it runs in their browser with their
rights, so only an administrator installs one (by copying the folder, then restarting). It talks
to Corrent through `window.corrent` (version 1):

```js
corrent.addItemAction({ id: 'tell', label: 'Tell a friend', run: (item) => corrent.toast(item.title) });
corrent.addSettingsPage({ id: 'stats', title: 'My stats', render(el) { el.textContent = 'Hello'; } });
const libs = await corrent.api.get('/libraries');   // /api/v1 as this person; post/put/del too
```

Item actions appear on item pages; pages under Settings → Extensions. Files are served at
`/api/v1/plugins/<id>/web/<path>` (signed-in people only, nothing outside `web/`).

## Catalogs and signatures

A server can install from a catalog: `plugin_catalog = "https://…/index.json"` and
`plugin_keys = ["<hex public key>"]` in `server.toml`. The index is
`{"plugins": [{"id", "name", "version", "description", "kinds", "url", "sha256", "signature"}]}`
where `url` is a zip with `plugin.toml` and `plugin.wasm`, `sha256` its hash and `signature` a
base64 Ed25519 signature of the hash's 32 bytes. `correntd plugin keygen` makes a key pair;
`correntd plugin sign <zip> <secret-key-file>` prints the entry's `sha256` and `signature`.
Archives that don't match or aren't signed by a listed key are refused.
