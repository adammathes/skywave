#!/usr/bin/env bash
# Package the macOS binary as an unsigned app bundle inside a tarball:
#   package-mac.sh <path/to/skywave> <tag> <out-dir> <arch>
set -euo pipefail
bin="$1"; tag="$2"; out="$3"; arch="${4:-arm64}"
name="skywave-macos-$arch-$tag"
app="$out/$name/Skywave.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/skywave"
chmod +x "$app/Contents/MacOS/skywave"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Skywave</string>
  <key>CFBundleDisplayName</key><string>Skywave</string>
  <key>CFBundleIdentifier</key><string>com.adammathes.skywave</string>
  <key>CFBundleVersion</key><string>$tag</string>
  <key>CFBundleShortVersionString</key><string>$tag</string>
  <key>CFBundleExecutable</key><string>skywave</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST
cp LICENSE "$out/$name/"
cat > "$out/$name/README-macos.txt" <<TXT
Skywave for macOS ($arch), unsigned build.

First launch: right-click Skywave.app and choose Open (Gatekeeper warns once
because the app is not notarized), or in Terminal:
  xattr -d com.apple.quarantine Skywave.app
Then double-click. Gamepads work over USB or Bluetooth. Settings are saved in
~/.config/skywave/settings.json. Everything the game needs is built in.
TXT
(cd "$out" && tar -czf "$name.tar.gz" "$name")
echo "packaged $out/$name.tar.gz"
