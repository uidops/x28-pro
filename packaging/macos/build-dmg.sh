#!/usr/bin/env bash
# Build "X28 PRO.app" (menu bar tray + the x28-pro CLI) and a drag-to-install
# DMG under target/macos/.
#
#   packaging/macos/build-dmg.sh
#
# Signing: ad-hoc by default. Set SIGN_IDENTITY to a "Developer ID
# Application: …" identity to sign for distribution (notarize separately).
set -euo pipefail

cd "$(dirname "$0")/../.."

NAME="X28 PRO"
BUNDLE_ID="com.x28-pro.tray"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
ARCH=$(uname -m)
SIGN_IDENTITY="${SIGN_IDENTITY:--}"

OUT=target/macos
APP="$OUT/$NAME.app"
DMG="$OUT/X28-PRO-$VERSION-$ARCH.dmg"

echo "==> building release binaries ($VERSION, $ARCH)"
cargo build --release --bins

echo "==> assembling $APP"
rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/x28-tray target/release/x28-pro "$APP/Contents/MacOS/"

echo "==> rendering icon"
ICONSET="$OUT/AppIcon.iconset"
mkdir -p "$ICONSET"
swift packaging/macos/icon.swift "$OUT/icon-1024.png"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$OUT/icon-1024.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$OUT/icon-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key>
    <string>x28-tray</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <!-- Menu bar app: no Dock icon at launch; the app adds itself to the
         Dock only while its window is open. -->
    <key>LSUIElement</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <!-- SF Symbol tray icons need macOS 11. -->
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF
plutil -lint "$APP/Contents/Info.plist" >/dev/null

echo "==> signing (identity: $SIGN_IDENTITY)"
# Inner executable first, then the bundle (which seals the main executable).
codesign --force --timestamp=none --sign "$SIGN_IDENTITY" "$APP/Contents/MacOS/x28-pro"
codesign --force --timestamp=none --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --strict --verbose=1 "$APP"

echo "==> creating $DMG"
STAGE="$OUT/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/$NAME.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "$NAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG"
rm -rf "$STAGE" "$ICONSET"

echo "==> done"
echo "    app: $APP"
echo "    dmg: $DMG ($(du -h "$DMG" | cut -f1))"
