# Testing

Nobody steps through this code in a debugger. Tests, determinism hashes and screenshots are how we know it works. Everything here runs in the cloud sandbox (no GPU) and in CI.

## Layers
| Layer | Where | What |
|---|---|---|
| Unit | beside code | pure functions: hashing, rng, physics steps, mesh builders, parsers, validators |
| Property/golden | `crates/*/tests` | determinism hashes per chunk for reference seeds; vehicle performance bands; Place validation over seeds 1..=64 |
| Scenario | `crates/app/tests` | scripted inputs through the full `App` for N steps → state hash and assertions (e.g. car ends inside the world, no NaNs) |
| Budget | `crates/*/benches` + CI thresholds | chunk generation time, traffic step time, submit CPU time (release, generous CI thresholds, trend logged) |
| Visual | `tools/screenshot` | web build rendered headless in Chromium (WebGL2 via SwiftShader); PNG per fixed camera; perceptual diff vs. goldens |
| Manual gate | you, at milestones | feel, vibe, platform matrix (Chrome/Firefox/Safari, Linux/Mac, keyboard/gamepad) |

## Commands
```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
tools/web-build.sh && node tools/screenshot/shoot.mjs           # writes tools/screenshot/out/*.png
cargo bench -p world -- chunk                                    # budgets (release)
```

## Visual loop for agents
1. Build the web bundle and run the screenshot tool with a scene name and camera (`--scene strip_dusk --seed 1`).
2. Open the PNG with the Read tool and look at it. Describe what is wrong before changing code.
3. Compare against `tools/screenshot/golden/<scene>.png` with the diff tool (`--diff`, prints a per-scene score; threshold 2% of pixels beyond ΔE 6). Goldens update only in a dedicated commit that explains the visual change and includes before/after in the PR.
WebGL2 under SwiftShader is slow (seconds per frame); the tool waits for `window.skywaveReady` and a settled frame, and uses a 1280×720 canvas. CI runs the smoke scene only; the full golden set runs on a label.

## Determinism
- `core::hash64` has fixed test vectors. `Rng` sequences are pinned.
- World: `crates/world/tests/golden.rs` holds a hash per chunk for seeds 1..=8 (whole 16×16 place). A mismatch fails with the chunk coordinates and the stage that diverged (each stage hashes its output).
- Vehicle: a 10 s scripted drive per spec produces a pinned state hash.
- Cross-platform: the WASM build runs the same hash tests in headless Chromium via a `?selftest=1` page mode that prints JSON to the console; the screenshot tool asserts it matches native. This is how we catch float divergence between targets.

## Performance
- Budgets in `docs/ARCHITECTURE.md` are constants; tests assert the CI machine stays under generous multiples (3×) and print actuals so trends are visible in logs.
- A `--stats` run of the native binary prints frame time percentiles; the M5 gate records them on the reference GPUs.

## Feel gates
Tests cannot judge feel. At each milestone gate the Pages deployment is played by a human. The gate notes go in `docs/MILESTONES.md` under the milestone, and anything that felt wrong becomes issues before the next milestone starts.

## Rules
- A PR that changes generation output updates goldens in its own commit with reasons.
- No test may read the network or the clock. Time is injected.
- Flaky tests are fixed or deleted the day they flake, never retried into green.
- Screenshot-affecting PRs attach the PNG.
