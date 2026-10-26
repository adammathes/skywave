#!/usr/bin/env bash
# Builds the browser version into dist/: release wasm, wasm-bindgen JS glue, index.html.
# wasm-bindgen-cli must match the wasm-bindgen crate in Cargo.lock exactly; it is installed
# with `cargo install` when missing or mismatched (GitHub release downloads are not used).
# wasm-opt (binaryen; `npm install -g binaryen` works) shrinks the wasm when it is on PATH;
# SKYWAVE_WASM_OPT sets its optimization flag (default -Oz).
# index.html gets the version (`git describe --tags --always --dirty`, else $SKYWAVE_VERSION)
# and the wasm size, for the loading bar.
# Honors CARGO_TARGET_DIR (default `target/`).
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
    "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/platform_web.wasm"
wasm="$dist/platform_web_bg.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
    # The features rustc's wasm32 target enables by default, so the output stays valid for
    # the same browsers (all of them in Safari 15+, Chrome 91+, Firefox 89+); nothing newer
    # (no SIMD, threads, memory64, exception handling or tail calls). Binaryen keeps IEEE
    # float semantics, so the self-test hashes hold.
    wasm-opt "${SKYWAVE_WASM_OPT:--Oz}" --enable-bulk-memory --enable-nontrapping-float-to-int \
        --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue \
        "$wasm" -o "$wasm"
else
    echo "web-build: wasm-opt not found, skipping (optional)" >&2
fi

# The wasm must not use a feature Safari 15 lacks: read the `target_features` section rustc
# writes (wasm-opt keeps it), and check that no memory is shared or 64-bit and that there is
# no exception tag section.
if command -v node >/dev/null 2>&1; then
    node --input-type=module - "$wasm" <<'JS'
import { readFileSync } from "node:fs";
const allowed = new Set(["mutable-globals", "nontrapping-fptoint", "sign-ext", "bulk-memory",
  "bulk-memory-opt", "reference-types", "call-indirect-overlong", "multivalue"]);
const b = readFileSync(process.argv[2]);
let p = 8;
const leb = () => {
  let r = 0, s = 0, x;
  do { x = b[p++]; r += (x & 0x7f) * 2 ** s; s += 7; } while (x & 0x80);
  return r;
};
const bad = [], used = [];
while (p < b.length) {
  const id = b[p++], len = leb(), end = p + len;
  if (id === 13) bad.push("exception handling (tag section)");
  if (id === 5) {
    for (let n = leb(); n > 0; n--) {
      const flags = leb();
      if (flags & 2) bad.push("shared memory (threads)");
      if (flags & 4) bad.push("memory64");
      leb();
      if (flags & 1) leb();
    }
  }
  if (id === 0) {
    const nameLen = leb();
    const name = b.subarray(p, p + nameLen).toString();
    p += nameLen;
    if (name === "target_features") {
      for (let n = leb(); n > 0; n--) {
        const prefix = String.fromCharCode(b[p++]), l = leb();
        const feature = b.subarray(p, p + l).toString();
        p += l;
        if (prefix !== "-") (allowed.has(feature) ? used : bad).push(feature);
      }
    }
  }
  p = end;
}
if (bad.length) {
  console.error(`web-build: the wasm uses features older browsers lack: ${bad.join(", ")}`);
  process.exit(1);
}
console.error(`web-build: wasm features: ${used.join(", ") || "(no target_features section)"}`);
JS
else
    echo "web-build: node not found, skipping the wasm feature check" >&2
fi

game_version="$(git describe --tags --always --dirty 2>/dev/null || true)"
game_version="${game_version:-${SKYWAVE_VERSION:-unknown}}"
# Keep the substitution safe for sed and HTML.
game_version="$(printf '%s' "$game_version" | tr -cd 'A-Za-z0-9._+-')"
sed -e "s/%SKYWAVE_VERSION%/$game_version/g" -e "s/%SKYWAVE_WASM_BYTES%/$(wc -c <"$wasm" | tr -d " ")/g" \
    crates/platform-web/web/index.html >"$dist/index.html"

echo "web-build: $game_version: $(wc -c <"$wasm") bytes wasm, $(gzip -9c "$wasm" | wc -c) bytes gzipped -> $dist"
