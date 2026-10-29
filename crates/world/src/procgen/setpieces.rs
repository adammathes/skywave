//! A basin's set pieces (`docs/WORLD_GENERATION.md`, "Set pieces"; `docs/backlog/
//! MIRAGE2001.md` item 7): places, not missions. [`site`] finds where each goes on the
//! basin's layout once its buildings stand; [`clear`] takes the buildings off their lots;
//! [`SetPieces::pois`] are their landmarks; [`dress`] (from the detailing) lays their props
//! and lights.
//!
//! - **The tunnel**: the straightest stretch of the freeway nearest downtown that no other
//!   road comes near, roofed over as a tiled box [`TUNNEL_M`] long with its own lamps.
//! - **The crew garage** and **the taco stand**: industrial or commercial lots on an
//!   arterial deep and wide enough for their footprints, the garage's nearest the
//!   boulevard, the stand's the next at least [`APART_M`] from it; the lot's buildings go.
//!   The garage's three bays hold the world's hero cars, a third on the apron; the stand's
//!   lot has picnic tables and a few parked cars.
//! - **The drag strip** on the airfield's runway: a start line [`STRIP_IN_M`] from its
//!   taxiway end, a finish a quarter mile on (an eighth on a short runway), the start tree,
//!   the timing board beside the strip and lamps along it.
//! - **The overlooks**: a turnout at the end of each overlook road, a guard rail along its
//!   edge, benches and a lamp.
//! - **The channel ramps**: chain-link along both lips of the flood channel, open where a
//!   road bridges it, around each ramp and at its top.
//! - **The landmarks** built from kits ([`crate::landmarks`]; the variant each wears is the
//!   owner's pick from the gallery's contact sheet, [`LETTERS`], [`WATER_TOWER`],
//!   [`DRIVE_IN`]): the hillside letters on the slope of the hills [`LETTERS_RISE_M`] above
//!   their foot, square to the slope and across from downtown, clear of every road; the
//!   water tower on the industrial lot with the most industry round it, at least
//!   [`TOWER_APART_M`] from the garage and the stand; the drive-in on the commercial,
//!   strip, industrial or desert lot nearest the boulevard at least [`APART_M`] from the
//!   other lots, its rows filling with cars after dark. A kit the grammar does not carry
//!   is not sited.
//!
//! Every draw is a pure function of the place; nothing here draws randomness but the
//! signs' name seeds (`hash64(seed, [SETPIECES, ..])`).

use alloc::vec;
use alloc::vec::Vec;

use place::{
    Light, LightKind, LotKind, Place, Poi, PoiKind, Prop, PropKind, RoadClass, ZoneId, ALL_HOURS,
    SIDEWALK_WIDTH_M,
};
use skywave_core::Seed;

use super::macro_map::{MacroMap, MacroPoiKind};
use crate::landmarks::Kit;
use crate::lots::LOT_CAR;
use crate::math::{abs, point_in_polygon, segment_distance, V2, V3};
use crate::mesh::terrain::TerrainGrid;
use crate::palette::Palette;
use crate::props::yaw_of_dir;
use crate::setpieces::{bay_x, GARAGE_M, STAND_M, TUNNEL_H_M};

/// Key of the set pieces' name seeds.
const SETPIECES: u64 = 0x5345_5450;
/// A tunnel's length and the clear width either side of the freeway's asphalt, m.
pub const TUNNEL_M: f32 = 128.0;
const TUNNEL_SHOULDER_M: f32 = 1.5;
/// A tunnel is laid in pieces this long...
const TUNNEL_PIECE_M: f32 = 16.0;
/// ...on a stretch that strays at most this far from its chord, with no other road
/// within this of its walls, m.
const TUNNEL_STRAIGHT_M: f32 = 0.75;
const TUNNEL_CLEAR_M: f32 = 2.0;
/// Its ceiling lamps' lights: one each side every this far, m.
const TUNNEL_LIGHT_STEP_M: f32 = 16.0;
/// A garage's lot: front width and depth from the property line (apron, then the garage).
const GARAGE_LOT_M: (f32, f32) = (24.0, 25.0);
const APRON_M: f32 = 9.0;
/// A taco stand's lot.
const STAND_LOT_M: (f32, f32) = (22.0, 20.0);
/// The garage and the stand stand at least this far apart, m.
pub const APART_M: f32 = 300.0;
/// The drag strip's start line lies this far down the runway from its taxiway end; its
/// finish a quarter mile on, or an eighth where the runway is short, m.
pub const STRIP_IN_M: f32 = 60.0;
pub const QUARTER_MILE_M: f32 = 402.336;
/// Room the strip leaves past its finish to stop in, m.
const SHUTDOWN_M: f32 = 150.0;
/// Lamps along the strip every this far, from the start line, m.
const STRIP_LAMP_STEP_M: f32 = 100.0;
/// Chain-link runs along the channel at most this long, and keeps this far from a road's
/// asphalt and outside the channel's lip, m.
const FENCE_RUN_M: f32 = 9.0;
const FENCE_ROAD_CLEAR_M: f32 = 4.0;
const FENCE_OUT_M: f32 = 0.6;
/// An overlook's turnout: its guard rail this far past the road's end, this long, m.
const RAIL_OUT_M: f32 = 9.0;
const RAIL_M: f32 = 18.0;
/// The landmarks a basin builds from kits: `(kit id, variant)`, the variant the owner's
/// pick from the gallery's contact sheet (`docs/WORLD_GENERATION.md`, "Landmarks").
pub const LETTERS: (&str, u8) = ("hillside_letters", 0);
/// B, the pedestal (the owner's pick from the gallery sample).
pub const WATER_TOWER: (&str, u8) = ("water_tower", 1);
pub const DRIVE_IN: (&str, u8) = ("drive_in", 0);
/// The letters stand where the hills have risen this far over the road along their foot,
/// their faces this far back on their lot, their line level to within this, and no road
/// comes within this of their lot, m.
pub const LETTERS_RISE_M: f32 = 50.0;
const LETTERS_FACE_M: f32 = 8.6;
const LETTERS_LEVEL_M: f32 = 3.0;
const LETTERS_ROAD_CLEAR_M: f32 = 12.0;
/// The water tower stands at least this far from the garage and the stand, and is scored
/// by the industry within this of it, m.
pub const TOWER_APART_M: f32 = 200.0;
const TOWER_REACH_M: f32 = 192.0;
/// The drive-in's car rows in its kit's frame (`assets/landmarks/drive_in.ron`): `x` of
/// each stall and `z` of each row, m, facing the screen.
const DRIVE_IN_STALLS: [f32; 5] = [-12.0, -6.0, 0.0, 6.0, 12.0];
const DRIVE_IN_ROWS: [f32; 3] = [-17.0, -25.0, -33.0];

/// A site a set piece stands on: the middle of its front at the property line, the way
/// into it from the road, the way along the road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lot {
    pub front: V2,
    pub inward: V2,
    pub along: V2,
    /// Orders sites alike in every other way.
    pub key: u64,
}

impl Lot {
    /// The point `x` along the front and `z` into the lot.
    fn at(&self, x: f32, z: f32) -> V2 {
        self.front + self.along * x + self.inward * z
    }

    /// The lot's rectangle `w` wide and `d` deep, counter-clockwise or not.
    fn rect(&self, w: f32, d: f32) -> Vec<V2> {
        vec![
            self.at(-0.5 * w, 0.0),
            self.at(0.5 * w, 0.0),
            self.at(0.5 * w, d),
            self.at(-0.5 * w, d),
        ]
    }
}

/// A landmark built from a kit on its own lot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub lot: Lot,
    pub kind: PoiKind,
    /// The kit's index in the grammar's and the variant it wears.
    pub kit: u8,
    pub variant: u8,
    /// Its lot's width and depth, m.
    pub size: [f32; 2],
}

impl Mark {
    /// The middle of its lot.
    fn middle(&self) -> V2 {
        self.lot.at(0.0, 0.5 * self.size[1])
    }
}

/// Where a basin's set pieces go (see the module docs); empty on any other place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetPieces {
    /// The tunnel's chord: its two ends on the freeway's centerline.
    pub tunnel: Option<[V2; 2]>,
    pub tunnel_width_m: f32,
    pub garage: Option<Lot>,
    pub stand: Option<Lot>,
    /// The drag strip: its start line's middle, the way down it, the distance to the
    /// finish and the runway's width.
    pub strip: Option<(V2, V2, f32, f32)>,
    /// Each overlook's turnout: the road's end (or the overlook) and the way out over the
    /// basin.
    pub overlooks: Vec<(V2, V2)>,
    /// The channel's ramps, top to foot, and its centerline's ends and half width at its
    /// lips.
    pub ramps: Vec<[V2; 2]>,
    pub channel: Option<(V2, V2, f32)>,
    /// Where the rail line crosses a road at grade, in order along it.
    pub crossings: Vec<V2>,
    /// The landmarks built from kits: the letters, the water tower, the drive-in, those
    /// that found a site.
    pub marks: Vec<Mark>,
}

impl SetPieces {
    /// Their landmarks.
    pub fn pois(&self, seed: Seed) -> Vec<Poi> {
        let mut out = Vec::new();
        let mut push = |pos: V2, kind: PoiKind| {
            let k = out.len() as u64;
            out.push(Poi {
                pos: [pos.x, pos.y],
                kind,
                name_seed: skywave_core::hash64(seed.0, &[SETPIECES, kind as u64, k]),
            });
        };
        if let Some([a, b]) = self.tunnel {
            push(a.lerp(b, 0.5), PoiKind::Tunnel);
        }
        if let Some(l) = &self.garage {
            push(l.at(0.0, APRON_M), PoiKind::CrewGarage);
        }
        if let Some(l) = &self.stand {
            push(l.at(0.0, 0.5 * STAND_LOT_M.1), PoiKind::TacoStand);
        }
        if let Some((start, _, _, _)) = self.strip {
            push(start, PoiKind::DragStrip);
        }
        for &(at, _) in &self.overlooks {
            push(at, PoiKind::Overlook);
        }
        for r in &self.ramps {
            push(r[0], PoiKind::ChannelRamp);
        }
        for &c in &self.crossings {
            push(c, PoiKind::GradeCrossing);
        }
        for mark in &self.marks {
            push(mark.middle(), mark.kind);
        }
        out
    }

    /// The ground each set piece keeps for itself: no other prop stands there.
    fn footprints(&self) -> Vec<Vec<V2>> {
        let mut out = Vec::new();
        if let Some([a, b]) = self.tunnel {
            let d = (b - a).normalize();
            let r = d.right() * (0.5 * self.tunnel_width_m + 3.0);
            let (a, b) = (a - d * 2.0, b + d * 2.0);
            out.push(vec![a - r, b - r, b + r, a + r]);
        }
        if let Some(l) = &self.garage {
            out.push(l.rect(GARAGE_LOT_M.0, GARAGE_LOT_M.1));
        }
        if let Some(l) = &self.stand {
            out.push(l.rect(STAND_LOT_M.0, STAND_LOT_M.1));
        }
        if let Some((start, dir, run, width)) = self.strip {
            let r = dir.right() * (0.5 * width + 3.0);
            let (a, b) = (start - dir * 40.0, start + dir * (run + 40.0));
            out.push(vec![a - r, b - r, b + r, a + r]);
        }
        for &(at, out_dir) in &self.overlooks {
            let r = out_dir.right() * (0.5 * RAIL_M + 2.0);
            let (a, b) = (at - out_dir * 2.0, at + out_dir * (RAIL_OUT_M + 2.0));
            out.push(vec![a - r, b - r, b + r, a + r]);
        }
        for mark in &self.marks {
            out.push(mark.lot.rect(mark.size[0], mark.size[1]));
        }
        out
    }
}

/// Distance from `p` to the polyline `pts`.
fn to_line(p: V2, pts: &[V2]) -> f32 {
    pts.windows(2)
        .map(|w| segment_distance(p, w[0], w[1]).0)
        .fold(f32::INFINITY, f32::min)
}

/// Distance between the segments `a..b` and `c..d`: 0 where they cross.
fn segments_distance(a: V2, b: V2, c: V2, d: V2) -> f32 {
    let side = |p: V2, q: V2, r: V2| (q - p).turn(r - p);
    let crosses = side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0;
    if crosses {
        return 0.0;
    }
    [
        segment_distance(a, c, d).0,
        segment_distance(b, c, d).0,
        segment_distance(c, a, b).0,
        segment_distance(d, a, b).0,
    ]
    .into_iter()
    .fold(f32::INFINITY, f32::min)
}

/// Whether edge `e` is the freeway itself (not a ramp, overpass or street).
fn is_freeway(e: &place::Edge) -> bool {
    e.class == RoadClass::Highway && !e.oneway && e.lanes_fwd >= 2 && e.lanes_back >= 2
}

/// The height of the road surface at `q` on the nearest edge whose asphalt holds it.
fn road_y(place: &Place, q: V2, keep: impl Fn(&place::Edge) -> bool) -> Option<f32> {
    let mut best: Option<(f32, f32)> = None;
    for e in place.roads.edges.iter().filter(|e| keep(e)) {
        for w in e.profile.windows(2) {
            let (a, b) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
            let (d, t) = segment_distance(q, a, b);
            if d <= 0.5 * e.width_m + 0.5 && best.is_none_or(|x| d < x.0) {
                best = Some((d, w[0][1] + (w[1][1] - w[0][1]) * t));
            }
        }
    }
    best.map(|b| b.1)
}

/// Kit `id` in `kits`: its index, `variant` and its lot; `None` if the grammar does not
/// carry it or it has no such variant.
fn kit_of(kits: &[Kit], (id, variant): (&str, u8)) -> Option<(u8, u8, [f32; 2])> {
    let k = kits.iter().position(|k| k.id == id)?;
    (usize::from(variant) < kits[k].variants.len()).then(|| (k as u8, variant, kits[k].lot))
}

/// Whether the lot `l`, `w` wide and `d` deep, overlaps one of `areas`.
fn overlaps(l: &Lot, (w, d): (f32, f32), areas: &[Vec<V2>]) -> bool {
    let rect = l.rect(w, d);
    areas.iter().any(|a| {
        samples(l, w, d).any(|q| point_in_polygon(q, a))
            || a.iter().any(|&q| point_in_polygon(q, &rect))
    })
}

/// Where the set pieces of the place `m` describes go, its roads, lots and buildings
/// standing (see the module docs), the landmarks from `kits` (the grammar's). Empty unless
/// `m` is a basin.
pub fn site(m: &MacroMap, place: &Place, kits: &[Kit]) -> SetPieces {
    let Some(b) = m.basin.as_ref() else {
        return SetPieces::default();
    };
    let mut s = SetPieces {
        tunnel_width_m: 0.0,
        ..SetPieces::default()
    };
    // The tunnel.
    let downtown = V2::from_array(m.downtown);
    let hwy: Vec<V2> = m.highway.iter().map(|&p| V2::from_array(p)).collect();
    let freeway_w = place
        .roads
        .edges
        .iter()
        .find(|e| is_freeway(e))
        .map_or(22.0, |e| e.width_m);
    let width = freeway_w + 2.0 * TUNNEL_SHOULDER_M;
    let others: Vec<(V2, V2, f32)> = place
        .roads
        .edges
        .iter()
        .filter(|e| !is_freeway(e))
        .flat_map(|e| {
            let hw = 0.5 * e.width_m + if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
            e.profile
                .windows(2)
                .map(move |w| (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]), hw))
        })
        .collect();
    let mut best: Option<(f32, [V2; 2])> = None;
    // Arc length along the freeway at each point.
    let mut arc = vec![0.0f32; hwy.len()];
    for k in 1..hwy.len() {
        arc[k] = arc[k - 1] + (hwy[k] - hwy[k - 1]).length();
    }
    for i in 0..hwy.len() {
        let Some(j) = (i..hwy.len()).find(|&j| arc[j] - arc[i] >= TUNNEL_M) else {
            break;
        };
        let (a, c) = (hwy[i], hwy[j]);
        let d = (c - a).normalize();
        let (a, c) = (a, a + d * TUNNEL_M);
        if (i..=j).any(|k| segment_distance(hwy[k], a, c).0 > TUNNEL_STRAIGHT_M) {
            continue;
        }
        let reach = 0.5 * width + TUNNEL_CLEAR_M;
        let crowded = others
            .iter()
            .any(|&(p, q, hw)| segments_distance(p, q, a, c) < reach + hw);
        let near_building = place.buildings.iter().any(|bd| {
            bd.footprint
                .iter()
                .any(|&p| segment_distance(V2::from_array(p), a, c).0 < 0.5 * width + 3.0)
        });
        if crowded
            || near_building
            || !m.in_playable(a.x, a.y, 40.0)
            || !m.in_playable(c.x, c.y, 40.0)
        {
            continue;
        }
        let score = (a.lerp(c, 0.5) - downtown).length();
        if best.is_none_or(|x| score < x.0) {
            best = Some((score, [a, c]));
        }
    }
    if let Some((_, t)) = best {
        s.tunnel = Some(t);
        s.tunnel_width_m = width;
    }

    // The lots, nearest the boulevard first.
    let blvd: Vec<V2> = b
        .lines
        .iter()
        .filter(|l| l.boulevard)
        .flat_map(|l| [V2::from_array(l.a), V2::from_array(l.b)])
        .collect();
    let to_blvd = |p: V2| {
        blvd.chunks(2)
            .map(|w| segment_distance(p, w[0], w[1]).0)
            .fold(f32::INFINITY, f32::min)
    };
    let tunnel_box = s.footprints();
    let by_blvd = |x: &Lot, y: &Lot| {
        to_blvd(x.front)
            .total_cmp(&to_blvd(y.front))
            .then(x.key.cmp(&y.key))
    };
    let classes = [RoadClass::Arterial, RoadClass::Collector, RoadClass::Local];
    let garage = lots(place, GARAGE_LOT_M, &[ZoneId::Industrial], &classes)
        .into_iter()
        .filter(|l| !tunnel_box.iter().any(|f| point_in_polygon(l.front, f)))
        .min_by(by_blvd);
    s.garage = garage;
    let zones = [ZoneId::Commercial, ZoneId::Strip, ZoneId::Industrial];
    s.stand = lots(place, STAND_LOT_M, &zones, &classes[..2])
        .into_iter()
        .filter(|l| garage.is_none_or(|g| (g.front - l.front).length() >= APART_M))
        .min_by(by_blvd);

    // The drag strip.
    let (r0, r1) = (V2::from_array(b.runway[0]), V2::from_array(b.runway[1]));
    let len = (r1 - r0).length();
    let dir = (r1 - r0).normalize();
    let run = if STRIP_IN_M + QUARTER_MILE_M + SHUTDOWN_M <= len {
        QUARTER_MILE_M
    } else {
        0.5 * QUARTER_MILE_M
    };
    if STRIP_IN_M + run + 30.0 <= len {
        s.strip = Some((r0 + dir * STRIP_IN_M, dir, run, b.runway_m));
    }

    // The overlooks' turnouts at their roads' ends.
    let mut degree = vec![0u32; place.roads.nodes.len()];
    for e in &place.roads.edges {
        degree[e.a as usize] += 1;
        degree[e.b as usize] += 1;
    }
    for poi in m.pois.iter().filter(|p| p.kind == MacroPoiKind::Overlook) {
        let p = V2::from_array(poi.pos);
        let end = place
            .roads
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| degree[*i] == 1 && (V2::from_array(n.pos) - p).length() < 80.0)
            .min_by(|x, y| {
                let d = |n: &place::Node| (V2::from_array(n.pos) - p).length();
                d(x.1).total_cmp(&d(y.1)).then(x.0.cmp(&y.0))
            })
            .map_or(p, |(_, n)| V2::from_array(n.pos));
        s.overlooks
            .push((end, V2::from_array(poi.facing).normalize()));
    }
    if let (Some(&c0), Some(&c1)) = (b.channel.first(), b.channel.last()) {
        s.channel = Some((V2::from_array(c0), V2::from_array(c1), b.channel_half_m.1));
    }
    // The grade crossings: where the main line crosses a road's centerline (every road it
    // meets crosses at grade), inside the place.
    let rail: Vec<V2> = b.rail.iter().map(|&p| V2::from_array(p)).collect();
    let mut crossings: Vec<(f32, V2)> = Vec::new();
    let mut arc = 0.0;
    for w in rail.windows(2) {
        let (a, c) = (w[0], w[1]);
        for e in &place.roads.edges {
            for q in e.profile.windows(2) {
                let (p0, p1) = (V2::new(q[0][0], q[0][2]), V2::new(q[1][0], q[1][2]));
                if segments_distance(a, c, p0, p1) > 0.0 {
                    continue;
                }
                // The crossing point: where the road's segment meets the rail's.
                let d = c - a;
                let side = |p: V2| d.turn(p - a);
                let (s0, s1) = (side(p0), side(p1));
                let x = p0.lerp(p1, s0 / (s0 - s1));
                let at = arc + (x - a).dot(d.normalize());
                if place.header.chunk_at([x.x, x.y]).is_some()
                    && !crossings.iter().any(|&(_, y)| (y - x).length() < 1.0)
                {
                    crossings.push((at, x));
                }
            }
        }
        arc += (c - a).length();
    }
    crossings.sort_by(|x, y| x.0.total_cmp(&y.0));
    s.crossings = crossings.into_iter().map(|c| c.1).collect();
    s.ramps = b
        .ramps
        .iter()
        .map(|r| [V2::from_array(r[0]), V2::from_array(r[1])])
        .collect();

    // The landmarks.
    if let Some((kit, variant, size)) = kit_of(kits, LETTERS) {
        if let Some(lot) = letters_site(m, place, &s, size) {
            s.marks.push(Mark {
                lot,
                kind: PoiKind::HillsideLetters,
                kit,
                variant,
                size,
            });
        }
    }
    let taken: Vec<V2> = [s.garage, s.stand]
        .iter()
        .flatten()
        .map(|l| l.front)
        .collect();
    if let Some((kit, variant, size)) = kit_of(kits, WATER_TOWER) {
        let (w, d) = (size[0], size[1]);
        let areas = s.footprints();
        // The industry round a lot: industrial zone samples every 32 m within reach.
        let industry = |l: &Lot| {
            let c = l.at(0.0, 0.5 * d);
            let n = (TOWER_REACH_M / 32.0) as i32;
            let mut k = 0u32;
            for i in -n..=n {
                for j in -n..=n {
                    let q = c + V2::new(i as f32, j as f32) * 32.0;
                    let inside = (q - c).length() <= TOWER_REACH_M;
                    if inside && m.zone_at(q.x, q.y) == ZoneId::Industrial {
                        k += 1;
                    }
                }
            }
            k
        };
        let lot = lots(place, (w, d), &[ZoneId::Industrial], &classes)
            .into_iter()
            .filter(|l| !overlaps(l, (w, d), &areas))
            .filter(|l| {
                taken
                    .iter()
                    .all(|&t| (t - l.front).length() >= TOWER_APART_M)
            })
            .map(|l| (industry(&l), l))
            .max_by(|x, y| x.0.cmp(&y.0).then(y.1.key.cmp(&x.1.key)))
            .map(|x| x.1);
        if let Some(lot) = lot {
            s.marks.push(Mark {
                lot,
                kind: PoiKind::WaterTower,
                kit,
                variant,
                size,
            });
        }
    }
    if let Some((kit, variant, size)) = kit_of(kits, DRIVE_IN) {
        let (w, d) = (size[0], size[1]);
        let areas = s.footprints();
        let mut taken = taken;
        taken.extend(s.marks.iter().map(|k| k.lot.front));
        let zones = [
            ZoneId::Commercial,
            ZoneId::Strip,
            ZoneId::Industrial,
            ZoneId::Farmland,
        ];
        let lot = lots(place, (w, d), &zones, &classes[..2])
            .into_iter()
            .filter(|l| !overlaps(l, (w, d), &areas))
            .filter(|l| taken.iter().all(|&t| (t - l.front).length() >= APART_M))
            .min_by(by_blvd);
        if let Some(lot) = lot {
            s.marks.push(Mark {
                lot,
                kind: PoiKind::DriveIn,
                kit,
                variant,
                size,
            });
        }
    }
    s
}

/// The hillside letters' lot, `size` wide and deep (see the module docs): its front along
/// the slope's contour below the letters, facing down it.
fn letters_site(m: &MacroMap, place: &Place, s: &SetPieces, size: [f32; 2]) -> Option<Lot> {
    let b = m.basin.as_ref()?;
    let foot = b.lines.first()?;
    let out = V2::from_array(m.frame.dir_to_world(1.0, 0.0));
    let downtown = V2::from_array(m.downtown);
    let h = |p: V2| m.height_at(p.x, p.y);
    let roads: Vec<(V2, V2, f32)> = place
        .roads
        .edges
        .iter()
        .flat_map(|e| {
            let hw = 0.5 * e.width_m + if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
            e.profile
                .windows(2)
                .map(move |w| (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]), hw))
        })
        .collect();
    let (a, c) = (V2::from_array(foot.a), V2::from_array(foot.b));
    let n = ((c - a).length() / 16.0) as usize;
    let [w, d] = size;
    let mut best: Option<(f32, usize, Lot)> = None;
    for k in 0..=n {
        let q = a.lerp(c, k as f32 / n.max(1) as f32);
        let h0 = h(q);
        // Up the slope from the road until it has risen enough.
        let Some(p) = (3..60)
            .map(|i| q - out * (8.0 * i as f32))
            .find(|&p| h(p) - h0 >= LETTERS_RISE_M)
        else {
            continue;
        };
        let g = V2::new(
            h(p + V2::new(8.0, 0.0)) - h(p - V2::new(8.0, 0.0)),
            h(p + V2::new(0.0, 8.0)) - h(p - V2::new(0.0, 8.0)),
        );
        if g.length() < 1e-3 {
            continue;
        }
        let down = -g.normalize();
        if down.dot(out) < 0.7 {
            continue;
        }
        let lot = Lot {
            front: p + down * LETTERS_FACE_M,
            inward: -down,
            along: down.left(),
            key: k as u64,
        };
        let line = (0..=12).map(|i| p + lot.along * ((i as f32 / 12.0 - 0.5) * w));
        let (lo, hi) = line.fold((f32::MAX, f32::MIN), |(lo, hi), q| {
            (lo.min(h(q)), hi.max(h(q)))
        });
        let rect = lot.rect(w, d);
        let clear = samples(&lot, w, d).chain(rect.iter().copied()).all(|q| {
            m.in_playable(q.x, q.y, 30.0)
                && roads
                    .iter()
                    .all(|&(r0, r1, hw)| segment_distance(q, r0, r1).0 > hw + LETTERS_ROAD_CLEAR_M)
        });
        let by_overlook = s
            .overlooks
            .iter()
            .any(|&(at, _)| (at - lot.at(0.0, 0.5 * d)).length() < 40.0);
        if hi - lo > LETTERS_LEVEL_M || !clear || by_overlook {
            continue;
        }
        let score = abs((p - downtown).dot(out.right()));
        if best.as_ref().is_none_or(|x| score < x.0) {
            best = Some((score, k, lot));
        }
    }
    best.map(|x| x.2)
}

/// Indices of items binned by their bounds on a square grid.
struct Grid {
    origin: V2,
    cell: f32,
    w: usize,
    h: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(place: &Place, cell: f32) -> Grid {
        let origin = V2::from_array(place.header.origin_m());
        let ext = place.header.extent_m();
        let (w, h) = ((ext[0] / cell) as usize + 1, (ext[1] / cell) as usize + 1);
        Grid {
            origin,
            cell,
            w,
            h,
            cells: (0..w * h).map(|_| Vec::new()).collect(),
        }
    }

    fn cell(&self, p: V2) -> (usize, usize) {
        let f = |v: f32, n: usize| ((v / self.cell).max(0.0) as usize).min(n - 1);
        (
            f(p.x - self.origin.x, self.w),
            f(p.y - self.origin.y, self.h),
        )
    }

    fn insert(&mut self, lo: V2, hi: V2, item: u32) {
        let ((x0, z0), (x1, z1)) = (self.cell(lo), self.cell(hi));
        for z in z0..=z1 {
            for x in x0..=x1 {
                self.cells[z * self.w + x].push(item);
            }
        }
    }

    fn at(&self, p: V2) -> &[u32] {
        let (x, z) = self.cell(p);
        &self.cells[z * self.w + x]
    }
}

/// The parcels of a place, binned, for finding the one under a point.
struct Parcels<'a> {
    place: &'a Place,
    polys: Vec<Vec<V2>>,
    grid: Grid,
}

impl<'a> Parcels<'a> {
    fn new(place: &'a Place) -> Parcels<'a> {
        let mut grid = Grid::new(place, 32.0);
        let mut polys = Vec::with_capacity(place.parcels.len());
        for (i, p) in place.parcels.iter().enumerate() {
            let poly: Vec<V2> = p.polygon.iter().map(|&q| V2::from_array(q)).collect();
            let (lo, hi) = poly.iter().fold(
                (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN)),
                |(lo, hi), q| {
                    (
                        V2::new(lo.x.min(q.x), lo.y.min(q.y)),
                        V2::new(hi.x.max(q.x), hi.y.max(q.y)),
                    )
                },
            );
            grid.insert(lo, hi, i as u32);
            polys.push(poly);
        }
        Parcels { place, polys, grid }
    }

    /// The parcel under `p`, if any.
    fn under(&self, p: V2) -> Option<usize> {
        self.grid
            .at(p)
            .iter()
            .map(|&i| i as usize)
            .find(|&i| point_in_polygon(p, &self.polys[i]))
    }

    /// Whether `p` lies on a lot (not a park or open land) of one of `zones`.
    fn on_lot(&self, p: V2, zones: &[ZoneId]) -> bool {
        self.under(p).is_some_and(|i| {
            let q = &self.place.parcels[i];
            q.lot_kind == LotKind::Lot && zones.contains(&q.zone)
        })
    }
}

/// Points over a lot's rectangle `w` wide and `d` deep: what must lie on lots for the lot
/// to be one.
fn samples(l: &Lot, w: f32, d: f32) -> impl Iterator<Item = V2> + '_ {
    (0..5).flat_map(move |i| {
        (0..5).map(move |k| {
            l.at(
                (i as f32 / 4.0 - 0.5) * (w - 1.0),
                0.5 + k as f32 / 4.0 * (d - 1.0),
            )
        })
    })
}

/// Sites `w` wide and `d` deep square to a road of `classes`, behind its walk, over lots
/// (one or several side by side) of `zones` and clear of every other road; every
/// [`SITE_STEP_M`] along each side of each such road where it runs straight.
pub fn lots(
    place: &Place,
    (w, d): (f32, f32),
    zones: &[ZoneId],
    classes: &[RoadClass],
) -> Vec<Lot> {
    let parcels = Parcels::new(place);
    let mut segs = Grid::new(place, 32.0);
    let mut seg_list: Vec<(V2, V2, f32, usize)> = Vec::new();
    for (i, e) in place.roads.edges.iter().enumerate() {
        let hw = 0.5 * e.width_m + if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.2 };
        for p in e.profile.windows(2) {
            let (a, b) = (V2::new(p[0][0], p[0][2]), V2::new(p[1][0], p[1][2]));
            let r = V2::new(hw + 1.0, hw + 1.0);
            let (lo, hi) = (
                V2::new(a.x.min(b.x), a.y.min(b.y)) - r,
                V2::new(a.x.max(b.x), a.y.max(b.y)) + r,
            );
            segs.insert(lo, hi, seg_list.len() as u32);
            seg_list.push((a, b, hw, i));
        }
    }
    let on_road = |q: V2| {
        segs.at(q).iter().any(|&k| {
            let (a, b, hw, _) = seg_list[k as usize];
            segment_distance(q, a, b).0 < hw + 0.5
        })
    };
    let mut out = Vec::new();
    for (ei, e) in place.roads.edges.iter().enumerate() {
        if !classes.contains(&e.class) {
            continue;
        }
        let road: Vec<V2> = e.profile.iter().map(|q| V2::new(q[0], q[2])).collect();
        let hw = 0.5 * e.width_m + if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.2 };
        let mut arc = 0.0;
        let mut next = 0.5 * w + 4.0;
        for (si, seg) in road.windows(2).enumerate() {
            let (a, b) = (seg[0], seg[1]);
            let len = (b - a).length();
            let dir = (b - a).normalize();
            while next < arc + len {
                let p = a + dir * (next - arc);
                next += SITE_STEP_M;
                // The road runs straight past the whole front.
                let straight = [p - dir * (0.5 * w), p + dir * (0.5 * w)]
                    .iter()
                    .all(|&q| to_line(q, &road) < 0.5);
                if !straight {
                    continue;
                }
                for (k, inward) in [dir.left(), dir.right()].into_iter().enumerate() {
                    let lot = Lot {
                        front: p + inward * (hw + 0.5),
                        inward,
                        along: dir,
                        key: (ei as u64) << 24
                            | (si as u64) << 8
                            | (next as u64 & 0x7F) << 1
                            | k as u64,
                    };
                    let mut pts = samples(&lot, w, d);
                    if pts.all(|q| parcels.on_lot(q, zones) && !on_road(q)) {
                        out.push(lot);
                    }
                }
            }
            arc += len;
        }
    }
    out
}

/// A site is tried every this far along a road, m.
const SITE_STEP_M: f32 = 6.0;

/// Takes the buildings standing on the set pieces' lots, or on any lot they cover part
/// of, off the place.
pub fn clear(place: &mut Place, s: &SetPieces) {
    let parcels = Parcels::new(place);
    let mut taken: Vec<usize> = Vec::new();
    let sites = [(&s.garage, GARAGE_LOT_M), (&s.stand, STAND_LOT_M)];
    for (l, (w, d)) in sites {
        if let Some(l) = l {
            taken.extend(samples(l, w, d).filter_map(|q| parcels.under(q)));
        }
    }
    for mark in &s.marks {
        let (w, d) = (mark.size[0], mark.size[1]);
        taken.extend(samples(&mark.lot, w, d).filter_map(|q| parcels.under(q)));
    }
    let lots: Vec<Vec<V2>> = taken.iter().map(|&i| parcels.polys[i].clone()).collect();
    let mut areas = s.footprints();
    areas.extend(lots);
    place.buildings.retain(|b| {
        let fp: Vec<V2> = b.footprint.iter().map(|&q| V2::from_array(q)).collect();
        let c = fp.iter().fold(V2::new(0.0, 0.0), |a, &q| a + q) * (1.0 / fp.len().max(1) as f32);
        !areas
            .iter()
            .any(|a| point_in_polygon(c, a) || fp.iter().any(|&q| point_in_polygon(q, a)))
    });
}

/// Whether `q` stands on ground a set piece keeps (see [`SetPieces::footprints`]).
pub fn kept(s: &SetPieces) -> impl Fn(V2) -> bool {
    let areas = s.footprints();
    move |q| areas.iter().any(|a| point_in_polygon(q, a))
}

fn prop(p: V2, y: f32, facing: V2, kind: PropKind) -> Prop {
    Prop {
        pos: [p.x, y, p.y],
        yaw: yaw_of_dir(facing),
        kind,
    }
}

/// The set pieces' props and their own lights (the street lamps' come with the
/// detailing's), on the graded `grid`; the landmarks from `kits` (the grammar's) lit in
/// `pal`.
pub fn dress(
    seed: Seed,
    place: &Place,
    grid: &TerrainGrid,
    s: &SetPieces,
    kits: &[Kit],
    pal: &Palette,
) -> (Vec<Prop>, Vec<Light>) {
    let mut props = Vec::new();
    let mut lights = Vec::new();
    let ground = |q: V2| grid.height_at(q.x, q.y);
    let name = |k: u64| skywave_core::hash64(seed.0, &[SETPIECES, 0x4E41_4D45, k]);
    let flood = |p: V2, y: f32, color: [f32; 3], radius: f32| Light {
        pos: [p.x, y, p.y],
        color,
        radius,
        kind: LightKind::Flood,
    };

    if let Some([a, c]) = s.tunnel {
        let d = (c - a).normalize();
        let n = (TUNNEL_M / TUNNEL_PIECE_M) as usize;
        let width_dm = (s.tunnel_width_m * 10.0 + 0.5) as u16;
        for k in 0..n {
            let mid = a + d * ((k as f32 + 0.5) * TUNNEL_PIECE_M);
            let y = road_y(place, mid, is_freeway).unwrap_or_else(|| ground(mid));
            let portals = u8::from(k + 1 == n) | u8::from(k == 0) << 1;
            props.push(prop(
                mid,
                y,
                d,
                PropKind::TunnelPiece {
                    len_dm: (TUNNEL_PIECE_M * 10.0 + 0.5) as u16,
                    width_dm,
                    portals,
                },
            ));
        }
        let lamp = [1.0, 0.95, 0.86];
        let mut t = 0.5 * TUNNEL_LIGHT_STEP_M;
        while t < TUNNEL_M {
            let q = a + d * t;
            let y = road_y(place, q, is_freeway).unwrap_or_else(|| ground(q));
            for sigma in [-1.0f32, 1.0] {
                let p = q + d.right() * (sigma * (0.5 * s.tunnel_width_m - 2.5));
                lights.push(flood(p, y + TUNNEL_H_M - 0.4, lamp, 18.0));
            }
            t += TUNNEL_LIGHT_STEP_M;
        }
    }

    if let Some(l) = &s.garage {
        let front = l.at(0.0, APRON_M);
        let y = ground(front);
        let out = -l.inward;
        props.push(prop(
            front,
            y,
            out,
            PropKind::CrewGarage { name_seed: name(1) },
        ));
        // Its hero cars: in the outer bays nose out, one on the apron at an angle before one
        // of them; the middle bay stays open.
        let x = out.left();
        let bay = |k: usize, z: f32| front + x * bay_x(k) - out * z;
        for (slot, k) in [(0u8, 0usize), (1, 2)] {
            let q = bay(k, 6.5);
            props.push(prop(q, ground(q), out, PropKind::HeroCar { slot }));
        }
        let q = bay(2, -4.5);
        let angled = (out + x * 0.6).normalize();
        props.push(prop(q, ground(q), angled, PropKind::HeroCar { slot: 2 }));
        let tube = [0.9, 0.96, 1.0];
        for k in 0..3 {
            let q = bay(k, 0.5 * GARAGE_M.1);
            lights.push(flood(q, y + GARAGE_M.2 - 0.6, tube, 9.0));
        }
        let sign = front + out * 0.5;
        lights.push(Light {
            pos: [sign.x, y + GARAGE_M.2 + 0.9, sign.y],
            color: [1.0, 0.97, 0.9],
            radius: 10.0,
            kind: LightKind::Neon,
        });
    }

    if let Some(l) = &s.stand {
        let at = l.at(2.0, 0.5 * STAND_LOT_M.1 + 0.5 * STAND_M.1);
        let out = -l.inward;
        let y = ground(at);
        props.push(prop(at, y, out, PropKind::TacoStand { name_seed: name(2) }));
        for k in 0..3 {
            let q = l.at(-1.0 + 4.0 * k as f32 - 4.0, 0.5 * STAND_LOT_M.1 - 4.0);
            props.push(prop(q, ground(q), l.along, PropKind::PicnicTable));
        }
        // A row of cars along one side of the lot, some there all day, some for supper.
        let classes = [0x01u8, 0x14, 0x23, 0x31, 0x45];
        let evening = (17..23).fold(0u32, |m, h| m | 1 << h);
        for (k, &class) in classes.iter().enumerate() {
            let q = l.at(-0.5 * STAND_LOT_M.0 + 2.5, 3.0 + 3.0 * k as f32);
            let hours = if k % 2 == 0 {
                ALL_HOURS
            } else {
                evening | 1 << 12 | 1 << 13 | 1 << 24
            };
            props.push(prop(
                q,
                ground(q),
                l.along,
                PropKind::ParkedCar {
                    vehicle_class: class,
                    hours,
                },
            ));
        }
        let w = at + out * 1.5;
        lights.push(Light {
            pos: [w.x, y + 2.2, w.y],
            color: [1.0, 0.78, 0.5],
            radius: 9.0,
            kind: LightKind::Window,
        });
        lights.push(Light {
            pos: [at.x, y + STAND_M.2 + 0.7, at.y],
            color: [1.0, 0.82, 0.3],
            radius: 8.0,
            kind: LightKind::Neon,
        });
    }

    if let Some((start, dir, run, width)) = s.strip {
        let runway = |e: &place::Edge| e.class == RoadClass::Collector && !e.sidewalk;
        let y_at = |q: V2| road_y(place, q, runway).unwrap_or_else(|| ground(q));
        let across = dir.right();
        let len_dm = ((width - 1.0) * 10.0) as u16;
        let finish = start + dir * run;
        props.push(prop(
            start,
            y_at(start),
            dir,
            PropKind::StripLine { len_dm },
        ));
        props.push(prop(
            finish,
            y_at(finish),
            dir,
            PropKind::StripLine { len_dm },
        ));
        let tree = start + dir * 1.0;
        props.push(prop(tree, y_at(tree), dir, PropKind::StartTree));
        let board = start + dir * 30.0 - across * (0.5 * width + 4.0);
        props.push(prop(board, ground(board), -dir, PropKind::TimingBoard));
        let mut t = -20.0;
        while t <= run + 20.0 {
            for sigma in [-1.0f32, 1.0] {
                let q = start + dir * t + across * (sigma * (0.5 * width + 1.5));
                props.push(prop(q, ground(q), across * -sigma, PropKind::StreetLight));
            }
            t += STRIP_LAMP_STEP_M;
        }
    }

    for &(at, out) in &s.overlooks {
        let rail = at + out * RAIL_OUT_M;
        let along = out.right();
        let n = 2;
        for k in 0..n {
            let x = (k as f32 + 0.5) / n as f32 * RAIL_M - 0.5 * RAIL_M;
            let q = rail + along * x;
            props.push(prop(
                q,
                ground(q),
                along,
                PropKind::GuardRail {
                    len_dm: (RAIL_M / n as f32 * 10.0) as u16,
                },
            ));
        }
        for sigma in [-1.0f32, 1.0] {
            let q = at + out * (RAIL_OUT_M - 2.5) + along * (sigma * 4.0);
            props.push(prop(q, ground(q), out, PropKind::Bench));
        }
        let lamp = at + along * (0.5 * RAIL_M + 1.0);
        props.push(prop(lamp, ground(lamp), -along, PropKind::StreetLight));
    }

    for mark in &s.marks {
        let (w, d) = (mark.size[0], mark.size[1]);
        let l = &mark.lot;
        let out = -l.inward;
        // Its base: the lowest ground under the letters' line, or under the lot (the pad
        // reaches down into the ground).
        let y = if mark.kind == PoiKind::HillsideLetters {
            (0..=12)
                .map(|i| ground(l.at((i as f32 / 12.0 - 0.5) * w, LETTERS_FACE_M)))
                .fold(f32::MAX, f32::min)
        } else {
            samples(l, w, d).map(ground).fold(f32::MAX, f32::min)
        };
        props.push(prop(
            l.front,
            y,
            out,
            PropKind::Landmark {
                kit: mark.kit,
                variant: mark.variant,
            },
        ));
        if let Some(kit) = kits.get(usize::from(mark.kit)) {
            let base = V3::new(l.front.x, y, l.front.y);
            let v = usize::from(mark.variant);
            crate::landmarks::lights(kit, v, base, out, pal, &mut lights);
        }
        // The drive-in's rows fill after dark: each car from 19:00 or 20:00 to 23:00 or
        // 01:00, by its stall.
        if mark.kind == PoiKind::DriveIn {
            let x = out.left();
            let classes = [0x01u8, 0x14, 0x23, 0x31, 0x45, 0x52];
            let mut k = 0usize;
            for &z in &DRIVE_IN_ROWS {
                for &sx in &DRIVE_IN_STALLS {
                    let q = l.front + x * sx + out * z;
                    let from = 19 + (k % 2) as u32;
                    let to = if k.is_multiple_of(3) { 25 } else { 23 };
                    let hours = (from..to).fold(0u32, |m, h| m | 1 << (h % 24));
                    props.push(prop(
                        q,
                        ground(q),
                        -out,
                        PropKind::ParkedCar {
                            vehicle_class: classes[k % classes.len()],
                            hours: hours | LOT_CAR,
                        },
                    ));
                    k += 1;
                }
            }
        }
    }

    channel_fences(place, grid, s, &mut props);
    (props, lights)
}

/// Chain-link along both lips of the channel (see the module docs).
fn channel_fences(place: &Place, grid: &TerrainGrid, s: &SetPieces, out: &mut Vec<Prop>) {
    let Some(ch) = s.channel.as_ref() else {
        return;
    };
    let (c0, c1) = (ch.0, ch.1);
    let d = (c1 - c0).normalize();
    let n = d.right();
    let len = (c1 - c0).length();
    let roads: Vec<(V2, V2, f32)> = place
        .roads
        .edges
        .iter()
        .flat_map(|e| {
            let hw = 0.5 * e.width_m + if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
            e.profile
                .windows(2)
                .map(move |w| (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]), hw))
        })
        .collect();
    let near_road = |q: V2| {
        roads
            .iter()
            .any(|&(a, b, hw)| segment_distance(q, a, b).0 < hw + FENCE_ROAD_CLEAR_M)
    };
    // Each ramp: its span along the channel and the side it is on.
    let ramps: Vec<(f32, f32, f32)> = s
        .ramps
        .iter()
        .map(|r| {
            let (u0, u1) = ((r[0] - c0).dot(d), (r[1] - c0).dot(d));
            let side = if (r[0] - c0).dot(n) >= 0.0 { 1.0 } else { -1.0 };
            (u0.min(u1), u0.max(u1), side)
        })
        .collect();
    let lip = ch.2 + FENCE_OUT_M;
    let ramp_out = ch.2 + crate::procgen::basin::RAMP_W_M + FENCE_OUT_M;
    // A fence line: `u` from `u0` to `u1` at `w` across, cut into runs, each run laid only
    // where it stands clear of the roads and inside the place.
    let mut line = |w: f32, u0: f32, u1: f32, skip: &dyn Fn(f32) -> bool| {
        let mut u = u0;
        while u < u1 - 1.0 {
            let e = (u + FENCE_RUN_M).min(u1);
            let mid = 0.5 * (u + e);
            let q = c0 + d * mid + n * w;
            let ends = [c0 + d * u + n * w, c0 + d * e + n * w];
            if !skip(mid) && !ends.iter().chain([&q]).any(|&p| near_road(p)) {
                out.push(prop(
                    q,
                    grid.height_at(q.x, q.y),
                    d,
                    PropKind::ChainLink {
                        len_dm: ((e - u) * 10.0) as u16,
                    },
                ));
            }
            u = e;
        }
    };
    for side in [-1.0f32, 1.0] {
        let on_ramp = |u: f32| {
            ramps
                .iter()
                .any(|&(lo, hi, rs)| rs == side && u > lo - 3.0 && u < hi + 3.0)
        };
        line(side * lip, 0.0, len, &on_ramp);
        // Along each ramp's outer edge (its top left open) and across its foot.
        for &(lo, hi, rs) in ramps.iter().filter(|r| r.2 == side) {
            line(rs * ramp_out, lo + 12.0, hi + 3.0, &|_| false);
        }
    }
    // Across each ramp's foot end, lip to its outer edge, so its end wall is fenced.
    for &(_, hi, rs) in &ramps {
        let q = c0 + d * (hi + 3.0) + n * (rs * 0.5 * (lip + ramp_out));
        if !near_road(q) {
            out.push(prop(
                q,
                grid.height_at(q.x, q.y),
                n,
                PropKind::ChainLink {
                    len_dm: ((ramp_out - lip) * 10.0) as u16,
                },
            ));
        }
    }
}
