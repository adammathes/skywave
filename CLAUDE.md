# Skywave — conventions for agents

Skywave is a chill, single-player, first-person driving game: period-correct cars,
a coherent seed-generated (or real-data-compiled) American place, low-poly geometry
lit like it is 2026, real music on the radio. Read `docs/VISION.md` first, then the
doc for the area you are touching. `docs/MILESTONES.md` says what is in scope now.

## Non-negotiables
- **License:** BSD-3-Clause. Dependencies must be BSD/MIT/Apache-2.0/Zlib/CC0/Unlicense
  compatible (`cargo deny check` enforces). No GPL/LGPL/ODbL code.
- **No copyrighted assets.** No audio, lyrics, logos, fonts, textures or models we do not own.
  Radio presets are URLs only. Car designs are *inspired by* an era, never a copy of a real
  model; no real brand or model names anywhere in code, data or docs. No real-world station call
  signs, neighborhood or street names in procedural content either, not even as placeholders:
  placeholders stick. Invented names only (compiled real places may use real place names).
- **Zero violence.** Nothing can be hurt. Pedestrians are non-collidable and step aside.
- **Offline.** The game makes no network requests except to music sources the user chose.
  No LLM/inference at runtime. Ever.
- **Single player.** No multiplayer code paths.

## Minimalism
- Prefer `std`/`alloc` and small, pure crates. A new dependency over ~5k lines needs an ADR in
  `docs/adr/` and sign-off. Delete before adding. Measure before optimizing.
- No framework churn: wgpu/winit/glam are the only engine-level deps. No ECS crates.
- Plain structs, arrays and loops in simulation code. No trait-object or macro cleverness in hot
  paths. We may hand-port the sim to C someday; write it so that is a transliteration.

## Architecture rules (see docs/ARCHITECTURE.md)
- `core`, `place`, `world`, `vehicle`, `traffic` are `#![no_std]` + `alloc`. No filesystem,
  threads, clocks, randomness or OS calls; the platform crate feeds them inputs.
- Simulation runs at a fixed 120 Hz step; rendering interpolates. Input is sampled every frame.
- Everything seeded is deterministic: same seed → bit-identical Place on every platform. Never use
  `f32::mul_add`, fast-math, `HashMap` iteration order, or platform RNG in generation code.
- Hot paths allocate nothing per frame. Chunk data lives in pools with fixed budgets.
- All cross-crate data is plain-old-data with a documented layout (`docs/PLACES.md`, `docs/VEHICLES.md`).
- The renderer must run on WebGL2 (wgpu GLES backend) as well as WebGPU/Metal/Vulkan. CI
  screenshots run on WebGL2 via SwiftShader; if it does not render there, it does not ship.

## Workflow
- One issue → one branch → one PR into `main`. CI must be green: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo deny check`, wasm build,
  headless screenshot smoke test.
- Visual changes attach a screenshot from `tools/screenshot` to the PR.
- Commit messages: imperative, one line, body explains *why*. No model names in titles or bodies (attribution trailers are fine).
- Keep PRs small and finished. No TODO-driven stubs on `main`. No drive-by refactors.
- Tests live beside code. World-gen changes update determinism hashes deliberately, in their own
  commit, with the reason.

## Cloud dev loop
- `cargo test` and `cargo clippy` run natively here (no GPU).
- Web: `tools/web-build.sh` → `tools/screenshot/shoot.mjs` renders the page in headless Chromium
  (WebGL2/SwiftShader) and writes a PNG you can look at with the Read tool.
- GitHub release downloads are blocked in this environment: install tools via `cargo install`
  or npm, never from release tarballs.
