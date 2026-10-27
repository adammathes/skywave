//! Small deterministic float helpers for mesh generation.
//!
//! `core` has no `sqrt`, `sin` or `cos`, and the platform libm versions may differ in the
//! last bit between targets. These use only IEEE basic operations (no `mul_add`), so the
//! same input gives the same bits everywhere and the generated meshes hash identically on
//! every platform.

pub type V3 = [f32; 3];

pub const PI: f32 = core::f32::consts::PI;

/// Square root by Newton iteration in f64; `0.0` for zero, negative or NaN input.
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    if x == f32::INFINITY {
        return x;
    }
    let xd = x as f64;
    // Halving the exponent bits gives a guess within a factor of ~1.5.
    let mut y = f64::from_bits((xd.to_bits() >> 1) + 0x1ff8_0000_0000_0000);
    for _ in 0..6 {
        y = 0.5 * (y + xd / y);
    }
    y as f32
}

/// `sin` and `cos` of `x` radians, by range reduction to [-pi/4, pi/4] and Taylor series in
/// f64 (error well below f32 precision).
pub fn sin_cos(x: f32) -> (f32, f32) {
    let xd = x as f64;
    let half_pi = core::f64::consts::FRAC_PI_2;
    // Quadrant k: x = k * pi/2 + r with |r| <= pi/4.
    let kf = xd / half_pi;
    let k = if kf >= 0.0 {
        (kf + 0.5) as i64
    } else {
        (kf - 0.5) as i64
    };
    let r = xd - (k as f64) * half_pi;
    let r2 = r * r;
    let s = r
        * (1.0
            - r2 / 6.0
                * (1.0 - r2 / 20.0 * (1.0 - r2 / 42.0 * (1.0 - r2 / 72.0 * (1.0 - r2 / 110.0)))));
    let c = 1.0
        - r2 / 2.0 * (1.0 - r2 / 12.0 * (1.0 - r2 / 30.0 * (1.0 - r2 / 56.0 * (1.0 - r2 / 90.0))));
    let (s, c) = match k.rem_euclid(4) {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    };
    (s as f32, c as f32)
}

pub fn cos(x: f32) -> f32 {
    sin_cos(x).1
}

pub fn tan(x: f32) -> f32 {
    let (s, c) = sin_cos(x);
    s / c
}

pub fn deg(d: f32) -> f32 {
    d * (PI / 180.0)
}

pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// `a + b * k`.
pub fn madd(a: V3, b: V3, k: f32) -> V3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

/// Point between `a` and `b` at `t`.
pub fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn len(a: V3) -> f32 {
    sqrt(dot(a, a))
}

/// `a` scaled to unit length; `[0, 1, 0]` for a zero vector.
pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Two unit vectors perpendicular to unit `n` and to each other, `(u, v)` with `u × v = n`.
pub fn basis(n: V3) -> (V3, V3) {
    let helper = if n[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = norm(cross(helper, n));
    let v = cross(n, u);
    (u, v)
}

/// Linear interpolation in a polyline of `(x, y)` points with strictly increasing x,
/// held flat beyond the ends.
pub fn polyline_at(points: &[(f32, f32)], x: f32) -> f32 {
    let Some(&(x0, y0)) = points.first() else {
        return 0.0;
    };
    if x <= x0 {
        return y0;
    }
    let mut prev = (x0, y0);
    for &(x1, y1) in &points[1..] {
        if x <= x1 {
            let t = (x - prev.0) / (x1 - prev.0);
            return lerp(prev.1, y1, t);
        }
        prev = (x1, y1);
    }
    prev.1
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn sqrt_matches_exact_squares() {
        for i in 0..2000u32 {
            let x = i as f32 * 0.37;
            let r = sqrt(x * x);
            assert!((r - x).abs() <= x * 1e-6 + 1e-7, "{x} {r}");
        }
        assert_eq!(sqrt(-1.0), 0.0);
        assert_eq!(sqrt(f32::NAN), 0.0);
        assert_eq!(sqrt(4.0), 2.0);
    }

    #[test]
    fn trig_is_accurate() {
        let mut x = -20.0f32;
        while x < 20.0 {
            let (s, c) = sin_cos(x);
            let (rs, rc) = ((x as f64).sin() as f32, (x as f64).cos() as f32);
            assert!((s - rs).abs() < 2e-6 && (c - rc).abs() < 2e-6, "{x}");
            x += 0.013;
        }
    }

    #[test]
    fn polyline_interpolates_and_clamps() {
        let p = [(0.0, 1.0), (1.0, 3.0), (3.0, 3.0)];
        assert_eq!(polyline_at(&p, -1.0), 1.0);
        assert_eq!(polyline_at(&p, 0.5), 2.0);
        assert_eq!(polyline_at(&p, 2.0), 3.0);
        assert_eq!(polyline_at(&p, 9.0), 3.0);
    }
}
