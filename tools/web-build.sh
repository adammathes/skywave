#!/usr/bin/env bash
# Builds the browser version into dist/: release wasm, wasm-bindgen JS glue, index.html.
# wasm-bindgen-cli must match the wasm-bindgen crate in Cargo.lock exactly; it is installed
# with `cargo install` when missing or mismatched (GitHub release downloads are not used).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
dist="$root/dist"

version="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3; exit }' Cargo.lock)"
if [[ -z "$version" ]]; then
    echo "web-build: wasm-bindgen not found in Cargo.lock" >&2
    exit 1
fi
if [[ "$(wasm-bindgen --version 2>/dev/null || true)" != "wasm-bindgen $version" ]]; then
    echo "web-build: installing wasm-bindgen-cli $version" >&2
    cargo install --locked wasm-bindgen-cli --version "$version"
fi

cargo build --release --locked --target wasm32-unknown-unknown -p platform-web

rm -rf "$dist"
mkdir -p "$dist"
wasm-bindgen --target web --no-typescript --out-dir "$dist" \
    target/wasm32-unknown-unknown/release/platform_web.wasm
if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
        --enable-mutable-globals --enable-reference-types \
        "$dist/platform_web_bg.wasm" -o "$dist/platform_web_bg.wasm"
else
    echo "web-build: wasm-opt not found, skipping (optional)" >&2
fi
cp crates/platform-web/web/index.html "$dist/"

wasm="$dist/platform_web_bg.wasm"
echo "web-build: $(wc -c <"$wasm") bytes wasm, $(gzip -9c "$wasm" | wc -c) bytes gzipped -> $dist"
