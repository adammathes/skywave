//! Cars that rest on the ground: a parked or traffic car's body follows the ground under its
//! wheels, pitched up a hill street and rolled across a crowned or sloping one, its wheels
//! on the surface rather than its middle (`docs/VEHICLES.md`). The player's car has its own
//! suspension; this is for the cars drawn without one.

use glam::{Quat, Vec3};

use crate::drive::DriveGround;

/// Half the wheelbase and half the track the four ground samples sit at, m: a mid-size
/// car's, close enough for every shape at street grades.
const HALF_BASE_M: f32 = 1.35;
const HALF_TRACK_M: f32 = 0.75;
/// How far above the car's own height the ground is looked for, m: over a curb or a
/// bridge's deck, under the next layer up.
const LOOK_UP_M: f32 = 2.0;
/// The steepest pitch and roll a resting car takes, rad (a 36% grade): past it the samples
/// caught a wall or a ledge, not the street.
const MAX_TILT: f32 = 0.35;

/// Where a car whose ground point is `pos`, facing yaw `yaw` (`-z` turned by it), rests on
/// `ground`: its ground point at the mean height under its four wheels, and its rotation,
/// yaw then pitch about its right axis and roll about its forward one.
pub fn rest(ground: &DriveGround, pos: Vec3, yaw: f32) -> (Vec3, Quat) {
    let (s, c) = yaw.sin_cos();
    let fwd = [-s, -c];
    let right = [c, -s];
    let from = pos.y + LOOK_UP_M;
    let h = |a: f32, b: f32| {
        let p = [
            pos.x + fwd[0] * a + right[0] * b,
            pos.z + fwd[1] * a + right[1] * b,
        ];
        ground.level_from(p, from).0
    };
    let (fl, fr) = (h(HALF_BASE_M, -HALF_TRACK_M), h(HALF_BASE_M, HALF_TRACK_M));
    let (bl, br) = (
        h(-HALF_BASE_M, -HALF_TRACK_M),
        h(-HALF_BASE_M, HALF_TRACK_M),
    );
    let y = 0.25 * (fl + fr + bl + br);
    if !y.is_finite() || (y - pos.y).abs() > LOOK_UP_M {
        return (pos, Quat::from_rotation_y(yaw));
    }
    let pitch = ((fl + fr - bl - br) * 0.5 / (2.0 * HALF_BASE_M))
        .atan()
        .clamp(-MAX_TILT, MAX_TILT);
    let roll = ((fr + br - fl - bl) * 0.5 / (2.0 * HALF_TRACK_M))
        .atan()
        .clamp(-MAX_TILT, MAX_TILT);
    let rot =
        Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Quat::from_rotation_z(roll);
    (Vec3::new(pos.x, y, pos.z), rot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty place whose ground climbs `grade` toward `-z` (north) and is level across.
    fn hill(grade: f32) -> DriveGround {
        let mut place = place::Place::test_grid();
        place.roads = Default::default();
        place.parcels.clear();
        place.buildings.clear();
        place.props.items.clear();
        place.header.region = place::RegionId::SocalBasin;
        let [w, _] = place::Terrain::samples(&place.header, place.terrain.res_per_chunk);
        let cell = place.header.chunk_size_m / f32::from(place.terrain.res_per_chunk);
        let z0 = place.header.origin_m()[1];
        for (i, h) in place.terrain.heights.iter_mut().enumerate() {
            let z = z0 + (i / w) as f32 * cell;
            *h = 40.0 - grade * z;
        }
        crate::drive::ground_for(&place)
    }

    #[test]
    fn a_car_on_a_hill_pitches_with_it_and_sits_on_it() {
        let ground = hill(0.2);
        let at = Vec3::new(0.0, 0.0, 0.0);
        let y0 = ground.level_at([0.0, 0.0]).0;
        // Facing north, up the hill: nose up by the grade's angle.
        let (p, q) = rest(&ground, Vec3::new(at.x, y0 + 0.5, at.z), 0.0);
        assert!((p.y - y0).abs() < 0.05, "{} vs {y0}", p.y);
        let nose = q * Vec3::NEG_Z;
        assert!((nose.y - 0.2f32.atan().sin()).abs() < 0.02, "{nose}");
        let side = q * Vec3::X;
        assert!(side.y.abs() < 0.01, "{side}");
        // Facing east, across it: rolled, its left (north) side high, nose level.
        let (_, q) = rest(
            &ground,
            Vec3::new(at.x, y0, at.z),
            -core::f32::consts::FRAC_PI_2,
        );
        let nose = q * Vec3::NEG_Z;
        assert!(nose.y.abs() < 0.01, "{nose}");
        let right = q * Vec3::X;
        assert!(right.y < -0.15, "the downhill side is low: {right}");
    }
}
