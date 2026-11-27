//! Zone map helpers: the zone table and the chamfer distance transform over 16 m cells.

use place::ZoneId;

use super::ZONE_CELL_M;
use crate::math::sqrt;

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

/// World position of the center of zone cell `(cx, cz)`.
pub fn cell_center(cx: usize, cz: usize) -> (f32, f32) {
    (
        (cx as f32 + 0.5) * ZONE_CELL_M,
        (cz as f32 + 0.5) * ZONE_CELL_M,
    )
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
    use alloc::vec;

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
}
