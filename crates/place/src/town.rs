//! The hand-laid test town, [`Place::test_grid`].

use alloc::vec;
use alloc::vec::Vec;
use core::f32::consts::FRAC_PI_2;
use skywave_core::{Rng, Seed};

use crate::{
    Building, BuildingUse, Edge, Era, Facades, Lanes, Lights, LotKind, Node, Parcel, Place,
    PlaceHeader, Props, RegionId, RoadClass, RoadGraph, Roof, Sidewalks, Spawn, SpawnKind,
    StyleTag, Terrain, ZoneId, Zones, CHUNK_SIZE_M, DEFAULT_RES_PER_CHUNK, PLACE_VERSION,
    SIDEWALK_WIDTH_M,
};

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

/// Where the car starts: on the ground in the eastbound outer lane at the west end of the
/// strip, facing east down it (so the late-afternoon sun is behind the car).
const TEST_GRID_START: [f32; 3] = [-92.0, 0.0, 4.9];
const TEST_GRID_START_HEADING: f32 = -FRAC_PI_2;

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

    // 8 x 8 chunks of 128 m centered on the origin hold the 800 m town and a margin.
    let header = PlaceHeader {
        version: PLACE_VERSION,
        seed,
        chunk_size_m: CHUNK_SIZE_M,
        size_chunks: [8, 8],
        chunk_origin: [-4, -4],
        region: RegionId::TestTown,
        era: Era::Eighties,
        origin_geo: None,
    };
    let terrain = Terrain::flat(&header, DEFAULT_RES_PER_CHUNK, 0.0);
    let zones = zone_map(&header, &parcels);
    let mut place = Place {
        header,
        terrain,
        zones,
        roads,
        parcels,
        buildings,
        pois: Vec::new(),
        spawns: vec![Spawn {
            pos: TEST_GRID_START,
            heading: TEST_GRID_START_HEADING,
            kind: SpawnKind::PlayerStart,
        }],
        lanes: Lanes::default(),
        sidewalks: Sidewalks::default(),
        facades: Facades::default(),
        props: Props::default(),
        lights: Lights::default(),
    };
    place.sort_by_chunk();
    place
}

/// Residential everywhere, each parcel's zone over the cells whose centers it covers.
fn zone_map(header: &PlaceHeader, parcels: &[Parcel]) -> Zones {
    let res = DEFAULT_RES_PER_CHUNK;
    let [w, h] = Zones::cells_per_side(header, res);
    let mut cells = Vec::new();
    cells.resize(w * h, ZoneId::Residential);
    let o = header.origin_m();
    let cell = header.chunk_size_m / f32::from(res);
    for p in parcels {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for v in &p.polygon {
            for k in 0..2 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        // Cell range whose centers may lie inside; the town is inside the place.
        let first = |k: usize| ((lo[k] - o[k]) / cell) as usize;
        let last = |k: usize, n: usize| (((hi[k] - o[k]) / cell) as usize + 1).min(n);
        for z in first(1)..last(1, h) {
            for x in first(0)..last(0, w) {
                let c = [
                    o[0] + (x as f32 + 0.5) * cell,
                    o[1] + (z as f32 + 0.5) * cell,
                ];
                if crate::validate::point_in_polygon(c, &p.polygon) {
                    cells[z * w + x] = p.zone;
                }
            }
        }
    }
    Zones { cells }
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
                elevation: 0.0,
            });
        }
    }
    let mut edges = Vec::new();
    let mut push = |a: u32, b: u32, line: u32| {
        let (class, lanes, width_m) = test_grid_road(line);
        let (pa, pb) = (nodes[a as usize].pos, nodes[b as usize].pos);
        edges.push(Edge {
            a,
            b,
            class,
            lanes_fwd: lanes / 2,
            lanes_back: lanes / 2,
            width_m,
            sidewalk: true,
            speed_kph: if class == RoadClass::Arterial { 56 } else { 40 },
            oneway: false,
            profile: vec![[pa[0], 0.0, pa[1]], [pb[0], 0.0, pb[1]]],
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
            height_m: height,
            floors,
            roof: sp.roof,
            style: sp.style,
            built_year: None,
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
            height_m: 6.5,
            floors: 1,
            roof: Roof::Flat,
            built_year: None,
            style: StyleTag::StripMall,
            usage: BuildingUse::Commercial,
        });
        self.buildings.push(Building {
            footprint: b.rect(front, b.x1 - 34.0, b.x1 - 10.0, 4.0, 20.0),
            height_m: 5.6,
            floors: 1,
            roof: Roof::Flat,
            built_year: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlaceError, Table};

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
        assert_eq!(a.content_hash(), 0x324D_158F_EC2A_D198);
    }

    #[test]
    fn test_grid_has_its_start_zones_and_sorted_tables() {
        let p = Place::test_grid();
        let s = p.player_start().unwrap();
        assert_eq!(s.pos, TEST_GRID_START);
        assert_eq!(s.heading, TEST_GRID_START_HEADING);
        assert_eq!(p.spawns.len(), 1);
        let zone = |x: f32, z: f32| {
            let o = p.header.origin_m();
            let cell = p.header.chunk_size_m / f32::from(p.terrain.res_per_chunk);
            let w = Zones::cells_per_side(&p.header, p.terrain.res_per_chunk)[0];
            p.zones.cells[((z - o[1]) / cell) as usize * w + ((x - o[0]) / cell) as usize]
        };
        // Storefronts on the strip, the park, houses in the corner, the empty margin.
        assert_eq!(zone(-60.0, 20.0), ZoneId::Strip);
        assert!(p.zones.cells.contains(&ZoneId::Park));
        assert!(p.zones.cells.contains(&ZoneId::Downtown));
        assert_eq!(zone(-340.0, -340.0), ZoneId::Residential);
        assert_eq!(zone(-500.0, -500.0), ZoneId::Residential);
        for t in Table::ALL {
            let keys: Vec<u32> = (0..p.table_len(t)).map(|i| p.chunk_key(t, i)).collect();
            assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{t:?}");
            assert!(keys.iter().all(|&k| k != u32::MAX), "{t:?}");
        }
        let mut q = p.clone();
        q.spawns.clear();
        assert_eq!(q.validate(), Err(PlaceError::NoPlayerStart));
    }
}
