//! The player's car: its [`VehicleSpec`], the sim state stepped at 120 Hz, the driver's
//! switches (headlights, horn), and what the renderer needs to draw it between two steps.
//!
//! Each fixed step maps the frame's [`InputState`] (already ramped and curved by the input
//! mapper) onto a [`VehicleInput`] and runs [`vehicle::step`] against [`DriveGround`].
//! Rendering asks for the body and wheel transforms `alpha` of the way from the previous to
//! the current step.
//!
//! Until `vehicle::shape` builds the real body, the car is drawn as a two-box silhouette made
//! from `spec.dims_m` ([`body_mesh`], [`wheel_mesh`], [`lamp_mesh`]).

use core::f32::consts::{FRAC_PI_2, TAU};
use glam::{Mat4, Quat, Vec3};
use place::Place;
use render::{Mesh, MeshId, MeshInstance, PointLight, Vertex};
use vehicle::sim::{DT, FRONT_LEFT, FRONT_RIGHT, WHEEL_RADIUS_M};
use vehicle::{Assists, FlatGround, Ground, VehicleClass, VehicleInput, VehicleSpec, VehicleState};

use crate::input::{Action, InputState};
use crate::vehicles;

/// The car you get without asking for one.
pub const DEFAULT_VEHICLE: &str = "corsair_gtx";

/// What the car drives on. When the world collider lands this becomes
/// `world::WorldCollider` and [`ground_for`] builds it from the Place.
pub type DriveGround = FlatGround;

/// The ground for `place`. The test town is flat with its asphalt at `y = 0`.
pub fn ground_for(_place: &Place) -> DriveGround {
    FlatGround {
        y: 0.0,
        friction: 1.0,
    }
}

/// Where the car starts until Places carry spawn points (`docs/PLACES.md`, "Spawns"): on
/// the ground in the test town's eastbound outer lane at the west end of the strip...
pub const START_GROUND: [f32; 3] = [-92.0, 0.0, 4.9];
/// ...facing east down it, so the late-afternoon sun is behind the car. Heading 0 faces
/// `-z` (north), positive turns left.
pub const START_HEADING_RAD: f32 = -FRAC_PI_2;

/// The sim's body box starts this far above the ground at rest (`vehicle::sim`'s
/// `BODY_LOW_CORNER_M`) and is `dims_m[2]` tall.
pub const BODY_CLEARANCE_M: f32 = 0.25;
/// Placeholder tire width, m (the `ShapeSpec` will own it).
pub const WHEEL_WIDTH_M: f32 = 0.22;

/// Headlights as two warm point lights pooled on the road ahead (the renderer has no spot
/// lights yet): distance ahead of the center of mass, half spacing, height above the ground,
/// all m; intensity per lamp, kcd (≈ 35 lux on the road below each); falloff radius, m.
const HEADLIGHT_AHEAD_M: f32 = 10.0;
const HEADLIGHT_SPREAD_M: f32 = 0.7;
const HEADLIGHT_HEIGHT_M: f32 = 1.5;
const HEADLIGHT_KCD: f32 = 0.08;
const HEADLIGHT_RADIUS_M: f32 = 20.0;
/// Halogen white, linear RGB.
const HEADLIGHT_COLOR: [f32; 3] = [1.0, 0.86, 0.68];
/// Below this the car counts as fallen out of the world and is put back at its start, m.
const FLOOR_Y: f32 = -100.0;

/// A rigid transform: body origin (the center of mass) and body-to-world rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: Vec3,
    pub rot: Quat,
}

impl Pose {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rot, self.pos)
    }

    /// A body-space point in world space.
    pub fn point(&self, local: Vec3) -> Vec3 {
        self.pos + self.rot * local
    }

    /// Body `-z`.
    pub fn forward(&self) -> Vec3 {
        self.rot * Vec3::NEG_Z
    }
}

/// `a` to `b` by `t`, exactly `a` at 0 and exactly `b` at 1.
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        a + (b - a) * t
    }
}

/// [`mix`] per component.
pub fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    Vec3::new(mix(a.x, b.x, t), mix(a.y, b.y, t), mix(a.z, b.z, t))
}

/// Spherical interpolation, exact at the ends.
pub fn slerp(a: Quat, b: Quat, t: f32) -> Quat {
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        a.slerp(b, t)
    }
}

fn pose_of(s: &VehicleState) -> Pose {
    Pose {
        pos: Vec3::from(s.pos),
        rot: Quat::from_array(s.rot),
    }
}

/// The driver's controls this step: pedals, wheel and handbrake straight from the mapper.
pub fn vehicle_input(input: &InputState) -> VehicleInput {
    VehicleInput {
        throttle: input.axis(Action::Throttle),
        brake: input.axis(Action::Brake),
        steer: input.axis(Action::Steer),
        handbrake: input.pressed(Action::Handbrake),
    }
}

/// What the HUD shows, read once per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudState {
    /// Forward speed, m/s (negative reversing).
    pub speed_mps: f32,
    pub rpm: f32,
    /// -1 reverse, 0 neutral, 1.. forward.
    pub gear: i8,
    /// Apparent local solar time, seconds since midnight.
    pub time_of_day: f32,
    pub camera_mode: crate::camera::CameraMode,
    pub headlights: bool,
}

/// The player's car.
#[derive(Clone, Debug)]
pub struct Drive {
    pub spec: VehicleSpec,
    /// The state after the last step...
    pub state: VehicleState,
    /// ...and before it, for interpolation.
    pub prev: VehicleState,
    pub assists: Assists,
    /// What the last step was asked for.
    pub input: VehicleInput,
    pub headlights: bool,
    /// Held this frame (for the audio task).
    pub horn: bool,
    /// Rolling angle of each wheel, rad, kept for drawing (the sim keeps only the rate),
    /// now and one step ago. Both wrap together so they always interpolate the short way.
    roll: [f32; 4],
    prev_roll: [f32; 4],
    /// Spring compression at rest, m.
    rest_compression: f32,
    start_ground: [f32; 3],
    start_heading: f32,
}

impl Drive {
    /// The built-in car `vehicle_id` at the start of the strip.
    pub fn new(vehicle_id: &str) -> Result<Drive, String> {
        let spec = vehicles::builtin_spec(vehicle_id)?;
        Ok(Drive::with_spec(spec, START_GROUND, START_HEADING_RAD))
    }

    /// `spec` at rest on the ground point `ground`, facing `heading_rad`.
    pub fn with_spec(spec: VehicleSpec, ground: [f32; 3], heading_rad: f32) -> Drive {
        let state = VehicleState::spawn(&spec, ground, heading_rad);
        Drive {
            rest_compression: state.wheels[0].compression,
            spec,
            state,
            prev: state,
            assists: Assists::default(),
            input: VehicleInput::default(),
            headlights: false,
            horn: false,
            roll: [0.0; 4],
            prev_roll: [0.0; 4],
            start_ground: ground,
            start_heading: heading_rad,
        }
    }

    /// Back to the start, at rest.
    pub fn respawn(&mut self) {
        self.state = VehicleState::spawn(&self.spec, self.start_ground, self.start_heading);
        self.prev = self.state;
        self.roll = [0.0; 4];
        self.prev_roll = [0.0; 4];
    }

    /// Once per frame, before the steps: the switches that act on a press.
    pub fn frame(&mut self, input: &InputState) {
        if input.just_pressed(Action::Headlights) {
            self.headlights = !self.headlights;
        }
        self.horn = input.pressed(Action::Horn);
    }

    /// One fixed step with the driver asking for `input`.
    pub fn step(&mut self, input: VehicleInput, ground: &impl Ground) {
        self.prev = self.state;
        self.prev_roll = self.roll;
        self.input = input;
        vehicle::step(&self.spec, &mut self.state, &input, &self.assists, ground);
        for i in 0..4 {
            let r = self.roll[i] + self.state.wheels[i].spin * DT;
            let wrap = if r >= TAU {
                -TAU
            } else if r < 0.0 {
                TAU
            } else {
                0.0
            };
            self.roll[i] = r + wrap;
            self.prev_roll[i] += wrap;
        }
        // The sim never panics on bad data; neither do we. A car that left the world or
        // went non-finite starts again.
        let s = &self.state;
        let finite = s
            .pos
            .iter()
            .chain(&s.rot)
            .chain(&s.vel)
            .all(|v| v.is_finite());
        if !finite || s.pos[1] < FLOOR_Y {
            self.respawn();
        }
    }

    /// Spring compression with the car at rest on level ground, m.
    pub fn rest_compression(&self) -> f32 {
        self.rest_compression
    }

    pub fn odometer_m(&self) -> f64 {
        self.state.odometer_m
    }

    /// The body `alpha` of the way from the previous step to the current one.
    pub fn pose(&self, alpha: f32) -> Pose {
        let (a, b) = (pose_of(&self.prev), pose_of(&self.state));
        Pose {
            pos: mix3(a.pos, b.pos, alpha),
            rot: slerp(a.rot, b.rot, alpha),
        }
    }

    /// Wheel-to-body transforms, front-left, front-right, rear-left, rear-right: each wheel
    /// at its hub (raised by spring compression, hanging at full droop off the ground),
    /// steered at the front and rolled about its axle. The wheel mesh's axle is body `x`.
    pub fn wheel_locals(&self, alpha: f32) -> [Mat4; 4] {
        let contacts = self.spec.wheel_positions();
        let steer = mix(self.prev.steer_angle_rad, self.state.steer_angle_rad, alpha);
        core::array::from_fn(|i| {
            let c = mix(
                self.prev.wheels[i].compression,
                self.state.wheels[i].compression,
                alpha,
            );
            let [x, y, z] = contacts[i];
            let hub = Vec3::new(x, y + WHEEL_RADIUS_M + (c - self.rest_compression), z);
            // Positive steer turns right, a negative turn about +y; rolling forward
            // turns the top of the wheel toward -z, a negative turn about +x.
            let yaw = if i == FRONT_LEFT || i == FRONT_RIGHT {
                -steer
            } else {
                0.0
            };
            let roll = mix(self.prev_roll[i], self.roll[i], alpha);
            Mat4::from_translation(hub) * Mat4::from_rotation_y(yaw) * Mat4::from_rotation_x(-roll)
        })
    }

    /// Wheel-to-world transforms, in [`Drive::wheel_locals`] order.
    pub fn wheels(&self, alpha: f32) -> [Mat4; 4] {
        let body = self.pose(alpha).matrix();
        self.wheel_locals(alpha).map(|w| body * w)
    }

    /// Appends the car's instances: body, four wheels, and the lit lamps when the
    /// headlights are on.
    pub fn push_instances(&self, alpha: f32, ids: &CarMeshes, out: &mut Vec<MeshInstance>) {
        let instance = |mesh: MeshId, m: Mat4| MeshInstance {
            mesh,
            transform: m.to_cols_array_2d(),
            color: [1.0; 4],
        };
        let body = self.pose(alpha).matrix();
        out.push(instance(ids.body, body));
        if self.headlights {
            out.push(instance(ids.lamps, body));
        }
        for w in self.wheels(alpha) {
            out.push(instance(ids.wheel, w));
        }
    }

    /// The headlight pools (two lights) when the headlights are on, else none.
    pub fn lights(&self, alpha: f32) -> ([PointLight; 2], usize) {
        let pose = self.pose(alpha);
        let y = HEADLIGHT_HEIGHT_M - self.spec.cg_height_m;
        let at = |x: f32| PointLight {
            position: pose.point(Vec3::new(x, y, -HEADLIGHT_AHEAD_M)).to_array(),
            color: HEADLIGHT_COLOR.map(|c| c * HEADLIGHT_KCD),
            radius: HEADLIGHT_RADIUS_M,
        };
        let lights = [at(-HEADLIGHT_SPREAD_M), at(HEADLIGHT_SPREAD_M)];
        (lights, if self.headlights { 2 } else { 0 })
    }
}

/// Mesh ids of the placeholder car, as registered with the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarMeshes {
    pub body: MeshId,
    pub wheel: MeshId,
    pub lamps: MeshId,
}

/// Paint for the placeholder body: brushed steel for the wedge, brown for the wagon.
fn body_color(spec: &VehicleSpec) -> [u8; 4] {
    match spec.id.as_str() {
        "corsair_gtx" => [176, 178, 182, 255],
        "palisade_wagon" => [118, 78, 48, 255],
        _ => [160, 36, 32, 255],
    }
}

/// Heights of the placeholder silhouette in body space: underside, belt line, roof, m.
pub fn body_heights(spec: &VehicleSpec) -> (f32, f32, f32) {
    let ground = -spec.cg_height_m;
    let h = spec.dims_m[2];
    (
        ground + BODY_CLEARANCE_M + 0.05,
        ground + BODY_CLEARANCE_M + 0.45 * h,
        ground + BODY_CLEARANCE_M + h,
    )
}

/// The placeholder body in body space: a lower box to the belt line (a little narrower
/// than the wheels so they read from the side), a glass house over the cabin (to the tail
/// on a wagon) and, inside, a dark dash ahead of the driver's `eye`.
pub fn body_mesh(spec: &VehicleSpec, eye: Vec3) -> Mesh {
    let [len, wid, _] = spec.dims_m;
    let (under, belt, roof) = body_heights(spec);
    let half_l = 0.5 * len;
    let half_w = (0.5 * wid).min(0.5 * spec.track_m + 0.5 * WHEEL_WIDTH_M - 0.05);
    let mut m = Mesh::cuboid(
        [-half_w, under, -half_l],
        [half_w, belt, half_l],
        body_color(spec),
    );
    let glass_front = (eye.z - 0.95).max(-half_l + 0.4);
    let glass_back = if spec.class == VehicleClass::Wagon {
        half_l - 0.15
    } else {
        (eye.z + 0.85).min(half_l - 0.3)
    };
    let glass = Mesh::cuboid(
        [-0.42 * wid, belt, glass_front],
        [0.42 * wid, roof, glass_back],
        [46, 54, 62, 255],
    );
    let dash = Mesh::cuboid(
        [-half_w + 0.12, belt - 0.12, eye.z - 0.80],
        [half_w - 0.12, belt + 0.04, eye.z - 0.55],
        [40, 38, 36, 255],
    );
    for part in [glass, dash] {
        let base = m.vertices.len() as u32;
        m.vertices.extend_from_slice(&part.vertices);
        m.indices.extend(part.indices.iter().map(|i| i + base));
    }
    m
}

/// Head and tail lamps on the front and rear faces, glowing: drawn over the body while the
/// headlights are on.
pub fn lamp_mesh(spec: &VehicleSpec) -> Mesh {
    let [len, wid, _] = spec.dims_m;
    let (_, belt, _) = body_heights(spec);
    let half_l = 0.5 * len;
    let x0 = 0.5 * wid - 0.45;
    let (y0, y1) = (belt - 0.20, belt - 0.07);
    let mut m = Mesh::default();
    for (z0, z1, color, glow) in [
        (
            -half_l - 0.02,
            -half_l + 0.02,
            [250, 244, 226, 255],
            render::emissive([255, 236, 200], 6.0),
        ),
        (
            half_l - 0.02,
            half_l + 0.02,
            [200, 30, 24, 255],
            render::emissive([255, 32, 20], 3.0),
        ),
    ] {
        for sx in [-1.0f32, 1.0] {
            let (a, b) = (sx * x0, sx * (x0 + 0.3));
            let lamp =
                Mesh::cuboid([a.min(b), y0, z0], [a.max(b), y1, z1], color).with_emissive(glow);
            let base = m.vertices.len() as u32;
            m.vertices.extend_from_slice(&lamp.vertices);
            m.indices.extend(lamp.indices.iter().map(|i| i + base));
        }
    }
    m
}

/// Sides of the placeholder wheel's prism.
const WHEEL_SIDES: usize = 12;

/// A wheel centered on its hub with its axle along `x`: a 12-sided tire with two-tone
/// tread and a spoked face on each side, so its roll reads.
pub fn wheel_mesh() -> Mesh {
    let r = WHEEL_RADIUS_M;
    let hw = 0.5 * WHEEL_WIDTH_M;
    let tread = [[30, 30, 32, 255], [46, 46, 48, 255]];
    let face = [[150, 152, 156, 255], [70, 70, 74, 255]];
    let ring = |k: usize| {
        let a = k as f32 * TAU / WHEEL_SIDES as f32;
        (a.cos(), a.sin())
    };
    let mut m = Mesh::default();
    let tri = |m: &mut Mesh, p: [[f32; 3]; 3], n: [f32; 3], c: [u8; 4]| {
        let base = m.vertices.len() as u32;
        for v in p {
            m.vertices.push(Vertex::new(v, n, c));
        }
        m.indices.extend_from_slice(&[base, base + 1, base + 2]);
    };
    for k in 0..WHEEL_SIDES {
        let (c0, s0) = ring(k);
        let (c1, s1) = ring(k + 1);
        // Tread: corners at angles k and k+1, outward normal at the middle.
        let mid = (k as f32 + 0.5) * TAU / WHEEL_SIDES as f32;
        m.push_quad(
            [
                [-hw, r * c0, r * s0],
                [-hw, r * c1, r * s1],
                [hw, r * c1, r * s1],
                [hw, r * c0, r * s0],
            ],
            [0.0, mid.cos(), mid.sin()],
            tread[k % 2],
        );
        // Faces: increasing angle is counter-clockwise seen from +x.
        let color = face[k % 2];
        tri(
            &mut m,
            [[hw, 0.0, 0.0], [hw, r * c0, r * s0], [hw, r * c1, r * s1]],
            [1.0, 0.0, 0.0],
            color,
        );
        tri(
            &mut m,
            [
                [-hw, 0.0, 0.0],
                [-hw, r * c1, r * s1],
                [-hw, r * c0, r * s0],
            ],
            [-1.0, 0.0, 0.0],
            color,
        );
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> FlatGround {
        FlatGround {
            y: 0.0,
            friction: 1.0,
        }
    }

    fn throttle(t: f32) -> VehicleInput {
        VehicleInput {
            throttle: t,
            ..VehicleInput::default()
        }
    }

    #[test]
    fn both_cars_spawn_on_the_strip_facing_east() {
        for id in ["corsair_gtx", "palisade_wagon"] {
            let d = Drive::new(id).unwrap();
            assert_eq!(d.spec.id, id);
            let pose = d.pose(1.0);
            assert!(
                (pose.forward() - Vec3::X).length() < 1e-5,
                "{}",
                pose.forward()
            );
            assert_eq!(pose.pos.y, d.spec.cg_height_m);
            assert_eq!(d.state, d.prev);
        }
        assert!(Drive::new("hovercar").is_err());
    }

    #[test]
    fn interpolation_reproduces_both_states_at_the_ends() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let g = flat();
        for i in 0..240 {
            let steer = if i > 120 { 0.7 } else { 0.0 };
            d.step(
                VehicleInput {
                    steer,
                    ..throttle(1.0)
                },
                &g,
            );
        }
        assert_ne!(d.prev, d.state);
        let (p0, p1) = (d.pose(0.0), d.pose(1.0));
        assert_eq!(p0, pose_of(&d.prev));
        assert_eq!(p1, pose_of(&d.state));
        let mid = d.pose(0.5);
        assert_ne!(mid, p0);
        assert_ne!(mid, p1);

        // The wheels at alpha 0 and 1 are those the two states alone give.
        let mut only_prev = d.clone();
        only_prev.state = d.prev;
        only_prev.roll = d.prev_roll;
        assert_eq!(d.wheel_locals(0.0), only_prev.wheel_locals(1.0));
        let mut only_curr = d.clone();
        only_curr.prev = d.state;
        only_curr.prev_roll = d.roll;
        assert_eq!(d.wheel_locals(1.0), only_curr.wheel_locals(0.0));
    }

    #[test]
    fn wheels_sit_on_the_ground_steer_and_roll() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let g = flat();
        // At rest every hub is one radius above the asphalt.
        for w in d.wheels(1.0) {
            let hub = w.transform_point3(Vec3::ZERO);
            assert!((hub.y - WHEEL_RADIUS_M).abs() < 1e-5, "{hub}");
        }
        for _ in 0..360 {
            d.step(throttle(0.6), &g);
        }
        for _ in 0..60 {
            d.step(
                VehicleInput {
                    steer: 1.0,
                    ..throttle(0.3)
                },
                &g,
            );
        }
        let locals = d.wheel_locals(1.0);
        // Steering right turns the fronts' forward (-z) toward +x; the rears stay straight.
        let axle = |m: Mat4| m.transform_vector3(Vec3::X);
        let fl = axle(locals[FRONT_LEFT]);
        assert!(fl.z > 0.05, "{fl}");
        let rl = axle(locals[2]);
        assert!(rl.z.abs() < 1e-5 && rl.y.abs() < 1e-5, "{rl}");
        // Rolling: the rear wheel's spoke has turned off its rest position.
        let spoke = locals[2].transform_vector3(Vec3::Y);
        assert!(spoke.z.abs() > 1e-3, "{spoke}");
        assert!(d.roll.iter().all(|r| (0.0..TAU).contains(r)));
        // Driving, the hubs stay near ground plus radius.
        for w in d.wheels(1.0) {
            let hub = w.transform_point3(Vec3::ZERO);
            assert!((hub.y - WHEEL_RADIUS_M).abs() < 0.12, "{hub}");
        }
    }

    #[test]
    fn switches_and_lights() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let mut input = InputState::default();
        assert_eq!(d.lights(1.0).1, 0);
        input.set(Action::Headlights, 1.0);
        input.set_just_pressed(Action::Headlights, true);
        input.set(Action::Horn, 1.0);
        d.frame(&input);
        assert!(d.headlights && d.horn);
        let (lights, n) = d.lights(1.0);
        assert_eq!(n, 2);
        for l in lights {
            let ahead = Vec3::from(l.position) - d.pose(1.0).pos;
            assert!((ahead.x - HEADLIGHT_AHEAD_M).abs() < 1e-4, "{ahead}");
            assert!(l.color[0] > l.color[2], "warm");
        }
        input.set_just_pressed(Action::Headlights, false);
        d.frame(&input);
        assert!(d.headlights, "holding the button does not toggle again");

        let ids = CarMeshes {
            body: MeshId(7),
            wheel: MeshId(8),
            lamps: MeshId(9),
        };
        let mut out = Vec::new();
        d.push_instances(0.5, &ids, &mut out);
        assert_eq!(out.len(), 6);
        d.headlights = false;
        out.clear();
        d.push_instances(0.5, &ids, &mut out);
        assert_eq!(out.len(), 5);
        assert!(out.iter().all(|i| i.mesh != ids.lamps));
    }

    #[test]
    fn a_car_that_leaves_the_world_starts_again() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        d.state.pos[1] = f32::NAN;
        d.step(VehicleInput::default(), &flat());
        assert_eq!(d.state, d.prev);
        assert_eq!(d.pose(1.0).pos, Vec3::new(-92.0, d.spec.cg_height_m, 4.9));
    }

    /// Every triangle's winding agrees with its normal, so back-face culling keeps it.
    fn assert_outward(m: &Mesh) {
        assert!(!m.indices.is_empty());
        for t in m.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.vertices[i as usize].pos));
            let face = (b - a).cross(c - a).normalize();
            for &i in t {
                let n = Vec3::from(m.vertices[i as usize].normal);
                assert!(face.dot(n) > 0.95, "{face} vs {n}");
            }
        }
    }

    #[test]
    fn placeholder_meshes_fit_the_spec() {
        for id in ["corsair_gtx", "palisade_wagon"] {
            let spec = vehicles::builtin_spec(id).unwrap();
            let eye = Vec3::new(-0.35, 0.55, 0.15);
            let body = body_mesh(&spec, eye);
            let lamps = lamp_mesh(&spec);
            for m in [&body, &lamps, &wheel_mesh()] {
                assert_outward(m);
            }
            let [len, wid, h] = spec.dims_m;
            for v in body.vertices.iter().chain(&lamps.vertices) {
                let [x, y, z] = v.pos;
                assert!(x.abs() <= 0.5 * wid + 1e-4);
                assert!(z.abs() <= 0.5 * len + 0.03);
                assert!(
                    y >= -spec.cg_height_m && y <= -spec.cg_height_m + 0.25 + h + 1e-4,
                    "{id} {y}"
                );
            }
            assert!(lamps
                .vertices
                .iter()
                .all(|v| render::emissive_scale(v.emissive).0 > 1.0));
        }
        let wheel = wheel_mesh();
        for v in &wheel.vertices {
            let [x, y, z] = v.pos;
            assert!(x.abs() <= 0.5 * WHEEL_WIDTH_M + 1e-6);
            assert!((y * y + z * z).sqrt() <= WHEEL_RADIUS_M + 1e-5);
        }
    }
}
