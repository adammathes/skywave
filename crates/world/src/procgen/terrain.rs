//! Stage 1: shoreline and heightfield.
//!
//! Terrain is built in the *canonical frame* of [`super::Frame`]: `u` runs inland from the
//! water edge of the place, `v` along the coast. Height is a profile of the distance inland
//! from the shoreline (sloping sea floor, beach, rising plain) plus hills: ridges roughly
//! parallel to the coast, domain-warped and broken up by ridged noise, with flat valley
//! floors cut into them. Smooth parts are
//! evaluated on a 16 m lattice and interpolated; only the profile and fine relief are
//! evaluated per 4 m sample.

use alloc::vec;
use alloc::vec::Vec;

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use skywave_core::{hash64, Rng, Seed};

use super::{stage, Frame, CELL_M};
use crate::math::{clamp, lerp, segment_distance, sqrt, V2};
use crate::template::RegionTemplate;

/// Fractal OpenSimplex2 noise in `[-1, 1]` keyed by `hash64(seed, key)`.
pub struct Noise(FastNoiseLite);

impl Noise {
    /// Fractal Brownian motion with features about `feature_m` across.
    pub fn fbm(seed: Seed, key: &[u64], feature_m: f32, octaves: i32) -> Noise {
        Noise::new(seed, key, feature_m, octaves, FractalType::FBm)
    }

    /// Ridged multifractal: sharp crests near 1, broad troughs.
    pub fn ridged(seed: Seed, key: &[u64], feature_m: f32, octaves: i32) -> Noise {
        Noise::new(seed, key, feature_m, octaves, FractalType::Ridged)
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

/// Compact bump: 1 at 0, 0 for `|t| >= 1`, smooth everywhere.
pub fn bump(t: f32) -> f32 {
    if !(-1.0..1.0).contains(&t) {
        return 0.0;
    }
    let a = 1.0 - t * t;
    a * a
}

/// A bay: the shoreline pushed inland by `depth` around `v`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bay {
    pub v: f32,
    pub depth: f32,
    pub half_width: f32,
}

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
    /// Draws the bays and shoreline for `nv` samples of `v` (stage [`stage::COAST`]).
    pub fn generate(seed: Seed, t: &RegionTemplate, nv: u32, len_v: f32) -> Shore {
        let c = &t.coast;
        let mut rng = Rng::new(seed, &[stage::COAST]);
        let offset = rng.range_f32(c.offset_m.0, c.offset_m.1);
        let n_bays = rng.range(u32::from(c.bays.0), u32::from(c.bays.1) + 1);
        let mut bays: Vec<Bay> = Vec::new();
        for i in 0..n_bays {
            let half_width = 0.5 * rng.range_f32(c.bay_width_m.0, c.bay_width_m.1);
            let depth = rng.range_f32(c.bay_depth_m.0, c.bay_depth_m.1);
            // The main bay sits near the middle; the others anywhere they do not overlap it.
            let mut v = if i == 0 {
                len_v * rng.range_f32(0.4, 0.6)
            } else {
                rng.range_f32(0.0, len_v)
            };
            if i > 0 {
                let mut ok = false;
                for _ in 0..16 {
                    if bays
                        .iter()
                        .all(|b| abs_f(b.v - v) > b.half_width + half_width * 0.6)
                    {
                        ok = true;
                        break;
                    }
                    v = rng.range_f32(0.0, len_v);
                }
                if !ok {
                    continue;
                }
                // Secondary bays are smaller.
                bays.push(Bay {
                    v,
                    depth: depth * 0.6,
                    half_width: half_width * 0.7,
                });
            } else {
                bays.push(Bay {
                    v,
                    depth,
                    half_width,
                });
            }
        }

        let wiggle = Noise::fbm(seed, &[stage::COAST, 1], 700.0, 3);
        let beach_noise = Noise::fbm(seed, &[stage::COAST, 2], 350.0, 2);
        let n = nv as usize;
        let mut u = vec![0.0; n];
        let mut beach = vec![0.0; n];
        let (b0, b1) = t.elevation.beach_width_m;
        for (i, (su, sb)) in u.iter_mut().zip(beach.iter_mut()).enumerate() {
            let v = i as f32 * CELL_M;
            let mut s = offset + c.wiggle_m * wiggle.at(v, 0.0);
            for b in &bays {
                s += b.depth * bump((v - b.v) / b.half_width);
            }
            *su = s.max(CELL_M);
            *sb = lerp(b0, b1, 0.5 + 0.5 * beach_noise.at(v, 17.0));
        }
        let k = slope_factors(&u);
        let mut road = box_blur(&u, n, 1, 50);
        road = box_blur(&road, n, 1, 50);
        let road_k = slope_factors(&road);
        Shore {
            u,
            k,
            beach,
            road,
            road_k,
            bays,
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

    /// Beach width at `v`.
    pub fn beach_at(&self, v: f32) -> f32 {
        Shore::lookup(&self.beach, v)
    }

    /// Signed distance inland from the shoreline (negative in the water), measured
    /// perpendicular to the local shore tangent.
    pub fn distance(&self, u: f32, v: f32) -> f32 {
        (u - self.shore_u(v)) * Shore::lookup(&self.k, v)
    }

    /// The canonical point `d` meters inland of the shoreline at `v`, along the shore normal.
    pub fn point(&self, v: f32, d: f32) -> (f32, f32) {
        offset_point(&self.u, v, d)
    }

    /// Unit inland normal at `v`, as `(du, dv)`.
    pub fn normal(&self, v: f32) -> (f32, f32) {
        normal(&self.u, v)
    }

    /// Smoothed shoreline `u` at `v`; see [`Shore::road`].
    pub fn road_u(&self, v: f32) -> f32 {
        Shore::lookup(&self.road, v)
    }

    /// [`Shore::distance`] from the smoothed shoreline.
    pub fn road_distance(&self, u: f32, v: f32) -> f32 {
        (u - self.road_u(v)) * Shore::lookup(&self.road_k, v)
    }

    /// [`Shore::point`] off the smoothed shoreline.
    pub fn road_point(&self, v: f32, d: f32) -> (f32, f32) {
        offset_point(&self.road, v, d)
    }
}

/// `1 / sqrt(1 + (du/dv)^2)` per sample of a line `u(v)`, by central differences.
fn slope_factors(u: &[f32]) -> Vec<f32> {
    let n = u.len();
    (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let du = (u[b] - u[a]) / ((b - a).max(1) as f32 * CELL_M);
            1.0 / sqrt(1.0 + du * du)
        })
        .collect()
}

fn normal(line: &[f32], v: f32) -> (f32, f32) {
    let s0 = Shore::lookup(line, v - CELL_M);
    let s1 = Shore::lookup(line, v + CELL_M);
    let slope = (s1 - s0) / (2.0 * CELL_M);
    let k = 1.0 / sqrt(1.0 + slope * slope);
    (k, -slope * k)
}

fn offset_point(line: &[f32], v: f32, d: f32) -> (f32, f32) {
    let (du, dv) = normal(line, v);
    (Shore::lookup(line, v) + d * du, v + d * dv)
}

fn abs_f(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// A ridge crest at hills-distance `at` (meters inland of the plain's edge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ridge {
    pub at: f32,
    pub height: f32,
    pub half_width: f32,
}

/// An elliptical, flat-floored valley in canonical coordinates, long axis along `u`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Valley {
    pub u: f32,
    pub v: f32,
    pub half_length: f32,
    pub half_width: f32,
    /// Absolute floor height at the seaward and inland ends.
    pub floor: (f32, f32),
}

/// Everything the heightfield needs, drawn by the coast, terrain and layout stages.
pub struct TerrainParams<'a> {
    pub shore: &'a Shore,
    pub water_level: f32,
    pub shelf_depth: f32,
    pub beach_height: f32,
    pub plain_width: f32,
    pub plain_rise: f32,
    pub relief: f32,
    pub warp: f32,
    pub ridges: &'a [Ridge],
    pub valleys: &'a [Valley],
    pub bounds: (f32, f32),
}

impl TerrainParams<'_> {
    /// Height of the plain's inland edge.
    pub fn plain_top(&self) -> f32 {
        self.water_level + self.beach_height + self.plain_rise
    }

    /// The coast profile at distance `d` inland, beach width `bw`.
    fn profile(&self, d: f32, bw: f32) -> f32 {
        let wl = self.water_level;
        if d < 0.0 {
            return wl - (0.6 + 0.035 * -d).min(self.shelf_depth);
        }
        if d < bw {
            return wl + 0.05 + (self.beach_height - 0.05) * (d / bw);
        }
        let t = (d - bw) / self.plain_width;
        let rise = if t < 1.0 {
            self.plain_rise * t * (0.4 + 0.6 * t)
        } else {
            self.plain_rise * (1.0 + 1.6 * (t - 1.0))
        };
        wl + self.beach_height + rise
    }
}

/// Smooth fields on the 16 m lattice, interpolated per sample.
struct Lattice {
    w: usize,
    hills: Vec<f32>,
    relief: Vec<f32>,
    valley: Vec<f32>,
    floor: Vec<f32>,
}

impl Lattice {
    /// Bilinear sample of all four fields at sample `(iu, iv)`; 4 samples per node.
    fn at(&self, iu: usize, iv: usize) -> [f32; 4] {
        let (i, j) = (iu / 4, iv / 4);
        let (fu, fv) = ((iu % 4) as f32 * 0.25, (iv % 4) as f32 * 0.25);
        let a = j * self.w + i;
        let idx = [a, a + 1, a + self.w, a + self.w + 1];
        let mix = |f: &[f32]| {
            let top = lerp(f[idx[0]], f[idx[1]], fu);
            let bot = lerp(f[idx[2]], f[idx[3]], fu);
            lerp(top, bot, fv)
        };
        [
            mix(&self.hills),
            mix(&self.relief),
            mix(&self.valley),
            mix(&self.floor),
        ]
    }
}

/// The heightfield in world sample order (`z` major, `x` minor), stage [`stage::TERRAIN`].
pub fn heights(seed: Seed, frame: &Frame, p: &TerrainParams<'_>) -> Vec<f32> {
    let (nu, nv) = (frame.n_u() as usize, frame.n_v() as usize);
    let lattice = lattice(seed, nu, nv, p);
    let fine = Noise::fbm(seed, &[stage::TERRAIN, 10], 40.0, 1);
    let mut out = vec![0.0; nu * nv];
    for iv in 0..nv {
        let v = iv as f32 * CELL_M;
        let s = p.shore.u[iv];
        let k = p.shore.k[iv];
        let bw = p.shore.beach[iv];
        for iu in 0..nu {
            let u = iu as f32 * CELL_M;
            let d = (u - s) * k;
            let [hills, relief, valley, floor] = lattice.at(iu, iv);
            let land = smoothstep(bw, bw + 120.0, d);
            let mut h = p.profile(d, bw) + land * (relief + hills);
            if valley > 0.0 {
                h = lerp(h, floor + 0.3 * land * relief, valley * land);
            }
            let fine_amp = 0.3 + 0.012 * hills.max(0.0);
            h += land * fine_amp * fine.at(u, v);
            out[frame.index(iu as u32, iv as u32)] = clamp(h, p.bounds.0, p.bounds.1);
        }
    }
    out
}

fn lattice(seed: Seed, nu: usize, nv: usize, p: &TerrainParams<'_>) -> Lattice {
    let w = (nu - 1) / 4 + 2;
    let h = (nv - 1) / 4 + 2;
    let warp_u = Noise::fbm(seed, &[stage::TERRAIN, 1], 900.0, 3);
    let warp_v = Noise::fbm(seed, &[stage::TERRAIN, 2], 900.0, 3);
    let crest = Noise::ridged(seed, &[stage::TERRAIN, 3], 420.0, 4);
    let roll = Noise::fbm(seed, &[stage::TERRAIN, 4], 380.0, 3);
    let base_h = p.ridges.first().map_or(0.0, |r| 0.3 * r.height);
    let base = |dh: f32| base_h * smoothstep(-200.0, 500.0, dh);
    let mut out = Lattice {
        w,
        hills: vec![0.0; w * h],
        relief: vec![0.0; w * h],
        valley: vec![0.0; w * h],
        floor: vec![0.0; w * h],
    };
    let step = 4.0 * CELL_M;
    for j in 0..h {
        let v = j as f32 * step;
        let iv = (j * 4).min(nv - 1);
        let (s, k, bw) = (p.shore.u[iv], p.shore.k[iv], p.shore.beach[iv]);
        for i in 0..w {
            let u = i as f32 * step;
            let wu = u + p.warp * warp_u.at(u, v);
            let wv = v + p.warp * warp_v.at(u, v);
            let d = (u - s) * k;
            let dh = d - bw - p.plain_width + (wu - u);
            let mut hills = 0.0;
            if !p.ridges.is_empty() {
                let mut ridge = 0.0;
                for r in p.ridges {
                    ridge += (r.height - base(r.at)).max(0.0) * bump((dh - r.at) / r.half_width);
                }
                let detail = 0.6 + 0.4 * (0.5 + 0.5 * crest.at(wu, wv));
                hills = base(dh) + ridge * detail;
            }
            let mut valley = 0.0f32;
            let mut floor = 0.0;
            for vy in p.valleys {
                let a = (wu - vy.u) / vy.half_length;
                let b = (wv - vy.v) / vy.half_width;
                let m = 1.0 - smoothstep(0.55, 1.0, sqrt(a * a + b * b));
                if m > valley {
                    valley = m;
                    let t = clamp(0.5 + 0.5 * (u - vy.u) / vy.half_length, 0.0, 1.0);
                    floor = lerp(vy.floor.0, vy.floor.1, t);
                }
            }
            let at = j * w + i;
            out.hills[at] = hills;
            out.relief[at] = p.relief * roll.at(wu, wv);
            out.valley[at] = valley;
            out.floor[at] = floor;
        }
    }
    out
}

/// Steepest grade of the highway corridor's bed, a hair under the highway's 5% so the
/// arterial stage's own profile (which re-smooths what it samples) never has to cut.
pub const CORRIDOR_GRADE: f32 = 0.048;
/// Half-width of the flat corridor bed: the highway's 22 m plus 4 m shoulders.
pub const CORRIDOR_HALF_M: f32 = 15.0;
/// Width beyond the bed over which the cut and fill slopes blend back into the terrain.
pub const CORRIDOR_BLEND_M: f32 = 60.0;
/// Rise over run of the corridor's cut and fill slopes (1:2).
pub const CORRIDOR_SLOPE: f32 = 0.5;
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

/// The highway corridor's bed height at every point of `line` (world `[x, z]`, the macro
/// highway from the coast inland): the largest profile under the (smoothed) terrain that
/// starts on the ground at the coast and never climbs or falls faster than
/// [`CORRIDOR_GRADE`], with rounded vertical curves. Never above the terrain by more than
/// the rounding, so the highway runs in cuts, not on embankments.
pub fn corridor_bed(heights: &[f32], w: usize, h: usize, line: &[[f32; 2]], wl: f32) -> Vec<f32> {
    let n = line.len();
    if n == 0 {
        return Vec::new();
    }
    let mut s = vec![0.0f32; n];
    for i in 1..n {
        let (dx, dz) = (line[i][0] - line[i - 1][0], line[i][1] - line[i - 1][1]);
        s[i] = s[i - 1] + sqrt(dx * dx + dz * dz);
    }
    let mut t: Vec<f32> = line
        .iter()
        .map(|p| grid_height(heights, w, h, p[0], p[1]))
        .collect();
    // Relief does not decide the bed: three passes of a 5-tap mean (about ±40 m).
    for _ in 0..3 {
        let prev = t.clone();
        for (i, ti) in t.iter_mut().enumerate() {
            let (a, b) = (i.saturating_sub(2), (i + 2).min(n - 1));
            let mut acc = 0.0;
            for x in &prev[a..=b] {
                acc += x;
            }
            *ti = acc / (b - a + 1) as f32;
        }
    }
    // The largest CORRIDOR_GRADE-Lipschitz minorant: a forward and a backward sweep.
    let mut bed = t.clone();
    for i in 1..n {
        bed[i] = bed[i].min(bed[i - 1] + CORRIDOR_GRADE * (s[i] - s[i - 1]));
    }
    for i in (0..n - 1).rev() {
        bed[i] = bed[i].min(bed[i + 1] + CORRIDOR_GRADE * (s[i + 1] - s[i]));
    }
    // Vertical curves: 3-tap means keep the grade bound; the ends stay put.
    for _ in 0..4 {
        let prev = bed.clone();
        for i in 1..n.saturating_sub(1) {
            bed[i] = (prev[i - 1] + prev[i] + prev[i + 1]) / 3.0;
        }
    }
    for b in &mut bed {
        *b = b.max(wl + 1.0);
    }
    bed
}

/// Carves the corridor into `heights` (world sample order, `w × h`): flat at the bed within
/// [`CORRIDOR_HALF_M`] of `line`, then cut and fill slopes of [`CORRIDOR_SLOPE`] blended
/// back into the terrain over [`CORRIDOR_BLEND_M`]. Each sample takes its nearest segment
/// (earlier segments win ties); samples under water stay. Returns how far each sample
/// moved (carved minus before; negative in cuts).
pub fn carve_corridor(
    heights: &mut [f32],
    w: usize,
    h: usize,
    line: &[[f32; 2]],
    bed: &[f32],
    wl: f32,
) -> Vec<f32> {
    let mut moved = vec![0.0f32; w * h];
    if line.len() < 2 || bed.len() != line.len() {
        return moved;
    }
    let reach = CORRIDOR_HALF_M + CORRIDOR_BLEND_M;
    let mut best = vec![f32::INFINITY; w * h];
    let mut ybed = vec![0.0f32; w * h];
    for k in 0..line.len() - 1 {
        let (pa, pb) = (V2::from_array(line[k]), V2::from_array(line[k + 1]));
        let span = |a: f32, b: f32, n: usize| {
            let lo = clamp((a.min(b) - reach) / CELL_M, 0.0, (n - 1) as f32) as usize;
            let hi = clamp((a.max(b) + reach) / CELL_M + 1.0, 0.0, (n - 1) as f32) as usize;
            (lo, hi)
        };
        let (x0, x1) = span(pa.x, pb.x, w);
        let (z0, z1) = span(pa.y, pb.y, h);
        for iz in z0..=z1 {
            for ix in x0..=x1 {
                let q = V2::new(ix as f32 * CELL_M, iz as f32 * CELL_M);
                let (d, t) = segment_distance(q, pa, pb);
                let i = iz * w + ix;
                if d < reach && d < best[i] {
                    best[i] = d;
                    ybed[i] = lerp(bed[k], bed[k + 1], t);
                }
            }
        }
    }
    for i in 0..w * h {
        let d = best[i];
        if !d.is_finite() || heights[i] < wl {
            continue;
        }
        let (h0, y) = (heights[i], ybed[i]);
        let carved = if d <= CORRIDOR_HALF_M {
            y
        } else {
            let slack = (d - CORRIDOR_HALF_M) * CORRIDOR_SLOPE;
            let sloped = clamp(h0, y - slack, y + slack);
            let blend = smoothstep(CORRIDOR_HALF_M, reach, d);
            sloped + (h0 - sloped) * blend
        };
        heights[i] = carved;
        moved[i] = carved - h0;
    }
    moved
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
    fn steps_and_bumps() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
        assert_eq!(smoothstep(1.0, 0.0, 0.0), 1.0);
        assert_eq!(bump(0.0), 1.0);
        assert_eq!(bump(1.0), 0.0);
        assert_eq!(bump(-2.0), 0.0);
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
