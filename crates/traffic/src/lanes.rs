//! The lane graph: lanes, junction turn tables, stop lines and signals from a place's roads.
//!
//! **Lanes.** Edges are first joined into chains through two-road nodes where the road
//! carries on unchanged (same lanes each way, class, width, speed, one-way, bending less
//! than 60 degrees): most such nodes are where the producer split a road at a chunk
//! border, and a lane should not end there. Every chain gets its forward lanes (the first
//! edge's direction of travel along the chain) then its back lanes, slot 0 first. A lane's
//! centerline is the chain's profile offset to the right of its driving direction, at the
//! lane centers the road paint uses (`world::roads`): two-way roads split each half of the
//! curb-to-curb width (less the gutter) into equal lanes from the centerline out; highways
//! put 3.6 m lanes out from a 1.2 m half median; one-way roads (ramps) spread their lanes
//! across the whole width, slot 0 on the left. Each end is cut back from its node by the
//! junction's setback, so lanes stop short of the junction box.
//!
//! **Junctions.** A node's setback per road keeps that road's pavement clear of every other
//! road's at the node; a turn ([`Turn`]) joins the end of an incoming lane to the start of
//! an outgoing one with a cubic curve tangent to both, so a car's path is continuous with
//! no kink from lane to lane. A road squeezed between two close junctions keeps a fifth of
//! its length as lane, and its turns come out tight; every turn carries its tightest
//! radius for turn speed. Through movements keep their slot (clamped to the outgoing
//! road's lanes), left turns leave from and arrive in slot 0 and right turns use the
//! outermost slots; nobody turns back onto the road they came from, and at a node touching
//! a highway nobody turns more than 60 degrees (no crossing the median). A lane that no
//! rule serves takes the straightest move available, so only lanes into dead ends have no
//! turn.
//!
//! **Control.** Signals where arterials cross (three or more arterial ends, or two on a
//! four-way at an even node), stop signs on every other approach that is not an arterial,
//! arterials through stop-controlled junctions have priority; the same rule `world::props`
//! places signal and stop-sign props by. Nodes with fewer than three roads or touching a
//! highway are uncontrolled. A controlled lane ends at its stop line. Signal approaches
//! split into two phases by axis: roughly parallel to the node's first road or across it.

use alloc::vec::Vec;

use place::{Place, RoadClass, StopLine};
use skywave_core::hash64;

use crate::math::{atan2, dir, dist2, dot, lerp3, right, turn_angle, unit2};
use crate::signal;
use crate::signal::Light;

/// "No index" in `u32` index fields.
pub const NONE: u32 = u32::MAX;

/// Concrete gutter along each curb, inside the curb-to-curb width (as `world::roads`).
const GUTTER_M: f32 = 0.45;
/// Highway half median and lane width (as `world::roads`).
const MEDIAN_HALF_M: f32 = 1.2;
const HIGHWAY_LANE_M: f32 = 3.6;
/// Profile samples closer than this to the previous kept one are dropped.
const MIN_SAMPLE_M: f32 = 1.0;
/// Extra setback past where the pavements clear: crosswalk and stop bar room at a
/// junction, curve room at a bend.
const JUNCTION_MARGIN_M: f32 = 4.4;
const BEND_MARGIN_M: f32 = 1.0;
/// Setbacks never exceed this, and an edge's two setbacks together never take more than
/// `MAX_TRIM_SHARE` of its length.
const MAX_TRIM_M: f32 = 20.0;
const MAX_TRIM_SHARE: f32 = 0.8;
/// Moves within this of straight ahead are [`TurnKind::Straight`].
const STRAIGHT_RAD: f32 = 35.0 * core::f32::consts::PI / 180.0;
/// Moves sharper than this are U-turns, never offered at a junction of three or more.
const UTURN_RAD: f32 = 150.0 * core::f32::consts::PI / 180.0;
/// Sharpest move at a node that touches a highway.
const HIGHWAY_TURN_RAD: f32 = 60.0 * core::f32::consts::PI / 180.0;
/// Samples used to measure a turn curve and map arc length onto it.
pub const CURVE_STEPS: usize = 16;

/// What a lane meets at its end.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// Continues without stopping: a bend, a highway node, or an arterial with priority.
    Free = 0,
    /// A stop sign at the lane's end.
    Stop = 1,
    /// A traffic signal at the lane's end.
    Signal = 2,
    /// A dead end: no turns leave the lane.
    DeadEnd = 3,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnKind {
    Straight = 0,
    Left = 1,
    Right = 2,
    /// Sharper than a U-turn's threshold; only through a two-road bend or as the last
    /// resort of a lane with no other move.
    UTurn = 3,
}

/// One lane of a road in its driving direction. A lane runs from junction to junction (or
/// dead end), so it may span several edges joined at two-road nodes; `edge` and `forward`
/// name the edge it ends on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lane {
    pub edge: u32,
    /// Driving from the edge's `a` to `b`.
    pub forward: bool,
    /// 0 is the lane next to the centerline (or the left edge of a one-way road).
    pub slot: u8,
    /// Signal phase (0 or 1) when `control` is [`Control::Signal`], else 0.
    pub phase: u8,
    pub control: Control,
    /// The nodes the lane drives from and toward.
    pub start_node: u32,
    pub end_node: u32,
    /// Centerline: `points[first_point..first_point + point_count]` of the graph, at
    /// arc lengths `arc[..]` from 0 to `length_m`.
    pub first_point: u32,
    pub point_count: u32,
    pub length_m: f32,
    /// Posted speed, m/s.
    pub speed_mps: f32,
    /// Index into [`LaneGraph::stop_lines`] when the lane ends at one, else [`NONE`].
    pub stop_line: u32,
    /// Turns leaving the lane: `turns[first_turn..first_turn + turn_count]`.
    pub first_turn: u32,
    pub turn_count: u32,
}

/// A movement through junction `node` from the end of lane `from` to the start of lane
/// `to`, along a cubic curve with control points `curve`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    pub node: u32,
    pub from: u32,
    pub to: u32,
    pub kind: TurnKind,
    pub curve: [[f32; 3]; 4],
    /// Arc length of the curve, measured over [`CURVE_STEPS`] chords.
    pub length_m: f32,
    /// Tightest radius along the curve (chord length over the heading change between
    /// chords, starting from the lane's direction), for turn speed; infinite when straight.
    pub min_radius_m: f32,
}

/// Per node: how traffic is controlled there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Junction {
    /// Road ends meeting at the node.
    pub degree: u16,
    /// Whether the node has signals.
    pub signal: bool,
    /// Where its signal cycle starts, ticks ([`signal::offset`]); 0 without signals.
    pub offset: u32,
}

/// A point on a lane or turn: position and heading in the vehicle sim's convention
/// (radians, 0 faces -z, positive turns left), plus the unit ground direction `(x, z)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: [f32; 3],
    pub heading: f32,
    pub dir: [f32; 2],
}

/// The lane graph of a place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneGraph {
    pub lanes: Vec<Lane>,
    /// Every lane's centerline points, lane after lane.
    pub points: Vec<[f32; 3]>,
    /// Arc length of each of `points` from its lane's start.
    pub arc: Vec<f32>,
    /// Grouped by `from` lane, lanes in order.
    pub turns: Vec<Turn>,
    /// One per controlled approach (edge, node), across its lanes' ends.
    pub stop_lines: Vec<StopLine>,
    /// One per road-graph node.
    pub junctions: Vec<Junction>,
}

/// A road end at a node: the edge, whether it is the edge's `b` end, and the ground
/// direction from the node along the road.
#[derive(Clone, Copy)]
struct End {
    edge: u32,
    at_b: bool,
    dir: [f32; 2],
}

/// Edge `e`'s profile polyline from `a` to `b`: node positions at the ends, interior
/// samples at least [`MIN_SAMPLE_M`] apart (as `world::roads` lays it).
fn raw_line(place: &Place, e: &place::Edge) -> Vec<[f32; 3]> {
    let nodes = &place.roads.nodes;
    let (na, nb) = (&nodes[e.a as usize], &nodes[e.b as usize]);
    let prof = &e.profile;
    let ya = prof.first().map_or(na.elevation, |p| p[1]);
    let yb = prof.last().map_or(nb.elevation, |p| p[1]);
    let b = [nb.pos[0], yb, nb.pos[1]];
    let mut pts = alloc::vec![[na.pos[0], ya, na.pos[1]]];
    if prof.len() > 2 {
        for &q in &prof[1..prof.len() - 1] {
            if dist2(pts[pts.len() - 1], q) >= MIN_SAMPLE_M && dist2(q, b) >= MIN_SAMPLE_M {
                pts.push(q);
            }
        }
    }
    pts.push(b);
    pts
}

/// Cumulative ground arc length of a polyline.
fn arcs(pts: &[[f32; 3]]) -> Vec<f32> {
    let mut s = alloc::vec![0.0f32; pts.len()];
    for k in 1..pts.len() {
        s[k] = s[k - 1] + dist2(pts[k - 1], pts[k]);
    }
    s
}

/// The part of `pts` (arc lengths `s`) between arc lengths `s0` and `s1`.
fn cut(pts: &[[f32; 3]], s: &[f32], s0: f32, s1: f32) -> Vec<[f32; 3]> {
    let at = |x: f32| {
        let k = s[1..].partition_point(|&v| v < x).min(pts.len() - 2);
        let span = s[k + 1] - s[k];
        let t = if span > 0.0 { (x - s[k]) / span } else { 0.0 };
        lerp3(pts[k], pts[k + 1], t.clamp(0.0, 1.0))
    };
    let mut out = alloc::vec![at(s0)];
    for k in 1..pts.len() - 1 {
        if s[k] > s0 + 0.01 && s[k] < s1 - 0.01 {
            out.push(pts[k]);
        }
    }
    out.push(at(s1));
    out
}

/// `pts` offset `off` meters to the right of its direction, mitered at the bends.
fn offset(pts: &[[f32; 3]], off: f32) -> Vec<[f32; 3]> {
    let n = pts.len();
    let seg: Vec<[f32; 2]> = (0..n - 1).map(|k| right(dir(pts[k], pts[k + 1]))).collect();
    (0..n)
        .map(|k| {
            let r = if k == 0 {
                seg[0]
            } else if k == n - 1 {
                seg[n - 2]
            } else {
                let (u, w) = (seg[k - 1], seg[k]);
                let m = unit2([u[0] + w[0], u[1] + w[1]]);
                let c = dot(m, w).max(0.5);
                [m[0] / c, m[1] / c]
            };
            let p = pts[k];
            [p[0] + r[0] * off, p[1], p[2] + r[1] * off]
        })
        .collect()
}

/// How far a road end must be cut back so its pavement clears the other ends' at the node.
fn setback(ends: &[End], i: usize, hw: &[f32]) -> f32 {
    let me = ends[i];
    let mut t = 0.0f32;
    for (j, o) in ends.iter().enumerate() {
        if j == i {
            continue;
        }
        // Where this road's curb meets the other road's curb along this road.
        let c = dot(me.dir, o.dir);
        let s = (1.0 - c * c).max(0.0);
        if c < -0.999 {
            continue; // Straight through: nothing to clear.
        }
        let sin = crate::math::sqrt(s).max(0.5);
        let (hi, hj) = (hw[me.edge as usize], hw[o.edge as usize]);
        t = t.max((hj + hi * c.abs()) / sin);
    }
    t.min(MAX_TRIM_M)
}

/// Lateral offset (to the right of the driving direction) of lane `slot` of `n`.
fn lane_offset(e: &place::Edge, slot: u8, n: u8) -> f32 {
    let inner = e.width_m * 0.5 - GUTTER_M;
    let k = f32::from(slot) + 0.5;
    if e.oneway {
        -inner + k * 2.0 * inner / f32::from(n)
    } else if e.class == RoadClass::Highway {
        MEDIAN_HALF_M + HIGHWAY_LANE_M * k
    } else {
        k * inner / f32::from(n)
    }
}

/// Point on cubic `c` at parameter `t`.
fn bezier(c: &[[f32; 3]; 4], t: f32) -> [f32; 3] {
    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    let mut p = [0.0; 3];
    for (a, v) in p.iter_mut().enumerate() {
        *v = c[0][a] * b0 + c[1][a] * b1 + c[2][a] * b2 + c[3][a] * b3;
    }
    p
}

/// Cubic `c` with its length over [`CURVE_STEPS`] chords and its tightest radius (chord
/// length over the heading change from the previous chord, starting from `d0`).
fn measure(c: [[f32; 3]; 4], d0: [f32; 2]) -> ([[f32; 3]; 4], f32, f32) {
    let mut length_m = 0.0;
    let mut min_radius_m = f32::INFINITY;
    let mut prev = c[0];
    let mut prev_dir = d0;
    for k in 1..=CURVE_STEPS {
        let p = bezier(&c, k as f32 / CURVE_STEPS as f32);
        let step = dist2(prev, p);
        length_m += step;
        if step > 0.0 {
            let d = dir(prev, p);
            let bend = turn_angle(prev_dir, d).abs();
            if bend > 0.0 {
                min_radius_m = min_radius_m.min(step / bend);
            }
            prev_dir = d;
        }
        prev = p;
    }
    (c, length_m, min_radius_m)
}

fn heading(d: [f32; 2]) -> f32 {
    atan2(-d[0], -d[1])
}

impl LaneGraph {
    /// Builds the lane graph of `place` (see the module docs).
    pub fn new(place: &Place) -> LaneGraph {
        let nodes = &place.roads.nodes;
        let edges = &place.roads.edges;
        let raws: Vec<Vec<[f32; 3]>> = edges.iter().map(|e| raw_line(place, e)).collect();
        let hw: Vec<f32> = edges.iter().map(|e| e.width_m * 0.5).collect();

        // Road ends per node, in edge order.
        let mut incident: Vec<Vec<End>> = (0..nodes.len()).map(|_| Vec::new()).collect();
        for (i, e) in edges.iter().enumerate() {
            let r = &raws[i];
            let n = r.len();
            incident[e.a as usize].push(End {
                edge: i as u32,
                at_b: false,
                dir: dir(r[0], r[1]),
            });
            incident[e.b as usize].push(End {
                edge: i as u32,
                at_b: true,
                dir: dir(r[n - 1], r[n - 2]),
            });
        }

        // Two-road nodes where the road carries on unchanged are not junctions: lanes run
        // straight through them (most are where the producer split a road at a chunk
        // border).
        let carries = |x: &End, y: &End| {
            let (e, f) = (&edges[x.edge as usize], &edges[y.edge as usize]);
            // Lanes arriving through `x` and leaving through `y`, and the other way.
            let (xin, xout) = if x.at_b {
                (e.lanes_fwd, e.lanes_back)
            } else {
                (e.lanes_back, e.lanes_fwd)
            };
            let (yout, yin) = if y.at_b {
                (f.lanes_back, f.lanes_fwd)
            } else {
                (f.lanes_fwd, f.lanes_back)
            };
            x.edge != y.edge
                && (xin, xout) == (yout, yin)
                && (e.class, e.oneway, e.speed_kph) == (f.class, f.oneway, f.speed_kph)
                && e.width_m == f.width_m
                && dot(x.dir, y.dir) <= -0.5
        };
        let mut joined: Vec<bool> = incident
            .iter()
            .map(|ends| ends.len() == 2 && carries(&ends[0], &ends[1]))
            .collect();

        // Chains of edges between junctions, each a list of (edge, traversed b to a). From
        // every junction end in edge order, then whatever is left: loops with no junction,
        // broken at their first edge's `a` node.
        let mut chains: Vec<Vec<(u32, bool)>> = Vec::new();
        let mut seen = alloc::vec![false; edges.len()];
        let walk = |start: u32, node: u32, seen: &mut [bool], joined: &[bool]| {
            let mut chain = Vec::new();
            let (mut e, mut n) = (start, node);
            loop {
                seen[e as usize] = true;
                let edge = &edges[e as usize];
                let reversed = edge.a != n;
                chain.push((e, reversed));
                n = if reversed { edge.a } else { edge.b };
                if !joined[n as usize] {
                    break;
                }
                let ends = &incident[n as usize];
                let next = if ends[0].edge == e {
                    ends[1].edge
                } else {
                    ends[0].edge
                };
                if seen[next as usize] {
                    break;
                }
                e = next;
            }
            chain
        };
        for (i, e) in edges.iter().enumerate() {
            for node in [e.a, e.b] {
                if !seen[i] && !joined[node as usize] {
                    chains.push(walk(i as u32, node, &mut seen, &joined));
                }
            }
        }
        for (i, e) in edges.iter().enumerate() {
            if !seen[i] {
                joined[e.a as usize] = false;
                chains.push(walk(i as u32, e.a, &mut seen, &joined));
            }
        }

        // Junction control and setbacks.
        let mut junctions = Vec::with_capacity(nodes.len());
        let mut trims = alloc::vec![[0.0f32; 2]; edges.len()];
        for (node, ends) in incident.iter().enumerate() {
            let deg = ends.len();
            let class = |x: &End| edges[x.edge as usize].class;
            let highway = ends.iter().any(|x| class(x) == RoadClass::Highway);
            let arterials = ends
                .iter()
                .filter(|x| class(x) == RoadClass::Arterial)
                .count();
            let signal = deg >= 3
                && !highway
                && (arterials >= 3 || (arterials == 2 && deg == 4 && node % 2 == 0));
            junctions.push(Junction {
                degree: deg as u16,
                signal,
                offset: if signal {
                    signal::offset(place.header.seed, node as u32)
                } else {
                    0
                },
            });
            if deg < 2 || joined[node] {
                continue;
            }
            let margin = if deg >= 3 {
                JUNCTION_MARGIN_M
            } else {
                BEND_MARGIN_M
            };
            for i in 0..deg {
                let t = setback(ends, i, &hw) + margin;
                trims[ends[i].edge as usize][usize::from(ends[i].at_b)] = t;
            }
        }

        // Lanes, chain by chain: forward along the chain, then back.
        let mut g = LaneGraph {
            junctions,
            ..LaneGraph::default()
        };
        // Per edge end (`a`, `b`): (first lane, count) arriving there and leaving there.
        let mut end_lanes = alloc::vec![[[(0u32, 0u32); 2]; 2]; edges.len()];
        for chain in &chains {
            let (e0, r0) = chain[0];
            let (e1, r1) = chain[chain.len() - 1];
            let first = &edges[e0 as usize];
            let mut line: Vec<[f32; 3]> = Vec::new();
            for &(e, reversed) in chain {
                let raw = &raws[e as usize];
                let pts: Vec<[f32; 3]> = if reversed {
                    raw.iter().rev().copied().collect()
                } else {
                    raw.clone()
                };
                let skip = usize::from(!line.is_empty());
                line.extend_from_slice(&pts[skip..]);
            }
            let s = arcs(&line);
            let len = s[s.len() - 1];
            let mut ta = trims[e0 as usize][usize::from(r0)];
            let mut tb = trims[e1 as usize][usize::from(!r1)];
            if ta + tb > MAX_TRIM_SHARE * len {
                let k = MAX_TRIM_SHARE * len / (ta + tb);
                ta *= k;
                tb *= k;
            }
            let core = cut(&line, &s, ta, len - tb);
            let back: Vec<[f32; 3]> = core.iter().rev().copied().collect();
            let (n_fwd, n_back) = if r0 {
                (first.lanes_back, first.lanes_fwd)
            } else {
                (first.lanes_fwd, first.lanes_back)
            };
            let (start_node, end_node) = (if r0 { first.b } else { first.a }, {
                let last = &edges[e1 as usize];
                if r1 {
                    last.a
                } else {
                    last.b
                }
            });
            for (along, n) in [(true, n_fwd), (false, n_back)] {
                let id = g.lanes.len() as u32;
                // The edge at the lane's end, and whether the lane drives it `a` to `b`.
                let (edge, forward) = if along { (e1, !r1) } else { (e0, r0) };
                let line = if along { &core } else { &back };
                if along {
                    end_lanes[e1 as usize][usize::from(!r1)][0] = (id, u32::from(n));
                    end_lanes[e0 as usize][usize::from(r0)][1] = (id, u32::from(n));
                } else {
                    end_lanes[e0 as usize][usize::from(r0)][0] = (id, u32::from(n));
                    end_lanes[e1 as usize][usize::from(!r1)][1] = (id, u32::from(n));
                }
                for slot in 0..n {
                    let pts = offset(line, lane_offset(first, slot, n));
                    let a = arcs(&pts);
                    g.lanes.push(Lane {
                        edge,
                        forward,
                        slot,
                        phase: 0,
                        control: Control::Free,
                        start_node: if along { start_node } else { end_node },
                        end_node: if along { end_node } else { start_node },
                        first_point: g.points.len() as u32,
                        point_count: pts.len() as u32,
                        length_m: a[a.len() - 1],
                        speed_mps: f32::from(first.speed_kph) / 3.6,
                        stop_line: NONE,
                        first_turn: 0,
                        turn_count: 0,
                    });
                    g.points.extend_from_slice(&pts);
                    g.arc.extend_from_slice(&a);
                }
            }
        }
        // Lanes of a road end: those arriving at the node and those leaving it.
        let arriving = |x: &End| end_lanes[x.edge as usize][usize::from(x.at_b)][0];
        let leaving = |x: &End| end_lanes[x.edge as usize][usize::from(x.at_b)][1];

        // Turns, stop lines and lane control, node by node. Each lane ends at one node, so
        // its turns come out contiguous.
        for (node, ends) in incident.iter().enumerate() {
            if joined[node] {
                continue;
            }
            let deg = ends.len();
            let j = g.junctions[node];
            let highway = ends
                .iter()
                .any(|x| edges[x.edge as usize].class == RoadClass::Highway);
            for (i, inc) in ends.iter().enumerate() {
                let (first, n_in) = arriving(inc);
                if n_in == 0 {
                    continue;
                }
                let class = edges[inc.edge as usize].class;
                let control = if deg < 2 {
                    Control::DeadEnd
                } else if j.signal {
                    Control::Signal
                } else if deg >= 3 && !highway && class != RoadClass::Arterial {
                    Control::Stop
                } else {
                    Control::Free
                };
                let phase =
                    u8::from(dot(inc.dir, ends[0].dir).abs() < core::f32::consts::FRAC_1_SQRT_2);
                if matches!(control, Control::Stop | Control::Signal) {
                    let (a, b) = (g.end_pose(first), g.end_pose(first + n_in - 1));
                    let r = right(a.dir);
                    let w = 1.6;
                    g.stop_lines.push(StopLine {
                        edge: inc.edge,
                        node: node as u32,
                        a: [a.pos[0] - r[0] * w, a.pos[1], a.pos[2] - r[1] * w],
                        b: [b.pos[0] + r[0] * w, b.pos[1], b.pos[2] + r[1] * w],
                    });
                }
                for k in 0..n_in {
                    let id = first + k;
                    let start = g.turns.len() as u32;
                    let from = g.end_pose(id);
                    // (out end, target slot, kind, |angle|) of the straightest move, kept
                    // for a lane no rule serves.
                    let mut best: Option<(usize, u32, TurnKind, f32)> = None;
                    for (o, out) in ends.iter().enumerate() {
                        let (ofirst, n_out) = leaving(out);
                        if o == i || n_out == 0 {
                            continue;
                        }
                        // By the lanes' own directions: a road may curve between its
                        // node and its setback.
                        let a = turn_angle(from.dir, g.pose(ofirst, 0.0).dir);
                        let kind = if a.abs() > UTURN_RAD {
                            TurnKind::UTurn
                        } else if a.abs() <= STRAIGHT_RAD {
                            TurnKind::Straight
                        } else if a > 0.0 {
                            TurnKind::Left
                        } else {
                            TurnKind::Right
                        };
                        let target = match kind {
                            TurnKind::Straight | TurnKind::UTurn => k.min(n_out - 1),
                            TurnKind::Left => 0,
                            TurnKind::Right => n_out - 1,
                        };
                        if best.is_none_or(|b| a.abs() < b.3) {
                            best = Some((o, target, kind, a.abs()));
                        }
                        let allowed = (deg == 2 || kind != TurnKind::UTurn)
                            && (!highway || a.abs() <= HIGHWAY_TURN_RAD)
                            && match kind {
                                TurnKind::Straight | TurnKind::UTurn => true,
                                TurnKind::Left => k == 0,
                                TurnKind::Right => k == n_in - 1,
                            };
                        if allowed {
                            g.push_turn(node as u32, id, ofirst + target, kind);
                        }
                    }
                    if g.turns.len() as u32 == start {
                        if let Some((o, target, kind, _)) = best {
                            let (ofirst, _) = leaving(&ends[o]);
                            g.push_turn(node as u32, id, ofirst + target, kind);
                        }
                    }
                    let n = g.turns.len() as u32 - start;
                    let lane = &mut g.lanes[id as usize];
                    lane.first_turn = start;
                    lane.turn_count = n;
                    lane.control = if n == 0 { Control::DeadEnd } else { control };
                    if matches!(lane.control, Control::Stop | Control::Signal) {
                        lane.stop_line = g.stop_lines.len() as u32 - 1;
                    }
                    if lane.control == Control::Signal {
                        lane.phase = phase;
                    }
                }
            }
        }
        g
    }

    fn push_turn(&mut self, node: u32, from: u32, to: u32, kind: TurnKind) {
        let (a, b) = (self.end_pose(from), self.pose(to, 0.0));
        let (da, db) = (a.dir, b.dir);
        let curve = |ha: f32, hb: f32| {
            [
                a.pos,
                [
                    a.pos[0] + da[0] * ha,
                    a.pos[1] + (b.pos[1] - a.pos[1]) / 3.0,
                    a.pos[2] + da[1] * ha,
                ],
                [
                    b.pos[0] - db[0] * hb,
                    a.pos[1] + (b.pos[1] - a.pos[1]) * 2.0 / 3.0,
                    b.pos[2] - db[1] * hb,
                ],
                b.pos,
            ]
        };
        // A circular arc's handles for the angle turned (a third of the chord when
        // straight); and where the two lanes' lines meet ahead of both
        // (`a + u a.dir = b - v b.dir`), the quadratic through that corner. The rounder of
        // the two wins: the arc for even legs, the quadratic for very unequal ones.
        let q = crate::math::sqrt(0.5 * (1.0 + dot(da, db)).max(0.0));
        let h = dist2(a.pos, b.pos) / (1.5 * (1.0 + q));
        let mut best = measure(curve(h, h), da);
        let w = [b.pos[0] - a.pos[0], b.pos[2] - a.pos[2]];
        let den = da[0] * db[1] - da[1] * db[0];
        if den.abs() > 0.05 {
            let u = (w[0] * db[1] - w[1] * db[0]) / den;
            let v = (da[0] * w[1] - da[1] * w[0]) / den;
            if u > 0.0 && v > 0.0 {
                let quad = measure(curve(u * 2.0 / 3.0, v * 2.0 / 3.0), da);
                if quad.2 > best.2 {
                    best = quad;
                }
            }
        }
        let (curve, length_m, min_radius_m) = best;
        self.turns.push(Turn {
            node,
            from,
            to,
            kind,
            curve,
            length_m,
            min_radius_m,
        });
    }

    /// Centerline points of `lane`.
    pub fn points(&self, lane: u32) -> &[[f32; 3]] {
        let l = &self.lanes[lane as usize];
        &self.points[l.first_point as usize..(l.first_point + l.point_count) as usize]
    }

    /// Pose `s` meters along `lane` (clamped to the lane).
    pub fn pose(&self, lane: u32, s: f32) -> Pose {
        let l = &self.lanes[lane as usize];
        let range = l.first_point as usize..(l.first_point + l.point_count) as usize;
        let (pts, arc) = (&self.points[range.clone()], &self.arc[range]);
        let s = s.clamp(0.0, l.length_m);
        let k = arc[1..].partition_point(|&v| v < s).min(pts.len() - 2);
        let span = arc[k + 1] - arc[k];
        let t = if span > 0.0 { (s - arc[k]) / span } else { 0.0 };
        let d = dir(pts[k], pts[k + 1]);
        Pose {
            pos: lerp3(pts[k], pts[k + 1], t),
            heading: heading(d),
            dir: d,
        }
    }

    /// Pose at the end of `lane`.
    pub fn end_pose(&self, lane: u32) -> Pose {
        self.pose(lane, self.lanes[lane as usize].length_m)
    }

    /// Pose `s` meters along `turn` (clamped), by arc length over [`CURVE_STEPS`] chords.
    pub fn turn_pose(&self, turn: u32, s: f32) -> Pose {
        let c = &self.turns[turn as usize].curve;
        let s = s.max(0.0);
        let mut prev = c[0];
        let mut acc = 0.0;
        let mut t = 1.0;
        for k in 1..=CURVE_STEPS {
            let p = bezier(c, k as f32 / CURVE_STEPS as f32);
            let d = dist2(prev, p);
            if acc + d >= s {
                let f = if d > 0.0 { (s - acc) / d } else { 0.0 };
                t = (k as f32 - 1.0 + f) / CURVE_STEPS as f32;
                break;
            }
            acc += d;
            prev = p;
        }
        // Tangent from the derivative of the cubic.
        let u = 1.0 - t;
        let (d0, d1, d2) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
        let tx = (c[1][0] - c[0][0]) * d0 + (c[2][0] - c[1][0]) * d1 + (c[3][0] - c[2][0]) * d2;
        let tz = (c[1][2] - c[0][2]) * d0 + (c[2][2] - c[1][2]) * d1 + (c[3][2] - c[2][2]) * d2;
        let mut d = unit2([tx, tz]);
        if d == [0.0, 0.0] {
            d = dir(c[0], c[3]);
        }
        Pose {
            pos: bezier(c, t),
            heading: heading(d),
            dir: d,
        }
    }

    /// The turns leaving `lane` at its end.
    pub fn next(&self, lane: u32) -> &[Turn] {
        let l = &self.lanes[lane as usize];
        &self.turns[l.first_turn as usize..(l.first_turn + l.turn_count) as usize]
    }

    /// Distance from `s` along `lane` to its stop line, when it ends at one (negative
    /// past it).
    pub fn stop_distance(&self, lane: u32, s: f32) -> Option<f32> {
        let l = &self.lanes[lane as usize];
        (l.stop_line != NONE).then_some(l.length_m - s)
    }

    /// The light `lane` faces at `tick`, when it ends at a signal.
    pub fn light(&self, lane: u32, tick: u64) -> Option<Light> {
        let l = &self.lanes[lane as usize];
        if l.control != Control::Signal {
            return None;
        }
        let j = &self.junctions[l.end_node as usize];
        Some(signal::light(j.offset, l.phase, tick))
    }

    /// Bytes the graph's arrays hold.
    pub fn bytes(&self) -> usize {
        use core::mem::size_of;
        self.lanes.len() * size_of::<Lane>()
            + self.points.len() * size_of::<[f32; 3]>()
            + self.arc.len() * size_of::<f32>()
            + self.turns.len() * size_of::<Turn>()
            + self.stop_lines.len() * size_of::<StopLine>()
            + self.junctions.len() * size_of::<Junction>()
    }

    /// A hash of everything in the graph, bit for bit.
    pub fn hash(&self) -> u64 {
        let mut w: Vec<u64> = Vec::with_capacity(self.points.len() * 2 + self.turns.len() * 8);
        let f = |x: f32| u64::from(x.to_bits());
        for l in &self.lanes {
            w.push(u64::from(l.edge) | u64::from(l.forward) << 32 | u64::from(l.slot) << 40);
            w.push(u64::from(l.phase) | (l.control as u64) << 8 | u64::from(l.end_node) << 16);
            w.push(u64::from(l.first_point) | u64::from(l.point_count) << 32);
            w.push(f(l.length_m) | f(l.speed_mps) << 32);
            w.push(u64::from(l.stop_line) | u64::from(l.first_turn) << 32);
            w.push(u64::from(l.turn_count) | u64::from(l.start_node) << 32);
        }
        for (p, &s) in self.points.iter().zip(&self.arc) {
            w.push(f(p[0]) | f(p[1]) << 32);
            w.push(f(p[2]) | f(s) << 32);
        }
        for t in &self.turns {
            w.push(u64::from(t.node) | (t.kind as u64) << 32);
            w.push(u64::from(t.from) | u64::from(t.to) << 32);
            for p in &t.curve {
                w.push(f(p[0]) | f(p[1]) << 32);
                w.push(f(p[2]));
            }
            w.push(f(t.length_m) | f(t.min_radius_m) << 32);
        }
        for s in &self.stop_lines {
            w.push(u64::from(s.edge) | u64::from(s.node) << 32);
            w.extend(s.a.iter().chain(&s.b).map(|&x| f(x)));
        }
        for j in &self.junctions {
            w.push(u64::from(j.degree) | u64::from(j.signal) << 16 | u64::from(j.offset) << 32);
        }
        hash64(0x1A4E_6AF4, &w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_lanes_drive_on_the_right_and_turn() {
        let place = Place::test_grid();
        let g = LaneGraph::new(&place);
        assert!(!g.lanes.is_empty());
        for (id, l) in g.lanes.iter().enumerate() {
            let e = &place.roads.edges[l.edge as usize];
            // The lane runs on the right of its edge's centerline, seen in its direction.
            let p = g.pose(id as u32, l.length_m * 0.5);
            let (na, nb) = (
                place.roads.nodes[e.a as usize].pos,
                place.roads.nodes[e.b as usize].pos,
            );
            let d = if l.forward {
                unit2([nb[0] - na[0], nb[1] - na[1]])
            } else {
                unit2([na[0] - nb[0], na[1] - nb[1]])
            };
            let rel = [p.pos[0] - na[0], p.pos[2] - na[1]];
            assert!(dot(rel, right(d)) > 0.5, "lane {id} not on the right");
            assert!(dot(p.dir, d) > 0.99);
        }
        assert!(!g.turns.is_empty());
        assert!(g.turns.iter().any(|t| t.kind == TurnKind::Left));
        assert!(g.turns.iter().any(|t| t.kind == TurnKind::Right));
        // A regular grid has room for every turn.
        for t in &g.turns {
            assert!(t.min_radius_m >= 3.0, "{t:?}");
        }
    }

    #[test]
    fn heading_follows_the_vehicle_convention() {
        // 0 faces north (-z), east is -PI/2.
        assert_eq!(heading([0.0, -1.0]), 0.0);
        assert!((heading([1.0, 0.0]) + core::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn queries_clamp_and_report_control() {
        let place = Place::test_grid();
        let g = LaneGraph::new(&place);
        for id in 0..g.lanes.len() as u32 {
            let l = g.lanes[id as usize];
            assert_eq!(g.pose(id, -5.0).pos, g.points(id)[0]);
            assert_eq!(g.pose(id, l.length_m + 5.0).pos, g.end_pose(id).pos);
            assert_eq!(g.stop_distance(id, 0.0).is_some(), l.stop_line != NONE);
            assert_eq!(g.light(id, 0).is_some(), l.control == Control::Signal);
            for t in g.next(id) {
                assert_eq!(t.from, id);
                assert_eq!(t.node, l.end_node);
            }
        }
    }
}
