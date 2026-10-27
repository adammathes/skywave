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
- Done on `main`: M0; M1 (drive, two cars, cockpit, HUD, settings, gamepad, invert Y); M2
  (procedural coast: terrain, roads, parcels, buildings, props, streaming, minimap, lights);
  M3 part 1 (synthesized sound; web car radio with internet stations); the second feel pass;
  convertibles; WebGPU→WebGL2 fallback with `?diag=1`; GPU performance pass (culling, quality
  presets with Steam Deck auto-pick, native `--stats`, `docs/PERFORMANCE.md`).
- Open branches:
  - `vehicle-picker` (c8c5770): in-game vehicle picker (`V`, Menu › Vehicle) and the "Created by
    Adam Mathes" credit on the About page and the web footer. Verified green on its base; needs a
    PR into `main` and a merge resolution against the radio and performance merges in
    `crates/app/src/ui/{draw,mod,pages,tests}.rs`, `crates/platform-web/src/lib.rs`, `index.html`.
    On the WebGL2-fallback restart, meshes already drained from `take_new_meshes` must be
    re-uploaded if the renderer is rebuilt but the `App` is kept.
- Known red: the two cockpit goldens (`strip-golden-cockpit`, `strip-night-cockpit`) predate the
  commit that widened the cockpit view and need regenerating in their own commit.
- Owner feedback not yet acted on: `docs/backlog/NOTES.md` (reset key, radio tuning sound, natural map
  edges, life and real buildings, mobile, light physics); `docs/backlog/ART.md` (more polygons and color in interiors,
  cars, NPC cars, props); Safari untested after the fallback work; the owner's Deck `--stats`
  output is wanted (`docs/PERFORMANCE.md` says what to run); native radio playback.
- Next: M3 remainder (local music files, native streams, ambience by zone), M4 (traffic,
  pedestrians, full car roster), M5 (release polish, Deck verification), M6 (place compiler),
  M7+ (eras, Chicago scenarios). Backlogs in `docs/backlog/`.

## Pitfalls met so far
- Software-rendered screenshots take minutes per scene; never run several at once on one box.
- Never share a cargo target dir across worktrees. Disk fills; `cargo clean --release` often.
- Wall-clock budget tests are noisy on shared CI runners; ceilings carry twofold slack.
- GitHub release downloads are blocked in cloud sessions; install tools with cargo or npm.
