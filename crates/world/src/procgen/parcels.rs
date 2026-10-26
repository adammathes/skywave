//! Stage 5 of `docs/WORLD_GENERATION.md`: blocks and parcels, generated per 128 m chunk.
//!
//! [`ParcelPlan::new`] does the whole-place part once, from the merged road graph
//! ([`super::streets_all`]): it **polygonizes** the graph's planar faces with a half-edge
//! traversal (edges sorted around each node by angle; crossings without a node, such as
//! highway overpasses, are split for the traversal only). Dangling edges (cul-de-sacs,
//! driveways) stay in the traversal: the walk goes up one side and back down the other, so
//! the block wraps around them and lots front them. Three kinds of *virtual* lines join the
//! traversal, never as frontage:
//! - the place boundary (2 m inside the place), so the land around the town is a face;
//! - a **bridge** from the northmost point of every connected piece of the graph straight
//!   north to the first road of another piece (or the boundary), so no face has a hole;
//! - **cell cuts** every 256 m (offset from the chunk borders) across faces that touch the
//!   boundary or exceed 1 km², so open land comes in cells a chunk can afford to build.
//!
//! It drops the outer faces, cul-de-sac bulb islands (faces of at most two roads), faces
//! with a hole left, and closed faces in `Beach`/`Water` zones unless they are parking lots
//! (faces with an `Alley` aisle). Faces cut by the boundary or cell lines, and closed faces
//! with water in them, are **open**: they get lots along their roads only, and the land
//! beyond is not parceled (nor are lots on water, beach or the highway corridor, or lots
//! far larger than their zone's).
//!
//! [`ParcelPlan::chunk`] then builds every face that overlaps the chunk and keeps the
//! parcels whose centroid (the vertex mean, as `place::Place::chunk_of` computes it) lies in
//! it. Building a face:
//! - **Block**: the face boundary is inset per side by half the road's width plus its
//!   sidewalk, with mitered corners: acute convex tips are cut where they get narrower than
//!   6 m, sharp reflex corners and dead ends get a flat cap, collinear joins of roads of
//!   different widths a step. Sides that collapse are removed (edge events), and what is
//!   left of a self-intersection is split into loops; only counter-clockwise loops whose
//!   vertices all keep their distance from every road of the face are blocks.
//! - **Strips**: the block boundary along streets (not the highway or its ramps) is grouped
//!   into *runs*: sides that continue within 35°. Longest run first, a frontage strip
//!   `depth_m` deep (the zone's [`Lots`]) is cut off along the run by the run's offset
//!   polyline, extended along its end tangents until it meets the boundary; blocks up to
//!   `2 × depth_m + 20 m` deep across from another road are split back to back instead.
//!   Runs shorter than the depth or with no room behind them get no strip; a cut that
//!   would leave the rest without frontage, or that wanders off the run, is not made. The
//!   rest is an interior core (deeper than `depth_m + 20 m`: `Park` in town zones,
//!   `LotEmpty` elsewhere) or a strip of its own.
//! - **Lots**: each strip is cut along its run into lots of the zone's widths by rays
//!   perpendicular to the frontage; corner lots take the strip's ends.
//! - **Farms**: one 60 m farmstead parcel around each farm driveway's end, cut out by the
//!   square's outline; the rest becomes `LotEmpty` fields, cut like lots at field widths.
//! - **Parking** faces are one lot each.
//!
//! Lots narrower than 8 m or smaller than 80 m² become `Park` (town zones) or `LotEmpty`.
//! A parcel's zone is the zone cell at its area centroid; lots in `Park` zones are `Park`,
//! in `Highway`/`Water` zones `LotEmpty`. Its frontage edge is the street it shares the most
//! property line with, checked to lie within `place::validate`'s reach.
//!
//! **Why hand-written geometry.** Every cut is a *chord split*: a simple polygon and a path
//! through its interior from one boundary point to another give exactly two simple
//! polygons. Parcels therefore partition their block exactly (no overlaps, no gaps) by
//! construction, every cut is validated where it is made and skipped if it is bad, and the
//! only general polygon operation needed is the inset above, which our blocks (polylines
//! with dead ends, no holes) keep simple. `geo` would cover the inset and boolean
//! operations but not the cutting, needs `std`, and is far beyond the ~5k-line budget for
//! a dependency; `spade`'s triangulations are not needed here.
//!
//! **Determinism.** Everything is plain `f32`/`f64` arithmetic in a fixed order and sorts
//! with total orders. A face is rotated to a canonical start and its random stream is
//! `Rng::new(seed, [PARCELS, id])`, where `id` hashes the face's sorted node positions
//! quantized to 0.1 m, so the same face built from any chunk yields the same parcels.

use alloc::vec;
use alloc::vec::Vec;

use place::{LotKind, Parcel, RoadClass, RoadGraph, ZoneId, SIDEWALK_WIDTH_M};
use skywave_core::{hash_bytes, Rng, Seed};

use super::macro_map::MacroMap;
use super::streets::DRIVEWAY_WIDTH_M;
use super::{stage, CHUNK_M};
use crate::math::{abs, floor, intersect_lines, sqrt, V2};
use crate::template::{Lots, RegionTemplate};

/// No road (an interior cut) or no run.
const NONE: u32 = u32::MAX;
/// Faces larger than this are not blocks (open hill country, the land beyond the town).
const MAX_FACE_M2: f32 = 1.0e6;
/// Narrower lots are slivers.
const SLIVER_M: f32 = 8.0;
/// Smaller lots are slivers.
const SLIVER_M2: f32 = 80.0;
/// Blocks smaller than this are dropped.
const MIN_BLOCK_M2: f32 = 40.0;
/// Room an interior core needs beyond the frontage strips.
const CORE_M: f32 = 20.0;
/// Shallowest back-to-back half.
const MIN_DEPTH_M: f32 = 10.0;
/// Consecutive block sides turning less than this (cosine of ~35°) belong to one run.
const RUN_COS: f32 = 0.82;
/// Reflex corners whose miter reaches farther than this many offsets get a flat cap.
const MITER_LIMIT: f32 = 2.0;
/// Acute block tips are cut where they get narrower than this.
const MIN_TIP_M: f32 = 6.0;
/// Edge of the square farmstead parcel around a driveway's end.
const FARMSTEAD_M: f32 = 60.0;
/// Interior cores larger than this are `LotEmpty` even in suburbs.
const BIG_CORE_M2: f32 = 40_000.0;
/// Vertices closer than this are merged.
const EPS_M: f32 = 1e-3;
/// How far a block vertex may come inside a road's property line.
const SLACK_M: f32 = 0.1;
/// Cap on loop splits when cleaning one inset.
const MAX_SPLITS: u32 = 256;
/// The virtual place boundary lies this far inside the place.
const BOUNDARY_M: f32 = 2.0;
/// Open and very large faces are cut into cells of this size, by lines offset from the
/// chunk borders (where street nodes sit) by `CELL_SHIFT_M`.
const CELL_CUT_M: f32 = 256.0;
const CELL_SHIFT_M: f32 = 64.5;
/// Half the gap parcels keep from virtual lines (boundary, bridges, cell cuts).
const VIRTUAL_OFF_M: f32 = 0.25;

/// What a kept face becomes.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceKind {
    /// Frontage strips cut into lots, maybe an interior core.
    Lots = 0,
    /// A beach parking lot: one lot.
    Parking = 1,
    /// Farmsteads at the driveway ends, fields elsewhere.
    Farm = 2,
}

/// A face of the merged road graph that holds parcels.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Canonical id: hash of the sorted boundary node positions quantized to 0.1 m.
    pub id: u64,
    /// Road centerline boundary, counter-clockwise, from a canonical start. Dangling edges
    /// appear twice (up one side, down the other).
    pub ring: Vec<[f32; 2]>,
    /// Road edge (index into the graph's edges) of each side `ring[i] → ring[i + 1]`;
    /// `u32::MAX` for virtual sides (place boundary, bridges, cell cuts).
    pub edges: Vec<u32>,
    pub area_m2: f32,
    /// Most common zone over the face.
    pub zone: ZoneId,
    pub kind: FaceKind,
    /// Built in open mode: lots along the roads only, the rest left out. Faces cut by the
    /// place boundary or cell lines, and faces with water, are open.
    pub open: bool,
    /// `[x0, z0, x1, z1]` of the ring.
    pub bbox: [f32; 4],
    /// Farm driveway ends on the boundary and the unit direction the driveway arrives in.
    pub farmsteads: Vec<([f32; 2], [f32; 2])>,
}

/// Counts of the faces the traversal found and why faces were dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaceStats {
    /// Every face walked, including the outer ones.
    pub walked: u32,
    pub outer: u32,
    /// Cul-de-sac bulb islands and other faces of at most two edges or a few m².
    pub islands: u32,
    pub too_big: u32,
    pub water: u32,
    /// Faces mostly in `Beach`/`Water` zones that are not parking lots.
    pub beach: u32,
    /// Faces with a disconnected piece of road inside.
    pub holes: u32,
    pub kept: u32,
    /// Kept faces built in open mode.
    pub open: u32,
    /// Crossings without a node (overpasses, bridges), split for the traversal.
    pub crossings: u32,
    /// Cell lines cutting open and very large faces.
    pub cuts: u32,
}

/// The buildable polygon of (part of) a face: inside the property lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub face: u64,
    /// Simple, counter-clockwise.
    pub polygon: Vec<[f32; 2]>,
    /// The face's zone.
    pub zone: ZoneId,
    /// From an open face: only the lots along its roads are parcels.
    pub open: bool,
}

/// Everything one face yields, before chunking.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceParcels {
    pub blocks: Vec<Block>,
    pub parcels: Vec<Parcel>,
    /// Pieces left out: not simple, or no road within reach (should stay 0).
    pub dropped: u32,
    /// Slivers (under 160 m²) left out because no road is within reach.
    pub omitted: u32,
}

/// The parcels whose centroid lies in one chunk, in face order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParcelChunk {
    pub chunk: [i32; 2],
    pub parcels: Vec<Parcel>,
    /// Parallel to `parcels`: the [`Face::id`] each came from.
    pub faces: Vec<u64>,
}

/// Generates the parcels of `chunk` from the merged road graph `roads`. Builds a
/// [`ParcelPlan`] first; when generating many chunks, build the plan once.
pub fn parcels(
    seed: Seed,
    template: &RegionTemplate,
    macro_map: &MacroMap,
    roads: &RoadGraph,
    chunk: [i32; 2],
) -> ParcelChunk {
    ParcelPlan::new(seed, template, macro_map, roads).chunk(chunk)
}

/// Every chunk's parcels, in chunk order (z major), each chunk's in face order.
pub fn parcels_all(
    seed: Seed,
    template: &RegionTemplate,
    macro_map: &MacroMap,
    roads: &RoadGraph,
) -> Vec<Parcel> {
    let plan = ParcelPlan::new(seed, template, macro_map, roads);
    plan.all().into_iter().flat_map(|c| c.parcels).collect()
}

/// Content hash of parcels (every field, in order), for determinism tests.
pub fn hash_parcels(parcels: &[Parcel]) -> u64 {
    let mut b: Vec<u8> = Vec::new();
    for p in parcels {
        b.extend_from_slice(&(p.polygon.len() as u32).to_le_bytes());
        for q in &p.polygon {
            b.extend_from_slice(&q[0].to_bits().to_le_bytes());
            b.extend_from_slice(&q[1].to_bits().to_le_bytes());
        }
        b.extend_from_slice(&[p.zone as u8, p.lot_kind as u8]);
        b.extend_from_slice(&p.frontage_edge.to_le_bytes());
    }
    hash_bytes(0x5041_5243_454C, &b)
}

// ---------------------------------------------------------------------------------------
// Plan.

/// The whole-place part of stage 5: the faces of the merged road graph and which chunks
/// they overlap. [`ParcelPlan::chunk`] builds chunks from it. See the module docs.
pub struct ParcelPlan<'a> {
    seed: Seed,
    t: &'a RegionTemplate,
    m: &'a MacroMap,
    g: &'a RoadGraph,
    faces: Vec<Face>,
    stats: FaceStats,
    size: [i32; 2],
    /// Faces overlapping each chunk (z major): `items[start[c]..start[c + 1]]`.
    start: Vec<u32>,
    items: Vec<u32>,
}

impl<'a> ParcelPlan<'a> {
    /// The plan for `macro_map` and its merged road graph `roads` under `template`.
    pub fn new(
        seed: Seed,
        template: &'a RegionTemplate,
        macro_map: &'a MacroMap,
        roads: &'a RoadGraph,
    ) -> ParcelPlan<'a> {
        let size = [
            i32::from(macro_map.size_chunks[0]),
            i32::from(macro_map.size_chunks[1]),
        ];
        let mut plan = ParcelPlan {
            seed,
            t: template,
            m: macro_map,
            g: roads,
            faces: Vec::new(),
            stats: FaceStats::default(),
            size,
            start: Vec::new(),
            items: Vec::new(),
        };
        plan.find_faces();
        plan.index();
        plan
    }

    /// The kept faces.
    pub fn faces(&self) -> &[Face] {
        &self.faces
    }

    pub fn stats(&self) -> FaceStats {
        self.stats
    }

    /// Indices into [`ParcelPlan::faces`] of the faces whose bounds overlap `chunk`.
    pub fn faces_in(&self, chunk: [i32; 2]) -> &[u32] {
        let [sx, sz] = self.size;
        if chunk[0] < 0 || chunk[1] < 0 || chunk[0] >= sx || chunk[1] >= sz {
            return &[];
        }
        let c = (chunk[1] * sx + chunk[0]) as usize;
        &self.items[self.start[c] as usize..self.start[c + 1] as usize]
    }

    /// The parcels of `chunk`.
    pub fn chunk(&self, chunk: [i32; 2]) -> ParcelChunk {
        let mut out = ParcelChunk {
            chunk,
            ..ParcelChunk::default()
        };
        for &f in self.faces_in(chunk) {
            let built = self.build_face(f as usize);
            for p in built.parcels {
                if chunk_of(&p.polygon) == Some(chunk) {
                    out.parcels.push(p);
                    out.faces.push(self.faces[f as usize].id);
                }
            }
        }
        out
    }

    /// Every chunk, z major, building each face once.
    pub fn all(&self) -> Vec<ParcelChunk> {
        let [sx, sz] = self.size;
        let mut out: Vec<ParcelChunk> = (0..sz)
            .flat_map(|z| (0..sx).map(move |x| [x, z]))
            .map(|chunk| ParcelChunk {
                chunk,
                ..ParcelChunk::default()
            })
            .collect();
        // Chunks list their faces in index order; so do the per-face runs here.
        let mut per: Vec<Vec<(u32, Parcel)>> = vec![Vec::new(); out.len()];
        for f in 0..self.faces.len() {
            for p in self.build_face(f).parcels {
                if let Some(c) = chunk_of(&p.polygon) {
                    if c[0] < sx && c[1] < sz {
                        per[(c[1] * sx + c[0]) as usize].push((f as u32, p));
                    }
                }
            }
        }
        for (o, list) in out.iter_mut().zip(per) {
            for (f, p) in list {
                o.parcels.push(p);
                o.faces.push(self.faces[f as usize].id);
            }
        }
        out
    }

    /// Builds face `face` (an index into [`ParcelPlan::faces`]): its blocks and all its
    /// parcels, whatever chunk they fall in.
    pub fn build_face(&self, face: usize) -> FaceParcels {
        let f = &self.faces[face];
        let mut rng = Rng::new(self.seed, &[stage::PARCELS, f.id]);
        let ring: Vec<V2> = f.ring.iter().map(|&p| V2::from_array(p)).collect();
        let offs: Vec<f32> = f
            .edges
            .iter()
            .map(|&e| {
                if e == NONE {
                    VIRTUAL_OFF_M
                } else {
                    self.offset(e)
                }
            })
            .collect();
        let mut out = FaceParcels::default();
        for block in inset(&ring, &f.edges, &offs) {
            let poly: Vec<[f32; 2]> = block.iter().map(|v| [v.p.x, v.p.y]).collect();
            if !simple_ccw(&block) {
                continue;
            }
            out.blocks.push(Block {
                face: f.id,
                polygon: poly,
                zone: f.zone,
                open: f.open,
            });
            let block = with_runs(block, |e| self.faces_lots(e));
            let mut pieces: Vec<(Vec<Vx>, Role)> = Vec::new();
            match f.kind {
                FaceKind::Parking => pieces.push((block, Role::Parking)),
                FaceKind::Farm => self.farm(f, block, &mut rng, &mut pieces),
                FaceKind::Lots => {
                    self.lots(f.zone, block, &mut rng, Role::Lot, f.open, &mut pieces)
                }
            }
            for (piece, role) in pieces {
                if f.open && (self.unwanted(&piece) || self.oversized(&piece, role)) {
                    continue;
                }
                match self.parcel(f, &piece, role) {
                    Some(p) => out.parcels.push(p),
                    None if area(&piece) < 2.0 * SLIVER_M2 => out.omitted += 1,
                    None => out.dropped += 1,
                }
            }
        }
        out
    }

    // -- Faces -----------------------------------------------------------------------------

    /// Whether lots may face road `edge`: not the highway or its one-way ramps.
    fn faces_lots(&self, edge: u32) -> bool {
        let e = &self.g.edges[edge as usize];
        !(e.class == RoadClass::Highway || e.oneway)
    }

    /// Half of the road plus its sidewalk: how far the property line is from the
    /// centerline.
    fn offset(&self, edge: u32) -> f32 {
        let e = &self.g.edges[edge as usize];
        let walk = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
        e.width_m * 0.5 + walk
    }

    fn lots_rule(&self, zone: ZoneId) -> Lots {
        self.t.zones.get(zone).map_or(
            Lots {
                width_m: (20.0, 40.0),
                depth_m: 40.0,
                setback_m: (0.0, 0.0, 0.0),
            },
            |r| r.lots,
        )
    }

    fn find_faces(&mut self) {
        let g = self.g;
        let ext = V2::new(self.size[0] as f32 * CHUNK_M, self.size[1] as f32 * CHUNK_M);
        let mut pos: Vec<V2> = g.nodes.iter().map(|n| V2::from_array(n.pos)).collect();
        let mut lines: Vec<PLine> = g
            .edges
            .iter()
            .enumerate()
            .map(|(e, edge)| PLine {
                edge: e as u32,
                kind: LineKind::Road,
                a: edge.a,
                b: edge.b,
                pts: edge.profile.iter().map(|p| V2::new(p[0], p[2])).collect(),
            })
            .collect();
        // The place boundary, a little inside the place.
        let (b0, b1) = (
            V2::new(BOUNDARY_M, BOUNDARY_M),
            ext - V2::new(BOUNDARY_M, BOUNDARY_M),
        );
        let corners = [b0, V2::new(b1.x, b0.y), b1, V2::new(b0.x, b1.y)];
        let c = pos.len() as u32;
        pos.extend_from_slice(&corners);
        for k in 0..4u32 {
            lines.push(PLine {
                edge: NONE,
                kind: LineKind::Boundary,
                a: c + k,
                b: c + (k + 1) % 4,
                pts: vec![corners[k as usize], corners[((k + 1) % 4) as usize]],
            });
        }
        bridges(g, &mut pos, &mut lines, BOUNDARY_M);
        let pg = Planar::new(pos.clone(), &lines);
        self.stats.crossings = pg.crossings;
        // Faces that are open (reach the place boundary) or too large are cut into cells.
        let mut cuts: Vec<(V2, V2)> = Vec::new();
        for walk in pg.walks() {
            let (ring, side) = pg.ring(&walk);
            if ring.len() < 3 {
                continue;
            }
            let area = ring_area(&ring);
            let open = side
                .iter()
                .any(|&s| pg.pieces[s as usize].kind == LineKind::Boundary);
            if area > 0.0 && (open || area > MAX_FACE_M2) {
                cell_cuts(&ring, ext, &mut cuts);
            }
        }
        self.stats.cuts = cuts.len() as u32;
        let pg = if cuts.is_empty() {
            pg
        } else {
            for (a, b) in cuts {
                let n = pos.len() as u32;
                pos.push(a);
                pos.push(b);
                lines.push(PLine {
                    edge: NONE,
                    kind: LineKind::Cut,
                    a: n,
                    b: n + 1,
                    pts: vec![a, b],
                });
            }
            Planar::new(pos, &lines)
        };
        let mut on_face = vec![u32::MAX; pg.pos.len()];
        for (k, walk) in pg.walks().iter().enumerate() {
            self.stats.walked += 1;
            if let Some(face) = self.face_of(&pg, walk, k as u32, &mut on_face) {
                self.faces.push(face);
            }
        }
        self.stats.kept = self.faces.len() as u32;
        self.stats.open = self.faces.iter().filter(|f| f.open).count() as u32;
    }

    /// The face of one boundary walk, or `None` (counted) when it is dropped.
    fn face_of(
        &mut self,
        pg: &Planar,
        walk: &[u32],
        mark: u32,
        on_face: &mut [u32],
    ) -> Option<Face> {
        let (mut ring, side) = pg.ring(walk);
        let area = ring_area(&ring);
        if ring.len() < 3 || area <= 0.0 {
            self.stats.outer += 1;
            return None;
        }
        let mut edges: Vec<u32> = side.iter().map(|&s| pg.pieces[s as usize].edge).collect();
        let mut open = side.iter().any(|&s| {
            matches!(
                pg.pieces[s as usize].kind,
                LineKind::Boundary | LineKind::Cut
            )
        });
        let mut distinct: Vec<u32> = edges.iter().copied().filter(|&e| e != NONE).collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.is_empty() || (!open && distinct.len() <= 2) || area < MIN_BLOCK_M2 {
            self.stats.islands += 1;
            return None;
        }
        if area > MAX_FACE_M2 {
            self.stats.too_big += 1;
            return None;
        }
        let mut bb = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for p in &ring {
            bb = [
                bb[0].min(p.x),
                bb[1].min(p.y),
                bb[2].max(p.x),
                bb[3].max(p.y),
            ];
        }
        // Zone and water samples on a grid of at most ~600 points inside the ring.
        let step = sqrt(area / 600.0).max(6.0);
        let mut zc = [0u32; 12];
        let mut wet = 0u32;
        let mut z = bb[1] + step * 0.5;
        while z < bb[3] {
            let mut x = bb[0] + step * 0.5;
            while x < bb[2] {
                if pip_ring(V2::new(x, z), &ring) {
                    let zone = self.m.zone_at(x, z);
                    zc[zone as usize] += 1;
                    if zone == ZoneId::Water || self.m.height_at(x, z) <= self.m.water_level {
                        wet += 1;
                    }
                }
                x += step;
            }
            z += step;
        }
        let zone = if zc.iter().all(|&c| c == 0) {
            let c = centroid(&ring);
            self.m.zone_at(c.x, c.y)
        } else {
            let mut best = 0;
            for k in 1..12 {
                if zc[k] > zc[best] {
                    best = k;
                }
            }
            super::zones::ALL_ZONES[best]
        };
        // Nodes on the boundary; any other road node inside means a hole.
        for &h in walk {
            on_face[pg.origin(h) as usize] = mark;
        }
        for (i, p) in pg.pos.iter().enumerate() {
            if on_face[i] == mark || pg.degree[i] == 0 {
                continue;
            }
            if p.x > bb[0] && p.x < bb[2] && p.y > bb[1] && p.y < bb[3] && pip_ring(*p, &ring) {
                self.stats.holes += 1;
                return None;
            }
        }
        if wet > 0 && !open {
            // Lots along its roads only; the parcels in the water are dropped.
            self.stats.water += 1;
            open = true;
        }
        let alley = distinct
            .iter()
            .any(|&e| self.g.edges[e as usize].class == RoadClass::Alley);
        let mut farmsteads: Vec<([f32; 2], [f32; 2])> = Vec::new();
        for &h in walk {
            let v = pg.dest(h);
            let pc = &pg.pieces[(h / 2) as usize];
            if pc.edge == NONE || pg.degree[v as usize] != 1 {
                continue;
            }
            let e = &self.g.edges[pc.edge as usize];
            if e.class == RoadClass::Alley && e.width_m == DRIVEWAY_WIDTH_M {
                let n = pc.pts.len();
                let (a, b) = if h.is_multiple_of(2) {
                    (pc.pts[n - 2], pc.pts[n - 1])
                } else {
                    (pc.pts[1], pc.pts[0])
                };
                let u = (b - a).normalize();
                farmsteads.push(([b.x, b.y], [u.x, u.y]));
            }
        }
        farmsteads.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]).then(a.0[1].total_cmp(&b.0[1])));
        let kind = if !open && matches!(zone, ZoneId::Beach | ZoneId::Water) {
            if !alley {
                self.stats.beach += 1;
                return None;
            }
            FaceKind::Parking
        } else if !farmsteads.is_empty() || zone == ZoneId::Farmland {
            FaceKind::Farm
        } else {
            FaceKind::Lots
        };
        // Canonical id and start.
        let mut keys: Vec<(i64, i64)> = walk
            .iter()
            .map(|&h| {
                let p = pg.pos[pg.origin(h) as usize];
                (quant(p.x), quant(p.y))
            })
            .collect();
        keys.sort_unstable();
        keys.dedup();
        let mut bytes: Vec<u8> = Vec::with_capacity(keys.len() * 16);
        for (x, z) in &keys {
            bytes.extend_from_slice(&x.to_le_bytes());
            bytes.extend_from_slice(&z.to_le_bytes());
        }
        let id = hash_bytes(0x4641_4345, &bytes);
        let n = ring.len();
        let mut s = 0;
        for i in 1..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (c, d) = (ring[s], ring[(s + 1) % n]);
            let ord =
                a.x.total_cmp(&c.x)
                    .then(a.y.total_cmp(&c.y))
                    .then(b.x.total_cmp(&d.x))
                    .then(b.y.total_cmp(&d.y));
            if ord.is_lt() {
                s = i;
            }
        }
        ring.rotate_left(s);
        edges.rotate_left(s);
        Some(Face {
            id,
            ring: ring.iter().map(|p| [p.x, p.y]).collect(),
            edges,
            area_m2: area,
            zone,
            kind,
            open,
            bbox: bb,
            farmsteads,
        })
    }

    /// Lists the faces overlapping each chunk.
    fn index(&mut self) {
        let [sx, sz] = self.size;
        let n = (sx * sz) as usize;
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (i, f) in self.faces.iter().enumerate() {
            let cx0 = (floor(f.bbox[0] / CHUNK_M) as i32).clamp(0, sx - 1);
            let cz0 = (floor(f.bbox[1] / CHUNK_M) as i32).clamp(0, sz - 1);
            let cx1 = (floor(f.bbox[2] / CHUNK_M) as i32).clamp(0, sx - 1);
            let cz1 = (floor(f.bbox[3] / CHUNK_M) as i32).clamp(0, sz - 1);
            for cz in cz0..=cz1 {
                for cx in cx0..=cx1 {
                    pairs.push(((cz * sx + cx) as u32, i as u32));
                }
            }
        }
        pairs.sort_unstable();
        self.start = vec![0; n + 1];
        for &(c, _) in &pairs {
            self.start[c as usize + 1] += 1;
        }
        for c in 0..n {
            self.start[c + 1] += self.start[c];
        }
        self.items = pairs.into_iter().map(|p| p.1).collect();
    }

    // -- Subdivision -----------------------------------------------------------------------

    /// Frontage strips, lots and an interior core for one block (or one remaining piece of
    /// a farm), appended to `out`. `role` is what the lots become (`Lot` or `Field`).
    fn lots(
        &self,
        zone: ZoneId,
        block: Vec<Vx>,
        rng: &mut Rng,
        role: Role,
        open: bool,
        out: &mut Vec<(Vec<Vx>, Role)>,
    ) {
        let depth = self.lots_rule(zone).depth_m;
        // Runs by length, longest first.
        let mut runs: Vec<(f32, u32)> = Vec::new();
        for (i, v) in block.iter().enumerate() {
            if v.run == NONE {
                continue;
            }
            let len = (block[(i + 1) % block.len()].p - v.p).length();
            match runs.iter_mut().find(|r| r.1 == v.run) {
                Some(r) => r.0 += len,
                None => runs.push((len, v.run)),
            }
        }
        runs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut rest = block;
        let mut strips: Vec<(Vec<Vx>, u32)> = Vec::new();
        for &(_, run) in &runs {
            let Some(ch) = chain(&rest, run) else {
                continue;
            };
            // Short runs of a closed block are left to the corner lots.
            if (!open && ch.len < depth) || ch.len < 2.0 * SLIVER_M {
                continue;
            }
            let (f, dir, k) = point_at(&rest, &ch, ch.len * 0.5);
            let n = dir.left();
            let Some((w, frontage)) = width_at(&rest, f, n) else {
                continue;
            };
            let d = if frontage {
                if w <= depth && !open {
                    continue;
                }
                if w <= 2.0 * depth + CORE_M {
                    w * 0.5
                } else {
                    depth
                }
            } else if open {
                // Whatever is left beyond the strip is dropped.
                depth.min(w - 0.5)
            } else {
                if w <= depth + CORE_M {
                    continue;
                }
                depth
            };
            if d < MIN_DEPTH_M {
                continue;
            }
            let Some(path) = offset_path(&rest, &ch, k, f, n, d) else {
                continue;
            };
            let Some((a, b)) = cut(&rest, &path.0, path.1) else {
                continue;
            };
            let probe = f + n * (d * 0.5).min(0.5);
            let (strip, remain) = if pip(probe, &a) { (a, b) } else { (b, a) };
            if area(&strip) > (ch.len + 2.0 * d) * d * 1.25 {
                // The cut wandered off: this is not a strip along the run.
                continue;
            }
            if !open && (frontage_len(&remain) < SLIVER_M || area(&remain) < SLIVER_M2) {
                // The rest would have no road: leave it as it is and stop cutting.
                break;
            }
            strips.push((strip, run));
            rest = remain;
        }
        for (strip, run) in strips {
            self.cut_lots(strip, run, rng, role, out);
        }
        // The rest: an interior core, or a strip along its longest run.
        let mut best: Option<(f32, u32, Chain)> = None;
        for &(_, run) in &runs {
            if let Some(ch) = chain(&rest, run) {
                if best.as_ref().is_none_or(|b| ch.len > b.0) {
                    best = Some((ch.len, run, ch));
                }
            }
        }
        let Some((_, run, ch)) = best else {
            if !open {
                out.push((rest, role));
            }
            return;
        };
        let (f, dir, _) = point_at(&rest, &ch, ch.len * 0.5);
        let deep = width_at(&rest, f, dir.left()).is_some_and(|(w, _)| w > depth + CORE_M);
        let (_, w1) = self.lots_rule(zone).width_m;
        if open && (deep || area(&rest) > 2.5 * w1 * depth) {
            // Open land beyond the lots.
        } else if deep && role == Role::Lot {
            out.push((rest, Role::Core));
        } else {
            self.cut_lots(rest, run, rng, role, out);
        }
    }

    /// Cuts a strip into lots along its run by rays perpendicular to the frontage.
    fn cut_lots(
        &self,
        strip: Vec<Vx>,
        run: u32,
        rng: &mut Rng,
        role: Role,
        out: &mut Vec<(Vec<Vx>, Role)>,
    ) {
        let Some(ch) = chain(&strip, run) else {
            out.push((strip, role));
            return;
        };
        let (mid, dir, _) = point_at(&strip, &ch, ch.len * 0.5);
        let probe = mid + dir.left() * 5.0;
        let zone = self.m.zone_at(probe.x, probe.y);
        let (w0, w1) = self.lots_rule(zone).width_m;
        let len = ch.len;
        let n_max = floor(len / w0).max(1.0) as u32;
        let n_min = ((-floor(-len / w1)) as u32).clamp(1, n_max);
        let n = n_min + rng.range(0, n_max - n_min + 1);
        let mut widths: Vec<f32> = (0..n).map(|_| rng.range_f32(w0, w1)).collect();
        let total: f32 = widths.iter().sum();
        for w in &mut widths {
            *w *= len / total;
        }
        // Cut points along the chain, measured on the uncut strip.
        let mut cuts: Vec<(V2, V2)> = Vec::with_capacity(n as usize);
        let mut s = 0.0;
        for w in widths.iter().take(widths.len() - 1) {
            s += *w;
            let (p, d, _) = point_at(&strip, &ch, s);
            cuts.push((p, d));
        }
        let mut rest = strip;
        for (p, d) in cuts {
            let nrm = d.left();
            let path = [p - nrm * 1.0, p + nrm * 0.02, p + nrm * 4000.0];
            if !pip(path[1], &rest) {
                continue;
            }
            let Some((a, b)) = cut(&rest, &path, 1) else {
                continue;
            };
            let probe = p - d * 0.25 + nrm * 0.25;
            let (lot, remain) = if pip(probe, &a) { (a, b) } else { (b, a) };
            out.push((lot, role));
            rest = remain;
        }
        out.push((rest, role));
    }

    /// Farmsteads around the driveway ends, fields elsewhere.
    fn farm(&self, f: &Face, block: Vec<Vx>, rng: &mut Rng, out: &mut Vec<(Vec<Vx>, Role)>) {
        let mut pieces = vec![block];
        let h = FARMSTEAD_M * 0.5;
        for &(end, u) in &f.farmsteads {
            let (c, u) = (V2::from_array(end), V2::from_array(u));
            let v = u.left();
            let inside = c + u * (DRIVEWAY_WIDTH_M * 0.5 + 1.5);
            let Some(k) = pieces.iter().position(|p| pip(inside, p)) else {
                continue;
            };
            let path = [
                c - u * h,
                c - u * h - v * h,
                c + u * h - v * h,
                c + u * h,
                c + u * h + v * h,
                c - u * h + v * h,
                c - u * h,
            ];
            if !pip(path[3], &pieces[k]) {
                continue;
            }
            let Some((a, b)) = cut(&pieces[k], &path, 3) else {
                continue;
            };
            let (stead, rest) = if pip(inside, &a) { (a, b) } else { (b, a) };
            out.push((stead, Role::Farmstead));
            pieces[k] = rest;
        }
        for p in pieces {
            let p = with_runs(p, |e| self.faces_lots(e));
            self.lots(ZoneId::Farmland, p, rng, Role::Field, f.open, out);
        }
    }

    /// The parcel of a finished piece, or `None` if it is not simple or no road is in
    /// reach.
    fn parcel(&self, f: &Face, piece: &[Vx], role: Role) -> Option<Parcel> {
        let polygon: Vec<[f32; 2]> = piece.iter().map(|v| [v.p.x, v.p.y]).collect();
        if !simple_ccw(piece) {
            return None;
        }
        let c = area_centroid(piece);
        let zone = self.m.zone_at(c.x, c.y);
        let frontage_edge = self.frontage(f, piece)?;
        let area = area(piece);
        let town = matches!(
            zone,
            ZoneId::Downtown
                | ZoneId::Strip
                | ZoneId::Residential
                | ZoneId::Commercial
                | ZoneId::Park
                | ZoneId::Suburb
        );
        let lot_kind = match role {
            Role::Core => {
                if town && (zone != ZoneId::Suburb || area < BIG_CORE_M2) {
                    LotKind::Park
                } else {
                    LotKind::LotEmpty
                }
            }
            Role::Field => LotKind::LotEmpty,
            Role::Farmstead | Role::Parking | Role::Lot => {
                if zone == ZoneId::Park {
                    LotKind::Park
                } else if matches!(zone, ZoneId::Highway | ZoneId::Water) {
                    LotKind::LotEmpty
                } else if role != Role::Farmstead && is_sliver(piece, area) {
                    if town {
                        LotKind::Park
                    } else {
                        LotKind::LotEmpty
                    }
                } else {
                    LotKind::Lot
                }
            }
        };
        Some(Parcel {
            polygon,
            zone,
            frontage_edge,
            lot_kind,
        })
    }

    /// The road the piece shares the most property line with, among those within
    /// `place::validate`'s reach; else the nearest road of the face in reach.
    fn frontage(&self, f: &Face, piece: &[Vx]) -> Option<u32> {
        let mut by: Vec<(f32, u32)> = Vec::new();
        for (i, v) in piece.iter().enumerate() {
            if v.tag == NONE {
                continue;
            }
            let len = (piece[(i + 1) % piece.len()].p - v.p).length();
            match by.iter_mut().find(|b| b.1 == v.tag) {
                Some(b) => b.0 += len,
                None => by.push((len, v.tag)),
            }
        }
        // Lots face streets rather than the highway or its ramps.
        by.sort_by(|a, b| {
            let (fa, fb) = (self.faces_lots(a.1), self.faces_lots(b.1));
            fb.cmp(&fa).then(b.0.total_cmp(&a.0)).then(a.1.cmp(&b.1))
        });
        for &(_, e) in &by {
            if self.reaches(e, piece) {
                return Some(e);
            }
        }
        let mut rest: Vec<u32> = f.edges.iter().copied().filter(|&e| e != NONE).collect();
        rest.sort_unstable();
        rest.dedup();
        let mut best: Option<(f64, u32)> = None;
        for e in rest {
            let d = self.gap(e, piece);
            if d <= 0.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, e));
            }
        }
        best.map(|b| b.1)
    }

    /// Squared distance from the piece to road `e` minus its squared reach (with 5 cm to
    /// spare): `<= 0` when the piece touches the road for `place::validate`.
    fn gap(&self, e: u32, piece: &[Vx]) -> f64 {
        let edge = &self.g.edges[e as usize];
        let reach = f64::from(self.offset(e) + place::validate::FRONTAGE_TOLERANCE_M - 0.05);
        polyline_polygon_dist2(&edge.profile, piece, reach as f32 + 1.0) - reach * reach
    }

    fn reaches(&self, e: u32, piece: &[Vx]) -> bool {
        self.gap(e, piece) <= 0.0
    }

    /// Whether a lot or field of an open face is far larger than its zone's lots (a piece of
    /// the open land beyond them), or a field outside farmland.
    fn oversized(&self, piece: &[Vx], role: Role) -> bool {
        let c = area_centroid(piece);
        let zone = self.m.zone_at(c.x, c.y);
        let rule = self.lots_rule(zone);
        match role {
            Role::Lot => area(piece) > 3.0 * rule.width_m.1 * rule.depth_m,
            Role::Field => zone != ZoneId::Farmland,
            _ => false,
        }
    }

    /// Whether a piece of an open face touches water, beach or the highway corridor.
    fn unwanted(&self, piece: &[Vx]) -> bool {
        let c = area_centroid(piece);
        core::iter::once(c)
            .chain(piece.iter().map(|v| v.p))
            .any(|p| {
                matches!(
                    self.m.zone_at(p.x, p.y),
                    ZoneId::Water | ZoneId::Beach | ZoneId::Highway
                ) || self.m.height_at(p.x, p.y) <= self.m.water_level
            })
    }
}

/// Joins every connected piece of the road graph to the first road of another piece (or
/// the boundary at `top`) straight north of its northmost point, so that no face has a
/// hole. Lines from the northmost point are split there.
fn bridges(g: &RoadGraph, pos: &mut Vec<V2>, lines: &mut Vec<PLine>, top: f32) {
    let n = g.nodes.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            let up = p[p[i as usize] as usize];
            p[i as usize] = up;
            i = up;
        }
        i
    }
    for e in &g.edges {
        let (a, b) = (root(&mut parent, e.a), root(&mut parent, e.b));
        if a != b {
            parent[a.max(b) as usize] = a.min(b);
        }
    }
    let mut comp: Vec<u32> = Vec::with_capacity(lines.len());
    // Northmost point per component: (z, x, line, vertex).
    let mut best: Vec<Option<(f32, f32, usize, usize)>> = vec![None; n];
    for (l, line) in lines.iter().enumerate() {
        if line.kind != LineKind::Road {
            comp.push(NONE);
            continue;
        }
        let r = root(&mut parent, line.a);
        comp.push(r);
        for (k, p) in line.pts.iter().enumerate() {
            let b = &mut best[r as usize];
            if b.is_none_or(|b| p.y < b.0 || (p.y == b.0 && p.x < b.1)) {
                *b = Some((p.y, p.x, l, k));
            }
        }
    }
    for (r, b) in best.iter().enumerate() {
        let Some((z, x, l, k)) = *b else {
            continue;
        };
        if z <= top + 1.0 {
            continue;
        }
        let mut reach = z - top;
        for (m, line) in lines.iter().enumerate() {
            if comp[m] == NONE || comp[m] == r as u32 {
                continue;
            }
            for w in line.pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if (a.x > x) == (b.x > x) {
                    continue;
                }
                let at = a.y + (x - a.x) / (b.x - a.x) * (b.y - a.y);
                if at < z - 1e-3 && z - at < reach {
                    reach = z - at;
                }
            }
        }
        let p = V2::new(x, z);
        let last = lines[l].pts.len() - 1;
        let start = if k == 0 {
            lines[l].a
        } else if k == last {
            lines[l].b
        } else {
            let node = pos.len() as u32;
            pos.push(p);
            let tail = PLine {
                edge: lines[l].edge,
                kind: LineKind::Road,
                a: node,
                b: lines[l].b,
                pts: lines[l].pts[k..].to_vec(),
            };
            lines[l].pts.truncate(k + 1);
            lines[l].b = node;
            lines.push(tail);
            comp.push(r as u32);
            node
        };
        let end = V2::new(x, z - reach - 0.05);
        let node = pos.len() as u32;
        pos.push(end);
        lines.push(PLine {
            edge: NONE,
            kind: LineKind::Bridge,
            a: start,
            b: node,
            pts: vec![p, end],
        });
        comp.push(NONE);
    }
}

/// Cell lines (every `CELL_CUT_M` along both axes, shifted) inside a face ring, each running 5 cm
/// past the boundary so it crosses it. Where a line passes a dangling road (the ring has
/// two sides at the same place) it carries on.
fn cell_cuts(ring: &[V2], ext: V2, out: &mut Vec<(V2, V2)>) {
    let mut lo = V2::new(f32::MAX, f32::MAX);
    let mut hi = V2::new(f32::MIN, f32::MIN);
    for p in ring {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let n = ring.len();
    let mut at: Vec<f32> = Vec::new();
    for axis in 0..2 {
        let (lo_a, hi_a, size) = if axis == 0 {
            (lo.x, hi.x, ext.x)
        } else {
            (lo.y, hi.y, ext.y)
        };
        let mut k = 0;
        while (k as f32) * CELL_CUT_M + CELL_SHIFT_M < size {
            let c = k as f32 * CELL_CUT_M + CELL_SHIFT_M;
            k += 1;
            if c <= lo_a || c >= hi_a {
                continue;
            }
            at.clear();
            for i in 0..n {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                let (ac, bc, ao, bo) = if axis == 0 {
                    (a.x, b.x, a.y, b.y)
                } else {
                    (a.y, b.y, a.x, b.x)
                };
                if (ac > c) != (bc > c) {
                    at.push(ao + (c - ac) / (bc - ac) * (bo - ao));
                }
            }
            at.sort_by(|a, b| a.total_cmp(b));
            let mut kept: Vec<f32> = Vec::with_capacity(at.len());
            let mut i = 0;
            while i < at.len() {
                if i + 1 < at.len() && at[i + 1] - at[i] < 1e-3 {
                    i += 2;
                } else {
                    kept.push(at[i]);
                    i += 1;
                }
            }
            if !kept.len().is_multiple_of(2) {
                continue;
            }
            for pair in kept.chunks(2) {
                let (s, e) = (pair[0] - 0.05, pair[1] + 0.05);
                if e - s < 1.1 {
                    continue;
                }
                out.push(if axis == 0 {
                    (V2::new(c, s), V2::new(c, e))
                } else {
                    (V2::new(s, c), V2::new(e, c))
                });
            }
        }
    }
}

/// What a finished piece becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Lot,
    Core,
    Field,
    Farmstead,
    Parking,
}

/// The chunk of a parcel: its vertex mean, as `place::Place::chunk_of` computes it.
fn chunk_of(poly: &[[f32; 2]]) -> Option<[i32; 2]> {
    let (mut sx, mut sz, mut n) = (0.0f32, 0.0f32, 0u32);
    for p in poly {
        sx += p[0];
        sz += p[1];
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let c = [sx / n as f32, sz / n as f32];
    if !(c[0] >= 0.0 && c[1] >= 0.0) {
        return None;
    }
    Some([(c[0] / CHUNK_M) as i32, (c[1] / CHUNK_M) as i32])
}

fn quant(x: f32) -> i64 {
    let s = f64::from(x) * 10.0;
    let f = s as i64;
    // Round half away from zero.
    if s - f as f64 >= 0.5 {
        f + 1
    } else if s - (f as f64) <= -0.5 {
        f - 1
    } else {
        f
    }
}

// ---------------------------------------------------------------------------------------
// The planar graph.

/// What a line of the planar graph is.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Road = 0,
    /// The place boundary, closing the land beyond the town into a face.
    Boundary = 1,
    /// Joins a piece of road graph to the next thing north of it, so it is never a hole.
    Bridge = 2,
    /// A cell line cutting a large or open face into cells.
    Cut = 3,
}

/// A polyline of the planar graph from node `a` to node `b`: a road edge (`edge`) or a
/// virtual line (`edge == NONE`).
#[derive(Clone, Debug)]
struct PLine {
    edge: u32,
    kind: LineKind,
    a: u32,
    b: u32,
    pts: Vec<V2>,
}

/// A piece of a line between two planar nodes (line ends or crossings).
struct PPiece {
    edge: u32,
    kind: LineKind,
    a: u32,
    b: u32,
    pts: Vec<V2>,
}

/// Lines split at their crossings, with half-edges `2 * piece` (a → b) and
/// `2 * piece + 1` (b → a) sorted around their origin by angle.
struct Planar {
    pos: Vec<V2>,
    pieces: Vec<PPiece>,
    degree: Vec<u32>,
    /// Outgoing half-edges per node, counter-clockwise: `out[start[v]..start[v + 1]]`.
    start: Vec<u32>,
    out: Vec<u32>,
    /// Position of each half-edge in its origin's list.
    slot: Vec<u32>,
    crossings: u32,
}

impl Planar {
    /// The planar graph of `lines` over `pos` (line ends index it). Crossings away from the
    /// ends become nodes; virtual pieces shorter than 0.2 m dangling from a crossing (the
    /// overshoot that makes virtual lines cross what they end on) are dropped.
    fn new(mut pos: Vec<V2>, lines: &[PLine]) -> Planar {
        // Every segment, then crossings by a grid.
        let mut segs: Vec<(u32, u32)> = Vec::new();
        let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
        for (l, line) in lines.iter().enumerate() {
            for k in 0..line.pts.len().saturating_sub(1) {
                segs.push((l as u32, k as u32));
            }
            for p in &line.pts {
                lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
            }
        }
        let seg_pts = |s: (u32, u32)| {
            let pts = &lines[s.0 as usize].pts;
            (pts[s.1 as usize], pts[s.1 as usize + 1])
        };
        const CELL: f32 = 32.0;
        let gw = if segs.is_empty() {
            1
        } else {
            (floor((hi.x - lo.x) / CELL) as usize) + 1
        };
        let mut cells: Vec<(u32, u32)> = Vec::new();
        for (i, &s) in segs.iter().enumerate() {
            let (a, b) = seg_pts(s);
            let x0 = floor((a.x.min(b.x) - lo.x) / CELL) as usize;
            let x1 = floor((a.x.max(b.x) - lo.x) / CELL) as usize;
            let z0 = floor((a.y.min(b.y) - lo.y) / CELL) as usize;
            let z1 = floor((a.y.max(b.y) - lo.y) / CELL) as usize;
            for z in z0..=z1 {
                for x in x0..=x1 {
                    cells.push(((z * gw + x) as u32, i as u32));
                }
            }
        }
        cells.sort_unstable();
        let mut hits: Vec<(u32, u32, V2)> = Vec::new();
        let mut c0 = 0;
        while c0 < cells.len() {
            let mut c1 = c0 + 1;
            while c1 < cells.len() && cells[c1].0 == cells[c0].0 {
                c1 += 1;
            }
            for i in c0..c1 {
                for j in i + 1..c1 {
                    let (si, sj) = (cells[i].1, cells[j].1);
                    let (a, b) = (segs[si as usize], segs[sj as usize]);
                    if a.0 == b.0 && a.1.abs_diff(b.1) <= 1 {
                        continue;
                    }
                    let (p1, p2) = seg_pts(a);
                    let (q1, q2) = seg_pts(b);
                    if let Some((t, u)) = seg_x(p1, p2, q1, q2) {
                        let inner = |x: f64| x > 1e-5 && x < 1.0 - 1e-5;
                        if inner(t) && inner(u) {
                            let x = p1 + (p2 - p1) * t as f32;
                            hits.push((si.min(sj), si.max(sj), x));
                        }
                    }
                }
            }
            c0 = c1;
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        hits.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        // Split points per line: (segment, t, node).
        let mut split: Vec<Vec<(u32, f32, u32)>> = vec![Vec::new(); lines.len()];
        for &(si, sj, x) in &hits {
            let node = pos.len() as u32;
            pos.push(x);
            for s in [si, sj] {
                let (l, k) = segs[s as usize];
                let (a, b) = seg_pts((l, k));
                let ab = b - a;
                let t = (x - a).dot(ab) / ab.dot(ab);
                split[l as usize].push((k, t, node));
            }
        }
        let crossings = hits.len() as u32;
        let mut pieces: Vec<PPiece> = Vec::new();
        for (l, line) in lines.iter().enumerate() {
            let sp = &mut split[l];
            sp.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2)));
            let pr = &line.pts;
            let mut a = line.a;
            let mut pts = vec![pr[0]];
            let mut k = 0;
            for s in 0..pr.len().saturating_sub(1) {
                while k < sp.len() && sp[k].0 as usize == s {
                    let node = sp[k].2;
                    pts.push(pos[node as usize]);
                    push_piece(&mut pieces, line, a, node, core::mem::take(&mut pts));
                    pts.push(pos[node as usize]);
                    a = node;
                    k += 1;
                }
                pts.push(pr[s + 1]);
            }
            push_piece(&mut pieces, line, a, line.b, pts);
        }
        let count = |pieces: &[PPiece], degree: &mut Vec<u32>| {
            degree.clear();
            degree.resize(pos.len(), 0);
            for pc in pieces {
                degree[pc.a as usize] += 1;
                degree[pc.b as usize] += 1;
            }
        };
        let mut degree: Vec<u32> = Vec::new();
        count(&pieces, &mut degree);
        // Drop virtual overshoots, then recount.
        let short = |pc: &PPiece| {
            pc.kind != LineKind::Road
                && pc
                    .pts
                    .windows(2)
                    .map(|w| (w[1] - w[0]).length())
                    .sum::<f32>()
                    < 0.2
        };
        loop {
            let before = pieces.len();
            let deg = &degree;
            pieces
                .retain(|pc| !(short(pc) && (deg[pc.a as usize] == 1 || deg[pc.b as usize] == 1)));
            if pieces.len() == before {
                break;
            }
            count(&pieces, &mut degree);
        }
        let mut lists: Vec<(u32, f32, u32)> = Vec::with_capacity(pieces.len() * 2);
        for (i, pc) in pieces.iter().enumerate() {
            let n = pc.pts.len();
            lists.push((pc.a, (pc.pts[1] - pc.pts[0]).pseudo_angle(), 2 * i as u32));
            lists.push((
                pc.b,
                (pc.pts[n - 2] - pc.pts[n - 1]).pseudo_angle(),
                2 * i as u32 + 1,
            ));
        }
        lists.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2)));
        let mut start = vec![0u32; pos.len() + 1];
        for l in &lists {
            start[l.0 as usize + 1] += 1;
        }
        for v in 0..pos.len() {
            start[v + 1] += start[v];
        }
        let mut slot = vec![0u32; pieces.len() * 2];
        let out: Vec<u32> = lists.iter().map(|l| l.2).collect();
        for (i, &h) in out.iter().enumerate() {
            let v = lists[i].0;
            slot[h as usize] = i as u32 - start[v as usize];
        }
        Planar {
            pos,
            pieces,
            degree,
            start,
            out,
            slot,
            crossings,
        }
    }

    /// Every face walk (half-edge lists), in order of their first half-edge.
    fn walks(&self) -> Vec<Vec<u32>> {
        let nh = self.pieces.len() * 2;
        let mut seen = vec![false; nh];
        let mut out = Vec::new();
        for h0 in 0..nh as u32 {
            if seen[h0 as usize] {
                continue;
            }
            let mut walk = Vec::new();
            let mut h = h0;
            loop {
                seen[h as usize] = true;
                walk.push(h);
                h = self.next(h);
                if h == h0 || walk.len() > nh {
                    break;
                }
            }
            out.push(walk);
        }
        out
    }

    /// The boundary points of a walk with, per side, the piece it lies on.
    fn ring(&self, walk: &[u32]) -> (Vec<V2>, Vec<u32>) {
        let mut ring: Vec<V2> = Vec::new();
        let mut side: Vec<u32> = Vec::new();
        for &h in walk {
            let pc = &self.pieces[(h / 2) as usize];
            let n = pc.pts.len();
            for k in 0..n - 1 {
                ring.push(if h.is_multiple_of(2) {
                    pc.pts[k]
                } else {
                    pc.pts[n - 1 - k]
                });
                side.push(h / 2);
            }
        }
        dedup_ring(ring, side)
    }

    fn origin(&self, h: u32) -> u32 {
        let p = &self.pieces[(h / 2) as usize];
        if h.is_multiple_of(2) {
            p.a
        } else {
            p.b
        }
    }

    fn dest(&self, h: u32) -> u32 {
        self.origin(h ^ 1)
    }

    /// The next half-edge around the face to the left of `h`: at `h`'s end, the outgoing
    /// half-edge just clockwise of the way back.
    fn next(&self, h: u32) -> u32 {
        let v = self.dest(h) as usize;
        let (s, e) = (self.start[v], self.start[v + 1]);
        let deg = e - s;
        let back = self.slot[(h ^ 1) as usize];
        self.out[(s + (back + deg - 1) % deg) as usize]
    }
}

fn push_piece(pieces: &mut Vec<PPiece>, line: &PLine, a: u32, b: u32, pts: Vec<V2>) {
    let mut clean: Vec<V2> = Vec::with_capacity(pts.len());
    for p in pts {
        if clean.last().is_none_or(|&q| (p - q).length() > EPS_M) {
            clean.push(p);
        }
    }
    if clean.len() >= 2 && a != b {
        pieces.push(PPiece {
            edge: line.edge,
            kind: line.kind,
            a,
            b,
            pts: clean,
        });
    }
}

/// Merges ring points closer than `EPS_M`; the side leaving a merged point keeps the later
/// tag.
fn dedup_ring(ring: Vec<V2>, tags: Vec<u32>) -> (Vec<V2>, Vec<u32>) {
    let mut r: Vec<V2> = Vec::with_capacity(ring.len());
    let mut t: Vec<u32> = Vec::with_capacity(ring.len());
    for (p, g) in ring.into_iter().zip(tags) {
        if let Some(&q) = r.last() {
            if (p - q).length() <= EPS_M {
                *t.last_mut().expect("tag") = g;
                continue;
            }
        }
        r.push(p);
        t.push(g);
    }
    while r.len() > 1 && (r[r.len() - 1] - r[0]).length() <= EPS_M {
        r.pop();
        t.pop();
    }
    (r, t)
}

fn ring_area(ring: &[V2]) -> f32 {
    let mut acc = 0.0f64;
    for (i, p) in ring.iter().enumerate() {
        let q = ring[(i + 1) % ring.len()];
        acc += f64::from(p.y) * f64::from(q.x) - f64::from(p.x) * f64::from(q.y);
    }
    (acc * 0.5) as f32
}

fn centroid(ring: &[V2]) -> V2 {
    let mut s = V2::new(0.0, 0.0);
    for p in ring {
        s = s + *p;
    }
    s * (1.0 / ring.len() as f32)
}

/// Even-odd point in polygon (in `f64`).
fn pip_ring(p: V2, ring: &[V2]) -> bool {
    let (px, pz) = (f64::from(p.x), f64::from(p.y));
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let (ax, az, bx, bz) = (
            f64::from(a.x),
            f64::from(a.y),
            f64::from(b.x),
            f64::from(b.y),
        );
        if (az > pz) != (bz > pz) {
            let x = ax + (pz - az) / (bz - az) * (bx - ax);
            if px < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Intersection of the closed segments `p1 p2` and `q1 q2`: `(t, u)` along each, or `None`
/// when they miss or are parallel.
fn seg_x(p1: V2, p2: V2, q1: V2, q2: V2) -> Option<(f64, f64)> {
    let (rx, rz) = (f64::from(p2.x - p1.x), f64::from(p2.y - p1.y));
    let (sx, sz) = (f64::from(q2.x - q1.x), f64::from(q2.y - q1.y));
    let den = rx * sz - rz * sx;
    if den == 0.0 {
        return None;
    }
    let (wx, wz) = (f64::from(q1.x - p1.x), f64::from(q1.y - p1.y));
    let t = (wx * sz - wz * sx) / den;
    let u = (wx * rz - wz * rx) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

// ---------------------------------------------------------------------------------------
// Polygons with tagged sides.

/// A polygon vertex; the side from it to the next vertex fronts road `tag` (`NONE` for an
/// interior cut) and belongs to run `run`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vx {
    p: V2,
    tag: u32,
    run: u32,
}

fn area(poly: &[Vx]) -> f32 {
    let mut acc = 0.0f64;
    for (i, v) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()].p;
        acc += f64::from(v.p.y) * f64::from(q.x) - f64::from(v.p.x) * f64::from(q.y);
    }
    (acc * 0.5) as f32
}

fn area_centroid(poly: &[Vx]) -> V2 {
    let o = poly[0].p;
    let (mut ax, mut az, mut aa) = (0.0f64, 0.0f64, 0.0f64);
    for i in 1..poly.len().saturating_sub(1) {
        let (b, c) = (poly[i].p - o, poly[i + 1].p - o);
        let cr = f64::from(b.y) * f64::from(c.x) - f64::from(b.x) * f64::from(c.y);
        aa += cr;
        ax += cr * f64::from(b.x + c.x);
        az += cr * f64::from(b.y + c.y);
    }
    if abs(aa as f32) < 1e-6 {
        return o;
    }
    o + V2::new((ax / (3.0 * aa)) as f32, (az / (3.0 * aa)) as f32)
}

fn pip(p: V2, poly: &[Vx]) -> bool {
    let (px, pz) = (f64::from(p.x), f64::from(p.y));
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i].p, poly[(i + 1) % n].p);
        let (ax, az, bx, bz) = (
            f64::from(a.x),
            f64::from(a.y),
            f64::from(b.x),
            f64::from(b.y),
        );
        if (az > pz) != (bz > pz) {
            let x = ax + (pz - az) / (bz - az) * (bx - ax);
            if px < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Calls `f(i, j)` for the pairs of non-adjacent sides `i < j` of a closed polygon whose
/// bounds overlap (found by a sweep along x), until it returns `true`.
fn sweep_pairs(pts: &[V2], mut f: impl FnMut(usize, usize) -> bool) {
    let n = pts.len();
    let boxes: Vec<[f32; 4]> = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)]
        })
        .collect();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|&a, &b| {
        boxes[a as usize][0]
            .total_cmp(&boxes[b as usize][0])
            .then(a.cmp(&b))
    });
    for x in 0..n {
        let i = order[x] as usize;
        let bi = boxes[i];
        for &o in &order[x + 1..] {
            let j = o as usize;
            let bj = boxes[j];
            if bj[0] > bi[2] {
                break;
            }
            if bj[1] > bi[3] || bj[3] < bi[1] {
                continue;
            }
            let (lo, hi) = (i.min(j), i.max(j));
            if hi == lo + 1 || (lo == 0 && hi == n - 1) {
                continue;
            }
            if f(lo, hi) {
                return;
            }
        }
    }
}

/// `place::validate::is_simple_ccw` (the same predicate, in the same `f64` arithmetic),
/// with a sweep instead of testing every pair of sides.
fn simple_ccw(poly: &[Vx]) -> bool {
    type P = [f64; 2];
    let n = poly.len();
    if n < 3 || !poly.iter().all(|v| v.p.x.is_finite() && v.p.y.is_finite()) {
        return false;
    }
    // `place::signed_area`, term for term.
    let mut acc = 0.0f32;
    for (i, v) in poly.iter().enumerate() {
        let q = poly[(i + 1) % n].p;
        acc += v.p.y * q.x - v.p.x * q.y;
    }
    if acc * 0.5 <= 0.0 {
        return false;
    }
    let at = |i: usize| -> P {
        let p = poly[i % n].p;
        [f64::from(p.x), f64::from(p.y)]
    };
    if (0..n).any(|i| at(i) == at(i + 1)) {
        return false;
    }
    let touch = touch64;
    if n <= 48 {
        for i in 0..n {
            let (a, b) = (poly[i].p, poly[(i + 1) % n].p);
            for j in i + 2..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                let (c, d) = (poly[j].p, poly[(j + 1) % n].p);
                if c.x.max(d.x) < a.x.min(b.x)
                    || c.x.min(d.x) > a.x.max(b.x)
                    || c.y.max(d.y) < a.y.min(b.y)
                    || c.y.min(d.y) > a.y.max(b.y)
                {
                    continue;
                }
                if touch(at(i), at(i + 1), at(j), at(j + 1)) {
                    return false;
                }
            }
        }
        return true;
    }
    let pts: Vec<V2> = poly.iter().map(|x| x.p).collect();
    let mut bad = false;
    sweep_pairs(&pts, |i, j| {
        bad = touch(at(i), at(i + 1), at(j), at(j + 1));
        bad
    });
    !bad
}

/// [`simple_ccw`] for a polygon known to be simple except possibly at sides `from..n` and
/// side 0: only those are tested against the others.
fn simple_from(poly: &[Vx], from: usize) -> bool {
    let n = poly.len();
    let mut acc = 0.0f32;
    for (i, v) in poly.iter().enumerate() {
        let q = poly[(i + 1) % n].p;
        acc += v.p.y * q.x - v.p.x * q.y;
    }
    if n < 3 || acc * 0.5 <= 0.0 {
        return false;
    }
    let at = |i: usize| -> [f64; 2] {
        let p = poly[i % n].p;
        [f64::from(p.x), f64::from(p.y)]
    };
    let fresh = core::iter::once(0).chain(from.max(1)..n);
    for i in fresh {
        if at(i) == at(i + 1) {
            return false;
        }
        let (a, b) = (poly[i].p, poly[(i + 1) % n].p);
        for j in 0..n {
            let adjacent = j == i || j == (i + 1) % n || i == (j + 1) % n;
            if adjacent {
                continue;
            }
            let (c, d) = (poly[j].p, poly[(j + 1) % n].p);
            if c.x.max(d.x) < a.x.min(b.x)
                || c.x.min(d.x) > a.x.max(b.x)
                || c.y.max(d.y) < a.y.min(b.y)
                || c.y.min(d.y) > a.y.max(b.y)
            {
                continue;
            }
            if touch64(at(i), at(i + 1), at(j), at(j + 1)) {
                return false;
            }
        }
    }
    true
}

/// Whether the closed segments `p1 p2` and `q1 q2` share a point, as
/// `place::validate` decides it.
fn touch64(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> bool {
    type P = [f64; 2];
    fn orient(a: P, b: P, c: P) -> f64 {
        (b[1] - a[1]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[1] - a[1])
    }
    let (d1, d2) = (orient(q1, q2, p1), orient(q1, q2, p2));
    let (d3, d4) = (orient(p1, p2, q1), orient(p1, p2, q2));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on = |a: P, b: P, c: P, d: f64| {
        d == 0.0
            && c[0] >= a[0].min(b[0])
            && c[0] <= a[0].max(b[0])
            && c[1] >= a[1].min(b[1])
            && c[1] <= a[1].max(b[1])
    };
    on(q1, q2, p1, d1) || on(q1, q2, p2, d2) || on(p1, p2, q1, d3) || on(p1, p2, q2, d4)
}

/// Merges vertices closer than `EPS_M`; the side leaving a merged vertex is the later one.
fn dedup(poly: Vec<Vx>) -> Vec<Vx> {
    let mut out: Vec<Vx> = Vec::with_capacity(poly.len());
    for v in poly {
        if let Some(last) = out.last_mut() {
            let d = v.p - last.p;
            if d.dot(d) <= EPS_M * EPS_M {
                last.tag = v.tag;
                last.run = v.run;
                continue;
            }
        }
        out.push(v);
    }
    while out.len() > 1 && (out[out.len() - 1].p - out[0].p).length() <= EPS_M {
        out.pop();
    }
    out
}

fn frontage_len(poly: &[Vx]) -> f32 {
    let mut acc = 0.0;
    for (i, v) in poly.iter().enumerate() {
        if v.tag != NONE {
            acc += (poly[(i + 1) % poly.len()].p - v.p).length();
        }
    }
    acc
}

/// Narrower than `SLIVER_M` across or along its frontage, or smaller than `SLIVER_M2`.
fn is_sliver(poly: &[Vx], area: f32) -> bool {
    if area < SLIVER_M2 {
        return true;
    }
    // Box along the longest road side.
    let mut best = (0.0f32, V2::new(1.0, 0.0));
    for (i, v) in poly.iter().enumerate() {
        let d = poly[(i + 1) % poly.len()].p - v.p;
        let l = d.length();
        if v.tag != NONE && l > best.0 {
            best = (l, d * (1.0 / l));
        }
    }
    let (u, n) = (best.1, best.1.left());
    let (mut u0, mut u1, mut n0, mut n1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for v in poly {
        let (a, b) = (v.p.dot(u), v.p.dot(n));
        u0 = u0.min(a);
        u1 = u1.max(a);
        n0 = n0.min(b);
        n1 = n1.max(b);
    }
    (u1 - u0).min(n1 - n0) < SLIVER_M || area / (u1 - u0).max(n1 - n0) < SLIVER_M * 0.5
}

/// Assigns runs: sides along roads lots may face (`faces(edge)`) continuing within ~35°
/// of each other.
fn with_runs(mut poly: Vec<Vx>, faces: impl Fn(u32) -> bool) -> Vec<Vx> {
    let n = poly.len();
    let dir = |i: usize| (poly[(i + 1) % n].p - poly[i].p).normalize();
    let front: Vec<bool> = poly.iter().map(|v| v.tag != NONE && faces(v.tag)).collect();
    let mut brk = vec![false; n];
    for (i, b) in brk.iter_mut().enumerate() {
        let prev = (i + n - 1) % n;
        *b = !front[prev] || !front[i] || dir(prev).dot(dir(i)) < RUN_COS;
    }
    if !brk.iter().any(|&b| b) {
        brk[0] = true;
        brk[n / 2] = true;
    }
    let first = brk.iter().position(|&b| b).expect("a break");
    let mut run = 0u32;
    for k in 0..n {
        let i = (first + k) % n;
        if brk[i] && k > 0 {
            run += 1;
        }
        poly[i].run = if front[i] { run } else { NONE };
    }
    poly
}

/// A contiguous stretch of one run's sides on a polygon: sides `first..first + count`
/// (cyclic), `len` long.
struct Chain {
    first: usize,
    count: usize,
    len: f32,
}

/// The longest contiguous stretch of `run` on `poly`.
fn chain(poly: &[Vx], run: u32) -> Option<Chain> {
    let n = poly.len();
    let side = |i: usize| (poly[(i + 1) % n].p - poly[i].p).length();
    let start = (0..n).find(|&i| poly[i].run != run)?;
    let mut best: Option<Chain> = None;
    let mut k = 0;
    while k < n {
        let i = (start + k) % n;
        if poly[i].run != run {
            k += 1;
            continue;
        }
        let (first, mut count, mut len) = (i, 0, 0.0);
        while k < n && poly[(start + k) % n].run == run {
            len += side((start + k) % n);
            count += 1;
            k += 1;
        }
        if best.as_ref().is_none_or(|b| len > b.len) {
            best = Some(Chain { first, count, len });
        }
    }
    if best.is_none() && poly.iter().all(|v| v.run == run) {
        let len = (0..n).map(side).sum();
        return Some(Chain {
            first: 0,
            count: n,
            len,
        });
    }
    best
}

/// The point `s` along a chain, the unit direction of its side and the chain-relative
/// index of that side.
fn point_at(poly: &[Vx], ch: &Chain, s: f32) -> (V2, V2, usize) {
    let n = poly.len();
    let mut acc = 0.0;
    for k in 0..ch.count {
        let i = (ch.first + k) % n;
        let (a, b) = (poly[i].p, poly[(i + 1) % n].p);
        let l = (b - a).length();
        if acc + l >= s || k + 1 == ch.count {
            let t = if l > 0.0 {
                ((s - acc) / l).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return (a.lerp(b, t), (b - a).normalize(), k);
        }
        acc += l;
    }
    let a = poly[ch.first].p;
    (a, V2::new(1.0, 0.0), 0)
}

/// Distance from frontage point `f` along the inward normal `n` to the far side, and
/// whether the far side is a road (else an interior cut).
fn width_at(poly: &[Vx], f: V2, n: V2) -> Option<(f32, bool)> {
    let from = f + n * 0.02;
    if !pip(from, poly) {
        return None;
    }
    let to = from + n * 10_000.0;
    let mut best: Option<(f64, usize)> = None;
    let m = poly.len();
    for i in 0..m {
        if let Some((t, _)) = seg_x(from, to, poly[i].p, poly[(i + 1) % m].p) {
            if t > 0.0 && best.is_none_or(|b| t < b.0) {
                best = Some((t, i));
            }
        }
    }
    best.map(|(t, i)| (t as f32 * 10_000.0 + 0.02, poly[i].tag != NONE))
}

/// The cut path `d` inside a chain: the chain offset inward, extended along its end
/// tangents, with the offset of the chain's midpoint `f` (side `k`) as the start.
fn offset_path(
    poly: &[Vx],
    ch: &Chain,
    k: usize,
    f: V2,
    n: V2,
    d: f32,
) -> Option<(Vec<V2>, usize)> {
    let m = poly.len();
    let all: Vec<V2> = (0..=ch.count).map(|j| poly[(ch.first + j) % m].p).collect();
    // Vertices closer than 1 m to the last kept one are skipped: offsetting tiny sides
    // folds the path.
    let mut idx: Vec<usize> = vec![0];
    for j in 1..all.len() {
        if (all[j] - all[idx[idx.len() - 1]]).length() >= 1.0 {
            idx.push(j);
        }
    }
    let last = all.len() - 1;
    if idx[idx.len() - 1] != last {
        if idx.len() > 1 {
            *idx.last_mut().expect("index") = last;
        } else {
            idx.push(last);
        }
    }
    // The kept vertex that starts the stretch holding side `k`.
    let k = idx
        .iter()
        .rposition(|&j| j <= k)
        .unwrap_or(0)
        .min(idx.len() - 2);
    let pts: Vec<V2> = idx.iter().map(|&j| all[j]).collect();
    let dirs: Vec<V2> = pts.windows(2).map(|w| (w[1] - w[0]).normalize()).collect();
    let c = dirs.len();
    let ext = 4.0 * d + 200.0;
    let mut path: Vec<V2> = Vec::with_capacity(c + 4);
    path.push(pts[0] + dirs[0].left() * d - dirs[0] * ext);
    let mut start = 0;
    for j in 0..=c {
        let nrm = if j == 0 {
            dirs[0].left()
        } else if j == c {
            dirs[c - 1].left()
        } else {
            let b = (dirs[j - 1].left() + dirs[j].left()).normalize();
            let cos = b.dot(dirs[j].left()).max(0.5);
            b * (1.0 / cos)
        };
        path.push(pts[j] + nrm * d);
        if j == k {
            path.push(f + n * d);
            start = path.len() - 1;
        }
    }
    path.push(pts[c] + dirs[c - 1].left() * d + dirs[c - 1] * ext);
    pip(path[start], poly).then_some((path, start))
}

/// Splits a polygon along the part of `path` around `path[s]` (strictly inside) between
/// its first boundary crossings on either side. `None` when the path never leaves the
/// polygon, folds on itself, or a piece comes out degenerate.
fn cut(poly: &[Vx], path: &[V2], s: usize) -> Option<(Vec<Vx>, Vec<Vx>)> {
    let m = poly.len();
    let hit = |a: V2, b: V2, first: bool| -> Option<(f64, usize, f64)> {
        let mut best: Option<(f64, usize, f64)> = None;
        let (x0, x1, z0, z1) = (a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y));
        for i in 0..m {
            let (c, d) = (poly[i].p, poly[(i + 1) % m].p);
            if c.x.max(d.x) < x0 || c.x.min(d.x) > x1 || c.y.max(d.y) < z0 || c.y.min(d.y) > z1 {
                continue;
            }
            if let Some((t, u)) = seg_x(a, b, c, d) {
                if (!first || t > 1e-9) && best.is_none_or(|x| t < x.0) {
                    best = Some((t, i, u));
                }
            }
        }
        best
    };
    // Forward to B.
    let mut fwd: Option<(usize, usize, f64, V2)> = None;
    for k in s..path.len() - 1 {
        if let Some((t, i, u)) = hit(path[k], path[k + 1], k == s) {
            fwd = Some((k, i, u, path[k] + (path[k + 1] - path[k]) * t as f32));
            break;
        }
    }
    let mut bwd: Option<(usize, usize, f64, V2)> = None;
    for k in (1..=s).rev() {
        if let Some((t, i, u)) = hit(path[k], path[k - 1], k == s) {
            bwd = Some((k, i, u, path[k] + (path[k - 1] - path[k]) * t as f32));
            break;
        }
    }
    let ((kb, jb, ub, pb), (ka, ia, ua, pa)) = (fwd?, bwd?);
    // Chord A → interior → B.
    let mut chord: Vec<V2> = vec![pa];
    chord.extend_from_slice(&path[ka..=kb]);
    chord.push(pb);
    let mut clean: Vec<V2> = Vec::with_capacity(chord.len());
    for p in chord {
        if clean.last().is_none_or(|&q| (p - q).length() > EPS_M) {
            clean.push(p);
        }
    }
    let chord = clean;
    if chord.len() < 2 || (pa - pb).length() < 0.05 && chord.len() < 3 {
        return None;
    }
    for i in 0..chord.len() - 1 {
        if i + 2 < chord.len() {
            let (u, v) = (chord[i + 1] - chord[i], chord[i + 2] - chord[i + 1]);
            if u.dot(v) < -0.5 * u.length() * v.length() {
                return None; // A fold.
            }
        }
        for j in i + 2..chord.len() - 1 {
            if seg_x(chord[i], chord[i + 1], chord[j], chord[j + 1]).is_some() {
                return None;
            }
        }
    }
    let interior = |v: V2| Vx {
        p: v,
        tag: NONE,
        run: NONE,
    };
    // Boundary from A to B (forward), then the chord back.
    let walk = |from: usize, uf: f64, to: usize, ut: f64| -> Vec<usize> {
        // Vertex indices strictly between the point on side `from` and the point on side
        // `to`, walking forward.
        let mut v = Vec::new();
        if from == to && uf <= ut {
            return v;
        }
        let mut i = (from + 1) % m;
        loop {
            v.push(i);
            if i == to {
                break;
            }
            i = (i + 1) % m;
        }
        v
    };
    let mut p1: Vec<Vx> = vec![Vx {
        p: pa,
        tag: poly[ia].tag,
        run: poly[ia].run,
    }];
    for i in walk(ia, ua, jb, ub) {
        p1.push(poly[i]);
    }
    p1.push(interior(pb));
    for &q in chord[1..chord.len() - 1].iter().rev() {
        p1.push(interior(q));
    }
    let mut p2: Vec<Vx> = vec![Vx {
        p: pb,
        tag: poly[jb].tag,
        run: poly[jb].run,
    }];
    for i in walk(jb, ub, ia, ua) {
        p2.push(poly[i]);
    }
    p2.push(interior(pa));
    for &q in &chord[1..chord.len() - 1] {
        p2.push(interior(q));
    }
    let (p1, p2) = (dedup(p1), dedup(p2));
    if p1.len() < 3 || p2.len() < 3 || area(&p1) <= 0.5 || area(&p2) <= 0.5 {
        return None;
    }
    // The pieces of a simple polygon can only fail to be simple at the cut: check the
    // sides from the far end of the cut around to the first side.
    let fresh = |q: &[Vx], end: V2| match q.iter().position(|v| v.p == end) {
        Some(k) if k > 0 => simple_from(q, k - 1),
        _ => simple_ccw(q),
    };
    (fresh(&p1, pb) && fresh(&p2, pa)).then_some((p1, p2))
}

/// Squared distance from `p` to the segment `a b`, in `f64`.
fn dist2(p: V2, a: V2, b: V2) -> f64 {
    let (px, pz) = (f64::from(p.x), f64::from(p.y));
    let (ax, az) = (f64::from(a.x), f64::from(a.y));
    let (dx, dz) = (f64::from(b.x) - ax, f64::from(b.y) - az);
    let l2 = dx * dx + dz * dz;
    let t = if l2 > 0.0 {
        (((px - ax) * dx + (pz - az) * dz) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ez) = (ax + dx * t - px, az + dz * t - pz);
    ex * ex + ez * ez
}

/// Squared distance between a polyline and a polygon, as `place::validate` measures it
/// (0 when they touch or the line starts inside); parts of the line farther than `cull`
/// from the polygon's bounds are skipped (`f64::MAX` if that is all of it).
fn polyline_polygon_dist2(line: &[[f32; 3]], poly: &[Vx], cull: f32) -> f64 {
    let (mut x0, mut z0, mut x1, mut z1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for v in poly {
        x0 = x0.min(v.p.x);
        z0 = z0.min(v.p.y);
        x1 = x1.max(v.p.x);
        z1 = z1.max(v.p.y);
    }
    let (x0, z0, x1, z1) = (x0 - cull, z0 - cull, x1 + cull, z1 + cull);
    if let Some(&p) = line.first() {
        if pip(V2::new(p[0], p[2]), poly) {
            return 0.0;
        }
    }
    let n = poly.len();
    let mut best = f64::MAX;
    for w in line.windows(2) {
        let (a, b) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
        if a.x.max(b.x) < x0 || a.x.min(b.x) > x1 || a.y.max(b.y) < z0 || a.y.min(b.y) > z1 {
            continue;
        }
        for i in 0..n {
            let (c, d) = (poly[i].p, poly[(i + 1) % n].p);
            if seg_x(a, b, c, d).is_some() {
                return 0.0;
            }
            best = best
                .min(dist2(a, c, d))
                .min(dist2(b, c, d))
                .min(dist2(c, a, b))
                .min(dist2(d, a, b));
        }
    }
    best
}

// ---------------------------------------------------------------------------------------
// Inset.

/// A property line: through `p` along unit `d`, fronting road `tag`.
#[derive(Clone, Copy, Debug)]
struct Line {
    p: V2,
    d: V2,
    tag: u32,
}

fn meet(a: &Line, b: &Line) -> V2 {
    intersect_lines(a.p, a.d, b.p, b.d).unwrap_or_else(|| {
        let foot = a.p + a.d * (b.p - a.p).dot(a.d);
        (foot + b.p) * 0.5
    })
}

/// The blocks inside a face: its ring inset by `offs[i]` per side, cleaned as described
/// in the module docs.
fn inset(ring: &[V2], tags: &[u32], offs: &[f32]) -> Vec<Vec<Vx>> {
    let n = ring.len();
    let mut lines: Vec<Line> = Vec::with_capacity(n + 16);
    for i in 0..n {
        let prev = (i + n - 1) % n;
        let p = ring[i];
        let d0 = (p - ring[prev]).normalize();
        let d1 = (ring[(i + 1) % n] - p).normalize();
        let (o0, o1) = (offs[prev], offs[i]);
        let om = o0.max(o1);
        let c = d0.dot(d1);
        let a0 = p + d0.left() * o0;
        let a1 = p + d1.left() * o1;
        if c < -0.9999 {
            // Dead end: a flat cap beyond the end.
            lines.push(Line {
                p: p + d0 * om,
                d: d0.right(),
                tag: tags[prev],
            });
        } else {
            let m = intersect_lines(a0, d0, a1, d1);
            let ml = m.map_or(f32::INFINITY, |m| (m - p).length());
            if ml > MITER_LIMIT * om && c > 0.5 {
                if abs(o1 - o0) > 1e-3 {
                    // A narrower road continues as a wider one: step where the narrow
                    // property line clears the wide road's.
                    let s = sqrt(abs(o1 * o1 - o0 * o0));
                    let nrm = d1.left();
                    let (at, d, tag) = if o1 > o0 {
                        (p - d0 * s, nrm, tags[i])
                    } else {
                        (p + d1 * s, -nrm, tags[prev])
                    };
                    lines.push(Line { p: at, d, tag });
                }
            } else if let Some(m) = m.filter(|_| ml > MITER_LIMIT * om) {
                let b = (m - p) * (1.0 / ml);
                if d0.turn(d1) < 0.0 {
                    // Sharp reflex corner: cap the miter.
                    lines.push(Line {
                        p: p + b * om,
                        d: b.right(),
                        tag: tags[prev],
                    });
                } else {
                    // Acute tip: cut it where it is `MIN_TIP_M` wide (no road there).
                    let half_tan = sqrt((1.0 + c) / (1.0 - c)).max(1e-3);
                    let t = MIN_TIP_M / (2.0 * half_tan);
                    lines.push(Line {
                        p: p + b * (ml + t),
                        d: b.right(),
                        tag: NONE,
                    });
                }
            }
        }
        lines.push(Line {
            p: a1,
            d: d1,
            tag: tags[i],
        });
    }
    // Remove property lines that turned around (their side collapsed), one at a time.
    let mut verts: Vec<V2> = Vec::new();
    loop {
        let k = lines.len();
        if k < 3 {
            return Vec::new();
        }
        verts.clear();
        for i in 0..k {
            verts.push(meet(&lines[(i + k - 1) % k], &lines[i]));
        }
        let bad = (0..k).find(|&i| (verts[(i + 1) % k] - verts[i]).dot(lines[i].d) < 0.0);
        match bad {
            Some(i) => {
                lines.remove(i);
            }
            None => break,
        }
    }
    let poly: Vec<Vx> = verts
        .iter()
        .zip(&lines)
        .map(|(&p, l)| Vx {
            p,
            tag: l.tag,
            run: NONE,
        })
        .collect();
    let mut loops: Vec<Vec<Vx>> = Vec::new();
    split_loops(dedup(poly), &mut loops);
    loops.retain(|l| area(l) >= MIN_BLOCK_M2);
    if loops.is_empty() {
        return loops;
    }
    // Every block vertex must keep its distance from every road of the face.
    let boxes: Vec<[f32; 4]> = (0..n)
        .map(|j| {
            let (a, b) = (ring[j], ring[(j + 1) % n]);
            let o = offs[j];
            [
                a.x.min(b.x) - o,
                a.y.min(b.y) - o,
                a.x.max(b.x) + o,
                a.y.max(b.y) + o,
            ]
        })
        .collect();
    loops.retain(|l| {
        l.iter().all(|v| {
            let p = v.p;
            boxes.iter().enumerate().all(|(j, bx)| {
                if p.x < bx[0] || p.x > bx[2] || p.y < bx[1] || p.y > bx[3] {
                    return true;
                }
                let min = f64::from((offs[j] - SLACK_M).max(0.0));
                dist2(p, ring[j], ring[(j + 1) % n]) >= min * min
            })
        })
    });
    loops
}

/// Splits a self-intersecting polygon at its crossings into loops; keeps the
/// counter-clockwise ones.
fn split_loops(poly: Vec<Vx>, out: &mut Vec<Vec<Vx>>) {
    let mut stack = vec![poly];
    let mut budget = MAX_SPLITS;
    while let Some(p) = stack.pop() {
        let n = p.len();
        if n < 3 {
            continue;
        }
        let pts: Vec<V2> = p.iter().map(|v| v.p).collect();
        let mut found: Option<(usize, usize, V2)> = None;
        sweep_pairs(&pts, |i, j| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            match seg_x(a, b, pts[j], pts[(j + 1) % n]) {
                Some((t, _)) => {
                    found = Some((i, j, a + (b - a) * t as f32));
                    true
                }
                None => false,
            }
        });
        let Some((i, j, x)) = found else {
            if area(&p) > 0.0 {
                out.push(p);
            }
            continue;
        };
        if budget == 0 {
            continue;
        }
        budget -= 1;
        let mut l1 = vec![Vx { p: x, ..p[i] }];
        l1.extend_from_slice(&p[i + 1..=j]);
        let mut l2 = vec![Vx { p: x, ..p[j] }];
        l2.extend_from_slice(&p[j + 1..]);
        l2.extend_from_slice(&p[..=i]);
        stack.push(dedup(l2));
        stack.push(dedup(l1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use place::{Edge, Node};

    fn road(a: u32, b: u32, pa: [f32; 2], pb: [f32; 2], w: f32) -> Edge {
        Edge {
            a,
            b,
            class: RoadClass::Local,
            lanes_fwd: 1,
            lanes_back: 1,
            width_m: w,
            sidewalk: false,
            speed_kph: 40,
            oneway: false,
            profile: vec![[pa[0], 0.0, pa[1]], [pb[0], 0.0, pb[1]]],
        }
    }

    #[test]
    fn a_square_has_one_bounded_ccw_face() {
        let p = [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]];
        let g = RoadGraph {
            nodes: p
                .iter()
                .map(|&pos| Node {
                    pos,
                    elevation: 0.0,
                })
                .collect(),
            edges: (0..4)
                .map(|i| road(i, (i + 1) % 4, p[i as usize], p[(i as usize + 1) % 4], 10.0))
                .collect(),
        };
        let pos = g.nodes.iter().map(|n| V2::from_array(n.pos)).collect();
        let lines: Vec<PLine> = g
            .edges
            .iter()
            .map(|e| PLine {
                edge: 0,
                kind: LineKind::Road,
                a: e.a,
                b: e.b,
                pts: e.profile.iter().map(|p| V2::new(p[0], p[2])).collect(),
            })
            .collect();
        let pg = Planar::new(pos, &lines);
        let mut seen = vec![false; pg.pieces.len() * 2];
        let mut areas = Vec::new();
        for h0 in 0..seen.len() as u32 {
            if seen[h0 as usize] {
                continue;
            }
            let mut ring = Vec::new();
            let mut h = h0;
            loop {
                seen[h as usize] = true;
                ring.push(pg.pos[pg.origin(h) as usize]);
                h = pg.next(h);
                if h == h0 {
                    break;
                }
            }
            areas.push(ring_area(&ring));
        }
        areas.sort_by(|a, b| a.total_cmp(b));
        assert_eq!(areas, vec![-10_000.0, 10_000.0]);
    }

    #[test]
    fn inset_shrinks_and_caps_dead_ends() {
        // A 100 m square (CCW for `place::signed_area`) with a 40 m dead end poking in from
        // the middle of one side.
        let mut ring = vec![
            V2::new(0.0, 0.0),
            V2::new(0.0, 100.0),
            V2::new(100.0, 100.0),
        ];
        ring.extend([
            V2::new(100.0, 50.0),
            V2::new(60.0, 50.0),
            V2::new(100.0, 50.0),
        ]);
        ring.push(V2::new(100.0, 0.0));
        assert!(ring_area(&ring) > 0.0, "{}", ring_area(&ring));
        let tags: Vec<u32> = (0..ring.len() as u32).collect();
        let offs = vec![5.0; ring.len()];
        let blocks = inset(&ring, &tags, &offs);
        assert_eq!(blocks.len(), 1);
        let b = &blocks[0];
        let a = area(b);
        // 90 × 90 minus the 10 m wide, 40 m long notch (capped 5 m beyond the end).
        assert!((a - (8100.0 - 10.0 * 40.0)).abs() < 1.0, "area {a}");
        let poly: Vec<[f32; 2]> = b.iter().map(|v| [v.p.x, v.p.y]).collect();
        assert!(place::validate::is_simple_ccw(&poly));
    }

    #[test]
    fn cuts_partition_a_rectangle() {
        let r = [[0.0, 0.0], [0.0, 60.0], [100.0, 60.0], [100.0, 0.0]];
        let poly: Vec<Vx> = r
            .iter()
            .enumerate()
            .map(|(i, &p)| Vx {
                p: V2::from_array(p),
                tag: i as u32,
                run: NONE,
            })
            .collect();
        assert!(area(&poly) > 0.0);
        let path = [
            V2::new(-10.0, 30.0),
            V2::new(50.0, 30.0),
            V2::new(110.0, 30.0),
        ];
        let (a, b) = cut(&poly, &path, 1).expect("cut");
        assert!((area(&a) - 3000.0).abs() < 0.01 && (area(&b) - 3000.0).abs() < 0.01);
        assert_eq!(frontage_len(&a) + frontage_len(&b), 320.0);
    }
}
