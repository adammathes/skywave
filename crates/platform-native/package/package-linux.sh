#!/usr/bin/env bash
# Packages a built `skywave` binary as skywave-linux-x86_64-<tag>.tar.gz in <out-dir>, with
# one top-level folder holding the stripped binary, the launcher, a menu entry, the README
# and the license. Everything else the game needs is compiled into the binary.
#
#   crates/platform-native/package/package-linux.sh target/release/skywave v0.1.0 out
set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <skywave binary> <tag> <out-dir>" >&2
    exit 2
fi
binary="$1"
tag="$2"
out="$3"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"

name="skywave-linux-x86_64-$tag"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage"
install -m 755 "$binary" "$stage/skywave"
strip "$stage/skywave"
install -m 755 "$here/skywave.sh" "$stage/skywave.sh"
install -m 644 "$here/skywave.desktop" "$here/README-linux.txt" "$root/LICENSE" "$stage/"
# Owner and timestamps from the build machine mean nothing to players.
tar -C "$out" --owner=0 --group=0 --numeric-owner -czf "$out/$name.tar.gz" "$name"
rm -rf "$stage"
echo "package-linux: $out/$name.tar.gz"
