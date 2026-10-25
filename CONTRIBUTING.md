# Contributing

Skywave is BSD-3-Clause. By contributing you agree your contribution is licensed the same way.

- Read `CLAUDE.md` (it applies to humans too) and the doc in `docs/` for the area you touch.
- No copyrighted assets: no audio, lyrics, logos, fonts, textures or models you do not own. No real car brand or model names. Radio presets are URLs only.
- One issue, one branch, one PR. Green CI: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, web build + screenshot smoke test.
- Visual changes attach a screenshot from `tools/screenshot`.
- New dependencies need a license in the allowlist (`deny.toml`) and, if large, an ADR in `docs/adr/`.
- Keep it small. Delete before adding.
