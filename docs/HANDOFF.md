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
A live board of every task lives at https://claude.ai/artifact/17baWQ1hwt3R194YbzjpWS (private to
the owner); its `items` collection is the queue of record. **Proposed** cards are ideas and
follow-ups: nothing in Proposed is worked. **Queued** means the owner pressed Approve: work those
top first, without asking again. Only the owner moves a card from Proposed to Queued. New ideas,
follow-ups and review leftovers go in as Proposed. Read `items` where status is `queued`, ordered
by `order`, before dispatching; update status as sessions start, PRs open and merge.

## Work queue (how approved work runs now)
Open a Claude Code session on the repo and say "run the work queue" (or "do all the approved items
in the work queue in new subagents", "just do <card>", "stop at PRs", "land what's open", "what's
queued?"). The `work-queue` skill (`.claude/skills/work-queue/SKILL.md`) turns that into a run of
the saved workflow `.claude/workflows/work-queue.js`, which:
1. reads the Queued cards (never Proposed) and any queue PRs left open by an earlier run;
2. lands those PRs (merge on green, one fix attempt on red);
3. builds up to 3 Queued cards in parallel subagents, each in its own worktree on `q/<card id>`;
4. has each reviewed by an independent read-only reviewer (its own probes for sim, world
   generation, traffic or pedestrians) for up to 3 fix rounds, then moves stuck cards to Needs you;
5. opens the PR, waits for CI, merges on green, and updates the card at every step.
Screenshots run under `flock /tmp/skywave-screenshots.lock` so builders never render two at once.
Out-of-scope ideas from reviews become new Proposed cards. Scheduling it (a timed session that
says "run the work queue") is a later step if the owner wants it.

## Where things stand (keep current)

### Resume here (2026-10-28 22:40 UTC: stopping point on the owner's word)
Nothing is running. Since the previous coordinator stopped: #42 (the Deck), #44 (handling) and
#47 (its revert), #37 (parking lots), #45 (2001 traffic cars), #20 (map edges), #46 (hero cars
part 2), #48 and #49 (Mirage Valley), #50 and #52 (set pieces parts 1 and 2: static places, grade
crossings and a freight train), #53 (music folders in the native app) merged.
- **How work runs now:** the owner approves cards on the board (Proposed → Queued); a session told
  "run the work queue" works them (see "Work queue" above). No standing coordinator.
- **Queued (approved 22:10):** seven "game feel" cards: the landmark gallery first, then pilot
  landmarks, vista siting, quiet the filler, the roadside set, downtown anchors, dusk moments.
- **Needs the owner:** drive the reverted handling (then decide feel pass 3, board `feel-3`);
  try the native music folders on a Mac (PR #53's body has the steps); the Deck `--stats` runs.
- **Handling:** #44 reverted by #47 on the owner's word (the tail snapped). Feel pass 3, if
  approved, fixes causes rather than adding a stronger yaw hand: keyboard steering ramp and
  speed-sensitive lock, a more progressive tire tail in the assisted presets, gentler stability
  that does not fight the counter-steer assist.
- **Set pieces part 3** (race line, truck convoy) is Proposed. Moving set pieces must guarantee
  zero violence themselves (NPC cars do not see people in general); review them with probes.
- **Paused with WIP:** `m4/npc-sees-peds` (156d224): cars stop for people in `traffic`, 122 → 0
  person-steps inside NPC cars in a scratch run; app wiring, sweeps, re-pins, goldens and docs
  remain. Resume from the board when the owner says; since #45 NPC cars have their own lengths (a
  17.5 m semi, a 7.4 m box truck), so seeing people must use each car's length, not 4.6 m.
- Everything else is deferred on the board. Costs so far: coordinator $1,035 (the expensive
  part: a 400k-token context woken 100+ times a day), builders $10–90 each.

- Done on `main` (2026-10-27 23:50 UTC): M0; M1; M2; M3 part 1 (synthesized sound; web radio with
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
  ads); the M8 spec for the second world, Mirage Valley 2001 (`docs/backlog/MIRAGE2001.md`); the 1980s sports cars
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
  beams, auto headlights and tell-tales (#29); native radio playback of the station streams (#15:
  rustls+ring and nanomp3-core, ADR 0006 accepted on the owner's word; 13 of 16 stations play
  natively, two are AAC-only and one server lacks the cipher suites rustls ships); pedestrian
  follow-ups (#36: 0.15 m body gap, 3,382-run crossing sweep at zero, dodge episodes capped at
  3 m, people clear a stopped car, the turn-curve cover at 0.55 MiB); scenario switching and the
  era filter (#35, M8 #1: `VehicleSpec::year`, scenario `years`/`heroes`, integer era weights per
  roster car and per body shape, the Worlds page in the pause menu with raw `.still` atlases,
  `?scenario=` / `--scenario`, music and panel rate carried across a world switch; the second
  world is a stub on the coast's template until M8 #2 lands); the player's own MP3 folders as
  stations in the native build (#39: `--music DIR`, else `music` beside the executable, else
  ~/Music; one station per folder, at most 64 added four a frame; removals stick through
  `music-ignored.txt`; every track ends through a silent prebuffer tail); the controls card
  (#41: keyboard and pad columns from the live bindings, 6 s at start for three sessions, H / F1
  / Start held / Show controls in the pause menu, modal over the road like the map; the horn moved
  from H to B); overlay modes (#40: F3 cycles Off / FPS / Full, saved on Settings › HUD; menus,
  the map, the card and an unfocused window hide it; the FPS number drops under a station toast);
  player–NPC car collisions without damage (#38: impulses by mass, restitution 0.2; the hit car
  brakes, holds, steers back; its shove is capped to the road's measured room, 0.5 m side and
  1.5 m back; head-on stops both; hits on grades stay level; probes at zero sidewalk reach and
  zero people inside); the 2001 hero cars part 1 (#43: Halberd Coupe and Targa, Tanager AWD,
  Destrier Coupe, Siskin Coupe; body kit, blower, targa, tall wing, twin rims; underglow on U;
  `VehicleSpec::built`; the basin is years 1995–2001 with these as heroes); the Deck (#42: one
  library from the folder layout and tags, TAPE / CD / MP3 by the world's year, Normal / Shuffle /
  Continuous, readout top right, Back held + d-pad on the pad, resumes next session); handling feel (#44, NOTES 17: stronger yaw damping in Arcade and Light,
  exotics' oversteer bias halved, no spins in Arcade or Light on any car, Off stays lively, the
  Destrier tuned into the bands, the Halberds lively with no assists; reverted by #47); parking lots (#37, B10: front, alley and beach lots from the
  grammar, parked cars by the hour and held near the player, golden `coast-beach-lot-noon`); natural map edges (#20: a sea horizon, walls of hills and
  mountains, a highway that loops back; every coast fixture re-sited; the edge pass kept inside
  the macro-map budget); the 2001 traffic cars (#45, M8 #6: Plover sedan, Sorrel utility,
  Wren minivan, Percheron crew cab, Linnet hatch, Sledge box truck, Anvil semi; weights by zone
  for traffic and parking; the box truck and semi never park, the semi keeps to freeways; per-car
  length in following and junction room; none in 1984; fixed extra axles in the shape DSL); the 2001 hero cars part 2 (#46, M8 #5: Shrike Coupe,
  Glaive Turbo, Falchion Sedan, Courser V8, the basin's heroes); Mirage Valley (#48 and
  #49, M8 #2 and #3: the 2001 world renamed from Inland Basin, the `socal_basin` region template,
  the 2001 grammar, lexicon and `sodium_night` palette; descriptor + trade pairs that name real
  chains blocked by a hashed list; storefront fascia signs lettered from the grammar's lexicon);
  the Deck's "Skywave
  Update" launcher written by `tools/install-deck.sh` (a08324f).
- Open PRs: none; building: set pieces (see "Resume here"). Paused: NPC cars see
  pedestrians (`m4/npc-sees-peds`). The owner's board (above) is the queue of record for what
  comes next.
- Owner feedback not yet acted on: mobile (NOTES #5, after Deck verification); the owner's Deck
  `--stats` output; NOTES 4(a) beyond traffic and pedestrians (window lights, parked cars that
  come and go); buildings B7, B11-B15.
- M4 gate facts for the owner (from #25): NPC cars yield at junctions; player–NPC collision
  (`m4/car-collision`, NOTES 14: reversing into a queue stops against it, the gate test is on); NPC cars do not see
  pedestrians; static wheels, no brake lights; traffic sparse near the start.
- Mirage Valley's world shows "coming" on the World page until its radio preset (M8 #8) lands.
- Next: the owner's drive of the reverted handling; set pieces (M8 #7, three PRs); NPC cars see
  pedestrians; music folders in the Mac app; M8 #8 (the 2001 radio preset); B7 facade sign bands on the
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
