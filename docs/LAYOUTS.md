# Authored layouts

A procedural region template invents a place's structure from scored noise: where the water is, where
the hills rise, where downtown sits, which way the arterials run. For a place someone knows, that is
the wrong way round: the structure should be **authored**, and only the fine grain generated.

An **authored layout** (`assets/layouts/<id>.ron`, `world::layout`) is a small hand-written map:
terrain shapes, water, districts and the main roads. `world::layout::build_place` turns it into the
same macro map and arterial road graph the procedural stages 1–3 produce, then runs the shared
stages 4–7 (local streets, parcels, buildings, detailing) over it, dressed by the scenario's region
template's zone rules (block sizes, street patterns, styles, trees, props) and grammar. The result
is an ordinary `place::Place`; nothing downstream knows how it was made. A future real-map import
emits the same thing.

Everything is still deterministic from the seed: the seed varies the fine grain (which lot gets
which building), never the structure.

## Format
Coordinates are meters in the place's own frame: `(x, z)`, x east, z south, `(0, 0)` the
north-west corner of chunk `(0, 0)`. Angles are degrees clockwise from north.

```
Layout(
  id: "bay_loop",
  size_chunks: (32, 32),            // 128 m chunks: 4.1 km square
  ground_m: 6,                      // land height where no shape says otherwise
  water_level_m: 0,
  edges: (north: Ocean, east: Hills, south: Hills, west: Ocean),   // place::EdgeKind
  // Terrain: added onto `ground_m`, each fading to nothing at `falloff_m` from its line or point.
  ridges: [ Ridge(name: "west", line: [(500, 900), (420, 3600)], height_m: 160,
                  falloff_m: 380, relief_m: 14) ],
  peaks:  [ Peak(name: "north", at: (2050, 300), height_m: 60, falloff_m: 260) ],
  // Water: polygons, painted over the terrain to `depth_m` under the water level.
  water: [ Water(kind: Bay, depth_m: 5, polygon: [(1500, 700), ...]) ],
  // Districts: polygons painted in order (a later one wins), each a zone::ZoneId. Land no
  // district covers is `default_zone`. A district may fix its street grid's bearing.
  default_zone: Hills,
  districts: [ District(name: "city", zone: Residential, grid_deg: Some(0), polygon: [...]) ],
  // Main roads: polylines, or a start, a bearing (or a peak to aim at) and a length.
  // Crossings become junctions; a road over water is a bridge (its deck `deck_m` over the water).
  roads: [
    Road(name: "loop", class: Arterial, lanes: 2, points: [(600, 400), (600, 1800), ...]),
    Road(name: "boulevard", class: Arterial, lanes: 3, from: (900, 3000), toward: "north", length_m: 2000),
  ],
  start: Start(road: "loop", along_m: 300),   // the player starts on this road, this far along it
)
```

Rules the loader checks (`Layout::validate`): every point inside the place; polygons simple; road
ends either on another road, at a place edge or at a named dead end (`ends: (Join, Join)` is the
default, so a road to nowhere is an error, not a default); names unique; `toward` names a peak.
`Highway` roads are grade-separated from the roads they cross (overpasses), the others meet at grade.

## Kept small on purpose
No scoring, no noise fields, no tuning curves. If a world needs something a layout cannot say,
add one plain field here first and use it in a second world before generalizing.
