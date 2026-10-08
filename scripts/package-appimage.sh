#!/usr/bin/env bash
# Build an AppImage on Linux (x86_64). Needs linuxdeploy in PATH or LINUXDEPLOY set:
#   https://github.com/linuxdeploy/linuxdeploy/releases
# Usage: scripts/package-appimage.sh   → dist/Corrent-<version>-x86_64.AppImage
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
cargo build --release --locked
appdir=target/appimage/AppDir
rm -rf "$appdir"
install -Dm755 target/release/corrent "$appdir/usr/bin/corrent"
install -Dm644 packaging/linux/corrent.desktop "$appdir/usr/share/applications/corrent.desktop"
install -Dm644 packaging/linux/io.github.ulaas.Corrent.metainfo.xml "$appdir/usr/share/metainfo/corrent.appdata.xml"
for s in 32 64 128 256 512; do
  install -Dm644 "assets/icon/corrent-$s.png" "$appdir/usr/share/icons/hicolor/${s}x${s}/apps/corrent.png"
done
mkdir -p dist
OUTPUT="dist/Corrent-$version-x86_64.AppImage" "${LINUXDEPLOY:-linuxdeploy}" \
  --appdir "$appdir" \
  --desktop-file "$appdir/usr/share/applications/corrent.desktop" \
  --icon-file assets/icon/corrent-256.png \
  --output appimage
echo "built dist/Corrent-$version-x86_64.AppImage"
