//! One wheel: tire with a sidewall profile plus a rim with real spoke geometry.
//!
//! Local frame: origin at the hub center, axle along `+x` with the outer face toward `+x`
//! (a right-side wheel). Left wheels are the same mesh turned 180° about `y`.

use super::mesh::{Mat, Soup};
use super::params::{Parts, Rims, Wheels};
use super::Materials;
use crate::math::*;

const AXIS: V3 = [1.0, 0.0, 0.0];

/// Rim radius for a tire of outer radius `r` on `rims`: 80s tires had tall sidewalls, an
/// aftermarket rim wears a low-profile tire.
pub fn rim_radius(r: f32, rims: Rims) -> f32 {
    match rims {
        Rims::Twin => r * 0.73,
        _ => r * 0.62,
    }
}

/// A wheel of a fixed axle ([`super::ShapeSpec::axles`]), hub-local and right-side like
/// [`build`]'s: a tire's tread and outer wall and a flat rim face, twelve-sided (eight at
/// level of detail 2). A truck's tandems are many wheels seen from afar.
pub fn fixed(w: &Wheels, parts: &Parts, lod: u8, m: &Materials) -> Soup {
    let mut s = Soup::new(40.0);
    let hw = w.width * 0.5;
    let segs = if lod > 1 { 8 } else { 12 };
    s.cylinder(
        [-hw, 0.0, 0.0],
        [hw, 0.0, 0.0],
        w.radius,
        segs,
        false,
        false,
        m.tire,
    );
    let rr = rim_radius(w.radius, parts.rims);
    s.disc([hw, 0.0, 0.0], AXIS, w.radius, segs, m.tire);
    s.disc([hw + 0.002, 0.0, 0.0], AXIS, rr, segs, m.rim);
    s
}

pub fn build(w: &Wheels, parts: &Parts, lod: u8, m: &Materials) -> Soup {
    let (rims, hubcaps) = (parts.rims, parts.hubcaps);
    let mut s = Soup::new(40.0);
    let r = w.radius;
    let hw = w.width * 0.5;
    let rr = rim_radius(r, rims);
    let o = [0.0; 3];
    let mid = (r + rr) * 0.5;
    let white = m.whitewall;
    if lod > 0 {
        let prof = [
            (hw - 0.01, rr),
            (hw, r - 0.03),
            (hw - 0.03, r),
            (-hw + 0.03, r),
            (-hw, r - 0.03),
            (-hw + 0.01, rr),
        ];
        let segs = if lod > 1 { 10 } else { 20 };
        s.lathe(o, AXIS, &prof, segs, 0.0, (0.0, mid), m.tire);
        if parts.whitewalls {
            let k = (r - rr) * 0.5;
            s.annulus(
                [hw + 0.002, 0.0, 0.0],
                AXIS,
                rr + k * 0.4,
                rr + k * 1.2,
                segs,
                white,
            );
        }
        // Rims become a flat disc with a hub.
        let face = if hubcaps { m.chrome } else { m.rim };
        s.disc([hw - 0.015, 0.0, 0.0], AXIS, rr, segs, face);
        if lod == 1 {
            s.disc([hw - 0.01, 0.0, 0.0], AXIS, rr * 0.3, 8, m.trim);
        }
        s.disc([-hw + 0.015, 0.0, 0.0], scale(AXIS, -1.0), rr, 8, m.chassis);
        return s;
    }
    // Tire: bead, sidewall bulge, shoulder, a tread with three grooves, and back.
    let segs = 24;
    let side = |t: f32| rr + t * (r - rr);
    let sw = |x: f32, t: f32| (x, side(t));
    let out = [
        sw(hw - 0.012, 0.03),
        sw(hw + 0.002, 0.25),
        sw(hw + 0.007, 0.5),
        sw(hw + 0.004, 0.72),
        sw(hw - 0.008, 0.9),
        (hw - 0.03, r - 0.004),
    ];
    // Outer sidewall, a whitewall band on it when the car has them.
    for k in 0..out.len() - 1 {
        let mat = if parts.whitewalls && (k == 1 || k == 2) {
            white
        } else {
            m.tire
        };
        s.lathe(o, AXIS, &out[k..k + 2], segs, 0.0, (0.0, mid), mat);
    }
    // Tread: ribs at the full radius, two grooves sunk 8 mm.
    let tw = hw - 0.03;
    let mut tread: alloc::vec::Vec<(f32, f32)> = alloc::vec![(tw, r - 0.004)];
    for g in [0.4f32, -0.4] {
        let x = g * tw;
        tread.extend([
            (x + 0.008, r - 0.004),
            (x + 0.006, r - 0.012),
            (x - 0.006, r - 0.012),
            (x - 0.008, r - 0.004),
        ]);
    }
    tread.push((-tw, r - 0.004));
    s.lathe(o, AXIS, &tread, segs, 0.0, (0.0, mid), m.tire);
    let back = [
        (-hw + 0.03, r - 0.004),
        sw(-hw + 0.008, 0.9),
        sw(-hw - 0.006, 0.5),
        sw(-hw + 0.012, 0.03),
    ];
    s.lathe(o, AXIS, &back, segs / 2, 0.0, (0.0, mid), m.tire);
    let face = hw - 0.012;
    if hubcaps {
        // Full wheel cover: a shallow chrome dome with ribs and a dark center.
        let cap = [
            (face, rr + 0.004),
            (face + 0.008, rr * 0.9),
            (face + 0.016, rr * 0.62),
            (face + 0.024, rr * 0.3),
            (face + 0.027, 0.0),
        ];
        s.lathe(o, AXIS, &cap, 24, 0.0, (-0.3, rr * 0.5), m.chrome);
        // A darker ring and flat radial ribs give the cover its pressed look.
        s.lathe(
            o,
            AXIS,
            &[(face + 0.0175, rr * 0.6), (face + 0.0205, rr * 0.48)],
            24,
            0.0,
            (-0.3, rr * 0.5),
            m.chrome.shade(0.7),
        );
        for k in 0..16 {
            let a = 2.0 * PI * (k as f32 + 0.5) / 16.0;
            let (sn, cs) = sin_cos(a);
            let (sn1, cs1) = sin_cos(a + 0.07);
            let pt = |r: f32, x: f32, sn: f32, cs: f32| [x, cs * r, sn * r];
            let q0 = pt(rr * 0.64, face + 0.0185, sn, cs);
            let q1 = pt(rr * 0.88, face + 0.0095, sn, cs);
            let q2 = pt(rr * 0.88, face + 0.0095, sn1, cs1);
            let q3 = pt(rr * 0.64, face + 0.0185, sn1, cs1);
            s.quad_dir(q0, q1, q2, q3, m.chrome.shade(0.62), AXIS);
        }
        s.disc([face + 0.028, 0.0, 0.0], AXIS, rr * 0.18, 16, m.trim);
        lugs(&mut s, face + 0.024, rr * 0.3, 5, m.chrome);
        return s;
    }
    // Barrel, flange and back plate (what you see between spokes).
    s.lathe(
        o,
        AXIS,
        &[(face, rr), (-hw + 0.015, rr)],
        24,
        0.0,
        (0.0, rr + 1.0),
        m.chassis,
    );
    s.lathe(
        o,
        AXIS,
        &[
            (face - 0.004, rr - 0.016),
            (face + 0.001, rr - 0.006),
            (face + 0.002, rr + 0.004),
        ],
        24,
        0.0,
        (-1.0, rr),
        m.rim,
    );
    // A dark back plate so the spokes stand out.
    s.disc([-0.03, 0.0, 0.0], AXIS, rr, 12, m.liner);
    // Brake drum or disc behind the spokes.
    s.cylinder(
        [-0.03, 0.0, 0.0],
        [-0.005, 0.0, 0.0],
        rr * 0.72,
        16,
        true,
        false,
        m.steel,
    );
    let hub_x = face - 0.03;
    match rims {
        Rims::Steel => {
            let dish = [
                (face - 0.004, rr - 0.016),
                (hub_x + 0.005, rr * 0.72),
                (hub_x, rr * 0.42),
                (hub_x + 0.012, rr * 0.3),
                (hub_x + 0.02, 0.0),
            ];
            s.lathe(o, AXIS, &dish, 24, 0.0, (-0.3, rr * 0.5), m.steel);
            for k in 0..8 {
                let a = 2.0 * PI * k as f32 / 8.0;
                let (sn, cs) = sin_cos(a);
                let dir = [0.0, cs, sn];
                let c = add_scaled([hub_x + 0.006, 0.0, 0.0], dir, rr * 0.57);
                let t = [0.0, -sn, cs];
                s.obox(c, [AXIS, dir, t], [0.002, 0.012, 0.025], m.chassis);
            }
            lugs(&mut s, hub_x + 0.022, 0.055, 5, m.chrome);
        }
        Rims::Alloy5 => {
            for k in 0..5 {
                let a = 2.0 * PI * k as f32 / 5.0;
                let (sn, cs) = sin_cos(a);
                let dir = [0.0, cs, sn];
                let p0 = add_scaled([hub_x + 0.01, 0.0, 0.0], dir, 0.05);
                let p1 = add_scaled([face - 0.006, 0.0, 0.0], dir, rr - 0.012);
                s.beam(p0, p1, AXIS, 0.026, 0.013, m.rim);
            }
            s.cylinder(
                [hub_x - 0.02, 0.0, 0.0],
                [hub_x + 0.022, 0.0, 0.0],
                0.065,
                12,
                false,
                false,
                m.rim,
            );
            s.disc([hub_x + 0.022, 0.0, 0.0], AXIS, 0.065, 16, m.rim.shade(1.1));
            // Spoke windows show the dark barrel; a bright lip rings the face.
            s.annulus(
                [face - 0.003, 0.0, 0.0],
                AXIS,
                rr - 0.03,
                rr - 0.012,
                24,
                m.rim,
            );
            lugs(&mut s, hub_x + 0.024, 0.042, 5, m.chrome);
            s.disc([hub_x + 0.03, 0.0, 0.0], AXIS, 0.022, 10, m.trim);
        }
        Rims::Mesh => {
            for k in 0..16 {
                let a = 2.0 * PI * k as f32 / 16.0;
                for (lean, x) in [(0.22f32, 0.0f32), (-0.22, 0.008)] {
                    let (sn, cs) = sin_cos(a);
                    let (sn1, cs1) = sin_cos(a + lean);
                    let p0 = [hub_x + 0.012 + x, cs * 0.07, sn * 0.07];
                    let p1 = [
                        face - 0.006 + x * 0.5,
                        cs1 * (rr - 0.012),
                        sn1 * (rr - 0.012),
                    ];
                    if k % 2 == 0 || lean > 0.0 {
                        s.beam(p0, p1, AXIS, 0.006, 0.006, m.rim);
                    }
                }
            }
            s.cylinder(
                [hub_x - 0.02, 0.0, 0.0],
                [hub_x + 0.024, 0.0, 0.0],
                0.072,
                12,
                false,
                false,
                m.rim,
            );
            s.disc([hub_x + 0.024, 0.0, 0.0], AXIS, 0.072, 16, m.chrome);
            lugs(&mut s, hub_x + 0.026, 0.045, 5, m.chrome.shade(0.8));
            s.annulus(
                [face - 0.003, 0.0, 0.0],
                AXIS,
                rr - 0.03,
                rr - 0.012,
                24,
                m.rim,
            );
        }
        Rims::Turbine => {
            let n = 12;
            for k in 0..n {
                let a = 2.0 * PI * k as f32 / n as f32;
                let (sn, cs) = sin_cos(a);
                let dir = [0.0, cs, sn];
                let t = [0.0, -sn, cs];
                // Blade twisted about its radial axis.
                let (ts, tc) = sin_cos(deg(38.0));
                let up = add(scale(AXIS, tc), scale(t, ts));
                let p0 = add_scaled([hub_x + 0.012, 0.0, 0.0], dir, 0.07);
                let p1 = add_scaled([face - 0.008, 0.0, 0.0], dir, rr - 0.016);
                s.beam(p0, p1, up, 0.003, 0.03, m.rim);
            }
            s.lathe(
                o,
                AXIS,
                &[(face - 0.002, rr - 0.012), (face - 0.004, rr - 0.04)],
                12,
                0.0,
                (-1.0, rr),
                m.rim,
            );
            s.cylinder(
                [hub_x - 0.02, 0.0, 0.0],
                [hub_x + 0.026, 0.0, 0.0],
                0.075,
                10,
                false,
                false,
                m.rim,
            );
            s.disc([hub_x + 0.026, 0.0, 0.0], AXIS, 0.075, 10, m.rim.shade(1.1));
            s.disc([hub_x + 0.027, 0.0, 0.0], AXIS, 0.03, 12, m.trim);
            lugs(&mut s, hub_x + 0.028, 0.05, 5, m.chrome);
        }
        Rims::Dish => {
            // A wide polished lip falls to a face sunk well inside it.
            let deep = hub_x - 0.02;
            s.lathe(
                o,
                AXIS,
                &[
                    (face + 0.002, rr - 0.004),
                    (face - 0.002, rr - 0.04),
                    (deep + 0.004, rr - 0.05),
                    (deep, rr * 0.8),
                ],
                24,
                0.0,
                (-1.0, rr),
                m.chrome,
            );
            s.disc([deep, 0.0, 0.0], AXIS, rr * 0.8, 24, m.rim);
            // Five round holes round the hub, dark through to the barrel.
            for k in 0..5 {
                let a = 2.0 * PI * (k as f32 + 0.5) / 5.0;
                let (sn, cs) = sin_cos(a);
                s.disc(
                    [deep + 0.002, cs * rr * 0.5, sn * rr * 0.5],
                    AXIS,
                    rr * 0.17,
                    12,
                    m.liner,
                );
            }
            s.cylinder(
                [deep, 0.0, 0.0],
                [deep + 0.03, 0.0, 0.0],
                0.06,
                12,
                false,
                false,
                m.rim,
            );
            s.disc([deep + 0.03, 0.0, 0.0], AXIS, 0.06, 12, m.rim.shade(1.1));
            lugs(&mut s, deep + 0.032, 0.04, 5, m.chrome);
            s.disc([deep + 0.036, 0.0, 0.0], AXIS, 0.02, 10, m.trim);
        }
        Rims::Slotted => {
            // A nearly flat face just inside the lip with a ring of long radial slots.
            let fx = face - 0.012;
            s.annulus(
                [face - 0.003, 0.0, 0.0],
                AXIS,
                rr - 0.03,
                rr - 0.012,
                24,
                m.rim,
            );
            s.lathe(
                o,
                AXIS,
                &[(face - 0.003, rr - 0.03), (fx, rr - 0.04)],
                24,
                0.0,
                (-1.0, rr),
                m.rim,
            );
            s.disc([fx, 0.0, 0.0], AXIS, rr - 0.04, 24, m.rim);
            let n = 10;
            for k in 0..n {
                let a = 2.0 * PI * (k as f32 + 0.5) / n as f32;
                let (sn, cs) = sin_cos(a);
                let dir = [0.0, cs, sn];
                let t = [0.0, -sn, cs];
                let c = add_scaled([fx + 0.001, 0.0, 0.0], dir, rr * 0.66);
                s.obox(c, [AXIS, dir, t], [0.002, rr * 0.17, 0.014], m.liner);
            }
            s.disc(
                [fx + 0.004, 0.0, 0.0],
                AXIS,
                rr * 0.36,
                16,
                m.rim.shade(1.1),
            );
            lugs(&mut s, fx + 0.008, 0.045, 5, m.chrome);
            s.disc([fx + 0.012, 0.0, 0.0], AXIS, 0.024, 10, m.trim);
        }
        Rims::Twin => {
            // Six pairs of thin spokes from a small cap out to a wide polished lip; the
            // spokes dish in toward the hub so the lip stands proud.
            s.annulus(
                [face - 0.002, 0.0, 0.0],
                AXIS,
                rr - 0.045,
                rr - 0.008,
                24,
                m.chrome,
            );
            s.lathe(
                o,
                AXIS,
                &[(face - 0.002, rr - 0.045), (face - 0.016, rr - 0.052)],
                24,
                0.0,
                (-1.0, rr),
                m.chrome,
            );
            let n = 6;
            for k in 0..n {
                for split in [-0.09f32, 0.09] {
                    let a = 2.0 * PI * k as f32 / n as f32;
                    let (sn, cs) = sin_cos(a);
                    let (sn1, cs1) = sin_cos(a + split);
                    let p0 = [hub_x + 0.014, cs * 0.05, sn * 0.05];
                    let p1 = [face - 0.016, cs1 * (rr - 0.05), sn1 * (rr - 0.05)];
                    s.beam(p0, p1, AXIS, 0.009, 0.008, m.rim);
                }
            }
            s.cylinder(
                [hub_x - 0.02, 0.0, 0.0],
                [hub_x + 0.02, 0.0, 0.0],
                0.058,
                12,
                false,
                false,
                m.rim,
            );
            s.disc([hub_x + 0.02, 0.0, 0.0], AXIS, 0.058, 12, m.chrome);
            lugs(&mut s, hub_x + 0.022, 0.04, 5, m.chrome.shade(0.8));
            s.disc([hub_x + 0.028, 0.0, 0.0], AXIS, 0.02, 10, m.trim);
        }
    }
    s
}

fn lugs(s: &mut Soup, x: f32, r: f32, n: usize, mat: Mat) {
    for k in 0..n {
        let a = 2.0 * PI * k as f32 / n as f32;
        let (sn, cs) = sin_cos(a);
        let c = [x, cs * r, sn * r];
        s.cylinder(
            sub(c, [0.006, 0.0, 0.0]),
            add(c, [0.006, 0.0, 0.0]),
            0.009,
            6,
            true,
            false,
            mat,
        );
    }
}
