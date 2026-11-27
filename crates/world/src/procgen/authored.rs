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
//!   split into edges at every crossing, with a profile that follows the ground at most
//!   `max_grade` steep and rides `deck_m` over the water on bridges. A `Highway` is
//!   grade-separated: a road crossing it meets nothing and rises to a deck
//!   [`OVERPASS_LIFT_M`] over it; the roads its `ramps` name get a diamond of one-way
//!   ramps; its right of way is zoned `Highway`, which keeps streets and lots off it.
//! - **Channels** ([`channels`]): cut into the ground after the roads are laid, so the
//!   roads that cross one bridge it; zoned `Highway` (dry) or `Water` (wet).
//!
//! Deterministic: the seed only roughens the terrain; the structure is the layout's.

use alloc::vec;
use alloc::vec::Vec;

use place::{Edge, Node, Place, RoadClass, RoadGraph, Spawn, SpawnKind, ZoneId};
use skywave_core::Seed;

use super::arterials::grade_limit;
use super::macro_map::{
    ccw, flatten_by_zone, paint_bands, raise_walls, BasinPlan, Frame, GridLine, MacroMap, MacroPoi,
    MacroPoiKind, Strip, WaterBody, WaterKind,
};
use super::profile::{ease, grade_at, ROAD_CURVE, STREET_CURVE};
use super::terrain::{smoothstep, Noise, Shore};
use super::zones::cell_center;
use super::{stage, CELL_M, RES_PER_CHUNK, ZONE_CELL_M, ZONE_SAMPLES};
use crate::edges;
use crate::grammar::Grammar;
use crate::layout::{self, point_at, Layout, MAX_GRADE, RAMP_RUN_M, SNAP_M};
use crate::math::{abs, atan2, clamp, lerp, point_in_polygon, seg_intersect, segment_distance, V2};
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
/// A road crossing a highway rides this far over it...
pub const OVERPASS_LIFT_M: f32 = 7.0;
/// ... level for this far past the highway's edge, then down at this grade.
const OVERPASS_FLAT_M: f32 = 9.0;
const APPROACH_GRADE: f32 = 0.04;
/// Steepest highway grade unless the road says otherwise.
const HIGHWAY_GRADE: f32 = 0.05;
/// A ramp meets its crossing road where the approach has come down this far from the deck.
const RAMP_FOOT_DROP_M: f32 = 1.0;
/// A ramp stays level with the highway this far from where it leaves it.
const RAMP_MERGE_M: f32 = 30.0;
/// A one-lane ramp with shoulders.
const RAMP_WIDTH_M: f32 = 8.0;
/// The right of way reaches this far past a highway's or a ramp's edge.
const RIGHT_OF_WAY_M: f32 = 10.0;
/// The foot of the hills is the straight road with the highest ground this far to one side.
const FOOT_REACH_M: f32 = 300.0;
/// A channel fades in over this length at an end that lies inside the place.
const CHANNEL_HEAD_M: f32 = 40.0;
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
    let (m, roads, start) = skeleton(seed, layout);
    super::build::fill(seed, template, grammar, &m, &roads, start)
}

/// Everything the layout says: its macro map with the roads laid and the channels cut,
/// the main roads and the start.
pub fn skeleton(seed: Seed, layout: &Layout) -> (MacroMap, RoadGraph, Option<Spawn>) {
    let mut m = macro_map(seed, layout);
    let (roads, start) = arterials(layout, &mut m);
    let bridge_heights = m.heights.clone();
    channels(layout, &mut m);
    m.basin = Some(plan(layout, &m, &roads, bridge_heights));
    m.highway = roads
        .edges
        .iter()
        .filter(|e| e.class == RoadClass::Highway)
        .flat_map(|e| e.profile.iter().step_by(2).map(|p| [p[0], p[2]]))
        .collect();
    // A lookout at the end of each dead-end road, facing downtown.
    let downtown = V2::from_array(m.downtown);
    for r in layout.roads.iter().filter(|r| r.dead_end) {
        let Some(&end) = layout.road_line(r).as_ref().and_then(|l| l.last()) else {
            continue;
        };
        m.pois.push(MacroPoi {
            kind: MacroPoiKind::Overlook,
            pos: [end.x, end.y],
            facing: (downtown - end).normalize().to_array(),
        });
    }
    (m, roads, start)
}

/// What the set pieces, the track and the detailing read of the layout's structure
/// ([`BasinPlan`], as a generated basin fills it): the straight roads as grid lines (those
/// of three lanes or more the boulevards), the first of them the one along the foot of the
/// highest ground; the first dry channel; the rail line; the airfield.
fn plan(layout: &Layout, m: &MacroMap, g: &RoadGraph, bridge_heights: Vec<f32>) -> BasinPlan {
    let h = |p: V2| m.height_at(p.x, p.y);
    // Straight roads, their ends where the graph has them (snapped onto the roads they meet).
    let node = |p: V2| {
        g.nodes
            .iter()
            .map(|n| V2::from_array(n.pos))
            .min_by(|a, b| (*a - p).length().total_cmp(&(*b - p).length()))
            .filter(|q| (*q - p).length() <= SNAP_M + 1.0)
            .unwrap_or(p)
    };
    let mut lines: Vec<GridLine> = Vec::new();
    // The rise to one side of each line, a few hundred meters off: (rise, way out).
    let mut foot: (f32, usize, V2) = (0.0, 0, V2::new(0.0, 1.0));
    for r in &layout.roads {
        let line = layout.road_line(r).unwrap_or_default();
        if line.len() != 2 || r.class == RoadClass::Highway {
            continue;
        }
        let (a, b) = (node(line[0]), node(line[1]));
        let (mid, side) = (a.lerp(b, 0.5), (b - a).normalize().right());
        for out in [side, -side] {
            let up = mid - out * FOOT_REACH_M;
            let rise = h(up) - h(mid);
            // A slope inside the place: one you can stand below and look up at.
            if rise > foot.0 && m.in_playable(up.x, up.y, 0.0) {
                foot = (rise, lines.len(), out);
            }
        }
        lines.push(GridLine {
            a: a.to_array(),
            b: b.to_array(),
            boulevard: r.lanes >= 3,
        });
    }
    if !lines.is_empty() {
        lines.swap(0, foot.1);
    }
    let dense = |line: &[(f32, f32)]| -> Vec<[f32; 2]> {
        densify(&pts(line), ZONE_CELL_M)
            .iter()
            .map(|p| p.to_array())
            .collect()
    };
    let wash = layout.channels.iter().find(|c| !c.wet);
    let field = layout.airfield.as_ref();
    let pt = |p: (f32, f32)| [p.0, p.1];
    BasinPlan {
        lines,
        cross: 0,
        freeway_ends: [0, 0],
        boulevard_lanes: 3,
        median_m: 0.0,
        channel: wash.map_or(Vec::new(), |c| dense(&c.line)),
        channel_half_m: wash.map_or((0.0, 0.0), |c| (0.5 * c.width_m, c.lip_half_m())),
        ramps: Vec::new(),
        rail: layout.rail.as_ref().map_or(Vec::new(), |r| dense(&r.line)),
        spurs: layout
            .rail
            .as_ref()
            .map_or(Vec::new(), |r| r.spurs.iter().map(|s| dense(s)).collect()),
        airfield: field.map_or(Vec::new(), |a| {
            a.corners().iter().map(|p| p.to_array()).collect()
        }),
        runway: field.map_or([[0.0; 2]; 2], |a| [pt(a.runway.0), pt(a.runway.1)]),
        runway_m: field.map_or(0.0, |a| a.runway_m),
        taxiway: field.map_or([[0.0; 2]; 2], |a| [pt(a.runway.0), pt(a.taxiway)]),
        bridge_heights,
        square: false,
        out: foot.2.to_array(),
        set_pieces: layout.set_pieces,
    }
}

/// A ridge ready to sample: its line, a height per point, falloff, relief and bounding box.
type RidgeShape = (Vec<V2>, Vec<f32>, f32, f32, [f32; 4]);

/// The macro map of `layout`: heights, water, zones and edges (see the module docs).
pub fn macro_map(seed: Seed, layout: &Layout) -> MacroMap {
    let size_chunks = [layout.size_chunks.0, layout.size_chunks.1];
    let w = usize::from(size_chunks[0]) * RES_PER_CHUNK as usize;
    let h = usize::from(size_chunks[1]) * RES_PER_CHUNK as usize;
    let wl = layout.water_level_m;
    let edges = layout.edges.sides();
    let bands = edges::bands(&edges);

    // Shapes, as plain polylines and polygons.
    let ridges: Vec<RidgeShape> = layout
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

    // The rail line's right of way and the airfield: nothing is built there.
    let field = layout.airfield.as_ref().map(|a| a.corners());
    let tracks: Vec<Vec<V2>> = layout
        .rail
        .iter()
        .flat_map(|r| core::iter::once(&r.line).chain(&r.spurs))
        .map(|l| pts(l))
        .collect();
    for cz in 0..zh {
        for cx in 0..zw {
            let (x, z) = cell_center(cx, cz);
            let c = V2::new(x, z);
            let i = cz * zw + cx;
            if zones[i] != ZoneId::Water
                && (field.is_some_and(|f| point_in_polygon(c, &f))
                    || tracks
                        .iter()
                        .any(|t| layout::distance_to_line(c, t) <= layout::RAIL_ROW_M))
            {
                zones[i] = ZoneId::Highway;
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
        pois: layout
            .pier
            .iter()
            .map(|p| {
                let out = (V2::new(p.toward.0, p.toward.1) - V2::new(p.at.0, p.at.1)).normalize();
                MacroPoi {
                    kind: MacroPoiKind::Pier,
                    pos: [p.at.0, p.at.1],
                    facing: [out.x, out.y],
                }
            })
            .collect(),
        edges,
        bands,
        basin: None,
    }
}

/// What a line of the road network is: a layout road, a highway, or one of its ramps.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Road,
    Highway,
    Ramp,
    /// An airfield's runway: a road as wide as the airfield says.
    Runway,
}

/// Half the width of a highway with `lanes` each way.
fn highway_half(lanes: u8) -> f32 {
    0.5 * (f32::from(lanes) * 7.4 + 7.2)
}

/// The diamond ramps of every highway's interchanges: each a line from the highway
/// [`RAMP_RUN_M`] before or after the crossing to the crossing road where its approach is
/// nearly at deck height, with the layout road (the highway) it belongs to. Off-ramps run
/// highway to road and leave on the right before the crossing; on-ramps run road to highway.
fn ramps(layout: &Layout, lines: &[Vec<V2>]) -> Vec<(usize, Vec<V2>)> {
    let mut out = Vec::new();
    for (i, r) in layout.roads.iter().enumerate() {
        let foot = highway_half(r.lanes) + OVERPASS_FLAT_M + RAMP_FOOT_DROP_M / APPROACH_GRADE;
        for name in &r.ramps {
            let Some(j) = layout.roads.iter().position(|q| &q.name == name) else {
                continue;
            };
            let Some((si, sj, c)) = layout::crossing(&lines[i], &lines[j]) else {
                continue;
            };
            let tan = point_at(&lines[i], si).1;
            let right = tan.right();
            for along in [foot, -foot] {
                let (f, _) = point_at(&lines[j], sj + along);
                if (f - c).length() < 0.9 * foot {
                    continue;
                }
                let side = if (f - c).dot(right) >= 0.0 { 1.0 } else { -1.0 };
                for delta in [-1.0f32, 1.0] {
                    let (h, _) = point_at(&lines[i], si + delta * RAMP_RUN_M);
                    let p1 = h - tan * (delta * 60.0) + right * (side * 10.0);
                    let p2 = f + tan * (delta * 40.0);
                    let mut pts = bezier(h, p1, p2, f);
                    if delta != -side {
                        pts.reverse();
                    }
                    out.push((i, pts));
                }
            }
        }
    }
    out
}

/// A cubic Bézier from `p0` to `p3` in steps of about [`STEP_M`].
fn bezier(p0: V2, p1: V2, p2: V2, p3: V2) -> Vec<V2> {
    let est = (p1 - p0).length() + (p2 - p1).length() + (p3 - p2).length();
    let n = ((est / STEP_M) as usize).max(4);
    (0..=n)
        .map(|k| {
            let t = k as f32 / n as f32;
            let u = 1.0 - t;
            p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t)
        })
        .collect()
}

/// The layout's roads as a graph (see the module docs) and the player's start on them.
pub fn arterials(layout: &Layout, m: &mut MacroMap) -> (RoadGraph, Option<Spawn>) {
    let wl = m.water_level;
    // The layout's roads, then an airfield's runway and its taxiway as two more.
    let mut roads = layout.roads.clone();
    let mut lines: Vec<Vec<V2>> = roads
        .iter()
        .map(|r| layout.road_line(r).unwrap_or_default())
        .collect();
    if let Some(a) = &layout.airfield {
        for (name, to, dead_end, speed) in [
            ("runway", a.runway.1, true, 40),
            ("taxiway", a.taxiway, false, 30),
        ] {
            lines.push(pts(&[a.runway.0, to]));
            roads.push(layout::Road {
                name: name.into(),
                class: RoadClass::Collector,
                lanes: 1,
                points: Vec::new(),
                from: None,
                toward: None,
                bearing_deg: None,
                length_m: None,
                speed_kph: Some(speed),
                max_grade: Some(0.03),
                sidewalk: false,
                dead_end,
                deck_m: layout::DECK_M,
                ramps: Vec::new(),
            });
        }
    }
    let runway = layout.airfield.as_ref().map(|_| layout.roads.len());
    // Free ends snap onto the road they meet.
    for i in 0..lines.len() {
        let closed =
            lines[i].len() >= 2 && (lines[i][0] - lines[i][lines[i].len() - 1]).length() < 1.0;
        if closed {
            continue;
        }
        for end in 0..2 {
            if end == 1 && roads[i].dead_end {
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
    // Each line's layout road and kind; the ramps follow the layout's own roads.
    let mut road: Vec<usize> = (0..lines.len()).collect();
    let mut kind: Vec<Kind> = roads
        .iter()
        .enumerate()
        .map(|(i, r)| match r.class {
            RoadClass::Highway => Kind::Highway,
            _ if runway == Some(i) => Kind::Runway,
            _ => Kind::Road,
        })
        .collect();
    for (i, pts) in ramps(layout, &lines) {
        road.push(i);
        kind.push(Kind::Ramp);
        lines.push(pts);
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
    // Where a road passes over a highway: (line, arc along it, the highway's line, point).
    let mut over: Vec<(usize, f32, usize, V2)> = Vec::new();
    for i in 0..lines.len() {
        for j in i..lines.len() {
            // Ramps meet roads only at their ends.
            if kind[i] == Kind::Ramp || kind[j] == Kind::Ramp {
                continue;
            }
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
                        match (kind[i], kind[j]) {
                            (Kind::Road, Kind::Highway) => over.push((i, si + t * la, j, p)),
                            (Kind::Highway, Kind::Road) => over.push((j, sj + u * lb, i, p)),
                            _ => {
                                let nd = node_at(&mut nodes, p);
                                cuts[i].push((si + t * la, nd));
                                cuts[j].push((sj + u * lb, nd));
                            }
                        }
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
    // A road that ends on a highway meets it at grade; one passing through does not.
    over.retain(|&(i, s, _, p)| {
        !cuts[i]
            .iter()
            .any(|&(c, nd)| abs(c - s) < 1.0 && (nodes[nd] - p).length() < MERGE_M)
    });

    // Edges between consecutive cuts.
    struct Piece {
        line: usize,
        a: usize,
        b: usize,
        /// Arc length along the line at `a`.
        s0: f32,
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
            pieces.push(Piece {
                line: i,
                a,
                b,
                s0,
                pts,
            });
        }
    }

    // Heights: the ground, or a deck over the water, graded; then the ground is cut and
    // filled to meet the roads ([`carve`]).
    let ground = |p: V2| m.height_at(p.x, p.y);
    let over_water = |p: V2| ground(p) < wl + 0.3;
    let target = |p: V2, deck: f32| if over_water(p) { wl + deck } else { ground(p) };
    let mut elev: Vec<Option<f32>> = vec![None; nodes.len()];
    let mut on_highway = vec![false; nodes.len()];
    for pc in &pieces {
        let deck = roads[road[pc.line]].deck_m;
        for nd in [pc.a, pc.b] {
            let y = target(nodes[nd], deck);
            elev[nd] = Some(elev[nd].map_or(y, |e: f32| e.max(y)));
            on_highway[nd] |= kind[pc.line] == Kind::Highway;
        }
    }
    let mut node_y: Vec<f32> = elev.iter().map(|e| e.unwrap_or(0.0)).collect();

    // A road takes its heights whole, along its length, so it runs smooth through its
    // junctions; the roads it meets are then held to it there. The highways first (the roads
    // over them and the ramps beside them take their heights), then the wider before the
    // narrower.
    let mut ys: Vec<Vec<f32>> = vec![Vec::new(); pieces.len()];
    let mut fixed = vec![false; nodes.len()];
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by_key(|&i| {
        let r = &roads[road[i]];
        let class = match r.class {
            RoadClass::Highway => 0,
            RoadClass::Arterial => 1,
            RoadClass::Collector => 2,
            _ => 3,
        };
        (class, u8::MAX - r.lanes, i)
    });
    let mut highway: Vec<([f32; 3], [f32; 3])> = Vec::new();
    for &i in order.iter().filter(|&&i| kind[i] == Kind::Highway) {
        let r = &roads[road[i]];
        let of_line = |q: &Piece| q.line == i;
        let parts: Vec<Part> = pieces
            .iter()
            .filter(|q| of_line(q))
            .map(|pc| Part {
                a: pc.a,
                b: pc.b,
                s: arcs(&pc.pts),
                target: pc.pts.iter().map(|&p| target(p, r.deck_m)).collect(),
            })
            .collect();
        let g = r.max_grade.unwrap_or(HIGHWAY_GRADE);
        let got = grade_line(&parts, g, ROAD_CURVE, &mut node_y, &mut fixed);
        for ((pc, ys), y) in pieces
            .iter()
            .zip(&mut ys)
            .filter(|(q, _)| of_line(q))
            .zip(got)
        {
            for k in 0..pc.pts.len() - 1 {
                let (p, q) = (pc.pts[k], pc.pts[k + 1]);
                highway.push(([p.x, y[k], p.y], [q.x, y[k + 1], q.y]));
            }
            *ys = y;
        }
    }
    // The highway's height beside `p`.
    let highway_y = |p: V2| {
        let mut best = (f32::INFINITY, 0.0);
        for (a, b) in &highway {
            let (d, t) = segment_distance(p, V2::new(a[0], a[2]), V2::new(b[0], b[2]));
            if d < best.0 {
                best = (d, lerp(a[1], b[1], t));
            }
        }
        best.1
    };
    // Overpasses per line: (arc, deck height, half the level span).
    let mut decks: Vec<Vec<(f32, f32, f32)>> = vec![Vec::new(); lines.len()];
    for &(i, s, j, p) in &over {
        let flat = highway_half(roads[road[j]].lanes) + OVERPASS_FLAT_M;
        decks[i].push((s, highway_y(p) + OVERPASS_LIFT_M, flat));
    }
    // The height an overpass asks of its road at arc `s`: the deck, falling away either side.
    let lift = |line: usize, s: f32| {
        decks[line]
            .iter()
            .map(|&(c, top, flat)| top - APPROACH_GRADE * (abs(s - c) - flat).max(0.0))
            .fold(f32::NEG_INFINITY, f32::max)
    };
    for (i, c) in cuts.iter().enumerate() {
        for &(s, nd) in c {
            if !on_highway[nd] {
                node_y[nd] = node_y[nd].max(lift(i, s));
            }
        }
    }
    for &i in order
        .iter()
        .filter(|&&i| matches!(kind[i], Kind::Road | Kind::Runway))
    {
        let r = &roads[road[i]];
        let of_line = |q: &Piece| q.line == i;
        let parts: Vec<Part> = pieces
            .iter()
            .filter(|q| of_line(q))
            .map(|pc| {
                let s = arcs(&pc.pts);
                let target = (0..pc.pts.len())
                    .map(|k| target(pc.pts[k], r.deck_m).max(lift(i, pc.s0 + s[k])))
                    .collect();
                Part {
                    a: pc.a,
                    b: pc.b,
                    s,
                    target,
                }
            })
            .collect();
        let g = r.max_grade.unwrap_or(MAX_GRADE);
        // A steep road is a slow one: it may turn over a crest sooner.
        let k = if g > MAX_GRADE {
            STREET_CURVE
        } else {
            ROAD_CURVE
        };
        let got = grade_line(&parts, g, k, &mut node_y, &mut fixed);
        for ((_, ys), y) in pieces
            .iter()
            .zip(&mut ys)
            .filter(|(q, _)| of_line(q))
            .zip(got)
        {
            *ys = y;
        }
    }

    let mut bridged: Vec<Vec<bool>> = Vec::new();
    let edges: Vec<Edge> = pieces
        .iter()
        .zip(ys)
        .map(|(pc, hy)| {
            let r = &roads[road[pc.line]];
            let s = arcs(&pc.pts);
            let total = s[s.len() - 1];
            // A deck spans the water, and the highway's bed under an overpass.
            let spans = |k: usize| {
                let at = pc.s0 + s[k];
                decks[pc.line]
                    .iter()
                    .any(|&(c, _, flat)| abs(at - c) < flat - 1.0)
            };
            bridged.push(
                (0..pc.pts.len())
                    .map(|k| over_water(pc.pts[k]) || spans(k))
                    .collect(),
            );
            let ys = match kind[pc.line] {
                Kind::Highway | Kind::Road | Kind::Runway => hy,
                // Level with the highway to past the gore, then up (or down) to the road.
                Kind::Ramp => {
                    let merge = RAMP_MERGE_M.min(0.4 * total);
                    let from_highway = on_highway[pc.a];
                    let foot = if from_highway {
                        node_y[pc.b]
                    } else {
                        node_y[pc.a]
                    };
                    (0..pc.pts.len())
                        .map(|k| {
                            let d = if from_highway { s[k] } else { total - s[k] };
                            lerp(highway_y(pc.pts[k]), foot, smoothstep(merge, total, d))
                        })
                        .collect()
                }
            };
            let (width_m, speed) = match (r.class, r.lanes) {
                _ if kind[pc.line] == Kind::Runway => {
                    (layout.airfield.as_ref().map_or(30.0, |a| a.runway_m), 40)
                }
                (RoadClass::Highway, l) => (2.0 * highway_half(l), 100),
                (RoadClass::Rural, _) => (8.0, 70),
                (_, 1) => (12.0, 50),
                (_, 2) => (14.0, 60),
                (_, l) => (f32::from(l) * 6.8 + 1.8, 60),
            };
            let profile = pc
                .pts
                .iter()
                .zip(&ys)
                .map(|(p, &y)| [p.x, y, p.y])
                .collect();
            if kind[pc.line] == Kind::Ramp {
                return Edge {
                    a: pc.a as u32,
                    b: pc.b as u32,
                    class: RoadClass::Collector,
                    lanes_fwd: 1,
                    lanes_back: 0,
                    width_m: RAMP_WIDTH_M,
                    sidewalk: false,
                    speed_kph: 60,
                    oneway: true,
                    profile,
                };
            }
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
                profile,
            }
        })
        .collect();
    let rank: Vec<Kind> = pieces.iter().map(|pc| kind[pc.line]).collect();
    carve(m, &edges, &bridged, &rank);
    right_of_way(m, &edges, &rank);
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

/// A stretch of a road between two of its junctions (nodes `a` and `b`): the arc length and
/// the height asked at each of its points.
struct Part {
    a: usize,
    b: usize,
    s: Vec<f32>,
    target: Vec<f32>,
}

/// A grade is read across this far either side of a junction.
const JUNCTION_REACH_M: f32 = 30.0;

/// The heights of one road along its `parts`, in order: near their targets, no steeper than
/// `g`, the grade changing by no more than `k` per meter ([`ease`]) and unbroken through the
/// road's own junctions. A junction a road before this one `fixed` keeps its height
/// (`node_y`); the rest take theirs from this road and are fixed for the next.
fn grade_line(
    parts: &[Part],
    g: f32,
    k: f32,
    node_y: &mut [f32],
    fixed: &mut [bool],
) -> Vec<Vec<f32>> {
    let mut out = Vec::with_capacity(parts.len());
    let mut from = 0;
    while from < parts.len() {
        // A run of parts joined end to end.
        let mut to = from + 1;
        while to < parts.len() && parts[to].a == parts[to - 1].b {
            to += 1;
        }
        let run = &parts[from..to];
        let (mut s, mut y, mut held) = (Vec::new(), Vec::new(), Vec::new());
        let mut first = Vec::with_capacity(run.len());
        for (j, p) in run.iter().enumerate() {
            let skip = usize::from(j > 0);
            let s0 = s.last().copied().unwrap_or(0.0);
            first.push(s.len() - skip);
            let n = p.s.len();
            for q in skip..n {
                s.push(s0 + p.s[q]);
                let node = if q == 0 {
                    Some(p.a)
                } else if q == n - 1 {
                    Some(p.b)
                } else {
                    None
                };
                y.push(node.map_or(p.target[q], |nd| node_y[nd]));
                held.push(node.is_some_and(|nd| fixed[nd]));
            }
        }
        let n = y.len();
        let mut stops: Vec<usize> = (1..n - 1).filter(|&i| held[i]).collect();
        stops.insert(0, 0);
        stops.push(n - 1);
        let mut limited = y.clone();
        for w in stops.windows(2) {
            let (i, j) = (w[0], w[1]);
            let v = grade_limit(&y[i..=j], &s[i..=j], g, None, None, None);
            limited[i..=j].copy_from_slice(&v);
        }
        let at_stop = |i: usize| {
            (i > 0 && i < n - 1).then(|| clamp(grade_at(&limited, &s, i, JUNCTION_REACH_M), -g, g))
        };
        let mut eased = limited.clone();
        for w in stops.windows(2) {
            let (i, j) = (w[0], w[1]);
            let v = ease(&limited[i..=j], &s[i..=j], k, at_stop(i), at_stop(j));
            eased[i..=j].copy_from_slice(&v);
        }
        for (j, p) in run.iter().enumerate() {
            let at = first[j];
            let part = eased[at..at + p.s.len()].to_vec();
            for (nd, h) in [(p.a, part[0]), (p.b, part[part.len() - 1])] {
                node_y[nd] = h;
                fixed[nd] = true;
            }
            out.push(part);
        }
        from = to;
    }
    out
}

/// Cuts and fills the ground to meet the roads: under a road and its shoulders the ground
/// is the road's profile (a hair under it); beyond, it blends back to the ground over
/// [`CARVE_BLEND_M`]. Where two roads' reaches overlap the one with the firmer hold wins,
/// a highway before any other. Bridge spans (`bridged`, per profile point) leave the
/// ground under them alone.
fn carve(m: &mut MacroMap, edges: &[Edge], bridged: &[Vec<bool>], kind: &[Kind]) {
    let (w, h) = (m.width as usize, m.height as usize);
    let mut hold = vec![0.0f32; w * h];
    let mut at = vec![0.0f32; w * h];
    let highways = (0..edges.len()).filter(|&i| kind[i] == Kind::Highway);
    let rest = (0..edges.len()).filter(|&i| kind[i] != Kind::Highway);
    for i in highways.chain(rest) {
        let (e, br) = (&edges[i], &bridged[i]);
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

/// Zones the land under the highways and their ramps `Highway`, and the land between a
/// ramp and its highway with it, so no street or lot is laid there.
fn right_of_way(m: &mut MacroMap, edges: &[Edge], kind: &[Kind]) {
    let highway: Vec<(V2, V2)> = edges
        .iter()
        .zip(kind)
        .filter(|(_, &k)| k == Kind::Highway)
        .flat_map(|(e, _)| e.profile.windows(2))
        .map(|s| (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2])))
        .collect();
    for (e, &k) in edges.iter().zip(kind) {
        if k == Kind::Road {
            continue;
        }
        let reach = 0.5 * e.width_m + RIGHT_OF_WAY_M;
        for s in e.profile.windows(2) {
            let (a, b) = (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2]));
            paint(m, a, b, reach, ZoneId::Highway);
            if k == Kind::Ramp {
                // Across to the highway beside it.
                let mut best = (f32::INFINITY, a);
                for &(p, q) in &highway {
                    let (d, t) = segment_distance(a, p, q);
                    if d < best.0 {
                        best = (d, p.lerp(q, t));
                    }
                }
                paint(m, a, best.1, RIGHT_OF_WAY_M, ZoneId::Highway);
            }
        }
    }
}

/// Zones every land cell whose center lies within `reach` of segment `a`-`b` as `zone`.
fn paint(m: &mut MacroMap, a: V2, b: V2, reach: f32, zone: ZoneId) {
    let (zw, zh) = (m.zone_width as usize, m.zone_height as usize);
    let cell = ZONE_CELL_M;
    let x0 = ((a.x.min(b.x) - reach) / cell).max(0.0) as usize;
    let z0 = ((a.y.min(b.y) - reach) / cell).max(0.0) as usize;
    let x1 = (((a.x.max(b.x) + reach) / cell) as usize).min(zw - 1);
    let z1 = (((a.y.max(b.y) + reach) / cell) as usize).min(zh - 1);
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let (x, z) = cell_center(cx, cz);
            let i = cz * zw + cx;
            if m.zones[i] != ZoneId::Water && segment_distance(V2::new(x, z), a, b).0 <= reach {
                m.zones[i] = zone;
            }
        }
    }
}

/// Cuts the layout's channels into the ground (see the module docs) and adds each as a
/// body of "water": dry concrete, or a river at the water level.
pub fn channels(layout: &Layout, m: &mut MacroMap) {
    let (w, h) = (m.width as usize, m.height as usize);
    let ext = layout.extent();
    for c in &layout.channels {
        let line = pts(&c.line);
        let total = layout::line_length(&line);
        let (bed, lip) = (0.5 * c.width_m, c.lip_half_m());
        // An end inside the place is a headwall the channel deepens from.
        let inside = |p: V2| p.x > 1.0 && p.y > 1.0 && p.x < ext[0] - 1.0 && p.y < ext[1] - 1.0;
        let heads = [inside(line[0]), inside(line[line.len() - 1])];
        let bb = bbox(&line, lip);
        for iz in 0..h {
            for ix in 0..w {
                let p = V2::new(ix as f32 * CELL_M, iz as f32 * CELL_M);
                if !in_box(p, &bb) {
                    continue;
                }
                // The nearest point of the centerline and the arc length there.
                let (mut d, mut s, mut run) = (f32::INFINITY, 0.0, 0.0);
                for seg in line.windows(2) {
                    let len = (seg[1] - seg[0]).length();
                    let (ds, t) = segment_distance(p, seg[0], seg[1]);
                    if ds < d {
                        (d, s) = (ds, run + t * len);
                    }
                    run += len;
                }
                let mut k = clamp((lip - d) / (lip - bed), 0.0, 1.0);
                if heads[0] {
                    k *= smoothstep(0.0, CHANNEL_HEAD_M, s);
                }
                if heads[1] {
                    k *= smoothstep(0.0, CHANNEL_HEAD_M, total - s);
                }
                let i = iz * w + ix;
                m.heights[i] = if c.wet {
                    lerp(m.heights[i], m.heights[i].min(m.water_level - c.depth_m), k)
                } else {
                    m.heights[i] - c.depth_m * k
                };
            }
        }
        let zone = if c.wet {
            ZoneId::Water
        } else {
            ZoneId::Highway
        };
        // A zone cell past the lips too, so no lot backs onto the wall.
        for seg in line.windows(2) {
            paint(m, seg[0], seg[1], lip + ZONE_CELL_M, zone);
        }
        // The outline at the lips: down one side and back up the other.
        let n = line.len();
        let side = |sign: f32| -> Vec<[f32; 2]> {
            (0..n)
                .map(|i| {
                    let dir = (line[(i + 1).min(n - 1)] - line[i.saturating_sub(1)]).normalize();
                    let p = line[i] + dir.right() * (sign * lip);
                    [p.x, p.y]
                })
                .collect()
        };
        let mut polygon = side(1.0);
        polygon.extend(side(-1.0).into_iter().rev());
        m.water.push(WaterBody {
            kind: if c.wet {
                WaterKind::Canal
            } else {
                WaterKind::Channel
            },
            polygon: ccw(polygon),
        });
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
