#!/usr/bin/env bash
# Build the macOS packages on an Apple silicon Mac (arm64 only):
#   dist/Corrent-<version>-macos.dmg              Corrent.app (drag to Applications)
#   dist/Corrent-Server-<version>-macos-arm64.pkg correntd + a per-user launchd agent
# Both carry libmpv and every library it needs (from Homebrew at build time), with install names
# rewritten to stay inside the package, so they run on a Mac without Homebrew.
#
# Build dependencies: Xcode command line tools, rustup, and `brew install mpv dylibbundler`.
#
# Signing (all optional; without an identity everything is signed ad hoc):
#   MACOS_SIGN_IDENTITY="Developer ID Application: …"   (CODESIGN_IDENTITY is read too)
#   MACOS_INSTALLER_IDENTITY="Developer ID Installer: …" signs the pkg
#   notarization: MACOS_NOTARY_PROFILE=<notarytool keychain profile> (NOTARY_PROFILE is read too),
#   or NOTARY_KEY_PATH=<AuthKey_XXXX.p8> NOTARY_KEY_ID=… NOTARY_ISSUER=… (App Store Connect API key, CI)
# Options: SKIP_PKG=1 builds only the dmg.
#
# Licensing: Homebrew's mpv and FFmpeg are GPL builds, so these packages are distributed under the
# GPL's terms for those parts. Each bundled library's license files go into the package
# (Contents/Resources/licenses, /usr/local/share/corrent-server/licenses) with Homebrew's version.
# Usage: scripts/package-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

[ "$(uname -s)" = Darwin ] || { echo "run this on macOS" >&2; exit 1; }
target=aarch64-apple-darwin
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
bundle_id=$(sed -n '/^\[package.metadata.bundle\]/,/^\[/s/^identifier *= *"\(.*\)"/\1/p' Cargo.toml)
: "${bundle_id:=io.github.ulaas.Corrent}"
server_id=io.github.ulaas.correntd
# The Rust code builds for macOS 11; the bundled Homebrew libraries may need a newer one (below).
export MACOSX_DEPLOYMENT_TARGET=11.0
sign_id="${MACOS_SIGN_IDENTITY:-${CODESIGN_IDENTITY:--}}"
notary_profile="${MACOS_NOTARY_PROFILE:-${NOTARY_PROFILE:-}}"
entitlements=packaging/macos/Corrent.entitlements
work=target/macos-package
dist=dist

command -v brew >/dev/null || { echo "Homebrew is needed for libmpv: https://brew.sh" >&2; exit 1; }
mpv_prefix=$(brew --prefix mpv 2>/dev/null || true)
libmpv="$mpv_prefix/lib/libmpv.2.dylib"
[ -f "$libmpv" ] || { echo "libmpv not found; run: brew install mpv" >&2; exit 1; }
command -v dylibbundler >/dev/null || { echo "run: brew install dylibbundler" >&2; exit 1; }

# ---------------------------------------------------------------------------------------- build
rustup target add "$target" >/dev/null
cargo build --release --locked --target "$target" \
  -p corrent -p corrent-mpv -p correntd --bin corrent --bin corrent-mpvw --bin correntd
bin="target/$target/release"

rm -rf "$work"
mkdir -p "$work" "$dist"

# Copies libmpv into $1 with every non-system library it pulls in, all referring to each other by
# @loader_path (so the folder works wherever it ends up), and signs nothing yet.
bundle_libmpv() {
  local dir=$1
  mkdir -p "$dir"
  cp -L "$libmpv" "$dir/libmpv.2.dylib"
  chmod u+w "$dir/libmpv.2.dylib"
  install_name_tool -id "@loader_path/libmpv.2.dylib" "$dir/libmpv.2.dylib" 2> >(grep -v 'invalidate the code signature' >&2)
  # -cd: create the folder, -of: overwrite, -b: copy, -x: fix this file, -p: the new prefix.
  # The search paths resolve @rpath references some Homebrew libraries use.
  dylibbundler -cd -of -b -ns -x "$dir/libmpv.2.dylib" -d "$dir" -p "@loader_path/" \
    -s /opt/homebrew/lib -s "$mpv_prefix/lib" >/dev/null 2> >(grep -v 'invalidate the code signature' >&2)
  # dylibbundler can add `@loader_path/` as an rpath more than once, and dyld refuses a library
  # with a duplicate LC_RPATH. Nothing refers to @rpath any more, so dropping them all is fine.
  local f
  for f in "$dir"/*.dylib; do
    while otool -l "$f" | grep -A2 LC_RPATH | grep -q ' path @loader_path/ '; do
      install_name_tool -delete_rpath @loader_path/ "$f" 2> >(grep -v 'invalidate the code signature' >&2)
    done
  done
  # Anything still pointing outside the folder (other than the OS) is a packaging bug.
  local bad
  bad=$(for f in "$dir"/*.dylib; do otool -L "$f" | tail -n +2 | awk '{print $1}'; done |
    grep -v -E '^(/usr/lib/|/System/Library/|@loader_path/)' | sort -u || true)
  if [ -n "$bad" ]; then
    echo "libraries outside the package are still referenced:" >&2
    echo "$bad" >&2
    exit 1
  fi
}

# The worker $1 must load the bundled libmpv from folder $2 (on the build Mac Homebrew's copy
# would otherwise hide a library dyld refuses).
check_libmpv() {
  local got
  got=$("$1" version) || { echo "$1 can't load libmpv" >&2; exit 1; }
  case "$got" in
    *"\"path\":\"$2/libmpv.2.dylib\""*) ;;
    *) echo "$1 loaded the wrong libmpv: $got (expected $2)" >&2; exit 1 ;;
  esac
}

# Copies the license files of the Homebrew formulae the libraries in $1 come from into $2.
copy_licenses() {
  local dir=$1 out=$2 lib name opt formula
  mkdir -p "$out"
  for lib in "$dir"/*.dylib; do
    name=$(basename "$lib")
    opt=$(ls -d /opt/homebrew/opt/*/lib/"$name" 2>/dev/null | head -1 || true)
    [ -n "$opt" ] || continue
    formula=$(basename "$(dirname "$(dirname "$opt")")")
    [ -d "$out/$formula" ] && continue
    mkdir -p "$out/$formula"
    find -L "/opt/homebrew/opt/$formula/" -maxdepth 1 -type f \
      \( -iname 'LICENSE*' -o -iname 'COPYING*' -o -iname 'Copyright*' -o -iname 'NOTICE*' \) \
      -exec cp {} "$out/$formula/" \;
    brew list --versions "$formula" > "$out/$formula/VERSION" 2>/dev/null || true
  done
}

# The newest minimum macOS among the given Mach-O files ("11.0" when they all build for older).
min_macos() {
  local v best=11.0
  for f in "$@"; do
    v=$(otool -l "$f" | awk '/LC_BUILD_VERSION/{b=1} b&&/minos/{print $2; exit}')
    [ -n "$v" ] || continue
    if [ "$(printf '%s\n%s\n' "$best" "$v" | sort -V | tail -1)" = "$v" ]; then best=$v; fi
  done
  echo "$best"
}

# Signs a file (dylib or executable) with the hardened runtime.
sign() {
  local ts=--timestamp
  [ "$sign_id" = "-" ] && ts=--timestamp=none
  codesign --force --options runtime "$ts" --sign "$sign_id" "$@" 2> >(grep -v 'replacing existing signature' >&2)
}

notarize() {
  local file=$1
  [ "$sign_id" != "-" ] || return 0
  if [ -n "$notary_profile" ]; then
    xcrun notarytool submit "$file" --keychain-profile "$notary_profile" --wait
    xcrun stapler staple "$file"
  elif [ -n "${NOTARY_KEY_PATH:-}" ]; then
    xcrun notarytool submit "$file" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" \
      --issuer "$NOTARY_ISSUER" --wait
    xcrun stapler staple "$file"
  fi
}

# ------------------------------------------------------------------------------------ the icon
# Rendered from assets/icon/corrent.svg on Apple's icon grid (the tile is 824 of 1024 pixels).
iconset="$work/Corrent.iconset"
mkdir -p "$iconset"
sed 's/viewBox="0 0 100 100"/viewBox="-12.14 -12.14 124.28 124.28" width="1024" height="1024"/' \
  assets/icon/corrent.svg > "$work/icon.svg"
sips -s format png "$work/icon.svg" --out "$work/icon-1024.png" >/dev/null
for s in 16 32 128 256 512; do
  sips -z $s $s "$work/icon-1024.png" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$work/icon-1024.png" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$work/Corrent.icns"

# --------------------------------------------------------------------------------- Corrent.app
app="$work/Corrent.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
install -m755 "$bin/corrent" "$bin/corrent-mpvw" "$app/Contents/MacOS/"
cp "$work/Corrent.icns" "$app/Contents/Resources/Corrent.icns"
bundle_libmpv "$app/Contents/Frameworks"
copy_licenses "$app/Contents/Frameworks" "$app/Contents/Resources/licenses"
cp LICENSE-MIT LICENSE-APACHE "$app/Contents/Resources/licenses/"
min_app=$(min_macos "$app/Contents/MacOS/"* "$app/Contents/Frameworks/"*.dylib)

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>CFBundleDevelopmentRegion</key>
<string>en</string>
<key>CFBundleDisplayName</key>
<string>Corrent</string>
<key>CFBundleName</key>
<string>Corrent</string>
<key>CFBundleExecutable</key>
<string>corrent</string>
<key>CFBundleIconFile</key>
<string>Corrent</string>
<key>CFBundleIdentifier</key>
<string>$bundle_id</string>
<key>CFBundleInfoDictionaryVersion</key>
<string>6.0</string>
<key>CFBundlePackageType</key>
<string>APPL</string>
<key>CFBundleShortVersionString</key>
<string>$version</string>
<key>CFBundleVersion</key>
<string>$version</string>
<key>LSApplicationCategoryType</key>
<string>public.app-category.utilities</string>
<key>LSMinimumSystemVersion</key>
<string>$min_app</string>
<key>LSArchitecturePriority</key>
<array>
  <string>arm64</string>
</array>
<key>NSPrincipalClass</key>
<string>NSApplication</string>
<key>NSSupportsAutomaticGraphicsSwitching</key>
<true/>
<key>NSHumanReadableCopyright</key>
<string>Corrent contributors. MIT or Apache-2.0; bundled libmpv and its libraries under their own licenses.</string>
<key>NSAppleEventsUsageDescription</key>
<string>Corrent asks System Events to shut down the Mac when downloads finish, if you choose that.</string>
<key>CFBundleURLTypes</key>
<array>
  <dict>
    <key>CFBundleURLName</key>
    <string>Magnet link</string>
    <key>CFBundleURLSchemes</key>
    <array>
      <string>magnet</string>
    </array>
  </dict>
</array>
$(cat packaging/macos/Info.plist.ext)
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null
printf 'APPL????' > "$app/Contents/PkgInfo"

# Inside out: the libraries, the worker, then the app (which seals everything in it).
sign "$app/Contents/Frameworks/"*.dylib
sign --entitlements "$entitlements" "$app/Contents/MacOS/corrent-mpvw"
sign --entitlements "$entitlements" "$app"
codesign --verify --deep --strict "$app"
check_libmpv "$app/Contents/MacOS/corrent-mpvw" "$(cd "$app/Contents/Frameworks" && pwd -P)"

# ------------------------------------------------------------------------------------- the dmg
dmg="$dist/Corrent-$version-macos.dmg"
stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
cp "$work/Corrent.icns" "$stage/.VolumeIcon.icns"
rm -f "$dmg"
rw="$work/rw.dmg"
hdiutil create -quiet -volname Corrent -srcfolder "$stage" -fs HFS+ -format UDRW -ov "$rw"
# The volume icon needs the custom-icon flag on the volume root (SetFile ships with Xcode tools).
if command -v SetFile >/dev/null; then
  mnt=$(mktemp -d)
  hdiutil attach -quiet -nobrowse -noautoopen -mountpoint "$mnt" "$rw"
  SetFile -a C "$mnt" || true
  hdiutil detach -quiet "$mnt"
  rmdir "$mnt"
fi
hdiutil convert -quiet "$rw" -format ULFO -o "$dmg"
rm -f "$rw"
if [ "$sign_id" != "-" ]; then
  codesign --sign "$sign_id" --timestamp "$dmg"
  notarize "$dmg"
fi
echo "built $dmg"

# ------------------------------------------------------------------------- the server package
[ "${SKIP_PKG:-}" = 1 ] && exit 0
root="$work/server-root"
mkdir -p "$root/usr/local/bin" "$root/usr/local/share/corrent-server"
install -m755 "$bin/correntd" "$bin/corrent-mpvw" "$root/usr/local/bin/"
# corrent-mpv looks for libmpv in `corrent-lib` beside the program.
bundle_libmpv "$root/usr/local/bin/corrent-lib"
copy_licenses "$root/usr/local/bin/corrent-lib" "$root/usr/local/share/corrent-server/licenses"
sign "$root/usr/local/bin/corrent-lib/"*.dylib
sign --entitlements "$entitlements" "$root/usr/local/bin/correntd" "$root/usr/local/bin/corrent-mpvw"
check_libmpv "$root/usr/local/bin/corrent-mpvw" "$(cd "$root/usr/local/bin/corrent-lib" && pwd -P)"
install -m644 packaging/server/macos/$server_id.plist "$root/usr/local/share/corrent-server/"
install -m644 docs/SERVER.md "$root/usr/local/share/corrent-server/README.md"
install -m755 packaging/server/macos/pkg/enable-agent.sh packaging/server/macos/pkg/uninstall.sh \
  "$root/usr/local/share/corrent-server/"
min_server=$(min_macos "$root/usr/local/bin/correntd" "$root/usr/local/bin/corrent-lib/"*.dylib)
scripts="$work/server-scripts"
mkdir -p "$scripts"
install -m755 packaging/server/macos/pkg/preinstall packaging/server/macos/pkg/postinstall "$scripts/"

pkgbuild --quiet --root "$root" --scripts "$scripts" --identifier "$server_id" --version "$version" \
  --install-location / --ownership recommended "$work/correntd.pkg"
mkdir -p "$work/pkg-resources"
cat > "$work/pkg-resources/conclusion.html" <<'EOF'
<html><body style="font-family: -apple-system, sans-serif">
<h3>Corrent Server is installed and running.</h3>
<p>Open <b>http://127.0.0.1:8088</b> on this Mac to create the administrator.</p>
<p>It runs as your user while you are logged in (launchd agent <code>io.github.ulaas.correntd</code>);
data and logs are in <code>~/Library/Application Support/Corrent Server</code>.
Other users start it with <code>/usr/local/share/corrent-server/enable-agent.sh</code>;
remove it with <code>sudo /usr/local/share/corrent-server/uninstall.sh</code>.</p>
</body></html>
EOF
cat > "$work/distribution.xml" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>Corrent Server $version</title>
  <conclusion file="conclusion.html" mime-type="text/html"/>
  <options customize="never" require-scripts="false" hostArchitectures="arm64"/>
  <domains enable_localSystem="true"/>
  <volume-check>
    <allowed-os-versions><os-version min="$min_server"/></allowed-os-versions>
  </volume-check>
  <choices-outline><line choice="default"><line choice="$server_id"/></line></choices-outline>
  <choice id="default"/>
  <choice id="$server_id" visible="false"><pkg-ref id="$server_id"/></choice>
  <pkg-ref id="$server_id" version="$version" onConclusion="none">correntd.pkg</pkg-ref>
</installer-gui-script>
EOF
pkg="$dist/Corrent-Server-$version-macos-arm64.pkg"
rm -f "$pkg"
pkg_sign=()
[ -n "${MACOS_INSTALLER_IDENTITY:-}" ] && pkg_sign=(--sign "$MACOS_INSTALLER_IDENTITY" --timestamp)
productbuild --quiet --distribution "$work/distribution.xml" --resources "$work/pkg-resources" \
  --package-path "$work" ${pkg_sign[@]+"${pkg_sign[@]}"} "$pkg"
[ -n "${MACOS_INSTALLER_IDENTITY:-}" ] && notarize "$pkg"
echo "built $pkg"
