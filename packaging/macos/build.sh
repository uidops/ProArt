#!/usr/bin/env bash
# Build dist/ProArt.app (+ dist/ProArt-<version>-macOS.dmg).
#   packaging/macos/build.sh            native architecture
#   TARGETS="aarch64-apple-darwin x86_64-apple-darwin" packaging/macos/build.sh   universal (needs both std targets)
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$PWD"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DIST="$ROOT/dist"
APP="$DIST/ProArt.app"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
TARGETS="${TARGETS:-$HOST}"

echo "==> ProArt $VERSION for macOS ($TARGETS)"
bins=()
clis=()
for t in $TARGETS; do
    cargo build --release --target "$t" -p proart-app -p proart-cli
    bins+=("target/$t/release/proart-app")
    clis+=("target/$t/release/proart")
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
if [ "${#bins[@]}" -gt 1 ]; then
    lipo -create "${bins[@]}" -output "$APP/Contents/MacOS/ProArt"
    lipo -create "${clis[@]}" -output "$APP/Contents/Resources/bin/proart"
else
    cp "${bins[0]}" "$APP/Contents/MacOS/ProArt"
    cp "${clis[0]}" "$APP/Contents/Resources/bin/proart"
fi

echo "==> icon"
ICONSET="$(mktemp -d)/ProArt.iconset"
mkdir -p "$ICONSET"
I=assets/icons
cp $I/app-16.png   "$ICONSET/icon_16x16.png"
cp $I/app-32.png   "$ICONSET/icon_16x16@2x.png"
cp $I/app-32.png   "$ICONSET/icon_32x32.png"
cp $I/app-64.png   "$ICONSET/icon_32x32@2x.png"
cp $I/app-128.png  "$ICONSET/icon_128x128.png"
cp $I/app-256.png  "$ICONSET/icon_128x128@2x.png"
cp $I/app-256.png  "$ICONSET/icon_256x256.png"
cp $I/app-512.png  "$ICONSET/icon_256x256@2x.png"
cp $I/app-512.png  "$ICONSET/icon_512x512.png"
cp $I/app-1024.png "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/ProArt.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>ProArt</string>
  <key>CFBundleDisplayName</key><string>ProArt</string>
  <key>CFBundleIdentifier</key><string>com.proart.app</string>
  <key>CFBundleExecutable</key><string>ProArt</string>
  <key>CFBundleIconFile</key><string>ProArt</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
PLIST

echo "==> ad-hoc signature"
codesign --force --deep -s - "$APP"

echo "==> DMG"
STAGING="$(mktemp -d)"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
DMG="$DIST/ProArt-$VERSION-macOS.dmg"
rm -f "$DMG"
hdiutil create -volname "ProArt" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null
shasum -a 256 "$DMG"
echo "done: $APP"
echo "done: $DMG"
echo "note: ad-hoc signed; if Gatekeeper complains: xattr -dr com.apple.quarantine /Applications/ProArt.app"
echo "CLI: $APP/Contents/Resources/bin/proart  (symlink it onto your PATH)"
