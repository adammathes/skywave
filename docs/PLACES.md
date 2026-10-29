# Places

A **Place** is the runtime's only description of the world. The procedural generator emits one; a future offline **place compiler** emits one from real-world data. The runtime, traffic, minimap and physics consume a Place and never know its origin.

## Format principles
- Plain-old-data, chunked, versioned, validated on load, language-neutral. Canonical encoding is a little-endian binary stream with a documented layout (`place::codec`); a JSON encoding exists for tests and debugging.
- Coordinates: meters, right-handed, X east, Z south, Y up (height). Origin at the Place's southwest corner for canonical places; `PlaceHeader.chunk_origin` (chunk coordinates of the southwest chunk, `[0,0]` canonically) lets a place such as the hand-laid test town stay centered on (0,0). A Place is `size_chunks.x × size_chunks.y` chunks of `chunk_size_m` (128 m) each. v1 Places are 16×16 chunks (2 km square); the format allows up to 256×256.
- Two layers: the **skeleton** (what both producers must emit) and the **detail** (what the shared detailing pass adds). A Place file may be skeleton-only; the runtime details on load.

## Skeleton
```
PlaceHeader { magic "SKYW", version u32, seed u64, chunk_size_m f32, size_chunks [u16;2],
              region: RegionId, era: Era, origin_geo: Option<[f64;2]> (lat/lon, compiled places) }
Terrain      { res_per_chunk u16 (default 32 → 4 m), heights: Vec<f32> row-major whole-place,
               water_level f32, water: Vec<WaterBody { polygon, kind: Ocean|Lake|River|Canal }>,
               edges: [EdgeKind; 4] [north, east, south, west] = Open | Ocean | Lake | Bay | Mountains |
                      Hills | Forest | HighwayLoop | RailYards (what bounds each side; Open for the test town) }
Zones        { grid at res_per_chunk: ZoneId per cell }  ZoneId = Downtown | Strip | Residential |
               Suburb | Industrial | Commercial | Park | Beach | Farmland | Hills | Highway | Water
RoadGraph    { nodes: Vec<Node { pos [f32;2], elevation f32 }>,
               edges: Vec<Edge { a,b: u32, class: RoadClass, lanes_fwd u8, lanes_back u8,
                                 width_m f32, sidewalk: bool, speed_kph u8, oneway: bool,
                                 profile: Vec<[f32;3]> (centerline samples incl. height) }> }
             RoadClass = Highway | Arterial | Collector | Local | Alley | Beachfront | Rural
Parcels      { Vec<Parcel { polygon, zone: ZoneId, frontage_edge: u32, lot_kind: Lot|Park|Lot_Empty }> }
Buildings    { Vec<Building { footprint: Vec<[f32;2]>, height_m f32, floors u8, roof: Flat|Gable|Hip|Mansard,
                              style: StyleTag (e.g. Deco, Dingbat, StuccoBox, Craftsman, Ranch, Tract70s,
                              Glass, Warehouse, Barn, Motel, StripMall), built_year: Option<u16>,
                              use: Residential|Commercial|Office|Industrial|Civic|Hotel|Parking }> }
POIs         { Vec<Poi { pos, kind: Pier|Lighthouse|Overlook|Diner|Gas|Marina|Stadium|DragStrip|ChannelRamp|Tunnel|
                         CrewGarage|TacoStand|GradeCrossing, name_seed u64 }> }
Spawns       { Vec<Spawn { pos, heading, kind: PlayerStart|Garage }> }
```

## Detail (added by `world::detail`, deterministic from header.seed)
```
Lanes        { per edge: lane centerlines, stop lines, junction turn tables }   (traffic + AI)
Sidewalks    { polylines with crossings }                                        (pedestrians)
Facades      { per building: window grid, ground-floor type, awning, sign slots, emissive windows mask }
Props        { Vec<Prop { pos, yaw, kind: StreetLight|PalmTree|Oak|Hedge{len_dm}|Bench|Hydrant|Mailbox|Sign{text_seed}|
                         NeonSign{palette}|Billboard|PowerPole|Fence{len_dm}|ParkedCar{vehicle_class, hours}|
                         Eucalyptus|TrashCan|StopSign|Signal{arm_dm}|Pier{len_m}|Track{len_dm, bed}|Crossbuck|
                         FaceBillboard{face}|BusShelter{face}|TunnelPiece{len_dm, width_dm, portals}|StartTree|
                         TimingBoard|StripLine{len_dm}|GuardRail{len_dm}|ChainLink{len_dm}|CrewGarage{name_seed}|
                         TacoStand{name_seed}|HeroCar{slot}|PicnicTable|CrossingGate{arm_dm}|Landmark{kit, variant}|
                         LotLabel{letter} }> }
Lights       { Vec<Light { pos, color, radius, kind: Street|Neon|Window|Headlight|Flood }> }
Chunk index  { per chunk: byte ranges into every table above, plus bounds and a content hash }
```
`Track` is a length of railway centered on `pos` along its facing, on its `bed`: `TRACK_BALLAST` (on a
ballast prism over the ground), `TRACK_CROSSING` (flush panels in a road it crosses at grade, `pos` on the
road's surface), `TRACK_BRIDGE` (a girder deck, `pos` at its top) or `TRACK_END` (ballast ending at a bumper), with `TRACK_SPUR` set on a spur's pieces;
a `Crossbuck` faces the drivers it warns. A `FaceBillboard`'s lit face is blank (`face` 0) or one big pictogram
(`face` - 1, a pictogram index); a `BusShelter` opens toward its facing (the road) and carries an ad box of the
same faces. The POI kinds from `DragStrip` on are a basin's set-piece landmarks and the props from `TunnelPiece`
on are theirs (`docs/WORLD_GENERATION.md`, "Set pieces"): a `TunnelPiece` is a length of a road tunnel's box
centered on `pos` (on the road) along its facing, `width_dm` between its walls, `portals` bit 0 a portal at its far
end, bit 1 at its near end; a `StartTree` faces down its drag strip; a `TimingBoard`'s face looks along its
facing; a `StripLine` is painted across its facing; `GuardRail` and `ChainLink` runs are centered on `pos` along
it; a `CrewGarage`'s bays and a `TacoStand`'s window open along it, their signs lettered from the grammar's
lexicon by `name_seed`; a `HeroCar` is one of the world's hero cars by `slot` (the app picks it), standing as a
parked car; a `CrossingGate` faces the drivers it stops, its arm `arm_dm` long swinging down across the
approach to the facing's right. A `Landmark` is variant `variant` of landmark kit `kit` (`docs/WORLD_GENERATION.md`, "Landmarks"), `pos` at the middle of its lot's front edge, facing the road; a `LotLabel` is the landmark gallery's post lettered with `letter`, facing the road. A `Flood` light is white in its own color, on with the street lamps. A prop's `pos` is where it stands (a hedge's or fence's run is centered on it, a pier starts there) and its `yaw` gives its facing `(-sin yaw, -cos yaw)` in `(x, z)`, a spawn's heading convention. `ParkedCar::vehicle_class` packs the vehicle crate's `VehicleClass` (low 4 bits) and the paint, an index into the car shape's palette (high 4 bits). `ParkedCar::hours` has bit `h` (0..24) set when the car is there through hour `h` of the day (`ALL_HOURS`, the low 24 bits, for a car always there; a parking lot's cars also set bit 24, so they are never `ALL_HOURS`): a parking lot's cars come and go with its zone's occupancy (`docs/WORLD_GENERATION.md`, "Parking lots").

Chunking: every object table is sorted by chunk; the chunk index holds byte ranges into the 14 object tables, a per-chunk hash (which also covers the chunk's height samples and zone cells) and bounds. The terrain grid, zones and water bodies are read once for the whole place (a height grid cannot be sliced per chunk as a byte range); only object tables stream per chunk. Objects straddling chunks belong to the chunk of their centroid; roads are split at chunk borders by the producer.

## Validation (`place::validate`, run on load and in tests)
Indices in range; polygons simple and counter-clockwise; no two buildings overlap more than 5%; every parcel touches its frontage edge; road graph connected (one component ≥ 95% of edges); every edge profile within terrain ± 8 m; at least one `PlayerStart`; hash per chunk matches contents.

## Producer contract
Anything that emits a valid skeleton is playable. Producers: `world::procgen` (v1) and the place compiler (M6). The detailing pass is shared, so a compiled real place inherits the same facades, props, neon and lights as a generated one, keyed by `style`, `zone` and `era`.

## Era layer
The skeleton is era-neutral. `Era` selects the detailing palette: building `StyleTag` substitutions (e.g. a 2020s `Glass` box in an 80s place becomes `StuccoBox`), sign/neon palettes, prop sets (payphones vs. scooters), parked-car roster, radio presets. `built_year` when present (compiled places) lets the era layer drop buildings newer than the era and backfill the parcel with an era-appropriate lot.

## Place compiler (future, M6) — sketch
Offline CLI `placec --bbox <lat,lon,lat,lon> --era 1985 --out place.skw`, separate binary, may use `std`, network and large deps.
- Sources: `docs/adr/0005-reference-sources.md` (public domain only for shipped data; OpenStreetMap and other ODbL data are out). Roads: US Census TIGER/Line, filtered by the earliest vintage for old eras. Footprints and heights: FEMA USA Structures or a city layer with a compatible license, else procedural parcels on the real roads. Land use: NLCD/GIRAS. Water: NHD, NOAA shoreline.
- Elevation: USGS 3DEP and NOAA coastal DEMs (public domain). US only for now.
- Style tags: heuristics from footprint shape, use tags and era; optionally photo-derived palettes (any photo analysis happens offline, outputs only numbers; no imagery is shipped).
- Output: a skeleton Place. The runtime detailing pass does the rest.
- Licensing: a compiled Place is public domain data plus our code's output, so it may ship in the repo or the binary. The provenance header lists every source and its retrieval date.
- Era approximation is honest: street layout is the earliest TIGER vintage's (about 1990); buildings newer than the era are dropped and backfilled; signage and props come from the era layer.

## Acceptance tests
- Round-trip: encode → decode → identical bytes for procedural seeds 1..8.
- `validate` rejects each corruption class with a specific error.
- Chunk index allows loading a single chunk and reproducing that chunk's meshes bit-identically to a full load.
- Detailing is deterministic: same skeleton + seed → same detail hash.
