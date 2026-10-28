//! Stage 2: the zone map, painted by weighted rules on 16 m cells, then smoothed.
//!
//! Each land cell gets every scored zone's [`Score`](crate::template::Score) from four
//! inputs (distance inland from the shore, slope, distance from the downtown seed, distance
//! from the highway; in a basin also the hills, the rail line and the arterials) plus that zone's own noise; the highest total wins, ties to the lower
//! `ZoneId`. A majority filter removes speckle. Structural zones (beach, strip, highway,
//! parks, canals) are painted on top by [`super::macro_map`].

use alloc::vec;
use alloc::vec::Vec;

use place::ZoneId;
use skywave_core::Seed;

use super::terrain::Noise;
use super::{stage, ZONE_CELL_M};
use crate::math::sqrt;
use crate::template::{curve_at, RegionTemplate, LAND_ZONES};

/// Every `ZoneId`, indexed by its discriminant.
pub const ALL_ZONES: [ZoneId; 12] = [
    ZoneId::Downtown,
    ZoneId::Strip,
    ZoneId::Residential,
    ZoneId::Suburb,
    ZoneId::Industrial,
    ZoneId::Commercial,
    ZoneId::Park,
    ZoneId::Beach,
    ZoneId::Farmland,
    ZoneId::Hills,
    ZoneId::Highway,
    ZoneId::Water,
];

/// Per-cell inputs to the zone rules, row-major over the zone grid.
pub struct ZoneInputs {
    pub w: usize,
    pub h: usize,
    /// Distance inland from the shoreline, meters.
    pub coast: Vec<f32>,
    /// Terrain slope, rise over run.
    pub slope: Vec<f32>,
    /// Distance from the downtown seed, meters.
    pub downtown: Vec<f32>,
    /// Distance from the highway centerline, meters.
    pub highway: Vec<f32>,
    /// Cells under water; they stay `Water`.
    pub water: Vec<bool>,
    /// An inland basin's inputs per cell, empty elsewhere: distance out from the foot of
    /// the hills, from the rail line, from the nearest arterial and from the nearest
    /// boulevard, meters (the scores' `hills`, `rail`, `arterial` and `boulevard`).
    pub basin: Vec<[f32; 4]>,
}

/// Scores every land cell and returns the winning zone per cell (`Water` for water).
pub fn score(seed: Seed, t: &RegionTemplate, inputs: &ZoneInputs) -> Vec<ZoneId> {
    let scored: Vec<(ZoneId, &crate::template::Score, Noise)> = LAND_ZONES
        .iter()
        .filter_map(|&z| {
            let s = t.zones.get(z)?.score.as_ref()?;
            let noise = Noise::fbm(seed, &[stage::ZONES, z as u64], s.noise_scale_m, 2);
            Some((z, s, noise))
        })
        .collect();
    let mut out = vec![ZoneId::Water; inputs.w * inputs.h];
    for cz in 0..inputs.h {
        for cx in 0..inputs.w {
            let i = cz * inputs.w + cx;
            if inputs.water[i] {
                continue;
            }
            let (x, z) = cell_center(cx, cz);
            let mut best = (f32::NEG_INFINITY, ZoneId::Residential);
            for (zone, s, noise) in &scored {
                let mut total = s.base
                    + curve_at(&s.coast, inputs.coast[i])
                    + curve_at(&s.slope, inputs.slope[i])
                    + curve_at(&s.downtown, inputs.downtown[i])
                    + curve_at(&s.highway, inputs.highway[i])
                    + s.noise * noise.at(x, z);
                if let Some(b) = inputs.basin.get(i) {
                    total += curve_at(&s.hills, b[0])
                        + curve_at(&s.rail, b[1])
                        + curve_at(&s.arterial, b[2])
                        + curve_at(&s.boulevard, b[3]);
                }
                if total > best.0 {
                    best = (total, *zone);
                }
            }
            out[i] = best.1;
        }
    }
    out
}

/// World position of the center of zone cell `(cx, cz)`.
pub fn cell_center(cx: usize, cz: usize) -> (f32, f32) {
    (
        (cx as f32 + 0.5) * ZONE_CELL_M,
        (cz as f32 + 0.5) * ZONE_CELL_M,
    )
}

/// One pass of a 3×3 majority filter over cells for which `mutable` holds. Only mutable
/// neighbors vote; ties keep the cell's own zone, then the lower `ZoneId`.
pub fn majority(
    zones: &[ZoneId],
    w: usize,
    h: usize,
    mutable: impl Fn(ZoneId) -> bool,
) -> Vec<ZoneId> {
    let mut out = zones.to_vec();
    for cz in 0..h {
        for cx in 0..w {
            let own = zones[cz * w + cx];
            if !mutable(own) {
                continue;
            }
            let mut votes = [0u8; 12];
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (x, z) = (cx as i32 + dx, cz as i32 + dz);
                    if x < 0 || z < 0 || x >= w as i32 || z >= h as i32 {
                        continue;
                    }
                    let n = zones[z as usize * w + x as usize];
                    if mutable(n) {
                        votes[n as usize] += 1;
                    }
                }
            }
            let mut best = own;
            for (k, &c) in votes.iter().enumerate() {
                if c > votes[best as usize] {
                    best = ALL_ZONES[k];
                }
            }
            out[cz * w + cx] = best;
        }
    }
    out
}

/// Two-pass 8-neighbor chamfer distance transform in place. Cells holding 0 are sources,
/// cells holding `f32::INFINITY` get the approximate distance to the nearest source, in
/// units of `cell` (exact along rows, columns and diagonals, within 8% elsewhere).
pub fn chamfer(dist: &mut [f32], w: usize, h: usize, cell: f32) {
    let diag = cell * sqrt(2.0);
    let fwd: [(isize, isize, f32); 4] =
        [(-1, 0, cell), (-1, -1, diag), (0, -1, cell), (1, -1, diag)];
    for z in 0..h {
        for x in 0..w {
            relax(dist, w, h, x, z, &fwd);
        }
    }
    let back: [(isize, isize, f32); 4] = [(1, 0, cell), (1, 1, diag), (0, 1, cell), (-1, 1, diag)];
    for z in (0..h).rev() {
        for x in (0..w).rev() {
            relax(dist, w, h, x, z, &back);
        }
    }
}

fn relax(dist: &mut [f32], w: usize, h: usize, x: usize, z: usize, n: &[(isize, isize, f32); 4]) {
    let mut best = dist[z * w + x];
    for &(dx, dz, c) in n {
        let (nx, nz) = (x as isize + dx, z as isize + dz);
        if nx < 0 || nz < 0 || nx >= w as isize || nz >= h as isize {
            continue;
        }
        let d = dist[nz as usize * w + nx as usize] + c;
        if d < best {
            best = d;
        }
    }
    dist[z * w + x] = best;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_table_matches_discriminants() {
        for (i, z) in ALL_ZONES.iter().enumerate() {
            assert_eq!(*z as usize, i);
        }
    }

    #[test]
    fn chamfer_measures_rows_and_diagonals() {
        let (w, h) = (7, 5);
        let mut d = vec![f32::INFINITY; w * h];
        d[0] = 0.0;
        chamfer(&mut d, w, h, 16.0);
        assert_eq!(d[6], 96.0);
        assert!((d[4 * w + 4] - 4.0 * 16.0 * sqrt(2.0)).abs() < 1e-3);
    }

    #[test]
    fn majority_removes_speckle() {
        let (w, h) = (5, 5);
        let mut z = vec![ZoneId::Residential; w * h];
        z[12] = ZoneId::Industrial;
        z[0] = ZoneId::Water;
        let out = majority(&z, w, h, |q| q != ZoneId::Water);
        assert_eq!(out[12], ZoneId::Residential);
        assert_eq!(out[0], ZoneId::Water);
    }
}
