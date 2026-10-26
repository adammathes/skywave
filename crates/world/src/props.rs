//! Street furniture and plants: where they go (a deterministic detailing pass producing
//! [`Prop`] records, as in `docs/PLACES.md`) and their low-poly meshes.

use alloc::vec::Vec;

use place::{Place, RoadClass, ZoneId};
use skywave_core::{Rng, Seed};

use crate::math::{point_in_polygon, sin_cos, TAU, V2, V3};
use crate::mesh::{
    boxf, face, frustum, icosphere, lathe, polygon_cap, quad, tri, Frame, Glow, Paint, Pattern,
    UnitSphere, WorldMesh,
};
use crate::palette::{mix, shade, Palette};
use crate::roads::Network;
use crate::{CURB_M, LOT_Y};

const KEY_STREET: u64 = 0x5354_5245;
const KEY_PARK: u64 = 0x5041_524B;
const KEY_PROP: u64 = 0x5052_4F50;

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

/// Shared geometry for building many props.
pub struct PropKit {
    sphere0: UnitSphere,
    sphere1: UnitSphere,
}

impl PropKit {
    pub fn new() -> PropKit {
        PropKit {
            sphere0: UnitSphere::new(0),
            sphere1: UnitSphere::new(1),
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

/// Signal pole with a mast arm over the approach and two lit heads facing traffic.
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
    let red = Paint::new(pal.neon_red, Pattern::Glass).glowing(pal.neon_red, Glow::Signal);
    let off = Paint::new(shade(pal.metal_dark, 140), Pattern::Glass);
    for t in [0.55, 0.92] {
        let x = arm_len * t;
        boxf(
            m,
            &f,
            [x - 0.17, 4.85, -0.15],
            [x + 0.17, 5.85, 0.15],
            face::ALL & !face::POS_Y,
            housing,
        );
        for (k, lit) in [(0, true), (1, false), (2, false)] {
            let y = 5.6 - k as f32 * 0.31;
            let paint = if lit { red } else { off };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tris(kind: PropKind) -> usize {
        let mut m = WorldMesh::default();
        let p = Prop {
            pos: V2::new(3.0, 4.0),
            y: CURB_M,
            dir: V2::new(0.6, 0.8),
            kind,
            seed: 42,
        };
        build(&mut m, &p, &Palette::socal_1985(), &PropKit::new());
        assert!(m
            .vertices
            .iter()
            .all(|v| v.normal.iter().all(|c| c.is_finite())));
        m.triangle_count()
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
    }
}
