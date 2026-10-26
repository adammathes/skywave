//! Plain-data description of a Skywave place.
//!
//! A [`Place`] is what world generation (or the real-data compiler) produces and what the
//! world, traffic and renderer consume. Coordinates are meters in a right-handed, Y-up world:
//! 2D positions are `[x, z]` on the ground plane, X east, Z south. Names follow
//! `docs/PLACES.md`.
#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use skywave_core::{hash_bytes, Rng, Seed};

/// Format version of the [`Place`] layout and of [`Place::write_bytes`].
pub const PLACE_VERSION: u32 = 2;

/// Width of each sidewalk of an [`Edge`] with `sidewalk: true`, in meters. Sidewalks lie
/// outside `width_m`, one on each side; the outer sidewalk edge is the property line.
pub const SIDEWALK_WIDTH_M: f32 = 2.5;

#[derive(Clone, Debug, PartialEq)]
pub struct PlaceHeader {
    pub version: u32,
    /// Root seed of the place. The detailing pass derives all of its randomness from it.
    pub seed: u64,
    /// Edge length of a streaming chunk, in meters.
    pub chunk_size_m: f32,
}

/// A regular heightfield of `width * height` samples, row-major (`z` major, `x` minor).
#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    pub width: u32,
    pub height: u32,
    /// Distance between neighbouring samples, in meters.
    pub cell_m: f32,
    /// Elevations in meters; `heights.len() == width * height`.
    pub heights: Vec<f32>,
}

impl Terrain {
    pub fn flat(width: u32, height: u32, cell_m: f32, elevation: f32) -> Terrain {
        let mut heights = Vec::new();
        heights.resize(width as usize * height as usize, elevation);
        Terrain {
            width,
            height,
            cell_m,
            heights,
        }
    }

    /// Elevation at sample `(x, z)`, or `None` outside the grid.
    pub fn sample(&self, x: u32, z: u32) -> Option<f32> {
        if x < self.width && z < self.height {
            self.heights
                .get(z as usize * self.width as usize + x as usize)
                .copied()
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub pos: [f32; 2],
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadClass {
    Local = 0,
    Collector = 1,
    Arterial = 2,
    Highway = 3,
}

/// An undirected road segment between nodes `a` and `b` (indices into `RoadGraph::nodes`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub class: RoadClass,
    /// Total lanes, both directions.
    pub lanes: u8,
    /// Curb-to-curb width (asphalt including gutters), meters.
    pub width_m: f32,
    /// Whether the road has a [`SIDEWALK_WIDTH_M`] sidewalk on both sides.
    pub sidewalk: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoadGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneId {
    Downtown = 0,
    Strip = 1,
    Residential = 2,
    Suburb = 3,
    Industrial = 4,
    Commercial = 5,
    Park = 6,
    Beach = 7,
    Farmland = 8,
    Hills = 9,
    Highway = 10,
    Water = 11,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LotKind {
    Lot = 0,
    Park = 1,
    LotEmpty = 2,
}

/// A piece of land fronting a road edge. Polygons are counter-clockwise seen from above.
#[derive(Clone, Debug, PartialEq)]
pub struct Parcel {
    pub polygon: Vec<[f32; 2]>,
    pub zone: ZoneId,
    /// Index into `RoadGraph::edges` of the road the parcel faces.
    pub frontage_edge: u32,
    pub lot_kind: LotKind,
}

/// Architectural style of a building; the detailing pass keys facades and palettes on it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleTag {
    /// Smooth stucco walls, simple parapet.
    StuccoBox = 0,
    /// Brick courses, stone trim, drainpipes.
    Brick = 1,
    /// Streamline/Art Deco: stepped parapet, speed-line bands.
    Deco = 2,
    /// Long, low row of storefronts under one deep canopy.
    StripMall = 3,
}

/// Roof shape. Pitched roofs rise above `Building::height` (the wall top).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Roof {
    /// Flat roof behind a parapet.
    Flat = 0,
    /// Two slopes meeting at a ridge along the long axis; gable walls at the ends.
    Gable = 1,
    /// Four slopes: a ridge along the long axis, hips at the ends.
    Hip = 2,
}

/// What a building is used for (`use` in `docs/PLACES.md`; `use` is a Rust keyword).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingUse {
    Residential = 0,
    Commercial = 1,
    Office = 2,
    Industrial = 3,
    Civic = 4,
    Hotel = 5,
    Parking = 6,
}

/// An extruded building: a closed, counter-clockwise (seen from above) footprint polygon.
#[derive(Clone, Debug, PartialEq)]
pub struct Building {
    pub footprint: Vec<[f32; 2]>,
    /// Height of the top of the walls (parapet included) above the ground, meters.
    pub height: f32,
    /// Number of storeys, at least 1.
    pub floors: u8,
    pub roof: Roof,
    pub style: StyleTag,
    pub usage: BuildingUse,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub header: PlaceHeader,
    pub terrain: Terrain,
    pub roads: RoadGraph,
    pub parcels: Vec<Parcel>,
    pub buildings: Vec<Building>,
}

/// Why [`Place::validate`] rejected a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceError {
    Version,
    TerrainSize,
    EdgeIndex { edge: u32 },
    DegenerateEdge { edge: u32 },
    NoLanes { edge: u32 },
    RoadWidth { edge: u32 },
    Parcel { parcel: u32 },
    FrontageEdge { parcel: u32 },
    Footprint { building: u32 },
    BuildingHeight { building: u32 },
    Floors { building: u32 },
    NonFinite,
}

/// Block pitch of the test town (road centerline to centerline), meters.
const GRID_PITCH: f32 = 100.0;
/// Road nodes per side of the test town: 8 x 8 blocks.
const GRID_NODES: u32 = 9;
/// Row and column index of the two arterials.
const ARTERIAL_LINE: u32 = 4;
/// The north-south segment at column 7 between rows 3 and 4 is left out, which merges two
/// blocks into the strip-mall superblock and makes two T junctions.
const MISSING_SEGMENT: (u32, u32) = (7, 3);
/// Rng key for the test town's hand-laid jitter.
const KEY_TEST_GRID: u64 = 0x5445_5354_4752_4944;

impl Place {
    /// A hand-laid flat town of 8 x 8 blocks on a 100 m grid (~0.8 km across) with an
    /// east-west and a north-south arterial (14 m) crossing at the center and 10 m local
    /// streets elsewhere. Around the crossing: a two-block commercial strip on the
    /// east-west arterial, a small downtown on the north-south one, a strip-mall superblock,
    /// a park, shops along the arterials and stucco houses everywhere else.
    pub fn test_grid() -> Place {
        let seed = 1;
        let roads = test_grid_roads();
        let mut town = TownBuilder {
            roads: &roads,
            parcels: Vec::new(),
            buildings: Vec::new(),
        };
        for bz in 0..GRID_NODES - 1 {
            for bx in 0..GRID_NODES - 1 {
                if (bx, bz) == (MISSING_SEGMENT.0, MISSING_SEGMENT.1) {
                    continue; // Part of the superblock starting at bx - 1.
                }
                let mut rng = Rng::new(Seed(seed), &[KEY_TEST_GRID, bx.into(), bz.into()]);
                town.block(bx, bz, &mut rng);
            }
        }
        let parcels = town.parcels;
        let buildings = town.buildings;

        let extent = (GRID_NODES - 1) as f32 * GRID_PITCH + 2.0 * GRID_PITCH;
        Place {
            header: PlaceHeader {
                version: PLACE_VERSION,
                seed,
                chunk_size_m: 128.0,
            },
            terrain: Terrain::flat(11, 11, extent / 10.0, 0.0),
            roads,
            parcels,
            buildings,
        }
    }

    /// Checks the structural invariants every consumer relies on.
    pub fn validate(&self) -> Result<(), PlaceError> {
        if self.header.version != PLACE_VERSION {
            return Err(PlaceError::Version);
        }
        let t = &self.terrain;
        if t.heights.len() != t.width as usize * t.height as usize {
            return Err(PlaceError::TerrainSize);
        }
        let finite = self.header.chunk_size_m.is_finite()
            && t.cell_m.is_finite()
            && t.heights.iter().all(|h| h.is_finite())
            && self
                .roads
                .nodes
                .iter()
                .all(|n| n.pos.iter().all(|c| c.is_finite()));
        if !finite {
            return Err(PlaceError::NonFinite);
        }
        let n = self.roads.nodes.len();
        for (i, e) in self.roads.edges.iter().enumerate() {
            let edge = i as u32;
            if e.a as usize >= n || e.b as usize >= n {
                return Err(PlaceError::EdgeIndex { edge });
            }
            if e.a == e.b {
                return Err(PlaceError::DegenerateEdge { edge });
            }
            if e.lanes == 0 {
                return Err(PlaceError::NoLanes { edge });
            }
            if !(e.width_m.is_finite() && e.width_m > 0.0) {
                return Err(PlaceError::RoadWidth { edge });
            }
        }
        for (i, p) in self.parcels.iter().enumerate() {
            let parcel = i as u32;
            if !is_ccw_polygon(&p.polygon) {
                return Err(PlaceError::Parcel { parcel });
            }
            if p.frontage_edge as usize >= self.roads.edges.len() {
                return Err(PlaceError::FrontageEdge { parcel });
            }
        }
        for (i, b) in self.buildings.iter().enumerate() {
            let building = i as u32;
            if !is_ccw_polygon(&b.footprint) {
                return Err(PlaceError::Footprint { building });
            }
            if !(b.height.is_finite() && b.height > 0.0) {
                return Err(PlaceError::BuildingHeight { building });
            }
            if b.floors == 0 {
                return Err(PlaceError::Floors { building });
            }
        }
        Ok(())
    }

    /// Canonical little-endian serialization, in field declaration order. Vectors are
    /// prefixed by their `u32` length, enums are one byte, bools one byte (0 or 1). Used for
    /// hashing and, later, the on-disk format.
    pub fn write_bytes(&self, out: &mut Vec<u8>) {
        let h = &self.header;
        put_u32(out, h.version);
        out.extend_from_slice(&h.seed.to_le_bytes());
        put_f32(out, h.chunk_size_m);

        let t = &self.terrain;
        put_u32(out, t.width);
        put_u32(out, t.height);
        put_f32(out, t.cell_m);
        put_u32(out, t.heights.len() as u32);
        for &v in &t.heights {
            put_f32(out, v);
        }

        put_u32(out, self.roads.nodes.len() as u32);
        for node in &self.roads.nodes {
            put_f32(out, node.pos[0]);
            put_f32(out, node.pos[1]);
        }
        put_u32(out, self.roads.edges.len() as u32);
        for e in &self.roads.edges {
            put_u32(out, e.a);
            put_u32(out, e.b);
            out.push(e.class as u8);
            out.push(e.lanes);
            put_f32(out, e.width_m);
            out.push(u8::from(e.sidewalk));
        }

        put_u32(out, self.parcels.len() as u32);
        for p in &self.parcels {
            put_polygon(out, &p.polygon);
            out.push(p.zone as u8);
            put_u32(out, p.frontage_edge);
            out.push(p.lot_kind as u8);
        }

        put_u32(out, self.buildings.len() as u32);
        for b in &self.buildings {
            put_polygon(out, &b.footprint);
            put_f32(out, b.height);
            out.push(b.floors);
            out.push(b.roof as u8);
            out.push(b.style as u8);
            out.push(b.usage as u8);
        }
    }

    /// Hash of [`Place::write_bytes`]: the determinism fingerprint of a place.
    pub fn content_hash(&self) -> u64 {
        let mut bytes = Vec::new();
        self.write_bytes(&mut bytes);
        hash_bytes(0, &bytes)
    }
}

/// Class, lane count and curb-to-curb width of grid line `line` of the test town.
fn test_grid_road(line: u32) -> (RoadClass, u8, f32) {
    if line == ARTERIAL_LINE {
        (RoadClass::Arterial, 4, 14.0)
    } else {
        (RoadClass::Local, 2, 10.0)
    }
}

/// Coordinate of grid line `i`; the town is centered on the origin.
fn grid_coord(i: u32) -> f32 {
    (i as f32 - ((GRID_NODES - 1) / 2) as f32) * GRID_PITCH
}

fn test_grid_roads() -> RoadGraph {
    const N: u32 = GRID_NODES;
    let mut nodes = Vec::new();
    for z in 0..N {
        for x in 0..N {
            nodes.push(Node {
                pos: [grid_coord(x), grid_coord(z)],
            });
        }
    }
    let mut edges = Vec::new();
    let mut push = |a: u32, b: u32, line: u32| {
        let (class, lanes, width_m) = test_grid_road(line);
        edges.push(Edge {
            a,
            b,
            class,
            lanes,
            width_m,
            sidewalk: true,
        });
    };
    for z in 0..N {
        for x in 0..N {
            let i = z * N + x;
            if x + 1 < N {
                push(i, i + 1, z);
            }
            if z + 1 < N && (x, z) != MISSING_SEGMENT {
                push(i, i + N, x);
            }
        }
    }
    RoadGraph { nodes, edges }
}

/// A side of a rectangular block. North is -Z.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    North,
    South,
    West,
    East,
}

impl Side {
    fn opposite(self) -> Side {
        match self {
            Side::North => Side::South,
            Side::South => Side::North,
            Side::West => Side::East,
            Side::East => Side::West,
        }
    }
}

/// A block's buildable area between the property lines, plus the grid lines around it.
#[derive(Clone, Copy)]
struct Block {
    x0: f32,
    z0: f32,
    x1: f32,
    z1: f32,
    /// Grid column of the west road and row of the north road; east/south are `+span`.
    col: u32,
    row: u32,
    span_x: u32,
}

impl Block {
    fn new(bx: u32, bz: u32, span_x: u32) -> Block {
        let half = |line: u32| test_grid_road(line).2 * 0.5 + SIDEWALK_WIDTH_M;
        Block {
            x0: grid_coord(bx) + half(bx),
            x1: grid_coord(bx + span_x) - half(bx + span_x),
            z0: grid_coord(bz) + half(bz),
            z1: grid_coord(bz + 1) - half(bz + 1),
            col: bx,
            row: bz,
            span_x,
        }
    }

    /// Range of the along-frontage coordinate (x for north/south, z for east/west).
    fn span(&self, side: Side) -> (f32, f32) {
        match side {
            Side::North | Side::South => (self.x0, self.x1),
            Side::West | Side::East => (self.z0, self.z1),
        }
    }

    /// Depth from the property line on `side` to the opposite one.
    fn depth(&self, side: Side) -> f32 {
        match side {
            Side::North | Side::South => self.z1 - self.z0,
            Side::West | Side::East => self.x1 - self.x0,
        }
    }

    /// Rectangle `s0..s1` along `side` and `d0..d1` inward from its property line.
    fn rect(&self, side: Side, s0: f32, s1: f32, d0: f32, d1: f32) -> Vec<[f32; 2]> {
        match side {
            Side::North => rect(s0, self.z0 + d0, s1 - s0, d1 - d0),
            Side::South => rect(s0, self.z1 - d1, s1 - s0, d1 - d0),
            Side::West => rect(self.x0 + d0, s0, d1 - d0, s1 - s0),
            Side::East => rect(self.x1 - d1, s0, d1 - d0, s1 - s0),
        }
    }
}

struct TownBuilder<'a> {
    roads: &'a RoadGraph,
    parcels: Vec<Parcel>,
    buildings: Vec<Building>,
}

/// One lot's building, described relative to the lot.
struct Spec {
    zone: ZoneId,
    usage: BuildingUse,
    style: StyleTag,
    floors: u8,
    /// Distance from the property line to the front wall.
    setback: f32,
    /// Footprint depth.
    depth: f32,
    /// Footprint width inset on each side of the lot.
    side_gap: f32,
    roof: Roof,
}

impl TownBuilder<'_> {
    fn block(&mut self, bx: u32, bz: u32, rng: &mut Rng) {
        let a = ARTERIAL_LINE;
        let strip_mall = (bx, bz) == (MISSING_SEGMENT.0 - 1, MISSING_SEGMENT.1);
        let b = Block::new(bx, bz, if strip_mall { 2 } else { 1 });
        // The arterial frontage, if any: the east-west arterial wins over the north-south.
        let row_front = if bz + 1 == a {
            Some(Side::South)
        } else if bz == a {
            Some(Side::North)
        } else {
            None
        };
        let col_front = if bx + 1 == a {
            Some(Side::East)
        } else if bx == a {
            Some(Side::West)
        } else {
            None
        };
        let central = bx + 1 == a || bx == a;
        let near_center = |d: u32| bz + d == a || bz == a + d - 1;

        if strip_mall {
            self.strip_mall(&b);
        } else if (bx, bz) == (a - 2, a) {
            let (front, poly) = (
                Side::North,
                b.rect(Side::North, b.x0, b.x1, 0.0, b.depth(Side::North)),
            );
            let edge = self.frontage(&b, front, (b.x0 + b.x1) * 0.5);
            self.parcels.push(Parcel {
                polygon: poly,
                zone: ZoneId::Park,
                frontage_edge: edge,
                lot_kind: LotKind::Park,
            });
        } else if let (true, Some(front), Some(side)) = (central, row_front, col_front) {
            // The strip: storefronts along the east-west arterial, a few more around the
            // corner on the north-south one, houses behind.
            let depth = 25.0;
            self.shops(&b, front, b.span(front), (12.0, 18.0), rng, |rng| Spec {
                zone: ZoneId::Strip,
                usage: BuildingUse::Commercial,
                style: pick(
                    rng,
                    &[
                        (4, StyleTag::StuccoBox),
                        (4, StyleTag::Deco),
                        (2, StyleTag::Brick),
                    ],
                ),
                floors: pick(rng, &[(4, 1), (4, 2), (2, 3)]),
                setback: 0.0,
                depth: rng.range_f32(depth - 3.0, depth),
                side_gap: 0.3,
                roof: Roof::Flat,
            });
            let (s0, s1) = b.span(side);
            let (s0, s1) = if front == Side::South {
                (s0, s1 - depth - 1.0)
            } else {
                (s0 + depth + 1.0, s1)
            };
            self.shops(&b, side, (s0, s1), (14.0, 18.0), rng, |rng| Spec {
                zone: ZoneId::Strip,
                usage: BuildingUse::Commercial,
                style: pick(rng, &[(1, StyleTag::StuccoBox), (1, StyleTag::Brick)]),
                floors: pick(rng, &[(1, 1), (1, 2)]),
                setback: 0.0,
                depth: rng.range_f32(16.0, 20.0),
                side_gap: 0.3,
                roof: Roof::Flat,
            });
            let back = front.opposite();
            let (h0, h1) = b.span(back);
            let span = if side == Side::East {
                (h0, h1 - 21.0)
            } else {
                (h0 + 21.0, h1)
            };
            self.houses(&b, back, span, rng);
        } else if let (true, Some(front), true) = (central, col_front, near_center(2)) {
            // Downtown (north of the strip) and its lower brick fringe (south of it).
            let north = bz < a;
            let depth = 30.0;
            self.shops(&b, front, b.span(front), (22.0, 28.0), rng, |rng| Spec {
                zone: ZoneId::Downtown,
                usage: BuildingUse::Office,
                style: if north {
                    pick(rng, &[(1, StyleTag::Deco), (1, StyleTag::Brick)])
                } else {
                    pick(rng, &[(1, StyleTag::Deco), (3, StyleTag::Brick)])
                },
                floors: if north {
                    rng.range(4, 8) as u8
                } else {
                    rng.range(2, 5) as u8
                },
                setback: 0.0,
                depth: rng.range_f32(depth - 4.0, depth),
                side_gap: 0.3,
                roof: Roof::Flat,
            });
            let (s0, s1) = b.span(Side::North);
            let (s0, s1) = if front == Side::East {
                (s0, s1 - depth - 1.0)
            } else {
                (s0 + depth + 1.0, s1)
            };
            for side in [Side::North, Side::South] {
                self.shops(&b, side, (s0, s1), (20.0, 26.0), rng, |rng| Spec {
                    zone: ZoneId::Downtown,
                    usage: BuildingUse::Commercial,
                    style: pick(rng, &[(1, StyleTag::StuccoBox), (2, StyleTag::Brick)]),
                    floors: rng.range(2, 4) as u8,
                    setback: 0.0,
                    depth: rng.range_f32(18.0, 22.0),
                    side_gap: 0.3,
                    roof: Roof::Flat,
                });
            }
            let back = front.opposite();
            let (z0, z1) = b.span(back);
            self.shops(&b, back, (z0 + 23.0, z1 - 23.0), (16.0, 20.0), rng, |rng| {
                Spec {
                    zone: ZoneId::Downtown,
                    usage: BuildingUse::Commercial,
                    style: pick(
                        rng,
                        &[
                            (1, StyleTag::StuccoBox),
                            (1, StyleTag::Brick),
                            (1, StyleTag::Deco),
                        ],
                    ),
                    floors: rng.range(1, 3) as u8,
                    setback: 0.0,
                    depth: rng.range_f32(16.0, 20.0),
                    side_gap: 0.3,
                    roof: Roof::Flat,
                }
            });
        } else if let Some(front) = row_front.or(col_front) {
            // Shops along the arterial, houses on the street behind.
            self.shops(&b, front, b.span(front), (14.0, 20.0), rng, |rng| Spec {
                zone: ZoneId::Commercial,
                usage: BuildingUse::Commercial,
                style: pick(
                    rng,
                    &[
                        (5, StyleTag::StuccoBox),
                        (3, StyleTag::Brick),
                        (2, StyleTag::Deco),
                    ],
                ),
                floors: pick(rng, &[(3, 1), (2, 2)]),
                setback: pick(rng, &[(2, 0.0), (1, 2.0)]),
                depth: rng.range_f32(16.0, 22.0),
                side_gap: 0.3,
                roof: Roof::Flat,
            });
            let back = front.opposite();
            self.houses(&b, back, b.span(back), rng);
        } else {
            self.houses(&b, Side::North, b.span(Side::North), rng);
            self.houses(&b, Side::South, b.span(Side::South), rng);
        }
    }

    /// Lots of `widths` (min, max) along `side` over `span`, each with one building.
    fn shops(
        &mut self,
        b: &Block,
        side: Side,
        span: (f32, f32),
        widths: (f32, f32),
        rng: &mut Rng,
        spec: impl Fn(&mut Rng) -> Spec,
    ) {
        let (mut s, end) = span;
        while end - s >= widths.0 {
            let mut w = rng.range_f32(widths.0, widths.1);
            if end - s - w < widths.0 {
                w = end - s;
            }
            let sp = spec(rng);
            let lot_depth = sp.setback + sp.depth;
            self.lot(b, side, s, s + w, lot_depth, &sp);
            s += w;
        }
    }

    /// Stucco houses along `side` over `span`, set back behind front lawns.
    fn houses(&mut self, b: &Block, side: Side, span: (f32, f32), rng: &mut Rng) {
        let (s0, s1) = span;
        let count = ((s1 - s0) / 20.0) as u32;
        if count == 0 {
            return;
        }
        let lot_w = (s1 - s0) / count as f32;
        let lot_depth = b.depth(side) * 0.5;
        for i in 0..count {
            let s = s0 + i as f32 * lot_w;
            let width = rng.range_f32(11.0, 15.0);
            let style = pick(
                rng,
                &[
                    (16, StyleTag::StuccoBox),
                    (3, StyleTag::Deco),
                    (1, StyleTag::Brick),
                ],
            );
            let roof = if style == StyleTag::StuccoBox {
                pick(rng, &[(11, Roof::Hip), (4, Roof::Gable), (5, Roof::Flat)])
            } else {
                Roof::Flat
            };
            let sp = Spec {
                zone: ZoneId::Residential,
                usage: BuildingUse::Residential,
                style,
                floors: pick(rng, &[(13, 1), (7, 2)]),
                setback: rng.range_f32(6.0, 8.5),
                depth: rng.range_f32(9.0, 12.5),
                side_gap: (lot_w - width) * 0.5,
                roof,
            };
            self.lot(b, side, s, s + lot_w, lot_depth, &sp);
        }
    }

    fn lot(&mut self, b: &Block, side: Side, s0: f32, s1: f32, depth: f32, sp: &Spec) {
        let gap = sp.side_gap;
        let edge = self.frontage(b, side, (s0 + s1) * 0.5);
        self.parcels.push(Parcel {
            polygon: b.rect(side, s0, s1, 0.0, depth),
            zone: sp.zone,
            frontage_edge: edge,
            lot_kind: LotKind::Lot,
        });
        let floors = sp.floors;
        let height = match sp.usage {
            BuildingUse::Residential => f32::from(floors) * 3.1 + 0.8,
            _ => f32::from(floors) * 3.7 + 1.4,
        };
        self.buildings.push(Building {
            footprint: b.rect(side, s0 + gap, s1 - gap, sp.setback, sp.setback + sp.depth),
            height,
            floors,
            roof: sp.roof,
            style: sp.style,
            usage: sp.usage,
        });
    }

    /// The strip-mall superblock: one long row of shops at the back of a parking lot that
    /// faces the arterial, and a small pad building on the corner.
    fn strip_mall(&mut self, b: &Block) {
        let front = Side::South;
        let edge = self.frontage(b, front, (b.x0 + b.x1) * 0.5);
        self.parcels.push(Parcel {
            polygon: b.rect(front, b.x0, b.x1, 0.0, b.depth(front)),
            zone: ZoneId::Strip,
            frontage_edge: edge,
            lot_kind: LotKind::Lot,
        });
        self.buildings.push(Building {
            footprint: b.rect(front, b.x0 + 12.0, b.x0 + 120.0, 36.0, 56.0),
            height: 6.5,
            floors: 1,
            roof: Roof::Flat,
            style: StyleTag::StripMall,
            usage: BuildingUse::Commercial,
        });
        self.buildings.push(Building {
            footprint: b.rect(front, b.x1 - 34.0, b.x1 - 10.0, 4.0, 20.0),
            height: 5.6,
            floors: 1,
            roof: Roof::Flat,
            style: StyleTag::StuccoBox,
            usage: BuildingUse::Commercial,
        });
    }

    /// Index of the road edge on `side` of `b` that the point `s` along the side faces.
    fn frontage(&self, b: &Block, side: Side, s: f32) -> u32 {
        let (line, fixed_is_row) = match side {
            Side::North => (b.row, true),
            Side::South => (b.row + 1, true),
            Side::West => (b.col, false),
            Side::East => (b.col + b.span_x, false),
        };
        let first = grid_coord(0);
        let cell = (((s - first) / GRID_PITCH) as u32).min(GRID_NODES - 2);
        let n = GRID_NODES;
        let (a, c) = if fixed_is_row {
            (line * n + cell, line * n + cell + 1)
        } else {
            (cell * n + line, (cell + 1) * n + line)
        };
        let found = self
            .roads
            .edges
            .iter()
            .position(|e| (e.a, e.b) == (a, c) || (e.a, e.b) == (c, a));
        found.unwrap_or(0) as u32
    }
}

/// Weighted choice; weights are small integers.
fn pick<T: Copy>(rng: &mut Rng, options: &[(u32, T)]) -> T {
    let total: u32 = options.iter().map(|o| o.0).sum();
    let mut r = rng.range(0, total);
    for &(w, v) in options {
        if r < w {
            return v;
        }
        r -= w;
    }
    options[options.len() - 1].1
}

fn rect(x0: f32, z0: f32, w: f32, d: f32) -> Vec<[f32; 2]> {
    // Counter-clockwise seen from above (+Y), i.e. positive area in the (x, z) plane
    // with the shoelace formula below.
    vec![[x0, z0], [x0, z0 + d], [x0 + w, z0 + d], [x0 + w, z0]]
}

fn is_ccw_polygon(poly: &[[f32; 2]]) -> bool {
    let finite = poly.iter().all(|p| p.iter().all(|c| c.is_finite()));
    poly.len() >= 3 && finite && signed_area(poly) > 0.0
}

/// Shoelace area, positive for counter-clockwise footprints viewed from +Y.
pub fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let mut acc = 0.0;
    for (i, p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        acc += p[1] * q[0] - p[0] * q[1];
    }
    acc * 0.5
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

fn put_polygon(out: &mut Vec<u8>, poly: &[[f32; 2]]) {
    put_u32(out, poly.len() as u32);
    for p in poly {
        put_f32(out, p[0]);
        put_f32(out, p[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(poly: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in poly {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn test_grid_is_valid() {
        let p = Place::test_grid();
        assert_eq!(p.validate(), Ok(()));
        assert_eq!(p.roads.nodes.len(), 81);
        // 2 * 9 * 8 grid segments minus the one merged into the strip-mall superblock.
        assert_eq!(p.roads.edges.len(), 143);
        assert!(p.buildings.len() > 300, "{}", p.buildings.len());
        assert!(p.parcels.iter().any(|q| q.lot_kind == LotKind::Park));
        assert!(p.buildings.iter().any(|b| b.style == StyleTag::StripMall));
        for roof in [Roof::Flat, Roof::Gable, Roof::Hip] {
            assert!(p.buildings.iter().any(|b| b.roof == roof), "{roof:?}");
        }
        for style in [StyleTag::StuccoBox, StyleTag::Brick, StyleTag::Deco] {
            assert!(p.buildings.iter().any(|b| b.style == style), "{style:?}");
        }
        let arterial = p
            .roads
            .edges
            .iter()
            .filter(|e| e.class == RoadClass::Arterial);
        assert!(arterial.clone().all(|e| e.width_m == 14.0));
        assert_eq!(arterial.count(), 16);
        let strip = p.parcels.iter().filter(|q| q.zone == ZoneId::Strip).count();
        assert!(strip >= 10, "{strip}");
    }

    #[test]
    fn test_grid_buildings_fit_their_blocks_and_do_not_overlap() {
        let p = Place::test_grid();
        let boxes: Vec<_> = p.buildings.iter().map(|b| bounds(&b.footprint)).collect();
        let overlap = |a: &([f32; 2], [f32; 2]), b: &([f32; 2], [f32; 2])| {
            (0..2).all(|k| a.1[k] > b.0[k] + 0.01 && b.1[k] > a.0[k] + 0.01)
        };
        // Road corridors: asphalt plus both sidewalks.
        let corridors: Vec<_> = p
            .roads
            .edges
            .iter()
            .map(|e| {
                let half = e.width_m * 0.5 + SIDEWALK_WIDTH_M;
                let (a, b) = (
                    p.roads.nodes[e.a as usize].pos,
                    p.roads.nodes[e.b as usize].pos,
                );
                (
                    [a[0].min(b[0]) - half, a[1].min(b[1]) - half],
                    [a[0].max(b[0]) + half, a[1].max(b[1]) + half],
                )
            })
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for (k, c) in corridors.iter().enumerate() {
                assert!(
                    !overlap(a, c),
                    "building {i} {a:?} {:?} intrudes on road {k} {c:?}",
                    p.buildings[i].style
                );
            }
            for (j, b) in boxes.iter().enumerate().skip(i + 1) {
                assert!(!overlap(a, b), "buildings {i} and {j} overlap");
            }
        }
        assert!(p.buildings.iter().all(|b| (1..=8).contains(&b.floors)));
    }

    #[test]
    fn test_grid_is_deterministic() {
        let a = Place::test_grid();
        let b = Place::test_grid();
        assert_eq!(a, b);
        assert_eq!(a.content_hash(), b.content_hash());
        // Pinned fingerprint: change deliberately, in its own commit, with the reason.
        assert_eq!(a.content_hash(), 0x6D8C_1D3E_DDFB_D18A);
    }

    #[test]
    fn validate_catches_broken_places() {
        let mut p = Place::test_grid();
        p.roads.edges[3].b = 999;
        assert_eq!(p.validate(), Err(PlaceError::EdgeIndex { edge: 3 }));

        let mut p = Place::test_grid();
        p.roads.edges[0].b = p.roads.edges[0].a;
        assert_eq!(p.validate(), Err(PlaceError::DegenerateEdge { edge: 0 }));

        let mut p = Place::test_grid();
        p.roads.edges[1].lanes = 0;
        assert_eq!(p.validate(), Err(PlaceError::NoLanes { edge: 1 }));

        let mut p = Place::test_grid();
        p.roads.edges[2].width_m = -1.0;
        assert_eq!(p.validate(), Err(PlaceError::RoadWidth { edge: 2 }));

        let mut p = Place::test_grid();
        p.parcels[4].polygon.reverse();
        assert_eq!(p.validate(), Err(PlaceError::Parcel { parcel: 4 }));

        let mut p = Place::test_grid();
        p.parcels[5].frontage_edge = 10_000;
        assert_eq!(p.validate(), Err(PlaceError::FrontageEdge { parcel: 5 }));

        let mut p = Place::test_grid();
        p.buildings[2].footprint.reverse();
        assert_eq!(p.validate(), Err(PlaceError::Footprint { building: 2 }));

        let mut p = Place::test_grid();
        p.buildings[0].height = 0.0;
        assert_eq!(
            p.validate(),
            Err(PlaceError::BuildingHeight { building: 0 })
        );

        let mut p = Place::test_grid();
        p.buildings[7].floors = 0;
        assert_eq!(p.validate(), Err(PlaceError::Floors { building: 7 }));

        let mut p = Place::test_grid();
        p.terrain.heights.pop();
        assert_eq!(p.validate(), Err(PlaceError::TerrainSize));

        let mut p = Place::test_grid();
        p.roads.nodes[0].pos[0] = f32::NAN;
        assert_eq!(p.validate(), Err(PlaceError::NonFinite));
    }

    #[test]
    fn terrain_sampling() {
        let t = Terrain::flat(3, 2, 10.0, 1.5);
        assert_eq!(t.sample(2, 1), Some(1.5));
        assert_eq!(t.sample(3, 0), None);
        assert_eq!(t.sample(0, 2), None);
    }

    #[test]
    fn serialization_changes_with_content() {
        let a = Place::test_grid();
        let mut b = a.clone();
        b.buildings[0].height += 1.0;
        assert_ne!(a.content_hash(), b.content_hash());
        let mut c = a.clone();
        c.buildings[0].style = StyleTag::Deco;
        assert_ne!(a.content_hash(), c.content_hash());
        let mut r = a.clone();
        r.buildings[0].roof = if r.buildings[0].roof == Roof::Hip {
            Roof::Flat
        } else {
            Roof::Hip
        };
        assert_ne!(a.content_hash(), r.content_hash());
        let mut d = a.clone();
        d.roads.edges[0].sidewalk = false;
        assert_ne!(a.content_hash(), d.content_hash());
    }
}
