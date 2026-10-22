#!/usr/bin/env bash
# Build Corrent.app and a .dmg on macOS. Needs `cargo install cargo-bundle`.
# Signing/notarization (optional) uses the owner's Developer ID:
#   CODESIGN_IDENTITY="Developer ID Application: …" (certificate in the keychain)
#   and either NOTARY_PROFILE=<notarytool keychain profile> (local) or
#   NOTARY_KEY_PATH=<AuthKey_XXXX.p8> NOTARY_KEY_ID=… NOTARY_ISSUER=… (App Store Connect API key, CI)
# Usage: scripts/package-macos.sh   → dist/Corrent-<version>-macos.dmg
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
# Universal (Apple silicon + Intel) app.
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
cargo bundle --release --target aarch64-apple-darwin
cargo build --release --locked --target x86_64-apple-darwin
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p corrent-mpv --bin corrent-mpvw --target $t
done
app=$(ls -d target/aarch64-apple-darwin/release/bundle/osx/*.app | head -1)
lipo -create -output "$app/Contents/MacOS/corrent" \
  target/aarch64-apple-darwin/release/corrent target/x86_64-apple-darwin/release/corrent
# The libmpv worker for the media library (libmpv comes from Homebrew: brew install mpv).
lipo -create -output "$app/Contents/MacOS/corrent-mpvw" \
  target/aarch64-apple-darwin/release/corrent-mpvw target/x86_64-apple-darwin/release/corrent-mpvw
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
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
  codesign --sign "$CODESIGN_IDENTITY" --timestamp "$dmg"
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$dmg"
  elif [ -n "${NOTARY_KEY_PATH:-}" ]; then
    xcrun notarytool submit "$dmg" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER" --wait
    xcrun stapler staple "$dmg"
  fi
fi
echo "built $dmg"
