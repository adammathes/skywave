//! Deterministic scalar math for graph building and queries.
//!
//! `core` offers no `sqrt` or `atan2` without `std`, and the graph must be bit-identical on
//! every platform, so these use only IEEE add, multiply and divide (the same algorithms as
//! `world::math`).

/// Square root by Newton iteration in f64. Returns 0 for zero, negative or NaN input.
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    let xd = f64::from(x);
    let mut y = f64::from_bits((xd.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..3 {
        y = 0.5 * (y + xd / y);
    }
    y as f32
}

/// The angle of `(x, y)` from the +x axis toward +y, in `(-PI, PI]`; 0 for `(0, 0)`.
pub fn atan2(y: f32, x: f32) -> f32 {
    use core::f64::consts::{FRAC_PI_2, PI};
    let (y, x) = (f64::from(y), f64::from(x));
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let sqrt64 = |v: f64| f64::from(sqrt(v as f32));
    let (ay, ax) = (if y < 0.0 { -y } else { y }, if x < 0.0 { -x } else { x });
    let swap = ay > ax;
    let mut z = if swap { ax / ay } else { ay / ax };
    for _ in 0..2 {
        z /= 1.0 + sqrt64(1.0 + z * z);
    }
    let z2 = z * z;
    let mut term = z;
    let mut sum = 0.0;
    for k in 0..12 {
        sum += term / (2 * k + 1) as f64;
        term *= -z2;
    }
    let mut a = 4.0 * sum;
    if swap {
        a = FRAC_PI_2 - a;
    }
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 {
        a = -a;
    }
    a as f32
}

/// `(sin x, cos x)` from Taylor polynomials after reduction to `[-pi/4, pi/4]` (as
/// `world::math`).
pub fn sin_cos(x: f32) -> (f32, f32) {
    use core::f64::consts::{FRAC_PI_2, TAU};
    let floor = |v: f64| {
        let t = v as i64 as f64;
        if t > v {
            t - 1.0
        } else {
            t
        }
    };
    let x = f64::from(x);
    let r = x - floor(x / TAU + 0.5) * TAU;
    let q = floor(r / FRAC_PI_2 + 0.5);
    let t = r - q * FRAC_PI_2;
    let t2 = t * t;
    let s = t
        * (1.0
            + t2 * (-1.0 / 6.0
                + t2 * (1.0 / 120.0
                    + t2 * (-1.0 / 5040.0
                        + t2 * (1.0 / 362_880.0
                            + t2 * (-1.0 / 39_916_800.0 + t2 / 6_227_020_800.0))))));
    let c = 1.0
        + t2 * (-0.5
            + t2 * (1.0 / 24.0
                + t2 * (-1.0 / 720.0
                    + t2 * (1.0 / 40_320.0 + t2 * (-1.0 / 3_628_800.0 + t2 / 479_001_600.0)))));
    let (s, c) = match (q as i64).rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (s as f32, c as f32)
}

/// Ground-plane length of `(x, z)`.
pub fn len2(v: [f32; 2]) -> f32 {
    sqrt(v[0] * v[0] + v[1] * v[1])
}

/// `v` scaled to unit length; `(0, 0)` stays `(0, 0)`.
pub fn unit2(v: [f32; 2]) -> [f32; 2] {
    let l = len2(v);
    if l > 0.0 {
        [v[0] / l, v[1] / l]
    } else {
        [0.0, 0.0]
    }
}

/// Ground-plane direction from `a` to `b` (`[x, y, z]` points), unit length.
pub fn dir(a: [f32; 3], b: [f32; 3]) -> [f32; 2] {
    unit2([b[0] - a[0], b[2] - a[2]])
}

/// Ground-plane distance between two `[x, y, z]` points.
pub fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    len2([b[0] - a[0], b[2] - a[2]])
}

/// The right-hand side of someone driving along `d` (X east, Z south, seen from above).
pub fn right(d: [f32; 2]) -> [f32; 2] {
    [-d[1], d[0]]
}

pub fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

/// Signed angle from direction `a` to `b`, positive turning left (counter-clockwise seen
/// from above), in `(-PI, PI]`.
pub fn turn_angle(a: [f32; 2], b: [f32; 2]) -> f32 {
    // `a`'s left is `(a.z, -a.x)`.
    atan2(b[0] * a[1] - b[1] * a[0], dot(a, b))
}

pub fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_atan2_and_sin_cos_match_std() {
        for &x in &[1e-6f32, 0.25, 2.0, 9.0, 1234.5, 1e7] {
            assert!((sqrt(x) - x.sqrt()).abs() <= x.sqrt() * 2e-7);
        }
        for i in 0..64 {
            let a = i as f32 * 0.1 - 3.2;
            let (y, x) = (a.sin(), a.cos());
            assert!((atan2(y, x) - y.atan2(x)).abs() < 1e-6, "{a}");
            let (sn, cs) = sin_cos(a * 3.0);
            assert!((sn - (a * 3.0).sin()).abs() < 1e-6 && (cs - (a * 3.0).cos()).abs() < 1e-6);
        }
    }

    #[test]
    fn left_turns_are_positive() {
        let east = [1.0, 0.0];
        let north = [0.0, -1.0];
        assert!((turn_angle(east, north) - core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((turn_angle(north, east) + core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(right(east), [0.0, 1.0]);
    }
}
