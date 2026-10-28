//! Parking lots (B10; `docs/WORLD_GENERATION.md`, "Parking lots"): laid out by the
//! grammar's [`LotRules`] on a terrain place's parcels and along its beach alleys. One plan
//! ([`plan`]) is what the ground meshes ([`mesh`]), the curb cuts in the road meshes, the
//! detailing's props and parked cars and the collider all read.
//!
//! - **Front lots** before strip malls: the covered walk along the shops, then one or two
//!   rows of stalls either side of an aisle, 90° (two-way) or 60° (one way through), as
//!   many as the yard holds (the building planner leaves room for two), then the planted
//!   strip along the street with a hedge, a low wall or nothing. A curb cut at one end
//!   (both ends on a long frontage or an angled lot) leads up a concrete ramp from the
//!   sidewalk's back and through the street-side row to the aisle.
//! - **Rear lots** behind shops, offices and civic buildings backing onto an alley: one
//!   row of 90° stalls opening onto the alley, its curb dropped along them.
//! - **Beach lots** along the beach zone's alleys: a row of stalls each side opening onto
//!   the alley wherever the ground beside it is clear.
//! - Every row is cut into runs of at most `run_max` stalls with an island between and at
//!   both ends (a palm, a shrub or bare ground). Light poles stand in islands `pole_m`
//!   apart; a lot has wheel stops in every stall with chance `wheel_stop_256`, stripes
//!   white or yellow, painted arrows at its entries (and along a one-way aisle).
//! - Only zones the grammar gives an occupancy curve get lots.
//!
//! Everything a lot holds is laid on the graded terrain: the asphalt [`LOT_LIFT_M`] above
//! it, cut to the terrain's own triangles so it never pokes through or floats, the paint
//! a little above that; islands, wheel stops, the walk and the low wall stand on it.
//!
//! Deterministic: a parcel's lot draws from `Rng::new(seed, [LOTS, parcel key])`, a beach
//! alley's from `Rng::new(seed, [LOTS, ALLEY | edge])`, in a fixed order.

use alloc::vec;
use alloc::vec::Vec;

use place::{BuildingUse, LotKind, Place, RoadClass, StyleTag, ZoneId};
use skywave_core::{Rng, Seed};

use crate::grammar::{Frontage, LotRules, Planting};
use crate::math::{abs, point_in_polygon, segment_distance, signed_area, V2, V3};
use crate::mesh::terrain::TerrainGrid;
use crate::mesh::{boxf, face, quad, tri, Frame as MeshFrame, Paint, Pattern, WorldMesh};
use crate::palette::{mix, shade, Palette};
use crate::procgen::buildings::{clip_to_place, front_edge, parcel_key, Lot as Frame};
use crate::procgen::stage;
use crate::roads::{CurbCut, Network};
use crate::{MeshKind, MeshSet, CURB_M, PAINT_LIFT};

/// The asphalt's height above the terrain, m.
pub const LOT_LIFT_M: f32 = 0.05;
/// Height of an island's curb, m.
pub const ISLAND_H_M: f32 = 0.15;
/// Height of a wheel stop, m.
pub const STOP_H_M: f32 = 0.12;
/// Height of an island's shrub above the island, m.
const SHRUB_H_M: f32 = 0.75;
/// Height of a low wall along a lot's frontage, m.
pub const WALL_H_M: f32 = 0.75;
/// Concrete ramp from a sidewalk's back down to its lot, m.
pub const RAMP_M: f32 = 1.2;
/// The curb top strip of a road without sidewalks, m (as the road meshes lay it).
pub const CURB_TOP_M: f32 = 0.2;
/// How far a lot keeps from its parcel's side lines and from the building, m.
const EDGE_GAP_M: f32 = 0.3;
/// An island's width along its row, m.
const ISLAND_M: f32 = 1.8;
/// A wheel stop's length and its distance from the stall's head, m.
const STOP_LEN_M: f32 = 1.8;
const STOP_IN_M: f32 = 0.7;
const STOP_W_M: f32 = 0.15;
/// The narrowest frontage a lot is laid on, m.
const MIN_WIDTH_M: f32 = 12.0;
/// Painted arrow: length and the width of its head, m.
const ARROW_M: f32 = 2.4;
const ARROW_HEAD_W_M: f32 = 0.9;
/// Stripe width, m.
const STRIPE_W_M: f32 = 0.1;
/// Longest run of hedge one prop holds, m (as the detailing's).
pub const HEDGE_RUN_M: f32 = 7.0;
/// How far past an alley's curb a lot's rear line may lie for the lot to back onto it, m.
const ALLEY_REACH_M: f32 = crate::procgen::buildings::ALLEY_REACH_M;
/// How much narrower than its own width a beach stall or island may come out where its
/// alley bends, m.
const SQUEEZE_M: f32 = 0.3;
/// Clearance of a beach stall from any other road's walk or curb top, m.
const ROAD_CLEAR_M: f32 = 0.3;
const SIN60: f32 = 0.866_025_4;
const COS60: f32 = 0.5;
/// High key bit of a beach alley's draws.
const KEY_ALLEY: u64 = 1 << 40;

/// One stall: its center, the unit way a car parked nose-in faces, a point in the aisle
/// (or the alley) in front of it, where a car leaving it comes out, and its quad (the
/// aisle side first).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stall {
    pub c: V2,
    pub nose: V2,
    pub mouth: V2,
    pub q: [V2; 4],
}

/// An island at a row's end: its quad, what it holds and where, and where a light pole
/// would stand in it and which way its arm would reach (over the aisle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Island {
    pub q: [V2; 4],
    pub plant: Planting,
    pub plant_at: V2,
    /// Along the row (a shrub's run).
    pub along: V2,
    pub pole_at: V2,
    pub pole_facing: V2,
}

/// The concrete ramp from a sidewalk's back down to a lot: `q[0]`, `q[1]` on the property
/// line of road `edge` (at its walk's top), `q[2]`, `q[3]` on the lot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    pub q: [V2; 4],
    pub edge: u32,
}

/// One parking lot. Quads are convex, their corners in order round them.
#[derive(Clone, Debug, PartialEq)]
pub struct ParkingLot {
    /// The zone whose occupancy curve fills it.
    pub zone: ZoneId,
    pub yellow: bool,
    /// Asphalt.
    pub pave: Vec<[V2; 4]>,
    /// A strip mall's covered walk, raised to curb height.
    pub walks: Vec<[V2; 4]>,
    pub ramps: Vec<Ramp>,
    /// Painted lines, end to end.
    pub stripes: Vec<[V2; 2]>,
    /// Painted arrows: the middle and the unit way they point.
    pub arrows: Vec<(V2, V2)>,
    /// Wheel stops, end to end.
    pub stops: Vec<[V2; 2]>,
    pub islands: Vec<Island>,
    /// Light poles: the base and the way the arm reaches.
    pub poles: Vec<(V2, V2)>,
    /// Hedge and low wall runs along the frontage, end to end.
    pub hedges: Vec<[V2; 2]>,
    pub walls: Vec<[V2; 2]>,
    pub stalls: Vec<Stall>,
    /// The aisles, and the drives from the curb cuts to them (for the layout tests: every
    /// stall's mouth is in an aisle, or on the alley, and every aisle meets a drive).
    pub aisles: Vec<[V2; 4]>,
    pub entries: Vec<[V2; 4]>,
    /// Opens onto an alley: the stalls' mouths are on its asphalt.
    pub on_alley: bool,
    pub cuts: Vec<CurbCut>,
}

impl ParkingLot {
    fn new(zone: ZoneId, yellow: bool) -> ParkingLot {
        ParkingLot {
            zone,
            yellow,
            pave: Vec::new(),
            walks: Vec::new(),
            ramps: Vec::new(),
            stripes: Vec::new(),
            arrows: Vec::new(),
            stops: Vec::new(),
            islands: Vec::new(),
            poles: Vec::new(),
            hedges: Vec::new(),
            walls: Vec::new(),
            stalls: Vec::new(),
            aisles: Vec::new(),
            entries: Vec::new(),
            on_alley: false,
            cuts: Vec::new(),
        }
    }
}

/// A row of stalls to lay: `s_a` to `s_b` along, `depth` deep from its aisle edge, each
/// stall `module` along it; a stripe leans `slant` along the row over the row's depth (0
/// for 90° stalls).
#[derive(Clone, Copy)]
struct Row {
    s_a: f32,
    s_b: f32,
    depth: f32,
    slant: f32,
    module: f32,
}

/// Depth of a row of stalls across it, m: a 60° stall's rectangle leans over.
fn row_depth(r: &LotRules, angled: bool) -> f32 {
    if angled {
        r.stall_d * SIN60 + r.stall_w * COS60
    } else {
        r.stall_d
    }
}

fn aisle(r: &LotRules, angled: bool) -> f32 {
    if angled {
        r.aisle_w_60
    } else {
        r.aisle_w
    }
}

fn bounds(pts: &[V2]) -> (V2, V2) {
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in pts {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

fn inside_box(p: V2, (lo, hi): (V2, V2), r: f32) -> bool {
    p.x >= lo.x - r && p.x <= hi.x + r && p.y >= lo.y - r && p.y <= hi.y + r
}

/// Whether convex quad `q` and polygon `poly` (any shape) overlap.
fn overlap(q: &[V2; 4], poly: &[V2]) -> bool {
    let cross = |a: V2, b: V2, c: V2| (b - a).turn(c - a);
    let crosses = |a: V2, b: V2, c: V2, d: V2| {
        (cross(c, d, a) > 0.0) != (cross(c, d, b) > 0.0)
            && (cross(a, b, c) > 0.0) != (cross(a, b, d) > 0.0)
    };
    let n = poly.len();
    for i in 0..4 {
        for j in 0..n {
            if crosses(q[i], q[(i + 1) % 4], poly[j], poly[(j + 1) % n]) {
                return true;
            }
        }
    }
    point_in_polygon(q[0], poly) || point_in_polygon(poly[0], q)
}

/// Whether quad `q` (aisle edge `q[0]`–`q[1]`, back edge `q[3]`–`q[2]`) is at least
/// `min` wide along `along` at both edges (negative where it folds over).
fn wide(q: &[V2; 4], along: V2, min: f32) -> bool {
    (q[1] - q[0]).dot(along) >= min && (q[2] - q[3]).dot(along) >= min
}

/// Quad `q` drawn 5 cm in toward its middle, so neighbours that only touch do not overlap.
fn inset(q: &[V2; 4]) -> [V2; 4] {
    let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
    q.map(|p| p + (c - p).normalize() * 0.05)
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

/// What the planner reads.
struct Ctx<'a> {
    place: &'a Place,
    net: &'a Network,
    rules: &'a LotRules,
    seed: u64,
    /// Building footprints with their bounds and vertex means.
    fps: Vec<(Vec<V2>, (V2, V2), V2)>,
    /// Every parcel (lots, parks, empty lots) with its bounds, but the beach's own lots:
    /// the strips of sand between its alleys and roads, where its parking lots go.
    parcels: Vec<(Vec<V2>, (V2, V2))>,
    /// Junction fans and corner walks with their bounds.
    junctions: Vec<(Vec<V2>, (V2, V2))>,
    /// Each edge's centerline's bounds.
    edge_bounds: Vec<(V2, V2)>,
}

/// Every parking lot of `place` (none but on a terrain place), in a fixed order: the
/// parcels' in parcel order, then the beach alleys' in edge order.
pub fn plan(place: &Place, net: &Network, rules: &LotRules) -> Vec<ParkingLot> {
    let mut out = Vec::new();
    if !crate::is_terrain_place(place) || rules.occupancy.is_empty() {
        return out;
    }
    let fps = place
        .buildings
        .iter()
        .map(|b| {
            let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
            let bb = bounds(&fp);
            let c = crate::ground::centroid(&fp);
            (fp, bb, c)
        })
        .collect();
    let parcels = place
        .parcels
        .iter()
        .filter(|p| !(p.lot_kind == LotKind::Lot && p.zone == ZoneId::Beach))
        .map(|p| {
            let poly: Vec<V2> = p.polygon.iter().map(|&q| V2::from_array(q)).collect();
            let bb = bounds(&poly);
            (poly, bb)
        })
        .collect();
    let mut junctions = Vec::new();
    for j in &net.junctions {
        let ring: Vec<V2> = net.junction_ring(j).iter().map(|p| p.xz()).collect();
        let walks = (0..j.corners.len()).map(|i| {
            let w: Vec<V2> = net.corner_walk(j, i).iter().map(|p| p.xz()).collect();
            w
        });
        for poly in core::iter::once(ring).chain(walks) {
            if poly.len() >= 3 {
                let bb = bounds(&poly);
                junctions.push((poly, bb));
            }
        }
    }
    let ctx = Ctx {
        place,
        net,
        rules,
        seed: place.header.seed,
        fps,
        parcels,
        junctions,
        edge_bounds: net.paths.iter().map(|p| bounds(&p.pts)).collect(),
    };
    for parcel in &place.parcels {
        if let Some(l) = ctx.parcel(parcel) {
            out.push(l);
        }
    }
    for i in 0..place.roads.edges.len() {
        if let Some(l) = ctx.beach(i) {
            out.push(l);
        }
    }
    out
}

/// The curb cuts of `lots`, for [`Network::cuts`].
pub fn cuts(lots: &[ParkingLot]) -> Vec<CurbCut> {
    lots.iter().flat_map(|l| l.cuts.iter().copied()).collect()
}

/// The hour mask of a car parked in a stall at `pos` of a lot of zone `z` (see
/// `place::PropKind::ParkedCar`): the stall has its own threshold, and is taken through
/// each hour whose occupancy is above it. 0: never taken.
pub fn stall_hours(rules: &LotRules, z: ZoneId, seed: u64, pos: V2) -> u32 {
    let Some(occ) = rules.occupancy(z) else {
        return 0;
    };
    let bits = [u64::from(pos.x.to_bits()), u64::from(pos.y.to_bits())];
    let u = (skywave_core::hash64(seed ^ KEY_STALL, &bits) % 100) as u8;
    let mut hours = 0;
    for (h, &p) in occ.iter().enumerate() {
        if p > u {
            hours |= 1 << h;
        }
    }
    if hours == 0 {
        0
    } else {
        hours | LOT_CAR
    }
}

const KEY_STALL: u64 = 0x5354_414C;
/// Bit 24 of a lot car's hour mask: it is a lot's (so never [`place::ALL_HOURS`]), and
/// comes and goes with the clock even when its stall is taken all day.
pub const LOT_CAR: u32 = 1 << 24;

impl Ctx<'_> {
    /// The lot of a parcel's main building, if it gets one.
    fn parcel(&self, parcel: &place::Parcel) -> Option<ParkingLot> {
        let r = self.rules;
        if parcel.lot_kind != LotKind::Lot || parcel.polygon.len() < 3 {
            return None;
        }
        r.occupancy(parcel.zone)?;
        let poly0: Vec<V2> = parcel.polygon.iter().map(|&p| V2::from_array(p)).collect();
        let bb = bounds(&poly0);
        let (k, (fp, _, _)) = self
            .fps
            .iter()
            .enumerate()
            .find(|(_, (_, _, c))| inside_box(*c, bb, 0.0) && point_in_polygon(*c, &poly0))?;
        let b = &self.place.buildings[k];
        let front_lot = b.style == StyleTag::StripMall;
        let rear_lot = matches!(
            b.usage,
            BuildingUse::Commercial | BuildingUse::Office | BuildingUse::Civic
        ) && !matches!(b.style, StyleTag::Motel | StyleTag::Dingbat);
        if !front_lot && !rear_lot {
            return None;
        }
        // The building planner's frame of the parcel.
        let h = &self.place.header;
        let extent = [
            f32::from(h.size_chunks[0]) * h.chunk_size_m,
            f32::from(h.size_chunks[1]) * h.chunk_size_m,
        ];
        let poly = clip_to_place(poly0, extent);
        if poly.len() < 3 {
            return None;
        }
        let road = self.place.roads.edges.get(parcel.frontage_edge as usize)?;
        let line: Vec<V2> = road.profile.iter().map(|p| V2::new(p[0], p[2])).collect();
        let walk = if road.sidewalk {
            place::SIDEWALK_WIDTH_M
        } else {
            0.0
        };
        let reach = road.width_m * 0.5 + walk + 2.0;
        let f = Frame::new(&poly, front_edge(&poly, &line, reach), signed_area(&poly));
        // The building in the frame.
        let (mut bs0, mut bs1, mut bt0, mut bt1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &p in fp {
            let d = p - f.o;
            let (s, t) = (d.dot(f.u), d.dot(f.v));
            bs0 = bs0.min(s);
            bs1 = bs1.max(s);
            bt0 = bt0.min(t);
            bt1 = bt1.max(t);
        }
        let mut rng = Rng::new(Seed(self.seed), &[stage::LOTS, parcel_key(&parcel.polygon)]);
        let mut lot = ParkingLot::new(parcel.zone, rng.range(0, 256) < r.yellow_256);
        let stops = rng.range(0, 256) < r.wheel_stop_256;
        let laid = if front_lot {
            let edge = parcel.frontage_edge as usize;
            self.front(&mut lot, &mut rng, &f, [bs0, bs1, bt0], edge, stops)
        } else if road.class == RoadClass::Alley {
            self.alley_lot(&mut lot, &mut rng, &f, [bs0, bs1], 0.0, bt0, -1.0, stops)
        } else {
            let (_, rear) = f.span_t((bs0 + bs1) * 0.5, bt1 + 0.1)?;
            self.alley_lot(&mut lot, &mut rng, &f, [bs0, bs1], rear, bt1, 1.0, stops)
        };
        if !laid || lot.stalls.is_empty() {
            return None;
        }
        // Clear of every other road (a lot on a corner, an alley lot where alleys meet).
        let own = lot.on_alley.then(|| lot.cuts[0].edge as usize);
        let stalls = lot.stalls.iter().map(|s| &s.q);
        let islands = lot.islands.iter().map(|i| &i.q);
        if !stalls
            .chain(islands)
            .all(|q| self.off_roads(q, own) && self.off_junctions(q))
        {
            return None;
        }
        self.poles(&mut lot, &mut rng);
        Some(lot)
    }

    /// A strip mall's front lot between the street (`edge`) and its walk.
    fn front(
        &self,
        lot: &mut ParkingLot,
        rng: &mut Rng,
        f: &Frame,
        [bs0, bs1, bt0]: [f32; 3],
        edge: usize,
        stops: bool,
    ) -> bool {
        let r = self.rules;
        let t_b = bt0 - r.walkway;
        let Some((ls0, ls1)) = f.band((bs0 + bs1) * 0.5, 0.0, bt0 - 0.05) else {
            return false;
        };
        let (ls0, ls1) = (ls0 + EDGE_GAP_M, ls1 - EDGE_GAP_M);
        if ls1 - ls0 < MIN_WIDTH_M {
            return false;
        }
        // As many rows as the yard holds, the drawn angle first; room along the street for
        // the drives (two for a one-way lot), an island at each end and a few stalls.
        let room = t_b - r.buffer;
        let first = rng.range(0, 256) < r.angled_256;
        let wide = |a: bool| {
            let drives = if a || ls1 - ls0 >= r.second_cut_m {
                2.0
            } else {
                1.0
            };
            drives * aisle(r, a) + 2.0 * ISLAND_M + 3.0 * r.stall_w <= ls1 - ls0
        };
        let pick = [(first, 2), (first, 1), (!first, 2), (!first, 1)]
            .into_iter()
            .find(|&(a, n)| n as f32 * row_depth(r, a) + aisle(r, a) <= room && wide(a));
        let Some((angled, rows)) = pick else {
            return false;
        };
        let (sd, aw) = (row_depth(r, angled), aisle(r, angled));
        let t_l = t_b - (rows as f32 * sd + aw);
        let forward: f32 = if angled && rng.range(0, 2) == 1 {
            -1.0
        } else {
            1.0
        };
        let w = |s: f32, t: f32| f.o + f.u * s + f.v * t;
        let rect =
            |s0: f32, s1: f32, t0: f32, t1: f32| [w(s0, t0), w(s1, t0), w(s1, t1), w(s0, t1)];

        // Curb cuts at the lot's ends: both on a long frontage or a one-way lot (in
        // upstream, out downstream), else one.
        let ends = [(ls0, ls0 + aw), (ls1 - aw, ls1)];
        let path = &self.net.paths[edge];
        let spans = self.net.side_spans(edge);
        // The curb cut of drive `c0..c1`, if the curb runs straight past it there (not
        // round a junction's corner).
        let cut_at = |(c0, c1): (f32, f32)| {
            let (sa, ta) = path.project(w(c0, 0.0));
            let (sb, _) = path.project(w(c1, 0.0));
            let (s0, s1) = (sa.min(sb), sa.max(sb));
            let left = ta > 0.0;
            let side = &spans[usize::from(!left)];
            side.iter()
                .any(|g| g[0] + 0.5 <= s0 && s1 + 0.5 <= g[1])
                .then_some(CurbCut {
                    edge: edge as u32,
                    left,
                    s0,
                    s1,
                })
        };
        let mut cuts: Vec<(f32, f32)> = if angled || ls1 - ls0 >= r.second_cut_m {
            ends.to_vec()
        } else {
            // One, at the drawn end if the curb allows, else at the other.
            let k = rng.range(0, 2) as usize;
            match [ends[k], ends[1 - k]]
                .into_iter()
                .find(|&c| cut_at(c).is_some())
            {
                Some(c) => vec![c],
                None => return false,
            }
        };
        if forward < 0.0 {
            cuts.reverse();
        }
        for &(c0, c1) in &cuts {
            let Some(cut) = cut_at((c0, c1)) else {
                return false;
            };
            lot.cuts.push(cut);
            let ramp = RAMP_M.min(t_l);
            // From the walk's back edge (the property line may lie a few cm off it).
            let mut q = rect(c0, c1, 0.0, ramp);
            let (end, _) = self.net.ends(edge);
            for p in &mut q[..2] {
                let (s, t) = path.project(*p);
                *p = path.xz(s, (end.hw + end.sw).copysign(t));
            }
            lot.ramps.push(Ramp {
                q,
                edge: edge as u32,
            });
            if t_l > ramp {
                lot.pave.push(rect(c0, c1, ramp, t_l));
            }
            let reach = if rows == 2 { t_l + sd } else { t_l };
            // From the curb, like the ramp: a parcel's front may lie back from the walk.
            let mut e = rect(c0, c1, 0.0, reach + 0.5);
            e[0] = q[0];
            e[1] = q[1];
            lot.entries.push(e);
        }
        lot.pave.push(rect(ls0, ls1, t_l, t_b));
        lot.aisles.push(rect(ls0, ls1, t_b - sd - aw, t_b - sd));
        let (ws0, ws1) = (ls0.max(bs0), ls1.min(bs1));
        if ws1 > ws0 {
            lot.walks.push(rect(ws0, ws1, t_b, bt0 - 0.02));
        }

        // The rows: along the walk, nosed to it, then (two rows) along the planted strip,
        // nosed to the street, broken by the drives.
        let module = if angled { r.stall_w / SIN60 } else { r.stall_w };
        let slant = if angled {
            forward * sd * COS60 / SIN60
        } else {
            0.0
        };
        let inner = Row {
            s_a: ls0,
            s_b: ls1,
            depth: sd,
            slant,
            module,
        };
        let a0 = t_b - sd;
        self.row(lot, rng, inner, |s, d| w(s, a0 + d), stops);
        if rows == 2 {
            let mut gaps = cuts.clone();
            gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut s = ls0;
            let b0 = t_l + sd;
            for &(c0, c1) in gaps.iter().chain(core::iter::once(&(ls1, ls1))) {
                if c0 > s {
                    let outer = Row {
                        s_a: s,
                        s_b: c0,
                        ..inner
                    };
                    self.row(lot, rng, outer, |s, d| w(s, b0 - d), stops);
                }
                s = c1;
            }
        }

        // Arrows: in at each entry (out at a one-way lot's exit), along a one-way aisle.
        let arrow_t = t_l + ARROW_M * 0.5 + 0.6;
        for (k, &(c0, c1)) in cuts.iter().enumerate() {
            let out = angled && k == 1;
            let dir = if out { -f.v } else { f.v };
            lot.arrows.push((w((c0 + c1) * 0.5, arrow_t), dir));
        }
        if angled {
            let t = t_b - sd - aw * 0.5;
            for x in [0.35, 0.65] {
                lot.arrows
                    .push((w(ls0 + (ls1 - ls0) * x, t), f.u * forward));
            }
        }

        // The planted strip's hedge or low wall, open at the drives.
        let kind = r.frontage(u64::from(rng.next_u32()));
        if kind != Frontage::Open {
            let t = t_l * 0.5;
            let mut s = ls0 + 0.3;
            let mut gaps = cuts;
            gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
            for &(c0, c1) in gaps.iter().chain(core::iter::once(&(ls1, ls1))) {
                let end = c0 - 0.8;
                while end - s > 1.0 {
                    let e = (s + HEDGE_RUN_M).min(end);
                    let run = [w(s, t), w(e, t)];
                    match kind {
                        Frontage::Hedge => lot.hedges.push(run),
                        _ => lot.walls.push(run),
                    }
                    s = e;
                }
                s = c1 + 0.8;
            }
        }
        true
    }

    /// A lot opening onto the alley along the parcel's front (`out` -1: the alley lies
    /// toward smaller `t`, the building from `t_bld` on) or rear (`out` 1: the alley lies
    /// beyond the rear line, the building's back at `t_bld`): one row of 90° stalls from
    /// the alley's curb toward the building, its curb dropped along them.
    #[allow(clippy::too_many_arguments)]
    fn alley_lot(
        &self,
        lot: &mut ParkingLot,
        rng: &mut Rng,
        f: &Frame,
        [bs0, bs1]: [f32; 2],
        t_line: f32,
        t_bld: f32,
        out: f32,
        stops: bool,
    ) -> bool {
        let r = self.rules;
        let sm = (bs0 + bs1) * 0.5;
        let w = |s: f32, t: f32| f.o + f.u * s + f.v * t;
        let q = w(sm, t_line);
        // The alley: the nearest one within reach, running along the line.
        let mut best: Option<(usize, f32, f32, f32)> = None;
        for (i, e) in self.place.roads.edges.iter().enumerate() {
            if e.class != RoadClass::Alley {
                continue;
            }
            let path = &self.net.paths[i];
            let (s, t) = path.project(q);
            let d = (q - path.xz(s, 0.0)).length();
            let hw = self.net.ends(i).0.hw;
            if d < hw + ALLEY_REACH_M && best.is_none_or(|b| d < b.3) {
                best = Some((i, s, t, d));
            }
        }
        let Some((edge, s_q, t_q, d)) = best else {
            return false;
        };
        let path = &self.net.paths[edge];
        if abs(path.left(s_q).dot(f.u)) > 0.1 {
            return false;
        }
        let hw = self.net.ends(edge).0.hw;
        let depth = r.stall_d;
        // The curb top's back, then the row away from the alley; moved off it where the
        // alley runs a little askew, so its whole length clears the curb.
        let dist = |p: V2| {
            let (s, _) = path.project(p);
            (p - path.xz(s, 0.0)).length()
        };
        let mut t_curb = t_line + out * (d - hw - CURB_TOP_M);
        let mut row = None;
        for _ in 0..2 {
            let t_back = t_curb - out * depth;
            if (t_bld - t_back) * -out < EDGE_GAP_M {
                return false;
            }
            let (t0, t1) = if out < 0.0 {
                (t_curb.max(t_line + 0.05), t_back)
            } else {
                (t_back, t_curb.min(t_line - 0.05))
            };
            let Some((ls0, ls1)) = f.band(sm, t0, t1) else {
                return false;
            };
            let (ls0, ls1) = (ls0 + EDGE_GAP_M, ls1 - EDGE_GAP_M);
            if ls1 - ls0 < MIN_WIDTH_M {
                return false;
            }
            let short = hw + CURB_TOP_M - dist(w(ls0, t_curb)).min(dist(w(ls1, t_curb)));
            if short <= 0.0 {
                row = Some((ls0, ls1, t_back));
                break;
            }
            t_curb -= out * (short + 0.01);
        }
        let Some((ls0, ls1, t_back)) = row else {
            return false;
        };
        let (sa, _) = path.project(w(ls0, t_curb));
        let (sb, _) = path.project(w(ls1, t_curb));
        let (s0, s1) = (sa.min(sb), sa.max(sb));
        let left = t_q > 0.0;
        let spans = self.net.side_spans(edge);
        if !spans[usize::from(!left)]
            .iter()
            .any(|g| g[0] + 0.3 <= s0 && s1 + 0.3 <= g[1])
        {
            return false;
        }
        lot.on_alley = true;
        lot.cuts.push(CurbCut {
            edge: edge as u32,
            left,
            s0,
            s1,
        });
        let row = Row {
            s_a: ls0,
            s_b: ls1,
            depth,
            slant: 0.0,
            module: r.stall_w,
        };
        self.row(lot, rng, row, |s, d| w(s, t_curb - out * d), stops);
        let q = [
            w(ls0, t_curb),
            w(ls1, t_curb),
            w(ls1, t_back),
            w(ls0, t_back),
        ];
        lot.pave.push(q);
        lot.entries.push(q);
        true
    }

    /// The lot along beach alley `i`: a row each side wherever the ground is clear.
    fn beach(&self, i: usize) -> Option<ParkingLot> {
        let r = self.rules;
        let e = &self.place.roads.edges[i];
        if e.class != RoadClass::Alley {
            return None;
        }
        let path = &self.net.paths[i];
        let zone = crate::zone_at(self.place, path.xz(path.len() * 0.5, 0.0));
        if zone != ZoneId::Beach {
            return None;
        }
        r.occupancy(zone)?;
        let mut rng = Rng::new(Seed(self.seed), &[stage::LOTS, KEY_ALLEY | i as u64]);
        let mut lot = ParkingLot::new(zone, rng.range(0, 256) < r.yellow_256);
        lot.on_alley = true;
        let stops = rng.range(0, 256) < r.wheel_stop_256;
        let hw = self.net.ends(i).0.hw;
        let t0 = hw + CURB_TOP_M;
        let depth = r.stall_d;
        let spans = self.net.side_spans(i);
        for (k, side) in spans.iter().enumerate() {
            let sigma = if k == 0 { 1.0 } else { -1.0 };
            let at = |s: f32, d: f32| path.xz(s, sigma * (t0 + d));
            for &[g0, g1] in side {
                // Runs of clear stall slots.
                let mut s = g0 + 1.0;
                let mut run: Option<f32> = None;
                while s <= g1 - 1.0 {
                    let next = s + r.stall_w;
                    let q = [at(s, 0.0), at(next, 0.0), at(next, depth), at(s, depth)];
                    let clear = next <= g1 - 1.0 && self.clear(&q, i);
                    match (clear, run) {
                        (true, None) => run = Some(s),
                        (false, Some(a)) => {
                            self.beach_run(&mut lot, &mut rng, i, sigma, &at, a, s, stops);
                            run = None;
                        }
                        _ => {}
                    }
                    s = next;
                }
            }
        }
        if lot.stalls.is_empty() {
            return None;
        }
        self.poles(&mut lot, &mut rng);
        Some(lot)
    }

    /// One run of a beach alley's side, `s_a` to `s_b` along the alley.
    #[allow(clippy::too_many_arguments)]
    fn beach_run(
        &self,
        lot: &mut ParkingLot,
        rng: &mut Rng,
        edge: usize,
        sigma: f32,
        at: &impl Fn(f32, f32) -> V2,
        s_a: f32,
        s_b: f32,
        stops: bool,
    ) {
        let r = self.rules;
        let row = Row {
            s_a,
            s_b,
            depth: r.stall_d,
            slant: 0.0,
            module: r.stall_w,
        };
        let first = lot.stalls.len();
        let mark = (lot.islands.len(), lot.stripes.len(), lot.stops.len());
        let quads = self.row(lot, rng, row, at, stops);
        // Where the alley bends, the inside of the bend squeezes stalls and islands: one
        // too narrow across the car (or folded over a neighbour) is dropped, the rest of
        // the run kept. Its paving stays wherever it does not fold over.
        let islands: Vec<Island> = lot.islands.drain(mark.0..).collect();
        for i in islands {
            let wide_enough = wide(&i.q, -i.pole_facing.right() * sigma, ISLAND_M - SQUEEZE_M);
            let free = lot.islands[mark.0..]
                .iter()
                .all(|k| !overlap(&inset(&i.q), &inset(&k.q)));
            if wide_enough && free {
                lot.islands.push(i);
            }
        }
        let stalls: Vec<Stall> = lot.stalls.drain(first..).collect();
        let stops_all: Vec<[V2; 2]> = lot.stops.drain(mark.2..).collect();
        lot.stripes.truncate(mark.1);
        let own = &self.net.paths[edge];
        let t0 = self.net.ends(edge).0.hw + CURB_TOP_M;
        let on_curb = |p: V2| {
            let (s, _) = own.project(p);
            abs((p - own.xz(s, 0.0)).length() - t0) < 0.5
        };
        for (k, st) in stalls.into_iter().enumerate() {
            let q = inset(&st.q);
            let keep = wide(&st.q, st.nose.right() * sigma, r.stall_w - SQUEEZE_M)
                && on_curb(st.q[0])
                && on_curb(st.q[1])
                && lot.stalls[first..]
                    .iter()
                    .all(|o| !overlap(&q, &inset(&o.q)))
                && lot.islands[mark.0..]
                    .iter()
                    .all(|i| !overlap(&q, &inset(&i.q)));
            if !keep {
                continue;
            }
            // Its side stripes, the one it shares with the stall before laid once.
            let (a, b) = ([st.q[0], st.q[3]], [st.q[1], st.q[2]]);
            if lot.stripes.len() == mark.1 || lot.stripes[lot.stripes.len() - 1] != a {
                lot.stripes.push(a);
            }
            lot.stripes.push(b);
            if stops {
                lot.stops.push(stops_all[k]);
            }
            lot.stalls.push(st);
        }
        if lot.stalls.len() == first {
            lot.islands.truncate(mark.0);
            return;
        }
        let pave: Vec<[V2; 4]> = quads
            .into_iter()
            .filter(|q| wide(q, (q[1] - q[0]).normalize(), 0.0))
            .collect();
        lot.pave.extend_from_slice(&pave);
        lot.entries
            .extend(lot.stalls[first..].iter().map(|st| st.q));
        lot.cuts.push(CurbCut {
            edge: edge as u32,
            left: sigma > 0.0,
            s0: s_a,
            s1: s_b,
        });
    }

    /// Whether a beach stall's quad `q` (beside alley `edge`) is clear: off every other
    /// road's walk and curb, off buildings and lots, not wet.
    fn clear(&self, q: &[V2; 4], edge: usize) -> bool {
        let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
        let pts = [q[0], q[1], q[2], q[3], c];
        let (lo, hi) = bounds(&pts);
        let wl = self.place.terrain.water_level;
        for &p in &pts {
            if crate::zone_at(self.place, p) == ZoneId::Water {
                return false;
            }
            let ground = self.place.terrain.height_at(&self.place.header, [p.x, p.y]);
            if ground.is_none_or(|y| y < wl + 0.3) {
                return false;
            }
        }
        if !self.off_roads(q, Some(edge)) || !self.off_junctions(q) {
            return false;
        }
        // Off its own alley too, where it bends.
        let own = &self.net.paths[edge];
        let curb = self.net.ends(edge).0.hw + CURB_TOP_M - 0.02;
        let off = |p: V2| {
            let (s, _) = own.project(p);
            (p - own.xz(s, 0.0)).length() >= curb
        };
        if !pts.iter().all(|&p| off(p)) {
            return false;
        }
        let overlaps = |bb: (V2, V2), r: f32| {
            lo.x <= bb.1.x + r && hi.x >= bb.0.x - r && lo.y <= bb.1.y + r && hi.y >= bb.0.y - r
        };
        let near = |poly: &[V2], bb: (V2, V2), r: f32| {
            overlaps(bb, r) && pts.iter().any(|&p| polygon_distance(p, poly) < r)
        };
        if self.fps.iter().any(|(fp, bb, _)| near(fp, *bb, 0.5)) {
            return false;
        }
        // Off every parcel but the beach's own lots (buildings are kept clear above), parks
        // and empty lots too: a lot stops at the property line.
        !self
            .parcels
            .iter()
            .any(|(poly, bb)| overlaps(*bb, 0.0) && overlap(q, poly))
    }

    /// Whether quad `q` keeps [`ROAD_CLEAR_M`] off every road's walk or curb top but
    /// `skip`'s.
    fn off_roads(&self, q: &[V2; 4], skip: Option<usize>) -> bool {
        let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
        let pts = [q[0], q[1], q[2], q[3], c];
        let (lo, hi) = bounds(&pts);
        for j in 0..self.place.roads.edges.len() {
            if Some(j) == skip {
                continue;
            }
            let (end, _) = self.net.ends(j);
            let reach = end.hw + end.sw.max(CURB_TOP_M) + ROAD_CLEAR_M;
            let path = &self.net.paths[j];
            let (plo, phi) = self.edge_bounds[j];
            if lo.x > phi.x + reach
                || hi.x < plo.x - reach
                || lo.y > phi.y + reach
                || hi.y < plo.y - reach
            {
                continue;
            }
            for w in path.pts.windows(2) {
                if pts
                    .iter()
                    .any(|&p| segment_distance(p, w[0], w[1]).0 < reach)
                {
                    return false;
                }
            }
        }
        true
    }

    /// Whether quad `q` keeps off every junction's asphalt and corner walks.
    fn off_junctions(&self, q: &[V2; 4]) -> bool {
        let bb = bounds(q);
        !self.junctions.iter().any(|(poly, jb)| {
            bb.0.x <= jb.1.x
                && bb.1.x >= jb.0.x
                && bb.0.y <= jb.1.y
                && bb.1.y >= jb.0.y
                && overlap(q, poly)
        })
    }

    /// Lays `row` (`at(s, d)`: the ground point `s` along and `d` back from its aisle
    /// edge) into `lot`: runs of stalls between islands, stripes, wheel stops (if
    /// `stops`). Returns the stalls' quads.
    fn row(
        &self,
        lot: &mut ParkingLot,
        rng: &mut Rng,
        row: Row,
        at: impl Fn(f32, f32) -> V2,
        stops: bool,
    ) -> Vec<[V2; 4]> {
        let r = self.rules;
        let mut quads = Vec::new();
        // Sheared: a stripe from the aisle edge at `s` to the back at `s + slant`.
        let p = |s: f32, d: f32| at(s + row.slant * (d / row.depth), d);
        let len = row.s_b - row.s_a - abs(row.slant);
        if len < 2.0 * ISLAND_M + 2.0 * row.module {
            return quads;
        }
        let max = i32::from(r.run_max);
        let mut runs = 1;
        let n = loop {
            let n = ((len - ISLAND_M * (runs + 1) as f32) / row.module) as i32;
            if n <= runs * max {
                break n;
            }
            runs += 1;
        };
        if n < 2 {
            return quads;
        }
        let slack = len - ISLAND_M * (runs + 1) as f32 - n as f32 * row.module;
        let mut s = row.s_a + (-row.slant).max(0.0) + slack * 0.5;
        let (base, extra) = (n / runs, n % runs);
        for k in 0..=runs {
            self.island(lot, rng, &p, s, row.depth);
            s += ISLAND_M;
            if k == runs {
                break;
            }
            lot.stripes.push([p(s, 0.0), p(s, row.depth)]);
            for _ in 0..base + i32::from(k < extra) {
                let (s0, s1) = (s, s + row.module);
                let sm = (s0 + s1) * 0.5;
                let nose = (p(sm, row.depth) - p(sm, 0.0)).normalize();
                let q = [p(s0, 0.0), p(s1, 0.0), p(s1, row.depth), p(s0, row.depth)];
                lot.stalls.push(Stall {
                    c: p(sm, row.depth * 0.5),
                    nose,
                    mouth: p(sm, 0.0) - nose * 1.0,
                    q,
                });
                lot.stripes.push([p(s1, 0.0), p(s1, row.depth)]);
                if stops {
                    let c = p(sm, row.depth - STOP_IN_M);
                    let half = nose.left() * (STOP_LEN_M * 0.5);
                    lot.stops.push([c - half, c + half]);
                }
                quads.push(q);
                s = s1;
            }
        }
        quads
    }

    /// An island across a row from `s` along (`p` as in [`Ctx::row`]).
    fn island(
        &self,
        lot: &mut ParkingLot,
        rng: &mut Rng,
        p: &impl Fn(f32, f32) -> V2,
        s: f32,
        depth: f32,
    ) {
        let plant = self.rules.island(u64::from(rng.next_u32()));
        let sm = s + ISLAND_M * 0.5;
        let into = (p(sm, depth) - p(sm, 0.0)).normalize();
        lot.islands.push(Island {
            q: [
                p(s, 0.0),
                p(s + ISLAND_M, 0.0),
                p(s + ISLAND_M, depth),
                p(s, depth),
            ],
            plant,
            plant_at: p(sm, depth * 0.3),
            along: (p(s + ISLAND_M, 0.0) - p(s, 0.0)).normalize(),
            pole_at: p(sm, depth * 0.75),
            pole_facing: -into,
        });
    }

    /// Light poles in the lot's islands, `pole_m` apart (drawn per lot).
    fn poles(&self, lot: &mut ParkingLot, rng: &mut Rng) {
        let (lo, hi) = self.rules.pole_m;
        let spacing = rng.range_f32(lo, hi);
        for i in 0..lot.islands.len() {
            let at = lot.islands[i].pole_at;
            if lot.poles.iter().all(|&(p, _)| (p - at).length() >= spacing) {
                lot.poles.push((at, lot.islands[i].pole_facing));
            }
        }
    }
}

/// The paints of a lot.
struct Paints {
    asphalt: Paint,
    stripe: Paint,
    concrete: Paint,
    curb: Paint,
    soil: Paint,
    wall: Paint,
    cap: Paint,
    leaf: Paint,
}

/// Meshes every lot piece `out` wants (each filed under its own middle's chunk) into
/// [`MeshKind::Ground`]: asphalt, paint, the walk, ramps, islands, wheel stops and low
/// walls, laid on the graded terrain `grid` (ramps from the walk of their road in `net`).
pub fn mesh(
    lots: &[ParkingLot],
    grid: &TerrainGrid,
    net: &Network,
    pal: &Palette,
    out: &mut MeshSet,
) {
    let concrete = Paint::new(shade(pal.concrete, 235), Pattern::Concrete);
    let mut paints = Paints {
        asphalt: Paint::new(mix(pal.asphalt, pal.asphalt_worn, 110), Pattern::Asphalt),
        stripe: Paint::new(pal.paint_white, Pattern::Paint),
        concrete,
        curb: Paint::new(pal.concrete, Pattern::Concrete),
        soil: Paint::new(mix(pal.lawn, pal.soil, 90), Pattern::Lawn),
        wall: Paint::new(mix(pal.concrete, pal.sand, 110), Pattern::Concrete),
        cap: Paint::new(
            shade(mix(pal.concrete, pal.sand, 110), 225),
            Pattern::Concrete,
        ),
        leaf: Paint::new(shade(pal.leaf_green, 215), Pattern::Foliage),
    };
    let mid = |q: &[V2]| {
        let mut c = V2::new(0.0, 0.0);
        for &p in q {
            c = c + p;
        }
        c * (1.0 / q.len() as f32)
    };
    for lot in lots {
        paints.stripe = Paint::new(
            if lot.yellow {
                pal.paint_yellow
            } else {
                pal.paint_white
            },
            Pattern::Paint,
        );
        for q in &lot.pave {
            let c = mid(q);
            if out.wants(c) {
                drape(
                    out.at(c, MeshKind::Ground),
                    grid,
                    q,
                    LOT_LIFT_M,
                    paints.asphalt,
                );
            }
        }
        let paint_y = LOT_LIFT_M + 2.0 * PAINT_LIFT;
        for s in &lot.stripes {
            let c = mid(s);
            if out.wants(c) {
                let half = (s[1] - s[0]).normalize().left() * (STRIPE_W_M * 0.5);
                let q = [s[0] - half, s[0] + half, s[1] + half, s[1] - half];
                drape(
                    out.at(c, MeshKind::Ground),
                    grid,
                    &q,
                    paint_y,
                    paints.stripe,
                );
            }
        }
        for &(c, dir) in &lot.arrows {
            if out.wants(c) {
                arrow(
                    out.at(c, MeshKind::Ground),
                    grid,
                    c,
                    dir,
                    paint_y,
                    paints.stripe,
                );
            }
        }
        for q in &lot.walks {
            let c = mid(q);
            if out.wants(c) {
                slab(
                    out.at(c, MeshKind::Ground),
                    grid,
                    q,
                    CURB_M,
                    paints.concrete,
                    paints.curb,
                );
            }
        }
        for r in &lot.ramps {
            let c = mid(&r.q);
            if out.wants(c) {
                ramp(out.at(c, MeshKind::Ground), grid, net, r, paints.concrete);
            }
        }
        for i in &lot.islands {
            let c = mid(&i.q);
            if out.wants(c) {
                let m = out.at(c, MeshKind::Ground);
                slab(m, grid, &i.q, ISLAND_H_M, paints.soil, paints.curb);
                if i.plant == Planting::Shrub {
                    shrub(m, grid, i, paints.leaf);
                }
            }
        }
        for s in &lot.stops {
            let c = mid(s);
            if out.wants(c) {
                let half = (s[1] - s[0]).normalize().left() * (STOP_W_M * 0.5);
                let q = [s[0] - half, s[0] + half, s[1] + half, s[1] - half];
                slab(
                    out.at(c, MeshKind::Ground),
                    grid,
                    &q,
                    STOP_H_M,
                    paints.curb,
                    paints.curb,
                );
            }
        }
        for s in &lot.walls {
            let c = mid(s);
            if out.wants(c) {
                low_wall(out.at(c, MeshKind::Ground), grid, s, &paints);
            }
        }
    }
}

/// The terrain's height at `p`.
fn ground(grid: &TerrainGrid, p: V2) -> f32 {
    grid.height_at(p.x, p.y)
}

/// Lays convex polygon `poly` on the terrain `lift` above it: cut to each terrain
/// triangle under it, each piece in that triangle's plane.
fn drape(m: &mut WorldMesh, grid: &TerrainGrid, poly: &[V2], lift: f32, paint: Paint) {
    let (lo, hi) = bounds(poly);
    let cell = grid.cell;
    let ix = |x: f32| ((x - grid.origin[0]) / cell).max(0.0) as usize;
    let iz = |z: f32| ((z - grid.origin[1]) / cell).max(0.0) as usize;
    let (x0, x1) = (ix(lo.x), (ix(hi.x) + 1).min(grid.w - 1));
    let (z0, z1) = (iz(lo.y), (iz(hi.y) + 1).min(grid.h - 1));
    let mut a: Vec<V2> = Vec::with_capacity(8);
    let mut b: Vec<V2> = Vec::with_capacity(8);
    for cz in z0..z1 {
        for cx in x0..x1 {
            let v = |x: usize, z: usize| grid.pos(x, z).at(grid.sample(x, z));
            let (p00, p10, p01, p11) = (v(cx, cz), v(cx + 1, cz), v(cx, cz + 1), v(cx + 1, cz + 1));
            let tris = if grid.flip[cz * (grid.w - 1) + cx] != 0 {
                [[p00, p10, p01], [p11, p01, p10]]
            } else {
                [[p00, p10, p11], [p00, p11, p01]]
            };
            for t in tris {
                a.clear();
                a.extend_from_slice(poly);
                for k in 0..3 {
                    let (e0, e1, other) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                    clip_half(&a, xz(e0), xz(e1), xz(other), &mut b);
                    core::mem::swap(&mut a, &mut b);
                    if a.len() < 3 {
                        break;
                    }
                }
                if a.len() < 3 {
                    continue;
                }
                let n = (t[1] - t[0]).cross(t[2] - t[0]);
                if abs(n.y) < 1e-9 {
                    continue;
                }
                let n = if n.y < 0.0 { -n } else { n };
                // The triangle's plane: n · (p - t0) = 0.
                let y = |p: V2| t[0].y - (n.x * (p.x - t[0].x) + n.z * (p.y - t[0].z)) / n.y + lift;
                let up = n.normalize();
                let p0 = a[0].at(y(a[0]));
                for k in 1..a.len() - 1 {
                    tri(m, p0, a[k].at(y(a[k])), a[k + 1].at(y(a[k + 1])), up, paint);
                }
            }
        }
    }
}

fn xz(p: V3) -> V2 {
    V2::new(p.x, p.z)
}

/// Sutherland-Hodgman: `poly` cut to the side of the line `e0`–`e1` that `keep` is on.
fn clip_half(poly: &[V2], e0: V2, e1: V2, keep: V2, out: &mut Vec<V2>) {
    out.clear();
    let d = e1 - e0;
    let side = |p: V2| d.turn(p - e0);
    let sign = if side(keep) < 0.0 { -1.0 } else { 1.0 };
    let n = poly.len();
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let (sp, sq) = (side(p) * sign, side(q) * sign);
        if sp >= 0.0 {
            out.push(p);
        }
        if (sp >= 0.0) != (sq >= 0.0) {
            out.push(p.lerp(q, sp / (sp - sq)));
        }
    }
}

/// A painted arrow at `c` pointing along `dir`: a shaft and a head.
fn arrow(m: &mut WorldMesh, grid: &TerrainGrid, c: V2, dir: V2, lift: f32, paint: Paint) {
    let side = dir.left();
    let back = c - dir * (ARROW_M * 0.5);
    let neck = c + dir * (ARROW_M * 0.1);
    let tip = c + dir * (ARROW_M * 0.5);
    let w = side * 0.15;
    drape(
        m,
        grid,
        &[back - w, back + w, neck + w, neck - w],
        lift,
        paint,
    );
    let h = side * (ARROW_HEAD_W_M * 0.5);
    drape(m, grid, &[neck - h, neck + h, tip], lift, paint);
}

/// A slab `h` high standing on the terrain over quad `q`: its top in `top`, its sides in
/// `side`, reaching a little into the ground.
fn slab(m: &mut WorldMesh, grid: &TerrainGrid, q: &[V2; 4], h: f32, top: Paint, side: Paint) {
    let ys = q.map(|p| ground(grid, p));
    let up = q
        .iter()
        .zip(ys)
        .map(|(p, y)| p.at(y + h))
        .collect::<Vec<_>>();
    quad(m, [up[0], up[1], up[2], up[3]], V3::Y, top);
    let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let out = ((a + b) * 0.5 - c).at(0.0);
        let (ya, yb) = (ys[k], ys[(k + 1) % 4]);
        quad(
            m,
            [a.at(ya - 0.1), b.at(yb - 0.1), b.at(yb + h), a.at(ya + h)],
            out,
            side,
        );
    }
}

/// A sloped concrete ramp from its road's walk top at the property line down to the lot,
/// with its sides.
fn ramp(m: &mut WorldMesh, grid: &TerrainGrid, net: &Network, r: &Ramp, paint: Paint) {
    let path = &net.paths[r.edge as usize];
    let top = |p: V2| path.y_at(p) + CURB_M;
    let q = [
        r.q[0].at(top(r.q[0])),
        r.q[1].at(top(r.q[1])),
        r.q[2].at(ground(grid, r.q[2]) + LOT_LIFT_M + PAINT_LIFT),
        r.q[3].at(ground(grid, r.q[3]) + LOT_LIFT_M + PAINT_LIFT),
    ];
    quad(m, q, V3::Y, paint);
    // Its sides, down into the ground.
    for (a, b, out) in [(0, 3, r.q[0] - r.q[1]), (1, 2, r.q[1] - r.q[0])] {
        let foot = |k: usize| r.q[k].at(ground(grid, r.q[k]) - 0.1);
        quad(m, [foot(a), foot(b), q[b], q[a]], out.at(0.0), paint);
    }
}

/// A shrub on island `i`: a low clump of two crossed boxes.
fn shrub(m: &mut WorldMesh, grid: &TerrainGrid, i: &Island, paint: Paint) {
    let at = i.plant_at;
    let base = at.at(ground(grid, at) + ISLAND_H_M - 0.05);
    let diag = (i.along + i.along.left()).normalize();
    for (dir, h, w) in [(i.along, SHRUB_H_M, 0.6), (diag, SHRUB_H_M - 0.12, 0.5)] {
        let f = MeshFrame::upright(base, dir);
        boxf(m, &f, [-w, 0.0, -w], [w, h, w], face::NO_BOTTOM, paint);
    }
}

/// A low block wall with a cap along run `s`, level on top, footed in the ground.
fn low_wall(m: &mut WorldMesh, grid: &TerrainGrid, s: &[V2; 2], p: &Paints) {
    let d = (s[1] - s[0]).normalize();
    let half = d.left() * 0.1;
    let (ya, yb) = (ground(grid, s[0]), ground(grid, s[1]));
    let top = ya.max(yb) + WALL_H_M;
    let base = ya.min(yb) - 0.3;
    let corners = [s[0] - half, s[1] - half, s[1] + half, s[0] + half];
    for k in 0..4 {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let out = ((a + b) * 0.5 - (s[0] + s[1]) * 0.5).at(0.0);
        quad(
            m,
            [a.at(base), b.at(base), b.at(top), a.at(top)],
            out,
            p.wall,
        );
    }
    let cap = d.left() * 0.13;
    let ends = [s[0] - d * 0.02, s[1] + d * 0.02];
    let c = [ends[0] - cap, ends[1] - cap, ends[1] + cap, ends[0] + cap];
    quad(m, c.map(|q| q.at(top + 0.08)), V3::Y, p.cap);
    for k in 0..4 {
        let (a, b) = (c[k], c[(k + 1) % 4]);
        let out = ((a + b) * 0.5 - (s[0] + s[1]) * 0.5).at(0.0);
        quad(
            m,
            [a.at(top), b.at(top), b.at(top + 0.08), a.at(top + 0.08)],
            out,
            p.cap,
        );
    }
}
