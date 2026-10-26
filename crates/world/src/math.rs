//! Deterministic scalar and vector math for generation code.
//!
//! `core` offers no `sqrt`, `sin` or `floor` without `std`, and generated geometry must be
//! bit-identical on every platform, so these use only IEEE add, multiply and divide (which
//! are correctly rounded everywhere; Rust never fuses them into multiply-adds).

use core::ops::{Add, Mul, Neg, Sub};

pub const PI: f32 = core::f32::consts::PI;
pub const TAU: f32 = core::f32::consts::TAU;

/// Square root by Newton iteration in f64. Returns 0 for zero, negative or NaN input.
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    let xd = f64::from(x);
    // Halving the exponent gives a first guess within ~6%; Newton doubles the correct
    // digits per step, so three steps are far below f32 rounding.
    let mut y = f64::from_bits((xd.to_bits() >> 1) + (1023u64 << 51));
    for _ in 0..3 {
        y = 0.5 * (y + xd / y);
    }
    y as f32
}

/// Largest integer not above `x`, for `|x| < 2^31`.
pub fn floor(x: f32) -> f32 {
    let t = x as i32 as f32;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

fn floor64(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

pub fn abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `(sin x, cos x)` from Taylor polynomials after reduction to `[-pi/4, pi/4]`; error
/// below 1e-9 before rounding to f32.
pub fn sin_cos(x: f32) -> (f32, f32) {
    use core::f64::consts::{FRAC_PI_2, TAU as TAU64};
    let x = f64::from(x);
    let r = x - floor64(x / TAU64 + 0.5) * TAU64;
    let q = floor64(r / FRAC_PI_2 + 0.5);
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

/// A point or direction in 3D. Y is up.
/// The angle of `(x, y)` from the +x axis toward +y, in `(-PI, PI]`; 0 for `(0, 0)`.
/// Two half-angle reductions and a Taylor series in f64; error about one f32 rounding.
pub fn atan2(y: f32, x: f32) -> f32 {
    use core::f64::consts::{FRAC_PI_2, PI as PI64};
    let (y, x) = (f64::from(y), f64::from(x));
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let sqrt64 = |v: f64| f64::from(sqrt(v as f32));
    // atan of |y/x| or |x/y| (whichever is at most 1), then two halvings:
    // atan(z) = 2 atan(z / (1 + sqrt(1 + z^2))), with this module's (f32) square root.
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
        a = PI64 - a;
    }
    if y < 0.0 {
        a = -a;
    }
    a as f32
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const ZERO: V3 = V3::new(0.0, 0.0, 0.0);
    pub const X: V3 = V3::new(1.0, 0.0, 0.0);
    pub const Y: V3 = V3::new(0.0, 1.0, 0.0);
    pub const Z: V3 = V3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32) -> V3 {
        V3 { x, y, z }
    }

    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: V3) -> V3 {
        V3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        sqrt(self.dot(self))
    }

    /// Unit vector, or zero for a zero vector.
    pub fn normalize(self) -> V3 {
        let l = self.length();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            V3::ZERO
        }
    }

    pub fn lerp(self, o: V3, t: f32) -> V3 {
        self + (o - self) * t
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// Ground-plane part `(x, z)`.
    pub fn xz(self) -> V2 {
        V2::new(self.x, self.z)
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        V3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        V3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        V3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        V3::new(-self.x, -self.y, -self.z)
    }
}

/// A point or direction on the ground plane: `x` east, `y` holds world Z (south).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

impl V2 {
    pub const fn new(x: f32, y: f32) -> V2 {
        V2 { x, y }
    }

    pub fn from_array(p: [f32; 2]) -> V2 {
        V2::new(p[0], p[1])
    }

    pub fn dot(self, o: V2) -> f32 {
        self.x * o.x + self.y * o.y
    }

    pub fn length(self) -> f32 {
        sqrt(self.dot(self))
    }

    pub fn normalize(self) -> V2 {
        let l = self.length();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            V2::new(0.0, 0.0)
        }
    }

    /// Rotated a quarter turn counter-clockwise seen from above: the left-hand side of
    /// someone walking along `self`.
    pub fn left(self) -> V2 {
        V2::new(self.y, -self.x)
    }

    /// The right-hand side of someone walking along `self`; the outward normal of an edge
    /// of a counter-clockwise polygon.
    pub fn right(self) -> V2 {
        V2::new(-self.y, self.x)
    }

    /// Positive when `o` points to the left of `self` (counter-clockwise seen from above):
    /// the sine of the turn from `self` to `o` for unit vectors.
    pub fn turn(self, o: V2) -> f32 {
        self.left().dot(o)
    }

    pub fn lerp(self, o: V2, t: f32) -> V2 {
        self + (o - self) * t
    }

    /// Lifts to 3D at height `y`.
    pub fn at(self, y: f32) -> V3 {
        V3::new(self.x, y, self.y)
    }

    /// A monotone stand-in for the counter-clockwise angle (seen from above) from east, in
    /// `[0, 4)`; sorts directions without trigonometry.
    pub fn pseudo_angle(self) -> f32 {
        // Map to math orientation (north positive) and use the "diamond angle".
        let (x, y) = (self.x, -self.y);
        let s = abs(x) + abs(y);
        if s == 0.0 {
            return 0.0;
        }
        let (x, y) = (x / s, y / s);
        if y >= 0.0 {
            if x >= 0.0 {
                y
            } else {
                1.0 - x
            }
        } else if x < 0.0 {
            2.0 - y
        } else {
            3.0 + x
        }
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, s: f32) -> V2 {
        V2::new(self.x * s, self.y * s)
    }
}

impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        V2::new(-self.x, -self.y)
    }
}

/// Intersection of the lines `p + s * d` and `q + t * e`, or `None` if (nearly) parallel.
pub fn intersect_lines(p: V2, d: V2, q: V2, e: V2) -> Option<V2> {
    let den = d.x * e.y - d.y * e.x;
    if abs(den) < 1e-6 {
        return None;
    }
    let w = q - p;
    let s = (w.x * e.y - w.y * e.x) / den;
    Some(p + d * s)
}

/// Polygon offset with mitered corners: edge `i` (from vertex `i` to `i + 1`) moves by
/// `dist(i)` along its outward normal (outward for a counter-clockwise polygon; negative
/// moves inward). Collinear neighbours keep the vertex on the shared offset line.
pub fn offset_polygon(poly: &[V2], dist: impl Fn(usize) -> f32) -> alloc::vec::Vec<V2> {
    let n = poly.len();
    let mut out = alloc::vec::Vec::with_capacity(n);
    for i in 0..n {
        let prev = (i + n - 1) % n;
        let p = poly[i];
        let d0 = (p - poly[prev]).normalize();
        let d1 = (poly[(i + 1) % n] - p).normalize();
        let (o0, o1) = (dist(prev), dist(i));
        let a = p + d0.right() * o0;
        let b = p + d1.right() * o1;
        out.push(intersect_lines(a, d0, b, d1).unwrap_or(b));
    }
    out
}

/// Shoelace area, positive for counter-clockwise polygons seen from above.
pub fn signed_area(poly: &[V2]) -> f32 {
    let mut acc = 0.0;
    for (i, p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        acc += p.y * q.x - p.x * q.y;
    }
    acc * 0.5
}

pub fn point_in_polygon(p: V2, poly: &[V2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from `p` to the segment `a..b`, and the parameter of the closest point.
pub fn segment_distance(p: V2, a: V2, b: V2) -> (f32, f32) {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 {
        clamp((p - a).dot(ab) / l2, 0.0, 1.0)
    } else {
        0.0
    };
    ((a + ab * t - p).length(), t)
}

/// Smooth 2D value noise in `[0, 1)` with lattice spacing `cell`, keyed by `seed`.
pub fn value_noise(seed: u64, x: f32, z: f32, cell: f32) -> f32 {
    let (fx, fz) = (x / cell, z / cell);
    let (ix, iz) = (floor(fx), floor(fz));
    let (tx, tz) = (fx - ix, fz - iz);
    let (tx, tz) = (tx * tx * (3.0 - 2.0 * tx), tz * tz * (3.0 - 2.0 * tz));
    let (ix, iz) = (ix as i32, iz as i32);
    let at = |i: i32, j: i32| lattice(seed, i, j);
    let a = lerp(at(ix, iz), at(ix + 1, iz), tx);
    let b = lerp(at(ix, iz + 1), at(ix + 1, iz + 1), tx);
    lerp(a, b, tz)
}

/// Uniform `[0, 1)` value for an integer lattice point.
pub fn lattice(seed: u64, i: i32, j: i32) -> f32 {
    let k = (i as u32 as u64) | ((j as u32 as u64) << 32);
    let h = skywave_core::mix64(seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    (h >> 40) as f32 * (1.0 / 16_777_216.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atan2_matches_std() {
        for (y, x) in [
            (0.0f32, 1.0f32),
            (1.0, 1.0),
            (1.0, 0.0),
            (1.0, -1.0),
            (0.0, -1.0),
            (-1.0, -1.0),
            (-1.0, 0.0),
            (-3.0, 0.5),
            (0.2, 7.0),
            (-0.001, -5.0),
        ] {
            assert!((atan2(y, x) - y.atan2(x)).abs() < 2e-6, "{y} {x}");
        }
        assert_eq!(atan2(0.0, 0.0), 0.0);
    }

    #[test]
    fn sqrt_matches_std() {
        for &x in &[1e-6f32, 0.25, 1.0, 2.0, 3.0, 10.0, 12345.678, 1e9] {
            let got = sqrt(x);
            let want = std::primitive::f32::sqrt(x);
            assert!((got - want).abs() <= want * 1e-6, "{x}: {got} vs {want}");
        }
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(-4.0), 0.0);
        assert_eq!(sqrt(f32::NAN), 0.0);
    }

    #[test]
    fn sin_cos_matches_std() {
        let mut x = -20.0f32;
        while x < 20.0 {
            let (s, c) = sin_cos(x);
            assert!((s - x.sin()).abs() < 2e-6, "sin {x}");
            assert!((c - x.cos()).abs() < 2e-6, "cos {x}");
            x += 0.137;
        }
    }

    #[test]
    fn floor_and_pseudo_angle() {
        assert_eq!(floor(2.7), 2.0);
        assert_eq!(floor(-2.2), -3.0);
        assert_eq!(floor(-3.0), -3.0);
        // East, north, west, south in counter-clockwise order seen from above.
        let dirs = [
            V2::new(1.0, 0.0),
            V2::new(0.0, -1.0),
            V2::new(-1.0, 0.0),
            V2::new(0.0, 1.0),
        ];
        let a: std::vec::Vec<f32> = dirs.iter().map(|d| d.pseudo_angle()).collect();
        assert!(a.windows(2).all(|w| w[0] < w[1]), "{a:?}");
        assert!(V2::new(1.0, 0.0).turn(V2::new(0.0, -1.0)) > 0.0);
    }

    #[test]
    fn offsets_and_polygons() {
        let sq = [
            V2::new(0.0, 0.0),
            V2::new(0.0, 2.0),
            V2::new(2.0, 2.0),
            V2::new(2.0, 0.0),
        ];
        assert!(signed_area(&sq) > 0.0);
        let out = offset_polygon(&sq, |_| 1.0);
        assert_eq!(out[0], V2::new(-1.0, -1.0));
        assert_eq!(out[2], V2::new(3.0, 3.0));
        assert!(point_in_polygon(V2::new(1.0, 1.0), &sq));
        assert!(!point_in_polygon(V2::new(3.0, 1.0), &sq));
        let n = value_noise(7, 3.3, -8.1, 4.0);
        assert!((0.0..1.0).contains(&n));
        assert_eq!(n, value_noise(7, 3.3, -8.1, 4.0));
    }
}
