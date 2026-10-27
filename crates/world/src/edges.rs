//! Map edges (`docs/WORLD_GENERATION.md`, "Map edges"): what bounds a place on each side,
//! as pure functions of `(seed, side, kind, position)` shared by the macro map (the walls
//! inside the place) and the horizon mesh (what continues past it).
//!
//! A land side raises a **wall** in a band inside it: flat ground up to the wall's toe, a
//! face steeper than 1.5:1 that no car climbs, and a crest at or just inside the side, so
//! the place never ends in open ground. The band is roadless ([`band_m`]): streets end at
//! its foot, the highway turns back along it. Past the side, [`beyond`] carries the ground
//! on up into ridges (mountains, hills), out over a forest or rail yards, or down under the
//! water to the horizon.
//!
//! Sides are indexed as `place::Terrain::edges`: north (`-z`), east, south, west (`-x`).
//! `inside` is the distance in from a side, `out` the distance out past it, `along` the
//! coordinate along it (`x` for north and south, `z` for east and west).

use place::{EdgeKind, EAST, NORTH, SOUTH, WEST};
use skywave_core::hash64;

use crate::math::{abs, clamp, lerp, value_noise};

/// The wall a land edge raises: crest height above the ground at its toe, and the width
/// from toe to crest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wall {
    pub height_m: (f32, f32),
    pub ramp_m: f32,
}

/// How far in from the side the crest may wander.
pub const CREST_WANDER_M: f32 = 24.0;
/// Clear ground between the wall's toe and the nearest road.
pub const FOOT_M: f32 = 28.0;
/// The face rises over the middle of the ramp, between these fractions of it: steepest
/// grade `1.5 · height / (0.7 · ramp)`.
const FACE: (f32, f32) = (0.15, 0.85);
/// Hash key of the edge noise.
const KEY: u64 = 0x4544_4745;

/// The wall of a land `kind`; `None` for water and open sides.
pub fn wall(kind: EdgeKind) -> Option<Wall> {
    let (height_m, ramp_m) = match kind {
        EdgeKind::Mountains => ((90.0, 140.0), 96.0),
        EdgeKind::Hills | EdgeKind::HighwayLoop => ((55.0, 85.0), 60.0),
        // A wooded bluff, and a rail embankment.
        EdgeKind::Forest => ((20.0, 30.0), 24.0),
        EdgeKind::RailYards => ((7.0, 9.0), 9.0),
        EdgeKind::Open | EdgeKind::Ocean | EdgeKind::Lake | EdgeKind::Bay => return None,
    };
    Some(Wall { height_m, ramp_m })
}

/// Width of the roadless band inside a side of `kind`: the wall, its wander and the foot;
/// 0 for water and open sides.
pub fn band_m(kind: EdgeKind) -> f32 {
    wall(kind).map_or(0.0, |w| w.ramp_m + CREST_WANDER_M + FOOT_M)
}

/// The bands of `edges` (`[north, east, south, west]`).
pub fn bands(edges: &[EdgeKind; 4]) -> [f32; 4] {
    edges.map(band_m)
}

/// Distance in from each side to `p` (`[x, z]`) in a place `ext` meters across.
pub fn inside(p: [f32; 2], ext: [f32; 2]) -> [f32; 4] {
    let mut d = [0.0; 4];
    d[NORTH] = p[1];
    d[EAST] = ext[0] - p[0];
    d[SOUTH] = ext[1] - p[1];
    d[WEST] = p[0];
    d
}

/// The coordinate of `p` along `side`.
pub fn along(side: usize, p: [f32; 2]) -> f32 {
    if side == NORTH || side == SOUTH {
        p[0]
    } else {
        p[1]
    }
}

fn key(seed: u64, side: usize) -> u64 {
    hash64(seed, &[KEY, side as u64])
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Height the wall of `side` adds `inside` meters in from it, `along` meters along it: 0
/// past the toe, the full (noisy) height at the crest and beyond.
pub fn rise(seed: u64, side: usize, kind: EdgeKind, along: f32, inside: f32) -> f32 {
    let Some(w) = wall(kind) else {
        return 0.0;
    };
    let k = key(seed, side);
    let n = 0.65 * value_noise(k, along, 0.0, 280.0) + 0.35 * value_noise(k ^ 1, along, 0.0, 70.0);
    let height = lerp(w.height_m.0, w.height_m.1, n);
    let crest = CREST_WANDER_M * value_noise(k ^ 2, along, 0.0, 160.0);
    let t = 1.0 - (inside - crest) / w.ramp_m;
    height * smoothstep(FACE.0, FACE.1, t)
}

/// The crest height of the wall of `side` at `along` above its toe (what [`rise`] reaches).
pub fn crest(seed: u64, side: usize, kind: EdgeKind, along: f32) -> f32 {
    rise(seed, side, kind, along, 0.0)
}

/// Ridge noise in `[0, 1]`: 1 on crest lines, 0 in the troughs between.
fn ridge(k: u64, along: f32, out: f32, cell: f32) -> f32 {
    let r =
        0.6 * value_noise(k, along, out, cell) + 0.4 * value_noise(k ^ 7, along, out, cell * 0.43);
    1.0 - abs(2.0 * r - 1.0)
}

/// Height past `side` relative to the ground at the side, `out` meters out and `along`
/// meters along it: ridges rising to the horizon behind mountains and hills, a low forest
/// or flat yards, the sea floor (deep under the water) past water, with a far shore of
/// hills across a bay.
pub fn beyond(seed: u64, side: usize, kind: EdgeKind, along: f32, out: f32) -> f32 {
    let k = key(seed, side) ^ 0x0B0B;
    let soft = smoothstep(0.0, 500.0, out);
    match kind {
        EdgeKind::Mountains => {
            330.0 * smoothstep(-300.0, 1800.0, out)
                + 240.0 * soft * ridge(k, along, out, 520.0)
                + 30.0 * value_noise(k ^ 3, along, out, 90.0)
        }
        EdgeKind::Hills | EdgeKind::HighwayLoop => {
            80.0 * smoothstep(-200.0, 1400.0, out)
                + 90.0 * soft * ridge(k, along, out, 380.0)
                + 14.0 * value_noise(k ^ 3, along, out, 70.0)
        }
        EdgeKind::Forest => 4.0 * soft + 6.0 * soft * value_noise(k, along, out, 110.0),
        EdgeKind::RailYards => 1.2 * soft * value_noise(k, along, out, 60.0),
        EdgeKind::Ocean | EdgeKind::Lake | EdgeKind::Open => -40.0,
        EdgeKind::Bay => {
            let shore = smoothstep(1500.0, 2300.0, out);
            -40.0 + shore * (40.0 + 120.0 * (0.35 + 0.65 * ridge(k, along, out, 420.0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walls_are_steep_and_end_at_the_toe() {
        for kind in [
            EdgeKind::Mountains,
            EdgeKind::Hills,
            EdgeKind::HighwayLoop,
            EdgeKind::Forest,
            EdgeKind::RailYards,
        ] {
            let w = wall(kind).unwrap();
            let steepest = 1.5 * w.height_m.0 / ((FACE.1 - FACE.0) * w.ramp_m);
            assert!(steepest > 1.5, "{kind:?}: {steepest}");
            for i in 0..64 {
                let a = i as f32 * 37.0;
                assert_eq!(rise(7, NORTH, kind, a, band_m(kind) - FOOT_M), 0.0);
                let top = crest(7, NORTH, kind, a);
                assert!(top >= w.height_m.0 - 1e-3 && top <= w.height_m.1 + 1e-3);
            }
        }
        assert_eq!(band_m(EdgeKind::Ocean), 0.0);
        assert_eq!(rise(7, WEST, EdgeKind::Ocean, 10.0, 0.0), 0.0);
    }
}
