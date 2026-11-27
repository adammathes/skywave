//! Heightfield helpers shared by the layouts: smoothstep, bilinear sampling and the box blur.

use alloc::vec;
use alloc::vec::Vec;

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use skywave_core::{hash64, Seed};

use super::CELL_M;
use crate::math::{clamp, lerp};

/// Fractal OpenSimplex2 noise in `[-1, 1]` keyed by `hash64(seed, key)`.
pub struct Noise(FastNoiseLite);

impl Noise {
    /// Fractal Brownian motion with features about `feature_m` across.
    pub fn fbm(seed: Seed, key: &[u64], feature_m: f32, octaves: i32) -> Noise {
        Noise::new(seed, key, feature_m, octaves, FractalType::FBm)
    }

    fn new(seed: Seed, key: &[u64], feature_m: f32, octaves: i32, kind: FractalType) -> Noise {
        let mut n = FastNoiseLite::with_seed(hash64(seed.0, key) as u32 as i32);
        n.set_noise_type(Some(NoiseType::OpenSimplex2));
        n.set_frequency(Some(1.0 / feature_m));
        n.set_fractal_type(Some(if octaves > 1 { kind } else { FractalType::None }));
        n.set_fractal_octaves(Some(octaves));
        Noise(n)
    }

    pub fn at(&self, x: f32, y: f32) -> f32 {
        self.0.get_noise_2d(x, y)
    }
}

/// Hermite step from 0 at `e0` to 1 at `e1` (either order).
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A bay: the shoreline pushed inland by `depth` around `v`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bay {
    pub v: f32,
    pub depth: f32,
    pub half_width: f32,
}

/// How far out past an inland place [`Shore::far`] puts its shoreline.
pub const FAR_SHORE_M: f32 = 100_000.0;

/// The shoreline in the canonical frame, sampled at every height-sample row of `v`.
#[derive(Clone, Debug, PartialEq)]
pub struct Shore {
    /// Shoreline `u` per `v` sample.
    pub u: Vec<f32>,
    /// `1 / sqrt(1 + (du/dv)^2)`: converts `u - shoreline` into distance from the shore.
    pub k: Vec<f32>,
    /// Beach width per `v` sample.
    pub beach: Vec<f32>,
    /// The shoreline smoothed along the coast over ~400 m: what the strip and other
    /// shore-parallel features are laid off, so they sweep across a bay's apex instead of
    /// folding into a corner behind it.
    pub road: Vec<f32>,
    /// [`Shore::k`] for `road`.
    pub road_k: Vec<f32>,
    /// Bays, the main one first.
    pub bays: Vec<Bay>,
}

impl Shore {
    /// No shoreline: a "shore" [`FAR_SHORE_M`] out past the place's first side, so every
    /// point is far inland and nothing is beach (an inland basin).
    pub fn far(nv: u32) -> Shore {
        let n = nv as usize;
        Shore {
            u: vec![-FAR_SHORE_M; n],
            k: vec![1.0; n],
            beach: vec![0.0; n],
            road: vec![-FAR_SHORE_M; n],
            road_k: vec![1.0; n],
            bays: Vec::new(),
        }
    }

    fn lookup(values: &[f32], v: f32) -> f32 {
        let x = clamp(v / CELL_M, 0.0, (values.len() - 1) as f32);
        let i = (x as usize).min(values.len().saturating_sub(2));
        let f = x - i as f32;
        if i + 1 < values.len() {
            lerp(values[i], values[i + 1], f)
        } else {
            values[i]
        }
    }

    /// Shoreline `u` at `v`.
    pub fn shore_u(&self, v: f32) -> f32 {
        Shore::lookup(&self.u, v)
    }

    /// Signed distance inland from the shoreline (negative in the water), measured
    /// perpendicular to the local shore tangent.
    pub fn distance(&self, u: f32, v: f32) -> f32 {
        (u - self.shore_u(v)) * Shore::lookup(&self.k, v)
    }

    /// Smoothed shoreline `u` at `v`; see [`Shore::road`].
    pub fn road_u(&self, v: f32) -> f32 {
        Shore::lookup(&self.road, v)
    }
}

/// Bilinear height of a `w × h` grid of samples [`CELL_M`] apart at world `(x, z)`,
/// clamped to the grid.
pub fn grid_height(heights: &[f32], w: usize, h: usize, x: f32, z: f32) -> f32 {
    let fx = clamp(x / CELL_M, 0.0, (w - 1) as f32);
    let fz = clamp(z / CELL_M, 0.0, (h - 1) as f32);
    let (ix, iz) = ((fx as usize).min(w - 2), (fz as usize).min(h - 2));
    let (tx, tz) = (fx - ix as f32, fz - iz as f32);
    let at = |i: usize, j: usize| heights[j * w + i];
    let a = lerp(at(ix, iz), at(ix + 1, iz), tx);
    let b = lerp(at(ix, iz + 1), at(ix + 1, iz + 1), tx);
    lerp(a, b, tz)
}

/// One pass of a `(2r+1)²` box blur with clamped edges, as two running-sum sweeps (both
/// walk memory in order).
pub fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut tmp = vec![0.0; src.len()];
    for (row, out) in src.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
        let mut acc = row[0] * r as f32;
        for i in 0..=r {
            acc += row[i.min(w - 1)];
        }
        // Left edge, interior, right edge: only the edges clamp.
        let lead = r.min(w);
        for x in 0..w {
            out[x] = acc * norm;
            let add = if x + r + 1 < w {
                row[x + r + 1]
            } else {
                row[w - 1]
            };
            let sub = if x >= lead { row[x - r] } else { row[0] };
            acc += add - sub;
        }
    }
    let mut out = vec![0.0; src.len()];
    let mut acc = vec![0.0f32; w];
    for (x, a) in acc.iter_mut().enumerate() {
        *a = tmp[x] * r as f32;
        for j in 0..=r {
            *a += tmp[j.min(h - 1) * w + x];
        }
    }
    for (z, row) in out.chunks_exact_mut(w).enumerate() {
        let add = (z + r + 1).min(h - 1) * w;
        let sub = z.saturating_sub(r) * w;
        let (add, sub) = (&tmp[add..add + w], &tmp[sub..sub + w]);
        for (((o, a), p), m) in row.iter_mut().zip(acc.iter_mut()).zip(add).zip(sub) {
            *o = *a * norm;
            *a += p - m;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
        assert_eq!(smoothstep(1.0, 0.0, 0.0), 1.0);
    }

    #[test]
    fn box_blur_keeps_constants_and_mass_center() {
        let w = 9;
        let src = vec![2.0; w * w];
        assert!(box_blur(&src, w, w, 2)
            .iter()
            .all(|&x| (x - 2.0).abs() < 1e-6));
        let mut spike = vec![0.0; w * w];
        spike[4 * w + 4] = 25.0;
        let b = box_blur(&spike, w, w, 2);
        assert!((b[4 * w + 4] - 1.0).abs() < 1e-6);
        assert!((b[2 * w + 6] - 1.0).abs() < 1e-6);
        assert_eq!(b[w + 4], 0.0);
    }
}
