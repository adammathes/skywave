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
- `Downtown` 4–8 blocks behind the strip center; `Commercial` ringing it; `Residential` grid filling the plain; `Suburb` (curvy) on gentle slopes; `Hills` (winding roads, sparse houses) on steep terrain; `Farmland` on flat valley floor far from downtown; `Industrial` near the highway and the harbor; `Park` 2–4 patches including one on the shore; `Highway` corridor round the town (stage 3). The edge bands take their own zones (see "Map edges").
- Rules are weighted scores per cell (distance to coast, slope, distance to downtown seed) + noise; the template sets weights. Different seeds move the bay, the downtown and the pass.

### 3. Arterials and highway (macro)
- Tensor field (Chen et al. 2008 style): a grid tensor aligned to the coast tangent in `Strip`/`Downtown`/`Residential`; radial tensor around downtown center; terrain-following tensor (perpendicular to gradient) in `Hills`; axis-aligned section grid in `Farmland` (1-mile sections subdivided to half/quarter miles).
- Trace arterials as streamlines of the major eigenvector from seed points (strip ends, downtown, the highway's highest point), with minimum spacing 250–400 m, snapping to existing nodes within 40 m, stopping at water and at the edge bands. The beachfront road follows the beach band. One `Highway` is a bypass that loops: it leaves the coast at the harbor's end of the place, runs inland 60 m inside that side's band, turns along the foot of the hills (a fixed distance behind the smoothed shore, clear of downtown) and comes back to the coast along the other side, its two ends on the beachfront road. Town roads that stop just short of it join it at grade (up to five, 320 m apart); a road that crosses it on a straight, level stretch with shallow banks gets a diamond interchange, and other crossings are overpasses (road profile includes height, bridges are implied by profile > terrain + 3 m).
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

## Map edges
Open-world games of the early polygon eras bounded their maps with water and mountains; so does every place
here. A template's `edges` says what bounds each side (`crate::edges`, `place::EdgeKind`):

| Kind | Inside the place | Past the side (the horizon mesh) |
|---|---|---|
| `Ocean`, `Lake` | the coast: beach and surf, the sea floor falling away | open water to the horizon |
| `Bay` | the coast | water, and a far shore of hills 1.5–2.3 km out |
| `Mountains` | a 90–140 m cliff wall rising over 96 m | ridges climbing to 300–600 m |
| `Hills` | a 55–85 m wall rising over 60 m | rolling hills, ridges to ~170 m |
| `HighwayLoop` | as `Hills`; the highway turns back along it | as `Hills` |
| `Forest` | a 20–30 m wooded bluff (`Park` zone) | a low forest plateau |
| `RailYards` | a 7–9 m embankment; 160 m of industry inside it | flat yards |
| `Desert` | a 32–48 m escarpment of mesas rising over 40 m (`Farmland` zone) | the desert floor to the horizon, buttes standing out of it |

The coast side must be water and the others land. A land side's wall stands in a roadless band (the wall,
24 m of crest wander and a 28 m clear foot); roads, lots and points of interest stay in the playable
rectangle inside the bands, so streets end at the foot of the hills, at the highway or at the beach. The
wall rises from the higher of the ground and the ground at the band's foot, its face steeper than 1.5:1, and
the collider gives the face a vertical wall about a meter up it, as it does a building's walls (a raycast
car's wheels would otherwise read a steep face as a step and ride up it): the car meets the cliff it sees
and stops there, even launched at it at 45 m/s. Across the
water the walls of the neighboring sides rise out of the sea as headlands. Water deeper than a car's hubs
drags it to a stop; after 1.5 s in deep water it goes back to its start with a "Deep water: resetting" toast.
`california_coast`: ocean west, mountains east, highway loops north and south. Tests: every road of seeds
1–8 stays in the playable rectangle and the highway is one chain whose ends are on the beachfront road
(`world/tests/place.rs`), the walls rise at least 50 m (85 m for mountains) from the band's foot, and the cars
cannot climb them or drive out to sea (`app::drive` tests).

## Inland basins
A template with a `basin` (and no `coast`, `elevation` or `layout`) is an inland place: `socal_basin`, the 2001
world's (`assets/regions/socal_basin.ron`; `procgen::basin`, `template::Basin`). Every side is land: hills on
`basin.hills.side` (a `Hills` edge), a desert fringe on `basin.desert.side` beside it (a `Desert` edge), the
other two any land kind (`socal_basin`: rail yards south, mountains west). There is no water; `water_level_m`
lies under all ground. The macro stage, in the frame of the hills' side:
- **Floor and hills.** The floor tilts up toward the hills (`tilt`) and rolls a little (`relief_m`); the foot
  of the hills lies `hills.depth_m` in from their side, wandering ±20 m, and the hills rise from it to
  `hills.height_m` at their band (smoothstep, swelling and gullied by `hills.relief_m`).
- **Grid.** Arterials every `grid.spacing_m` both ways: "cross" lines parallel to the hills, the first along
  their foot (24 m out), the last leaving room for the airfield before the far band; "out" lines from the
  first cross line to the last, from 40 m inside the far side's band to the desert. One interior line each way
  is a boulevard: two one-way carriageways of `grid.boulevard_lanes` lanes (3.4 m each) either side of a
  `grid.median_m` median, their median-side sidewalks and street trees making the planted median; a
  boulevard's end that meets no other line gets a short link between its carriageways. The local streets lay
  the grid's blocks square to its axes (the industrial zone shares the grid pattern with its commercial
  frontage, at 100 m blocks with truck-width streets).
- **Freeway.** Between two cross lines near the middle, from the first out line (where it ends at grade, a
  T) east into the desert, round a 220 m turn along the desert's band 60 m in, back to the last cross line
  (where it ends at grade): the desert highway that turns back along the edge. The cross lines between it
  and the last stop 150 m short of its desert leg. Every out line crosses it on an overpass (the coast's deck
  rule); `freeway.interchanges` of the crossings (not the boulevards') get ramps: diamonds, and `loops` of
  them a partial cloverleaf: in the quadrant before the bridge on the side its traffic keeps to, a loop
  on-ramp leaves the crossing road 85 m out, turns 225° on a 38 m radius and merges 220 m past the bridge;
  that direction's off-ramp runs round the outside of the loop to the road 132 m out; the other direction
  has the diamond's two ramps.
- **Channel.** Between the first two out lines, from 80 m into the hills to the far edge (cut through its
  wall): a trapezoid `channel.depth_m` under the ground beside it (7.5 m, under the 8 m a road profile may
  stand off the terrain), a `bed_m` bed and `wall_m` walls, deepening over its first 40 m. Every road over
  it is a bridge: the arterial stage reads the ground with the channel filled (`BasinPlan::bridge_heights`),
  so profiles run level across and the road mesh draws the deck and its piers. At the first bridge past
  every `ramp_every_m` along it a ramp 8 m wide runs down one wall (alternating) at `ramp_grade`, from 16 m
  off the bridge. Its right of way (`row_m`) is `Highway` zone, so streets, lots and buildings stop at its
  banks; the Place carries its concrete as `WaterKind::Channel` bodies (lip to lip, and the ramps), which
  the ground mesh paints concrete (paler bed, darker walls) with a darker low-flow strip down the middle
  of the bed and expansion joints across it every 6 m (flat marks 2 cm over the ground), and the map grey.
- **Rail.** Between the first two cross lines, edge to edge (into the bands' walls), at grade with every road
  it crosses; `rail.spurs` sidings 4.5 m beside it (off a 30 m turnout, ending at a bumper) between the out
  lines in industrial land; its right of way (`row_m`) is `Highway` zone. The detailing lays it as
  `PropKind::Track` pieces (ballast; flush crossing panels where it crosses a road, with a `Crossbuck` on
  each approach, on the right; a girder deck over the channel; a bumper at a spur's end).
- **Airfield.** Between the last cross line and the far band, past the channel: a `Park`-zone field
  `airfield.width_m` across and `length_m` long (no trees), its runway (`runway_m` wide, no sidewalks; the
  future drag strip) and a taxiway to the last cross line.
- **Zones.** Scored as on the coast with four more curves for basins (`Score::hills`, `rail`, `arterial`,
  `boulevard`: distance out from the foot of the hills, from the rail line, from the nearest arterial and
  from the nearest boulevard), then by structure: the hills, the desert fringe (`Farmland`: open dirt and
  section roads), the rights of way (`Highway`), the airfield and parks (`Park`), the bands.
- **Overlooks** stand on the highest benches of the hills (no steeper than 14% over 32 m, a third to two
  thirds of the way up; gentler ground where there is none) and the arterial stage's hill roads climb to them.

A non-freeway lane through a right of way takes the zone at either end of it (else industry's), so a shape
weighed only for freeway lanes (the semi) never starts on a road across the rail line or the channel.
Set pieces (below) are landmark kinds placed on this layout. Tests: `world/tests/basin.rs` (seeds 1–4 pinned; every road in the
playable rectangle, the freeway one chain ending at grade, overpasses, a loop, the bridges, the crossings,
the runway, a road to an overlook).

## Set pieces
A basin's set pieces (`docs/backlog/MIRAGE2001.md` item 7) are places and NPC behaviour, never missions:
no scores, no targets, no timers that press the player, and nothing can be hurt. Each is a landmark kind
(`place::PoiKind`: `DragStrip`, `ChannelRamp`, `Tunnel`, `CrewGarage`, `TacoStand`, and the basin's
`Overlook`s, now in the Place's points of interest, so the map names them and walkers gather at the ones by a
walk) sited by `procgen::setpieces::site` on the basin's layout once its buildings stand, with its props and
lights laid by the detailing (`procgen::setpieces::dress`; meshes and collision boxes in `world::setpieces`).
Nothing else the detailing places stands on a set piece's ground. A place without a basin has none, so the
coast's Place is unchanged.
- **The tunnel.** The straight stretch of the freeway nearest downtown, 128 m long, that no other road (ramp,
  overpass or street, asphalt and walk) comes within 2 m of and no building within 3 m of its walls: roofed
  over as a cut-and-cover box (`TunnelPiece`s of 16 m following the road's grade, 25 m between the walls, 6 m
  clear): glazed white tile over a teal band and a trim line, a lamp strip under the ceiling along each side,
  always lit, and a `Flood` light every 16 m each side; a portal's headwall at each end. Its walls are collider
  boxes. On seeds 1–3 it is on the freeway's straight leg east of downtown; on seed 4, whose straight leg has
  no clear stretch, on the desert leg.
- **The crew garage.** A site 24 m wide and 25 m deep square to a road, behind its walk, over industrial lots
  (one or several side by side) and clear of every other road; the one nearest the boulevard. The lots it
  covers lose their buildings. A three-bay block garage (`CrewGarage`) 9 m back from the property line: bays
  open, roll-up doors drawn up, a pale lit interior with tubes (`Flood` lights), a backlit box sign over the
  bays lettered with an invented name and a garage trade from the 2001 lexicon (`GARAGE`, `BODYWORK`, `WHEELS`,
  `TINT`, `CAR AUDIO`). Three `HeroCar`s: two nosed out in the outer bays, one angled on the apron; the app
  draws each as one of the world's hero cars it allows (`scenario::Lineup::heroes`, in the scenario's
  order), in one of its own colors. Its walls, piers and back wall are collider boxes; a car can drive into
  the middle bay.
- **The taco stand.** A site 22 by 20 m the same way over commercial, strip or industrial lots on an arterial
  or collector, the nearest the boulevard at least 300 m from the garage: a walk-up stand (`TacoStand`: a
  serving window under a striped awning, a lit menu, a lit roof sign with an invented given name and `TACOS`
  or `TAQUERIA`), three `PicnicTable`s and a row of five parked cars, some all day, some for lunch and
  supper.
- **The drag strip.** On the airfield's runway: a painted start line 60 m from its taxiway end, a finish a
  quarter mile on (an eighth where the runway is short), a `StartTree` a meter past the start on the runway's
  middle (the two lanes either side of it), the `TimingBoard` beside the strip 30 m on facing back at the
  line, street lamps every 100 m along both sides. In the app (`app::dragstrip`) a car stopped square on the
  line in a lane for a second gets the tree: the staging lamps, three ambers half a second apart, the green;
  the clock runs from the nose leaving the line (a red lamp if before the green, still timed) to the finish
  line, and the board shows the time and the speed over the line, with a toast. Only the last run is kept.
- **The overlooks.** At the end of each overlook road: a guard rail 9 m past the end across the view, two
  benches facing out, a lamp.
- **The grade crossings.** Every road the rail line crosses at grade: a `GradeCrossing` landmark on the track's
  middle, and a `CrossingGate` (mast, crossed blades, twin lamps, bell, the arm's pivot) on each approach on the
  right, its arm reaching across the approach to 0.3 m short of the road's middle (across the whole road on a
  one-way road, which gets the gate on its one approach only); they replace the crossing signs. The spurs' track
  pieces carry `TRACK_SPUR`, so the main line is the pieces without it. The train and the gates run in the game
  (`docs/NPCS.md`, "The train").
- **The channel ramps.** Chain-link along both lips of the flood channel, 0.6 m outside them, open within 4 m
  of any road (the bridges) and round each ramp: along a ramp's outer edge from 12 m below its top, and across
  its foot. A `ChannelRamp` landmark marks each ramp's top.
- **The landmarks** built from kits ("Landmarks" below), each a `PropKind::Landmark` at its lot's front middle
  facing out, its kit's lamps as `Flood` lights, its lot's buildings and other props gone, and a point of
  interest at its lot's middle. The variant each wears is the owner's pick from the gallery's contact sheet
  (`procgen::setpieces::{LETTERS, WATER_TOWER, DRIVE_IN}`); a kit the grammar does not carry is not sited.
  - **The hillside letters** (`hillside_letters`, `HillsideLetters`): from the road along the foot of the hills,
    every 16 m, up the slope until it has risen 50 m; there the lot faces down the local slope (within about
    45° of straight out over the valley), its letters' line level to within 3 m, the whole lot inside the
    playable area, at least 12 m from every road's asphalt and walk and 40 m from an overlook; of those, the
    one most nearly across from downtown. Its base is the lowest ground under the letters' line, so they are
    buried a little at their feet and never float; they block a car on the slope.
  - **The water tower** (`water_tower`, `WaterTower`, variant B, the pedestal): a site its kit's lot wide and
    deep over industrial lots on any road, at least 200 m from the garage and the stand, with the most
    industrial zone within 192 m of its middle (sampled every 32 m). Its column (not the bowl over it) blocks a
    car.
  - **The drive-in** (`drive_in`, `DriveIn`): a 48 by 44 m site over commercial, strip, industrial or desert
    lots on an arterial or collector, the nearest the boulevard at least 300 m from the garage, the stand and
    the tower. Its screen faces the road over three rows of speaker posts; after dark (from 19:00 or 20:00 to
    23:00 or 01:00, by stall) fifteen cars park in its rows facing the screen (lot cars, `LOT_CAR`). The screen
    glows dusk to dawn.

  On seeds 1–4 every basin has all three.

Tests: `world/tests/setpieces.rs` (every set piece on seeds 1–4, clear of roads and buildings, the tunnel's
walls hold a car to the freeway, a car drives through it and into the garage's middle bay; every landmark
sited, lit, solid and clear of roads and buildings, the letters on the ground up the hills, the drive-in empty
at noon and full at nine, the tower's column and the drive-in's screen stop a car), the meshes' and boxes'
unit tests in `world::setpieces`, `app::dragstrip`'s tests.

## Landmarks
Landmarks are the places you would slow down to look at (the feel cards, 2026-10-28). Each is a kit
(`assets/landmarks/<id>.ron`, ADR 0007, `world::landmarks`): boxes, struts, cylinders, lathed profiles and
lamps in named paints, in the lot's frame (`x` across, to the right seen from the road; `y` up; `z` toward the
road; the lot spans `x` -w/2..w/2 and `z` -d..0), with lettered variants that start `from` an earlier one and
add parts or repaint. A place stores one as `PropKind::Landmark { kit, variant }` at the middle of its lot's
front edge facing the road; `kit` indexes the kits in file-name order. Parts that start within 2 m of the lot
block a car (an upright lathe only as wide as it is within those 2 m, so a pedestal's bowl is not a wall
under it); each `Lamp` is a `Flood` light. Three kits ship: `drive_in` (a drive-in theater's screen, speaker
rows and marquee), `hillside_letters` (the valley's name on the hills) and `water_tower`; the basin sites one
of each ("Set pieces" above).

Taste defaults until the owner overrules: mostly grounded (a real kind of structure a place would have),
a few roadside-kitsch pieces (`tone: Kitsch`); signs only on landmarks and the main drag; a landmark shows
on the map once you have seen it.

**The gallery** (`place=gallery`, `world::landmarks::gallery`) lines every kit's variants up on flat lawn:
one east-west road per kit, its variants' lots north of it from A westward, each with a lit label post
lettered with the variant, dressed in the scene's world (`scenario=`). `app::gallery` places fixed cameras
for each variant (`front` and `quarter` from the hood, `street` from the far chase camera with the car for
scale) and writes them to `tools/screenshot/gallery.json` (`cargo test -p app -- --ignored write_gallery`;
a test fails while it is stale). `node tools/screenshot/shoot.mjs --gallery <id>` shoots each at noon,
golden hour and night and tiles them into `out/gallery/<id>.png`: a column per variant, a row per time and
camera. A landmark PR attaches that sheet and the owner picks a letter.

Tests: `world::landmarks`' unit tests (inheritance, refusals, boxes and lights, the gallery's layout),
`app::places` (the kits load and round-trip, the gallery builds in every world), `app::gallery` (every
camera on open ground facing its landmark).

## Region templates
A `RegionTemplate` is data (RON): elevation profile, zone weights, block sizes, street patterns, tree species, prop densities, sign languages, driving side. v1 ships `california_coast`; the 2001 world ships `socal_basin` (above). Later: `midwest_river_town`, `desert_highway`, `new_england`, then `tokyo_ward` (requires narrow streets, mixed-use, elevated rail props; design later).

## Facade grammars
A `world::grammar::Grammar` is data (RON, `assets/grammar/<template>_<era>.ron`, ADR 0002):
the numbers and weights the building planner and the detailing read, one file per region
template and era (`docs/backlog/BUILDINGS.md`). `app::places::grammar()` parses and
validates it once; `build_place`, `ChunkPlan::new`, `build_chunk_meshes` and
`WorldCollider::build` take it beside the palette. The geometry builders stay in
`world::buildings`; a grammar says how big, how many and how often. Two grammars ship:
`california_coast_80s.ron` dresses the coast (and the test town), `socal_basin_2001.ron` the 2001
basin (below). Its B2 values are the
planner's former code constants (`shipped_grammar_keeps_the_former_constants` in
`crates/world/src/grammar.rs`); the three-band facade's (B4) are starting values from
`docs/backlog/BUILDINGS.md`.

As built (B2, B4, B8, B9):
```
Grammar     { era: Era, template: String, styles: [StyleRules], signs: SignRules, lots: LotRules,
              street: StreetDress }  (street optional: none, and the lettered ad billboards)
            (+ lexicon: Lexicon, attached by the loader from assets/grammar/lexicon.ron)
StyleRules  { style: StyleTag, zones: ZoneMask, massing, base, body, top, addenda }
  zones:    All | Only([ZoneId, ...])
  massing:  even_floors: ByUse<bool>, floor_h, ground_floor_h: (min, max), ground_share_max
  base:     plinth_h: ByUse<f32>, bay_w: ByUse<f32>, door: ByUse<(w, h)>,
            ground: ByUse<GroundBays { storefront, lobby, garage, wall, door }>  (weights),
            front_bay_w: (min, max), pier_w: (min, max),
            storefront: Storefront { bulkhead_h, glass_top, transom_h: (min, max),
                                     transom_glass_256, entry: (w, h), recessed_256,
                                     recess: (min, max), side_entry_256 }
  body:     window: ByUse<Window { w_share, w: (min, max), head, h_max }>,
            kinds: ByUse<WindowKinds { punched, paired, ribbon, bay, picture }>  (weights),
            window_h_min, sill_ground, sill_upper
  top:      parapet_h: ByUse<f32>, cornice_d,
            kinds: TopKinds { parapet, cornice, stepped, shaped, false_front, bracketed,
                              mansard }  (weights)
  addenda:  roof_vents: ByUse<(min, max)>  (a count),
            roof_units, exhaust_fans, skylights: ByUse<(min, max)>  (per 1000 m² of roof),
            bulkhead_floors, water_tank_1_in
ByUse<T>    { residential, office, other }   other: commercial, industrial, civic, hotel, parking
SignRules   { awnings: [(Awning, weight)], mall_canopies: [(Awning, weight)], lit_box_256,
              shop_lit_256, lexicon: String, trades: [TradeRule], forms: [(Form, weight)],
              wordmarks: [(Block | Condensed | Script | Slab, weight)], line_256, name_radius,
              fascias: [(Lettered | Bare | Band | Mural, weight)] (optional, empty: Lettered) }
StreetDress { bus_stop_m (0: none), faces: [(Ad | Blank | Pictogram, weight)] (empty: Ad) }
Awning      Striped | Solid | Canopy | None | Bullnose | Aluminum | Mansard
TradeRule   { trade: String, weight, pictograms: [(Pictogram, weight)],
              lines: [(Word(String) | Since, weight)], descriptors_out: [String] }
Lexicon     { id, syllables: { onsets, nuclei, codas }, given, surnames, descriptors, trades,
              lines }
LotRules    { stall_w, stall_d, aisle_w, aisle_w_60, angled_256, walkway, buffer,
              frontage: [(Hedge | Wall | Open, weight)], second_cut_m, wheel_stop_256,
              pole_m: (min, max), run_max, islands: [(Palm | Shrub | Bare, weight)],
              yellow_256, backed_in_256, rear_lot_256,
              occupancy: [(ZoneId, (24 hourly %))] }
```
**Which rules.** A style may have several entries. A building takes the first entry, in file
order, whose `style` is its style and whose `zones` hold its zone (the zone under its
footprint's centroid, `world::building_zone`); so zone-specific entries come before an `All`
one. `Grammar::validate` refuses a grammar that leaves some style without rules in some zone.
The coast grammar gives stucco and brick buildings in `Industrial` zones their own entries
(loading doors and blank wall instead of shops).

How the planner reads it, for a building of style `s` and use `u` in zone `z` (rules
`r = grammar.style(s, z)`, every `ByUse` taken at `u`; weights are drawn in listed order, per
building, from an `Rng` keyed by the building's door pick):
- **Floors.** Wall top `height_m`; the roof sits `parapet_h` below it (none on pitched roofs, at
  most 30% of the height), the walls start `plinth_h` above the lot. With `even_floors` every
  floor is the same; otherwise the ground floor is
  `clamp(span − (floors − 1) · floor_h, ground_floor_h).min(span · ground_share_max)` and the
  upper floors share the rest.
- **Three bands.** The street front of a building whose `ground` weights are not all zero (and
  whose style does not draw its own ground floor, as dingbats, warehouses, glass towers and
  motels do) gets a **base**:
  the ground floor split into `round(len / w)` bays (`w` drawn from `front_bay_w`, at most 12)
  with a pier (`pier_w`) on every bay line. Each bay is drawn from `ground`: a storefront, the
  lobby (one at most, moved to the middle), a garage, blank wall, or the door to the flats
  (one at most, never on one storey); a front left with no way in gets its likeliest entrance
  in the middle. A storefront bay is, bottom up, a bulkhead (`bulkhead_h`), display glass with
  thin mullions to `glass_top`, a transom bar and a transom (`transom_h`; glazed with chance
  `transom_glass_256`, else panelled). A run of adjacent storefront bays is one business: one
  awning hung from the transoms' top and one sign band above it span the run.
  On a low ground floor the glass and then the transom give way to the sign band, never below
  the entry's height. The entry (`entry`) is centred or, with chance `side_entry_256`, at one
  side, and on a building with chance `recessed_256` it is recessed by `recess` behind a floor
  and glass returns; otherwise flush. A belt course caps the base. The **body**'s windows are
  a whole number of window bays per base bay, so every pier has wall, not glass, over it.
  The window kind is drawn per building from `kinds`: punched (one per bay), paired (two
  narrow ones), ribbon (glass bay to bay with mullions on the bay lines; upper floors and
  door-less ground floors), bay (a projecting glazed box; upper floors) or picture (one wide
  one). The **top** of a flat roof is drawn from `top.kinds`: the plain parapet, a cornice
  projecting `cornice_d`, a parapet stepped up or shaped (shoulders and a curved crown)
  over the street front's middle, a false front (the street wall carried up 1.2–2.4 m;
  one storey only, taller buildings get the plain parapet), a deep cornice on brackets
  along the street front (two storeys or more, else the plain cornice), or a shingled
  mansard all round, 1.1 m down the wall and 0.7 m out (two storeys or more and not a
  motel, else the plain parapet: a one-storey front's signs reach the top of its wall, and
  a motel's walkway roof runs where the eave would).
- **Windows.** Bays of `bay_w` (1.5× on facades more than 30 m from a road, then widened until
  the triangle budget fits); a window is `clamp(bay_w · w_share, w)` wide and
  `min(upper floor − head, h_max)` tall, at least `window_h_min`; sills `sill_ground` above the
  plinth and `sill_upper` above each upper floor. Doors are `door`.
- **Triangles.** The planner aims for 2,000 triangles a building
  (`buildings::PLAIN_TRIANGLE_TARGET`); a building with a base band may use the 3,500 ceiling
  of `docs/backlog/ART.md` (`BUILDING_TRIANGLE_BUDGET`).
- **Roofs.** `roof_vents` is an inclusive count per flat roof (`(0, 0)` draws nothing).
  `roof_units` (packaged units: a cabinet on a curb with a duct elbow into the roof),
  `exhaust_fans` and `skylights` are rates per 1000 m² of footprint: a rate drawn from the
  range times the area, rounded at random, at least one when the range is above zero, at
  most 6 units, 3 fans and 8 skylights. They are counted at planning (`BuildingPlan::roof`,
  so the triangle estimate covers them) and placed toward the back of the roof, in the
  street front's frame, so the parapet hides most of them from the street, and only where the
  whole item stands on the roof; a strip mall's
  1.8 m parapet hides them all. Buildings of `bulkhead_floors` or more get a stair
  bulkhead, and one in `water_tank_1_in` of those a water tank.
- **Signs** (terrain places). The awning over each run of storefronts is drawn from
  `awnings` by weight in listed order: a striped or solid canvas shed, a flat stucco
  canopy, a vinyl bullnose, a ribbed aluminum shed, a shingled mansard pent, or none. Each
  hangs from the transoms' top down, under the sign band, its lowest edge 2.4 m over the
  floor; a transom too low for that gets none. A strip mall's canopy is drawn from
  `mall_canopies`: a shingled mansard on posts (0.9–1.5 m tall, 2–3 m out, the tenants'
  letters on its shingled face) or the flat canopy (any other kind). Where the shop is
  open late the awning's or canopy's underside glows at window strength, the spill of the
  light the detailing puts in front of it. `lit_box_256` and `shop_lit_256` are shares out
  of 256 of backlit-box signs and late shops.
- **Shop signs** (the blade signs the detailing hangs on storefronts; `world::signs`). Each
  draws a trade from `trades` by weight, a form from `forms` (`Name's`, `Name's Trade`,
  `Descriptor Trade`, `Trade`), a name from the lexicon's given names and surnames, a
  descriptor (never one in its trade's `descriptors_out`: pairs that would name a well-known
  business), a pictogram from its trade's `pictograms`, a lettering style from `wordmarks`
  and, `line_256` times in 256, a secondary line from its trade's `lines` (`SINCE` and a year
  2-35 years before the era). `Descriptor Trade` that would say a word twice becomes
  `Name's Trade`; a line that repeats the trade is dropped. `ChunkPlan` assigns them in prop
  order and redraws (up to 24 times) any sign whose main line or name a sign within
  `name_radius` already has. Every word comes from the lexicon (`assets/grammar/lexicon.ron`,
  closed and reviewed; names are two or three syllables from its `syllables` tables), which
  `app::places::load_grammar` attaches to the grammar after checking its `id` against
  `signs.lexicon`. A grammar parsed without it gets plain blade signs (a pictogram, no words).
  The blade puts the word first: bold stacked capitals up to 0.6 m tall (the stroke a fifth
  of the cap), or the main line reading upward in script, inside a neon frame; a backlit
  box (`lit_box_256`) letters dark on a lit panel. The pictogram (`signs::pictogram`, 32
  generic shapes as 2D polygons, extruded 4 cm proud of both faces and outlined in neon)
  goes above the word when the word leaves at least 0.4 m and the triangles for it, so short
  words get one. A sign spends at most 360 triangles (`props::BLADE_TRIANGLES`): a word that
  does not fit gives way to the trade, then to block capitals; the pictogram's neon goes
  before the pictogram. Blades carry no second line (the carriers of B7 will).
- **Letterforms** (`signs::letters`): stroke polylines on a 20-unit cap grid, ours, in four
  styles (block capitals, condensed sans, connected script, slab serif), built as neon tubes,
  dark strokes on a backlit box, extruded channel letters or flat paint. The sign bands and
  billboards letter their (still syllable-built, `signs::business_name`) names in the block
  capitals until B7 moves the band carriers onto the shop signs.
- **Lots** (B10, terrain places; see "Parking lots" below). The test town's strip-mall lots
  use `stall_w` too.

## The 2001 basin's grammar
`socal_basin_2001.ron` (`Aughts`, `socal_basin`) started from the coast's and changes what the turn of the
century changed: stucco boxes with plain parapets (no false fronts, hardly a cornice); auto shops in the
industrial stucco (roll-up bays and a shop front); mini-malls under flat canopies; vinyl and aluminum
awnings, little canvas; lots busy late into the night. Its filler keeps quiet so the landmarks stand out:
the buildings' sign bands (over a run of storefronts, a mini-mall unit, a motel's parapet) carry no names
(`fascias`: bare stucco 3, a plain fascia in the trim's color 4, a flat panel of one strong color 1, the
mural buildings' blank ground floor bays in it too; a motel or a mansard canopy without a name has no
band at all); the shops are told apart by their tops (stepped and shaped parapets, no cornices) and a
roll-up bay or a blank wall among the storefronts. Its signs draw from their own closed lexicon,
`lexicon_2001.ron` (`us_basin`: the coast's invented names, the period's descriptors and trades: taquerias
and taco stands, wheels, tint, smog checks, car audio, pagers and phones, nails), which `app::places`
attaches by the id the grammar names; the coast's `lexicon.ron` is untouched, so the coast's signs never
change. They letter the blade neon and the landmarks; a lettered fascia (the coast's, every one) keeps the
older syllable names of `signs::business_name`. Its trades leave out every descriptor whose
pair would read as a real chain, singular or plural (`descriptors_out`; their keys are on the test
blocklist and `no_descriptor_names_a_chain_with_its_trade` checks every descriptor and trade). Its `street` puts a bus stop (`PropKind::BusShelter`: posts, a flat roof, a glazed back screen, a
bench and a lit ad box across the downstream end) on an arterial's walk about every 320 m in commercial,
residential, downtown and suburban zones (a block's piece of arterial gets one with the chance of its share
of the spacing), and gives billboards and the shelters' ad boxes faces mostly blank, one in four a big pictogram
(`PropKind::FaceBillboard`), never the coast's lettered ads. The tilt-up warehouses, dingbats, the tract
houses' tile roofs and the downtown towers come from their styles as on the coast. The `sodium_night`
palette (the template's and the scenario's) paints it: beige and tan stucco, terracotta tiles, deep
sodium-orange lamps (the detailing reads a template's palette by name for its lights, `Palette::named`; the
coast's names none and keeps the 1985 one), purple, blue and green neon.

## Parking lots
`world::lots::plan` lays a terrain place's parking lots from its parcels, buildings and
roads and the grammar's `LotRules`, once; the ground meshes (`lots::mesh`, in the ground
stage), the road meshes (the curb cuts, `Network::cuts`), the detailing (poles, island
plants, frontage hedges, the parked cars) and the collider all read that one plan. Only
zones with an `occupancy` curve get lots (the coast: strip, commercial, downtown, beach).
- **Front lots** before strip malls. The building planner keeps the open ground for one:
  the strip mall's front setback is `walkway + 2 stall_d + aisle_w + buffer` (and 0.5 m),
  squeezed on shallow lots. The lot, from the shops out: the covered walk (`walkway`, at
  curb height), then two rows of stalls either side of an aisle, or one row and the aisle,
  whichever fits the yard, 90° (`stall_w` × `stall_d`, two-way `aisle_w`) or, with chance
  `angled_256`, 60° (one way through, `aisle_w_60`); then the planted strip along the
  street (`buffer`) with a hedge, a low wall or nothing (`frontage`). A curb cut at one end
  of the frontage, at both ends when it is at least `second_cut_m` long or the lot is
  angled (in upstream, out downstream); each cut's drive climbs a concrete ramp from the
  walk's back down to the lot and crosses the street-side row to the aisle. Painted arrows
  point in at the entries (out at an angled lot's exit, and along its aisle).
- **Alley lots** behind shops, offices and civic buildings that back onto an alley (or
  whose parcel fronts one): with chance `rear_lot_256` the planner keeps `stall_d` and
  0.6 m on the alley side, and the lot is one row of 90° stalls opening onto the alley,
  its curb dropped along them.
- **Beach lots** along the beach zone's alleys: a row of stalls each side opening onto the
  alley, wherever a stall's ground is clear of every other road, junction, building,
  parcel (parks and empty lots too) and the water. The beach zone's own lot parcels, the
  strips of sand between its alleys and roads, are where these lots go, so only they may
  be paved. Where the alley bends, a run whose stall or island comes out
  more than 0.3 m narrower than its width (the inside of the bend) is dropped.
- Every row is cut into runs of at most `run_max` stalls with a 1.8 m island between runs
  and at both ends, planted (`islands`) with a palm, a shrub or nothing. Light poles (the
  street light) stand in islands `pole_m` apart; a lot has wheel stops in every stall
  with chance `wheel_stop_256`; its stripes are yellow with chance `yellow_256`, else
  white. A lot is dropped whole if any stall or island would touch another road or a
  junction.
- **Ground.** The concrete ramp runs from the walk's back edge; the asphalt lies 5 cm above
  the graded terrain, cut to the terrain's own
  triangles (each piece in its triangle's plane), so it neither pokes through nor floats
  and never meets the road or walk meshes (lots stop at the property line); stripes and
  arrows lie a centimeter above it; islands (15 cm), wheel stops (12 cm), the walk and the
  low wall stand on it. A curb cut drops the road's curb: the gutter stays, a concrete apron
  rises from it to the walk's top (to the lot where there is no walk), and wings close the
  full curb at its ends.
- **Parked cars** on the stalls, centered, nosed in (backed in with chance
  `backed_in_256`), in the built-in parked-car shapes and paints. Each stall has its own
  threshold (a hash of where it is) and is taken through each hour whose `occupancy` for
  the lot's zone is above it: the car's `hours` mask in the Place
  (`place::PropKind::ParkedCar`; a lot's car also sets bit 24, `lots::LOT_CAR`). The
  clock says where each car should be (`props::parked_now`, each coming or going at its
  own minute past the hour); `WorldCollider::advance_lot_cars` moves it there only out of
  sight (never within `HOLD_M`, 60 m, whichever way the camera looks, nor within `SEEN_M`,
  150 m, in the half-plane ahead of it) and never
  onto the player's car, and the app draws, and the collider stops the car on, only the
  cars that are there. A chunk keeps at most `MAX_CARS_PER_CHUNK` (24) cars at its curbs
  and in its driveways, as before, and `MAX_LOT_CARS_PER_CHUNK` (16) in its lots at any
  hour. That second cap is a choice for the draw cost: a long beach alley's lot never
  looks full.
- **Collision.** Islands and wheel stops are raised patches (the wheels climb them like
  curbs), the walk a patch at curb height, the ramps planes, the asphalt a patch 5 cm over
  the terrain like the mesh; poles and plants are props; the low walls boxes; paint has none.
- Determinism: a parcel's lot draws from `Rng::new(seed, [LOTS, parcel key])`, a beach
  alley's from `Rng::new(seed, [LOTS, ALLEY | edge])`, in a fixed order.

Still in code, to move with the tasks that change them: the palette (`world::Palette` and
`buildings::look`), which styles get carports, walkways and step-backs, the step-back floors
(`STEP_BACK_FLOORS`, also read by the building generator), the triangle budget, and every
builder's own dimensions (sign bands, awnings and canopies, the parapet and cornice
shapes, rooftop units, the belt course). A value moves into
the grammar when a task first needs to vary it; a fitted value names its source
(`CREDITS.md`).

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
