# World generation (procedural producer + detailing pass)

Goal: a **place**, not a ribbon. Coherent macro structure you can learn, with local variety. Deterministic from a seed. Generated in chunks within a per-frame time budget. Produces a Place skeleton (`docs/PLACES.md`) and then detail.

## Pipeline
Everything is a pure function of `(seed, region_template, era, chunk)`; macro stages are computed once per Place (cheap, coarse) and cached; chunk stages run on demand.

```
Macro (whole place, ~ms):   coast & terrain → zone map → arterial network → highway
Chunk (on demand, ≤ 2 ms):  local streets → blocks → parcels → buildings → detailing (facades, props, lights, lanes)
```

### 1. Coast and terrain (macro)
- Region template `california_coast`: ocean along the west edge (configurable side), a coastline as a low-frequency curve with a bay or two; beach band 60–120 m; a coastal plain rising inland via 2–3 ridges to hills (120–250 m) at the east edge; one or two valleys with flat farmland.
- Heightfield: layered `fastnoise-lite` simplex + ridge noise, domain-warped, shaped by distance-to-coast and the template's elevation profile; then hydraulic-ish smoothing (a few box-blur passes at zone transitions) so roads sit well. Resolution 4 m. Water bodies: ocean below `water_level`; optional lagoon/canal network in `Residential` near the coast (the Venice/Naples canal motif).

### 2. Zone map (macro)
Zones drive everything downstream. Painted by rules on the coarse grid, then smoothed:
- `Beach` on the band along the coast. `Strip` (the main drag) parallel to the beach 1–2 blocks inland for 800–1500 m centered on the bay.
- `Downtown` 4–8 blocks behind the strip center; `Commercial` ringing it; `Residential` grid filling the plain; `Suburb` (curvy) on gentle slopes; `Hills` (winding roads, sparse houses) on steep terrain; `Farmland` on flat valley floor far from downtown; `Industrial` near the highway and the harbor; `Park` 2–4 patches including one on the shore; `Highway` corridor from the coast inland through a pass.
- Rules are weighted scores per cell (distance to coast, slope, distance to downtown seed) + noise; the template sets weights. Different seeds move the bay, the downtown and the pass.

### 3. Arterials and highway (macro)
- Tensor field (Chen et al. 2008 style): a grid tensor aligned to the coast tangent in `Strip`/`Downtown`/`Residential`; radial tensor around downtown center; terrain-following tensor (perpendicular to gradient) in `Hills`; axis-aligned section grid in `Farmland` (1-mile sections subdivided to half/quarter miles).
- Trace arterials as streamlines of the major eigenvector from seed points (strip ends, downtown, pass), with minimum spacing 250–400 m, snapping to existing nodes within 40 m, stopping at water. The beachfront road follows the beach band. One `Highway` is traced through the pass with limited access: on/off ramps at 2–4 arterial crossings, overpasses elsewhere (road profile includes height, bridges are implied by profile > terrain + 3 m).
- Graph cleanup: merge near nodes, remove dead-end stubs < 60 m except in `Suburb`/`Hills`, ensure connectivity.

### 4. Local streets (chunk)
Per zone pattern, seeded by chunk, constrained to arterial cells:
- `Downtown`/`Strip`/`Residential`: grid subdivision of arterial cells, block 80–120 m × 60–100 m, with alleys in `Downtown` and occasional T-intersections.
- `Suburb`: recursive curvy subdivision: a collector loop + cul-de-sacs (bulb radius 12 m), streets as splines with 60–120 m radii.
- `Hills`: contour-following switchbacks with max grade 12%, sparse, loops rare.
- `Farmland`: straight section roads only; dirt driveways to farm clusters.
- `Industrial`: large blocks 200 m, truck-width roads. `Beach`: the beachfront road, parking lots, a pier road.
Streets are split at chunk borders; cross-chunk consistency comes from hashing the border segment's canonical key, never from the neighbor chunk's output.

### 5. Blocks and parcels (chunk)
Polygonize faces of the local graph (half-edge traversal). Each block is offset inward by half road width + sidewalk to produce the buildable polygon, then subdivided along the longest frontage into parcels of zone-dependent width (Downtown 12–25 m; Residential 15–20 m; Suburb 20–30 m; Strip 15–40 m; Industrial 60–120 m; Farmland: whole section with a farmstead parcel). Odd slivers become `Park` or `Lot_Empty`. `spade` provides CDT where terrain triangulation needs constraints; `geo` for polygon offset/intersection (small, pure crates, MIT/Apache).

### 6. Buildings (chunk)
Per parcel, choose `use`, `height`, `style` by zone + era distributions, then a footprint: a rectangle inset from the lot (setbacks per zone), optionally with an L/U cut or a courtyard for larger lots; `Downtown` towers step back above 6 floors; `Strip` buildings sit at the sidewalk with a 1-floor sign band. Roof type per style. Height distribution: Downtown log-normal with mean 8 floors, max 30; Strip 1–3; Residential 1–2; Suburb 1–2; Industrial 1 (tall); Farm: house + barn + silo cluster. Coverage target per zone (e.g. Downtown 85%, Residential 35%).

### 7. Detailing (chunk; shared with compiled places)
- **Facades**: window grid from floors and width; ground floor type by use (storefront, lobby, garage door); awnings and sign slots on `Strip`/`Commercial`; `emissive windows mask` seeded per building so ~30–60% of windows light at night.
- **Props**: street lights at 30–40 m spacing on arterials (both sides), 60 m on locals; palms along `Beach`/`Strip`, oaks in `Residential`/`Suburb`, eucalyptus in `Hills`, power poles in `Farmland`; benches, hydrants, mailboxes by density tables; `NeonSign` on Strip buildings with era palette; billboards along the highway; parked cars 20–60% of curb length by zone from the era's vehicle roster.
- **Lights**: one point light per street light (warm sodium for 80s, white LED for 2010s+), per neon sign, per lit storefront; clustered later.
- **Lanes/sidewalks** for traffic (`docs/NPCS.md`).

### 8. Meshes (chunk)
`world::mesh` turns a detailed chunk into vertex-colored low-poly meshes: terrain (4 m grid, beach sand and grass by zone, slope darkening), roads as ribbons stamped into the terrain with curbs, sidewalks, lane markings as vertex color strips; buildings as extruded footprints with window quads in a second emissive channel; props as instances of a small shared prop mesh library (also generated, see `docs/RENDERING.md`). Output: position, normal, color, emissive, per vertex; 16-bit indices; capped at 64k vertices per chunk (buildings decimate windows first if over).

## Determinism and budget
- Every stage derives its `Rng` from `core::hash64(seed, [stage_id, chunk_x, chunk_y])`.
- Chunk generation is resumable in stages so the streaming budget can stop between stages.
- Reference seeds `1..=8` have golden hashes per chunk in `crates/world/tests/golden.rs`; changing generation means updating goldens in a dedicated commit with a before/after screenshot.

## Region templates
A `RegionTemplate` is data (RON): elevation profile, zone weights, block sizes, street patterns, tree species, prop densities, sign languages, driving side. v1 ships `california_coast`. Later: `midwest_river_town`, `desert_highway`, `new_england`, then `tokyo_ward` (requires narrow streets, mixed-use, elevated rail props; design later).

## Known hard parts (plan for them)
- Junction geometry at odd angles (use a generic polygon junction with per-edge trimming; no special cases per count).
- Roads on slopes: cut/fill the terrain to the road profile within road width + 4 m shoulder.
- Highway ramps and overpasses: generate as separate profile curves, not as graph tricks.
- Cross-chunk continuity: everything that crosses a border is generated from the border's canonical key.

## Acceptance tests
- Golden chunk hashes for reference seeds. Validation passes for all chunks of seeds 1..=64.
- Connectivity: ≥ 95% of edges in one component; a path exists from `PlayerStart` to every zone.
- Budget: median chunk generation ≤ 2 ms, p99 ≤ 6 ms on the CI runner (native release).
- Screenshots of seed 1 from four fixed cameras (strip at dusk, downtown noon, suburb morning, farmland sunset) checked visually at each milestone gate.
