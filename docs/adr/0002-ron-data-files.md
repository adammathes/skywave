# ADR 0002: RON + serde for data files

Status: accepted, 2026-10-26

## Context
Vehicle specs, shape specs, region templates, scenarios and radio presets are data files that humans edit and agents generate. `CLAUDE.md` requires an ADR for any dependency over ~5k lines; `ron` 0.12 is ~11k lines, `serde` was already present.

## Decision
Use RON (Rusty Object Notation) via `serde` for every human-edited data file in `assets/`. Parsing lives in `app` (std); the data types live in the `no_std` crates with `serde` derives (`alloc` feature only). JSON (`serde_json`, ~18k lines, MIT/Apache) is used only for settings/bindings persisted by the platform, because browsers and config dirs speak JSON; it is accepted under this same ADR.

## Alternatives
Hand-written parser for the subset we use (~400 lines, grows with every new file type, no error positions); TOML (no enums with payloads, awkward nested data); JSON for everything (no comments, noisy for humans).

## Consequences
One parser and one error format for all data files, with line:col errors. `ron` is MIT/Apache, pure Rust, no transitive surprises (`typeid` only). Struct names are checked by ron 0.12, so Rust type names and RON names must match; the loader tests enforce this.
