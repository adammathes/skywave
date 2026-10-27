# Handoff: how to resume Skywave in a fresh session

Everything a new coordinator session needs is in this repo. Read in this order: `CLAUDE.md`,
this file, `docs/MILESTONES.md` (status + gate notes), `docs/backlog/*.md`, `docs/IDEAS.md`.
Then say to the new session: **"Read docs/HANDOFF.md and continue the autonomous milestone run."**

## Where things stand (update this section at every push of a milestone or merge)
- Branch: `claude/intelligent-hypatia-vnrj1a` (all work; `main` is still empty). Pages deploys from this
  branch to https://adammathes.github.io/skywave/ on every push; the nightly pre-release
  (https://github.com/adammathes/skywave/releases/tag/nightly) rebuilds web, Linux tarball, macOS
  app bundle and a Steam Deck AppImage on every push; `tools/install-deck.sh` installs it on a Deck.
- Done and pushed: M0, M1 (drive, two cars, cockpit, HUD, settings, gamepad, invert Y), M2 (procedural
  coast: terrain, roads, streets, parcels, buildings, detailing, streaming, far terrain, minimap,
  lights/coronas), M3 part 1 (synthesized sound; radio merge in progress), feel pass 2 (arcade
  assists, cockpit FOV/seat settings), convertibles, WebGPU→WebGL2 fallback + `?diag=1`.
- Also merged: car radio with internet stations on the web (`]`/`[` tune, `\` power; native
  playback not yet), Deck performance pass (CPU culling, Low/Deck/Medium/High/Ultra presets with
  Deck auto-pick, `--stats` with GPU timings, `docs/PERFORMANCE.md` with what to run on a Deck),
  in-game vehicle picker (`V`, Menu › Vehicle) and the "Created by Adam Mathes" credit.
- Owner feedback not yet acted on: `docs/backlog/ART.md` (more polygons and color in interiors,
  cars, NPC cars, props); Safari untested after the fallback work; native never run by us; the
  owner's Deck `--stats` output is wanted (`docs/PERFORMANCE.md` says what to run).
- Next milestones: M3 remainder (local music files native+web, native streams), M4 (traffic,
  pedestrians, full roster, vehicle select polish), M5 (release polish, Steam Deck verification),
  M6 (place compiler), M7+ (eras, Chicago scenarios, styles).

## How the work is run
- **Coordinator** (one session, the expensive model): writes specs/backlogs, spawns builders, merges,
  verifies, re-pins, pushes, reports. Does not type code except trivial conflict resolution.
- **Builders**: Opus subagents, one task each, `isolation: worktree`, briefed with: the docs to read,
  the files they own, tests to add, the verification list, "commit on your branch, do not push".
  Each returns a report; the coordinator merges in an order that minimizes conflicts (sim/vehicle
  first, then render, then app/platform) and delegates non-trivial merges to an Opus agent working
  in the main checkout ("do not abort, do not push, verify, commit the merge").
- **Verification after every merge**: `cargo fmt --all --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, the wasm32 clippy for `platform-web`, `cargo test --workspace`,
  `cargo deny check`, `tools/web-build.sh`, `node tools/screenshot/shoot.mjs --selftest --all &&
  node tools/screenshot/diff.mjs --all`. Push only green. Re-pin determinism hashes only when a
  change is intended, in their own commit with the reason; `cargo test -p app -- --ignored
  write_selftest` rewrites `tools/screenshot/selftest.json`.
- **Rules learned the hard way**: never share a cargo target dir across worktrees (artifacts
  cross-contaminate); the disk is ~250 GB nominal but ~30 GB usable, so delete `target/` of
  finished worktrees and `cargo clean --release` often; wall-clock budget tests assert only when
  `SKYWAVE_ENFORCE_BUDGETS` is set (CI sets it); SwiftShader screenshots take minutes under load,
  so capture one scene at a time when the machine is busy; the Opus session limit can pause all
  builders at once (work is saved in worktrees; resume with a message, or re-spawn with "continue
  from the worktree state"); a container restart kills agents but not their worktrees.
- **Feel gates** need the owner on real hardware; the coordinator records its own assessment in
  `docs/MILESTONES.md` and keeps going.

## Moving to a Claude Project / new session
1. Make sure the branch is pushed and `git status` is clean (finish or abort any in-progress merge:
   `cat .git/MERGE_HEAD`).
2. Update "Where things stand" above and push.
3. Start the new session on the repo (same branch) with the one-line instruction at the top. If
   using a Claude Project, put `CLAUDE.md` and this file in the project instructions and attach the
   repo; the chat history is not needed.
4. Pending worktrees (`git worktree list`) carry unfinished builder work; a new session can
   re-spawn a builder with "continue from the worktree state" (see the briefs in
   `docs/backlog/*.md` for the task definitions).
