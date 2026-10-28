//! Stage 7, detailing (`docs/WORLD_GENERATION.md`): the street furniture, trees, parked
//! cars, signs and lights of a place, written into its `Props` and `Lights` tables so the
//! meshes, the collider, the streamer and the app all read the same list.
//!
//! What goes where, by the zone of each road side and the template's per-zone [`Props`]:
//! - **Street lights** (sodium cobra heads at the curb): both sides of arterials and the
//!   beachfront road every 30–40 m, staggered; one side of collectors (45 m and up) and
//!   locals (60 m and up); none along rural, hill, highway or alley roads, but one at each
//!   of their junctions.
//! - **Street trees** at the zone's spacing and species: palms in sidewalk wells on the
//!   strip, downtown and commercial streets, shade trees in residential parkways, trees in
//!   the front yards behind the walk elsewhere; eucalyptus groves scattered over the hills,
//!   windbreak rows along some farm roads, a few trees in parks and back yards.
//! - **Hydrants** at the zone's spacing, **benches** with a **trash can** by the zone's
//!   density, **power poles** along residential, industrial, hill and farm roads,
//!   **mailboxes** at houses in the suburbs, hills and farms, **hedges and fences** along
//!   suburban lot lines, **billboards** along the highway and the zone's arterials.
//! - At junctions: **signal** poles where major roads cross, **stop signs** on the minor
//!   approaches (every approach of a four-way between equals), a **street-name post**.
//! - **Neon blade signs** on the zone's share of storefront buildings; **parked cars** along
//!   local curbs (parallel) for the zone's share of curb length (never across a curb cut),
//!   in driveways, and in the parking lots' stalls ([`crate::lots`]: with their light
//!   poles, island palms and frontage hedges), each lot car parked through the
//!   hours its zone's occupancy gives; at most [`MAX_CARS_PER_CHUNK`] at the curbs and in
//!   driveways a chunk and [`MAX_LOT_CARS_PER_CHUNK`] in its lots at any hour;
//!   the **pier** at the pier point, with lamps and benches.
//!
//! Every prop stands on the ground under it (a sidewalk's top, the graded terrain, the
//! asphalt for a parked car, the pier's deck), never on a road (but parked cars in their
//! lane), clear of buildings and, but for the lots' own, off the parking lots; trees keep
//! 1 m clear of buildings and of the walks but for those planted in parkways and sidewalk
//! wells.
//!
//! **Lights**: a sodium `Street` light in each street lamp's head, a `Neon` light at each
//! neon sign (the buildings' sign bands, found in their meshes, and the blade signs), a
//! warm `Window` fill in front of each lit shop.
//!
//! Deterministic: every draw comes from `Rng::new(seed, [DETAIL, part, item])`.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use place::{
    BuildingUse, Light, LightKind, LotKind, Place, Prop, PropKind, RoadClass, StyleTag, ZoneId,
    ALL_HOURS, SIDEWALK_WIDTH_M,
};
use skywave_core::{Rng, Seed};

use super::macro_map::{MacroMap, MacroPoiKind};
use super::stage;
use crate::buildings::{self, BuildingPlan, FacadeEnv};
use crate::grammar::{Grammar, LotRules, Planting};
use crate::lots::{self, ParkingLot};
use crate::math::{floor, point_in_polygon, segment_distance, V2, V3};
use crate::mesh::terrain::TerrainGrid;
use crate::mesh::{Glow, WorldMesh};
use crate::palette::Palette;
use crate::props::{
    self, yaw_of_dir, NEON_SIGN_OUT, NEON_SIGN_Y, PIER_HALF_WIDTH_M, PIER_HEAD_HALF_WIDTH_M,
    PIER_HEAD_M,
};
use crate::roads::{EdgePath, Network};
use crate::template::{RegionTemplate, Tree};
use crate::{ChunkPlan, CURB_M};

/// Parked cars at the curbs and in driveways a chunk holds at most...
pub const MAX_CARS_PER_CHUNK: usize = 24;
/// ...and parked cars in its lots at any hour.
pub const MAX_LOT_CARS_PER_CHUNK: usize = 16;
/// Length of a pier, m.
pub const PIER_M: f32 = 150.0;

/// Sub-keys of the detailing's draws.
const PART_SIDE: u64 = 1;
const PART_JUNCTION: u64 = 2;
const PART_HOUSE: u64 = 3;
const PART_LOT_LINE: u64 = 4;
const PART_NEON: u64 = 5;
const PART_LOTS: u64 = 6;
const PART_SCATTER: u64 = 7;
const PART_BILLBOARD: u64 = 8;
const PART_PIER: u64 = 9;
const PART_CAP: u64 = 10;
/// Key of the order the lots' cars are capped in.
const KEY_LOT_CAR: u64 = 0x4C4F_5443;

/// Lamp head: along the arm and up from the base to the light (just above the lens).
const LAMP_ARM_M: f32 = 2.175;
const LAMP_LIGHT_Y: f32 = 7.26;
/// Lateral offset of curbside furniture from the curb face, m.
const LAMP_IN_M: f32 = 0.45;
/// Gap from the gutter line to a parallel-parked car's center, m.
const PARK_IN_M: f32 = 1.35;
/// Grid of the scattered trees, m.
const SCATTER_M: f32 = 18.0;
/// Longest run of hedge or fence one prop holds, m: a run stands at one height.
const RUN_M: f32 = lots::HEDGE_RUN_M;
/// How far street furniture keeps from a curb cut along the curb, m.
const CUT_CLEAR_M: f32 = 1.5;

/// Light radii, m.
const STREET_RADIUS_M: f32 = 22.0;
const NEON_RADIUS_M: f32 = 8.0;
const WINDOW_RADIUS_M: f32 = 7.0;

/// Fills `place.props` and `place.lights` (see the module docs). `m` is the macro map the
/// place was built from (for the pier). Leaves the tables unsorted; the caller sorts the
/// place by chunk.
pub fn detail(
    seed: Seed,
    template: &RegionTemplate,
    grammar: &Grammar,
    m: &MacroMap,
    place: &mut Place,
) {
    let pal = Palette::socal_1985();
    let plan = ChunkPlan::new(place, &pal, grammar);
    let (props, lights) = {
        let mut d = Detailer::new(place, &plan, template, &grammar.lots, seed);
        d.streets();
        d.junctions();
        d.houses();
        d.lot_lines();
        d.neon_signs();
        d.lots();
        d.scatter();
        d.billboards();
        for poi in m.pois.iter().filter(|p| p.kind == MacroPoiKind::Pier) {
            d.pier(
                V2::from_array(poi.pos),
                V2::from_array(poi.facing).normalize(),
            );
        }
        d.cap_cars();
        // Whatever stands outside the place has no chunk to belong to.
        let h = &place.header;
        d.props
            .retain(|p| h.chunk_at([p.pos[0], p.pos[2]]).is_some());
        let mut lights = d.lights(&pal);
        lights.retain(|l| h.chunk_at([l.pos[0], l.pos[2]]).is_some());
        (d.props, lights)
    };
    place.props.items = props;
    place.lights.items = lights;
}

/// Determinism fingerprint of a place's detail: its props and lights, every field.
pub fn hash_detail(place: &Place) -> u64 {
    let mut words: Vec<u64> = Vec::with_capacity(place.props.items.len() * 4);
    for p in &place.props.items {
        words.extend(p.pos.iter().map(|v| u64::from(v.to_bits())));
        words.push(u64::from(p.yaw.to_bits()));
        let (tag, payload) = match p.kind {
            PropKind::StreetLight => (0, 0),
            PropKind::PalmTree => (1, 0),
            PropKind::Oak => (2, 0),
            PropKind::Hedge { len_dm } => (3, u64::from(len_dm)),
            PropKind::Bench => (4, 0),
            PropKind::Hydrant => (5, 0),
            PropKind::Mailbox => (6, 0),
            PropKind::Sign { text_seed } => (7, text_seed),
            PropKind::NeonSign { palette } => (8, u64::from(palette)),
            PropKind::Billboard => (9, 0),
            PropKind::PowerPole => (10, 0),
            PropKind::Fence { len_dm } => (11, u64::from(len_dm)),
            PropKind::ParkedCar {
                vehicle_class,
                hours,
            } => (12, u64::from(vehicle_class) | u64::from(hours) << 8),
            PropKind::Eucalyptus => (13, 0),
            PropKind::TrashCan => (14, 0),
            PropKind::StopSign => (15, 0),
            PropKind::Signal { arm_dm } => (16, u64::from(arm_dm)),
            PropKind::Pier { len_m } => (17, u64::from(len_m)),
        };
        words.push(tag);
        words.push(payload);
    }
    for l in &place.lights.items {
        let f = l
            .pos
            .iter()
            .chain(&l.color)
            .chain(core::iter::once(&l.radius));
        words.extend(f.map(|v| u64::from(v.to_bits())));
        words.push(l.kind as u64);
    }
    skywave_core::hash64(place.header.seed, &words)
}

/// Road segments, junction fans and building footprints binned on a grid, for clearance
/// tests.
struct Bins {
    origin: V2,
    cell: f32,
    w: usize,
    h: usize,
    cells: Vec<Vec<u32>>,
}

impl Bins {
    fn new(origin: V2, extent: V2, cell: f32) -> Bins {
        let w = (extent.x / cell) as usize + 1;
        let h = (extent.y / cell) as usize + 1;
        Bins {
            origin,
            cell,
            w,
            h,
            cells: (0..w * h).map(|_| Vec::new()).collect(),
        }
    }

    fn range(&self, lo: V2, hi: V2) -> Option<[usize; 4]> {
        let f = |v: f32, o: f32| floor((v - o) / self.cell);
        let (x0, x1) = (f(lo.x, self.origin.x), f(hi.x, self.origin.x));
        let (z0, z1) = (f(lo.y, self.origin.y), f(hi.y, self.origin.y));
        if x1 < 0.0 || z1 < 0.0 || x0 >= self.w as f32 || z0 >= self.h as f32 {
            return None;
        }
        let c = |v: f32, n: usize| (v.max(0.0) as usize).min(n - 1);
        Some([c(x0, self.w), c(x1, self.w), c(z0, self.h), c(z1, self.h)])
    }

    fn insert(&mut self, lo: V2, hi: V2, item: u32) {
        if let Some([x0, x1, z0, z1]) = self.range(lo, hi) {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    self.cells[z * self.w + x].push(item);
                }
            }
        }
    }

    fn at(&self, p: V2) -> &[u32] {
        match self.range(p, p) {
            Some([x, _, z, _]) => &self.cells[z * self.w + x],
            None => &[],
        }
    }
}

/// A road segment: ends, half width, the outer edge of its walk (or curb top), edge.
#[derive(Clone, Copy)]
struct Seg {
    a: V2,
    b: V2,
    hw: f32,
    walk: f32,
    edge: u32,
}

/// Largest reach a clearance test asks about beyond a road's walk or a footprint, m.
const MAX_REACH_M: f32 = 8.0;

/// What the detailing reads and the props it has made.
struct Detailer<'a> {
    place: &'a Place,
    net: &'a Network,
    side_zone: &'a [[Option<ZoneId>; 2]],
    plans: &'a [(Vec<FacadeEnv>, BuildingPlan)],
    footings: &'a [(f32, f32)],
    grid: &'a TerrainGrid,
    template: &'a RegionTemplate,
    lots: &'a LotRules,
    lot_plan: &'a [ParkingLot],
    /// The lots' ground (asphalt, walks, ramps, islands) with bounds, binned.
    lot_quads: Vec<([V2; 4], V2, V2)>,
    lot_bins: Bins,
    seed: u64,
    segs: Vec<Seg>,
    seg_bins: Bins,
    /// Junction asphalt fans and corner walks with their bounds.
    fans: Vec<(Vec<V2>, V2, V2)>,
    fan_bins: Bins,
    corners: Vec<(Vec<V2>, V2, V2)>,
    corner_bins: Bins,
    footprints: Vec<(Vec<V2>, V2, V2)>,
    fp_bins: Bins,
    /// Props made so far; parked cars apart until they are capped.
    props: Vec<Prop>,
    cars: Vec<Prop>,
}

/// Bounds of a point set.
fn bounds(pts: &[V2]) -> (V2, V2) {
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in pts {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

/// Distance from `p` to polygon `poly`'s boundary, 0 inside.
fn polygon_distance(p: V2, poly: &[V2]) -> f32 {
    if point_in_polygon(p, poly) {
        return 0.0;
    }
    let n = poly.len();
    (0..n)
        .map(|i| segment_distance(p, poly[i], poly[(i + 1) % n]).0)
        .fold(f32::MAX, f32::min)
}

/// The template's tree species as a prop kind: palms, tall gums and pines, canopy trees.
fn tree_kind(t: Tree) -> PropKind {
    match t {
        Tree::Palm | Tree::FanPalm => PropKind::PalmTree,
        Tree::Eucalyptus | Tree::Pine | Tree::Cypress => PropKind::Eucalyptus,
        _ => PropKind::Oak,
    }
}

/// How far a tree's trunk keeps from walls so its crown clears them, m.
fn tree_clearance(kind: PropKind) -> f32 {
    match kind {
        PropKind::Oak => 3.0,
        PropKind::Eucalyptus => 2.5,
        _ => 1.5,
    }
}

fn prop(p: V2, y: f32, facing: V2, kind: PropKind) -> Prop {
    Prop {
        pos: [p.x, y, p.y],
        yaw: yaw_of_dir(facing),
        kind,
    }
}

impl<'a> Detailer<'a> {
    fn new(
        place: &'a Place,
        plan: &'a ChunkPlan,
        template: &'a RegionTemplate,
        lots: &'a LotRules,
        seed: Seed,
    ) -> Detailer<'a> {
        let net = &plan.net;
        let grid = &plan.terrain.as_ref().expect("a terrain place").0;
        let origin = V2::from_array(place.header.origin_m()) - V2::new(64.0, 64.0);
        let extent = V2::from_array(place.header.extent_m()) + V2::new(128.0, 128.0);
        let cell = 32.0;
        let mut segs = Vec::new();
        let mut seg_bins = Bins::new(origin, extent, cell);
        for (i, e) in place.roads.edges.iter().enumerate() {
            let (end, _) = net.ends(i);
            let walk = if e.sidewalk {
                end.hw + SIDEWALK_WIDTH_M
            } else {
                end.hw + 0.2
            };
            // The layout centerline the road meshes are laid along.
            let raw = &net.paths[i].pts;
            for w in raw.windows(2) {
                let (lo, hi) = bounds(&[w[0], w[1]]);
                let r = V2::new(walk + MAX_REACH_M, walk + MAX_REACH_M);
                seg_bins.insert(lo - r, hi + r, segs.len() as u32);
                segs.push(Seg {
                    a: w[0],
                    b: w[1],
                    hw: end.hw,
                    walk,
                    edge: i as u32,
                });
            }
        }
        let mut fans = Vec::new();
        let mut fan_bins = Bins::new(origin, extent, cell);
        let mut corners = Vec::new();
        let mut corner_bins = Bins::new(origin, extent, cell);
        for j in &net.junctions {
            if j.ends.len() < 2 {
                continue;
            }
            for i in 0..j.corners.len() {
                let walk: Vec<V2> = net.corner_walk(j, i).iter().map(|p| p.xz()).collect();
                if walk.len() >= 3 {
                    let (lo, hi) = bounds(&walk);
                    let r = V2::new(MAX_REACH_M, MAX_REACH_M);
                    corner_bins.insert(lo - r, hi + r, corners.len() as u32);
                    corners.push((walk, lo, hi));
                }
            }
            let ring: Vec<V2> = net.junction_ring(j).iter().map(|p| p.xz()).collect();
            if ring.len() < 3 {
                continue;
            }
            let (lo, hi) = bounds(&ring);
            let r = V2::new(MAX_REACH_M, MAX_REACH_M);
            fan_bins.insert(lo - r, hi + r, fans.len() as u32);
            fans.push((ring, lo, hi));
        }
        let mut lot_quads = Vec::new();
        let mut lot_bins = Bins::new(origin, extent, cell);
        for lot in plan.lots() {
            let ramps = lot.ramps.iter().map(|r| &r.q);
            let islands = lot.islands.iter().map(|i| &i.q);
            for q in lot
                .pave
                .iter()
                .chain(&lot.walks)
                .chain(ramps)
                .chain(islands)
            {
                let (lo, hi) = bounds(q);
                let r = V2::new(MAX_REACH_M, MAX_REACH_M);
                lot_bins.insert(lo - r, hi + r, lot_quads.len() as u32);
                lot_quads.push((*q, lo, hi));
            }
        }
        let mut footprints = Vec::new();
        let mut fp_bins = Bins::new(origin, extent, cell);
        for b in &place.buildings {
            let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
            let (lo, hi) = bounds(&fp);
            let r = V2::new(MAX_REACH_M, MAX_REACH_M);
            fp_bins.insert(lo - r, hi + r, footprints.len() as u32);
            footprints.push((fp, lo, hi));
        }
        Detailer {
            place,
            net,
            side_zone: &plan.side_zone,
            plans: &plan.plans,
            footings: &plan.footings,
            grid,
            template,
            lots,
            lot_plan: plan.lots(),
            lot_quads,
            lot_bins,
            seed: seed.0,
            segs,
            seg_bins,
            fans,
            fan_bins,
            corners,
            corner_bins,
            footprints,
            fp_bins,
            props: Vec::new(),
            cars: Vec::new(),
        }
    }

    fn rng(&self, part: u64, item: u64) -> Rng {
        Rng::new(Seed(self.seed), &[stage::DETAIL, part, item])
    }

    /// The zone under ground point `p`.
    fn zone_at(&self, p: V2) -> ZoneId {
        crate::zone_at(self.place, p)
    }

    fn ground(&self, p: V2) -> f32 {
        self.grid.height_at(p.x, p.y)
    }

    /// Whether `p` is over water.
    fn wet(&self, p: V2) -> bool {
        self.zone_at(p) == ZoneId::Water || self.ground(p) < self.grid.water_level + 0.3
    }

    /// Whether a disc of radius `r` at `p` touches any road's asphalt or junction fan.
    fn on_asphalt(&self, p: V2, r: f32) -> bool {
        let hit_seg = self.seg_bins.at(p).iter().any(|&k| {
            let s = &self.segs[k as usize];
            segment_distance(p, s.a, s.b).0 < s.hw + r
        });
        hit_seg
            || self.fan_bins.at(p).iter().any(|&k| {
                let (poly, lo, hi) = &self.fans[k as usize];
                p.x >= lo.x - r
                    && p.x <= hi.x + r
                    && p.y >= lo.y - r
                    && p.y <= hi.y + r
                    && polygon_distance(p, poly) < r
            })
    }

    /// Whether `p` lies within `r` of the walk (or curb top) of any road but `edge`: where
    /// two roads' walks overlap their surfaces differ.
    fn on_other_walk(&self, p: V2, edge: usize, r: f32) -> bool {
        self.seg_bins.at(p).iter().any(|&k| {
            let s = &self.segs[k as usize];
            s.edge as usize != edge && segment_distance(p, s.a, s.b).0 < s.walk + r
        })
    }

    /// Whether a disc of radius `r` at `p` touches the asphalt of any road but `edge`, or
    /// a junction fan.
    fn on_other_road(&self, p: V2, edge: usize, r: f32) -> bool {
        let other = self.seg_bins.at(p).iter().any(|&k| {
            let s = &self.segs[k as usize];
            s.edge as usize != edge && segment_distance(p, s.a, s.b).0 < s.hw + r
        });
        other
            || self.fan_bins.at(p).iter().any(|&k| {
                let (poly, lo, hi) = &self.fans[k as usize];
                p.x >= lo.x - r
                    && p.x <= hi.x + r
                    && p.y >= lo.y - r
                    && p.y <= hi.y + r
                    && polygon_distance(p, poly) < r
            })
    }

    /// Whether `p` lies within `r` of any road's walk (or curb top, or a junction's corner
    /// walk), or on its asphalt.
    fn near_walk(&self, p: V2, r: f32) -> bool {
        let corner = |k: &u32| {
            let (poly, lo, hi) = &self.corners[*k as usize];
            p.x >= lo.x - r
                && p.x <= hi.x + r
                && p.y >= lo.y - r
                && p.y <= hi.y + r
                && polygon_distance(p, poly) < r
        };
        self.seg_bins.at(p).iter().any(|&k| {
            let s = &self.segs[k as usize];
            segment_distance(p, s.a, s.b).0 < s.walk + r
        }) || self.corner_bins.at(p).iter().any(corner)
            || self.on_asphalt(p, r)
    }

    /// Whether `p` lies within `r` of a building footprint.
    fn near_building(&self, p: V2, r: f32) -> bool {
        self.fp_bins.at(p).iter().any(|&k| {
            let (fp, lo, hi) = &self.footprints[k as usize];
            p.x >= lo.x - r
                && p.x <= hi.x + r
                && p.y >= lo.y - r
                && p.y <= hi.y + r
                && polygon_distance(p, fp) < r
        })
    }

    /// The height of the surface at `(s, t)` of edge `e`: the walk's top on the walk and
    /// curb, the graded ground beyond.
    fn side_height(&self, e: usize, path: &EdgePath, q: V2, t: f32) -> f32 {
        let edge = &self.place.roads.edges[e];
        let (end, _) = self.net.ends(e);
        let walk = if edge.sidewalk {
            end.hw + SIDEWALK_WIDTH_M
        } else {
            end.hw + 0.2
        };
        if t.abs() <= walk {
            path.y_at(q) + CURB_M
        } else {
            self.ground(q)
        }
    }

    /// The point `s` along the road of end `slot` of junction `ji` (from the junction) and
    /// `t` to the left of the way out of it, on the road's own centerline (so it follows a
    /// curve); its surface height and the way out there. `None` past the road's middle.
    fn along_end(&self, ji: usize, slot: usize, s: f32, t: f32) -> Option<(V2, f32, V2)> {
        let e = &self.net.junctions[ji].ends[slot];
        let path = &self.net.paths[e.edge];
        let len = path.len();
        if s >= len * 0.5 {
            return None;
        }
        let from_a = self.net.edge_ends[e.edge][0] == (ji, slot);
        let (sp, tp) = if from_a { (s, t) } else { (len - s, -t) };
        let q = path.xz(sp, tp);
        let along = path.left(sp).right();
        let out = if from_a { along } else { -along };
        // Where another road's walk or a corner's overlaps, the surface is ambiguous.
        let in_corner = self.corner_bins.at(q).iter().any(|&k| {
            let (poly, lo, hi) = &self.corners[k as usize];
            q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y && point_in_polygon(q, poly)
        });
        if in_corner || self.on_other_walk(q, e.edge, 0.2) {
            return None;
        }
        Some((q, self.side_height(e.edge, path, q, t), out))
    }

    /// Whether a disc of radius `r` at `p` touches a parking lot's ground.
    fn in_lot(&self, p: V2, r: f32) -> bool {
        self.lot_bins.at(p).iter().any(|&k| {
            let (q, lo, hi) = &self.lot_quads[k as usize];
            p.x >= lo.x - r
                && p.x <= hi.x + r
                && p.y >= lo.y - r
                && p.y <= hi.y + r
                && polygon_distance(p, q) < r
        })
    }

    /// Whether `s` along edge `edge`'s `left` (else right) side lies within `margin` of a
    /// curb cut.
    fn in_cut(&self, edge: usize, left: bool, s: f32, margin: f32) -> bool {
        self.net.cuts.iter().any(|c| {
            c.edge as usize == edge && c.left == left && s > c.s0 - margin && s < c.s1 + margin
        })
    }

    /// Adds a prop that stands off the asphalt (by `r`), clear of buildings (by `clear`)
    /// and off the parking lots; returns whether it was added.
    fn add(&mut self, p: Prop, r: f32, clear: f32) -> bool {
        let q = V2::new(p.pos[0], p.pos[2]);
        if self.on_asphalt(q, r) || self.near_building(q, clear) || self.in_lot(q, r) {
            return false;
        }
        self.props.push(p);
        true
    }

    /// Adds a tree off the walks and clear of buildings by its crown, unless the ground is
    /// wet; `planted`: the edge of a street tree in a parkway or sidewalk well, which may
    /// stand by that edge's walk (but no other's).
    fn add_tree(&mut self, p: V2, y: f32, kind: PropKind, planted: Option<usize>) -> bool {
        let walk_ok = match planted {
            Some(edge) => !self.seg_bins.at(p).iter().any(|&k| {
                let s = &self.segs[k as usize];
                s.edge as usize != edge && segment_distance(p, s.a, s.b).0 < s.walk + 1.0
            }),
            None => !self.near_walk(p, 1.0),
        };
        if !walk_ok || self.wet(p) {
            return false;
        }
        self.add(
            prop(p, y, V2::new(0.0, -1.0), kind),
            0.6,
            tree_clearance(kind),
        )
    }

    /// A tree species from zone `z`'s weights.
    fn species(&self, z: ZoneId, rng: &mut Rng) -> Option<PropKind> {
        let rule = self.template.zones.get(z)?;
        let total: f32 = rule.trees.iter().map(|t| t.1).sum();
        if total <= 0.0 {
            return None;
        }
        let mut x = rng.range_f32(0.0, total);
        for &(t, w) in &rule.trees {
            if x < w {
                return Some(tree_kind(t));
            }
            x -= w;
        }
        rule.trees.last().map(|t| tree_kind(t.0))
    }

    /// Lamps, trees, hydrants, benches, power poles and parked cars along both sides of
    /// every road.
    fn streets(&mut self) {
        let (place, net) = (self.place, self.net);
        let edges = &place.roads.edges;
        for (i, e) in edges.iter().enumerate() {
            let path = &net.paths[i];
            let (ea, _) = net.ends(i);
            let hw = ea.hw;
            let sw = ea.sw;
            let mut rng = self.rng(PART_SIDE, i as u64);
            let spans = net.side_spans(i);
            for (k, spans) in spans.iter().enumerate() {
                let sigma = if k == 0 { 1.0f32 } else { -1.0 };
                for &[s0, s1] in spans {
                    if s1 - s0 < 12.0 {
                        continue;
                    }
                    let mid = path.xz((s0 + s1) * 0.5, sigma * (hw + sw + 4.0));
                    let zone = self.side_zone[i][k].unwrap_or_else(|| self.zone_at(mid));
                    self.side(i, e.class, path, sigma, s0, s1, zone, &mut rng);
                }
            }
        }
    }

    /// One side (`sigma` 1 left, -1 right of the edge's `a` to `b`) of edge `i` from `s0`
    /// to `s1`.
    #[allow(clippy::too_many_arguments)]
    fn side(
        &mut self,
        i: usize,
        class: RoadClass,
        path: &EdgePath,
        sigma: f32,
        s0: f32,
        s1: f32,
        zone: ZoneId,
        rng: &mut Rng,
    ) {
        let Some(rule) = self.template.zones.get(zone) else {
            return;
        };
        let rule = rule.props;
        let (ea, eb) = self.net.ends(i);
        let (hw, sw) = (ea.hw, ea.sw);
        let walk = sw > 0.0;
        let left = sigma > 0.0;
        // Furniture taken on this side: (s, t, radius).
        let mut taken: Vec<(f32, f32, f32)> = Vec::new();
        let free = |taken: &[(f32, f32, f32)], s: f32, t: f32, r: f32| {
            taken.iter().all(|&(ts, tt, tr)| {
                let (ds, dt) = (ts - s, tt - t);
                ds * ds + dt * dt > (tr + r) * (tr + r)
            })
        };
        let at = |s: f32, t: f32| path.xz(s, sigma * t);
        let toward_road = |s: f32| path.left(s) * -sigma;
        let along = |s: f32| path.left(s).right();
        // Behind the walk (or the curb top) on the ground.
        let back = if walk { hw + sw } else { hw + 0.2 };
        let hills = zone == ZoneId::Hills;

        // Street lights.
        let spacing = match class {
            RoadClass::Arterial | RoadClass::Beachfront if !hills => {
                (rule.street_light_m > 0.0).then(|| rule.street_light_m.clamp(30.0, 40.0))
            }
            RoadClass::Collector if !hills => (rule.street_light_m > 0.0)
                .then(|| rule.street_light_m.max(45.0))
                .filter(|_| i.is_multiple_of(2) == left),
            RoadClass::Local if !hills => (rule.street_light_m > 0.0)
                .then(|| rule.street_light_m.max(60.0))
                .filter(|_| i.is_multiple_of(2) == left),
            _ => None,
        };
        if let Some(spacing) = spacing {
            let span = s1 - s0 - 12.0;
            let count = ((span / spacing) as u32).max(1);
            let step = span / count as f32;
            let staggered = matches!(class, RoadClass::Arterial | RoadClass::Beachfront);
            let phase = if staggered && !left { 0.5 } else { 0.0 };
            let t = if walk { hw + LAMP_IN_M } else { hw + 1.2 };
            for c in 0..=count {
                let s = s0 + 6.0 + step * (c as f32 + phase);
                if s > s1 - 6.0 {
                    break;
                }
                let q = at(s, t);
                let y = self.side_height(i, path, q, t);
                let p = prop(q, y, toward_road(s), PropKind::StreetLight);
                let cut = self.in_cut(i, left, s, CUT_CLEAR_M);
                if !cut && !self.on_other_walk(q, i, 0.3) && self.add(p, 0.2, 0.3) {
                    taken.push((s, t, 1.0));
                }
            }
        }

        // Hydrants near the start of the side, then at the zone's spacing.
        if rule.hydrant_m > 0.0
            && ((i + usize::from(left)).is_multiple_of(2) || class == RoadClass::Arterial)
        {
            let t = if walk { hw + 0.35 } else { hw + 1.0 };
            let mut s = s0 + 3.5;
            while s < s1 - 3.0 {
                if free(&taken, s, t, 0.6) && !self.in_cut(i, left, s, CUT_CLEAR_M) {
                    let q = at(s, t);
                    let y = self.side_height(i, path, q, t);
                    let p = prop(q, y, toward_road(s), PropKind::Hydrant);
                    if !self.on_other_walk(q, i, 0.3) && self.add(p, 0.25, 0.3) {
                        taken.push((s, t, 0.6));
                    }
                }
                s += rule.hydrant_m;
            }
        }

        // Benches with a trash can beside each, by the zone's density; more cans on the
        // busy zones' walks.
        if walk && rule.bench_per_km > 0.0 {
            let slot = 50.0;
            let p_bench = (rule.bench_per_km * slot / 1000.0).min(0.9);
            let mut s = s0 + rng.range_f32(8.0, 20.0);
            while s < s1 - 6.0 {
                let roll = rng.next_f32();
                let busy = matches!(zone, ZoneId::Strip | ZoneId::Downtown | ZoneId::Beach);
                let tb = hw + sw - 0.55;
                if roll < p_bench
                    && free(&taken, s, tb, 1.2)
                    && free(&taken, s + 1.6, hw + 0.6, 0.5)
                    && !self.in_cut(i, left, s, CUT_CLEAR_M + 1.0)
                {
                    let q = at(s, tb);
                    let y = self.side_height(i, path, q, tb);
                    let p = prop(q, y, toward_road(s), PropKind::Bench);
                    if !self.on_other_walk(q, i, 0.5) && self.add(p, 0.5, 0.3) {
                        taken.push((s, tb, 1.2));
                    }
                }
                if roll < p_bench || (busy && roll < p_bench + 0.25) {
                    let tc = hw + 0.6;
                    let sc = s + 1.6;
                    if free(&taken, sc, tc, 0.5) && !self.in_cut(i, left, sc, CUT_CLEAR_M) {
                        let q = at(sc, tc);
                        let y = self.side_height(i, path, q, tc);
                        let p = prop(q, y, toward_road(sc), PropKind::TrashCan);
                        if !self.on_other_walk(q, i, 0.35) && self.add(p, 0.35, 0.3) {
                            taken.push((sc, tc, 0.5));
                        }
                    }
                }
                s += slot;
            }
        }

        // Street trees.
        if rule.street_tree_m > 0.0 && class != RoadClass::Alley && class != RoadClass::Highway {
            let parkway = crate::roads::side_has_parkway(class, Some(zone));
            let wells =
                walk && matches!(zone, ZoneId::Strip | ZoneId::Downtown | ZoneId::Commercial);
            let (t, planted) = if parkway {
                (hw + 0.2 + 0.5, true)
            } else if wells {
                (hw + 0.95, true)
            } else {
                (back + rng.range_f32(1.2, 2.5), false)
            };
            let mut s = s0 + rng.range_f32(4.0, rule.street_tree_m.max(5.0));
            while s < s1 - 4.0 {
                if let Some(kind) = self.species(zone, rng) {
                    let r = if planted { 2.5 } else { 1.0 };
                    if free(&taken, s, t, r) && !self.in_cut(i, left, s, CUT_CLEAR_M + 1.0) {
                        let q = at(s, t);
                        let y = self.side_height(i, path, q, t);
                        if self.add_tree(q, y, kind, planted.then_some(i)) {
                            taken.push((s, t, 1.0));
                        }
                    }
                }
                s += rule.street_tree_m * rng.range_f32(0.85, 1.2);
            }
        }

        // Power poles behind the walk, along the line.
        let poles = rule.power_pole_m > 0.0
            && matches!(
                class,
                RoadClass::Local | RoadClass::Rural | RoadClass::Collector
            )
            && i.is_multiple_of(2) != left;
        if poles {
            let t = back + if walk { 0.6 } else { 2.3 };
            let mut s = s0 + rng.range_f32(3.0, 12.0);
            while s < s1 - 3.0 {
                if free(&taken, s, t, 1.0) && !self.in_cut(i, left, s, CUT_CLEAR_M) {
                    let q = at(s, t);
                    let y = self.ground(q);
                    let clear = !self.wet(q) && !self.near_walk(q, 0.2);
                    if clear && self.add(prop(q, y, along(s), PropKind::PowerPole), 0.3, 0.6) {
                        taken.push((s, t, 1.0));
                    }
                }
                s += rule.power_pole_m;
            }
        }

        // Farm windbreaks: a row of gum trees along some section roads.
        if zone == ZoneId::Farmland && class == RoadClass::Rural && rng.range(0, 100) < 35 {
            let t = back + rng.range_f32(4.0, 6.0);
            let mut s = s0 + rng.range_f32(2.0, 8.0);
            while s < s1 - 2.0 {
                let q = at(s, t + rng.range_f32(-0.5, 0.5));
                let y = self.ground(q);
                self.add_tree(q, y, PropKind::Eucalyptus, None);
                s += rng.range_f32(7.0, 10.0);
            }
        }

        // Parked cars along local curbs, facing the way traffic runs on this side.
        let parks = matches!(
            class,
            RoadClass::Local | RoadClass::Collector | RoadClass::Beachfront
        );
        let (lo, hi) = rule.parked_cars;
        if parks && hi > 0.0 {
            let share = (lo + (hi - lo) * rng.next_f32()).clamp(0.0, 0.8);
            // A car takes 5 m and a gap of 1–3 m; an empty slot 6 m.
            let p_car = (6.0 * share / (5.0 - share)).min(1.0);
            let t = hw - PARK_IN_M;
            let facing_sign = -sigma;
            let (start, end) = (ea.trim.max(s0) + 8.0, (path.len() - eb.trim).min(s1) - 8.0);
            let mut s = start;
            while s + 5.0 < end {
                if rng.next_f32() < p_car {
                    let class_roll = rng.range(0, 100);
                    let c = s + 2.5;
                    let near_hydrant = taken
                        .iter()
                        .any(|&(ts, tt, _)| (ts - c).abs() < 4.0 && tt < hw + 0.5);
                    let q = at(c, t);
                    let cut = self.in_cut(i, left, c, 2.5 + CUT_CLEAR_M);
                    if !near_hydrant && !cut && !self.on_other_road(q, i, 1.2) {
                        let facing = path.left(c).right() * facing_sign;
                        self.car(q, path.y_at(q), facing, ALL_HOURS, class_roll, rng);
                    }
                    s += 5.0 + rng.range_f32(1.0, 3.0);
                } else {
                    s += 6.0;
                }
            }
        }
    }

    /// A parked car candidate at `q` facing `facing`, parked through the hours of
    /// `hours`, its body picked by `class_roll` (0..100) and its paint by `rng`.
    #[allow(clippy::too_many_arguments)]
    fn car(&mut self, q: V2, y: f32, facing: V2, hours: u32, class_roll: u32, rng: &mut Rng) {
        // Mostly sedans and wagons (the long body), a third sporty coupes.
        let class: u8 = match class_roll {
            0..=19 => 0,  // Sports
            20..=33 => 3, // Coupe
            34..=59 => 1, // Sedan
            60..=84 => 2, // Wagon
            _ => 8,       // Luxury
        };
        let paint = rng.range(0, 16) as u8;
        self.cars.push(prop(
            q,
            y,
            facing,
            PropKind::ParkedCar {
                vehicle_class: class | paint << 4,
                hours,
            },
        ));
    }

    /// Signals, stop signs, street-name posts and lone lamps at junctions.
    fn junctions(&mut self) {
        let (place, net) = (self.place, self.net);
        let edges = &place.roads.edges;
        let rank = |c: RoadClass| match c {
            RoadClass::Highway => 5,
            RoadClass::Arterial | RoadClass::Beachfront => 4,
            RoadClass::Collector => 3,
            RoadClass::Local | RoadClass::Rural => 2,
            RoadClass::Alley => 1,
        };
        for (ji, j) in net.junctions.iter().enumerate() {
            let n = j.ends.len();
            if n < 3 || j.ends.iter().any(|e| e.class == RoadClass::Highway) {
                continue;
            }
            let mut rng = self.rng(PART_JUNCTION, ji as u64);
            let major = j
                .ends
                .iter()
                .filter(|e| matches!(e.class, RoadClass::Arterial | RoadClass::Beachfront))
                .count();
            let collectors = j
                .ends
                .iter()
                .filter(|e| e.class == RoadClass::Collector)
                .count();
            let signals = major >= 2 || (major == 1 && collectors >= 1 && n == 4);
            let top = j.ends.iter().map(|e| rank(e.class)).max().unwrap_or(0);
            let equal = j.ends.iter().all(|e| rank(e.class) == top);
            // A T between equals stops its stem: the end most across the other two.
            let stem = (0..n)
                .map(|k| {
                    let d = j.ends[k].dir;
                    let across: f32 = (0..n)
                        .filter(|&o| o != k)
                        .map(|o| d.dot(j.ends[o].dir).abs())
                        .sum();
                    (k, across)
                })
                .fold((0, f32::MAX), |a, b| if b.1 < a.1 { b } else { a })
                .0;
            for (k, e) in j.ends.iter().enumerate() {
                if e.class == RoadClass::Alley {
                    continue;
                }
                let has_walk = edges[e.edge].sidewalk;
                // The corner on the approach's right as drivers come in: the left of the
                // way out.
                let lat = if has_walk { e.hw + 0.8 } else { e.hw + 1.2 };
                if signals {
                    let s = (e.trim + 1.2).max(e.s_left + 0.6);
                    if let Some((q, y, out)) = self.along_end(ji, k, s, lat) {
                        let arm_dm = ((e.hw * 0.85) * 10.0).min(255.0) as u8;
                        let arm = out.right();
                        self.add(prop(q, y, arm, PropKind::Signal { arm_dm }), 0.2, 0.3);
                    }
                } else {
                    let stops = rank(e.class) < top || (equal && (n == 4 || k == stem));
                    if stops {
                        let s = (e.trim + 1.0).max(e.s_left + 0.5);
                        if let Some((q, y, out)) = self.along_end(ji, k, s, lat - 0.2) {
                            self.add(prop(q, y, out, PropKind::StopSign), 0.15, 0.3);
                        }
                    }
                }
            }
            // A street-name post on the far corner of the first road with a walk.
            if let Some(k) = j.ends.iter().position(|e| edges[e.edge].sidewalk) {
                let e = &j.ends[k];
                let s = (e.trim + 1.6).max(e.s_right + 0.8);
                let text_seed = rng.next_u32() as u64 | (u64::from(rng.next_u32()) << 32);
                if let Some((q, y, out)) = self.along_end(ji, k, s, -(e.hw + 0.6)) {
                    self.add(prop(q, y, out, PropKind::Sign { text_seed }), 0.15, 0.3);
                }
            }
            // Unlit roads get one lamp at their junctions.
            let unlit = j.ends.iter().all(|e| {
                let zone = self.zone_at(j.pos + e.dir * (e.trim + 10.0));
                let s = self
                    .template
                    .zones
                    .get(zone)
                    .map_or(0.0, |r| r.props.street_light_m);
                e.class == RoadClass::Rural || zone == ZoneId::Hills || s <= 0.0
            });
            if unlit {
                let k = (0..n).max_by_key(|&k| rank(j.ends[k].class)).unwrap_or(0);
                let e = &j.ends[k];
                let has_walk = edges[e.edge].sidewalk;
                let lat = if has_walk {
                    e.hw + LAMP_IN_M
                } else {
                    e.hw + 1.2
                };
                let s = (e.trim + 3.0).max(e.s_left + 1.0);
                if let Some((q, y, _)) = self.along_end(ji, k, s, lat) {
                    let toward = (j.pos - q).normalize();
                    self.add(prop(q, y, toward, PropKind::StreetLight), 0.2, 0.3);
                }
            }
        }
    }

    /// Each house site's yard tree and driveway car, and a mailbox at the curb in the
    /// suburbs, hills and farms.
    fn houses(&mut self) {
        let pal = Palette::socal_1985();
        let mut scratch = WorldMesh::default();
        let (place, plans) = (self.place, self.plans);
        for (i, b) in place.buildings.iter().enumerate() {
            let (env, plan) = &plans[i];
            scratch.vertices.clear();
            scratch.indices.clear();
            let site = buildings::house_site(&mut scratch, b, plan, env, &pal, self.seed, i as u32);
            let base = self.footings[i].0;
            let mut rng = self.rng(PART_HOUSE, i as u64);
            for p in site {
                match p.kind {
                    props::PropKind::ParkedCar { .. } => {
                        // On the drive slab or the ground, whichever is higher.
                        let y = (base + p.y).max(self.ground(p.pos));
                        let roll = rng.range(0, 100);
                        let q = p.pos;
                        if !self.near_building(q, 0.3) && !self.on_asphalt(q, 1.0) {
                            self.car(q, y, p.dir, ALL_HOURS, roll, &mut rng);
                        }
                    }
                    props::PropKind::PalmTree | props::PropKind::Oak => {
                        let kind = if p.kind == props::PropKind::PalmTree {
                            PropKind::PalmTree
                        } else {
                            PropKind::Oak
                        };
                        let y = self.ground(p.pos);
                        self.add_tree(p.pos, y, kind, None);
                    }
                    _ => {}
                }
            }
            // A mailbox at the front property line.
            let fi = plan.frontage;
            let fpl = &plan.facades[fi];
            let yard = env[fi].yard;
            let zone = self.zone_at(V2::from_array(b.footprint[0]));
            let rural = matches!(zone, ZoneId::Suburb | ZoneId::Hills | ZoneId::Farmland);
            if b.usage == BuildingUse::Residential
                && rural
                && fpl.columns > 0
                && (2.0..40.0).contains(&yard)
            {
                let n = b.footprint.len();
                let a = V2::from_array(b.footprint[fi]);
                let c = V2::from_array(b.footprint[(fi + 1) % n]);
                let out = (c - a).normalize().right();
                let x = rng.range_f32(0.2, 0.8);
                let q = a.lerp(c, x) + out * (yard - 0.5);
                let y = self.ground(q);
                let p = prop(q, y, out, PropKind::Mailbox);
                if !self.near_walk(q, 0.1) {
                    self.add(p, 0.4, 0.5);
                }
            }
        }
    }

    /// Hedges and fences along the side and back lines of suburban house lots.
    fn lot_lines(&mut self) {
        let place = self.place;
        let mut seen: BTreeSet<(i32, i32)> = BTreeSet::new();
        let polys: Vec<(Vec<V2>, V2, V2)> = place
            .parcels
            .iter()
            .map(|p| {
                let poly = crate::ground::to_v2(&p.polygon);
                let (lo, hi) = bounds(&poly);
                (poly, lo, hi)
            })
            .collect();
        let mut has_house = alloc::vec![false; place.parcels.len()];
        for b in &place.buildings {
            if b.usage != BuildingUse::Residential {
                continue;
            }
            let c = crate::ground::centroid(&crate::ground::to_v2(&b.footprint));
            let hit = polys.iter().position(|(poly, lo, hi)| {
                c.x >= lo.x
                    && c.x <= hi.x
                    && c.y >= lo.y
                    && c.y <= hi.y
                    && point_in_polygon(c, poly)
            });
            if let Some(k) = hit {
                has_house[k] = true;
            }
        }
        for (pi, parcel) in place.parcels.iter().enumerate() {
            if parcel.zone != ZoneId::Suburb || parcel.lot_kind != LotKind::Lot || !has_house[pi] {
                continue;
            }
            let mut rng = self.rng(PART_LOT_LINE, pi as u64);
            let kind_hedge = rng.range(0, 100) < 40;
            let poly = &polys[pi].0;
            let n = poly.len();
            for k in 0..n {
                let (a, b) = (poly[k], poly[(k + 1) % n]);
                let len = (b - a).length();
                if len < 4.0 {
                    continue;
                }
                let mid = (a + b) * 0.5;
                // Street sides (fronts, a corner lot's flank) stay open.
                if self.near_walk(mid, 4.0) {
                    continue;
                }
                let key = ((mid.x * 2.0) as i32, (mid.y * 2.0) as i32);
                if !seen.insert(key) || rng.range(0, 100) < 25 {
                    continue;
                }
                let dir = (b - a) * (1.0 / len);
                let pieces = ((len / RUN_M) as u32 + 1).max(1);
                let piece = len / pieces as f32;
                for q in 0..pieces {
                    let c = a + dir * (piece * (q as f32 + 0.5));
                    // Clear of walls and of the walks at both ends.
                    let ends = [c - dir * (piece * 0.5), c, c + dir * (piece * 0.5)];
                    if ends
                        .iter()
                        .any(|&e| self.near_building(e, 0.8) || self.near_walk(e, 3.0))
                    {
                        continue;
                    }
                    let len_dm = ((piece - 0.1) * 10.0) as u16;
                    let kind = if kind_hedge {
                        PropKind::Hedge { len_dm }
                    } else {
                        PropKind::Fence { len_dm }
                    };
                    let y = self.ground(c);
                    self.props.push(prop(c, y, dir, kind));
                }
            }
        }
    }

    /// Neon blade signs on the zone's share of storefront buildings.
    fn neon_signs(&mut self) {
        let neon_count = Palette::socal_1985().neon().len() as u32;
        let (place, plans) = (self.place, self.plans);
        for (i, b) in place.buildings.iter().enumerate() {
            let (_, plan) = &plans[i];
            let fi = plan.frontage;
            let fpl = &plan.facades[fi];
            if !fpl.storefront && b.style != StyleTag::Motel && b.style != StyleTag::StripMall {
                continue;
            }
            if plan.top < 6.0 || fpl.len < 6.0 {
                continue;
            }
            let fp = &b.footprint;
            let n = fp.len();
            let a = V2::from_array(fp[fi]);
            let c = V2::from_array(fp[(fi + 1) % n]);
            let zone = self.zone_at((a + c) * 0.5);
            let share = self.template.zones.get(zone).map_or(0.0, |r| r.props.neon);
            let mut rng = self.rng(PART_NEON, i as u64);
            if rng.next_f32() >= share {
                continue;
            }
            let out = (c - a).normalize().right();
            let x = if rng.range(0, 2) == 0 {
                0.9
            } else {
                fpl.len - 0.9
            };
            let q = a + (c - a).normalize() * x + out * 0.02;
            let y = self.footings[i].0 + crate::LOT_Y;
            let palette = rng.range(0, neon_count) as u8;
            // The sign reaches out over the walk: keep its box off the asphalt.
            let tip = q + out * NEON_SIGN_OUT.1;
            if !self.on_asphalt(tip, 0.3) {
                self.props
                    .push(prop(q, y, out, PropKind::NeonSign { palette }));
            }
        }
    }

    /// The parking lots' props: light poles, island palms, frontage hedges, and
    /// the cars in their stalls, each parked through the hours its lot's occupancy and its
    /// stall's threshold give ([`lots::stall_hours`]), nosed in, a few backed in.
    fn lots(&mut self) {
        let plan = self.lot_plan;
        for (li, lot) in plan.iter().enumerate() {
            let mut rng = self.rng(PART_LOTS, li as u64);
            for &(p, facing) in &lot.poles {
                let y = self.ground(p) + lots::ISLAND_H_M;
                self.props.push(prop(p, y, facing, PropKind::StreetLight));
            }
            for isl in &lot.islands {
                let y = self.ground(isl.plant_at) + lots::ISLAND_H_M;
                // Shrubs are the lot's ground mesh.
                if isl.plant == Planting::Palm {
                    let up = V2::new(0.0, -1.0);
                    self.props
                        .push(prop(isl.plant_at, y, up, PropKind::PalmTree));
                }
            }
            for h in &lot.hedges {
                let (c, d) = ((h[0] + h[1]) * 0.5, h[1] - h[0]);
                let len_dm = (d.length() * 10.0) as u16;
                let y = self.ground(c);
                self.props
                    .push(prop(c, y, d.normalize(), PropKind::Hedge { len_dm }));
            }
            for st in &lot.stalls {
                let hours = lots::stall_hours(self.lots, lot.zone, self.seed, st.c);
                let roll = rng.range(0, 100);
                let backed = rng.range(0, 256) < self.lots.backed_in_256;
                if hours == 0 {
                    continue;
                }
                let facing = if backed { -st.nose } else { st.nose };
                let y = self.ground(st.c) + lots::LOT_LIFT_M;
                self.car(st.c, y, facing, hours, roll, &mut rng);
            }
        }
    }

    /// Trees scattered on a jittered grid: groves over the hills, a few in parks, back
    /// yards and along the highway.
    fn scatter(&mut self) {
        let h = &self.place.header;
        let o = V2::from_array(h.origin_m());
        let e = V2::from_array(h.extent_m());
        let (nx, nz) = ((e.x / SCATTER_M) as u32, (e.y / SCATTER_M) as u32);
        for iz in 0..nz {
            let mut rng = self.rng(PART_SCATTER, u64::from(iz));
            for ix in 0..nx {
                let jx = rng.range_f32(0.15, 0.85);
                let jz = rng.range_f32(0.15, 0.85);
                let roll = rng.next_f32();
                let q = o + V2::new((ix as f32 + jx) * SCATTER_M, (iz as f32 + jz) * SCATTER_M);
                let zone = self.zone_at(q);
                let p = match zone {
                    ZoneId::Hills => 0.3,
                    ZoneId::Park => 0.35,
                    ZoneId::Suburb => 0.1,
                    ZoneId::Residential => 0.05,
                    ZoneId::Highway => 0.06,
                    _ => 0.0,
                };
                if roll >= p {
                    continue;
                }
                let Some(kind) = self.species(zone, &mut rng) else {
                    continue;
                };
                if self.near_walk(q, 2.0) {
                    continue;
                }
                let y = self.ground(q);
                if self.add_tree(q, y, kind, None) && zone == ZoneId::Hills {
                    // A grove: one or two more close by.
                    for _ in 0..rng.range(1, 3) {
                        let g = q + V2::new(rng.range_f32(-6.0, 6.0), rng.range_f32(-6.0, 6.0));
                        if !self.near_walk(g, 2.0) {
                            let y = self.ground(g);
                            self.add_tree(g, y, kind, None);
                        }
                    }
                }
            }
        }
    }

    /// Billboards along the highway and the arterials of zones that have them.
    fn billboards(&mut self) {
        let (place, net) = (self.place, self.net);
        let edges = &place.roads.edges;
        for (i, e) in edges.iter().enumerate() {
            let class = e.class;
            if !matches!(class, RoadClass::Highway | RoadClass::Arterial) {
                continue;
            }
            let path = &net.paths[i];
            let (ea, eb) = net.ends(i);
            let mut rng = self.rng(PART_BILLBOARD, i as u64);
            let step = 120.0;
            let mut s = ea.trim + rng.range_f32(10.0, step);
            while s < path.len() - eb.trim - 20.0 {
                let sigma = if rng.range(0, 2) == 0 { 1.0f32 } else { -1.0 };
                let roll = rng.next_f32();
                let off = if class == RoadClass::Highway {
                    ea.hw + rng.range_f32(12.0, 20.0)
                } else {
                    ea.hw + ea.sw + rng.range_f32(4.0, 8.0)
                };
                let angle = rng.range_f32(0.2, 0.5);
                let here = s;
                let q = path.xz(here, sigma * off);
                s += step * rng.range_f32(0.8, 1.2);
                // The highway's own rate along it; an arterial's by the zone it runs through.
                let zone = if class == RoadClass::Highway {
                    ZoneId::Highway
                } else {
                    self.zone_at(path.xz(here, 0.0))
                };
                let rate = self
                    .template
                    .zones
                    .get(zone)
                    .map_or(0.0, |r| r.props.billboard_per_km);
                if roll >= rate * step / 1000.0 {
                    continue;
                }
                // Facing the road, turned toward the traffic coming on its side (which keeps
                // right: on the edge's right it runs from `a` to `b`).
                let n = path.left(here) * -sigma;
                let travel = path.left(here).right() * -sigma;
                let (sa, ca) = crate::math::sin_cos(angle);
                let facing = (n * ca - travel * sa).normalize();
                let span = facing.left() * 5.4;
                let pts = [q, q + span, q - span];
                let clear = pts.iter().all(|&p| {
                    !self.near_walk(p, 2.0)
                        && !self.near_building(p, 3.0)
                        && !self.wet(p)
                        && !self.in_lot(p, 2.0)
                });
                if clear {
                    let y = self.ground(q);
                    self.props.push(prop(q, y, facing, PropKind::Billboard));
                }
            }
        }
    }

    /// The pier from its land end at `at` out along `out`, with lamps and benches.
    fn pier(&mut self, at: V2, out: V2) {
        let mut rng = self.rng(PART_PIER, 0);
        let y = self.ground(at).max(self.grid.water_level + 3.0);
        self.props.push(prop(
            at,
            y,
            out,
            PropKind::Pier {
                len_m: PIER_M as u8,
            },
        ));
        let across = out.right();
        let mut s = 12.0;
        let mut k = 0;
        while s < PIER_M + PIER_HEAD_M - 4.0 {
            let sigma = if k % 2 == 0 { 1.0 } else { -1.0 };
            let w = if s > PIER_M {
                PIER_HEAD_HALF_WIDTH_M
            } else {
                PIER_HALF_WIDTH_M
            };
            let q = at + out * s + across * (sigma * (w - 0.35));
            self.props
                .push(prop(q, y, across * -sigma, PropKind::StreetLight));
            let b = at + out * (s + 6.0) + across * (-sigma * (PIER_HALF_WIDTH_M - 0.8));
            if s + 6.0 < PIER_M && rng.range(0, 2) == 0 {
                self.props.push(prop(b, y, across * sigma, PropKind::Bench));
            }
            s += 24.0;
            k += 1;
        }
    }

    /// Keeps at most [`MAX_CARS_PER_CHUNK`] parked cars at the curbs and in driveways a
    /// chunk (where a chunk has more, each stays with an even chance, then the first ones
    /// up to the cap), and [`MAX_LOT_CARS_PER_CHUNK`] in its lots at any hour (in an order
    /// drawn from where they stand, while every hour they are parked has room).
    fn cap_cars(&mut self) {
        let h = &self.place.header;
        let slot = |p: &Prop| h.chunk_at([p.pos[0], p.pos[2]]).map(|c| h.chunk_slot(c));
        let hours = |p: &Prop| match p.kind {
            PropKind::ParkedCar { hours, .. } => hours,
            _ => 0,
        };
        let cars = core::mem::take(&mut self.cars);
        let (always, mut lots): (Vec<Prop>, Vec<Prop>) =
            cars.into_iter().partition(|p| hours(p) == ALL_HOURS);
        let mut count = alloc::vec![0usize; h.chunk_count()];
        for p in &always {
            if let Some(k) = slot(p) {
                count[k] += 1;
            }
        }
        let mut kept = alloc::vec![0usize; h.chunk_count()];
        let mut rng = self.rng(PART_CAP, 0);
        for p in always {
            let Some(k) = slot(&p) else { continue };
            let keep = count[k] <= MAX_CARS_PER_CHUNK
                || rng.range(0, count[k] as u32) < MAX_CARS_PER_CHUNK as u32;
            if keep && kept[k] < MAX_CARS_PER_CHUNK {
                kept[k] += 1;
                self.props.push(p);
            }
        }
        let mut kept = alloc::vec![[0usize; 24]; h.chunk_count()];
        let key = |p: &Prop| {
            let bits = [u64::from(p.pos[0].to_bits()), u64::from(p.pos[2].to_bits())];
            skywave_core::hash64(self.seed ^ KEY_LOT_CAR, &bits)
        };
        lots.sort_by_key(key);
        for p in lots {
            let Some(k) = slot(&p) else { continue };
            let mask = hours(&p);
            let room =
                (0..24).all(|hr| mask >> hr & 1 == 0 || kept[k][hr] < MAX_LOT_CARS_PER_CHUNK);
            if room {
                for (hr, c) in kept[k].iter_mut().enumerate() {
                    *c += (mask >> hr & 1) as usize;
                }
                self.props.push(p);
            }
        }
    }

    /// The place's lights: one in every street lamp's head, one at each neon sign (the
    /// blade signs and the clusters of neon in the buildings' meshes) and a fill in front
    /// of each lit shop.
    fn lights(&self, pal: &Palette) -> Vec<Light> {
        let lin = |c: [u8; 3]| c.map(|v| f32::from(v) / 255.0);
        let mut out = Vec::new();
        for p in &self.props {
            let w = props::from_place(p, self.seed);
            match p.kind {
                PropKind::StreetLight => {
                    let head = w.pos + w.dir * LAMP_ARM_M;
                    out.push(Light {
                        pos: [head.x, w.y + LAMP_LIGHT_Y, head.y],
                        color: lin(pal.sodium),
                        radius: STREET_RADIUS_M,
                        kind: LightKind::Street,
                    });
                }
                PropKind::NeonSign { palette } => {
                    let neons = pal.neon();
                    let c = w.pos + w.dir * ((NEON_SIGN_OUT.0 + NEON_SIGN_OUT.1) * 0.5);
                    out.push(Light {
                        pos: [c.x, w.y + (NEON_SIGN_Y.0 + NEON_SIGN_Y.1) * 0.5, c.y],
                        color: lin(neons[usize::from(palette) % neons.len()]),
                        radius: NEON_RADIUS_M,
                        kind: LightKind::Neon,
                    });
                }
                _ => {}
            }
        }
        let mut scratch = WorldMesh::default();
        for (i, b) in self.place.buildings.iter().enumerate() {
            let (_, plan) = &self.plans[i];
            let fi = plan.frontage;
            let front = &plan.facades[fi];
            let signs =
                front.storefront || matches!(b.style, StyleTag::Motel | StyleTag::StripMall);
            if !signs {
                continue;
            }
            let (base, drop) = self.footings[i];
            scratch.vertices.clear();
            scratch.indices.clear();
            buildings::build_on(&mut scratch, b, plan, pal, self.seed, i as u32, drop);
            for (c, n, rgb) in neon_clusters(&scratch) {
                let p = c + n * 1.2;
                out.push(Light {
                    pos: [p.x, p.y + base, p.z],
                    color: lin(rgb),
                    radius: NEON_RADIUS_M,
                    kind: LightKind::Neon,
                });
            }
            let lit = plan.signage.is_some_and(|s| s.shop_lit);
            if lit && front.storefront || lit && b.style == StyleTag::StripMall {
                let fp = &b.footprint;
                let a = V2::from_array(fp[fi]);
                let c = V2::from_array(fp[(fi + 1) % fp.len()]);
                // Out over the walk, so it pools on the pavement more than on the glass.
                let q = (a + c) * 0.5 + (c - a).normalize().right() * 3.0;
                out.push(Light {
                    pos: [q.x, base + plan.plinth_top + 2.6, q.y],
                    color: lin(pal.window_warm),
                    radius: WINDOW_RADIUS_M,
                    kind: LightKind::Window,
                });
            }
        }
        out
    }
}

/// The neon of a building mesh as signs: runs of neon vertices in mesh order, split where
/// a vertex lies more than a meter from the run so far. Each sign's center, outward
/// normal and color.
fn neon_clusters(m: &WorldMesh) -> Vec<(V3, V3, [u8; 3])> {
    let mut out = Vec::new();
    // Bounds, normal sum, color of the run being gathered.
    let mut run: Option<(V3, V3, V3, [u8; 3])> = None;
    let flush = |run: &mut Option<(V3, V3, V3, [u8; 3])>, out: &mut Vec<_>| {
        if let Some((lo, hi, n, rgb)) = run.take() {
            out.push(((lo + hi) * 0.5, n.normalize(), rgb));
        }
    };
    for v in &m.vertices {
        if v.emissive[3] != Glow::Neon as u8 {
            continue;
        }
        let p = V3::new(v.pos[0], v.pos[1], v.pos[2]);
        let n = V3::new(v.normal[0], v.normal[1], v.normal[2]);
        let near = run.is_some_and(|(lo, hi, _, _)| {
            let d = V3::new(
                (lo.x - p.x).max(p.x - hi.x).max(0.0),
                (lo.y - p.y).max(p.y - hi.y).max(0.0),
                (lo.z - p.z).max(p.z - hi.z).max(0.0),
            );
            d.length() <= 1.0
        });
        if !near {
            flush(&mut run, &mut out);
            run = Some((
                p,
                p,
                V3::new(0.0, 0.0, 0.0),
                [v.emissive[0], v.emissive[1], v.emissive[2]],
            ));
        }
        if let Some((lo, hi, sum, _)) = run.as_mut() {
            *lo = V3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            *hi = V3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
            *sum = *sum + n;
        }
    }
    flush(&mut run, &mut out);
    out
}
