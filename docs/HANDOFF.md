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

## The owner's board
A live board of every task (needs the owner / in review / building / queued / merged) lives at
https://claude.ai/artifact/17baWQ1hwt3R194YbzjpWS (private to the owner). Its `items` collection
is the queue of record: the owner reorders the Queued column and leaves notes on cards; the
coordinator reads it (`ArtifactData` query on `items`, status `queued`, ordered by `order`) before
dispatching the next session, and updates statuses as PRs open and merge.

## Where things stand (keep current)
- Done on `main` (2026-10-27 20:10 UTC): M0; M1; M2; M3 part 1 (synthesized sound; web radio with
  ad-free stations per era (the aggregator presets were dropped for their pre-roll ads) and NO SIGNAL auto-skip; car reset key; dial-sweep tune sound); feel
  pass 2; convertibles; WebGPU→WebGL2 fallback and the Safari shader fix; GPU performance pass;
  vehicle picker and credit line; art pass; the twelve-car 1980s roster (M4 #2); light collision
  physics; the hill-launch fix (wheel rays meet the ground's tangent plane); traffic lane graph
  (M4 #1), NPC car movement (M4 #4), pedestrian figure (M4 #3) and behaviour (M4 #5) in
  `crates/traffic`; NPC traffic drawn in the game with Settings › World › Traffic and passing-car
  sound (M4 #6); vehicle select screen with turntable preview and paint picker (M4 #7); reference
  sources ADR 0005; facade grammar loader (B1, B2); three-band facades with real storefronts (B4);
  sign pictograms, stroke letterforms and the closed lexicon on blade signs (B5, B6); chunk pool
  raised to 1211 web / 1617 native pages; CI gates stabilised; the M4 gate (#25: non-violence
  scenarios, density tests, two gate goldens, gate notes in `docs/backlog/M4.md`); pedestrians
  drawn in the game (#27: crowd beside the traffic, 8 figures × 13 parts instanced, Traffic
  setting drives both, golden `coast-peds-noon`); awnings, canopies, rooflines and rooftop units
  (#26, B8 and B9; motels keep the parapet, rooftop items stay on the roof); pedestrian hardening
  (#24: four review rounds, 2040-run crossing sweep at zero reached, 0.4 m step-aside clearance);
  the radio starts off every session (ce560a3) and the 181.FM presets are gone (7ca473c, pre-roll
  ads); the M8 spec for the second world, Inland Basin 2001 (`docs/backlog/INLAND2001.md`); the 1980s sports cars
  and their open versions (#28: 20 cars, exotics player-only, `TRAFFIC_SHAPES`); ad-free radio
  presets (#30: 7 / 6 / 6 listener-supported, public and college stations, each cold-start
  checked; the checker gained --listen/--save/--url and ad-sign warnings); the rename of the
  two cars whose names were real models (#33: `gannet_gt`, `homestead_wagon`; old ids migrate);
  GPU time per pass in `--stats` and F3, sky drawn last, a Deck-only depth pre-pass
  (`--no-prepass` to A/B), kept shadow cascades (#32; the owner's Deck `--stats` runs with and
  without the pre-pass decide whether it stays on); the player's own music folders as stations on
  the web (#34: folder picker and drag-and-drop, one station per pick, tags on the dash, seeded
  shuffle with resume, remembered handles, silent at start; native follows #15); draw only what
  changed (#31: World/Ui/Skip frames, GPU near idle when paused, in menus or unfocused; a
  frame-rate cap of whole panel divisors, default the panel rate; Battery = half rate at 0.85
  scale; the game pauses when unfocused); brake lamps that read in the sun, NPC brake lamps, high
  beams, auto headlights and tell-tales (#29).
- Open PRs: native radio (#15, approved; the owner signed off on 2026-10-27 19:55 by asking for
  stations to work locally; its session is refreshing it on main, then native music folders follow);
  natural map edges (#20, approved; refreshing on main after #24, then merges on green);
  Sessions building: parking lots
  (`bldg/parking-lots`, B10); era sports cars with open-top variants
  (`m4/sports-cars`, NOTES 7); NPC brake lights and headlight controls (`m4/lamps`, NOTES 8);
  ; idle frames, frame cap, Battery preset
 pedestrian follow-ups (`m4/peds-followups`: body-width gap in
  the sweep, public height, turn curves in the lane cover, chained dodges, standing in the
  road); scenario switching and the era filter (`m8/scenarios`, M8 #1); the rename of
  two roster cars that shared real model names (#33, now the Gannet GT and the Homestead Wagon). PRs in review:
  idle frames and the frame cap
  
- Owner feedback not yet acted on: mobile (NOTES #5, after Deck verification); the owner's Deck
  `--stats` output; NOTES 4(a) beyond traffic and pedestrians (window lights, parked cars that
  come and go); buildings B7, B10-B15.
- M4 gate facts for the owner (from #25): NPC cars yield at junctions; no player–NPC collision
  yet (reversing into a queue passes through; ignored test with numbers); NPC cars do not see
  pedestrians; static wheels, no brake lights; traffic sparse near the start.
- Next: merge #20, #35; M8 waves (region template `socal_basin` and the 2001 grammar after #20,
  hero cars after #28, traffic cars, set pieces, the 2001 preset); B7 facade sign bands on the
  lexicon; player–NPC collision; M5 (release polish, Deck verification); M6 place compiler.
- Conventions learned this run: reviews are read-only Opus subagents over `git diff
  origin/main...origin/<branch>` with probe tests on an exported copy, and for zero-violence code
  the probe is re-run on every round until it counts zero; a PR that regenerates goldens merges
  before PRs touching the same scenes, and after each such merge every open PR refreshes from
  main; CI's pull_request event sometimes does not fire for a session-created PR, so a branch
  update from main (or any push) is used to start it; two PRs that change the same signature
  within an hour can both be green and still break main.

## Pitfalls met so far
- Software-rendered screenshots take minutes per scene; never run several at once on one box.
- Never share a cargo target dir across worktrees. Disk fills; `cargo clean --release` often.
- Wall-clock budget tests are noisy on shared CI runners; ceilings carry twofold slack.
- GitHub release downloads are blocked in cloud sessions; install tools with cargo or npm.
