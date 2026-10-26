//! Road meshes: asphalt ribbons, generic polygon junctions, curbs with gutters, sidewalks
//! with seams, crosswalks and lane paint.
//!
//! Junctions are built the same way for any number of incoming roads: the roads are
//! sorted counter-clockwise around the node and each consecutive pair forms a corner that
//! is either a curb return (a fillet arc, for turns under ~175 degrees), straight (the two
//! curbs continue each other) or reflex (the outside of a bend: two curbs meet at a point).
//! Each road end is trimmed back to where its corners end; the junction asphalt is the
//! star-shaped polygon between the trimmed ends.

use alloc::vec::Vec;

use place::{Place, RoadClass, ZoneId, SIDEWALK_WIDTH_M};

use crate::math::{floor, intersect_lines, lerp, value_noise, V2, V3};
use crate::mesh::{quad, quad_vc, tri, Paint, Pattern, WorldMesh};
use crate::palette::{mix, shade, Palette};
use crate::{MeshKind, MeshSet, CURB_M, PAINT_LIFT, TERRAIN_Y};

/// Concrete gutter pan along each curb, inside the curb-to-curb width.
const GUTTER_M: f32 = 0.45;
/// Curb top strip, part of the sidewalk width.
const CURB_TOP_M: f32 = 0.2;
/// Grass parkway between curb and walk on residential streets.
const PARKWAY_M: f32 = 1.0;
/// Sidewalk slab length (seam spacing).
const SLAB_M: f32 = 1.5;
/// Width of the seam between slabs.
const SEAM_M: f32 = 0.03;
/// Asphalt ribbon subdivision along the road, for vertex-color variation.
const RIBBON_STEP_M: f32 = 12.0;
/// Rng/noise keys.
const KEY_ASPHALT: u64 = 0x4153_5048;
const KEY_SLABS: u64 = 0x534C_4142;
const KEY_PAINT: u64 = 0x5041_494E;

/// One road arriving at a junction, seen from the junction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadEnd {
    pub edge: usize,
    /// Unit direction from the node along the road.
    pub dir: V2,
    /// Half the curb-to-curb width.
    pub hw: f32,
    /// Sidewalk width (0 without sidewalks).
    pub sw: f32,
    pub class: RoadClass,
    /// Distance from the node at which the road's own asphalt ribbon starts.
    pub trim: f32,
    /// Distance at which the curb on the left (counter-clockwise) side of `dir` starts.
    pub s_left: f32,
    /// Same for the right side.
    pub s_right: f32,
}

/// The corner between consecutive ends `a` and `b` (counter-clockwise), lying on `a`'s
/// left and `b`'s right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Corner {
    /// Curb return: arc of radius `r` around `q` from `ta` (on `a`'s curb) to `tb`.
    Fillet {
        q: V2,
        r: f32,
        ta: V2,
        tb: V2,
        back_a: V2,
        back_b: V2,
        prop: V2,
    },
    /// The two curbs meet at `c`; `prop` is the property-line corner behind it.
    Reflex {
        c: V2,
        back_a: V2,
        back_b: V2,
        prop: V2,
    },
    /// The curbs continue each other through the node.
    Straight {
        ta: V2,
        tb: V2,
        back_a: V2,
        back_b: V2,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Junction {
    pub node: usize,
    pub pos: V2,
    /// Sorted counter-clockwise seen from above.
    pub ends: Vec<RoadEnd>,
    /// `corners[i]` lies between `ends[i]` and `ends[(i + 1) % n]`; empty for dead ends.
    pub corners: Vec<Corner>,
}

/// The road graph prepared for meshing and prop placement.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub junctions: Vec<Junction>,
    /// For each edge, `(junction, slot)` of its `a` end and its `b` end.
    pub edge_ends: Vec<[(usize, usize); 2]>,
}

impl Network {
    pub fn new(place: &Place) -> Network {
        let nodes = &place.roads.nodes;
        let edges = &place.roads.edges;
        let mut incident: Vec<Vec<RoadEnd>> = (0..nodes.len()).map(|_| Vec::new()).collect();
        for (i, e) in edges.iter().enumerate() {
            let (a, b) = (
                V2::from_array(nodes[e.a as usize].pos),
                V2::from_array(nodes[e.b as usize].pos),
            );
            let d = (b - a).normalize();
            let sw = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
            for (node, dir) in [(e.a, d), (e.b, -d)] {
                incident[node as usize].push(RoadEnd {
                    edge: i,
                    dir,
                    hw: e.width_m * 0.5,
                    sw,
                    class: e.class,
                    trim: 0.0,
                    s_left: 0.0,
                    s_right: 0.0,
                });
            }
        }
        let mut junctions = Vec::with_capacity(nodes.len());
        let mut edge_ends = alloc::vec![[(0, 0); 2]; edges.len()];
        for (node, mut ends) in incident.into_iter().enumerate() {
            // Stable sort by angle, ties broken by edge index: deterministic.
            ends.sort_by(|x, y| {
                x.dir
                    .pseudo_angle()
                    .total_cmp(&y.dir.pseudo_angle())
                    .then(x.edge.cmp(&y.edge))
            });
            let pos = V2::from_array(nodes[node].pos);
            let n = ends.len();
            let mut corners = Vec::new();
            if n >= 2 {
                for i in 0..n {
                    let (a, b) = (ends[i], ends[(i + 1) % n]);
                    let (corner, sa, sb) = make_corner(pos, &a, &b);
                    corners.push(corner);
                    ends[i].s_left = sa;
                    ends[(i + 1) % n].s_right = sb;
                }
                for e in &mut ends {
                    e.trim = e.s_left.max(e.s_right).max(0.0);
                }
            }
            let j = junctions.len();
            for (slot, e) in ends.iter().enumerate() {
                let which = usize::from(edges[e.edge].a as usize != node);
                edge_ends[e.edge][which] = (j, slot);
            }
            junctions.push(Junction {
                node,
                pos,
                ends,
                corners,
            });
        }
        Network {
            junctions,
            edge_ends,
        }
    }

    /// The two ends of edge `e`: at node `a` and at node `b`.
    pub fn ends(&self, e: usize) -> (&RoadEnd, &RoadEnd) {
        let [(ja, sa), (jb, sb)] = self.edge_ends[e];
        (&self.junctions[ja].ends[sa], &self.junctions[jb].ends[sb])
    }

    pub fn degree(&self, junction: usize) -> usize {
        self.junctions[junction].ends.len()
    }
}

/// Corner radius of the curb return between two roads.
fn fillet_radius(a: &RoadEnd, b: &RoadEnd) -> f32 {
    if a.class == RoadClass::Local && b.class == RoadClass::Local {
        4.5
    } else {
        6.0
    }
}

/// Builds the corner between `a` (its left side) and `b` (its right side) and returns the
/// distances along each road at which its curb starts.
fn make_corner(n: V2, a: &RoadEnd, b: &RoadEnd) -> (Corner, f32, f32) {
    let (la, rb) = (a.dir.left(), b.dir.right());
    let turn = a.dir.turn(b.dir);
    let line_a = |off: f32| n + la * off;
    let line_b = |off: f32| n + rb * off;
    let meet = |oa: f32, ob: f32| intersect_lines(line_a(oa), a.dir, line_b(ob), b.dir);
    // sin(5 degrees): closer to straight than this counts as straight.
    const STRAIGHT: f32 = 0.087;
    if turn > STRAIGHT {
        let r = fillet_radius(a, b);
        if let (Some(q), Some(prop)) = (meet(a.hw + r, b.hw + r), meet(a.hw + a.sw, b.hw + b.sw)) {
            let sa = (q - n).dot(a.dir);
            let sb = (q - n).dot(b.dir);
            let corner = Corner::Fillet {
                q,
                r,
                ta: n + a.dir * sa + la * a.hw,
                tb: n + b.dir * sb + rb * b.hw,
                back_a: n + a.dir * sa + la * (a.hw + a.sw),
                back_b: n + b.dir * sb + rb * (b.hw + b.sw),
                prop,
            };
            return (corner, sa, sb);
        }
    } else if turn < -STRAIGHT {
        if let (Some(c), Some(prop)) = (meet(a.hw, b.hw), meet(a.hw + a.sw, b.hw + b.sw)) {
            let sa = (c - n).dot(a.dir);
            let sb = (c - n).dot(b.dir);
            let corner = Corner::Reflex {
                c,
                back_a: c + la * a.sw,
                back_b: c + rb * b.sw,
                prop,
            };
            return (corner, sa, sb);
        }
    }
    let (ta, tb) = (line_a(a.hw), line_b(b.hw));
    let corner = Corner::Straight {
        ta,
        tb,
        back_a: ta + la * a.sw,
        back_b: tb + rb * b.sw,
    };
    (corner, 0.0, 0.0)
}

/// What lines a side of a road: decides parkway grass versus full-width concrete.
pub fn side_has_parkway(class: RoadClass, zone: Option<ZoneId>) -> bool {
    class == RoadClass::Local && matches!(zone, Some(ZoneId::Residential) | Some(ZoneId::Park))
}

/// Paints used by the road builder.
struct RoadPaints {
    asphalt: Paint,
    asphalt_worn: [u8; 3],
    gutter: Paint,
    curb: Paint,
    curb_top: Paint,
    walk: Paint,
    grout: Paint,
    grass: Paint,
    white: Paint,
    yellow: Paint,
}

impl RoadPaints {
    fn new(p: &Palette) -> RoadPaints {
        RoadPaints {
            asphalt: Paint::new(p.asphalt, Pattern::Asphalt),
            asphalt_worn: p.asphalt_worn,
            gutter: Paint::new(mix(p.concrete_dark, p.concrete, 60), Pattern::Concrete),
            curb: Paint::new(shade(p.concrete, 230), Pattern::Concrete),
            curb_top: Paint::new(mix(p.concrete, p.paint_white, 110), Pattern::Concrete),
            walk: Paint::new(p.concrete, Pattern::Concrete),
            grout: Paint::new(shade(p.concrete_dark, 150), Pattern::Concrete),
            grass: Paint::new(p.lawn, Pattern::Lawn),
            white: Paint::new(p.paint_white, Pattern::Paint),
            yellow: Paint::new(p.paint_yellow, Pattern::Paint),
        }
    }
}

/// Builds every road and junction mesh into `out`.
pub fn build(
    place: &Place,
    net: &Network,
    side_zone: &[[Option<ZoneId>; 2]],
    pal: &Palette,
    seed: u64,
    out: &mut MeshSet,
) {
    let paints = RoadPaints::new(pal);
    let nodes = &place.roads.nodes;
    for (i, e) in place.roads.edges.iter().enumerate() {
        let a = V2::from_array(nodes[e.a as usize].pos);
        let b = V2::from_array(nodes[e.b as usize].pos);
        let (ea, eb) = net.ends(i);
        let len = (b - a).length();
        let d = (b - a).normalize();
        let rf = RoadFrame { a, d, l: d.left() };
        let mid = (a + b) * 0.5;
        let deg = [
            net.degree(net.edge_ends[i][0].0),
            net.degree(net.edge_ends[i][1].0),
        ];
        {
            let m = out.at(mid, MeshKind::Road);
            ribbon(m, &rf, ea.trim, len - eb.trim, ea.hw, &paints, seed);
            // Left side runs from a's left curb to b's right curb, and vice versa.
            let sides = [
                (1.0, ea.s_left, len - eb.s_right),
                (-1.0, ea.s_right, len - eb.s_left),
            ];
            for (k, (sigma, s0, s1)) in sides.into_iter().enumerate() {
                let parkway = side_has_parkway(e.class, side_zone[i][k]);
                side_strip(
                    m,
                    &rf,
                    sigma,
                    s0,
                    s1,
                    ea.hw,
                    ea.sw,
                    parkway,
                    &paints,
                    seed ^ i as u64,
                );
            }
        }
        let m = out.at(mid, MeshKind::Marking);
        markings(
            m,
            &rf,
            ea,
            eb,
            len,
            deg,
            e.class,
            e.lanes(),
            &paints,
            seed ^ i as u64,
        );
    }
    for j in &net.junctions {
        junction(out.at(j.pos, MeshKind::Road), j, &paints, seed);
    }
}

/// A road's local frame on the ground: origin at node `a`, `d` along, `l` to the left.
struct RoadFrame {
    a: V2,
    d: V2,
    l: V2,
}

impl RoadFrame {
    /// Point at distance `s` along the road and lateral offset `t` (positive left).
    fn p(&self, s: f32, t: f32, y: f32) -> V3 {
        (self.a + self.d * s + self.l * t).at(y)
    }

    /// Flat quad covering `s0..s1` along and `t0..t1` across at height `y`.
    #[allow(clippy::too_many_arguments)]
    fn flat(&self, m: &mut WorldMesh, s0: f32, s1: f32, t0: f32, t1: f32, y: f32, paint: Paint) {
        if s1 - s0 <= 1e-4 || abs_diff(t1, t0) <= 1e-4 {
            return;
        }
        let q = [
            self.p(s0, t0, y),
            self.p(s1, t0, y),
            self.p(s1, t1, y),
            self.p(s0, t1, y),
        ];
        quad(m, q, V3::Y, paint);
    }

    /// Vertical quad at lateral `t` from `y0` to `y1`, facing toward `+left * facing`.
    #[allow(clippy::too_many_arguments)]
    fn wall(
        &self,
        m: &mut WorldMesh,
        s0: f32,
        s1: f32,
        t: f32,
        y0: f32,
        y1: f32,
        facing: f32,
        paint: Paint,
    ) {
        if s1 - s0 <= 1e-4 {
            return;
        }
        let q = [
            self.p(s0, t, y0),
            self.p(s1, t, y0),
            self.p(s1, t, y1),
            self.p(s0, t, y1),
        ];
        quad(m, q, (self.l * facing).at(0.0), paint);
    }
}

fn abs_diff(a: f32, b: f32) -> f32 {
    if a > b {
        a - b
    } else {
        b - a
    }
}

/// Asphalt with low-frequency patching variation in the vertex colors.
#[allow(clippy::too_many_arguments)]
fn ribbon(m: &mut WorldMesh, rf: &RoadFrame, s0: f32, s1: f32, hw: f32, p: &RoadPaints, seed: u64) {
    if s1 <= s0 {
        return;
    }
    let steps = ((s1 - s0) / RIBBON_STEP_M) as u32 + 1;
    let lanes = [-hw, 0.0, hw];
    let tint = |pt: V3| {
        let n = value_noise(seed ^ KEY_ASPHALT, pt.x, pt.z, 9.0);
        mix(p.asphalt.rgb(), p.asphalt_worn, (n * n * 300.0) as u32)
    };
    for k in 0..steps {
        let sa = lerp(s0, s1, k as f32 / steps as f32);
        let sb = lerp(s0, s1, (k + 1) as f32 / steps as f32);
        for w in lanes.windows(2) {
            let q = [
                rf.p(sa, w[0], 0.0),
                rf.p(sb, w[0], 0.0),
                rf.p(sb, w[1], 0.0),
                rf.p(sa, w[1], 0.0),
            ];
            quad_vc(m, q, V3::Y, p.asphalt, q.map(tint));
        }
    }
}

/// Gutter, curb, curb top, parkway, slabbed walk and back skirt along one side.
#[allow(clippy::too_many_arguments)]
fn side_strip(
    m: &mut WorldMesh,
    rf: &RoadFrame,
    sigma: f32,
    s0: f32,
    s1: f32,
    hw: f32,
    sw: f32,
    parkway: bool,
    p: &RoadPaints,
    seed: u64,
) {
    if s1 <= s0 {
        return;
    }
    let t = |off: f32| sigma * off;
    // Gutter pan, a hair above the asphalt.
    rf.flat(
        m,
        s0,
        s1,
        t(hw - GUTTER_M),
        t(hw),
        PAINT_LIFT * 0.8,
        p.gutter,
    );
    // Curb face looks back toward the road center.
    rf.wall(m, s0, s1, t(hw), 0.0, CURB_M, -sigma, p.curb);
    if sw <= 0.0 {
        rf.flat(m, s0, s1, t(hw), t(hw + CURB_TOP_M), CURB_M, p.curb_top);
        rf.wall(
            m,
            s0,
            s1,
            t(hw + CURB_TOP_M),
            TERRAIN_Y,
            CURB_M,
            sigma,
            p.curb,
        );
        return;
    }
    rf.flat(m, s0, s1, t(hw), t(hw + CURB_TOP_M), CURB_M, p.curb_top);
    let mut walk0 = hw + CURB_TOP_M;
    if parkway {
        let g = walk0 + PARKWAY_M;
        rf.flat(m, s0, s1, t(walk0), t(g), CURB_M - 0.01, p.grass);
        walk0 = g;
    }
    let walk1 = hw + sw;
    // Grout below, slabs on top: the seams are the gaps between slabs.
    rf.flat(m, s0, s1, t(walk0), t(walk1), CURB_M - 0.012, p.grout);
    let n = ((s1 - s0) / SLAB_M) as u32;
    let n = n.max(1);
    let step = (s1 - s0) / n as f32;
    for k in 0..n {
        let a = s0 + step * k as f32;
        let h = skywave_core::mix64(seed ^ KEY_SLABS ^ u64::from(k) << 8);
        let tone = 244 + (h & 15) as u32;
        let paint = p.walk.with_rgb(shade(p.walk.rgb(), tone));
        rf.flat(
            m,
            a + SEAM_M * 0.5,
            a + step - SEAM_M * 0.5,
            t(walk0),
            t(walk1),
            CURB_M,
            paint,
        );
    }
    rf.wall(m, s0, s1, t(walk1), TERRAIN_Y, CURB_M, sigma, p.curb);
}

/// Crosswalks, stop bars and lane lines on one edge.
#[allow(clippy::too_many_arguments)]
fn markings(
    m: &mut WorldMesh,
    rf: &RoadFrame,
    ea: &RoadEnd,
    eb: &RoadEnd,
    len: f32,
    deg: [usize; 2],
    class: RoadClass,
    lanes: u8,
    p: &RoadPaints,
    seed: u64,
) {
    let hw = ea.hw;
    let (s0, s1) = (ea.trim, len - eb.trim);
    if s1 - s0 < 12.0 {
        return;
    }
    let y = PAINT_LIFT;
    let inner = hw - GUTTER_M;
    let worn = |paint: Paint, k: u32| {
        let h = skywave_core::mix64(seed ^ KEY_PAINT ^ u64::from(k));
        paint.with_rgb(shade(paint.rgb(), 220 + (h & 31) as u32))
    };
    let mut m0 = s0 + 1.0;
    let mut m1 = s1 - 1.0;
    // Crosswalk and stop bar at each signal/stop-controlled end.
    for (end, junction_deg) in [(0, deg[0]), (1, deg[1])] {
        if junction_deg < 3 {
            continue;
        }
        let (c0, c1, stop0, stop1, t0, t1) = if end == 0 {
            (s0 + 0.8, s0 + 3.8, s0 + 4.4, s0 + 4.85, 0.25, inner - 0.1)
        } else {
            (
                s1 - 3.8,
                s1 - 0.8,
                s1 - 4.85,
                s1 - 4.4,
                -(inner - 0.1),
                -0.25,
            )
        };
        let span = 2.0 * (inner - 0.3);
        let bars = floor((span + 0.6) / 1.2) as i32;
        let start = -(bars as f32 * 1.2 - 0.6) * 0.5;
        for k in 0..bars {
            let t = start + k as f32 * 1.2;
            rf.flat(
                m,
                c0,
                c1,
                t,
                t + 0.6,
                y,
                worn(p.white, k as u32 + 100 * end),
            );
        }
        rf.flat(m, stop0, stop1, t0, t1, y, p.white);
        if end == 0 {
            m0 = stop1 + 0.5;
        } else {
            m1 = stop0 - 0.5;
        }
    }
    if m1 - m0 < 3.0 {
        return;
    }
    let dashed = |m: &mut WorldMesh, t: f32, w: f32, dash: f32, gap: f32, paint: Paint| {
        let period = dash + gap;
        let count = floor((m1 - m0 + gap) / period) as i32;
        let lead = (m1 - m0 - (count as f32 * period - gap)) * 0.5;
        for k in 0..count {
            let a = m0 + lead + k as f32 * period;
            rf.flat(
                m,
                a,
                a + dash,
                t - w * 0.5,
                t + w * 0.5,
                y,
                worn(paint, k as u32),
            );
        }
    };
    match class {
        RoadClass::Local
        | RoadClass::Collector
        | RoadClass::Alley
        | RoadClass::Beachfront
        | RoadClass::Rural => {
            dashed(m, 0.0, 0.12, 3.0, 6.0, p.yellow);
        }
        RoadClass::Arterial | RoadClass::Highway => {
            for t in [0.14, -0.14] {
                rf.flat(m, m0, m1, t - 0.06, t + 0.06, y, p.yellow);
            }
            let per_side = u32::from(lanes / 2).max(1);
            let lane_w = inner / per_side as f32;
            for k in 1..per_side {
                for sign in [1.0, -1.0] {
                    dashed(m, sign * lane_w * k as f32, 0.12, 3.0, 9.0, p.white);
                }
            }
            for sign in [1.0, -1.0] {
                let t = sign * (inner - 0.2);
                rf.flat(m, m0, m1, t - 0.06, t + 0.06, y, p.white);
            }
        }
    }
}

/// Junction asphalt (a fan around the node) and the corner curbs and sidewalks.
fn junction(m: &mut WorldMesh, j: &Junction, p: &RoadPaints, _seed: u64) {
    let n = j.ends.len();
    if n < 2 {
        return;
    }
    let c = j.pos;
    let mut ring: Vec<V2> = Vec::with_capacity(n * 10);
    for i in 0..n {
        let e = &j.ends[i];
        ring.push(c + e.dir * e.trim + e.dir.right() * e.hw);
        ring.push(c + e.dir * e.trim + e.dir.left() * e.hw);
        match j.corners[i] {
            Corner::Fillet { q, r, ta, tb, .. } => {
                for pt in arc(q, r, ta, tb) {
                    ring.push(pt);
                }
            }
            Corner::Reflex { c: pc, .. } => ring.push(pc),
            Corner::Straight { ta, tb, .. } => {
                ring.push(ta);
                ring.push(tb);
            }
        }
    }
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        tri(m, c.at(0.0), a.at(0.0), b.at(0.0), V3::Y, p.asphalt);
    }
    for (i, corner) in j.corners.iter().enumerate() {
        let (ea, eb) = (&j.ends[i], &j.ends[(i + 1) % n]);
        match *corner {
            Corner::Fillet {
                q,
                r,
                ta,
                tb,
                back_a,
                back_b,
                prop,
            } => {
                let pts = arc(q, r, ta, tb);
                for w in pts.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let away = ((a + b) * 0.5 - q).at(0.0);
                    // Curb face toward the road, gutter band on the asphalt.
                    quad(
                        m,
                        [a.at(0.0), b.at(0.0), b.at(CURB_M), a.at(CURB_M)],
                        away,
                        p.curb,
                    );
                    let ga = a + (a - q).normalize() * GUTTER_M;
                    let gb = b + (b - q).normalize() * GUTTER_M;
                    let y = PAINT_LIFT * 0.8;
                    quad(m, [a.at(y), b.at(y), gb.at(y), ga.at(y)], V3::Y, p.gutter);
                }
                // Corner sidewalk: fan from the property corner over the curb arc.
                let mut fan: Vec<V2> = Vec::with_capacity(pts.len() + 2);
                fan.push(back_a);
                fan.extend_from_slice(&pts);
                fan.push(back_b);
                for w in fan.windows(2) {
                    tri(
                        m,
                        prop.at(CURB_M),
                        w[0].at(CURB_M),
                        w[1].at(CURB_M),
                        V3::Y,
                        p.walk,
                    );
                }
                skirt(m, back_a, prop, ea.dir.left(), p);
                skirt(m, prop, back_b, eb.dir.right(), p);
            }
            Corner::Reflex {
                c: pc,
                back_a,
                back_b,
                prop,
            } => {
                let y = CURB_M;
                quad(
                    m,
                    [pc.at(y), back_a.at(y), prop.at(y), back_b.at(y)],
                    V3::Y,
                    p.walk,
                );
                skirt(m, back_a, prop, ea.dir.left(), p);
                skirt(m, prop, back_b, eb.dir.right(), p);
            }
            Corner::Straight {
                ta,
                tb,
                back_a,
                back_b,
            } => {
                let y = CURB_M;
                quad(
                    m,
                    [ta.at(y), back_a.at(y), back_b.at(y), tb.at(y)],
                    V3::Y,
                    p.walk,
                );
            }
        }
    }
}

/// Back face of a sidewalk edge, down to the terrain, facing `out`.
fn skirt(m: &mut WorldMesh, a: V2, b: V2, out: V2, p: &RoadPaints) {
    quad(
        m,
        [a.at(TERRAIN_Y), b.at(TERRAIN_Y), b.at(CURB_M), a.at(CURB_M)],
        out.at(0.0),
        p.curb,
    );
}

/// Points on the arc of radius `r` around `q` from `a` to `b` (both included).
pub(crate) fn arc(q: V2, r: f32, a: V2, b: V2) -> Vec<V2> {
    const SEGMENTS: u32 = 6;
    let (va, vb) = (a - q, b - q);
    (0..=SEGMENTS)
        .map(|k| {
            let t = k as f32 / SEGMENTS as f32;
            q + va.lerp(vb, t).normalize() * r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_way_and_tee_junctions_have_fillets_and_trims() {
        let place = Place::test_grid();
        let net = Network::new(&place);
        let degrees: Vec<usize> = (0..net.junctions.len()).map(|j| net.degree(j)).collect();
        assert!(degrees.contains(&4));
        assert!(degrees.contains(&3));
        assert!(degrees.contains(&2));
        for j in &net.junctions {
            assert_eq!(j.corners.len(), j.ends.len());
            let fillets = j
                .corners
                .iter()
                .filter(|c| matches!(c, Corner::Fillet { .. }))
                .count();
            match j.ends.len() {
                4 => assert_eq!(fillets, 4),
                3 => assert_eq!(fillets, 2),
                2 => assert_eq!(fillets, 1),
                _ => {}
            }
            for e in &j.ends {
                assert!(e.trim >= 0.0 && e.trim < 20.0, "{}", e.trim);
                // A local meeting another road is trimmed to the far curb plus the radius.
                if j.ends.len() == 4 && e.class == RoadClass::Local {
                    assert!(e.trim >= 5.0 + 4.5 - 1e-3, "{}", e.trim);
                }
            }
        }
        // Every edge's two ends point at each other.
        for e in 0..place.roads.edges.len() {
            let (a, b) = net.ends(e);
            assert_eq!(a.edge, e);
            assert_eq!(b.edge, e);
            assert!((a.dir + b.dir).length() < 1e-5);
        }
    }

    #[test]
    fn five_way_junction_is_generic() {
        // A star of five roads at uneven angles.
        let mut place = Place::test_grid();
        place.roads.nodes.clear();
        place.roads.edges.clear();
        place.roads.nodes.push(place::Node {
            pos: [0.0, 0.0],
            elevation: 0.0,
        });
        let template = Place::test_grid().roads.edges[0].clone();
        for (k, (x, z)) in [
            (80.0, 0.0),
            (30.0, -70.0),
            (-60.0, -50.0),
            (-70.0, 40.0),
            (10.0, 90.0),
        ]
        .into_iter()
        .enumerate()
        {
            place.roads.nodes.push(place::Node {
                pos: [x, z],
                elevation: 0.0,
            });
            place.roads.edges.push(place::Edge {
                a: 0,
                b: k as u32 + 1,
                ..template.clone()
            });
        }
        let net = Network::new(&place);
        let j = &net.junctions[0];
        assert_eq!(j.ends.len(), 5);
        assert!(j.corners.iter().all(|c| matches!(c, Corner::Fillet { .. })));
        let mut m = WorldMesh::default();
        junction(&mut m, j, &RoadPaints::new(&Palette::socal_1985()), 0);
        assert!(m.triangle_count() > 40);
        assert!(m
            .vertices
            .iter()
            .all(|v| v.normal.iter().all(|c| c.is_finite())));
    }
}
