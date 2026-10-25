# Rendering

The look: **2002 geometry, 2026 light.** Low-poly hard-edged meshes with flat and vertex colors and almost no textures, rendered through a modern HDR pipeline with a real sky, real shadows, haze, bloom and a moving sun. If a frame could be mistaken for a PS2 screenshot that someone lit beautifully, it is right.

## Style rules
- Geometry: hard silhouettes, face normals, no smoothing groups except wheels and curved sky objects. Buildings are extrusions. Props are < 200 triangles.
- Color: vertex colors from a per-place palette (12–24 colors per era) plus one emissive channel. Textures only where a pattern cannot be geometry: a 4-color dither/noise tile for asphalt and sand grain, lane markings via vertex strips, window grids via geometry, instrument faces procedurally drawn. No photo textures ever.
- Lighting is where the fidelity goes: physically plausible sun and sky, shadows, atmosphere, bloom on emissives, reflections on wet roads and water, night lights.
- Readable at a glance from a moving car. Contrast and silhouettes over detail.

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
HUD            2D pass: minimap texture, speed, station, menu text (bitmap font, one texture)
```
WebGL2 constraints (CI target, fallback in the wild): no compute, no storage buffers, uniform arrays ≤ 16 KiB, ≤ 16 textures per stage, depth sampling via comparison sampler, no texture arrays for cascades (use an atlas 3×1). Everything above fits. WebGPU/native get larger light lists and SSAO.

## Time of day and sky
A `Daylight` struct is derived from `(latitude 34°N, day-of-year, seconds)`: sun elevation/azimuth, sun color/intensity (through a simple atmosphere: scattering tint by air mass), sky model params (turbidity by weather), moon phase/position, exposure, fog density/color, streetlight on/off (civil dusk), window emissive scale, star brightness. Default clock runs at 10× real time (24 h in 2.4 h); configurable 0× (frozen), 1×, 10×, 60×, and a scrub control. Golden hour and blue hour are the showpieces; tune there first.

## Night
Street lights are warm sodium (80s) point lights with a 25 m radius; neon signs are emissive meshes with matching small point lights; windows are emissive quads with a mask; headlights are two spotlights with a 60 m cone casting no shadow (v1), lighting the road and props ahead; tail lights emissive. Light culling: per chunk, a list of lights; per draw, the 8 nearest. Phase 2: clustered lighting on WebGPU/native.

## Reflections and wetness (phase 2)
Ocean/canal water: planar reflection render at half res of sky + near chunks, Fresnel blend, low-poly waves by vertex displacement. Wet-road mode: darker asphalt, screen-space reflection of emissives only (cheap), puddle mask by noise. Rain itself is a later feature.

## Cameras
First person cockpit (eye position from the spec; head sways with suspension and g-forces, small lag; dashboard and wheel visible; mirrors are optional render-to-texture later), hood, chase near/mid/far (spring-damped follow with lookahead, collision pushes in), cinematic orbit when idle. FOV 75° cockpit, 65° chase. All switchable by one key/button, order cycles.

## Mesh and instance budgets
Chunk meshes: pooled vertex/index buffers (fixed capacity, free-list per chunk slot). Vehicles and props: instanced draws (per-instance transform + color). Target draw calls per frame ≤ 300 web, ≤ 800 native. Vertex: position f32×3, normal octahedral-encoded u16×2, color rgba8, emissive rgba8 (24 bytes).

## Optional styles (later)
Cel: quantized Lambert + edge detect from depth/normal. Dither/CRT: ordered dither to palette, scanline/curvature post. VHS: chroma shift, blur. All in the post pass; off by default.

## Debug
A debug overlay (toggle) showing frame time graph, draw calls, chunk budget use, light count, and camera/sun parameters, drawn with the HUD font. Screenshot command writes a PNG (native) or triggers canvas download (web).

## Acceptance tests
- Renders under WebGL2 (CI screenshot, SwiftShader) and WebGPU (manual) with identical scene content.
- Golden screenshots at four fixed cameras/times for seed 1 compared with a perceptual threshold; changes require intent in the PR.
- Frame budget: synthetic scene of 9×9 chunks + 64 cars at 1080p under 16.6 ms on the reference GPUs (manual gate), and under 2 ms CPU submit time on the CI runner.
- No frame allocates GPU resources after warmup (assert in debug builds).
