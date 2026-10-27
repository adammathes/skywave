# Handoff: how work on Skywave runs from here

Everything a session needs is in this repo. A new session reads `CLAUDE.md`, this file,
`docs/MILESTONES.md` (status and gate notes), then the backlog it is picking up.

## The model: a normal project
- `main` is the trunk. CI (`.github/workflows/ci.yml`) runs on every push and pull request;
  Pages (https://adammathes.github.io/skywave/) and the `nightly` pre-release
  (https://github.com/adammathes/skywave/releases/tag/nightly, with the Steam Deck AppImage that
  `tools/install-deck.sh` installs) rebuild from `main` only.
- **One task, one branch, one session, one PR into `main`.** Each task runs in its own cloud
  session on its own branch (`m<n>/<slug>`), independent of every other. The PR is the unit of
  review: CI green, screenshots attached for visual work, then merge. No local merging of other
  sessions' branches, no shared checkouts, no coordinator session that everything funnels through.
- Tasks are written so they do not overlap in files. Two tasks that must touch the same file
  (`crates/app/src/ui/*`, `crates/platform-web/src/web.rs` are the usual hot spots) run one after
  the other, not side by side.
- A task's session may still use subagents for grunt work inside its own branch. It does not
  spawn sessions for other tasks.
- `docs/AGENT_WORKFLOW.md` has the builder loop and review checklist; it was written for this model.

## To start a task
Start a cloud session on `adammathes/skywave` from `main`, branch `m<n>/<slug>`, with the task
text from the backlog (or the issue) as the prompt, plus: "Read CLAUDE.md and docs/HANDOFF.md. Open
a PR into main when CI is green." That is the whole handoff; no chat history is needed. In a
Claude Project, put `CLAUDE.md` and this file in the project instructions and start sessions from it.

## Verification before a PR
`cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo clippy --target wasm32-unknown-unknown -p platform-web --all-targets -- -D warnings`;
`cargo test --workspace`; `cargo deny check`; `tools/web-build.sh`; for visual work
`node tools/screenshot/shoot.mjs --selftest --all && node tools/screenshot/diff.mjs --all` (software
rendering, minutes per scene; run it once, not per subagent). Determinism hashes move only on
purpose, in their own commit, with the reason; `cargo test -p app -- --ignored write_selftest`
rewrites `tools/screenshot/selftest.json`. Budget tests assert only under `SKYWAVE_ENFORCE_BUDGETS`.

## Where things stand (keep current)
- Done on `main` (2026-10-27 04:50 UTC): M0; M1 (drive, cockpit, HUD, settings, gamepad, invert Y);
  M2 (procedural coast: terrain, roads, parcels, buildings, props, streaming, minimap, lights);
  M3 part 1 (synthesized sound; web car radio with 7 working stations per era, NO SIGNAL
  auto-skip); the second feel pass; convertibles; WebGPU→WebGL2 fallback, Safari shader fix;
  GPU performance pass (culling, Low/Deck/Medium/High/Ultra presets with Deck auto-pick, native
  `--stats`, `docs/PERFORMANCE.md`); in-game vehicle picker (`V`, Menu › Vehicle) and the credit
  line; art pass (era cabins with backlit dials, detailed exteriors, two-tone paint, colored parked
  cars with LOD2, richer props); traffic lane graph (M4 #1, `crates/traffic`); reference-sources
  ADR 0005 and the buildings facade grammar backlog (`docs/backlog/BUILDINGS.md`); CI gates
  stabilised (no single-worst-piece wall-clock assert; one CI run per PR).
- Open PRs: light collision physics (#7, approved, merging main); reset key + radio dial sweep
  (#9, approved after fixes, in CI). Branches in progress: `m4/pedestrian-figure` (M4 #3),
  `m4/roster` (M4 #2, eight more cars), `m2/map-edges` (NOTES #3, natural boundaries).
- Owner feedback not yet acted on: `docs/backlog/NOTES.md` 4(a) life (traffic behaviour M4 #4-#6,
  pedestrians #5, window lights, parked cars that come and go) and 4(b) buildings
  (`docs/backlog/BUILDINGS.md` B1-B15); mobile (NOTES #5, after Deck verification); the owner's
  Deck `--stats` output is wanted (`docs/PERFORMANCE.md`); native radio playback.
- Next: M4 #4 car following (after lane graph), #5 pedestrians (after figure), #6 density settings,
  #7 vehicle select polish; M3 remainder (local music files, native streams, ambience by zone);
  buildings B1-B5 can start now (B4/B8/B9/B11/B13 touch `world::buildings` and run one at a time);
  M5 (release polish, Deck verification), M6 (place compiler), M7+ (eras, Chicago scenarios).
- Conventions learned this run: reviews are read-only Opus subagents over `git diff
  origin/main...origin/<branch>`; a PR that regenerates goldens merges before PRs that touch the
  same scenes; a session merges `origin/main` into its branch itself when GitHub reports a
  conflict; owner-priority fixes (radio, picker) merge ahead of feature work.

## Pitfalls met so far
- Software-rendered screenshots take minutes per scene; never run several at once on one box.
- Never share a cargo target dir across worktrees. Disk fills; `cargo clean --release` often.
- Wall-clock budget tests are noisy on shared CI runners; ceilings carry twofold slack.
- GitHub release downloads are blocked in cloud sessions; install tools with cargo or npm.
