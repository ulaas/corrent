#!/usr/bin/env bash
# Build Corrent.app and a .dmg on macOS. Needs `cargo install cargo-bundle`.
# Signing/notarization (optional) uses the owner's Developer ID:
#   CODESIGN_IDENTITY="Developer ID Application: …" NOTARY_PROFILE=<notarytool keychain profile>
# Usage: scripts/package-macos.sh   → dist/Corrent-<version>-macos.dmg
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
cargo bundle --release
app=$(ls -d target/release/bundle/osx/*.app | head -1)
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
  codesign --force --deep --options runtime --timestamp --sign "$CODESIGN_IDENTITY" "$app"
fi
mkdir -p dist
dmg="dist/Corrent-$version-macos.dmg"
rm -f "$dmg"
staging=$(mktemp -d)
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -volname Corrent -srcfolder "$staging" -ov -format UDZO "$dmg"
rm -rf "$staging"
if [ -n "${CODESIGN_IDENTITY:-}" ] && [ -n "${NOTARY_PROFILE:-}" ]; then
  codesign --sign "$CODESIGN_IDENTITY" --timestamp "$dmg"
  xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$dmg"
fi
echo "built $dmg"
