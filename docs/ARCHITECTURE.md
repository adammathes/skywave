# Architecture

Read with `CLAUDE.md`. This document is the contract between crates and the rules every PR is judged against.

## Shape
A thin custom engine. Not a general engine: it renders exactly one kind of scene (a Place, vehicles, NPCs, sky) and runs exactly one kind of simulation.

```
                 ┌──────────────── platform-web / platform-native ────────────────┐
                 │ window, canvas, input events, clock, audio sink, file access,  │
                 │ gamepad, GPU surface. The ONLY crates that touch the OS/browser│
                 └───────────────┬───────────────────────────────┬───────────────┘
                                 │ InputState, real dt           │ frames
                          ┌──────▼──────┐                 ┌──────▼──────┐
                          │     app     │  SceneView ───▶ │   render    │ (wgpu behind a tiny RHI trait)
                          │ loop, cams, │                 └─────────────┘
                          │ HUD, radio  │                 ┌─────────────┐
                          │ state, menu │  AudioCmd ────▶ │    audio    │ (decode + mix; sink provided by platform)
                          └──┬───┬───┬──┘                 └─────────────┘
          no_std + alloc ─▶  │   │   │
          ┌──────────┐ ┌─────▼┐ ┌▼──────┐ ┌─────────┐
          │   core   │ │world │ │vehicle│ │ traffic │   pure simulation & generation
          │ hash,rng,│ │place │ │       │ │         │
          │ fixedstep│ └──────┘ └───────┘ └─────────┘
          └──────────┘
```

| Crate | std? | Owns | Must never |
|---|---|---|---|
| `core` (crate `skywave-core`; a crate named `core` would shadow Rust's) | no_std+alloc | `Seed`, `hash64`, `Rng`, `FixedStep`, fixed-capacity containers, color/palette math | allocate per frame in hot paths |
| `place` | no_std+alloc | the Place format (`docs/PLACES.md`): types, validation, chunk index, (de)serialization | know how a Place was produced |
| `world` | no_std+alloc | procedural producer and the detailing pass (`docs/WORLD_GENERATION.md`); produces chunk meshes as plain vertex arrays | touch the GPU, read files |
| `vehicle` | no_std+alloc | vehicle specs, raycast-car sim, parametric meshes (`docs/VEHICLES.md`) | depend on `world` |
| `traffic` | no_std+alloc | NPC cars and pedestrians over a Place's lane graph (`docs/NPCS.md`) | collide the player into harm |
| `render` | std | RHI trait + wgpu backend, passes, shaders, mesh/instance upload, HUD text (`docs/RENDERING.md`) | run game logic |
| `audio` | std | station model, decoding (symphonia native / Web Audio web), mixing, procedural engine sound (`docs/AUDIO_RADIO.md`) | block the main thread |
| `app` | std | game loop, cameras, input action mapping, settings, HUD/minimap/menu state, seed codes | call OS or browser APIs directly |
| `platform-*` | std | everything OS/browser-specific, as thin as possible | contain game logic |

Dependency direction is strictly downward in the diagram. `app` is the only crate that knows about more than one sibling.

## Frame
1. Platform collects events since last frame into `InputState` (digital actions + analog axes, already mapped through the user's bindings). Latest state wins; nothing is queued.
2. `app.update(input, real_dt)`: `FixedStep` yields N sim steps at **120 Hz** (clamped to 8 per frame; beyond that time slows rather than spiraling). Each step: vehicle sim → traffic → world streaming budget (below) → camera. Interpolation alpha returned.
3. `app.scene_view(alpha)` produces a plain `SceneView`: camera, sun/sky state, instance lists (chunk meshes, vehicles, NPCs, props), lights, HUD primitives. No GPU handles in it, only `MeshId`s.
4. `render.render(view)`. 5. `audio.tick(cmds)`.

Input latency target: input sampled at the start of the frame that renders it. No triple buffering by default; present mode FIFO/AutoVsync with 1 frame in flight.

## Streaming
The world is chunked (`chunk_size_m` = 128). `app::stream` keeps the chunks within a radius of the camera's chunk resident (4 on web → 9×9, 6 native → 13×13; `app::stream::StreamBudget`), nearest first and those ahead before those behind, and evicts chunks more than one chunk past the radius. A `ChunkSource` builds one chunk in five stages (ground, roads, buildings, props from `world::ChunkPlan::build_stage`, then its light tiles); each sim step runs stages until its budget is spent (2 ms web, 4 ms native, measured with the clock the platform passes to `App::frame`, checked before each stage, so a step overruns by at most one stage). Resident chunks live in a fixed pool of `(2r + 3)²` slots; finished chunks wait in a few staging buffers until `App::sync_chunks` writes them into the renderer's pooled vertex/index buffers (`render::ChunkStore`, `docs/RENDERING.md` budgets) and frees the evicted ones'. The streamer allocates nothing per step after warmup; the world builders allocate while a stage runs. Static lights are known before any chunk is built and shine while their chunk is drawn. `?stream=0` builds the whole place at load instead (the screenshot goldens use it). The collider and the minimap still cover the whole place. No threads in v1 (WASM threads need COOP/COEP headers and Safari caveats); the budget keeps the main thread smooth. Threads are a later optimization behind the same interface.

## Determinism
Same seed → bit-identical Place on every platform and in every build. Rules: integer hashing (`core::hash64`) for all per-chunk randomness; `Rng` seeded from `(seed, chunk, purpose)`; no `HashMap` iteration in generation; no `mul_add`/fast-math; f32 only with explicit rounding where results cross chunks; generation never reads sim state. Tests hash every chunk of reference seeds (`docs/TESTING.md`).

## Platform tiers
Web (primary; what CI tests) → **Steam Deck** (Linux x86_64 binary, Vulkan, 1280×800 @ 60, gamepad-only UI; runs as a non-Steam game first, Steam build later) → **TV via Deck dock** (1080p/4K output, UI scale, HUD safe areas). The same `platform-native` binary serves Deck and desktop Linux; Mac via Metal. Mobile shells later. Everything in `app` is written gamepad-first.

## Memory budgets (v1, per platform tier)
| | Web (default) | Native |
|---|---|---|
| Resident chunks | 9×9 (1.15 km square) | 13×13 |
| Chunk vertices | 384k per chunk max | same |
| Vehicle instances | 64 | 128 |
| Pedestrians | 256 | 512 |
| Point lights per frame | 256 | 1024 |
| WASM download (brotli) | < 4 MB | n/a |
| Time to first frame | < 1 s after download | < 0.5 s |
| Frame time | 16.6 ms at 1080p on Intel Iris Xe / Apple M1 | same at 1440p |

Budgets are constants in `app::budgets` and tested where measurable.

## Platform contract (`app::Platform` trait implemented per platform)
`now_seconds()`, `surface size`, `input bindings load/save`, `settings load/save` (JSON string through `app::SettingsStore`; localStorage on web, XDG config dir native; bindings live in the same document), `open_music_files()` (file picker / folder scan → list of `MusicSource`), `audio sink` (push f32 interleaved stereo at 48 kHz), `open_url` (for the sharing code), `log`. Nothing else.

## Settings and sharing
Settings are one JSON document, versioned, with defaults filled on load. Share codes: `sw1:<era>:<region>:<seed-hex>` plus optional `:t<time-of-day>` and `:v<vehicle-id>`; also encoded in the web URL query. Loading a code reproduces the place, time and car.

## Error policy
Generation and sim never panic on bad data: validation returns `Result`, invalid Places are rejected at load with a message. Rendering degrades (lower shadow resolution, fewer lights) rather than failing. Panics in WASM show an overlay with a copyable message.

## Portability hedge (ADR 0001)
Sim crates are plain-old-data and loops; `render` talks to a small RHI trait (`Rhi`: create buffer/texture/pipeline, begin pass, draw) with wgpu as the only backend for now. `platform-*` is the only place with OS code. This is what keeps a future C port or console backend bounded.
