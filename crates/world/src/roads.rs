//! Road meshes: asphalt ribbons, generic polygon junctions, curbs with gutters, sidewalks
//! with seams, crosswalks and lane paint, and decks with parapets and piers where a road
//! rides above the ground.
//!
//! Every edge is laid along its profile ([`EdgePath`]): a polyline through the profile
//! samples, straight from each node out to where its junction trims it, with miter offsets
//! at the bends. Surfaces take their height from the profile: a point's height is the
//! profile's at the nearest point of the centerline, so cross sections are level and the
//! road sits exactly on its profile (the hand-laid test town's profiles are flat at 0).
//!
//! Junctions are built the same way for any number of incoming roads: the roads are
//! sorted counter-clockwise around the node and each consecutive pair forms a corner that
//! is either a curb return (a fillet arc, for turns under ~175 degrees), straight (the two
//! curbs continue each other) or reflex (the outside of a bend: two curbs meet at a point).
//! Each road end is trimmed back to where its corners end; the junction asphalt is the
//! star-shaped polygon between the trimmed ends, a fan from the node (at the average height
//! of the incoming profiles' ends) to the trimmed ends (at their own profile heights).

use alloc::vec::Vec;

use place::{Place, RoadClass, ZoneId, SIDEWALK_WIDTH_M};

use crate::math::{floor, intersect_lines, lerp, segment_distance, value_noise, V2, V3};
use crate::mesh::terrain::TerrainGrid;
use crate::mesh::{quad, quad_vc, tri, Paint, Pattern, WorldMesh};
use crate::palette::{mix, shade, Palette};
use crate::{MeshKind, MeshSet, CURB_M, PAINT_LIFT, TERRAIN_Y};

/// Concrete gutter pan along each curb, inside the curb-to-curb width.
pub(crate) const GUTTER_M: f32 = 0.45;
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
/// A road more than this far above the (cut and filled) ground gets a deck.
pub const DECK_CLEARANCE_M: f32 = 2.0;
/// Deck slab thickness; on terrain places sidewalk backs reach this far down too.
pub const DECK_THICKNESS_M: f32 = 1.0;
/// Parapet height above the curb top, and its thickness.
const PARAPET_M: f32 = 0.9;
const PARAPET_THICK_M: f32 = 0.3;
/// Pier spacing along a deck and the pier's square section.
const PIER_SPACING_M: f32 = 20.0;
const PIER_M: f32 = 1.2;
/// Highway cross section: median barrier, lanes and paint.
pub(crate) const MEDIAN_HALF_M: f32 = 1.2;
const BARRIER_M: f32 = 0.8;
pub(crate) const HIGHWAY_LANE_M: f32 = 3.6;
/// Rng/noise keys.
const KEY_ASPHALT: u64 = 0x4153_5048;
const KEY_SLABS: u64 = 0x534C_4142;
const KEY_PAINT: u64 = 0x5041_494E;

/// One road arriving at a junction, seen from the junction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadEnd {
    pub edge: usize,
    /// Unit direction from the node along the road's first profile segment.
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
    /// Stretch `[from, to]` (distances from the node) where the left curb gives way to a
    /// road merging in from that side; `[0, 0]` for none.
    pub gap_left: [f32; 2],
    /// Same for the right side.
    pub gap_right: [f32; 2],
}

/// A road joining another at a shallow angle (a ramp at the highway): it is not part of
/// the junction's fan; its ribbon starts at the gore, where its inner curb leaves the
/// other road's pavement, and a wedge of asphalt fills its pavement outside the other
/// road's from where its outer edge leaves it to the gore.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Merge {
    pub end: RoadEnd,
    /// Slot in `Junction::ends` of the road it joins.
    pub major: usize,
    /// Distances from the node along the merging road where its outer edge and its inner
    /// curb leave the major road's pavement.
    pub s_outer: f32,
    pub s_gore: f32,
    /// 1 when it lies to the major's left, -1 to its right.
    pub side: f32,
}

/// Ends meeting at less than this angle's cosine (about 30 degrees) merge.
const MERGE_COS: f32 = 0.866;
/// Merges whose gore would lie farther than this from the node are ordinary corners.
const MERGE_MAX_M: f32 = 160.0;

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
    /// Height of the junction's center: the mean of the incoming profiles' end heights.
    pub y: f32,
    /// Sorted counter-clockwise seen from above.
    pub ends: Vec<RoadEnd>,
    /// `corners[i]` lies between `ends[i]` and `ends[(i + 1) % n]`; empty for dead ends.
    pub corners: Vec<Corner>,
    /// Roads merging into one of `ends` at a shallow angle (slots after `ends`).
    pub merges: Vec<Merge>,
}

/// The centerline an edge's meshes are laid along, and its height profile.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgePath {
    /// The profile as the place gives it, node to node (`x, z`), interior samples closer
    /// than a meter to their neighbor dropped...
    pub raw: Vec<V2>,
    /// ...and its heights.
    pub raw_y: Vec<f32>,
    /// The height of every profile sample when they are all the same.
    pub flat: Option<f32>,
    /// Layout centerline: straight from each node to its trim, then the profile samples.
    pub pts: Vec<V2>,
    /// Arc length at each of `pts`.
    pub s: Vec<f32>,
    /// Unit direction of each segment.
    pub d: Vec<V2>,
    /// Offset direction at each of `pts`, toward the left: the end normals at the nodes
    /// and trims, miters (lengthened to keep the width) at the bends.
    pub n: Vec<V2>,
}

impl EdgePath {
    /// Total length of the layout centerline.
    pub fn len(&self) -> f32 {
        self.s[self.s.len() - 1]
    }

    /// Whether the path has no length.
    pub fn is_empty(&self) -> bool {
        self.len() <= 0.0
    }

    fn seg(&self, s: f32) -> usize {
        let last = self.d.len() - 1;
        for k in 0..last {
            if s < self.s[k + 1] {
                return k;
            }
        }
        last
    }

    /// Ground point at distance `s` along the centerline, `t` to its left.
    pub fn xz(&self, s: f32, t: f32) -> V2 {
        let k = self.seg(s);
        let ds = s - self.s[k];
        let c = self.pts[k] + self.d[k] * ds;
        let (n0, n1) = (self.n[k], self.n[k + 1]);
        let n = if n0 == n1 {
            n0
        } else {
            n0.lerp(n1, ds / (self.s[k + 1] - self.s[k]))
        };
        c + n * t
    }

    /// Left normal of the centerline at `s` (for facing).
    pub fn left(&self, s: f32) -> V2 {
        self.d[self.seg(s)].left()
    }

    /// Road surface height at ground point `p`: the profile's at the nearest point of the
    /// profile polyline.
    pub fn y_at(&self, p: V2) -> f32 {
        if let Some(y) = self.flat {
            return y;
        }
        profile_height(&self.raw, &self.raw_y, p)
    }

    /// Surface point at `(s, t)`, raised by `lift`.
    pub fn p(&self, s: f32, t: f32, lift: f32) -> V3 {
        let q = self.xz(s, t);
        q.at(self.y_at(q) + lift)
    }

    /// Calls `f(a, b)` for the pieces of `s0..s1` between the centerline's bends.
    pub fn pieces(&self, s0: f32, s1: f32, mut f: impl FnMut(f32, f32)) {
        let mut a = s0;
        for &b in &self.s[1..self.s.len() - 1] {
            if b > a && b < s1 {
                f(a, b);
                a = b;
            }
        }
        f(a, s1);
    }
}

/// Width over which a point's height blends between the profile segments it is nearly
/// equally close to (inside a bend, so the surface has no seam).
pub const BEND_BLEND_M: f32 = 2.0;

/// Height of the polyline `pts` (heights `ys`) at ground point `p`, level across the road:
/// the nearest segment's height line (extended past its ends between segments) at `p`,
/// blended with the lines of segments nearly as close (within [`BEND_BLEND_M`]), so the
/// surface is continuous inside bends and exact along straight stretches.
pub fn profile_height(pts: &[V2], ys: &[f32], p: V2) -> f32 {
    let n = pts.len();
    if n < 2 {
        return ys.first().copied().unwrap_or(0.0);
    }
    let mut d_min = f32::INFINITY;
    for k in 0..n - 1 {
        d_min = d_min.min(segment_distance(p, pts[k], pts[k + 1]).0);
    }
    let (mut sum, mut wsum) = (0.0f32, 0.0f32);
    for k in 0..n - 1 {
        let (a, b) = (pts[k], pts[k + 1]);
        let (d, _) = segment_distance(p, a, b);
        let x = (d - d_min) / BEND_BLEND_M;
        if x >= 1.0 {
            continue;
        }
        let w = (1.0 - x) * (1.0 - x);
        let ab = b - a;
        let l2 = ab.dot(ab);
        let mut t = if l2 > 0.0 { (p - a).dot(ab) / l2 } else { 0.0 };
        // Past the polyline's own ends the height holds.
        if k == 0 {
            t = t.max(0.0);
        }
        if k == n - 2 {
            t = t.min(1.0);
        }
        sum += w * lerp(ys[k], ys[k + 1], t);
        wsum += w;
    }
    sum / wsum
}

/// Edge `e`'s profile polyline from node `a` to node `b` (`x, z` and heights).
fn raw_line(place: &Place, e: &place::Edge) -> (Vec<V2>, Vec<f32>) {
    let nodes = &place.roads.nodes;
    let (na, nb) = (&nodes[e.a as usize], &nodes[e.b as usize]);
    let (a, b) = (V2::from_array(na.pos), V2::from_array(nb.pos));
    let prof = &e.profile;
    let ya = prof.first().map_or(na.elevation, |p| p[1]);
    let yb = prof.last().map_or(nb.elevation, |p| p[1]);
    let mut pts = alloc::vec![a];
    let mut ys = alloc::vec![ya];
    if prof.len() > 2 {
        for q in &prof[1..prof.len() - 1] {
            let p = V2::new(q[0], q[2]);
            if (p - pts[pts.len() - 1]).length() >= 1.0 && (b - p).length() >= 1.0 {
                pts.push(p);
                ys.push(q[1]);
            }
        }
    }
    pts.push(b);
    ys.push(yb);
    (pts, ys)
}

/// Directions from node `a` along the road and from node `b` along the road.
fn end_dirs(raw: &[V2]) -> (V2, V2) {
    let n = raw.len();
    if n == 2 {
        let d = (raw[1] - raw[0]).normalize();
        (d, -d)
    } else {
        (
            (raw[1] - raw[0]).normalize(),
            (raw[n - 2] - raw[n - 1]).normalize(),
        )
    }
}

/// The layout centerline for an edge trimmed `ta` at `a` and `tb` at `b`.
fn layout(raw: Vec<V2>, raw_y: Vec<f32>, ta: f32, tb: f32) -> EdgePath {
    let flat = raw_y[1..]
        .iter()
        .all(|&y| y == raw_y[0])
        .then_some(raw_y[0]);
    let n = raw.len();
    let (da, db) = end_dirs(&raw);
    let (la, lb) = (da.left(), (-db).left());
    // (point, pinned normal) pairs.
    let mut v: Vec<(V2, Option<V2>)> = Vec::with_capacity(n + 2);
    let mut arc = alloc::vec![0.0f32; n];
    for k in 1..n {
        arc[k] = arc[k - 1] + (raw[k] - raw[k - 1]).length();
    }
    let total = arc[n - 1];
    if n == 2 || ta + tb + 2.0 >= total {
        v.push((raw[0], Some(la)));
        v.push((raw[n - 1], Some(lb)));
    } else {
        v.push((raw[0], Some(la)));
        if ta > 0.01 {
            v.push((raw[0] + da * ta, Some(la)));
        }
        for k in 1..n - 1 {
            if arc[k] > ta + 1.0 && arc[k] < total - tb - 1.0 {
                v.push((raw[k], None));
            }
        }
        if tb > 0.01 {
            v.push((raw[n - 1] + db * tb, Some(lb)));
        }
        v.push((raw[n - 1], Some(lb)));
    }
    let pts: Vec<V2> = v.iter().map(|x| x.0).collect();
    let mut d = Vec::with_capacity(pts.len() - 1);
    let mut s = alloc::vec![0.0f32];
    for k in 0..pts.len() - 1 {
        let seg = pts[k + 1] - pts[k];
        d.push(seg.normalize());
        s.push(s[k] + seg.length());
    }
    if n == 2 {
        // Exactly the straight frame: direction a to b, its left normal.
        let dd = (raw[1] - raw[0]).normalize();
        d[0] = dd;
        v[0].1 = Some(dd.left());
        v[1].1 = Some(dd.left());
    }
    let normals = (0..pts.len())
        .map(|k| match v[k].1 {
            Some(nrm) => nrm,
            None => {
                let (u, w) = (d[k - 1].left(), d[k].left());
                let m = (u + w).normalize();
                let c = m.dot(u);
                if c <= 0.0 {
                    u
                } else {
                    m * (1.0 / c.max(0.5))
                }
            }
        })
        .collect();
    EdgePath {
        raw,
        raw_y,
        flat,
        pts,
        s,
        d,
        n: normals,
    }
}

/// The road graph prepared for meshing and prop placement.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub junctions: Vec<Junction>,
    /// For each edge, `(junction, slot)` of its `a` end and its `b` end.
    pub edge_ends: Vec<[(usize, usize); 2]>,
    /// For each edge, its layout centerline and profile.
    pub paths: Vec<EdgePath>,
}

impl Network {
    pub fn new(place: &Place) -> Network {
        let nodes = &place.roads.nodes;
        let edges = &place.roads.edges;
        let raws: Vec<(Vec<V2>, Vec<f32>)> = edges.iter().map(|e| raw_line(place, e)).collect();
        let lines: Vec<Vec<V2>> = raws.iter().map(|r| r.0.clone()).collect();
        let mut incident: Vec<Vec<RoadEnd>> = (0..nodes.len()).map(|_| Vec::new()).collect();
        let mut end_y: Vec<Vec<f32>> = (0..nodes.len()).map(|_| Vec::new()).collect();
        for (i, e) in edges.iter().enumerate() {
            let (da, db) = end_dirs(&raws[i].0);
            let ys = &raws[i].1;
            let sw = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
            for (node, dir, y) in [(e.a, da, ys[0]), (e.b, db, ys[ys.len() - 1])] {
                incident[node as usize].push(RoadEnd {
                    edge: i,
                    dir,
                    hw: e.width_m * 0.5,
                    sw,
                    class: e.class,
                    trim: 0.0,
                    s_left: 0.0,
                    s_right: 0.0,
                    gap_left: [0.0; 2],
                    gap_right: [0.0; 2],
                });
                end_y[node as usize].push(y);
            }
        }
        let mut junctions = Vec::with_capacity(nodes.len());
        let mut edge_ends = alloc::vec![[(0, 0); 2]; edges.len()];
        for (node, mut ends) in incident.into_iter().enumerate() {
            let ys = &end_y[node];
            let mut y = 0.0;
            for &h in ys {
                y += h;
            }
            let y = if ys.is_empty() {
                nodes[node].elevation
            } else {
                y / ys.len() as f32
            };
            // Stable sort by angle, ties broken by edge index: deterministic.
            ends.sort_by(|x, y| {
                x.dir
                    .pseudo_angle()
                    .total_cmp(&y.dir.pseudo_angle())
                    .then(x.edge.cmp(&y.edge))
            });
            let pos = V2::from_array(nodes[node].pos);
            let at_b: Vec<bool> = ends
                .iter()
                .map(|e| edges[e.edge].a as usize != node)
                .collect();
            let merges_at = merges(&mut ends, &lines, &at_b);
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
            let all = ends.iter().chain(merges_at.iter().map(|m| &m.end));
            for (slot, e) in all.enumerate() {
                let which = usize::from(edges[e.edge].a as usize != node);
                edge_ends[e.edge][which] = (j, slot);
            }
            junctions.push(Junction {
                node,
                pos,
                y,
                ends,
                corners,
                merges: merges_at,
            });
        }
        let paths = raws
            .into_iter()
            .enumerate()
            .map(|(i, (raw, ys))| {
                let [(ja, sa), (jb, sb)] = edge_ends[i];
                // A merging end keeps its curve: no straight run out to its (far) trim.
                let straight = |j: usize, slot: usize| {
                    let jn: &Junction = &junctions[j];
                    if slot < jn.ends.len() {
                        jn.ends[slot].trim
                    } else {
                        0.0
                    }
                };
                layout(raw, ys, straight(ja, sa), straight(jb, sb))
            })
            .collect();
        Network {
            junctions,
            edge_ends,
            paths,
        }
    }

    /// The two ends of edge `e`: at node `a` and at node `b`.
    pub fn ends(&self, e: usize) -> (&RoadEnd, &RoadEnd) {
        let [(ja, sa), (jb, sb)] = self.edge_ends[e];
        (self.junctions[ja].end(sa), self.junctions[jb].end(sb))
    }

    /// Whether edge `e`'s end at node `a` (`b_end` false) or `b` merges into another road.
    pub fn merges_in(&self, e: usize, b_end: bool) -> bool {
        let (j, slot) = self.edge_ends[e][usize::from(b_end)];
        slot >= self.junctions[j].ends.len()
    }

    /// The wedge of merge `mg` at junction `j`, as pairs of points from where the merging
    /// road's outer edge leaves the major road's pavement to the gore: each its outer edge
    /// (at its own height) and where its cross section meets the major's edge (at the
    /// major's height).
    pub fn merge_wedge(&self, j: &Junction, mg: &Merge) -> Vec<(V3, V3)> {
        let e = mg.end.edge;
        let path = &self.paths[e];
        let major = &self.paths[j.ends[mg.major].edge];
        let hw_a = j.ends[mg.major].hw;
        let len = path.len();
        let from_b = self.junctions[self.edge_ends[e][1].0].node == j.node;
        // In the path's own terms: arc from its `a` end, offsets to its left.
        let s_at = |s: f32| if from_b { len - s } else { s };
        let inner = if from_b { mg.side } else { -mg.side } * mg.end.hw;
        let dist = |q: V2| polyline_nearest(&major.raw, q).0 - hw_a;
        let steps = ((mg.s_gore - mg.s_outer) / 4.0) as u32 + 1;
        (0..=steps)
            .map(|k| {
                let s = s_at(lerp(mg.s_outer, mg.s_gore, k as f32 / steps as f32));
                let o = path.xz(s, -inner);
                // Bisect for the major's edge between the outer edge and the inner curb.
                let (mut lo, mut hi) = (-inner, inner);
                for _ in 0..16 {
                    let mid = 0.5 * (lo + hi);
                    if dist(path.xz(s, mid)) >= 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let i = path.xz(s, lo);
                (o.at(path.y_at(o)), i.at(major.y_at(i)))
            })
            .collect()
    }

    pub fn degree(&self, junction: usize) -> usize {
        let j = &self.junctions[junction];
        j.ends.len() + j.merges.len()
    }

    /// The sides of edge `e` where curbs and sidewalks run: for its left side (seen from
    /// `a` toward `b`) then its right, the `[from, to]` stretches along the edge (from its
    /// `a` end), around the corners at both ends and the gaps merging roads leave.
    pub fn side_spans(&self, e: usize) -> [Vec<[f32; 2]>; 2] {
        let (ea, eb) = self.ends(e);
        let len = self.paths[e].len();
        let flip = |g: [f32; 2]| [len - g[1], len - g[0]];
        let sides = [
            (ea.s_left, len - eb.s_right, ea.gap_left, flip(eb.gap_right)),
            (ea.s_right, len - eb.s_left, ea.gap_right, flip(eb.gap_left)),
        ];
        sides.map(|(s0, s1, ga, gb)| {
            let mut spans = alloc::vec![[s0, s1]];
            for g in [ga, gb] {
                if g[1] <= g[0] {
                    continue;
                }
                let mut next = Vec::with_capacity(spans.len() + 1);
                for [a, b] in spans {
                    if g[0] > a {
                        next.push([a, g[0].min(b)]);
                    }
                    if g[1] < b {
                        next.push([g[1].max(a), b]);
                    }
                }
                spans = next;
            }
            spans
        })
    }

    /// Junction `j`'s asphalt as a fan: the center, then the ring of trimmed ends and curb
    /// returns (counter-clockwise), each with its surface height. Empty for dead ends.
    pub fn junction_ring(&self, j: &Junction) -> Vec<V3> {
        let n = j.ends.len();
        let mut ring: Vec<V3> = Vec::with_capacity(n * 10);
        if n < 2 {
            return ring;
        }
        let c = j.pos;
        for i in 0..n {
            let e = &j.ends[i];
            let path = &self.paths[e.edge];
            let next = &self.paths[j.ends[(i + 1) % n].edge];
            let pr = c + e.dir * e.trim + e.dir.right() * e.hw;
            let pl = c + e.dir * e.trim + e.dir.left() * e.hw;
            ring.push(pr.at(path.y_at(pr)));
            ring.push(pl.at(path.y_at(pl)));
            match j.corners[i] {
                Corner::Fillet { q, r, ta, tb, .. } => {
                    let (ya, yb) = (path.y_at(ta), next.y_at(tb));
                    let pts = arc(q, r, ta, tb);
                    let last = (pts.len() - 1) as f32;
                    for (k, pt) in pts.into_iter().enumerate() {
                        ring.push(pt.at(lerp(ya, yb, k as f32 / last)));
                    }
                }
                Corner::Reflex { c: pc, .. } => {
                    ring.push(pc.at(0.5 * (path.y_at(pc) + next.y_at(pc))));
                }
                Corner::Straight { ta, tb, .. } => {
                    ring.push(ta.at(path.y_at(ta)));
                    ring.push(tb.at(next.y_at(tb)));
                }
            }
        }
        ring
    }
}

impl Network {
    /// The sidewalk of corner `i` of junction `j` at curb height, as a fan polygon: for a
    /// curb return the property corner, then the back of `a`'s walk, the curb arc and the
    /// back of `b`'s walk; otherwise the corner's quad. Heights follow the two roads.
    pub fn corner_walk(&self, j: &Junction, i: usize) -> Vec<V3> {
        let n = j.ends.len();
        let (ea, eb) = (&j.ends[i], &j.ends[(i + 1) % n]);
        let y_on = |e: &RoadEnd, q: V2| self.paths[e.edge].y_at(q);
        let y = CURB_M;
        match j.corners[i] {
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
                let (ya, yb) = (y_on(ea, ta), y_on(eb, tb));
                let (yba, ybb) = (y_on(ea, back_a), y_on(eb, back_b));
                let last = (pts.len() - 1) as f32;
                let mut fan: Vec<V3> = Vec::with_capacity(pts.len() + 3);
                fan.push(prop.at(0.5 * (yba + ybb) + y));
                fan.push(back_a.at(yba + y));
                for (k, pt) in pts.iter().enumerate() {
                    fan.push(pt.at(lerp(ya, yb, k as f32 / last) + y));
                }
                fan.push(back_b.at(ybb + y));
                fan
            }
            Corner::Reflex {
                c: pc,
                back_a,
                back_b,
                prop,
            } => {
                let yc = 0.5 * (y_on(ea, pc) + y_on(eb, pc));
                let (yba, ybb) = (y_on(ea, back_a), y_on(eb, back_b));
                let yp = 0.5 * (yba + ybb);
                alloc::vec![
                    pc.at(yc + y),
                    back_a.at(yba + y),
                    prop.at(yp + y),
                    back_b.at(ybb + y),
                ]
            }
            Corner::Straight {
                ta,
                tb,
                back_a,
                back_b,
            } => alloc::vec![
                ta.at(y_on(ea, ta) + y),
                back_a.at(y_on(ea, back_a) + y),
                back_b.at(y_on(eb, back_b) + y),
                tb.at(y_on(eb, tb) + y),
            ],
        }
    }
}

impl Junction {
    /// The road end in `slot`: one of `ends`, then the merges.
    pub fn end(&self, slot: usize) -> &RoadEnd {
        if slot < self.ends.len() {
            &self.ends[slot]
        } else {
            &self.merges[slot - self.ends.len()].end
        }
    }
}

/// Distance from `p` to the polyline `line`, and the arc length at the nearest point.
fn polyline_nearest(line: &[V2], p: V2) -> (f32, f32) {
    let (mut best, mut arc, mut acc) = (f32::INFINITY, 0.0, 0.0);
    for w in line.windows(2) {
        let len = (w[1] - w[0]).length();
        let (d, t) = segment_distance(p, w[0], w[1]);
        if d < best {
            best = d;
            arc = acc + t * len;
        }
        acc += len;
    }
    (best, arc)
}

/// `line` (a profile polyline) as seen from one of its ends: reversed when `from_b`.
fn from_end(line: &[V2], from_b: bool) -> Vec<V2> {
    let mut v = line.to_vec();
    if from_b {
        v.reverse();
    }
    v
}

/// Point and unit direction at arc `s` along `line` (held at its end).
fn along(line: &[V2], s: f32) -> (V2, V2) {
    let last = line.len() - 2;
    let mut acc = 0.0;
    for (k, w) in line.windows(2).enumerate() {
        let len = (w[1] - w[0]).length();
        if s <= acc + len || k == last {
            let d = (w[1] - w[0]).normalize();
            return (w[0] + d * (s - acc).min(len), d);
        }
        acc += len;
    }
    (line[0], V2::new(1.0, 0.0))
}

/// Takes the ends that join a wider road at a shallow angle out of `ends` (sorted, which
/// stays sorted) as merges, and opens the gaps they need in the wider roads' curbs.
/// `lines[e]` is edge `e`'s profile polyline and `node_at_b[k]` whether `ends[k]` is its
/// edge's `b` end.
fn merges(ends: &mut Vec<RoadEnd>, lines: &[Vec<V2>], node_at_b: &[bool]) -> Vec<Merge> {
    let count = ends.len();
    // For each end, the wider road it would merge into (the most nearly parallel).
    let mut into: Vec<Option<usize>> = alloc::vec![None; count];
    for i in 0..count {
        let b = &ends[i];
        let mut best: Option<(f32, usize)> = None;
        for (k, a) in ends.iter().enumerate() {
            let wider = a.hw > b.hw || (a.hw == b.hw && a.edge < b.edge);
            let cos = a.dir.dot(b.dir);
            if k != i && wider && cos > MERGE_COS && best.is_none_or(|q| cos > q.0) {
                best = Some((cos, k));
            }
        }
        into[i] = best.map(|q| q.1);
    }
    // A road that merges cannot be merged into.
    for i in 0..count {
        if let Some(k) = into[i] {
            if into[k].is_some() {
                into[i] = None;
            }
        }
    }
    let mut out: Vec<(usize, usize, RoadEnd, f32, f32, f32)> = Vec::new();
    for i in 0..count {
        let Some(k) = into[i] else { continue };
        let (a, mut b) = (ends[k], ends[i]);
        let side = if a.dir.turn(b.dir) > 0.0 { 1.0 } else { -1.0 };
        let major = from_end(&lines[a.edge], node_at_b[k]);
        let minor = from_end(&lines[b.edge], node_at_b[i]);
        let len: f32 = minor.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        // Walk out along the merging road until its outer edge, then its inner curb, are
        // off the major's pavement.
        let off = |s: f32, t: f32| {
            let (p, d) = along(&minor, s);
            polyline_nearest(&major, p + d.right() * (side * t)).0 >= a.hw
        };
        let (mut s_outer, mut s_gore) = (None, None);
        let mut s = 0.0;
        while s < len.min(MERGE_MAX_M) && s_gore.is_none() {
            if s_outer.is_none() && off(s, -b.hw) {
                s_outer = Some(s);
            }
            if off(s, b.hw) {
                s_gore = Some(s);
            }
            s += 1.0;
        }
        let (Some(so), Some(sg)) = (s_outer, s_gore) else {
            continue;
        };
        // The gap in the major's curb: between where the two edges leave its pavement.
        let at = |s: f32, t: f32| {
            let (p, d) = along(&minor, s);
            polyline_nearest(&major, p + d.right() * (side * t)).1
        };
        let (ga0, ga1) = (at(so, -b.hw), at(sg, b.hw));
        if sg <= 0.0 || ga1 <= ga0 {
            continue;
        }
        b.trim = sg;
        if side > 0.0 {
            (b.s_right, b.s_left) = (sg, so);
        } else {
            (b.s_left, b.s_right) = (sg, so);
        }
        out.push((i, k, b, so, sg, side));
        let gap = if side > 0.0 {
            &mut ends[k].gap_left
        } else {
            &mut ends[k].gap_right
        };
        *gap = if gap[1] > gap[0] {
            [gap[0].min(ga0), gap[1].max(ga1)]
        } else {
            [ga0, ga1]
        };
    }
    if out.is_empty() {
        return Vec::new();
    }
    // Remove the merging ends; renumber the majors' slots.
    let gone: Vec<usize> = out.iter().map(|m| m.0).collect();
    let slot = |k: usize| k - gone.iter().filter(|&&g| g < k).count();
    let merges = out
        .iter()
        .map(|&(_, k, end, s_outer, s_gore, side)| Merge {
            end,
            major: slot(k),
            s_outer,
            s_gore,
            side,
        })
        .collect();
    let mut i = 0;
    ends.retain(|_| {
        i += 1;
        !gone.contains(&(i - 1))
    });
    merges
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
    shoulder: Paint,
    gutter: Paint,
    curb: Paint,
    curb_top: Paint,
    walk: Paint,
    grout: Paint,
    grass: Paint,
    white: Paint,
    yellow: Paint,
    deck: Paint,
}

impl RoadPaints {
    fn new(p: &Palette) -> RoadPaints {
        RoadPaints {
            asphalt: Paint::new(p.asphalt, Pattern::Asphalt),
            asphalt_worn: p.asphalt_worn,
            shoulder: Paint::new(mix(p.asphalt, p.asphalt_worn, 150), Pattern::Asphalt),
            gutter: Paint::new(mix(p.concrete_dark, p.concrete, 60), Pattern::Concrete),
            curb: Paint::new(shade(p.concrete, 230), Pattern::Concrete),
            curb_top: Paint::new(mix(p.concrete, p.paint_white, 110), Pattern::Concrete),
            walk: Paint::new(p.concrete, Pattern::Concrete),
            grout: Paint::new(shade(p.concrete_dark, 150), Pattern::Concrete),
            grass: Paint::new(p.lawn, Pattern::Lawn),
            white: Paint::new(p.paint_white, Pattern::Paint),
            yellow: Paint::new(p.paint_yellow, Pattern::Paint),
            deck: Paint::new(shade(p.concrete, 200), Pattern::Concrete),
        }
    }
}

/// How deep the backs of sidewalks and curbs reach below their road: just under the town's
/// flat apron, or a deck's thickness on terrain (cut and fill leave the ground near, not
/// at, the road's height).
fn skirt_depth(terrain: Option<&TerrainGrid>) -> f32 {
    if terrain.is_some() {
        -DECK_THICKNESS_M
    } else {
        TERRAIN_Y
    }
}

/// Builds every road and junction mesh into `out`. `terrain` is the cut and filled ground
/// of a terrain place (decks and piers go where a road rides above it), `None` for the
/// flat test town.
pub fn build(
    place: &Place,
    net: &Network,
    side_zone: &[[Option<ZoneId>; 2]],
    pal: &Palette,
    seed: u64,
    terrain: Option<&TerrainGrid>,
    out: &mut MeshSet,
) {
    build_from(
        place,
        net,
        side_zone,
        pal,
        seed,
        terrain,
        out,
        0,
        usize::MAX,
    );
}

/// [`build`] in pieces: the edges, then the junctions, from item `from` on (edges first,
/// `0..edges`, then junctions), stopping after the item that brings what `out` keeps to
/// `max_vertices` or more. Returns the item to resume from, `None` when every item is
/// built. Resumed until done, `out` gets the same writes in the same order as one
/// [`build`].
#[allow(clippy::too_many_arguments)]
pub fn build_from(
    place: &Place,
    net: &Network,
    side_zone: &[[Option<ZoneId>; 2]],
    pal: &Palette,
    seed: u64,
    terrain: Option<&TerrainGrid>,
    out: &mut MeshSet,
    from: usize,
    max_vertices: usize,
) -> Option<usize> {
    let paints = RoadPaints::new(pal);
    let skirt = skirt_depth(terrain);
    let edges = place.roads.edges.len();
    for (i, e) in place.roads.edges.iter().enumerate().skip(from) {
        let path = &net.paths[i];
        let (ea, eb) = net.ends(i);
        let len = path.len();
        let mid = if path.pts.len() == 2 {
            (path.pts[0] + path.pts[1]) * 0.5
        } else {
            path.xz(len * 0.5, 0.0)
        };
        // An edge's meshes are filed under its midpoint's chunk.
        if !out.wants(mid) {
            continue;
        }
        // A merging end has no crosswalk: it joins the other road's lanes.
        let deg = [false, true].map(|b_end| {
            if net.merges_in(i, b_end) {
                0
            } else {
                net.degree(net.edge_ends[i][usize::from(b_end)].0)
            }
        });
        {
            let m = out.at(mid, MeshKind::Road);
            ribbon(m, path, ea.trim, len - eb.trim, ea.hw, &paints, seed);
            // Left side runs from a's left curb to b's right curb, and vice versa.
            for (k, spans) in net.side_spans(i).into_iter().enumerate() {
                let sigma = if k == 0 { 1.0 } else { -1.0 };
                let parkway = side_has_parkway(e.class, side_zone[i][k]);
                for [s0, s1] in spans {
                    let side = Side {
                        sigma,
                        s0,
                        s1,
                        hw: ea.hw,
                        sw: ea.sw,
                        parkway,
                        skirt,
                    };
                    side_strip(m, path, &side, &paints, seed ^ i as u64);
                }
            }
            if e.class == RoadClass::Highway {
                median(m, path, ea.trim, len - eb.trim, &paints);
            }
            if let Some(g) = terrain {
                deck(m, net, i, ea.trim, len - eb.trim, ea.hw, ea.sw, g, &paints);
            }
        }
        let m = out.at(mid, MeshKind::Marking);
        markings(
            m,
            path,
            ea,
            eb,
            len,
            deg,
            (e.class, e.lanes(), e.oneway),
            &paints,
            seed ^ i as u64,
        );
        if out.kept_vertices() >= max_vertices {
            return Some(i + 1);
        }
    }
    for (k, j) in net
        .junctions
        .iter()
        .enumerate()
        .skip(from.saturating_sub(edges))
    {
        if out.wants(j.pos) {
            junction(out.at(j.pos, MeshKind::Road), net, j, &paints, skirt);
            if out.kept_vertices() >= max_vertices {
                return Some(edges + k + 1);
            }
        }
    }
    None
}

/// Flat (level across) quad covering `s0..s1` along and `t0..t1` across, `lift` above the
/// road surface, cut at the centerline's bends.
#[allow(clippy::too_many_arguments)]
fn flat(
    m: &mut WorldMesh,
    path: &EdgePath,
    s0: f32,
    s1: f32,
    t0: f32,
    t1: f32,
    lift: f32,
    paint: Paint,
) {
    if s1 - s0 <= 1e-4 || abs_diff(t1, t0) <= 1e-4 {
        return;
    }
    path.pieces(s0, s1, |a, b| {
        let q = [
            path.p(a, t0, lift),
            path.p(b, t0, lift),
            path.p(b, t1, lift),
            path.p(a, t1, lift),
        ];
        quad(m, q, V3::Y, paint);
    });
}

/// Vertical quads at lateral `t` from `y0` to `y1` above the surface, facing toward
/// `+left * facing`.
#[allow(clippy::too_many_arguments)]
fn wall(
    m: &mut WorldMesh,
    path: &EdgePath,
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
    path.pieces(s0, s1, |a, b| {
        let q = [
            path.p(a, t, y0),
            path.p(b, t, y0),
            path.p(b, t, y1),
            path.p(a, t, y1),
        ];
        quad(m, q, (path.left(a) * facing).at(0.0), paint);
    });
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
fn ribbon(
    m: &mut WorldMesh,
    path: &EdgePath,
    s0: f32,
    s1: f32,
    hw: f32,
    p: &RoadPaints,
    seed: u64,
) {
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
        path.pieces(sa, sb, |sa, sb| {
            for w in lanes.windows(2) {
                let q = [
                    path.p(sa, w[0], 0.0),
                    path.p(sb, w[0], 0.0),
                    path.p(sb, w[1], 0.0),
                    path.p(sa, w[1], 0.0),
                ];
                quad_vc(m, q, V3::Y, p.asphalt, q.map(tint));
            }
        });
    }
}

/// One side of a road for [`side_strip`].
struct Side {
    /// 1 for the left side, -1 for the right.
    sigma: f32,
    s0: f32,
    s1: f32,
    hw: f32,
    sw: f32,
    parkway: bool,
    /// How far below the road the outer face reaches.
    skirt: f32,
}

/// Gutter, curb, curb top, parkway, slabbed walk and back skirt along one side.
fn side_strip(m: &mut WorldMesh, path: &EdgePath, side: &Side, p: &RoadPaints, seed: u64) {
    let Side {
        sigma,
        s0,
        s1,
        hw,
        sw,
        parkway,
        skirt,
    } = *side;
    if s1 <= s0 {
        return;
    }
    let t = |off: f32| sigma * off;
    // Gutter pan, a hair above the asphalt.
    flat(
        m,
        path,
        s0,
        s1,
        t(hw - GUTTER_M),
        t(hw),
        PAINT_LIFT * 0.8,
        p.gutter,
    );
    // Curb face looks back toward the road center.
    wall(m, path, s0, s1, t(hw), 0.0, CURB_M, -sigma, p.curb);
    if sw <= 0.0 {
        flat(
            m,
            path,
            s0,
            s1,
            t(hw),
            t(hw + CURB_TOP_M),
            CURB_M,
            p.curb_top,
        );
        wall(
            m,
            path,
            s0,
            s1,
            t(hw + CURB_TOP_M),
            skirt,
            CURB_M,
            sigma,
            p.curb,
        );
        return;
    }
    flat(
        m,
        path,
        s0,
        s1,
        t(hw),
        t(hw + CURB_TOP_M),
        CURB_M,
        p.curb_top,
    );
    let mut walk0 = hw + CURB_TOP_M;
    if parkway {
        let g = walk0 + PARKWAY_M;
        flat(m, path, s0, s1, t(walk0), t(g), CURB_M - 0.01, p.grass);
        walk0 = g;
    }
    let walk1 = hw + sw;
    // Grout below, slabs on top: the seams are the gaps between slabs.
    flat(m, path, s0, s1, t(walk0), t(walk1), CURB_M - 0.012, p.grout);
    let n = ((s1 - s0) / SLAB_M) as u32;
    let n = n.max(1);
    let step = (s1 - s0) / n as f32;
    for k in 0..n {
        let a = s0 + step * k as f32;
        let h = skywave_core::mix64(seed ^ KEY_SLABS ^ u64::from(k) << 8);
        let tone = 244 + (h & 15) as u32;
        let paint = p.walk.with_rgb(shade(p.walk.rgb(), tone));
        flat(
            m,
            path,
            a + SEAM_M * 0.5,
            a + step - SEAM_M * 0.5,
            t(walk0),
            t(walk1),
            CURB_M,
            paint,
        );
    }
    wall(m, path, s0, s1, t(walk1), skirt, CURB_M, sigma, p.curb);
}

/// The highway's median: a concrete barrier on the centerline (sloped faces, flat top).
fn median(m: &mut WorldMesh, path: &EdgePath, s0: f32, s1: f32, p: &RoadPaints) {
    if s1 - s0 < 1.0 {
        return;
    }
    let (base, top) = (0.3, 0.1);
    path.pieces(s0, s1, |a, b| {
        for sigma in [1.0f32, -1.0] {
            let q = [
                path.p(a, sigma * base, 0.0),
                path.p(b, sigma * base, 0.0),
                path.p(b, sigma * top, BARRIER_M),
                path.p(a, sigma * top, BARRIER_M),
            ];
            quad(m, q, (path.left(a) * sigma).at(0.3), p.curb);
        }
    });
    flat(m, path, s0, s1, -top, top, BARRIER_M, p.curb_top);
}

/// Where the road rides more than [`DECK_CLEARANCE_M`] above `ground`: a slab under it,
/// parapets along its edges and square piers every [`PIER_SPACING_M`] (none standing on
/// another road's asphalt).
#[allow(clippy::too_many_arguments)]
fn deck(
    m: &mut WorldMesh,
    net: &Network,
    edge: usize,
    s0: f32,
    s1: f32,
    hw: f32,
    sw: f32,
    ground: &TerrainGrid,
    p: &RoadPaints,
) {
    let path = &net.paths[edge];
    if s1 - s0 < 1.0 {
        return;
    }
    let edge_t = hw + if sw > 0.0 { sw } else { CURB_TOP_M };
    let elevated = |s: f32| {
        let c = path.xz(s, 0.0);
        path.y_at(c) - ground.height_at(c.x, c.y) > DECK_CLEARANCE_M
    };
    // Elevated spans, sampled every 2 m.
    let steps = ((s1 - s0) / 2.0) as u32 + 1;
    let mut spans: Vec<(f32, f32)> = Vec::new();
    let mut open: Option<f32> = None;
    for k in 0..=steps {
        let s = lerp(s0, s1, k as f32 / steps as f32);
        match (elevated(s), open) {
            (true, None) => open = Some(s),
            (false, Some(a)) => {
                spans.push((a, s));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(a) = open {
        spans.push((a, s1));
    }
    let lift = CURB_M;
    for (a, b) in spans {
        // Soffit, facing down.
        path.pieces(a, b, |u, v| {
            let q = [
                path.p(u, -edge_t, -DECK_THICKNESS_M),
                path.p(v, -edge_t, -DECK_THICKNESS_M),
                path.p(v, edge_t, -DECK_THICKNESS_M),
                path.p(u, edge_t, -DECK_THICKNESS_M),
            ];
            quad(m, q, V3::new(0.0, -1.0, 0.0), p.deck);
        });
        for sigma in [1.0f32, -1.0] {
            let (t_out, t_in) = (sigma * edge_t, sigma * (edge_t - PARAPET_THICK_M));
            let top = lift + PARAPET_M;
            wall(m, path, a, b, t_out, lift, top, sigma, p.deck);
            wall(m, path, a, b, t_in, lift, top, -sigma, p.deck);
            flat(m, path, a, b, t_in, t_out, top, p.curb_top);
        }
        // Piers.
        let count = ((b - a) / PIER_SPACING_M) as u32;
        for k in 0..count {
            let s = a + PIER_SPACING_M * (k as f32 + 0.5);
            let c = path.xz(s, 0.0);
            if on_other_road(net, edge, c, PIER_M) {
                continue;
            }
            let top = path.y_at(c) - DECK_THICKNESS_M;
            let foot = ground.height_at(c.x, c.y) - 0.5;
            if top - foot < 0.5 {
                continue;
            }
            let d = path.d[path.seg(s)];
            let f = crate::mesh::Frame::upright(c.at(foot), d);
            let h = PIER_M * 0.5;
            crate::mesh::boxf(
                m,
                &f,
                [-h, 0.0, -h],
                [h, top - foot, h],
                crate::mesh::face::ALL & !crate::mesh::face::NEG_Y & !crate::mesh::face::POS_Y,
                p.deck,
            );
        }
    }
}

/// Whether `p` lies within `margin` of another edge's asphalt.
fn on_other_road(net: &Network, edge: usize, p: V2, margin: f32) -> bool {
    net.paths.iter().enumerate().any(|(i, path)| {
        if i == edge {
            return false;
        }
        let (ea, _) = net.ends(i);
        path.raw
            .windows(2)
            .any(|w| segment_distance(p, w[0], w[1]).0 < ea.hw + margin)
    })
}

/// Crosswalks, stop bars and lane lines on one edge. `kind` is the edge's class, total
/// lanes and whether it is one-way.
#[allow(clippy::too_many_arguments)]
fn markings(
    m: &mut WorldMesh,
    path: &EdgePath,
    ea: &RoadEnd,
    eb: &RoadEnd,
    len: f32,
    deg: [usize; 2],
    kind: (RoadClass, u8, bool),
    p: &RoadPaints,
    seed: u64,
) {
    let (class, lanes, oneway) = kind;
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
    // Crosswalk and stop bar at each signal/stop-controlled end (never on the highway).
    for (end, junction_deg) in [(0, deg[0]), (1, deg[1])] {
        if junction_deg < 3 || class == RoadClass::Highway {
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
            flat(
                m,
                path,
                c0,
                c1,
                t,
                t + 0.6,
                y,
                worn(p.white, k as u32 + 100 * end),
            );
        }
        flat(m, path, stop0, stop1, t0, t1, y, p.white);
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
            flat(
                m,
                path,
                a,
                a + dash,
                t - w * 0.5,
                t + w * 0.5,
                y,
                worn(paint, k as u32),
            );
        }
    };
    if oneway {
        // Ramps: edge lines only.
        for sign in [1.0, -1.0] {
            let t = sign * (inner - 0.2);
            flat(m, path, m0, m1, t - 0.06, t + 0.06, y, p.white);
        }
        return;
    }
    match class {
        RoadClass::Local
        | RoadClass::Collector
        | RoadClass::Alley
        | RoadClass::Beachfront
        | RoadClass::Rural => {
            dashed(m, 0.0, 0.12, 3.0, 6.0, p.yellow);
        }
        RoadClass::Highway => {
            // Median edge lines, lanes from the median out, edge lines, paved shoulders.
            let per_side = u32::from(lanes / 2).max(1);
            let edge = MEDIAN_HALF_M + HIGHWAY_LANE_M * per_side as f32;
            for sign in [1.0, -1.0] {
                let t = sign * (MEDIAN_HALF_M - 0.2);
                flat(m, path, m0, m1, t - 0.06, t + 0.06, y, p.yellow);
                for k in 1..per_side {
                    let t = sign * (MEDIAN_HALF_M + HIGHWAY_LANE_M * k as f32);
                    dashed(m, t, 0.12, 3.0, 9.0, p.white);
                }
                let t = sign * edge;
                flat(m, path, m0, m1, t - 0.08, t + 0.08, y, p.white);
                let (a, b) = (sign * (edge + 0.08), sign * (inner - 0.02));
                flat(m, path, m0, m1, a, b, y * 0.5, p.shoulder);
            }
        }
        RoadClass::Arterial => {
            for t in [0.14, -0.14] {
                flat(m, path, m0, m1, t - 0.06, t + 0.06, y, p.yellow);
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
                flat(m, path, m0, m1, t - 0.06, t + 0.06, y, p.white);
            }
        }
    }
}

/// Junction asphalt (a fan around the node) and the corner curbs and sidewalks.
fn junction(m: &mut WorldMesh, net: &Network, j: &Junction, p: &RoadPaints, skirt_y: f32) {
    // Merge wedges: the merging road's pavement outside the road it joins.
    for mg in &j.merges {
        let w = net.merge_wedge(j, mg);
        for k in 0..w.len().saturating_sub(1) {
            let q = [w[k].0, w[k + 1].0, w[k + 1].1, w[k].1];
            quad(m, q, V3::Y, p.asphalt);
        }
    }
    let n = j.ends.len();
    if n < 2 {
        return;
    }
    let c = j.pos.at(j.y);
    let ring = net.junction_ring(j);
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        tri(m, c, a, b, V3::Y, p.asphalt);
    }
    let y_on = |e: &RoadEnd, q: V2| net.paths[e.edge].y_at(q);
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
                let (ya, yb) = (y_on(ea, ta), y_on(eb, tb));
                let last = (pts.len() - 1) as f32;
                let ys: Vec<f32> = (0..pts.len())
                    .map(|k| lerp(ya, yb, k as f32 / last))
                    .collect();
                for k in 0..pts.len() - 1 {
                    let (a, b) = (pts[k], pts[k + 1]);
                    let (y0, y1) = (ys[k], ys[k + 1]);
                    let away = ((a + b) * 0.5 - q).at(0.0);
                    // Curb face toward the road, gutter band on the asphalt.
                    quad(
                        m,
                        [a.at(y0), b.at(y1), b.at(y1 + CURB_M), a.at(y0 + CURB_M)],
                        away,
                        p.curb,
                    );
                    let ga = a + (a - q).normalize() * GUTTER_M;
                    let gb = b + (b - q).normalize() * GUTTER_M;
                    let y = PAINT_LIFT * 0.8;
                    quad(
                        m,
                        [a.at(y0 + y), b.at(y1 + y), gb.at(y1 + y), ga.at(y0 + y)],
                        V3::Y,
                        p.gutter,
                    );
                }
                // Corner sidewalk: fan from the property corner over the curb arc.
                let fan = net.corner_walk(j, i);
                for w in fan[1..].windows(2) {
                    tri(m, fan[0], w[0], w[1], V3::Y, p.walk);
                }
                let (yba, ybb) = (y_on(ea, back_a), y_on(eb, back_b));
                let yp = 0.5 * (yba + ybb);
                skirt(m, (back_a, yba), (prop, yp), ea.dir.left(), p, skirt_y);
                skirt(m, (prop, yp), (back_b, ybb), eb.dir.right(), p, skirt_y);
            }
            Corner::Reflex {
                back_a,
                back_b,
                prop,
                ..
            } => {
                let w = net.corner_walk(j, i);
                quad(m, [w[0], w[1], w[2], w[3]], V3::Y, p.walk);
                let (yba, ybb) = (y_on(ea, back_a), y_on(eb, back_b));
                let yp = 0.5 * (yba + ybb);
                skirt(m, (back_a, yba), (prop, yp), ea.dir.left(), p, skirt_y);
                skirt(m, (prop, yp), (back_b, ybb), eb.dir.right(), p, skirt_y);
            }
            Corner::Straight { .. } => {
                let w = net.corner_walk(j, i);
                quad(m, [w[0], w[1], w[2], w[3]], V3::Y, p.walk);
            }
        }
    }
}

/// Back face of a sidewalk edge from `skirt_y` below the road to the curb top, facing
/// `out`; the ends carry their road heights.
fn skirt(m: &mut WorldMesh, a: (V2, f32), b: (V2, f32), out: V2, p: &RoadPaints, skirt_y: f32) {
    let ((a, ya), (b, yb)) = (a, b);
    quad(
        m,
        [
            a.at(ya + skirt_y),
            b.at(yb + skirt_y),
            b.at(yb + CURB_M),
            a.at(ya + CURB_M),
        ],
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
            assert_eq!(j.y, 0.0);
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
        // Every edge's two ends point at each other, and its path is the straight line.
        for e in 0..place.roads.edges.len() {
            let (a, b) = net.ends(e);
            assert_eq!(a.edge, e);
            assert_eq!(b.edge, e);
            assert!((a.dir + b.dir).length() < 1e-5);
            let path = &net.paths[e];
            assert_eq!(path.pts.len(), 2);
            assert_eq!(path.flat, Some(0.0));
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
                profile: alloc::vec![[0.0, 0.0, 0.0], [x, 0.0, z]],
                ..template.clone()
            });
        }
        let net = Network::new(&place);
        let j = &net.junctions[0];
        assert_eq!(j.ends.len(), 5);
        assert!(j.corners.iter().all(|c| matches!(c, Corner::Fillet { .. })));
        let mut m = WorldMesh::default();
        junction(
            &mut m,
            &net,
            j,
            &RoadPaints::new(&Palette::socal_1985()),
            TERRAIN_Y,
        );
        assert!(m.triangle_count() > 40);
        assert!(m
            .vertices
            .iter()
            .all(|v| v.normal.iter().all(|c| c.is_finite())));
    }

    /// A curved, climbing edge: the layout runs straight out to the trims, bends with
    /// miters, and every surface point sits on the profile.
    #[test]
    fn curved_sloped_edge_follows_its_profile() {
        let raw: Vec<V2> = (0..8)
            .map(|k| {
                let a = k as f32 * 0.15;
                let (s, c) = crate::math::sin_cos(a);
                V2::new(100.0 * s, 100.0 - 100.0 * c)
            })
            .collect();
        let ys: Vec<f32> = (0..8).map(|k| k as f32 * 1.5).collect();
        let path = layout(raw.clone(), ys.clone(), 12.0, 9.0);
        assert_eq!(path.flat, None);
        assert_eq!(path.n[0], path.n[1]);
        let (da, _) = end_dirs(&raw);
        assert!((path.xz(12.0, 0.0) - (raw[0] + da * 12.0)).length() < 1e-4);
        for k in 0..60 {
            let s = path.len() * k as f32 / 59.0;
            for t in [-7.0, 0.0, 7.0] {
                let p = path.p(s, t, 0.0);
                let expect = profile_height(&raw, &ys, V2::new(p.x, p.z));
                assert!((p.y - expect).abs() < 1e-5);
            }
            // Miter offsets keep the width within a few percent.
            let w = (path.xz(s, 7.0) - path.xz(s, -7.0)).length();
            assert!((w - 14.0).abs() < 0.5, "{w}");
        }
    }
}
