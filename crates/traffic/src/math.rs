//! Ground-plane vector helpers for graph building and queries, on the deterministic
//! scalars of `skywave_core::math` (re-exported here).

pub use skywave_core::math::{atan2, sin_cos, sqrt};

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
    fn left_turns_are_positive() {
        let east = [1.0, 0.0];
        let north = [0.0, -1.0];
        assert!((turn_angle(east, north) - core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((turn_angle(north, east) + core::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(right(east), [0.0, 1.0]);
    }
}
