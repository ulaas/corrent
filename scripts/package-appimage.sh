#!/usr/bin/env bash
# Build an AppImage on Linux (x86_64). Needs linuxdeploy in PATH or LINUXDEPLOY set:
#   https://github.com/linuxdeploy/linuxdeploy/releases
# Usage: scripts/package-appimage.sh   → dist/Corrent-<version>-x86_64.AppImage
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
# The app and the libmpv worker it runs for probing and transcoding (media library).
cargo build --release --locked -p corrent -p corrent-mpv --bin corrent --bin corrent-mpvw
appdir=target/appimage/AppDir
rm -rf "$appdir"
install -Dm755 target/release/corrent "$appdir/usr/bin/corrent"
install -Dm755 target/release/corrent-mpvw "$appdir/usr/bin/corrent-mpvw"
install -Dm644 packaging/linux/corrent.desktop "$appdir/usr/share/applications/corrent.desktop"
install -d "$appdir/usr/share/metainfo"
# Inside the AppImage the desktop file is corrent.desktop: point the AppStream data at it.
sed -e 's|<id>io.github.ulaas.Corrent</id>|<id>corrent</id>|' \
    -e 's|io.github.ulaas.Corrent.desktop|corrent.desktop|' \
    packaging/linux/io.github.ulaas.Corrent.metainfo.xml > "$appdir/usr/share/metainfo/corrent.appdata.xml"
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
