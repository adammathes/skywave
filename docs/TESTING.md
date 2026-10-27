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
tools/web-build.sh && node tools/screenshot/shoot.mjs --all     # writes tools/screenshot/out/<scene>.png
node tools/screenshot/diff.mjs --all                             # out/ vs golden/, 2% beyond ΔE 6
node tools/screenshot/shoot.mjs --selftest                       # wasm hashes vs native
cargo bench -p world -- chunk                                    # budgets (release)
```

## Visual loop for agents
1. Build the web bundle and capture a scene: `tools/web-build.sh && node tools/screenshot/shoot.mjs --scene strip-golden-chase` writes `tools/screenshot/out/strip-golden-chase.png` (`--all` captures every scene; `--scene` may repeat).
2. Open the PNG with the Read tool and look at it. Describe what is wrong before changing code.
3. Compare against `tools/screenshot/golden/<scene>.png`: `node tools/screenshot/diff.mjs --scene <name>` (or `--all`) prints the share of pixels beyond ΔE 6 (CIE76 in CIELAB) and fails above 2%, writing `out/<name>.diff.png` (differing pixels red to yellow over the dimmed golden). Goldens update only in a dedicated commit that explains the visual change and includes before/after in the PR.

Scenes live in `tools/screenshot/scenes.json`, each a set of URL parameters that fully specify the frame (`crates/app/src/scene.rs`, `SceneSetup`): `seed`, `car`, `cam`, `t=HH:MM` (time of day, clock frozen), `pos=x,z` and `yaw=deg` (car placement; compass heading, 90 = east), `lights=1`, `still=1`, which stops the simulation so every frame is the same picture (a parked car still jitters in the last float bits, enough to flip distant depth ties between captures), `map=1` (start on the full-screen map), `ui=<1..2>` (UI scale for the session), `traffic=off|low|normal|busy` and `phase=<tick>` (the signal tick the junction lights start at before the traffic's warm-up, 120 a second, so a still scene can catch a red with a queue at it). The desktop build takes the same keys as `--scene key=value`. `shoot.mjs --size 1280x800` captures another viewport (the Deck's 16:10) for ad hoc checks; goldens stay 1280×720.

Regenerating every golden (after a deliberate visual change) is one line: `tools/web-build.sh && node tools/screenshot/shoot.mjs --all --golden`. Goldens are 1280×720 PNGs and are committed; `out/` is not.

WebGL2 under SwiftShader is slow (seconds per frame); the tool waits for `window.skywaveReady` and ten more frames, allows the capture 180 s, and uses a 1280×720 viewport. A blank capture fails and never becomes a golden. CI captures `strip-golden-chase` only, diffs it and runs the self-test, and uploads `out/` as an artifact; run the full set locally.

## Shaders and the web start-up
`crates/render` tests parse and validate every WGSL module with naga and translate every entry point to GLSL ES 3.00 (WebGL2), MSL 2.1 and SPIR-V; build the whole renderer on wgpu's no-op backend under WebGL2 limits in both HDR modes and both corona depth reads; and check that the shaders stay core WGSL (no `enable`, `f16`, `override`, storage or sample-index use). The MSL translation is the stand-in for Safari, which compiles WGSL to Metal itself: it catches much of what Metal rejects (depth reads, packing builtins, interpolation, reserved names). `crates/platform-web` tests the backend decision (`fallback`: every WebGPU start-up failure kind retries on WebGL2 unless `gpu=1`) and the canvas sizing. In the browser, `?diag=1` shows the start-up checks as a report and `?gl=1` / `?gpu=1` pin the backend (docs/RENDERING.md); `tools/web-build.sh` fails when the wasm uses a feature Safari 15 lacks (SIMD, threads, memory64, exception handling, tail calls).

## Determinism
- `core::hash64` has fixed test vectors. `Rng` sequences are pinned.
- World: `crates/world/tests/golden.rs` holds a hash per chunk for seeds 1..=8 (whole 16×16 place). A mismatch fails with the chunk coordinates and the stage that diverged (each stage hashes its output).
- Vehicle: a 10 s scripted drive per spec produces a pinned state hash.
- Cross-platform: the web build opened with `?selftest=1` draws nothing; it runs `app::selftest` (the test town's content and mesh hashes, the app's scripted drive, the vehicle replay for both cars) and prints one console line `{"skywave_selftest": {...}}`. `node tools/screenshot/shoot.mjs --selftest` compares that line with `tools/screenshot/selftest.json`, and a native test (`selftest_matches_the_pins_and_the_committed_json`) keeps that file equal to the native values and to the crates' own pins. After a deliberate hash change, rewrite it with `cargo test -p app -- --ignored write_selftest`. This is how we catch float divergence between targets.

## Performance
- Budgets in `docs/ARCHITECTURE.md` are constants; tests assert the CI machine stays under generous multiples (3×) and print actuals so trends are visible in logs.
- A `--stats` run of the native binary prints frame time percentiles; the M5 gate records them on the reference GPUs.

## Feel gates
Tests cannot judge feel. At each milestone gate the Pages deployment is played by a human. The gate notes go in `docs/MILESTONES.md` under the milestone, and anything that felt wrong becomes issues before the next milestone starts.

## Rules
- A PR that changes generation output updates goldens in its own commit with reasons.
- No test may read the network or the clock. Time is injected. The few wall-clock budget tests print their numbers always and assert only when `SKYWAVE_ENFORCE_BUDGETS` is set (CI sets it; a loaded dev machine does not).
- Flaky tests are fixed or deleted the day they flake, never retried into green.
- Screenshot-affecting PRs attach the PNG.
