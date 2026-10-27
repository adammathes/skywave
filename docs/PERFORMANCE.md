# Performance

Read with `docs/RENDERING.md` (the pipeline) and `docs/ARCHITECTURE.md` (budgets). This is
how a frame spends the GPU, what each quality preset buys, how to measure on a real
machine, and what is still unverified.

The reference machine is the **Steam Deck**: RDNA2 APU (8 CUs, ~1.6 TFLOPS, 88 GB/s shared
LPDDR5), 1280×800, and a **90 Hz panel on the OLED model** (60 Hz on the LCD one). With vsync on
a 90 Hz panel a frame must finish in **11.1 ms** or it waits for the next refresh (and
the rate falls toward 45 or 60 Hz, depending on the compositor). The first report from a
Deck: ~65 fps with the GPU at 99%, so GPU bound at roughly 15 ms a frame.

## Cost model

Per frame, in order (`crates/render/src/lib.rs`):

| Pass | Work | Scales with |
|---|---|---|
| Shadow | depth-only, every caster once per cascade (3 cascades, `size²` each, side by side in one `Depth32Float` atlas) | casters × cascades (vertex), `size²` × cascades (depth fill and clear) |
| Forward | sky (one fullscreen triangle) then every visible instance: atlas texel, Lambert sun with 3×3 PCF (twice in the cascade blend band), hemisphere ambient, up to 8 point/spot lights, fog, emissive | visible triangles (vertex), pixels × overdraw × lights (fragment) |
| Coronas | one instanced quad per light with a corona (≤ 258), 5 depth taps each in the vertex shader | lights at night, sprite size on screen |
| Bloom | prefilter (13 taps) into half the scene's size, then down and up a chain of 4–5 levels | scene pixels / 4, levels |
| Tonemap | one fullscreen triangle: exposure, bloom, AgX, vignette, dither, upscale when scaled | swapchain pixels |
| HUD | one instanced draw of ≤ 4096 quads | quads |

Where the time goes at 1280×800, reasoned from the pipeline and the CPU-side counts (no
GPU here to measure on; `--stats` with timestamps will replace this with numbers):

- **Geometry dominated the shadow pass.** Before culling every instance (every resident chunk
  tile of the 13×13 ring, every prop, the car, the far terrain) was drawn four times: three
  cascades and the forward pass. Most of it cannot be seen: the camera sees about a quarter
  of the ring, the near cascade a few tiles, the middle one a tenth of the ring. The numbers
  for the coast are in "Measured on the CPU" below.
- **Shadow fill.** Three 2048² cascades are 12.6 M depth texels cleared and written per frame
  (50 MB of `Depth32Float`). 1536² is 44% less; two cascades a third less again.
- **Forward fragments.** About 1 M pixels × overdraw, each with the PCF kernel and the light
  loop. Resolution scale cuts this linearly with the pixel count (0.75 → 56%).
- **Bloom and tonemap** are a few percent: bloom works at a quarter of the pixels and below.
- **Coronas** are cheap by day (none drawn) and can be large additive quads at night.

## What changed, and what each item should save

| Item | Change | Expected GPU saving (reasoned) |
|---|---|---|
| Frustum culling | every instance tested on the CPU against the camera frustum (forward pass) and each cascade's box including its caster margin toward the sun (shadow pass), with mesh or chunk-part bounds; survivors packed per pass, no allocation | the largest: vertex work in the shadow and forward passes drops to what each pass can see (see the measured triangle counts); fill is unchanged |
| Glow meshes cast no shadow | neon/signal and lamp-lens meshes skip the shadow pass (`MeshInstance::casts_shadow`) | small: a few draws and their triangles per cascade |
| Small casters | casters under N texels of a cascade skipped in it (Medium/Deck 2, Low 3) | small to moderate in the far cascade: hydrants, mailboxes, benches, signs at 0.3–0.5 m per texel |
| Far cascade every other frame | the farthest cascade is kept from the frame before while its map (fitted 8% larger) still holds this frame's slice and the sun has not moved; only the drawn cascades are cleared | up to half of the far cascade's cost, which is the largest of the three (largest area, most casters) |
| Shadow resolution | 1024 / 1536 / 2048 / 3072 per cascade by preset | fill and clear scale with `size²`: 1536 is −44% vs 2048, 1024 −75% |
| Two cascades (Low, Deck) | 0–50 m and 45–400 m (`render::TWO_CASCADE_SPLITS`) | a third of the shadow pass; shadows end at 400 m |
| Resolution scale | the HDR target and depth at `scale ×` the swapchain, bilinear upscale in the tonemap pass, HUD at native resolution | forward, coronas and bloom fill scale with `scale²` (0.75 → −44%) |
| Bloom levels | 4 levels at Low/Deck, 5 from Medium up; `bloom_strength = 0` (Bloom off) skips every bloom pass (tested) | the dropped level is the smallest (40×25 at 1280×800): two passes of pass overhead, well under 0.1 ms; bloom off saves all of it (~5% of the frame) |
| Far terrain drawn last | `MeshInstance::late`: the far terrain (under the chunks everywhere) is drawn after them, so hidden pixels fail the depth test before shading | the far terrain's overdraw under the ring (it covers the ground under every chunk) |
| Coronas off at Low | no corona pass | night scenes only; one draw, potentially large additive quads |
| Present mode | `AutoVsync` by default (it was already); `--no-vsync` for benchmarks | none; makes measurements honest |

Checked and left alone:

- **Corona pass**: one instanced draw of quads; occlusion reads the depth texture in the
  vertex shader (5 taps per sprite, at most 258 sprites). Depth is single-sampled, so there
  is no resolve; reading it needs the forward pass's depth `Store`, which it has.
- **sRGB swapchain**: the tonemap writes an `*Srgb` format and the hardware encodes on write.
  No MSAA, no resolve; switching to a linear format would only move the encode into the
  shader. Kept.
- **DPI**: the native window asks for 1280×720 logical pixels, so a 1.5× desktop scale renders
  1920×1080. That is the honest size of the window; `--stats` prints physical and logical
  sizes and the scale factor, and resolution scale is the knob when it is too much.

## Parked-car meshes

Parked cars are instances of one mesh per (shape, paint, level of detail), built when the
place loads (`app::propinst`). With every paint a place uses that set grows with the roster:
on the coast (seed 1), vertices plus indices, measured in `parked_car_meshes_stay_within_budget`:

| Parked cars | Meshes | Triangles | Bytes |
|---|---|---|---|
| Four shapes, every paint used (before the full roster) | 152 | 421k | 22.9 MB |
| Twelve shapes, every paint used | 436 | 1.20M | 65.0 MB |
| Twelve shapes, 2 palette + 4 era paints per shape per place (shipped) | 144 | 396k | 21.5 MB |
| The same plus NPC traffic (its era paints are the parked cars'; shares every mesh on the coast) | 144 | 396k | 21.5 MB |

Each place draws its six paints per shape from the era's weights by its seed, so the street
mix still varies place to place. The test fails above 27 MB (about 1.2x the four-shape set),
always, not only under `SKYWAVE_ENFORCE_BUDGETS`, and counts the meshes the traffic builds for itself (`app::npc`: only shape and paint pairs the place does not park; none on the coast). The lasting fix is paint as a per-instance
attribute (`docs/IDEAS.md`).

## Quality presets

Settings → Video → Quality. `Settings::quality` is `None` until the player picks one; then
it is saved (`"video": { "quality": "deck" }`). Until then the game picks per device each
run (`Quality::auto`): **Deck** when the adapter is the Deck's APU ("Van Gogh",
"Sephiroth", "Custom GPU 0405"), an AMD integrated GPU, or the surface is 1280×800 on
Linux; **Medium** everywhere else. `--quality NAME` forces one for a session (not saved).

| Preset | Cascades | Cascade size | Far cascade | Small casters | Resolution | Bloom levels | Coronas | For |
|---|---|---|---|---|---|---|---|---|
| Low | 2 (50 / 400 m) | 1024² | every other frame | < 3 texels skipped | 75% | 4 | off | weak integrated GPUs |
| Battery | 2 (50 / 400 m) | 1536² | every other frame | < 2 texels | 85% | 4 | on | the Deck on battery: Deck's knobs, and choosing it sets the frame rate to 60 |
| Deck | 2 (50 / 400 m) | 1536² | every other frame | < 2 texels | 100% | 4 | on | Steam Deck, 1280×800 @ 90 Hz |
| Medium (default) | 3 (40 / 160 / 600 m) | 1536² | every other frame | < 2 texels | 100% | 5 | on | everything else until chosen |
| High | 3 | 2048² | every frame | all | 100% | 5 | on | desktop GPUs (the renderer's own default, `RenderQuality::default()`) |
| Ultra | 3 | 3072² (where the device allows 9216-wide textures) | every frame | all | 100% | 5 | on | big desktop GPUs |

Choosing a preset sets the resolution scale slider to the preset's value (Battery also sets
Frame rate to 60); the slider can
then move it (50–100%). Far terrain detail is the same at every preset: it is one mesh of
~8k triangles drawn last (a coarser level is a follow-up). Bloom on/off stays its own
toggle.

The screenshot goldens are taken on the web build in a fresh profile (SwiftShader), so
they render at **Medium**. Medium keeps the fifth bloom level for them: with four, the wide
halos round the coast's neon at night change about 2% of the night golden's pixels, for a
saving too small to measure (with five the night golden matches exactly). Its smaller
cascades and every-other-frame far cascade stay well inside the goldens' tolerance (at most
0.4% of pixels past the threshold, in the day scenes).

## Idle frames and the frame-rate cap

The owner's Deck ran at 90 fps with the GPU at 90% even paused or standing still. Two things
now keep it from drawing what it already drew:

**Draw only what changed** (`Renderer::plan` / `Renderer::draw`, both shells). Each frame
the app builds its scene as before; the renderer compares it with what the last world frame
was drawn from (camera, sun, sky, quality, every instance and light; positions and
orientations within 2 ppm, because a car at rest jitters by a billionth of a radian on its
springs forever) and the HUD's quads with the last ones drawn:

| What changed | Frame | GPU work |
|---|---|---|
| anything the world passes draw, the targets (resize, resolution scale), a mesh or a streamed chunk | World | every pass, as before |
| only the HUD (a toast fading, the clock text, a menu cursor, the F3 overlay) | Ui | the tonemap of the kept HDR target plus the HUD: one fullscreen pass |
| nothing | Skip | none: no swapchain image is taken and nothing is presented; the window (or canvas) keeps showing the last frame |

A window the system exposes or uncovers gets a Ui frame (`Renderer::screen_lost`). In LDR
mode (no HDR target to keep) a Ui frame is a World frame. The first change after idle is
drawn on the frame it happens: the comparison runs every frame, so a stale frame is never
shown. `--no-vsync` (benchmarks) still draws the whole world every frame, uncapped.

**Frame rate** (Settings › Video › Frame rate: Panel, 60, 45, 40, 30). The simulation stays
at 120 Hz; rendering interpolates as before. Natively the loop sleeps to deadlines on a grid
(`app::pacing::Pacer`: a late frame does not push the next ones back) and presents with
vsync, so a cap that divides the panel's rate lands on every Nth refresh: 45 and 30 on the
OLED Deck's 90 Hz panel, 30 on the LCD's 60. 60 on a 90 Hz panel alternates one and two
refreshes (60 a second, not perfectly even; set the panel to 60 Hz in the Quick Access
menu for that). The web skips `requestAnimationFrame` ticks the same way. Default: Panel,
and 60 on a Steam Deck (`FrameRate::auto`, the same detection as `Quality::auto`). The
**Battery** preset is Deck at 85% resolution and sets the cap to 60.

**Background.** Unfocused, the game draws at most 4 frames a second
(`app::pacing::BACKGROUND_FPS`); hidden or minimized, natively, it draws nothing (the game
runs on; at 4 Hz the sim catches up at most 8 steps a frame, so it runs slow). The radio
keeps playing unless Settings › Audio › Radio in background is Mute.

Measured on the no-op device at 1280×800 (`idle_frames_draw_the_world_once`, in CI; and
`cargo test --release -p platform-native -- --ignored --nocapture idle_cost`), 2026-10-27:

| Scene, 120 frames at 90 Hz (coast: 600) | World | Ui | Skip | Pixels written |
|---|---|---|---|---|
| test town, paused in the menu | 1 | 0 | 119 | 9.8 M (one world frame) |
| test town, still (car at rest, clock stopped, no traffic) | 1 | 0 | 119 | 9.8 M |
| test town, driving | 120 | 0 | 0 | 1035 M (8.6 M a frame) |
| coast, busy traffic, paused (600 frames) | 1 | 0 | 599 | 9.8 M |

"Pixels written" is from the renderer's own size counters: each cascade drawn, the scene's
target once (overdraw not counted), the bloom chain, the tonemap (Medium here, three 1536²
cascades). At Deck it is about 7.4 M for a world frame (5 M on frames that keep the far
cascade) and 1.0 M for a Ui frame. Deciding to draw nothing costs the CPU 9 µs a frame in
the test town and 52 µs on the streamed coast (2347 instances), release build. The coast's
start is on a slope: with no input the car rolls at a walking pace, so "standing" there
still draws every frame, correctly.

**What the Deck should show:** paused, or parked on the flat with the clock frozen and no
traffic, GPU use near zero (no frames drawn; the CPU still runs the game and builds the
scene at the panel's rate, not yet timed on the Deck); a menu with a moving element costs one fullscreen pass
a frame. Driving, the 60 cap (the Deck's new default) draws two frames in three at 90 Hz,
cutting GPU work by a third; Battery takes another ~28% off the scene's pixels (0.85²).
With the clock running (10× by default) or traffic on, the world changes every frame, so
the cap is what saves power there.

## Measured on the CPU

`cargo test --release -p platform-native -- --ignored --nocapture culling_on_the_coast` streams the
coast at the native budget (13×13 chunks) and renders it at 1280×800 on wgpu's no-op
device, printing the renderer's counts per preset, without traffic and with busy traffic
(warmed up 20 s, 77 cars live). From this container (seed 1, the start on the strip,
chase camera, 17:40), 2026-10-27, with the twelve-car roster and NPC traffic (M4 #6):

```
traffic off: 0 cars
high, no culling: instances 2829 fwd 2829 shadow [2766, 2766, 2766] (3 cascades) | tris fwd 3221k shadow 9601k a frame | draws 4748
            high: instances 2829 fwd 837 shadow [80, 402, 1193] (3 cascades) | tris fwd 828k shadow 1863k a frame | draws 1183
          medium: instances 2829 fwd 837 shadow [80, 403, 1299] (3 cascades) | tris fwd 828k shadow 1278k a frame | draws 655
            deck: instances 2829 fwd 837 shadow [100, 1085, 0] (2 cascades) | tris fwd 828k shadow 723k a frame | draws 477
             low: instances 2829 fwd 837 shadow [100, 1057, 0] (2 cascades) | tris fwd 828k shadow 717k a frame | draws 476
traffic busy: 77 cars
high, no culling: instances 2899 fwd 2899 shadow [2836, 2836, 2836] (3 cascades) | tris fwd 3365k shadow 10035k a frame | draws 4896
            high: instances 2899 fwd 838 shadow [82, 406, 1208] (3 cascades) | tris fwd 830k shadow 1906k a frame | draws 1201
          medium: instances 2899 fwd 838 shadow [82, 406, 1320] (3 cascades) | tris fwd 830k shadow 1311k a frame | draws 661
            deck: instances 2899 fwd 838 shadow [102, 1105, 0] (2 cascades) | tris fwd 830k shadow 749k a frame | draws 480
             low: instances 2899 fwd 838 shadow [102, 1080, 0] (2 cascades) | tris fwd 830k shadow 746k a frame | draws 479
```

From the start few cars are in view (one in the forward pass here): cars appear and vanish
only out of the camera's view, so most of the pool is off screen.

**Traffic in view.** `traffic_at_a_junction_within_budget` (`platform-native`, not ignored:
it runs in CI, about 50 s in a debug build, 10 s in release) streams the junction of the
`coast-traffic-noon` golden at the web budget, busy traffic warmed up 20 s with queued,
oncoming and crossing cars in view, and renders Deck and Medium with traffic off and busy:

| Junction, 1280×800 | Forward instances | Draws | Forward tris | Shadow tris |
|---|---|---|---|---|
| Deck, traffic off | 395 | 273 | 394k | 614k |
| Deck, busy (76 cars, 4 in the forward pass) | 399 | 281 | 408k | 671k |
| Medium, traffic off | 395 | 396 | 394k | 1109k |
| Medium, busy | 399 | 414 | 408k | 1201k |

Cars of one mesh share one instanced draw, so four cars cost eight draws at Deck (their
level-1 meshes in the forward pass and the cascades), about 3.5k triangles each forward and
again per cascade. Under `SKYWAVE_ENFORCE_BUDGETS` it fails above **Deck 310 draws, 450k
forward and 740k shadow triangles; Medium 455 draws, 450k and 1320k** (the busy numbers plus
about 10%), and if fewer than three NPC cars are in the forward pass. The traffic sim (step
and populate, busy) has a median under 0.5 ms a step in a debug build (`app::npc` and
`traffic` budget tests).

The scene itself had grown since the first measurement (below: 725k forward triangles and
436 draws at Deck), before traffic, with the detailed cars and props.

The first measurement (same command, no traffic, an earlier `main`):

```
high, no culling: instances 2828 fwd 2828 shadow [2765, 2765, 2765] (3 cascades) | tris fwd 2878k shadow 8572k a frame | draws 4452
            high: instances 2828 fwd 838 shadow [79, 402, 1192] (3 cascades) | tris fwd 725k shadow 1634k a frame | draws 1056
          medium: instances 2828 fwd 838 shadow [79, 402, 1296] (3 cascades) | tris fwd 725k shadow 1119k a frame | draws 570
            deck: instances 2828 fwd 838 shadow [100, 1086, 0] (2 cascades) | tris fwd 725k shadow 625k a frame | draws 436
             low: instances 2828 fwd 838 shadow [101, 1054, 0] (2 cascades) | tris fwd 725k shadow 625k a frame | draws 435
```

`shadow` is the most instances drawn into each cascade over 40 frames (a kept far cascade
draws none on alternate frames), and its triangles are the mean a frame. So the GPU used to
be handed **11.45 M triangles a frame** (2.9 M forward, 8.6 M shadow) in 4452 draws; with
culling it is **2.36 M at High (−79%), 1.84 M at Medium (−84%) and 1.35 M at Deck (−88%)**
in 436–1056 draws. The medium far cascade holds a few more casters than High's because its
map is fitted 8% wider so it can be kept. Recording the frame (`render()` on the no-op
device, which still culls, packs and encodes) went from 3.0 ms to 0.5–0.8 ms on this
shared 4-vCPU container (under heavy load from other builds, so read these as upper
bounds).

`cargo test --release -p render -- --ignored --nocapture cull_cost` times the culling
itself:

```
cull_and_pack: 16384 instances, 26317 kept over 4 passes: 1.881 ms (114.8 ns each)
  the same packing everything into every pass (no culling): 1.906 ms
```

The box tests themselves are nearly free next to writing the instance records: culling 16k
instances against four passes costs no more than packing them all unculled (same loaded
container). The coast submits under 3k instances, so this is a fraction of a millisecond.

## Capturing numbers on the machine

Build a release binary (`cargo build --release -p platform-native`, binary
`target/release/skywave`; on the Deck the nightly AppImage works the same), then:

```sh
# 600 frames on the coast, vsync off so the number is the GPU's, not the panel's:
skywave --stats --no-vsync --frames 600 --exit --scene place=coast 2> stats.txt
# The same with vsync (what the player sees), and per preset:
skywave --stats --frames 600 --exit --scene place=coast 2> stats-vsync.txt
for q in low deck medium high; do
  skywave --stats --no-vsync --frames 600 --exit --quality $q --scene place=coast 2> stats-$q.txt
done
# Culling off, to see what it saves:
skywave --stats --no-vsync --frames 600 --exit --no-cull --scene place=coast 2> stats-nocull.txt
# Another backend (Vulkan is the default on Linux; GL is the fallback):
WGPU_BACKEND=gl skywave --stats --no-vsync --frames 600 --exit --scene place=coast 2> stats-gl.txt
# Quieter logs:
RUST_LOG=wgpu_hal=warn skywave --stats ...
```

Useful scenes: `--scene t=22:30 --scene lights=1` (night: coronas and lamps),
`--scene cam=cockpit`, `--scene stream=0` (the test town built at load).

### The `--stats` output

At startup:

```
adapter: AMD Custom GPU 0405 (RADV VANGOGH) | Vulkan, radv Mesa 24.1.0 | IntegratedGpu
surface: 1280x800 px (scale factor 1, 1280x800 logical) Bgra8UnormSrgb present AutoVsync (supported [Fifo, Mailbox, Immediate])
quality: deck RenderQuality { shadow_size: 1536, cascades: 2, ... } culling on
gpu timing: timestamp queries per pass
```

Then once a second (one line, wrapped here):

```
[stats] t=12s frames=88 fps=88.0 frame_ms avg=11.36 p99=14.20 max=16.90 cpu_ms avg=1.92 p99=2.40
  | inst=5123 fwd=1432 sh=890/1201/2240 draws=412 tris_fwd=812k tris_sh=1630k shadow=1536x2 drawn=1
    view=1280x800 surface=1280x800 bloom=4
  | gpu_ms total=9.80 shadow=3.10 forward=4.90 corona=0.10 bloom=0.60 tonemap=0.30 hud=0.10
  | present=AutoVsync quality=deck
```

- `frame_ms`: time between frames (avg, 99th percentile, max over the second). With vsync it
  is the display's pace; with `--no-vsync` it is the slower of CPU and GPU.
- `cpu_ms`: CPU time to build the scene view and record and submit the frame (not waiting
  for the swapchain).
- `inst`: instances submitted; `fwd`: drawn in the forward pass; `sh`: drawn into each
  cascade (0 for one kept from the frame before); `drawn`: cascades drawn this frame.
- `tris_fwd`, `tris_sh`: triangles drawn in the forward pass and in all cascades together.
- `shadow=SIZExN`: cascade edge in texels and cascade count; `view`: the scene's target
  (scaled); `surface`: the swapchain; `bloom`: levels drawn.
- `gpu_ms`: GPU time per pass from timestamp queries, a frame or two old; only when the
  adapter has `TIMESTAMP_QUERY` (Vulkan, Metal, DX12; not GL).

`--frames N` ends with a `[summary]` line over the run after 60 warmup frames.

The F3 overlay shows the same numbers in game (web and native): draws, CPU time, instances
per pass, triangles, cascades, view and surface size, GPU time per pass where available,
and the adapter.

### What to send back

1. `stats.txt`, `stats-vsync.txt`, the per-preset files and `stats-nocull.txt`.
2. Which Deck (LCD 60 Hz or OLED 90 Hz), SteamOS version, Game Mode or Desktop Mode, and
   the frame limiter / refresh settings in the Quick Access menu.
3. A screenshot with F3 on at the slowest spot you find.
4. If one pass stands out, a RenderDoc capture (below).

### A frame capture with RenderDoc

RenderDoc is free (MIT) and runs on Linux and the Deck's Desktop Mode.

```sh
renderdoccmd capture --wait-for-exit ./skywave --scene place=coast
# press F12 (or PrtScn) in game at the slow spot; the .rdc lands in /tmp/RenderDoc/
```

Or the GUI: File → Launch Application, executable `skywave`, arguments
`--scene place=coast`, Launch, then F12 in game. Send the `.rdc` (compressed) and, from
the GUI, the Event Browser timings (Tools → Performance counters, or the clock icon:
"Time durations for the drawcalls") as a screenshot or CSV. Use the Vulkan backend
(RenderDoc does not capture wgpu's GL path well).

## Checklist: confirm these first

- **Vsync and the panel**: with `AutoVsync` on a 60 Hz panel the rate cannot pass 60; on the
  OLED Deck's 90 Hz panel, 90. Missing 11.1 ms drops a frame. Measure with `--no-vsync`.
- **The frame limiter**: SteamOS's Quick Access menu can cap the rate (and halve it).
- **Compositor**: Game Mode (gamescope) vs Desktop Mode (KWin) present differently; note which.
- **DPI scale**: `--stats` prints the physical size and scale factor; a 2× desktop scale
  renders 4× the pixels of the window's logical size.
- **Which GPU**: on laptops with two GPUs check the `adapter:` line names the one expected
  (`DRI_PRIME=1` picks the discrete one on Mesa).
- **Backend**: the `adapter:` line should say Vulkan; GL is slower and has no timestamps.
- **Thermals and power**: the Deck's TDP slider and a warm APU change clocks; note them.
- **Debug build**: numbers only mean something from `--release`.

## Performance follow-ups

What could not be verified without a GPU here:

- **Real pass timings.** Every saving above is reasoned from counts; the `gpu_ms` numbers
  from a Deck will say which pass dominates. Re-rank this list by them.
- **Whether keeping the far cascade shows.** The map is fitted 8% wider and re-drawn as soon
  as the slice leaves it or the sun moves; check fast turns at speed for a one-frame edge
  at 400–600 m.
- **The small-caster threshold.** Check that 2 texels does not pop props' shadows in and out
  at the far cascade's distance.
- **Bilinear upscale quality** at 75% and below on the Deck's panel; a sharpening pass (or
  an FSR-1-like spatial upscale) if it looks soft.
- **Fragment cost of the forward pass**: if `forward` dominates, a depth prepass (native)
  or front-to-back sorting of chunk tiles would cut shading overdraw; light-loop cost at
  night with 8 lights per draw.
- **Mailbox vs FIFO** on gamescope for latency at 90 Hz.
- **Far terrain detail level** (a coarser mesh at Low) and **chunk facade LOD beyond 400 m**
  (`docs/RENDERING.md`), not implemented.
- **Corona fill at night**: large sprites near lamps; measure before limiting their size.
- **The web build on integrated GPUs**: WebGL2 has no timestamps; measure frame time only.
- **Automatic preset by measured frame time** (drop a preset after N slow seconds) instead
  of by adapter name.
