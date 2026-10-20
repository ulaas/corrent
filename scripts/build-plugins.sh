#!/bin/sh
# Builds the first-party plugins (WebAssembly components) and copies them next to their
# manifests into crates/corrent-plugins/builtin/ (committed, so the main workspace builds
# without the wasm toolchain). Needs: rustup target add wasm32-wasip2
set -eu
cd "$(dirname "$0")/../plugins"
CARGO_TARGET_DIR=../target/plugins cargo build --release --target wasm32-wasip2
out=../crates/corrent-plugins/builtin
mkdir -p "$out"
for p in example example-subtitles example-parser example-task example-events tmdb musicbrainz fanart opensubtitles tvdb omdb; do
  cp "../target/plugins/wasm32-wasip2/release/corrent_plugin_$(echo $p | tr - _).wasm" "$out/$p.wasm"
  cp "$p/plugin.toml" "$out/$p.toml"
  echo "$p: $(wc -c < "$out/$p.wasm") bytes"
done
