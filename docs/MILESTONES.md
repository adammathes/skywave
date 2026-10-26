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
- `tools/placec`: bbox + era → Place skeleton from OSM/Overture + USGS elevation; style heuristics; era backfill; provenance header. First target: a real California beach town, compared side-by-side with the procedural one.
**Gate:** does the real place beat the generated one? What does the generator need to learn from it?

## M7+ — Breadth (each its own gate)
Scenarios `chicago_1977` and `north_shore_suburb_1986` (templates `great_lakes_city`, `north_shore_suburb`, rosters, presets); eras 90s/00s/10s/20s; more regions, then `tokyo_ward`; Steam build; cel/dither/CRT styles; destinations with directions; getting out of the car; YouTube source (web); mobile shells; photo-derived palettes for the compiler; threads for streaming.

## Gate notes
(appended after each gate)
