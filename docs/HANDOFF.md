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
- Done on `main` (2026-10-27 08:15 UTC): M0; M1; M2; M3 part 1 (synthesized sound; web radio with
  7 working stations per era and NO SIGNAL auto-skip; car reset key; dial-sweep tune sound); feel
  pass 2; convertibles; WebGPU→WebGL2 fallback and the Safari shader fix; GPU performance pass
  (culling, quality presets, Deck auto-pick, `--stats`, `docs/PERFORMANCE.md`); vehicle picker and
  the credit line; art pass (era cabins, detailed exteriors, colored parked cars with LOD2, props);
  the full twelve-car 1980s roster (M4 #2); light collision physics with rocking parked cars;
  traffic lane graph (M4 #1), NPC car movement (M4 #4), pedestrian figure (M4 #3) and pedestrian
  behaviour (M4 #5), all simulation-only in `crates/traffic`; reference-sources ADR 0005 and the
  buildings backlog; facade grammar loader (B1, B2); CI gates stabilised (median step budget, no
  worst-piece assert, one CI run per PR).
- Open PRs: native radio (#15, approved, waits on the owner's sign-off for rustls+ring, ADR 0006);
  NPC traffic drawn in the app with a density setting (M4 #6); sign pictograms and the invented-name
  lexicon (B5+B6). Branches in progress: `m2/map-edges` (NOTES #3), `bldg/facade-bands` (B4),
  `m4/vehicle-select` (M4 #7).
- Owner feedback not yet acted on: mobile (NOTES #5, after Deck verification); the owner's Deck
  `--stats` output is wanted; NOTES 4(a) life beyond traffic (window lights, parked cars that come
  and go); buildings B7-B15.
- Next: M4 #8 (non-violence scenarios, density tests, gate screenshots) once #6 merges; pedestrians
  drawn in the app (figure instancing) after #6; B8 awnings and B9 rooflines after B4; B10 parking
  lots after map edges; M5 (release polish, Deck verification); M6 place compiler; M7+ scenarios.
- Conventions learned this run: reviews are read-only Opus subagents over `git diff
  origin/main...origin/<branch>` with probe tests on an exported copy; a PR that regenerates
  goldens merges before PRs touching the same scenes; a session merges `origin/main` itself when
  GitHub reports a conflict; two PRs that change the same function signature within an hour can
  both be green and still break main (the traffic tests after the grammar loader): after a merge
  that changes a shared signature, refresh the other open PRs before merging them.

## Pitfalls met so far
- Software-rendered screenshots take minutes per scene; never run several at once on one box.
- Never share a cargo target dir across worktrees. Disk fills; `cargo clean --release` often.
- Wall-clock budget tests are noisy on shared CI runners; ceilings carry twofold slack.
- GitHub release downloads are blocked in cloud sessions; install tools with cargo or npm.
