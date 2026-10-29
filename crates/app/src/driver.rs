//! The person at the wheel (`docs/NPCS.md`, "Drivers"): the pedestrian figure
//! (`traffic::figure`) seated in a car by [`traffic::figure::Skeleton::drive`], scaled so its
//! eyes are where the car's cockpit camera is, its hands on the rim following the steering
//! wheel, its head turned by the look-around.
//!
//! - **The player's driver** is one of [`figure::PRESETS`], picked on the Vehicle page and
//!   kept in the settings (`driver`, the `driver=` scene key). [`Drivers::build`] builds all
//!   of them for the place's era at setup (13 part meshes each, a few thousand triangles in
//!   all), so a pick switches at once. The outside cameras draw the whole figure; the
//!   cockpit draws it without the head and torso, so nothing comes between the eye and the
//!   road and the arms and hands are on the wheel below it.
//! - **NPC cars' drivers** are the crowd's figures (`crate::peds`), one per car by its id,
//!   drawn by [`crate::npc`] in the cars within the near level of detail.

use glam::{Mat3, Mat4, Vec3};
use render::{Mesh, MeshId};
use traffic::figure::{self, Era, FigureSpec, Part, Seat, Skeleton, Xform, PARTS};
use vehicle::shape::{CarMesh, Detail, ShapeSpec};

/// How far the hands follow the wheel round from where they grip it, rad; past it they
/// stay put and the rim slides through them (no hand-over-hand).
pub const HAND_FOLLOW_RAD: f32 = 1.4;
/// Where the hands grip the rim: this far above 9 and 3 o'clock, rad (ten to two).
const GRIP_RAD: f32 = 0.55;
/// The head turns by this share of the camera's look-around, at most [`LOOK_MAX_RAD`].
const LOOK_SHARE: f32 = 0.6;
const LOOK_MAX_RAD: f32 = 1.2;
/// The heels sit this far left and right of the driver's line, m: the left foot by the
/// clutch, the right on the accelerator.
const HEEL_X_M: [f32; 2] = [-0.13, 0.08];
/// The feet's pitch on the pedals, rad (toe up).
const TOE_RAD: f32 = 0.45;
/// A figure is scaled this much past sitting its eyes at the car's cockpit eye (a point
/// that suits a slight adult): a grown-up at the wheel...
const GROW: f32 = 1.12;
/// ...as far as the roof allows, keeping the crown this far under its skin, m...
const HEADROOM_M: f32 = 0.05;
/// ...and never less or more than this.
const SCALE: (f32, f32) = (0.7, 1.25);
/// The crown above the eyes, as a share of the head's height (`traffic::figure`'s head: the
/// eyes at 0.47 of it, the skull to 0.91, a little hair over that).
const CROWN_OVER_EYE: f32 = 0.66;
/// Below this speed (m/s) the left hand comes off the wheel to rest on the thigh, all of it
/// standing still.
const REST_BELOW_MPS: f32 = 1.0;

/// Where a driver sits in one car's frame (body space for the player's car, the parked
/// mesh's for an NPC car).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seating {
    pub hip: Vec3,
    pub heel: Vec3,
    pub eye: Vec3,
    pub wheel_pivot: Vec3,
    /// Toward the driver; a positive turn about it is counter-clockwise as they see it.
    pub wheel_axis: Vec3,
    pub wheel_radius: f32,
    /// Height of the roof's skin over the driver's head; infinite over an open top.
    pub roof: f32,
}

impl Seating {
    /// The driver's seat of `car` (built from `shape`, model space) in the frame `offset`
    /// from it.
    pub fn of(shape: &ShapeSpec, car: &CarMesh, offset: Vec3) -> Seating {
        let cockpit = &car.cockpit;
        let at = |p: [f32; 3]| Vec3::from(p) + offset;
        // The body's highest point over the head: within 15 cm of the driver's line, from
        // just ahead of the eyes to 30 cm behind them.
        let open = shape.roof.convertible() || shape.details.contains(&Detail::Targa);
        let [x, _, z] = cockpit.eye;
        let roof = car
            .body
            .positions
            .iter()
            .filter(|p| !open && (p[0] - x).abs() < 0.15 && (-0.05..0.3).contains(&(p[2] - z)))
            .fold(f32::INFINITY, |r, p| {
                if r == f32::INFINITY {
                    p[1]
                } else {
                    r.max(p[1])
                }
            });
        Seating {
            roof: roof + offset.y,
            hip: at(cockpit.hip),
            heel: at(cockpit.heel),
            eye: at(cockpit.eye),
            wheel_pivot: at(cockpit.wheel_pivot),
            wheel_axis: Vec3::from(cockpit.wheel_axis),
            wheel_radius: cockpit.wheel_radius,
        }
    }

    /// The seat moved by the player's seat adjustment `shift` (body space: up, and `-z`
    /// forward), which moves the cockpit eye: the driver moves with it, hands still on the
    /// wheel and heels on the floor.
    pub fn moved(&self, shift: Vec3) -> Seating {
        Seating {
            hip: self.hip + shift,
            eye: self.eye + shift,
            heel: self.heel + Vec3::new(0.0, 0.0, shift.z),
            ..*self
        }
    }

    /// The hands' grips on the rim, left then right, with the wheel turned `turn` rad.
    pub fn grips(&self, turn: f32) -> [Vec3; 2] {
        let axis = self.wheel_axis.normalize_or(Vec3::Z);
        let up = (Vec3::Y - axis * Vec3::Y.dot(axis)).normalize_or(Vec3::Y);
        let right = up.cross(axis);
        let follow = turn.clamp(-HAND_FOLLOW_RAD, HAND_FOLLOW_RAD);
        [core::f32::consts::PI - GRIP_RAD, GRIP_RAD].map(|a| {
            let (s, c) = (a + follow).sin_cos();
            self.wheel_pivot + (right * c + up * s) * self.wheel_radius
        })
    }
}

/// How far the left hand has come off the wheel at `speed` m/s (0 on it, 1 resting).
pub fn rest_at(speed: f32) -> f32 {
    (1.0 - speed.abs() / REST_BELOW_MPS).clamp(0.0, 1.0)
}

/// The head's turn for a camera look-around of `look_yaw` rad.
pub fn head_turn(look_yaw: f32) -> f32 {
    (look_yaw * LOOK_SHARE).clamp(-LOOK_MAX_RAD, LOOK_MAX_RAD)
}

/// How big `skeleton`'s figure sits at `seat`: [`GROW`] past its eyes at the cockpit eye,
/// within [`SCALE`], its crown [`HEADROOM_M`] under the roof.
pub fn scale(skeleton: &Skeleton, seat: &Seating) -> f32 {
    let to_eye = seat.eye - seat.hip;
    let fit = to_eye.length() / skeleton.hip_to_eye();
    let h = skeleton.height_m;
    let crown = (skeleton.hip_to_eye() + CROWN_OVER_EYE * 0.23 * (h / 1.75).sqrt())
        * (to_eye.y / to_eye.length().max(1e-3));
    let room = (seat.roof - HEADROOM_M - seat.hip.y) / crown.max(1e-3);
    (fit * GROW).min(room).clamp(SCALE.0, SCALE.1)
}

/// Each part of `skeleton`'s figure seated at `seat` into `out`, as transforms from the
/// part's mesh into the car's frame: the wheel turned `turn` rad (the steering wheel's own
/// angle), the head turned `look` rad, the left hand resting on the thigh by `rest` (0 on
/// the wheel, 1 off it; [`rest_at`]). Allocates nothing.
pub fn pose(
    skeleton: &Skeleton,
    seat: &Seating,
    turn: f32,
    look: f32,
    rest: f32,
    out: &mut [Mat4; PARTS],
) {
    // The back leans along the hip-to-eye line.
    let to_eye = seat.eye - seat.hip;
    let k = scale(skeleton, seat);
    let recline = to_eye.z.atan2(to_eye.y.max(1e-3));
    let local = |p: Vec3| ((p - seat.hip) / k).to_array();
    // The heels on the floor behind the pedals, or as far toward them as the legs reach
    // with the knees a little bent (a short driver in a tall van).
    let reach = 0.93 * (skeleton.thigh_m + skeleton.shin_m);
    let floor = local(seat.heel);
    let drop = -floor[1] - skeleton.ankle_m;
    let ahead = (reach * reach - drop * drop).max(0.0).sqrt();
    let heel = |x: f32| [x / k, floor[1], floor[2].max(-ahead)];
    let grips = seat.grips(turn);
    // Resting, the left hand lies on the thigh a little short of the knee.
    let knee = 0.7 * skeleton.thigh_m;
    let thigh = [
        -skeleton.offset[Part::UpperLegR as usize][0] * 1.1,
        0.06 * skeleton.height_m,
        -knee,
    ];
    let left = local(grips[0]);
    let rest = rest.clamp(0.0, 1.0);
    let rest = rest * rest * (3.0 - 2.0 * rest);
    let left = core::array::from_fn(|i| left[i] + (thigh[i] - left[i]) * rest);
    let s = Seat {
        hip: [0.0; 3],
        recline,
        heels: [heel(HEEL_X_M[0]), heel(HEEL_X_M[1])],
        toe: TOE_RAD,
        hands: [left, local(grips[1])],
        look,
    };
    let mut xf = [Xform::IDENTITY; PARTS];
    skeleton.pose(&skeleton.drive(&s), &mut xf);
    let root = Mat4::from_translation(seat.hip) * Mat4::from_scale(Vec3::splat(k));
    for (m, x) in out.iter_mut().zip(&xf) {
        *m = root * part_matrix(x);
    }
}

/// A part's transform in the figure frame as a matrix.
pub fn part_matrix(x: &Xform) -> Mat4 {
    // `Xform::rot` is rows; glam takes columns.
    let r = Mat3::from_cols_array_2d(&x.rot).transpose();
    Mat4::from_cols(
        r.x_axis.extend(0.0),
        r.y_axis.extend(0.0),
        r.z_axis.extend(0.0),
        Vec3::from(x.pos).extend(1.0),
    )
}

/// Whether the cockpit draws part `k`: not the head and torso, which sit round the eye.
pub fn in_cockpit(k: usize) -> bool {
    k != Part::Head as usize && k != Part::Torso as usize
}

/// One built driver: its skeleton and part meshes.
#[derive(Clone, Debug, PartialEq)]
pub struct Driver {
    pub skeleton: Skeleton,
    pub ids: [MeshId; PARTS],
}

/// The player's preset drivers for one era, built and registered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drivers {
    pub era: Option<Era>,
    pub list: Vec<Driver>,
}

impl Drivers {
    /// Builds every preset dressed for `era`, registering each part mesh with `register`.
    pub fn build(era: Era, mut register: impl FnMut(Mesh) -> MeshId) -> Drivers {
        let list = (0..figure::PRESETS)
            .map(|i| {
                let f = figure::build(&FigureSpec::preset(i, era));
                Driver {
                    skeleton: f.skeleton,
                    ids: core::array::from_fn(|p| register(crate::car::plain_mesh(&f.parts[p]))),
                }
            })
            .collect();
        Drivers {
            era: Some(era),
            list,
        }
    }

    /// Preset `i` (wrapping), if built.
    pub fn get(&self, i: usize) -> Option<&Driver> {
        (!self.list.is_empty()).then(|| &self.list[i % self.list.len()])
    }
}

/// Stable name of preset `i` (settings JSON, the `driver=` scene key): its picker name in
/// lower case.
pub fn preset_key(i: usize) -> String {
    figure::PRESET_NAMES[i % figure::PRESETS].to_ascii_lowercase()
}

/// The preset a key names.
pub fn preset_from_key(key: &str) -> Option<usize> {
    (0..figure::PRESETS).find(|&i| preset_key(i) == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vehicles;

    /// Every built-in car seats a driver: hands on the rim, the neck below the eye and the
    /// crown 3 cm under the roof (a hardtop's single skin), feet on the floor ahead of the hips.
    #[test]
    fn every_car_seats_a_driver_with_hands_on_the_wheel() {
        let figures: Vec<figure::Figure> = (0..figure::PRESETS)
            .map(|i| figure::build(&FigureSpec::preset(i, Era::Eighties)))
            .chain((0..8).map(|s| figure::figure(s, Era::Seventies)))
            .collect();
        for (_, src) in vehicles::BUILTIN_SHAPES {
            let shape = vehicles::load_shape(src).unwrap();
            let car = vehicle::shape::build_lod(&shape, 0, 1).unwrap();
            let seat = Seating::of(&shape, &car, Vec3::ZERO);
            // The roof over the head: the body's highest point near it, if it has one.
            let open =
                shape.roof.convertible() || shape.details.contains(&vehicle::shape::Detail::Targa);
            for f in &figures {
                let sk = &f.skeleton;
                for turn in [-3.0, -1.0, 0.0, 1.0, 3.0] {
                    let mut out = [Mat4::IDENTITY; PARTS];
                    pose(sk, &seat, turn, 0.0, 0.0, &mut out);
                    assert!(out.iter().all(|m| m.is_finite()), "{}", shape.id);
                    let grips = seat.grips(turn);
                    for (j, lower) in [Part::LowerArmL, Part::LowerArmR].into_iter().enumerate() {
                        let hand = out[lower as usize].transform_point3(Vec3::new(
                            0.0,
                            -0.19 * sk.height_m,
                            0.0,
                        ));
                        let miss = hand.distance(grips[j]);
                        // On the rim, or a few centimeters short at a long reach round it.
                        let near = if turn == 0.0 { 0.03 } else { 0.08 };
                        assert!(miss < near, "{}: hand {j} {miss} m off", shape.id);
                    }
                    let head = out[Part::Head as usize].w_axis.truncate();
                    assert!(head.y < seat.eye.y, "{}: neck over the eye", shape.id);
                    let crown = f.parts[Part::Head as usize]
                        .positions
                        .iter()
                        .map(|&p| out[Part::Head as usize].transform_point3(Vec3::from(p)))
                        .fold(Vec3::NEG_INFINITY, Vec3::max);
                    if !open {
                        let roof = car
                            .body
                            .positions
                            .iter()
                            .filter(|p| {
                                (p[0] - head.x).abs() < 0.15 && (p[2] - head.z).abs() < 0.15
                            })
                            .fold(f32::MIN, |m, p| m.max(p[1]));
                        // (A big cab's roof has no vertex that near: far overhead.)
                        assert!(
                            roof == f32::MIN || crown.y < roof - 0.03,
                            "{}: head {crown} under {roof}",
                            shape.id
                        );
                    }
                    for foot in [Part::FootL, Part::FootR] {
                        let at = out[foot as usize].w_axis.truncate();
                        assert!(at.z < seat.hip.z, "{}: feet ahead of the hips", shape.id);
                        assert!(
                            (at.y - seat.heel.y).abs() < 0.2,
                            "{}: foot {at} heel {} hip {}",
                            shape.id,
                            seat.heel,
                            seat.hip
                        );
                    }
                }
            }
        }
    }

    /// First person: from the cockpit eye wherever the seat adjustment and the tuning put
    /// it, no part the cockpit draws comes within 5 cm of the camera's near plane, hands on
    /// the wheel or the left at rest, the wheel at any turn; and the hidden head and torso
    /// hold the eye (the camera is never outside its own body looking back at it).
    #[test]
    fn the_cockpit_camera_never_cuts_into_the_driver() {
        let tuning = crate::camera::TUNING;
        let clear = tuning.cockpit_near_m + 0.05;
        let (up, fwd) = (
            crate::settings::SEAT_HEIGHT_CM,
            crate::settings::SEAT_FORWARD_CM,
        );
        let figures: Vec<figure::Figure> = (0..figure::PRESETS)
            .map(|i| figure::build(&FigureSpec::preset(i, Era::Eighties)))
            .chain((0..8).map(|s| figure::figure(s, Era::Seventies)))
            .collect();
        let mut worst = f32::MAX;
        for (_, src) in vehicles::BUILTIN_SHAPES {
            let shape = vehicles::load_shape(src).unwrap();
            let car = vehicle::shape::build_lod(&shape, 0, 0).unwrap();
            let seat = Seating::of(&shape, &car, Vec3::ZERO);
            for f in &figures {
                for (turn, rest) in [(-3.0, 0.0), (0.0, 0.0), (3.0, 0.0), (0.0, 1.0)] {
                    for u in [up[0], 0.0, up[1]] {
                        for w in [fwd[0], 0.0, fwd[1]] {
                            // The seat adjustment moves the driver and the eye together.
                            let shift = Vec3::new(0.0, u / 100.0, -w / 100.0);
                            let moved = seat.moved(shift);
                            let mut out = [Mat4::IDENTITY; PARTS];
                            pose(&f.skeleton, &moved, turn, 0.0, rest, &mut out);
                            let eye = moved.eye
                                + Vec3::new(
                                    0.0,
                                    tuning.cockpit_eye_up_m,
                                    -tuning.cockpit_eye_forward_m,
                                );
                            for k in (0..PARTS).filter(|&k| in_cockpit(k)) {
                                for p in &f.parts[k].positions {
                                    let d = out[k].transform_point3(Vec3::from(*p)).distance(eye);
                                    worst = worst.min(d);
                                    assert!(
                                        d > clear,
                                        "{}: part {k} {d:.3} m from the eye",
                                        shape.id
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        eprintln!("nearest drawn driver vertex to the cockpit eye: {worst:.3} m");
    }

    #[test]
    fn preset_keys_round_trip() {
        for i in 0..figure::PRESETS {
            assert_eq!(preset_from_key(&preset_key(i)), Some(i));
        }
        assert_eq!(preset_from_key("nobody"), None);
    }
}
