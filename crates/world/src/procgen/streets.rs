//! Stage 4 of `docs/WORLD_GENERATION.md`: local streets, generated per 128 m chunk and
//! identical on both sides of every chunk border.
//!
//! [`StreetPlan::new`] does the cheap whole-place part once (like the macro stages): the
//! graded terrain, an 8 m raster of the land between the arterials (cells of one street
//! pattern that no arterial separates form a *component*, the "arterial cell" a pattern is
//! laid in), each component's dominant arterial direction and block size, a distance field
//! to the arterials, a segment index over them, where the suburb pods go, and the hill
//! roads (few and long: each is traced once from its canonical start). [`StreetPlan::chunk`]
//! then lays the zone patterns into one chunk:
//! - **Grid** (`Downtown`/`Strip`/`Residential`/`Commercial`) and **Industrial**: a lattice
//!   per component, lines parallel and perpendicular to its dominant arterial direction at
//!   the template's block size (each family anchored on the bounding arterial parallel to
//!   it, so the blocks along it are whole), cut where they leave the component, meet water
//!   or steep ground, or run along an arterial, and joined to the arterials they cross;
//!   one cross segment in ten is dropped (a T), dead ends overhanging a cross street by
//!   less than 30 m are trimmed, alleys run down the middle of blocks in zones with
//!   `alleys`.
//! - **Curvy** (`Suburb`): pods placed greedily at the deepest points of each component, a
//!   stadium-shaped collector loop (ends of 42–95 m radius, sides bowed, sampled every 8 m)
//!   as wide as the component allows, or a gently waving spine street where it is too
//!   narrow; cul-de-sacs (12 m bulbs) inward and outward, connectors to the nearest
//!   arterial and links to nearby pods. Pods with no way to an arterial are dropped.
//! - **Switchback** (`Hills`): a road every 300 m along the arterials with hills within
//!   120 m, traced along the contours (≤ 11% per 8 m step, turning up to 30°, sharper or
//!   with a hairpin where boxed in), one uphill branch from the middle of each long one,
//!   all ending in bulbs. Traces ignore each other, so loops are rare.
//! - **Sections** (`Farmland`): a 400 m section lattice aligned with the farm arterials,
//!   kept only where no arterial already runs parallel within 150 m, plus dirt driveways
//!   (`Alley`, 4 m) to one farmstead per 200 m quarter cell.
//! - **Beachfront**: U-shaped parking aisles (`Alley`) off the beachfront road every 150 m
//!   and a pier road ending in a bulb at the pier.
//!
//! Every street gets a vertical profile when it is made: the graded terrain filtered to a
//! 11.8% grade, rejected where that needs more than 5 m of cut or fill.
//!
//! **Border stability.** No street is ever drawn from a chunk's own random stream. Every
//! street is a *feature*: a polyline (with its profile) that is a pure function of a
//! canonical key of the thing that owns it (a lattice line of a component, a pod, the
//! arterial edge and 300 m slot a hill road starts from, a farm quarter cell, an aisle
//! slot), computed in full by every chunk it touches and then clipped to the chunk. A
//! feature's border crossing is therefore computed from the same inputs on both sides;
//! its node carries the border's canonical key `hash64(seed, [STREETS, min(cx), min(cz),
//! axis, index])` with `index` naming the feature and its segment, and its height
//! interpolates the feature's own profile. Decisions that need neighbors (T drops, alley
//! ends, dead-end trims, pod anchoring) look at the neighboring features' global
//! definitions, never at another chunk's output. Per-chunk post-processing (crossings,
//! T-junctions, the 6 m merge, stub removal, height relaxation) never moves or removes a
//! border node or the node next to one.
//!
//! [`streets_all`] merges every chunk with the arterials into one graph: arterial edges
//! are split where streets join them and at chunk borders, border nodes are de-duplicated
//! by key.

use alloc::vec;
use alloc::vec::Vec;
use core::f32::consts::FRAC_1_SQRT_2;

use place::{Edge, Node, RoadClass, RoadGraph, ZoneId, SIDEWALK_WIDTH_M};
use skywave_core::{hash64, hash_bytes, Seed};

use super::arterials::{
    coast_tangent, grade_limit, major_of, sample_nodes, seg_intersect, tensor, HIGHWAY_WIDTH_M,
};
use super::macro_map::{MacroMap, MacroPoiKind};
use super::zones::{chamfer, ALL_ZONES};
use super::{stage, CELL_M, CHUNK_M};
use crate::math::{abs, clamp, floor, lerp, segment_distance, sin_cos, PI, TAU, V2};
use crate::template::{RegionTemplate, StreetPattern};

/// Curb-to-curb width of a local street.
pub const LOCAL_WIDTH_M: f32 = 10.0;
/// Width of an industrial (truck) street.
pub const INDUSTRIAL_WIDTH_M: f32 = 12.0;
/// Width of an alley or parking aisle.
pub const ALLEY_WIDTH_M: f32 = 6.0;
/// Width of a dirt farm driveway.
pub const DRIVEWAY_WIDTH_M: f32 = 4.0;
/// Width of a section-line road.
pub const SECTION_WIDTH_M: f32 = 8.0;
pub const LOCAL_KPH: u8 = 40;
pub const ALLEY_KPH: u8 = 20;
pub const SECTION_KPH: u8 = 60;
/// Radius of a cul-de-sac bulb (centerline).
pub const BULB_RADIUS_M: f32 = 12.0;
/// Street nodes closer than this merge.
pub const MERGE_M: f32 = 6.0;
/// Dead ends shorter than this are removed (cul-de-sacs are never dead ends).
pub const STUB_M: f32 = 30.0;
/// Steepest street profile segment.
pub const MAX_GRADE: f32 = 0.12;
/// Largest spacing of profile samples on straight streets.
pub const PROFILE_STEP_M: f32 = 16.0;
/// Sample spacing of curved streets.
pub const SPLINE_STEP_M: f32 = 8.0;

/// Raster cell of the component map.
const RASTER_M: f32 = 8.0;
/// Sampling step along lattice lines.
const SAMPLE_M: f32 = 4.0;
/// Streets keep this far from the playable rectangle's edge.
const MARGIN_M: f32 = 8.0;
/// Terrain steeper than this over 16 m breaks a lattice line.
const STEEP_GRADE: f32 = 0.15;
/// The profile filter's slope, a hair under [`MAX_GRADE`] for rounding.
const FILTER_GRADE: f32 = 0.118;
/// How far profiles may leave the graded terrain; `place::validate` allows 8 m.
const PROFILE_BAND_M: f32 = 7.8;
/// Deepest cut or highest fill a street profile may need; steeper ground gets no street.
const CUT_FILL_M: f32 = 5.0;
/// Streets crossing with profiles further apart than this do not meet.
const CROSS_STEP_M: f32 = 1.5;
/// Grade node heights are relaxed to where streets meet (between the filter and the max).
const RELAX_GRADE: f32 = 0.119;
/// A run of a lattice line reaches an arterial it ends this close to.
const SNAP_REACH_M: f32 = 12.0;
/// Gap a lattice line keeps from an arterial it may not join (highway, ramps, decks).
const HARD_GAP_M: f32 = 16.0;
/// Streets join an arterial only where its deck is this close to the terrain.
const JOIN_STEP_M: f32 = 1.5;
/// Share of lattice cross segments dropped to make a T.
const T_CHANCE: f32 = 0.1;
/// Lattice runs stay within 36 m of their component's cells and alleys within 70 m: a
/// chunk lays the lattices of the components this close.
const LATTICE_REACH_M: f32 = 80.0;
/// A run's feature reaches this far past the run's ends (onto the centerline of the road it
/// snaps to), so a chunk takes the runs that end this close to it.
const RUN_REACH_M: f32 = 20.0;
/// Most consecutive lattice samples on arterial cells a run may cross.
const MAX_BARRIER_SAMPLES: usize = 6;
/// Suburb pod loops: half-width range of the stadium, room kept to the component's edge,
/// gap between pods and how far a pod stretches along its band.
const POD_MIN_B_M: f32 = 42.0;
const POD_MAX_B_M: f32 = 95.0;
const POD_EDGE_M: f32 = 18.0;
const POD_GAP_M: f32 = 60.0;
const POD_MAX_STRETCH_M: f32 = 160.0;
/// Suburb spines (bands too narrow for a loop): least depth at the center, reach of their
/// cul-de-sacs, longest half-length, least length, amplitude of the wave.
const SPINE_MIN_DEPTH_M: f32 = 28.0;
const SPINE_HALF_M: f32 = 40.0;
const SPINE_MAX_HALF_M: f32 = 220.0;
const SPINE_MIN_LENGTH_M: f32 = 100.0;
const SPINE_WAVE_M: f32 = 6.0;
/// Longest connector from a pod loop (or a driveway, or the pier road) to an arterial.
const CONNECT_M: f32 = 220.0;
/// Longest link between two neighboring pod loops.
const LINK_M: f32 = 120.0;
/// Pod loops and hill roads keep this far from arterial centerlines.
const CLEAR_M: f32 = 24.0;
/// Section lattice spacing (a quarter mile) and how near a parallel arterial replaces a line.
const SECTION_M: f32 = 400.0;
const SECTION_PARALLEL_M: f32 = 150.0;
/// One farmstead per quarter of a section cell.
const FARM_M: f32 = 200.0;
/// Spacing of hill-road starts along the arterials.
const TRACE_SPACING_M: f32 = 300.0;
/// Trace step and steering.
const TRACE_STEP_M: f32 = 8.0;
const TRACE_GRADE: f32 = 0.11;
const TRACE_CLIMB: f32 = 0.05;
/// Most consecutive trace steps outside the hills (past the approach).
const TRACE_OUTSIDE: usize = 5;
/// Parking aisle slots along the beachfront road.
const AISLE_SPACING_M: f32 = 150.0;
const AISLE_LENGTH_M: f32 = 60.0;
const AISLE_DEPTH_M: f32 = 18.0;
const NONE: u32 = u32::MAX;
/// The lattice id of the section lattice (component lattices use their component index).
const SECTIONS_ID: u32 = u32::MAX - 1;

/// Second words of the feature keys (the first is `stage::STREETS`).
mod key {
    pub const COMP: u64 = 1;
    pub const LINE: u64 = 2;
    pub const DROP: u64 = 3;
    pub const ALLEY: u64 = 4;
    pub const POD: u64 = 5;
    pub const TRACE: u64 = 6;
    pub const FARM: u64 = 7;
    pub const AISLE: u64 = 8;
    pub const PIER: u64 = 9;
    pub const SECTIONS: u64 = 10;
    pub const BULB: u64 = 11;
    pub const PART: u64 = 12;
    pub const BRANCH: u64 = 13;
}

/// Street patterns as laid here (`Park`, `Beach`, `Highway` and `Water` get none).
const P_NONE: u8 = 0;
const P_GRID: u8 = 1;
const P_CURVY: u8 = 2;
const P_SWITCH: u8 = 3;
const P_SECTIONS: u8 = 4;
const P_INDUSTRIAL: u8 = 5;

/// `(cos, sin)` of ±45°, ±60°, ±75°, ±90°: sharper turns where the gentle ones fail.
const WIDE: [(f32, f32); 8] = [
    (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (0.5, 0.866_025),
    (0.5, -0.866_025),
    (0.258_819, 0.965_926),
    (0.258_819, -0.965_926),
    (0.0, 1.0),
    (0.0, -1.0),
];

/// `(cos, sin)` of 0°, ±10°, ±20°, ±30°, in steering preference order.
const STEER: [(f32, f32); 7] = [
    (1.0, 0.0),
    (0.984_808, 0.173_648),
    (0.984_808, -0.173_648),
    (0.939_693, 0.342_02),
    (0.939_693, -0.342_02),
    (0.866_025, 0.5),
    (0.866_025, -0.5),
];

/// What a street node of a chunk is attached to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeRef {
    /// Inside the chunk and owned by it.
    Inner,
    /// On a chunk border; the neighbor's output has a node with the same key and position.
    Border { key: u64 },
    /// On arterial edge `edge`, `u` of the way along its profile segment `seg` (a true
    /// intersection: [`streets_all`] splits the arterial there).
    Arterial { edge: u32, seg: u32, u: f32 },
    /// At node `node` of the arterial graph.
    ArterialNode { node: u32 },
}

/// The local streets of one chunk: nodes (with what they attach to) and edges whose
/// profiles run between them. Edges are `Local`, `Alley` or (section lines) `Rural`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreetChunk {
    pub chunk: [i32; 2],
    pub nodes: Vec<Node>,
    /// Parallel to `nodes`.
    pub refs: Vec<NodeRef>,
    pub edges: Vec<Edge>,
    /// Farmstead points in this chunk (the ends of the farm driveways).
    pub farmsteads: Vec<[f32; 2]>,
}

/// A grid or industrial lattice, for inspection and tests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridCell {
    pub comp: u32,
    /// The component's most common zone, which set the block size.
    pub zone: ZoneId,
    pub industrial: bool,
    /// Unit direction of the lines parallel to the dominant arterial.
    pub dir: [f32; 2],
    pub origin: [f32; 2],
    /// Block `[length along dir, depth across]`: the spacing of the cross lines and of the
    /// parallel lines.
    pub block_m: [f32; 2],
}

/// Generates the local streets of `chunk`. Builds a [`StreetPlan`] first; when generating
/// many chunks, build the plan once and call [`StreetPlan::chunk`].
pub fn streets(
    seed: Seed,
    template: &RegionTemplate,
    macro_map: &MacroMap,
    arterials: &RoadGraph,
    chunk: [i32; 2],
) -> StreetChunk {
    StreetPlan::new(seed, template, macro_map, arterials).chunk(chunk)
}

/// Every chunk's local streets merged with the arterials into one graph; see
/// [`StreetPlan::merged`].
pub fn streets_all(
    seed: Seed,
    template: &RegionTemplate,
    macro_map: &MacroMap,
    arterials: &RoadGraph,
) -> RoadGraph {
    StreetPlan::new(seed, template, macro_map, arterials).merged()
}

/// Content hash of a chunk's streets (every field, in order), for determinism tests.
pub fn hash_chunk(c: &StreetChunk) -> u64 {
    let mut b: Vec<u8> = Vec::new();
    let f = |b: &mut Vec<u8>, x: f32| b.extend_from_slice(&x.to_bits().to_le_bytes());
    b.extend_from_slice(&c.chunk[0].to_le_bytes());
    b.extend_from_slice(&c.chunk[1].to_le_bytes());
    for (n, r) in c.nodes.iter().zip(&c.refs) {
        f(&mut b, n.pos[0]);
        f(&mut b, n.pos[1]);
        f(&mut b, n.elevation);
        match *r {
            NodeRef::Inner => b.push(0),
            NodeRef::Border { key } => {
                b.push(1);
                b.extend_from_slice(&key.to_le_bytes());
            }
            NodeRef::Arterial { edge, seg, u } => {
                b.push(2);
                b.extend_from_slice(&edge.to_le_bytes());
                b.extend_from_slice(&seg.to_le_bytes());
                f(&mut b, u);
            }
            NodeRef::ArterialNode { node } => {
                b.push(3);
                b.extend_from_slice(&node.to_le_bytes());
            }
        }
    }
    for e in &c.edges {
        b.extend_from_slice(&e.a.to_le_bytes());
        b.extend_from_slice(&e.b.to_le_bytes());
        b.extend_from_slice(&[e.class as u8, e.lanes_fwd, e.lanes_back, e.sidewalk as u8]);
        b.extend_from_slice(&[e.speed_kph, e.oneway as u8]);
        f(&mut b, e.width_m);
        for p in &e.profile {
            f(&mut b, p[0]);
            f(&mut b, p[1]);
            f(&mut b, p[2]);
        }
    }
    for p in &c.farmsteads {
        f(&mut b, p[0]);
        f(&mut b, p[1]);
    }
    hash_bytes(0x5354_5245_4554, &b)
}

// ---------------------------------------------------------------------------------------
// Small helpers.

fn ceil(x: f32) -> f32 {
    -floor(-x)
}

/// A signed chunk or lattice index as a hash word.
fn word(x: i32) -> u64 {
    x as i64 as u64
}

/// Uniform `[0, 1)` from a key and a salt.
fn unit(h: u64, salt: u64) -> f32 {
    (hash64(h, &[salt]) >> 40) as f32 * (1.0 / 16_777_216.0)
}

/// Points from `a` to `b` (both included) at most `step` apart, evenly spread.
fn densify(a: V2, b: V2, step: f32) -> Vec<V2> {
    let len = (b - a).length();
    let n = (ceil(len / step) as usize).max(1);
    let mut out = Vec::with_capacity(n + 1);
    out.push(a);
    for k in 1..n {
        out.push(a.lerp(b, k as f32 / n as f32));
    }
    out.push(b);
    out
}

/// A polyline through `corners`, densified per segment.
fn densify_path(corners: &[V2], step: f32) -> Vec<V2> {
    let mut out: Vec<V2> = Vec::new();
    for w in corners.windows(2) {
        let seg = densify(w[0], w[1], step);
        let skip = usize::from(!out.is_empty());
        out.extend_from_slice(&seg[skip..]);
    }
    out
}

fn polyline_length(pts: &[V2]) -> f32 {
    let mut acc = 0.0;
    for w in pts.windows(2) {
        acc += (w[1] - w[0]).length();
    }
    acc
}

fn rotate(d: V2, c: f32, s: f32) -> V2 {
    // Positive `s` turns toward `d.left()`.
    d * c + d.left() * s
}

/// The chunk's box: `[x0, x1) × [z0, z1)` owned, closed for clipping.
#[derive(Clone, Copy, Debug)]
struct Bx {
    x0: f32,
    z0: f32,
    x1: f32,
    z1: f32,
}

impl Bx {
    fn of(c: [i32; 2]) -> Bx {
        Bx {
            x0: c[0] as f32 * CHUNK_M,
            z0: c[1] as f32 * CHUNK_M,
            x1: (c[0] + 1) as f32 * CHUNK_M,
            z1: (c[1] + 1) as f32 * CHUNK_M,
        }
    }

    fn grow(&self, r: f32) -> Bx {
        Bx {
            x0: self.x0 - r,
            z0: self.z0 - r,
            x1: self.x1 + r,
            z1: self.z1 + r,
        }
    }

    fn owns(&self, p: V2) -> bool {
        p.x >= self.x0 && p.x < self.x1 && p.y >= self.z0 && p.y < self.z1
    }

    fn hits(&self, lo: V2, hi: V2) -> bool {
        hi.x >= self.x0 && lo.x <= self.x1 && hi.y >= self.z0 && lo.y <= self.z1
    }

    fn corners(&self) -> [V2; 4] {
        [
            V2::new(self.x0, self.z0),
            V2::new(self.x1, self.z0),
            V2::new(self.x0, self.z1),
            V2::new(self.x1, self.z1),
        ]
    }

    /// Which border `p` lies exactly on: 1 `x0`, 2 `x1`, 3 `z0`, 4 `z1`, else 0.
    fn code_of(&self, p: V2) -> u8 {
        if p.x == self.x0 {
            1
        } else if p.x == self.x1 {
            2
        } else if p.y == self.z0 {
            3
        } else if p.y == self.z1 {
            4
        } else {
            0
        }
    }
}

fn bbox(pts: &[V2]) -> (V2, V2) {
    let mut lo = V2::new(f32::INFINITY, f32::INFINITY);
    let mut hi = V2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in pts {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

/// The parameter range of the line `base + t dir` inside the closed box.
fn clip_line(base: V2, dir: V2, b: &Bx) -> Option<(f32, f32)> {
    let mut t0 = f32::NEG_INFINITY;
    let mut t1 = f32::INFINITY;
    for (o, d, lo, hi) in [(base.x, dir.x, b.x0, b.x1), (base.y, dir.y, b.z0, b.z1)] {
        if abs(d) < 1e-9 {
            if o < lo || o > hi {
                return None;
            }
        } else {
            let a = (lo - o) / d;
            let c = (hi - o) / d;
            t0 = t0.max(a.min(c));
            t1 = t1.min(a.max(c));
        }
    }
    (t1 > t0).then_some((t0, t1))
}

// ---------------------------------------------------------------------------------------
// Plan.

/// A point on an arterial edge a street joins.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Snap {
    edge: u32,
    seg: u32,
    u: f32,
    pos: V2,
    y: f32,
    /// The arterial node the point merged into, or `NONE`.
    node: u32,
}

impl Snap {
    fn same(&self, o: &Snap) -> bool {
        if self.node != NONE || o.node != NONE {
            return self.node == o.node;
        }
        self.edge == o.edge && self.seg == o.seg && self.u.to_bits() == o.u.to_bits()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    /// A dead end, or a point on another street found by geometry.
    Free,
    Snap(Snap),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Local,
    Industrial,
    Alley,
    Driveway,
    Section,
}

/// A street before clipping: a pure function of its key. Vertices are at most 16 m apart;
/// `ys` is its road surface height per vertex (grade-limited, see [`StreetPlan::make`]).
#[derive(Clone)]
struct Feature {
    key: u64,
    kind: Kind,
    pts: Vec<V2>,
    ys: Vec<f32>,
    ends: [End; 2],
}

#[derive(Clone, Copy, Debug)]
struct Lattice {
    origin: V2,
    d: V2,
    n: V2,
    /// Spacing of the cross lines (along `d`) and of the parallel lines (along `n`).
    len: f32,
    dep: f32,
    key: u64,
    comp: u32,
    pattern: u8,
    kind: Kind,
    /// Runs of family 0 / 1 lines this close to a parallel arterial are dropped.
    dpar: [f32; 2],
}

struct Comp {
    pattern: u8,
    zone: ZoneId,
    dir: V2,
    lat: Option<Lattice>,
}

/// Arterial profile segments bucketed on a uniform grid (CSR layout).
struct SegIndex {
    cell: f32,
    w: usize,
    h: usize,
    start: Vec<u32>,
    items: Vec<[u32; 2]>,
}

impl SegIndex {
    fn new(g: &RoadGraph, ext: [f32; 2], cell: f32) -> SegIndex {
        let w = (ext[0] / cell) as usize + 1;
        let h = (ext[1] / cell) as usize + 1;
        let mut idx = SegIndex {
            cell,
            w,
            h,
            start: vec![0; w * h + 1],
            items: Vec::new(),
        };
        let mut count = vec![0u32; w * h];
        for pass in 0..2 {
            let mut cursor: Vec<u32> = if pass == 1 {
                idx.start[..w * h].to_vec()
            } else {
                Vec::new()
            };
            for (e, edge) in g.edges.iter().enumerate() {
                for (s, p) in edge.profile.windows(2).enumerate() {
                    let (i0, i1, j0, j1) = idx.range(
                        p[0][0].min(p[1][0]),
                        p[0][2].min(p[1][2]),
                        p[0][0].max(p[1][0]),
                        p[0][2].max(p[1][2]),
                    );
                    for j in j0..=j1 {
                        for i in i0..=i1 {
                            let c = j * w + i;
                            if pass == 0 {
                                count[c] += 1;
                            } else {
                                idx.items[cursor[c] as usize] = [e as u32, s as u32];
                                cursor[c] += 1;
                            }
                        }
                    }
                }
            }
            if pass == 0 {
                let mut acc = 0u32;
                for (start, &n) in idx.start.iter_mut().zip(&count) {
                    *start = acc;
                    acc += n;
                }
                idx.start[w * h] = acc;
                idx.items = vec![[0, 0]; acc as usize];
            }
        }
        idx
    }

    fn range(&self, x0: f32, z0: f32, x1: f32, z1: f32) -> (usize, usize, usize, usize) {
        let ci = |x: f32, n: usize| clamp(floor(x / self.cell), 0.0, (n - 1) as f32) as usize;
        (
            ci(x0, self.w),
            ci(x1, self.w),
            ci(z0, self.h),
            ci(z1, self.h),
        )
    }

    fn cell(&self, i: usize, j: usize) -> &[[u32; 2]] {
        let c = j * self.w + i;
        &self.items[self.start[c] as usize..self.start[c + 1] as usize]
    }
}

/// The whole-place part of stage 4, computed once; [`StreetPlan::chunk`] generates chunks
/// from it. See the module docs.
pub struct StreetPlan<'a> {
    seed: Seed,
    t: &'a RegionTemplate,
    m: &'a MacroMap,
    art: &'a RoadGraph,
    graded: Vec<f32>,
    ext: [f32; 2],
    rw: usize,
    rh: usize,
    /// Component per raster cell, `NONE` on arterials, water and pattern-less zones.
    label: Vec<u32>,
    /// Approximate distance from each raster cell to the nearest arterial centerline.
    adist: Vec<f32>,
    /// Street pattern by `ZoneId` discriminant.
    pat: [u8; 12],
    alley_zone: [bool; 12],
    comps: Vec<Comp>,
    pods: Vec<PodSpec>,
    /// Every hill road with its bounding box.
    traces: Vec<((V2, V2), Feature)>,
    /// Bounds of the `Sections` (farmland) cells.
    farm_box: Option<(V2, V2)>,
    sections: Option<Lattice>,
    index: SegIndex,
    /// Per arterial edge: whether local streets may join it.
    joinable: Vec<bool>,
    ebox: Vec<(V2, V2)>,
}

impl<'a> StreetPlan<'a> {
    /// The plan for a place: `macro_map` and its `arterials` (from
    /// [`super::arterials()`]) under `template`.
    pub fn new(
        seed: Seed,
        template: &'a RegionTemplate,
        macro_map: &'a MacroMap,
        arterials: &'a RoadGraph,
    ) -> StreetPlan<'a> {
        let m = macro_map;
        // The macro map carries the highway corridor's cut and fill: its heights are the
        // Place's heights.
        let graded = m.heights.clone();
        let (w, h) = (m.width as usize, m.height as usize);
        let ext = [(w - 1) as f32 * CELL_M, (h - 1) as f32 * CELL_M];
        let rw = (ext[0] / RASTER_M) as usize + 1;
        let rh = (ext[1] / RASTER_M) as usize + 1;
        let mut pat = [P_NONE; 12];
        let mut alley_zone = [false; 12];
        for z in ALL_ZONES {
            let Some(rule) = template.zones.get(z) else {
                continue;
            };
            alley_zone[z as usize] = rule.alleys;
            pat[z as usize] = match z {
                ZoneId::Park | ZoneId::Beach | ZoneId::Highway | ZoneId::Water => P_NONE,
                _ => match rule.streets {
                    StreetPattern::Grid => P_GRID,
                    StreetPattern::Curvy => P_CURVY,
                    StreetPattern::Switchback => P_SWITCH,
                    StreetPattern::Sections => P_SECTIONS,
                    StreetPattern::Industrial => P_INDUSTRIAL,
                    StreetPattern::Beachfront => P_NONE,
                },
            };
        }
        let joinable: Vec<bool> = arterials
            .edges
            .iter()
            .map(|e| e.class != RoadClass::Highway && !e.oneway)
            .collect();
        let ebox: Vec<(V2, V2)> = arterials
            .edges
            .iter()
            .map(|e| {
                let pts: Vec<V2> = e.profile.iter().map(|p| V2::new(p[0], p[2])).collect();
                bbox(&pts)
            })
            .collect();
        let index = SegIndex::new(arterials, ext, 64.0);
        let mut plan = StreetPlan {
            seed,
            t: template,
            m,
            art: arterials,
            graded,
            ext,
            rw,
            rh,
            label: Vec::new(),
            adist: Vec::new(),
            pat,
            alley_zone,
            comps: Vec::new(),
            pods: Vec::new(),
            traces: Vec::new(),
            farm_box: None,
            sections: None,
            index,
            joinable,
            ebox,
        };
        plan.raster();
        plan.place_pods();
        plan.sections = plan.section_lattice();
        plan.trace_all();
        plan
    }

    /// Rasterizes the arterials, labels the components and sets up their lattices.
    fn raster(&mut self) {
        let (rw, rh) = (self.rw, self.rh);
        let n = rw * rh;
        let wl = self.m.water_level;
        // 0 open, 1 water or outside, 2 arterial.
        let mut barrier = vec![0u8; n];
        let mut cpat = vec![P_NONE; n];
        let mut czone = vec![ZoneId::Water; n];
        for j in 0..rh {
            for i in 0..rw {
                let c = V2::new((i as f32 + 0.5) * RASTER_M, (j as f32 + 0.5) * RASTER_M);
                let zone = self.m.zone_at(c.x, c.y);
                czone[j * rw + i] = zone;
                cpat[j * rw + i] = self.pat[zone as usize];
                if !self.inside(c, MARGIN_M) || zone == ZoneId::Water || self.hgt(c) <= wl + 0.5 {
                    barrier[j * rw + i] = 1;
                }
            }
        }
        let mut tens = vec![[0.0f32; 2]; n];
        for e in &self.art.edges {
            for p in e.profile.windows(2) {
                let (a, b) = (V2::new(p[0][0], p[0][2]), V2::new(p[1][0], p[1][2]));
                let tt = tensor((b - a).normalize());
                let steps = ((b - a).length() / 2.0) as usize + 1;
                let mut prev: Option<(usize, usize)> = None;
                for s in 0..=steps {
                    let q = a.lerp(b, s as f32 / steps as f32);
                    let ci = clamp(floor(q.x / RASTER_M), 0.0, (rw - 1) as f32) as usize;
                    let cj = clamp(floor(q.y / RASTER_M), 0.0, (rh - 1) as f32) as usize;
                    let mut mark = |i: usize, j: usize| {
                        let c = j * rw + i;
                        barrier[c] = 2;
                        tens[c] = tt;
                    };
                    if let Some((pi, pj)) = prev {
                        if pi != ci && pj != cj {
                            mark(ci, pj);
                        }
                    }
                    mark(ci, cj);
                    prev = Some((ci, cj));
                }
            }
        }
        let farm: Vec<V2> = (0..n)
            .filter(|&c| cpat[c] == P_SECTIONS)
            .flat_map(|c| {
                let (i, j) = ((c % rw) as f32, (c / rw) as f32);
                [
                    V2::new(i * RASTER_M, j * RASTER_M),
                    V2::new((i + 1.0) * RASTER_M, (j + 1.0) * RASTER_M),
                ]
            })
            .collect();
        self.farm_box = (!farm.is_empty()).then(|| bbox(&farm));
        let mut adist: Vec<f32> = barrier
            .iter()
            .map(|&b| if b == 2 { 0.0 } else { f32::INFINITY })
            .collect();
        chamfer(&mut adist, rw, rh, RASTER_M);
        self.adist = adist;

        let mut label = vec![NONE; n];
        let mut stack: Vec<usize> = Vec::new();
        let mut comps: Vec<Comp> = Vec::new();
        for first in 0..n {
            let p = cpat[first];
            if barrier[first] != 0
                || label[first] != NONE
                || !(p == P_GRID || p == P_CURVY || p == P_INDUSTRIAL)
            {
                continue;
            }
            let id = comps.len() as u32;
            label[first] = id;
            stack.push(first);
            let (mut cells, mut sx, mut sz) = (0u32, 0.0f64, 0.0f64);
            let mut zc = [0u32; 12];
            let mut ts = [0.0f32; 2];
            let mut contacts = 0u32;
            let mut touch: Vec<usize> = Vec::new();
            while let Some(c) = stack.pop() {
                let (ci, cj) = (c % rw, c / rw);
                cells += 1;
                sx += (ci as f64 + 0.5) * f64::from(RASTER_M);
                sz += (cj as f64 + 0.5) * f64::from(RASTER_M);
                zc[czone[c] as usize] += 1;
                let nbs = [
                    (ci > 0).then(|| c - 1),
                    (ci + 1 < rw).then(|| c + 1),
                    (cj > 0).then(|| c - rw),
                    (cj + 1 < rh).then(|| c + rw),
                ];
                for nb in nbs.into_iter().flatten() {
                    if barrier[nb] == 2 {
                        ts[0] += tens[nb][0];
                        ts[1] += tens[nb][1];
                        contacts += 1;
                        touch.push(nb);
                    } else if barrier[nb] == 0 && label[nb] == NONE && cpat[nb] == p {
                        label[nb] = id;
                        stack.push(nb);
                    }
                }
            }
            let key = hash64(self.seed.0, &[stage::STREETS, key::COMP, first as u64]);
            let centroid = V2::new(
                (sx / f64::from(cells)) as f32,
                (sz / f64::from(cells)) as f32,
            );
            let mut zone = 0;
            for k in 0..12 {
                if zc[k] > zc[zone] {
                    zone = k;
                }
            }
            let zone = ALL_ZONES[zone];
            let (maj, coh) = major_of(ts);
            let dir = if contacts >= 6 && coh >= 0.3 * contacts as f32 {
                maj
            } else {
                coast_tangent(self.m, centroid)
            };
            let (lr, dr) = self
                .t
                .zones
                .get(zone)
                .map_or(((100.0, 100.0), (80.0, 80.0)), |r| r.block_m);
            let len = lerp(lr.0, lr.1, unit(key, 1));
            let dep = lerp(dr.0, dr.1, unit(key, 2));
            // No lattice where it could never reach an arterial or hold a block.
            let area = cells as f32 * RASTER_M * RASTER_M;
            let lat = (p != P_CURVY && contacts > 0 && area >= 1.5 * len * dep).then(|| {
                let n = dir.left();
                // Anchor each family on the bounding arterial parallel to it nearest the
                // centroid, so the blocks along it are whole (else a random offset).
                let mut anchor: [Option<(f32, f32)>; 2] = [None, None];
                for &c in &touch {
                    let along = major_of(tens[c]).0;
                    let at = V2::new(
                        ((c % rw) as f32 + 0.5) * RASTER_M,
                        ((c / rw) as f32 + 0.5) * RASTER_M,
                    ) - centroid;
                    let dist = at.length();
                    for (k, axis, across) in [(0, dir, n), (1, n, dir)] {
                        if abs(along.dot(axis)) > 0.95 && anchor[k].is_none_or(|a| dist < a.0) {
                            anchor[k] = Some((dist, at.dot(across)));
                        }
                    }
                }
                let off_n = anchor[0].map_or(dep * unit(key, 4), |a| a.1);
                let off_d = anchor[1].map_or(len * unit(key, 3), |a| a.1);
                Lattice {
                    origin: centroid + dir * off_d + n * off_n,
                    d: dir,
                    n,
                    len,
                    dep,
                    key,
                    comp: id,
                    pattern: p,
                    kind: if p == P_INDUSTRIAL {
                        Kind::Industrial
                    } else {
                        Kind::Local
                    },
                    dpar: [(0.45 * dep).min(45.0), (0.45 * len).min(45.0)],
                }
            });
            comps.push(Comp {
                pattern: p,
                zone,
                dir,
                lat,
            });
        }
        self.label = label;
        self.comps = comps;
    }

    /// The section lattice: axis-aligned like the farm arterials (the world axis nearest
    /// the coast), through the midpoints of the longest aligned `Rural` arterials so the
    /// lines continue theirs.
    fn section_lattice(&self) -> Option<Lattice> {
        if !self.pat.contains(&P_SECTIONS)
            || !self
                .m
                .zones
                .iter()
                .any(|&z| self.pat[z as usize] == P_SECTIONS)
        {
            return None;
        }
        let mid = V2::new(self.ext[0] * 0.5, self.ext[1] * 0.5);
        let ct = coast_tangent(self.m, mid);
        let d = if abs(ct.x) >= abs(ct.y) {
            V2::new(1.0, 0.0)
        } else {
            V2::new(0.0, 1.0)
        };
        let n = d.left();
        let key = hash64(self.seed.0, &[stage::STREETS, key::SECTIONS]);
        // Best aligned rural edge per axis: (length, midpoint).
        let mut best = [(0.0f32, V2::new(0.0, 0.0)); 2];
        for e in &self.art.edges {
            if e.class != RoadClass::Rural || e.profile.len() < 2 {
                continue;
            }
            let pts: Vec<V2> = e.profile.iter().map(|p| V2::new(p[0], p[2])).collect();
            let chord = pts[pts.len() - 1] - pts[0];
            let len = chord.length();
            let dir = chord.normalize();
            let mid = pts[pts.len() / 2];
            for (k, axis) in [d, n].iter().enumerate() {
                if abs(dir.dot(*axis)) > 0.98 && len > best[k].0 {
                    best[k] = (len, mid);
                }
            }
        }
        // Family 0 lines run along `d` at offsets along `n`: they pass through the edge
        // aligned with `d`; family 1 through the one aligned with `n`.
        let off_n = if best[0].0 > 0.0 {
            best[0].1.dot(n)
        } else {
            SECTION_M * unit(key, 1)
        };
        let off_d = if best[1].0 > 0.0 {
            best[1].1.dot(d)
        } else {
            SECTION_M * unit(key, 2)
        };
        Some(Lattice {
            origin: d * off_d + n * off_n,
            d,
            n,
            len: SECTION_M,
            dep: SECTION_M,
            key,
            comp: NONE,
            pattern: P_SECTIONS,
            kind: Kind::Section,
            dpar: [SECTION_PARALLEL_M, SECTION_PARALLEL_M],
        })
    }

    /// Every grid and industrial lattice.
    pub fn grid_cells(&self) -> Vec<GridCell> {
        self.comps
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let l = c.lat.as_ref()?;
                Some(GridCell {
                    comp: i as u32,
                    zone: c.zone,
                    industrial: c.pattern == P_INDUSTRIAL,
                    dir: [l.d.x, l.d.y],
                    origin: [l.origin.x, l.origin.y],
                    block_m: [l.len, l.dep],
                })
            })
            .collect()
    }

    /// The component at world `p` (as in [`GridCell::comp`]), if any.
    pub fn comp_at(&self, p: [f32; 2]) -> Option<u32> {
        let l = self.label_at(V2::from_array(p));
        (l != NONE).then_some(l)
    }

    /// The terrain the street profiles follow: the macro heights, corridor carve and all.
    pub fn graded(&self) -> &[f32] {
        &self.graded
    }

    // -- Sampling ------------------------------------------------------------------------

    fn hgt(&self, p: V2) -> f32 {
        sample_nodes(
            &self.graded,
            self.m.width as usize,
            self.m.height as usize,
            p,
        )
    }

    fn cell_of(&self, p: V2) -> Option<usize> {
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let (i, j) = ((p.x / RASTER_M) as usize, (p.y / RASTER_M) as usize);
        (i < self.rw && j < self.rh).then_some(j * self.rw + i)
    }

    fn label_at(&self, p: V2) -> u32 {
        self.cell_of(p).map_or(NONE, |c| self.label[c])
    }

    fn adist_at(&self, p: V2) -> f32 {
        self.cell_of(p).map_or(0.0, |c| self.adist[c])
    }

    fn pattern_at(&self, p: V2) -> u8 {
        self.pat[self.m.zone_at(p.x, p.y) as usize]
    }

    /// Within the playable rectangle (the place less its edge bands), `margin` in.
    fn inside(&self, p: V2, margin: f32) -> bool {
        self.m.in_playable(p.x, p.y, margin)
    }

    /// Dry ground a street may use.
    fn land(&self, p: V2) -> bool {
        let wl = self.m.water_level;
        self.inside(p, MARGIN_M)
            && self.m.zone_at(p.x, p.y) != ZoneId::Water
            && self.hgt(p) > wl + 0.5
            && self.m.height_at(p.x, p.y) > wl + 0.2
    }

    fn ok(&self, p: V2, pattern: u8) -> bool {
        self.pattern_at(p) == pattern && self.land(p)
    }

    /// A straight piece from `a` to `b` stays on dry land (sampled every 4 m).
    fn dry(&self, a: V2, b: V2) -> bool {
        let n = (ceil((b - a).length() / SAMPLE_M) as usize).max(1);
        (0..=n).all(|k| self.land(a.lerp(b, k as f32 / n as f32)))
    }

    /// The grade-limited profile of a street through `pts` from height `y0` to `y1`: the
    /// graded terrain filtered to [`FILTER_GRADE`]; `None` where that leaves the terrain
    /// by more than [`CUT_FILL_M`] (too steep for a street).
    fn profile(&self, pts: &[V2], y0: f32, y1: f32) -> Option<Vec<f32>> {
        let ground: Vec<f32> = pts.iter().map(|&p| self.hgt(p)).collect();
        let mut s = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        for (k, &p) in pts.iter().enumerate() {
            if k > 0 {
                acc += (p - pts[k - 1]).length();
            }
            s.push(acc);
        }
        let ys = grade_limit(
            &ground,
            &s,
            FILTER_GRADE,
            Some(y0),
            Some(y1),
            Some((&ground, PROFILE_BAND_M)),
        );
        let fits = ys
            .iter()
            .zip(&ground)
            .all(|(y, g)| abs(y - g) <= CUT_FILL_M)
            && (1..ys.len()).all(|k| abs(ys[k] - ys[k - 1]) <= MAX_GRADE * (s[k] - s[k - 1]));
        fits.then_some(ys)
    }

    /// A feature through `pts` with its profile, the end heights pinned to `y0`/`y1` when
    /// given (else the arterial's at a joined end, the terrain's at a free one); `None`
    /// where it would leave dry land or be too steep.
    fn make(
        &self,
        key: u64,
        kind: Kind,
        pts: Vec<V2>,
        ends: [End; 2],
        y0: Option<f32>,
        y1: Option<f32>,
    ) -> Option<Feature> {
        if pts.len() < 2 || !pts.windows(2).all(|w| self.dry(w[0], w[1])) {
            return None;
        }
        let end_y = |e: End, p: V2, y: Option<f32>| match (y, e) {
            (Some(y), _) => y,
            (None, End::Snap(s)) => s.y,
            (None, End::Free) => self.hgt(p),
        };
        let ya = end_y(ends[0], pts[0], y0);
        let yb = end_y(ends[1], pts[pts.len() - 1], y1);
        let ys = self.profile(&pts, ya, yb)?;
        Some(Feature {
            key,
            kind,
            pts,
            ys,
            ends,
        })
    }

    // -- Arterials -------------------------------------------------------------------------

    fn seg(&self, e: u32, s: u32) -> ([f32; 3], [f32; 3]) {
        let p = &self.art.edges[e as usize].profile;
        (p[s as usize], p[s as usize + 1])
    }

    /// The point `u` along segment `s` of arterial `e`, merged into the edge's end node
    /// when within [`MERGE_M`].
    fn snap_at(&self, e: u32, s: u32, u: f32) -> Snap {
        let (a, b) = self.seg(e, s);
        let pos = V2::new(lerp(a[0], b[0], u), lerp(a[2], b[2], u));
        let y = lerp(a[1], b[1], u);
        let edge = &self.art.edges[e as usize];
        for nd in [edge.a, edge.b] {
            let node = &self.art.nodes[nd as usize];
            if (V2::from_array(node.pos) - pos).length() < MERGE_M {
                return Snap {
                    edge: e,
                    seg: s,
                    u,
                    pos: V2::from_array(node.pos),
                    y: node.elevation,
                    node: nd,
                };
            }
        }
        Snap {
            edge: e,
            seg: s,
            u,
            pos,
            y,
            node: NONE,
        }
    }

    /// Whether a street may join the arterial at `s`: not the highway or a ramp, and not
    /// on an overpass approach.
    fn joins(&self, s: &Snap) -> bool {
        self.joinable[s.edge as usize] && abs(s.y - self.hgt(s.pos)) <= JOIN_STEP_M
    }

    /// The nearest point within `r` of `p` on an arterial a street may join.
    fn nearest_join(&self, p: V2, r: f32) -> Option<(f32, Snap)> {
        let (i0, i1, j0, j1) = self.index.range(p.x - r, p.y - r, p.x + r, p.y + r);
        let mut best: Option<(f32, u32, u32, f32)> = None;
        for j in j0..=j1 {
            for i in i0..=i1 {
                for &[e, s] in self.index.cell(i, j) {
                    if !self.joinable[e as usize] {
                        continue;
                    }
                    let (a, b) = self.seg(e, s);
                    let (d, u) = segment_distance(p, V2::new(a[0], a[2]), V2::new(b[0], b[2]));
                    let better = match best {
                        None => d <= r,
                        Some((bd, be, bs, _)) => d < bd || (d == bd && (e, s) < (be, bs)),
                    };
                    if better && self.joins(&self.snap_at(e, s, u)) {
                        best = Some((d, e, s, u));
                    }
                }
            }
        }
        let (d, e, s, u) = best?;
        Some((d, self.snap_at(e, s, u)))
    }

    /// Whether an arterial runs within `r` of `p` nearly parallel to `dir`.
    fn parallel_art(&self, p: V2, dir: V2, r: f32) -> bool {
        if self.adist_at(p) > r + RASTER_M * 1.5 {
            return false;
        }
        let (i0, i1, j0, j1) = self.index.range(p.x - r, p.y - r, p.x + r, p.y + r);
        for j in j0..=j1 {
            for i in i0..=i1 {
                for &[e, s] in self.index.cell(i, j) {
                    let (a, b) = self.seg(e, s);
                    let (a, b) = (V2::new(a[0], a[2]), V2::new(b[0], b[2]));
                    if abs((b - a).normalize().dot(dir)) > 0.9 && segment_distance(p, a, b).0 < r {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Whether the segment `a`–`b` crosses an arterial (ignoring a touch at `b` when
    /// `touch_b`).
    fn crosses_art(&self, a: V2, b: V2, touch_b: bool) -> bool {
        let (lo, hi) = bbox(&[a, b]);
        let (i0, i1, j0, j1) = self.index.range(lo.x, lo.y, hi.x, hi.y);
        let len = (b - a).length().max(1e-3);
        for j in j0..=j1 {
            for i in i0..=i1 {
                for &[e, s] in self.index.cell(i, j) {
                    let (p, q) = self.seg(e, s);
                    if let Some((t, _)) =
                        seg_intersect(a, b, V2::new(p[0], p[2]), V2::new(q[0], q[2]))
                    {
                        if !(touch_b && (1.0 - t) * len < 2.0) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Arc position `s` along arterial edge `e`: `(segment, u, point, unit tangent)`.
    fn at_arc(&self, e: u32, s: f32) -> (u32, f32, V2, V2) {
        let p = &self.art.edges[e as usize].profile;
        let mut acc = 0.0;
        for k in 0..p.len() - 1 {
            let (a, b) = (V2::new(p[k][0], p[k][2]), V2::new(p[k + 1][0], p[k + 1][2]));
            let l = (b - a).length();
            if acc + l >= s || k == p.len() - 2 {
                let u = if l > 0.0 {
                    clamp((s - acc) / l, 0.0, 1.0)
                } else {
                    0.0
                };
                return (k as u32, u, a.lerp(b, u), (b - a).normalize());
            }
            acc += l;
        }
        let a = V2::new(p[0][0], p[0][2]);
        (0, 0.0, a, V2::new(1.0, 0.0))
    }

    fn edge_length(&self, e: usize) -> f32 {
        let pts: Vec<V2> = self.art.edges[e]
            .profile
            .iter()
            .map(|p| V2::new(p[0], p[2]))
            .collect();
        polyline_length(&pts)
    }

    // -- Lattices --------------------------------------------------------------------------

    fn lattice(&self, id: u32) -> &Lattice {
        if id == SECTIONS_ID {
            self.sections.as_ref().expect("section lattice")
        } else {
            self.comps[id as usize].lat.as_ref().expect("lattice")
        }
    }

    fn line_geo(lat: &Lattice, fam: u8, idx: i32) -> (V2, V2) {
        if fam == 0 {
            (lat.origin + lat.n * (idx as f32 * lat.dep), lat.d)
        } else {
            (lat.origin + lat.d * (idx as f32 * lat.len), lat.n)
        }
    }

    /// The cached line data for `(lattice, family, index)`, its arterial crossings
    /// computed on first use.
    fn line(&self, cache: &mut Cache, id: (u32, u8, i32)) -> usize {
        if let Some(i) = cache.lines.iter().position(|l| l.id == id) {
            return i;
        }
        let lat = *self.lattice(id.0);
        let (base, dir) = Self::line_geo(&lat, id.1, id.2);
        let play = self.m.playable();
        let place = Bx {
            x0: play[0] + MARGIN_M,
            z0: play[1] + MARGIN_M,
            x1: play[2] - MARGIN_M,
            z1: play[3] - MARGIN_M,
        };
        let mut bounds: Vec<(f32, Bound)> = Vec::new();
        if let Some((tmin, tmax)) = clip_line(base, dir, &place) {
            let nrm = dir.left();
            let mut xs: Vec<(f32, Bound)> = Vec::new();
            for (e, edge) in self.art.edges.iter().enumerate() {
                let (lo, hi) = self.ebox[e];
                let side = |p: V2| (p - base).dot(nrm) > 0.0;
                let c = [
                    side(lo),
                    side(hi),
                    side(V2::new(lo.x, hi.y)),
                    side(V2::new(hi.x, lo.y)),
                ];
                if c.iter().all(|&s| s) || c.iter().all(|&s| !s) {
                    continue;
                }
                for (k, w) in edge.profile.windows(2).enumerate() {
                    let (a, b) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
                    let (sa, sb) = ((a - base).dot(nrm), (b - base).dot(nrm));
                    if (sa > 0.0) == (sb > 0.0) {
                        continue;
                    }
                    let u = sa / (sa - sb);
                    let t = (a.lerp(b, u) - base).dot(dir);
                    if t <= tmin || t >= tmax {
                        continue;
                    }
                    let sn = self.snap_at(e as u32, k as u32, u);
                    xs.push((
                        t,
                        if self.joins(&sn) {
                            Bound::Soft(sn)
                        } else {
                            Bound::Hard
                        },
                    ));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            bounds.push((tmin, Bound::Edge));
            for x in xs {
                if x.0 - bounds[bounds.len() - 1].0 >= 0.5 {
                    bounds.push(x);
                }
            }
            bounds.push((tmax, Bound::Edge));
        }
        let n = bounds.len().saturating_sub(1);
        cache.lines.push(LineData {
            id,
            base,
            dir,
            bounds,
            runs: vec![None; n],
        });
        cache.lines.len() - 1
    }

    /// The raw runs of interval `i` of a line (no drops, no trims), computed on first use.
    fn interval(&self, cache: &mut Cache, li: usize, i: usize) -> Vec<Run> {
        if let Some(r) = &cache.lines[li].runs[i] {
            return r.clone();
        }
        let l = &cache.lines[li];
        let lat = *self.lattice(l.id.0);
        let runs = self.interval_runs(&lat, l.id.1, l.base, l.dir, l.bounds[i], l.bounds[i + 1]);
        cache.lines[li].runs[i] = Some(runs.clone());
        runs
    }

    /// Samples one interval between arterial crossings every 4 m and returns the maximal
    /// stretches that are in the lattice's pattern (and component), dry, not too steep,
    /// long enough and not running along an arterial.
    fn interval_runs(
        &self,
        lat: &Lattice,
        fam: u8,
        base: V2,
        dir: V2,
        lo: (f32, Bound),
        hi: (f32, Bound),
    ) -> Vec<Run> {
        let s0 = lo.0 + if lo.1 == Bound::Hard { HARD_GAP_M } else { 0.0 };
        let s1 = hi.0 - if hi.1 == Bound::Hard { HARD_GAP_M } else { 0.0 };
        if s1 - s0 < STUB_M {
            return Vec::new();
        }
        let ka = ceil((s0 + 0.01) / SAMPLE_M) as i32;
        let kb = floor((s1 - 0.01) / SAMPLE_M) as i32;
        if kb < ka {
            return Vec::new();
        }
        let n = (kb - ka + 1) as usize;
        let mut ok = vec![false; n];
        let mut hs = vec![0.0f32; n];
        let mut lab = vec![NONE; n];
        for k in 0..n {
            let p = base + dir * ((ka + k as i32) as f32 * SAMPLE_M);
            hs[k] = self.hgt(p);
            lab[k] = self.label_at(p);
            ok[k] = self.ok(p, lat.pattern)
                && (lat.comp == NONE || lab[k] == lat.comp || lab[k] == NONE);
        }
        // Steep stretches (over 16 m; the profile filter evens out gentler ones) break runs.
        let mut good = ok.clone();
        for k in 2..n.saturating_sub(2) {
            if abs(hs[k + 2] - hs[k - 2]) > STEEP_GRADE * 4.0 * SAMPLE_M {
                good[k - 1] = false;
                good[k] = false;
                good[k + 1] = false;
            }
        }
        // Long stretches on arterial cells (running along one) break runs too.
        if lat.comp != NONE {
            let mut k = 0;
            while k < n {
                if good[k] && lab[k] == NONE {
                    let start = k;
                    while k < n && good[k] && lab[k] == NONE {
                        k += 1;
                    }
                    if k - start > MAX_BARRIER_SAMPLES {
                        good[start..k].iter_mut().for_each(|g| *g = false);
                    }
                } else {
                    k += 1;
                }
            }
        }
        let mut out = Vec::new();
        let mut k = 0;
        while k < n {
            if !good[k] {
                k += 1;
                continue;
            }
            let ia = k;
            while k < n && good[k] {
                k += 1;
            }
            let ib = k - 1;
            let mut r = Run {
                t0: (ka + ia as i32) as f32 * SAMPLE_M,
                t1: (ka + ib as i32) as f32 * SAMPLE_M,
                e0: REnd::Dead,
                e1: REnd::Dead,
            };
            if let Bound::Soft(sn) = lo.1 {
                if r.t0 - lo.0 <= SNAP_REACH_M {
                    r.t0 = lo.0;
                    r.e0 = REnd::Snap(sn);
                }
            }
            if let Bound::Soft(sn) = hi.1 {
                if hi.0 - r.t1 <= SNAP_REACH_M {
                    r.t1 = hi.0;
                    r.e1 = REnd::Snap(sn);
                }
            }
            if r.t1 - r.t0 < STUB_M {
                continue;
            }
            if lat.comp != NONE && !lab[ia..=ib].contains(&lat.comp) {
                continue;
            }
            // Along an arterial (one already serving this line's purpose): drop.
            let (mut near, mut total) = (0u32, 0u32);
            let mut j = ia;
            while j <= ib {
                let p = base + dir * ((ka + j as i32) as f32 * SAMPLE_M);
                total += 1;
                if self.parallel_art(p, dir, lat.dpar[fam as usize]) {
                    near += 1;
                }
                j += 4;
            }
            if near * 5 > total * 2 {
                continue;
            }
            // A profile must exist (the final feature recomputes it after trims).
            let end = |e: REnd, t: f32| match e {
                REnd::Snap(s) => (s.pos, s.y),
                _ => {
                    let p = base + dir * t;
                    (p, self.hgt(p))
                }
            };
            let ((p0, y0), (p1, y1)) = (end(r.e0, r.t0), end(r.e1, r.t1));
            if self
                .profile(&densify(p0, p1, PROFILE_STEP_M), y0, y1)
                .is_none()
            {
                continue;
            }
            out.push(r);
        }
        out
    }

    /// Index of the interval of line `li` holding parameter `t`.
    fn interval_at(cache: &Cache, li: usize, t: f32) -> Option<usize> {
        let b = &cache.lines[li].bounds;
        (0..b.len().saturating_sub(1)).find(|&i| b[i].0 <= t && t <= b[i + 1].0)
    }

    /// Whether a raw run of the line covers `[a, b]`.
    fn covers(&self, cache: &mut Cache, id: (u32, u8, i32), a: f32, b: f32) -> bool {
        let li = self.line(cache, id);
        let Some(i) = Self::interval_at(cache, li, a) else {
            return false;
        };
        self.interval(cache, li, i)
            .iter()
            .any(|r| r.t0 - 0.5 <= a && b <= r.t1 + 0.5)
    }

    /// Whether the lattice drops the segment of cross line `j` between parallel lines `k`
    /// and `k + 1` (a T). Only inside one raw run, only where both parallel lines pass.
    fn dropped(&self, cache: &mut Cache, id: u32, j: i32, k: i32) -> bool {
        let lat = *self.lattice(id);
        if lat.pattern == P_SECTIONS
            || k.rem_euclid(2) != 0
            || unit(hash64(lat.key, &[key::DROP, word(j), word(k)]), 0) >= T_CHANCE
        {
            return false;
        }
        let (a, b) = (k as f32 * lat.dep, (k + 1) as f32 * lat.dep);
        let tj = j as f32 * lat.len;
        self.covers(cache, (id, 1, j), a - 1.0, b + 1.0)
            && self.covers(cache, (id, 0, k), tj, tj)
            && self.covers(cache, (id, 0, k + 1), tj, tj)
    }

    /// The final runs of a line overlapping `[ta, tb]`: raw runs with T drops cut out,
    /// dead ends trimmed back to the last cross street when they overhang it by less than
    /// [`STUB_M`], and runs touching nothing dropped.
    fn runs(&self, cache: &mut Cache, id: (u32, u8, i32), ta: f32, tb: f32) -> Vec<Run> {
        let lat = *self.lattice(id.0);
        let li = self.line(cache, id);
        let nb = cache.lines[li].bounds.len();
        let mut out = Vec::new();
        for i in 0..nb.saturating_sub(1) {
            let (b0, b1) = (cache.lines[li].bounds[i].0, cache.lines[li].bounds[i + 1].0);
            if b1 < ta - RUN_REACH_M || b0 > tb + RUN_REACH_M {
                continue;
            }
            for r in self.interval(cache, li, i) {
                if r.t1 < ta - RUN_REACH_M || r.t0 > tb + RUN_REACH_M {
                    continue;
                }
                let mut pieces = vec![r];
                if id.1 == 1 && lat.pattern != P_SECTIONS {
                    let k0 = ceil((r.t0 + 1.0) / lat.dep) as i32;
                    let k1 = floor((r.t1 - 1.0) / lat.dep) as i32 - 1;
                    for k in k0..=k1 {
                        if self.dropped(cache, id.0, id.2, k) {
                            let last = pieces.len() - 1;
                            let tail = pieces[last];
                            pieces[last].t1 = k as f32 * lat.dep;
                            pieces[last].e1 = REnd::Joint;
                            pieces.push(Run {
                                t0: (k + 1) as f32 * lat.dep,
                                t1: tail.t1,
                                e0: REnd::Joint,
                                e1: tail.e1,
                            });
                        }
                    }
                }
                for p in pieces {
                    if let Some(p) = self.trim(cache, &lat, id, p) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    fn trim(&self, cache: &mut Cache, lat: &Lattice, id: (u32, u8, i32), run: Run) -> Option<Run> {
        let (fam, idx) = (id.1, id.2);
        let sp = if fam == 0 { lat.len } else { lat.dep };
        // The cross line through parameter `c sp` of this line, and where on it we are.
        let cross = |c: i32| -> ((u32, u8, i32), f32) {
            if fam == 0 {
                ((id.0, 1, c), idx as f32 * lat.dep)
            } else {
                ((id.0, 0, c), idx as f32 * lat.len)
            }
        };
        let mut r = run;
        if r.e1 == REnd::Dead {
            let mut c = floor(r.t1 / sp) as i32;
            while c as f32 * sp >= r.t0 && r.t1 - c as f32 * sp < STUB_M {
                let (cid, t) = cross(c);
                if self.covers(cache, cid, t, t) {
                    r.t1 = c as f32 * sp;
                    r.e1 = REnd::Joint;
                    break;
                }
                c -= 1;
            }
        }
        if r.e0 == REnd::Dead {
            let mut c = ceil(r.t0 / sp) as i32;
            while c as f32 * sp <= r.t1 && c as f32 * sp - r.t0 < STUB_M {
                let (cid, t) = cross(c);
                if self.covers(cache, cid, t, t) {
                    r.t0 = c as f32 * sp;
                    r.e0 = REnd::Joint;
                    break;
                }
                c += 1;
            }
        }
        if r.t1 - r.t0 < STUB_M {
            return None;
        }
        // A run that reaches no arterial must cross one that does (else it would be an
        // island, or part of one).
        let snapped = |e: REnd| matches!(e, REnd::Snap(_));
        if !snapped(r.e0) && !snapped(r.e1) {
            let mut c = ceil(r.t0 / sp) as i32;
            let mut found = false;
            while c as f32 * sp <= r.t1 {
                let (cid, t) = cross(c);
                if self
                    .covering(cache, cid, t)
                    .is_some_and(|x| snapped(x.e0) || snapped(x.e1))
                {
                    found = true;
                    break;
                }
                c += 1;
            }
            if !found {
                return None;
            }
        }
        Some(r)
    }

    /// The raw run of the line covering parameter `t`, if any.
    fn covering(&self, cache: &mut Cache, id: (u32, u8, i32), t: f32) -> Option<Run> {
        let li = self.line(cache, id);
        let i = Self::interval_at(cache, li, t)?;
        self.interval(cache, li, i)
            .into_iter()
            .find(|r| r.t0 - 0.5 <= t && t <= r.t1 + 0.5)
    }

    fn run_feature(&self, lat: &Lattice, fam: u8, idx: i32, r: &Run) -> Option<Feature> {
        let (base, dir) = Self::line_geo(lat, fam, idx);
        let end = |e: REnd, t: f32| match e {
            REnd::Snap(s) => (s.pos, End::Snap(s)),
            _ => (base + dir * t, End::Free),
        };
        let (p0, e0) = end(r.e0, r.t0);
        let (p1, e1) = end(r.e1, r.t1);
        let line_key = hash64(lat.key, &[key::LINE, u64::from(fam), word(idx)]);
        self.make(
            hash64(line_key, &[u64::from(r.t0.to_bits())]),
            lat.kind,
            densify(p0, p1, PROFILE_STEP_M),
            [e0, e1],
            None,
            None,
        )
    }

    /// The lattice lines, alleys and (for sections) driveways reaching `bx`.
    fn lattice_features(
        &self,
        cache: &mut Cache,
        id: u32,
        bx: &Bx,
        feats: &mut Vec<Feature>,
        farms: &mut Vec<[f32; 2]>,
    ) {
        let lat = *self.lattice(id);
        let corners = bx.grow(1.0).corners();
        let span = |axis: V2, sp: f32, pad: f32| {
            let s: Vec<f32> = corners
                .iter()
                .map(|&c| (c - lat.origin).dot(axis))
                .collect();
            let lo = s.iter().copied().fold(f32::INFINITY, f32::min) - pad;
            let hi = s.iter().copied().fold(f32::NEG_INFINITY, f32::max) + pad;
            (floor(lo / sp) as i32, ceil(hi / sp) as i32)
        };
        for fam in 0..2u8 {
            let (axis, sp) = if fam == 0 {
                (lat.n, lat.dep)
            } else {
                (lat.d, lat.len)
            };
            let (i0, i1) = span(axis, sp, 0.0);
            for idx in i0..=i1 {
                let (base, dir) = Self::line_geo(&lat, fam, idx);
                let Some((ta, tb)) = clip_line(base, dir, bx) else {
                    continue;
                };
                for r in self.runs(cache, (id, fam, idx), ta, tb) {
                    feats.extend(self.run_feature(&lat, fam, idx, &r));
                }
            }
        }
        if lat.pattern == P_GRID {
            let (j0, j1) = span(lat.d, lat.len, 0.0);
            let (k0, k1) = span(lat.n, lat.dep, 0.0);
            for k in k0 - 1..=k1 {
                for j in j0 - 1..=j1 {
                    if let Some(f) = self.alley(cache, id, &lat, j, k, bx) {
                        feats.push(f);
                    }
                }
            }
        }
        if lat.pattern == P_SECTIONS {
            let (j0, j1) = span(lat.d, FARM_M, CONNECT_M);
            let (k0, k1) = span(lat.n, FARM_M, CONNECT_M);
            for k in k0 - 1..=k1 {
                for j in j0 - 1..=j1 {
                    if let Some((f, farm)) = self.driveway(cache, &lat, j, k, bx) {
                        if bx.owns(farm) {
                            farms.push([farm.x, farm.y]);
                        }
                        feats.push(f);
                    }
                }
            }
        }
    }

    /// The alley down the middle of block `(j, k)`, parallel to its long side, when the
    /// block's zone has alleys and both cross streets pass its ends.
    fn alley(
        &self,
        cache: &mut Cache,
        id: u32,
        lat: &Lattice,
        j: i32,
        k: i32,
        bx: &Bx,
    ) -> Option<Feature> {
        let base = lat.origin + lat.n * ((k as f32 + 0.5) * lat.dep);
        let a = base + lat.d * (j as f32 * lat.len);
        let b = base + lat.d * ((j + 1) as f32 * lat.len);
        let (lo, hi) = bbox(&[a, b]);
        if !bx.hits(lo, hi) {
            return None;
        }
        let c = a.lerp(b, 0.5);
        if !self.alley_zone[self.m.zone_at(c.x, c.y) as usize] || self.label_at(c) != lat.comp {
            return None;
        }
        let (sa, sb) = (k as f32 * lat.dep, (k + 1) as f32 * lat.dep);
        if !self.covers(cache, (id, 1, j), sa - 1.0, sb + 1.0)
            || !self.covers(cache, (id, 1, j + 1), sa - 1.0, sb + 1.0)
            || self.dropped(cache, id, j, k)
            || self.dropped(cache, id, j + 1, k)
        {
            return None;
        }
        if self.crosses_art(a, b, false) || self.parallel_art(c, lat.d, 14.0) {
            return None;
        }
        let n = (ceil((b - a).length() / SAMPLE_M) as usize).max(1);
        for s in 0..=n {
            let p = a.lerp(b, s as f32 / n as f32);
            let l = self.label_at(p);
            if !self.ok(p, P_GRID) || (l != NONE && l != lat.comp) {
                return None;
            }
        }
        self.make(
            hash64(lat.key, &[key::ALLEY, word(j), word(k)]),
            Kind::Alley,
            densify(a, b, PROFILE_STEP_M),
            [End::Free, End::Free],
            None,
            None,
        )
    }

    /// The driveway to the farmstead of quarter cell `(j, k)` (200 m) of the section
    /// lattice, from the nearest section line or arterial.
    fn driveway(
        &self,
        cache: &mut Cache,
        lat: &Lattice,
        j: i32,
        k: i32,
        bx: &Bx,
    ) -> Option<(Feature, V2)> {
        let key = hash64(lat.key, &[key::FARM, word(j), word(k)]);
        // The first of four spots, each 40–120 m in from a side of the cell, on farmland
        // and off the arterials.
        let corner = lat.origin + lat.d * (j as f32 * FARM_M) + lat.n * (k as f32 * FARM_M);
        let f = (0..4u64).find_map(|t| {
            let along = 0.2 + 0.6 * unit(key, 10 + t);
            let inward = lerp(40.0, 120.0, unit(key, 20 + t));
            let (u, v) = match (unit(key, 30 + t) * 4.0) as u32 {
                0 => (along * FARM_M, inward),
                1 => (along * FARM_M, FARM_M - inward),
                2 => (inward, along * FARM_M),
                _ => (FARM_M - inward, along * FARM_M),
            };
            let f = corner + lat.d * u + lat.n * v;
            (self.pattern_at(f) == P_SECTIONS && self.land(f) && self.adist_at(f) >= 30.0)
                .then_some(f)
        })?;
        if !bx.grow(CONNECT_M).owns(f) {
            return None;
        }
        // The section lines around the section cell holding the farmstead.
        let rel = f - lat.origin;
        let (sj, sk) = (
            floor(rel.dot(lat.d) / lat.len) as i32,
            floor(rel.dot(lat.n) / lat.dep) as i32,
        );
        let mut best: Option<(f32, V2, End)> = None;
        for (fam, idx) in [(0u8, sk), (0, sk + 1), (1, sj), (1, sj + 1)] {
            let (base, dir) = Self::line_geo(lat, fam, idx);
            let t = (f - base).dot(dir);
            let foot = base + dir * t;
            let d = (f - foot).length();
            if best.is_some_and(|b| b.0 <= d) {
                continue;
            }
            if self.covers(cache, (SECTIONS_ID, fam, idx), t - STUB_M, t + STUB_M) {
                best = Some((d, foot, End::Free));
            }
        }
        if let Some((d, sn)) = self.nearest_join(f, CONNECT_M) {
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, sn.pos, End::Snap(sn)));
            }
        }
        let (d, foot, end) = best?;
        if !(40.0..=CONNECT_M).contains(&d)
            || self.crosses_art(f, foot, matches!(end, End::Snap(_)))
        {
            return None;
        }
        let feat = self.make(
            key,
            Kind::Driveway,
            densify(foot, f, PROFILE_STEP_M),
            [end, End::Free],
            None,
            None,
        )?;
        Some((feat, f))
    }

    // -- Suburb pods ---------------------------------------------------------------------------

    /// Places the suburb pods (part of the plan): greedily at the deepest points of the
    /// `Curvy` components (by distance to their edges), each a stadium as wide as the
    /// component allows there and stretched along the component's local axis, at least
    /// [`POD_GAP_M`] from the pods placed before it.
    fn place_pods(&mut self) {
        let (rw, rh) = (self.rw, self.rh);
        let n = rw * rh;
        let curvy = |l: u32| l != NONE && self.comps[l as usize].pattern == P_CURVY;
        let mut depth: Vec<f32> = self
            .label
            .iter()
            .map(|&l| if curvy(l) { f32::INFINITY } else { 0.0 })
            .collect();
        chamfer(&mut depth, rw, rh, RASTER_M);
        let mut cand: Vec<(f32, usize)> = (0..n)
            .filter(|&c| depth[c] >= POD_MIN_B_M + POD_EDGE_M)
            .map(|c| (depth[c], c))
            .collect();
        cand.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let center_of = |c: usize| {
            V2::new(
                ((c % rw) as f32 + 0.5) * RASTER_M,
                ((c / rw) as f32 + 0.5) * RASTER_M,
            )
        };
        let depth_at = |p: V2| -> f32 {
            if p.x < 0.0 || p.y < 0.0 {
                return 0.0;
            }
            let (i, j) = ((p.x / RASTER_M) as usize, (p.y / RASTER_M) as usize);
            if i < rw && j < rh {
                depth[j * rw + i]
            } else {
                0.0
            }
        };
        let mut pods: Vec<PodSpec> = Vec::new();
        for &(r, c) in &cand {
            let center = center_of(c);
            let comp = self.label[c];
            // A little narrower than the deepest point allows, so it can stretch.
            let mut b = (0.85 * (r - POD_EDGE_M)).min(POD_MAX_B_M);
            for p in &pods {
                b = b.min(segment_distance(center, p.core0(), p.core1()).0 - p.room() - POD_GAP_M);
            }
            if b < POD_MIN_B_M {
                continue;
            }
            let axis = self.local_axis(c, comp);
            // Stretch along the axis while the band stays wide enough.
            let mut ext = [0.0f32; 2];
            for (k, sgn) in [1.0f32, -1.0].iter().enumerate() {
                let mut s = SPLINE_STEP_M;
                while s <= POD_MAX_STRETCH_M {
                    let q = center + axis * (sgn * s);
                    let clear = pods.iter().all(|p| {
                        segment_distance(q, p.core0(), p.core1()).0 >= p.room() + b + POD_GAP_M
                    });
                    if self.label_at(q) != comp || depth_at(q) < b + 0.5 * POD_EDGE_M || !clear {
                        break;
                    }
                    ext[k] = s;
                    s += SPLINE_STEP_M;
                }
            }
            let center = center + axis * (0.5 * (ext[0] - ext[1]));
            let h = 0.5 * (ext[0] + ext[1]);
            pods.push(PodSpec {
                key: hash64(self.seed.0, &[stage::STREETS, key::POD, c as u64]),
                center,
                comp,
                axis,
                h,
                b,
            });
        }
        // Spines: a curving street along the bands too narrow for a loop.
        for (c, &dc) in depth.iter().enumerate() {
            if dc < SPINE_MIN_DEPTH_M {
                continue;
            }
            let center = center_of(c);
            let comp = self.label[c];
            let gap = |q: V2, pods: &[PodSpec]| {
                pods.iter()
                    .map(|p| segment_distance(q, p.core0(), p.core1()).0 - p.room() - SPINE_HALF_M)
                    .fold(f32::INFINITY, f32::min)
            };
            if gap(center, &pods) < POD_GAP_M {
                continue;
            }
            let axis = self.local_axis(c, comp);
            let mut ext = [0.0f32; 2];
            for (k, sgn) in [1.0f32, -1.0].iter().enumerate() {
                let mut s = SPLINE_STEP_M;
                while s <= SPINE_MAX_HALF_M {
                    let q = center + axis * (sgn * s);
                    if self.label_at(q) != comp
                        || depth_at(q) < 0.6 * SPINE_MIN_DEPTH_M
                        || gap(q, &pods) < POD_GAP_M
                    {
                        break;
                    }
                    ext[k] = s;
                    s += SPLINE_STEP_M;
                }
            }
            let h = 0.5 * (ext[0] + ext[1]);
            if h < 0.5 * SPINE_MIN_LENGTH_M {
                continue;
            }
            pods.push(PodSpec {
                key: hash64(self.seed.0, &[stage::STREETS, key::POD, c as u64]),
                center: center + axis * (0.5 * (ext[0] - ext[1])),
                comp,
                axis,
                h,
                b: 0.0,
            });
        }
        self.pods = pods;
    }

    /// The principal direction of component `comp`'s raster cells within 160 m of cell `c`
    /// (the component's arterial direction where that is undefined).
    fn local_axis(&self, c: usize, comp: u32) -> V2 {
        let (rw, rh) = (self.rw, self.rh);
        let mut ts = [0.0f32; 2];
        let reach = (160.0 / RASTER_M) as i32;
        let (ci, cj) = ((c % rw) as i32, (c / rw) as i32);
        let mut dj = -reach;
        while dj <= reach {
            let mut di = -reach;
            while di <= reach {
                let (i, j) = (ci + di, cj + dj);
                if i >= 0 && j >= 0 && (i as usize) < rw && (j as usize) < rh {
                    let q = j as usize * rw + i as usize;
                    if self.label[q] == comp && (di != 0 || dj != 0) {
                        let t = tensor(V2::new(di as f32, dj as f32).normalize());
                        ts[0] += t[0];
                        ts[1] += t[1];
                    }
                }
                di += 2;
            }
            dj += 2;
        }
        let (axis, coh) = major_of(ts);
        if coh > 1e-3 {
            axis
        } else {
            self.comps[comp as usize].dir
        }
    }

    /// Pod `pi`'s loop fitted inside its component with its profile (a closed
    /// grade-limited ring), the arcs kept where it does not fit whole, and its connector to
    /// an arterial.
    fn pod(&self, pi: usize) -> Option<Pod> {
        let spec = &self.pods[pi];
        let (key, c, comp) = (spec.key, spec.center, spec.comp);
        let ax = spec.axis;
        let nn = ax.left();
        let bulge = 0.06 * unit(key, 1);
        let mut fallback: Option<Ring> = None;
        let mut chosen: Option<(Ring, bool)> = None;
        if spec.b == 0.0 {
            // A spine: a gentle wave along the axis (radii well over 100 m).
            let n = (ceil(2.0 * spec.h / SPLINE_STEP_M) as usize).max(1) + 1;
            let wave = lerp(160.0, 240.0, unit(key, 2));
            let phase = TAU * unit(key, 3);
            let pts: Vec<V2> = (0..n)
                .map(|k| {
                    let x = -spec.h + 2.0 * spec.h * k as f32 / (n - 1) as f32;
                    c + ax * x + nn * (SPINE_WAVE_M * sin_cos(TAU * x / wave + phase).0)
                })
                .collect();
            let ground: Vec<f32> = pts.iter().map(|&p| self.hgt(p)).collect();
            let mut arc = Vec::with_capacity(n);
            let mut acc = 0.0;
            for (k, &p) in pts.iter().enumerate() {
                if k > 0 {
                    acc += (p - pts[k - 1]).length();
                }
                arc.push(acc);
            }
            // The far end within the grade's reach of the near one, so the filter has a
            // solution (the cut/fill test below rejects what it cannot follow).
            let reach = 0.95 * FILTER_GRADE * arc[n - 1];
            let y1 = clamp(ground[n - 1], ground[0] - reach, ground[0] + reach);
            let ys = grade_limit(
                &ground,
                &arc,
                FILTER_GRADE,
                Some(ground[0]),
                Some(y1),
                Some((&ground, PROFILE_BAND_M)),
            );
            let mut inside: Vec<bool> = (0..n)
                .map(|q| {
                    let p = pts[q];
                    self.label_at(p) == comp
                        && self.adist_at(p) >= CLEAR_M
                        && self.land(p)
                        && abs(ys[q] - ground[q]) <= CUT_FILL_M
                })
                .collect();
            for q in 0..n - 1 {
                if !self.dry(pts[q], pts[q + 1]) {
                    inside[q] = false;
                    inside[q + 1] = false;
                }
            }
            fallback = Some((pts, ys, inside));
        }
        for s in [1.0f32, 0.85] {
            if spec.b == 0.0 {
                break;
            }
            let (h, b) = (spec.h * s, spec.b * s);
            let per = 4.0 * h + TAU * b;
            let n = (ceil(per / SPLINE_STEP_M) as usize).max(24);
            let mut pts = Vec::with_capacity(n);
            for q in 0..n {
                let sig = per * q as f32 / n as f32;
                let (x, y) = if sig < 2.0 * h {
                    let x = -h + sig;
                    let t = if h > 0.0 { (x + h) / (2.0 * h) } else { 0.0 };
                    (x, b + bulge * 2.0 * h * sin_cos(PI * t).0)
                } else if sig < 2.0 * h + PI * b {
                    let (sn, cs) = sin_cos(0.5 * PI - (sig - 2.0 * h) / b);
                    (h + b * cs, b * sn)
                } else if sig < 4.0 * h + PI * b {
                    let x = h - (sig - 2.0 * h - PI * b);
                    let t = if h > 0.0 { (h - x) / (2.0 * h) } else { 0.0 };
                    (x, -b - bulge * 2.0 * h * sin_cos(PI * t).0)
                } else {
                    let (sn, cs) = sin_cos(-0.5 * PI - (sig - 4.0 * h - PI * b) / b);
                    (-h + b * cs, b * sn)
                };
                pts.push(c + ax * x + nn * y);
            }
            // The ring's profile, pinned at vertex 0.
            let mut ring = pts.clone();
            ring.push(pts[0]);
            let ground: Vec<f32> = ring.iter().map(|&p| self.hgt(p)).collect();
            let mut arc = Vec::with_capacity(n + 1);
            let mut acc = 0.0;
            for (k, &p) in ring.iter().enumerate() {
                if k > 0 {
                    acc += (p - ring[k - 1]).length();
                }
                arc.push(acc);
            }
            let mut ys = grade_limit(
                &ground,
                &arc,
                FILTER_GRADE,
                Some(ground[0]),
                Some(ground[0]),
                Some((&ground, PROFILE_BAND_M)),
            );
            ys.pop();
            let mut inside: Vec<bool> = (0..n)
                .map(|q| {
                    let p = pts[q];
                    self.label_at(p) == comp
                        && self.adist_at(p) >= CLEAR_M
                        && self.land(p)
                        && abs(ys[q] - ground[q]) <= CUT_FILL_M
                })
                .collect();
            for q in 0..n {
                if !self.dry(pts[q], pts[(q + 1) % n]) {
                    inside[q] = false;
                    inside[(q + 1) % n] = false;
                }
            }
            let count = inside.iter().filter(|&&x| x).count();
            if count == n {
                chosen = Some(((pts, ys, inside), true));
                break;
            }
            if fallback.is_none() && count * 10 >= n * 6 {
                fallback = Some((pts, ys, inside));
            }
        }
        let ((pts, ys, inside), full) = match chosen {
            Some(c) => c,
            None => (fallback?, false),
        };
        let n = pts.len();
        let mut arcs: Vec<Vec<usize>> = Vec::new();
        let mut usable = vec![full; n];
        if spec.b == 0.0 {
            let mut q = 0;
            while q < n {
                if !inside[q] {
                    q += 1;
                    continue;
                }
                let mut arc = Vec::new();
                while q < n && inside[q] {
                    arc.push(q);
                    q += 1;
                }
                if arc.len() >= 6 {
                    for &v in &arc[1..arc.len() - 1] {
                        usable[v] = true;
                    }
                    arcs.push(arc);
                }
            }
            if arcs.is_empty() {
                return None;
            }
        } else if !full {
            let start = (0..n).find(|&q| !inside[q])?;
            let mut q = (start + 1) % n;
            let mut visited = 0;
            while visited < n {
                if !inside[q] {
                    q = (q + 1) % n;
                    visited += 1;
                    continue;
                }
                let mut arc = Vec::new();
                while inside[q] && visited < n {
                    arc.push(q);
                    q = (q + 1) % n;
                    visited += 1;
                }
                if arc.len() >= 6 {
                    for &v in &arc[1..arc.len() - 1] {
                        usable[v] = true;
                    }
                    arcs.push(arc);
                }
            }
            if arcs.is_empty() {
                return None;
            }
        }
        // The connector: from every third usable vertex to its nearest joinable arterial
        // point; the shortest that crosses nothing and has a profile.
        let closed = spec.b > 0.0;
        let mut cands: Vec<(f32, usize, Snap)> = Vec::new();
        for q in (0..n).step_by(3) {
            if usable[q] {
                if let Some((d, sn)) = self.nearest_join(pts[q], CONNECT_M) {
                    cands.push((d, q, sn));
                }
            }
        }
        cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut art = None;
        for &(len, q, sn) in cands.iter().take(6) {
            let p = pts[q];
            let crosses_loop = (0..n).any(|r| {
                let r1 = (r + 1) % n;
                (closed || r1 > r)
                    && r != q
                    && r1 != q
                    && seg_intersect(p, sn.pos, pts[r], pts[r1]).is_some()
            });
            if len < 4.0 || crosses_loop || self.crosses_art(p, sn.pos, true) {
                continue;
            }
            art = self
                .make(
                    hash64(key, &[key::PART, 1]),
                    Kind::Local,
                    densify(p, sn.pos, PROFILE_STEP_M),
                    [End::Free, End::Snap(sn)],
                    Some(ys[q]),
                    None,
                )
                .map(|f| (q, f));
            if art.is_some() {
                break;
            }
        }
        Some(Pod {
            idx: pi,
            key,
            center: c,
            comp,
            pts,
            ys,
            usable,
            full,
            arcs,
            art,
        })
    }

    fn pod_memo(&self, memo: &mut [Option<Option<Pod>>], pi: usize) {
        if memo[pi].is_none() {
            memo[pi] = Some(self.pod(pi));
        }
    }

    /// The link street between pods `lo` and `hi` (`lo` the lower index): the usable loop
    /// vertex of `lo` nearest `hi`'s center, joined to `hi`'s usable vertex nearest that.
    fn link(&self, lo: &Pod, hi: &Pod) -> Option<(usize, usize, Feature)> {
        let nearest = |pod: &Pod, p: V2| {
            let mut best: Option<(f32, usize)> = None;
            for (q, &v) in pod.pts.iter().enumerate() {
                let d = (v - p).length();
                if pod.usable[q] && best.is_none_or(|b| d < b.0) {
                    best = Some((d, q));
                }
            }
            best
        };
        let (_, qa) = nearest(lo, hi.center)?;
        let (d, qb) = nearest(hi, lo.pts[qa])?;
        if !(4.0..=LINK_M).contains(&d) {
            return None;
        }
        let (pa, pb) = (lo.pts[qa], hi.pts[qb]);
        if self.crosses_art(pa, pb, false) {
            return None;
        }
        let f = self.make(
            hash64(lo.key, &[key::PART, 2, hi.idx as u64]),
            Kind::Local,
            densify(pa, pb, PROFILE_STEP_M),
            [End::Free, End::Free],
            Some(lo.ys[qa]),
            Some(hi.ys[qb]),
        )?;
        Some((qa, qb, f))
    }

    /// Links of pod `pi` to the pods near it: `(other pod, vertex here, link street)`, each
    /// computed in (lower, higher) index order.
    fn links(&self, memo: &mut [Option<Option<Pod>>], pi: usize) -> Vec<(usize, usize, Feature)> {
        let mut out = Vec::new();
        self.pod_memo(memo, pi);
        if memo[pi].as_ref().is_some_and(|p| p.is_none()) {
            return out;
        }
        let a = &self.pods[pi];
        for pj in 0..self.pods.len() {
            let b = &self.pods[pj];
            if pj == pi || (a.center - b.center).length() > a.h + a.b + b.h + b.b + LINK_M {
                continue;
            }
            self.pod_memo(memo, pj);
            let (Some(Some(pa)), Some(Some(pb))) = (&memo[pi], &memo[pj]) else {
                continue;
            };
            let l = if pi < pj {
                self.link(pa, pb).map(|(qa, _, f)| (qa, f))
            } else {
                self.link(pb, pa).map(|(_, qb, f)| (qb, f))
            };
            if let Some((q, f)) = l {
                out.push((pj, q, f));
            }
        }
        out
    }

    /// A pod is kept when it reaches an arterial itself or links to a pod that does.
    fn anchored(&self, memo: &mut [Option<Option<Pod>>], pi: usize) -> bool {
        self.pod_memo(memo, pi);
        match &memo[pi] {
            Some(Some(p)) if p.art.is_some() => return true,
            Some(Some(_)) => {}
            _ => return false,
        }
        for (pj, _, _) in self.links(memo, pi) {
            self.pod_memo(memo, pj);
            if matches!(&memo[pj], Some(Some(p)) if p.art.is_some()) {
                return true;
            }
        }
        false
    }

    /// The suburb pods whose streets can reach `bx`.
    fn pod_features(&self, bx: &Bx, feats: &mut Vec<Feature>) {
        let mut memo: Vec<Option<Option<Pod>>> = (0..self.pods.len()).map(|_| None).collect();
        for pi in 0..self.pods.len() {
            let spec = &self.pods[pi];
            let r = (spec.h + spec.b) * 1.1 + CONNECT_M + 2.0 * BULB_RADIUS_M + 40.0;
            if !bx.grow(r).owns(spec.center) || !self.anchored(&mut memo, pi) {
                continue;
            }
            let mut links: Vec<(usize, usize, Feature)> = Vec::new();
            for l in self.links(&mut memo, pi) {
                if self.anchored(&mut memo, l.0) {
                    links.push(l);
                }
            }
            if let Some(Some(pod)) = &memo[pi] {
                self.emit_pod(pod, &links, feats);
            }
        }
    }

    fn emit_pod(&self, pod: &Pod, links: &[(usize, usize, Feature)], feats: &mut Vec<Feature>) {
        let n = pod.pts.len();
        let key = pod.key;
        let mut attach: Vec<usize> = Vec::new();
        if let Some((q, f)) = &pod.art {
            attach.push(*q);
            feats.push(f.clone());
        }
        for (pj, q, f) in links {
            attach.push(*q);
            // The lower pod of the pair owns the link.
            if *pj > pod.idx {
                feats.push(f.clone());
            }
        }
        // Of a loop that does not fit whole, only the arcs something attaches to are kept
        // (the others would be islands).
        let arcs: Vec<&Vec<usize>> = pod
            .arcs
            .iter()
            .filter(|arc| arc.iter().any(|v| attach.contains(v)))
            .collect();
        let mut usable = pod.usable.clone();
        if !pod.full {
            usable.iter_mut().for_each(|u| *u = false);
            for arc in &arcs {
                for &v in &arc[1..arc.len() - 1] {
                    usable[v] = true;
                }
            }
        }
        // Cul-de-sacs every ~64 m of loop, alternating inward and outward.
        let step = 8usize;
        let phase = (unit(key, 9) * step as f32) as usize;
        let cyc = |a: usize, b: usize| {
            let d = a.abs_diff(b);
            d.min(n - d)
        };
        let spec = &self.pods[pod.idx];
        let mut culs: Vec<usize> = Vec::new();
        let mut m = 0usize;
        while phase + m * step + 3 < n {
            let q = phase + m * step;
            let mm = m;
            m += 1;
            if !(usable[q] && usable[(q + 1) % n] && usable[(q + n - 1) % n])
                || attach.iter().any(|&a| cyc(a, q) <= 3)
            {
                continue;
            }
            let p = pod.pts[q];
            let tg = (pod.pts[(q + 1) % n] - pod.pts[(q + n - 1) % n]).normalize();
            let nin = if tg.left().dot(pod.center - p) > 0.0 {
                tg.left()
            } else {
                tg.right()
            };
            // Inward stubs reach at most halfway to the loop's spine; outward ones (and
            // both sides of a spine street) 26–40 m.
            let spine = segment_distance(p, spec.core0(), spec.core1()).0;
            let is_spine = spec.b == 0.0;
            let outward = is_spine || !mm.is_multiple_of(2);
            let (dir, len) = if !outward {
                (nin, clamp(0.5 * spine - BULB_RADIUS_M - 6.0, 0.0, 40.0))
            } else if is_spine {
                let side = if mm.is_multiple_of(2) {
                    tg.left()
                } else {
                    tg.right()
                };
                (side, lerp(26.0, 40.0, unit(key, 100 + mm as u64)))
            } else {
                (-nin, lerp(26.0, 40.0, unit(key, 100 + mm as u64)))
            };
            if len < 12.0 {
                continue;
            }
            let end = p + dir * len;
            let far = end + dir * (2.0 * BULB_RADIUS_M);
            if outward {
                // Outward: stays in the component, clear of arterials and other loops.
                let crowded = self.pods.iter().enumerate().any(|(pj, o)| {
                    pj != pod.idx
                        && segment_distance(far, o.core0(), o.core1()).0 < o.room() * 1.1 + 24.0
                });
                if crowded
                    || self.label_at(far) != pod.comp
                    || self.label_at(end) != pod.comp
                    || self.adist_at(far) < CLEAR_M
                {
                    continue;
                }
            }
            let Some(stub) = self.make(
                hash64(key, &[key::PART, 3, q as u64]),
                Kind::Local,
                densify(p, end, PROFILE_STEP_M),
                [End::Free, End::Free],
                Some(pod.ys[q]),
                None,
            ) else {
                continue;
            };
            let y1 = stub.ys[stub.ys.len() - 1];
            if let Some(halves) = self.bulb(end, y1, dir, hash64(key, &[key::BULB, q as u64])) {
                feats.push(stub);
                feats.extend(halves);
                culs.push(q);
            }
        }
        // The loop, split at every attachment and at thirds (so it never closes on one
        // node).
        let mut splits: Vec<usize> = attach.clone();
        splits.extend_from_slice(&culs);
        splits.extend_from_slice(&[0, n / 3, 2 * n / 3]);
        splits.sort_unstable();
        splits.dedup();
        let piece = |vs: &[usize], tag: u64| Feature {
            key: hash64(key, &[key::PART, tag, vs[0] as u64]),
            kind: Kind::Local,
            pts: vs.iter().map(|&v| pod.pts[v]).collect(),
            ys: vs.iter().map(|&v| pod.ys[v]).collect(),
            ends: [End::Free, End::Free],
        };
        if pod.full {
            for s in 0..splits.len() {
                let (a, b) = (splits[s], splits[(s + 1) % splits.len()]);
                let mut vs = vec![a];
                let mut q = a;
                while q != b || vs.len() == 1 {
                    q = (q + 1) % n;
                    vs.push(q);
                }
                feats.push(piece(&vs, 4));
            }
            return;
        }
        // Open loop: the arcs, split at attachments, with bulbs at their ends.
        for arc in arcs {
            let mut cut = vec![0usize];
            for (k, &v) in arc.iter().enumerate() {
                if k > 0 && k + 1 < arc.len() && splits.binary_search(&v).is_ok() {
                    cut.push(k);
                }
            }
            cut.push(arc.len() - 1);
            for w in cut.windows(2) {
                feats.push(piece(&arc[w[0]..=w[1]], 5));
            }
            for (end, prev) in [(arc[0], arc[1]), (arc[arc.len() - 1], arc[arc.len() - 2])] {
                let dir = (pod.pts[end] - pod.pts[prev]).normalize();
                let bk = hash64(key, &[key::BULB, end as u64]);
                if let Some(h) = self.bulb(pod.pts[end], pod.ys[end], dir, bk) {
                    feats.extend(h);
                }
            }
        }
    }

    /// Whether a bulb's centerline point `p` keeps the bulb's street and sidewalk off every
    /// arterial's (the highway's included): the two half widths, both sidewalks and a
    /// meter apart.
    fn bulb_clear(&self, p: V2) -> bool {
        let own = 0.5 * LOCAL_WIDTH_M + SIDEWALK_WIDTH_M + 1.0;
        let r = own + 0.5 * HIGHWAY_WIDTH_M + SIDEWALK_WIDTH_M;
        if self.adist_at(p) > r + RASTER_M * 1.5 {
            return true;
        }
        let (i0, i1, j0, j1) = self.index.range(p.x - r, p.y - r, p.x + r, p.y + r);
        for j in j0..=j1 {
            for i in i0..=i1 {
                for &[e, s] in self.index.cell(i, j) {
                    let edge = &self.art.edges[e as usize];
                    let side = if edge.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
                    let (a, b) = self.seg(e, s);
                    let (d, _) = segment_distance(p, V2::new(a[0], a[2]), V2::new(b[0], b[2]));
                    if d < own + 0.5 * edge.width_m + side {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// A cul-de-sac bulb at `n1` (road height `y1`) heading along `dir`: two half circles
    /// of [`BULB_RADIUS_M`] from `n1` to the far point, or `None` where it does not fit.
    fn bulb(&self, n1: V2, y1: f32, dir: V2, key: u64) -> Option<[Feature; 2]> {
        let c = n1 + dir * BULB_RADIUS_M;
        let n2 = n1 + dir * (2.0 * BULB_RADIUS_M);
        let rise = 0.11 * PI * BULB_RADIUS_M;
        let y2 = clamp(self.hgt(n2), y1 - rise, y1 + rise);
        let mut out: Vec<Feature> = Vec::with_capacity(2);
        for (h, side) in [1.0f32, -1.0].iter().enumerate() {
            let mut pts = vec![n1];
            for s in 1..5 {
                let (sn, cs) = sin_cos(PI * s as f32 / 5.0);
                pts.push(c - dir * (cs * BULB_RADIUS_M) + dir.left() * (side * sn * BULB_RADIUS_M));
            }
            pts.push(n2);
            if self.adist_at(pts[2]) < 10.0 || !pts[1..].iter().all(|&p| self.bulb_clear(p)) {
                return None;
            }
            let f = self.make(
                hash64(key, &[h as u64]),
                Kind::Local,
                pts,
                [End::Free, End::Free],
                Some(y1),
                Some(y2),
            )?;
            out.push(f);
        }
        let h1 = out.pop()?;
        let h0 = out.pop()?;
        Some([h0, h1])
    }

    // -- Hill roads --------------------------------------------------------------------------

    /// Traces every hill road (part of the plan; they are few and long, so each is traced
    /// once from its canonical start and clipped per chunk): one per 300 m slot of every
    /// arterial with hills within 120 m beside it, plus one uphill branch from the middle
    /// of each road longer than 200 m, all ending in bulbs.
    fn trace_all(&mut self) {
        let mut out: Vec<Feature> = Vec::new();
        for e in 0..self.art.edges.len() {
            if !self.joinable[e] {
                continue;
            }
            let edge: Vec<V2> = self.art.edges[e]
                .profile
                .iter()
                .map(|p| V2::new(p[0], p[2]))
                .collect();
            let len = polyline_length(&edge);
            let mut s = 0.5 * TRACE_SPACING_M;
            let mut slot = 0u64;
            while s < len - 40.0 {
                let (seg, u, p, tg) = self.at_arc(e as u32, s);
                let tkey = hash64(self.seed.0, &[stage::STREETS, key::TRACE, e as u64, slot]);
                s += TRACE_SPACING_M;
                slot += 1;
                let first = if unit(tkey, 0) < 0.5 { 1.0 } else { -1.0 };
                for sg in [first, -first] {
                    let nn = tg.left() * sg;
                    let Some(reach) = (1..=5).map(|k| 24.0 * k as f32).find(|&d| {
                        let probe = p + nn * d;
                        self.pattern_at(probe) == P_SWITCH && self.land(probe)
                    }) else {
                        continue;
                    };
                    let sn = self.snap_at(e as u32, seg, u);
                    if self.joins(&sn) {
                        self.hill_road(
                            tkey,
                            sn.pos,
                            nn,
                            reach + 24.0,
                            &edge,
                            End::Snap(sn),
                            None,
                            &mut out,
                        );
                    }
                    break;
                }
            }
        }
        self.traces = out.into_iter().map(|f| (bbox(&f.pts), f)).collect();
    }

    /// One traced road from `start` (with its bulb and, if long, its branch) into `out`.
    #[allow(clippy::too_many_arguments)]
    fn hill_road(
        &self,
        tkey: u64,
        start: V2,
        nn: V2,
        approach: f32,
        parent: &[V2],
        end: End,
        y0: Option<f32>,
        out: &mut Vec<Feature>,
    ) {
        let Some(pts) = self.trace(tkey, start, nn, approach, parent) else {
            return;
        };
        let last = pts[pts.len() - 1];
        let dir = (last - pts[pts.len() - 2]).normalize();
        let Some(road) = self.make(tkey, Kind::Local, pts, [end, End::Free], y0, None) else {
            return;
        };
        let y = road.ys[road.ys.len() - 1];
        if let Some(h) = self.bulb(last, y, dir, hash64(tkey, &[key::BULB])) {
            out.extend(h);
        }
        let n = road.pts.len();
        if y0.is_none() && polyline_length(&road.pts) >= 200.0 {
            let m = n / 2;
            let (p, d) = (road.pts[m], (road.pts[m + 1] - road.pts[m - 1]).normalize());
            let l = d.left();
            let side = if self.hgt(p + l * 8.0) > self.hgt(p - l * 8.0) {
                l
            } else {
                -l
            };
            let bkey = hash64(tkey, &[key::BRANCH]);
            let ym = road.ys[m];
            let main = road.pts.clone();
            out.push(road);
            self.hill_road(bkey, p, side, 0.0, &main, End::Free, Some(ym), out);
        } else {
            out.push(road);
        }
    }

    /// The hill roads reaching `bx`.
    fn trace_features(&self, bx: &Bx, feats: &mut Vec<Feature>) {
        for ((lo, hi), f) in &self.traces {
            if bx.hits(*lo, *hi) {
                feats.push(f.clone());
            }
        }
    }

    /// Traces a hill road from `start` leaving `parent` (the arterial or road it starts on)
    /// on the `nn` side, the hills within `approach`: 8 m steps along the contours,
    /// climbing gently, turning at most 30° per step (up to 90° where boxed in, or on the
    /// first step), with up to two hairpins. Stretches of up to [`TRACE_OUTSIDE`] steps
    /// outside the hills are allowed; it ends in the hills.
    fn trace(&self, tkey: u64, start: V2, nn: V2, approach: f32, parent: &[V2]) -> Option<Vec<V2>> {
        let target = lerp(220.0, 520.0, unit(tkey, 1));
        let off_parent = |q: V2| {
            let mut d = f32::INFINITY;
            for w in parent.windows(2) {
                d = d.min(segment_distance(q, w[0], w[1]).0);
            }
            d
        };
        let far_from = |pts: &[V2], q: V2, skip: usize| {
            pts.len() <= skip
                || pts[..pts.len() - skip]
                    .iter()
                    .all(|&p| (p - q).length() >= CLEAR_M)
        };
        // A step is dry, off parks and the highway, and moves away from the start until
        // clear of its parent and all arterials.
        let valid = |q: V2, len: f32| {
            let away = 0.45 * (len + TRACE_STEP_M);
            let zone = self.m.zone_at(q.x, q.y);
            self.inside(q, 16.0)
                && self.land(q)
                && !matches!(zone, ZoneId::Highway | ZoneId::Park | ZoneId::Beach)
                && off_parent(q) >= away.min(CLEAR_M)
                && (len < 40.0 || self.adist_at(q) >= away.min(CLEAR_M))
        };
        let hills = |q: V2| self.pattern_at(q) == P_SWITCH;
        let mut pts = vec![start];
        let mut pos = start;
        let mut dir = nn;
        let mut len = 0.0f32;
        let mut hairpins = 0;
        // Steps since the trace was last in the hills, and its length then.
        let mut outside = 0usize;
        let mut last_in = 0usize;
        while len < target {
            let h0 = self.hgt(pos);
            let mut best: Option<(f32, V2)> = None;
            let tries: [&[(f32, f32)]; 2] = if pts.len() == 1 {
                [&STEER, &WIDE[..4]]
            } else {
                [&STEER, &WIDE]
            };
            for turns in tries {
                for (k, &(c, s)) in turns.iter().enumerate() {
                    let cand = rotate(dir, c, s);
                    let q = pos + cand * TRACE_STEP_M;
                    let grade = (self.hgt(q) - h0) / TRACE_STEP_M;
                    if abs(grade) > TRACE_GRADE {
                        continue;
                    }
                    let away = if hills(q) || len < approach {
                        0.0
                    } else {
                        0.05
                    };
                    let cost = abs(grade - TRACE_CLIMB) + 0.002 * k.div_ceil(2) as f32 + away;
                    if best.is_some_and(|b| b.0 <= cost) || !valid(q, len) || !far_from(&pts, q, 6)
                    {
                        continue;
                    }
                    best = Some((cost, cand));
                }
                if best.is_some() {
                    break;
                }
            }
            if let Some((_, cand)) = best {
                pos = pos + cand * TRACE_STEP_M;
                dir = cand;
                pts.push(pos);
                len += TRACE_STEP_M;
                if hills(pos) {
                    outside = 0;
                    last_in = pts.len();
                } else if len >= approach {
                    outside += 1;
                    if outside > TRACE_OUTSIDE {
                        break;
                    }
                }
                continue;
            }
            if hairpins >= 2 || len < 60.0 {
                break;
            }
            // Hairpin toward the uphill side: six 30° steps (radius ~15 m).
            let l = dir.left();
            let up = self.hgt(pos + l * 8.0) > self.hgt(pos - l * 8.0);
            let s = if up { 0.5 } else { -0.5 };
            let mut d2 = dir;
            let mut p2 = pos;
            let mut turn: Vec<V2> = Vec::new();
            let mut fine = true;
            for _ in 0..6 {
                d2 = rotate(d2, 0.866_025, s);
                let q = p2 + d2 * TRACE_STEP_M;
                if !valid(q, len)
                    || !hills(q)
                    || abs(self.hgt(q) - self.hgt(p2)) > TRACE_GRADE * TRACE_STEP_M
                {
                    fine = false;
                    break;
                }
                turn.push(q);
                p2 = q;
            }
            if !fine || !far_from(&pts, p2, 10) {
                break;
            }
            pts.extend_from_slice(&turn);
            pos = p2;
            dir = d2;
            len += 6.0 * TRACE_STEP_M;
            hairpins += 1;
            last_in = pts.len();
        }
        // End in the hills.
        pts.truncate(last_in.max(1));
        (polyline_length(&pts) >= 100.0).then_some(pts)
    }

    // -- Beach -----------------------------------------------------------------------------

    /// Parking aisles off the beachfront road and the pier road.
    fn beach_features(&self, bx: &Bx, feats: &mut Vec<Feature>) {
        let reach = bx.grow(AISLE_LENGTH_M + 40.0);
        for (e, edge) in self.art.edges.iter().enumerate() {
            if edge.class != RoadClass::Beachfront {
                continue;
            }
            let (lo, hi) = self.ebox[e];
            if !reach.hits(lo, hi) {
                continue;
            }
            let len = self.edge_length(e);
            let mut s = 0.5 * AISLE_SPACING_M;
            let mut k = 0u64;
            while s + AISLE_LENGTH_M < len - 10.0 {
                let (slot, at) = (k, s);
                s += AISLE_SPACING_M;
                k += 1;
                let (sa, ua, pa, _) = self.at_arc(e as u32, at);
                let (sb, ub, pb, _) = self.at_arc(e as u32, at + AISLE_LENGTH_M);
                let (lo, hi) = bbox(&[pa, pb]);
                if !bx.grow(AISLE_DEPTH_M + 4.0).hits(lo, hi) {
                    continue;
                }
                let tg = (pb - pa).normalize();
                let mut nn = tg.left();
                let probe = pa + nn * 20.0;
                if self.m.coast_distance(probe.x, probe.y) > self.m.coast_distance(pa.x, pa.y) {
                    nn = -nn;
                }
                let side = pa.lerp(pb, 0.5) + nn * 14.0;
                if self.m.zone_at(side.x, side.y) != ZoneId::Beach {
                    continue;
                }
                let (ja, jb) = (
                    self.snap_at(e as u32, sa, ua),
                    self.snap_at(e as u32, sb, ub),
                );
                if ja.same(&jb) || !self.joins(&ja) || !self.joins(&jb) {
                    continue;
                }
                let (a, b) = (ja.pos, jb.pos);
                let corners = [
                    a,
                    a + nn * 12.0 + tg * 3.0,
                    a + nn * AISLE_DEPTH_M + tg * 10.0,
                    b + nn * AISLE_DEPTH_M - tg * 10.0,
                    b + nn * 12.0 - tg * 3.0,
                    b,
                ];
                feats.extend(self.make(
                    hash64(self.seed.0, &[stage::STREETS, key::AISLE, e as u64, slot]),
                    Kind::Alley,
                    densify_path(&corners, SPLINE_STEP_M),
                    [End::Snap(ja), End::Snap(jb)],
                    None,
                    None,
                ));
            }
        }
        for (pi, poi) in self.m.pois.iter().enumerate() {
            if poi.kind != MacroPoiKind::Pier {
                continue;
            }
            let pkey = hash64(self.seed.0, &[stage::STREETS, key::PIER, pi as u64]);
            let facing = V2::from_array(poi.facing);
            let end = V2::from_array(poi.pos) - facing * (22.0 + 2.0 * BULB_RADIUS_M);
            if !bx.grow(CONNECT_M + 30.0).owns(end) || !self.land(end) {
                continue;
            }
            let Some((d, sn)) = self.nearest_join(end, CONNECT_M) else {
                continue;
            };
            if d < STUB_M || self.crosses_art(end, sn.pos, true) {
                continue;
            }
            let dir = (end - sn.pos).normalize();
            let pts = densify(sn.pos, end, PROFILE_STEP_M);
            let Some(road) = self.make(
                pkey,
                Kind::Local,
                pts,
                [End::Snap(sn), End::Free],
                None,
                None,
            ) else {
                continue;
            };
            let y = road.ys[road.ys.len() - 1];
            feats.push(road);
            if let Some(h) = self.bulb(end, y, dir, hash64(pkey, &[key::BULB])) {
                feats.extend(h);
            }
        }
    }

    // -- Chunks ----------------------------------------------------------------------------

    /// The local streets of `chunk`.
    pub fn chunk(&self, chunk: [i32; 2]) -> StreetChunk {
        let bx = Bx::of(chunk);
        let mut feats: Vec<Feature> = Vec::new();
        let mut farms: Vec<[f32; 2]> = Vec::new();
        let mut cache = Cache { lines: Vec::new() };
        // Lattices of the components with cells near the chunk (runs stay within 36 m of
        // their component, alleys within 70 m of their block's center), and the section
        // lattice where farmland is within a driveway's reach.
        let g = bx.grow(LATTICE_REACH_M);
        let (ci0, ci1) = (
            clamp(floor(g.x0 / RASTER_M), 0.0, (self.rw - 1) as f32) as usize,
            clamp(floor(g.x1 / RASTER_M), 0.0, (self.rw - 1) as f32) as usize,
        );
        let (cj0, cj1) = (
            clamp(floor(g.z0 / RASTER_M), 0.0, (self.rh - 1) as f32) as usize,
            clamp(floor(g.z1 / RASTER_M), 0.0, (self.rh - 1) as f32) as usize,
        );
        let mut ids: Vec<u32> = Vec::new();
        for j in cj0..=cj1 {
            for i in ci0..=ci1 {
                let l = self.label[j * self.rw + i];
                if l != NONE && self.comps[l as usize].lat.is_some() {
                    ids.push(l);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        if let (Some(_), Some((lo, hi))) = (&self.sections, self.farm_box) {
            if bx.grow(CONNECT_M + 2.0 * RASTER_M).hits(lo, hi) {
                ids.push(SECTIONS_ID);
            }
        }
        for id in ids {
            self.lattice_features(&mut cache, id, &bx, &mut feats, &mut farms);
        }
        self.pod_features(&bx, &mut feats);
        self.trace_features(&bx, &mut feats);
        self.beach_features(&bx, &mut feats);
        self.assemble(chunk, &bx, &feats, farms)
    }

    fn border_key(&self, c: [i32; 2], code: u8, fkey: u64, seg: usize) -> u64 {
        let (mx, mz, axis) = match code {
            1 => (c[0] - 1, c[1], 0),
            2 => (c[0], c[1], 0),
            3 => (c[0], c[1] - 1, 1),
            _ => (c[0], c[1], 1),
        };
        hash64(
            self.seed.0,
            &[
                stage::STREETS,
                word(mx),
                word(mz),
                axis,
                hash64(fkey, &[seg as u64]),
            ],
        )
    }

    /// Clips a feature to the chunk (owned half-open, so a border point belongs to both
    /// sides as an end and to neither as a stretch).
    fn clip(&self, c: [i32; 2], bx: &Bx, f: &Feature, out: &mut Vec<Piece>) {
        let n = f.pts.len();
        if n < 2 {
            return;
        }
        let (lo, hi) = bbox(&f.pts);
        if !bx.hits(lo, hi) {
            return;
        }
        let hts = &f.ys;
        let end_of = |e: End| match e {
            End::Free => PEnd::Free,
            End::Snap(s) => PEnd::Snap(s),
        };
        let mut cur: Option<Piece> = None;
        for i in 0..n - 1 {
            let (p, q) = (f.pts[i], f.pts[i + 1]);
            let mut bp = [(0.0f32, p, 0u8); 6];
            let mut nb = 1;
            let (dx, dz) = (q.x - p.x, q.y - p.y);
            for (code, x) in [(1u8, bx.x0), (2, bx.x1)] {
                if dx != 0.0 {
                    let t = (x - p.x) / dx;
                    if t > 0.0 && t < 1.0 {
                        let z = p.y + dz * t;
                        if z >= bx.z0 && z <= bx.z1 {
                            bp[nb] = (t, V2::new(x, z), code);
                            nb += 1;
                        }
                    }
                }
            }
            for (code, z) in [(3u8, bx.z0), (4, bx.z1)] {
                if dz != 0.0 {
                    let t = (z - p.y) / dz;
                    if t > 0.0 && t < 1.0 {
                        let x = p.x + dx * t;
                        if x >= bx.x0 && x <= bx.x1 {
                            bp[nb] = (t, V2::new(x, z), code);
                            nb += 1;
                        }
                    }
                }
            }
            bp[nb] = (1.0, q, 0);
            nb += 1;
            for a in 1..nb {
                let mut b = a;
                while b > 0 && bp[b].0 < bp[b - 1].0 {
                    bp.swap(b, b - 1);
                    b -= 1;
                }
            }
            for k in 0..nb - 1 {
                let (ta, pa, ca) = bp[k];
                let (tb, pb, _) = bp[k + 1];
                if tb <= ta {
                    continue;
                }
                let inside = bx.owns(pa.lerp(pb, 0.5));
                let ya = lerp(hts[i], hts[i + 1], ta);
                let yb = lerp(hts[i], hts[i + 1], tb);
                let border = |code: u8, at: V2| {
                    let code = if code != 0 { code } else { bx.code_of(at) };
                    if code == 0 {
                        PEnd::Free
                    } else {
                        PEnd::Border(self.border_key(c, code, f.key, i))
                    }
                };
                if inside {
                    let pc = cur.get_or_insert_with(|| {
                        let start = if i == 0 && ta == 0.0 {
                            end_of(f.ends[0])
                        } else {
                            border(ca, pa)
                        };
                        Piece {
                            kind: f.kind,
                            pts: vec![pa],
                            ys: vec![ya],
                            ends: [start, PEnd::Free],
                        }
                    });
                    pc.pts.push(pb);
                    pc.ys.push(yb);
                } else if let Some(mut pc) = cur.take() {
                    pc.ends[1] = border(ca, pa);
                    out.push(pc);
                }
            }
        }
        if let Some(mut pc) = cur.take() {
            pc.ends[1] = end_of(f.ends[1]);
            out.push(pc);
        }
    }

    /// Clips the features, nodes them (crossings, T-junctions), merges close nodes,
    /// removes stubs and builds the profiles.
    fn assemble(
        &self,
        c: [i32; 2],
        bx: &Bx,
        feats: &[Feature],
        farmsteads: Vec<[f32; 2]>,
    ) -> StreetChunk {
        let mut pieces: Vec<Piece> = Vec::new();
        for f in feats {
            self.clip(c, bx, f, &mut pieces);
        }
        let mut nodes: Vec<BNode> = Vec::new();
        let mut edges: Vec<BEdge> = Vec::new();
        for p in pieces {
            if p.pts.len() < 2 || polyline_length(&p.pts) < 1e-3 {
                continue;
            }
            let last = p.pts.len() - 1;
            let a = add_node(&mut nodes, p.pts[0], p.ys[0], p.ends[0]);
            let b = add_node(&mut nodes, p.pts[last], p.ys[last], p.ends[1]);
            if a == b {
                continue;
            }
            edges.push(BEdge {
                a,
                b,
                kind: p.kind,
                pts: p.pts,
                ys: p.ys,
            });
        }

        // Crossings between edges.
        let mut splits: Vec<Vec<(u32, f32, u32)>> = vec![Vec::new(); edges.len()];
        let boxes: Vec<(V2, V2)> = edges.iter().map(|e| bbox(&e.pts)).collect();
        for i in 0..edges.len() {
            for j in i + 1..edges.len() {
                let ((la, ha), (lb, hb)) = (boxes[i], boxes[j]);
                if ha.x < lb.x || hb.x < la.x || ha.y < lb.y || hb.y < la.y {
                    continue;
                }
                let ends = [edges[i].a, edges[i].b, edges[j].a, edges[j].b];
                let mut found: Vec<V2> = Vec::new();
                for si in 0..edges[i].pts.len() - 1 {
                    for sj in 0..edges[j].pts.len() - 1 {
                        let (p, q) = (edges[i].pts[si], edges[i].pts[si + 1]);
                        let (r, s) = (edges[j].pts[sj], edges[j].pts[sj + 1]);
                        let Some((t, u)) = seg_intersect(p, q, r, s) else {
                            continue;
                        };
                        let x = p.lerp(q, t);
                        let yi = lerp(edges[i].ys[si], edges[i].ys[si + 1], t);
                        let yj = lerp(edges[j].ys[sj], edges[j].ys[sj + 1], u);
                        // Streets crossing at clearly different heights (on steep ground)
                        // pass each other rather than meet.
                        if abs(yi - yj) > CROSS_STEP_M
                            || ends
                                .iter()
                                .any(|&nd| (nodes[nd as usize].pos - x).length() < 0.5)
                            || found.iter().any(|&f| (f - x).length() < 0.5)
                        {
                            continue;
                        }
                        // The heights the border ends of both streets allow here (each a
                        // fixed height some way along its street).
                        let (mut lo, mut hi) = (f32::NEG_INFINITY, f32::INFINITY);
                        for (e, seg, f) in [(&edges[i], si, t), (&edges[j], sj, u)] {
                            let arc = arc_to(&e.pts, seg, f);
                            let total = polyline_length(&e.pts);
                            for (end, d) in [(e.a, arc), (e.b, total - arc)] {
                                let nd = &nodes[end as usize];
                                if matches!(nd.pin, Pin::Border(_)) {
                                    lo = lo.max(nd.y - RELAX_GRADE * d);
                                    hi = hi.min(nd.y + RELAX_GRADE * d);
                                }
                            }
                        }
                        // None does: they pass each other.
                        if lo > hi {
                            continue;
                        }
                        found.push(x);
                        let id = nodes.len() as u32;
                        nodes.push(BNode {
                            pos: x,
                            y: clamp(0.5 * (yi + yj), lo, hi),
                            pin: Pin::Inner,
                        });
                        splits[i].push((si as u32, t, id));
                        splits[j].push((sj as u32, u, id));
                    }
                }
            }
        }
        // Nodes next to a border node never move, so the stretch up to the border keeps the
        // feature's line on both sides.
        let mut fixed = vec![false; nodes.len()];
        for e in &edges {
            for (x, y) in [(e.a, e.b), (e.b, e.a)] {
                if matches!(nodes[y as usize].pin, Pin::Border(_)) {
                    fixed[x as usize] = true;
                }
            }
        }
        // T-junctions: dead ends within 1 m of another edge join it.
        let mut deg = vec![0u32; nodes.len()];
        for e in &edges {
            deg[e.a as usize] += 1;
            deg[e.b as usize] += 1;
        }
        for nd in 0..nodes.len() {
            if nodes[nd].pin != Pin::Inner || deg[nd] != 1 {
                continue;
            }
            let p = nodes[nd].pos;
            // Next to a border node only exact meetings (lattice ends) join.
            let reach = if fixed[nd] { 0.05 } else { 1.0 };
            let mut best: Option<(f32, usize, u32, f32)> = None;
            for (ei, e) in edges.iter().enumerate() {
                if e.a as usize == nd || e.b as usize == nd {
                    continue;
                }
                let (lo, hi) = boxes[ei];
                if p.x < lo.x - 1.0 || p.x > hi.x + 1.0 || p.y < lo.y - 1.0 || p.y > hi.y + 1.0 {
                    continue;
                }
                for s in 0..e.pts.len() - 1 {
                    let (d, t) = segment_distance(p, e.pts[s], e.pts[s + 1]);
                    if d < reach && best.is_none_or(|b| d < b.0) {
                        best = Some((d, ei, s as u32, t));
                    }
                }
            }
            if let Some((_, ei, s, t)) = best {
                let e = &edges[ei];
                let near_end = [e.a, e.b]
                    .iter()
                    .any(|&x| (nodes[x as usize].pos - p).length() < 1.0);
                if !near_end {
                    // The dead end moves (under 1 m) onto the street it meets, which stays
                    // straight.
                    let (a, b) = (e.pts[s as usize], e.pts[s as usize + 1]);
                    nodes[nd].pos = a.lerp(b, t);
                    nodes[nd].y = lerp(e.ys[s as usize], e.ys[s as usize + 1], t);
                    splits[ei].push((s, t, nd as u32));
                }
            }
        }
        let mut cut: Vec<BEdge> = Vec::new();
        for (ei, e) in edges.into_iter().enumerate() {
            let mut sp = core::mem::take(&mut splits[ei]);
            if sp.is_empty() {
                cut.push(e);
                continue;
            }
            sp.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2)));
            let mut a = e.a;
            let mut pts = vec![e.pts[0]];
            let mut ys = vec![e.ys[0]];
            let mut k = 0;
            for s in 0..e.pts.len() - 1 {
                while k < sp.len() && sp[k].0 as usize == s {
                    let nd = sp[k].2;
                    k += 1;
                    if nd == a {
                        continue;
                    }
                    pts.push(nodes[nd as usize].pos);
                    ys.push(nodes[nd as usize].y);
                    cut.push(BEdge {
                        a,
                        b: nd,
                        kind: e.kind,
                        pts: core::mem::take(&mut pts),
                        ys: core::mem::take(&mut ys),
                    });
                    a = nd;
                    pts.push(nodes[nd as usize].pos);
                    ys.push(nodes[nd as usize].y);
                }
                pts.push(e.pts[s + 1]);
                ys.push(e.ys[s + 1]);
            }
            if a != e.b {
                cut.push(BEdge {
                    a,
                    b: e.b,
                    kind: e.kind,
                    pts,
                    ys,
                });
            }
        }
        let mut edges = cut;

        // Merge nodes within MERGE_M.
        let nn = nodes.len();
        let mut parent: Vec<u32> = (0..nn as u32).collect();
        fn find(parent: &mut [u32], mut i: u32) -> u32 {
            while parent[i as usize] != i {
                let up = parent[parent[i as usize] as usize];
                parent[i as usize] = up;
                i = up;
            }
            i
        }
        // Border nodes never merge; inner nodes next to them may absorb others but never
        // move; arterial points merge only with each other or absorb inner nodes.
        let mut fixed = vec![false; nn];
        for e in &edges {
            for (x, y) in [(e.a, e.b), (e.b, e.a)] {
                if matches!(nodes[y as usize].pin, Pin::Border(_)) {
                    fixed[x as usize] = true;
                }
            }
        }
        // 0 inner, 1 arterial point, 2 border.
        let kind = |i: u32| match nodes[i as usize].pin {
            Pin::Inner => 0u8,
            Pin::Snap(_) => 1,
            Pin::Border(_) => 2,
        };
        let mut order: Vec<u32> = (0..nn as u32).collect();
        order.sort_by(|&x, &y| {
            nodes[x as usize]
                .pos
                .x
                .total_cmp(&nodes[y as usize].pos.x)
                .then(x.cmp(&y))
        });
        for oi in 0..nn {
            let i = order[oi];
            for &j in &order[oi + 1..] {
                let (pi, pj) = (nodes[i as usize].pos, nodes[j as usize].pos);
                if pj.x - pi.x >= MERGE_M {
                    break;
                }
                if (pj - pi).length() >= MERGE_M {
                    continue;
                }
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri == rj {
                    continue;
                }
                let (ki, kj) = (kind(ri), kind(rj));
                if ki == 2 || kj == 2 {
                    continue;
                }
                // A movable inner node joins anything; a movable arterial point joins only
                // another arterial point.
                let (mi, mj) = (!fixed[ri as usize], !fixed[rj as usize]);
                let i_into_j = mi && (ki == 0 || kj == 1);
                let j_into_i = mj && (kj == 0 || ki == 1);
                let root = match (i_into_j, j_into_i) {
                    (true, true) if ki != kj => {
                        if ki == 1 {
                            ri
                        } else {
                            rj
                        }
                    }
                    (true, true) => ri.min(rj),
                    (true, false) => rj,
                    (false, true) => ri,
                    (false, false) => continue,
                };
                let other = if root == ri { rj } else { ri };
                parent[other as usize] = root;
            }
        }
        for e in &mut edges {
            e.a = find(&mut parent, e.a);
            e.b = find(&mut parent, e.b);
            let last = e.pts.len() - 1;
            e.pts[0] = nodes[e.a as usize].pos;
            e.ys[0] = nodes[e.a as usize].y;
            e.pts[last] = nodes[e.b as usize].pos;
            e.ys[last] = nodes[e.b as usize].y;
        }
        edges.retain(|e| e.a != e.b && polyline_length(&e.pts) > 1e-3);

        // Interior stubs.
        loop {
            let mut deg = vec![0u32; nn];
            for e in &edges {
                deg[e.a as usize] += 1;
                deg[e.b as usize] += 1;
            }
            let before = edges.len();
            edges.retain(|e| {
                let dead = |x: u32, other: u32| {
                    nodes[x as usize].pin == Pin::Inner
                        && deg[x as usize] == 1
                        && !matches!(nodes[other as usize].pin, Pin::Border(_))
                };
                !((dead(e.a, e.b) || dead(e.b, e.a)) && polyline_length(&e.pts) < STUB_M)
            });
            if edges.len() == before {
                break;
            }
        }

        // Short edges between nodes at very different heights (where features meet) cannot
        // hold the grade: pull their inner ends together, a few sweeps, within the band.
        for _ in 0..32 {
            let mut moved = false;
            for e in &edges {
                let (a, b) = (e.a as usize, e.b as usize);
                let excess = abs(nodes[a].y - nodes[b].y) - RELAX_GRADE * polyline_length(&e.pts);
                if excess <= 1e-4 {
                    continue;
                }
                let (ka, kb) = match (nodes[a].pin == Pin::Inner, nodes[b].pin == Pin::Inner) {
                    (true, true) => (0.5, 0.5),
                    (true, false) => (1.0, 0.0),
                    (false, true) => (0.0, 1.0),
                    _ => continue,
                };
                let down = if nodes[a].y > nodes[b].y { 1.0 } else { -1.0 };
                for (n, k, sgn) in [(a, ka, -down), (b, kb, down)] {
                    let g0 = self.hgt(nodes[n].pos);
                    let band = PROFILE_BAND_M - 0.1;
                    nodes[n].y = clamp(nodes[n].y + sgn * k * excess, g0 - band, g0 + band);
                }
                moved = true;
            }
            if !moved {
                break;
            }
        }
        for e in &mut edges {
            let last = e.ys.len() - 1;
            e.ys[0] = nodes[e.a as usize].y;
            e.ys[last] = nodes[e.b as usize].y;
        }

        // Output.
        let mut remap = vec![NONE; nn];
        let mut out = StreetChunk {
            chunk: c,
            farmsteads,
            ..StreetChunk::default()
        };
        for e in &edges {
            for x in [e.a, e.b] {
                if remap[x as usize] == NONE {
                    remap[x as usize] = out.nodes.len() as u32;
                    let nd = &nodes[x as usize];
                    out.nodes.push(Node {
                        pos: [nd.pos.x, nd.pos.y],
                        elevation: nd.y,
                    });
                    out.refs.push(match nd.pin {
                        Pin::Inner => NodeRef::Inner,
                        Pin::Border(key) => NodeRef::Border { key },
                        Pin::Snap(s) if s.node != NONE => NodeRef::ArterialNode { node: s.node },
                        Pin::Snap(s) => NodeRef::Arterial {
                            edge: s.edge,
                            seg: s.seg,
                            u: s.u,
                        },
                    });
                }
            }
        }
        for e in edges {
            let mut s = Vec::with_capacity(e.pts.len());
            let mut acc = 0.0;
            for (k, &p) in e.pts.iter().enumerate() {
                if k > 0 {
                    acc += (p - e.pts[k - 1]).length();
                }
                s.push(acc);
            }
            let ground: Vec<f32> = e.pts.iter().map(|&p| self.hgt(p)).collect();
            let last = e.ys.len() - 1;
            let ys = grade_limit(
                &e.ys,
                &s,
                FILTER_GRADE,
                Some(e.ys[0]),
                Some(e.ys[last]),
                Some((&ground, PROFILE_BAND_M)),
            );
            let (class, width_m, sidewalk, speed_kph) = match e.kind {
                Kind::Local => (RoadClass::Local, LOCAL_WIDTH_M, true, LOCAL_KPH),
                Kind::Industrial => (RoadClass::Local, INDUSTRIAL_WIDTH_M, true, LOCAL_KPH),
                Kind::Alley => (RoadClass::Alley, ALLEY_WIDTH_M, false, ALLEY_KPH),
                Kind::Driveway => (RoadClass::Alley, DRIVEWAY_WIDTH_M, false, ALLEY_KPH),
                Kind::Section => (RoadClass::Rural, SECTION_WIDTH_M, false, SECTION_KPH),
            };
            out.edges.push(Edge {
                a: remap[e.a as usize],
                b: remap[e.b as usize],
                class,
                lanes_fwd: 1,
                lanes_back: 1,
                width_m,
                sidewalk,
                speed_kph,
                oneway: false,
                profile: e.pts.iter().zip(&ys).map(|(p, &y)| [p.x, y, p.y]).collect(),
            });
        }
        out
    }

    // -- Merging -----------------------------------------------------------------------------

    /// Every chunk's streets merged with the arterials: arterial edges are split where
    /// streets join them and at chunk borders, border nodes are de-duplicated by key.
    pub fn merged(&self) -> RoadGraph {
        let [sx, sz] = self.m.size_chunks;
        let chunks: Vec<StreetChunk> = (0..i32::from(sz))
            .flat_map(|z| (0..i32::from(sx)).map(move |x| [x, z]))
            .map(|c| self.chunk(c))
            .collect();
        self.merge(&chunks)
    }

    /// Merges chunk outputs (any subset, any order of generation, listed in chunk order)
    /// with the arterials.
    pub fn merge(&self, chunks: &[StreetChunk]) -> RoadGraph {
        let art = self.art;
        let mut nodes: Vec<Node> = art.nodes.clone();
        // Arterial split points per edge: (segment, u, node).
        let mut asplit: Vec<Vec<(u32, f32, u32)>> = vec![Vec::new(); art.edges.len()];
        let mut border: Vec<(u64, u32)> = Vec::new();
        let mut maps: Vec<Vec<u32>> = Vec::with_capacity(chunks.len());
        for ch in chunks {
            for r in &ch.refs {
                if let NodeRef::Border { key } = *r {
                    border.push((key, NONE));
                }
            }
        }
        border.sort_unstable();
        border.dedup_by_key(|b| b.0);
        for ch in chunks {
            let mut map = Vec::with_capacity(ch.nodes.len());
            for (n, r) in ch.nodes.iter().zip(&ch.refs) {
                let id = match *r {
                    NodeRef::Inner => {
                        nodes.push(*n);
                        nodes.len() as u32 - 1
                    }
                    NodeRef::Border { key } => {
                        let k = border
                            .binary_search_by_key(&key, |b| b.0)
                            .expect("border key");
                        if border[k].1 == NONE {
                            nodes.push(*n);
                            border[k].1 = nodes.len() as u32 - 1;
                        }
                        debug_assert_eq!(nodes[border[k].1 as usize].pos, n.pos);
                        border[k].1
                    }
                    NodeRef::ArterialNode { node } => node,
                    NodeRef::Arterial { edge, seg, u } => {
                        let list = &mut asplit[edge as usize];
                        match list
                            .iter()
                            .find(|s| s.0 == seg && s.1.to_bits() == u.to_bits())
                        {
                            Some(s) => s.2,
                            None => {
                                nodes.push(*n);
                                let id = nodes.len() as u32 - 1;
                                list.push((seg, u, id));
                                id
                            }
                        }
                    }
                };
                map.push(id);
            }
            maps.push(map);
        }
        // Chunk borders split the arterials too.
        let lines_x: Vec<f32> = (1..i32::from(self.m.size_chunks[0]))
            .map(|k| k as f32 * CHUNK_M)
            .collect();
        let lines_z: Vec<f32> = (1..i32::from(self.m.size_chunks[1]))
            .map(|k| k as f32 * CHUNK_M)
            .collect();
        for (e, edge) in art.edges.iter().enumerate() {
            for (s, w) in edge.profile.windows(2).enumerate() {
                let (a, b) = (w[0], w[1]);
                for (lines, ax, oth) in [(&lines_x, 0usize, 2usize), (&lines_z, 2, 0)] {
                    for &x in lines.iter() {
                        if (a[ax] < x) == (b[ax] < x) || a[ax] == x || b[ax] == x {
                            continue;
                        }
                        let u = (x - a[ax]) / (b[ax] - a[ax]);
                        let mut p = [0.0f32; 2];
                        p[ax / 2] = x;
                        p[oth / 2] = a[oth] + (b[oth] - a[oth]) * u;
                        nodes.push(Node {
                            pos: p,
                            elevation: lerp(a[1], b[1], u),
                        });
                        asplit[e].push((s as u32, u, nodes.len() as u32 - 1));
                    }
                }
            }
        }
        let mut edges: Vec<Edge> = Vec::new();
        for (e, edge) in art.edges.iter().enumerate() {
            let mut sp = core::mem::take(&mut asplit[e]);
            sp.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2)));
            let mut a = edge.a;
            let mut prof = vec![edge.profile[0]];
            let mut k = 0;
            for s in 0..edge.profile.len() - 1 {
                while k < sp.len() && sp[k].0 as usize == s {
                    let nd = sp[k].2;
                    k += 1;
                    let n = nodes[nd as usize];
                    let p = [n.pos[0], n.elevation, n.pos[1]];
                    let last = prof[prof.len() - 1];
                    if nd == a || (abs(last[0] - p[0]) + abs(last[2] - p[2])) < 1e-4 {
                        continue;
                    }
                    prof.push(p);
                    let mut piece = edge.clone();
                    piece.a = a;
                    piece.b = nd;
                    piece.profile = core::mem::replace(&mut prof, vec![p]);
                    edges.push(piece);
                    a = nd;
                }
                prof.push(edge.profile[s + 1]);
            }
            if a != edge.b {
                let mut piece = edge.clone();
                piece.a = a;
                piece.profile = prof;
                edges.push(piece);
            }
        }
        for (ch, map) in chunks.iter().zip(&maps) {
            for e in &ch.edges {
                let mut e = e.clone();
                e.a = map[e.a as usize];
                e.b = map[e.b as usize];
                if e.a != e.b {
                    edges.push(e);
                }
            }
        }
        RoadGraph { nodes, edges }
    }
}

// ---------------------------------------------------------------------------------------
// Chunk-local working types.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Bound {
    /// The playable rectangle's edge (the foot of an edge wall, or the place edge on water).
    Edge,
    /// An arterial the street may not join.
    Hard,
    Soft(Snap),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum REnd {
    Dead,
    Snap(Snap),
    /// At a lattice point, on the cross street.
    Joint,
}

#[derive(Clone, Copy, Debug)]
struct Run {
    t0: f32,
    t1: f32,
    e0: REnd,
    e1: REnd,
}

struct LineData {
    id: (u32, u8, i32),
    base: V2,
    dir: V2,
    bounds: Vec<(f32, Bound)>,
    runs: Vec<Option<Vec<Run>>>,
}

struct Cache {
    lines: Vec<LineData>,
}

/// Where a suburb pod goes (part of the plan): a stadium of half-width `b` around the
/// core segment `center ± axis h`.
struct PodSpec {
    key: u64,
    center: V2,
    comp: u32,
    axis: V2,
    h: f32,
    b: f32,
}

impl PodSpec {
    fn core0(&self) -> V2 {
        self.center - self.axis * self.h
    }

    fn core1(&self) -> V2 {
        self.center + self.axis * self.h
    }

    /// How far its streets reach from the core: the loop's half-width, or a spine's
    /// cul-de-sacs.
    fn room(&self) -> f32 {
        if self.b == 0.0 {
            SPINE_HALF_M
        } else {
            self.b
        }
    }
}

/// A pod loop's points, heights and which points fit.
type Ring = (Vec<V2>, Vec<f32>, Vec<bool>);

struct Pod {
    idx: usize,
    key: u64,
    center: V2,
    comp: u32,
    pts: Vec<V2>,
    ys: Vec<f32>,
    /// Vertices other streets may attach to: on the loop (or an arc, not at its ends).
    usable: Vec<bool>,
    full: bool,
    /// The arcs kept of a loop that does not fit whole, as vertex lists.
    arcs: Vec<Vec<usize>>,
    /// The connector to an arterial and the vertex it leaves from.
    art: Option<(usize, Feature)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PEnd {
    Free,
    Snap(Snap),
    Border(u64),
}

struct Piece {
    kind: Kind,
    pts: Vec<V2>,
    ys: Vec<f32>,
    ends: [PEnd; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pin {
    Inner,
    Border(u64),
    Snap(Snap),
}

struct BNode {
    pos: V2,
    y: f32,
    pin: Pin,
}

struct BEdge {
    a: u32,
    b: u32,
    kind: Kind,
    pts: Vec<V2>,
    ys: Vec<f32>,
}

/// Arc length along `pts` to fraction `t` of segment `seg`.
fn arc_to(pts: &[V2], seg: usize, t: f32) -> f32 {
    let mut acc = 0.0;
    for k in 0..seg {
        acc += (pts[k + 1] - pts[k]).length();
    }
    acc + t * (pts[seg + 1] - pts[seg]).length()
}

/// The node for a piece end: border nodes by key, arterial points by position on the
/// arterial, free ends by exact position.
fn add_node(nodes: &mut Vec<BNode>, pos: V2, y: f32, end: PEnd) -> u32 {
    let pin = match end {
        PEnd::Free => Pin::Inner,
        PEnd::Snap(s) => Pin::Snap(s),
        PEnd::Border(k) => Pin::Border(k),
    };
    let found = nodes.iter().position(|n| match (n.pin, pin) {
        (Pin::Border(a), Pin::Border(b)) => a == b,
        (Pin::Snap(a), Pin::Snap(b)) => a.same(&b),
        (Pin::Inner, Pin::Inner) => {
            n.pos.x.to_bits() == pos.x.to_bits() && n.pos.y.to_bits() == pos.y.to_bits()
        }
        _ => false,
    });
    if let Some(i) = found {
        return i as u32;
    }
    nodes.push(BNode { pos, y, pin });
    nodes.len() as u32 - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn densify_keeps_ends_and_spacing() {
        let (a, b) = (V2::new(0.0, 0.0), V2::new(40.0, 0.0));
        let p = densify(a, b, 16.0);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], a);
        assert_eq!(p[3], b);
        assert!(p.windows(2).all(|w| (w[1] - w[0]).length() <= 16.0));
    }

    #[test]
    fn clip_line_finds_box_span() {
        let b = Bx::of([0, 0]);
        let (t0, t1) = clip_line(V2::new(-10.0, 64.0), V2::new(1.0, 0.0), &b).unwrap();
        assert!((t0 - 10.0).abs() < 1e-4 && (t1 - 138.0).abs() < 1e-4);
        assert!(clip_line(V2::new(-10.0, 200.0), V2::new(1.0, 0.0), &b).is_none());
    }
}
