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

### Resume here (2026-10-28 00:15 UTC, coordinator handed off to save quota)
The previous coordinator stopped mid-wave. Five builder sessions are still finishing; each
reports its state in its PR body and expects nothing but a merge. Keep at most two or three
threads alive, no timed check-ins, probe reviewers only for sim or zero-violence code; small UI
and docs PRs merge on the builder's verification plus green CI. Merge with a merge commit via
the GitHub tool, `expectedHeadSha` set to the full head. After each merge tell the other open
sessions main moved (`send_message`, Claude Code Remote), update this section and the board.
- **#44 handling feel** (`vehicle/handling-feel`, session_0121vThANMtZPyLXazaRBJXm): review
  done; last ask: tune the Destrier into the Arcade and Light bands instead of widening its band,
  put the two Halberds in `LIVELY` for the no-assist launch, document the 2.9× Light band; merge
  on green. The owner should drive this one first.
- **#37 parking lots** (`bldg/parking-lots`, session_0121vThBfSpYpVxsB7L5B2nD): fixes pushed
  (3fe9cf8, verifying); last asks: the hold covers cars beside a parked player within 60 m in any
  direction and 150 m / 90° beyond; beach stalls off park and empty parcels; only the failing
  stalls dropped at the golden's bend (look at the regenerated golden: both rows must be there);
  merge on green. It re-pins place hashes and a golden, so #20 refreshes after it.
- **#42 the Deck** (`m3/deck`, session_0121vThB5ZsVvpTV5D5q8345): fixes pushed (fe2b6aa,
  verifying): partly tagged albums, one library rebuild per batch, album keys no longer silence
  the radio, duplicate picks; merging #40/#41/#43 with controls-card rows for the Deck actions;
  merge on green after a look at the diff.
- **#20 map edges** (`m2/map-edges`, session_0121vThBxy2WQvcKQzuoJpnE, open since 04:38, $88):
  goldens accepted; the native junction gate is being made to hold the world still while
  streaming, gates re-measured in their own commit, junction goldens re-sited to 737.1,464.8
  phase=1440 (fallback 945.7,375.6); merge on green after a look at the two junction goldens.
  It unblocks the basin region template (board `m8-basin`).
- **2001 traffic cars** (`m8/traffic-2001`, session_0121vThDbC8noH1ttGK4FL3c, M8 #6): seven
  shapes built, tuning; opens a PR; a short review (names, era filter, budgets) then merge.
- **Paused with WIP:** `m4/npc-sees-peds` (156d224): cars stop for people in `traffic`, 122 → 0
  person-steps inside NPC cars in a scratch run; app wiring, sweeps, re-pins, goldens and docs
  remain. Resume from the board when the owner says.
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
  `VehicleSpec::built`; the basin is years 1995–2001 with these as heroes); the Deck's "Skywave
  Update" launcher written by `tools/install-deck.sh` (a08324f).
- Open PRs: natural map edges (#20; goldens re-sited and accepted; the native junction draw gate
  measured runner speed, so the session holds the world still while streaming, re-measures the
  gates and sites the junction goldens in budget; merges on green after a look); parking lots
  (#37; fixes verified except the hold for cars beside a parked player, beach stalls on park and
  empty parcels, and a bend fix that removed a whole row at the golden's bend); the Deck (#42;
  review sent: a partly tagged album splits, native rebuilds the library per folder, album keys
  silence the radio, duplicate picks on Firefox/Safari; plus controls-card rows after #41).
  Sessions building: handling feel (`vehicle/handling-feel`, NOTES 17); NPC cars see
  pedestrians (`m4/npc-sees-peds`, the next zero-violence item). The owner's board (above) is
  the queue of record for what comes next.
- Owner feedback not yet acted on: mobile (NOTES #5, after Deck verification); the owner's Deck
  `--stats` output; NOTES 4(a) beyond traffic and pedestrians (window lights, parked cars that
  come and go); buildings B7, B10-B15.
- M4 gate facts for the owner (from #25): NPC cars yield at junctions; player–NPC collision
  (`m4/car-collision`, NOTES 14: reversing into a queue stops against it, the gate test is on); NPC cars do not see
  pedestrians; static wheels, no brake lights; traffic sparse near the start.
- Next: merge #20, #37, #42; NPC cars see pedestrians (board order 15, after #38); M8 waves
  (region template `socal_basin` and the 2001 grammar after #20, hero cars part 2, traffic cars,
  set pieces, the 2001 preset); B7 facade sign bands on the
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
