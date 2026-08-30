#!/bin/bash
# Builds OhMyLock.app: a minimal macOS app bundle wrapping the release
# binary, so it gets a real icon (Finder, Login Items, Get Info) and can
# declare LSUIElement (no Dock icon / no Cmd+Tab entry) at the OS level --
# on top of the ActivationPolicy::Accessory the binary already sets itself,
# so both the bundled and bare-binary ways of running this behave the same.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="OhMyLock"
BUNDLE_ID="dev.facelock.FaceLock"
BIN_NAME="OhMyLock"
DIST_DIR="dist"
APP_DIR="$DIST_DIR/$APP_NAME.app"
ICONSET="$DIST_DIR/AppIcon.iconset"

if [ ! -f "assets/AppIcon.png" ]; then
	echo "assets/AppIcon.png not found -- run: cargo run --release --example gen_app_icon" >&2
	exit 1
fi

echo "==> building release binary"
cargo build --release

echo "==> building .icns from assets/AppIcon.png"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
	sips -z "$size" "$size" assets/AppIcon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
	double=$((size * 2))
	sips -z "$double" "$double" assets/AppIcon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$DIST_DIR/AppIcon.icns"
rm -rf "$ICONSET"

echo "==> assembling $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "target/release/$BIN_NAME" "$APP_DIR/Contents/MacOS/$APP_NAME"
cp "$DIST_DIR/AppIcon.icns" "$APP_DIR/Contents/Resources/AppIcon.icns"

cat >"$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>$APP_NAME</string>
	<key>CFBundleDisplayName</key>
	<string>$APP_NAME</string>
	<key>CFBundleIdentifier</key>
	<string>$BUNDLE_ID</string>
	<key>CFBundleVersion</key>
	<string>0.1.0</string>
	<key>CFBundleShortVersionString</key>
	<string>0.1.0</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleExecutable</key>
	<string>$APP_NAME</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSCameraUsageDescription</key>
	<string>$APP_NAME kilit ekranını açmak için kayıtlı yüzünüzü kamerayla doğrular.</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

echo "==> done: $APP_DIR"
echo "First launch will re-prompt for camera access (new bundle identity)."
