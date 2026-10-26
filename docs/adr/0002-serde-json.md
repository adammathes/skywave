# ADR 0002: serde + serde_json for settings JSON in `app`

Status: proposed, 2026-10-26 (needs owner sign-off per CLAUDE.md)

## Context
`docs/CONTROLS_INPUT.md` and `docs/ARCHITECTURE.md` store input bindings, and later all
settings, as one JSON document (localStorage on web, a config file natively). Loading must be
forgiving: a file from an older or newer build, or one edited by hand, fills in defaults and never
locks the player out. The issue for the bindings model (M1 #6) asks for `serde` + `serde_json`.

## Options considered
| Option | Size | Notes |
|---|---|---|
| serde + serde_json | serde_json ~18k lines, plus itoa (~0.5k) and zmij (~2k); serde, serde_core and serde_derive are already in the dependency graph through winit/naga | The de facto standard; derive keeps the format next to the types; MIT/Apache-2.0 (zmij MIT) |
| Hand-written JSON reader/writer | ~400–600 lines we own | No new dependency, but a parser to test and maintain, and every settings type needs hand-written field code |
| A smaller JSON crate (e.g. nanoserde, tinyjson) | 2–5k lines | Less battle-tested; nanoserde's derive does not handle externally tagged enums the same way, and we would still hand-roll the forgiving loader |

## Decision
serde + serde_json, pinned exactly in the workspace, used only by `app` (std). Simulation
crates (`core`, `place`, `world`, `vehicle`, `traffic`) do not depend on it.

The loader reads the document as `serde_json::Value` and pulls out each device, action and option
separately, so one bad entry falls back to its default instead of failing the whole file.
Individual bindings use derived `Deserialize`.

## Consequences
- Three new crates in `Cargo.lock` (serde_json, itoa, zmij); `cargo deny check` passes.
- The wasm bundle grows a little; JSON is only parsed at load and save, never per frame.
- Later settings (video, audio, world) reuse the same crates and the same forgiving pattern.
