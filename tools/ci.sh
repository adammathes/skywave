#!/usr/bin/env bash
# Runs exactly the checks CI runs (.github/workflows/ci.yml calls this script), so a green run
# here means a green PR, wall-clock budget noise on shared runners and macOS-only code aside
# (ci.yml's macos job checks that).
# --setup first installs what a fresh cloud container lacks: the Linux build deps, rustfmt,
# clippy, the wasm and i686 targets, cargo-deny and Playwright (wasm-bindgen-cli comes with
# web-build.sh).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [[ "${1:-}" == "--setup" ]]; then
    sudo=""
    [[ "$(id -u)" == 0 ]] || sudo="sudo"
    $sudo apt-get update
    $sudo apt-get install -y libudev-dev libasound2-dev pkg-config mesa-vulkan-drivers gcc-multilib
    rustup component add rustfmt clippy
    rustup target add wasm32-unknown-unknown i686-unknown-linux-gnu
    deny="$(awk -F'"' '/CARGO_DENY_VERSION:/ { print $2 }' .github/workflows/ci.yml)"
    if [[ "$(cargo deny --version 2>/dev/null || true)" != "cargo-deny $deny" ]]; then
        cargo install cargo-deny --locked --version "$deny"
    fi
    node -e "require('playwright')" 2>/dev/null || npm install --global playwright
fi

step() {
    echo "== $*" >&2
    "$@"
}

step cargo fmt --all --check
step cargo clippy --locked --all-targets --all-features -- -D warnings
step cargo clippy --locked --target wasm32-unknown-unknown -p platform-web --all-targets -- -D warnings
SKYWAVE_ENFORCE_BUDGETS=1 step cargo test --locked --workspace
# The pure crates at 32-bit usize, as on wasm32, where an overflow the 64-bit run misses panics.
step cargo test --locked --target i686-unknown-linux-gnu \
    -p skywave-core -p place -p world -p vehicle -p traffic -p audio
step cargo deny check
step tools/web-build.sh
# One scene keeps this to minutes; the full golden set is `--all` (docs/TESTING.md).
export NODE_PATH="${NODE_PATH:-$(npm root --global)}"
step node tools/screenshot/shoot.mjs --selftest --music --scene strip-golden-chase
step node tools/screenshot/diff.mjs --scene strip-golden-chase
echo "== ci.sh: all checks passed" >&2
