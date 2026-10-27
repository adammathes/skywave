# Performance

Read with `docs/RENDERING.md` (the pipeline) and `docs/ARCHITECTURE.md` (budgets). This is
how a frame spends the GPU, what each quality preset buys, how to measure on a real
machine, and what is still unverified.

The reference machine is the **Steam Deck**: RDNA2 APU (8 CUs, ~1.6 TFLOPS, 88 GB/s shared
LPDDR5), 1280×800, and a **90 Hz panel on the OLED model** (60 Hz on the LCD one). With vsync on
a 90 Hz panel a frame must finish in **11.1 ms** or it waits for the next refresh (and
the rate falls toward 45 or 60 Hz, depending on the compositor). The first report from a
Deck: ~65 fps with the GPU at 99%, so GPU bound at roughly 15 ms a frame. The second
(after culling and the Deck preset): 90 fps with the GPU at 90%, about 10 ms of every
11.1 ms frame, fans audible. Idle frames and a frame cap are the frame loop's job; this
page is about what one rendered frame costs.

**To get a per-pass breakdown from the Deck**, in Desktop Mode (Konsole):

```sh
~/Applications/Skywave/Skywave-x86_64.AppImage --stats 2> ~/stats-deck.txt
# drive for 30 s, then pause (Esc) for 30 s, then quit; then the same with the depth
# pre-pass off, to see what it buys on the real GPU:
~/Applications/Skywave/Skywave-x86_64.AppImage --stats --no-prepass 2> ~/stats-deck-noprepass.txt
```

Every 5 s `--stats` prints a `[gpu]` table: each pass's GPU time (and each cascade's and
the forward pass's parts) as min / median / p95 over those 5 s, and the frame's draws,
triangles and shaded fragments (format below). Send both files.

## Cost model

Per frame, in order (`crates/render/src/lib.rs`):

| Pass | Work | Scales with |
|---|---|---|
| Shadow | depth-only, every caster once per cascade (3 cascades, `size²` each, side by side in one `Depth32Float` atlas); a cascade whose casters, coverage and sun have not changed is kept from an earlier frame | casters × cascades redrawn (vertex), `size²` × cascades redrawn (depth fill and clear) |
| Forward | the depth pre-pass at Deck (every visible mesh, depth only), then every visible instance: atlas texel, Lambert sun with 3×3 PCF (twice in the cascade blend band), hemisphere ambient, up to 8 point/spot lights, fog, emissive; then the sky (one fullscreen triangle at the far plane, shading only what no mesh covered) | visible triangles (vertex; twice with the pre-pass), shaded fragments × lights (fragment) |
| Coronas | one instanced quad per light with a corona (≤ 258), 5 depth taps each in the vertex shader | lights at night, sprite size on screen |
| Bloom | prefilter (13 taps) into half the scene's size, then down and up a chain of 4–5 levels | scene pixels / 4, levels |
| Tonemap | one fullscreen triangle: exposure, bloom, AgX, vignette, dither, upscale when scaled | swapchain pixels |
| HUD | one instanced draw of ≤ 4096 quads | quads |

Where the time goes at 1280×800, reasoned from the pipeline and the counts (no GPU here to
measure time on; the fragment counts below are exact, from occlusion queries on a software
Vulkan device; `--stats` on the Deck gives the times):

- **Geometry dominated the shadow pass.** Before culling every instance (every resident chunk
  tile of the 13×13 ring, every prop, the car, the far terrain) was drawn four times: three
  cascades and the forward pass. Most of it cannot be seen: the camera sees about a quarter
  of the ring, the near cascade a few tiles, the middle one a tenth of the ring. The numbers
  for the coast are in "Measured on the CPU" below.
- **Shadow fill.** Three 2048² cascades are 12.6 M depth texels cleared and written per frame
  (50 MB of `Depth32Float`). 1536² is 44% less; two cascades a third less again.
- **Forward fragments.** The meshes shaded each covered pixel 2.0–2.7 times on the coast
  (below), each with the PCF kernel, fog (a Preetham sky evaluation) and the light loop,
  and the sky shaded every pixel first. With the sky drawn last and the depth pre-pass,
  each pixel is shaded once. Resolution scale cuts this linearly with the pixel count
  (0.75 → 56%).
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
| Sky drawn last | the sky's triangle sits at the far plane and is drawn after the meshes with a less-or-equal depth test, so it shades only the pixels they leave | measured: 1024k sky fragments a frame → 104–240k on the coast scenes (−77 to −90%); each runs the sky model, stars, moon and sun disc |
| Depth pre-pass (Deck) | every visible mesh drawn depth-only first (same position math, `@invariant`), then shaded with less-or-equal: each covered pixel shades once | measured: the meshes' fragments −51 to −63% (2.0–2.7× overdraw → 1.00×), night light evaluations 15.0 M → 6.2 M; costs the forward pass's vertex work again and ~300 draws (why it is Deck only: not at Medium, the web default, nor Low, which runs on the web too, where draws cost the CPU most). Reasoned for the Deck: ~1.1 M fewer fragments of ~300 ALU ops and ~11 texture reads ≈ 1–2 ms, against ~0.4–0.6 ms of depth-only geometry; `--no-prepass` measures it |
| Cascades kept while nothing changes | each cascade is kept from an earlier frame while its map still covers this frame's slice (to half a texel), a hash of its casters (meshes, transforms and the chunk pool's writes) is unchanged, and the sun has moved too little to shift a 50 m caster's shadow by half a texel; the far cascade between its frames still keeps the old rule; with no cascade to draw there is no shadow pass at all | pays when the world is still: paused, in a menu, parked with nothing moving near. Otherwise anything moving in a cascade (the player's car while driving, NPC cars, people within 40 m) changes its hash, and at the default 10× clock the sun passes the threshold about every 2 s at noon (the still check binds first), every ~0.15 s at golden hour for the far cascade and every frame or two for the near one. Kept maps are sampled with their own fit, so a kept frame can differ from a fresh one by under half a shadow texel |
| Far cascade every third frame (Deck, Low) | `far_cascade_interval` 2 → 3 | while driving, the far cascade is redrawn on its interval (something in it moves), a third of its cost instead of half: on the coast at Deck, driving at 100 km/h with the world running, 723k → 534k shadow triangles a frame (busy traffic 749k → 561k), about −26% |

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
| Twelve shapes, 2 palette + 4 era paints per shape per place | 144 | 396k | 21.5 MB |
| The same plus NPC traffic (its era paints are the parked cars'; shares every mesh on the coast) | 144 | 396k | 21.5 MB |
| Fourteen shapes (the Longbow Coupe and Appaloosa GT added; the exotics and open sports cars never park), traffic included (shipped) | 168 | 460k | 24.8 MB |

Each place draws its six paints per shape from the era's weights by its seed, so the street
mix still varies place to place. The test fails above 27 MB (about 1.2x the four-shape set),
always, not only under `SKYWAVE_ENFORCE_BUDGETS`, and counts the meshes the traffic builds for itself (`app::npc`: only shape and paint pairs the place does not park; none on the coast). The lasting fix is paint as a per-instance
attribute (`docs/IDEAS.md`).

## Pedestrian figure meshes

Pedestrians are instances of eight figures per place, 13 part meshes each, built the first
time traffic is on (`app::peds`). On the coast (seed 1): 104 meshes, 8.3k triangles,
266 kB (vertices plus indices), measured in `figure_meshes_stay_within_budget`, which fails
above **3 MB** always. Each figure added costs a draw per part in view per pass, not
memory: eight is the trade between a varied crowd and the draw budget below (twelve
figures drew about 50 more at Deck on the busiest block).

## Quality presets

Settings → Video → Quality. `Settings::quality` is `None` until the player picks one; then
it is saved (`"video": { "quality": "deck" }`). Until then the game picks per device each
run (`Quality::auto`): **Deck** when the adapter is the Deck's APU ("Van Gogh",
"Sephiroth", "Custom GPU 0405"), an AMD integrated GPU, or the surface is 1280×800 on
Linux; **Medium** everywhere else. `--quality NAME` forces one for a session (not saved).

| Preset | Cascades | Cascade size | Far cascade | Small casters | Resolution | Bloom levels | Coronas | Depth pre-pass | For |
|---|---|---|---|---|---|---|---|---|---|
| Low | 2 (50 / 400 m) | 1024² | every third frame | < 3 texels skipped | 75% | 4 | off | off | weak integrated GPUs |
| Battery | 2 (50 / 400 m) | 1536² | every third frame | < 2 texels | 85% | 4 | on | on | the Deck on battery: Deck's knobs, and choosing it draws every other refresh (45 fps at 90 Hz, 30 at 60 Hz) |
| Deck | 2 (50 / 400 m) | 1536² | every third frame | < 2 texels | 100% | 4 | on | on | Steam Deck, 1280×800 @ 90 Hz |
| Medium (default) | 3 (40 / 160 / 600 m) | 1536² | every other frame | < 2 texels | 100% | 5 | on | off | everything else until chosen |
| High | 3 | 2048² | every frame | all | 100% | 5 | on | off | desktop GPUs (the renderer's own default, `RenderQuality::default()`) |
| Ultra | 3 | 3072² (where the device allows 9216-wide textures) | every frame | all | 100% | 5 | on | off | big desktop GPUs |

At every preset the sky is drawn last and a cascade is kept while nothing it shows has
changed (a kept cascade within half a shadow texel of a fresh one). The pre-pass changes 130 isolated pixels of
1 024 000 on the coast, where two triangles of one mesh tie in depth and its
less-or-equal test lets the later one win (`depth_prepass_keeps_the_image`).

Choosing a preset sets the resolution scale slider to the preset's value (Battery also sets
Frame rate to half the panel's; `--quality battery` brings that cap for the session too); the
slider can then move it (50–100%). Far terrain detail is the same at every preset: it is one mesh of
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

**Draw only what changed** (`Renderer::plan` / `Renderer::draw`, both shells; the contract
is in `docs/RENDERING.md`). Each frame the app builds its scene as before; the renderer
compares it with what the last world frame was drawn from (camera, sun, sky, quality, every
instance and light) and the HUD's quads with the last ones drawn. Orientations may differ
by two millionths and positions by 0.1 mm plus a millionth of their distance from the eye
(a car at rest jitters by a billionth of a radian on its springs forever; measured from the
camera, so a block 3 m ahead creeping 4 mm a frame at the far corner of a 3 km place is
still drawn every frame):

| What changed | Frame | GPU work |
|---|---|---|
| anything the world passes draw, the targets (resize, resolution scale), a mesh or a streamed chunk | World | every pass, as before |
| only the HUD (a toast fading, the clock text, a menu cursor, the F3 overlay) | Ui | the tonemap of the kept HDR target plus the HUD: one fullscreen pass |
| nothing | Skip | none: no swapchain image is taken and nothing is presented; the window (or canvas) keeps showing the last frame |

Any surface reconfigure (a resize, even to the same size as Windows sends restoring from
minimized; an outdated or lost swapchain) and a window coming back from hidden draw at least
a Ui frame (`Renderer::screen_lost`). In LDR mode (no HDR target to keep) a Ui frame is a
World frame. Kept cascades and idle frames agree: Ui and Skip frames leave the kept cascades
alone, a world frame that held the far cascade over between its frames (without checking
its casters) keeps the next frames drawing the world until one checks it, and the first
world frame after kept or skipped ones holds nothing unchecked
(`idle_frames_never_leave_a_held_cascade_stale`). The first change after idle is drawn on the frame it happens: the comparison
runs every frame, so a stale frame is never shown. A World frame copies the scene's
instances and lights for the next comparison (about 250 KB on the coast: 2347 instances of
~104 bytes, into a buffer reserved at load). `--no-vsync` (benchmarks) still draws the whole
world every frame, uncapped. `--stats` ends its line with `last=World|Ui|Skip` and running
counts of each; the full F3 overlay shows the same (with an overlay up, it changes every
frame, so frames are at least Ui; menus and the map hide it, so they idle).

**Frame rate** (Settings › Video › Frame rate): the panel's rate, or every Nth refresh, so
frames are always evenly paced. Offered: the panel's rate, then each whole divisor of at
least 20 fps (90 Hz: 45, 30; 60 Hz: 30, 20; 144 Hz: 72, 48, 36; `FrameRate::offered`). The
simulation stays at 120 Hz; rendering interpolates as before. Natively the loop sleeps to
deadlines N refreshes apart (`app::pacing::Pacer`, on a grid so a late frame does not push
the next ones back) and presents with vsync; the panel's rate needs no timer. The web
processes every Nth animation frame and estimates the refresh from them. Default: the
panel's rate everywhere (the owner likes the 90; idle frames are what cool a paused Deck).
The **Battery** preset is Deck at 85% resolution and every other refresh.

Why not 60 on a 90 Hz panel: a 60 fps frame would alternate two and one refreshes, each
shown 5.6 ms early or late, about 15 cm of judder at 100 km/h; 45 is exactly two.

**Background.** Unfocused, the game pauses (as under the pause menu, without opening it)
and resumes when the focus comes back; the loop runs at most 4 times a second
(`app::pacing::BACKGROUND_FPS`) and, nothing changing, draws nothing. Minimized natively
it draws nothing at all; winit 0.30 reports that (`Occluded`) on X11 and macOS but not on
Windows or Wayland, where minimizing is the unfocused path. On the web a hidden page (a tab
switch) pauses the game; a visible page without the focus (an unclicked frame) runs. The
radio keeps playing unless Settings › Audio › Radio in background is Mute (the web's
player; the native radio has no player yet, #15).

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

**What the Deck should show:** paused, in a menu, in the background, or parked on the flat
with the clock frozen and no traffic, GPU use near zero (no frames drawn; the CPU still runs
the game and builds the scene at the panel's rate, not yet timed on the Deck); a menu with a
moving element costs one fullscreen pass a frame. Driving at the default 90 the GPU works as
before (that is `perf/gpu-time`'s side); Battery draws every other refresh, half the GPU
work, and takes another ~28% off the scene's pixels (0.85²). With the clock running (10× by
default) or traffic on, the world changes every frame, so only the cap saves power there.

## Fragments, measured on a software GPU

`cargo test --release -p platform-native -- --ignored --nocapture gpu_passes_on_the_coast`
renders the coast's golden scenes at 1280×800 on a Vulkan device (lavapipe, Mesa's CPU
rasterizer, in this container: `apt-get install mesa-vulkan-drivers`) with the renderer's
GPU queries on, and prints `--stats`'s work line per preset. Occlusion queries count the
samples that pass the depth test, so these are exact counts of fragments shaded; lavapipe's
*times* are not the Deck's (a CPU rasterizer pays for geometry far more than for
fragments: there the pre-pass makes the forward pass slower, 192 → 244 ms), so they are
not quoted. Deck preset, 2026-10-27:

| Scene | Pixels covered | Sky fragments, before → after | Mesh fragments (overdraw): before | with the pre-pass |
|---|---|---|---|---|
| coast-strip-golden | 920k | 1024k → 104k | 1959k (2.13×) | 920k (1.00×) |
| coast-traffic-noon | 784k | 1024k → 240k | 2121k (2.71×) | 784k (1.00×) |
| coast-night-golden | 885k | 1024k → 139k | 2089k (2.36×) | 885k (1.00×) |

Drawing the meshes front to back instead (runs of one mesh sorted by distance, no extra
pass) only got to 2.04–2.22× (most overdraw is inside a chunk's own mesh), and it would
let the camera's distances decide which of two coplanar meshes wins the depth test, which
can flicker while driving; it was taken out.

At night the light loop runs 15.0 M times a frame before, 6.2 M with the
pre-pass (7.0 per fragment: the 36 instances with a full set of 8 lights are chunk tiles
under most of the street). A pre-pass of the chunk geometry alone (fewer draws) leaves
1.07–1.38× overdraw (cars and props in front of walls), so every mesh goes in it.
`fragments_on_the_coast_at_deck` (runs in CI on lavapipe) holds these: the sky under a
fifth of the pixels, the pre-pass at most 55% of the fragments without it and within 2% of
the covered pixels, bloom's seven passes, and a still view keeping both cascades.

Cuts looked at and not made, by this accounting:

- **PCF** (3×3 compare taps, twice in a blend band): after the pre-pass the coast runs
  ~0.9 M lookups a frame, 8 M comparisons. Four taps instead of nine (a 2×2 of bilinear
  compares) saves ~4.5 M bilinear reads, ~0.1 ms at the Deck's ~51 G texels/s, for softer,
  blockier edges; a narrower blend band or a cheaper far-cascade filter saves less (the
  bands are a few percent of the pixels). Not worth a visible change.
- **Fogged-out instances**: fog is `1 - exp(-d × 0.0003 × turbidity)`, so it reaches
  opacity (within 1/255) only past 3 km (thick haze) to 9 km (clear); nothing drawn is that
  far, so there is nothing to drop.
- **Lights**: the shader's loop already stops at the instance's own light count (its set is
  nearest first); what costs is the few chunk tiles with full sets under most pixels. Finer
  light tiles in `app::lighttiles` would cut the 7 evaluations per fragment; a follow-up.
- **Bloom at Deck**: checked against the counters: the chain starts at half the target
  (640×400) and has four levels, seven passes (prefilter, three down, three up), about
  7.4 M bilinear reads; a level less saves under 1%. Unchanged.

## Measured on the CPU

`cargo test --release -p platform-native -- --ignored --nocapture culling_on_the_coast` streams the
coast at the native budget (13×13 chunks) and renders it at 1280×800 on wgpu's no-op
device, printing the renderer's counts per preset, without traffic and with busy traffic
(warmed up 20 s, 78 cars and 55 people live). Since the cascades can be kept, the camera
drives at 100 km/h (0.3 m a frame) over the 40 frames while the world runs (traffic,
people and the clock move a 90 Hz frame each); `draws` is the most in a frame (the first,
drawing every cascade) and the median frame's after it, and the triangles are the mean a
frame. With the GPU cuts, 2026-10-27:

```
traffic off: 0 cars, 0 people
high, no culling: instances 2863 fwd 2863 shadow [2800, 2800, 2800] (3 cascades) | tris fwd 3265k shadow 9692k a frame | draws 4908 at most, median 4908 after the first
            high: instances 2863 fwd 850 shadow [81, 421, 1229] (3 cascades) | tris fwd 843k shadow 1913k a frame | draws 1238 at most, median 1235 after the first
          medium: instances 2862 fwd 850 shadow [81, 423, 1323] (3 cascades) | tris fwd 843k shadow 574k a frame | draws 1276 at most, median 674 after the first
            deck: instances 2862 fwd 850 shadow [101, 1101, 0] (2 cascades) | tris fwd 843k shadow 534k a frame | draws 1389 at most, median 903 after the first
             low: instances 2862 fwd 849 shadow [101, 1074, 0] (2 cascades) | tris fwd 842k shadow 531k a frame | draws 962 at most, median 485 after the first
traffic busy: 78 cars, 55 people
            high: instances 3646 fwd 1017 shadow [148, 437, 1244] (3 cascades) | tris fwd 858k shadow 1959k a frame | draws 1399 at most, median 1396 after the first
          medium: instances 3648 fwd 1012 shadow [148, 435, 1346] (3 cascades) | tris fwd 857k shadow 1350k a frame | draws 1448 at most, median 825 after the first
            deck: instances 3647 fwd 1007 shadow [168, 1126, 0] (2 cascades) | tris fwd 857k shadow 561k a frame | draws 1632 at most, median 1125 after the first
             low: instances 3646 fwd 1008 shadow [168, 1101, 0] (2 cascades) | tris fwd 859k shadow 558k a frame | draws 1115 at most, median 616 after the first
```

Medium without traffic keeps its far cascade (nothing in it moves; its interval is 2 and
its map 8% wider than the view); with traffic it is redrawn every other frame as before.
(The forward triangles at Deck are drawn twice, once depth-only; recording a
frame still takes 0.4–0.9 ms on the no-op device.) Before the GPU cuts (same command with
a still camera, `draws` the last frame's, an earlier `main` with M4 #6):

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
`coast-traffic-noon` golden at the web budget, busy traffic and pedestrians warmed up with
queued, oncoming and crossing cars and people on the walks in view, and renders Deck and
Medium with traffic off and busy:

| Junction, 1280×800 | Forward instances | Draws | Forward tris | Shadow tris |
|---|---|---|---|---|
| Deck, traffic off | 395 | 273 | 394k | 614k |
| Deck, busy cars only (76 cars, 4 in the forward pass; before pedestrians) | 399 | 281 | 408k | 671k |
| Deck, busy cars and people (41 people, 14 in the forward pass) | 581 | 385 | 422k | 672k |
| Medium, traffic off | 395 | 396 | 394k | 1109k |
| Medium, busy cars only (before pedestrians) | 399 | 414 | 408k | 1201k |
| Medium, busy cars and people | 581 | 530 | 422k | 1204k |

Cars of one mesh share one instanced draw, so four cars cost eight draws at Deck (their
level-1 meshes in the forward pass and the cascades), about 3.5k triangles each forward and
again per cascade. A person is 13 part instances of about 1k triangles in all; people drawn
as one figure share its 13 draws per pass, so the draws grow with the figures in view (at
most 8 × 13 per pass), not the people, and only people within 40 m cast shadows. Those
numbers are the last frame of a still camera, with the simulation stopped (`still=1`).
Since the GPU cuts the test drives the camera over 8 frames with the world running and
counts the most draws in a frame, the median frame's after the first, and the mean
triangles: busy, **Deck 1134 draws, 681 in the median frame, 428k forward, 672k shadow;
Medium 1121, 503, 428k, 1206k**. Where the draws come from, frame by frame (busy):

| Frame | Medium | Deck |
|---|---|---|
| Before (main, the last of 4 still frames: far cascade kept) | 530 | 385 |
| Now, the median frame (the near cascade(s) only) | 503 | 681 (the 309 pre-pass draws on top) |
| Now, the frame that also draws the far cascade (the worst) | 1121 | 1134 |

So Medium's draws are unchanged: the old count's frame parity skipped the far cascade's
~600 draws, which it redraws every other frame now too. Deck's frames gain the ~300
pre-pass draws, depth-only, for the junction's 2121k → 784k shaded fragments (overdraw
2.71 → 1.00, lavapipe): about 1.3 M fragments of the full forward shader against ~0.3k draws
and 428k depth-only triangles. Under `SKYWAVE_ENFORCE_BUDGETS` it fails above **Deck 1250
draws, 750 in the median frame, 470k forward and 740k shadow triangles; Medium 1235, 555,
470k and 1325k** (about
10% over), and if fewer than three cars' and three people's instances more are in the
forward pass than with traffic off. The traffic sim (step and populate, busy) and the
pedestrian sim (busy) each have a median under 0.5 ms a step in a debug build
(`app::npc` and the `traffic` budget tests), the best of three 5 s windows; the app's
pedestrian wrapper (`app::peds`) gets 1 ms, because it runs beside the whole app test
binary and a 2-core runner measured 0.56 ms. The p99 gate is loose, only for a tenfold regression: under 2 ms for the cars, under 5 ms for
the pedestrians in `traffic` (whose p99 reaches 3.2 ms on a 2-core CI runner with the
other pedestrian tests running beside it; the median stays near 0.2 ms).

The coast's busiest block (the `coast-peds-noon` golden, 95 cars and 119 people live, 349
more instances in the forward pass), same measurement before the GPU cuts (a still
camera, the last frame), release: Deck 371 draws with traffic off, 528 busy; Medium 623
and 859. Forward triangles 857k and 886k, shadow 1191k and 1243k at Deck. With twelve
figures it was 580 at Deck, and 632 with every person casting a shadow.

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
# Culling off, to see what it saves; the depth pre-pass off (Deck), the same:
skywave --stats --no-vsync --frames 600 --exit --no-cull --scene place=coast 2> stats-nocull.txt
skywave --stats --no-vsync --frames 600 --exit --no-prepass --scene place=coast 2> stats-noprepass.txt
# Another backend (Vulkan is the default on Linux; GL is the fallback):
WGPU_BACKEND=gl skywave --stats --no-vsync --frames 600 --exit --scene place=coast 2> stats-gl.txt
# Quieter logs:
RUST_LOG=wgpu_hal=warn skywave --stats ...
```

Useful scenes: `--scene t=22:30 --scene lights=1` (night: coronas and lamps),
`--scene cam=cockpit`, `--scene stream=0` (the test town built at load). A scripted run
holds the camera still, so after the first frames it keeps its cascades and shows no shadow
cost: for the shadow pass, drive (the Deck command at the top).

### The `--stats` output

At startup:

```
adapter: AMD Custom GPU 0405 (RADV VANGOGH) | Vulkan, radv Mesa 24.1.0 | IntegratedGpu
surface: 1280x800 px (scale factor 1, 1280x800 logical) Bgra8UnormSrgb present AutoVsync (supported [Fifo, Mailbox, Immediate])
quality: deck RenderQuality { shadow_size: 1536, cascades: 2, ... } culling on
gpu timing: timestamp queries per pass and inside passes; occlusion queries
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

Every 5 s, with timestamp queries, a table over those 5 s (one row per frame read back):

```
[gpu] t=15s over 5 s, quality=deck
gpu ms             min  median     p95   (440 frames read)
  shadow          0.05    0.90    2.60
    cascade0      0.02    0.40    0.70
    cascade1      0.00    0.00    1.90
    cascade2      0.00    0.00    0.00
  forward         5.10    5.40    6.00
    depth         0.50    0.55    0.60
    meshes        4.30    4.60    5.10
    late          0.05    0.06    0.08
    sky           0.15    0.16    0.20
  corona          0.00    0.05    0.10
  bloom           0.40    0.42    0.45
  tonemap         0.30    0.31    0.33
  hud             0.05    0.06    0.08
  total           6.10    7.20    9.40
  draws 636 (shadow 120, bloom 7) | tris fwd 620k shadow 100k | lights: 0 instances with a
  full set, 0 in all sets | px 1024k: sky 104k, meshes 920k (overdraw 1.00), light evals 0k,
  pcf 920k lookups x9 taps | depth texels 2400k/0k/0k
```

(The numbers here are made up to show the layout.) The parts (each cascade; the forward
pass's depth pre-pass, meshes, `late` backdrop and sky) are timed from inside their pass
where the adapter has `TIMESTAMP_QUERY_INSIDE_PASSES` (most Vulkan and DX12 drivers), and are
rough there (the GPU overlaps work); without it those rows are 0. A kept cascade costs
nothing, so its row's median is 0 when it is kept more often than drawn. The last line is
the newest frame read: draws and triangles; instances with a full set of 8 lights and the
lights in all sets; and from occlusion queries the fragments shaded (`sky`; `meshes`, with
`overdraw` their share of the pixels the sky did not get, 1.00 with the pre-pass), the light
loop's iterations, the fragments of meshes that receive the sun's shadow (each lit one
runs 9 comparisons, 18 in a cascade blend band), and the depth texels written per cascade.

`--frames N` ends with a `[summary]` line over the run after 60 warmup frames.

F3 cycles the overlay Off → FPS (the frame rate alone, top left) → Full; the mode is saved
(`hud.overlay`) and menus hide it while open. The full overlay shows the same numbers in
game (web and native): draws, CPU time, instances
per pass, triangles, cascades, view and surface size, the adapter, and where timestamps
work one line of GPU medians over the last 64 frames read: `gpu med 7.20 ms: sh 0.90 fw
5.40 co 0.05 bl 0.42 tm 0.31 hud 0.06 | c 0.40/0.00/0.00 z 0.55 m 4.60 sky 0.16`
(passes; then cascades, pre-pass, meshes and sky where timed inside passes).

### What to send back

1. `stats-deck.txt` and `stats-deck-noprepass.txt` (the command at the top), and if there
   is time `stats.txt`, `stats-vsync.txt`, the per-preset files and `stats-nocull.txt`.
2. Which Deck (LCD 60 Hz or OLED 90 Hz), SteamOS version, Game Mode or Desktop Mode, and
   the frame limiter / refresh settings in the Quick Access menu.
3. A screenshot with the full F3 overlay (F3 twice from off) at the slowest spot you find.
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

- **Real pass timings.** Every saving above is reasoned from counts; the `[gpu]` tables
  from a Deck will say which pass dominates. Re-rank this list by them, and keep the depth
  pre-pass at Deck only if `--no-prepass` is slower there.
- **Whether keeping the far cascade shows.** The map is fitted 8% wider and re-drawn as soon
  as the slice leaves it or the sun moves; check fast turns at speed for a one-frame edge
  at 400–600 m.
- **The small-caster threshold.** Check that 2 texels does not pop props' shadows in and out
  at the far cascade's distance.
- **Bilinear upscale quality** at 75% and below on the Deck's panel; a sharpening pass (or
  an FSR-1-like spatial upscale) if it looks soft.
- **Fragment cost of the forward pass**, if `meshes` still dominates with the pre-pass:
  fog evaluates the sky model per fragment (about half the fragment shader's arithmetic);
  per vertex would cost a slight change where large triangles cross the horizon. Finer
  light tiles at night (above). The pre-pass at Medium on desktop GPUs, if they turn out
  fragment bound too.
- **Dynamic casters in the far cascade**: a moving NPC car anywhere in it makes it redraw
  on its interval; a separate small map (or leaving cars beyond 100 m out of it) would let
  it be kept while traffic moves.
- **Mailbox vs FIFO** on gamescope for latency at 90 Hz.
- **Far terrain detail level** (a coarser mesh at Low) and **chunk facade LOD beyond 400 m**
  (`docs/RENDERING.md`), not implemented.
- **Corona fill at night**: large sprites near lamps; measure before limiting their size.
- **The web build on integrated GPUs**: WebGL2 has no timestamps; measure frame time only.
- **Automatic preset by measured frame time** (drop a preset after N slow seconds) instead
  of by adapter name.
