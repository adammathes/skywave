//! One wheel: tire with a sidewall profile plus a rim with real spoke geometry.
//!
//! Local frame: origin at the hub center, axle along `+x` with the outer face toward `+x`
//! (a right-side wheel). Left wheels are the same mesh turned 180° about `y`.

use super::math::*;
use super::mesh::{Mat, Soup};
use super::params::{Rims, Wheels};
use super::Materials;

const AXIS: V3 = [1.0, 0.0, 0.0];

/// Rim radius for a tire of outer radius `r` (80s tires had tall sidewalls).
pub fn rim_radius(r: f32) -> f32 {
    r * 0.62
}

pub fn build(w: &Wheels, rims: Rims, hubcaps: bool, lod: u8, m: &Materials) -> Soup {
    let mut s = Soup::new(40.0);
    let r = w.radius;
    let hw = w.width * 0.5;
    let rr = rim_radius(r);
    let o = [0.0; 3];
    let mid = (r + rr) * 0.5;
    if lod > 0 {
        let prof = [
            (hw - 0.01, rr),
            (hw, r - 0.03),
            (hw - 0.03, r),
            (-hw + 0.03, r),
            (-hw, r - 0.03),
            (-hw + 0.01, rr),
        ];
        s.lathe(o, AXIS, &prof, 10, 0.0, (0.0, mid), m.tire);
        // Rims become a flat disc with a hub.
        s.disc([hw - 0.015, 0.0, 0.0], AXIS, rr, 10, m.rim);
        s.disc([-hw + 0.015, 0.0, 0.0], scale(AXIS, -1.0), rr, 8, m.chassis);
        return s;
    }
    let prof = [
        (hw - 0.012, rr + 0.004),
        (hw + 0.006, rr + 0.45 * (r - rr)),
        (hw - 0.03, r - 0.004),
        (-hw + 0.03, r - 0.004),
        (-hw - 0.006, rr + 0.45 * (r - rr)),
        (-hw + 0.012, rr + 0.004),
    ];
    s.lathe(o, AXIS, &prof, 16, 0.0, (0.0, mid), m.tire);
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
        s.lathe(o, AXIS, &cap, 14, 0.0, (-0.3, rr * 0.5), m.chrome);
        // A darker ring and flat radial ribs give the cover its pressed look.
        s.lathe(
            o,
            AXIS,
            &[(face + 0.0175, rr * 0.6), (face + 0.0205, rr * 0.48)],
            14,
            0.0,
            (-0.3, rr * 0.5),
            m.chrome.shade(0.7),
        );
        for k in 0..10 {
            let a = 2.0 * PI * (k as f32 + 0.5) / 10.0;
            let (sn, cs) = sin_cos(a);
            let (sn1, cs1) = sin_cos(a + 0.07);
            let pt = |r: f32, x: f32, sn: f32, cs: f32| [x, cs * r, sn * r];
            let q0 = pt(rr * 0.64, face + 0.0185, sn, cs);
            let q1 = pt(rr * 0.88, face + 0.0095, sn, cs);
            let q2 = pt(rr * 0.88, face + 0.0095, sn1, cs1);
            let q3 = pt(rr * 0.64, face + 0.0185, sn1, cs1);
            s.quad_dir(q0, q1, q2, q3, m.chrome.shade(0.62), AXIS);
        }
        s.disc([face + 0.028, 0.0, 0.0], AXIS, rr * 0.18, 10, m.trim);
        return s;
    }
    // Barrel, flange and back plate (what you see between spokes).
    s.lathe(
        o,
        AXIS,
        &[(face, rr), (-hw + 0.015, rr)],
        12,
        0.0,
        (0.0, rr + 1.0),
        m.chassis,
    );
    s.lathe(
        o,
        AXIS,
        &[(face - 0.004, rr - 0.016), (face + 0.002, rr + 0.004)],
        12,
        0.0,
        (-1.0, rr),
        m.rim,
    );
    s.disc([-0.03, 0.0, 0.0], AXIS, rr, 8, m.chassis);
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
            s.lathe(o, AXIS, &dish, 12, 0.0, (-0.3, rr * 0.5), m.steel);
            for k in 0..6 {
                let a = 2.0 * PI * k as f32 / 6.0;
                let (sn, cs) = sin_cos(a);
                let dir = [0.0, cs, sn];
                let c = madd([hub_x + 0.006, 0.0, 0.0], dir, rr * 0.57);
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
                let p0 = madd([hub_x + 0.01, 0.0, 0.0], dir, 0.05);
                let p1 = madd([face - 0.006, 0.0, 0.0], dir, rr - 0.012);
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
            s.disc([hub_x + 0.022, 0.0, 0.0], AXIS, 0.065, 12, m.rim.shade(1.1));
            lugs(&mut s, hub_x + 0.024, 0.042, 5, m.chrome);
        }
        Rims::Mesh => {
            for k in 0..10 {
                let a = 2.0 * PI * k as f32 / 10.0;
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
            s.disc([hub_x + 0.024, 0.0, 0.0], AXIS, 0.072, 12, m.chrome);
        }
        Rims::Turbine => {
            let n = 8;
            for k in 0..n {
                let a = 2.0 * PI * k as f32 / n as f32;
                let (sn, cs) = sin_cos(a);
                let dir = [0.0, cs, sn];
                let t = [0.0, -sn, cs];
                // Blade twisted about its radial axis.
                let (ts, tc) = sin_cos(deg(38.0));
                let up = add(scale(AXIS, tc), scale(t, ts));
                let p0 = madd([hub_x + 0.012, 0.0, 0.0], dir, 0.07);
                let p1 = madd([face - 0.008, 0.0, 0.0], dir, rr - 0.016);
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
            s.disc([hub_x + 0.027, 0.0, 0.0], AXIS, 0.03, 8, m.trim);
        }
    }
    s
}

fn lugs(s: &mut Soup, x: f32, r: f32, n: usize, mat: Mat) {
    for k in 0..n {
        let a = 2.0 * PI * k as f32 / n as f32;
        let (sn, cs) = sin_cos(a);
        let c = [x, cs * r, sn * r];
        s.aabb(
            sub(c, [0.006, 0.009, 0.009]),
            add(c, [0.006, 0.009, 0.009]),
            mat,
        );
    }
}
