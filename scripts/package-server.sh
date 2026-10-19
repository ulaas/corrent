#!/usr/bin/env bash
# Build the Corrent Server tarball for this OS (Linux or macOS):
#   dist/Corrent-Server-<version>-<os>-<arch>.tar.gz
# with correntd, the libmpv worker corrent-mpvw, the service files and the server docs.
# libmpv itself comes from the system (Debian/Ubuntu: libmpv2, Fedora: mpv-libs, macOS: brew install mpv).
# Usage: scripts/package-server.sh
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
cargo build --release --locked -p correntd -p corrent-mpv --bin correntd --bin corrent-mpvw
name="Corrent-Server-$version-$os-$arch"
stage="target/server/$name"
rm -rf "$stage"
install -d "$stage"
install -m755 target/release/correntd target/release/corrent-mpvw "$stage/"
install -m644 docs/SERVER.md "$stage/README.md"
case "$os" in
  linux) cp packaging/server/linux/* "$stage/" ;;
  darwin) cp packaging/server/macos/* "$stage/" ;;
esac
mkdir -p dist
tar -C target/server -czf "dist/$name.tar.gz" "$name"
echo "built dist/$name.tar.gz"
