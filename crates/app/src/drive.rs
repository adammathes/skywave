//! The player's car: its [`VehicleSpec`], the sim state stepped at 120 Hz, the driver's
//! switches (headlights, horn), and what the renderer needs to draw it between two steps.
//!
//! Each fixed step maps the frame's [`InputState`] (already ramped and curved by the input
//! mapper) onto a [`VehicleInput`] and runs [`vehicle::step`] against [`DriveGround`].
//! Rendering asks for the body and wheel transforms `alpha` of the way from the previous to
//! the current step. The car's meshes and their layout come from [`crate::car`].

use core::f32::consts::{FRAC_PI_2, PI, TAU};
use glam::{Mat4, Quat, Vec3};
use place::Place;
use render::{Corona, MeshId, MeshInstance, PointLight};
use vehicle::sim::{DT, FRONT_LEFT, FRONT_RIGHT, REAR_LEFT};
use vehicle::{Assists, VehicleInput, VehicleSpec, VehicleState};

use traffic::figure::PARTS;

use crate::car::{CarLayout, CarMeshes, CarModel};
use crate::input::{Action, InputState};
use crate::vehicles;

/// The car you get without asking for one.
pub const DEFAULT_VEHICLE: &str = "gannet_gt";

/// What the car drives on: the town's collider (ground heights, curbs, walls, props).
pub type DriveGround = world::WorldCollider;

/// The collider for `place`, built once.
pub fn ground_for(place: &Place) -> DriveGround {
    ground_in(place, crate::places::grammar())
}

/// The collider for `place` dressed by `grammar`.
pub fn ground_in(place: &Place, grammar: &world::Grammar) -> DriveGround {
    world::WorldCollider::build(place, grammar)
}

/// The test town's `PlayerStart` spawn, for what starts without a Place at hand
/// ([`Drive::new`], the default scene; [`crate::App`] reads the spawn itself): on the
/// ground in the eastbound outer lane at the west end of the strip...
pub const START_GROUND: [f32; 3] = [-92.0, 0.0, 4.9];
/// ...facing east down it, so the late-afternoon sun is behind the car. Heading 0 faces
/// `-z` (north), positive turns left.
pub const START_HEADING_RAD: f32 = -FRAC_PI_2;

/// Most instances one car adds per frame: body, glass, lamps, underglow, four wheels, then
/// the cabin walls, the cabin, the steering wheel, the dial ticks, up to three needles and
/// the two headlight tell-tales, and the driver's parts.
pub const MAX_CAR_INSTANCES: usize = 17 + PARTS;

/// Headlights: one spot per headlamp, from just behind its lens (inside the lamp, so the
/// car's own nose is not lit), aimed this far below the car's forward axis, degrees...
pub const HEADLIGHT_SETBACK_M: f32 = 0.05;
pub const HEADLIGHT_PITCH_DEG: f32 = 2.0;
/// ...at full intensity within this angle across the axis and gone beyond that one,
/// degrees, and `HEADLIGHT_SQUASH` times narrower above and below it (2.5° and 8°): a flat
/// low beam that meets the road from ~5 m past the bumper and lights both lanes...
pub const HEADLIGHT_INNER_DEG: f32 = 10.0;
pub const HEADLIGHT_OUTER_DEG: f32 = 32.0;
pub const HEADLIGHT_SQUASH: f32 = 4.0;
/// ...with the road's grazing cosine flattened by this wrap, so the pool reaches ~25 m
/// instead of dying 10 m out (a point source falls as h/d³ on flat ground; a real low beam
/// throws most of its light at the horizon)...
pub const HEADLIGHT_WRAP: f32 = 0.25;
/// ...with this intensity per lamp, kcd, and this reach, m. Was a point light of 0.08 kcd
/// hung 1.5 m up, 10 m ahead: a dim blob in the middle distance and nothing near the car.
pub const HEADLIGHT_KCD: f32 = 14.0;
const HEADLIGHT_RADIUS_M: f32 = 60.0;
/// Halogen white, linear RGB.
const HEADLIGHT_COLOR: [f32; 3] = [1.0, 0.86, 0.68];
/// The headlamps' halo, seen from in front.
const HEADLIGHT_CORONA: Corona = Corona {
    radius: 0.6,
    strength: 0.8,
};
/// High beams: aimed this far below the axis, degrees, half as squashed (a taller beam
/// that lights the road far ahead and the fronts beside it), this bright per lamp, kcd,
/// reaching this far, m, with a bigger halo.
const HIGH_BEAM_PITCH_DEG: f32 = 0.5;
const HIGH_BEAM_SQUASH: f32 = 2.0;
pub const HIGH_BEAM_KCD: f32 = 40.0;
const HIGH_BEAM_RADIUS_M: f32 = 140.0;
const HIGH_BEAM_CORONA: Corona = Corona {
    radius: 0.9,
    strength: 1.0,
};
/// Tail lamps light nothing much; they are coronas only (radius-0 lights), red, as bright
/// (kcd, as the corona reads it) with the headlights on and under braking...
const TAIL_RGB: [f32; 3] = [1.0, 0.04, 0.02];
const TAIL_KCD: f32 = 4.0;
const BRAKE_KCD: f32 = 12.0;
/// ...from just behind the lens, with this halo.
const TAIL_CORONA: Corona = Corona {
    radius: 0.4,
    strength: 0.8,
};
/// Most lights one car gives the frame: two headlamps, two tail lamps, two underglow lights.
pub const CAR_LIGHTS: usize = 6;
/// The underglow's lights: as bright (kcd) as a dim lamp and short-reaching, so they pool
/// on the road round the car and no further.
const UNDERGLOW_KCD: f32 = 0.8;
const UNDERGLOW_RADIUS_M: f32 = 4.5;
/// Instance tint of the cabin walls (the body shell inside out, so in the paint color): the
/// inside of the roof and pillars reads as a dark lining, not as bright paint under the
/// cabin light.
const CABIN_WALL_TINT: f32 = 0.3;
/// A car switched to while the old one rolls slower than this keeps its velocity; faster,
/// it starts at rest, m/s.
pub const KEEP_SPEED_MPS: f32 = 5.0;
/// At or under this the car is standing, for the brake lamps, m/s.
const STANDING_MPS: f32 = 0.05;
/// Below this the car counts as fallen out of the world and is put back at its start, m.
const FLOOR_Y: f32 = -100.0;
/// Water: the car wades through water up to this deep at its wheels' contact with the
/// ground (about the hubs), m...
pub const WADE_M: f32 = 0.3;
/// ...and deeper drags it to a stop, this much more per second for every meter past
/// that, 1/s...
const WATER_DRAG: f32 = 6.0;
/// ...and after this long deeper than this, s and m, the car is put back at its start.
pub const DEEP_RESET_S: f32 = 1.5;
pub const DEEP_M: f32 = 0.6;
/// A reset looks this far for a lane; with none that close the car goes back to its
/// start, m.
pub const RESET_RANGE_M: f32 = 200.0;
/// How far along the lane a reset looks for a clear spot, in steps of this many meters.
const RESET_STEP_M: f32 = 2.0;
const RESET_STEPS: usize = 40;

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

/// How brightly the car's self-lit parts glow this frame (emissive multipliers, from
/// [`crate::lighting::car_glow`]): the exterior lamps, and the instruments (dial faces,
/// needles, ticks, radio display).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarGlow {
    pub lamps: f32,
    pub instruments: f32,
    /// The speedometer's needle on top of the instruments' (the green wave, [`crate::wave`]).
    pub speedo: f32,
}

impl Default for CarGlow {
    /// As built.
    fn default() -> CarGlow {
        CarGlow {
            lamps: 1.0,
            instruments: 1.0,
            speedo: 1.0,
        }
    }
}

/// How much of the cabin a frame draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cabin {
    /// The outside cameras: none of it.
    Outside,
    /// "Hood only": the cabin walls (the shell inside out: roof, pillars, windshield frame),
    /// no dashboard, wheel or gauges.
    Frame,
    /// Walls, dashboard and seats, steering wheel, gauge needles.
    Full,
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
    /// Where the eye, wheels, lamps and dials are on this car's body.
    pub layout: CarLayout,
    /// The state after the last step...
    pub state: VehicleState,
    /// ...and before it, for interpolation.
    pub prev: VehicleState,
    pub assists: Assists,
    /// What the last step was asked for.
    pub input: VehicleInput,
    /// What the collider did in the last step (for the sound).
    pub impact: world::Impact,
    /// Parked cars this car has knocked, swaying until they settle (for drawing).
    pub rocking: world::collide::Rocking,
    pub headlights: bool,
    /// High beams selected: the head lamps throw [`high_beam`]s while they are on.
    pub high_beams: bool,
    /// The headlights follow dusk and dawn (Settings › Drive › Headlights: Auto): they
    /// switch when it gets dark or light, and the switch overrides them in between.
    pub auto_headlights: bool,
    /// The underglow switched on (off at the start; lit only on a car that has one).
    pub underglow: bool,
    /// Whether it was dark at the last frame, `None` until the next frame decides.
    pub auto_dark: Option<bool>,
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
    /// Seconds the car has been in deep water without a break.
    deep_s: f32,
    /// Times deep water has put the car back at its start (the app toasts each).
    pub water_resets: u32,
}

impl Drive {
    /// The built-in car `vehicle_id` at the start of the strip (building its meshes for
    /// the layout; [`Drive::with_layout`] takes one already built).
    pub fn new(vehicle_id: &str) -> Result<Drive, String> {
        let spec = vehicles::builtin_spec(vehicle_id)?;
        let shape = vehicles::builtin_shape(&spec)?;
        let layout = CarModel::build(&spec, &shape)?.layout;
        Ok(Drive::with_layout(
            spec,
            layout,
            START_GROUND,
            START_HEADING_RAD,
        ))
    }

    /// `spec` with its body `layout` at rest on the ground point `ground`, facing
    /// `heading_rad`.
    pub fn with_layout(
        spec: VehicleSpec,
        layout: CarLayout,
        ground: [f32; 3],
        heading_rad: f32,
    ) -> Drive {
        let state = VehicleState::spawn(&spec, ground, heading_rad);
        Drive {
            rest_compression: state.wheels[0].compression,
            spec,
            layout,
            state,
            prev: state,
            assists: Assists::default(),
            input: VehicleInput::default(),
            impact: world::Impact::default(),
            rocking: world::collide::Rocking::default(),
            headlights: false,
            high_beams: false,
            auto_headlights: false,
            underglow: false,
            auto_dark: None,
            horn: false,
            roll: [0.0; 4],
            prev_roll: [0.0; 4],
            start_ground: ground,
            start_heading: heading_rad,
            deep_s: 0.0,
            water_resets: 0,
        }
    }

    /// Back to the start, at rest.
    pub fn respawn(&mut self) {
        self.state = VehicleState::spawn(&self.spec, self.start_ground, self.start_heading);
        self.prev = self.state;
        self.roll = [0.0; 4];
        self.prev_roll = [0.0; 4];
        self.deep_s = 0.0;
    }

    /// How deep the water is at the car's wheels' contact with the ground, m.
    pub fn water_depth(&self, ground: &DriveGround) -> f32 {
        let s = &self.state;
        let contact = s.pos[1] - self.spec.cg_height_m;
        ground.water_depth([s.pos[0], s.pos[2]], contact)
    }

    /// Water past [`WADE_M`] drags the car to a stop; after [`DEEP_RESET_S`] deeper than
    /// [`DEEP_M`] it goes back to its start.
    fn wade(&mut self, ground: &DriveGround) {
        let depth = self.water_depth(ground);
        if depth > WADE_M {
            let keep = 1.0 / (1.0 + WATER_DRAG * (depth - WADE_M) * DT);
            let s = &mut self.state;
            s.vel[0] *= keep;
            s.vel[2] *= keep;
            s.speed_mps *= keep;
            for w in &mut s.wheels {
                w.spin *= keep;
            }
        }
        self.deep_s = if depth > DEEP_M {
            self.deep_s + DT
        } else {
            0.0
        };
        if self.deep_s >= DEEP_RESET_S {
            self.respawn();
            self.water_resets += 1;
        }
    }

    /// The `Reset` action: at rest and upright on the centerline of the nearest of `lanes`
    /// (on the ground under it, the deck it is on), heading along it the way it drives; at
    /// the start when no lane is within [`RESET_RANGE_M`]. A spot that is not `clear` (on
    /// the track before a train) gives way to the nearest clear one along the lane, a few
    /// meters at a time either way. The odometer, headlights and assists stay.
    pub fn reset(
        &mut self,
        lanes: &world::lanes::LaneLines,
        ground: &DriveGround,
        clear: impl Fn([f32; 2]) -> bool,
    ) {
        let odometer = self.state.odometer_m;
        let p = self.state.pos;
        // The ground point under the body, for which level of a bridge it is on.
        let foot = [p[0], p[1] - self.spec.cg_height_m, p[2]];
        let spot = lanes.nearest(foot, RESET_RANGE_M).and_then(|h| {
            if clear([h.pos[0], h.pos[2]]) {
                return Some(h);
            }
            (1..=RESET_STEPS).find_map(|k| {
                [-1.0f32, 1.0].into_iter().find_map(|sign| {
                    let d = sign * RESET_STEP_M * k as f32;
                    let q = [h.pos[0] + h.dir[0] * d, h.pos[1], h.pos[2] + h.dir[1] * d];
                    lanes
                        .nearest(q, RESET_STEP_M)
                        .filter(|g| clear([g.pos[0], g.pos[2]]))
                })
            })
        });
        match spot {
            Some(h) => {
                let [x, y, z] = h.pos;
                let (y, _) = ground.level_from([x, z], y + 0.5);
                let heading = (-h.dir[0]).atan2(-h.dir[1]);
                self.state = VehicleState::spawn(&self.spec, [x, y, z], heading);
                self.prev = self.state;
                self.roll = [0.0; 4];
                self.prev_roll = [0.0; 4];
            }
            None => self.respawn(),
        }
        self.state.odometer_m = odometer;
        self.prev.odometer_m = odometer;
    }

    /// Swaps in the car `spec` (with its body `layout`) where this one is: on the ground
    /// under it, level, facing the same way; at rest, unless this one was rolling slower
    /// than [`KEEP_SPEED_MPS`], whose velocity it keeps. The odometer starts again; the
    /// headlights, the underglow switch, the assists and the respawn point stay.
    pub fn switch_car(&mut self, spec: VehicleSpec, layout: CarLayout, ground: &DriveGround) {
        let pose = self.pose(1.0);
        let f = pose.forward();
        let heading = (-f.x).atan2(-f.z);
        let p = pose.pos;
        // Seen from the body's height, so the car stays on the deck it is on.
        let (y, _) = ground.level_from([p.x, p.z], p.y + 0.5);
        let mut next = Drive::with_layout(spec, layout, [p.x, y, p.z], heading);
        next.assists = self.assists;
        next.headlights = self.headlights;
        next.high_beams = self.high_beams;
        next.auto_headlights = self.auto_headlights;
        next.underglow = self.underglow;
        next.auto_dark = self.auto_dark;
        next.start_ground = self.start_ground;
        next.start_heading = self.start_heading;
        next.water_resets = self.water_resets;
        let speed = self.state.speed_mps;
        if speed.abs() < KEEP_SPEED_MPS {
            next.state.vel = self.state.vel;
            next.state.speed_mps = speed;
            for w in &mut next.state.wheels {
                w.spin = speed / vehicle::sim::WHEEL_RADIUS_M;
            }
            next.prev = next.state;
        }
        *self = next;
    }

    /// Once per frame, before the steps: the switches that act on a press, and with
    /// [`Drive::auto_headlights`] the headlights following `dark` when it changes. High
    /// beams switched on switch the headlights on too.
    pub fn frame(&mut self, input: &InputState, dark: bool) {
        if self.auto_headlights && self.auto_dark != Some(dark) {
            self.headlights = dark;
        }
        self.auto_dark = Some(dark);
        if input.just_pressed(Action::Headlights) {
            self.headlights = !self.headlights;
        }
        if input.just_pressed(Action::HighBeams) {
            self.high_beams = !self.headlights || !self.high_beams;
            self.headlights = true;
        }
        if input.just_pressed(Action::Underglow) {
            self.underglow = !self.underglow;
        }
        self.horn = input.pressed(Action::Horn);
    }

    /// After [`Drive::step`]: an NPC car touched this car (`npc::Npcs::collide`), heard
    /// and felt like any other hit.
    pub fn note_car_hit(&mut self, hit: world::Bump) {
        self.impact.contact = true;
        if hit.speed > self.impact.hit.speed {
            self.impact.hit = hit;
        }
    }

    /// The high beams are lit: selected, with the headlights on.
    pub fn high_beams_lit(&self) -> bool {
        self.headlights && self.high_beams
    }

    /// One fixed step with the driver asking for `input`.
    pub fn step(&mut self, input: VehicleInput, ground: &DriveGround) {
        self.prev = self.state;
        self.prev_roll = self.roll;
        self.input = input;
        vehicle::step(&self.spec, &mut self.state, &input, &self.assists, ground);
        self.impact = world::resolve(&mut self.state, &self.spec, ground);
        self.rocking.note(&self.impact, ground);
        self.rocking.step();
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
            return;
        }
        self.wade(ground);
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
    /// at its hub (the shape's wheel center, raised by spring compression past rest),
    /// steered at the front and rolled about its axle. The wheel mesh is a right-side wheel
    /// with its axle along `x`; the left ones are turned 180° about `y` first.
    pub fn wheel_locals(&self, alpha: f32) -> [Mat4; 4] {
        let steer = mix(self.prev.steer_angle_rad, self.state.steer_angle_rad, alpha);
        core::array::from_fn(|i| {
            let c = mix(
                self.prev.wheels[i].compression,
                self.state.wheels[i].compression,
                alpha,
            );
            let hub = self.layout.wheel_centers[i] + Vec3::Y * (c - self.rest_compression);
            // Positive steer turns right, a negative turn about +y; rolling forward
            // turns the top of the wheel toward -z, a negative turn about +x.
            let yaw = if i == FRONT_LEFT || i == FRONT_RIGHT {
                -steer
            } else {
                0.0
            };
            let roll = mix(self.prev_roll[i], self.roll[i], alpha);
            let side = if i == FRONT_LEFT || i == REAR_LEFT {
                Mat4::from_rotation_y(PI)
            } else {
                Mat4::IDENTITY
            };
            Mat4::from_translation(hub)
                * Mat4::from_rotation_y(yaw)
                * Mat4::from_rotation_x(-roll)
                * side
        })
    }

    /// Wheel-to-world transforms, in [`Drive::wheel_locals`] order.
    pub fn wheels(&self, alpha: f32) -> [Mat4; 4] {
        let body = self.pose(alpha).matrix();
        self.wheel_locals(alpha).map(|w| body * w)
    }

    /// Whether the brake lamps are lit: the driver asked the service brakes for anything
    /// in the last step (the brake pedal, or in reverse the throttle, which brakes there;
    /// reversing on the brake pedal and coasting leave them dark), or held the brake
    /// standing still, whatever gear brake-to-reverse picked (waiting at a light).
    pub fn braking(&self) -> bool {
        if self.state.speed_mps.abs() < STANDING_MPS && self.input.brake > 0.0 {
            return true;
        }
        let pedal = if self.state.gear < 0 {
            self.input.throttle
        } else {
            self.input.brake
        };
        pedal > 0.0
    }

    /// Steering-wheel-to-body transform `alpha` of the way through the step.
    pub fn steering_wheel(&self, alpha: f32) -> Mat4 {
        let steer = mix(self.prev.steer_angle_rad, self.state.steer_angle_rad, alpha);
        self.layout.steering_wheel(steer)
    }

    /// Needle-to-body transforms of the dials (the first `layout.dial_count` are real).
    pub fn needles(&self, alpha: f32) -> [Mat4; 3] {
        let speed = mix(self.prev.speed_mps, self.state.speed_mps, alpha);
        let rpm = mix(self.prev.engine_rpm, self.state.engine_rpm, alpha);
        self.layout
            .dials
            .map(|d| d.needle(d.reading.fraction(speed, rpm)))
    }

    /// Appends the car's instances at level of detail 0: body (pop-ups up while the
    /// headlights are on), glass (see-through), the lamps in their state, four wheels and, by
    /// `cabin` (the first-person cameras), the cabin walls (the shell inside out) and then
    /// the interior, steering wheel and gauge needles, lit by the cabin light. The outside
    /// cameras see the interior and steering wheel through the glass (or over an open top),
    /// lit like the outside ("hood only" keeps to the walls). The lamps and the instruments glow by `glow`. At most
    /// [`MAX_CAR_INSTANCES`].
    pub fn push_instances(
        &self,
        alpha: f32,
        ids: &CarMeshes,
        cabin: Cabin,
        glow: CarGlow,
        out: &mut Vec<MeshInstance>,
    ) {
        let instance = |mesh: MeshId, m: Mat4| MeshInstance::new(mesh, m.to_cols_array_2d());
        let inside = |mesh: MeshId, m: Mat4| MeshInstance {
            glow: glow.instruments,
            cabin: 1.0,
            ..instance(mesh, m)
        };
        let lod = &ids.lods[0];
        let on = usize::from(self.headlights);
        let body = self.pose(alpha).matrix();
        out.push(instance(lod.body[on], body));
        // See-through: the driver and the cabin show through it. Sunlight comes in.
        out.push(MeshInstance {
            glass: true,
            casts_shadow: false,
            ..instance(lod.glass, body)
        });
        out.push(MeshInstance {
            glow: glow.lamps,
            ..instance(lod.lights[on][usize::from(self.braking())], body)
        });
        if let Some(tubes) = lod.underglow.filter(|_| self.underglow) {
            out.push(MeshInstance {
                glow: glow.lamps,
                ..instance(tubes, body)
            });
        }
        for w in self.wheels(alpha) {
            out.push(instance(lod.wheel, w));
        }
        if cabin == Cabin::Outside {
            let open = |mesh: MeshId, m: Mat4| MeshInstance {
                glow: glow.instruments,
                ..instance(mesh, m)
            };
            out.push(open(lod.interior, body));
            out.push(open(lod.steering_wheel, body * self.steering_wheel(alpha)));
        }
        if cabin != Cabin::Outside {
            out.push(MeshInstance {
                color: [CABIN_WALL_TINT, CABIN_WALL_TINT, CABIN_WALL_TINT, 1.0],
                ..inside(lod.shell_inside, body)
            });
        }
        if cabin == Cabin::Full {
            out.push(inside(lod.interior, body));
            out.push(inside(
                lod.steering_wheel,
                body * self.steering_wheel(alpha),
            ));
            out.push(inside(ids.dial_ticks, body));
            let needles = self.needles(alpha);
            for (k, n) in needles.iter().enumerate().take(self.layout.dial_count) {
                let mut needle = inside(ids.needles[k], body * *n);
                if k == 0 {
                    needle.glow *= glow.speedo;
                }
                out.push(needle);
            }
            let [low, high] = ids.telltales;
            if self.headlights {
                out.push(inside(low, body));
            }
            if self.high_beams_lit() {
                out.push(inside(high, body));
            }
        }
    }

    /// Appends the driver's parts seated in the car `alpha` of the way through the step,
    /// hands on the steering wheel (the left resting on the thigh while the car stands),
    /// head turned by the camera's `look_yaw`, moved with the seat by `seat_shift` (the
    /// player's seat adjustment, body space; the cockpit's alone): the whole figure
    /// from the outside cameras, without the head and torso in the cockpit ([`Cabin::Full`];
    /// "hood only" draws no driver). Lit by the cabin light in the cockpit, casting no
    /// shadow (the car's own shadows them).
    pub fn push_driver(
        &self,
        alpha: f32,
        driver: &crate::driver::Driver,
        cabin: Cabin,
        look_yaw: f32,
        seat_shift: Vec3,
        out: &mut Vec<MeshInstance>,
    ) {
        if cabin == Cabin::Frame {
            return;
        }
        let steer = mix(self.prev.steer_angle_rad, self.state.steer_angle_rad, alpha);
        let turn = -steer * self.layout.steering_ratio;
        let speed = mix(self.prev.speed_mps, self.state.speed_mps, alpha);
        let mut parts = [Mat4::IDENTITY; PARTS];
        crate::driver::pose(
            &driver.skeleton,
            &self.layout.seating.moved(seat_shift),
            turn,
            crate::driver::head_turn(look_yaw),
            crate::driver::rest_at(speed),
            &mut parts,
        );
        let body = self.pose(alpha).matrix();
        for (k, m) in parts.iter().enumerate() {
            if cabin == Cabin::Full && !crate::driver::in_cockpit(k) {
                continue;
            }
            out.push(MeshInstance {
                cabin: if cabin == Cabin::Full { 1.0 } else { 0.0 },
                casts_shadow: false,
                ..MeshInstance::new(driver.ids[k], (body * *m).to_cols_array_2d())
            });
        }
    }

    /// The car's lights: the headlight beams (two spots, from the left and right headlamps
    /// along the car's nose, pitched down; high beams when selected) when the headlights are
    /// on, then the tail lamps' coronas while they are lit (headlights on or braking), then
    /// the underglow's two pools of light while it is on. Returns them and how many.
    pub fn lights(&self, alpha: f32) -> ([PointLight; CAR_LIGHTS], usize) {
        let pose = self.pose(alpha);
        let high = self.high_beams_lit();
        let at = |lamp: Vec3| {
            if high {
                high_beam(&pose, lamp)
            } else {
                headlight(&pose, lamp)
            }
        };
        let tail_kcd = if self.braking() { BRAKE_KCD } else { TAIL_KCD };
        let tail = |lamp: Vec3| PointLight {
            corona: TAIL_CORONA,
            ..PointLight::omni(
                pose.point(lamp - Vec3::Z * HEADLIGHT_SETBACK_M).to_array(),
                TAIL_RGB.map(|c| c * tail_kcd),
                0.0,
            )
        };
        let [l, r] = self.layout.headlamps;
        let [tl, tr] = self.layout.taillamps;
        let mut out = [PointLight::omni([0.0; 3], [0.0; 3], 0.0); CAR_LIGHTS];
        let mut n = 0;
        let mut push = |light: PointLight| {
            out[n] = light;
            n += 1;
        };
        if self.headlights {
            push(at(l));
            push(at(r));
        }
        if self.headlights || self.braking() {
            push(tail(tl));
            push(tail(tr));
        }
        if let Some(glow) = self.layout.underglow.filter(|_| self.underglow) {
            for p in glow.lights {
                let rgb = glow.rgb.map(|c| c * UNDERGLOW_KCD);
                push(PointLight::omni(
                    pose.point(p).to_array(),
                    rgb,
                    UNDERGLOW_RADIUS_M,
                ));
            }
        }
        (out, n)
    }
}

/// The high beam of the head lamp at body point `lamp` of a car at `pose`: the low beam
/// aimed higher, taller, brighter and reaching further.
pub fn high_beam(pose: &Pose, lamp: Vec3) -> PointLight {
    let pitch = HIGH_BEAM_PITCH_DEG.to_radians();
    PointLight {
        direction: (pose.rot * Vec3::new(0.0, -pitch.sin(), -pitch.cos())).to_array(),
        color: HEADLIGHT_COLOR.map(|c| c * HIGH_BEAM_KCD),
        radius: HIGH_BEAM_RADIUS_M,
        cone_squash: HIGH_BEAM_SQUASH,
        corona: HIGH_BEAM_CORONA,
        ..headlight(pose, lamp)
    }
}

/// The low beam of the head lamp at body point `lamp` of a car at `pose`: a spot from just
/// behind the lens, aimed [`HEADLIGHT_PITCH_DEG`] below the car's forward axis.
pub fn headlight(pose: &Pose, lamp: Vec3) -> PointLight {
    let pitch = HEADLIGHT_PITCH_DEG.to_radians();
    let axis = pose.rot * Vec3::new(0.0, -pitch.sin(), -pitch.cos());
    PointLight {
        position: pose.point(lamp + Vec3::Z * HEADLIGHT_SETBACK_M).to_array(),
        color: HEADLIGHT_COLOR.map(|c| c * HEADLIGHT_KCD),
        radius: HEADLIGHT_RADIUS_M,
        direction: axis.to_array(),
        cone_cos: [
            HEADLIGHT_OUTER_DEG.to_radians().cos(),
            HEADLIGHT_INNER_DEG.to_radians().cos(),
        ],
        cone_squash: HEADLIGHT_SQUASH,
        wrap: HEADLIGHT_WRAP,
        corona: HEADLIGHT_CORONA,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::car::CarLodIds;

    fn flat() -> DriveGround {
        ground_for(&place::Place::test_grid())
    }

    #[test]
    fn start_is_the_test_towns_player_start() {
        let place = place::Place::test_grid();
        let s = place.player_start().unwrap();
        assert_eq!((s.pos, s.heading), (START_GROUND, START_HEADING_RAD));
    }

    fn throttle(t: f32) -> VehicleInput {
        VehicleInput {
            throttle: t,
            ..VehicleInput::default()
        }
    }

    /// Distinct ids for every mesh of a car.
    fn ids() -> CarMeshes {
        let lod = |k: u32| CarLodIds {
            body: [MeshId(k), MeshId(k + 1)],
            shell_inside: MeshId(k + 10),
            glass: MeshId(k + 2),
            interior: MeshId(k + 3),
            steering_wheel: MeshId(k + 4),
            wheel: MeshId(k + 5),
            lights: [
                [MeshId(k + 6), MeshId(k + 7)],
                [MeshId(k + 8), MeshId(k + 9)],
            ],
            underglow: Some(MeshId(k + 11)),
        };
        CarMeshes {
            lods: [lod(10), lod(30)],
            needles: [MeshId(50), MeshId(51), MeshId(52)],
            dial_ticks: MeshId(53),
            telltales: [MeshId(54), MeshId(55)],
        }
    }

    /// Braking bakes the brake group far above the tail group, and in full sun the lit
    /// brake lens is past the bloom threshold at the cap while the tail lamp (lights off)
    /// is dark; the brake pedal lights them, reversing on it and coasting do not, and the
    /// throttle braking a reversing car does.
    #[test]
    fn brake_lamps_outshine_the_tail_lamps_and_reversing_leaves_them_dark() {
        let spec = vehicles::builtin_spec("gannet_gt").unwrap();
        let shape = vehicles::builtin_shape(&spec).unwrap();
        let model = CarModel::build(&spec, &shape).unwrap();
        // Mean tail-group emissive scale of a lamp state.
        let tail = |m: &render::Mesh| {
            let s: Vec<f32> = m
                .vertices
                .iter()
                .filter(|v| crate::car::lamp_group(v.emissive) == crate::car::LampGroup::Tail)
                .map(|v| render::emissive_scale(v.emissive).0)
                .collect();
            s.iter().sum::<f32>() / s.len().max(1) as f32
        };
        let lights = &model.lods[0].lights;
        let (lit, brake, dark_brake) = (
            tail(&lights[1][0]),
            tail(&lights[1][1]),
            tail(&lights[0][1]),
        );
        assert!(
            brake > 3.0 * lit && dark_brake == brake,
            "{lit} {brake} {dark_brake}"
        );
        let noon = crate::lighting::TimeOfDay {
            clock: crate::daylight::Clock::new(crate::lighting::DEFAULT_DAY_OF_YEAR, 12.0 * 3600.0),
            ..Default::default()
        }
        .daylight();
        let k = crate::lighting::EMISSIVE_BOOST
            * crate::lighting::car_glow(&noon).lamps
            * render::exposure_from_ev100(noon.ev100);
        let (brake_noon, tail_noon) = (brake * k, lit * k);
        assert!(
            (2.0..3.0).contains(&brake_noon) && tail_noon < 1.0,
            "brake lens at {brake_noon}, tail at {tail_noon} after exposure at noon"
        );

        // Standing on the brake, the lamps stay lit after brake-to-reverse picks reverse.
        let mut d = Drive::new("gannet_gt").unwrap();
        let ground = flat();
        let mut reversed = false;
        for _ in 0..120 {
            d.step(
                VehicleInput {
                    brake: 1.0,
                    ..VehicleInput::default()
                },
                &ground,
            );
            if d.state.speed_mps.abs() < STANDING_MPS {
                reversed |= d.state.gear < 0;
                assert!(
                    d.braking(),
                    "standing on the brake in gear {}",
                    d.state.gear
                );
            }
        }
        assert!(reversed, "brake-to-reverse picked reverse while standing");

        let ground = flat();
        let mut d = Drive::new("gannet_gt").unwrap();
        let pedal = |throttle: f32, brake: f32| VehicleInput {
            throttle,
            brake,
            ..VehicleInput::default()
        };
        for _ in 0..240 {
            d.step(throttle(1.0), &ground);
        }
        assert!(!d.braking(), "driving");
        d.step(VehicleInput::default(), &ground);
        assert!(!d.braking(), "coasting");
        let mut out = Vec::new();
        for _ in 0..600 {
            d.step(pedal(0.0, 1.0), &ground);
            if d.state.gear >= 0 {
                assert!(d.braking(), "braking at {}", d.state.speed_mps);
                out.clear();
                d.push_instances(1.0, &ids(), Cabin::Outside, CarGlow::default(), &mut out);
                assert_eq!(out[2].mesh, ids().lods[0].lights[0][1]);
            }
        }
        assert!(d.state.gear < 0 && d.state.speed_mps < -1.0, "reversing");
        assert!(!d.braking(), "reversing on the brake pedal");
        d.step(pedal(1.0, 0.0), &ground);
        assert!(d.braking(), "the throttle brakes a reversing car");
    }

    #[test]
    fn both_cars_spawn_on_the_strip_facing_east() {
        for id in ["gannet_gt", "homestead_wagon"] {
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
    fn wheels_sit_at_the_shape_hubs_steer_and_roll() {
        for id in ["gannet_gt", "homestead_wagon"] {
            let mut d = Drive::new(id).unwrap();
            let g = flat();
            let r = d.layout.wheel_radius;
            // At rest every hub is at the shape's wheel center, one radius over the asphalt.
            for (k, w) in d.wheel_locals(1.0).iter().enumerate() {
                let hub = w.transform_point3(Vec3::ZERO);
                assert!(
                    (hub - d.layout.wheel_centers[k]).length() < 1e-6,
                    "{id} {hub}"
                );
            }
            for w in d.wheels(1.0) {
                let hub = w.transform_point3(Vec3::ZERO);
                assert!((hub.y - r).abs() < 1e-5, "{id} {hub}");
            }
            // Left wheels face out to the left, right ones to the right.
            let axle = |m: Mat4| m.transform_vector3(Vec3::X);
            let locals = d.wheel_locals(1.0);
            assert!(axle(locals[FRONT_LEFT]).x < -0.999);
            assert!(axle(locals[REAR_LEFT]).x < -0.999);
            assert!(axle(locals[FRONT_RIGHT]).x > 0.999);
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
            let steer = d.state.steer_angle_rad;
            assert!(steer > 0.1, "{id} {steer}");
            let locals = d.wheel_locals(1.0);
            // Steering right turns the fronts about y by the steer angle (forward, -z, toward
            // +x, so the outward axle toward +z); the rears stay straight.
            for i in [FRONT_LEFT, FRONT_RIGHT] {
                let out = axle(locals[i]) * if i == FRONT_LEFT { -1.0 } else { 1.0 };
                let yaw = out.z.atan2(out.x);
                assert!((yaw - steer).abs() < 1e-5, "{id} {yaw} vs {steer}");
                assert!(out.y.abs() < 1e-5);
            }
            let rl = axle(locals[REAR_LEFT]);
            assert!(rl.z.abs() < 1e-5 && rl.y.abs() < 1e-5, "{rl}");
            // Rolling: the rear wheel's spoke has turned off its rest position.
            let spoke = locals[2].transform_vector3(Vec3::Y);
            assert!(spoke.z.abs() > 1e-3, "{spoke}");
            assert!(d.roll.iter().all(|r| (0.0..TAU).contains(r)));
            // Driving, the hubs follow the springs near ground plus radius.
            for w in d.wheels(1.0) {
                let hub = w.transform_point3(Vec3::ZERO);
                assert!((hub.y - r).abs() < 0.12, "{hub}");
            }
        }
    }

    #[test]
    fn the_steering_wheel_follows_the_road_wheels() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let l = d.layout;
        assert_eq!(d.steering_wheel(1.0), l.steering_wheel(0.0));
        d.state.steer_angle_rad = -0.05;
        let up = (Vec3::Y - l.wheel_axis * l.wheel_axis.y).normalize();
        // Steering left: the rim's top swings left (counter-clockwise from the seat).
        let top = d.steering_wheel(1.0).transform_vector3(up);
        assert!(top.x < -0.5, "{top}");
        // Half way through the step it has turned half as far.
        let half = d.steering_wheel(0.5).transform_vector3(up);
        assert!(half.x > top.x && half.x < 0.0);
    }

    #[test]
    fn switches_lamps_and_pop_ups() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let ids = ids();
        let lod = ids.lods[0];
        let mut input = InputState::default();
        assert_eq!(d.lights(1.0).1, 0);
        let mut out = Vec::new();
        d.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
        // Body (pop-ups down), glass, unlit lamps, four wheels, the cabin through the glass.
        let meshes: Vec<MeshId> = out.iter().map(|i| i.mesh).collect();
        assert_eq!(
            meshes,
            [
                lod.body[0],
                lod.glass,
                lod.lights[0][0],
                lod.wheel,
                lod.wheel,
                lod.wheel,
                lod.wheel,
                lod.interior,
                lod.steering_wheel
            ]
        );

        input.set(Action::Headlights, 1.0);
        input.set_just_pressed(Action::Headlights, true);
        input.set(Action::Horn, 1.0);
        d.frame(&input, false);
        assert!(d.headlights && d.horn);
        let (lights, n) = d.lights(1.0);
        assert_eq!(n, 4);
        let pose = d.pose(1.0);
        for (l, lamp) in lights.iter().zip(d.layout.headlamps) {
            assert!(l.corona.radius > 0.0 && l.radius > 10.0);
            // Just behind its lamp, aimed down the road ahead and a little down.
            let local = pose.rot.inverse() * (Vec3::from(l.position) - pose.pos);
            assert!((local - lamp - Vec3::Z * HEADLIGHT_SETBACK_M).length() < 1e-4);
            let axis = pose.rot.inverse() * Vec3::from(l.direction);
            assert!(
                axis.z < -0.99 && axis.y < -0.01 && axis.x.abs() < 1e-5,
                "{axis}"
            );
            assert!(l.cone_cos[0] < l.cone_cos[1]);
            assert!(l.color[0] > l.color[2], "warm");
        }
        // Then the tail lamps: red coronas that light nothing, just behind their lenses.
        for (l, lamp) in lights[2..].iter().zip(d.layout.taillamps) {
            let local = pose.rot.inverse() * (Vec3::from(l.position) - pose.pos);
            assert!((local - lamp + Vec3::Z * HEADLIGHT_SETBACK_M).length() < 1e-4);
            assert_eq!(l.radius, 0.0);
            assert!(l.corona.radius > 0.0 && l.color[0] > 10.0 * l.color[1]);
        }
        // The pool on the road: what the asphalt straight ahead receives (the renderer's
        // squashed smoothstep cone and wrapped cosine) at the lamp's height, relative to
        // 10 m out. Dark under the nose, lit from ~5 m, still a fifth as bright at 25 m.
        let lamp = d.layout.headlamps[0];
        let h = lamp.y + d.spec.cg_height_m;
        let beam = |ahead: f32| {
            let to = Vec3::new(0.0, -h, -ahead).normalize();
            let axis = Vec3::new(0.0, -HEADLIGHT_PITCH_DEG.to_radians().sin(), -1.0).normalize();
            let right = axis.cross(Vec3::Y).normalize();
            let up = right.cross(axis);
            let (f, u) = (to.dot(axis), to.dot(up) * HEADLIGHT_SQUASH);
            let off = f / (f * f + u * u).sqrt();
            let (o, i) = (
                HEADLIGHT_OUTER_DEG.to_radians().cos(),
                HEADLIGHT_INNER_DEG.to_radians().cos(),
            );
            let c = ((off - o) / (i - o)).clamp(0.0, 1.0);
            let d2 = ahead * ahead + h * h;
            let ndl = (h / d2.sqrt() + HEADLIGHT_WRAP) / (1.0 + HEADLIGHT_WRAP);
            HEADLIGHT_KCD * c * c * (3.0 - 2.0 * c) * ndl / d2
        };
        let ten = beam(10.0);
        assert_eq!(
            beam(2.0),
            0.0,
            "the road under the nose is outside the beam"
        );
        assert!(beam(6.0) > 0.3 * ten, "{}", beam(6.0) / ten);
        assert!(beam(8.0) > ten && beam(8.0) < 3.0 * ten);
        assert!(beam(25.0) > 0.1 * ten, "{}", beam(25.0) / ten);
        assert!(beam(25.0) < beam(15.0));
        input.set_just_pressed(Action::Headlights, false);
        d.frame(&input, false);
        assert!(d.headlights, "holding the button does not toggle again");

        out.clear();
        d.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
        assert_eq!(out[0].mesh, lod.body[1], "pop-ups up");
        assert_eq!(out[2].mesh, lod.lights[1][0], "lamps lit");
        // Braking lights the brake lamps.
        d.step(
            VehicleInput {
                brake: 1.0,
                ..VehicleInput::default()
            },
            &flat(),
        );
        assert!(d.braking());
        out.clear();
        d.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
        assert_eq!(out[2].mesh, lod.lights[1][1]);
        let brake = d.lights(1.0).0[2];
        assert!(
            brake.color[0] > lights[2].color[0],
            "brighter under braking"
        );
        d.headlights = false;
        out.clear();
        d.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
        assert_eq!(out[0].mesh, lod.body[0]);
        assert_eq!(out[2].mesh, lod.lights[0][1]);
        // Headlights off, braking: just the brake lamps' coronas.
        let (lights, n) = d.lights(1.0);
        assert_eq!(n, 2);
        assert!(lights[..2]
            .iter()
            .all(|l| l.radius == 0.0 && l.corona == brake.corona));
    }

    #[test]
    fn the_underglow_has_its_own_switch() {
        let mut d = Drive::new("siskin_coupe").unwrap();
        let glow = d.layout.underglow.expect("the tuner has underglow");
        assert!(glow.rgb[2] > glow.rgb[0], "blue");
        let ids = ids();
        let tubes = ids.lods[0].underglow.unwrap();
        let drawn = |d: &Drive| {
            let mut out = Vec::new();
            d.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
            out.iter().any(|i| i.mesh == tubes)
        };
        // Off at the start: nothing drawn, nothing lit.
        assert!(!d.underglow && !drawn(&d));
        assert_eq!(d.lights(1.0).1, 0);
        let mut input = InputState::default();
        input.set(Action::Underglow, 1.0);
        input.set_just_pressed(Action::Underglow, true);
        d.frame(&input, true);
        assert!(d.underglow && drawn(&d) && !d.headlights);
        // Two pools of its color under the car, front and rear.
        let (lights, n) = d.lights(1.0);
        assert_eq!(n, 2);
        let pose = d.pose(1.0);
        let local: Vec<Vec3> = lights[..2]
            .iter()
            .map(|l| pose.rot.inverse() * (Vec3::from(l.position) - pose.pos))
            .collect();
        for (l, p) in lights.iter().zip(&local) {
            assert!(l.radius > 1.0 && l.direction == [0.0; 3]);
            assert!(l.color[2] > l.color[0]);
            // Low in the body, on the centerline.
            let above_ground = p.y + d.spec.cg_height_m;
            assert!(
                p.x.abs() < 1e-4 && (0.2..0.6).contains(&above_ground),
                "{p}"
            );
        }
        assert!(local[0].z < local[1].z, "front then rear");
        // With the headlights too: beams, tail lamps, then the underglow.
        d.headlights = true;
        let (all, n) = d.lights(1.0);
        assert_eq!(n, CAR_LIGHTS);
        assert_eq!(all[4], lights[0]);
        // It stays on across a switch of cars, and lights nothing on a car without one.
        let next = Drive::new(DEFAULT_VEHICLE).unwrap();
        d.switch_car(next.spec, next.layout, &flat());
        assert!(d.underglow && d.layout.underglow.is_none());
        d.headlights = false;
        assert_eq!(d.lights(1.0).1, 0);
        // The switch turns it off again.
        input.set_just_pressed(Action::Underglow, true);
        d.frame(&input, true);
        assert!(!d.underglow);
    }

    #[test]
    fn the_cabin_draws_interior_wheel_and_needles() {
        for id in ["gannet_gt", "homestead_wagon"] {
            let d = Drive::new(id).unwrap();
            let ids = ids();
            let lod = ids.lods[0];
            let mut out = Vec::new();
            let glow = CarGlow {
                lamps: 0.5,
                instruments: 0.25,
                speedo: 2.0,
            };
            d.push_instances(0.5, &ids, Cabin::Full, glow, &mut out);
            let dials = d.layout.dial_count;
            assert_eq!(out.len(), 11 + dials, "{id}");
            assert!(out.len() <= MAX_CAR_INSTANCES);
            assert_eq!(out[7].mesh, lod.shell_inside);
            assert_eq!(out[8].mesh, lod.interior);
            assert_eq!(out[9].mesh, lod.steering_wheel);
            assert_eq!(out[10].mesh, ids.dial_ticks);
            for (k, n) in out[11..].iter().enumerate() {
                assert_eq!(n.mesh, ids.needles[k]);
            }
            // The outside is outside the cabin light; the lamps and instruments glow as
            // asked.
            assert!(out[..7].iter().all(|i| i.cabin == 0.0));
            assert!(out[7..].iter().all(|i| i.cabin == 1.0));
            // The speedometer's needle glows by the green wave on top.
            assert!(out[7..]
                .iter()
                .enumerate()
                .all(|(k, i)| i.glow == if k == 4 { 0.5 } else { 0.25 }));
            assert_eq!(out[2].glow, 0.5);
            assert!(out[7].color[0] < 0.5, "dark cabin walls");
            assert!(out.iter().skip(8).all(|i| i.color == [1.0; 4]));
            // Idling: the speedometer reads zero.
            let n = d.needles(1.0)[0];
            let dial = d.layout.dials[0];
            assert_eq!(n, dial.needle(0.0));
            // Hood only: the walls (windshield frame), no dashboard, wheel or gauges.
            out.clear();
            d.push_instances(0.5, &ids, Cabin::Frame, glow, &mut out);
            assert_eq!(out.len(), 8, "{id}");
            assert_eq!(out[7].mesh, lod.shell_inside);
        }
    }

    #[test]
    fn the_cabin_shows_from_every_camera() {
        let ids = ids();
        let lod = ids.lods[0];
        for id in [
            "seabreeze_convertible",
            "regent_coupe_cabrio",
            "halberd_targa",
        ] {
            let d = Drive::new(id).unwrap();
            assert!(d.layout.open_top, "{id}");
            let mut out = Vec::new();
            d.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
            // Outside: the interior and wheel after the wheels, lit like the outside, and
            // no inside-out shell.
            assert_eq!(out.len(), 9, "{id}");
            assert_eq!(out[7].mesh, lod.interior);
            assert_eq!(out[8].mesh, lod.steering_wheel);
            assert!(out
                .iter()
                .all(|i| i.cabin == 0.0 && i.mesh != lod.shell_inside));
            // The cockpit draws what a hardtop's does.
            out.clear();
            d.push_instances(0.5, &ids, Cabin::Full, CarGlow::default(), &mut out);
            assert_eq!(out.len(), 11 + d.layout.dial_count, "{id}");
            assert!(out.len() <= MAX_CAR_INSTANCES);
            assert_eq!(out[7].mesh, lod.shell_inside);
            // "Hood only" keeps to the walls, as for a hardtop.
            out.clear();
            d.push_instances(0.5, &ids, Cabin::Frame, CarGlow::default(), &mut out);
            assert_eq!(out.len(), 8, "{id}");
            assert_eq!(out[7].mesh, lod.shell_inside);
        }
        // A hardtop's cabin shows through its glass, which is see-through and casts no
        // shadow.
        let hardtop = Drive::new("homestead_wagon").unwrap();
        assert!(!hardtop.layout.open_top);
        let mut out = Vec::new();
        hardtop.push_instances(0.5, &ids, Cabin::Outside, CarGlow::default(), &mut out);
        assert_eq!(out.len(), 9);
        assert_eq!(
            (out[7].mesh, out[8].mesh),
            (lod.interior, lod.steering_wheel)
        );
        assert!(out[1].mesh == lod.glass && out[1].glass && !out[1].casts_shadow);
        assert_eq!(out.iter().filter(|i| i.glass).count(), 1);
    }

    /// The driver: every part from outside, no head or torso in the cockpit, nobody with
    /// "hood only"; within the car's instance budget, casting no shadow, hands moving with
    /// the steering wheel.
    #[test]
    fn the_driver_sits_at_the_wheel_from_every_camera() {
        let f = traffic::figure::build(&traffic::figure::FigureSpec::preset(
            0,
            traffic::figure::Era::Eighties,
        ));
        let driver = crate::driver::Driver {
            skeleton: f.skeleton,
            ids: core::array::from_fn(|k| MeshId(100 + k as u32)),
        };
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let ids = ids();
        let count = |d: &Drive, cabin| {
            let mut out = Vec::new();
            d.push_instances(0.5, &ids, cabin, CarGlow::default(), &mut out);
            let car = out.len();
            d.push_driver(0.5, &driver, cabin, 0.0, Vec3::ZERO, &mut out);
            assert!(out.len() <= MAX_CAR_INSTANCES);
            assert!(out[car..].iter().all(|i| !i.casts_shadow));
            out[car..].to_vec()
        };
        assert_eq!(count(&d, Cabin::Outside).len(), PARTS);
        let cockpit = count(&d, Cabin::Full);
        assert_eq!(cockpit.len(), PARTS - 2);
        assert!(cockpit.iter().all(|i| i.cabin == 1.0));
        let head = driver.ids[traffic::figure::Part::Head as usize];
        assert!(cockpit.iter().all(|i| i.mesh != head));
        assert!(count(&d, Cabin::Frame).is_empty());
        // Steering moves the right hand (the forearm's end) round with the rim; standing,
        // the left rests on the thigh, and comes back to the wheel under way.
        use traffic::figure::Part::{LowerArmL, LowerArmR};
        let hand = |d: &Drive, part: traffic::figure::Part| {
            let i = count(d, Cabin::Outside)[part as usize];
            Mat4::from_cols_array_2d(&i.transform).transform_point3(Vec3::NEG_Y * 0.3)
        };
        let straight = hand(&d, LowerArmR);
        let resting = hand(&d, LowerArmL);
        d.state.steer_angle_rad = 0.2;
        d.prev.steer_angle_rad = 0.2;
        assert!(
            hand(&d, LowerArmR).distance(straight) > 0.05,
            "the hands follow the wheel"
        );
        d.state.speed_mps = 5.0;
        d.prev.speed_mps = 5.0;
        let grip = d.layout.seating.grips(-0.2 * d.layout.steering_ratio)[0];
        assert!(hand(&d, LowerArmL).distance(grip) < resting.distance(grip) - 0.1);
    }

    #[test]
    fn a_car_that_leaves_the_world_starts_again() {
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        d.state.pos[1] = f32::NAN;
        d.step(VehicleInput::default(), &flat());
        assert_eq!(d.state, d.prev);
        assert_eq!(d.pose(1.0).pos, Vec3::new(-92.0, d.spec.cg_height_m, 4.9));
    }

    /// The coast's walls turn the car: from the foot of each land side's band, at full
    /// throttle straight at the side for 30 s, at rest or already at 45 m/s, every car stops
    /// at the face (the collider's wall in it) well short of the crest.
    #[test]
    fn the_coast_walls_turn_the_car() {
        use place::{EAST, NORTH, SOUTH};
        let place = crate::places::build(crate::PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        let ground = ground_for(&place);
        let ext = place.header.extent_m();
        let bands = place.terrain.edges.map(world::edges::band_m);
        for id in [DEFAULT_VEHICLE, "homestead_wagon"] {
            let runs = [NORTH, EAST, SOUTH]
                .into_iter()
                .flat_map(|s| [(s, 0.35), (s, 0.65)])
                .flat_map(|(s, f)| [(s, f, 0.0), (s, f, 45.0)]);
            for (side, f, speed) in runs {
                // At the band's foot, facing out.
                let (p, heading) = match side {
                    NORTH => ([f * ext[0], bands[NORTH] + 2.0], 0.0),
                    SOUTH => ([f * ext[0], ext[1] - bands[SOUTH] - 2.0], PI),
                    _ => ([ext[0] - bands[EAST] - 2.0, f * ext[1]], -FRAC_PI_2),
                };
                let (y, _) = ground.level_at(p);
                let mut d = Drive::new(id).unwrap();
                let (spec, layout) = (d.spec.clone(), d.layout);
                d = Drive::with_layout(spec, layout, [p[0], y, p[1]], heading);
                let (sin, cos) = heading.sin_cos();
                d.state.vel = [-sin * speed, 0.0, -cos * speed];
                d.state.speed_mps = speed;
                for w in &mut d.state.wheels {
                    w.spin = speed / vehicle::sim::WHEEL_RADIUS_M;
                }
                let input = VehicleInput {
                    throttle: 1.0,
                    ..VehicleInput::default()
                };
                let mut closest = f32::INFINITY;
                for _ in 0..(30.0 / DT) as usize {
                    d.step(input, &ground);
                    let q = d.state.pos;
                    let inside = world::edges::inside([q[0], q[2]], ext)[side];
                    closest = closest.min(inside);
                }
                eprintln!("{id} at {speed} m/s toward side {side} at {f}: {closest:.1} m from it");
                assert!(closest > 30.0, "{id} reached {closest} m from side {side}");
            }
        }
    }

    /// Off the beach into the sea at full throttle: the car wades in a little, stalls, and
    /// after [`DEEP_RESET_S`] in deep water goes back to its start, once.
    #[test]
    fn deep_water_stalls_the_car_and_puts_it_back() {
        let place = crate::places::build(crate::PlaceKey::Coast, skywave_core::Seed(1)).unwrap();
        let ground = ground_for(&place);
        // On the sand north of the pier, facing the sea (west).
        let p = [440.0, 940.0];
        let (y, _) = ground.level_at(p);
        assert!(y > place.terrain.water_level, "start on dry sand");
        let mut d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let (spec, layout) = (d.spec.clone(), d.layout);
        d = Drive::with_layout(spec, layout, [p[0], y, p[1]], FRAC_PI_2);
        let input = VehicleInput {
            throttle: 1.0,
            ..VehicleInput::default()
        };
        let (mut furthest, mut wet) = (f32::INFINITY, 0usize);
        let mut steps = 0;
        while d.water_resets == 0 {
            d.step(input, &ground);
            if d.water_resets == 0 {
                furthest = furthest.min(d.state.pos[0]);
                wet += usize::from(d.water_depth(&ground) > WADE_M);
            }
            steps += 1;
            assert!(
                steps < (20.0 / DT) as usize,
                "never reset; at {:?}",
                d.state.pos
            );
        }
        // Where the water starts, along the drive.
        let mut shore = p[0];
        while ground.water_depth([shore, p[1]], ground.level_at([shore, p[1]]).0) == 0.0 {
            shore -= 0.5;
        }
        eprintln!(
            "reset after {:.1} s, {:.1} s wet, {:.1} m past the waterline",
            steps as f32 * DT,
            wet as f32 * DT,
            shore - furthest
        );
        assert!(
            shore - furthest < 30.0,
            "drove {} m into the sea",
            shore - furthest
        );
        assert!(wet as f32 * DT >= DEEP_RESET_S);
        // Back at the start, at rest.
        assert_eq!(d.pose(1.0).pos.x, p[0]);
        assert!(d.state.speed_mps.abs() < 1e-3);
    }
}
