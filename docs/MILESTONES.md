# Milestones and review gates

Work inside a milestone is autonomous and test-driven. **Gates** are where a human plays the build (from the GitHub Pages deployment of `main`) and decides whether the feel is right before more is built on it. Gate notes are appended here.

Vertical slice first: **1980s California beach town, one sports car and one wagon, yacht rock.** Then the Chicago scenarios (`docs/SCENARIOS.md`).

Fidelity bar from the M0 review: early-2000s console density, not flat boxes (`docs/RENDERING.md`, "The fidelity bar"). Platform order: web → Steam Deck → TV via dock.

## M0 — Foundation (this session)
Design docs, BSD license, agent conventions, Cargo workspace skeleton compiling natively and to WASM, fixed-step loop, lit placeholder scene (ground + boxes + sun shadow + sky), headless screenshot tool, CI, Pages deploy.
**Gate:** docs approved; placeholder scene loads from the Pages URL in your browser.

## M1 — Drive
- `vehicle`: raycast-car sim per `docs/VEHICLES.md` with two specs (`corsair_gtx`, `palisade_wagon`), assists, auto gearbox.
- `vehicle::shape`: parametric body + interior for those two, `tools/shape-preview`.
- `place::test_grid` → a hand-laid flat test town with curbs, sidewalks, crosswalks, lamp posts, trees, facade geometry on buildings, parked-car placeholders; collision.
- `render`: CSM shadows, analytic sky, fog, bloom, tonemap, cockpit + chase cameras, HUD speed/gear.
- `app::input`: action mapping, keyboard ramps, gamepad (native + web), per-axis invert/sensitivity/curve (invert Y mandatory), remap data model **and a gamepad-navigable bindings/settings menu**.
- Fixed-step loop with interpolation; frame stats overlay.
**Gate (feel):** does driving feel immediate and fun on your machine, keyboard and controller (with invert Y)? Do the two cars feel clearly different? Is first person with the dashboard pleasant? Does the test town already look like a place and not a diagram?

## M2 — Beach town
- `world`: procedural producer per `docs/WORLD_GENERATION.md` for `california_coast` (coast, strip, downtown, residential, suburb, highway through hills to farmland), detailing pass (facades, props, lights, parked cars), chunk meshes, streaming with budget, golden hashes.
- `place`: full format, codec, validation, chunk index.
- Minimap + full map (road graph rendered to a texture; player arrow; zone tint).
- Time of day: `Daylight` model, sun/moon, streetlights and windows at night, 10× clock, scrub control.
- Collision with buildings/curbs/props/water.
**Gate (vibe):** does it feel like a real place at sunset? Can you learn your way around? Any hitches while streaming?

## M3 — Radio
- `audio`: mixer, station model, files (native + web) and streams, ICY titles, seeded "already playing" resume, dash radio display, station toasts, synthesized engine/wind/tire/ambience, ducking.
- Era preset `1980s.ron` with a few real public stations (URLs only) and a yacht-rock folder hint.
**Gate:** yacht rock at golden hour on the strip. Does switching stations feel like a car radio? Does the engine sound sit right under the music?

## M4 — Life
- `traffic`: cars and pedestrians per `docs/NPCS.md`, density settings, non-violence scenarios.
- Full 80s roster (10 cars) with distinct feel; vehicle select screen (parametric previews).
**Gate:** does the town feel alive but calm? Do all ten cars feel different and era-right? Is the zero-violence behavior convincing and never creepy?

## M5 — v0.1 release
- Settings UI (controls remap, video, audio, world), seed/share codes in URL, persistence.
- Performance pass against budgets on Intel Iris Xe and Apple M1 in Chrome/Firefox/Safari; WASM size budget.
- Native Linux (Steam Deck-ready tarball with a launcher script and a gamepad-only path verified at 1280×800) and Mac binaries via GitHub Releases; TV mode (UI scale, safe areas) checked at 1080p/4K; README with screenshots; Pages landing page.
**Gate (ship):** would you send the link to a friend?

## M6 — Place compiler v1
- `tools/placec`: bbox + era → Place skeleton from public-domain sources (TIGER roads, USGS 3DEP elevation, FEMA/city footprints; `docs/adr/0005-reference-sources.md`); style heuristics; era backfill; provenance header. First target: a real California beach town, compared side-by-side with the procedural one.
**Gate:** does the real place beat the generated one? What does the generator need to learn from it?

## M7+ — Breadth (each its own gate)
Scenarios `chicago_1977` and `north_shore_suburb_1986` (templates `great_lakes_city`, `north_shore_suburb`, rosters, presets); eras 90s/00s/10s/20s; more regions, then `tokyo_ward`; Steam build; cel/dither/CRT styles; destinations with directions; getting out of the car; YouTube source (web); mobile shells; photo-derived palettes for the compiler; threads for streaming.

## Gate notes
(appended after each gate)

### M1 status (2026-10-26, autonomous run)
Everything in the M1 scope is merged on `main`: raycast-car sim with two
cars, parametric car meshes (exterior + cockpit), detailed test town with atlas patterns, cascaded
shadows, Preetham sky, bloom, Daylight model, world collision, cockpit/hood/chase cameras, HUD,
gamepad + mouse on web and native, remappable bindings with invert Y, gamepad-navigable settings
menu, five golden screenshot scenes with a diff tool, and a wasm-vs-native self-test. 356 tests.
**Waiting on the human feel gate**: play the web build (keyboard and controller, invert Y on),
judge steering at speed, assist strength, chase lag, cockpit sway, FOV, night brightness, and
whether the two cars feel different. Known gaps carried to M2: chase camera clips into buildings;
parked cars are placeholder boxes; `strip-dusk-near` headlight pool blends with the sodium lamps.

### M1 tuning pass (agent)
Before the M1 feel gate. Tuned from numbers, tests and screenshots only; nobody has driven it.
Every constant below lives next to its rationale in the code.

**Night and cabin lighting** (`render`, `app::lighting`, `app::drive`, `app::lib`)
- Renderer: per-instance `glow` (emissive multiplier) and `cabin` share; `Sky::cabin_light`, an
  ambient added to the car's interior, full on up-facing surfaces and 10% on down-facing ones;
  `Sky::emissive_cap`, a soft post-exposure ceiling on emissives (fourth-power knee, hue kept);
  spot lights on `PointLight` (`direction`, `cone_cos`, `cone_squash` for a flat beam, `wrap`).
- Cabin light, post-exposure: day 1.6 neutral, night 0.9 warm (was nothing: the sky hemisphere
  only). Cabin walls (the shell inside out, in body paint) are tinted ×0.3.
- Emissive cap 5 (5× the bloom threshold). Car lamps peak at 1.5 after exposure at night
  (tails a saturated red, brakes near the cap); instruments at 0.6 (radio display ≈ 0.9,
  needles ≈ 2, dial faces a dim green backlight). Lit windows 0.125 (were 1.0, every window
  at the cap at night). Night sky floor 2.4e-4 kcd/m² (deep blue, was 2e-5, black).
- Street lamps: own mesh, switched by the civil-dusk photocell (`Daylight::streetlights_on`),
  not the window ramp. The 6 nearest a point 12 m down the view become sodium spots pointing
  down: 2 kcd, 22 m reach, full to 45° off vertical and cut off at 72°, fading in and out over
  30% of the distance to the first lamp left out, so no popping. Night EV stays 3.
- Headlights: two spots from the lamps (were a 0.08 kcd point light 10 m ahead): 14 kcd each,
  pitched 2° down, full within 10° across and 2.5° vertically, gone at 32° / 8°, wrap 0.25.
  The pool runs from ~5 m past the bumper to ~25 m (a fifth as bright there as at 10 m).

**Gauges** (`vehicle::shape::interior`, shape RON). The cluster face sat inside the dash
below its top surface, so the dash hid the lower half of the dials, and the wheel rim covered
the rest. That was not fixable from the RON alone, so one line in `interior.rs` moves the face
behind the dash lip (`dash_x + 0.025`, was `- 0.10`). The wheels sit lower (corsair
`wheel_pos.y` 0.64 → 0.59, palisade 0.80 → 0.74), so the rim passes under the sight line.

**Cameras** (`app::camera`). Vertical FOV: cockpit 75° → 58°, hood 70° → 55°, chase 65° → 50°.
Chase distance/height near 4.2/1.5 → 5.0/1.65 m, mid 6.0/2.1 → 7.0/2.3, far 9.0/2.9 →
10.5/3.25 (the car's width stays ≈ 20% of the frame from mid). Chase yaw spring 4 → 6.5 rad/s;
lead into the turn up to 8° at full stick, faded in from 4 to 15 m/s. In the test corner the
camera sits 2.4° off the car's heading, against 6.1° without the lead (≈ 11° at the old 4 rad/s).

**Sim** (`vehicle::sim`). Grip-limited steering, the new `Assists::steering`: 1 in the default
"light" assists, 0 in "off". Above 8–11 m/s (faded in) the road-wheel target is the front
axle's direction of travel plus stick × the tire's peak slip angle, within the old
speed-sensitive lock. In a steady turn that is `atan(a·L/v²) + α_peak` with the car's real
lateral acceleration, and in a slide it counter-steers. A fixed `atan(1.25 g·L/v²) + margin`
version was tried first: it spun the corsair under full throttle at full lock, because fronts
that grip expose its light rear. Reverse is capped at 8 m/s (was ~20).

| Test number | Before | After |
|---|---|---|
| Full-stick target at 30 m/s, corsair / palisade | 22.4° / 22.5° | 7.0° / 9.5° (straight) |
| Full stick from 25 m/s, corsair: lateral g, front slip ÷ peak | 0.76 g, 3.8 | 1.15 g, 1.0 |
| Full stick from 25 m/s, palisade | 0.54 g, 2.8 | 0.81 g, 1.0 |
| Full throttle + full lock on grass (μ 0.6), corsair from 14 m/s | spins | holds, 0.55 g |
| Reverse top speed, corsair / palisade | ~20 m/s | 7.96 / 7.91 m/s |
| 0–60 mph, corsair / palisade (bands 5–8, 10–16 s) | 5.83 / 12.67 s | unchanged |
| Top speed | 69.6 / 49.3 m/s | unchanged |
| Skidpad (assists off; corsair > palisade) | 1.075 / 0.764 g | unchanged |
| Full lock at 30 mph, assists light, peak g | 1.06 / 0.86 g | 1.16 / 0.83 g, no roll |

Pinned hashes moved deliberately in their own commit: vehicle replay, world curb drive
(corsair), app scripted drive, shape mesh hashes, `selftest.json`.

**For the human at the feel gate**
- Steering at speed: does full stick at 25–30 m/s turn hard enough and feel planted, or too
  "on rails"? Does the counter-steer in slides feel helpful or like the car steers itself? Try
  assists off (the old speed-sensitive lock) for comparison. Keyboard players hold full lock,
  and that case is now grip-limited.
- The corsair still power-slides with full throttle mid-corner (rear slip ~4–6× peak, caught).
  Is that fun or too loose? The knobs are `STEER_LIMIT_MARGIN` and the stability assist.
- Camera lag: is the chase camera (6.5 rad/s, 8° lead) too stiff, or the lead too much on
  quick left-right flicks? Is the 50° chase FOV too tight at speed?
- Cockpit: is 58° comfortable on your screen? Is the head sway (unchanged: 3 cm/g lateral,
  4 cm/g longitudinal, 6 cm cap, 0.12 s lag) right? Are the gauges readable while driving?
- Night: is the dash bright enough to read but dark enough to feel like night (cabin night
  0.9)? Do the headlights reach far enough? Do the sodium pools and neon sit right against the
  deep blue sky, or should the night exposure (EV 3) come down?

### Feel pass 2 (after first play)
The owner's first play of the web build on real hardware: "a little bit too slippy/sloppy ...
a little challenging. Braking also seems too slow/sloppy, maybe we want it to feel tighter";
"hard to see out of the cockpits ... maybe an option to change FOV or move it a bit?"; and the
camera mode was not remembered between sessions.

**Handling** (`vehicle::sim`, specs, `app::input`). New `Assists::ARCADE`, the default:
stability 0.8 (was 0.3), traction 0.8 (0.5), ABS, grip-limited steering, `grip_boost` 1.2
(tire grip, both directions) and `brake_boost` 1.4 (service brakes). `LIGHT` and `OFF` are
kept and offered on the new Drive page (Handling: Arcade / Light / Off, saved). The stability
assist now also damps a sideways slide past the tires' peak slip angle (`SLIDE_GAIN` 4/s) and
lets go while the handbrake is held; the steering assist then helps yaw toward
`steer × 1.6 rad/s` (`HANDBRAKE_YAW_*`), so the handbrake pivots the car. Engine braking is a
drag at the driven wheels through any gear, 0.13 g of the car's weight at idle rpm to 0.22 g
at the redline (was 0.3 × peak torque at the redline, at the crank). Steering `return_rate`
corsair 9 → 12, palisade 3.5 → 5; keyboard steer ramp 150 → 120 ms out (300 ms back as
before). `brakes.max_nm` is unchanged: the brake boost and engine braking already put the
arcade stops under the ~38 m / ~45 m targets, and 3500 / 4200 N·m was tried and gave 34.7 m /
41.7 m.

| Test number (default assists unless named) | Before (light default) | After (arcade default) |
|---|---|---|
| 60–0 mph, corsair / palisade | 47.6 / 56.4 m | 36.2 / 43.3 m (asserted ≤ 38.5 / 45.5) |
| 60–0 mph with light assists | 47.6 / 56.4 m | 45.3 / 55.2 m (asserted ≤ before) |
| 60–0 mph assists off, ABS / none | 47.6 / 56.4, 47.6 / 58.2 m | 45.3 / 55.2, 48.9 / 58.1 m |
| Lift-off at 25 m/s, mean decel over 2 s | 0.065 / 0.043 g | 0.160 / 0.141 g |
| Handbrake + half stick, 1 s from 15 m/s | 42° / 21° | 92° / 59° (off: 82° / 42°) |
| Launch rear slip ratio (wheelspin) | 0.433 / 0.072 | 0.062 / 0.058 |
| Full lock at 30 mph, peak lateral g | 1.16 / 0.83 g | 1.36 / 0.97 g, no roll |
| 0–60 mph (bands 5–8, 10–16 s) | 5.83 / 12.67 s | 5.60 / 12.60 s |
| Top speed | 69.6 / 49.3 m/s | 69.6 / 49.4 m/s |
| Skidpad (assists off) | 1.075 / 0.764 g | 1.075 / 0.763 g |
| Road-wheel chase time constant | 0.11 / 0.29 s | 0.08 / 0.20 s |

**Cockpit** (`app::camera`, `app::settings`, `app::ui`). Cockpit FOV 58° → 62°; hood (55°) and
chase (50°) FOVs are separate and unchanged. The eye sits 5 cm higher and 3 cm further forward
than the shape's seat (`cockpit_eye_up_m`, `cockpit_eye_forward_m`). A ray test against the
cabin meshes (`the_road_shows_from_six_metres_ahead_in_the_cockpit`) measures where the road
first shows across the lane ahead of the eye: corsair more than 22 m → 6.45 m (3 cm up and
forward gave only 9.5 m, so the eye went up 5); palisade 20.9 m → 10.9 m, held there by its long
flat hood (hood only: 10.0 m). New Camera page, all saved: Camera (the selected camera, also
updated and saved whenever the camera is cycled; the game starts in it, a scene's `cam=` still
wins), Cockpit FOV 50–80°, Seat height −5…+10 cm, Seat forward −10…+10 cm, Dashboard (full
cockpit, or hood only: the cabin walls and windshield frame without the dash, wheel and
gauges), Camera shake (the cockpit head sway).

Pinned hashes moved deliberately in their own commits: vehicle replay, world curb drive, app
scripted drive, `selftest.json`; the cockpit screenshot goldens were regenerated after review.

**For the human at the next play**
- Is Arcade now tight enough, or too grippy ("on rails")? Light is the old default for
  comparison. Knobs: `Assists::ARCADE`, `SLIDE_GAIN`.
- Brakes: do ~36 m stops (corsair) feel firm without being abrupt? Lifting off now slows at
  ~0.15 g: does coasting feel natural or draggy? Knob: `ENGINE_BRAKE_G`.
- Handbrake: does a flick with the stick pivot the car the way you expect?
- Cockpit: is the default 62° / +5 cm right on your screen, and does the hood-only view help?
