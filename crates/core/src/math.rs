//! Deterministic scalar math for generation and simulation code.
//!
//! `core` offers no `sqrt`, `sin` or `floor` without `std`, and generated geometry must be
//! bit-identical on every platform, so these use only IEEE add, multiply and divide (which
//! are correctly rounded everywhere; Rust never fuses them into multiply-adds).

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
            let want = f32::sqrt(x);
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
    fn floor_rounds_down() {
        assert_eq!(floor(2.7), 2.0);
        assert_eq!(floor(-2.2), -3.0);
        assert_eq!(floor(-3.0), -3.0);
    }
}
