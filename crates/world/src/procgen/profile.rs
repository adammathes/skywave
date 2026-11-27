//! Vertical curves: a road's height profile eased so its grade never changes faster than a
//! bound, the way a road engineer rounds every crest and sag. A profile that only follows
//! the ground under a slope limit has a kink at every sample; at speed each kink is a jolt
//! and a sharp crest throws the car off the road.

use alloc::vec;
use alloc::vec::Vec;

use crate::math::{abs, clamp, lerp};

/// Grade change per meter a main road allows (a crest at 40 m/s unloads the car by a sixth
/// of its weight).
pub(crate) const ROAD_CURVE: f32 = 0.001;
/// The same for a steep road, which is driven slower and has less room to do it in.
pub(crate) const STREET_CURVE: f32 = 0.004;

/// `y` (heights at arc lengths `s`) with its grade changing by no more than `k` per meter:
/// the same ends, and the grade `g0` leaving the first and `g1` arriving at the last when
/// given, so two stretches that share an end and its grade meet without a kink.
///
/// The grades of `y` are clamped to what the bound can reach from each end, rate-limited
/// forward and backward and averaged (each pass keeps the bound, so their mean does); the
/// heights are those grades summed, and what the far end then misses by is given back as a
/// parabola of grade that is nothing at both ends. Where the ends leave no solution (a
/// short stretch between two grades far apart) the bound gives way, not the ends.
pub(crate) fn ease(y: &[f32], s: &[f32], k: f32, g0: Option<f32>, g1: Option<f32>) -> Vec<f32> {
    let n = y.len();
    if n < 3 {
        return y.to_vec();
    }
    let m = n - 1;
    let d: Vec<f32> = (0..m).map(|j| (s[j + 1] - s[j]).max(1e-3)).collect();
    // Each segment's middle, as arc length: grades live there.
    let c: Vec<f32> = (0..m).map(|j| 0.5 * (s[j] + s[j + 1])).collect();
    let (a, b) = (s[0], s[m]);
    let q: Vec<f32> = (0..m)
        .map(|j| {
            let x = (y[j + 1] - y[j]) / d[j];
            let (mut lo, mut hi) = (f32::NEG_INFINITY, f32::INFINITY);
            if let Some(g) = g0 {
                lo = lo.max(g - k * (c[j] - a));
                hi = hi.min(g + k * (c[j] - a));
            }
            if let Some(g) = g1 {
                lo = lo.max(g - k * (b - c[j]));
                hi = hi.min(g + k * (b - c[j]));
            }
            // No solution: halfway between what each end asks.
            if lo > hi {
                0.5 * (lo + hi)
            } else {
                clamp(x, lo, hi)
            }
        })
        .collect();
    let mut f = vec![0.0; m];
    f[0] = q[0];
    for j in 1..m {
        let r = k * (c[j] - c[j - 1]);
        f[j] = clamp(q[j], f[j - 1] - r, f[j - 1] + r);
    }
    let mut r = vec![0.0; m];
    r[m - 1] = q[m - 1];
    for j in (0..m - 1).rev() {
        let w = k * (c[j + 1] - c[j]);
        r[j] = clamp(q[j], r[j + 1] - w, r[j + 1] + w);
    }
    let mut g: Vec<f32> = f.iter().zip(&r).map(|(x, z)| 0.5 * (x + z)).collect();
    // What the far end misses by, given back as a bump of grade `t (1 - t)`.
    let rise: f32 = (0..m).map(|j| g[j] * d[j]).sum();
    let bump = |j: usize| {
        let t = (c[j] - a) / (b - a);
        t * (1.0 - t)
    };
    let area: f32 = (0..m).map(|j| bump(j) * d[j]).sum();
    if area > 0.0 {
        let scale = (y[m] - y[0] - rise) / area;
        for (j, g) in g.iter_mut().enumerate() {
            *g += scale * bump(j);
        }
    }
    let mut out = Vec::with_capacity(n);
    let mut h = y[0];
    out.push(h);
    for j in 0..m {
        h += g[j] * d[j];
        out.push(h);
    }
    out[m] = y[m];
    out
}

/// The grade of `y` at sample `i`, across the samples within `reach` meters either side.
pub(crate) fn grade_at(y: &[f32], s: &[f32], i: usize, reach: f32) -> f32 {
    let (mut lo, mut hi) = (i, i);
    while lo > 0 && s[i] - s[lo] < reach {
        lo -= 1;
    }
    while hi + 1 < y.len() && s[hi] - s[i] < reach {
        hi += 1;
    }
    if hi == lo {
        return 0.0;
    }
    (y[hi] - y[lo]) / (s[hi] - s[lo]).max(1e-3)
}

/// A profile close to `target` (heights at arc lengths `s`) whose slope never exceeds
/// `g`, starting at `a` and ending at `b` when given (else at the target's ends), and kept
/// within `band` of the target where the slope bound allows. The average of a forward and
/// a backward pass, each clamping to the reachable band from both pinned ends.
pub(crate) fn grade_limit(
    target: &[f32],
    s: &[f32],
    g: f32,
    a: Option<f32>,
    b: Option<f32>,
    band: Option<(&[f32], f32)>,
) -> Vec<f32> {
    let n = target.len();
    if n == 0 {
        return Vec::new();
    }
    let a = a.unwrap_or(target[0]);
    let b = b.unwrap_or(target[n - 1]);
    if n == 1 {
        return vec![a];
    }
    let len = s[n - 1] - s[0];
    if abs(a - b) > g * len {
        // No profile can join the ends within the bound: a straight ramp.
        return (0..n)
            .map(|i| lerp(a, b, if len > 0.0 { (s[i] - s[0]) / len } else { 0.0 }))
            .collect();
    }
    let v: Vec<f32> = (0..n)
        .map(|i| {
            let si = s[i] - s[0];
            let mut x = target[i];
            if let Some((ground, r)) = band {
                x = clamp(x, ground[i] - r, ground[i] + r);
            }
            let lo = (a - g * si).max(b - g * (len - si));
            let hi = (a + g * si).min(b + g * (len - si));
            clamp(x, lo, hi.max(lo))
        })
        .collect();
    let mut f = vec![0.0; n];
    f[0] = a;
    for i in 1..n {
        let d = g * (s[i] - s[i - 1]);
        f[i] = clamp(v[i], f[i - 1] - d, f[i - 1] + d);
    }
    let mut r = vec![0.0; n];
    r[n - 1] = b;
    for i in (0..n - 1).rev() {
        let d = g * (s[i + 1] - s[i]);
        r[i] = clamp(v[i], r[i + 1] - d, r[i + 1] + d);
    }
    let mut y: Vec<f32> = f.iter().zip(&r).map(|(x, z)| 0.5 * (x + z)).collect();
    y[0] = a;
    y[n - 1] = b;
    y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::abs;

    /// The most the grade changes per meter anywhere along `y`.
    fn bend(y: &[f32], s: &[f32]) -> f32 {
        (1..y.len() - 1)
            .map(|i| {
                let (d0, d1) = (s[i] - s[i - 1], s[i + 1] - s[i]);
                abs((y[i + 1] - y[i]) / d1 - (y[i] - y[i - 1]) / d0) / (0.5 * (d0 + d1))
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_crest_is_rounded_and_the_ends_hold() {
        // Up at 10%, then down at 10%: a 20% kink at the top.
        let s: Vec<f32> = (0..60).map(|i| i as f32 * 8.0).collect();
        let y: Vec<f32> = s.iter().map(|&x| 0.1 * (240.0 - abs(x - 240.0))).collect();
        let e = ease(&y, &s, ROAD_CURVE, None, None);
        assert_eq!((e[0], e[59]), (y[0], y[59]));
        assert!(bend(&e, &s) < ROAD_CURVE * 1.05, "{}", bend(&e, &s));
        // The crest came down, the foot of the hill did not move far.
        assert!(e[30] < 23.0 && abs(e[5] - 4.0) < 1.0, "{} {}", e[30], e[5]);
        for i in 1..e.len() {
            assert!(abs(e[i] - e[i - 1]) <= 0.105 * 8.0);
        }
    }

    #[test]
    fn a_smooth_profile_is_left_alone() {
        let s: Vec<f32> = (0..40).map(|i| i as f32 * 10.0).collect();
        let y: Vec<f32> = s.iter().map(|&x| 0.0005 * x * x).collect();
        let e = ease(&y, &s, ROAD_CURVE, None, None);
        for i in 0..40 {
            assert!(abs(e[i] - y[i]) < 1e-2, "{} {}", e[i], y[i]);
        }
    }

    #[test]
    fn two_stretches_that_share_a_grade_meet_without_a_kink() {
        // A junction 3 m up in the middle of a flat road, crossed level.
        let s: Vec<f32> = (0..21).map(|i| i as f32 * 8.0).collect();
        let up: Vec<f32> = (0..21).map(|i| 3.0 * i as f32 / 20.0).collect();
        let down: Vec<f32> = up.iter().rev().copied().collect();
        let a = ease(&up, &s, ROAD_CURVE, None, Some(0.0));
        let b = ease(&down, &s, ROAD_CURVE, Some(0.0), None);
        assert_eq!((a[20], b[0]), (3.0, 3.0));
        let whole: Vec<f32> = a.iter().chain(&b[1..]).copied().collect();
        let arc: Vec<f32> = (0..41).map(|i| i as f32 * 8.0).collect();
        assert!(
            bend(&whole, &arc) < ROAD_CURVE * 1.3,
            "{}",
            bend(&whole, &arc)
        );
    }

    #[test]
    fn grade_limit_holds_slope_and_ends() {
        let s: Vec<f32> = (0..40).map(|i| i as f32 * 16.0).collect();
        let target: Vec<f32> = s
            .iter()
            .map(|&x| if x > 300.0 { 20.0 } else { 0.0 })
            .collect();
        let y = grade_limit(&target, &s, 0.05, Some(0.0), None, None);
        assert_eq!(y[0], 0.0);
        assert_eq!(*y.last().unwrap(), 20.0);
        for k in 1..y.len() {
            assert!(abs(y[k] - y[k - 1]) <= 0.05 * 16.0 + 1e-4);
        }
    }
}
