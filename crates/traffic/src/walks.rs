//! The sidewalk graph: where pedestrians may walk, cross and stop.
//!
//! Built once from a place (like the lane graph, derived: `Place::sidewalks` is empty from
//! every producer today), on the geometry `world::roads` draws. Every road with sidewalks
//! gets a walk line along each side, [`WALK_LINE_M`] out from the curb, on the slab;
//! the walks of two roads meeting at a junction join around the corner between them: along
//! the curb return (an arc about the fillet's center, inside the sidewalk) where the curbs
//! turn in, through the point where the walk lines meet on the outside of a bend, straight
//! on where the roads continue each other. The walk lines start where each road's curb
//! starts at the junction (`world`'s corner layout, mirrored here: fillet radii, the
//! straight threshold, the straight run of a road end out to its trim). Walks are cut into
//! links of at most [`MAX_LINK_M`].
//!
//! **Crossings** go where `world` paints a crosswalk: at each end of a road with sidewalks
//! that meets two or more others (not a highway, not a shallow merge, with at least 12 m of
//! road between its trims), 2.3 m past its trim. A crossing is a chain of links from the
//! walk line out to the curb (a spur), down onto the road, across it and back up; the
//! pedestrians wait at the curb vertex. On a signalized junction a crossing shows *walk*
//! while the road's own approach has red long enough to cross ([`WalkGraph::walk`]),
//! otherwise pedestrians look for a gap.
//!
//! **Spots** are where pedestrians go: shop windows and doors (walls of commercial, office,
//! hotel and civic buildings facing a walk), parking structures, benches the place stands
//! by a walk (sit; on collectors and arterials also a stop to wait at), the beach behind a
//! walk, and the place's landmarks (pier, diner, overlook...) at their nearest walk.
//!
//! Plain arrays: nodes with their links (`adj`), links, crossings, spots, and a coarse grid
//! of links and spots for spawning and target picking. Building allocates; queries never do.

use alloc::vec::Vec;

use place::{BuildingUse, Place, PropKind, RoadClass, ZoneId, SIDEWALK_WIDTH_M};
use skywave_core::hash64;

use crate::lanes::{lane_width, raw_line, Control, LaneGraph, NONE};
use crate::math::{dir, dist2, dot, len2, right, sin_cos, unit2};
use crate::signal::Light;

/// Walk line: this far out from the curb face, on the slab past any parkway grass.
pub const WALK_LINE_M: f32 = 1.7;
/// Walks are cut into links no longer than this.
pub const MAX_LINK_M: f32 = 16.0;
/// Height of the curb (and the sidewalk) above the road surface (`world::CURB_M`).
pub const CURB_M: f32 = 0.15;
/// Curb top strip (`world::roads`).
const CURB_TOP_M: f32 = 0.2;
/// Lateral room either side of a walk line: toward the curb (keeping off the curb top's
/// edge) and toward the back of the walk (the property line).
const ROOM_CURB_M: f32 = WALK_LINE_M - CURB_TOP_M - 0.05;
const ROOM_BACK_M: f32 = SIDEWALK_WIDTH_M - WALK_LINE_M - 0.05;
/// Half the crosswalk's painted length along the road, less a margin.
const ROOM_CROSSING_M: f32 = 1.2;
/// sin(5 degrees): closer to straight than this, curbs continue each other (`world`).
const STRAIGHT_SIN: f32 = 0.087;
/// cos(5 degrees): two road ends this close to opposite continue each other.
const STRAIGHT_COS: f32 = 0.996;
/// Ends meeting closer than this (cosine; about 30 degrees) merge (`world`): no corners and
/// no crossings at such a node; the walks there stop this far back.
const MERGE_COS: f32 = 0.866;
const MERGE_CUT_M: f32 = 20.0;
/// Crosswalk center past the trim, and the least road between trims that gets one
/// (`world::roads` markings).
const CROSSWALK_M: f32 = 2.3;
const MIN_MARKED_M: f32 = 12.0;
/// Crossing chain: the curb vertex (where people wait) at least just behind the 0.2 m curb
/// top and `WAIT_CLEAR_M` outside every lane; the road vertex just past
/// the gutter edge.
const CURB_STEP_M: f32 = 0.35;
const WAIT_CLEAR_M: f32 = 0.8;
/// Walk lines stay this far outside every lane (normally they are over 2 m out).
const WALK_LANE_CLEAR_M: f32 = 1.0;
const ROAD_STEP_M: f32 = 0.35;
/// Grid cell for spawning and spot lookup.
pub const CELL_M: f32 = 32.0;
/// A shop wall counts for a walk within this distance of its middle.
const WALL_REACH_M: f32 = 4.5;
/// A bench counts for a walk within this distance.
const BENCH_REACH_M: f32 = 1.6;
/// A landmark counts for a walk within this distance.
const LANDMARK_REACH_M: f32 = 60.0;
/// Beach spots along a walk backed by beach, about this far apart.
const BEACH_STEP_M: f32 = 40.0;
/// Hash key of the per-link spot jitter.
const KEY_WALKS: u64 = 0x5741_4c4b_5300_0001;

/// What a link is.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// Along a sidewalk, or around a corner.
    Walk = 0,
    /// From a walk line out to the curb where a crossing starts.
    Spur = 1,
    /// Off the curb, across the road, back up: on the asphalt.
    Crossing = 2,
}

/// A straight piece of walkable path from node `a` to node `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub a: u32,
    pub b: u32,
    pub length_m: f32,
    /// Unit ground direction from `a` to `b`.
    pub dir: [f32; 2],
    pub kind: LinkKind,
    /// The road edge the link runs along or across, [`NONE`] around corners.
    pub edge: u32,
    /// Index into [`WalkGraph::crossings`] for spurs and crossing links, else [`NONE`].
    pub crossing: u32,
    /// How far a pedestrian may stand off the line, to its right and to its left (seen
    /// from `a` toward `b`).
    pub room: [f32; 2],
    /// Which side the road is on, seen from `a` toward `b`: 1 left, -1 right, 0 neither
    /// (corners, crossings).
    pub road_side: i8,
    /// Pedestrians per 100 m at normal density, from the zone it is in.
    pub density: f32,
}

/// A point of the sidewalk graph: `adj[first_link..first_link + link_count]` are its links.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkNode {
    /// On the walk surface: the sidewalk is [`CURB_M`] above its road.
    pub pos: [f32; 3],
    pub first_link: u32,
    pub link_count: u32,
}

/// A pedestrian crossing over road `edge` at junction `node`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    pub node: u32,
    pub edge: u32,
    /// A lane arriving at `node` on `edge`: the light it faces gates the crossing.
    /// [`NONE`] when no lane arrives there (a one-way road leaving the junction).
    pub lane: u32,
    /// The junction is signalized and `lane` is one of its approaches.
    pub signal: bool,
    /// The two curb vertices where pedestrians wait.
    pub curbs: [u32; 2],
    /// Curb to curb, m.
    pub length_m: f32,
}

/// What a [`Spot`] is.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotKind {
    /// A shop window: pause and look in.
    Window = 0,
    /// The door of an office, hotel or civic building: stand by it a while.
    Door = 1,
    /// A bench: sit (two seats).
    Bench = 2,
    /// Waiting by a bench on a busy road, as at a bus stop (the place has no bus stops
    /// of its own yet).
    Stop = 3,
    /// The beach behind a walk: stand and look out.
    Beach = 4,
    /// A parking structure's entrance.
    Parking = 5,
    /// A landmark of the place (pier, diner, overlook...): stand in a group.
    Landmark = 6,
}

/// A place to go: `link` at `s` along it, `lat` to its left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub kind: SpotKind,
    pub link: u32,
    pub s: f32,
    pub lat: f32,
    /// Where to stand, or for a bench its seat point (the middle of the seat, at the
    /// sitter's pelvis, on the ground).
    pub pos: [f32; 3],
    /// Ground direction to face: into the window, the way a sitter looks, out to sea.
    pub facing: [f32; 2],
}

/// The sidewalk graph of a place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WalkGraph {
    pub nodes: Vec<WalkNode>,
    /// Link indices, grouped by node.
    pub adj: Vec<u32>,
    pub links: Vec<Link>,
    pub crossings: Vec<Crossing>,
    pub spots: Vec<Spot>,
    /// Grid origin (`x, z`) and size in cells of [`CELL_M`].
    pub grid_origin: [f32; 2],
    pub grid_cells: [u32; 2],
    /// Per cell, `cell_links[cell_link_start[c]..cell_link_start[c + 1]]` are the links
    /// whose midpoint lies in it; the same for spots.
    pub cell_link_start: Vec<u32>,
    pub cell_links: Vec<u32>,
    pub cell_spot_start: Vec<u32>,
    pub cell_spots: Vec<u32>,
    /// The lanes' painted bands, for [`WalkGraph::in_lane`].
    lane_cover: Cover,
}

/// A road end at a node, as `world::roads` sees it.
#[derive(Clone, Copy)]
struct End {
    edge: u32,
    at_b: bool,
    dir: [f32; 2],
    hw: f32,
    walk: bool,
    class: RoadClass,
}

impl End {
    /// The edge side (0 left of `a`→`b`, 1 right) on this end's left, and on its right.
    fn left_side(&self) -> usize {
        usize::from(self.at_b)
    }
    fn right_side(&self) -> usize {
        usize::from(!self.at_b)
    }
}

/// How a corner joins the walk ending on one road to the walk starting on the next.
#[derive(Clone, Copy)]
enum Turn {
    /// Around `q` at radius `r`.
    Arc { q: [f32; 2], r: f32 },
    /// Through the point where the two walk lines meet.
    Point([f32; 2]),
    /// Straight across.
    Direct,
}

fn left(d: [f32; 2]) -> [f32; 2] {
    [d[1], -d[0]]
}

fn add2(p: [f32; 2], d: [f32; 2], k: f32) -> [f32; 2] {
    [p[0] + d[0] * k, p[1] + d[1] * k]
}

fn sub2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn xz(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[2]]
}

/// Intersection of the lines `p + s d` and `q + t e` (`world::math::intersect_lines`).
fn meet(p: [f32; 2], d: [f32; 2], q: [f32; 2], e: [f32; 2]) -> Option<[f32; 2]> {
    let den = d[0] * e[1] - d[1] * e[0];
    if den.abs() < 1e-6 {
        return None;
    }
    let w = sub2(q, p);
    let s = (w[0] * e[1] - w[1] * e[0]) / den;
    Some(add2(p, d, s))
}

/// `world::math::V2::pseudo_angle`: sorts directions counter-clockwise from east.
fn pseudo_angle(d: [f32; 2]) -> f32 {
    let (x, y) = (d[0], -d[1]);
    let s = x.abs() + y.abs();
    if s == 0.0 {
        return 0.0;
    }
    let (x, y) = (x / s, y / s);
    if y >= 0.0 {
        if x >= 0.0 {
            y
        } else {
            1.0 - x
        }
    } else if x < 0.0 {
        2.0 - y
    } else {
        3.0 + x
    }
}

/// Distance from `p` to segment `a`–`b` and the parameter of the nearest point (0–1).
fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, f32) {
    let ab = sub2(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 > 0.0 {
        (dot(sub2(p, a), ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (len2(sub2(p, add2(a, ab, t))), t)
}

/// An edge's layout centerline (`world::roads::layout`): straight from each node out to
/// its trim, then the profile samples; normals pinned at the ends and trims, mitered at
/// the bends. Heights follow the profile by arc length.
struct Path {
    pts: Vec<[f32; 2]>,
    s: Vec<f32>,
    d: Vec<[f32; 2]>,
    n: Vec<[f32; 2]>,
    raw_s: Vec<f32>,
    raw_y: Vec<f32>,
}

impl Path {
    fn new(raw: &[[f32; 3]], ta: f32, tb: f32) -> Path {
        let n = raw.len();
        let r2: Vec<[f32; 2]> = raw.iter().map(|&p| xz(p)).collect();
        let mut raw_s = alloc::vec![0.0f32; n];
        for k in 1..n {
            raw_s[k] = raw_s[k - 1] + dist2(raw[k - 1], raw[k]);
        }
        let total = raw_s[n - 1];
        let da = dir(raw[0], raw[1]);
        let db = dir(raw[n - 1], raw[n - 2]);
        let (la, lb) = (left(da), left([-db[0], -db[1]]));
        let mut v: Vec<([f32; 2], Option<[f32; 2]>)> = Vec::with_capacity(n + 2);
        if n == 2 || ta + tb + 2.0 >= total {
            v.push((r2[0], Some(la)));
            v.push((r2[n - 1], Some(lb)));
        } else {
            v.push((r2[0], Some(la)));
            if ta > 0.01 {
                v.push((add2(r2[0], da, ta), Some(la)));
            }
            for k in 1..n - 1 {
                if raw_s[k] > ta + 1.0 && raw_s[k] < total - tb - 1.0 {
                    v.push((r2[k], None));
                }
            }
            if tb > 0.01 {
                v.push((add2(r2[n - 1], db, tb), Some(lb)));
            }
            v.push((r2[n - 1], Some(lb)));
        }
        let pts: Vec<[f32; 2]> = v.iter().map(|x| x.0).collect();
        let mut d = Vec::with_capacity(pts.len() - 1);
        let mut s = alloc::vec![0.0f32];
        for k in 0..pts.len() - 1 {
            let seg = sub2(pts[k + 1], pts[k]);
            d.push(unit2(seg));
            s.push(s[k] + len2(seg));
        }
        if n == 2 {
            let dd = unit2(sub2(r2[1], r2[0]));
            d[0] = dd;
            v[0].1 = Some(left(dd));
            v[1].1 = Some(left(dd));
        }
        let normals = (0..pts.len())
            .map(|k| match v[k].1 {
                Some(nrm) => nrm,
                None => {
                    let (u, w) = (left(d[k - 1]), left(d[k]));
                    let m = unit2([u[0] + w[0], u[1] + w[1]]);
                    let c = dot(m, u);
                    if c <= 0.0 {
                        u
                    } else {
                        let k = 1.0 / c.max(0.5);
                        [m[0] * k, m[1] * k]
                    }
                }
            })
            .collect();
        Path {
            pts,
            s,
            d,
            n: normals,
            raw_s,
            raw_y: raw.iter().map(|p| p[1]).collect(),
        }
    }

    fn len(&self) -> f32 {
        self.s[self.s.len() - 1]
    }

    /// Ground point `s` along the centerline, `t` to its left (`world`'s `EdgePath::xz`).
    fn xz(&self, s: f32, t: f32) -> [f32; 2] {
        let last = self.d.len() - 1;
        let k = (0..last).find(|&k| s < self.s[k + 1]).unwrap_or(last);
        let ds = s - self.s[k];
        let c = add2(self.pts[k], self.d[k], ds);
        let (n0, n1) = (self.n[k], self.n[k + 1]);
        let span = self.s[k + 1] - self.s[k];
        let f = if span > 0.0 { ds / span } else { 0.0 };
        let n = [n0[0] + (n1[0] - n0[0]) * f, n0[1] + (n1[1] - n0[1]) * f];
        add2(c, n, t)
    }

    /// Road surface height `s` along (by the profile's arc length, scaled to the layout's).
    fn y(&self, s: f32) -> f32 {
        let total = self.raw_s[self.raw_s.len() - 1];
        let x = if self.len() > 0.0 {
            s / self.len() * total
        } else {
            0.0
        };
        let k = self.raw_s[1..]
            .partition_point(|&v| v < x)
            .min(self.raw_s.len() - 2);
        let span = self.raw_s[k + 1] - self.raw_s[k];
        let f = if span > 0.0 {
            ((x - self.raw_s[k]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.raw_y[k] + (self.raw_y[k + 1] - self.raw_y[k]) * f
    }

    fn p3(&self, s: f32, t: f32, lift: f32) -> [f32; 3] {
        let q = self.xz(s, t);
        [q[0], self.y(s) + lift, q[1]]
    }
}

/// Pedestrians per 100 m of walk at normal density, by zone.
fn zone_density(z: ZoneId) -> f32 {
    match z {
        ZoneId::Downtown | ZoneId::Strip => 3.0,
        ZoneId::Beach => 2.5,
        ZoneId::Commercial => 2.0,
        ZoneId::Park => 1.5,
        ZoneId::Residential => 0.6,
        ZoneId::Suburb | ZoneId::Industrial => 0.3,
        ZoneId::Hills => 0.1,
        ZoneId::Farmland | ZoneId::Highway | ZoneId::Water => 0.0,
    }
}

/// The zone under ground point `p` (`world`'s detail pass lookup).
fn zone_at(place: &Place, p: [f32; 2]) -> ZoneId {
    let h = &place.header;
    let res = usize::from(place.terrain.res_per_chunk.max(1));
    let [w, d] = place::Zones::cells_per_side(h, place.terrain.res_per_chunk);
    let cell = h.chunk_size_m / res as f32;
    let o = h.origin_m();
    let (x, z) = ((p[0] - o[0]) / cell, (p[1] - o[1]) / cell);
    if x < 0.0 || z < 0.0 || w == 0 || d == 0 {
        return ZoneId::Water;
    }
    let (x, z) = ((x as usize).min(w - 1), (z as usize).min(d - 1));
    place
        .zones
        .cells
        .get(z * w + x)
        .copied()
        .unwrap_or(ZoneId::Water)
}

/// Builder state: nodes and links before the adjacency is grouped.
struct Build {
    nodes: Vec<[f32; 3]>,
    links: Vec<Link>,
}

impl Build {
    fn node(&mut self, p: [f32; 3]) -> u32 {
        self.nodes.push(p);
        (self.nodes.len() - 1) as u32
    }

    /// Links `a` to `b`, cut into pieces of at most [`MAX_LINK_M`].
    fn link(&mut self, a: u32, b: u32, kind: LinkKind, edge: u32, crossing: u32, road_side: i8) {
        let (pa, pb) = (self.nodes[a as usize], self.nodes[b as usize]);
        let len = dist2(pa, pb);
        let pieces = ((len / MAX_LINK_M) as u32 + 1).max(1);
        let room = match kind {
            LinkKind::Walk if road_side > 0 => [ROOM_BACK_M, ROOM_CURB_M],
            LinkKind::Walk if road_side < 0 => [ROOM_CURB_M, ROOM_BACK_M],
            LinkKind::Walk => [ROOM_BACK_M; 2],
            LinkKind::Spur | LinkKind::Crossing => [ROOM_CROSSING_M; 2],
        };
        let mut prev = a;
        for k in 1..=pieces {
            let next = if k == pieces {
                b
            } else {
                let f = k as f32 / pieces as f32;
                self.node(crate::math::lerp3(pa, pb, f))
            };
            let (p0, p1) = (self.nodes[prev as usize], self.nodes[next as usize]);
            let l = dist2(p0, p1);
            self.links.push(Link {
                a: prev,
                b: next,
                length_m: l,
                dir: if l > 0.0 { dir(p0, p1) } else { [1.0, 0.0] },
                kind,
                edge,
                crossing,
                room,
                road_side,
                density: 0.0,
            });
            prev = next;
        }
    }
}

impl WalkGraph {
    /// Builds the sidewalk graph of `place`; `lanes` (its lane graph) gates the crossings.
    pub fn new(place: &Place, lanes: &LaneGraph) -> WalkGraph {
        let rnodes = &place.roads.nodes;
        let edges = &place.roads.edges;
        let raws: Vec<Vec<[f32; 3]>> = edges.iter().map(|e| raw_line(place, e)).collect();

        // Road ends per node, counter-clockwise as `world` sorts them.
        let mut incident: Vec<Vec<End>> = (0..rnodes.len()).map(|_| Vec::new()).collect();
        for (i, e) in edges.iter().enumerate() {
            let r = &raws[i];
            let n = r.len();
            for (node, at_b, d) in [
                (e.a, false, dir(r[0], r[1])),
                (e.b, true, dir(r[n - 1], r[n - 2])),
            ] {
                incident[node as usize].push(End {
                    edge: i as u32,
                    at_b,
                    dir: d,
                    hw: e.width_m * 0.5,
                    walk: e.sidewalk,
                    class: e.class,
                });
            }
        }
        // Per edge, per side (0 left, 1 right), per end (a, b): where the walk starts,
        // measured from that end; and per end its trim. Corners to join afterwards.
        let mut cut = alloc::vec![[[0.0f32; 2]; 2]; edges.len()];
        let mut trim = alloc::vec![[0.0f32; 2]; edges.len()];
        let mut merging = alloc::vec![false; rnodes.len()];
        let mut corners: Vec<(End, End, Turn)> = Vec::new();
        for (node, ends) in incident.iter_mut().enumerate() {
            ends.sort_by(|x, y| {
                pseudo_angle(x.dir)
                    .total_cmp(&pseudo_angle(y.dir))
                    .then(x.edge.cmp(&y.edge))
            });
            let n = ends.len();
            merging[node] =
                (0..n).any(|i| (i + 1..n).any(|j| dot(ends[i].dir, ends[j].dir) > MERGE_COS));
            if merging[node] {
                for e in ends.iter() {
                    let c = &mut cut[e.edge as usize];
                    c[0][usize::from(e.at_b)] = MERGE_CUT_M;
                    c[1][usize::from(e.at_b)] = MERGE_CUT_M;
                }
                continue;
            }
            if n < 2 {
                continue;
            }
            let np = rnodes[node].pos;
            for i in 0..n {
                let (a, b) = (ends[i], ends[(i + 1) % n]);
                let (la, rb) = (left(a.dir), right(b.dir));
                let turn = dot(la, b.dir);
                let at = |oa: f32, ob: f32| meet(add2(np, la, oa), a.dir, add2(np, rb, ob), b.dir);
                let (mut sa, mut sb, mut how) = (0.0, 0.0, Turn::Direct);
                if turn > STRAIGHT_SIN {
                    let r = if a.class == RoadClass::Local && b.class == RoadClass::Local {
                        4.5
                    } else {
                        6.0
                    };
                    if let Some(q) = at(a.hw + r, b.hw + r) {
                        sa = dot(sub2(q, np), a.dir);
                        sb = dot(sub2(q, np), b.dir);
                        how = Turn::Arc {
                            q,
                            r: r - WALK_LINE_M,
                        };
                    }
                } else if turn < -STRAIGHT_SIN {
                    if let (Some(c), Some(p)) =
                        (at(a.hw, b.hw), at(a.hw + WALK_LINE_M, b.hw + WALK_LINE_M))
                    {
                        sa = dot(sub2(c, np), a.dir);
                        sb = dot(sub2(c, np), b.dir);
                        how = Turn::Point(p);
                    }
                }
                cut[a.edge as usize][a.left_side()][usize::from(a.at_b)] = sa;
                cut[b.edge as usize][b.right_side()][usize::from(b.at_b)] = sb;
                if a.walk && b.walk {
                    corners.push((a, b, how));
                }
            }
            for e in ends.iter() {
                let c = &cut[e.edge as usize];
                let end = usize::from(e.at_b);
                trim[e.edge as usize][end] =
                    c[e.left_side()][end].max(c[e.right_side()][end]).max(0.0);
            }
        }
        let paths: Vec<Path> = raws
            .iter()
            .enumerate()
            .map(|(i, r)| Path::new(r, trim[i][0], trim[i][1]))
            .collect();

        // A walk cut back further than its edge is long (a short stub, often where a chunk
        // border splits a road just past a junction) carries the rest of the cut on through
        // the straight two-road node at its far end into the next edge; the corner then
        // joins that edge's walk. `owner[edge][side][end]` is the walk end that stands in.
        let mut owner = alloc::vec![[[(NONE, 0usize, 0usize); 2]; 2]; edges.len()];
        let mut gone = alloc::vec![[false; 2]; edges.len()];
        for i in 0..edges.len() {
            for side in 0..2 {
                for end in 0..2 {
                    let (mut e, mut sd, mut en) = (i, side, end);
                    let mut c = cut[i][side][end];
                    for _ in 0..4 {
                        let len = paths[e].len();
                        if c < len - 0.5 {
                            break;
                        }
                        let far = 1 - en;
                        let node = if far == 0 { edges[e].a } else { edges[e].b };
                        let ends = &incident[node as usize];
                        let at_far = |x: &End| x.edge as usize == e && usize::from(x.at_b) == far;
                        let (Some(me), Some(next)) = (
                            ends.iter().find(|x| at_far(x)),
                            ends.iter().find(|x| !at_far(x)),
                        ) else {
                            break;
                        };
                        let n = next.edge as usize;
                        if ends.len() != 2
                            || merging[node as usize]
                            || dot(me.dir, next.dir) > -STRAIGHT_COS
                            || !edges[n].sidewalk
                            || n == e
                        {
                            break;
                        }
                        gone[e][sd] = true;
                        let sd2 = if (far == 1) != next.at_b { sd } else { 1 - sd };
                        let en2 = usize::from(next.at_b);
                        c -= len;
                        cut[n][sd2][en2] = cut[n][sd2][en2].max(c);
                        (e, sd, en) = (n, sd2, en2);
                    }
                    owner[i][side][end] = (e as u32, sd, en);
                }
            }
        }

        // Crosswalks: (edge, end) → distance from that end.
        let mut crosswalk = alloc::vec![[None::<f32>; 2]; edges.len()];
        for (node, ends) in incident.iter().enumerate() {
            if merging[node] || ends.len() < 3 {
                continue;
            }
            for e in ends {
                let i = e.edge as usize;
                let len = paths[i].len();
                if !edges[i].sidewalk
                    || edges[i].class == RoadClass::Highway
                    || len - trim[i][0] - trim[i][1] < MIN_MARKED_M
                {
                    continue;
                }
                crosswalk[i][usize::from(e.at_b)] =
                    Some(trim[i][usize::from(e.at_b)] + CROSSWALK_M);
            }
        }

        let mut b = Build {
            nodes: Vec::new(),
            links: Vec::new(),
        };
        // Walk lines: per edge and side, the end nodes and the crosswalk feet.
        let mut walk_end = alloc::vec![[[NONE; 2]; 2]; edges.len()];
        let mut feet = alloc::vec![[[NONE; 2]; 2]; edges.len()];
        for (i, e) in edges.iter().enumerate() {
            if !e.sidewalk {
                continue;
            }
            let path = &paths[i];
            let len = path.len();
            let hw = e.width_m * 0.5;
            for side in 0..2 {
                if gone[i][side] {
                    continue;
                }
                let sigma = if side == 0 { 1.0 } else { -1.0 };
                let t = sigma * (hw + WALK_LINE_M);
                let s0 = cut[i][side][0];
                let s1 = len - cut[i][side][1];
                if s1 - s0 < 0.5 {
                    // Too short to walk along: one node joins whatever meets it.
                    let mid = 0.5 * (s0 + s1).clamp(0.0, 2.0 * len);
                    let k = b.node(path.p3(mid.clamp(0.0, len), t, CURB_M));
                    walk_end[i][side] = [k, k];
                    continue;
                }
                // Breakpoints: the ends, the layout bends, the crosswalk feet.
                let mut stops: Vec<(f32, i8)> = alloc::vec![(s0, -1), (s1, -1)];
                for &s in &path.s[1..path.s.len() - 1] {
                    if s > s0 + 0.3 && s < s1 - 0.3 {
                        stops.push((s, -1));
                    }
                }
                for (end, at) in crosswalk[i].iter().enumerate() {
                    if let Some(c) = *at {
                        let s = if end == 0 { c } else { len - c };
                        if s > s0 + 0.3 && s < s1 - 0.3 {
                            stops.push((s, end as i8));
                        }
                    }
                }
                stops.sort_by(|x, y| x.0.total_cmp(&y.0));
                let road_side = if side == 0 { -1 } else { 1 };
                let mut prev = NONE;
                for &(s, foot) in &stops {
                    let k = b.node(path.p3(s, t, CURB_M));
                    if prev == NONE {
                        walk_end[i][side][0] = k;
                    } else {
                        b.link(prev, k, LinkKind::Walk, i as u32, NONE, road_side);
                    }
                    if foot >= 0 {
                        feet[i][side][foot as usize] = k;
                    }
                    prev = k;
                }
                walk_end[i][side][1] = prev;
            }
        }
        // Corners.
        for &(a, c, how) in &corners {
            let end_of = |e: &End, side: usize| {
                let (oe, os, on) = owner[e.edge as usize][side][usize::from(e.at_b)];
                walk_end[oe as usize][os][on]
            };
            let (from, to) = (end_of(&a, a.left_side()), end_of(&c, c.right_side()));
            if from == NONE || to == NONE || from == to {
                continue;
            }
            let (pf, pt) = (b.nodes[from as usize], b.nodes[to as usize]);
            let y = 0.5 * (pf[1] + pt[1]);
            let mut prev = from;
            match how {
                Turn::Arc { q, r } if r > 0.0 => {
                    // About `q` from the direction of `from` to that of `to`, the short way.
                    let u = unit2(sub2(xz(pf), q));
                    let w = unit2(sub2(xz(pt), q));
                    let ang = crate::math::atan2(u[0] * w[1] - u[1] * w[0], dot(u, w));
                    let steps = ((ang.abs() / 0.35) as u32).clamp(1, 8);
                    for k in 1..steps {
                        let f = k as f32 / steps as f32;
                        let (sn, cs) = sin_cos(ang * f);
                        let d = [u[0] * cs - u[1] * sn, u[0] * sn + u[1] * cs];
                        let p = add2(q, d, r);
                        let h = pf[1] + (pt[1] - pf[1]) * f;
                        let n = b.node([p[0], h, p[1]]);
                        b.link(prev, n, LinkKind::Walk, NONE, NONE, 0);
                        prev = n;
                    }
                }
                Turn::Point(p) => {
                    let n = b.node([p[0], y, p[1]]);
                    b.link(prev, n, LinkKind::Walk, NONE, NONE, 0);
                    prev = n;
                }
                _ => {}
            }
            b.link(prev, to, LinkKind::Walk, NONE, NONE, 0);
        }
        // Crossings.
        let lane_segs: Vec<CoverSeg> = lanes
            .lanes
            .iter()
            .enumerate()
            .flat_map(|(l, lane)| {
                let half = 0.5 * lane_width(&edges[lane.edge as usize], lane.forward);
                lanes
                    .points(l as u32)
                    .windows(2)
                    .map(move |w| (w[0], w[1], half, l as u32))
            })
            .collect();
        let lane_cover = Cover::new(&lane_segs, WAIT_CLEAR_M + 0.2);
        let mut crossings: Vec<Crossing> = Vec::new();
        for (i, e) in edges.iter().enumerate() {
            for end in 0..2 {
                let (Some(c), [f0, f1]) = (crosswalk[i][end], [feet[i][0][end], feet[i][1][end]])
                else {
                    continue;
                };
                if f0 == NONE || f1 == NONE {
                    continue;
                }
                let path = &paths[i];
                let len = path.len();
                let s = if end == 0 { c } else { len - c };
                let hw = e.width_m * 0.5;
                let node = if end == 0 { e.a } else { e.b };
                let at_b = end == 1;
                let lane = lanes
                    .lanes
                    .iter()
                    .position(|l| l.end_node == node && l.edge == i as u32 && l.forward == at_b)
                    .map_or(NONE, |k| k as u32);
                let signal = lane != NONE && lanes.lanes[lane as usize].control == Control::Signal;
                // Where people wait: behind the curb top, and farther back where any lane
                // (as the lane graph lays it, which can stray from the drawn curb near a
                // junction) comes closer than `WAIT_CLEAR_M`.
                let wait = |sigma: f32| {
                    let mut t = hw + CURB_STEP_M;
                    while t < hw + WALK_LINE_M - 0.3
                        && lane_cover.clearance(path.p3(s, sigma * t, 0.0)) < WAIT_CLEAR_M
                    {
                        t += 0.1;
                    }
                    t
                };
                let (t0, t1) = (wait(1.0), wait(-1.0));
                let curb0 = b.node(path.p3(s, t0, CURB_M));
                let road0 = b.node(path.p3(s, hw - ROAD_STEP_M, 0.0));
                let road1 = b.node(path.p3(s, -(hw - ROAD_STEP_M), 0.0));
                let curb1 = b.node(path.p3(s, -t1, CURB_M));
                let k = crossings.len() as u32;
                let edge = i as u32;
                b.link(f0, curb0, LinkKind::Spur, edge, k, 0);
                b.link(curb0, road0, LinkKind::Crossing, edge, k, 0);
                b.link(road0, road1, LinkKind::Crossing, edge, k, 0);
                b.link(road1, curb1, LinkKind::Crossing, edge, k, 0);
                b.link(curb1, f1, LinkKind::Spur, edge, k, 0);
                crossings.push(Crossing {
                    node,
                    edge,
                    lane,
                    signal,
                    curbs: [curb0, curb1],
                    length_m: 2.0 * (hw + CURB_STEP_M),
                });
            }
        }
        // Where two roads overlap, `world` lays one's pavement across the other's sidewalk:
        // walks running over another road's pavement are dropped (the walk ends there).
        let road_segs: Vec<CoverSeg> = raws
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                let hw = edges[i].width_m * 0.5;
                r.windows(2).map(move |w| (w[0], w[1], hw, i as u32))
            })
            .collect();
        let cover = Cover::new(&road_segs, COVER_MARGIN_M);
        let nodes = &b.nodes;
        // The same for walks into a lane (where the lane graph's lanes and the drawn road
        // disagree near a junction): a walk keeps `WALK_LANE_CLEAR_M` outside every lane.
        let into_lane = |a: [f32; 3], c: [f32; 3]| {
            let steps = (dist2(a, c) as i32 + 1).max(1);
            (0..=steps).any(|k| {
                let q = crate::math::lerp3(a, c, k as f32 / steps as f32);
                lane_cover.clearance(q) < WALK_LANE_CLEAR_M
            })
        };
        b.links.retain(|l| {
            let (a, c) = (nodes[l.a as usize], nodes[l.b as usize]);
            l.kind != LinkKind::Walk || !(cover.over_road(a, c, l.edge) || into_lane(a, c))
        });
        for l in &mut b.links {
            let (pa, pb) = (b.nodes[l.a as usize], b.nodes[l.b as usize]);
            let mid = [(pa[0] + pb[0]) * 0.5, (pa[2] + pb[2]) * 0.5];
            l.density = zone_density(zone_at(place, mid));
        }

        // Adjacency, grouped by node, links in order.
        let mut count = alloc::vec![0u32; b.nodes.len()];
        for l in &b.links {
            count[l.a as usize] += 1;
            count[l.b as usize] += 1;
        }
        let mut nodes = Vec::with_capacity(b.nodes.len());
        let mut first = 0u32;
        for (k, &p) in b.nodes.iter().enumerate() {
            nodes.push(WalkNode {
                pos: p,
                first_link: first,
                link_count: 0,
            });
            first += count[k];
        }
        let mut adj = alloc::vec![0u32; first as usize];
        for (i, l) in b.links.iter().enumerate() {
            for v in [l.a, l.b] {
                let n = &mut nodes[v as usize];
                adj[(n.first_link + n.link_count) as usize] = i as u32;
                n.link_count += 1;
            }
        }
        let mut g = WalkGraph {
            nodes,
            adj,
            links: b.links,
            crossings,
            lane_cover,
            ..WalkGraph::default()
        };
        g.grid();
        g.spots = spots(place, &g);
        g.grid();
        g
    }

    /// (Re)builds the cell lists of links and spots.
    fn grid(&mut self) {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for n in &self.nodes {
            lo = [lo[0].min(n.pos[0]), lo[1].min(n.pos[2])];
            hi = [hi[0].max(n.pos[0]), hi[1].max(n.pos[2])];
        }
        if self.nodes.is_empty() {
            (lo, hi) = ([0.0; 2], [0.0; 2]);
        }
        self.grid_origin = [lo[0] - CELL_M, lo[1] - CELL_M];
        self.grid_cells = [
            ((hi[0] - lo[0]) / CELL_M) as u32 + 3,
            ((hi[1] - lo[1]) / CELL_M) as u32 + 3,
        ];
        let cells = (self.grid_cells[0] * self.grid_cells[1]) as usize;
        let link_cells: Vec<usize> = (0..self.links.len())
            .map(|i| {
                let l = &self.links[i];
                let (pa, pb) = (self.nodes[l.a as usize].pos, self.nodes[l.b as usize].pos);
                self.cell_of([(pa[0] + pb[0]) * 0.5, (pa[2] + pb[2]) * 0.5])
                    .unwrap_or(0)
            })
            .collect();
        let spot_cells: Vec<usize> = self
            .spots
            .iter()
            .map(|s| self.cell_of(xz(s.pos)).unwrap_or(0))
            .collect();
        (self.cell_link_start, self.cell_links) = bucket(cells, &link_cells);
        (self.cell_spot_start, self.cell_spots) = bucket(cells, &spot_cells);
    }

    /// The grid cell holding ground point `p`.
    pub fn cell_of(&self, p: [f32; 2]) -> Option<usize> {
        let x = (p[0] - self.grid_origin[0]) / CELL_M;
        let z = (p[1] - self.grid_origin[1]) / CELL_M;
        if x < 0.0 || z < 0.0 {
            return None;
        }
        let (x, z) = (x as u32, z as u32);
        (x < self.grid_cells[0] && z < self.grid_cells[1])
            .then_some((z * self.grid_cells[0] + x) as usize)
    }

    /// Links with their midpoint in grid cell `c`.
    pub fn cell_links(&self, c: usize) -> &[u32] {
        let (a, b) = (self.cell_link_start[c], self.cell_link_start[c + 1]);
        &self.cell_links[a as usize..b as usize]
    }

    /// Spots in grid cell `c`.
    pub fn cell_spots(&self, c: usize) -> &[u32] {
        let (a, b) = (self.cell_spot_start[c], self.cell_spot_start[c + 1]);
        &self.cell_spots[a as usize..b as usize]
    }

    /// The links at node `n`.
    pub fn links_at(&self, n: u32) -> &[u32] {
        let n = &self.nodes[n as usize];
        &self.adj[n.first_link as usize..(n.first_link + n.link_count) as usize]
    }

    /// Ground point `s` along link `l` and `lat` to its left (`s` may run past either end).
    pub fn point(&self, l: u32, s: f32, lat: f32) -> [f32; 3] {
        let link = &self.links[l as usize];
        let (pa, pb) = (
            self.nodes[link.a as usize].pos,
            self.nodes[link.b as usize].pos,
        );
        let f = if link.length_m > 0.0 {
            (s / link.length_m).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let n = left(link.dir);
        [
            pa[0] + link.dir[0] * s + n[0] * lat,
            pa[1] + (pb[1] - pa[1]) * f,
            pa[2] + link.dir[1] * s + n[1] * lat,
        ]
    }

    /// Whether crossing `c` shows *walk* at signal `tick`: on a signalized junction while
    /// the road's own approach is red now and still red `clear_ticks` later (red is one
    /// unbroken stretch of the cycle, so it stays red throughout); always without a signal.
    pub fn walk(&self, lanes: &LaneGraph, c: u32, tick: u64, clear_ticks: u64) -> bool {
        let x = &self.crossings[c as usize];
        if !x.signal {
            return true;
        }
        lanes.light(x.lane, tick) == Some(Light::Red)
            && lanes.light(x.lane, tick + clear_ticks) == Some(Light::Red)
    }

    /// Total walk length, m.
    pub fn walk_length(&self) -> f32 {
        self.links
            .iter()
            .filter(|l| l.kind == LinkKind::Walk)
            .map(|l| l.length_m)
            .sum()
    }

    /// Heap bytes held by the graph.
    pub fn bytes(&self) -> usize {
        use core::mem::size_of;
        self.nodes.len() * size_of::<WalkNode>()
            + (self.adj.len() + self.cell_links.len() + self.cell_spots.len()) * 4
            + (self.cell_link_start.len() + self.cell_spot_start.len()) * 4
            + self.links.len() * size_of::<Link>()
            + self.crossings.len() * size_of::<Crossing>()
            + self.spots.len() * size_of::<Spot>()
            + self.lane_cover.start.len() * 4
            + self.lane_cover.segs.len() * size_of::<CoverSeg>()
    }

    /// Whether `p` is inside a lane's painted band, at its level (where the lane graph's
    /// lanes and the world's road ends differ near a junction, a walk's edge can be).
    pub fn in_lane(&self, p: [f32; 3]) -> bool {
        self.lane_cover.near(p).iter().any(|&(s0, s1, half, _)| {
            let (d, t) = seg_dist(xz(p), xz(s0), xz(s1));
            let y = s0[1] + (s1[1] - s0[1]) * t;
            d < half && (p[1] - CURB_M - y).abs() < COVER_DY_M
        })
    }
}

/// The roads' pavement, as centerline segments with their half widths in a grid.
#[derive(Clone, Debug, Default, PartialEq)]
struct Cover {
    origin: [f32; 2],
    size: [i32; 2],
    start: Vec<u32>,
    segs: Vec<CoverSeg>,
}

/// A centerline segment, its road's half width and the road edge.
type CoverSeg = ([f32; 3], [f32; 3], f32, u32);

/// Cover grid cell, m.
const COVER_CELL_M: f32 = 16.0;
/// A walk this close to another road's centerline (past its half width) is on it; heights
/// further apart than `COVER_DY_M` are different levels (a bridge).
const COVER_MARGIN_M: f32 = 0.3;
const COVER_DY_M: f32 = 1.5;

impl Cover {
    /// The segments `segs`, each filed under the cells within its half width plus `grow`.
    fn new(segs: &[CoverSeg], grow: f32) -> Cover {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        let mut reach = 0.0f32;
        for &(a, b, half, _) in segs {
            for p in [a, b] {
                lo = [lo[0].min(p[0]), lo[1].min(p[2])];
                hi = [hi[0].max(p[0]), hi[1].max(p[2])];
            }
            reach = reach.max(half + grow);
        }
        if segs.is_empty() {
            (lo, hi) = ([0.0; 2], [0.0; 2]);
        }
        let origin = [lo[0] - reach - COVER_CELL_M, lo[1] - reach - COVER_CELL_M];
        let size = [
            ((hi[0] - origin[0] + reach) / COVER_CELL_M) as i32 + 2,
            ((hi[1] - origin[1] + reach) / COVER_CELL_M) as i32 + 2,
        ];
        let cell = |x: f32, z: f32| {
            (
                ((x - origin[0]) / COVER_CELL_M) as i32,
                ((z - origin[1]) / COVER_CELL_M) as i32,
            )
        };
        let mut items: Vec<(usize, CoverSeg)> = Vec::new();
        for &seg in segs {
            let (a, b, half, _) = seg;
            let g = half + grow;
            let (x0, z0) = cell(a[0].min(b[0]) - g, a[2].min(b[2]) - g);
            let (x1, z1) = cell(a[0].max(b[0]) + g, a[2].max(b[2]) + g);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    items.push(((z * size[0] + x) as usize, seg));
                }
            }
        }
        let cells = (size[0] * size[1]) as usize;
        let of: Vec<usize> = items.iter().map(|x| x.0).collect();
        let (start, order) = bucket(cells, &of);
        let segs = order.iter().map(|&k| items[k as usize].1).collect();
        Cover {
            origin,
            size,
            start,
            segs,
        }
    }

    /// The segments filed under the cell holding `p`.
    fn near(&self, p: [f32; 3]) -> &[CoverSeg] {
        let x = ((p[0] - self.origin[0]) / COVER_CELL_M) as i32;
        let z = ((p[2] - self.origin[1]) / COVER_CELL_M) as i32;
        if x < 0 || z < 0 || x >= self.size[0] || z >= self.size[1] {
            return &[];
        }
        let c = (z * self.size[0] + x) as usize;
        &self.segs[self.start[c] as usize..self.start[c + 1] as usize]
    }

    /// How far `p` is outside every segment's band (negative inside one), up to the
    /// grow distance the cover was built with.
    fn clearance(&self, p: [f32; 3]) -> f32 {
        let mut best = f32::MAX;
        for &(s0, s1, half, _) in self.near(p) {
            best = best.min(seg_dist(xz(p), xz(s0), xz(s1)).0 - half);
        }
        best
    }

    /// Whether the walk from `a` to `b` (along road `own`) runs over another road.
    fn over_road(&self, a: [f32; 3], b: [f32; 3], own: u32) -> bool {
        let steps = (dist2(a, b) as i32 + 1).max(1);
        (0..=steps).any(|k| {
            let p = crate::math::lerp3(a, b, k as f32 / steps as f32);
            self.near(p).iter().any(|&(s0, s1, hw, e)| {
                if e == own {
                    return false;
                }
                let (d, t) = seg_dist(xz(p), xz(s0), xz(s1));
                let y = s0[1] + (s1[1] - s0[1]) * t;
                d < hw + COVER_MARGIN_M && (p[1] - CURB_M - y).abs() < COVER_DY_M
            })
        })
    }
}

/// Items by cell: `(start, items)` with cell `c`'s items at `items[start[c]..start[c + 1]]`.
fn bucket(cells: usize, of: &[usize]) -> (Vec<u32>, Vec<u32>) {
    let mut start = alloc::vec![0u32; cells + 1];
    for &c in of {
        start[c + 1] += 1;
    }
    for c in 0..cells {
        start[c + 1] += start[c];
    }
    let mut fill = start.clone();
    let mut items = alloc::vec![0u32; of.len()];
    for (i, &c) in of.iter().enumerate() {
        items[fill[c] as usize] = i as u32;
        fill[c] += 1;
    }
    (start, items)
}

/// The nearest walk link to `p` within `reach` among the cells around it: `(link, s, lat)`.
fn nearest_walk(g: &WalkGraph, p: [f32; 2], reach: f32) -> Option<(u32, f32, f32)> {
    let r = (reach / CELL_M) as i32 + 1;
    let c = g.cell_of(p)? as i32;
    let w = g.grid_cells[0] as i32;
    let (cx, cz) = (c % w, c / w);
    let mut best: Option<(f32, u32, f32, f32)> = None;
    for dz in -r..=r {
        for dx in -r..=r {
            let (x, z) = (cx + dx, cz + dz);
            if x < 0 || z < 0 || x >= w || z >= g.grid_cells[1] as i32 {
                continue;
            }
            for &l in g.cell_links((z * w + x) as usize) {
                let link = &g.links[l as usize];
                if link.kind != LinkKind::Walk || link.road_side == 0 {
                    continue;
                }
                let (a, b) = (g.nodes[link.a as usize].pos, g.nodes[link.b as usize].pos);
                let (d, t) = seg_dist(p, xz(a), xz(b));
                if d <= reach && best.is_none_or(|q| d < q.0) {
                    let s = t * link.length_m;
                    let lat = dot(sub2(p, xz(a)), left(link.dir));
                    best = Some((d, l, s, lat));
                }
            }
        }
    }
    best.map(|q| (q.1, q.2, q.3))
}

/// Unit direction from a walk link toward its back (away from the road).
fn back_of(link: &Link) -> [f32; 2] {
    let l = left(link.dir);
    let k = -f32::from(link.road_side);
    [l[0] * k, l[1] * k]
}

fn spot_at(g: &WalkGraph, kind: SpotKind, link: u32, s: f32, lat: f32, facing: [f32; 2]) -> Spot {
    let lk = &g.links[link as usize];
    let s = s.clamp(0.0, lk.length_m);
    let lat = lat.clamp(-lk.room[0] + 0.1, lk.room[1] - 0.1);
    Spot {
        kind,
        link,
        s,
        lat,
        pos: g.point(link, s, lat),
        facing,
    }
}

/// Every spot of the place (see the module docs).
fn spots(place: &Place, g: &WalkGraph) -> Vec<Spot> {
    let mut out = Vec::new();
    // Shop windows, doors and parking entrances on walls facing a walk.
    for b in &place.buildings {
        let kind = match b.usage {
            BuildingUse::Commercial => SpotKind::Window,
            BuildingUse::Office | BuildingUse::Hotel | BuildingUse::Civic => SpotKind::Door,
            BuildingUse::Parking => SpotKind::Parking,
            _ => continue,
        };
        let n = b.footprint.len();
        for k in 0..n {
            let (p, q) = (b.footprint[k], b.footprint[(k + 1) % n]);
            let wall = sub2(q, p);
            let len = len2(wall);
            if len < 4.0 {
                continue;
            }
            let out_n = right(unit2(wall));
            let count = ((len / 10.0) as u32).clamp(1, 3);
            let count = if kind == SpotKind::Window { count } else { 1 };
            for j in 0..count {
                let m = add2(p, wall, (j as f32 + 0.5) / count as f32);
                let probe = add2(m, out_n, 1.0);
                let Some((l, s, _)) = nearest_walk(g, probe, WALL_REACH_M) else {
                    continue;
                };
                let link = &g.links[l as usize];
                // The wall must face the walk, from behind it.
                let to_walk = unit2(sub2(xz(g.point(l, s, 0.0)), m));
                if dot(out_n, to_walk) < 0.7 || dot(back_of(link), out_n) > -0.7 {
                    continue;
                }
                let back = -f32::from(link.road_side);
                let lat = back * link.room[usize::from(back > 0.0)];
                out.push(spot_at(g, kind, l, s, lat, [-out_n[0], -out_n[1]]));
            }
        }
    }
    // Benches by a walk; stops by those on busy roads.
    for p in &place.props.items {
        if p.kind != PropKind::Bench {
            continue;
        }
        let at = [p.pos[0], p.pos[2]];
        let Some((l, s, lat)) = nearest_walk(g, at, BENCH_REACH_M) else {
            continue;
        };
        let (sn, cs) = sin_cos(p.yaw);
        let facing = [-sn, -cs];
        let seat = add2(at, facing, -0.05);
        out.push(Spot {
            kind: SpotKind::Bench,
            link: l,
            s,
            lat,
            pos: [seat[0], p.pos[1], seat[1]],
            facing,
        });
        let link = &g.links[l as usize];
        let busy = link.edge != NONE
            && matches!(
                place.roads.edges[link.edge as usize].class,
                RoadClass::Collector | RoadClass::Arterial
            );
        if busy {
            let toward = f32::from(link.road_side);
            let lat = toward * 0.9;
            let side = if dot(facing, link.dir) >= 0.0 {
                1.6
            } else {
                -1.6
            };
            out.push(spot_at(g, SpotKind::Stop, l, s + side, lat, facing));
        }
    }
    // Landmarks and piers at their nearest walk.
    let mut landmarks: Vec<([f32; 2], [f32; 2])> =
        place.pois.iter().map(|p| (p.pos, [0.0; 2])).collect();
    for p in &place.props.items {
        if let PropKind::Pier { .. } = p.kind {
            let (sn, cs) = sin_cos(p.yaw);
            landmarks.push(([p.pos[0], p.pos[2]], [-sn, -cs]));
        }
    }
    for (at, facing) in landmarks {
        let Some((l, s, _)) = nearest_walk(g, at, LANDMARK_REACH_M) else {
            continue;
        };
        let foot = xz(g.point(l, s, 0.0));
        let toward = unit2(sub2(at, foot));
        let facing = if len2(facing) > 0.0 { facing } else { toward };
        out.push(spot_at(g, SpotKind::Landmark, l, s, 0.0, facing));
    }
    // Beach behind a walk.
    let mut run = 0.0f32;
    for (i, link) in g.links.iter().enumerate() {
        if link.kind != LinkKind::Walk || link.road_side == 0 {
            continue;
        }
        let back = back_of(link);
        let mid = xz(g.point(i as u32, 0.5 * link.length_m, 0.0));
        if zone_at(place, add2(mid, back, 6.0)) != ZoneId::Beach {
            continue;
        }
        run += link.length_m;
        if run < BEACH_STEP_M {
            continue;
        }
        run = 0.0;
        let lat = -f32::from(link.road_side) * (ROOM_BACK_M - 0.1);
        let jitter = (hash64(KEY_WALKS, &[i as u64]) % 100) as f32 / 100.0;
        out.push(spot_at(
            g,
            SpotKind::Beach,
            i as u32,
            link.length_m * (0.25 + 0.5 * jitter),
            lat,
            back,
        ));
    }
    out
}
