//! Stage 3 of `docs/WORLD_GENERATION.md`: the arterial network and the highway, as a
//! [`RoadGraph`] the local streets (stage 4) hang off.
//!
//! [`arterials`] runs, in order:
//! 1. **Highway profile.** `MacroMap::highway` from the beachfront road to the inland
//!    edge, resampled every 16 m; its height follows the terrain from the coast under a 5%
//!    slope limit. The macro map already carved the corridor bed (at most 4.8%) into its
//!    heightfield, so the profile stays on the ground and the macro heights are the
//!    Place's heights as they are.
//! 2. **Tensor field** on 32 m cells: a grid aligned to the smoothed coast tangent
//!    (`Grid`/`Beachfront` zones), bent toward the downtown ring by a radial term,
//!    contour-following on slopes (`Curvy`/`Switchback`), axis-aligned on farmland
//!    (`Sections`), and nothing in `Industrial` (filled in by neighbors); blended by a
//!    normalized box blur.
//! 3. **Explicit roads:** the beachfront road along the beach band, the strip's spine and
//!    a ring road around downtown.
//! 4. **Streamlines** of the major and minor eigenvectors, traced with RK2 steps of 8 m
//!    from seeds (strip ends, downtown, farm valleys, overlooks, harbor, pass) and then from
//!    seeds along and beside every traced line, keeping a per-zone minimum spacing
//!    (250–400 m); they stop at water, the beach, the place edge, the highway (except to
//!    cross it on a shallow cut with room for an overpass), a same-family neighbor or a
//!    degenerate field. Each step takes the direction nearest the field that turns at most
//!    30° and climbs at most 10%, so on slopes the trace swings along the contours. Ends
//!    join the nearest road within 40 m (160 m ahead).
//! 5. **Interchanges:** up to 2–4 (as many as suitable) arterial crossings of the
//!    highway get diamond ramps; every other crossing is an overpass and stays
//!    unconnected.
//! 6. **Graph:** true intersections split the lines into edges; nodes within 12 m merge;
//!    dead-end stubs under 60 m go (except in `Suburb`/`Hills`); degree-2 chains of one
//!    class join; small components reconnect to the main one (straight, else by a
//!    grade-limited route) or are dropped; routed roads climb from the network to the
//!    gentler parts of the hills.
//! 7. **Profiles** every 16 m on the terrain: crossing roads rise to an overpass
//!    deck 7 m above the highway with 4% approaches (propagated through the graph), ramps
//!    rise 6 m over ~150 m, and every other road keeps a 12% min/max slope filter.
//!
//! Everything is a pure function of `(seed, template, macro map)`: plain loops in index
//! order, no hash maps, `crate::math` for square roots.

use alloc::vec;
use alloc::vec::Vec;

use place::{Edge, Node, RoadClass, RoadGraph, ZoneId};
use skywave_core::{hash_bytes, Rng, Seed};

use super::macro_map::{MacroMap, MacroPoiKind};
use super::terrain::{box_blur, bump, smoothstep};
use super::{stage, CELL_M, ZONE_CELL_M};
use crate::math::{abs, clamp, lerp, segment_distance, sin_cos, sqrt, TAU, V2};
use crate::template::{DrivingSide, RegionTemplate, StreetPattern};

/// Streamline integration step.
pub const STEP_M: f32 = 8.0;
/// Spacing of profile samples along every edge.
pub const PROFILE_STEP_M: f32 = 16.0;
/// A line end joins a road this close in any direction.
pub const SNAP_M: f32 = 40.0;
/// ... or this close straight ahead.
pub const SNAP_AHEAD_M: f32 = 160.0;
/// Nodes closer than this merge.
pub const MERGE_M: f32 = 12.0;
/// Dead ends shorter than this are removed outside `Suburb`/`Hills`.
pub const STUB_M: f32 = 60.0;
/// Steepest profile segment of any road but the ramps.
pub const MAX_GRADE: f32 = 0.12;
/// Steepest highway profile segment.
pub const HIGHWAY_GRADE: f32 = 0.05;
/// Height of an overpass road above the terrain where it crosses the highway.
pub const OVERPASS_LIFT_M: f32 = 7.0;
/// Approach grade of an overpass.
pub const APPROACH_GRADE: f32 = 0.04;
/// How far a ramp climbs from the highway, and over what length.
pub const RAMP_RISE_M: f32 = 6.0;
pub const RAMP_RUN_M: f32 = 150.0;
/// Curb-to-curb widths by class.
pub const HIGHWAY_WIDTH_M: f32 = 22.0;
pub const ARTERIAL_WIDTH_M: f32 = 14.0;
pub const COLLECTOR_WIDTH_M: f32 = 12.0;
pub const RURAL_WIDTH_M: f32 = 8.0;
pub const BEACHFRONT_WIDTH_M: f32 = 12.0;
/// A one-lane ramp with shoulders.
pub const RAMP_WIDTH_M: f32 = 8.0;

/// Tensor field cell size.
const FIELD_M: f32 = 32.0;
/// Grade the trace steers to on slopes, below [`MAX_GRADE`] to leave room for relief.
const STEER_GRADE: f32 = 0.10;
/// The profile filter's slope, a hair under [`MAX_GRADE`] for rounding.
const FILTER_GRADE: f32 = 0.118;
/// How far road profiles may leave the terrain; `place::validate` allows 8 m.
const PROFILE_BAND_M: f32 = 7.8;
/// Road length kept flat at full lift each side of a highway crossing.
const OVERPASS_FLAT_M: f32 = HIGHWAY_WIDTH_M * 0.5 + 9.0;
/// Where along a crossing road a ramp meets it, from the highway centerline: the overpass
/// lift there is `RAMP_RISE_M`.
const RAMP_FOOT_M: f32 = OVERPASS_FLAT_M + (OVERPASS_LIFT_M - RAMP_RISE_M) / APPROACH_GRADE;
/// Road a crossing needs on each side of the highway (the overpass and its approaches).
const OVERPASS_ROOM_M: f32 = 200.0;
/// No crossings (only dead ends) on the highway's first stretch from the coast.
const TERMINUS_M: f32 = 300.0;
/// Roads stop where the corridor carve moved the terrain by more than this...
const CUT_BARRIER_M: f32 = 1.0;
/// ... except that crossing roads bridge highway cuts up to this deep.
const CROSS_CUT_M: f32 = 6.0;
/// `(cos, sin)` of 0°, 10°, … 70°: the steering rotations.
const ROTATIONS: [(f32, f32); 8] = [
    (1.0, 0.0),
    (0.984_808, 0.173_648),
    (0.939_693, 0.342_02),
    (0.866_025, 0.5),
    (0.766_044, 0.642_788),
    (0.642_788, 0.766_044),
    (0.5, 0.866_025),
    (0.342_02, 0.939_693),
];
/// Largest heading change per trace step: 30°.
const MAX_TURN_COS: f32 = 0.866_025;
/// Most streamlines traced.
const MAX_LINES: usize = 220;
/// Longest streamline.
const MAX_LINE_M: f32 = 3600.0;

/// Minimum spacing between same-family streamlines, by `ZoneId` discriminant. Farmland
/// comes from the template (half its section size).
const SPACING: [f32; 12] = [
    250.0, // Downtown
    260.0, // Strip
    270.0, // Residential
    320.0, // Suburb
    300.0, // Industrial
    250.0, // Commercial
    300.0, // Park
    280.0, // Beach
    400.0, // Farmland
    400.0, // Hills
    300.0, // Highway
    400.0, // Water
];

/// What a line is; decides its class and how it joins the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Beachfront,
    Spine,
    Ring,
    Stream,
    Highway,
    Ramp,
    Connector,
}

/// Why a trace (or an explicit line) ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// At the place edge: leave as is.
    Edge,
    /// Already attached to another line.
    Fixed,
    Water,
    Near,
    Loop,
    Barrier,
    Flat,
    Turn,
    Steep,
    Long,
    Open,
}

struct Line {
    kind: Kind,
    /// 0 major, 1 minor; explicit lines take theirs per point in `fam`.
    family: u8,
    /// 0 for lines from the initial seeds, 1 for the rest.
    rank: u8,
    pts: Vec<V2>,
    fam: Vec<u8>,
    ends: [Stop; 2],
}

/// Generates the arterial network and highway of `macro_map`. See the module docs.
pub fn arterials(seed: Seed, template: &RegionTemplate, macro_map: &MacroMap) -> RoadGraph {
    let m = macro_map;
    let ctx = Ctx::new(template, m);
    let mut rng = Rng::new(seed, &[stage::ARTERIALS]);
    let (lines, index) = trace_stage(&ctx, m);
    let mut lines = lines;
    let mut classes: Vec<Vec<RoadClass>> = lines.iter().map(|l| classify(&ctx, l)).collect();

    // Highway line from the beachfront road, interchanges and ramps.
    let mut forced: Vec<(usize, f32, u32)> = Vec::new();
    let mut end_nodes: Vec<(usize, usize, u32)> = Vec::new();
    let mut nodes: Vec<V2> = Vec::new();
    let mut hw_arc: Vec<Option<f32>> = Vec::new();
    let mut oneway_rev: Vec<(usize, bool)> = Vec::new();
    let hw = &ctx.hwy;
    let hw_line = lines.len();
    if hw.pts.len() >= 2 {
        let mut hl = ctx.explicit(Kind::Highway, hw.pts.clone());
        hl.ends = [Stop::Fixed, Stop::Edge];
        lines.push(hl);
        classes.push(vec![RoadClass::Highway; hw.pts.len()]);
        if let Some((li, param)) = hw.terminal {
            nodes.push(hw.pts[0]);
            hw_arc.push(Some(0.0));
            forced.push((li, param, 0));
            end_nodes.push((hw_line, 0, 0));
        }
        let crossings = highway_crossings(&ctx, &lines[..hw_line], &classes);
        let target = rng.range(2, 5) as usize;
        let chosen = choose_interchanges(&ctx, &lines, &crossings, target);
        for c in &chosen {
            build_interchange(
                &ctx,
                &mut lines,
                &mut classes,
                &index,
                c,
                hw_line,
                &mut nodes,
                &mut hw_arc,
                &mut forced,
                &mut end_nodes,
                &mut oneway_rev,
            );
        }
    }

    let mut g = build_graph(
        &lines,
        &classes,
        &forced,
        &end_nodes,
        nodes,
        hw_arc,
        &oneway_rev,
    );
    cleanup(&ctx, &mut g);
    finish(&ctx, m, g)
}

/// Stages 3–4: the explicit lines and the streamlines, ends joined.
fn trace_stage(ctx: &Ctx<'_>, m: &MacroMap) -> (Vec<Line>, Index) {
    let mut lines: Vec<Line> = Vec::new();
    let spine = spine_points(ctx, m);
    for pts in &ctx.beach {
        lines.push(ctx.explicit(Kind::Beachfront, pts.clone()));
    }
    if spine.len() >= 2 {
        lines.push(ctx.explicit(Kind::Spine, spine.clone()));
    }
    for pts in ring(m, ctx) {
        // The ring curves through every direction: it keeps no streamline away.
        let mut l = ctx.explicit(Kind::Ring, pts);
        l.fam.iter_mut().for_each(|f| *f = 2);
        lines.push(l);
    }
    let mut index = Index::new(ctx.ext);
    for (i, l) in lines.iter().enumerate() {
        index.insert(i as u32, &l.pts);
    }

    // Streamlines.
    trace_all(ctx, m, &spine, &mut lines, &mut index);
    for i in 0..lines.len() {
        for end in 0..2 {
            join_end(ctx, &mut lines, &mut index, i, end);
        }
    }
    (lines, index)
}

/// The strip's centerline, trimmed where it would come within 40 m of the highway.
fn spine_points(ctx: &Ctx<'_>, m: &MacroMap) -> Vec<V2> {
    let mut pts: Vec<V2> = m
        .strip
        .centerline
        .iter()
        .map(|&p| V2::from_array(p))
        .collect();
    if ctx.hwy.pts.len() >= 2 {
        while pts.len() >= 2 && ctx.hwy.nearest(pts[pts.len() - 1]).0 < 40.0 {
            pts.pop();
        }
        while pts.len() >= 2 && ctx.hwy.nearest(pts[0]).0 < 40.0 {
            pts.remove(0);
        }
    }
    pts
}

/// Content hash of a road graph (every field, in order), for determinism tests.
pub fn hash_graph(g: &RoadGraph) -> u64 {
    let mut b: Vec<u8> = Vec::new();
    let f = |b: &mut Vec<u8>, x: f32| b.extend_from_slice(&x.to_bits().to_le_bytes());
    for n in &g.nodes {
        f(&mut b, n.pos[0]);
        f(&mut b, n.pos[1]);
        f(&mut b, n.elevation);
    }
    for e in &g.edges {
        b.extend_from_slice(&e.a.to_le_bytes());
        b.extend_from_slice(&e.b.to_le_bytes());
        b.extend_from_slice(&[
            e.class as u8,
            e.lanes_fwd,
            e.lanes_back,
            e.sidewalk as u8,
            e.speed_kph,
            e.oneway as u8,
        ]);
        f(&mut b, e.width_m);
        for p in &e.profile {
            f(&mut b, p[0]);
            f(&mut b, p[1]);
            f(&mut b, p[2]);
        }
    }
    hash_bytes(0x0041_5254_4552, &b)
}

// ---------------------------------------------------------------------------------------
// Fields and sampling.

/// Cell-centered scalar grid.
struct Grid {
    w: usize,
    h: usize,
    cell: f32,
    v: Vec<f32>,
}

impl Grid {
    fn at(&self, p: V2) -> f32 {
        let fx = clamp(p.x / self.cell - 0.5, 0.0, (self.w - 1) as f32);
        let fz = clamp(p.y / self.cell - 0.5, 0.0, (self.h - 1) as f32);
        let (ix, iz) = (
            (fx as usize).min(self.w.saturating_sub(2)),
            (fz as usize).min(self.h.saturating_sub(2)),
        );
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let at = |i: usize, j: usize| self.v[j.min(self.h - 1) * self.w + i.min(self.w - 1)];
        let a = lerp(at(ix, iz), at(ix + 1, iz), tx);
        let b = lerp(at(ix, iz + 1), at(ix + 1, iz + 1), tx);
        lerp(a, b, tz)
    }
}

/// Bilinear sample of a node-centered grid of 4 m samples (the `MacroMap::heights`
/// layout), clamped like `MacroMap::height_at`.
fn sample_nodes(v: &[f32], w: usize, h: usize, p: V2) -> f32 {
    let fx = clamp(p.x / CELL_M, 0.0, (w - 1) as f32);
    let fz = clamp(p.y / CELL_M, 0.0, (h - 1) as f32);
    let (ix, iz) = ((fx as usize).min(w - 2), (fz as usize).min(h - 2));
    let (tx, tz) = (fx - ix as f32, fz - iz as f32);
    let at = |i: usize, j: usize| v[j * w + i];
    let a = lerp(at(ix, iz), at(ix + 1, iz), tx);
    let b = lerp(at(ix, iz + 1), at(ix + 1, iz + 1), tx);
    lerp(a, b, tz)
}

/// The symmetric tensor of a unit direction, as `(cos 2θ, sin 2θ)`.
fn tensor(d: V2) -> [f32; 2] {
    [d.x * d.x - d.y * d.y, 2.0 * d.x * d.y]
}

/// Major eigenvector (unit, sign arbitrary) and coherence `|t|` of a blended tensor.
fn major_of(t: [f32; 2]) -> (V2, f32) {
    let c = sqrt(t[0] * t[0] + t[1] * t[1]);
    if c < 1e-6 {
        return (V2::new(1.0, 0.0), 0.0);
    }
    let (a, b) = (t[0] / c, t[1] / c);
    // Half the angle: bisect (1, 0) and (a, b), or (a, b) and (-1, 0) near 180°.
    let d = if a >= 0.0 {
        V2::new(1.0 + a, b)
    } else {
        V2::new(b, 1.0 - a)
    };
    (d.normalize(), c)
}

/// Tensor field on [`FIELD_M`] cells: blended `(cos 2θ, sin 2θ)` with coherence as length.
struct Field {
    a: Grid,
    b: Grid,
}

impl Field {
    fn at(&self, p: V2) -> [f32; 2] {
        [self.a.at(p), self.b.at(p)]
    }
}

/// The highway: 16 m samples from its coastal terminal, with the graded profile.
struct Highway {
    pts: Vec<V2>,
    arc: Vec<f32>,
    y: Vec<f32>,
    /// The beachfront line and parameter the terminal sits on.
    terminal: Option<(usize, f32)>,
    /// Segment grid for crossing tests.
    grid: SegGrid,
}

impl Highway {
    /// Distance, arc length and unit tangent at the closest point.
    fn nearest(&self, p: V2) -> (f32, f32, V2) {
        let mut best = (f32::INFINITY, 0.0, V2::new(1.0, 0.0));
        let r = 2;
        let (cx, cz) = self.grid.cell_of(p);
        for gz in cz.saturating_sub(r)..(cz + r + 1).min(self.grid.h) {
            for gx in cx.saturating_sub(r)..(cx + r + 1).min(self.grid.w) {
                for &(_, k) in &self.grid.cells[gz * self.grid.w + gx] {
                    let k = k as usize;
                    let (a, b) = (self.pts[k], self.pts[k + 1]);
                    let (d, t) = segment_distance(p, a, b);
                    if d < best.0 {
                        best = (
                            d,
                            lerp(self.arc[k], self.arc[k + 1], t),
                            (b - a).normalize(),
                        );
                    }
                }
            }
        }
        best
    }

    /// Whether the segment `a`–`b` crosses the highway.
    fn crosses(&self, a: V2, b: V2) -> bool {
        let (cx, cz) = self.grid.cell_of(a.lerp(b, 0.5));
        for gz in cz.saturating_sub(1)..(cz + 2).min(self.grid.h) {
            for gx in cx.saturating_sub(1)..(cx + 2).min(self.grid.w) {
                for &(_, k) in &self.grid.cells[gz * self.grid.w + gx] {
                    let k = k as usize;
                    if seg_intersect(a, b, self.pts[k], self.pts[k + 1]).is_some() {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn y_at(&self, s: f32) -> f32 {
        interp(&self.arc, &self.y, s)
    }

    fn length(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }
}

/// Linear interpolation of `ys` over increasing `xs`, held beyond the ends.
fn interp(xs: &[f32], ys: &[f32], x: f32) -> f32 {
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    if x <= xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    // Binary search for the segment.
    let (mut lo, mut hi) = (0, n - 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if xs[mid] <= x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let span = xs[hi] - xs[lo];
    let t = if span > 0.0 { (x - xs[lo]) / span } else { 0.0 };
    lerp(ys[lo], ys[hi], t)
}

/// Grid of segment references `(line, segment)` over 64 m cells.
struct SegGrid {
    w: usize,
    h: usize,
    cell: f32,
    cells: Vec<Vec<(u32, u32)>>,
}

impl SegGrid {
    fn new(ext: [f32; 2], cell: f32) -> SegGrid {
        let w = (ext[0] / cell) as usize + 2;
        let h = (ext[1] / cell) as usize + 2;
        SegGrid {
            w,
            h,
            cell,
            cells: vec![Vec::new(); w * h],
        }
    }

    fn cell_of(&self, p: V2) -> (usize, usize) {
        let cx = clamp(p.x / self.cell, 0.0, (self.w - 1) as f32) as usize;
        let cz = clamp(p.y / self.cell, 0.0, (self.h - 1) as f32) as usize;
        (cx, cz)
    }

    fn insert(&mut self, line: u32, pts: &[V2]) {
        for k in 0..pts.len().saturating_sub(1) {
            let (a, b) = (pts[k], pts[k + 1]);
            let (x0, z0) = self.cell_of(V2::new(a.x.min(b.x), a.y.min(b.y)));
            let (x1, z1) = self.cell_of(V2::new(a.x.max(b.x), a.y.max(b.y)));
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    self.cells[cz * self.w + cx].push((line, k as u32));
                }
            }
        }
    }
}

/// Point index of every traced line: `(line, point)` per 64 m cell.
struct Index {
    w: usize,
    h: usize,
    cells: Vec<Vec<(u32, u32)>>,
}

const INDEX_CELL: f32 = 64.0;

impl Index {
    fn new(ext: [f32; 2]) -> Index {
        let w = (ext[0] / INDEX_CELL) as usize + 2;
        let h = (ext[1] / INDEX_CELL) as usize + 2;
        Index {
            w,
            h,
            cells: vec![Vec::new(); w * h],
        }
    }

    fn cell_of(&self, p: V2) -> (usize, usize) {
        (
            clamp(p.x / INDEX_CELL, 0.0, (self.w - 1) as f32) as usize,
            clamp(p.y / INDEX_CELL, 0.0, (self.h - 1) as f32) as usize,
        )
    }

    fn insert(&mut self, line: u32, pts: &[V2]) {
        for (i, &p) in pts.iter().enumerate() {
            let (cx, cz) = self.cell_of(p);
            self.cells[cz * self.w + cx].push((line, i as u32));
        }
    }

    /// Calls `f(line, point)` for every indexed point in the cells within `r` of `p`.
    fn visit(&self, p: V2, r: f32, mut f: impl FnMut(u32, u32)) {
        let (x0, z0) = self.cell_of(V2::new(p.x - r, p.y - r));
        let (x1, z1) = self.cell_of(V2::new(p.x + r, p.y + r));
        for cz in z0..=z1 {
            for cx in x0..=x1 {
                for &(l, i) in &self.cells[cz * self.w + cx] {
                    f(l, i);
                }
            }
        }
    }

    /// Whether a point of `family` lies within `r` of `p`.
    fn near(&self, lines: &[Line], p: V2, family: u8, r: f32, ignore: u8) -> bool {
        let mut hit = false;
        let r2 = r * r;
        self.visit(p, r, |l, i| {
            let line = &lines[l as usize];
            if hit || (ignore >> line.kind as u8) & 1 == 1 {
                return;
            }
            if line.fam[i as usize] == family {
                let q = line.pts[i as usize];
                let d = q - p;
                if d.dot(d) < r2 {
                    hit = true;
                }
            }
        });
        hit
    }
}

/// Everything the stages read besides the lines themselves.
struct Ctx<'a> {
    m: &'a MacroMap,
    t: &'a RegionTemplate,
    /// Extent of the height grid, meters.
    ext: [f32; 2],
    /// Roads stay inside `[lo, hi]` on both axes.
    lo: f32,
    hi: [f32; 2],
    field: Field,
    gx: Grid,
    gz: Grid,
    spacing: [f32; 12],
    hwy: Highway,
    /// The beachfront road's pieces.
    beach: Vec<Vec<V2>>,
    /// Routing grid on zone cells: 1 where no road may go, else 0.
    route_blocked: Vec<u8>,
    /// Routing grid heights: 3×3 means of the cell-center heights.
    route_height: Vec<f32>,
}

impl<'a> Ctx<'a> {
    fn new(t: &'a RegionTemplate, m: &'a MacroMap) -> Ctx<'a> {
        let (w, h) = (m.width as usize, m.height as usize);
        let ext = [(w - 1) as f32 * CELL_M, (h - 1) as f32 * CELL_M];
        let (gx, gz) = gradient(m, 2, 2);
        let (fx, fz) = gradient(m, 4, 3);
        let mut spacing = SPACING;
        let (_, (_, d1)) = t.zones.farmland.block_m;
        spacing[ZoneId::Farmland as usize] = clamp(0.5 * d1, 250.0, 400.0);
        let field = build_field(m, t, &fx, &fz);
        let mut ctx = Ctx {
            m,
            t,
            ext,
            lo: 4.0,
            hi: [ext[0] - 4.0, ext[1] - 4.0],
            field,
            gx,
            gz,
            spacing,
            hwy: Highway {
                pts: Vec::new(),
                arc: Vec::new(),
                y: Vec::new(),
                terminal: None,
                grid: SegGrid::new(ext, 64.0),
            },
            beach: Vec::new(),
            route_blocked: Vec::new(),
            route_height: Vec::new(),
        };
        ctx.beach = beachfront(m, &ctx);
        ctx.hwy = highway(&ctx);
        let (zw, zh) = (m.zone_width as usize, m.zone_height as usize);
        let mut blocked = vec![0u8; zw * zh];
        let mut height = vec![0.0f32; zw * zh];
        for c in 0..zw * zh {
            let p = V2::new(
                (c % zw) as f32 * ZONE_CELL_M + 8.0,
                (c / zw) as f32 * ZONE_CELL_M + 8.0,
            );
            height[c] = ctx.height(p);
            let near_hwy = ctx.hwy.pts.len() >= 2 && ctx.hwy.nearest(p).0 < 30.0;
            if !ctx.inside(p) || !ctx.land(p) || abs(ctx.cut_at(p)) > CUT_BARRIER_M || near_hwy {
                blocked[c] = 1;
            }
        }
        // Relief under a cell does not decide the route.
        ctx.route_height = box_blur(&height, zw, zh, 1);
        ctx.route_blocked = blocked;
        ctx
    }

    fn height(&self, p: V2) -> f32 {
        self.m.height_at(p.x, p.y)
    }

    fn zone(&self, p: V2) -> ZoneId {
        self.m.zone_at(p.x, p.y)
    }

    fn dsep(&self, p: V2) -> f32 {
        self.spacing[self.zone(p) as usize]
    }

    fn inside(&self, p: V2) -> bool {
        p.x >= self.lo && p.y >= self.lo && p.x <= self.hi[0] && p.y <= self.hi[1]
    }

    /// Dry ground a road may use: not water, not the beach.
    fn land(&self, p: V2) -> bool {
        let z = self.zone(p);
        z != ZoneId::Water && z != ZoneId::Beach && self.height(p) > self.m.water_level + 0.5
    }

    /// How far the macro map's corridor carve moved the terrain at `p` (negative in cuts).
    fn cut_at(&self, p: V2) -> f32 {
        sample_nodes(
            &self.m.corridor,
            self.m.width as usize,
            self.m.height as usize,
            p,
        )
    }

    /// Whether a road heading along `dir` must stop before `p` because of the highway:
    /// its cuts and fills, its first stretch from the coast, or running alongside it.
    fn barrier(&self, p: V2, dir: V2) -> bool {
        // Roads cross over shallow cuts but never over fills.
        let cut = self.cut_at(p);
        if !(-CROSS_CUT_M..=CUT_BARRIER_M).contains(&cut) {
            return true;
        }
        if self.hwy.pts.len() < 2 {
            return false;
        }
        let (d, s, tan) = self.hwy.nearest(p);
        (d < 60.0 && abs(dir.dot(tan)) > 0.82) || (d < 30.0 && s < TERMINUS_M)
    }

    fn gradient(&self, p: V2) -> V2 {
        V2::new(self.gx.at(p), self.gz.at(p))
    }

    /// The family direction at `p` aligned with `prev`, or `None` where the field is
    /// degenerate.
    fn dir(&self, p: V2, family: u8, prev: V2) -> Option<V2> {
        let (maj, coh) = major_of(self.field.at(p));
        if coh < 0.08 {
            return None;
        }
        let d = if family == 0 { maj } else { maj.left() };
        Some(if d.dot(prev) < 0.0 { -d } else { d })
    }

    /// The rotation of `d` by the fewest 10° steps (up to 70°, either way) that turns at
    /// most 30° from `prev` and whose terrain grade over the next 24 m is at most
    /// [`STEER_GRADE`], with the number of steps; `None` when there is none.
    fn steer(&self, p: V2, d: V2, prev: Option<V2>) -> Option<(V2, u32)> {
        let h0 = self.height(p);
        let look = 3.0 * STEP_M;
        // On a tie in steps, try first the side that turns toward the contour.
        let g = self.gradient(p);
        let first = if g.dot(d.left()) > 0.0 { -1.0 } else { 1.0 };
        for (k, &(c, s)) in ROTATIONS.iter().enumerate() {
            for side in [first, -first] {
                if k == 0 && side != first {
                    continue;
                }
                let s = s * side;
                let r = V2::new(d.x * c - d.y * s, d.x * s + d.y * c);
                if prev.is_some_and(|q| r.dot(q) < MAX_TURN_COS) {
                    continue;
                }
                let grade = abs(self.height(p + r * look) - h0) / look;
                if grade <= STEER_GRADE {
                    return Some((r, k as u32));
                }
            }
        }
        None
    }

    /// An explicit line with per-point families from the field.
    fn explicit(&self, kind: Kind, pts: Vec<V2>) -> Line {
        let n = pts.len();
        let fam = (0..n)
            .map(|i| {
                let (a, b) = (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)]);
                let tan = (b - a).normalize();
                let (maj, _) = major_of(self.field.at(pts[i]));
                // Only clearly aligned stretches keep same-family streamlines away.
                let c = abs(tan.dot(maj));
                if c >= 0.9 {
                    0
                } else if c <= 0.4359 {
                    1
                } else {
                    2
                }
            })
            .collect();
        Line {
            kind,
            family: 2,
            rank: 0,
            pts,
            fam,
            ends: [Stop::Open, Stop::Open],
        }
    }
}

/// Terrain gradient on 16 m cells from a copy of the heights box-blurred `passes` times
/// with `radius` cells: radius 2 twice (~100 m) for steering, wider for the field.
fn gradient(m: &MacroMap, radius: usize, passes: usize) -> (Grid, Grid) {
    let (w, h) = (m.width as usize, m.height as usize);
    let (cw, ch) = (w / 4, h / 4);
    let mut mean = vec![0.0; cw * ch];
    for cz in 0..ch {
        for cx in 0..cw {
            let mut acc = 0.0;
            for j in 0..4 {
                for i in 0..4 {
                    acc += m.heights[(cz * 4 + j) * w + cx * 4 + i];
                }
            }
            mean[cz * cw + cx] = acc / 16.0;
        }
    }
    let mut blurred = mean;
    for _ in 0..passes {
        blurred = box_blur(&blurred, cw, ch, radius);
    }
    let cell = ZONE_CELL_M;
    let mut gx = vec![0.0; cw * ch];
    let mut gz = vec![0.0; cw * ch];
    for cz in 0..ch {
        for cx in 0..cw {
            let (x0, x1) = (cx.saturating_sub(1), (cx + 1).min(cw - 1));
            let (z0, z1) = (cz.saturating_sub(1), (cz + 1).min(ch - 1));
            let at = |x: usize, z: usize| blurred[z * cw + x];
            gx[cz * cw + cx] = (at(x1, cz) - at(x0, cz)) / ((x1 - x0).max(1) as f32 * cell);
            gz[cz * cw + cx] = (at(cx, z1) - at(cx, z0)) / ((z1 - z0).max(1) as f32 * cell);
        }
    }
    let grid = |v| Grid {
        w: cw,
        h: ch,
        cell,
        v,
    };
    (grid(gx), grid(gz))
}

/// Unit coast tangent (along increasing `v`) at a world point, from the smoothed
/// shoreline differenced over 400 m.
fn coast_tangent(m: &MacroMap, p: V2) -> V2 {
    let (_, v) = m.frame.to_canon([p.x, p.y]);
    let slope = (m.shore.road_u(v + 200.0) - m.shore.road_u(v - 200.0)) / 400.0;
    let k = 1.0 / sqrt(1.0 + slope * slope);
    V2::from_array(m.frame.dir_to_world(slope * k, k))
}

/// Downtown as an ellipse in the canonical frame: center and semi-axes `(along u, along
/// v)` from the 90th percentile of its cells' offsets.
fn downtown_ellipse(m: &MacroMap) -> Option<((f32, f32), f32, f32)> {
    let c = m.frame.to_canon(m.downtown);
    let zw = m.zone_width as usize;
    let mut du: Vec<f32> = Vec::new();
    let mut dv: Vec<f32> = Vec::new();
    for (i, &z) in m.zones.iter().enumerate() {
        if z == ZoneId::Downtown {
            let x = (i % zw) as f32 * ZONE_CELL_M + 8.0;
            let zz = (i / zw) as f32 * ZONE_CELL_M + 8.0;
            let (u, v) = m.frame.to_canon([x, zz]);
            du.push(abs(u - c.0));
            dv.push(abs(v - c.1));
        }
    }
    if du.len() < 8 {
        return None;
    }
    du.sort_by(|a, b| a.total_cmp(b));
    dv.sort_by(|a, b| a.total_cmp(b));
    let k = du.len() * 9 / 10;
    Some((c, du[k].max(80.0), dv[k].max(80.0)))
}

fn build_field(m: &MacroMap, t: &RegionTemplate, gx: &Grid, gz: &Grid) -> Field {
    let (w, h) = (m.width as usize, m.height as usize);
    let ext = [(w - 1) as f32 * CELL_M, (h - 1) as f32 * CELL_M];
    let fw = (ext[0] / FIELD_M) as usize + 1;
    let fh = (ext[1] / FIELD_M) as usize + 1;
    let zw = m.zone_width as usize;
    let zh = m.zone_height as usize;
    // The section grid's major axis is the world axis closest to the mean coast tangent.
    let mid = V2::new(ext[0] * 0.5, ext[1] * 0.5);
    let ct = coast_tangent(m, mid);
    let axis = if abs(ct.x) >= abs(ct.y) {
        V2::new(1.0, 0.0)
    } else {
        V2::new(0.0, 1.0)
    };
    let t_axis = tensor(axis);
    let ell = downtown_ellipse(m);
    let mut ta = vec![0.0; fw * fh];
    let mut tb = vec![0.0; fw * fh];
    let mut wt = vec![0.0; fw * fh];
    for j in 0..fh {
        for i in 0..fw {
            let p = V2::new((i as f32 + 0.5) * FIELD_M, (j as f32 + 0.5) * FIELD_M);
            let t_coast = tensor(coast_tangent(m, p));
            let g = V2::new(gx.at(p), gz.at(p));
            let gl = g.length();
            let t_terr = if gl > 1e-5 {
                tensor(V2::new(-g.y, g.x) * (1.0 / gl))
            } else {
                [0.0, 0.0]
            };
            let st = smoothstep(0.02, 0.08, gl);
            let mut acc = [0.0f32; 2];
            let mut sum = 0.0f32;
            let add = |tt: [f32; 2], wgt: f32, acc: &mut [f32; 2], sum: &mut f32| {
                acc[0] += tt[0] * wgt;
                acc[1] += tt[1] * wgt;
                *sum += wgt;
            };
            for dz in 0..2 {
                for dx in 0..2 {
                    let (cx, cz) = (2 * i + dx, 2 * j + dz);
                    if cx >= zw || cz >= zh {
                        continue;
                    }
                    let z = m.zones[cz * zw + cx];
                    if matches!(z, ZoneId::Water | ZoneId::Highway) {
                        continue;
                    }
                    let Some(rule) = t.zones.get(z) else { continue };
                    match rule.streets {
                        StreetPattern::Grid | StreetPattern::Beachfront => {
                            add(t_coast, 0.25, &mut acc, &mut sum);
                        }
                        StreetPattern::Curvy => {
                            add(t_coast, 0.25 * 0.3, &mut acc, &mut sum);
                            add(t_terr, 0.25 * st, &mut acc, &mut sum);
                        }
                        StreetPattern::Switchback => {
                            add(t_coast, 0.25 * 0.15, &mut acc, &mut sum);
                            add(t_terr, 0.25 * st, &mut acc, &mut sum);
                        }
                        StreetPattern::Sections => add(t_axis, 0.25, &mut acc, &mut sum),
                        StreetPattern::Industrial => {}
                    }
                }
            }
            // The downtown ring bends the grid toward radial/circumferential, only as far
            // as the two agree (so the blend never cancels out).
            if let Some((c, a, b)) = ell {
                let (u, v) = m.frame.to_canon([p.x, p.y]);
                let (ra, rb) = (a * 1.1 + 40.0, b * 1.1 + 40.0);
                let (nu, nv) = ((u - c.0) / ra, (v - c.1) / rb);
                let r = sqrt(nu * nu + nv * nv);
                let wr = 0.9 * bump((r - 1.0) / 0.4);
                if wr > 0.0 {
                    let radial = (p - V2::from_array(m.downtown)).normalize();
                    let t_r = tensor(radial);
                    let s = t_r[0] * t_coast[0] + t_r[1] * t_coast[1];
                    add([t_r[0] * s, t_r[1] * s], wr, &mut acc, &mut 0.0);
                    sum += wr * abs(s);
                }
            }
            ta[j * fw + i] = acc[0];
            tb[j * fw + i] = acc[1];
            wt[j * fw + i] = sum;
        }
    }
    for _ in 0..2 {
        ta = box_blur(&ta, fw, fh, 2);
        tb = box_blur(&tb, fw, fh, 2);
        wt = box_blur(&wt, fw, fh, 2);
    }
    for k in 0..fw * fh {
        if wt[k] > 1e-4 {
            ta[k] /= wt[k];
            tb[k] /= wt[k];
        } else {
            ta[k] = 0.0;
            tb[k] = 0.0;
        }
    }
    let grid = |v| Grid {
        w: fw,
        h: fh,
        cell: FIELD_M,
        v,
    };
    Field {
        a: grid(ta),
        b: grid(tb),
    }
}

// ---------------------------------------------------------------------------------------
// Highway.

/// Resamples a polyline to at most `step` spacing with points evenly spread per segment
/// run: `n = ceil(L / step)` equal pieces along the arc.
fn resample(pts: &[V2], step: f32) -> Vec<V2> {
    let total = polyline_length(pts);
    if pts.len() < 2 || total <= 0.0 {
        return pts.to_vec();
    }
    let n = ((total / step) as usize + 1).max(1);
    let n = if (n - 1) as f32 * step >= total && n > 1 {
        n - 1
    } else {
        n
    };
    let n = n.max(1);
    let mut out = Vec::with_capacity(n + 1);
    out.push(pts[0]);
    let mut seg = 0;
    let mut seg_start = 0.0;
    for k in 1..n {
        let s = total * k as f32 / n as f32;
        while seg + 1 < pts.len() - 1 && seg_start + (pts[seg + 1] - pts[seg]).length() < s {
            seg_start += (pts[seg + 1] - pts[seg]).length();
            seg += 1;
        }
        let len = (pts[seg + 1] - pts[seg]).length();
        let t = if len > 0.0 {
            clamp((s - seg_start) / len, 0.0, 1.0)
        } else {
            0.0
        };
        out.push(pts[seg].lerp(pts[seg + 1], t));
    }
    out.push(pts[pts.len() - 1]);
    out
}

fn polyline_length(pts: &[V2]) -> f32 {
    let mut acc = 0.0;
    for w in pts.windows(2) {
        acc += (w[1] - w[0]).length();
    }
    acc
}

fn arcs(pts: &[V2]) -> Vec<f32> {
    let mut out = Vec::with_capacity(pts.len());
    let mut acc = 0.0;
    for (i, &p) in pts.iter().enumerate() {
        if i > 0 {
            acc += (p - pts[i - 1]).length();
        }
        out.push(acc);
    }
    out
}

/// The highway polyline from its coastal terminal (on the beachfront road when that runs
/// within 60 m of the macro highway's start) to the place edge, with its graded profile.
fn highway(ctx: &Ctx<'_>) -> Highway {
    let m = ctx.m;
    let mut pts: Vec<V2> = Vec::new();
    for &p in &m.highway {
        let p = V2::from_array(p);
        if ctx.inside(p) {
            pts.push(p);
        } else if !pts.is_empty() {
            // Clip the last segment at the place edge.
            let a = *pts.last().unwrap_or(&p);
            let mut t = 1.0;
            for _ in 0..24 {
                let q = a.lerp(p, t);
                if ctx.inside(q) {
                    break;
                }
                t *= 0.9;
            }
            let q = a.lerp(p, t);
            if (q - a).length() > 1.0 && ctx.inside(q) {
                pts.push(q);
            }
            break;
        }
    }
    let mut out = Highway {
        pts: Vec::new(),
        arc: Vec::new(),
        y: Vec::new(),
        terminal: None,
        grid: SegGrid::new(ctx.ext, 64.0),
    };
    if pts.len() < 2 {
        return out;
    }
    // The terminal: the beachfront road point nearest the macro highway's start.
    let beach = &ctx.beach;
    let mut best: Option<(f32, usize, f32, V2)> = None;
    for (li, line) in beach.iter().enumerate() {
        for k in 0..line.len().saturating_sub(1) {
            let (d, t) = segment_distance(pts[0], line[k], line[k + 1]);
            if d < 60.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, li, k as f32 + t, line[k].lerp(line[k + 1], t)));
            }
        }
    }
    if let Some((_, li, param, q)) = best {
        pts.insert(0, q);
        out.terminal = Some((li, param));
    }
    let pts = resample(&pts, PROFILE_STEP_M);
    let arc = arcs(&pts);
    let n = pts.len();
    let ground: Vec<f32> = pts.iter().map(|&p| ctx.height(p)).collect();
    // Light smoothing of the target, then the slope filter pinned at the terminal.
    let mut target = ground.clone();
    for _ in 0..2 {
        let prev = target.clone();
        for i in 1..n - 1 {
            target[i] = (prev[i - 1] + prev[i] + prev[i + 1]) / 3.0;
        }
    }
    // Forward only: the highway hugs the plain from the coast (where the crossings and
    // interchanges are) and climbs no steeper than 5%, into a cut where the hills rise
    // faster.
    let mut y = vec![ground[0]; n];
    for i in 1..n {
        let d = HIGHWAY_GRADE * (arc[i] - arc[i - 1]);
        y[i] = clamp(target[i], y[i - 1] - d, y[i - 1] + d);
    }
    // Vertical curves: 3-tap averages keep the slope bound (ends stay put).
    for _ in 0..3 {
        let prev = y.clone();
        for i in 1..n - 1 {
            y[i] = (prev[i - 1] + prev[i] + prev[i + 1]) / 3.0;
        }
    }
    out.grid.insert(0, &pts);
    out.pts = pts;
    out.arc = arc;
    out.y = y;
    out
}

/// A profile close to `target` (heights at arc lengths `s`) whose slope never exceeds
/// `g`, starting at `a` and ending at `b` when given (else at the target's ends), and kept
/// within `band` of the target where the slope bound allows. The average of a forward and
/// a backward pass, each clamping to the reachable band from both pinned ends.
fn grade_limit(
    target: &[f32],
    s: &[f32],
    g: f32,
    a: Option<f32>,
    b: Option<f32>,
    band: Option<(&[f32], f32)>,
) -> Vec<f32> {
    let n = target.len();
    if n == 0 {
        return Vec::new();
    }
    let a = a.unwrap_or(target[0]);
    let b = b.unwrap_or(target[n - 1]);
    if n == 1 {
        return vec![a];
    }
    let len = s[n - 1] - s[0];
    if abs(a - b) > g * len {
        // No profile can join the ends within the bound: a straight ramp.
        return (0..n)
            .map(|i| lerp(a, b, if len > 0.0 { (s[i] - s[0]) / len } else { 0.0 }))
            .collect();
    }
    let v: Vec<f32> = (0..n)
        .map(|i| {
            let si = s[i] - s[0];
            let mut x = target[i];
            if let Some((ground, r)) = band {
                x = clamp(x, ground[i] - r, ground[i] + r);
            }
            let lo = (a - g * si).max(b - g * (len - si));
            let hi = (a + g * si).min(b + g * (len - si));
            clamp(x, lo, hi.max(lo))
        })
        .collect();
    let mut f = vec![0.0; n];
    f[0] = a;
    for i in 1..n {
        let d = g * (s[i] - s[i - 1]);
        f[i] = clamp(v[i], f[i - 1] - d, f[i - 1] + d);
    }
    let mut r = vec![0.0; n];
    r[n - 1] = b;
    for i in (0..n - 1).rev() {
        let d = g * (s[i + 1] - s[i]);
        r[i] = clamp(v[i], r[i + 1] - d, r[i + 1] + d);
    }
    let mut y: Vec<f32> = f.iter().zip(&r).map(|(x, z)| 0.5 * (x + z)).collect();
    y[0] = a;
    y[n - 1] = b;
    y
}

// ---------------------------------------------------------------------------------------
// Explicit lines.

/// The beachfront road: 8 m inland of the beach band, measured perpendicular to the
/// shore, as pieces broken where it would run within 50 m of the strip's spine (the strip
/// is the beachfront there) or off dry land.
fn beachfront(m: &MacroMap, ctx: &Ctx<'_>) -> Vec<Vec<V2>> {
    let spine: Vec<V2> = m
        .strip
        .centerline
        .iter()
        .map(|&p| V2::from_array(p))
        .collect();
    let f = &m.frame;
    let len_v = f.len_v();
    let n = (len_v / ZONE_CELL_M) as usize + 1;
    let mut us: Vec<f32> = (0..n)
        .map(|i| {
            let v = (i as f32 * ZONE_CELL_M).min(len_v);
            let s = m.shore.shore_u(v);
            let k = m.shore.distance(s + 1.0, v);
            s + (m.shore.beach_at(v) + 8.0) / k.max(0.2)
        })
        .collect();
    for _ in 0..2 {
        let prev = us.clone();
        for i in 1..n - 1 {
            us[i] = (prev[i - 1] + prev[i] + prev[i + 1]) / 3.0;
        }
    }
    let mut out: Vec<Vec<V2>> = Vec::new();
    let mut cur: Vec<V2> = Vec::new();
    for (i, &u) in us.iter().enumerate() {
        let v = (i as f32 * ZONE_CELL_M).min(len_v);
        let p = V2::from_array(f.to_world(u, v));
        let near_spine = spine
            .windows(2)
            .any(|s| segment_distance(p, s[0], s[1]).0 < 50.0);
        let ok = ctx.inside(p)
            && !near_spine
            && m.zone_at(p.x, p.y) != ZoneId::Water
            && ctx.height(p) > m.water_level + 0.5;
        if ok {
            cur.push(p);
        } else if !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|l| polyline_length(l) >= 100.0);
    out
}

/// A ring road around downtown (an ellipse ~10% outside its cells), cut where it would
/// leave dry land or enter the strip, beach or highway corridor.
fn ring(m: &MacroMap, ctx: &Ctx<'_>) -> Vec<Vec<V2>> {
    let Some((c, a, b)) = downtown_ellipse(m) else {
        return Vec::new();
    };
    let (ra, rb) = (a * 1.1 + 40.0, b * 1.1 + 40.0);
    let perim = TAU * sqrt(0.5 * (ra * ra + rb * rb));
    let n = ((perim / ZONE_CELL_M) as usize).max(16);
    let mut pts: Vec<(V2, bool)> = Vec::with_capacity(n + 1);
    for k in 0..=n {
        let (s, co) = sin_cos(TAU * k as f32 / n as f32);
        let p = V2::from_array(m.frame.to_world(c.0 + ra * co, c.1 + rb * s));
        let z = ctx.zone(p);
        let ok = ctx.inside(p)
            && ctx.land(p)
            && !matches!(z, ZoneId::Strip | ZoneId::Highway)
            && abs(ctx.cut_at(p)) <= CUT_BARRIER_M;
        pts.push((p, ok));
    }
    let mut out: Vec<Vec<V2>> = Vec::new();
    if pts.iter().all(|q| q.1) {
        out.push(pts.iter().map(|q| q.0).collect());
        return out;
    }
    // Start at a gap so pieces do not wrap around the seam.
    let start = pts.iter().position(|q| !q.1).unwrap_or(0);
    let mut cur: Vec<V2> = Vec::new();
    for k in 0..n {
        let (p, ok) = pts[(start + k) % n];
        if ok {
            cur.push(p);
        } else if !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|l| polyline_length(l) >= 150.0);
    out
}

// ---------------------------------------------------------------------------------------
// Streamlines.

struct SeedPoint {
    p: V2,
    family: u8,
    rank: u8,
    /// Trace only this way (strip ends), else both.
    dir: Option<V2>,
    /// Kinds of line (bit `Kind as u8`) this trace need not keep its distance from: the
    /// strip's extensions continue the spine beside the beachfront road.
    ignore: u8,
}

fn trace_all(ctx: &Ctx<'_>, m: &MacroMap, spine: &[V2], lines: &mut Vec<Line>, index: &mut Index) {
    let mut queue: Vec<SeedPoint> = Vec::new();
    // Strip ends, outward along the coast.
    if spine.len() >= 2 {
        let n = spine.len();
        queue.push(SeedPoint {
            p: spine[0],
            family: 0,
            rank: 0,
            dir: Some((spine[0] - spine[1]).normalize()),
            ignore: (1 << Kind::Spine as u8) | (1 << Kind::Beachfront as u8),
        });
        queue.push(SeedPoint {
            p: spine[n - 1],
            family: 0,
            rank: 0,
            dir: Some((spine[n - 1] - spine[n - 2]).normalize()),
            ignore: (1 << Kind::Spine as u8) | (1 << Kind::Beachfront as u8),
        });
    }
    // Downtown: two lines of each family half a spacing either side of the center, so the
    // core is the block between them.
    let dt = V2::from_array(m.downtown);
    let (maj, _) = major_of(ctx.field.at(dt));
    let half = 0.5 * ctx.dsep(dt);
    for family in 0..2u8 {
        let across = if family == 0 { maj.left() } else { maj };
        for s in [1.0, -1.0] {
            queue.push(SeedPoint {
                p: dt + across * (s * half),
                family,
                rank: 0,
                dir: None,
                ignore: 0,
            });
        }
    }
    // Farm valleys (section roads both ways) and overlooks (a contour road past each).
    for v in valley_centers(m) {
        for family in 0..2 {
            queue.push(SeedPoint {
                p: v,
                family,
                rank: 0,
                dir: None,
                ignore: 0,
            });
        }
    }
    let keep_in = |p: V2| {
        V2::new(
            clamp(p.x, 40.0, ctx.ext[0] - 40.0),
            clamp(p.y, 40.0, ctx.ext[1] - 40.0),
        )
    };
    for poi in m.pois.iter().filter(|p| p.kind == MacroPoiKind::Overlook) {
        queue.push(SeedPoint {
            p: keep_in(V2::from_array(poi.pos)),
            family: 0,
            rank: 0,
            dir: None,
            ignore: 0,
        });
    }
    if let Some(h) = harbor_seed(ctx, m, spine) {
        queue.push(SeedPoint {
            p: h,
            family: 1,
            rank: 0,
            dir: None,
            ignore: 0,
        });
    }
    for p in pass_seeds(ctx, m) {
        queue.push(SeedPoint {
            p,
            family: 0,
            rank: 0,
            dir: None,
            ignore: 0,
        });
    }
    let initial = queue.len();
    let mut head = 0;
    let mut streams = 0;
    while head < queue.len() && streams < MAX_LINES && head < 12000 {
        let s = &queue[head];
        let (p, family, rank, dir, ignore) = (s.p, s.family, s.rank, s.dir, s.ignore);
        let first = head < initial;
        head += 1;
        if !seed_ok(ctx, lines, index, p, family, first, ignore) {
            continue;
        }
        let Some((pts, ends)) = trace_line(ctx, lines, index, p, family, dir, ignore) else {
            continue;
        };
        let min_len = if first { 80.0 } else { 150.0 };
        if polyline_length(&pts) < min_len {
            continue;
        }
        let id = lines.len() as u32;
        let n = pts.len();
        lines.push(Line {
            kind: Kind::Stream,
            family,
            rank,
            pts,
            fam: vec![family; n],
            ends,
        });
        index.insert(id, &lines[id as usize].pts);
        streams += 1;
        push_seeds(ctx, &lines[id as usize], &mut queue);
    }
}

/// Seeds along a new line: the other family every spacing along it, the same family a
/// spacing to either side.
fn push_seeds(ctx: &Ctx<'_>, line: &Line, queue: &mut Vec<SeedPoint>) {
    let pts = &line.pts;
    if pts.len() < 2 {
        return;
    }
    let mut acc = 0.0;
    let d0 = ctx.dsep(pts[0]);
    let mut next = 0.25 * d0;
    for k in 1..pts.len() {
        acc += (pts[k] - pts[k - 1]).length();
        if acc < next {
            continue;
        }
        let p = pts[k];
        let d = ctx.dsep(p);
        let n = (pts[k] - pts[k - 1]).normalize().left();
        queue.push(SeedPoint {
            p,
            family: 1 - line.family,
            rank: 1,
            dir: None,
            ignore: 0,
        });
        for s in [1.0, -1.0] {
            queue.push(SeedPoint {
                p: p + n * (s * d),
                family: line.family,
                rank: 1,
                dir: None,
                ignore: 0,
            });
        }
        next += 0.5 * d;
    }
}

fn seed_ok(
    ctx: &Ctx<'_>,
    lines: &[Line],
    index: &Index,
    p: V2,
    family: u8,
    first: bool,
    ignore: u8,
) -> bool {
    let margin = 32.0;
    if !(p.x >= margin && p.y >= margin && p.x <= ctx.ext[0] - margin && p.y <= ctx.ext[1] - margin)
    {
        return false;
    }
    if !ctx.land(p) || abs(ctx.cut_at(p)) > CUT_BARRIER_M {
        return false;
    }
    if ctx.hwy.pts.len() >= 2 && ctx.hwy.nearest(p).0 < 40.0 {
        return false;
    }
    let (_, coh) = major_of(ctx.field.at(p));
    if coh < 0.15 {
        return false;
    }
    let r = ctx.dsep(p) * if first { 0.5 } else { 0.8 };
    !index.near(lines, p, family, r, ignore)
}

/// Traces both ways from `p` (or only along `dir`), returning the joined points and why
/// each end stopped, start end first.
fn trace_line(
    ctx: &Ctx<'_>,
    lines: &[Line],
    index: &Index,
    p: V2,
    family: u8,
    dir: Option<V2>,
    ignore: u8,
) -> Option<(Vec<V2>, [Stop; 2])> {
    let (maj, coh) = major_of(ctx.field.at(p));
    if coh < 0.08 && dir.is_none() {
        return None;
    }
    let d0 = match dir {
        Some(d) => d,
        None => {
            if family == 0 {
                maj
            } else {
                maj.left()
            }
        }
    };
    let (fwd, stop_f) = trace_dir(ctx, lines, index, p, family, d0, ignore);
    let (mut pts, mut ends) = if dir.is_some() {
        (fwd, [Stop::Fixed, stop_f])
    } else {
        let (back, stop_b) = trace_dir(ctx, lines, index, p, family, -d0, ignore);
        let mut pts: Vec<V2> = back.iter().rev().copied().collect();
        pts.extend_from_slice(&fwd[1..]);
        (pts, [stop_b, stop_f])
    };
    trim_crossings(ctx, &mut pts, &mut ends);
    Some((pts, ends))
}

/// An overpass needs room: where the line crosses the highway with less than
/// [`OVERPASS_ROOM_M`] of road on one side, that side is cut off 40 m short of the highway.
/// Ends within 40 m of the highway are cut back too.
fn trim_crossings(ctx: &Ctx<'_>, pts: &mut Vec<V2>, ends: &mut [Stop; 2]) {
    if ctx.hwy.pts.len() < 2 {
        return;
    }
    loop {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let arc = arcs(pts);
        let total = arc[n - 1];
        let mut cut: Option<(usize, bool)> = None;
        for k in 0..n - 1 {
            if !ctx.hwy.crosses(pts[k], pts[k + 1]) {
                continue;
            }
            if arc[k] < OVERPASS_ROOM_M {
                cut = Some((k, false));
                break;
            }
            if total - arc[k + 1] < OVERPASS_ROOM_M {
                cut = Some((k, true));
                break;
            }
        }
        let Some((k, keep_start)) = cut else {
            // Never end on or beside the highway either.
            while pts.len() >= 2 && ctx.hwy.nearest(pts[pts.len() - 1]).0 < 40.0 {
                pts.pop();
                ends[1] = Stop::Barrier;
            }
            while pts.len() >= 2 && ctx.hwy.nearest(pts[0]).0 < 40.0 {
                pts.remove(0);
                ends[0] = Stop::Barrier;
            }
            return;
        };
        if keep_start {
            let mut j = k;
            while j > 0 && ctx.hwy.nearest(pts[j]).0 < 40.0 {
                j -= 1;
            }
            pts.truncate(j + 1);
            ends[1] = Stop::Barrier;
        } else {
            let mut j = k + 1;
            while j + 1 < n && ctx.hwy.nearest(pts[j]).0 < 40.0 {
                j += 1;
            }
            pts.drain(..j);
            ends[0] = Stop::Barrier;
        }
    }
}

fn trace_dir(
    ctx: &Ctx<'_>,
    lines: &[Line],
    index: &Index,
    start: V2,
    family: u8,
    d0: V2,
    ignore: u8,
) -> (Vec<V2>, Stop) {
    let mut pts = vec![start];
    let mut p = start;
    let mut prev = d0;
    let mut flat = 0;
    let mut off = 0;
    let mut len = 0.0;
    let stop = loop {
        if len >= MAX_LINE_M {
            break Stop::Long;
        }
        let d = match ctx.dir(p, family, prev) {
            None => {
                flat += 1;
                if flat > 8 {
                    break Stop::Flat;
                }
                prev
            }
            Some(d1) => {
                flat = 0;
                let pm = p + d1 * (0.5 * STEP_M);
                ctx.dir(pm, family, d1).unwrap_or(d1)
            }
        };
        if pts.len() > 1 && d.dot(prev) < 0.5 {
            // Sharper than 60°: a singularity of the field.
            break Stop::Turn;
        }
        // The direction closest to the field that turns at most 30° from the last step and
        // climbs at most STEER_GRADE over the next 24 m: on slopes the road swings along
        // the contours.
        let Some((d, theta)) = ctx.steer(p, d, if pts.len() > 1 { Some(prev) } else { None })
        else {
            break Stop::Steep;
        };
        // Steered 60° or more off the field for a while: the road cannot go this way.
        if theta >= 6 {
            off += 1;
            if off > 6 {
                break Stop::Turn;
            }
        } else {
            off = 0;
        }
        let q = p + d * STEP_M;
        if !ctx.inside(q) {
            let mut t = 1.0;
            for _ in 0..30 {
                t *= 0.85;
                if ctx.inside(p + d * (STEP_M * t)) {
                    break;
                }
            }
            let e = p + d * (STEP_M * t);
            if ctx.inside(e) && t > 0.05 {
                pts.push(e);
            }
            break Stop::Edge;
        }
        if !ctx.land(q) {
            break Stop::Water;
        }
        if ctx.barrier(q, d) {
            break Stop::Barrier;
        }
        if pts.len() % 2 == 0 {
            let r = 0.65 * ctx.dsep(q);
            if index.near(lines, q, family, r, ignore) {
                break Stop::Near;
            }
            let skip = (3.0 * r / STEP_M) as usize + 2;
            if pts.len() > skip {
                let r2 = r * r;
                if pts[..pts.len() - skip].iter().any(|&o| {
                    let dd = o - q;
                    dd.dot(dd) < r2
                }) {
                    break Stop::Loop;
                }
            }
        }
        pts.push(q);
        len += STEP_M;
        p = q;
        prev = d;
    };
    if stop == Stop::Near {
        truncate_tail(ctx, lines, index, &mut pts);
    }
    (pts, stop)
}

/// A trace that ran up against a parallel neighbor ends at its last crossing (plus a 3 m
/// overshoot so the crossing is a true intersection) when the tail beyond is short.
fn truncate_tail(ctx: &Ctx<'_>, lines: &[Line], index: &Index, pts: &mut Vec<V2>) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let limit = 1.2 * ctx.dsep(pts[n - 1]);
    let max_back = ((limit / STEP_M) as usize).min(n - 2);
    for back in 0..max_back {
        let k = n - 2 - back;
        let (a, b) = (pts[k], pts[k + 1]);
        let mid = a.lerp(b, 0.5);
        let mut hit: Option<f32> = None;
        index.visit(mid, 16.0, |l, i| {
            let line = &lines[l as usize];
            let i = i as usize;
            if i + 1 < line.pts.len() {
                if let Some((t, _)) = seg_intersect(a, b, line.pts[i], line.pts[i + 1]) {
                    if hit.is_none_or(|h| t > h) {
                        hit = Some(t);
                    }
                }
            }
        });
        if let Some(t) = hit {
            let d = (b - a).normalize();
            let x = a.lerp(b, t) + d * 3.0;
            pts.truncate(k + 1);
            pts.push(x);
            return;
        }
    }
}

/// Segment intersection parameters `(t, u)` along `p1→p2` and `q1→q2`, both in `[0, 1]`.
fn seg_intersect(p1: V2, p2: V2, q1: V2, q2: V2) -> Option<(f32, f32)> {
    let r = p2 - p1;
    let s = q2 - q1;
    let den = r.x * s.y - r.y * s.x;
    if abs(den) < 1e-9 {
        return None;
    }
    let w = q1 - p1;
    let t = (w.x * s.y - w.y * s.x) / den;
    let u = (w.x * r.y - w.y * r.x) / den;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some((t, u))
    } else {
        None
    }
}

/// For every 4-connected farmland patch of at least 40 cells, largest first, the patch
/// cell nearest its centroid.
fn valley_centers(m: &MacroMap) -> Vec<V2> {
    let (zw, zh) = (m.zone_width as usize, m.zone_height as usize);
    let mut label = vec![u32::MAX; zw * zh];
    // (cells, centroid x, centroid z, label)
    let mut patches: Vec<(usize, f32, f32, u32)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..zw * zh {
        if m.zones[start] != ZoneId::Farmland || label[start] != u32::MAX {
            continue;
        }
        let id = patches.len() as u32;
        let (mut n, mut sx, mut sz) = (0usize, 0.0f32, 0.0f32);
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, z) = (i % zw, i / zw);
            sx += x as f32;
            sz += z as f32;
            let nb = [
                (x > 0).then(|| i - 1),
                (x + 1 < zw).then_some(i + 1),
                (z > 0).then(|| i - zw),
                (z + 1 < zh).then_some(i + zw),
            ];
            for j in nb.into_iter().flatten() {
                if m.zones[j] == ZoneId::Farmland && label[j] == u32::MAX {
                    label[j] = id;
                    stack.push(j);
                }
            }
        }
        patches.push((n, sx / n as f32, sz / n as f32, id));
    }
    patches.retain(|p| p.0 >= 40);
    patches.sort_by(|a, b| b.0.cmp(&a.0).then(a.3.cmp(&b.3)));
    patches
        .iter()
        .filter_map(|&(_, cx, cz, id)| {
            let mut pick: Option<(f32, usize)> = None;
            for (i, &l) in label.iter().enumerate() {
                if l != id {
                    continue;
                }
                let (dx, dz) = ((i % zw) as f32 - cx, (i / zw) as f32 - cz);
                let d = dx * dx + dz * dz;
                if pick.is_none_or(|p| d < p.0) {
                    pick = Some((d, i));
                }
            }
            let (_, i) = pick?;
            Some(V2::new(
                (i % zw) as f32 * ZONE_CELL_M + 8.0,
                (i / zw) as f32 * ZONE_CELL_M + 8.0,
            ))
        })
        .collect()
}

/// Inland of the harbor, 150 m along the coast away from the strip: the industrial
/// waterfront's avenue.
fn harbor_seed(ctx: &Ctx<'_>, m: &MacroMap, spine: &[V2]) -> Option<V2> {
    let poi = m.pois.iter().find(|p| p.kind == MacroPoiKind::Harbor)?;
    let pos = V2::from_array(poi.pos);
    let tan = coast_tangent(m, pos);
    let center = spine.get(spine.len() / 2).copied().unwrap_or(pos);
    let side = if (pos + tan * 150.0 - center).length() > (pos - tan * 150.0 - center).length() {
        1.0
    } else {
        -1.0
    };
    let along = pos + tan * (150.0 * side);
    let (_, v) = m.frame.to_canon([along.x, along.y]);
    let inland = -V2::from_array(poi.facing);
    let p = along + inland * (m.shore.beach_at(v) + 60.0);
    if ctx.land(p) {
        Some(p)
    } else {
        None
    }
}

/// Either side of the pass, just outside the highway's cut.
fn pass_seeds(ctx: &Ctx<'_>, m: &MacroMap) -> Vec<V2> {
    let mut out = Vec::new();
    let Some(pass) = m.pass else { return out };
    if ctx.hwy.pts.len() < 2 {
        return out;
    }
    let pass = V2::from_array(pass);
    let (_, _, tan) = ctx.hwy.nearest(pass);
    let n = tan.left();
    for s in [1.0, -1.0] {
        let mut d = 30.0;
        while d < 400.0 && abs(ctx.cut_at(pass + n * (s * d))) > 0.5 {
            d += 8.0;
        }
        let p = pass + n * (s * (d + 40.0));
        if ctx.inside(p) && ctx.land(p) {
            out.push(p);
        }
    }
    out
}

/// Joins a line end to the nearest other road within [`SNAP_M`] (or [`SNAP_AHEAD_M`]
/// within 35° straight ahead) by a straight extension that overshoots 3 m.
fn join_end(ctx: &Ctx<'_>, lines: &mut [Line], index: &mut Index, li: usize, end: usize) {
    let stop = lines[li].ends[end];
    if matches!(stop, Stop::Edge | Stop::Fixed) || lines[li].pts.len() < 2 {
        return;
    }
    let pts = &lines[li].pts;
    let n = pts.len();
    let (e, d) = if end == 0 {
        (pts[0], (pts[0] - pts[1]).normalize())
    } else {
        (pts[n - 1], (pts[n - 1] - pts[n - 2]).normalize())
    };
    // Own points this far along the line from the end do not count.
    let own_skip = (2.0 * SNAP_AHEAD_M / STEP_M) as usize;
    let mut best: Option<(f32, V2)> = None;
    let mut connected = false;
    let lines_ro: &[Line] = lines;
    index.visit(e, SNAP_AHEAD_M, |l, i| {
        let (l, i) = (l as usize, i as usize);
        if l == li {
            let from_end = if end == 0 { i } else { n - 1 - i };
            if from_end < own_skip {
                return;
            }
        }
        let line = &lines_ro[l];
        let q = line.pts[i];
        if (q - e).length() < 2.0 {
            connected = true;
        }
        // Nearest point on the segments either side of this vertex.
        for k in [i.wrapping_sub(1), i] {
            if k == usize::MAX || k + 1 >= line.pts.len() {
                continue;
            }
            let (dist, t) = segment_distance(e, line.pts[k], line.pts[k + 1]);
            let x = line.pts[k].lerp(line.pts[k + 1], t);
            let ahead = dist > 0.0 && (x - e).normalize().dot(d) >= 0.819;
            let ok = dist <= SNAP_M || (dist <= SNAP_AHEAD_M && ahead);
            if ok && best.is_none_or(|b| dist < b.0) {
                best = Some((dist, x));
            }
        }
    });
    if connected {
        lines[li].ends[end] = Stop::Fixed;
        return;
    }
    let Some((dist, x)) = best else { return };
    if dist < 0.5 {
        lines[li].ends[end] = Stop::Fixed;
        return;
    }
    let dir = (x - e).normalize();
    let target = x + dir * 3.0;
    if !straight_ok(ctx, e, x) {
        return;
    }
    let steps = ((dist + 3.0) / STEP_M) as usize + 1;
    let ext: Vec<V2> = (1..=steps)
        .map(|k| e.lerp(target, k as f32 / steps as f32))
        .collect();
    // Never join across the highway.
    if ctx.hwy.pts.len() >= 2 {
        for w in ctx.hwy.pts.windows(2) {
            if seg_intersect(e, target, w[0], w[1]).is_some() {
                return;
            }
        }
    }
    let line = &mut lines[li];
    let fam = line.fam[if end == 0 { 0 } else { n - 1 }];
    let first_new = line.pts.len();
    if end == 0 {
        let mut pts: Vec<V2> = ext.iter().rev().copied().collect();
        pts.extend_from_slice(&line.pts);
        let mut f = vec![fam; ext.len()];
        f.extend_from_slice(&line.fam);
        line.pts = pts;
        line.fam = f;
    } else {
        line.pts.extend_from_slice(&ext);
        line.fam.extend(core::iter::repeat_n(fam, ext.len()));
    }
    line.ends[end] = Stop::Fixed;
    // Index the new points (all of them for a prepend, since indices shifted).
    if end == 0 {
        for cell in index.cells.iter_mut() {
            cell.retain(|&(l, _)| l as usize != li);
        }
        let pts = lines[li].pts.clone();
        index.insert(li as u32, &pts);
    } else {
        let pts = lines[li].pts[first_new..].to_vec();
        for (k, &p) in pts.iter().enumerate() {
            let (cx, cz) = index.cell_of(p);
            let w = index.w;
            index.cells[cz * w + cx].push((li as u32, (first_new + k) as u32));
        }
    }
}

/// Whether a straight road from `a` to `b` stays on dry land, off the highway's cuts and
/// within about [`STEER_GRADE`] over every 16 m and end to end.
fn straight_ok(ctx: &Ctx<'_>, a: V2, b: V2) -> bool {
    let len = (b - a).length();
    let steps = (len / STEP_M) as usize + 1;
    let step = len / steps as f32;
    let mut hs: Vec<f32> = Vec::with_capacity(steps + 1);
    for k in 0..=steps {
        let q = a.lerp(b, k as f32 / steps as f32);
        if k > 0 && (!ctx.land(q) || abs(ctx.cut_at(q)) > CUT_BARRIER_M) {
            return false;
        }
        hs.push(ctx.height(q));
    }
    // Over two steps (~16 m) like the trace's look-ahead, and end to end.
    let span = 2.min(steps);
    for k in span..=steps {
        if abs(hs[k] - hs[k - span]) > (STEER_GRADE + 0.01) * step * span as f32 {
            return false;
        }
    }
    abs(hs[steps] - hs[0]) <= STEER_GRADE * len.max(1.0)
}

/// Class per point: explicit lines by kind; streamlines by zone (smoothed over ±80 m,
/// runs under 120 m absorbed by their neighbors).
fn classify(ctx: &Ctx<'_>, line: &Line) -> Vec<RoadClass> {
    let n = line.pts.len();
    match line.kind {
        Kind::Beachfront => return vec![RoadClass::Beachfront; n],
        Kind::Spine => return vec![RoadClass::Arterial; n],
        Kind::Ring | Kind::Ramp | Kind::Connector => return vec![RoadClass::Collector; n],
        Kind::Highway => return vec![RoadClass::Highway; n],
        Kind::Stream => {}
    }
    let raw: Vec<u8> = line
        .pts
        .iter()
        .map(|&p| match ctx.zone(p) {
            ZoneId::Downtown | ZoneId::Commercial | ZoneId::Strip => 0,
            ZoneId::Suburb => 1,
            ZoneId::Hills | ZoneId::Farmland => 2,
            _ => {
                if line.rank == 0 {
                    0
                } else {
                    1
                }
            }
        })
        .collect();
    let win = 10usize;
    let mut sm: Vec<u8> = (0..n)
        .map(|i| {
            let mut c = [0u32; 3];
            for &r in &raw[i.saturating_sub(win)..(i + win + 1).min(n)] {
                c[r as usize] += 1;
            }
            let mut best = raw[i];
            for k in 0..3u8 {
                if c[k as usize] > c[best as usize] {
                    best = k;
                }
            }
            best
        })
        .collect();
    // Absorb short runs into the previous run (or the next, for a leading run).
    let min_run = 15usize;
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j < n && sm[j] == sm[i] {
            j += 1;
        }
        if j - i < min_run && !(i == 0 && j == n) {
            let fill = if i > 0 {
                sm[i - 1]
            } else if j < n {
                sm[j]
            } else {
                sm[i]
            };
            for s in &mut sm[i..j] {
                *s = fill;
            }
        }
        i = j;
    }
    sm.iter()
        .map(|&c| match c {
            0 => RoadClass::Arterial,
            1 => RoadClass::Collector,
            _ => RoadClass::Rural,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Interchanges.

struct Crossing {
    line: usize,
    /// Parameter along the line (segment index + fraction).
    param: f32,
    /// Arc length along the highway.
    s: f32,
    point: V2,
    /// `|sin|` of the crossing angle.
    sin: f32,
    class: RoadClass,
}

fn highway_crossings(ctx: &Ctx<'_>, lines: &[Line], classes: &[Vec<RoadClass>]) -> Vec<Crossing> {
    let hw = &ctx.hwy;
    let mut out: Vec<Crossing> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        for (k, (w, &class)) in line.pts.windows(2).zip(&classes[li]).enumerate() {
            let (a, b) = (w[0], w[1]);
            let (cx, cz) = hw.grid.cell_of(a.lerp(b, 0.5));
            let mut found: Option<(f32, usize, f32)> = None;
            for gz in cz.saturating_sub(1)..(cz + 2).min(hw.grid.h) {
                for gx in cx.saturating_sub(1)..(cx + 2).min(hw.grid.w) {
                    for &(_, j) in &hw.grid.cells[gz * hw.grid.w + gx] {
                        let j = j as usize;
                        if let Some((t, u)) = seg_intersect(a, b, hw.pts[j], hw.pts[j + 1]) {
                            if found.is_none_or(|f| j < f.1 || (j == f.1 && t < f.0)) {
                                found = Some((t, j, u));
                            }
                        }
                    }
                }
            }
            if let Some((t, j, u)) = found {
                let tan = (hw.pts[j + 1] - hw.pts[j]).normalize();
                let d = (b - a).normalize();
                out.push(Crossing {
                    line: li,
                    param: k as f32 + t,
                    s: lerp(hw.arc[j], hw.arc[j + 1], u),
                    point: a.lerp(b, t),
                    sin: abs(tan.turn(d)),
                    class,
                });
            }
        }
    }
    out.sort_by(|a, b| a.s.total_cmp(&b.s).then(a.line.cmp(&b.line)));
    out
}

/// Point and parameter `dist` meters of arc from `param` along a line (negative: back).
fn walk(pts: &[V2], param: f32, dist: f32) -> Option<(f32, V2)> {
    let n = pts.len();
    let mut k = (param as usize).min(n - 2);
    let mut t = param - k as f32;
    let mut left = abs(dist);
    if dist >= 0.0 {
        loop {
            let seg = (pts[k + 1] - pts[k]).length();
            let rest = seg * (1.0 - t);
            if rest >= left && seg > 0.0 {
                let tt = t + left / seg;
                return Some((k as f32 + tt, pts[k].lerp(pts[k + 1], tt)));
            }
            left -= rest;
            k += 1;
            t = 0.0;
            if k + 1 >= n {
                return None;
            }
        }
    } else {
        loop {
            let seg = (pts[k + 1] - pts[k]).length();
            let rest = seg * t;
            if rest >= left && seg > 0.0 {
                let tt = t - left / seg;
                return Some((k as f32 + tt, pts[k].lerp(pts[k + 1], tt)));
            }
            left -= rest;
            if k == 0 {
                return None;
            }
            k -= 1;
            t = 1.0;
        }
    }
}

fn choose_interchanges<'c>(
    ctx: &Ctx<'_>,
    lines: &[Line],
    crossings: &'c [Crossing],
    target: usize,
) -> Vec<&'c Crossing> {
    let hw = &ctx.hwy;
    let total = hw.length();
    let level = |s: f32| {
        let mut ok = true;
        for (i, &a) in hw.arc.iter().enumerate() {
            if abs(a - s) <= RAMP_RUN_M + 30.0 && abs(hw.y[i] - ctx.height(hw.pts[i])) > 1.5 {
                ok = false;
            }
        }
        ok
    };
    let mut chosen: Vec<&Crossing> = Vec::new();
    for pass in 0..2 {
        let (min_sin, clear) = if pass == 0 {
            (0.75, 220.0)
        } else {
            (0.6, 190.0)
        };
        let mut cand: Vec<&Crossing> = crossings
            .iter()
            .filter(|c| {
                c.sin >= min_sin
                    && c.s >= TERMINUS_M
                    && c.s <= total - RAMP_RUN_M - 50.0
                    && !crossings
                        .iter()
                        .any(|o| !core::ptr::eq(o, *c) && abs(o.s - c.s) < clear)
                    && walk(&lines[c.line].pts, c.param, RAMP_FOOT_M + 25.0).is_some()
                    && walk(&lines[c.line].pts, c.param, -(RAMP_FOOT_M + 25.0)).is_some()
                    && level(c.s)
            })
            .collect();
        let prio = |c: &Crossing| match c.class {
            RoadClass::Arterial => 0,
            RoadClass::Collector | RoadClass::Beachfront => 1,
            _ => 2,
        };
        cand.sort_by(|a, b| prio(a).cmp(&prio(b)).then(a.s.total_cmp(&b.s)));
        for c in cand {
            if chosen.len() >= target {
                break;
            }
            if chosen.iter().any(|o| abs(o.s - c.s) < 450.0) {
                continue;
            }
            chosen.push(c);
        }
        if chosen.len() >= 2 {
            break;
        }
    }
    chosen.sort_by(|a, b| a.s.total_cmp(&b.s));
    chosen
}

/// A diamond: up to four one-way ramps from the highway 150 m either side of the crossing
/// to the crossing road `RAMP_FOOT_M` either side of the highway.
#[allow(clippy::too_many_arguments)]
fn build_interchange(
    ctx: &Ctx<'_>,
    lines: &mut Vec<Line>,
    classes: &mut Vec<Vec<RoadClass>>,
    index: &Index,
    c: &Crossing,
    hw_line: usize,
    nodes: &mut Vec<V2>,
    hw_arc: &mut Vec<Option<f32>>,
    forced: &mut Vec<(usize, f32, u32)>,
    end_nodes: &mut Vec<(usize, usize, u32)>,
    oneway_rev: &mut Vec<(usize, bool)>,
) {
    let hw = &ctx.hwy;
    let road = &lines[c.line].pts;
    let (_, _, tan) = hw.nearest(c.point);
    let right = tan.right();
    let right_hand = ctx.t.driving_side == DrivingSide::Right;
    // Feet on the crossing road, keyed by side (+1 right of increasing arc, -1 left).
    let mut feet: Vec<(f32, f32, V2)> = Vec::new();
    for dist in [RAMP_FOOT_M, -RAMP_FOOT_M] {
        if let Some((param, p)) = walk(road, c.param, dist) {
            let side = if (p - c.point).dot(right) >= 0.0 {
                1.0
            } else {
                -1.0
            };
            feet.push((side, param, p));
        }
    }
    let mut ramps: Vec<(f32, f32, Vec<V2>, usize)> = Vec::new(); // (delta, side, pts, H index)
    for delta in [-1.0f32, 1.0] {
        let s_h = c.s + delta * RAMP_RUN_M;
        let hi = nearest_index(&hw.arc, s_h);
        if hi == 0 || hi + 1 >= hw.pts.len() {
            continue;
        }
        let h = hw.pts[hi];
        for &(side, _, j) in &feet {
            let p1 = h - tan * (delta * 60.0) + right * (side * 10.0);
            let p2 = j + tan * (delta * 40.0);
            let pts = bezier(h, p1, p2, j, STEP_M);
            if ramp_ok(ctx, lines, index, &pts, c.line) {
                ramps.push((delta, side, pts, hi));
            }
        }
    }
    if ramps.len() < 2 {
        return;
    }
    // Nodes: one per highway point used, one per foot used.
    let mut h_nodes: Vec<(usize, u32)> = Vec::new();
    let mut j_nodes: Vec<(f32, u32)> = Vec::new();
    for (delta, side, pts, hi) in ramps {
        let hn = match h_nodes.iter().find(|x| x.0 == hi) {
            Some(x) => x.1,
            None => {
                let id = nodes.len() as u32;
                nodes.push(hw.pts[hi]);
                hw_arc.push(Some(hw.arc[hi]));
                forced.push((hw_line, hi as f32, id));
                h_nodes.push((hi, id));
                id
            }
        };
        let jn = match j_nodes.iter().find(|x| x.0 == side) {
            Some(x) => x.1,
            None => {
                let foot = feet.iter().find(|f| f.0 == side).copied();
                let Some((_, param, p)) = foot else { continue };
                let id = nodes.len() as u32;
                nodes.push(p);
                hw_arc.push(None);
                forced.push((c.line, param, id));
                j_nodes.push((side, id));
                id
            }
        };
        let li = lines.len();
        let n = pts.len();
        let mut line = ctx.explicit(Kind::Ramp, pts);
        line.ends = [Stop::Fixed, Stop::Fixed];
        lines.push(line);
        classes.push(vec![RoadClass::Collector; n]);
        end_nodes.push((li, 0, hn));
        end_nodes.push((li, 1, jn));
        // Off-ramps run highway → road: before the crossing on the side traffic keeps to.
        let keep = if right_hand { side } else { -side };
        let off = delta == -keep;
        oneway_rev.push((li, !off));
    }
}

fn nearest_index(xs: &[f32], x: f32) -> usize {
    let mut best = (f32::INFINITY, 0);
    for (i, &v) in xs.iter().enumerate() {
        if abs(v - x) < best.0 {
            best = (abs(v - x), i);
        }
    }
    best.1
}

fn bezier(p0: V2, p1: V2, p2: V2, p3: V2, step: f32) -> Vec<V2> {
    let est = (p1 - p0).length() + (p2 - p1).length() + (p3 - p2).length();
    let n = ((est / step) as usize).max(4);
    (0..=n)
        .map(|k| {
            let t = k as f32 / n as f32;
            let u = 1.0 - t;
            p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t)
        })
        .collect()
}

/// A ramp stays on dry land and clear (10 m) of every road but its own crossing road.
fn ramp_ok(ctx: &Ctx<'_>, lines: &[Line], index: &Index, pts: &[V2], road: usize) -> bool {
    let n = pts.len();
    for (k, &p) in pts.iter().enumerate() {
        if !ctx.inside(p) || !ctx.land(p) {
            return false;
        }
        if k < 2 || k + 3 >= n {
            continue;
        }
        let mut clash = false;
        index.visit(p, 12.0, |l, i| {
            if l as usize == road {
                return;
            }
            let q = lines[l as usize].pts[i as usize];
            if (q - p).length() < 12.0 {
                clash = true;
            }
        });
        if clash {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------------------
// Graph.

struct GEdge {
    a: u32,
    b: u32,
    class: RoadClass,
    kind: Kind,
    pts: Vec<V2>,
    oneway: bool,
    alive: bool,
}

struct Graph {
    nodes: Vec<V2>,
    /// Arc length along the highway of highway nodes.
    hw_arc: Vec<Option<f32>>,
    edges: Vec<GEdge>,
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        let up = parent[parent[i as usize] as usize];
        parent[i as usize] = up;
        i = up;
    }
    i
}

fn build_graph(
    lines: &[Line],
    classes: &[Vec<RoadClass>],
    forced: &[(usize, f32, u32)],
    end_nodes: &[(usize, usize, u32)],
    mut nodes: Vec<V2>,
    mut hw_arc: Vec<Option<f32>>,
    oneway_rev: &[(usize, bool)],
) -> Graph {
    let ext = {
        let mut e = [0.0f32; 2];
        for l in lines {
            for p in &l.pts {
                e[0] = e[0].max(p.x);
                e[1] = e[1].max(p.y);
            }
        }
        [e[0] + 64.0, e[1] + 64.0]
    };
    let mut splits: Vec<Vec<(f32, u32)>> = vec![Vec::new(); lines.len()];
    for &(l, param, node) in forced {
        splits[l].push((param, node));
    }
    // True intersections between every pair of non-highway lines.
    let mut grid = SegGrid::new(ext, 64.0);
    for (i, l) in lines.iter().enumerate() {
        if l.kind != Kind::Highway {
            grid.insert(i as u32, &l.pts);
        }
    }
    for cz in 0..grid.h {
        for cx in 0..grid.w {
            let cell = &grid.cells[cz * grid.w + cx];
            for x in 0..cell.len() {
                let (li, si) = (cell[x].0 as usize, cell[x].1 as usize);
                for &(lj, sj) in &cell[x + 1..] {
                    let (lj, sj) = (lj as usize, sj as usize);
                    if li == lj && (si as isize - sj as isize).abs() <= 1 {
                        continue;
                    }
                    let (ki, kj) = (lines[li].kind, lines[lj].kind);
                    if ki == Kind::Ramp && kj == Kind::Ramp {
                        continue;
                    }
                    let (a, b) = (lines[li].pts[si], lines[li].pts[si + 1]);
                    let (c, d) = (lines[lj].pts[sj], lines[lj].pts[sj + 1]);
                    let Some((t, u)) = seg_intersect(a, b, c, d) else {
                        continue;
                    };
                    let p = a.lerp(b, t);
                    // Count each pair once: in the cell holding the intersection.
                    if grid.cell_of(p) != (cx, cz) {
                        continue;
                    }
                    // Ends touching ends are left to the node merge.
                    let end_i = (si == 0 && t <= 1e-4)
                        || (si + 2 == lines[li].pts.len() && t >= 1.0 - 1e-4);
                    let end_j = (sj == 0 && u <= 1e-4)
                        || (sj + 2 == lines[lj].pts.len() && u >= 1.0 - 1e-4);
                    if end_i && end_j {
                        continue;
                    }
                    let id = nodes.len() as u32;
                    nodes.push(p);
                    hw_arc.push(None);
                    splits[li].push((si as f32 + t, id));
                    splits[lj].push((sj as f32 + u, id));
                }
            }
        }
    }
    // Line ends.
    for (i, l) in lines.iter().enumerate() {
        let n = l.pts.len();
        for end in 0..2 {
            let over = end_nodes
                .iter()
                .find(|x| x.0 == i && x.1 == end)
                .map(|x| x.2);
            let id = match over {
                Some(id) => id,
                None => {
                    let id = nodes.len() as u32;
                    nodes.push(if end == 0 { l.pts[0] } else { l.pts[n - 1] });
                    hw_arc.push(None);
                    id
                }
            };
            splits[i].push((if end == 0 { 0.0 } else { (n - 1) as f32 }, id));
        }
    }
    // Merge nodes within MERGE_M; highway nodes never merge.
    let nn = nodes.len();
    let mut parent: Vec<u32> = (0..nn as u32).collect();
    {
        let mut ngrid: Vec<Vec<u32>> = vec![Vec::new(); grid.w * grid.h];
        for (i, &p) in nodes.iter().enumerate() {
            let (cx, cz) = grid.cell_of(p);
            ngrid[cz * grid.w + cx].push(i as u32);
        }
        for i in 0..nn {
            if hw_arc[i].is_some() {
                continue;
            }
            let p = nodes[i];
            let (cx, cz) = grid.cell_of(p);
            for gz in cz.saturating_sub(1)..(cz + 2).min(grid.h) {
                for gx in cx.saturating_sub(1)..(cx + 2).min(grid.w) {
                    for &j in &ngrid[gz * grid.w + gx] {
                        let j = j as usize;
                        if j <= i || hw_arc[j].is_some() {
                            continue;
                        }
                        if (nodes[j] - p).length() < MERGE_M {
                            let (ri, rj) =
                                (find(&mut parent, i as u32), find(&mut parent, j as u32));
                            if ri != rj {
                                parent[ri.max(rj) as usize] = ri.min(rj);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut root: Vec<u32> = (0..nn as u32).map(|i| find(&mut parent, i)).collect();
    // Pieces between consecutive splits.
    let mut edges: Vec<GEdge> = Vec::new();
    let mut seen: Vec<(u32, u32)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let mut sp = splits[i].clone();
        sp.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        sp.dedup_by(|b, a| abs(a.0 - b.0) < 1e-4);
        // A loop (a piece whose ends merged into one node) gets two nodes at its thirds.
        let mut k = 0;
        while k + 1 < sp.len() {
            let (p0, n0) = sp[k];
            let (p1, n1) = sp[k + 1];
            if root[n0 as usize] == root[n1 as usize] && span_length(&l.pts, p0, p1) > 3.0 * MERGE_M
            {
                for (j, f) in [(1, 1.0 / 3.0), (2, 2.0 / 3.0)] {
                    let pm = lerp(p0, p1, f);
                    let id = nodes.len() as u32;
                    nodes.push(point_at(&l.pts, pm));
                    hw_arc.push(None);
                    root.push(id);
                    sp.insert(k + j, (pm, id));
                }
            }
            k += 1;
        }
        let rev = oneway_rev.iter().find(|x| x.0 == i).map(|x| x.1);
        for w in sp.windows(2) {
            let (p0, n0) = w[0];
            let (p1, n1) = w[1];
            let (a, b) = (root[n0 as usize], root[n1 as usize]);
            if a == b {
                continue;
            }
            let mut pts = vec![nodes[a as usize]];
            let first = p0 as usize + 1;
            let last = p1 as usize; // inclusive candidate
            for k in first..=last.min(l.pts.len() - 1) {
                let kf = k as f32;
                if kf > p0 + 1e-3 && kf < p1 - 1e-3 {
                    pts.push(l.pts[k]);
                }
            }
            pts.push(nodes[b as usize]);
            let key = (a.min(b), a.max(b));
            if l.kind != Kind::Ramp && seen.contains(&key) {
                continue;
            }
            seen.push(key);
            // Majority class over the piece's points.
            let mut count = [0u32; 7];
            let k0 = (p0 as usize).min(l.pts.len() - 1);
            let k1 = (p1 as usize).min(l.pts.len() - 1);
            for k in k0..=k1 {
                count[classes[i][k] as usize] += 1;
            }
            let mut class = classes[i][k0];
            for (c, &cnt) in count.iter().enumerate() {
                if cnt > count[class as usize] {
                    class = class_from(c as u8);
                }
            }
            let (mut a, mut b) = (a, b);
            if rev == Some(true) {
                pts.reverse();
                core::mem::swap(&mut a, &mut b);
            }
            edges.push(GEdge {
                a,
                b,
                class,
                kind: l.kind,
                pts,
                oneway: rev.is_some(),
                alive: true,
            });
        }
    }
    Graph {
        nodes,
        hw_arc,
        edges,
    }
}

/// The point at parameter `param` (segment index + fraction) of a polyline.
fn point_at(pts: &[V2], param: f32) -> V2 {
    let k = (param.max(0.0) as usize).min(pts.len() - 2);
    pts[k].lerp(pts[k + 1], clamp(param - k as f32, 0.0, 1.0))
}

/// Arc length of a polyline between two parameters.
fn span_length(pts: &[V2], p0: f32, p1: f32) -> f32 {
    let mut acc = 0.0;
    let mut prev = point_at(pts, p0);
    let mut k = p0 as usize + 1;
    while (k as f32) < p1 {
        acc += (pts[k] - prev).length();
        prev = pts[k];
        k += 1;
    }
    acc + (point_at(pts, p1) - prev).length()
}

fn class_from(c: u8) -> RoadClass {
    match c {
        0 => RoadClass::Local,
        1 => RoadClass::Collector,
        2 => RoadClass::Arterial,
        3 => RoadClass::Highway,
        4 => RoadClass::Alley,
        5 => RoadClass::Beachfront,
        _ => RoadClass::Rural,
    }
}

fn degrees(g: &Graph) -> Vec<u32> {
    let mut deg = vec![0u32; g.nodes.len()];
    for e in g.edges.iter().filter(|e| e.alive) {
        deg[e.a as usize] += 1;
        deg[e.b as usize] += 1;
    }
    deg
}

fn cleanup(ctx: &Ctx<'_>, g: &mut Graph) {
    remove_stubs(ctx, g);
    merge_chains(g);
    connect(ctx, g);
    hill_roads(ctx, g);
    remove_stubs(ctx, g);
    merge_chains(g);
    g.edges.retain(|e| e.alive);
}

/// Dead ends under [`STUB_M`] go, except where the dead end lies in `Suburb`/`Hills`;
/// overshoots under 10 m go everywhere.
fn remove_stubs(ctx: &Ctx<'_>, g: &mut Graph) {
    loop {
        let deg = degrees(g);
        let mut changed = false;
        for e in g.edges.iter_mut() {
            if !e.alive || matches!(e.kind, Kind::Highway | Kind::Ramp) {
                continue;
            }
            let len = polyline_length(&e.pts);
            for (end, other) in [(e.a, e.b), (e.b, e.a)] {
                if deg[end as usize] != 1 || deg[other as usize] < 2 {
                    continue;
                }
                let z = ctx.zone(g.nodes[end as usize]);
                let keep_zone = matches!(z, ZoneId::Suburb | ZoneId::Hills);
                if len < 10.0 || (len < STUB_M && !keep_zone) {
                    e.alive = false;
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Joins edges at degree-2 nodes when both have the same class and kind.
fn merge_chains(g: &mut Graph) {
    loop {
        let deg = degrees(g);
        let mut changed = false;
        for (n, &dn) in deg.iter().enumerate() {
            if dn != 2 || g.hw_arc[n].is_some() {
                continue;
            }
            let inc: Vec<usize> = (0..g.edges.len())
                .filter(|&i| {
                    g.edges[i].alive && (g.edges[i].a as usize == n || g.edges[i].b as usize == n)
                })
                .collect();
            if inc.len() != 2 {
                continue;
            }
            let (i, j) = (inc[0], inc[1]);
            let (ei, ej) = (&g.edges[i], &g.edges[j]);
            if ei.class != ej.class
                || ei.kind != ej.kind
                || matches!(ei.kind, Kind::Highway | Kind::Ramp)
                || ei.oneway
                || ej.oneway
            {
                continue;
            }
            // Orient i to end at n and j to start at n.
            let mut pi = ei.pts.clone();
            let mut ai = ei.a;
            if ei.a as usize == n {
                pi.reverse();
                ai = ei.b;
            }
            let mut pj = ej.pts.clone();
            let mut bj = ej.b;
            if ej.b as usize == n {
                pj.reverse();
                bj = ej.a;
            }
            if ai == bj {
                continue;
            }
            let dup = g.edges.iter().enumerate().any(|(k, e)| {
                e.alive
                    && k != i
                    && k != j
                    && ((e.a == ai && e.b == bj) || (e.a == bj && e.b == ai))
            });
            if dup {
                continue;
            }
            pi.extend_from_slice(&pj[1..]);
            g.edges[i].pts = pi;
            g.edges[i].a = ai;
            g.edges[i].b = bj;
            g.edges[j].alive = false;
            changed = true;
            break;
        }
        if !changed {
            break;
        }
    }
}

/// Components by union-find over live edges: `(root per node, edges per root)`.
fn components(g: &Graph) -> (Vec<u32>, Vec<usize>) {
    let n = g.nodes.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for e in g.edges.iter().filter(|e| e.alive) {
        let (ra, rb) = (find(&mut parent, e.a), find(&mut parent, e.b));
        if ra != rb {
            parent[ra.max(rb) as usize] = ra.min(rb);
        }
    }
    let root: Vec<u32> = (0..n as u32).map(|i| find(&mut parent, i)).collect();
    let mut count = vec![0usize; n];
    for e in g.edges.iter().filter(|e| e.alive) {
        count[root[e.a as usize] as usize] += 1;
    }
    (root, count)
}

/// Reconnects every smaller component to the largest by a straight connector (≤ 200 m,
/// dry, clear of the highway and its cuts, crossing no other road), else by a routed one
/// ([`route`]), or drops it.
fn connect(ctx: &Ctx<'_>, g: &mut Graph) {
    for _round in 0..64 {
        let (root, count) = components(g);
        let mut main = 0usize;
        for (r, &c) in count.iter().enumerate() {
            if c > count[main] {
                main = r;
            }
        }
        // The smallest-root other component with edges.
        let Some(other) = (0..count.len()).find(|&r| r != main && count[r] > 0) else {
            return;
        };
        let comp_nodes: Vec<usize> = (0..g.nodes.len())
            .filter(|&i| root[i] as usize == other)
            .collect();
        let mut best: Option<(f32, usize, usize, usize, V2)> = None;
        for &n in &comp_nodes {
            let p = g.nodes[n];
            for (ei, e) in g.edges.iter().enumerate() {
                if !e.alive
                    || root[e.a as usize] as usize != main
                    || matches!(e.kind, Kind::Highway | Kind::Ramp)
                {
                    continue;
                }
                for k in 0..e.pts.len() - 1 {
                    let (d, t) = segment_distance(p, e.pts[k], e.pts[k + 1]);
                    if d <= 200.0 && best.is_none_or(|b| d < b.0) {
                        let q = e.pts[k].lerp(e.pts[k + 1], t);
                        if connector_ok(ctx, g, p, q, ei) {
                            best = Some((d, n, ei, k, q));
                        }
                    }
                }
            }
        }
        if let Some((_, n, ei, k, q)) = best {
            let id = split_edge(g, ei, k, q);
            let class = connector_class(ctx, &[g.nodes[n], q]);
            g.edges.push(GEdge {
                a: n as u32,
                b: id,
                class,
                kind: Kind::Connector,
                pts: vec![g.nodes[n], q],
                oneway: false,
                alive: true,
            });
            continue;
        }
        let starts: Vec<V2> = comp_nodes.iter().map(|&n| g.nodes[n]).collect();
        if let Some((si, pts, ei, k, q)) = route(ctx, g, &root, &starts, Some(other), main) {
            let n = comp_nodes[si];
            let id = split_edge(g, ei, k, q);
            let class = connector_class(ctx, &pts);
            g.edges.push(GEdge {
                a: n as u32,
                b: id,
                class,
                kind: Kind::Connector,
                pts,
                oneway: false,
                alive: true,
            });
            continue;
        }
        for e in g.edges.iter_mut() {
            if e.alive && root[e.a as usize] as usize == other {
                e.alive = false;
            }
        }
    }
}

/// Winding roads up into the hills: from every overlook, then from the gentlest hill cell of
/// every block one hill spacing square (nearest downtown first) with no road within half a
/// spacing, a routed road ([`route`]) down to the network, so the roads grow into the hills
/// as a tree.
fn hill_roads(ctx: &Ctx<'_>, g: &mut Graph) {
    let m = ctx.m;
    let mut targets: Vec<V2> = Vec::new();
    let keep_in = |p: V2| {
        V2::new(
            clamp(p.x, 40.0, ctx.ext[0] - 40.0),
            clamp(p.y, 40.0, ctx.ext[1] - 40.0),
        )
    };
    for poi in m.pois.iter().filter(|p| p.kind == MacroPoiKind::Overlook) {
        targets.push(keep_in(V2::from_array(poi.pos)));
    }
    // Per block of one hill spacing, its gentlest hill cell (benches and saddles, not the
    // cliffs most of the hills are).
    let step = ctx.spacing[ZoneId::Hills as usize];
    let (zw, zh) = (m.zone_width as usize, m.zone_height as usize);
    let per = (step / ZONE_CELL_M) as usize;
    let dt = V2::from_array(m.downtown);
    let hgt = &ctx.route_height;
    let mut lattice: Vec<(f32, V2)> = Vec::new();
    for bz in 0..zh / per {
        for bx in 0..zw / per {
            let mut best: Option<(f32, usize)> = None;
            for cz in bz * per..(bz + 1) * per {
                for cx in bx * per..(bx + 1) * per {
                    let c = cz * zw + cx;
                    if m.zones[c] != ZoneId::Hills || ctx.route_blocked[c] == 1 {
                        continue;
                    }
                    let (x0, x1) = (cx.saturating_sub(1), (cx + 1).min(zw - 1));
                    let (z0, z1) = (cz.saturating_sub(1), (cz + 1).min(zh - 1));
                    let gx = (hgt[cz * zw + x1] - hgt[cz * zw + x0]) / ZONE_CELL_M;
                    let gz = (hgt[z1 * zw + cx] - hgt[z0 * zw + cx]) / ZONE_CELL_M;
                    let slope = sqrt(gx * gx + gz * gz);
                    if slope <= 0.4 && best.is_none_or(|b| slope < b.0) {
                        best = Some((slope, c));
                    }
                }
            }
            if let Some((_, c)) = best {
                let p = V2::new(
                    (c % zw) as f32 * ZONE_CELL_M + 8.0,
                    (c / zw) as f32 * ZONE_CELL_M + 8.0,
                );
                lattice.push(((p - dt).length(), p));
            }
        }
    }
    lattice.sort_by(|a, b| a.0.total_cmp(&b.0));
    targets.extend(lattice.iter().map(|l| l.1));
    for (k, &p) in targets.iter().enumerate() {
        let reach = if k < targets.len() - lattice.len() {
            60.0
        } else {
            0.5 * step
        };
        let near = g.edges.iter().filter(|e| e.alive).any(|e| {
            e.pts
                .windows(2)
                .any(|w| segment_distance(p, w[0], w[1]).0 < reach)
        });
        if near {
            continue;
        }
        let (root, count) = components(g);
        let mut main = 0usize;
        for (r, &c) in count.iter().enumerate() {
            if c > count[main] {
                main = r;
            }
        }
        if let Some((_, pts, ei, k, q)) = route(ctx, g, &root, &[p], None, main) {
            if polyline_length(&pts) < 60.0 {
                continue;
            }
            let id = split_edge(g, ei, k, q);
            let n = g.nodes.len() as u32;
            g.nodes.push(p);
            g.hw_arc.push(None);
            let mut pts = pts;
            pts.reverse();
            g.edges.push(GEdge {
                a: id,
                b: n,
                class: RoadClass::Rural,
                kind: Kind::Connector,
                pts,
                oneway: false,
                alive: true,
            });
        }
    }
}

/// `Rural` through hills and farmland, `Collector` elsewhere.
fn connector_class(ctx: &Ctx<'_>, pts: &[V2]) -> RoadClass {
    match majority_zone(ctx, pts) {
        ZoneId::Hills | ZoneId::Farmland => RoadClass::Rural,
        _ => RoadClass::Collector,
    }
}

/// Moves of the routing grid: 32 directions (every ~11°), `(dx, dz)` in cells.
const MOVES: [(i32, i32); 32] = [
    (1, 0),
    (3, 1),
    (2, 1),
    (3, 2),
    (1, 1),
    (2, 3),
    (1, 2),
    (1, 3),
    (0, 1),
    (-1, 3),
    (-1, 2),
    (-2, 3),
    (-1, 1),
    (-3, 2),
    (-2, 1),
    (-3, 1),
    (-1, 0),
    (-3, -1),
    (-2, -1),
    (-3, -2),
    (-1, -1),
    (-2, -3),
    (-1, -2),
    (-1, -3),
    (0, -1),
    (1, -3),
    (1, -2),
    (2, -3),
    (1, -1),
    (3, -2),
    (2, -1),
    (3, -1),
];

/// Grade limit of routed connectors between (smoothed) cell heights.
const ROUTE_GRADE: f32 = 0.115;

/// Longest routed connector.
const MAX_ROUTE_M: f32 = 3000.0;

/// A road from component `other` to component `main` over 16 m cells (Dijkstra, 32
/// directions), never steeper than [`ROUTE_GRADE`] between smoothed cell heights, on dry land, off
/// the highway and its cuts and clear of every other road: on slopes it winds and doubles
/// back like a hill road. Returns the index of the source it starts from, the smoothed path
/// and where it meets `main` (edge, segment, point).
fn route(
    ctx: &Ctx<'_>,
    g: &Graph,
    root: &[u32],
    sources: &[V2],
    other: Option<usize>,
    main: usize,
) -> Option<(usize, Vec<V2>, usize, usize, V2)> {
    use alloc::collections::BinaryHeap;
    use core::cmp::Reverse;
    let m = ctx.m;
    let (w, h) = (m.zone_width as usize, m.zone_height as usize);
    let center = |c: usize| {
        V2::new(
            (c % w) as f32 * ZONE_CELL_M + 8.0,
            (c / w) as f32 * ZONE_CELL_M + 8.0,
        )
    };
    let cell_of = |p: V2| {
        let cx = clamp(p.x / ZONE_CELL_M, 0.0, (w - 1) as f32) as usize;
        let cz = clamp(p.y / ZONE_CELL_M, 0.0, (h - 1) as f32) as usize;
        cz * w + cx
    };
    // 0 free, 1 blocked, 2 target.
    let mut state = ctx.route_blocked.clone();
    let height = &ctx.route_height;
    for e in g.edges.iter().filter(|e| e.alive) {
        let comp = root[e.a as usize] as usize;
        if Some(comp) == other {
            continue;
        }
        let target = comp == main && !matches!(e.kind, Kind::Highway | Kind::Ramp);
        for k in 0..e.pts.len() - 1 {
            let len = (e.pts[k + 1] - e.pts[k]).length();
            let n = (len / 8.0) as usize + 1;
            for j in 0..=n {
                let c = cell_of(e.pts[k].lerp(e.pts[k + 1], j as f32 / n as f32));
                state[c] = if target { 2 } else { 1 };
            }
        }
    }
    let inf = u32::MAX;
    let mut cost = vec![inf; w * h];
    let mut prev = vec![u32::MAX; w * h];
    let mut src_node = vec![u32::MAX; w * h];
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    for (k, &p) in sources.iter().enumerate() {
        let c = cell_of(p);
        if cost[c] != 0 {
            cost[c] = 0;
            src_node[c] = k as u32;
            heap.push(Reverse((0, c as u32)));
        }
    }
    let max = (MAX_ROUTE_M * 10.0) as u32;
    let mut found = None;
    while let Some(Reverse((d, c))) = heap.pop() {
        let c = c as usize;
        if d > cost[c] {
            continue;
        }
        if state[c] == 2 {
            found = Some(c);
            break;
        }
        if d > max {
            break;
        }
        let (cx, cz) = ((c % w) as i32, (c / w) as i32);
        for &(dx, dz) in &MOVES {
            let (nx, nz) = (cx + dx, cz + dz);
            if nx < 0 || nz < 0 || nx >= w as i32 || nz >= h as i32 {
                continue;
            }
            let nc = nz as usize * w + nx as usize;
            if state[nc] == 1 {
                continue;
            }
            let len = sqrt((dx * dx + dz * dz) as f32) * ZONE_CELL_M;
            let rise = height[nc] - height[c];
            if abs(rise) > ROUTE_GRADE * len {
                continue;
            }
            // Long moves pass other cells: passable, and near the straight climb.
            let steps = dx.abs().max(dz.abs());
            let mut ok = true;
            for j in 1..steps {
                let f = j as f32 / steps as f32;
                let mx = cx + (dx as f32 * f + 0.5 * dx.signum() as f32) as i32;
                let mz = cz + (dz as f32 * f + 0.5 * dz.signum() as f32) as i32;
                let mc =
                    mz.clamp(0, h as i32 - 1) as usize * w + mx.clamp(0, w as i32 - 1) as usize;
                if state[mc] == 1 || abs(height[mc] - (height[c] + rise * f)) > 1.5 {
                    ok = false;
                }
            }
            if !ok {
                continue;
            }
            let nd = d + (len * 10.0) as u32;
            if nd < cost[nc] {
                cost[nc] = nd;
                prev[nc] = c as u32;
                src_node[nc] = src_node[c];
                heap.push(Reverse((nd, nc as u32)));
            }
        }
    }
    let end = found?;
    let mut cells = vec![end];
    let mut c = end;
    while prev[c] != u32::MAX {
        c = prev[c] as usize;
        cells.push(c);
    }
    cells.reverse();
    let start = src_node[end] as usize;
    // Where it meets the main component: the nearest point of a target edge.
    let tip = center(end);
    let mut best: Option<(f32, usize, usize, V2)> = None;
    for (ei, e) in g.edges.iter().enumerate() {
        if !e.alive
            || root[e.a as usize] as usize != main
            || matches!(e.kind, Kind::Highway | Kind::Ramp)
        {
            continue;
        }
        for k in 0..e.pts.len() - 1 {
            let (d, t) = segment_distance(tip, e.pts[k], e.pts[k + 1]);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, ei, k, e.pts[k].lerp(e.pts[k + 1], t)));
            }
        }
    }
    let (_, ei, k, q) = best?;
    let mut raw: Vec<V2> = vec![sources[start]];
    raw.extend(
        cells
            .iter()
            .skip(1)
            .take(cells.len().saturating_sub(2))
            .map(|&c| center(c)),
    );
    raw.push(q);
    // Chaikin corner cutting, twice.
    let mut pts = raw.clone();
    for _ in 0..2 {
        let mut out = vec![pts[0]];
        for s in pts.windows(2) {
            out.push(s[0].lerp(s[1], 0.25));
            out.push(s[0].lerp(s[1], 0.75));
        }
        out.push(pts[pts.len() - 1]);
        pts = out;
    }
    if polyline_length(&pts) < 1.0 {
        return None;
    }
    Some((start, pts, ei, k, q))
}

fn connector_ok(ctx: &Ctx<'_>, g: &Graph, p: V2, q: V2, target: usize) -> bool {
    if !straight_ok(ctx, p, q) {
        return false;
    }
    for (i, e) in g.edges.iter().enumerate() {
        if !e.alive || i == target {
            continue;
        }
        for w in e.pts.windows(2) {
            if let Some((t, _)) = seg_intersect(p, q, w[0], w[1]) {
                if t > 1e-3 && t < 1.0 - 1e-3 {
                    return false;
                }
            }
        }
    }
    true
}

/// Splits live edge `ei` at point `q` on its segment `k`, returning the new node.
fn split_edge(g: &mut Graph, ei: usize, k: usize, q: V2) -> u32 {
    let id = g.nodes.len() as u32;
    g.nodes.push(q);
    g.hw_arc.push(None);
    let e = &g.edges[ei];
    let mut first: Vec<V2> = e.pts[..=k].to_vec();
    first.push(q);
    let mut second = vec![q];
    second.extend_from_slice(&e.pts[k + 1..]);
    let tail = GEdge {
        a: id,
        b: e.b,
        class: e.class,
        kind: e.kind,
        pts: second,
        oneway: e.oneway,
        alive: true,
    };
    g.edges[ei].pts = first;
    g.edges[ei].b = id;
    g.edges.push(tail);
    id
}

// ---------------------------------------------------------------------------------------
// Elevations and output.

fn finish(ctx: &Ctx<'_>, m: &MacroMap, g: Graph) -> RoadGraph {
    let hw = &ctx.hwy;
    // Keep only nodes in use, in order.
    let mut used = vec![false; g.nodes.len()];
    for e in &g.edges {
        used[e.a as usize] = true;
        used[e.b as usize] = true;
    }
    let mut remap = vec![u32::MAX; g.nodes.len()];
    let mut pos: Vec<V2> = Vec::new();
    let mut hw_node: Vec<Option<f32>> = Vec::new();
    for i in 0..g.nodes.len() {
        if used[i] {
            remap[i] = pos.len() as u32;
            pos.push(g.nodes[i]);
            hw_node.push(g.hw_arc[i]);
        }
    }
    struct Out {
        a: usize,
        b: usize,
        class: RoadClass,
        kind: Kind,
        pts: Vec<V2>,
        s: Vec<f32>,
        oneway: bool,
    }
    let mut es: Vec<Out> = g
        .edges
        .iter()
        .map(|e| {
            let pts = resample(&e.pts, PROFILE_STEP_M);
            let s = arcs(&pts);
            Out {
                a: remap[e.a as usize] as usize,
                b: remap[e.b as usize] as usize,
                class: e.class,
                kind: e.kind,
                pts,
                s,
                oneway: e.oneway,
            }
        })
        .collect();
    // Highway edges: the graded profile by arc length from the node the edge starts at.
    let mut profiles: Vec<Vec<f32>> = vec![Vec::new(); es.len()];
    let mut segs: Vec<([f32; 3], [f32; 3])> = Vec::new();
    for (i, e) in es.iter().enumerate() {
        if e.kind != Kind::Highway {
            continue;
        }
        let (sa, sb) = (
            hw_node[e.a].unwrap_or_else(|| hw.nearest(pos[e.a]).1),
            hw_node[e.b].unwrap_or_else(|| hw.nearest(pos[e.b]).1),
        );
        let total = e.s.last().copied().unwrap_or(0.0);
        let ys: Vec<f32> =
            e.s.iter()
                .map(|&s| {
                    let f = if total > 0.0 { s / total } else { 0.0 };
                    hw.y_at(lerp(sa, sb, f))
                })
                .collect();
        for k in 0..e.pts.len().saturating_sub(1) {
            segs.push((
                [e.pts[k].x, ys[k], e.pts[k].y],
                [e.pts[k + 1].x, ys[k + 1], e.pts[k + 1].y],
            ));
        }
        profiles[i] = ys;
    }
    let (w, h) = (m.width as usize, m.height as usize);
    let ground = |p: V2| sample_nodes(&m.heights, w, h, p);
    // Overpass crossings: (edge, arc along it, highway height there).
    let mut cross: Vec<(usize, f32, f32)> = Vec::new();
    for (i, e) in es.iter().enumerate() {
        if matches!(e.kind, Kind::Highway | Kind::Ramp) {
            continue;
        }
        let total = e.s.last().copied().unwrap_or(0.0);
        for k in 0..e.pts.len() - 1 {
            for sg in &segs {
                let (c, d) = (V2::new(sg.0[0], sg.0[2]), V2::new(sg.1[0], sg.1[2]));
                if let Some((t, _)) = seg_intersect(e.pts[k], e.pts[k + 1], c, d) {
                    let s = lerp(e.s[k], e.s[k + 1], t);
                    // Touching the highway at a shared node (the terminal) is no crossing.
                    let at_a = s < 1.0 && hw_node[e.a].is_some();
                    let at_b = s > total - 1.0 && hw_node[e.b].is_some();
                    if !at_a && !at_b {
                        let (_, u) =
                            seg_intersect(e.pts[k], e.pts[k + 1], c, d).unwrap_or((t, 0.0));
                        cross.push((i, s, lerp(sg.0[1], sg.1[1], u)));
                    }
                }
            }
        }
    }
    // Graph distance from the nearest crossing over ordinary roads (Dijkstra, O(V²)), and
    // that crossing's highway height.
    let nn = pos.len();
    let mut dist = vec![f32::INFINITY; nn];
    let mut deck_y = vec![0.0f32; nn];
    for &(i, s, y) in &cross {
        let e = &es[i];
        let total = e.s.last().copied().unwrap_or(0.0);
        if hw_node[e.a].is_none() && s < dist[e.a] {
            dist[e.a] = s;
            deck_y[e.a] = y;
        }
        if hw_node[e.b].is_none() && total - s < dist[e.b] {
            dist[e.b] = total - s;
            deck_y[e.b] = y;
        }
    }
    let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); nn];
    for e in &es {
        if matches!(e.kind, Kind::Highway | Kind::Ramp) {
            continue;
        }
        let len = e.s.last().copied().unwrap_or(0.0);
        adj[e.a].push((e.b, len));
        adj[e.b].push((e.a, len));
    }
    let mut done = vec![false; nn];
    loop {
        let mut u = usize::MAX;
        for i in 0..nn {
            if !done[i] && dist[i].is_finite() && (u == usize::MAX || dist[i] < dist[u]) {
                u = i;
            }
        }
        if u == usize::MAX {
            break;
        }
        done[u] = true;
        if dist[u] > 400.0 {
            break;
        }
        for &(v, len) in &adj[u] {
            if hw_node[v].is_none() && dist[u] + len < dist[v] {
                dist[v] = dist[u] + len;
                deck_y[v] = deck_y[u];
            }
        }
    }
    // An overpass deck: OVERPASS_LIFT_M above the highway where it crosses, falling at
    // APPROACH_GRADE beyond the flat span until it meets the ground (and never more than
    // the profile band above it).
    let deck = |d: f32, y: f32, g0: f32| {
        if d.is_finite() {
            let top = y + OVERPASS_LIFT_M - APPROACH_GRADE * (d - OVERPASS_FLAT_M).max(0.0);
            clamp(top, g0, g0 + PROFILE_BAND_M - 0.1)
        } else {
            g0
        }
    };
    let mut elev: Vec<f32> = (0..nn)
        .map(|i| match hw_node[i] {
            Some(s) => hw.y_at(s),
            None => deck(dist[i], deck_y[i], ground(pos[i])),
        })
        .collect();
    // Short edges between nodes at very different heights cannot hold the grade: pull
    // their (non-highway) ends together, a few sweeps, within the profile band.
    for _ in 0..16 {
        let mut moved = false;
        for e in &es {
            if matches!(e.kind, Kind::Highway | Kind::Ramp) {
                continue;
            }
            let len = e.s.last().copied().unwrap_or(0.0);
            let excess = abs(elev[e.a] - elev[e.b]) - 0.98 * FILTER_GRADE * len;
            if excess <= 1e-4 {
                continue;
            }
            let (fa, fb) = (hw_node[e.a].is_none(), hw_node[e.b].is_none());
            let (ka, kb) = match (fa, fb) {
                (true, true) => (0.5, 0.5),
                (true, false) => (1.0, 0.0),
                (false, true) => (0.0, 1.0),
                _ => continue,
            };
            let down = if elev[e.a] > elev[e.b] { 1.0 } else { -1.0 };
            for (n, k, sgn) in [(e.a, ka, -down), (e.b, kb, down)] {
                let g0 = ground(pos[n]);
                elev[n] = clamp(
                    elev[n] + sgn * k * excess,
                    g0 - PROFILE_BAND_M,
                    g0 + PROFILE_BAND_M,
                );
            }
            moved = true;
        }
        if !moved {
            break;
        }
    }
    for (i, e) in es.iter().enumerate() {
        if e.kind == Kind::Highway {
            continue;
        }
        let total = e.s.last().copied().unwrap_or(0.0);
        let (ya, yb) = (elev[e.a], elev[e.b]);
        if e.kind == Kind::Ramp {
            profiles[i] =
                e.s.iter()
                    .map(|&s| lerp(ya, yb, smoothstep(0.0, total.max(1e-3), s)))
                    .collect();
            continue;
        }
        let gr: Vec<f32> = e.pts.iter().map(|&p| ground(p)).collect();
        let target: Vec<f32> =
            e.s.iter()
                .zip(&gr)
                .map(|(&s, &g0)| {
                    let (mut d, mut y) = if dist[e.a] + s <= dist[e.b] + total - s {
                        (dist[e.a] + s, deck_y[e.a])
                    } else {
                        (dist[e.b] + total - s, deck_y[e.b])
                    };
                    for &(ci, cs, cy) in &cross {
                        if ci == i && abs(s - cs) < d {
                            d = abs(s - cs);
                            y = cy;
                        }
                    }
                    deck(d, y, g0)
                })
                .collect();
        profiles[i] = grade_limit(
            &target,
            &e.s,
            FILTER_GRADE,
            Some(ya),
            Some(yb),
            Some((&gr, PROFILE_BAND_M)),
        );
    }
    let nodes: Vec<Node> = (0..nn)
        .map(|i| Node {
            pos: [pos[i].x, pos[i].y],
            elevation: elev[i],
        })
        .collect();
    let edges: Vec<Edge> = es
        .iter_mut()
        .zip(profiles)
        .map(|(e, ys)| {
            let zone = majority_zone(ctx, &e.pts);
            let (lanes_fwd, lanes_back, width_m, sidewalk, speed_kph) = match (e.kind, e.class) {
                (Kind::Ramp, _) => (1, 0, RAMP_WIDTH_M, false, 60),
                (_, RoadClass::Highway) => (2, 2, HIGHWAY_WIDTH_M, false, 100),
                (_, RoadClass::Arterial) => (2, 2, ARTERIAL_WIDTH_M, true, 50),
                (_, RoadClass::Beachfront) => (1, 1, BEACHFRONT_WIDTH_M, true, 40),
                (_, RoadClass::Rural) => (
                    1,
                    1,
                    RURAL_WIDTH_M,
                    false,
                    if zone == ZoneId::Farmland { 90 } else { 60 },
                ),
                _ => (1, 1, COLLECTOR_WIDTH_M, true, 40),
            };
            let n = e.pts.len();
            let profile: Vec<[f32; 3]> = (0..n)
                .map(|k| {
                    let p = if k == 0 {
                        pos[e.a]
                    } else if k == n - 1 {
                        pos[e.b]
                    } else {
                        e.pts[k]
                    };
                    [p.x, ys[k], p.y]
                })
                .collect();
            Edge {
                a: e.a as u32,
                b: e.b as u32,
                class: e.class,
                lanes_fwd,
                lanes_back,
                width_m,
                sidewalk,
                speed_kph,
                oneway: e.oneway,
                profile,
            }
        })
        .collect();
    RoadGraph { nodes, edges }
}

fn majority_zone(ctx: &Ctx<'_>, pts: &[V2]) -> ZoneId {
    let mut count = [0u32; 12];
    for &p in pts {
        count[ctx.zone(p) as usize] += 1;
    }
    let mut best = 0;
    for k in 0..12 {
        if count[k] > count[best] {
            best = k;
        }
    }
    super::zones::ALL_ZONES[best]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_round_trips_directions() {
        for &(x, z) in &[
            (1.0f32, 0.0f32),
            (0.0, 1.0),
            (0.6, 0.8),
            (-0.6, 0.8),
            (0.8, -0.6),
        ] {
            let d = V2::new(x, z);
            let (m, c) = major_of(tensor(d));
            assert!((c - 1.0).abs() < 1e-5);
            assert!(abs(m.dot(d)) > 0.9999, "{d:?} -> {m:?}");
        }
    }

    #[test]
    fn grade_limit_holds_slope_and_ends() {
        let s: Vec<f32> = (0..40).map(|i| i as f32 * 16.0).collect();
        let target: Vec<f32> = s
            .iter()
            .map(|&x| if x > 300.0 { 20.0 } else { 0.0 })
            .collect();
        let y = grade_limit(&target, &s, 0.05, Some(0.0), None, None);
        assert_eq!(y[0], 0.0);
        assert_eq!(*y.last().unwrap(), 20.0);
        for k in 1..y.len() {
            assert!(abs(y[k] - y[k - 1]) <= 0.05 * 16.0 + 1e-4);
        }
    }

    #[test]
    fn segments_intersect() {
        let (t, u) = seg_intersect(
            V2::new(0.0, 0.0),
            V2::new(10.0, 0.0),
            V2::new(5.0, -5.0),
            V2::new(5.0, 5.0),
        )
        .unwrap();
        assert!((t - 0.5).abs() < 1e-6 && (u - 0.5).abs() < 1e-6);
        assert!(seg_intersect(
            V2::new(0.0, 0.0),
            V2::new(1.0, 0.0),
            V2::new(5.0, -5.0),
            V2::new(5.0, 5.0)
        )
        .is_none());
    }
}
