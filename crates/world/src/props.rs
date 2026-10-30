//! Street furniture and plants: where they go on the test town (a deterministic pass
//! producing [`Prop`] records) and their low-poly meshes. A terrain place lists its props
//! in the Place's `Props` table (`procgen::detail`); [`from_place`] turns those into
//! [`Prop`]s, and [`instanced`] says which the app draws as shared-mesh instances instead
//! of meshing them into chunks (trees and parked cars).

use alloc::vec::Vec;

use place::{Place, RoadClass, ZoneId};
use skywave_core::{mix64, Rng, Seed};

use crate::math::{atan2, point_in_polygon, sin_cos, TAU, V2, V3};
use crate::mesh::{
    boxf, face, frustum, icosphere, lathe, polygon_cap, quad, rect, tri, Frame, Glow, Paint,
    Pattern, UnitSphere, WorldMesh,
};
use crate::palette::{mix, shade, Palette, Rgb};
use crate::roads::Network;
use crate::signs;
use crate::signs::letters::{Finish, LetterStyle, Strokes};
use crate::signs::lexicon::{Lexicon, ShopSign};
use crate::signs::pictogram::{self, Pictogram, Tints};
use crate::{CURB_M, LOT_Y};

const KEY_STREET: u64 = 0x5354_5245;
const KEY_PARK: u64 = 0x5041_524B;
const KEY_PROP: u64 = 0x5052_4F50;
const KEY_PARKED_HOUR: u64 = 0x484F_5552;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    StreetLight,
    PalmTree,
    Oak,
    Hydrant,
    Bench,
    /// Placeholder two-box car; `color` indexes `Palette::cars`.
    ParkedCar {
        color: u8,
    },
    StopSign,
    /// A signal pole with a mast arm of `arm_dm` decimeters.
    TrafficSignal {
        arm_dm: u8,
    },
    Fountain,
    TrashCan,
    /// A tall gum tree: long pale trunk, a sparse crown of lobes.
    Eucalyptus,
    /// A wooden utility pole with a crossarm; `dir` runs along the line.
    PowerPole,
    /// A roadside billboard on posts facing `dir`, with an invented ad.
    Billboard,
    /// A curbside mailbox on a post, door toward `dir`.
    Mailbox,
    /// A clipped hedge `len_dm` decimeters long along `dir`, centered on `pos`.
    Hedge {
        len_dm: u16,
    },
    /// A wood fence or a block wall `len_dm` decimeters long along `dir`, centered on
    /// `pos`.
    Fence {
        len_dm: u16,
    },
    /// A street-name post: two crossed blades lettered with an invented name.
    StreetSign,
    /// A vertical neon blade sign projecting along `dir` from a wall at `pos`; `palette`
    /// indexes `Palette::neon`.
    NeonSign {
        palette: u8,
    },
    /// A pier `len_m` meters long from its land end at `pos` (on the deck) out along `dir`.
    Pier {
        len_m: u8,
    },
    /// Railway track `len_dm` decimeters long centered on `pos` along `dir`, on its `bed`
    /// (`place::TRACK_BALLAST` and the others).
    Track {
        len_dm: u16,
        bed: u8,
    },
    /// A railroad crossing sign on a post, facing `dir`.
    Crossbuck,
    /// A billboard with a blank (0) or pictogram (pictogram index + 1) face, facing `dir`.
    FaceBillboard {
        face: u8,
    },
    /// A bus shelter open toward `dir`, its ad box's face as a [`PropKind::FaceBillboard`]'s.
    BusShelter {
        face: u8,
    },
    /// The set pieces' props, as `place::PropKind`'s (built by [`crate::setpieces`]).
    TunnelPiece {
        len_dm: u16,
        width_dm: u16,
        portals: u8,
    },
    StartTree,
    TimingBoard,
    StripLine {
        len_dm: u16,
    },
    GuardRail {
        len_dm: u16,
    },
    ChainLink {
        len_dm: u16,
    },
    CrewGarage {
        name_seed: u64,
    },
    TacoStand {
        name_seed: u64,
    },
    PicnicTable,
    /// A railroad crossing gate's mast (its arm is the app's), facing the drivers it stops.
    CrossingGate {
        arm_dm: u8,
    },
    /// A landmark from the grammar's kits, as `place::PropKind`'s (built by
    /// [`crate::landmarks`]).
    Landmark {
        kit: u8,
        variant: u8,
    },
    /// The landmark gallery's lettered lot label.
    LotLabel {
        letter: u8,
    },
}

/// One placed prop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prop {
    pub pos: V2,
    /// Base height.
    pub y: f32,
    /// Facing (benches, signs, cars) or arm direction (lights, signals); unit.
    pub dir: V2,
    pub kind: PropKind,
    /// Per-prop variation seed.
    pub seed: u32,
}

/// Places street props along every road side and in parks.
pub fn place(
    place: &Place,
    net: &Network,
    side_zone: &[[Option<ZoneId>; 2]],
    parks: &[Vec<V2>],
    seed: u64,
) -> Vec<Prop> {
    let mut out = Vec::new();
    let nodes = &place.roads.nodes;
    for (i, e) in place.roads.edges.iter().enumerate() {
        let mut rng = Rng::new(Seed(seed), &[KEY_STREET, i as u64]);
        let a = V2::from_array(nodes[e.a as usize].pos);
        let b = V2::from_array(nodes[e.b as usize].pos);
        let (ea, eb) = net.ends(i);
        let len = (b - a).length();
        let d = (b - a).normalize();
        let l = d.left();
        let hw = ea.hw;
        let at = |s: f32, t: f32| a + d * s + l * t;
        let arterial = e.class == RoadClass::Arterial;
        let sides = [
            (1.0f32, ea.s_left, len - eb.s_right),
            (-1.0, ea.s_right, len - eb.s_left),
        ];
        for (k, (sigma, s0, s1)) in sides.into_iter().enumerate() {
            let zone = side_zone[i][k];
            if s1 - s0 < 12.0 {
                continue;
            }
            let toward_road = l * -sigma;
            // Street lights: both sides of arterials (staggered), one side of locals.
            let mut lamps: Vec<f32> = Vec::new();
            let lit_side = arterial || (i % 2 == 0) == (sigma > 0.0);
            if lit_side {
                let spacing = if arterial { 32.0 } else { 44.0 };
                let span = s1 - s0 - 12.0;
                let count = ((span / spacing) as u32).max(1);
                let step = span / count as f32;
                let phase = if arterial && sigma < 0.0 { 0.5 } else { 0.0 };
                for c in 0..=count {
                    let s = s0 + 6.0 + step * (c as f32 + phase);
                    if s > s1 - 6.0 {
                        break;
                    }
                    lamps.push(s);
                    out.push(prop(
                        at(s, sigma * (hw + 0.45)),
                        CURB_M,
                        toward_road,
                        PropKind::StreetLight,
                        &mut rng,
                    ));
                }
            }
            let clear = |s: f32, lamps: &[f32], r: f32| lamps.iter().all(|&x| (x - s).abs() > r);
            match zone {
                Some(ZoneId::Residential) | Some(ZoneId::Park) if !arterial => {
                    // Shade trees in the parkway.
                    let mut s = s0 + rng.range_f32(5.0, 10.0);
                    while s < s1 - 5.0 {
                        if clear(s, &lamps, 4.0) {
                            out.push(prop(
                                at(s, sigma * (hw + 0.7)),
                                CURB_M,
                                toward_road,
                                PropKind::Oak,
                                &mut rng,
                            ));
                        }
                        s += rng.range_f32(30.0, 40.0);
                    }
                }
                Some(ZoneId::Downtown) => {}
                Some(_) if arterial => {
                    // Palms between the lights.
                    for w in lamps.windows(2) {
                        let s = (w[0] + w[1]) * 0.5;
                        out.push(prop(
                            at(s, sigma * (hw + 1.0)),
                            CURB_M,
                            toward_road,
                            PropKind::PalmTree,
                            &mut rng,
                        ));
                    }
                }
                _ => {}
            }
            if matches!(zone, Some(ZoneId::Strip) | Some(ZoneId::Downtown)) {
                for frac in [0.3, 0.7] {
                    let s = s0 + (s1 - s0) * frac;
                    if clear(s, &lamps, 3.0) {
                        out.push(prop(
                            at(s, sigma * (hw + 1.9)),
                            CURB_M,
                            toward_road,
                            PropKind::Bench,
                            &mut rng,
                        ));
                        out.push(prop(
                            at(s + 1.6, sigma * (hw + 0.6)),
                            CURB_M,
                            toward_road,
                            PropKind::TrashCan,
                            &mut rng,
                        ));
                    }
                }
            }
            // A hydrant near one corner of every other side.
            if (i + k) % 2 == 0 {
                out.push(prop(
                    at(s0 + 3.5, sigma * (hw + 0.35)),
                    CURB_M,
                    toward_road,
                    PropKind::Hydrant,
                    &mut rng,
                ));
            }
            // Parked cars along local curbs, facing the direction of travel on that side.
            if !arterial {
                let occupancy = match zone {
                    Some(ZoneId::Residential) => 75,
                    Some(ZoneId::Park) => 50,
                    _ => 140,
                };
                let lateral = sigma * (hw - 0.45 - 1.0);
                let facing = d * -sigma;
                let mut s = ea.trim.max(s0) + 8.0;
                let end = s1.min(len - eb.trim) - 8.0;
                while s + 4.6 < end {
                    if rng.range(0, 256) < occupancy && s > s0 + 6.0 {
                        let color = rng.range(0, 8) as u8;
                        out.push(prop(
                            at(s + 2.3, lateral),
                            0.0,
                            facing,
                            PropKind::ParkedCar { color },
                            &mut rng,
                        ));
                    }
                    s += 4.6 + rng.range_f32(1.2, 6.0);
                }
            }
        }
    }
    intersections(net, &mut out, seed);
    for (k, park) in parks.iter().enumerate() {
        park_props(park, &mut out, seed ^ k as u64);
    }
    out
}

fn prop(pos: V2, y: f32, dir: V2, kind: PropKind, rng: &mut Rng) -> Prop {
    Prop {
        pos,
        y,
        dir,
        kind,
        seed: rng.next_u32(),
    }
}

/// Signals where arterials cross, stop signs on the other approaches.
fn intersections(net: &Network, out: &mut Vec<Prop>, seed: u64) {
    for (ji, j) in net.junctions.iter().enumerate() {
        if j.ends.len() < 3 {
            continue;
        }
        let mut rng = Rng::new(Seed(seed), &[KEY_STREET, 0xFFFF, ji as u64]);
        let arterials = j
            .ends
            .iter()
            .filter(|e| e.class == RoadClass::Arterial)
            .count();
        // Signals where arterials cross, and at every other four-way along an arterial.
        let signals = arterials >= 3 || (arterials == 2 && j.ends.len() == 4 && j.node % 2 == 0);
        for e in &j.ends {
            // Drivers approaching the node keep to the right: the left side of `dir`.
            let side = e.dir.left();
            if signals {
                let pos = j.pos + e.dir * (e.trim + 1.2) + side * (e.hw + 0.8);
                let arm_dm = ((e.hw * 0.85) * 10.0) as u8;
                out.push(prop(
                    pos,
                    CURB_M,
                    -side,
                    PropKind::TrafficSignal { arm_dm },
                    &mut rng,
                ));
            } else if e.class != RoadClass::Arterial {
                let pos = j.pos + e.dir * (e.trim + 1.0) + side * (e.hw + 0.6);
                out.push(prop(pos, CURB_M, e.dir, PropKind::StopSign, &mut rng));
            }
        }
    }
}

/// Trees on a jittered grid, benches around the plaza, a fountain in the middle.
fn park_props(poly: &[V2], out: &mut Vec<Prop>, seed: u64) {
    let mut rng = Rng::new(Seed(seed), &[KEY_PARK]);
    let mut c = V2::new(0.0, 0.0);
    for &p in poly {
        c = c + p;
    }
    let c = c * (1.0 / poly.len() as f32);
    out.push(prop(
        c,
        LOT_Y,
        V2::new(1.0, 0.0),
        PropKind::Fountain,
        &mut rng,
    ));
    let mids: Vec<V2> = (0..poly.len())
        .map(|i| (poly[i] + poly[(i + 1) % poly.len()]) * 0.5)
        .collect();
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in poly {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    let spacing = 13.0;
    let mut z = lo.y + 6.0;
    while z < hi.y - 4.0 {
        let mut x = lo.x + 6.0;
        while x < hi.x - 4.0 {
            let p = V2::new(x + rng.range_f32(-3.5, 3.5), z + rng.range_f32(-3.5, 3.5));
            let near_path = mids
                .iter()
                .any(|&m| crate::math::segment_distance(p, m, c).0 < 4.0);
            if point_in_polygon(p, poly)
                && !near_path
                && (p - c).length() > 14.0
                && rng.range(0, 4) != 0
            {
                let kind = if rng.range(0, 5) == 0 {
                    PropKind::PalmTree
                } else {
                    PropKind::Oak
                };
                out.push(prop(p, LOT_Y, V2::new(1.0, 0.0), kind, &mut rng));
            }
            x += spacing;
        }
        z += spacing;
    }
    for m in mids {
        let d = (m - c).normalize();
        let pos = c + d * 11.5 + d.left() * 2.4;
        out.push(prop(pos, LOT_Y, -d.left(), PropKind::Bench, &mut rng));
    }
}

/// Shared geometry for building many props, and how finely to build them.
pub struct PropKit {
    sphere0: UnitSphere,
    sphere1: UnitSphere,
    /// Build the [`instanced`] kinds at full detail (see [`PropKit::instanced`]).
    fine: bool,
}

impl PropKit {
    /// The kit for props meshed into chunks (the test town's): every kind in its light
    /// shape, which the chunk budgets and the pinned mesh hashes assume.
    pub fn new() -> PropKit {
        PropKit {
            sphere0: UnitSphere::new(0),
            sphere1: UnitSphere::new(1),
            fine: false,
        }
    }

    /// The kit for the shared meshes the app draws as instances: the [`instanced`] kinds
    /// at full detail (lobed trees, a cobra-head street light, furniture with its caps,
    /// slats and insulators) and the street-clutter designs a bench or a trash can stands
    /// for, picked by the prop's seed. A shape is built once and drawn many times, so it
    /// may spend the art budgets (`docs/backlog/ART.md`) that a chunk cannot.
    pub fn instanced() -> PropKit {
        PropKit {
            fine: true,
            ..PropKit::new()
        }
    }
}

impl Default for PropKit {
    fn default() -> PropKit {
        PropKit::new()
    }
}

/// Builds one prop's mesh.
pub fn build(m: &mut WorldMesh, p: &Prop, pal: &Palette, kit: &PropKit) {
    let base = p.pos.at(p.y);
    let mut rng = Rng::new(Seed(u64::from(p.seed)), &[KEY_PROP]);
    if kit.fine && build_fine(m, base, p, pal, kit, &mut rng) {
        return;
    }
    match p.kind {
        PropKind::StreetLight => street_light(m, base, p.dir, pal),
        PropKind::PalmTree => palm(m, base, pal, kit, &mut rng),
        PropKind::Oak => oak(m, base, pal, kit, &mut rng),
        PropKind::Hydrant => hydrant(m, base, p.dir, pal),
        PropKind::Bench => bench(m, base, p.dir, pal),
        PropKind::ParkedCar { color } => {
            car(m, base, p.dir, pal.cars()[usize::from(color) % 8], pal)
        }
        PropKind::StopSign => stop_sign(m, base, p.dir, pal),
        PropKind::TrafficSignal { arm_dm } => signal(m, base, p.dir, f32::from(arm_dm) * 0.1, pal),
        PropKind::Fountain => fountain(m, base, pal),
        PropKind::TrashCan => trash_can(m, base, pal),
        PropKind::Eucalyptus => eucalyptus(m, base, pal, kit, &mut rng),
        PropKind::PowerPole => power_pole(m, base, p.dir, pal, &mut rng),
        PropKind::Billboard => billboard(m, base, p.dir, pal, p.seed),
        PropKind::Mailbox => mailbox(m, base, p.dir, pal, &mut rng),
        PropKind::Hedge { len_dm } => hedge(m, base, p.dir, f32::from(len_dm) * 0.1, pal, &mut rng),
        PropKind::Fence { len_dm } => fence(m, base, p.dir, f32::from(len_dm) * 0.1, pal, &mut rng),
        PropKind::StreetSign => street_sign(m, base, p.dir, pal, p.seed),
        PropKind::NeonSign { palette } => {
            neon_sign(m, base, p.dir, palette, pal, p.seed, None);
        }
        PropKind::Pier { len_m } => pier(m, base, p.dir, f32::from(len_m), pal, &mut rng),
        PropKind::Track { len_dm, bed } => track(m, base, p.dir, f32::from(len_dm) * 0.1, bed, pal),
        PropKind::Crossbuck => crossbuck(m, base, p.dir, pal),
        PropKind::FaceBillboard { face } => face_billboard(m, base, p.dir, face, pal, p.seed),
        PropKind::BusShelter { face } => bus_shelter(m, base, p.dir, face, pal, p.seed),
        PropKind::TunnelPiece { .. }
        | PropKind::StartTree
        | PropKind::TimingBoard
        | PropKind::StripLine { .. }
        | PropKind::GuardRail { .. }
        | PropKind::ChainLink { .. }
        | PropKind::CrewGarage { .. }
        | PropKind::TacoStand { .. }
        | PropKind::PicnicTable
        | PropKind::CrossingGate { .. } => crate::setpieces::build(m, p, pal, &Lexicon::default()),
        // A landmark needs its kit, which only the chunk plan has.
        PropKind::Landmark { .. } => {}
        PropKind::LotLabel { letter } => crate::landmarks::lot_label(m, base, p.dir, letter, pal),
    }
}

/// The lit face of a billboard or an ad box, `w` × `h` centered on `(cx, cy)` at depth `z`
/// in frame `f`: a backlit white panel, or a plain ground with one big pictogram on it.
#[allow(clippy::too_many_arguments)]
fn ad_face(
    m: &mut WorldMesh,
    f: &Frame,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    z: f32,
    face: u8,
    pal: &Palette,
    seed: u32,
) {
    let (x0, x1, y0, y1) = (cx - 0.5 * w, cx + 0.5 * w, cy - 0.5 * h, cy + 0.5 * h);
    let Some(pic) = face
        .checked_sub(1)
        .and_then(|k| Pictogram::ALL.get(usize::from(k)).copied())
    else {
        let blank =
            Paint::new(pal.stucco_white, Pattern::Paint).glowing(pal.window_warm, Glow::Neon);
        rect(m, f, x0, y0, x1, y1, z, false, blank);
        return;
    };
    let grounds = [
        pal.navy,
        pal.teal,
        pal.hydrant_red,
        pal.paint_yellow,
        pal.stucco_white,
    ];
    let h2 = mix64(u64::from(seed) ^ 0x4641_4345);
    let ground = grounds[(h2 % grounds.len() as u64) as usize];
    rect(
        m,
        f,
        x0,
        y0,
        x1,
        y1,
        z,
        false,
        Paint::new(ground, Pattern::Paint),
    );
    let light = if ground == pal.paint_yellow || ground == pal.stucco_white {
        shade(pal.navy, 160)
    } else {
        pal.stucco_white
    };
    let tints = Tints {
        body: Paint::new(light, Pattern::Paint),
        accent: Paint::new(pal.salmon, Pattern::Paint),
        dark: Paint::new(shade(pal.navy, 90), Pattern::Paint),
        light: Paint::new(pal.stucco_white, Pattern::Paint),
    };
    let parts = pictogram::parts(pic);
    let size = 0.85 * h.min(w);
    pictogram::extrude(m, f, &parts, cx, cy, size, z, z + 0.04, false, &tints);
}

/// A billboard on two posts with a catwalk and flood lamps (as [`billboard`]), its face
/// blank or one pictogram.
fn face_billboard(m: &mut WorldMesh, base: V3, facing: V2, face: u8, pal: &Palette, seed: u32) {
    let steel = Paint::new(shade(pal.metal_dark, 260), Pattern::Metal);
    let f = Frame::upright(base, facing.left());
    for x in [-3.2, 3.2] {
        let (lo, hi) = ([x - 0.16, 0.0, -0.55], [x + 0.16, 9.2, -0.25]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, steel);
    }
    let (w, y0, y1) = (5.2, 5.6, 9.2);
    boxf(
        m,
        &f,
        [-w, y0, -0.25],
        [w, y1, 0.0],
        face::ALL & !face::POS_Z,
        steel,
    );
    boxf(
        m,
        &f,
        [-w, y0 - 0.25, 0.0],
        [w, y0 - 0.15, 0.9],
        face::ALL,
        steel,
    );
    ad_face(
        m,
        &f,
        0.0,
        0.5 * (y0 + y1),
        2.0 * w,
        y1 - y0,
        0.0,
        face,
        pal,
        seed,
    );
    let lens = Paint::new(pal.window_warm, Pattern::Glass).glowing(pal.sodium, Glow::Lamp);
    for x in [-3.4, 0.0, 3.4] {
        let (lo, hi) = ([x - 0.2, y0 - 0.2, 1.2], [x + 0.2, y0 + 0.05, 1.45]);
        boxf(m, &f, lo, hi, face::ALL & !face::POS_Y, steel);
        quad(
            m,
            [
                f.p(x - 0.16, y0 + 0.051, 1.24),
                f.p(x + 0.16, y0 + 0.051, 1.24),
                f.p(x + 0.16, y0 + 0.051, 1.41),
                f.p(x - 0.16, y0 + 0.051, 1.41),
            ],
            f.y,
            lens,
        );
    }
}

/// Size of a bus shelter: length along the curb, depth, roof height, m.
pub const SHELTER_M: (f32, f32, f32) = (3.6, 1.5, 2.5);

/// A bus shelter: posts, a flat roof, a glazed back screen, a bench, and at one end a lit
/// ad box facing the oncoming traffic, open toward `facing`.
fn bus_shelter(m: &mut WorldMesh, base: V3, facing: V2, face: u8, pal: &Palette, seed: u32) {
    let (len, depth, h) = SHELTER_M;
    let steel = Paint::new(pal.metal_light, Pattern::Metal);
    let glass = Paint::new(mix(pal.glass, pal.sky_blue, 90), Pattern::Glass);
    // X along the curb, +Z out of the open side.
    let f = Frame::upright(base, facing.left());
    let (x0, x1, zb) = (-0.5 * len, 0.5 * len, -depth);
    for x in [x0 + 0.05, x1 - 0.05] {
        for z in [zb + 0.05, -0.05] {
            boxf(
                m,
                &f,
                [x - 0.04, 0.0, z - 0.04],
                [x + 0.04, h, z + 0.04],
                face::NO_BOTTOM,
                steel,
            );
        }
    }
    boxf(
        m,
        &f,
        [x0 - 0.1, h, zb - 0.1],
        [x1 + 0.1, h + 0.12, 0.2],
        face::ALL,
        steel,
    );
    rect(m, &f, x0, 0.3, x1, h - 0.1, zb, false, glass);
    rect(m, &f, x0, 0.3, x1, h - 0.1, zb, true, glass);
    let seat = Paint::new(shade(pal.metal_dark, 300), Pattern::Metal);
    boxf(
        m,
        &f,
        [x0 + 0.4, 0.42, zb + 0.05],
        [x1 - 1.4, 0.48, zb + 0.45],
        face::ALL,
        seat,
    );
    // The ad box at the downstream end, its face toward the street end of the shelter.
    let g = Frame::upright(f.p(x1 - 0.5, 0.0, 0.5 * zb), facing);
    boxf(
        m,
        &g,
        [-0.6, 0.15, -0.1],
        [0.6, 2.0, 0.1],
        face::NO_BOTTOM,
        steel,
    );
    ad_face(m, &g, 0.0, 1.1, 1.05, 1.65, 0.105, face, pal, seed);
    let back = Frame::upright(g.o, -facing);
    ad_face(m, &back, 0.0, 1.1, 1.05, 1.65, 0.105, face, pal, seed ^ 1);
}

/// Distance between a track's rails (standard gauge) and between its ties.
const GAUGE_M: f32 = 1.435;
const TIE_STEP_M: f32 = 1.2;

/// Two rails on ties, on a ballast prism (its foot under the ground), a crossing's flush
/// panels or a steel deck of two plate girders; a spur's end gets a bumper.
fn track(m: &mut WorldMesh, base: V3, along: V2, len: f32, bed: u8, pal: &Palette) {
    let bed = bed & !place::TRACK_SPUR;
    let f = Frame::upright(base, along);
    let half = len * 0.5;
    let steel = Paint::new(shade(pal.metal_dark, 200), Pattern::Metal);
    let rail_top = Paint::new(pal.metal_light, Pattern::Metal);
    let tie = Paint::new(mix(pal.bark, pal.concrete_dark, 140), Pattern::Plain);
    // Heights of the tie tops and the rail tops over `base`.
    let (tie_y, rail_y) = match bed {
        place::TRACK_CROSSING => (0.0, 0.03),
        _ => (0.42, 0.58),
    };
    match bed {
        place::TRACK_CROSSING => {
            let panel = Paint::new(shade(pal.asphalt_worn, 120), Pattern::Concrete);
            boxf(
                m,
                &f,
                [-half, -0.1, -1.4],
                [half, 0.02, 1.4],
                face::POS_Y,
                panel,
            );
        }
        place::TRACK_BRIDGE => {
            let girder = Paint::new(shade(pal.metal_dark, 170), Pattern::Metal);
            for z in [-1.3f32, 1.3] {
                boxf(
                    m,
                    &f,
                    [-half, tie_y - 1.4, z - 0.12],
                    [half, tie_y + 0.25, z + 0.12],
                    face::ALL & !face::NEG_X & !face::POS_X,
                    girder,
                );
            }
            boxf(
                m,
                &f,
                [-half, tie_y - 1.4, -1.3],
                [half, tie_y - 1.25, 1.3],
                face::NEG_Y,
                girder,
            );
        }
        _ => {
            let ballast = Paint::new(mix(pal.concrete_dark, pal.soil, 90), Pattern::Concrete);
            let (y0, y1) = (-0.4, tie_y - 0.08);
            for sigma in [-1.0f32, 1.0] {
                quad(
                    m,
                    [
                        f.p(-half, y0, sigma * 2.0),
                        f.p(half, y0, sigma * 2.0),
                        f.p(half, y1, sigma * 1.4),
                        f.p(-half, y1, sigma * 1.4),
                    ],
                    f.y + f.z * sigma,
                    ballast,
                );
            }
            quad(
                m,
                [
                    f.p(-half, y1, -1.4),
                    f.p(half, y1, -1.4),
                    f.p(half, y1, 1.4),
                    f.p(-half, y1, 1.4),
                ],
                f.y,
                ballast,
            );
        }
    }
    if bed != place::TRACK_CROSSING {
        let ties = ((len / TIE_STEP_M) as u32).max(1);
        for k in 0..ties {
            let x = -half + (k as f32 + 0.5) * len / ties as f32;
            boxf(
                m,
                &f,
                [x - 0.12, tie_y - 0.08, -1.25],
                [x + 0.12, tie_y, 1.25],
                face::POS_Y | face::NEG_X | face::POS_X,
                tie,
            );
        }
    }
    for z in [-0.5 * GAUGE_M, 0.5 * GAUGE_M] {
        boxf(
            m,
            &f,
            [-half, tie_y, z - 0.04],
            [half, rail_y - 0.02, z + 0.04],
            face::NEG_Z | face::POS_Z,
            steel,
        );
        boxf(
            m,
            &f,
            [-half, rail_y - 0.02, z - 0.04],
            [half, rail_y, z + 0.04],
            face::POS_Y,
            rail_top,
        );
    }
    if bed == place::TRACK_END {
        let bumper = Paint::new(pal.paint_yellow, Pattern::Paint);
        boxf(
            m,
            &f,
            [half - 0.6, rail_y, -1.1],
            [half, rail_y + 0.9, 1.1],
            face::NO_BOTTOM,
            bumper,
        );
    }
}

/// A white crossed pair of blades on a post over two red lamps, facing `facing`.
fn crossbuck(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let post = Paint::new(pal.metal_light, Pattern::Metal);
    let f = Frame::upright(base, facing.left());
    boxf(
        m,
        &f,
        [-0.06, 0.0, -0.06],
        [0.06, 3.9, 0.06],
        face::NO_BOTTOM,
        post,
    );
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let c = f.p(0.0, 3.4, 0.08);
    for (dx, dy) in [(0.69f32, 0.72f32), (0.69, -0.72)] {
        let (u, v) = (f.x * dx + f.y * dy, f.x * -dy + f.y * dx);
        let (u, v) = (u * 0.8, v * 0.1);
        quad(m, [c - u - v, c + u - v, c + u + v, c - u + v], f.z, white);
        quad(m, [c - u - v, c - u + v, c + u + v, c + u - v], -f.z, post);
    }
    // The lamps on a crossarm, dark: a crossing with a train (`setpieces`' gates) flashes
    // its own lamps.
    let arm = f.p(0.0, 2.6, 0.08);
    boxf(
        m,
        &f,
        [-0.55, 2.55, 0.0],
        [0.55, 2.65, 0.1],
        face::ALL,
        post,
    );
    let lamp = Paint::new(shade(pal.neon_red, 90), Pattern::Glass);
    for x in [-0.42f32, 0.42] {
        let q = arm + f.x * x + f.z * 0.04;
        let (u, v) = (f.x * 0.12, f.y * 0.12);
        quad(m, [q - u - v, q + u - v, q + u + v, q - u + v], f.z, lamp);
    }
}

/// Pole on a base, an arm over the road and a cobra head with a glowing sodium lens.
fn street_light(m: &mut WorldMesh, base: V3, arm: V2, pal: &Palette) {
    let metal = Paint::new(pal.metal_light, Pattern::Metal);
    let concrete = Paint::new(pal.concrete, Pattern::Concrete);
    let f = Frame::upright(base, arm);
    frustum(m, &f, 0.24, 0.18, 0.55, 8, true, false, concrete);
    frustum(
        m,
        &f.offset(0.0, 0.55, 0.0),
        0.1,
        0.07,
        7.0,
        8,
        false,
        false,
        metal,
    );
    let h = 7.4;
    // Arm rising slightly from the pole, then the head.
    quad(
        m,
        [
            f.p(0.0, h - 0.2, -0.04),
            f.p(2.0, h, -0.04),
            f.p(2.0, h + 0.08, -0.04),
            f.p(0.0, h - 0.1, -0.04),
        ],
        -f.z,
        metal,
    );
    quad(
        m,
        [
            f.p(0.0, h - 0.2, 0.04),
            f.p(2.0, h, 0.04),
            f.p(2.0, h + 0.08, 0.04),
            f.p(0.0, h - 0.1, 0.04),
        ],
        f.z,
        metal,
    );
    quad(
        m,
        [
            f.p(0.0, h - 0.1, -0.04),
            f.p(2.0, h + 0.08, -0.04),
            f.p(2.0, h + 0.08, 0.04),
            f.p(0.0, h - 0.1, 0.04),
        ],
        f.y,
        metal,
    );
    quad(
        m,
        [
            f.p(0.0, h - 0.2, -0.04),
            f.p(2.0, h, -0.04),
            f.p(2.0, h, 0.04),
            f.p(0.0, h - 0.2, 0.04),
        ],
        -f.y,
        metal,
    );
    boxf(
        m,
        &f,
        [1.75, h - 0.18, -0.19],
        [2.6, h + 0.12, 0.19],
        face::ALL,
        metal,
    );
    let lens = Paint::new(pal.window_warm, Pattern::Glass).glowing(pal.sodium, Glow::Lamp);
    quad(
        m,
        [
            f.p(1.85, h - 0.19, -0.14),
            f.p(2.5, h - 0.19, -0.14),
            f.p(2.5, h - 0.19, 0.14),
            f.p(1.85, h - 0.19, 0.14),
        ],
        -f.y,
        lens,
    );
}

/// Upright frame with local Y along `axis`.
fn frame_along(o: V3, axis: V3) -> Frame {
    let y = axis.normalize();
    let r = if y.z.abs() < 0.9 { V3::Z } else { V3::X };
    let x = y.cross(r).normalize();
    let z = x.cross(y);
    Frame { o, x, y, z }
}

/// A canopy tree: tapered trunk, two limbs and 3-5 smooth leaf lobes.
fn oak(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let bark = Paint::new(pal.bark, Pattern::Bark);
    let trunk_h = rng.range_f32(2.4, 3.3);
    let f = Frame::at(base);
    frustum(m, &f, 0.2, 0.13, trunk_h + 0.6, 6, false, false, bark);
    let top = base + V3::new(0.0, trunk_h, 0.0);
    let lobes = rng.range(3, 5);
    let spin = rng.range_f32(0.0, TAU);
    let leaf = Paint::new(pal.leaf_green, Pattern::Foliage);
    let tint = rng.range(0, 64);
    let (dark, light) = (
        shade(pal.leaf_green, 236 + tint),
        mix(pal.leaf_light, pal.palm_green, tint * 2),
    );
    let main_r = rng.range_f32(1.9, 2.5);
    icosphere(
        m,
        &kit.sphere1,
        top + V3::new(0.0, main_r * 0.75, 0.0),
        V3::new(main_r, main_r * 0.8, main_r),
        true,
        leaf,
        |n| mix(dark, light, ((n.y * 0.5 + 0.5) * 256.0) as u32),
    );
    for k in 1..lobes {
        let (s, c) = sin_cos(spin + TAU * k as f32 / (lobes - 1) as f32);
        let r = rng.range_f32(1.3, 1.8);
        let off = V3::new(
            c * main_r * 0.75,
            rng.range_f32(0.2, 1.1),
            s * main_r * 0.75,
        );
        // A limb out to each side lobe.
        if k <= 2 {
            let limb = frame_along(top + V3::new(0.0, -0.3, 0.0), off + V3::new(0.0, 0.8, 0.0));
            frustum(
                m,
                &limb,
                0.09,
                0.05,
                off.length() * 0.8,
                5,
                false,
                false,
                bark,
            );
        }
        icosphere(
            m,
            &kit.sphere1,
            top + off + V3::new(0.0, r * 0.6, 0.0),
            V3::new(r, r * 0.8, r),
            true,
            leaf,
            |n| mix(dark, light, ((n.y * 0.5 + 0.5) * 240.0) as u32),
        );
    }
}

/// A tall fan palm: bent ringed trunk and a crown of drooping V-section fronds.
fn palm(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let bark = Paint::new(shade(pal.bark, 300), Pattern::Bark);
    let height = rng.range_f32(9.0, 12.5);
    let (ls, lc) = sin_cos(rng.range_f32(0.0, TAU));
    let lean = V3::new(lc, 0.0, ls) * rng.range_f32(0.4, 1.4);
    let segments = 7;
    let spine = |t: f32| base + V3::new(0.0, height * t, 0.0) + lean * (t * t);
    for k in 0..segments {
        let (t0, t1) = (k as f32 / segments as f32, (k + 1) as f32 / segments as f32);
        let (p0, p1) = (spine(t0), spine(t1));
        let r0 = 0.24 - 0.08 * t0;
        let r1 = 0.24 - 0.08 * t1;
        let f = frame_along(p0, p1 - p0);
        frustum(
            m,
            &f,
            r0 * 1.06,
            r1,
            (p1 - p0).length(),
            6,
            false,
            false,
            bark,
        );
    }
    let crown = spine(1.0);
    let frond = Paint::new(pal.palm_green, Pattern::Foliage);
    icosphere(
        m,
        &kit.sphere0,
        crown,
        V3::new(0.45, 0.6, 0.45),
        false,
        frond,
        |_| shade(pal.palm_green, 160),
    );
    let count = 9;
    let spin = rng.range_f32(0.0, TAU);
    for k in 0..count {
        let (s, c) = sin_cos(spin + TAU * k as f32 / count as f32 + rng.range_f32(-0.15, 0.15));
        let out = V3::new(c, 0.0, s);
        let side = V3::new(-s, 0.0, c);
        let lift = rng.range_f32(0.2, 0.7);
        let reach = rng.range_f32(0.9, 1.15);
        let widths = [0.12, 0.55, 0.5, 0.32, 0.03];
        let pts: Vec<V3> = (0..5)
            .map(|j| {
                let t = j as f32;
                crown + out * (t * 0.95 * reach) + V3::Y * (lift * t - 0.32 * t * t * reach)
            })
            .collect();
        let tone = rng.range(200, 290);
        let top_paint = frond.with_rgb(shade(pal.palm_green, tone));
        let under = frond.with_rgb(shade(pal.palm_green, tone * 3 / 5));
        for j in 0..4 {
            let (a, b) = (pts[j], pts[j + 1]);
            let droop = V3::Y * -0.16;
            let (la, lb) = (
                a + side * widths[j] + droop,
                b + side * widths[j + 1] + droop,
            );
            let (ra, rb) = (
                a - side * widths[j] + droop,
                b - side * widths[j + 1] + droop,
            );
            for half in [[a, b, lb, la], [a, ra, rb, b]] {
                quad(m, half, V3::Y, top_paint);
                quad(m, half, -V3::Y, under);
            }
        }
    }
}

fn hydrant(m: &mut WorldMesh, base: V3, dir: V2, pal: &Palette) {
    let red = Paint::new(pal.hydrant_red, Pattern::Metal);
    let f = Frame::upright(base, dir);
    lathe(
        m,
        &f,
        &[
            [0.17, 0.0],
            [0.17, 0.07],
            [0.11, 0.1],
            [0.11, 0.56],
            [0.0, 0.72],
        ],
        8,
        false,
        false,
        red,
    );
    let chrome = Paint::new(pal.metal_light, Pattern::Metal);
    boxf(
        m,
        &f,
        [-0.24, 0.32, -0.05],
        [0.24, 0.42, 0.05],
        face::ALL & !face::NEG_Y,
        red,
    );
    boxf(
        m,
        &f,
        [-0.05, 0.3, 0.0],
        [0.05, 0.44, 0.2],
        face::NO_BOTTOM & !face::NEG_Z,
        chrome,
    );
}

fn bench(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let wood = Paint::new(shade(pal.terracotta, 190), Pattern::Plain);
    let iron = Paint::new(pal.metal_dark, Pattern::Metal);
    let f = Frame::upright(base, facing.left());
    for (z0, z1) in [(-0.22, -0.09), (-0.06, 0.07), (0.1, 0.22)] {
        boxf(m, &f, [-0.95, 0.42, z0], [0.95, 0.46, z1], face::ALL, wood);
    }
    for (y0, y1) in [(0.56, 0.66), (0.72, 0.82)] {
        boxf(m, &f, [-0.95, y0, -0.3], [0.95, y1, -0.26], face::ALL, wood);
    }
    for x in [-0.78, 0.78] {
        boxf(
            m,
            &f,
            [x - 0.03, 0.0, -0.22],
            [x + 0.03, 0.42, 0.2],
            face::NO_BOTTOM,
            iron,
        );
        boxf(
            m,
            &f,
            [x - 0.03, 0.42, -0.3],
            [x + 0.03, 0.86, -0.25],
            face::NO_BOTTOM,
            iron,
        );
    }
}

/// A 60-triangle two-box car silhouette (stand-in until real cars arrive).
fn car(m: &mut WorldMesh, base: V3, forward: V2, color: [u8; 3], pal: &Palette) {
    let body = Paint::new(color, Pattern::Paint);
    let glass = Paint::new(pal.glass, Pattern::Glass);
    // Frame X is forward, Z to the right.
    let f = Frame::upright(base, forward);
    boxf(
        m,
        &f,
        [-2.25, 0.32, -0.88],
        [2.25, 0.95, 0.88],
        face::NO_BOTTOM,
        body,
    );
    let lo = |x: f32, z: f32| f.p(x, 0.95, z);
    let hi = |x: f32, z: f32| f.p(x, 1.42, z);
    let (bx0, bx1, bz) = (-1.35, 0.95, 0.84);
    let (tx0, tx1, tz) = (-1.05, 0.25, 0.7);
    quad(
        m,
        [lo(bx1, -bz), lo(bx1, bz), hi(tx1, tz), hi(tx1, -tz)],
        f.x + f.y,
        glass,
    );
    quad(
        m,
        [lo(bx0, -bz), hi(tx0, -tz), hi(tx0, tz), lo(bx0, bz)],
        -f.x + f.y,
        glass,
    );
    quad(
        m,
        [lo(bx0, bz), hi(tx0, tz), hi(tx1, tz), lo(bx1, bz)],
        f.z,
        glass,
    );
    quad(
        m,
        [lo(bx0, -bz), lo(bx1, -bz), hi(tx1, -tz), hi(tx0, -tz)],
        -f.z,
        glass,
    );
    quad(
        m,
        [hi(tx0, -tz), hi(tx1, -tz), hi(tx1, tz), hi(tx0, tz)],
        f.y,
        body,
    );
    let tire = Paint::new(shade(pal.metal_dark, 120), Pattern::Plain);
    for (x, z) in [(1.42, 0.9), (1.42, -0.9), (-1.42, 0.9), (-1.42, -0.9)] {
        let side = if z > 0.0 { 1.0 } else { -1.0 };
        let c = f.p(x, 0.34, z);
        let hex: Vec<V3> = (0..6)
            .map(|k| {
                let (s, co) = sin_cos(TAU * k as f32 / 6.0);
                c + f.x * (co * 0.34) + f.y * (s * 0.34)
            })
            .collect();
        for k in 1..5 {
            tri(m, hex[0], hex[k], hex[k + 1], f.z * side, tire);
            tri(m, hex[0], hex[k], hex[k + 1], -f.z * side, tire);
        }
    }
    let lamp = Paint::new(pal.stucco_white, Pattern::Plain);
    let tail = Paint::new(pal.neon_red, Pattern::Plain);
    for z in [-0.6, 0.6] {
        quad(
            m,
            [
                f.p(2.255, 0.72, z - 0.18),
                f.p(2.255, 0.72, z + 0.18),
                f.p(2.255, 0.86, z + 0.18),
                f.p(2.255, 0.86, z - 0.18),
            ],
            f.x,
            lamp,
        );
        quad(
            m,
            [
                f.p(-2.255, 0.72, z - 0.18),
                f.p(-2.255, 0.72, z + 0.18),
                f.p(-2.255, 0.86, z + 0.18),
                f.p(-2.255, 0.86, z - 0.18),
            ],
            -f.x,
            tail,
        );
    }
}

/// Octagonal stop sign on a square post, facing `facing`.
fn stop_sign(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let post = Paint::new(pal.metal_light, Pattern::Metal);
    let f = Frame::upright(base, facing.left());
    boxf(
        m,
        &f,
        [-0.03, 0.0, -0.03],
        [0.03, 2.35, 0.03],
        face::NO_BOTTOM,
        post,
    );
    let red = Paint::new(pal.hydrant_red, Pattern::Paint);
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let center = f.p(0.0, 2.05, 0.05);
    let oct = |r: f32, z: f32| -> Vec<V3> {
        (0..8)
            .map(|k| {
                let (s, c) = sin_cos(TAU * (k as f32 + 0.5) / 8.0);
                center + f.x * (c * r) + f.y * (s * r) + f.z * z
            })
            .collect()
    };
    let (outer, inner, back) = (oct(0.38, 0.0), oct(0.33, 0.004), oct(0.38, -0.005));
    for k in 1..7 {
        tri(m, outer[0], outer[k], outer[k + 1], f.z, white);
        tri(m, inner[0], inner[k], inner[k + 1], f.z, red);
        tri(m, back[0], back[k], back[k + 1], -f.z, post);
    }
    street_blades(m, &f, 2.62, pal);
}

/// Two green street-name blades crossing at height `y` on a pole.
fn street_blades(m: &mut WorldMesh, f: &Frame, y: f32, pal: &Palette) {
    let green = Paint::new(shade(pal.palm_green, 170), Pattern::Paint);
    let letters = Paint::new(pal.paint_white, Pattern::Paint);
    boxf(
        m,
        f,
        [-0.45, y, -0.012],
        [0.45, y + 0.16, 0.012],
        face::ALL & !face::NEG_Y,
        green,
    );
    boxf(
        m,
        f,
        [-0.012, y + 0.17, -0.45],
        [0.012, y + 0.33, 0.45],
        face::ALL & !face::NEG_Y,
        green,
    );
    // A strip of "lettering" on each face.
    for z in [0.0125, -0.0125] {
        let q = [
            f.p(-0.33, y + 0.06, z),
            f.p(0.33, y + 0.06, z),
            f.p(0.33, y + 0.1, z),
            f.p(-0.33, y + 0.1, z),
        ];
        quad(m, q, f.z * z, letters);
    }
}

/// A steel trash can.
fn trash_can(m: &mut WorldMesh, base: V3, pal: &Palette) {
    let body = Paint::new(shade(pal.teal, 170), Pattern::Metal);
    let f = Frame::at(base);
    frustum(m, &f, 0.28, 0.3, 0.9, 8, false, false, body);
    frustum(
        m,
        &f.offset(0.0, 0.9, 0.0),
        0.32,
        0.18,
        0.12,
        8,
        true,
        false,
        Paint::new(pal.metal_dark, Pattern::Metal),
    );
}

/// Where a signal's two heads hang along its mast arm, as shares of the arm's length.
pub const SIGNAL_HEADS: [f32; 2] = [0.55, 0.92];

/// A signal head's lens colors in `traffic::signal::Light` order: green, yellow, red.
fn signal_colors(pal: &Palette) -> [Rgb; 3] {
    [[40, 255, 140], [255, 176, 0], pal.neon_red]
}

/// Height of the middle of a signal head's lens showing light `light` (0 green, 1 yellow,
/// 2 red: red on top, green at the bottom).
fn signal_lens_y(light: usize) -> f32 {
    5.6 - (2 - light.min(2)) as f32 * 0.31
}

/// Signal pole with a mast arm over the approach and two heads facing traffic, their
/// lenses dark: the app lights one per head from the live signal ([`signal_lamp`]).
fn signal(m: &mut WorldMesh, base: V3, arm: V2, arm_len: f32, pal: &Palette) {
    let metal = Paint::new(shade(pal.metal_dark, 300), Pattern::Metal);
    let housing = Paint::new(shade(pal.paint_yellow, 200), Pattern::Metal);
    let f = Frame::upright(base, arm);
    frustum(m, &f, 0.16, 0.12, 6.3, 8, true, false, metal);
    street_blades(m, &f, 3.2, pal);
    boxf(
        m,
        &f,
        [0.0, 5.85, -0.06],
        [arm_len, 5.97, 0.06],
        face::ALL,
        metal,
    );
    // The frame's +Z points into the junction; the heads face the other way, toward
    // drivers on the approach this pole serves.
    let colors = signal_colors(pal);
    for t in SIGNAL_HEADS {
        let x = arm_len * t;
        boxf(
            m,
            &f,
            [x - 0.17, 4.85, -0.15],
            [x + 0.17, 5.85, 0.15],
            face::ALL & !face::POS_Y,
            housing,
        );
        for (k, &rgb) in colors.iter().enumerate() {
            let y = signal_lens_y(k);
            let paint = Paint::new(shade(rgb, 10), Pattern::Glass);
            let z = -0.152;
            quad(
                m,
                [
                    f.p(x - 0.1, y - 0.1, z),
                    f.p(x + 0.1, y - 0.1, z),
                    f.p(x + 0.1, y + 0.1, z),
                    f.p(x - 0.1, y + 0.1, z),
                ],
                -f.z,
                paint,
            );
        }
    }
}

/// The lit lens showing light `light` (0 green, 1 yellow, 2 red) of a signal head hung
/// at the origin of a pole facing yaw 0 (its arm along [`dir_of_yaw`]`(0)`), just in front
/// of the dark lens there; the app turns and moves it onto each head.
pub fn signal_lamp(m: &mut WorldMesh, light: usize, pal: &Palette) {
    let f = Frame::upright(V3::ZERO, dir_of_yaw(0.0));
    let rgb = signal_colors(pal)[light.min(2)];
    let lit = Paint::new(rgb, Pattern::Plain).glowing(rgb, Glow::Signal);
    let (y, z) = (signal_lens_y(light), -0.162);
    quad(
        m,
        [
            f.p(-0.11, y - 0.11, z),
            f.p(0.11, y - 0.11, z),
            f.p(0.11, y + 0.11, z),
            f.p(-0.11, y + 0.11, z),
        ],
        -f.z,
        lit,
    );
}

/// A two-tier fountain in a round basin.
fn fountain(m: &mut WorldMesh, base: V3, pal: &Palette) {
    let stone = Paint::new(mix(pal.concrete, pal.sand, 120), Pattern::Concrete);
    let water = Paint::new(mix(pal.sky_blue, pal.teal, 128), Pattern::Glass);
    let f = Frame::at(base);
    lathe(
        m,
        &f,
        &[[4.2, 0.0], [4.2, 0.55], [3.9, 0.6], [3.85, 0.3]],
        14,
        false,
        false,
        stone,
    );
    let disc: Vec<V2> = (0..14)
        .map(|k| {
            let (s, c) = sin_cos(TAU * k as f32 / 14.0);
            base.xz() + V2::new(c, s) * 3.86
        })
        .collect();
    polygon_cap(m, &disc, base.y + 0.38, true, water);
    frustum(m, &f, 0.4, 0.3, 1.5, 8, false, false, stone);
    lathe(
        m,
        &f.offset(0.0, 1.35, 0.0),
        &[[0.3, 0.0], [1.3, 0.25], [1.35, 0.42], [1.15, 0.42]],
        10,
        false,
        false,
        stone,
    );
    frustum(
        m,
        &f.offset(0.0, 1.5, 0.0),
        0.12,
        0.08,
        0.9,
        6,
        true,
        false,
        stone,
    );
}

/// The ground direction a Place prop's `yaw` faces: `(-sin yaw, -cos yaw)` (a spawn's
/// heading convention).
pub fn dir_of_yaw(yaw: f32) -> V2 {
    let (s, c) = sin_cos(yaw);
    V2::new(-s, -c)
}

/// The `yaw` that faces unit ground direction `d` ([`dir_of_yaw`]'s inverse).
pub fn yaw_of_dir(d: V2) -> f32 {
    atan2(-d.x, -d.y)
}

/// Whether the app draws props of `kind` as instances of shared meshes (trees, parked
/// cars, street lights, power poles and the small street furniture) rather than as part of
/// their chunk's mesh, on a terrain place.
pub fn instanced(kind: place::PropKind) -> bool {
    use place::PropKind as P;
    matches!(
        kind,
        P::StreetLight
            | P::PalmTree
            | P::Oak
            | P::Eucalyptus
            | P::ParkedCar { .. }
            | P::Hydrant
            | P::Bench
            | P::TrashCan
            | P::Mailbox
            | P::PowerPole
            | P::StopSign
            | P::HeroCar { .. }
    )
}

/// Which of the four built-in parked-car shapes a Place `ParkedCar`'s `vehicle_class`
/// (low 4 bits: the vehicle crate's `VehicleClass`) stands for: 0, the low sporty wedge
/// (sports, hatch, muscle); 1, the long wagon-sized body (sedan, wagon, pickup, van); 2, the
/// boxy mid-size convertible (coupe, convertible); 3, the big luxury coupe (luxury).
pub fn parked_car_shape(vehicle_class: u8) -> usize {
    // VehicleClass order: Sports, Sedan, Wagon, Coupe, Pickup, Van, Hatch, Convertible,
    // Luxury, Muscle.
    match vehicle_class & 15 {
        0 | 6 | 9 => 0,
        3 | 7 => 2,
        8 => 3,
        _ => 1,
    }
}

/// Length, width and height of a parked car of `vehicle_class`, m: the built-in shapes'.
pub fn parked_car_size(vehicle_class: u8) -> [f32; 3] {
    [
        [4.3, 1.75, 1.15],
        [5.25, 1.9, 1.45],
        [4.62, 1.75, 1.30],
        [5.2, 1.86, 1.36],
    ][parked_car_shape(vehicle_class)]
}

/// Whether a parked car with hour mask `hours` (`place::PropKind::ParkedCar`) standing at
/// `pos` is there at `seconds_of_day`: each car comes or goes at its own minute past the
/// hour (from where it stands), so a lot fills and empties a car at a time.
pub fn parked_now(hours: u32, pos: [f32; 3], seconds_of_day: f32) -> bool {
    if hours == place::ALL_HOURS {
        return true;
    }
    let bits = [u64::from(pos[0].to_bits()), u64::from(pos[2].to_bits())];
    let shift = (skywave_core::hash64(KEY_PARKED_HOUR, &bits) % 3600) as f32;
    let mut t = seconds_of_day - shift;
    while t < 0.0 {
        t += 86_400.0;
    }
    let hour = (t / 3600.0) as u32 % 24;
    hours >> hour & 1 == 1
}

/// A Place prop as a world prop: its facing from its yaw and its variation seed from the
/// place's `seed` and its position (a street sign's from its text seed).
pub fn from_place(p: &place::Prop, seed: u64) -> Prop {
    use place::PropKind as P;
    let kind = match p.kind {
        P::StreetLight => PropKind::StreetLight,
        P::PalmTree => PropKind::PalmTree,
        P::Oak => PropKind::Oak,
        P::Eucalyptus => PropKind::Eucalyptus,
        P::Hedge { len_dm } => PropKind::Hedge { len_dm },
        P::Fence { len_dm } => PropKind::Fence { len_dm },
        P::Bench => PropKind::Bench,
        P::Hydrant => PropKind::Hydrant,
        P::Mailbox => PropKind::Mailbox,
        P::TrashCan => PropKind::TrashCan,
        P::Sign { .. } => PropKind::StreetSign,
        P::NeonSign { palette } => PropKind::NeonSign { palette },
        P::Billboard => PropKind::Billboard,
        P::PowerPole => PropKind::PowerPole,
        P::ParkedCar { vehicle_class, .. } => PropKind::ParkedCar {
            color: vehicle_class >> 4,
        },
        P::StopSign => PropKind::StopSign,
        P::Signal { arm_dm } => PropKind::TrafficSignal { arm_dm },
        P::Pier { len_m } => PropKind::Pier { len_m },
        P::Track { len_dm, bed } => PropKind::Track { len_dm, bed },
        P::Crossbuck => PropKind::Crossbuck,
        P::FaceBillboard { face } => PropKind::FaceBillboard { face },
        P::BusShelter { face } => PropKind::BusShelter { face },
        P::TunnelPiece {
            len_dm,
            width_dm,
            portals,
        } => PropKind::TunnelPiece {
            len_dm,
            width_dm,
            portals,
        },
        P::StartTree => PropKind::StartTree,
        P::TimingBoard => PropKind::TimingBoard,
        P::StripLine { len_dm } => PropKind::StripLine { len_dm },
        P::GuardRail { len_dm } => PropKind::GuardRail { len_dm },
        P::ChainLink { len_dm } => PropKind::ChainLink { len_dm },
        P::CrewGarage { name_seed } => PropKind::CrewGarage { name_seed },
        P::TacoStand { name_seed } => PropKind::TacoStand { name_seed },
        // A hero car stands and collides as a parked car; the app draws it.
        P::HeroCar { slot } => PropKind::ParkedCar { color: slot },
        P::PicnicTable => PropKind::PicnicTable,
        P::CrossingGate { arm_dm } => PropKind::CrossingGate { arm_dm },
        P::Landmark { kit, variant } => PropKind::Landmark { kit, variant },
        P::LotLabel { letter } => PropKind::LotLabel { letter },
    };
    let variation = match p.kind {
        P::Sign { text_seed } => text_seed,
        _ => skywave_core::hash64(
            seed,
            &[u64::from(p.pos[0].to_bits()), u64::from(p.pos[2].to_bits())],
        ),
    };
    Prop {
        pos: V2::new(p.pos[0], p.pos[2]),
        y: p.pos[1],
        dir: dir_of_yaw(p.yaw),
        kind,
        seed: (variation ^ (variation >> 32)) as u32,
    }
}

/// Lettering of `text` in block capitals in the frame's XY plane at depth `z` (facing -Z
/// if `back`), its left end at `x0`, baseline at `y0`, `px` meters a fifth of the cap
/// height. Seen from behind, the text still reads left to right.
#[allow(clippy::too_many_arguments)]
pub(crate) fn letters(
    m: &mut WorldMesh,
    f: &Frame,
    text: &[u8],
    x0: f32,
    y0: f32,
    px: f32,
    z: f32,
    back: bool,
    paint: Paint,
) {
    let mut s = signs::letters::layout(text, signs::letters::LetterStyle::Block);
    s.place(5.0 * px, x0, y0);
    if back {
        s.mirror(x0 + s.ink_w() * 0.5);
    }
    signs::letters::flat(m, f, &s, z, back, paint);
}

/// A eucalyptus: a tall, leaning, pale trunk, a few limbs and a sparse crown of tall
/// lobes with sky between them.
fn eucalyptus(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let bark = Paint::new(mix(pal.bark, pal.stucco_white, 150), Pattern::Bark);
    let height = rng.range_f32(14.0, 19.0);
    let (ls, lc) = sin_cos(rng.range_f32(0.0, TAU));
    let lean = V3::new(lc, 0.0, ls) * rng.range_f32(0.6, 1.8);
    let spine = |t: f32| base + V3::new(0.0, height * t, 0.0) + lean * (t * t);
    let segments = 5;
    let r = |t: f32| 0.34 - 0.24 * t;
    for k in 0..segments {
        let (t0, t1) = (k as f32 / segments as f32, (k + 1) as f32 / segments as f32);
        let (p0, p1) = (spine(t0), spine(t1));
        let f = frame_along(p0, p1 - p0);
        let h = (p1 - p0).length();
        frustum(m, &f, r(t0), r(t1), h, 6, false, false, bark);
    }
    let leaf = Paint::new(pal.leaf_green, Pattern::Foliage);
    let tint = rng.range(0, 64);
    let dark = shade(mix(pal.leaf_green, pal.teal, 80 + tint), 190);
    let light = mix(mix(pal.leaf_light, pal.sky_blue, 70), pal.lawn_dry, tint);
    let lobes = rng.range(5, 8);
    let spin = rng.range_f32(0.0, TAU);
    for k in 0..lobes {
        let t = if k == 0 {
            1.0
        } else {
            0.5 + 0.45 * (k as f32 / lobes as f32)
        };
        let (s, c) = sin_cos(spin + TAU * 0.38 * k as f32);
        let reach = if k == 0 {
            0.4
        } else {
            rng.range_f32(1.6, 3.2) * (1.15 - 0.5 * t)
        };
        let at = spine(t) + V3::new(c * reach, rng.range_f32(0.2, 1.2), s * reach);
        if (1..=3).contains(&k) {
            // A limb out to the lobe.
            let from = spine(t - 0.12);
            let limb = frame_along(from, at - from);
            let len = (at - from).length();
            frustum(m, &limb, 0.09, 0.04, len, 5, false, false, bark);
        }
        let rad = rng.range_f32(1.3, 2.0);
        let tall = rng.range_f32(1.1, 1.5);
        icosphere(
            m,
            &kit.sphere1,
            at,
            V3::new(rad, rad * tall, rad),
            true,
            leaf,
            |n| mix(dark, light, ((n.y * 0.5 + 0.5) * 240.0) as u32),
        );
    }
}

/// A wooden utility pole with a crossarm and insulators, and now and then a transformer.
fn power_pole(m: &mut WorldMesh, base: V3, along: V2, pal: &Palette, rng: &mut Rng) {
    let wood = Paint::new(shade(pal.bark, 240), Pattern::Bark);
    let f = Frame::upright(base, along);
    frustum(m, &f, 0.15, 0.11, 10.4, 6, true, false, wood);
    // Crossarm across the line, insulators on it.
    boxf(
        m,
        &f,
        [-0.06, 9.45, -1.3],
        [0.06, 9.6, 1.3],
        face::ALL,
        wood,
    );
    let glass = Paint::new(mix(pal.mint, pal.concrete, 120), Pattern::Plain);
    for z in [-1.15, -0.45, 0.45, 1.15] {
        let (lo, hi) = ([-0.04, 9.6, z - 0.04], [0.04, 9.8, z + 0.04]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, glass);
    }
    if rng.range(0, 3) == 0 {
        boxf(
            m,
            &f,
            [-0.06, 8.6, -0.9],
            [0.06, 8.72, 0.9],
            face::ALL,
            wood,
        );
    }
    if rng.range(0, 4) == 0 {
        let can = Paint::new(shade(pal.concrete, 200), Pattern::Metal);
        let at = f.offset(0.0, 7.2, 0.42);
        frustum(m, &at, 0.26, 0.26, 0.85, 6, true, false, can);
    }
}

/// A billboard on two posts: a catwalk, three flood lamps and an invented ad on its face.
fn billboard(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette, seed: u32) {
    let mut rng = Rng::new(Seed(u64::from(seed)), &[KEY_PROP, 0xB1]);
    let steel = Paint::new(shade(pal.metal_dark, 260), Pattern::Metal);
    // X along the panel, +Z toward the viewer.
    let f = Frame::upright(base, facing.left());
    for x in [-3.2, 3.2] {
        let (lo, hi) = ([x - 0.16, 0.0, -0.55], [x + 0.16, 9.2, -0.25]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, steel);
    }
    let (w, y0, y1) = (5.2, 5.6, 9.2);
    boxf(
        m,
        &f,
        [-w, y0, -0.25],
        [w, y1, 0.0],
        face::ALL & !face::POS_Z,
        steel,
    );
    boxf(
        m,
        &f,
        [-w, y0 - 0.25, 0.0],
        [w, y0 - 0.15, 0.9],
        face::ALL,
        steel,
    );
    // The ad: a ground, a band and a name.
    let colors = [
        pal.sky_blue,
        pal.salmon,
        pal.teal,
        pal.navy,
        pal.peach,
        pal.mint,
        pal.paint_yellow,
        pal.lavender,
        pal.pastel_pink,
        pal.stucco_white,
    ];
    let pick = |rng: &mut Rng| colors[rng.range(0, colors.len() as u32) as usize];
    let ground = pick(&mut rng);
    let mut band = pick(&mut rng);
    if band == ground {
        band = pal.hydrant_red;
    }
    let ground_paint = Paint::new(ground, Pattern::Paint);
    rect(m, &f, -w, y0, w, y1, 0.0, false, ground_paint);
    let low = rng.range(0, 2) == 0;
    let (b0, b1) = if low {
        (y0 + 0.25, y0 + 1.2)
    } else {
        (y1 - 1.0, y1 - 0.3)
    };
    let band_paint = Paint::new(band, Pattern::Paint);
    rect(m, &f, -w + 0.3, b0, w - 0.3, b1, 0.01, false, band_paint);
    let name = signs::business_name(u64::from(seed), 9);
    let ink = if ground == pal.navy || ground == pal.teal {
        pal.stucco_white
    } else {
        shade(pal.navy, 160)
    };
    let px = 0.24;
    let tw = signs::text_width(&name) as f32 * px;
    let ty = if low { y0 + 1.7 } else { y0 + 0.8 };
    let ink = Paint::new(ink, Pattern::Paint);
    letters(m, &f, &name, -tw * 0.5, ty, px, 0.02, false, ink);
    // Flood lamps on arms over the catwalk, shining up at the face.
    let lens = Paint::new(pal.window_warm, Pattern::Glass).glowing(pal.sodium, Glow::Lamp);
    for x in [-3.4, 0.0, 3.4] {
        let (lo, hi) = ([x - 0.04, y0 - 0.15, 0.0], [x + 0.04, y0 - 0.07, 1.25]);
        boxf(m, &f, lo, hi, face::ALL, steel);
        let (lo, hi) = ([x - 0.2, y0 - 0.2, 1.2], [x + 0.2, y0 + 0.05, 1.45]);
        boxf(m, &f, lo, hi, face::ALL & !face::POS_Y, steel);
        quad(
            m,
            [
                f.p(x - 0.16, y0 + 0.051, 1.24),
                f.p(x + 0.16, y0 + 0.051, 1.24),
                f.p(x + 0.16, y0 + 0.051, 1.41),
                f.p(x - 0.16, y0 + 0.051, 1.41),
            ],
            f.y,
            lens,
        );
    }
}

/// A curbside mailbox on a wooden post with a red flag.
fn mailbox(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette, rng: &mut Rng) {
    let wood = Paint::new(shade(pal.bark, 280), Pattern::Plain);
    let tones = [
        pal.metal_light,
        pal.stucco_white,
        pal.navy,
        shade(pal.metal_dark, 300),
    ];
    let tin = Paint::new(tones[rng.range(0, 4) as usize], Pattern::Metal);
    // +Z toward the street.
    let f = Frame::upright(base, facing.left());
    boxf(
        m,
        &f,
        [-0.05, 0.0, -0.05],
        [0.05, 1.02, 0.05],
        face::NO_BOTTOM,
        wood,
    );
    boxf(
        m,
        &f,
        [-0.12, 1.02, -0.26],
        [0.12, 1.22, 0.24],
        face::ALL,
        tin,
    );
    // Rounded top as a low ridge.
    let (a0, a1) = (f.p(-0.12, 1.22, -0.26), f.p(-0.12, 1.22, 0.24));
    let (b0, b1) = (f.p(0.12, 1.22, -0.26), f.p(0.12, 1.22, 0.24));
    let (r0, r1) = (f.p(0.0, 1.31, -0.26), f.p(0.0, 1.31, 0.24));
    quad(m, [a0, a1, r1, r0], -f.x + f.y, tin);
    quad(m, [b0, r0, r1, b1], f.x + f.y, tin);
    tri(m, a0, r0, b0, -f.z, tin);
    tri(m, a1, b1, r1, f.z, tin);
    let flag = Paint::new(pal.hydrant_red, Pattern::Paint);
    boxf(
        m,
        &f,
        [0.12, 1.1, -0.12],
        [0.14, 1.38, -0.08],
        face::ALL,
        flag,
    );
}

/// A clipped hedge: lumpy boxes along the run, sunk into the ground on slopes.
fn hedge(m: &mut WorldMesh, base: V3, along: V2, len: f32, pal: &Palette, rng: &mut Rng) {
    let f = Frame::upright(base, along);
    let height = rng.range_f32(1.1, 1.6);
    let n = ((len / 1.6) as u32).max(1);
    let step = len / n as f32;
    for k in 0..n {
        let x0 = -len * 0.5 + step * k as f32;
        let h = height + rng.range_f32(-0.12, 0.12);
        let w = rng.range_f32(0.4, 0.5);
        let tone = rng.range(170, 230);
        let leaf = Paint::new(shade(pal.leaf_green, tone), Pattern::Foliage);
        boxf(
            m,
            &f,
            [x0, -0.5, -w],
            [x0 + step, h, w],
            face::NO_BOTTOM,
            leaf,
        );
    }
}

/// A wood fence (boards between posts, two rails behind) or a capped block wall.
fn fence(m: &mut WorldMesh, base: V3, along: V2, len: f32, pal: &Palette, rng: &mut Rng) {
    let f = Frame::upright(base, along);
    let half = len * 0.5;
    if rng.range(0, 2) == 0 {
        let tones = [
            pal.concrete,
            pal.sand,
            pal.stucco_cream,
            shade(pal.concrete, 230),
        ];
        let block = Paint::new(tones[rng.range(0, 4) as usize], Pattern::Concrete);
        let h = rng.range_f32(1.5, 1.8);
        boxf(
            m,
            &f,
            [-half, -0.5, -0.1],
            [half, h, 0.1],
            face::NO_BOTTOM,
            block,
        );
        let cap = block.with_rgb(shade(block.rgb(), 230));
        let (lo, hi) = ([-half, h, -0.13], [half, h + 0.08, 0.13]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, cap);
        return;
    }
    let tones = [
        shade(pal.bark, 300),
        shade(pal.bark, 380),
        mix(pal.bark, pal.sand, 120),
    ];
    let wood = Paint::new(tones[rng.range(0, 3) as usize], Pattern::Plain);
    let h = 1.8;
    let posts = ((len / 2.4) as u32).max(1);
    for k in 0..=posts {
        let x = (-half + len * k as f32 / posts as f32).clamp(-half + 0.05, half - 0.05);
        let (lo, hi) = ([x - 0.05, -0.5, -0.12], [x + 0.05, h + 0.05, -0.02]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, wood);
    }
    for y in [0.35, 1.45] {
        boxf(
            m,
            &f,
            [-half, y, -0.1],
            [half, y + 0.09, -0.02],
            face::ALL,
            wood,
        );
    }
    let boards = wood.with_rgb(shade(wood.rgb(), 270));
    rect(m, &f, -half, -0.5, half, h, 0.0, false, boards);
    rect(m, &f, -half, -0.5, half, h, -0.02, true, boards);
}

/// A street-name post: two crossed blades whose lettering follows an invented name (a
/// block per letter: at reading distance a sign's words are shapes).
fn street_sign(m: &mut WorldMesh, base: V3, dir: V2, pal: &Palette, seed: u32) {
    let post = Paint::new(pal.metal_light, Pattern::Metal);
    let f = Frame::upright(base, dir);
    boxf(
        m,
        &f,
        [-0.03, 0.0, -0.03],
        [0.03, 3.05, 0.03],
        face::NO_BOTTOM,
        post,
    );
    let green = Paint::new(shade(pal.palm_green, 170), Pattern::Paint);
    let ink = Paint::new(pal.paint_white, Pattern::Paint);
    for (k, y) in [(0u64, 2.7f32), (1, 2.9)] {
        let name = signs::street_name(u64::from(seed) ^ k);
        // The second blade turned a quarter.
        let b = if k == 0 {
            f
        } else {
            Frame {
                x: f.z,
                z: -f.x,
                ..f
            }
        };
        let (lo, hi) = ([-0.5, y, -0.012], [0.5, y + 0.18, 0.012]);
        boxf(m, &b, lo, hi, face::ALL & !face::NEG_Y, green);
        let n = name.len().max(1) as f32;
        let cell = 0.84 / n;
        for (i, &c) in name.iter().enumerate() {
            if c == b' ' {
                continue;
            }
            let x = -0.42 + cell * i as f32;
            let (xa, xb) = (x + cell * 0.12, x + cell * 0.88);
            rect(m, &b, xa, y + 0.05, xb, y + 0.13, 0.0125, false, ink);
            rect(m, &b, -xb, y + 0.05, -xa, y + 0.13, -0.0125, true, ink);
        }
    }
}

/// Builds a blade sign prop `p` lettered as the shop sign `sign` with words from `lex`
/// (`p` must be a [`PropKind::NeonSign`]).
pub fn shop_sign(m: &mut WorldMesh, p: &Prop, pal: &Palette, sign: &ShopSign, lex: &Lexicon) {
    if let PropKind::NeonSign { palette } = p.kind {
        let base = p.pos.at(p.y);
        neon_sign(m, base, p.dir, palette, pal, p.seed, Some((sign, lex)));
    }
}

/// Most triangles a blade sign spends (`docs/RENDERING.md`, "Mesh and instance budgets").
pub const BLADE_TRIANGLES: usize = 360;
/// Cap height of a blade sign's stacked letters when they fit, m; the stroke is a fifth of it.
const BLADE_CAP: f32 = 0.6;

/// A vertical blade sign on brackets out from a wall, lettered on both faces. The word
/// comes first: stacked capitals up to [`BLADE_CAP`] tall (or, in script, reading upward),
/// bold enough to read from the street. The shop's pictogram goes above it when the word
/// leaves room and triangles for it. A neon sign is a dark box framed in a neon tube with
/// neon letters, the pictogram outlined in the frame's color; a backlit one
/// ([`ShopSign::lit_box`]) a lit panel with dark letters. Without a sign (a prop built on its
/// own) only the frame and a pictogram. True when a word was lettered.
fn neon_sign(
    m: &mut WorldMesh,
    base: V3,
    out: V2,
    palette: u8,
    pal: &Palette,
    seed: u32,
    sign: Option<(&ShopSign, &Lexicon)>,
) -> bool {
    let start = m.triangle_count();
    let neons = pal.neon();
    let c = neons[usize::from(palette) % neons.len()];
    let tube = Paint::new(c, Pattern::Plain).glowing(c, Glow::Neon);
    let other = neons[(usize::from(palette) + 2) % neons.len()];
    let border = Paint::new(other, Pattern::Plain).glowing(other, Glow::Neon);
    let dark = Paint::new(shade(pal.navy, 150), Pattern::Metal);
    let steel = Paint::new(pal.metal_dark, Pattern::Metal);
    // X out from the wall, Y up, the faces toward ±Z.
    let f = Frame::upright(base, out);
    let (x0, x1, y0, y1, t) = (0.3, 1.5, NEON_SIGN_Y.0, NEON_SIGN_Y.1, 0.12);
    let cx = (x0 + x1) * 0.5;
    for y in [y0 + 0.4, y1 - 0.4] {
        let (lo, hi) = ([0.0, y - 0.05, -0.05], [x0, y + 0.05, 0.05]);
        boxf(
            m,
            &f,
            lo,
            hi,
            face::ALL & !(face::NEG_X | face::POS_X),
            steel,
        );
    }
    boxf(m, &f, [x0, y0, -t], [x1, y1, t], face::ALL, dark);
    let lit_box = sign.is_some_and(|(s, _)| s.lit_box);
    let ink = if lit_box {
        let glow = shade(mix(c, [255, 244, 214], 190), 140);
        let panel = Paint::new(glow, Pattern::Plain).glowing(glow, Glow::Neon);
        for back in [false, true] {
            let z = if back { -t - 0.003 } else { t + 0.003 };
            rect(
                m,
                &f,
                x0 + 0.05,
                y0 + 0.05,
                x1 - 0.05,
                y1 - 0.05,
                z,
                back,
                panel,
            );
        }
        Paint::new(shade(pal.navy, 110), Pattern::Plain)
    } else {
        // The frame tube.
        for back in [false, true] {
            let z = if back { -t - 0.01 } else { t + 0.01 };
            let (a, b, d) = (x0 + 0.07, x1 - 0.07, 0.05);
            rect(m, &f, a, y0 + 0.07, b, y0 + 0.07 + d, z, back, border);
            rect(m, &f, a, y1 - 0.07 - d, b, y1 - 0.07, z, back, border);
            rect(m, &f, a, y0 + 0.07, a + d, y1 - 0.07, z, back, border);
            rect(m, &f, b - d, y0 + 0.07, b, y1 - 0.07, z, back, border);
        }
        tube
    };
    // Inside the frame.
    let (bottom, top, width) = (y0 + 0.2, y1 - 0.2, x1 - x0 - 0.3);
    let finish = if lit_box {
        Finish::Backlit
    } else {
        Finish::Neon
    };
    // The word, with the triangles of what is already built.
    let mut word = None;
    if let Some((s, lex)) = sign {
        let left = BLADE_TRIANGLES.saturating_sub(m.triangle_count() - start);
        let mut words: Vec<Strokes> = Vec::with_capacity(3);
        if s.style == LetterStyle::Script {
            words.push(sideways_word(&s.text(lex), s.style, cx, bottom, top, width));
        }
        // Then the trade, then both in block capitals (the fewest strokes).
        let trade = lex.trades[usize::from(s.trade)].as_bytes();
        for style in [s.style, LetterStyle::Block] {
            words.push(stacked_word(&s.word(lex), style, cx, bottom, top, width));
            words.push(stacked_word(trade, style, cx, bottom, top, width));
        }
        word = words.into_iter().find(|w| 4 * w.segments() <= left);
    }
    // The pictogram above the word, in the room it leaves.
    let used_top = word.as_ref().map_or(bottom, |w| w.bounds[3]);
    let room = top - used_top - 0.12;
    let picture = match sign {
        Some((s, _)) => s.pictogram.filter(|_| room >= 0.4),
        None => Some(Pictogram::ALL[(mix64(u64::from(seed) ^ 0x5049_4354) % 32) as usize]),
    };
    let mut word_top = top;
    if let Some(pic) = picture {
        let size = room.clamp(0.4, 0.9);
        let pic_y = top - size * 0.5;
        let parts = pictogram::parts(pic);
        let slab: usize = parts
            .iter()
            .map(|q| 2 * (q.poly.len() - 2) + if q.solid { 2 * q.poly.len() } else { 0 })
            .sum();
        let outline = if lit_box {
            0
        } else {
            4 * pictogram::outline_len(&parts)
        };
        let words = word.as_ref().map_or(0, |w| 4 * w.segments());
        let spent = m.triangle_count() - start + words;
        if spent + slab <= BLADE_TRIANGLES {
            let h = mix64(u64::from(seed) ^ 0x5449_4E54);
            let colors = [
                pal.salmon,
                pal.mint,
                pal.sky_blue,
                pal.paint_yellow,
                pal.peach,
                pal.pastel_pink,
                pal.stucco_white,
                pal.lavender,
            ];
            let body = colors[(h % colors.len() as u64) as usize];
            let accent = colors[((h >> 8) % colors.len() as u64) as usize];
            let accent = if accent == body { pal.teal } else { accent };
            let tints = Tints {
                body: Paint::new(body, Pattern::Paint),
                accent: Paint::new(accent, Pattern::Paint),
                dark: Paint::new(shade(pal.navy, 90), Pattern::Paint),
                light: Paint::new(pal.stucco_white, Pattern::Paint),
            };
            let (za, zb) = (-t - 0.04, t + 0.04);
            pictogram::extrude(m, &f, &parts, cx, pic_y, size, za, zb, true, &tints);
            word_top = pic_y - size * 0.5 - 0.1;
            if outline > 0 && spent + slab + outline <= BLADE_TRIANGLES {
                for back in [false, true] {
                    let z = if back { za - 0.016 } else { zb + 0.016 };
                    pictogram::neon_outline(m, &f, &parts, cx, pic_y, size, z, back, 0.035, border);
                }
            }
        }
    }
    // The word centered in what the pictogram leaves.
    if let Some(w) = &mut word {
        let dy = (bottom + word_top - w.bounds[1] - w.bounds[3]) * 0.5;
        w.transform([1.0, 0.0, 0.0, 1.0, 0.0, dy], 1.0);
    }
    let lettered = word.is_some();
    if let Some(st) = word {
        for back in [false, true] {
            let mut q = st.clone();
            if back {
                q.mirror(cx);
            }
            let z = if back { -t } else { t };
            signs::letters::build(m, &f, &q, finish, z, back, ink, ink, 0.0);
        }
    }
    lettered
}

/// `text` as bold letters stacked down a blade from `bottom`, centered on `cx`, at most
/// [`BLADE_CAP`] tall and `width` across, shrunk to fit below `top`.
fn stacked_word(
    text: &[u8],
    style: LetterStyle,
    cx: f32,
    bottom: f32,
    top: f32,
    width: f32,
) -> Strokes {
    let style = if style == LetterStyle::Script {
        LetterStyle::Block
    } else {
        style
    };
    let slots: f32 = text
        .iter()
        .map(|&ch| if ch == b' ' { 0.5 } else { 1.0 })
        .sum();
    let pitch = ((top - bottom) / slots.max(1.0)).min(BLADE_CAP * 1.2);
    let mut out = Strokes::default();
    let mut y = bottom + pitch * slots;
    for &ch in text {
        if ch == b' ' {
            y -= pitch * 0.5;
            continue;
        }
        let mut g = signs::letters::layout(&[ch], style);
        let cap = (pitch / 1.2).min(width / g.ink_w().max(1e-3));
        g.place(
            cap,
            cx - g.ink_w() * cap * 0.5,
            y - pitch + (pitch - cap) * 0.5,
        );
        y -= pitch;
        for line in g.lines() {
            out.starts.push(out.points.len() as u32);
            out.points.extend_from_slice(line);
        }
        out.width = BLADE_CAP.min(cap) * 0.2;
    }
    out.bounds = [
        cx - width * 0.5,
        bottom,
        cx + width * 0.5,
        bottom + pitch * slots,
    ];
    out
}

/// `text` on its side reading upward along a blade, centered on `cx` in `bottom..top`, its
/// letters at most `width` tall, the tube bold.
fn sideways_word(
    text: &[u8],
    style: LetterStyle,
    cx: f32,
    bottom: f32,
    top: f32,
    width: f32,
) -> Strokes {
    let mut s = signs::letters::layout(text, style);
    let k = ((top - bottom) / s.ink_w().max(1e-3))
        .min(width / s.ink_h().max(1e-3))
        .min(1.0);
    let [bx0, by0, bx1, by1] = s.bounds;
    let (mx, my) = ((bx0 + bx1) * 0.5, (by0 + by1) * 0.5);
    let len = (bx1 - bx0) * k;
    // From the foot of the frame up, turned a quarter counter-clockwise: the tops of the
    // letters toward the wall.
    let yc = bottom + len * 0.5;
    s.transform([0.0, -k, k, 0.0, cx + k * my, yc - k * mx], k);
    s.width = (k * 0.11).max(s.width);
    s
}

/// Height range of a neon blade sign's box above its base, m.
pub const NEON_SIGN_Y: (f32, f32) = (3.6, 7.6);
/// How far a neon blade sign's box reaches out from the wall: `(near, far)`, m.
pub const NEON_SIGN_OUT: (f32, f32) = (0.3, 1.5);

/// A wooden pier on pilings: a deck with railings out over the water to a wider head with
/// a bait shop.
fn pier(m: &mut WorldMesh, base: V3, out: V2, len: f32, pal: &Palette, rng: &mut Rng) {
    let f = Frame::upright(base, out);
    let wood = mix(pal.bark, pal.sand, 130);
    let piling = Paint::new(shade(pal.bark, 220), Pattern::Bark);
    let rail = Paint::new(pal.stucco_white, Pattern::Paint);
    let (hw, head_w, head_l) = (PIER_HALF_WIDTH_M, PIER_HEAD_HALF_WIDTH_M, PIER_HEAD_M);
    let span = 6.0;
    let n = ((len / span) as u32).max(1);
    let sides = face::ALL & !face::NEG_Y & !face::POS_Y;
    let deck = |m: &mut WorldMesh, x0: f32, x1: f32, w: f32, rng: &mut Rng| {
        let p = Paint::new(shade(wood, rng.range(215, 250)), Pattern::Plain);
        boxf(m, &f, [x0, -0.45, -w], [x1, 0.0, w], face::ALL, p);
    };
    for k in 0..n {
        let x = span * k as f32;
        deck(m, x, x + span, hw, rng);
        // A bent of two pilings and a cap beam under each joint.
        for z in [-hw + 0.5, hw - 0.5] {
            let (lo, hi) = ([x + 0.2, -9.0, z - 0.18], [x + 0.56, -0.45, z + 0.18]);
            boxf(m, &f, lo, hi, sides, piling);
        }
        let (lo, hi) = ([x + 0.15, -0.8, -hw], [x + 0.6, -0.45, hw]);
        boxf(m, &f, lo, hi, face::ALL & !face::POS_Y, piling);
    }
    let end = span * n as f32;
    // The head: wider, on its own pilings.
    deck(m, end, end + head_l, head_w, rng);
    for x in [end + 2.0, end + 8.0, end + 14.0] {
        for z in [-head_w + 0.6, 0.0, head_w - 0.6] {
            let (lo, hi) = ([x - 0.18, -9.0, z - 0.18], [x + 0.18, -0.45, z + 0.18]);
            boxf(m, &f, lo, hi, sides, piling);
        }
    }
    // Railings: posts every span, two rails.
    let posts = |m: &mut WorldMesh, x0: f32, x1: f32, z: f32| {
        let k = (((x1 - x0) / span) as u32).max(1);
        for i in 0..=k {
            let x = x0 + (x1 - x0) * i as f32 / k as f32;
            let (lo, hi) = ([x - 0.06, 0.0, z - 0.06], [x + 0.06, 1.1, z + 0.06]);
            boxf(m, &f, lo, hi, face::NO_BOTTOM, rail);
        }
    };
    for z in [-hw + 0.1, hw - 0.1] {
        posts(m, 0.0, end, z);
        for y in [0.5, 1.02] {
            boxf(
                m,
                &f,
                [0.0, y, z - 0.04],
                [end, y + 0.08, z + 0.04],
                face::ALL,
                rail,
            );
        }
    }
    for z in [-head_w + 0.1, head_w - 0.1] {
        posts(m, end, end + head_l, z);
        let (lo, hi) = ([end, 1.02, z - 0.04], [end + head_l, 1.1, z + 0.04]);
        boxf(m, &f, lo, hi, face::ALL, rail);
    }
    let (lo, hi) = (
        [end + head_l - 0.1, 1.02, -head_w],
        [end + head_l, 1.1, head_w],
    );
    boxf(m, &f, lo, hi, face::ALL, rail);
    // The bait shop at the end: a stucco box with a lipped flat roof, a door and a window.
    let wall = Paint::new(pal.stucco_white, Pattern::Stucco);
    let trim = Paint::new(pal.teal, Pattern::Paint);
    let (sx0, sx1, sz) = (end + head_l - 7.5, end + head_l - 1.5, 3.0);
    boxf(
        m,
        &f,
        [sx0, 0.0, -sz],
        [sx1, 3.0, sz],
        face::NO_BOTTOM,
        wall,
    );
    let (lo, hi) = ([sx0 - 0.3, 3.0, -sz - 0.3], [sx1 + 0.3, 3.25, sz + 0.3]);
    boxf(m, &f, lo, hi, face::ALL, trim);
    let glass = Paint::new(pal.glass, Pattern::Glass).glowing(pal.window_warm, Glow::Window);
    // The shop's landward wall, seen from the deck.
    let front = Frame::upright(f.p(sx0, 0.0, -sz), f.z.xz());
    rect(m, &front, 0.8, 1.0, 2.8, 2.2, 0.01, false, glass);
    rect(m, &front, 3.6, 0.0, 4.6, 2.2, 0.01, false, trim);
}

/// Half the width of a pier's walk, of its head, and the head's length, m.
pub const PIER_HALF_WIDTH_M: f32 = 4.5;
pub const PIER_HEAD_HALF_WIDTH_M: f32 = 9.0;
pub const PIER_HEAD_M: f32 = 16.0;

/// The [`instanced`] kinds at full detail ([`PropKit::instanced`]), each bench or trash
/// can as one of its street-clutter designs; false for the other kinds, which have one
/// shape.
fn build_fine(
    m: &mut WorldMesh,
    base: V3,
    p: &Prop,
    pal: &Palette,
    kit: &PropKit,
    rng: &mut Rng,
) -> bool {
    match p.kind {
        PropKind::StreetLight => cobra_light(m, base, p.dir, pal),
        PropKind::Oak => lobed_oak(m, base, pal, kit, rng),
        PropKind::PalmTree => fan_palm(m, base, pal, kit, rng),
        PropKind::Eucalyptus => gum_tree(m, base, pal, kit, rng),
        PropKind::Hydrant => full_hydrant(m, base, p.dir, pal),
        // Each design takes a share of eighths: half the benches are plain, a quarter each
        // at a bus stop or under a shelter; most trash cans are cans, some vending boxes
        // and a planter now and then. (Which eighths: so that the app's few shared shapes
        // per kind, `app::propinst`, come out in about these shares.)
        PropKind::Bench => match rng.range(0, 8) {
            0 | 5 | 6 | 7 => {
                let (wood, iron) = bench_paints(pal, rng);
                park_bench(m, &Frame::upright(base, p.dir.left()), wood, iron);
            }
            1 | 4 => bus_stop(m, base, p.dir, pal, rng),
            _ => bench_shelter(m, base, p.dir, pal, rng),
        },
        PropKind::TrashCan => match rng.range(0, 8) {
            0 | 1 | 3 => steel_can(m, base, pal, rng),
            2 | 4 => wire_can(m, base, pal),
            6 | 7 => news_boxes(m, base, p.dir, pal, rng),
            _ => planter(m, base, pal, kit, rng),
        },
        PropKind::Mailbox => full_mailbox(m, base, p.dir, pal, rng),
        PropKind::PowerPole => full_power_pole(m, base, p.dir, pal, rng),
        PropKind::StopSign => full_stop_sign(m, base, p.dir, pal),
        _ => return false,
    }
    true
}

/// A square-section bar `w` by `t` from `a` to `b`, its `w` side across `across`; `ends`
/// closes it.
#[allow(clippy::too_many_arguments)]
fn bar(m: &mut WorldMesh, a: V3, b: V3, across: V3, w: f32, t: f32, ends: bool, paint: Paint) {
    let axis = (b - a).normalize();
    let u = (across - axis * across.dot(axis)).normalize();
    let v = axis.cross(u);
    let (hu, hv) = (u * (w * 0.5), v * (t * 0.5));
    for (n, s) in [(hu, hv), (-hu, hv), (hv, hu), (-hv, hu)] {
        quad(m, [a + n - s, b + n - s, b + n + s, a + n + s], n, paint);
    }
    if ends {
        for (c, n) in [(a, -axis), (b, axis)] {
            quad(
                m,
                [c - hu - hv, c + hu - hv, c + hu + hv, c - hu + hv],
                n,
                paint,
            );
        }
    }
}

/// A round tube tapering from `r0` at `a` to `r1` at `b`.
fn tube(m: &mut WorldMesh, a: V3, b: V3, r0: f32, r1: f32, sides: u32, paint: Paint) {
    let f = frame_along(a, b - a);
    frustum(m, &f, r0, r1, (b - a).length(), sides, false, false, paint);
}

/// An irregular leaf lump: a smooth [`UnitSphere`] stretched to `radii`, its surface pushed
/// in and out by three waves in seeded directions so no two lumps share a silhouette.
/// `shade` maps the unit normal to the vertex color, as in [`icosphere`].
fn lump(
    m: &mut WorldMesh,
    sphere: &UnitSphere,
    center: V3,
    radii: V3,
    rng: &mut Rng,
    paint: Paint,
    shade: impl Fn(V3) -> [u8; 3],
) {
    const AMP: f32 = 0.07;
    const FREQ: f32 = 2.0;
    let mut waves = [(V3::ZERO, 0.0f32); 3];
    for w in &mut waves {
        let (s0, c0) = sin_cos(rng.range_f32(0.0, TAU));
        let (s1, c1) = sin_cos(rng.range_f32(-1.2, 1.2));
        *w = (V3::new(c0 * c1, s1, s0 * c1), rng.range_f32(0.0, TAU));
    }
    let inv = V3::new(1.0 / radii.x, 1.0 / radii.y, 1.0 / radii.z);
    for &[a, b, c] in &sphere.tris {
        let first = m.vertices.len() as u32;
        for u in [a, b, c] {
            // Radius 1 + AMP * sum(sin), and its gradient for the normal.
            let (mut k, mut g) = (1.0, V3::ZERO);
            for &(d, phase) in &waves {
                let (s, co) = sin_cos(FREQ * u.dot(d) + phase);
                k += AMP * s;
                g = g + d * (AMP * FREQ * co);
            }
            let n = u * k - (g - u * u.dot(g));
            let n = V3::new(n.x * inv.x, n.y * inv.y, n.z * inv.z).normalize();
            let p = center + V3::new(u.x * radii.x, u.y * radii.y, u.z * radii.z) * k;
            m.vertices.push(crate::mesh::WorldVertex {
                pos: p.to_array(),
                normal: n.to_array(),
                uv: [u.x, u.y],
                color: paint.with_rgb(shade(n)).color,
                emissive: paint.emissive,
            });
        }
        m.indices.extend_from_slice(&[first, first + 1, first + 2]);
    }
}

/// A street light at full detail: a base plate and a flared foot, a tapered pole that
/// bends into a curved arm, and a cobra-head housing over the same lens as
/// [`street_light`]'s (the photocell and the lamp heads key on that lens).
fn cobra_light(m: &mut WorldMesh, base: V3, arm: V2, pal: &Palette) {
    let metal = Paint::new(pal.metal_light, Pattern::Metal);
    let concrete = Paint::new(pal.concrete, Pattern::Concrete);
    let f = Frame::upright(base, arm);
    let h = 7.4;
    boxf(
        m,
        &f,
        [-0.3, 0.0, -0.3],
        [0.3, 0.06, 0.3],
        face::NO_BOTTOM,
        concrete,
    );
    lathe(
        m,
        &f,
        &[[0.2, 0.06], [0.18, 0.4], [0.13, 0.55], [0.095, 0.6]],
        8,
        false,
        false,
        metal,
    );
    let top = h - 0.65;
    frustum(
        m,
        &f.offset(0.0, 0.6, 0.0),
        0.095,
        0.065,
        top - 0.6,
        8,
        false,
        false,
        metal,
    );
    // The arm: a quarter bend out of the pole top, then straight to the head.
    let bend = 0.6;
    let mut pts = [V3::ZERO; 6];
    for (k, p) in pts.iter_mut().enumerate().take(5) {
        let (s, c) = sin_cos(TAU * 0.25 * k as f32 / 4.0);
        *p = f.p(bend * (1.0 - c), top + bend * s, 0.0);
    }
    pts[5] = f.p(1.75, top + bend + 0.02, 0.0);
    for k in 0..5 {
        let r = |t: f32| 0.065 - 0.02 * t;
        tube(
            m,
            pts[k],
            pts[k + 1],
            r(k as f32 / 5.0),
            r((k + 1) as f32 / 5.0),
            6,
            metal,
        );
    }
    // The head: half-elliptic sections along the arm over a flat underside at `yb`.
    let yb = h - 0.18;
    let rings: [(f32, f32, f32); 5] = [
        (1.68, 0.08, 0.1),
        (1.85, 0.18, 0.2),
        (2.2, 0.215, 0.24),
        (2.5, 0.19, 0.19),
        (2.7, 0.07, 0.08),
    ];
    let pt = |r: (f32, f32, f32), k: u32| {
        let (s, c) = sin_cos(TAU * 0.5 * k as f32 / 4.0);
        f.p(r.0, yb + r.2 * s, r.1 * c)
    };
    for w in rings.windows(2) {
        for k in 0..4 {
            let (s, c) = sin_cos(TAU * 0.5 * (k as f32 + 0.5) / 4.0);
            let out = f.v(0.0, s, c);
            quad(
                m,
                [pt(w[0], k), pt(w[1], k), pt(w[1], k + 1), pt(w[0], k + 1)],
                out,
                metal,
            );
        }
        quad(
            m,
            [pt(w[0], 0), pt(w[1], 0), pt(w[1], 4), pt(w[0], 4)],
            -f.y,
            metal,
        );
    }
    for (r, n) in [(rings[0], -f.x), (rings[4], f.x)] {
        for k in 1..3 {
            tri(m, pt(r, 0), pt(r, k), pt(r, k + 1), n, metal);
        }
        tri(m, pt(r, 0), pt(r, 3), pt(r, 4), n, metal);
    }
    let lens = Paint::new(pal.window_warm, Pattern::Glass).glowing(pal.sodium, Glow::Lamp);
    quad(
        m,
        [
            f.p(1.85, h - 0.19, -0.14),
            f.p(2.5, h - 0.19, -0.14),
            f.p(2.5, h - 0.19, 0.14),
            f.p(1.85, h - 0.19, 0.14),
        ],
        -f.y,
        lens,
    );
}

/// Leaf colors of a canopy: `(dark underside, sunlit top)`, tinted per tree.
fn canopy_tones(pal: &Palette, rng: &mut Rng) -> ([u8; 3], [u8; 3]) {
    let tint = rng.range(0, 64);
    (
        shade(pal.leaf_green, 220 + tint),
        mix(pal.leaf_light, pal.palm_green, tint * 2),
    )
}

/// A canopy tree at full detail: a flared trunk forking into three or four limbs (two of
/// them forking again), and a lobed crown of irregular lumps over the limb ends.
fn lobed_oak(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let bark = Paint::new(pal.bark, Pattern::Bark);
    let trunk_h = rng.range_f32(2.3, 3.2);
    lathe(
        m,
        &Frame::at(base),
        &[
            [0.32, 0.0],
            [0.22, 0.3],
            [0.19, trunk_h * 0.55],
            [0.15, trunk_h],
        ],
        7,
        false,
        false,
        bark,
    );
    let top = base + V3::new(0.0, trunk_h, 0.0);
    let leaf = Paint::new(pal.leaf_green, Pattern::Foliage);
    let (dark, light) = canopy_tones(pal, rng);
    let tone = |n: V3| mix(dark, light, ((n.y * 0.5 + 0.5) * 250.0) as u32);
    let main_r = rng.range_f32(1.8, 2.3);
    let crown = top + V3::new(0.0, main_r * 0.85, 0.0);
    let radii = V3::new(main_r, main_r * 0.75, main_r);
    lump(m, &kit.sphere1, crown, radii, rng, leaf, tone);
    let r = main_r * rng.range_f32(0.55, 0.7);
    let crest = crown + V3::new(rng.range_f32(-0.5, 0.5), main_r * 0.6, 0.3);
    lump(
        m,
        &kit.sphere1,
        crest,
        V3::new(r, r * 0.8, r),
        rng,
        leaf,
        tone,
    );
    let limbs = rng.range(3, 5);
    let spin = rng.range_f32(0.0, TAU);
    for k in 0..limbs {
        let a = spin + TAU * k as f32 / limbs as f32 + rng.range_f32(-0.3, 0.3);
        let (s, c) = sin_cos(a);
        let out = V3::new(c, 0.0, s);
        let reach = rng.range_f32(1.3, 2.0);
        let tip = top + out * reach + V3::new(0.0, rng.range_f32(1.0, 1.7), 0.0);
        tube(m, top - V3::Y * 0.3, tip, 0.12, 0.06, 6, bark);
        let r = rng.range_f32(1.2, 1.6);
        let at = tip + V3::new(0.0, r * 0.3, 0.0);
        lump(
            m,
            &kit.sphere1,
            at,
            V3::new(r, r * 0.78, r),
            rng,
            leaf,
            tone,
        );
        if k < 2 {
            // A fork off the limb with a smaller lump of its own.
            let mid = top.lerp(tip, 0.5);
            let side = V3::new(-s, 0.0, c) * if k == 0 { 1.0 } else { -1.0 };
            let twig = mid + side * 0.9 + out * 0.3 + V3::new(0.0, 1.0, 0.0);
            tube(m, mid, twig, 0.06, 0.03, 5, bark);
            let r = rng.range_f32(0.9, 1.2);
            lump(
                m,
                &kit.sphere1,
                twig,
                V3::new(r, r * 0.8, r),
                rng,
                leaf,
                tone,
            );
        }
    }
}

/// A fan palm at full detail: a bent trunk banded with frond scars, a skirt of dead fronds
/// under the crown and two tiers of V-section fronds drooping toward their tips.
fn fan_palm(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let bark = shade(pal.bark, 300);
    let height = rng.range_f32(9.0, 12.5);
    let (ls, lc) = sin_cos(rng.range_f32(0.0, TAU));
    let lean = V3::new(lc, 0.0, ls) * rng.range_f32(0.4, 1.4);
    let spine = |t: f32| base + V3::new(0.0, height * t, 0.0) + lean * (t * t);
    let segments = 10;
    for k in 0..segments {
        let (t0, t1) = (k as f32 / segments as f32, (k + 1) as f32 / segments as f32);
        let (p0, p1) = (spine(t0), spine(t1));
        let (r0, r1) = (0.24 - 0.08 * t0, 0.24 - 0.08 * t1);
        let f = frame_along(p0, p1 - p0);
        let h = (p1 - p0).length();
        let tone = if k % 2 == 0 { bark } else { shade(bark, 236) };
        // A ring band at the foot of each segment.
        lathe(
            m,
            &f,
            &[[r0 * 1.12, 0.0], [r0, h * 0.2], [r1, h]],
            6,
            false,
            false,
            Paint::new(tone, Pattern::Bark),
        );
    }
    let crown = spine(1.0);
    let frond = Paint::new(pal.palm_green, Pattern::Foliage);
    icosphere(
        m,
        &kit.sphere0,
        crown,
        V3::new(0.45, 0.6, 0.45),
        false,
        frond,
        |_| shade(pal.palm_green, 160),
    );
    let spin = rng.range_f32(0.0, TAU);
    let dead = Paint::new(mix(pal.bark, pal.lawn_dry, 150), Pattern::Foliage);
    for k in 0..6 {
        let (s, c) = sin_cos(spin + TAU * (k as f32 + 0.5) / 6.0);
        let (out, side) = (V3::new(c, 0.0, s), V3::new(-s, 0.0, c));
        let a = crown + out * 0.25 - V3::Y * 0.15;
        let b = a + out * 0.3 - V3::Y * rng.range_f32(0.8, 1.1);
        let q = [
            a - side * 0.2,
            a + side * 0.2,
            b + side * 0.1,
            b - side * 0.1,
        ];
        quad(m, q, out, dead);
        quad(m, q, -out, dead.with_rgb(shade(dead.rgb(), 180)));
    }
    let count = 13;
    let widths = [0.1, 0.42, 0.55, 0.48, 0.3, 0.03];
    for k in 0..count {
        let a = spin + TAU * k as f32 / count as f32 + rng.range_f32(-0.12, 0.12);
        let (s, c) = sin_cos(a);
        let (out, side) = (V3::new(c, 0.0, s), V3::new(-s, 0.0, c));
        // Odd fronds are the young upper tier: shorter, standing higher.
        let young = k % 2 == 1;
        let lift = if young {
            rng.range_f32(0.9, 1.2)
        } else {
            rng.range_f32(0.3, 0.6)
        };
        let reach = rng.range_f32(0.9, 1.15) * if young { 0.85 } else { 1.0 };
        let mut pts = [V3::ZERO; 6];
        for (j, p) in pts.iter_mut().enumerate() {
            let t = j as f32 * 0.8;
            *p = crown + out * (t * 0.95 * reach) + V3::Y * (lift * t - 0.2 * t * t * reach);
        }
        let tone = rng.range(200, 290);
        let top_paint = frond.with_rgb(shade(pal.palm_green, tone));
        let under = frond.with_rgb(shade(pal.palm_green, tone * 3 / 5));
        for j in 0..5 {
            let (a, b) = (pts[j], pts[j + 1]);
            // The leaflets fold down from the midrib, more toward the tip.
            let (da, db) = (
                V3::Y * -(0.08 + 0.05 * j as f32),
                V3::Y * -(0.13 + 0.05 * j as f32),
            );
            let (la, lb) = (a + side * widths[j] + da, b + side * widths[j + 1] + db);
            let (ra, rb) = (a - side * widths[j] + da, b - side * widths[j + 1] + db);
            for half in [[a, b, lb, la], [a, ra, rb, b]] {
                quad(m, half, V3::Y, top_paint);
                quad(m, half, -V3::Y, under);
            }
        }
    }
}

/// A eucalyptus at full detail: a tall, leaning, pale trunk flared at the foot, four limbs
/// (two forking) and a sparse crown of tall irregular lobes with sky between them.
fn gum_tree(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let bark = Paint::new(mix(pal.bark, pal.stucco_white, 150), Pattern::Bark);
    let height = rng.range_f32(14.0, 19.0);
    let (ls, lc) = sin_cos(rng.range_f32(0.0, TAU));
    let lean = V3::new(lc, 0.0, ls) * rng.range_f32(0.6, 1.8);
    let spine = |t: f32| base + V3::new(0.0, height * t, 0.0) + lean * (t * t);
    let segments = 7;
    let r = |t: f32| 0.34 - 0.24 * t;
    for k in 0..segments {
        let (t0, t1) = (k as f32 / segments as f32, (k + 1) as f32 / segments as f32);
        let (p0, p1) = (spine(t0), spine(t1));
        let f = frame_along(p0, p1 - p0);
        let h = (p1 - p0).length();
        if k == 0 {
            let profile = [[r(t0) * 1.35, 0.0], [r(t0) * 1.05, 0.5], [r(t1), h]];
            lathe(m, &f, &profile, 7, false, false, bark);
        } else {
            frustum(m, &f, r(t0), r(t1), h, 7, false, false, bark);
        }
    }
    let leaf = Paint::new(pal.leaf_green, Pattern::Foliage);
    let tint = rng.range(0, 64);
    let dark = shade(mix(pal.leaf_green, pal.teal, 80 + tint), 190);
    let light = mix(mix(pal.leaf_light, pal.sky_blue, 70), pal.lawn_dry, tint);
    let tone = |n: V3| mix(dark, light, ((n.y * 0.5 + 0.5) * 240.0) as u32);
    let lobes = rng.range(7, 10);
    let spin = rng.range_f32(0.0, TAU);
    for k in 0..lobes {
        let t = if k == 0 {
            1.0
        } else {
            0.5 + 0.45 * (k as f32 / lobes as f32)
        };
        let (s, c) = sin_cos(spin + TAU * 0.38 * k as f32);
        let reach = if k == 0 {
            0.4
        } else {
            rng.range_f32(1.6, 3.2) * (1.15 - 0.5 * t)
        };
        let at = spine(t) + V3::new(c * reach, rng.range_f32(0.2, 1.2), s * reach);
        if (1..=4).contains(&k) {
            // A limb out to the lobe; the first two fork.
            let from = spine(t - 0.12);
            tube(m, from, at, 0.1, 0.04, 6, bark);
            if k <= 2 {
                let mid = from.lerp(at, 0.55);
                let twig = mid + V3::new(-s * 1.1, 1.3, c * 1.1);
                tube(m, mid, twig, 0.05, 0.02, 5, bark);
            }
        }
        let rad = rng.range_f32(1.3, 2.0);
        let tall = rng.range_f32(1.1, 1.5);
        let radii = V3::new(rad, rad * tall, rad);
        lump(m, &kit.sphere1, at, radii, rng, leaf, tone);
    }
}

/// A hydrant at full detail: a flanged barrel and domed bonnet with an operating nut, two
/// hose outlets and a pumper outlet with caps, the side caps on chains.
fn full_hydrant(m: &mut WorldMesh, base: V3, dir: V2, pal: &Palette) {
    let red = Paint::new(pal.hydrant_red, Pattern::Metal);
    let chrome = Paint::new(pal.metal_light, Pattern::Metal);
    let f = Frame::upright(base, dir);
    lathe(
        m,
        &f,
        &[
            [0.17, 0.0],
            [0.17, 0.05],
            [0.125, 0.08],
            [0.11, 0.14],
            [0.11, 0.5],
            [0.135, 0.52],
            [0.135, 0.56],
            [0.115, 0.58],
            [0.07, 0.68],
            [0.0, 0.71],
        ],
        8,
        false,
        false,
        red,
    );
    let nut = f.offset(0.0, 0.67, 0.0);
    frustum(m, &nut, 0.035, 0.03, 0.07, 5, true, false, chrome);
    // Outlets: the two hose outlets across, the pumper toward the street.
    let outlet = |m: &mut WorldMesh, axis: V3, y: f32, r: f32| {
        let o = frame_along(f.p(0.0, y, 0.0), axis);
        let profile = [
            [r, 0.06],
            [r, 0.13],
            [r * 1.3, 0.14],
            [r * 1.3, 0.2],
            [r * 0.5, 0.21],
        ];
        lathe(m, &o, &profile[..3], 6, false, false, red);
        lathe(m, &o, &profile[2..], 6, true, false, chrome);
    };
    outlet(m, f.z, 0.38, 0.04);
    outlet(m, -f.z, 0.38, 0.04);
    outlet(m, f.x, 0.34, 0.06);
    // A chain from each side cap down and back up to the barrel.
    let link = Paint::new(shade(pal.metal_dark, 300), Pattern::Metal);
    for side in [1.0f32, -1.0] {
        let p = [
            f.p(0.03, 0.34, 0.19 * side),
            f.p(0.06, 0.27, 0.16 * side),
            f.p(0.07, 0.27, 0.125 * side),
            f.p(0.06, 0.45, 0.108 * side),
        ];
        for k in 0..3 {
            bar(m, p[k], p[k + 1], f.x, 0.014, 0.014, false, link);
        }
    }
}

/// Wood and iron paints of a bench, the slats in one of a few stains.
fn bench_paints(pal: &Palette, rng: &mut Rng) -> (Paint, Paint) {
    let stains = [
        shade(pal.terracotta, 190),
        shade(pal.bark, 330),
        mix(pal.bark, pal.sand, 140),
        shade(pal.palm_green, 150),
    ];
    let wood = Paint::new(stains[rng.range(0, 4) as usize], Pattern::Plain);
    (wood, Paint::new(pal.metal_dark, Pattern::Metal))
}

/// A slatted bench with iron end frames and armrests in frame `f` (X along the seat, +Z
/// the way it faces).
fn park_bench(m: &mut WorldMesh, f: &Frame, wood: Paint, iron: Paint) {
    for z0 in [0.15, 0.035, -0.08, -0.195] {
        boxf(
            m,
            f,
            [-0.95, 0.42, z0],
            [0.95, 0.455, z0 + 0.09],
            face::ALL,
            wood,
        );
    }
    // The back's slats, leaning back.
    let tilt = f.y - f.z * 0.3;
    for j in 0..3 {
        let (y, z) = (0.56 + 0.13 * j as f32, -0.27 - 0.04 * j as f32);
        bar(
            m,
            f.p(-0.95, y, z),
            f.p(0.95, y, z),
            tilt,
            0.1,
            0.03,
            true,
            wood,
        );
    }
    for s in [-1.0f32, 1.0] {
        let x = 0.82 * s;
        let a = 0.92 * s;
        let bars = [
            (f.p(x, 0.0, 0.2), f.p(x, 0.42, 0.18)),
            (f.p(x, 0.0, -0.2), f.p(x, 0.42, -0.22)),
            (f.p(x, 0.42, -0.22), f.p(x, 0.92, -0.37)),
            (f.p(x, 0.4, 0.24), f.p(x, 0.4, -0.24)),
            (f.p(a, 0.42, 0.2), f.p(a, 0.64, 0.2)),
            (f.p(a, 0.66, 0.27), f.p(a, 0.68, -0.28)),
        ];
        for (k, (p, q)) in bars.into_iter().enumerate() {
            let across = if k == 3 { f.y } else { f.x };
            let (w, t) = if k == 5 { (0.07, 0.03) } else { (0.05, 0.045) };
            bar(m, p, q, across, w, t, true, iron);
        }
    }
}

/// A bench at a bus stop: the bench and, at one end, a route-sign post with a blank sign
/// and a timetable case.
fn bus_stop(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette, rng: &mut Rng) {
    let f = Frame::upright(base, facing.left());
    let (wood, iron) = bench_paints(pal, rng);
    park_bench(m, &f, wood, iron);
    let steel = Paint::new(pal.metal_light, Pattern::Metal);
    let pole = f.offset(1.35, 0.0, 0.0);
    frustum(m, &pole, 0.035, 0.035, 2.6, 6, true, false, steel);
    let colors = [
        pal.teal,
        pal.navy,
        shade(pal.palm_green, 170),
        pal.hydrant_red,
    ];
    let band = Paint::new(colors[rng.range(0, 4) as usize], Pattern::Paint);
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let (x0, x1, y0, y1) = (1.12, 1.58, 2.0, 2.55);
    boxf(m, &f, [x0, y0, 0.04], [x1, y1, 0.065], face::ALL, white);
    for (z, back) in [(0.066, false), (0.039, true)] {
        rect(m, &f, x0, y1 - 0.16, x1, y1, z, back, band);
        // The route as a block of color on the white field.
        rect(m, &f, 1.25, 2.12, 1.45, 2.3, z, back, band);
    }
    boxf(
        m,
        &f,
        [1.25, 1.3, -0.05],
        [1.45, 1.75, 0.07],
        face::ALL,
        steel,
    );
    let sheet = Paint::new(pal.stucco_cream, Pattern::Plain);
    rect(m, &f, 1.28, 1.34, 1.42, 1.7, 0.071, false, sheet);
}

/// A bench under a small shelter: four posts, a glazed back and one glazed end, and a flat
/// roof.
fn bench_shelter(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette, rng: &mut Rng) {
    let f = Frame::upright(base, facing.left());
    let (wood, iron) = bench_paints(pal, rng);
    park_bench(m, &f, wood, iron);
    let frame = Paint::new(shade(pal.metal_dark, 330), Pattern::Metal);
    let glass = Paint::new(mix(pal.glass, pal.sky_blue, 90), Pattern::Glass);
    let (xh, zb, zf, top) = (1.3, -0.5, 0.6, 2.3);
    for (x, z) in [(-xh, zb), (xh, zb), (-xh, zf), (xh, zf)] {
        let (lo, hi) = ([x - 0.04, 0.0, z - 0.04], [x + 0.04, top, z + 0.04]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, frame);
    }
    let roof = [-xh - 0.15, top, zb - 0.15];
    boxf(
        m,
        &f,
        roof,
        [xh + 0.15, top + 0.1, zf + 0.2],
        face::ALL,
        frame,
    );
    for y in [0.2, 2.1] {
        let (lo, hi) = ([-xh, y, zb - 0.03], [xh, y + 0.05, zb + 0.03]);
        boxf(m, &f, lo, hi, face::ALL, frame);
    }
    rect(m, &f, -xh, 0.25, xh, 2.1, zb, false, glass);
    rect(m, &f, -xh, 0.25, xh, 2.1, zb, true, glass);
    let end = Frame {
        o: f.p(-xh, 0.0, 0.0),
        x: f.z,
        y: f.y,
        z: -f.x,
    };
    rect(m, &end, zb, 0.25, zf - 0.2, 2.1, 0.0, false, glass);
    rect(m, &end, zb, 0.25, zf - 0.2, 2.1, 0.0, true, glass);
}

/// A steel trash can with hoops round its body and a domed lid with a knob.
fn steel_can(m: &mut WorldMesh, base: V3, pal: &Palette, rng: &mut Rng) {
    let tones = [
        shade(pal.teal, 170),
        shade(pal.palm_green, 150),
        shade(pal.navy, 200),
        shade(pal.metal_dark, 300),
    ];
    let body = Paint::new(tones[rng.range(0, 4) as usize], Pattern::Metal);
    let lid = Paint::new(pal.metal_dark, Pattern::Metal);
    let f = Frame::at(base);
    let mut profile = Vec::new();
    profile.extend_from_slice(&[[0.26, 0.0], [0.27, 0.04]]);
    for y in [0.16, 0.5, 0.82] {
        profile.extend_from_slice(&[[0.27, y], [0.29, y + 0.02], [0.29, y + 0.06]]);
    }
    profile.push([0.27, 0.9]);
    lathe(m, &f, &profile, 10, false, false, body);
    let dome = [
        [0.31, 0.9],
        [0.31, 0.94],
        [0.25, 1.0],
        [0.12, 1.04],
        [0.0, 1.05],
    ];
    lathe(m, &f, &dome, 10, false, false, lid);
    frustum(
        m,
        &f.offset(0.0, 1.04, 0.0),
        0.04,
        0.035,
        0.05,
        6,
        true,
        false,
        lid,
    );
}

/// A wire-basket trash can: hoops and uprights round a dark liner.
fn wire_can(m: &mut WorldMesh, base: V3, pal: &Palette) {
    let wire = Paint::new(shade(pal.metal_dark, 330), Pattern::Metal);
    let liner = Paint::new(shade(pal.metal_dark, 140), Pattern::Plain);
    let f = Frame::at(base);
    frustum(m, &f, 0.25, 0.25, 0.06, 12, false, false, wire);
    lathe(
        m,
        &f,
        &[[0.22, 0.04], [0.25, 0.84], [0.18, 0.86], [0.0, 0.87]],
        10,
        false,
        false,
        liner,
    );
    let r = |y: f32| 0.26 + 0.04 * y;
    for y in [0.3, 0.6, 0.86] {
        let band = [[r(y), y], [r(y + 0.04), y + 0.04]];
        lathe(m, &f, &band, 12, false, false, wire);
    }
    for k in 0..12 {
        let (s, c) = sin_cos(TAU * (k as f32 + 0.5) / 12.0);
        let (lo, hi) = (V3::new(c, 0.0, s) * r(0.05), V3::new(c, 0.0, s) * r(0.86));
        let a = base + lo + V3::Y * 0.05;
        let b = base + hi + V3::Y * 0.86;
        bar(m, a, b, V3::new(-s, 0.0, c), 0.015, 0.015, false, wire);
    }
}

/// A pair of newspaper vending boxes on pedestals, their windows to the sidewalk.
fn news_boxes(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette, rng: &mut Rng) {
    let f = Frame::upright(base, facing.left());
    let colors = [
        pal.paint_yellow,
        pal.hydrant_red,
        pal.navy,
        pal.teal,
        pal.stucco_white,
        shade(pal.palm_green, 170),
    ];
    let first = rng.range(0, 6);
    let second = (first + rng.range(1, 6)) % 6;
    let steel = Paint::new(pal.metal_dark, Pattern::Metal);
    let glass = Paint::new(mix(pal.glass, pal.stucco_cream, 100), Pattern::Glass);
    let paper = Paint::new(pal.stucco_cream, Pattern::Plain);
    for (k, c) in [first, second].into_iter().enumerate() {
        let body = Paint::new(colors[c as usize], Pattern::Paint);
        let x = -0.29 + 0.58 * k as f32;
        boxf(
            m,
            &f,
            [x - 0.2, 0.0, -0.2],
            [x + 0.2, 0.05, 0.2],
            face::NO_BOTTOM,
            steel,
        );
        let (lo, hi) = ([x - 0.04, 0.05, -0.04], [x + 0.04, 0.35, 0.04]);
        boxf(m, &f, lo, hi, face::NO_BOTTOM, steel);
        let (lo, hi) = ([x - 0.24, 0.35, -0.2], [x + 0.24, 1.05, 0.2]);
        boxf(m, &f, lo, hi, face::ALL, body);
        let (lo, hi) = ([x - 0.25, 1.05, -0.22], [x + 0.25, 1.1, 0.21]);
        boxf(
            m,
            &f,
            lo,
            hi,
            face::ALL,
            body.with_rgb(shade(body.rgb(), 200)),
        );
        // The front (-Z): a window on a folded paper, the coin box and the pull handle.
        rect(m, &f, x - 0.15, 0.7, x + 0.15, 0.95, -0.201, true, paper);
        rect(m, &f, x - 0.18, 0.62, x + 0.18, 0.98, -0.202, true, glass);
        let (lo, hi) = ([x + 0.08, 0.46, -0.24], [x + 0.2, 0.58, -0.2]);
        boxf(m, &f, lo, hi, face::ALL & !face::POS_Z, steel);
        let (lo, hi) = ([x - 0.13, 0.53, -0.23], [x + 0.02, 0.56, -0.2]);
        boxf(m, &f, lo, hi, face::ALL & !face::POS_Z, steel);
    }
}

/// A sidewalk planter: a lipped square tub with a shrub and a few flowers.
fn planter(m: &mut WorldMesh, base: V3, pal: &Palette, kit: &PropKit, rng: &mut Rng) {
    let f = Frame::at(base);
    let tub = if rng.range(0, 2) == 0 {
        Paint::new(pal.terracotta, Pattern::Plain)
    } else {
        Paint::new(mix(pal.concrete, pal.sand, 90), Pattern::Concrete)
    };
    let (w, h) = (0.42, 0.5);
    boxf(m, &f, [-w, 0.0, -w], [w, h, w], face::NO_BOTTOM, tub);
    let lip = tub.with_rgb(shade(tub.rgb(), 230));
    let (lo, hi) = ([-w - 0.04, h, -w - 0.04], [w + 0.04, h + 0.06, w + 0.04]);
    boxf(m, &f, lo, hi, face::ALL, lip);
    let soil = Paint::new(pal.soil, Pattern::Plain);
    rect(
        m,
        &Frame {
            o: base,
            x: V3::X,
            y: -V3::Z,
            z: V3::Y,
        },
        -w + 0.03,
        -w + 0.03,
        w - 0.03,
        w - 0.03,
        h + 0.061,
        false,
        soil,
    );
    let leaf = Paint::new(pal.leaf_green, Pattern::Foliage);
    let (dark, light) = canopy_tones(pal, rng);
    let tone = |n: V3| mix(dark, light, ((n.y * 0.5 + 0.5) * 250.0) as u32);
    let blooms = [pal.pastel_pink, pal.salmon, pal.paint_yellow, pal.lavender];
    let bloom = Paint::new(blooms[rng.range(0, 4) as usize], Pattern::Foliage);
    let spin = rng.range_f32(0.0, TAU);
    for k in 0..3 {
        let (s, c) = sin_cos(spin + TAU * k as f32 / 3.0);
        let (out, side) = (V3::new(c, 0.0, s), V3::new(-s, 0.0, c));
        let r = rng.range_f32(0.24, 0.3);
        let radii = V3::new(r, r * 0.85, r);
        let at = base + out * 0.13 + V3::Y * (h + 0.22 + 0.08 * k as f32);
        lump(m, &kit.sphere1, at, radii, rng, leaf, tone);
        // Two blooms on the lump's sunny shoulder.
        for t in [-0.45, 0.45] {
            let d = (out * 0.7 + V3::Y * 0.7 + side * t).normalize();
            let p = at + V3::new(d.x * radii.x, d.y * radii.y, d.z * radii.z);
            let r = V3::new(0.07, 0.06, 0.07);
            icosphere(m, &kit.sphere0, p, r, false, bloom, |_| bloom.rgb());
        }
    }
}

/// A tunnel-top mailbox body in frame `f`: flat floor at `y0`, upright sides `half` either
/// side of `x`, a half-round roof, from `z0` (back) to `z1` (the door).
#[allow(clippy::too_many_arguments)]
fn tunnel_box(
    m: &mut WorldMesh,
    f: &Frame,
    x: f32,
    y0: f32,
    half: f32,
    z0: f32,
    z1: f32,
    paint: Paint,
) {
    let wall = y0 + half * 0.9;
    let mut ring = [(0.0f32, 0.0f32); 9];
    ring[0] = (x - half, y0);
    for k in 0..7 {
        let (s, c) = sin_cos(TAU * 0.5 * (1.0 - k as f32 / 6.0));
        ring[k + 1] = (x + half * c, wall + half * s);
    }
    ring[8] = (x + half, y0);
    let center = (x, (y0 + wall) * 0.5);
    for k in 0..9 {
        let (a, b) = (ring[k], ring[(k + 1) % 9]);
        let out = f.v(
            (a.0 + b.0) * 0.5 - center.0,
            (a.1 + b.1) * 0.5 - center.1,
            0.0,
        );
        let q = [
            f.p(a.0, a.1, z0),
            f.p(b.0, b.1, z0),
            f.p(b.0, b.1, z1),
            f.p(a.0, a.1, z1),
        ];
        quad(m, q, out, paint);
        let door = paint.with_rgb(shade(paint.rgb(), 235));
        let c = |z: f32| f.p(center.0, center.1, z);
        tri(m, c(z1), f.p(a.0, a.1, z1), f.p(b.0, b.1, z1), f.z, door);
        tri(m, c(z0), f.p(a.0, a.1, z0), f.p(b.0, b.1, z0), -f.z, paint);
    }
}

/// A curbside mailbox at full detail, one of four shapes: on a wooden post, on a curved
/// steel post, a pair on a crossbar, or set into a stucco pillar.
fn full_mailbox(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette, rng: &mut Rng) {
    let wood = Paint::new(shade(pal.bark, 280), Pattern::Plain);
    let tones = [
        pal.metal_light,
        pal.stucco_white,
        pal.navy,
        shade(pal.metal_dark, 300),
    ];
    let tin = Paint::new(tones[rng.range(0, 4) as usize], Pattern::Metal);
    let steel = Paint::new(shade(pal.metal_dark, 260), Pattern::Metal);
    let flag = Paint::new(pal.hydrant_red, Pattern::Paint);
    // +Z toward the street.
    let f = Frame::upright(base, facing.left());
    // The box at `x` on a floor at `y`, with its door latch and its flag.
    let mailbox = |m: &mut WorldMesh, x: f32, y: f32| {
        tunnel_box(m, &f, x, y, 0.12, -0.26, 0.24, tin);
        let (lo, hi) = ([x - 0.02, y + 0.1, 0.24], [x + 0.02, y + 0.14, 0.27]);
        boxf(m, &f, lo, hi, face::ALL & !face::NEG_Z, steel);
        let (lo, hi) = ([x + 0.12, y + 0.06, -0.13], [x + 0.135, y + 0.36, -0.1]);
        boxf(m, &f, lo, hi, face::ALL & !face::NEG_X, flag);
        let (lo, hi) = ([x + 0.12, y + 0.26, -0.24], [x + 0.13, y + 0.36, -0.13]);
        boxf(m, &f, lo, hi, face::ALL & !face::NEG_X, flag);
    };
    match rng.range(0, 4) {
        0 => {
            boxf(
                m,
                &f,
                [-0.05, 0.0, -0.05],
                [0.05, 0.98, 0.05],
                face::NO_BOTTOM,
                wood,
            );
            let (lo, hi) = ([-0.1, 0.98, -0.28], [0.1, 1.02, 0.26]);
            boxf(m, &f, lo, hi, face::ALL, wood);
            mailbox(m, 0.0, 1.02);
        }
        1 => {
            let foot = f.offset(0.0, 0.0, -0.32);
            frustum(m, &foot, 0.09, 0.07, 0.04, 6, true, false, steel);
            // A swan-neck post rising behind the box and bending over to carry it.
            let mut pts = [V3::ZERO; 5];
            pts[0] = f.p(0.0, 0.0, -0.32);
            for (k, p) in pts.iter_mut().enumerate().skip(1) {
                let (s, c) = sin_cos(TAU * 0.25 * (k - 1) as f32 / 3.0);
                *p = f.p(0.0, 0.8 + 0.2 * s, -0.32 + 0.2 * (1.0 - c));
            }
            for k in 0..4 {
                tube(m, pts[k], pts[k + 1], 0.035, 0.035, 6, steel);
            }
            let (lo, hi) = ([-0.06, 0.98, -0.2], [0.06, 1.02, 0.1]);
            boxf(m, &f, lo, hi, face::ALL, steel);
            mailbox(m, 0.0, 1.02);
        }
        2 => {
            boxf(
                m,
                &f,
                [-0.05, 0.0, -0.05],
                [0.05, 0.98, 0.05],
                face::NO_BOTTOM,
                wood,
            );
            let (lo, hi) = ([-0.36, 0.98, -0.12], [0.36, 1.02, 0.12]);
            boxf(m, &f, lo, hi, face::ALL, wood);
            mailbox(m, -0.18, 1.02);
            mailbox(m, 0.18, 1.02);
        }
        _ => {
            let tones = [pal.stucco_white, pal.stucco_cream, pal.sand, pal.peach];
            let pillar = Paint::new(tones[rng.range(0, 4) as usize], Pattern::Stucco);
            let (lo, hi) = ([-0.24, 0.0, -0.3], [0.24, 1.2, 0.2]);
            boxf(m, &f, lo, hi, face::NO_BOTTOM, pillar);
            let cap = pillar.with_rgb(shade(pillar.rgb(), 225));
            let (lo, hi) = ([-0.28, 1.2, -0.34], [0.28, 1.28, 0.24]);
            boxf(m, &f, lo, hi, face::ALL, cap);
            // The box set into the pillar, its door standing proud of the face.
            tunnel_box(m, &f, 0.0, 0.82, 0.12, 0.1, 0.23, tin);
            let plaque = Paint::new(shade(pal.metal_dark, 200), Pattern::Metal);
            rect(m, &f, -0.12, 0.45, 0.12, 0.6, 0.201, false, plaque);
        }
    }
}

/// A pin insulator on a crossarm: a stem under stacked skirts.
fn insulator(m: &mut WorldMesh, f: &Frame, paint: Paint) {
    let profile = [
        [0.015, 0.0],
        [0.015, 0.05],
        [0.06, 0.07],
        [0.035, 0.1],
        [0.055, 0.13],
        [0.03, 0.16],
        [0.0, 0.17],
    ];
    lathe(m, f, &profile, 6, false, false, paint);
}

/// A utility pole at full detail: a tapered pole, one or two braced crossarms of pin
/// insulators and now and then a transformer with its bushings.
fn full_power_pole(m: &mut WorldMesh, base: V3, along: V2, pal: &Palette, rng: &mut Rng) {
    let wood = Paint::new(shade(pal.bark, 240), Pattern::Bark);
    let f = Frame::upright(base, along);
    lathe(
        m,
        &f,
        &[[0.16, 0.0], [0.14, 5.0], [0.11, 10.4], [0.0, 10.5]],
        8,
        false,
        false,
        wood,
    );
    let glass = Paint::new(mix(pal.mint, pal.concrete, 120), Pattern::Plain);
    let steel = Paint::new(shade(pal.metal_dark, 280), Pattern::Metal);
    let arms = 1 + rng.range(0, 2);
    for (k, y) in [9.45f32, 8.6].into_iter().take(arms as usize).enumerate() {
        let half = if k == 0 { 1.3 } else { 0.95 };
        let (lo, hi) = ([-0.06, y, -half], [0.06, y + 0.12, half]);
        boxf(m, &f, lo, hi, face::ALL, wood);
        for s in [-1.0f32, 1.0] {
            let (a, b) = (f.p(0.07, y - 0.6, 0.0), f.p(0.07, y, 0.65 * s));
            bar(m, a, b, f.z, 0.04, 0.012, true, steel);
        }
        let pins: &[f32] = if k == 0 {
            &[-1.15, -0.45, 0.45, 1.15]
        } else {
            &[-0.8, 0.8]
        };
        for &z in pins {
            insulator(m, &f.offset(0.0, y + 0.12, z), glass);
        }
    }
    if rng.range(0, 3) == 0 {
        let can = Paint::new(shade(pal.concrete, 200), Pattern::Metal);
        let (lo, hi) = ([-0.05, 7.4, 0.12], [0.05, 7.9, 0.2]);
        boxf(m, &f, lo, hi, face::ALL, steel);
        let at = f.offset(0.0, 7.2, 0.45);
        frustum(m, &at, 0.26, 0.26, 0.85, 8, true, false, can);
        let lid = [[0.28, 0.0], [0.28, 0.04], [0.0, 0.1]];
        lathe(m, &at.offset(0.0, 0.85, 0.0), &lid, 8, false, false, can);
        for x in [-0.12, 0.12] {
            insulator(m, &at.offset(x, 0.9, 0.0), glass);
        }
    }
}

/// A stop sign at full detail: a square post in a ground sleeve, an octagon plate with a
/// white border, an edge and a back, its mounting bolts and bracket, and the street-name
/// blades on top.
fn full_stop_sign(m: &mut WorldMesh, base: V3, facing: V2, pal: &Palette) {
    let post = Paint::new(pal.metal_light, Pattern::Metal);
    let f = Frame::upright(base, facing.left());
    let sleeve = Paint::new(pal.metal_dark, Pattern::Metal);
    let (lo, hi) = ([-0.05, 0.0, -0.05], [0.05, 0.12, 0.05]);
    boxf(m, &f, lo, hi, face::NO_BOTTOM, sleeve);
    let (lo, hi) = ([-0.03, 0.0, -0.03], [0.03, 2.35, 0.03]);
    boxf(m, &f, lo, hi, face::NO_BOTTOM, post);
    let (lo, hi) = ([-0.06, 1.8, 0.03], [0.06, 2.3, 0.04]);
    boxf(m, &f, lo, hi, face::ALL & !face::NEG_Z, post);
    let red = Paint::new(pal.hydrant_red, Pattern::Paint);
    let white = Paint::new(pal.paint_white, Pattern::Paint);
    let center = f.p(0.0, 2.05, 0.0);
    let oct = |r: f32, z: f32| -> [V3; 8] {
        let mut out = [V3::ZERO; 8];
        for (k, p) in out.iter_mut().enumerate() {
            let (s, c) = sin_cos(TAU * (k as f32 + 0.5) / 8.0);
            *p = center + f.x * (c * r) + f.y * (s * r) + f.z * z;
        }
        out
    };
    let (front, inner, back) = (oct(0.38, 0.05), oct(0.33, 0.052), oct(0.38, 0.04));
    for k in 1..7 {
        tri(m, front[0], front[k], front[k + 1], f.z, white);
        tri(m, inner[0], inner[k], inner[k + 1], f.z, red);
        tri(m, back[0], back[k], back[k + 1], -f.z, post);
    }
    for k in 0..8 {
        let j = (k + 1) % 8;
        let out = (front[k] + front[j]) * 0.5 - center - f.z * 0.05;
        quad(m, [back[k], back[j], front[j], front[k]], out, post);
    }
    for y in [1.8, 2.3] {
        let (lo, hi) = ([-0.015, y - 0.015, 0.052], [0.015, y + 0.015, 0.06]);
        boxf(m, &f, lo, hi, face::ALL & !face::NEG_Z, post);
    }
    street_blades(m, &f, 2.62, pal);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_one(kind: PropKind, seed: u32, kit: &PropKit) -> WorldMesh {
        let mut m = WorldMesh::default();
        let p = Prop {
            pos: V2::new(3.0, 4.0),
            y: CURB_M,
            dir: V2::new(0.6, 0.8),
            kind,
            seed,
        };
        build(&mut m, &p, &Palette::socal_1985(), kit);
        assert!(m
            .vertices
            .iter()
            .all(|v| v.normal.iter().chain(&v.pos).all(|c| c.is_finite())));
        m
    }

    fn tris(kind: PropKind) -> usize {
        build_one(kind, 42, &PropKit::new()).triangle_count()
    }

    #[test]
    fn prop_triangle_counts() {
        let oak = tris(PropKind::Oak);
        assert!((300..=800).contains(&oak), "oak {oak}");
        let palm = tris(PropKind::PalmTree);
        assert!((300..=800).contains(&palm), "palm {palm}");
        let car = tris(PropKind::ParkedCar { color: 1 });
        assert!((50..=70).contains(&car), "car {car}");
        for kind in [
            PropKind::StreetLight,
            PropKind::Hydrant,
            PropKind::Bench,
            PropKind::StopSign,
            PropKind::TrafficSignal { arm_dm: 60 },
            PropKind::Fountain,
            PropKind::TrashCan,
        ] {
            let n = tris(kind);
            assert!((20..=400).contains(&n), "{kind:?} {n}");
        }
        // The instanced shapes at full detail (`docs/backlog/ART.md` budgets), over enough
        // seeds to meet every design.
        let kit = PropKit::instanced();
        for seed in 0..48 {
            for kind in [
                PropKind::Oak,
                PropKind::PalmTree,
                PropKind::Eucalyptus,
                PropKind::StreetLight,
                PropKind::Hydrant,
                PropKind::Bench,
                PropKind::TrashCan,
                PropKind::Mailbox,
                PropKind::PowerPole,
                PropKind::StopSign,
            ] {
                let n = build_one(kind, seed, &kit).triangle_count();
                let budget = match kind {
                    PropKind::Oak | PropKind::PalmTree | PropKind::Eucalyptus => 600..=1200,
                    PropKind::StreetLight => 100..=250,
                    // Small street items (mailboxes, signs, hydrants) may be under 300.
                    _ => 50..=2500,
                };
                assert!(budget.contains(&n), "{kind:?} seed {seed}: {n}");
            }
        }
    }

    /// Every shop sign the coast grammar draws keeps to the sign budget, letters its words
    /// when the triangles allow, and glows only where it is neon.
    #[test]
    fn shop_signs_keep_the_sign_budget() {
        let g = crate::grammar::coast_80s();
        let pal = Palette::socal_1985();
        let mut worded = 0;
        let n = 3000u64;
        for h in 0..n {
            let s = ShopSign::draw(&g.signs, &g.lexicon, g.era, mix64(h));
            let p = Prop {
                pos: V2::new(3.0, 4.0),
                y: CURB_M,
                dir: V2::new(0.6, 0.8),
                kind: PropKind::NeonSign {
                    palette: (h % 5) as u8,
                },
                seed: h as u32,
            };
            let mut m = WorldMesh::default();
            let base = p.pos.at(p.y);
            let lettered = neon_sign(&mut m, base, p.dir, 1, &pal, p.seed, Some((&s, &g.lexicon)));
            let tris = m.triangle_count();
            assert!(
                tris <= BLADE_TRIANGLES,
                "{:?}: {tris}",
                core::str::from_utf8(&s.text(&g.lexicon))
            );
            if lettered {
                worded += 1;
            }
            assert!(m.vertices.iter().all(|v| v
                .pos
                .iter()
                .chain(&v.normal)
                .all(|c| c.is_finite())));
            assert!(m.vertices.iter().any(|v| v.emissive[3] == Glow::Neon as u8));
        }
        // Every sign has room for its word.
        assert_eq!(worded, n, "{worded} of {n} signs lettered");
    }

    /// The full-detail kit changes only the instanced kinds, and a street light's lens (the
    /// photocell's and the lamp heads' key) not at all.
    #[test]
    fn full_detail_keeps_the_lens_and_the_baked_kinds() {
        let (light, fine) = (PropKit::new(), PropKit::instanced());
        let lens = |m: &WorldMesh| -> Vec<[u32; 3]> {
            m.vertices
                .iter()
                .filter(|v| v.emissive[3] == Glow::Lamp as u8)
                .map(|v| v.pos.map(f32::to_bits))
                .collect()
        };
        let a = build_one(PropKind::StreetLight, 7, &light);
        let b = build_one(PropKind::StreetLight, 7, &fine);
        assert_eq!(lens(&a).len(), 4);
        assert_eq!(lens(&a), lens(&b));
        for kind in [
            PropKind::ParkedCar { color: 3 },
            PropKind::TrafficSignal { arm_dm: 60 },
            PropKind::Fountain,
            PropKind::Billboard,
            PropKind::Hedge { len_dm: 60 },
            PropKind::Fence { len_dm: 60 },
            PropKind::StreetSign,
            PropKind::NeonSign { palette: 1 },
            PropKind::Pier { len_m: 30 },
        ] {
            let (a, b) = (build_one(kind, 9, &light), build_one(kind, 9, &fine));
            assert_eq!(a.indices, b.indices, "{kind:?}");
            assert!(a.vertices == b.vertices, "{kind:?}");
        }
    }
}
