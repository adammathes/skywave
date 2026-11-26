//! An authored layout's macro map and arterials (`docs/LAYOUTS.md`, [`crate::layout`]):
//! the structure stages 1–3 would invent, read from the layout instead, then the shared
//! stages 4–7 ([`super::build::fill`]) over it.
//!
//! - **Heights** ([`MacroMap::heights`], 4 m samples): the layout's ground, each ridge and
//!   peak raised over it (the highest shape wins, rounded at its crest, roughened by its
//!   relief), a gentle roll, then every water polygon sunk to its depth with a shore slope
//!   [`SHORE_M`] wide outside it.
//! - **Zones** (16 m cells): `Water` where a cell's mean lies under the water level, else
//!   the last district whose polygon holds the cell's center, else the default zone. Urban
//!   zones then smooth the ground under them (the procedural stages' rule) except in
//!   `hilly` districts, and the land edges raise their walls and paint their bands.
//! - **Arterials**: each road's line, its free ends snapped onto the road they meet,
//!   split into edges at every crossing (all at grade), with a profile that follows the
//!   ground at most `max_grade` steep and rides `deck_m` over the water on bridges.
//!
//! Deterministic: the seed only roughens the terrain; the structure is the layout's.

use alloc::vec;
use alloc::vec::Vec;

use place::{Edge, Node, Place, RoadClass, RoadGraph, Spawn, SpawnKind, ZoneId};
use skywave_core::Seed;

use super::arterials::{grade_limit, seg_intersect};
use super::macro_map::{
    ccw, flatten_by_zone, paint_bands, raise_walls, Frame, MacroMap, Strip, WaterBody, WaterKind,
};
use super::terrain::{smoothstep, Noise, Shore};
use super::zones::cell_center;
use super::{stage, CELL_M, RES_PER_CHUNK, ZONE_SAMPLES};
use crate::edges;
use crate::grammar::Grammar;
use crate::layout::{self, Layout, MAX_GRADE, SNAP_M};
use crate::math::{abs, atan2, lerp, point_in_polygon, segment_distance, V2};
use crate::template::{CoastSide, RegionTemplate};
use crate::{ChunkPlan, Palette};

/// Width of the slope from the water's edge up to the land.
pub const SHORE_M: f32 = 60.0;
/// Ground at the water's edge stands this far above the water.
const SHORE_LIP_M: f32 = 0.8;
/// The water's bed reaches its full depth this far in from its edge.
const SHELF_M: f32 = 80.0;
/// A hilly district's own ground blends in over this distance inside its edge.
const HILLY_BLEND_M: f32 = 48.0;
/// Road profile sample spacing.
const STEP_M: f32 = 8.0;
/// The ground matches a road's profile this far past its curbs...
const CARVE_SHOULDER_M: f32 = 6.0;
/// ... and blends back to the land over this much more.
const CARVE_BLEND_M: f32 = 36.0;
/// The carved ground lies this far under the road.
const CARVE_UNDER_M: f32 = 0.05;
/// Nodes this close merge.
const MERGE_M: f32 = 2.0;
/// Hash key of the layout's terrain noise.
const KEY: u64 = 0x4c41_594f_5554;

/// The place `layout` describes for `seed`, dressed by `template`'s zone rules and
/// `grammar`. Both must have passed their checks.
pub fn build_place(
    seed: Seed,
    layout: &Layout,
    template: &RegionTemplate,
    grammar: &Grammar,
) -> Place {
    build_detailed(seed, layout, template, grammar).0
}

/// [`build_place`] and the plan its detail pass read, brought up to date for `palette`.
pub fn build_place_planned(
    seed: Seed,
    layout: &Layout,
    template: &RegionTemplate,
    grammar: &Grammar,
    palette: &Palette,
) -> (Place, ChunkPlan) {
    let (place, mut plan) = build_detailed(seed, layout, template, grammar);
    plan.redetail(&place, palette);
    (place, plan)
}

fn build_detailed(
    seed: Seed,
    layout: &Layout,
    template: &RegionTemplate,
    grammar: &Grammar,
) -> (Place, ChunkPlan) {
    let mut m = macro_map(seed, layout);
    let (roads, start) = arterials(layout, &mut m);
    super::build::fill(seed, template, grammar, &m, &roads, start)
}

/// The macro map of `layout`: heights, water, zones and edges (see the module docs).
pub fn macro_map(seed: Seed, layout: &Layout) -> MacroMap {
    let size_chunks = [layout.size_chunks.0, layout.size_chunks.1];
    let w = usize::from(size_chunks[0]) * RES_PER_CHUNK as usize;
    let h = usize::from(size_chunks[1]) * RES_PER_CHUNK as usize;
    let wl = layout.water_level_m;
    let edges = layout.edges.sides();
    let bands = edges::bands(&edges);

    // Shapes, as plain polylines and polygons.
    let ridges: Vec<(Vec<V2>, Vec<f32>, f32, f32, [f32; 4])> = layout
        .ridges
        .iter()
        .map(|r| {
            let pts = pts(&r.line);
            let bb = bbox(&pts, r.falloff_m);
            let hs = (0..pts.len()).map(|i| r.height_at(i)).collect();
            (pts, hs, r.falloff_m, r.relief_m, bb)
        })
        .collect();
    let waters: Vec<(Vec<V2>, f32, [f32; 4])> = layout
        .water
        .iter()
        .map(|wb| {
            let p = pts(&wb.polygon);
            let bb = bbox(&p, SHORE_M);
            (p, wb.depth_m, bb)
        })
        .collect();
    let relief = Noise::fbm(seed, &[stage::TERRAIN, KEY, 1], 220.0, 3);
    let roll = Noise::fbm(seed, &[stage::TERRAIN, KEY, 2], 600.0, 2);

    let mut heights = vec![0.0f32; w * h];
    for iz in 0..h {
        for ix in 0..w {
            let p = V2::new(ix as f32 * CELL_M, iz as f32 * CELL_M);
            let mut lift = 0.0f32;
            for (line, hs, falloff, rel, bb) in &ridges {
                if !in_box(p, bb) {
                    continue;
                }
                let (d, height) = nearest_on(p, line, hs);
                let t = 1.0 - smoothstep(0.0, *falloff, d);
                if t > 0.0 {
                    let r = rel * relief.at(p.x, p.y) * t;
                    lift = lift.max(height * t + r);
                }
            }
            for pk in &layout.peaks {
                let d = (p - V2::new(pk.at.0, pk.at.1)).length();
                let t = 1.0 - smoothstep(0.0, pk.falloff_m, d);
                lift = lift.max(pk.height_m * t);
            }
            let mut y = layout.ground_m + lift + 1.5 * roll.at(p.x, p.y);
            for (poly, depth, bb) in &waters {
                if !in_box(p, bb) {
                    continue;
                }
                let edge = polygon_distance(p, poly);
                if point_in_polygon(p, poly) {
                    let t = smoothstep(0.0, SHELF_M, edge);
                    y = wl - 0.6 - (depth - 0.6) * t;
                    break;
                }
                let lip = wl + SHORE_LIP_M;
                if edge < SHORE_M && y > lip {
                    y = lerp(lip, y, smoothstep(0.0, SHORE_M, edge));
                }
            }
            heights[iz * w + ix] = y;
        }
    }

    // Zones on 16 m cells.
    let n = ZONE_SAMPLES as usize;
    let (zw, zh) = (w / n, h / n);
    let districts: Vec<(Vec<V2>, ZoneId, bool)> = layout
        .districts
        .iter()
        .map(|d| (pts(&d.polygon), d.zone, d.hilly))
        .collect();
    let mut zones = vec![layout.default_zone; zw * zh];
    for cz in 0..zh {
        for cx in 0..zw {
            let mut acc = 0.0;
            for j in 0..n {
                for i in 0..n {
                    acc += heights[(cz * n + j) * w + cx * n + i];
                }
            }
            let i = cz * zw + cx;
            if acc / ((n * n) as f32) < wl {
                zones[i] = ZoneId::Water;
                continue;
            }
            let (x, z) = cell_center(cx, cz);
            let c = V2::new(x, z);
            if let Some(d) = districts.iter().rev().find(|d| point_in_polygon(c, &d.0)) {
                zones[i] = d.1;
            }
        }
    }

    // Grading: urban ground smoothed except in hilly districts, then the walls.
    let before = heights.clone();
    flatten_by_zone(&mut heights, w, h, &zones, zw, zh);
    for (poly, _, hilly) in &districts {
        if !*hilly {
            continue;
        }
        let bb = bbox(poly, 0.0);
        for iz in 0..h {
            for ix in 0..w {
                let p = V2::new(ix as f32 * CELL_M, iz as f32 * CELL_M);
                if !in_box(p, &bb) || !point_in_polygon(p, poly) {
                    continue;
                }
                let t = smoothstep(0.0, HILLY_BLEND_M, polygon_distance(p, poly));
                let i = iz * w + ix;
                heights[i] = lerp(heights[i], before[i], t);
            }
        }
    }
    raise_walls(seed, &mut heights, w, h, &edges, (wl - 60.0, 600.0));
    paint_bands(&heights, w, &edges, &bands, wl, &mut zones, zw, zh);

    let water = layout
        .water
        .iter()
        .map(|wb| WaterBody {
            kind: match wb.kind {
                layout::WaterKind::Sea => WaterKind::Ocean,
                layout::WaterKind::Lake => WaterKind::Lake,
            },
            polygon: ccw(wb.polygon.iter().map(|&(x, z)| [x, z]).collect()),
        })
        .collect();
    let downtown = layout
        .districts
        .iter()
        .find(|d| d.zone == ZoneId::Downtown)
        .map_or([w as f32 * CELL_M * 0.5, h as f32 * CELL_M * 0.5], |d| {
            let p = pts(&d.polygon);
            let c = p.iter().fold(V2::new(0.0, 0.0), |a, &q| a + q) * (1.0 / p.len() as f32);
            [c.x, c.y]
        });
    let frame = Frame::new(CoastSide::West, w as u32, h as u32);
    MacroMap {
        seed,
        size_chunks,
        frame,
        width: w as u32,
        height: h as u32,
        corridor: vec![0.0; w * h],
        heights,
        water_level: wl,
        water,
        zone_width: zw as u32,
        zone_height: zh as u32,
        zones,
        shore: Shore::far(frame.n_v()),
        downtown,
        strip: Strip {
            centerline: Vec::new(),
            inland_m: 0.0,
            width_m: 0.0,
            length_m: 0.0,
        },
        highway: Vec::new(),
        highway_bed: Vec::new(),
        pass: None,
        pois: Vec::new(),
        edges,
        bands,
        basin: None,
    }
}

/// The layout's roads as a graph (see the module docs) and the player's start on them.
pub fn arterials(layout: &Layout, m: &mut MacroMap) -> (RoadGraph, Option<Spawn>) {
    let wl = m.water_level;
    // Lines, free ends snapped onto the road they meet.
    let mut lines: Vec<Vec<V2>> = layout
        .roads
        .iter()
        .map(|r| layout.road_line(r).unwrap_or_default())
        .collect();
    for i in 0..lines.len() {
        let closed =
            lines[i].len() >= 2 && (lines[i][0] - lines[i][lines[i].len() - 1]).length() < 1.0;
        if closed {
            continue;
        }
        for end in 0..2 {
            if end == 1 && layout.roads[i].dead_end {
                continue;
            }
            let p = if end == 0 {
                lines[i][0]
            } else {
                lines[i][lines[i].len() - 1]
            };
            let mut best: Option<(f32, V2)> = None;
            for (j, l) in lines.iter().enumerate() {
                if j == i {
                    continue;
                }
                for s in l.windows(2) {
                    let (d, t) = segment_distance(p, s[0], s[1]);
                    if d <= SNAP_M && best.is_none_or(|b| d < b.0) {
                        best = Some((d, s[0].lerp(s[1], t)));
                    }
                }
            }
            if let Some((d, q)) = best {
                let line = &mut lines[i];
                match (end, d < 1.0) {
                    (0, true) => line[0] = q,
                    (0, false) => line.insert(0, q),
                    (_, true) => {
                        let k = line.len() - 1;
                        line[k] = q;
                    }
                    (_, false) => line.push(q),
                }
            }
        }
    }

    // Nodes: every crossing and every end.
    let mut nodes: Vec<V2> = Vec::new();
    let node_at = |nodes: &mut Vec<V2>, p: V2| -> usize {
        if let Some(i) = nodes.iter().position(|&q| (q - p).length() < MERGE_M) {
            return i;
        }
        nodes.push(p);
        nodes.len() - 1
    };
    // Cuts per road: (arc length, node).
    let mut cuts: Vec<Vec<(f32, usize)>> = vec![Vec::new(); lines.len()];
    for i in 0..lines.len() {
        let a = node_at(&mut nodes, lines[i][0]);
        let b = node_at(&mut nodes, lines[i][lines[i].len() - 1]);
        cuts[i].push((0.0, a));
        cuts[i].push((layout::line_length(&lines[i]), b));
    }
    for i in 0..lines.len() {
        for j in i..lines.len() {
            let mut si = 0.0;
            for (ki, a) in lines[i].windows(2).enumerate() {
                let la = (a[1] - a[0]).length();
                let mut sj = 0.0;
                for (kj, b) in lines[j].windows(2).enumerate() {
                    let lb = (b[1] - b[0]).length();
                    // A line meets itself only away from its own neighbouring segments.
                    if i == j && kj <= ki + 1 {
                        sj += lb;
                        continue;
                    }
                    if let Some((t, u)) = seg_intersect(a[0], a[1], b[0], b[1]) {
                        let p = a[0].lerp(a[1], t);
                        let nd = node_at(&mut nodes, p);
                        cuts[i].push((si + t * la, nd));
                        cuts[j].push((sj + u * lb, nd));
                    }
                    sj += lb;
                }
                si += la;
            }
        }
    }
    // A snapped end lies on the road it met: cut that road there too.
    for i in 0..lines.len() {
        for end in [lines[i][0], lines[i][lines[i].len() - 1]] {
            let nd = node_at(&mut nodes, end);
            for (j, l) in lines.iter().enumerate() {
                if j == i {
                    continue;
                }
                let mut s = 0.0;
                for seg in l.windows(2) {
                    let len = (seg[1] - seg[0]).length();
                    let (d, t) = segment_distance(end, seg[0], seg[1]);
                    if d < 0.5 {
                        cuts[j].push((s + t * len, nd));
                        break;
                    }
                    s += len;
                }
            }
        }
    }

    // Edges between consecutive cuts.
    struct Piece {
        road: usize,
        a: usize,
        b: usize,
        pts: Vec<V2>,
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for (i, c) in cuts.iter_mut().enumerate() {
        c.sort_by(|x, y| x.0.total_cmp(&y.0));
        c.dedup_by(|x, y| x.1 == y.1 && abs(x.0 - y.0) < 1.0);
        for k in 0..c.len().saturating_sub(1) {
            let ((s0, a), (s1, b)) = (c[k], c[k + 1]);
            if s1 - s0 < 1.0 || a == b && s1 - s0 < 10.0 {
                continue;
            }
            let pts = densify(&sub_line(&lines[i], s0, s1, nodes[a], nodes[b]), STEP_M);
            pieces.push(Piece { road: i, a, b, pts });
        }
    }

    // Heights: the ground, or a deck over the water, graded; then the ground is cut and
    // filled to meet the roads ([`carve`]).
    let ground = |p: V2| m.height_at(p.x, p.y);
    let over_water = |p: V2| ground(p) < wl + 0.3;
    let target = |p: V2, deck: f32| if over_water(p) { wl + deck } else { ground(p) };
    let mut elev: Vec<Option<f32>> = vec![None; nodes.len()];
    for pc in &pieces {
        let deck = layout.roads[pc.road].deck_m;
        for nd in [pc.a, pc.b] {
            let y = target(nodes[nd], deck);
            elev[nd] = Some(elev[nd].map_or(y, |e: f32| e.max(y)));
        }
    }
    let node_y: Vec<f32> = elev.iter().map(|e| e.unwrap_or(0.0)).collect();
    let mut bridged: Vec<Vec<bool>> = Vec::new();
    let edges: Vec<Edge> = pieces
        .iter()
        .map(|pc| {
            let r = &layout.roads[pc.road];
            let s = arcs(&pc.pts);
            let tgt: Vec<f32> = pc.pts.iter().map(|&p| target(p, r.deck_m)).collect();
            bridged.push(pc.pts.iter().map(|&p| over_water(p)).collect());
            let ys = grade_limit(
                &tgt,
                &s,
                r.max_grade.unwrap_or(MAX_GRADE),
                Some(node_y[pc.a]),
                Some(node_y[pc.b]),
                None,
            );
            let (width_m, speed) = match (r.class, r.lanes) {
                (RoadClass::Highway, l) => (f32::from(l) * 7.4 + 7.2, 100),
                (RoadClass::Rural, _) => (8.0, 70),
                (_, 1) => (12.0, 50),
                (_, 2) => (14.0, 60),
                (_, l) => (f32::from(l) * 6.8 + 1.8, 60),
            };
            Edge {
                a: pc.a as u32,
                b: pc.b as u32,
                class: r.class,
                lanes_fwd: r.lanes,
                lanes_back: r.lanes,
                width_m,
                sidewalk: r.sidewalk && r.class != RoadClass::Highway,
                speed_kph: r.speed_kph.unwrap_or(speed),
                oneway: false,
                profile: pc
                    .pts
                    .iter()
                    .zip(&ys)
                    .map(|(p, &y)| [p.x, y, p.y])
                    .collect(),
            }
        })
        .collect();
    carve(m, &edges, &bridged);
    let graph = RoadGraph {
        nodes: nodes
            .iter()
            .zip(&node_y)
            .map(|(p, &y)| Node {
                pos: [p.x, p.y],
                elevation: y,
            })
            .collect(),
        edges,
    };
    let start = start(layout, &lines, &graph);
    (graph, start)
}

/// Cuts and fills the ground to meet the roads: under a road and its shoulders the ground
/// is the road's profile (a hair under it); beyond, it blends back to the ground over
/// [`CARVE_BLEND_M`]. Where two roads' reaches overlap the one with the firmer hold wins.
/// Bridge spans (`bridged`, per profile point) leave the water's bed alone.
fn carve(m: &mut MacroMap, edges: &[Edge], bridged: &[Vec<bool>]) {
    let (w, h) = (m.width as usize, m.height as usize);
    let mut hold = vec![0.0f32; w * h];
    let mut at = vec![0.0f32; w * h];
    for (e, br) in edges.iter().zip(bridged) {
        let core = 0.5 * e.width_m + CARVE_SHOULDER_M;
        let reach = core + CARVE_BLEND_M;
        for k in 0..e.profile.len().saturating_sub(1) {
            if br[k] && br[k + 1] {
                continue;
            }
            let (a, b) = (e.profile[k], e.profile[k + 1]);
            let (pa, pb) = (V2::new(a[0], a[2]), V2::new(b[0], b[2]));
            let x0 = ((pa.x.min(pb.x) - reach) / CELL_M).max(0.0) as usize;
            let z0 = ((pa.y.min(pb.y) - reach) / CELL_M).max(0.0) as usize;
            let x1 = (((pa.x.max(pb.x) + reach) / CELL_M) as usize + 1).min(w - 1);
            let z1 = (((pa.y.max(pb.y) + reach) / CELL_M) as usize + 1).min(h - 1);
            for iz in z0..=z1 {
                for ix in x0..=x1 {
                    let p = V2::new(ix as f32 * CELL_M, iz as f32 * CELL_M);
                    let (d, t) = segment_distance(p, pa, pb);
                    if d >= reach {
                        continue;
                    }
                    let f = 1.0 - smoothstep(core, reach, d);
                    let i = iz * w + ix;
                    if f > hold[i] {
                        hold[i] = f;
                        at[i] = lerp(a[1], b[1], t) - CARVE_UNDER_M;
                    }
                }
            }
        }
    }
    for i in 0..w * h {
        if hold[i] > 0.0 {
            m.heights[i] = lerp(m.heights[i], at[i], hold[i]);
        }
    }
}

/// The player's start: in the outer lane on the side traffic keeps to (right), facing
/// along the start road, on its profile.
fn start(layout: &Layout, lines: &[Vec<V2>], g: &RoadGraph) -> Option<Spawn> {
    let ri = layout
        .roads
        .iter()
        .position(|r| r.name == layout.start.road)?;
    let line = &lines[ri];
    let mut s = layout.start.along_m;
    let (mut p, mut dir) = (line[0], (line[1] - line[0]).normalize());
    for seg in line.windows(2) {
        let len = (seg[1] - seg[0]).length();
        dir = (seg[1] - seg[0]).normalize();
        if s <= len {
            p = seg[0].lerp(seg[1], s / len.max(1e-6));
            break;
        }
        s -= len;
        p = seg[1];
    }
    // The edge under the start for its width and height.
    let mut best: Option<(f32, &Edge, f32)> = None;
    for e in &g.edges {
        for k in e.profile.windows(2) {
            let (a, b) = (V2::new(k[0][0], k[0][2]), V2::new(k[1][0], k[1][2]));
            let (d, t) = segment_distance(p, a, b);
            if best.is_none_or(|q| d < q.0) {
                best = Some((d, e, lerp(k[0][1], k[1][1], t)));
            }
        }
    }
    let (_, e, y) = best?;
    let inner = e.width_m * 0.5 - 0.45;
    let lanes = f32::from(e.lanes_fwd.max(1));
    let q = p + dir.right() * (inner - 0.5 * inner / lanes);
    Some(Spawn {
        pos: [q.x, y, q.y],
        heading: atan2(-dir.x, -dir.y),
        kind: SpawnKind::PlayerStart,
    })
}

/// Distance from `p` to `line` and the value of `hs` (one per point) interpolated at the
/// nearest point.
fn nearest_on(p: V2, line: &[V2], hs: &[f32]) -> (f32, f32) {
    let mut best = (f32::INFINITY, 0.0);
    for i in 0..line.len() - 1 {
        let (d, t) = segment_distance(p, line[i], line[i + 1]);
        if d < best.0 {
            best = (d, lerp(hs[i], hs[i + 1], t));
        }
    }
    best
}

fn pts(v: &[(f32, f32)]) -> Vec<V2> {
    v.iter().map(|&(x, z)| V2::new(x, z)).collect()
}

/// `[x0, z0, x1, z1]` of `pts`, grown by `grow`.
fn bbox(pts: &[V2], grow: f32) -> [f32; 4] {
    let mut b = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for p in pts {
        b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
    }
    [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow]
}

fn in_box(p: V2, b: &[f32; 4]) -> bool {
    p.x >= b[0] && p.y >= b[1] && p.x <= b[2] && p.y <= b[3]
}

/// Distance from `p` to the boundary of `poly`.
fn polygon_distance(p: V2, poly: &[V2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| segment_distance(p, poly[i], poly[(i + 1) % n]).0)
        .fold(f32::INFINITY, f32::min)
}

/// The part of `line` between arc lengths `s0` and `s1`, its ends exactly `a` and `b`.
fn sub_line(line: &[V2], s0: f32, s1: f32, a: V2, b: V2) -> Vec<V2> {
    let mut out = vec![a];
    let mut s = 0.0;
    for p in line.windows(2) {
        s += (p[1] - p[0]).length();
        if s > s0 + 0.5 && s < s1 - 0.5 {
            out.push(p[1]);
        }
    }
    out.push(b);
    out
}

/// `line` with points added so no segment is longer than `step`.
fn densify(line: &[V2], step: f32) -> Vec<V2> {
    let mut out = vec![line[0]];
    for p in line.windows(2) {
        let len = (p[1] - p[0]).length();
        let k = (len / step).max(1.0) as usize;
        let k = if (len / k as f32) > step { k + 1 } else { k };
        for i in 1..=k {
            out.push(p[0].lerp(p[1], i as f32 / k as f32));
        }
    }
    out
}

/// Cumulative arc length along `pts`.
fn arcs(pts: &[V2]) -> Vec<f32> {
    let mut s = vec![0.0; pts.len()];
    for i in 1..pts.len() {
        s[i] = s[i - 1] + (pts[i] - pts[i - 1]).length();
    }
    s
}
