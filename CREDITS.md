# Credits and sources

Every outside source whose numbers, formulas or data are in Skywave gets a line here
(`docs/adr/0005-reference-sources.md`). Code dependencies are not listed: `Cargo.toml` and
`cargo deny` cover them. Radio presets are not credits: they are links to stations the
player chooses to listen to, and nothing of theirs is in the repo.

## How to record a source
- **Which sources.** Every *Ship* source (its bytes, or data derived from it, are in the repo,
  the binary or a Place file) and every *Fit* source (we measured it and kept only numbers).
  *Consult* sources (looked at, nothing taken) are not listed. ADR 0005 says which mode a source
  allows; use it in that mode only.
- **One row per source**, in the table for its kind: the source (author, title, year; or the
  collection and item ID), its license or rights statement as the source states it, the date
  you retrieved it, and what we took and where it lives. When ADR 0005 says "verify", quote the
  license text you found in the PR that adds the row.
- **A fitted number** carries its provenance where it is written, as a comment beside it:
  the source and item, then what was measured, e.g.
  `bay_w: (6.0, 9.0), // LoC Margolies archive, items <id>, <id>, ...: 22 strip-mall fronts,
  bays 5.8-9.4 m`. Then add or extend that source's row here. A grammar or template value
  without such a comment is a starting value and its file says so; a PR that adds a fitted
  number without its source is sent back.
- **Dates** are ISO (`YYYY-MM-DD`). For sources used before this file existed, the date is the
  first commit that used them.

## Published methods and fitted constants
| Source | License / rights | Retrieved | What we took | Where |
|---|---|---|---|---|
| A. J. Preetham, P. Shirley, B. Smits, "A Practical Analytic Model for Daylight", SIGGRAPH 1999 | Published paper; we use its model and published coefficients (facts), none of its text or figures | 2026-10-26 | The Perez sky luminance and chromaticity model: the turbidity-linear Perez coefficients A-E and the zenith chromaticity polynomials (appendix A.2) | `crates/render/src/sky.rs` |
| J. W. Spencer, "Fourier series representation of the position of the sun", *Search* 2(5), 1971 | Published paper; formula and coefficients | 2026-10-26 | The Fourier fit of solar declination by day of year | `crates/app/src/daylight.rs` (`declination`) |
| F. Kasten, A. T. Young, "Revised optical air mass tables and approximation formula", *Applied Optics* 28(22), 1989 | Published paper; formula and coefficients | 2026-10-26 | The relative air mass approximation used to tint and dim the low sun | `crates/app/src/daylight.rs` (`air_mass`) |
| IEC 61966-2-1 (sRGB) and ITU-R BT.709 | Published standards; transfer function, primaries and white point (facts) | 2026-10-26 | The sRGB transfer curve, the D65 white point and the Rec. 709 luminance weights | `crates/world/src/palette.rs`, `crates/render/src/sky.rs`, `crates/app/src/daylight.rs` |

## Shipped data
None yet. Terrain, roads, water and footprints for compiled places (M6) will be listed here
with their vintages, per ADR 0005.

## Fitted references (photographs, drawings, datasets, CC0 kits)
None yet. The facade grammar (`assets/grammar/`) holds starting values only; the fit pass
(`docs/backlog/BUILDINGS.md`, B3) adds its sources here. No CC0 model, texture or HDRI has been
shipped or measured so far.
