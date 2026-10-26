#!/usr/bin/env bash
# Installs (or updates) Skywave on a Steam Deck or any x86_64 Linux desktop, for the current
# user only: no sudo, nothing outside $HOME.
#
#   curl -fsSL https://raw.githubusercontent.com/adammathes/skywave/claude/intelligent-hypatia-vnrj1a/tools/install-deck.sh | bash
#   bash install-deck.sh --tag v0.2.0      # a tagged release instead of the nightly build
#
# It downloads Skywave-x86_64.AppImage into ~/Applications/Skywave/, checks it against the
# release's SHA256SUMS.txt, adds a menu entry (which Steam's "Add a Non-Steam Game" list
# shows), and prints the steps to put it in the Steam library. Run it again to update.
#
# Testing hook: SKYWAVE_INSTALL_FAKE=<file> copies that file instead of downloading
# (no checksum), and SKYWAVE_INSTALL_FAKE_ICON=<png> does the same for the icon.
set -euo pipefail

repo="${SKYWAVE_REPO:-adammathes/skywave}"
tag="nightly"

usage() {
    echo "usage: install-deck.sh [--tag vX.Y.Z]   (default: the rolling nightly build)"
}
while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag)
            [[ $# -ge 2 && -n "$2" ]] || { usage >&2; exit 2; }
            tag="$2"
            shift 2
            ;;
        --tag=*)
            tag="${1#--tag=}"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done
[[ "$tag" =~ ^[A-Za-z0-9._+-]+$ ]] || { echo "install-deck: unusable tag: $tag" >&2; exit 2; }
[[ -n "${HOME:-}" && "$HOME" == /* ]] || { echo "install-deck: HOME is not set" >&2; exit 1; }

if [[ "$(uname -m)" != x86_64 && -z "${SKYWAVE_INSTALL_FAKE:-}" ]]; then
    echo "install-deck: Skywave's Linux build is x86_64 only (this machine is $(uname -m))." >&2
    exit 1
fi

name="Skywave-x86_64.AppImage"
dir="$HOME/Applications/Skywave"
app="$dir/$name"
icon="$dir/skywave.png"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
desktop="$apps/skywave.desktop"
base="https://github.com/$repo/releases/download/$tag"
config="${XDG_CONFIG_HOME:-}"
[[ "$config" == /* ]] || config="$HOME/.config"

mkdir -p "$dir" "$apps"
work="$(mktemp -d "$dir/.install.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# 1. The AppImage, downloaded beside the old one and swapped in only once it is complete.
if [[ -n "${SKYWAVE_INSTALL_FAKE:-}" ]]; then
    echo "Copying $SKYWAVE_INSTALL_FAKE (SKYWAVE_INSTALL_FAKE) ..."
    cp "$SKYWAVE_INSTALL_FAKE" "$work/$name"
else
    command -v curl > /dev/null || { echo "install-deck: needs curl" >&2; exit 1; }
    echo "Downloading Skywave ($tag) from $base/$name ..."
    curl -fL --retry 3 --progress-bar -o "$work/$name" "$base/$name" || {
        echo "install-deck: could not download $base/$name (is there a release named $tag?)" >&2
        exit 1
    }
    if curl -fsSL --retry 3 -o "$work/SHA256SUMS.txt" "$base/SHA256SUMS.txt"; then
        want="$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1 }' "$work/SHA256SUMS.txt")"
        have="$(sha256sum "$work/$name" | awk '{ print $1 }')"
        if [[ -z "$want" ]]; then
            echo "install-deck: warning: SHA256SUMS.txt does not list $name; not verified." >&2
        elif [[ "$want" != "$have" ]]; then
            echo "install-deck: checksum mismatch for $name." >&2
            echo "The release may be being updated right now; try again in a few minutes." >&2
            exit 1
        else
            echo "Checksum OK."
        fi
    else
        echo "install-deck: warning: no SHA256SUMS.txt in the release; not verified." >&2
    fi
fi
chmod 755 "$work/$name"
mv -f "$work/$name" "$app"

# 2. The icon: from this checkout when run from one, else from the same tag on GitHub.
#    A missing icon only costs the picture, so it never stops the install.
here=""
[[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]] && here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
local_icon="${here:+$here/../crates/platform-native/package/skywave.png}"
if [[ -n "${SKYWAVE_INSTALL_FAKE_ICON:-}" ]]; then
    cp "$SKYWAVE_INSTALL_FAKE_ICON" "$work/skywave.png"
elif [[ -n "$local_icon" && -f "$local_icon" ]]; then
    cp "$local_icon" "$work/skywave.png"
elif [[ -z "${SKYWAVE_INSTALL_FAKE:-}" ]]; then
    curl -fsSL --retry 3 -o "$work/skywave.png" \
        "https://raw.githubusercontent.com/$repo/$tag/crates/platform-native/package/skywave.png" ||
        rm -f "$work/skywave.png"
fi
if [[ -f "$work/skywave.png" ]]; then
    mv -f "$work/skywave.png" "$icon"
elif [[ ! -f "$icon" ]]; then
    echo "install-deck: warning: could not fetch the icon; the menu entry has none." >&2
fi

# 3. The menu entry. Desktop files want quoted paths when they contain spaces.
cat > "$desktop.tmp" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Skywave
Comment=A chill, single-player driving game
Exec="$app"
Path=$dir
Icon=$icon
Terminal=false
Categories=Game;Simulation;
DESKTOP
mv -f "$desktop.tmp" "$desktop"
command -v update-desktop-database > /dev/null && update-desktop-database "$apps" 2> /dev/null || true

# Prints "skywave <git describe>"; it needs FUSE, so an empty answer is not an error.
version="$("$app" --version 2> /dev/null || true)"
version="${version#skywave }"

cat <<DONE

Skywave ${version:-($tag)} is installed.
  Game:       $app
  Menu entry: $desktop
  Settings:   $config/skywave/settings.json

Put it in your Steam library (once; updates keep the same path):
  1. In Desktop Mode, open Steam.
  2. Games > Add a Non-Steam Game to My Library...
  3. Tick "Skywave" in the list, or click Browse... and pick
     $app
     (set the file type filter to "All Files" to see it).
  4. Click "Add Selected Programs". Leave Launch Options empty.
  5. Return to Gaming Mode: Skywave is in your Library under Non-Steam.

The Deck's controls act as a standard gamepad and Skywave is gamepad-first, so no
controller layout is needed. Press Start (Menu) for settings.

To update, run this installer again. To uninstall, delete
  $dir and $desktop
and remove the shortcut in Steam.
DONE
