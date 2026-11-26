# Authored layouts

A procedural region template invents a place's structure from scored noise: where the water is, where
the hills rise, where downtown sits, which way the arterials run. For a place someone knows, that is
the wrong way round: the structure should be **authored**, and only the fine grain generated.

An **authored layout** (`assets/layouts/<id>.ron`, `world::layout`) is a small hand-written map:
terrain shapes, water, districts and the main roads. `world::procgen::authored::build_place` turns
it into the same macro map and arterial road graph the procedural stages 1–3 produce, then runs the
shared stages 4–7 (local streets, parcels, buildings, set pieces, detailing) over it, dressed by
the scenario's region template's zone rules (block sizes, street patterns, styles, trees, props)
and grammar. The result is an ordinary `place::Place`; nothing downstream knows how it was made. A
future real-map import emits the same thing.

A scenario opts in with `layout: Some("<id>")`. Its region template then only supplies zone rules:
a template with no scored zones (`downtown`, `strip`, `industrial` and `coast` all `None`) is
valid for exactly this use.

Everything is still deterministic from the seed: the seed varies the fine grain (which lot gets
which building, the terrain's small relief), never the structure.

## Format
Coordinates are meters in the place's own frame: `(x, z)`, x east, z south, `(0, 0)` the
north-west corner of chunk `(0, 0)`. Angles are degrees clockwise from north.

```
Layout(
  id: "bay_loop",
  size_chunks: (32, 32),            // 128 m chunks: 4.1 km square
  ground_m: 6,                      // land height where no shape says otherwise
  water_level_m: 0,
  edges: Edges(north: Ocean, east: Hills, south: Hills, west: Ocean),
  // Terrain, added onto `ground_m`; the highest shape wins. Each fades to nothing at
  // `falloff_m` from its line or point. A ridge takes one `height_m`, or `heights_m` with one
  // height per point, interpolated along the line; `relief_m` roughens its crest.
  ridges: [ Ridge(name: "spine", line: [(760, 1600), (720, 2150), (900, 4060)],
                  heights_m: [20, 88, 24], falloff_m: 400, relief_m: 14) ],
  peaks:  [ Peak(name: "high crest", at: (820, 1060), height_m: 78, falloff_m: 340) ],
  // Water: `Sea` or `Lake` polygons, cut to `depth_m` under the water level with a shelf.
  water: [ Water(kind: Sea, depth_m: 4.2, polygon: [(0, 0), (2300, 0), ...]) ],
  // Districts: polygons painted in order (a later one wins), each a zone::ZoneId. Land no
  // district covers is `default_zone`. Districts are graded flat for building, except a
  // `hilly: true` one, which keeps its hills (a street grid laid straight over them).
  default_zone: Hills,
  districts: [ District(name: "city", zone: Residential, hilly: true, polygon: [...]) ],
  // Main roads: polylines, or `from` plus `bearing_deg` (or `toward` a peak) and `length_m`.
  roads: [
    Road(name: "loop", class: Arterial, lanes: 2, speed_kph: Some(80), points: [...]),
    Road(name: "lookout", class: Rural, lanes: 1, sidewalk: false, dead_end: true, points: [...]),
    // A grade-separated highway: roads cross over it; the ones `ramps` names get ramps.
    Road(name: "freeway", class: Highway, lanes: 2, ramps: ["summit avenue"], points: [...]),
  ],
  // Channels cut along polylines: a dry concrete flood channel, or with `wet: true` a river.
  channels: [ Channel(name: "wash", line: [(1280, 60), (1290, 1480)], width_m: 14, depth_m: 5) ],
  start: Start(road: "crest street", along_m: 160),   // the player starts this far along this road
)
```

Road fields beyond the line: `lanes`, `speed_kph` (default from class and lanes), `max_grade`
(default 0.12; hill streets may take more), `sidewalk` (default true), `dead_end` (default
false) and `deck_m` (a bridge's deck height over the water, default 3).

How roads are built: an end within 30 m of another road snaps onto it; ends and crossings become
junctions. Over water a road rides its deck; on land it follows the ground, limited
to its `max_grade`, and the terrain is then cut and filled under it (with a blended shoulder) so it
sits on the ground.

A `Highway` road is grade-separated. A road that crosses it does not meet it: it rises (4%
approaches) to a deck 7 m over the highway, so keep its junctions some 200 m clear of the
crossing or they are lifted too. Each road named in the highway's `ramps` gets a diamond: four
one-way ramps leaving the highway 150 m either side of the crossing. The highway's own ends meet
roads at grade like any road's, and a road that *ends* on a highway joins it at grade. The land
under a highway and inside its ramps is zoned `Highway`: no streets or lots.

A channel is cut after the roads are laid, so every road that crosses one bridges it. `width_m`
is its flat bed; the walls slope 1.5 m out per meter down. Dry, its bed lies `depth_m` (at most
7.8) under the ground and it is zoned `Highway`; `wet`, its bed lies `depth_m` under the water
level and it is water, for land that stands a few meters above the water level (ground plus
depth within 7.8 m). An end inside the place is a headwall; run the line to the place's edge to
carry it out.

Rules the loader checks (`Layout::validate`): names unique; every point inside the place; sane
values; a deck low enough to stay in the road profile band; every road end on another road unless
the road is a closed loop or a `dead_end`, so a road to nowhere is an error, not a default; ramps
only on a highway, each naming a road that crosses it at least 160 m from its ends; a channel
shallow enough to bridge; the start names a road and lies along it.

`tools/mapdump --layout assets/layouts/<id>.ron` draws the macro map and main roads to check a
layout by eye.

## Kept small on purpose
No scoring, no noise fields, no tuning curves. If a world needs something a layout cannot say,
add one plain field here first and use it in a second world before generalizing.
