//! The sim's vector, quaternion and scalar math, built from `+ - * /` only.
//!
//! `vehicle` is `no_std`, so `f32::sqrt`/`sin`/`atan2` are not available; the versions here
//! are also bit-identical on every target (no libm, no `mul_add`, IEEE basic operations
//! only), which keeps replays deterministic. Accuracy is ~1e-6 or better, plenty for a car.

pub type V3 = [f32; 3];
/// Unit quaternion `[x, y, z, w]` rotating body space into world space.
pub type Quat = [f32; 4];

pub const PI: f32 = core::f32::consts::PI;
pub const ZERO: V3 = [0.0, 0.0, 0.0];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `a + b * s`, rounded as two separate operations (never fused).
pub fn add_scaled(a: V3, b: V3, s: f32) -> V3 {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
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

pub fn length(a: V3) -> f32 {
    sqrt(dot(a, a))
}

/// `a` scaled to unit length, or zero if `a` is (nearly) zero.
pub fn normalize(a: V3) -> V3 {
    let len = length(a);
    if len > 1e-9 {
        scale(a, 1.0 / len)
    } else {
        ZERO
    }
}

/// Hamilton product `a * b` (apply `b`, then `a`).
pub fn q_mul(a: Quat, b: Quat) -> Quat {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Rotates `v` by unit quaternion `q`: body space to world space.
pub fn rotate(q: Quat, v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    let t = scale(cross(u, v), 2.0);
    add(add_scaled(v, t, q[3]), cross(u, t))
}

/// Rotates `v` by the inverse of `q`: world space to body space.
pub fn rotate_inv(q: Quat, v: V3) -> V3 {
    rotate([-q[0], -q[1], -q[2], q[3]], v)
}

pub fn q_normalize(q: Quat) -> Quat {
    let len = sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
    if len > 1e-9 {
        let k = 1.0 / len;
        [q[0] * k, q[1] * k, q[2] * k, q[3] * k]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Advances orientation `q` by world-space angular velocity `w` over `dt`, renormalized.
pub fn q_integrate(q: Quat, w: V3, dt: f32) -> Quat {
    let d = q_mul([w[0], w[1], w[2], 0.0], q);
    let h = 0.5 * dt;
    q_normalize([
        q[0] + d[0] * h,
        q[1] + d[1] * h,
        q[2] + d[2] * h,
        q[3] + d[3] * h,
    ])
}

/// Rotation by `angle` radians about the world `+y` axis.
pub fn q_from_yaw(angle: f32) -> Quat {
    let h = 0.5 * angle;
    [0.0, sin(h), 0.0, cos(h)]
}

pub fn abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// `x` clamped to `[0, 1]`; NaN becomes 0.
pub fn unit(x: f32) -> f32 {
    if x > 0.0 {
        if x < 1.0 {
            x
        } else {
            1.0
        }
    } else {
        0.0
    }
}

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

pub fn sin(x: f32) -> f32 {
    sin_cos(x).0
}

pub fn cos(x: f32) -> f32 {
    sin_cos(x).1
}

/// Arctangent on `[-1, 1]`, a minimax polynomial (error < 2e-6 rad).
fn atan_unit(z: f32) -> f32 {
    let z2 = z * z;
    let p = 0.052_653_32 - z2 * 0.011_721_2;
    let p = 0.193_543_46 + z2 * (-0.116_432_87 + z2 * p);
    z * (0.999_977_26 + z2 * (-0.332_623_47 + z2 * p))
}

/// Four-quadrant arctangent of `y / x`, in `[-pi, pi]`; 0 for `(0, 0)`.
pub fn atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (abs(x), abs(y));
    if ax == 0.0 && ay == 0.0 {
        return 0.0;
    }
    let mut a = if ay <= ax {
        atan_unit(ay / ax)
    } else {
        0.5 * PI - atan_unit(ax / ay)
    };
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 {
        -a
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        abs(a - b) <= tol
    }

    #[test]
    fn sqrt_is_accurate() {
        for &x in &[1e-6_f32, 0.01, 0.5, 1.0, 2.0, 9.0, 1234.5, 6.0e6] {
            let y = sqrt(x);
            assert!(close(y * y, x, x * 2e-6), "sqrt({x}) = {y}");
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-4.0), 0.0);
        assert_eq!(sqrt(f32::NAN), 0.0);
        assert_eq!(sqrt(4.0), 2.0);
    }

    #[test]
    fn trig_matches_known_values() {
        assert!(close(sin(0.0), 0.0, 1e-7));
        assert!(close(sin(PI / 6.0), 0.5, 1e-6));
        assert!(close(sin(PI / 2.0), 1.0, 1e-6));
        assert!(close(sin(-PI / 2.0), -1.0, 1e-6));
        assert!(close(sin(3.0 * PI / 4.0), 0.707_106_77, 1e-6));
        assert!(close(sin(7.0), 0.656_986_6, 1e-5));
        assert!(close(cos(0.0), 1.0, 1e-6));
        assert!(close(cos(PI), -1.0, 1e-6));
        assert!(close(cos(-PI / 3.0), 0.5, 1e-6));
        assert!(close(atan2(1.0, 1.0), PI / 4.0, 3e-6));
        assert!(close(atan2(1.0, -1.0), 3.0 * PI / 4.0, 3e-6));
        assert!(close(atan2(-1.0, -1.0), -3.0 * PI / 4.0, 3e-6));
        assert!(close(atan2(-2.0, 0.0), -PI / 2.0, 3e-6));
        assert!(close(atan2(0.1, 3.0), 0.033_320_99, 3e-6));
        assert_eq!(atan2(0.0, 0.0), 0.0);
    }

    #[test]
    fn rotation_round_trips() {
        // A quarter turn left about +y takes forward (-z) to -x.
        let q = q_from_yaw(PI / 2.0);
        let f = rotate(q, [0.0, 0.0, -1.0]);
        assert!(close(f[0], -1.0, 1e-6) && close(f[2], 0.0, 1e-6));
        let back = rotate_inv(q, f);
        assert!(close(back[2], -1.0, 1e-6));
        // Integrating a constant yaw rate for one second turns by that many radians.
        let mut q = [0.0, 0.0, 0.0, 1.0];
        for _ in 0..1000 {
            q = q_integrate(q, [0.0, 0.5, 0.0], 0.001);
        }
        let f = rotate(q, [0.0, 0.0, -1.0]);
        assert!(close(atan2(-f[0], -f[2]), 0.5, 1e-3));
    }
}
