#!/bin/bash
set -e

APP_NAME="Marmaro"
BUNDLE_ID="com.bedrockchat.app"
VERSION="0.1.0"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/target/release"
APP_OUTPUT="$SCRIPT_DIR/${APP_NAME}.app"

echo "🔨 Building release..."
cargo build --release

echo "📦 Creating .app bundle..."
rm -rf "$APP_OUTPUT"
mkdir -p "$APP_OUTPUT/Contents"/{MacOS,Resources}

# Create Info.plist
cat > "$APP_OUTPUT/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>bedrock-chat</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.13</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Copy binary
cp "$BUILD_DIR/bedrock-chat" "$APP_OUTPUT/Contents/MacOS/"
chmod +x "$APP_OUTPUT/Contents/MacOS/bedrock-chat"

echo "✅ Done: $APP_OUTPUT"
