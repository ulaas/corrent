#!/bin/sh
# Builds the first-party plugins (WebAssembly components) and copies them next to their
# manifests into crates/corrent-plugins/builtin/ (committed, so the main workspace builds
# without the wasm toolchain). Needs: rustup target add wasm32-wasip2
set -eu
cd "$(dirname "$0")/../plugins"
CARGO_TARGET_DIR=../target/plugins cargo build --release --target wasm32-wasip2
out=../crates/corrent-plugins/builtin
mkdir -p "$out"
for p in example tmdb musicbrainz; do
  cp "../target/plugins/wasm32-wasip2/release/corrent_plugin_$p.wasm" "$out/$p.wasm"
  cp "$p/plugin.toml" "$out/$p.toml"
  echo "$p: $(wc -c < "$out/$p.wasm") bytes"
done
