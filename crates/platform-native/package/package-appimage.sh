#!/usr/bin/env bash
# Assembles Skywave.AppDir in <out-dir> around a built `skywave` binary and, when an
# appimagetool is given, packs it as <out-dir>/Skywave-x86_64.AppImage. Without one it stops
# after the AppDir, which is how the layout is checked on machines without the tool.
#
#   crates/platform-native/package/package-appimage.sh target/release/skywave out [appimagetool [runtime]]
#
# The AppImage holds only the binary: libudev (for gamepads), the GPU drivers and glibc come
# from the system, as they must. SteamOS and desktop distributions ship all three.
set -euo pipefail

if [[ $# -lt 2 || $# -gt 4 ]]; then
    echo "usage: $0 <skywave binary> <out-dir> [appimagetool [runtime-file]]" >&2
    exit 2
fi
binary="$1"
out="$2"
tool="${3:-}"
runtime="${4:-}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"

appdir="$out/Skywave.AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/256x256/apps" "$appdir/usr/share/doc/skywave"
install -m 755 "$binary" "$appdir/usr/bin/skywave"
strip "$appdir/usr/bin/skywave"
install -m 644 "$root/LICENSE" "$here/README-linux.txt" "$appdir/usr/share/doc/skywave/"
install -m 644 "$here/skywave.png" "$appdir/usr/share/icons/hicolor/256x256/apps/skywave.png"
install -m 644 "$here/skywave.png" "$appdir/skywave.png"
ln -s skywave.png "$appdir/.DirIcon"

cat > "$appdir/skywave.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Skywave
Comment=A chill, single-player driving game
Exec=skywave
Icon=skywave
Terminal=false
Categories=Game;Simulation;
DESKTOP
cp "$appdir/skywave.desktop" "$appdir/usr/share/applications/skywave.desktop"

# The game reads nothing next to itself, so AppRun keeps the caller's working directory.
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/skywave" "$@"
APPRUN
chmod 755 "$appdir/AppRun"
echo "package-appimage: $appdir"

if [[ -n "$tool" ]]; then
    image="$out/Skywave-x86_64.AppImage"
    rm -f "$image"
    args=()
    [[ -n "$runtime" ]] && args+=(--runtime-file "$runtime")
    # Extract-and-run: CI runners have no FUSE to mount the tool's own image.
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" "${args[@]}" "$appdir" "$image"
    echo "package-appimage: $image"
fi
