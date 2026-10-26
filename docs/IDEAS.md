# Ideas considered

A ledger of suggestions (from the owner, from other agents, from play) with a verdict, so nothing is re-debated and placeholders never stick. Verdicts: **now** (in the current milestone), **later** (milestone named), **already** (in the docs), **no** (with the reason).

## From a conversation about early-3D open-world rendering techniques (2026-10-26)

### Rendering and look
| Idea | Verdict | Notes |
|---|---|---|
| Ban modern techniques (no shadow maps, SSAO, TAA, physical bloom) to force the era look | **no** | The pillar is "2002 geometry, 2026 light": modern lighting on period geometry is the point. The era look comes from the geometry, textures, palette and post, not from crippling the lighting. Agents are told the opposite: keep fidelity in light, not in polygons. |
| Coronas: additive glow sprites on streetlights, headlights, tail lights, the sun, scaled by distance and faded when occluded | **later: M2** | The most evocative element of the era's night-driving look. Cheap: a depth-tested billboard pass after bloom. Added to the M2 backlog (night lighting). |
| Early-console "trails" filter: blurred previous frame blended back, tinted by time of day | **later: M7 style options** | As an optional post style next to cel/dither/CRT. Off by default. |
| Blob shadows under cars, baked prelit vertex colors | **no** | We have cascaded shadow maps; blob shadows would fight them. |
| Spherical environment map on car paint for cheap chrome and gloss | **later: M4 (roster)** | Yes. A tiny procedural sky/horizon env map, Fresnel-weighted, applied to paint and chrome. Big win for little cost. |
| Fog color matches the sky at the horizon so LOD pops dissolve into haze | **already** | `docs/RENDERING.md`: fog tinted by the sky toward the sun; the Daylight model sets fog color = horizon color. |
| Time-ranged objects: mesh/texture swaps by hour (lit windows, neon, signs on) | **already (emissive scaling); later (mesh swaps)** | Windows, lamps and neon scale with the Daylight model today. Hour-gated prop variants (shutters down, signs on) can ride the detailing pass in M2. |
| Small textures with real mipmaps, bilinear; no PS1 affine wobble or vertex snapping | **already** | `docs/RENDERING.md` style rules: 64–256 px procedural atlas, mips floored at 32 px, nothing PS1. |
| Weather as a state machine (clear, overcast, rain, fog) cross-fading the time-of-day tables | **later: M7** | `Daylight` already takes a haze/turbidity input; a `Weather` state machine over it is small. Rain itself (particles, wet roads) later. |

### World and life
| Idea | Verdict | Notes |
|---|---|---|
| Ambient traffic on a path-node graph, density by zone and hour, spawn out of view | **already: M4** | `docs/NPCS.md`. Hour-dependent density is added there. |
| Zone names popping up on entry; per-zone traffic density, ambience, radio reception | **later: M2 (names + ambience), M3 (reception)** | Named neighborhoods are part of what makes a place a place. Invented names from the region template; radio static when far from a station's transmitter is a nice M3 touch. |
| Separate low-poly collision meshes, never the render mesh | **already** | `world::WorldCollider` is built from Place data, not from meshes. |
| Game clock at one game minute per real second (a day in 24 min) | **setting; default stays 10×** | The classic open-world pacing is 60×. Ours defaults to 10× (a day in 2.4 h) because the drive is the game and golden hour should last. 60× is in the clock-speed options; the owner can flip the default at the feel gate. |

### Audio
| Idea | Verdict | Notes |
|---|---|---|
| Radio "live" from seed + absolute game clock so switching lands mid-track | **already** | `docs/AUDIO_RADIO.md`: seeded shuffle with resume math. |
| Station IDs, DJ links and fake ads between songs | **no** | The owner's rule: no made-up radio filler. Station idents stay visual. |
| Radio muffled and fading with distance when out of the car | **later: on-foot milestone** | |
| Engine from pitch-shifted loops by RPM, tire squeal on slip, per-zone per-hour ambience beds | **already (synthesized)** | We synthesize rather than ship loops (no audio assets). Ambience by zone and hour is in M3. |

### Driving
| Idea | Verdict | Notes |
|---|---|---|
| Low-poly interior, head lag and lean under lateral g, look-into-the-turn; mirrors skipped or low-res | **already (M1)** | Cockpit sway and lag exist; look-into-the-turn added to the camera tuning list for the feel gate. Mirrors deferred. |
| Handling as one plain-text line per vehicle so tuning needs no rebuild | **already (RON specs); later (hot reload)** | Specs are RON data. Hot reload of `assets/vehicles/*.ron` at runtime is a small M5 item. |
| Arcade forgiveness: auto-righting, generous grip, pivoting handbrake | **already** | In the sim. Strength is a feel-gate knob. |

### Process
| Idea | Verdict | Notes |
|---|---|---|
| World data as plain-text tables (IDE/IPL/timecyc spirit), diffable, converter output lands there | **already** | Place has a JSON encoding; templates, scenarios, specs and presets are RON. The place compiler emits Place files. |
| Fixed-timestep seeded simulation | **already** | 120 Hz, deterministic, hash-tested. |
| Golden-frame visual regression in a headless browser | **already** | `docs/TESTING.md`; goldens arrive with the M1 screenshot scenes. |
| Budgets as hard numbers treated as spec | **already** | `docs/ARCHITECTURE.md`, `docs/RENDERING.md`. |
| All names original, no era placeholders because placeholders stick | **already; strengthened** | `CLAUDE.md` forbids real brands and models; this adds: no real-world station call signs, place names or vehicle names as placeholders in procedural content either. |

## Performance follow-ups noted during M2
- Parcels: a face spanning several chunks is rebuilt by every chunk it touches (worst chunk ~7 ms vs the 2 ms target). Cache built faces in the plan, keyed by face id, so each chunk only slices.
- Streaming stages: buildings and light tiles on dense chunks exceeded the step budget; now built in ~2 ms pieces (done).
- Web chunk pool is 4.5M vertices (~189 MB of GPU buffers) to hold the densest 11×11 coast area with buildings. Reduce per-building vertices (blank unseen facades, LOD by distance) to bring it back under 128 MB.

## Not from that list, noted during M1
- Chase camera collision with buildings (push in): M2.
- Look-into-the-turn and a stronger speed-sensitive steering limit: feel gate.
- Street-lamp switch at civil dusk (currently they ramp with the window scale): M2 night lighting.
- Hot reload of vehicle specs: M5.
