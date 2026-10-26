# Rendering

The look: **2002 geometry, 2026 light.** Early-2000s console density (the GTA III/Vice City, Burnout 3, Midnight Club 2 generation), flat-shaded with tiny pattern textures, rendered through a modern HDR pipeline with a real sky, real shadows, haze, bloom and a moving sun. If a frame could be mistaken for a great PS2/Xbox screenshot that someone lit beautifully, it is right.

## The fidelity bar (read this twice)
Not a DOS game, not flat boxes on a plane. A street must have: curbs with a gutter, sidewalks with seams, crosswalks and lane paint, lamp posts with arms and heads, signs on poles, trees with volume (3–5 lobed canopies, a trunk), hydrants, parked cars. A building must have: a plinth, window insets with sills, a storefront bay with a door and awning where it faces the street, a cornice or parapet, roof clutter (vents, a water tank downtown), a sign band, drainpipes on brick. A car must read as a specific shape from 30 m with seams, mirrors, rims and lights. Then lighting makes it sing.

## Style rules
- Geometry: hard silhouettes, face normals (smoothing only on wheels, canopies, rounded car panels and curved sky objects). Buildings are extrusions **with facade geometry**, never bare boxes. Props 300–1500 triangles. Cars 3–6k body + 1.5–3k interior.
- Color: vertex colors from a per-place palette (16–32 colors per era) plus one emissive channel. **Tiny pattern textures are allowed and expected** where geometry is wasteful: asphalt grain, concrete, brick and stucco courses, window glass with a mullion cross, roof gravel, sand, lawn, wood siding, awning stripes. 64–256 px, generated procedurally at startup from a few parameters (never photos, never shipped bitmaps), stored in one atlas, sampled with nearest/linear and no mips below 32 px so they stay crisp and period-looking. Lane markings via vertex strips or the atlas.
- Lighting is where the fidelity goes: physically plausible sun and sky, cascaded shadows, atmosphere, bloom on emissives, reflections on wet roads and water, night lights with real falloff.
- Readable at a glance from a moving car. Contrast and silhouettes over clutter, but clutter exists.

## Pipeline (forward, HDR)
```
shadow pass    sun CSM, 3 cascades (0–40 m, 40–160 m, 160–600 m), 2048² each, PCF 3×3
depth prepass  (optional on web; on for native)  → used by fog/SSAO later
main pass      → HDR Rgba16Float: Lambert diffuse + simple specular (Blinn-Phong, low) for cars/water,
               sun with CSM shadow, sky ambient (hemisphere: sky color up, ground color down),
               up to N point/spot lights per object via per-chunk light lists (WebGL2: N=8 per draw, uniform array),
               height + distance fog tinted by sky toward the sun, emissive channel added last
sky            analytic sky model (Preetham or Hosek-Wilkie fit, evaluated in the shader per pixel) + sun disc + moon + stars at night
transparents   water (planar reflection, phase 2), windows glow quads
bloom          threshold on emissive/bright, 5-level downsample/upsample (Rgba16Float, half-res)
tonemap        AgX or ACES-fitted, exposure from time-of-day curve (no auto-exposure in v1), then sRGB; optional
               style post (cel/dither/CRT) here
HUD            2D pass: minimap and full map, speed, station, menu text (bitmap font + map texture)
```
HUD layout: a virtual 1280×720 space letterboxed into the largest centered 16:9 box (16:10 Deck and 4:3
get bars, so nothing stretches), shrunk to `1280/ui_scale × 720/ui_scale` for the UI scale (1–2×), with a
3% safe-area inset for TVs. Items are text runs, rectangles and images, drawn in order as alpha-blended
instanced quads in one draw from a fixed 4096-quad buffer (extra quads are dropped and counted). The font is
our own bold 7×9 pixel face for ASCII 32–126 (`render::font`, `const` bit rows rasterized at startup); text
is drawn at a whole number of pixels per font pixel with nearest sampling, so it is crisp at any resolution.
Image quads (`HudImage`) show the map texture (`Renderer::upload_map`, once at load: RGBA8 with a CPU-built
mip chain, sampled trilinearly at a per-quad mip level), a flat fill or the player arrow (a dart evaluated
in the shader), with the content rotated by any angle and an optional circle or rounded-rectangle mask, all
edges anti-aliased in the fragment shader. The HUD bind group holds the font atlas and the map (two textures,
two samplers); each quad's flags pick one, so images cost no extra draw call or texture switch.

Map (`app::map`): the Place rasterized once at load into a 1024 px (longer side) image, north up: muted zone
tints, parks, water with a darker shoreline, faint building footprints, roads by class with a 1 px darker
outline (highways thick and light, arterials pale yellow, locals thin, alleys faint), POI dots; deterministic
(pinned hash). The minimap is a 200-unit circle at the bottom left of the safe area, centered on the car,
turned with the heading (or north-up), 100/150/250 m across, with the player arrow and a north badge; hidden
under menus. The full map (MapToggle) fits the whole place into the safe area north-up, with the player
arrow, POI labels (invented names), the time and the controls; it pans and zooms (1–8×) and is not a pause.
WebGL2 constraints (CI target, fallback in the wild): no compute, no storage buffers, uniform arrays ≤ 16 KiB, ≤ 16 textures per stage, depth sampling via comparison sampler, no texture arrays for cascades (use an atlas 3×1). Everything above fits. WebGPU/native get larger light lists and SSAO.

## Time of day and sky
A `Daylight` struct is derived from `(latitude 34°N, day-of-year, seconds)`: sun elevation/azimuth, sun color/intensity (through a simple atmosphere: scattering tint by air mass), sky model params (turbidity by weather), moon phase/position, exposure, fog density/color, streetlight on/off (civil dusk), window emissive scale, star brightness. Default clock runs at 10× real time (24 h in 2.4 h); configurable 0× (frozen), 1×, 10×, 60×, and a scrub control. Golden hour and blue hour are the showpieces; tune there first.

## Night
Street lights are warm sodium (80s) point lights with a 25 m radius; neon signs are emissive meshes with matching small point lights; windows are emissive quads with a mask; headlights are two spotlights with a 60 m cone casting no shadow (v1), lighting the road and props ahead; tail lights emissive. Light culling: per chunk, a list of lights; per draw, the 8 nearest. Phase 2: clustered lighting on WebGPU/native.

## Reflections and wetness (phase 2)
Ocean/canal water: planar reflection render at half res of sky + near chunks, Fresnel blend, low-poly waves by vertex displacement. Wet-road mode: darker asphalt, screen-space reflection of emissives only (cheap), puddle mask by noise. Rain itself is a later feature.

## Cameras
First person cockpit (eye position from the spec; head sways with suspension and g-forces, small lag; dashboard and wheel visible; mirrors are optional render-to-texture later), hood, chase near/mid/far (spring-damped follow with lookahead, collision pushes in), cinematic orbit when idle. Vertical FOV 58° cockpit (≈ 92° horizontal at 16:9), 55° hood, 50° chase. All switchable by one key/button, order cycles.

## Mesh and instance budgets
Chunk meshes: pooled vertex/index buffers (fixed capacity, free-list per chunk slot), up to 256k vertices per chunk (32-bit indices). Vehicles and props: instanced draws (per-instance transform + color). Target draw calls per frame ≤ 400 web, ≤ 1000 native. Vertex: position f32×3, normal f32×3, uv u16×2 (atlas), color rgba8, emissive rgba8 (36 bytes; emissive alpha bit 7 = window flag, low 7 bits = intensity 0..8×). Props and cars use 2 LODs (full within 80 m, half beyond); chunks beyond 400 m drop facade detail to silhouettes.

## Platform tiers
| Tier | Target | Notes |
|---|---|---|
| Web | Chrome/Firefox/Safari, integrated GPU, 1080p | WebGPU, WebGL2 fallback; this is what CI tests |
| Steam Deck | SteamOS (Linux x86_64), RDNA2 iGPU, 1280×800 @ 60, Vulkan | gamepad-only, every menu navigable with a stick and two buttons; 16:10 HUD layout |
| TV via Deck dock | 1080p/4K output, viewed from 3 m | UI scale setting (1×–2×), large-text HUD, safe-area margins; same binary |
A resolution-scale setting (50–100%) and a shadow/bloom quality tier keep the Deck at 60 at native res.

## Optional styles (later)
Cel: quantized Lambert + edge detect from depth/normal. Dither/CRT: ordered dither to palette, scanline/curvature post. VHS: chroma shift, blur. All in the post pass; off by default.

## Debug
A debug overlay (toggle) showing frame time graph, draw calls, chunk budget use, light count, and camera/sun parameters, drawn with the HUD font. Screenshot command writes a PNG (native) or triggers canvas download (web).

## Acceptance tests
- Renders under WebGL2 (CI screenshot, SwiftShader) and WebGPU (manual) with identical scene content.
- Golden screenshots at four fixed cameras/times for seed 1 compared with a perceptual threshold; changes require intent in the PR.
- Frame budget: synthetic scene of 9×9 chunks + 64 cars at 1080p under 16.6 ms on the reference GPUs (manual gate), and under 2 ms CPU submit time on the CI runner.
- No frame allocates GPU resources after warmup (assert in debug builds).
