//! Stage 6 of `docs/WORLD_GENERATION.md`: buildings on parcels, generated per chunk.
//!
//! Every `Lot` parcel of a zone with a [`Massing`] gets a building (farmsteads a cluster:
//! house, barn and silo; bungalows a detached garage), decided from the parcel alone:
//! - **Frame.** The parcel's *front* is its longest side lying on the frontage road's
//!   property line and running along it. Everything is laid out in the lot's frame: `s`
//!   along the front, `t` inward from it.
//! - **Style and use** by the zone's `styles` and `Massing::style_weights`, each style
//!   weighted 0 before the year it appears (a 1977 place has no 1980s glass); the use
//!   follows from zone, style and height (downtown low-rises are shops with offices above,
//!   taller ones offices, now and then a hotel; motels are hotels; strip malls shops).
//! - **Height.** Storeys from the zone's log-normal draw (`floors_mean`, `floors_spread`,
//!   clamped to `floors`), capped on narrow lots; styles with a height of their own
//!   (strip malls, motels, bungalows, ranches, dingbats, warehouses) take theirs. Wall
//!   heights follow from storeys by use. The mesher steps towers back above six floors.
//! - **Footprint.** A rectangle in the lot frame: the front, side and rear setbacks of the
//!   zone's [`Lots`] (strip malls keep room for a front parking lot, houses a driveway on
//!   one side; shops and offices backing onto an alley may keep a row of stalls on it,
//!   `crate::lots`) give
//!   the largest rectangle inside the parcel, found exactly from horizontal spans of the
//!   parcel at its vertex levels. Houses are capped to their style's size and centered;
//!   shops and offices run lot line to lot line (party walls). Then the footprint shrinks
//!   (depth first) to the zone's coverage target, jittered ±10% per lot. Wide flat-roofed
//!   low-rises may lose a rear corner (an L) or a rear middle (a U, a light court); motels
//!   are an L or a U of 10 m deep wings around a parking court open to the street.
//! - **Checks.** A footprint is a simple counter-clockwise polygon of at least 4 vertices
//!   and 40 m², strictly inside its parcel (so never over the road: the parcel already
//!   stops at the property line) and inside the place (a lot on the place's edge is
//!   clipped to it first), not on ground falling more than 6 m across it, and
//!   never overlapping another footprint of its lot. Anything else is dropped.
//!
//! **Determinism.** A parcel's stream is `Rng::new(seed, [BUILDINGS, key])` with `key` a
//! hash of its polygon quantized to 0.1 m, so a parcel gives the same buildings whichever
//! chunk asks; plain `f32`/`f64` arithmetic in a fixed order (`exp`/`ln` are series here,
//! never the platform's).

use alloc::vec;
use alloc::vec::Vec;

use place::{
    Building, BuildingUse, LotKind, Parcel, RoadClass, RoadGraph, Roof, StyleTag, ZoneId,
    SIDEWALK_WIDTH_M,
};
use skywave_core::{hash_bytes, Rng, Seed};

use super::macro_map::MacroMap;
use super::{stage, CHUNK_M};
use crate::grammar::LotRules;
use crate::math::{abs, floor, segment_distance, V2};
use crate::template::{BuildingStyle, Lots, Massing, RegionTemplate, ZoneRule};

/// Smallest footprint kept.
pub const MIN_FOOTPRINT_M2: f32 = 40.0;
/// Shortest footprint side.
const MIN_SIDE_M: f32 = 4.0;
/// Gap kept from every lot line, so neighbours never touch exactly.
const LOT_GAP_M: f32 = 0.05;
/// Most the ground may fall across a footprint.
const MAX_FALL_M: f32 = 6.0;
/// Tallest masonry, deco or stucco building (the old 150 ft limit); taller ones are glass.
pub const MASONRY_FLOORS: u8 = 13;
/// Range of the per-lot coverage aim, as a multiple of the zone's target.
const AIM: (f32, f32) = (1.1, 1.5);
/// Room left beyond an alley lot's stalls, m.
const ALLEY_LOT_GAP_M: f32 = 0.6;
/// How far past an alley's curb a lot's rear line may lie for the lot to back onto it, m.
pub(crate) const ALLEY_REACH_M: f32 = 1.5;
/// A detached garage: width and depth.
const GARAGE_M: (f32, f32) = (6.2, 6.6);
/// Depth of a motel wing: rooms plus the walkway in front of them.
const MOTEL_WING_M: f32 = 10.0;
/// Farm country lots at least this big are farmsteads.
const FARMSTEAD_M2: f32 = 2000.0;
/// Radius of a silo's octagon.
const SILO_R_M: f32 = 3.8;

/// The buildings of the parcels of one chunk (any parcels, in fact), in parcel order.
pub fn buildings(
    seed: Seed,
    template: &RegionTemplate,
    lots: &LotRules,
    macro_map: &MacroMap,
    parcels: &[Parcel],
    roads: &RoadGraph,
) -> Vec<Building> {
    let mut out = Vec::new();
    for p in parcels {
        out.extend(lot_buildings(seed, template, lots, macro_map, p, roads));
    }
    out
}

/// The buildings of every parcel of the place (as [`super::parcels_all`] gives them),
/// stably sorted by the chunk of their footprint's vertex mean, z major: the order
/// `place::Place::sort_by_chunk` keeps.
pub fn buildings_all(
    seed: Seed,
    template: &RegionTemplate,
    lots: &LotRules,
    macro_map: &MacroMap,
    parcels: &[Parcel],
    roads: &RoadGraph,
) -> Vec<Building> {
    let all = without_overlaps(buildings(seed, template, lots, macro_map, parcels, roads));
    let sx = i64::from(macro_map.size_chunks[0]);
    let key = |b: &Building| {
        let n = b.footprint.len() as f32;
        let (mut x, mut z) = (0.0f32, 0.0f32);
        for p in &b.footprint {
            x += p[0];
            z += p[1];
        }
        let (cx, cz) = (floor(x / n / CHUNK_M) as i64, floor(z / n / CHUNK_M) as i64);
        cz * sx + cx
    };
    let mut keyed: Vec<(i64, Building)> = all.into_iter().map(|b| (key(&b), b)).collect();
    keyed.sort_by_key(|k| k.0);
    keyed.into_iter().map(|k| k.1).collect()
}

/// `all` less every building whose footprint overlaps one kept before it by more than half
/// of what `place::validate` tolerates: two lots a degenerate block cut from the same
/// frontage never both build.
fn without_overlaps(all: Vec<Building>) -> Vec<Building> {
    let bbox = |b: &Building| {
        let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in &b.footprint {
            r = [
                r[0].min(p[0]),
                r[1].min(p[1]),
                r[2].max(p[0]),
                r[3].max(p[1]),
            ];
        }
        r
    };
    let area = |b: &Building| {
        let f = &b.footprint;
        let mut a = 0.0f64;
        for i in 0..f.len() {
            let (p, q) = (f[i], f[(i + 1) % f.len()]);
            a += f64::from(p[0]) * f64::from(q[1]) - f64::from(q[0]) * f64::from(p[1]);
        }
        0.5 * a.abs()
    };
    let mut kept: Vec<(Building, [f32; 4], f64)> = Vec::with_capacity(all.len());
    for b in all {
        let (r, a) = (bbox(&b), area(&b));
        let clash = kept.iter().any(|(k, kr, ka)| {
            let apart = r[0] >= kr[2] || kr[0] >= r[2] || r[1] >= kr[3] || kr[1] >= r[3];
            !apart
                && place::validate::overlap_area(&b.footprint, &k.footprint)
                    > 0.5 * place::validate::MAX_BUILDING_OVERLAP * a.min(*ka)
        });
        if !clash {
            kept.push((b, r, a));
        }
    }
    kept.into_iter().map(|k| k.0).collect()
}

/// Content hash of buildings (every field, in order), for determinism tests.
pub fn hash_buildings(buildings: &[Building]) -> u64 {
    let mut b: Vec<u8> = Vec::new();
    for x in buildings {
        b.extend_from_slice(&(x.footprint.len() as u32).to_le_bytes());
        for q in &x.footprint {
            b.extend_from_slice(&q[0].to_bits().to_le_bytes());
            b.extend_from_slice(&q[1].to_bits().to_le_bytes());
        }
        b.extend_from_slice(&x.height_m.to_bits().to_le_bytes());
        b.extend_from_slice(&[x.floors, x.roof as u8, x.style as u8, x.usage as u8]);
        b.extend_from_slice(&x.built_year.unwrap_or(0).to_le_bytes());
    }
    hash_bytes(0x4255_494C_4453, &b)
}

/// The place style a template style family stands for.
pub fn style_tag(s: BuildingStyle) -> StyleTag {
    match s {
        BuildingStyle::StuccoBox | BuildingStyle::Brutalist => StyleTag::StuccoBox,
        BuildingStyle::Brick | BuildingStyle::ThreeFlat => StyleTag::Brick,
        BuildingStyle::Deco => StyleTag::Deco,
        BuildingStyle::StripMall => StyleTag::StripMall,
        BuildingStyle::Dingbat => StyleTag::Dingbat,
        BuildingStyle::Craftsman | BuildingStyle::Bungalow => StyleTag::Craftsman,
        BuildingStyle::Ranch => StyleTag::Ranch,
        BuildingStyle::Tract70s => StyleTag::Tract70s,
        BuildingStyle::Glass | BuildingStyle::Tower => StyleTag::Glass,
        BuildingStyle::Warehouse => StyleTag::Warehouse,
        BuildingStyle::Barn => StyleTag::Barn,
        BuildingStyle::Motel => StyleTag::Motel,
    }
}

/// Years a style was built in.
fn years(s: StyleTag) -> (u16, u16) {
    match s {
        StyleTag::StuccoBox => (1925, 1984),
        StyleTag::Brick => (1895, 1935),
        StyleTag::Deco => (1926, 1948),
        StyleTag::StripMall => (1958, 1990),
        StyleTag::Dingbat => (1954, 1972),
        StyleTag::Craftsman => (1905, 1932),
        StyleTag::Ranch => (1946, 1968),
        StyleTag::Tract70s => (1967, 1982),
        StyleTag::Glass => (1962, 2025),
        StyleTag::Warehouse => (1925, 1985),
        StyleTag::Barn => (1880, 1945),
        StyleTag::Motel => (1946, 1968),
    }
}

/// The buildings on one parcel: none unless it is a `Lot` in a zone with a massing rule.
pub fn lot_buildings(
    seed: Seed,
    template: &RegionTemplate,
    lots: &LotRules,
    macro_map: &MacroMap,
    parcel: &Parcel,
    roads: &RoadGraph,
) -> Vec<Building> {
    if parcel.lot_kind != LotKind::Lot || parcel.polygon.len() < 3 {
        return Vec::new();
    }
    let Some(rule) = template.zones.get(parcel.zone) else {
        return Vec::new();
    };
    let Some(mass) = &rule.massing else {
        return Vec::new();
    };
    let Some(road) = roads.edges.get(parcel.frontage_edge as usize) else {
        return Vec::new();
    };
    let poly: Vec<V2> = parcel.polygon.iter().map(|&p| V2::from_array(p)).collect();
    // Lots on the place's edge may run past it; only the part inside is built on.
    let extent = [
        f32::from(macro_map.size_chunks[0]) * CHUNK_M,
        f32::from(macro_map.size_chunks[1]) * CHUNK_M,
    ];
    let poly = clip_to_place(poly, extent);
    if poly.len() < 3 {
        return Vec::new();
    }
    let area = area(&poly);
    if area < 2.0 * MIN_FOOTPRINT_M2 {
        return Vec::new();
    }
    let line: Vec<V2> = road.profile.iter().map(|p| V2::new(p[0], p[2])).collect();
    let walk = if road.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
    let reach = road.width_m * 0.5 + walk + 2.0;
    let front = front_edge(&poly, &line, reach);
    let mut rng = Rng::new(seed, &[stage::BUILDINGS, parcel_key(&parcel.polygon)]);
    let lot = Lot::new(&poly, front, area);
    let ctx = Ctx {
        lot: &lot,
        zone: parcel.zone,
        rule,
        mass,
        year: template.era.year,
        m: macro_map,
        lots,
        roads,
        on_alley: road.class == RoadClass::Alley,
    };
    // Farmsteads get a cluster; small lots in farm country a house.
    let mut out = match parcel.zone {
        ZoneId::Farmland if area >= FARMSTEAD_M2 => ctx.farm(&mut rng),
        ZoneId::Farmland => ctx.one(&mut rng, Some(StyleTag::Ranch)),
        _ => ctx.one(&mut rng, None),
    };
    // Keep what is sound and clear of the lot's other buildings; without its main
    // building (the first) a lot keeps nothing.
    let mut kept: Vec<Building> = Vec::new();
    for b in out.drain(..) {
        let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
        let clear = kept.iter().all(|k| {
            let o: Vec<V2> = k.footprint.iter().map(|&p| V2::from_array(p)).collect();
            disjoint(&fp, &o)
        });
        if clear && sound(&fp, &poly) && ctx.flat_enough(&fp) {
            kept.push(b);
        } else if kept.is_empty() {
            return Vec::new();
        }
    }
    kept
}

/// The open ground a strip mall keeps in front for its lot: the covered walk, two rows of
/// 90° stalls either side of a two-way aisle and the planted strip along the street, and
/// a little to spare, m.
pub(crate) fn front_lot_depth(lots: &LotRules) -> f32 {
    lots.walkway + 2.0 * lots.stall_d + lots.aisle_w + lots.buffer + 0.5
}

/// Hash of a polygon quantized to 0.1 m: a parcel's identity whatever chunk built it.
pub(crate) fn parcel_key(poly: &[[f32; 2]]) -> u64 {
    let mut b: Vec<u8> = Vec::with_capacity(poly.len() * 16);
    for p in poly {
        b.extend_from_slice(&quant(p[0]).to_le_bytes());
        b.extend_from_slice(&quant(p[1]).to_le_bytes());
    }
    hash_bytes(0x004C_4F54, &b)
}

fn quant(x: f32) -> i64 {
    let s = f64::from(x) * 10.0;
    let f = s as i64;
    if s - f as f64 >= 0.5 {
        f + 1
    } else if s - (f as f64) <= -0.5 {
        f - 1
    } else {
        f
    }
}

/// The parcel side on the frontage road: the longest one within `reach` of its
/// centerline and running along it; else the side nearest to it.
pub(crate) fn front_edge(poly: &[V2], line: &[V2], reach: f32) -> usize {
    let n = poly.len();
    let (mut best, mut best_len) = (usize::MAX, 0.0f32);
    let (mut near, mut near_d) = (0, f32::MAX);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let len = (b - a).length();
        if len < 0.5 {
            continue;
        }
        let mid = (a + b) * 0.5;
        let (mut d, mut dir) = (f32::MAX, V2::new(1.0, 0.0));
        for w in line.windows(2) {
            let (dd, _) = segment_distance(mid, w[0], w[1]);
            if dd < d && (w[1] - w[0]).length() > 1e-3 {
                d = dd;
                dir = (w[1] - w[0]).normalize();
            }
        }
        if d < near_d {
            near_d = d;
            near = i;
        }
        let along = abs((b - a).normalize().dot(dir)) > 0.85;
        if d <= reach && along && len > best_len {
            best = i;
            best_len = len;
        }
    }
    if best == usize::MAX {
        near
    } else {
        best
    }
}

/// A parcel in its own frame: `s` along the front side from its start, `t` inward.
pub(crate) struct Lot {
    pub(crate) o: V2,
    pub(crate) u: V2,
    pub(crate) v: V2,
    /// Middle of the front, along it.
    pub(crate) mid: f32,
    /// The parcel, in world coordinates.
    poly: Vec<V2>,
    local: Vec<V2>,
    area: f32,
}

impl Lot {
    pub(crate) fn new(poly: &[V2], front: usize, area: f32) -> Lot {
        let n = poly.len();
        let (a, b) = (poly[front], poly[(front + 1) % n]);
        let u = (b - a).normalize();
        // Inward: the left of a counter-clockwise polygon's side.
        let v = u.left();
        let local: Vec<V2> = poly
            .iter()
            .map(|&p| {
                let d = p - a;
                V2::new(d.dot(u), d.dot(v))
            })
            .collect();
        // The front runs on through collinear neighbours (lots split along a curving
        // property line): its middle is the lot's.
        let on_line = |p: V2| abs(p.y) < 0.3;
        let (mut lo, mut hi) = (0.0f32, local[(front + 1) % n].x);
        for k in 1..n {
            let p = local[(front + 1 + k) % n];
            if !on_line(p) {
                break;
            }
            hi = hi.max(p.x);
        }
        for k in 1..n {
            let p = local[(front + n - k) % n];
            if !on_line(p) {
                break;
            }
            lo = lo.min(p.x);
        }
        Lot {
            o: a,
            u,
            v,
            mid: (lo + hi) * 0.5,
            poly: poly.to_vec(),
            local,
            area,
        }
    }

    fn world(&self, s: f32, t: f32) -> [f32; 2] {
        let p = self.o + self.u * s + self.v * t;
        [p.x, p.y]
    }

    /// A local polygon as a counter-clockwise world footprint.
    fn footprint(&self, pts: &[(f32, f32)]) -> Vec<[f32; 2]> {
        let mut fp: Vec<[f32; 2]> = pts.iter().map(|&(s, t)| self.world(s, t)).collect();
        if place::signed_area(&fp) < 0.0 {
            fp.reverse();
        }
        fp
    }

    /// The interval of the polygon's cross-section at depth `t` holding `s`, if any.
    fn span_s(&self, t: f32, s: f32) -> Option<(f32, f32)> {
        let mut xs: Vec<f32> = Vec::new();
        let n = self.local.len();
        for i in 0..n {
            let (a, b) = (self.local[i], self.local[(i + 1) % n]);
            if (a.y > t) != (b.y > t) {
                xs.push(a.x + (t - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        interval(&mut xs, s)
    }

    /// The interval of the polygon's cross-section at `s` along the front holding `t`.
    pub(crate) fn span_t(&self, s: f32, t: f32) -> Option<(f32, f32)> {
        let mut ys: Vec<f32> = Vec::new();
        let n = self.local.len();
        for i in 0..n {
            let (a, b) = (self.local[i], self.local[(i + 1) % n]);
            if (a.x > s) != (b.x > s) {
                ys.push(a.y + (s - a.x) / (b.x - a.x) * (b.y - a.y));
            }
        }
        interval(&mut ys, t)
    }

    /// The widest `[s0, s1]` around `s` such that the band `[s0, s1] x [t0, t1]` lies
    /// inside the polygon: the intersection of its cross-sections at both ends and just
    /// either side of every vertex level in between (the section's ends move linearly
    /// between vertex levels).
    pub(crate) fn band(&self, s: f32, t0: f32, t1: f32) -> Option<(f32, f32)> {
        const EPS: f32 = 1e-3;
        let (mut lo, mut hi) = (f32::MIN, f32::MAX);
        let mut cut = |t: f32| -> Option<()> {
            let (a, b) = self.span_s(t, s)?;
            lo = lo.max(a);
            hi = hi.min(b);
            Some(())
        };
        cut(t0 + EPS)?;
        cut(t1 - EPS)?;
        for p in &self.local {
            if p.y > t0 + EPS && p.y < t1 - EPS {
                cut(p.y - EPS)?;
                cut(p.y + EPS)?;
            }
        }
        (hi > lo).then_some((lo, hi))
    }

    /// [`Lot::band`] the other way: the deepest `[t0, t1]` around `t` such that the band
    /// `[s0, s1] x [t0, t1]` lies inside the polygon.
    fn band_t(&self, s0: f32, s1: f32, t: f32) -> Option<(f32, f32)> {
        const EPS: f32 = 1e-3;
        let (mut lo, mut hi) = (f32::MIN, f32::MAX);
        let mut cut = |s: f32| -> Option<()> {
            let (a, b) = self.span_t(s, t)?;
            lo = lo.max(a);
            hi = hi.min(b);
            Some(())
        };
        cut(s0 + EPS)?;
        cut(s1 - EPS)?;
        for p in &self.local {
            if p.x > s0 + EPS && p.x < s1 - EPS {
                cut(p.x - EPS)?;
                cut(p.x + EPS)?;
            }
        }
        (hi > lo).then_some((lo, hi))
    }
}

/// The pair of sorted crossings enclosing `at`.
fn interval(xs: &mut [f32], at: f32) -> Option<(f32, f32)> {
    xs.sort_by(f32::total_cmp);
    xs.chunks_exact(2)
        .find(|w| w[0] <= at && at <= w[1])
        .map(|w| (w[0], w[1]))
}

/// How one style sits on its lot.
#[derive(Clone, Copy)]
struct Fit {
    front: f32,
    side: f32,
    rear: f32,
    /// Driveway kept clear on one side.
    drive: f32,
    /// Footprint width (along the front) and depth limits.
    w: (f32, f32),
    d: (f32, f32),
    /// Centered house (else from lot line to lot line, as wide as it may be).
    house: bool,
}

/// A rectangle in lot coordinates: `[s0, s1] x [t0, t1]`.
#[derive(Clone, Copy, Debug)]
struct Rect {
    s0: f32,
    s1: f32,
    t0: f32,
    t1: f32,
}

impl Rect {
    fn w(&self) -> f32 {
        self.s1 - self.s0
    }
    fn d(&self) -> f32 {
        self.t1 - self.t0
    }
    fn pts(&self) -> [(f32, f32); 4] {
        [
            (self.s0, self.t0),
            (self.s1, self.t0),
            (self.s1, self.t1),
            (self.s0, self.t1),
        ]
    }
}

struct Ctx<'a> {
    lot: &'a Lot,
    zone: ZoneId,
    rule: &'a ZoneRule,
    mass: &'a Massing,
    year: u16,
    m: &'a MacroMap,
    lots: &'a LotRules,
    roads: &'a RoadGraph,
    /// The lot fronts an alley.
    on_alley: bool,
}

/// The side of a lot an alley lot is left on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AlleySide {
    Front,
    Rear,
}

impl Ctx<'_> {
    fn lots(&self) -> Lots {
        self.rule.lots
    }

    /// Picks one of the zone's styles by weight; styles from after the place's year
    /// weigh nothing.
    fn style(&self, rng: &mut Rng) -> Option<StyleTag> {
        let w: Vec<(StyleTag, f32)> = self
            .rule
            .styles
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let tag = style_tag(s);
                let weight = self.mass.style_weights.get(i).copied().unwrap_or(1.0);
                let weight = if years(tag).0 > self.year {
                    0.0
                } else {
                    weight
                };
                (tag, weight)
            })
            .collect();
        let total: f32 = w.iter().map(|x| x.1).sum();
        if total <= 0.0 {
            return None;
        }
        let mut r = rng.range_f32(0.0, total);
        for &(tag, weight) in &w {
            if r < weight {
                return Some(tag);
            }
            r -= weight;
        }
        w.iter().rev().find(|x| x.1 > 0.0).map(|x| x.0)
    }

    fn residential_zone(&self) -> bool {
        matches!(
            self.zone,
            ZoneId::Residential | ZoneId::Suburb | ZoneId::Hills | ZoneId::Farmland
        )
    }

    /// How `style` sits on this zone's lots.
    fn fit(&self, style: StyleTag) -> Fit {
        let l = self.lots();
        let (f, s, r) = l.setback_m;
        let big = (6.0, 999.0);
        let block = Fit {
            front: f,
            side: s,
            rear: r,
            drive: 0.0,
            w: big,
            d: (6.0, 999.0),
            house: false,
        };
        let house = |w: (f32, f32), d: (f32, f32), drive: f32| Fit {
            front: f,
            side: s,
            rear: r,
            drive,
            w,
            d,
            house: true,
        };
        match style {
            // Room for a front lot: the covered walk, two rows of stalls either side of an
            // aisle and the planted strip along the street (squeezed on shallow lots).
            StyleTag::StripMall if self.lots.occupancy(self.zone).is_some() => Fit {
                front: f.max(front_lot_depth(self.lots)),
                side: s.max(1.5),
                w: (20.0, 999.0),
                d: (12.0, 22.0),
                ..block
            },
            StyleTag::StripMall => Fit {
                front: f.max(12.0),
                side: s.max(1.5),
                w: (20.0, 999.0),
                d: (12.0, 22.0),
                ..block
            },
            StyleTag::Motel => Fit {
                front: f.max(3.0),
                side: s.max(2.0),
                w: (22.0, 70.0),
                d: (20.0, 40.0),
                ..block
            },
            StyleTag::Dingbat => Fit {
                front: f.max(2.0),
                side: s.max(1.0),
                w: (11.0, 22.0),
                d: (14.0, 24.0),
                house: true,
                ..block
            },
            StyleTag::Craftsman => house((8.0, 12.5), (8.5, 19.0), 3.0),
            StyleTag::Ranch => house((13.0, 26.0), (8.5, 20.0), 3.0),
            StyleTag::Tract70s => house((10.0, 18.0), (8.5, 18.0), 3.0),
            StyleTag::Glass if self.residential_zone() => house((11.0, 18.0), (8.5, 14.0), 3.0),
            StyleTag::StuccoBox if self.zone == ZoneId::Residential => {
                house((10.0, 22.0), (10.0, 26.0), 0.0)
            }
            StyleTag::StuccoBox if self.residential_zone() => house((11.0, 20.0), (8.5, 18.0), 3.0),
            StyleTag::Warehouse | StyleTag::Brick if self.zone == ZoneId::Industrial => Fit {
                side: s.min(2.0),
                w: (10.0, 999.0),
                d: (12.0, 999.0),
                ..block
            },
            _ => block,
        }
    }

    /// The footprint rectangle for `fit` at coverage `target` (m²) and the room around
    /// it (between the side setbacks, the driveway included, from the front setback to
    /// 1 m off the rear lot line), or `None` if the lot cannot hold it.
    fn place(&self, fit: &Fit, target: f32, drive_left: bool) -> Option<(Rect, Rect)> {
        let lot = self.lot;
        let mid = lot.mid;
        let (t0, t1) = lot.span_t(mid, 0.25)?;
        // Shallow lots squeeze the rear setback (to 1 m), then the front (to 3 m or what
        // the zone asks if less), before giving up.
        let (front, rear) = (fit.front.max(LOT_GAP_M), fit.rear.max(LOT_GAP_M));
        let short = front + rear + fit.d.0 + 0.01 - (t1 - t0);
        let new_rear = (rear - short.max(0.0)).max(rear.min(1.0));
        let short = short - (rear - new_rear);
        let front = (front - short.max(0.0)).max(front.min(3.0));
        let (ta, tb) = (t0 + front, t1 - new_rear);
        if tb - ta < fit.d.0 {
            return None;
        }
        // Houses may build into the rear yard down to half its setback when the coverage
        // target needs the depth, and step back from the street on lots that widen away
        // from it (cul-de-sac wedges).
        let tb = if fit.house {
            t1 - (new_rear * 0.5).max(1.0)
        } else {
            tb
        };
        let steps = if fit.house { 4 } else { 1 };
        let mut found = None;
        for k in 0..steps {
            let ta = ta + 3.0 * k as f32;
            if tb - ta < fit.d.0 {
                break;
            }
            let d0 = (tb - ta).min(fit.d.1);
            let Some((mut sa, mut sb)) = lot.band(mid, ta, ta + d0) else {
                continue;
            };
            sa += fit.side.max(LOT_GAP_M);
            sb -= fit.side.max(LOT_GAP_M);
            let room = Rect {
                s0: sa,
                s1: sb,
                t0: ta,
                t1: t1 - 1.0,
            };
            if fit.drive > 0.0 {
                if drive_left {
                    sa += fit.drive;
                } else {
                    sb -= fit.drive;
                }
            }
            if sb - sa >= fit.w.0 {
                found = Some((ta, d0, sa, sb, room));
                break;
            }
        }
        let (ta, d0, sa, sb, room) = found?;
        let mut w = (sb - sa).min(fit.w.1);
        let d = (target / w).clamp(fit.d.0, d0);
        if fit.house && w * d > target {
            w = (target / d).max(fit.w.0);
        }
        let s0 = if fit.house { (sa + sb - w) * 0.5 } else { sa };
        // The front and rear lot lines are not quite straight: keep clear of them across
        // the whole width.
        let (lo, hi) = lot.band_t(s0, s0 + w, ta + d * 0.5)?;
        let r0 = ta.max(lo + LOT_GAP_M);
        let r1 = (ta + d).min(hi - LOT_GAP_M);
        if r1 - r0 < fit.d.0 - 0.5 {
            return None;
        }
        let r = Rect {
            s0,
            s1: s0 + w,
            t0: r0,
            t1: r1,
        };
        Some((r, room))
    }

    /// The parcel's outline inset by the setbacks (front sides by the front setback, sides
    /// facing back by the rear one, the rest by the side one), cut from the rear down to
    /// the coverage `target` (m²): a footprint for lot-filling buildings on odd lots.
    /// `None` if the inset is not a sound polygon of at least 4 vertices.
    fn inset(&self, fit: &Fit, target: f32) -> Option<Vec<[f32; 2]>> {
        let lot = self.lot;
        let (t0, t1) = lot.span_t(lot.mid, 0.25)?;
        let depth = t1 - t0;
        let poly = simplify(&lot.poly);
        let n = poly.len();
        if n < 4 {
            return None;
        }
        let local = |p: V2| {
            let d = p - lot.o;
            V2::new(d.dot(lot.u), d.dot(lot.v))
        };
        let dist: Vec<f32> = (0..n)
            .map(|i| {
                let (a, b) = (local(poly[i]), local(poly[(i + 1) % n]));
                let along = abs((b - a).normalize().x) > 0.6;
                if abs(a.y) < 0.3 && abs(b.y) < 0.3 {
                    fit.front.max(LOT_GAP_M)
                } else if along && (a.y + b.y) * 0.5 > t0 + depth * 0.5 {
                    fit.rear.max(LOT_GAP_M)
                } else {
                    fit.side.max(LOT_GAP_M)
                }
            })
            .collect();
        let inner = crate::math::offset_polygon(&poly, |i| -dist[i]);
        let mut fp = simplify(&inner);
        if fp.len() < 4 || !simple(&fp) {
            return None;
        }
        // Cut from the rear to the target, keeping the zone's least depth.
        if area(&fp) > target {
            let t_front = fp.iter().map(|&p| local(p).y).fold(f32::MAX, f32::min);
            let t_back = fp.iter().map(|&p| local(p).y).fold(f32::MIN, f32::max);
            let (mut lo, mut hi) = ((t_front + fit.d.0).min(t_back), t_back);
            for _ in 0..24 {
                let m = (lo + hi) * 0.5;
                if area(&clip_depth(&fp, lot, m)) > target {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            fp = simplify(&clip_depth(&fp, lot, hi));
        }
        if fp.len() < 4 || !sound(&fp, &lot.poly) {
            return None;
        }
        Some(fp.iter().map(|p| [p.x, p.y]).collect())
    }

    /// Storeys from the zone's log-normal draw, clamped.
    fn floors(&self, rng: &mut Rng) -> u8 {
        let (lo, hi) = self.mass.floors;
        let s = f64::from(self.mass.floors_spread);
        // Irwin-Hall: the sum of 12 uniforms less 6 is close to a standard normal.
        let mut z = -6.0f64;
        for _ in 0..12 {
            z += f64::from(rng.next_f32());
        }
        let mu = ln(f64::from(self.mass.floors_mean)) - s * s * 0.5;
        let f = exp(mu + s * z);
        let n = (f + 0.5) as i64;
        n.clamp(i64::from(lo), i64::from(hi)) as u8
    }

    /// Which side of the lot a building of `style` leaves room for an alley lot on (a draw
    /// from `rng` for those that may): shops, offices and civic buildings in a zone with
    /// lots and alleys, with chance `rear_lot_256`, on the alley the lot fronts or else the
    /// one along its rear line.
    fn alley_lot(&self, style: StyleTag, rng: &mut Rng) -> Option<AlleySide> {
        let may = self.rule.alleys
            && !self.residential_zone()
            && self.lots.occupancy(self.zone).is_some()
            && !matches!(
                style,
                StyleTag::StripMall
                    | StyleTag::Motel
                    | StyleTag::Dingbat
                    | StyleTag::Warehouse
                    | StyleTag::Barn
            );
        if !may {
            return None;
        }
        let side = if self.on_alley {
            AlleySide::Front
        } else if self.alley_behind() {
            AlleySide::Rear
        } else {
            return None;
        };
        (rng.range(0, 256) < self.lots.rear_lot_256).then_some(side)
    }

    /// Whether an alley runs along the lot's rear line (at its middle).
    fn alley_behind(&self) -> bool {
        let lot = self.lot;
        let Some((_, t1)) = lot.span_t(lot.mid, 0.25) else {
            return false;
        };
        let w = lot.world(lot.mid, t1);
        let q = V2::new(w[0], w[1]);
        self.roads.edges.iter().any(|e| {
            if e.class != RoadClass::Alley {
                return false;
            }
            let reach = e.width_m * 0.5 + ALLEY_REACH_M;
            e.profile.windows(2).any(|p| {
                let (a, b) = (V2::new(p[0][0], p[0][2]), V2::new(p[1][0], p[1][2]));
                segment_distance(q, a, b).0 < reach
            })
        })
    }

    fn built_year(&self, style: StyleTag, rng: &mut Rng) -> Option<u16> {
        let (a, b) = years(style);
        let b = b.min(self.year).max(a);
        Some(a + rng.range(0, u32::from(b - a) + 1) as u16)
    }

    /// The building of an ordinary lot (plus a bungalow's garage).
    fn one(&self, rng: &mut Rng, force: Option<StyleTag>) -> Vec<Building> {
        let Some(mut style) = self.style(rng) else {
            return Vec::new();
        };
        if let Some(f) = force {
            style = f;
        }
        if self.zone == ZoneId::Beach && self.lot.area > 6000.0 {
            // A beach parking lot.
            return Vec::new();
        }
        // Aim a little high: lots that cannot reach the target (shallow, odd, small for
        // their style) pull the zone's share down.
        let aim = rng.range_f32(AIM.0, AIM.1);
        let mut target = self.mass.coverage * self.lot.area * aim;
        if style == StyleTag::Craftsman {
            // Leave the garage its share.
            target -= GARAGE_M.0 * GARAGE_M.1;
        }
        let drive_left = rng.range(0, 2) == 0;
        let mut floors = self.floors(rng);
        let mut fit = self.fit(style);
        let room = self.lots.stall_d + ALLEY_LOT_GAP_M;
        match self.alley_lot(style, rng) {
            Some(AlleySide::Front) => fit.front = fit.front.max(room),
            Some(AlleySide::Rear) => fit.rear = fit.rear.max(room),
            None => {}
        }
        let mut rect = self.place(&fit, target, drive_left);
        if rect.is_none() && fit.drive > 0.0 {
            // A narrow lot: the driveway runs in the side yard instead.
            fit.drive = 0.0;
            rect = self.place(&fit, target, drive_left);
        }
        if rect.is_none() && self.rule.styles.len() > 1 {
            // Too small for the style: the zone's plainest instead.
            style = if self.residential_zone() && self.zone != ZoneId::Residential {
                StyleTag::Tract70s
            } else if self.zone == ZoneId::Industrial {
                StyleTag::Warehouse
            } else {
                StyleTag::StuccoBox
            };
            fit = self.fit(style);
            rect = self.place(&fit, target, drive_left);
            if rect.is_none() && fit.drive > 0.0 {
                fit.drive = 0.0;
                rect = self.place(&fit, target, drive_left);
            }
        }
        let Some((r, room)) = rect else {
            return Vec::new();
        };
        // Masonry and stucco stay under the old height limit; anything taller is a
        // post-war glass tower (where the place's year has them).
        let old = matches!(
            style,
            StyleTag::Deco | StyleTag::Brick | StyleTag::StuccoBox
        );
        if old && floors > MASONRY_FLOORS {
            if years(StyleTag::Glass).0 <= self.year {
                style = StyleTag::Glass;
            } else {
                floors = MASONRY_FLOORS;
            }
        }
        let year = self.built_year(style, rng);
        let (usage, roof);
        match style {
            StyleTag::StripMall => {
                floors = 1;
                usage = BuildingUse::Commercial;
                roof = Roof::Flat;
            }
            StyleTag::Motel => {
                floors = 2;
                usage = BuildingUse::Hotel;
                roof = Roof::Flat;
            }
            StyleTag::Dingbat => {
                // Two storeys of flats over the carport level.
                floors = 3;
                usage = BuildingUse::Residential;
                roof = Roof::Flat;
            }
            StyleTag::Craftsman => {
                floors = 1;
                usage = BuildingUse::Residential;
                roof = Roof::Gable;
            }
            StyleTag::Ranch => {
                floors = 1;
                usage = BuildingUse::Residential;
                roof = if rng.range(0, 2) == 0 {
                    Roof::Hip
                } else {
                    Roof::Gable
                };
            }
            StyleTag::Tract70s => {
                floors = floors.min(2);
                usage = BuildingUse::Residential;
                roof = if rng.range(0, 10) < 7 {
                    Roof::Gable
                } else {
                    Roof::Hip
                };
            }
            StyleTag::Warehouse => {
                floors = 1;
                usage = BuildingUse::Industrial;
                roof = Roof::Flat;
            }
            StyleTag::Barn => {
                floors = 1;
                usage = BuildingUse::Industrial;
                roof = Roof::Gable;
            }
            _ if self.residential_zone() => {
                floors = floors.min(2);
                usage = BuildingUse::Residential;
                roof = if style == StyleTag::Glass {
                    Roof::Flat
                } else {
                    match rng.range(0, 20) {
                        0..=10 => Roof::Hip,
                        11..=14 => Roof::Gable,
                        _ => Roof::Flat,
                    }
                };
            }
            _ => {
                usage = match self.zone {
                    ZoneId::Industrial => BuildingUse::Industrial,
                    ZoneId::Downtown if floors > 4 => {
                        if style == StyleTag::Deco && rng.range(0, 4) == 0 {
                            BuildingUse::Hotel
                        } else {
                            BuildingUse::Office
                        }
                    }
                    _ => BuildingUse::Commercial,
                };
                roof = Roof::Flat;
            }
        }
        // Narrow lots stay low: no pencil towers.
        let short = r.w().min(r.d());
        let cap = ((short * 0.9) as u8).max(2);
        floors = floors.min(cap).max(1);
        let pts = self.shape(style, floors, &r, rng);
        // Shops, offices and industry with party walls follow the lot's own outline.
        let block = !fit.house
            && pts.len() == 4
            && matches!(
                style,
                StyleTag::StuccoBox
                    | StyleTag::Deco
                    | StyleTag::Brick
                    | StyleTag::Glass
                    | StyleTag::Warehouse
            );
        let footprint = block
            .then(|| self.inset(&fit, target))
            .flatten()
            .unwrap_or_else(|| self.lot.footprint(&pts));
        let rect = footprint.len() == 4;
        let mut out = vec![Building {
            footprint,
            height_m: height(style, usage, floors, roof, rng),
            floors,
            roof: if rect { roof } else { Roof::Flat },
            style,
            built_year: year,
            usage,
        }];
        let t0 = r.t1 + 2.0;
        let (gw, gd) = GARAGE_M;
        if style == StyleTag::Craftsman && fit.drive > 0.0 && t0 + gd <= room.t1 {
            // A detached garage at the end of the driveway, behind the house.
            let (g0, g1) = if drive_left {
                (room.s0, room.s0 + gw)
            } else {
                (room.s1 - gw, room.s1)
            };
            let g = Rect {
                s0: g0,
                s1: g1,
                t0,
                t1: t0 + gd,
            };
            out.push(Building {
                footprint: self.lot.footprint(&g.pts()),
                height_m: 2.9,
                floors: 1,
                roof: Roof::Gable,
                style,
                built_year: year,
                usage: BuildingUse::Parking,
            });
        }
        out
    }

    /// The footprint outline in lot coordinates: the rectangle, or an L or U.
    fn shape(&self, style: StyleTag, floors: u8, r: &Rect, rng: &mut Rng) -> Vec<(f32, f32)> {
        let (w, d) = (r.w(), r.d());
        let flip = rng.range(0, 2) == 0;
        let roll = rng.range(0, 100);
        let wing = MOTEL_WING_M;
        if style == StyleTag::Motel {
            // Wings around a parking court open to the street.
            if w >= 34.0 && roll < 50 {
                return u_shape(r, wing, d - wing, true);
            }
            return l_shape(r, w - wing, d - wing, flip);
        }
        let low_flat = matches!(
            style,
            StyleTag::StuccoBox | StyleTag::Deco | StyleTag::Brick | StyleTag::Dingbat
        ) && floors <= crate::buildings::STEP_BACK_FLOORS
            && !self.residential_zone();
        if low_flat && w > 20.0 && d > 18.0 {
            let big = self.zone == ZoneId::Downtown && w > 26.0 && d > 24.0;
            if big && roll < 30 {
                // A light court open to the rear.
                return u_shape(r, (w * 0.32).max(7.0), d * 0.4, false);
            }
            if roll < 20 {
                return l_shape(r, w * 0.4, d * 0.35, flip);
            }
        }
        r.pts().to_vec()
    }

    /// A farmstead: a house as near the road as it fits, a barn beside or behind it and a
    /// silo by the barn when there is room. Farmsteads are odd shapes cut around a
    /// driveway's end, so each piece takes the first spot on a 3 m grid, scanned from the
    /// front middle outward, where it lies inside the parcel clear of the others.
    fn farm(&self, rng: &mut Rng) -> Vec<Building> {
        let lot = self.lot;
        let (hw, hd) = (rng.range_f32(13.0, 17.0), rng.range_f32(9.5, 12.0));
        let (bw, bd) = (rng.range_f32(12.0, 15.0), rng.range_f32(18.0, 24.0));
        let house_roof = if rng.range(0, 2) == 0 {
            Roof::Gable
        } else {
            Roof::Hip
        };
        let barn_h = rng.range_f32(4.6, 5.6);
        let silo_h = rng.range_f32(13.0, 17.0);
        let want_silo = rng.range(0, 100) < 75;
        let (mut s_lo, mut s_hi, mut t_lo, mut t_hi) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &lot.local {
            s_lo = s_lo.min(p.x);
            s_hi = s_hi.max(p.x);
            t_lo = t_lo.min(p.y);
            t_hi = t_hi.max(p.y);
        }
        let mut placed: Vec<Vec<V2>> = Vec::new();
        // The first free spot for a `w x d` rectangle (or octagon of radius `w / 2`),
        // `gap` clear of what is placed, scanning rows from the front.
        let mut spot = |w: f32, d: f32, gap: f32, octagon: bool| -> Option<Vec<[f32; 2]>> {
            let cols = ((s_hi - s_lo) / 3.0) as i32 + 1;
            let mut t = t_lo + 2.0;
            while t + d < t_hi {
                for k in 0..cols {
                    // From the middle of the front outward, alternating sides.
                    let off = ((k + 1) / 2) as f32 * 3.0 * if k % 2 == 0 { 1.0 } else { -1.0 };
                    let s0 = lot.mid + off - w * 0.5;
                    let pts: Vec<(f32, f32)> = if octagon {
                        octagon_pts(s0 + w * 0.5, t + d * 0.5, w * 0.5)
                    } else {
                        Rect {
                            s0,
                            s1: s0 + w,
                            t0: t,
                            t1: t + d,
                        }
                        .pts()
                        .to_vec()
                    };
                    let fp = lot.footprint(&pts);
                    let v: Vec<V2> = fp.iter().map(|&p| V2::from_array(p)).collect();
                    let grown: Vec<V2> = crate::math::offset_polygon(&v, |_| gap);
                    if sound(&v, &lot.poly) && placed.iter().all(|o| disjoint(&grown, o)) {
                        placed.push(v);
                        return Some(fp);
                    }
                }
                t += 3.0;
            }
            None
        };
        let Some(house) = spot(hw, hd, 0.0, false) else {
            return Vec::new();
        };
        let mut out = vec![Building {
            footprint: house,
            height_m: 3.2,
            floors: 1,
            roof: house_roof,
            style: StyleTag::Ranch,
            built_year: self.built_year(StyleTag::Ranch, rng),
            usage: BuildingUse::Residential,
        }];
        let year = self.built_year(StyleTag::Barn, rng);
        if let Some(barn) = spot(bw, bd, 8.0, false) {
            out.push(Building {
                footprint: barn,
                height_m: barn_h,
                floors: 1,
                roof: Roof::Gable,
                style: StyleTag::Barn,
                built_year: year,
                usage: BuildingUse::Industrial,
            });
            if want_silo {
                if let Some(silo) = spot(2.0 * SILO_R_M, 2.0 * SILO_R_M, 2.0, true) {
                    out.push(Building {
                        footprint: silo,
                        height_m: silo_h,
                        floors: 1,
                        roof: Roof::Flat,
                        style: StyleTag::Barn,
                        built_year: year,
                        usage: BuildingUse::Industrial,
                    });
                }
            }
        }
        out
    }

    /// Whether the macro ground under `fp` falls at most [`MAX_FALL_M`].
    fn flat_enough(&self, fp: &[V2]) -> bool {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut c = V2::new(0.0, 0.0);
        for &p in fp {
            let h = self.m.height_at(p.x, p.y);
            lo = lo.min(h);
            hi = hi.max(h);
            c = c + p * (1.0 / fp.len() as f32);
        }
        let h = self.m.height_at(c.x, c.y);
        lo = lo.min(h);
        hi = hi.max(h);
        hi - lo <= MAX_FALL_M
    }
}

/// A regular octagon of radius `r` around `(s, t)`, vertices at 22.5° + k·45°.
fn octagon_pts(s: f32, t: f32, r: f32) -> Vec<(f32, f32)> {
    const C: [(f32, f32); 8] = [
        (0.923_88, 0.382_683),
        (0.382_683, 0.923_88),
        (-0.382_683, 0.923_88),
        (-0.923_88, 0.382_683),
        (-0.923_88, -0.382_683),
        (-0.382_683, -0.923_88),
        (0.382_683, -0.923_88),
        (0.923_88, -0.382_683),
    ];
    C.iter().map(|&(x, y)| (s + x * r, t + y * r)).collect()
}

/// An L: the rectangle less its rear corner `cw x cd`, on the left (`flip`) or right.
fn l_shape(r: &Rect, cw: f32, cd: f32, flip: bool) -> Vec<(f32, f32)> {
    let tm = r.t1 - cd;
    if flip {
        let sm = r.s0 + cw;
        vec![
            (r.s0, r.t0),
            (r.s1, r.t0),
            (r.s1, r.t1),
            (sm, r.t1),
            (sm, tm),
            (r.s0, tm),
        ]
    } else {
        let sm = r.s1 - cw;
        vec![
            (r.s0, r.t0),
            (r.s1, r.t0),
            (r.s1, tm),
            (sm, tm),
            (sm, r.t1),
            (r.s0, r.t1),
        ]
    }
}

/// A U open to the front (`front`) or rear: wings `wing` wide, the middle cut `cd` deep.
/// Motels open to the street; light courts to the rear.
fn u_shape(r: &Rect, wing: f32, cd: f32, front: bool) -> Vec<(f32, f32)> {
    let (a, b) = (r.s0 + wing, r.s1 - wing);
    if front {
        let tm = r.t0 + cd;
        return vec![
            (r.s0, r.t0),
            (a, r.t0),
            (a, tm),
            (b, tm),
            (b, r.t0),
            (r.s1, r.t0),
            (r.s1, r.t1),
            (r.s0, r.t1),
        ];
    }
    let tm = r.t1 - cd;
    vec![
        (r.s0, r.t0),
        (r.s1, r.t0),
        (r.s1, r.t1),
        (b, r.t1),
        (b, tm),
        (a, tm),
        (a, r.t1),
        (r.s0, r.t1),
    ]
}

/// Wall top by style, use and storeys (parapets included; pitched roofs rise above it).
fn height(style: StyleTag, usage: BuildingUse, floors: u8, roof: Roof, rng: &mut Rng) -> f32 {
    let n = f32::from(floors);
    let pitched = roof != Roof::Flat;
    match (style, usage) {
        (StyleTag::Dingbat, _) => 2.7 + 2.0 * 2.9 + 0.6,
        (StyleTag::Motel, _) => 2.0 * 3.0 + 0.6,
        (StyleTag::StripMall, _) => 5.0 + 1.8,
        (StyleTag::Warehouse, _) => rng.range_f32(7.5, 10.0) + 1.0,
        (StyleTag::Glass, BuildingUse::Office) => 4.6 + (n - 1.0) * 3.7 + 1.0,
        (_, BuildingUse::Residential) => n * 2.95 + if pitched { 0.3 } else { 0.6 },
        (_, BuildingUse::Industrial) => n * 5.0 + 1.0,
        (_, BuildingUse::Commercial) => {
            let parapet = if style == StyleTag::StuccoBox {
                0.8
            } else {
                1.0
            };
            4.4 + (n - 1.0) * 3.5 + parapet
        }
        _ => 4.2 + (n - 1.0) * 3.6 + 1.2,
    }
}

// ---------------------------------------------------------------------------------------
// Geometry.

/// Drops vertices closer than 0.1 m to the previous one and vertices within 0.02 m of the
/// line through their neighbours (less than [`LOT_GAP_M`], so an inset of the simplified
/// outline stays inside the original), until none is left to drop.
fn simplify(poly: &[V2]) -> Vec<V2> {
    let mut v: Vec<V2> = poly.to_vec();
    loop {
        let n = v.len();
        if n <= 3 {
            return v;
        }
        let drop = (0..n).find(|&i| {
            let (a, p, b) = (v[(i + n - 1) % n], v[i], v[(i + 1) % n]);
            if (p - a).length() < 0.1 {
                return true;
            }
            let ab = b - a;
            let l = ab.length();
            l > 1e-4 && abs(ab.normalize().left().dot(p - a)) < 0.02
        });
        match drop {
            Some(i) => {
                v.remove(i);
            }
            None => return v,
        }
    }
}

/// The part of counter-clockwise `poly` at most `t` deep in the lot frame
/// (Sutherland-Hodgman against one half-plane).
fn clip_depth(poly: &[V2], lot: &Lot, t: f32) -> Vec<V2> {
    let depth = |p: V2| (p - lot.o).dot(lot.v) - t;
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (da, db) = (depth(a), depth(b));
        if da <= 0.0 {
            out.push(a);
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// The part of counter-clockwise `poly` inside the place rectangle `[0, extent]`
/// (Sutherland-Hodgman against its four sides); `poly` itself when it lies inside.
pub(crate) fn clip_to_place(poly: Vec<V2>, extent: [f32; 2]) -> Vec<V2> {
    let inside = |p: &V2| p.x >= 0.0 && p.y >= 0.0 && p.x <= extent[0] && p.y <= extent[1];
    if poly.iter().all(inside) {
        return poly;
    }
    // Each side as (axis, sign, bound): a point is kept where `sign * (p[axis] - bound)`
    // is at most 0.
    let sides = [
        (0, -1.0f32, 0.0f32),
        (1, -1.0, 0.0),
        (0, 1.0, extent[0]),
        (1, 1.0, extent[1]),
    ];
    let mut cur = poly;
    for (axis, sign, bound) in sides {
        let out_by = |p: V2| sign * (if axis == 0 { p.x } else { p.y } - bound);
        let n = cur.len();
        let mut next = Vec::with_capacity(n + 2);
        for i in 0..n {
            let (a, b) = (cur[i], cur[(i + 1) % n]);
            let (da, db) = (out_by(a), out_by(b));
            if da <= 0.0 {
                next.push(a);
            }
            if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
                next.push(a + (b - a) * (da / (da - db)));
            }
        }
        cur = next;
    }
    cur
}

/// Simple and counter-clockwise.
fn simple(poly: &[V2]) -> bool {
    let arr: Vec<[f32; 2]> = poly.iter().map(|p| [p.x, p.y]).collect();
    place::validate::is_simple_ccw(&arr)
}

fn area(poly: &[V2]) -> f32 {
    let mut acc = 0.0f64;
    for (i, p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        acc += f64::from(p.y) * f64::from(q.x) - f64::from(p.x) * f64::from(q.y);
    }
    (acc * 0.5) as f32
}

type P = [f64; 2];

fn p64(p: V2) -> P {
    [f64::from(p.x), f64::from(p.y)]
}

fn orient(a: P, b: P, c: P) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Whether segments `p1 p2` and `q1 q2` share any point.
fn touch(p1: P, p2: P, q1: P, q2: P) -> bool {
    let (d1, d2) = (orient(q1, q2, p1), orient(q1, q2, p2));
    let (d3, d4) = (orient(p1, p2, q1), orient(p1, p2, q2));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on = |a: P, b: P, c: P, d: f64| {
        d == 0.0
            && c[0] >= a[0].min(b[0])
            && c[0] <= a[0].max(b[0])
            && c[1] >= a[1].min(b[1])
            && c[1] <= a[1].max(b[1])
    };
    on(q1, q2, p1, d1) || on(q1, q2, p2, d2) || on(p1, p2, q1, d3) || on(p1, p2, q2, d4)
}

fn edges_touch(a: &[V2], b: &[V2]) -> bool {
    let (n, m) = (a.len(), b.len());
    for i in 0..n {
        let (p1, p2) = (p64(a[i]), p64(a[(i + 1) % n]));
        for j in 0..m {
            if touch(p1, p2, p64(b[j]), p64(b[(j + 1) % m])) {
                return true;
            }
        }
    }
    false
}

/// A kept footprint: at least 4 vertices and [`MIN_FOOTPRINT_M2`], sides of at least
/// [`MIN_SIDE_M`], simple and counter-clockwise, strictly inside the parcel.
fn sound(fp: &[V2], parcel: &[V2]) -> bool {
    let n = fp.len();
    if n < 4 || area(fp) < MIN_FOOTPRINT_M2 {
        return false;
    }
    // Outlines that follow the lot may have short jogs; silos (octagons) short sides.
    let min_side = if n == 4 { MIN_SIDE_M } else { 1.0 };
    if (0..n).any(|i| (fp[(i + 1) % n] - fp[i]).length() < min_side) {
        return false;
    }
    let arr: Vec<[f32; 2]> = fp.iter().map(|p| [p.x, p.y]).collect();
    if !place::validate::is_simple_ccw(&arr) {
        return false;
    }
    fp.iter().all(|&p| crate::math::point_in_polygon(p, parcel))
        && !parcel.iter().any(|&p| crate::math::point_in_polygon(p, fp))
        && !edges_touch(fp, parcel)
}

/// Whether two footprints share no point.
fn disjoint(a: &[V2], b: &[V2]) -> bool {
    !edges_touch(a, b)
        && !crate::math::point_in_polygon(a[0], b)
        && !crate::math::point_in_polygon(b[0], a)
}

// ---------------------------------------------------------------------------------------
// exp and ln as plain f64 series, the same on every platform.

const LN2: f64 = core::f64::consts::LN_2;

/// `e^x` for `|x| < 700`: `2^k · e^r` with `|r| <= ln 2 / 2` and a Taylor series.
fn exp(x: f64) -> f64 {
    let x = x.clamp(-700.0, 700.0);
    let k = floor_f64(x / LN2 + 0.5);
    let r = x - k * LN2;
    let (mut term, mut sum) = (1.0f64, 1.0f64);
    for i in 1..18 {
        term = term * r / f64::from(i);
        sum += term;
    }
    sum * f64::from_bits(((k as i64 + 1023) as u64) << 52)
}

/// Natural log for `x > 0`: `e·ln 2 + ln m` with `m` in `[1, 2)` and the atanh series.
fn ln(x: f64) -> f64 {
    if x <= 0.0 {
        return f64::MIN;
    }
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7FF) as i64 - 1023;
    let m = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0x3FF0_0000_0000_0000);
    let y = (m - 1.0) / (m + 1.0);
    let y2 = y * y;
    let (mut term, mut sum) = (y, 0.0f64);
    for i in 0..40 {
        sum += term / f64::from(2 * i + 1);
        term *= y2;
    }
    e as f64 * LN2 + 2.0 * sum
}

fn floor_f64(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_match_known_values() {
        assert!((exp(1.0) - core::f64::consts::E).abs() < 1e-12);
        assert!((exp(-3.5) - 0.030_197_383_422_318_5).abs() < 1e-14);
        assert!((ln(8.0) - 3.0 * LN2).abs() < 1e-12);
        assert!((ln(exp(2.7)) - 2.7).abs() < 1e-12);
    }

    #[test]
    fn bands_follow_slanted_sides() {
        // A trapezoid in lot coordinates: front 0..20 at t = 0, back 2..16 at t = 30.
        let poly = [
            V2::new(0.0, 0.0),
            V2::new(20.0, 0.0),
            V2::new(16.0, 30.0),
            V2::new(2.0, 30.0),
        ];
        let lot = Lot {
            o: V2::new(0.0, 0.0),
            u: V2::new(1.0, 0.0),
            v: V2::new(0.0, 1.0),
            mid: 10.0,
            poly: poly.to_vec(),
            local: poly.to_vec(),
            area: 0.0,
        };
        let (a, b) = lot.band(10.0, 0.0, 15.0).unwrap();
        assert!((a - 1.0).abs() < 1e-2 && (b - 18.0).abs() < 1e-2, "{a} {b}");
        assert_eq!(lot.span_t(10.0, 0.25).map(|s| s.1), Some(30.0));
    }

    #[test]
    fn lots_on_the_edge_are_clipped_to_the_place() {
        // A lot straddling z = 0: only its half inside is kept, still counter-clockwise.
        let poly = vec![
            V2::new(10.0, -10.0),
            V2::new(10.0, 10.0),
            V2::new(30.0, 10.0),
            V2::new(30.0, -10.0),
        ];
        let c = clip_to_place(poly.clone(), [100.0, 100.0]);
        assert!(c.iter().all(|p| p.y >= 0.0));
        assert!((area(&c) - 200.0).abs() < 1e-3, "{c:?}");
        // Inside: untouched. Outside: nothing left.
        let inner: Vec<V2> = poly.iter().map(|&p| p + V2::new(0.0, 20.0)).collect();
        assert_eq!(clip_to_place(inner.clone(), [100.0, 100.0]), inner);
        let outer: Vec<V2> = poly.iter().map(|&p| p + V2::new(200.0, 0.0)).collect();
        assert!(clip_to_place(outer, [100.0, 100.0]).len() < 3);
    }
}
