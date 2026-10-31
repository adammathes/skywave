//! `vehicle::sim`: the raycast-car rigid-body simulation (`docs/VEHICLES.md`, "Physics
//! model"), one fixed [`DT`] step at a time.
//!
//! One rigid body per car. Each step: filter inputs and steer; run the automatic gearbox and
//! engine; cast one suspension ray per wheel (spring, damper, bump stop, anti-roll bar);
//! turn slip into tire forces (peaked curve, combined slip, load sensitivity); spin the
//! wheels under drive, tire, brake and rolling-resistance torque; push the body's corners
//! out of the ground; add aero, gravity and the driver assists; integrate with
//! semi-implicit Euler.
//!
//! Conventions: world and body space are right-handed and Y-up; body origin is the center of
//! mass, `+x` right, `+y` up, `-z` forward. Wheels are indexed front-left, front-right,
//! rear-left, rear-right ([`VehicleSpec::wheel_positions`]). Signs: steer and lateral g are
//! positive to the right, wheel spin positive when rolling forward, heading positive turning
//! left (counter-clockwise seen from above), heading 0 faces `-z`.
//!
//! The step is plain arithmetic on plain structs: no allocation, no trait objects, no
//! transcendental functions from a platform library, so the same inputs give bit-identical
//! states everywhere and a C port is a transliteration.

// Indexed loops over the four wheels read like the C they may become; iterator chains would not.
#![allow(clippy::needless_range_loop)]

use crate::math::{
    abs, add, add_scaled, atan2, cos, cross, dot, length, normalize, q_from_yaw, q_integrate,
    rotate, rotate_inv, scale, sin, sqrt, sub, unit, ZERO,
};
use crate::spec::{Drive, VehicleSpec};

/// The fixed simulation step, s (`skywave_core::SIM_HZ` = 120).
pub const DT: f32 = 1.0 / 120.0;
/// m/s².
pub const GRAVITY: f32 = 9.81;
/// Tire rolling radius, m. Lives in the `ShapeSpec` once that exists; every car shares it now.
pub const WHEEL_RADIUS_M: f32 = 0.32;

/// Wheel indices, in [`VehicleSpec::wheel_positions`] order.
pub const FRONT_LEFT: usize = 0;
pub const FRONT_RIGHT: usize = 1;
pub const REAR_LEFT: usize = 2;
pub const REAR_RIGHT: usize = 3;

// Tuning constants shared by every car. Per-car character belongs in the spec; these are
// the physics the spec numbers are interpreted through.

/// Wheel, tire and brake rotor inertia per corner, kg·m².
const WHEEL_INERTIA: f32 = 1.2;
/// Engine and flywheel inertia, reflected to the driven wheels through the gearing when the
/// clutch is locked, kg·m².
const ENGINE_INERTIA: f32 = 0.12;
/// Share of engine torque that reaches the wheels.
const DRIVELINE_EFFICIENCY: f32 = 0.9;
/// kg/m³.
const AIR_DENSITY: f32 = 1.225;
/// Rolling-resistance coefficient (force per unit load).
const ROLLING_RESISTANCE: f32 = 0.015;
/// Spring compression at rest as a fraction of travel. The springs are preloaded so every
/// car rides here: ride height is a design choice, not a side effect of the spring rate.
const REST_FRACTION: f32 = 0.4;
/// Droop over which the spring preload fades in from full extension, m.
const PRELOAD_FADE_M: f32 = 0.02;
/// Bump-stop rate past full travel, as a multiple of the spring rate.
const BUMP_STOP: f32 = 8.0;
/// Tire force left at large slip, as a fraction of peak (`docs/VEHICLES.md`: "falloff to 70%").
const TAIL_GRIP: f32 = 0.7;
/// Normalized slip at which the falloff reaches [`TAIL_GRIP`].
const TAIL_SLIP: f32 = 3.0;
/// Peak slip ratio per radian of peak slip angle: a tire peaking late sideways also peaks
/// late under power and braking.
const SLIP_RATIO_PER_RAD: f32 = 0.8;
/// Bisection steps of the implicit longitudinal tire solve (2^-20 of the bracket).
const LONG_SOLVE_ITERS: u32 = 20;
/// Floor under the speed that slip is measured against, m/s; keeps slip finite near rest.
const SLIP_SPEED_FLOOR: f32 = 1.0;
/// Rear lateral grip is scaled by `1 - OVERSTEER_GAIN * oversteer_bias`.
const OVERSTEER_GAIN: f32 = 0.25;
/// Load-sensitivity factor bounds.
const LOAD_FACTOR_MIN: f32 = 0.5;
const LOAD_FACTOR_MAX: f32 = 1.25;
/// Share of the force that would stop a contact's sliding in one step that the tire may
/// apply. Below 1 the contact cannot overshoot, which makes it act as static friction at rest.
const CONTACT_CAP: f32 = 0.5;
/// Below this speed along the ground, m/s, a braked car is held by static friction: the
/// tire model's force fades to nothing with slip, so on its own a parked car creeps downhill.
const STATIC_SPEED: f32 = 0.2;
/// Closed-throttle engine braking: the drag it puts on the driven wheels through whatever
/// gear is in, as a share of the car's weight (g), at idle rpm and at the redline (linear
/// between). With aero and rolling resistance on top, every car coasts down at about
/// 0.15 g from 25 m/s in gear. Was 0.3 × peak torque at the redline, falling to nothing at
/// idle at the crank: ~0.05 g of coast-down in the high gears the automatic picks off
/// throttle, which read as a car that would not slow down unless braked.
const ENGINE_BRAKE_G: [f32; 2] = [0.13, 0.22];
/// Torque-converter stall: the engine can rev this far into its range against stopped wheels.
const STALL_FRACTION: f32 = 0.35;
/// How fast a slipping engine's rpm chases its target, 1/s.
const RPM_RATE: f32 = 12.0;
/// Pedal smoothing, 1/s.
const PEDAL_RATE: f32 = 20.0;
/// After a shift the automatic holds its gear this long, s, so it never hunts.
const SHIFT_HOLD_S: f32 = 0.6;
/// A downshift must land below this share of the upshift rpm (hysteresis).
const DOWNSHIFT_MARGIN: f32 = 0.8;
/// Below this forward speed the auto box may switch between drive and reverse, m/s.
const DIRECTION_SWITCH_SPEED: f32 = 0.8;
/// Brake held this long at a stop before the auto box selects reverse, s, so waiting at a
/// light with the brake down keeps the car still.
const REVERSE_DWELL_S: f32 = 0.5;
/// Steering lock reaches `max_deg * (1 - speed_sens)` at this speed, m/s.
const STEER_SENS_SPEED: f32 = 60.0;
/// Grip-limited steering (the `steering` assist) aims the front wheels this many times the
/// tires' peak slip angle off the front axle's direction of travel at full stick: the
/// fronts work at their peak whatever the speed, never the 20°+ that only scrubs them...
const STEER_LIMIT_MARGIN: f32 = 1.0;
/// ...fading in between these forward speeds, m/s, so full lock is always there for
/// parking and tight junctions.
const STEER_LIMIT_SPEED: [f32; 2] = [8.0, 11.0];
/// Reverse tops out at this speed, m/s; drive torque fades out over the last
/// `REVERSE_FADE_MPS` below it (reverse used to run up to first gear's ~20 m/s).
pub const REVERSE_MAX_MPS: f32 = 8.0;
const REVERSE_FADE_MPS: f32 = 1.5;
/// Traction control: proportional cut per unit of excess normalized slip, and its smoothing.
const TRACTION_GAIN: f32 = 2.0;
const TRACTION_RATE: f32 = 30.0;
/// ABS releases a wheel whose braking slip passes this multiple of the peak slip ratio.
const ABS_SLIP: f32 = 1.1;
/// Stability assist yaw-rate correction gain at full strength, 1/s.
const STABILITY_GAIN: f32 = 8.0;
/// Stability assist slide damping at full strength, 1/s: the share per second of the
/// sideways speed at the center of mass beyond the tires' peak slip angle that it takes
/// away (a car caught sliding wide stops sliding instead of carrying on sideways).
const SLIDE_GAIN: f32 = 4.0;
/// Handbrake pivot (part of the steering assist): with the handbrake held above
/// `HANDBRAKE_PIVOT_SPEED` m/s, yaw is helped toward `steer × HANDBRAKE_YAW_RATE` rad/s at
/// `HANDBRAKE_YAW_GAIN` 1/s, rotation only, never against it; the stability assist lets go
/// meanwhile. The rears lock either way; this turns the lock into a pivot.
const HANDBRAKE_YAW_RATE: f32 = 1.6;
const HANDBRAKE_YAW_GAIN: f32 = 6.0;
const HANDBRAKE_PIVOT_SPEED: f32 = 4.0;
/// Body-corner ground contact: stiffness and damping per corner per kg of car.
const BODY_STIFFNESS: f32 = 400.0;
const BODY_DAMPING: f32 = 14.0;
const BODY_FRICTION: f32 = 0.6;
/// Height of the lower body corners above the ground at rest, m.
const BODY_LOW_CORNER_M: f32 = 0.25;
/// How far above a body corner its ground probe starts, m.
const BODY_PROBE_M: f32 = 0.5;
/// Auto-right: seconds upset before it starts, the up·world-up below which the car counts as
/// upset, the value at which righting stops, the target roll rate (rad/s), its gain (1/s),
/// and the share of the car's weight lifted while righting.
const UPSET_DELAY_S: f32 = 3.0;
const UPSET_UP: f32 = 0.3;
const RIGHTED_UP: f32 = 0.9;
const RIGHT_RATE: f32 = 1.2;
const RIGHT_GAIN: f32 = 4.0;
const RIGHT_LIFT: f32 = 0.8;

const RAD_S_TO_RPM: f32 = 60.0 / (2.0 * crate::math::PI);
const DEG_TO_RAD: f32 = crate::math::PI / 180.0;

/// What the driver asks for this step, after the app's keyboard ramps and deadzones.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleInput {
    /// `[0, 1]`.
    pub throttle: f32,
    /// `[0, 1]`. Held at a standstill it selects reverse, where it becomes the throttle.
    pub brake: f32,
    /// `[-1, 1]`, positive steers right.
    pub steer: f32,
    pub handbrake: bool,
}

/// Driver assists (`docs/VEHICLES.md` step 7). `Default` is [`Assists::ARCADE`], the
/// tight, forgiving feel; [`Assists::LIGHT`] is the arcade-sim feel the game first shipped
/// with and [`Assists::OFF`] the "driver" feel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Assists {
    /// `[0, 1]`: yaw damping toward the yaw rate the steering asks for, and damping of a
    /// sideways slide past the tires' peak slip angle. Lets go while the handbrake is held.
    pub stability: f32,
    /// Driven-wheel slip cap, `[0, 1]`: 0 off, 1 holds wheelspin at the tire's peak.
    pub traction: f32,
    /// Anti-lock brakes on every car, whatever its spec says.
    pub abs: bool,
    /// Grip-limited steering, `[0, 1]`: blends the road-wheel target from the spec's
    /// speed-sensitive lock toward the front axle's direction of travel plus the stick's
    /// share of the tires' peak slip angle. In a steady corner that is the kinematic angle
    /// of the turn plus the peak slip (`atan(a · wheelbase / v²) + α_peak`, with the yaw the
    /// car actually has), so full stick at speed turns as hard as the fronts can and no
    /// harder; in a slide the travel direction swings out and the wheels follow it, a
    /// counter-steer. Fades in above 8 m/s. It also pivots the car on the handbrake.
    pub steering: f32,
    /// Tire grip multiplier, lateral and longitudinal (1 = the spec's tires).
    pub grip_boost: f32,
    /// Service-brake torque multiplier (1 = the spec's brakes; the handbrake is not boosted).
    pub brake_boost: f32,
}

impl Assists {
    pub const OFF: Assists = Assists {
        stability: 0.0,
        traction: 0.0,
        abs: false,
        steering: 0.0,
        grip_boost: 1.0,
        brake_boost: 1.0,
    };
    pub const LIGHT: Assists = Assists {
        stability: 0.3,
        traction: 0.5,
        abs: true,
        steering: 1.0,
        grip_boost: 1.0,
        brake_boost: 1.0,
    };
    /// After the first play on real hardware ("too slippy, braking too slow"): firm
    /// stability, strong traction control, ABS, grip-limited steering, a fifth more grip
    /// and 40% more brake.
    pub const ARCADE: Assists = Assists {
        stability: 0.8,
        traction: 0.8,
        abs: true,
        steering: 1.0,
        grip_boost: 1.2,
        brake_boost: 1.4,
    };
}

/// The road-wheel angle, rad (positive right), that steering input `steer` (`[-1, 1]`)
/// asks for in `state`: the spec's lock less its speed sensitivity, times the input; then,
/// by `assists.steering` and above 8 m/s forward, the front axle's direction of travel plus
/// `steer` times the tires' peak slip angle, within that lock.
pub fn steer_target(
    spec: &VehicleSpec,
    assists: &Assists,
    state: &VehicleState,
    steer: f32,
) -> f32 {
    let q = state.rot;
    let fwd = rotate(q, [0.0, 0.0, -1.0]);
    let fwd_speed = dot(state.vel, fwd);
    let speed_frac = unit(abs(fwd_speed) / STEER_SENS_SPEED);
    let lock = spec.steering.max_deg * DEG_TO_RAD * (1.0 - spec.steering.speed_sens * speed_frac);
    let plain = lock * steer;
    let [lo, hi] = STEER_LIMIT_SPEED;
    // Forward only: rolling backward the travel direction is behind the car.
    let ramp = unit((fwd_speed - lo) / (hi - lo));
    let k = unit(assists.steering) * ramp * ramp * (3.0 - 2.0 * ramp);
    if k <= 0.0 {
        return plain;
    }
    let right = rotate(q, [1.0, 0.0, 0.0]);
    let axle = rotate(q, [0.0, 0.0, -spec.cg_fore_aft * spec.wheelbase_m]);
    let v = add(state.vel, cross(state.ang_vel, axle));
    let travel = atan2(dot(v, right), dot(v, fwd));
    let mut aimed = travel + steer * STEER_LIMIT_MARGIN * spec.tires.slip_peak_deg * DEG_TO_RAD;
    if aimed > lock {
        aimed = lock;
    } else if aimed < -lock {
        aimed = -lock;
    }
    plain + (aimed - plain) * k
}

impl Default for Assists {
    fn default() -> Self {
        Assists::ARCADE
    }
}

/// A ray hit on the static world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Distance along the ray, m, in `[0, max_len]`. 0 if the ray starts inside the ground.
    pub t: f32,
    /// Unit surface normal.
    pub normal: [f32; 3],
    /// Surface grip multiplier: 1 dry asphalt, lower for dirt, grass, wet.
    pub friction: f32,
}

/// The static world as the sim sees it: something rays can hit.
pub trait Ground {
    /// First hit along `origin + t * dir` (`dir` unit length) for `t` in `[0, max_len]`.
    fn probe(&self, origin: [f32; 3], dir: [f32; 3], max_len: f32) -> Option<Hit>;
}

/// An infinite horizontal plane at height `y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatGround {
    pub y: f32,
    pub friction: f32,
}

impl Ground for FlatGround {
    fn probe(&self, origin: [f32; 3], dir: [f32; 3], max_len: f32) -> Option<Hit> {
        if dir[1] > -1e-6 {
            return None;
        }
        let t = (origin[1] - self.y) / -dir[1];
        if t > max_len {
            return None;
        }
        Some(Hit {
            t: if t > 0.0 { t } else { 0.0 },
            normal: [0.0, 1.0, 0.0],
            friction: self.friction,
        })
    }
}

/// One wheel's state and its per-step outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct WheelState {
    /// Spring compression, m: 0 fully extended, `travel_m` at the bump stop. 0 when airborne.
    pub compression: f32,
    /// Angular speed, rad/s; positive rolls the car forward.
    pub spin: f32,
    /// `(surface speed - ground speed) / ground speed`: positive under wheelspin, -1 locked.
    pub slip_ratio: f32,
    /// Angle between where the tire points and where it goes, rad; positive sliding right.
    pub slip_angle: f32,
    /// Combined slip relative to the tire's peak: below 1 it grips, above it slides (audio,
    /// tire marks).
    pub slip: f32,
    /// Normal load, N.
    pub load: f32,
    /// Contact point and ground normal, world space; valid when `grounded`.
    pub contact: [f32; 3],
    pub normal: [f32; 3],
    pub grounded: bool,
}

/// Everything that changes while a car drives. Plain data: copy it to save or rewind.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct VehicleState {
    /// Center of mass, world space, m.
    pub pos: [f32; 3],
    /// Body-to-world rotation, unit quaternion `[x, y, z, w]`.
    pub rot: [f32; 4],
    /// m/s, world space.
    pub vel: [f32; 3],
    /// rad/s, world space.
    pub ang_vel: [f32; 3],
    pub wheels: [WheelState; 4],
    pub engine_rpm: f32,
    /// -1 reverse, 0 neutral, 1..=n forward.
    pub gear: i8,
    /// Counts down after a gear change, s: torque is cut for its first `shift_ms`, and the
    /// box holds the gear until it reaches 0.
    pub shift_timer: f32,
    /// How long the brake has been held at a stop in a forward gear, s; reverse engages at
    /// `REVERSE_DWELL_S`. Zero whenever the car moves or the brake is released.
    pub reverse_hold_s: f32,
    /// Smoothed pedals in `[0, 1]` and the clamped steer input in `[-1, 1]`.
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
    /// Front road-wheel angle, rad, positive right.
    pub steer_angle_rad: f32,
    pub handbrake: bool,
    /// Traction-control torque multiplier, `[0, 1]` (1 = no cut).
    pub traction_cut: f32,
    /// How long the car has been on its side or roof, s.
    pub upset_s: f32,
    /// Distance driven, m.
    pub odometer_m: f64,
    /// Forward speed, m/s (negative reversing).
    pub speed_mps: f32,
    /// Acceleration felt in the cabin over the last step, in g: lateral (positive right) and
    /// longitudinal (positive forward). For the camera and seat-of-the-pants cues.
    pub lat_g: f32,
    pub long_g: f32,
}

/// Number of 64-bit words [`VehicleState::hash`] absorbs.
const HASH_WORDS: usize = 13 + 4 * 13 + 14;

impl VehicleState {
    /// A car at rest: `ground` is the point on the ground under its center of mass, the body
    /// level and facing `heading_rad`, springs at rest compression, engine idling in first.
    pub fn spawn(spec: &VehicleSpec, ground: [f32; 3], heading_rad: f32) -> VehicleState {
        let rot = q_from_yaw(heading_rad);
        let pos = [ground[0], ground[1] + spec.cg_height_m, ground[2]];
        let loads = static_loads(spec);
        let contacts = spec.wheel_positions();
        let mut wheels = [WheelState {
            compression: REST_FRACTION * spec.suspension.travel_m,
            spin: 0.0,
            slip_ratio: 0.0,
            slip_angle: 0.0,
            slip: 0.0,
            load: 0.0,
            contact: ZERO,
            normal: [0.0, 1.0, 0.0],
            grounded: true,
        }; 4];
        for i in 0..4 {
            wheels[i].load = loads[i];
            wheels[i].contact = add(pos, rotate(rot, contacts[i]));
        }
        VehicleState {
            pos,
            rot,
            vel: ZERO,
            ang_vel: ZERO,
            wheels,
            engine_rpm: spec.engine.idle_rpm,
            gear: if spec.gears.is_empty() { 0 } else { 1 },
            shift_timer: 0.0,
            reverse_hold_s: 0.0,
            throttle: 0.0,
            brake: 0.0,
            steer: 0.0,
            steer_angle_rad: 0.0,
            handbrake: false,
            traction_cut: 1.0,
            upset_s: 0.0,
            odometer_m: 0.0,
            speed_mps: 0.0,
            lat_g: 0.0,
            long_g: 0.0,
        }
    }

    /// Body `+x` in world space.
    pub fn right(&self) -> [f32; 3] {
        rotate(self.rot, [1.0, 0.0, 0.0])
    }

    /// Body `+y` in world space.
    pub fn up(&self) -> [f32; 3] {
        rotate(self.rot, [0.0, 1.0, 0.0])
    }

    /// Body `-z` in world space.
    pub fn forward(&self) -> [f32; 3] {
        rotate(self.rot, [0.0, 0.0, -1.0])
    }

    /// `skywave_core::hash64` of the bit pattern of every field: the odometer seeds it, the
    /// rest are absorbed in declaration order. Two states hash equal exactly when they are
    /// bit-identical (up to NaN payloads).
    pub fn hash(&self) -> u64 {
        let mut words = [0u64; HASH_WORDS];
        let mut n = 0;
        let mut put = |x: f32| {
            words[n] = x.to_bits() as u64;
            n += 1;
        };
        for v in self
            .pos
            .iter()
            .chain(&self.rot)
            .chain(&self.vel)
            .chain(&self.ang_vel)
        {
            put(*v);
        }
        for w in &self.wheels {
            put(w.compression);
            put(w.spin);
            put(w.slip_ratio);
            put(w.slip_angle);
            put(w.slip);
            put(w.load);
            for v in w.contact.iter().chain(&w.normal) {
                put(*v);
            }
            put(if w.grounded { 1.0 } else { 0.0 });
        }
        put(self.engine_rpm);
        put(self.gear as f32);
        put(self.shift_timer);
        put(self.reverse_hold_s);
        put(self.throttle);
        put(self.brake);
        put(self.steer);
        put(self.steer_angle_rad);
        put(if self.handbrake { 1.0 } else { 0.0 });
        put(self.traction_cut);
        put(self.upset_s);
        put(self.speed_mps);
        put(self.lat_g);
        put(self.long_g);
        let odometer = self.odometer_m.to_bits();
        skywave_core::hash64(odometer, &words)
    }
}

/// Static normal load per wheel on level ground, N.
fn static_loads(spec: &VehicleSpec) -> [f32; 4] {
    let weight = spec.mass_kg * GRAVITY;
    let front = 0.5 * weight * (1.0 - spec.cg_fore_aft);
    let rear = 0.5 * weight * spec.cg_fore_aft;
    [front, front, rear, rear]
}

/// Share of drive torque per wheel; sums to 1.
fn drive_shares(drive: Drive) -> [f32; 4] {
    match drive {
        Drive::Front => [0.5, 0.5, 0.0, 0.0],
        Drive::Rear => [0.0, 0.0, 0.5, 0.5],
        Drive::All => [0.2, 0.2, 0.3, 0.3],
    }
}

/// Overall ratio from crank to wheel for `gear`, signed (negative in reverse), 0 in neutral.
fn overall_ratio(spec: &VehicleSpec, gear: i8) -> f32 {
    if gear > 0 {
        match spec.gears.get(gear as usize - 1) {
            Some(r) => r * spec.final_drive,
            None => 0.0,
        }
    } else if gear < 0 {
        -spec.reverse * spec.final_drive
    } else {
        0.0
    }
}

/// The tire's two-parameter peaked curve over slip normalized to the peak (`s >= 0`): rises
/// almost linearly to 1 at `s = 1`, then eases down to [`TAIL_GRIP`] by [`TAIL_SLIP`].
pub fn tire_curve(s: f32) -> f32 {
    if s <= 1.0 {
        s * (1.5 - 0.5 * s * s)
    } else if s < TAIL_SLIP {
        let t = (s - 1.0) / (TAIL_SLIP - 1.0);
        1.0 - (1.0 - TAIL_GRIP) * t * t * (3.0 - 2.0 * t)
    } else {
        TAIL_GRIP
    }
}

fn clamp_abs(x: f32, limit: f32) -> f32 {
    if x > limit {
        limit
    } else if x < -limit {
        -limit
    } else {
        x
    }
}

/// Reduces `|spin|` by `dw` without letting it change sign (a friction torque).
fn friction_spin(spin: f32, dw: f32) -> f32 {
    if spin > dw {
        spin - dw
    } else if spin < -dw {
        spin + dw
    } else {
        0.0
    }
}

/// One tire's grip at this step's load, for the longitudinal solve.
struct Tire {
    mu_x: f32,
    mu_y: f32,
    load: f32,
    sr_peak: f32,
    alpha_peak: f32,
    /// Ground speed slip is measured against, m/s.
    ref_speed: f32,
}

impl Tire {
    /// Longitudinal force at slip velocity `slip_v` (m/s) with slip angle `alpha` held.
    fn force_long(&self, slip_v: f32, alpha: f32) -> f32 {
        let sx = slip_v / self.ref_speed / self.sr_peak;
        let sy = alpha / self.alpha_peak;
        let s = sqrt(sx * sx + sy * sy);
        if s > 1e-6 {
            self.mu_x * self.load * tire_curve(s) * sx / s
        } else {
            0.0
        }
    }

    /// Backward-Euler solve of the contact along the tire: the slip velocity `s` after the
    /// step satisfies `s = slip_pre - force(s) * k`, where `k` is how much one newton for one
    /// step closes the slip (wheel and body together). Fixed-count bisection between 0 and
    /// `slip_pre`, so the answer is bracketed and bit-reproducible. Returns `(force, s)`.
    fn solve_long(&self, slip_pre: f32, k: f32, alpha: f32) -> (f32, f32) {
        let a = abs(slip_pre);
        let sign = if slip_pre < 0.0 { -1.0 } else { 1.0 };
        let (mut lo, mut hi) = (0.0, a);
        for _ in 0..LONG_SOLVE_ITERS {
            let mid = 0.5 * (lo + hi);
            if mid - a + self.force_long(mid, alpha) * k > 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let s = 0.5 * (lo + hi);
        (sign * self.force_long(s, alpha), sign * s)
    }
}

/// Advances `state` by one [`DT`] step.
pub fn step(
    spec: &VehicleSpec,
    state: &mut VehicleState,
    input: &VehicleInput,
    assists: &Assists,
    ground: &impl Ground,
) {
    // Per-spec numbers. Cheap enough to derive every step, which keeps the API one call.
    let mass = spec.mass_kg;
    let inertia = spec.inertia_box();
    let susp = &spec.suspension;
    let tires = &spec.tires;
    let travel = susp.travel_m;
    let rest_c = REST_FRACTION * travel;
    let radius = WHEEL_RADIUS_M;
    let ray_len = travel + radius;
    let loads0 = static_loads(spec);
    let mean_load = 0.25 * mass * GRAVITY;
    let contacts0 = spec.wheel_positions();
    let shares = drive_shares(spec.drive);
    let alpha_peak = tires.slip_peak_deg * DEG_TO_RAD;
    let sr_peak = SLIP_RATIO_PER_RAD * alpha_peak;
    let rear_lat = 1.0 - OVERSTEER_GAIN * spec.feel.oversteer_bias;
    let idle = spec.engine.idle_rpm;
    let redline = spec.engine.redline_rpm;
    let rpm_span = redline - idle;
    let grip_boost = assists.grip_boost;

    // Body frame at the start of the step.
    let q = state.rot;
    let right = rotate(q, [1.0, 0.0, 0.0]);
    let up = rotate(q, [0.0, 1.0, 0.0]);
    let fwd = rotate(q, [0.0, 0.0, -1.0]);
    let fwd_speed = dot(state.vel, fwd);

    // 1. Inputs. Pedals are smoothed lightly; the steering filter is the road-wheel chase.
    let k = PEDAL_RATE * DT;
    state.throttle += (unit(input.throttle) - state.throttle) * k;
    state.brake += (unit(input.brake) - state.brake) * k;
    state.steer = if input.steer > 0.0 {
        unit(input.steer)
    } else {
        -unit(-input.steer)
    };
    state.handbrake = input.handbrake;
    let target = steer_target(spec, assists, state, state.steer);
    let chase = unit(spec.steering.return_rate * DT);
    state.steer_angle_rad += (target - state.steer_angle_rad) * chase;

    // 2. Gearbox and engine (automatic; `spec.auto == false` shifts the same until manual
    // mode exists). Stopped + brake held `REVERSE_DWELL_S` selects reverse; stopped + throttle
    // selects drive.
    let shift_s = spec.shift_ms * 0.001 + SHIFT_HOLD_S;
    let n_gears = spec.gears.len() as i8;
    if state.shift_timer > 0.0 {
        state.shift_timer -= DT;
        if state.shift_timer < 0.0 {
            state.shift_timer = 0.0;
        }
    }
    let stopped = abs(fwd_speed) < DIRECTION_SWITCH_SPEED;
    if stopped && state.gear >= 0 && state.brake > 0.2 && state.throttle < 0.05 {
        state.reverse_hold_s += DT;
    } else {
        state.reverse_hold_s = 0.0;
    }
    if stopped {
        if state.reverse_hold_s >= REVERSE_DWELL_S {
            state.gear = -1;
            state.shift_timer = shift_s;
        } else if state.gear <= 0 && state.throttle > 0.2 && state.brake < 0.05 && n_gears > 0 {
            state.gear = 1;
            state.shift_timer = shift_s;
        }
    }
    let (drive_pedal, brake_pedal) = if state.gear < 0 {
        (state.brake, state.throttle)
    } else {
        (state.throttle, state.brake)
    };

    let mut driven_spin = 0.0;
    for i in 0..4 {
        driven_spin += shares[i] * state.wheels[i].spin;
    }
    let mut ratio = overall_ratio(spec, state.gear);
    let mut wheel_rpm = driven_spin * ratio * RAD_S_TO_RPM;
    if state.gear >= 1 && state.shift_timer <= 0.0 {
        let up_rpm = idle + rpm_span * (0.5 + 0.44 * drive_pedal);
        let down_rpm = idle + rpm_span * (0.15 + 0.4 * drive_pedal);
        let g = state.gear as usize;
        if state.gear < n_gears && wheel_rpm > up_rpm {
            state.gear += 1;
            state.shift_timer = shift_s;
        } else if g > 1
            && wheel_rpm < down_rpm
            && wheel_rpm * (spec.gears[g - 2] / spec.gears[g - 1]) < DOWNSHIFT_MARGIN * up_rpm
        {
            state.gear -= 1;
            state.shift_timer = shift_s;
        }
        ratio = overall_ratio(spec, state.gear);
        wheel_rpm = driven_spin * ratio * RAD_S_TO_RPM;
    }
    let shifting = state.shift_timer > SHIFT_HOLD_S;
    let launch_rpm = idle + drive_pedal * STALL_FRACTION * rpm_span;
    // The handbrake declutches, so it locks the rears instead of fighting the engine.
    let declutched = shifting || state.handbrake;
    let coupled = state.gear != 0 && !declutched && wheel_rpm >= launch_rpm;
    if coupled {
        state.engine_rpm = wheel_rpm;
    } else {
        let target = if state.gear != 0 && wheel_rpm > launch_rpm {
            wheel_rpm
        } else {
            launch_rpm
        };
        state.engine_rpm += (target - state.engine_rpm) * unit(RPM_RATE * DT);
    }

    // Traction control watches last step's driven-wheel slip.
    let mut tc_target = 1.0;
    if assists.traction > 0.0 && ratio != 0.0 {
        let dir = if ratio > 0.0 { 1.0 } else { -1.0 };
        let mut worst = 0.0;
        for i in 0..4 {
            let s = state.wheels[i].slip_ratio * dir;
            if shares[i] > 0.0 && state.wheels[i].grounded && s > worst {
                worst = s;
            }
        }
        let allowed = sr_peak * (1.0 + 2.0 * (1.0 - unit(assists.traction)));
        if worst > allowed {
            tc_target =
                1.0 / (1.0 + TRACTION_GAIN * unit(assists.traction) * (worst - allowed) / sr_peak);
        }
    }
    state.traction_cut += (tc_target - state.traction_cut) * unit(TRACTION_RATE * DT);

    let mut engine_torque = 0.0;
    if state.engine_rpm < redline {
        engine_torque = spec.engine.torque_at(state.engine_rpm) * drive_pedal * state.traction_cut;
    }
    if state.gear < 0 {
        // Reverse is for parking: the drive fades out toward the reverse speed cap.
        engine_torque *= unit((REVERSE_MAX_MPS + fwd_speed) / REVERSE_FADE_MPS);
    }
    if coupled {
        // Engine braking is set as a drag at the wheels and taken back to the crank through
        // the gear in, so it is the same share of the car's weight in every gear.
        let rev = unit((state.engine_rpm - idle) / rpm_span);
        let [lo, hi] = ENGINE_BRAKE_G;
        let wheel_drag = (lo + (hi - lo) * rev) * mass * GRAVITY * radius;
        engine_torque -= wheel_drag / (abs(ratio) * DRIVELINE_EFFICIENCY) * (1.0 - drive_pedal);
    }
    let axle_torque = if declutched {
        0.0
    } else {
        engine_torque * ratio * DRIVELINE_EFFICIENCY
    };
    let reflected_inertia = if coupled {
        ENGINE_INERTIA * ratio * ratio
    } else {
        0.0
    };

    // 3. Suspension: one ray per wheel from the top of its travel, straight down the body.
    let down = scale(up, -1.0);
    let mount_lift = radius + travel - rest_c;
    let mut spring = [0.0f32; 4];
    let mut origin = [ZERO; 4];
    let mut friction = [0.0f32; 4];
    for i in 0..4 {
        let w = &mut state.wheels[i];
        let mount = [
            contacts0[i][0],
            contacts0[i][1] + mount_lift,
            contacts0[i][2],
        ];
        origin[i] = add(state.pos, rotate(q, mount));
        w.grounded = false;
        w.compression = 0.0;
        if let Some(hit) = ground.probe(origin[i], down, ray_len) {
            let c_raw = ray_len - hit.t;
            let c = if c_raw < travel { c_raw } else { travel };
            let bump = c_raw - c;
            // Compression rate: the mount's approach to the ground along its normal, mapped
            // onto the ray. Sliding along the ground does not compress the spring.
            let v_mount = add(state.vel, cross(state.ang_vel, sub(origin[i], state.pos)));
            let d_n = dot(down, hit.normal);
            let c_rate = if d_n < -0.1 {
                dot(v_mount, hit.normal) / d_n
            } else {
                0.0
            };
            // The preload fades in over the last few cm of droop so touchdown is not a step.
            let preload = (loads0[i] - susp.spring_n_m * rest_c) * unit(c / PRELOAD_FADE_M);
            spring[i] = preload + susp.spring_n_m * (c + BUMP_STOP * bump) + susp.damper * c_rate;
            w.grounded = true;
            w.compression = c;
            w.contact = add_scaled(origin[i], down, hit.t);
            w.normal = hit.normal;
            friction[i] = hit.friction;
        }
    }
    let bar = susp.antiroll * susp.spring_n_m;
    for (l, r) in [(FRONT_LEFT, FRONT_RIGHT), (REAR_LEFT, REAR_RIGHT)] {
        if state.wheels[l].grounded && state.wheels[r].grounded {
            let d = bar * (state.wheels[l].compression - state.wheels[r].compression);
            spring[l] += d;
            spring[r] -= d;
        }
    }

    let mut force = ZERO;
    let mut torque = ZERO;
    let mut total_load = 0.0;
    // What the tires could hold the body with at rest (step 10): the brakes' grip along the
    // ground, every tire's grip across it, and the mean ground normal.
    let mut tire_force = ZERO;
    let mut hold_long = 0.0;
    let mut hold_lat = 0.0;
    let mut normal_sum = ZERO;
    for i in 0..4 {
        let w = &mut state.wheels[i];
        if !w.grounded {
            w.load = 0.0;
            continue;
        }
        if spring[i] < 0.0 {
            spring[i] = 0.0;
        }
        let f = scale(up, spring[i]);
        force = add(force, f);
        torque = add(torque, cross(sub(w.contact, state.pos), f));
        let n_dot = dot(up, w.normal);
        w.load = if n_dot > 0.0 { spring[i] * n_dot } else { 0.0 };
        total_load += w.load;
    }

    // 4. Tires and wheel spin. Lateral force is explicit (capped so it cannot overshoot);
    // the stiff wheel/tire/body loop along the tire is solved implicitly, so a light wheel
    // under a big drive or brake impulse settles on its true slip in one step.
    let brake_nm = spec.brakes.max_nm * assists.brake_boost;
    let brake_front = 0.5 * brake_nm * spec.brakes.bias_front * brake_pedal;
    let brake_rear = 0.5 * brake_nm * (1.0 - spec.brakes.bias_front) * brake_pedal;
    let handbrake = if state.handbrake {
        0.5 * spec.brakes.handbrake_nm
    } else {
        0.0
    };
    let abs_on = assists.abs || spec.brakes.abs;
    let (sa, ca) = (sin(state.steer_angle_rad), cos(state.steer_angle_rad));
    for i in 0..4 {
        let front = i < 2;
        let w = &mut state.wheels[i];
        let inertia_w = WHEEL_INERTIA + reflected_inertia * shares[i];
        let drive = axle_torque * shares[i];

        // Brake torque; ABS eases off a wheel that was locking last step.
        let mut brake = if front { brake_front } else { brake_rear };
        if abs_on && brake > 0.0 {
            let travel_dir = if fwd_speed < 0.0 { -1.0 } else { 1.0 };
            let lock = -w.slip_ratio * travel_dir / sr_peak;
            if lock > ABS_SLIP {
                brake *= 0.1 + 0.9 * unit(1.0 - (lock - ABS_SLIP) / ABS_SLIP);
            }
        }
        if !front {
            brake += handbrake;
        }
        let held = brake - abs(drive);
        brake += ROLLING_RESISTANCE * w.load * radius;

        if !(w.grounded && w.load > 0.0) {
            w.slip_ratio = 0.0;
            w.slip_angle = 0.0;
            w.slip = 0.0;
            let spin = w.spin + drive * DT / inertia_w;
            w.spin = friction_spin(spin, brake * DT / inertia_w);
            continue;
        }
        let n = w.normal;
        let heading = if front {
            rotate(q, [sa, 0.0, -ca])
        } else {
            fwd
        };
        let f_axis = normalize(add_scaled(heading, n, -dot(heading, n)));
        let s_axis = cross(f_axis, n);
        let v_c = add(state.vel, cross(state.ang_vel, sub(w.contact, state.pos)));
        let v_long = dot(v_c, f_axis);
        let v_lat = dot(v_c, s_axis);

        let load_factor = 1.0 - tires.load_sens * (w.load - mean_load) / mean_load;
        let load_factor =
            load_factor.clamp(LOAD_FACTOR_MIN, LOAD_FACTOR_MAX) * friction[i] * grip_boost;
        let tire = Tire {
            mu_x: tires.grip_long * load_factor,
            mu_y: tires.grip_lat * load_factor * if front { 1.0 } else { rear_lat },
            load: w.load,
            sr_peak,
            alpha_peak,
            ref_speed: if abs(v_long) > SLIP_SPEED_FLOOR {
                abs(v_long)
            } else {
                SLIP_SPEED_FLOOR
            },
        };
        let m_eff = mass * w.load / total_load;
        let alpha = atan2(v_lat, tire.ref_speed);

        // Brakes oppose the way the wheel turns. A stopped wheel turns the way the drive
        // pushes it if the drive beats the brake, otherwise the brake holds it against the
        // way the ground drags it.
        let turning = if abs(w.spin) > 0.01 {
            w.spin
        } else if abs(drive) > brake {
            drive
        } else {
            v_long
        };
        let brake_dir = if turning > 0.0 {
            1.0
        } else if turning < 0.0 {
            -1.0
        } else {
            0.0
        };
        let torque_w = drive - brake * brake_dir;
        let slip_pre = (w.spin + torque_w * DT / inertia_w) * radius - v_long;
        let k_roll = DT * (radius * radius / inertia_w + 1.0 / m_eff);
        let (mut fx, mut slip_v) = tire.solve_long(slip_pre, k_roll, alpha);
        let mut spin = w.spin + (torque_w - fx * radius) * DT / inertia_w;
        if brake > 0.0 && spin * brake_dir < 0.0 {
            // The brake would turn the wheel backwards: it locks, and the body alone
            // answers the tire.
            spin = 0.0;
            (fx, slip_v) = tire.solve_long(-v_long, DT / m_eff, alpha);
        }
        w.spin = spin;

        let sx = slip_v / tire.ref_speed / sr_peak;
        let sy = alpha / alpha_peak;
        let s = sqrt(sx * sx + sy * sy);
        let mut fy = 0.0;
        if s > 1e-6 {
            fy = -tire.mu_y * tire.load * tire_curve(s) * sy / s;
        }
        fy = clamp_abs(fy, CONTACT_CAP * m_eff * abs(v_lat) / DT);
        w.slip_ratio = slip_v / tire.ref_speed;
        w.slip_angle = alpha;
        w.slip = s;

        let f = add(scale(f_axis, fx), scale(s_axis, fy));
        force = add(force, f);
        torque = add(torque, cross(sub(w.contact, state.pos), f));

        tire_force = add(tire_force, f);
        if held > 0.0 {
            let grip = tire.mu_x * tire.load;
            hold_long += if held / radius < grip {
                held / radius
            } else {
                grip
            };
        }
        hold_lat += tire.mu_y * tire.load;
        normal_sum = add(normal_sum, n);
    }

    // 5. Body corners against the ground: lets a rolled car land and slide instead of
    // falling through. Buildings, curbs and props are the collision task's.
    let [len, wid, hgt] = spec.dims_m;
    let low = -spec.cg_height_m + BODY_LOW_CORNER_M;
    let high = -spec.cg_height_m + BODY_LOW_CORNER_M + hgt;
    for c in 0..8 {
        let corner = [
            if c & 1 == 0 { -0.48 * wid } else { 0.48 * wid },
            if c & 2 == 0 { low } else { high },
            if c & 4 == 0 { -0.48 * len } else { 0.48 * len },
        ];
        let p = add(state.pos, rotate(q, corner));
        let start = [p[0], p[1] + BODY_PROBE_M, p[2]];
        if let Some(hit) = ground.probe(start, [0.0, -1.0, 0.0], BODY_PROBE_M) {
            let depth = BODY_PROBE_M - hit.t;
            let n = hit.normal;
            let r = sub(p, state.pos);
            let v_p = add(state.vel, cross(state.ang_vel, r));
            let vn = dot(v_p, n);
            let mut normal_f = mass * (BODY_STIFFNESS * depth - BODY_DAMPING * vn);
            if normal_f < 0.0 {
                normal_f = 0.0;
            }
            let v_t = add_scaled(v_p, n, -vn);
            let speed_t = length(v_t);
            let mut ft = BODY_FRICTION * hit.friction * normal_f;
            let cap = CONTACT_CAP * 0.25 * mass * speed_t / DT;
            if ft > cap {
                ft = cap;
            }
            let f = add_scaled(scale(n, normal_f), normalize(v_t), -ft);
            force = add(force, f);
            torque = add(torque, cross(r, f));
        }
    }

    // 6. Aero: drag against the velocity, downforce along the body's down.
    let speed = length(state.vel);
    let qbar = 0.5 * AIR_DENSITY * spec.aero.frontal_m2 * speed;
    force = add_scaled(force, state.vel, -qbar * spec.aero.cd);
    force = add_scaled(force, up, -qbar * speed * spec.aero.downforce_coef);

    // 7. Stability assist: damp yaw beyond what the steering asks for (oversteer only), and
    // a sideways slide beyond the tires' peak slip angle. The handbrake asks for a slide, so
    // it lets go while that is held, and the steering assist pivots the car instead.
    let grounded = state.wheels.iter().any(|w| w.grounded);
    let yaw = dot(state.ang_vel, up);
    let gate = unit(abs(fwd_speed) / 5.0);
    if assists.stability > 0.0 && abs(fwd_speed) > 2.0 && !state.handbrake {
        let wheelbase = spec.wheelbase_m;
        let (s, c) = (sin(state.steer_angle_rad), cos(state.steer_angle_rad));
        let mut wanted = -fwd_speed * s / (c * wheelbase);
        let limit = tires.grip_lat * grip_boost * GRAVITY / abs(fwd_speed);
        wanted = clamp_abs(wanted, limit);
        let excess = yaw - wanted;
        if yaw * excess > 0.0 || yaw * wanted < 0.0 {
            let t = -unit(assists.stability) * STABILITY_GAIN * excess * inertia[1] * gate;
            torque = add_scaled(torque, up, t);
        }
        if grounded {
            let side = dot(state.vel, right);
            let allowed = abs(fwd_speed) * sin(alpha_peak) / cos(alpha_peak);
            let over = if side > allowed {
                side - allowed
            } else if side < -allowed {
                side + allowed
            } else {
                0.0
            };
            let f = -unit(assists.stability) * SLIDE_GAIN * mass * over * gate;
            force = add_scaled(force, right, f);
        }
    }
    if assists.steering > 0.0 && state.handbrake && grounded && fwd_speed > HANDBRAKE_PIVOT_SPEED {
        // Steering right (positive) is a clockwise, negative yaw.
        let target = -state.steer * HANDBRAKE_YAW_RATE * unit(assists.steering);
        if target * yaw >= 0.0 && abs(yaw) < abs(target) {
            let t = HANDBRAKE_YAW_GAIN * (target - yaw) * inertia[1] * gate;
            torque = add_scaled(torque, up, t);
        }
    }

    // 8. Auto-right: after a few seconds on its side or roof, a gentle hand rolls the car back.
    if up[1] < UPSET_UP {
        state.upset_s += DT;
    } else if up[1] > RIGHTED_UP {
        state.upset_s = 0.0;
    }
    if state.upset_s >= UPSET_DELAY_S {
        let mut axis = cross(up, [0.0, 1.0, 0.0]);
        if length(axis) < 0.05 {
            axis = [fwd[0], 0.0, fwd[2]];
        }
        let target = scale(normalize(axis), RIGHT_RATE);
        let i_mean = (inertia[0] + inertia[1] + inertia[2]) / 3.0;
        torque = add_scaled(torque, sub(target, state.ang_vel), RIGHT_GAIN * i_mean);
        force[1] += RIGHT_LIFT * mass * GRAVITY;
    }

    // 9. Integrate (semi-implicit Euler: velocities first, then positions with them).
    let acc = scale(force, 1.0 / mass);
    state.lat_g = dot(acc, right) / GRAVITY;
    state.long_g = dot(acc, fwd) / GRAVITY;
    state.vel = add_scaled(state.vel, acc, DT);
    state.vel[1] -= GRAVITY * DT;
    let mut w_body = rotate_inv(q, state.ang_vel);
    let t_body = rotate_inv(q, torque);
    for a in 0..3 {
        w_body[a] += t_body[a] / inertia[a] * DT;
    }
    state.ang_vel = rotate(q, w_body);

    // 10. Static friction. Near rest a braked car's tires hold it: if stopping its motion
    // along the ground this step (before the tires' own force) needs no more than the brakes
    // and tires can give, it stops; otherwise it slides on the tire model as before.
    // A pressed throttle asks the car to move, so the hold lets go (the handbrake declutches).
    if hold_long > 0.0 && (declutched || drive_pedal < 0.05) && length(normal_sum) > 0.0 {
        let n = normalize(normal_sum);
        let v_pre = add_scaled(state.vel, tire_force, -DT / mass);
        let v_t = add_scaled(v_pre, n, -dot(v_pre, n));
        if length(v_t) < STATIC_SPEED {
            let f_axis = normalize(add_scaled(fwd, n, -dot(fwd, n)));
            let s_axis = cross(f_axis, n);
            let need_long = mass * dot(v_t, f_axis) / DT / hold_long;
            let need_lat = mass * dot(v_t, s_axis) / DT / hold_lat;
            if need_long * need_long + need_lat * need_lat <= 1.0 {
                state.vel = sub(state.vel, add_scaled(state.vel, n, -dot(state.vel, n)));
            }
        }
    }
    state.pos = add_scaled(state.pos, state.vel, DT);
    state.rot = q_integrate(q, state.ang_vel, DT);
    state.speed_mps = dot(state.vel, rotate(state.rot, [0.0, 0.0, -1.0]));
    state.odometer_m += (abs(state.speed_mps) * DT) as f64;
}

/// World-space inverse inertia of the body applied to `v`.
fn inv_inertia(spec: &VehicleSpec, q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let i = spec.inertia_box();
    let b = rotate_inv(q, v);
    rotate(q, [b[0] / i[0], b[1] / i[1], b[2] / i[2]])
}

/// How much one N·s along unit `n` at offset `r` from the center of mass (world space)
/// changes that point's speed along `n`, m/s: the inverse effective mass there.
pub fn inv_mass_at(spec: &VehicleSpec, state: &VehicleState, r: [f32; 3], n: [f32; 3]) -> f32 {
    1.0 / spec.mass_kg + dot(n, cross(inv_inertia(spec, state.rot, cross(r, n)), r))
}

/// A knock from outside the sim (`world::collide`): `impulse`, N·s in world space, at
/// offset `r` from the center of mass. Changes the body's velocity and spin; when the
/// forward speed changes the wheels roll on at the new one (a hit scrubs the tires), so
/// they do not drive the car back into what it hit, and a braked or locked wheel is not
/// left spinning backward.
pub fn knock(spec: &VehicleSpec, state: &mut VehicleState, r: [f32; 3], impulse: [f32; 3]) {
    let dv = scale(impulse, 1.0 / spec.mass_kg);
    state.vel = add(state.vel, dv);
    let dw = inv_inertia(spec, state.rot, cross(r, impulse));
    state.ang_vel = add(state.ang_vel, dw);
    let fwd = rotate(state.rot, [0.0, 0.0, -1.0]);
    // A curb's upward kick barely changes the forward speed: the wheels keep their spin.
    if abs(dot(dv, fwd)) > KNOCK_SPIN_MPS {
        let spin = dot(state.vel, fwd) / WHEEL_RADIUS_M;
        for w in &mut state.wheels {
            w.spin = spin;
        }
    }
}

/// Change of forward speed, m/s, past which a knock sets the wheels rolling at the new speed.
const KNOCK_SPIN_MPS: f32 = 0.01;

/// A parked car's sway, Hz, and its damping ratio: a knock dies away in about a second.
const ROCK_HZ: f32 = 1.4;
const ROCK_DAMPING: f32 = 0.35;
/// Tilt rate a parked car picks up per m/s of the hit that knocks it, rad/s, and its cap.
const ROCK_PER_MPS: f32 = 0.05;
const ROCK_MAX_RATE: f32 = 0.5;

/// A parked car rocking on its springs after a knock: its tilt toward world `+x` and `+z`,
/// rad, and the tilt rates, rad/s. One damped spring per axis; the car never moves along
/// the ground. `Default` is at rest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Rock {
    pub tilt: [f32; 2],
    pub rate: [f32; 2],
}

impl Rock {
    /// Knocked toward the unit ground-plane direction `push` (`[x, z]`) by a hit closing
    /// at `speed`, m/s: the body leans that way.
    pub fn knock(&mut self, push: [f32; 2], speed: f32) {
        let mut kick = ROCK_PER_MPS * speed;
        if kick > ROCK_MAX_RATE {
            kick = ROCK_MAX_RATE;
        }
        for a in 0..2 {
            self.rate[a] = clamp_abs(self.rate[a] + push[a] * kick, ROCK_MAX_RATE);
        }
    }

    /// One [`DT`] step of the springs (semi-implicit Euler).
    pub fn step(&mut self) {
        let w = 2.0 * crate::math::PI * ROCK_HZ;
        for a in 0..2 {
            let acc = -w * w * self.tilt[a] - 2.0 * ROCK_DAMPING * w * self.rate[a];
            self.rate[a] += acc * DT;
            self.tilt[a] += self.rate[a] * DT;
        }
    }

    /// Still enough to stop drawing it tilted.
    pub fn settled(&self) -> bool {
        let still = |a: usize| abs(self.tilt[a]) < 1e-4 && abs(self.rate[a]) < 1e-3;
        still(0) && still(1)
    }
}

/// Compile-time proof that the sim's state types are plain data (no heap, no drop glue).
const _: () = {
    const fn copy<T: Copy>() {}
    copy::<Rock>();
    copy::<VehicleState>();
    copy::<WheelState>();
    copy::<VehicleInput>();
    copy::<Assists>();
    copy::<Hit>();
    copy::<FlatGround>();
};

#[cfg(test)]
mod tests {
    use super::*;

    // Tests that need a real spec parse it through serde.
    #[cfg(feature = "serde")]
    fn gannet() -> VehicleSpec {
        ron::from_str(include_str!("../../../assets/vehicles/gannet_gt.ron")).unwrap()
    }

    #[test]
    fn flat_ground_probe() {
        let g = FlatGround {
            y: 1.0,
            friction: 0.8,
        };
        let down = [0.0, -1.0, 0.0];
        let hit = g.probe([2.0, 3.0, -4.0], down, 5.0).unwrap();
        assert_eq!(hit.t, 2.0);
        assert_eq!(hit.normal, [0.0, 1.0, 0.0]);
        assert_eq!(hit.friction, 0.8);
        assert_eq!(g.probe([0.0, 3.0, 0.0], down, 1.5), None, "out of reach");
        assert_eq!(
            g.probe([0.0, 3.0, 0.0], [0.0, 1.0, 0.0], 9.0),
            None,
            "pointing up"
        );
        assert_eq!(
            g.probe([0.0, 0.5, 0.0], down, 1.0).unwrap().t,
            0.0,
            "starts inside"
        );
    }

    #[test]
    fn tire_curve_is_peaked() {
        assert_eq!(tire_curve(0.0), 0.0);
        assert_eq!(tire_curve(1.0), 1.0);
        assert_eq!(tire_curve(TAIL_SLIP), TAIL_GRIP);
        assert_eq!(tire_curve(10.0), TAIL_GRIP);
        let mut prev = 0.0;
        for i in 1..=100 {
            let f = tire_curve(i as f32 * 0.01);
            assert!(f > prev, "rises to the peak");
            prev = f;
        }
        for i in 1..=200 {
            let f = tire_curve(1.0 + i as f32 * 0.01);
            assert!(f <= prev && f >= TAIL_GRIP, "falls off to the tail");
            prev = f;
        }
    }

    #[test]
    fn longitudinal_solve_is_bracketed_and_consistent() {
        let tire = Tire {
            mu_x: 1.1,
            mu_y: 1.2,
            load: 3000.0,
            sr_peak: 0.1,
            alpha_peak: 0.12,
            ref_speed: 20.0,
        };
        for &slip_pre in &[-30.0, -2.0, -0.05, 0.0, 0.05, 2.0, 30.0] {
            for &k in &[1e-5, 1e-3, 0.1] {
                let (f, s) = tire.solve_long(slip_pre, k, 0.03);
                assert!(abs(s) <= abs(slip_pre) && s * slip_pre >= 0.0);
                assert!(f * slip_pre >= 0.0, "force opposes the slip");
                assert!(abs(f) <= tire.mu_x * tire.load * 1.0001);
                // `s` is a root of `x - slip_pre + force(x) * k` to the bisection tolerance
                // (2^-20 of the bracket): the residual changes sign across it.
                let a = abs(slip_pre);
                let g = |x: f32| x - a + tire.force_long(x, 0.03) * k;
                let d = a * 4e-6 + 1e-9;
                assert!(
                    g(abs(s) - d) <= 0.0 && g(abs(s) + d) >= 0.0,
                    "not a root: {s}"
                );
            }
        }
    }

    #[test]
    fn friction_spin_stops_at_zero() {
        assert_eq!(friction_spin(5.0, 2.0), 3.0);
        assert_eq!(friction_spin(-5.0, 2.0), -3.0);
        assert_eq!(friction_spin(1.0, 2.0), 0.0);
        assert_eq!(friction_spin(-1.0, 2.0), 0.0);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn gear_ratios_carry_direction() {
        let spec = gannet();
        assert_eq!(overall_ratio(&spec, 1), 3.1 * 3.7);
        assert_eq!(overall_ratio(&spec, 5), 0.85 * 3.7);
        assert_eq!(overall_ratio(&spec, 6), 0.0, "no sixth gear");
        assert_eq!(overall_ratio(&spec, 0), 0.0);
        assert_eq!(overall_ratio(&spec, -1), -3.0 * 3.7);
        let shares = drive_shares(Drive::All);
        assert_eq!(shares[0] + shares[1] + shares[2] + shares[3], 1.0);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn spawn_rests_on_the_ground_at_static_load() {
        let spec = gannet();
        let s = VehicleState::spawn(&spec, [10.0, 2.0, -5.0], 0.5 * crate::math::PI);
        assert_eq!(s.pos, [10.0, 2.0 + spec.cg_height_m, -5.0]);
        let f = s.forward();
        assert!(
            abs(f[0] + 1.0) < 1e-6 && abs(f[2]) < 1e-6,
            "heading pi/2 faces -x: {f:?}"
        );
        let mut load = 0.0;
        for w in &s.wheels {
            assert!(w.grounded);
            assert!(
                abs(w.contact[1] - 2.0) < 1e-6,
                "contact on the ground: {:?}",
                w.contact
            );
            load += w.load;
        }
        assert!(abs(load - spec.mass_kg * GRAVITY) < 0.01);
        assert_eq!(s.gear, 1);
        assert_eq!(s.engine_rpm, spec.engine.idle_rpm);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn hash_sees_every_kind_of_field() {
        let spec = gannet();
        let base = VehicleState::spawn(&spec, [0.0; 3], 0.0);
        let h = base.hash();
        assert_eq!(h, base.hash());
        let mut changed = [base; 6];
        changed[0].pos[2] = 1e-6;
        changed[1].wheels[3].spin = 0.5;
        changed[2].wheels[1].grounded = false;
        changed[3].gear = 2;
        changed[4].odometer_m = 1.0;
        changed[5].long_g = 0.1;
        for c in &changed {
            assert_ne!(c.hash(), h);
        }
    }

    #[test]
    #[cfg(feature = "serde")]
    fn free_fall_is_gravity() {
        let spec = gannet();
        let mut s = VehicleState::spawn(&spec, [0.0, 100.0, 0.0], 0.0);
        let input = VehicleInput::default();
        let ground = FlatGround {
            y: 0.0,
            friction: 1.0,
        };
        for _ in 0..120 {
            step(&spec, &mut s, &input, &Assists::OFF, &ground);
        }
        assert!(s.wheels.iter().all(|w| !w.grounded));
        // One second of semi-implicit Euler at 120 Hz, slowed only by a whisker of drag.
        assert!(abs(s.vel[1] + GRAVITY) < 0.05, "vy {}", s.vel[1]);
        assert!(abs(s.pos[1] - (100.0 + spec.cg_height_m - 4.9)) < 0.1);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn knocks_move_the_body_and_the_wheels_follow() {
        let spec = gannet();
        let mut s = VehicleState::spawn(&spec, [0.0; 3], 0.0);
        s.vel = [0.0, 0.0, -10.0];
        for w in &mut s.wheels {
            w.spin = 10.0 / WHEEL_RADIUS_M;
        }
        // Square on the nose, through the center of mass: straight back, no spin.
        let nose = [0.0, 0.0, -2.0];
        let back = [0.0, 0.0, 1.0];
        let j = 10.5 / inv_mass_at(&spec, &s, nose, back);
        knock(&spec, &mut s, nose, scale(back, j));
        assert!(abs(s.vel[2] - 0.5) < 1e-4, "{:?}", s.vel);
        assert_eq!(s.ang_vel, ZERO);
        for w in &s.wheels {
            assert!(
                abs(w.spin * WHEEL_RADIUS_M + 0.5) < 1e-4,
                "rolls back with the car"
            );
        }
        // On the right front corner, pushed left: the nose swings left, a positive yaw.
        let mut s = VehicleState::spawn(&spec, [0.0; 3], 0.0);
        let corner = [0.9, 0.0, -2.0];
        knock(&spec, &mut s, corner, [-1000.0, 0.0, 0.0]);
        assert!(s.ang_vel[1] > 0.0 && s.vel[0] < 0.0, "{:?}", s.ang_vel);
        // The point's speed change matches its inverse effective mass.
        let k = inv_mass_at(
            &spec,
            &VehicleState::spawn(&spec, [0.0; 3], 0.0),
            corner,
            [-1.0, 0.0, 0.0],
        );
        let v = add(s.vel, cross(s.ang_vel, corner));
        assert!(abs(-v[0] - 1000.0 * k) < 1e-5);
    }

    /// A plane rising `grade` m per m toward +z: a car at heading 0 faces downhill.
    struct Slope {
        grade: f32,
        friction: f32,
    }

    impl Ground for Slope {
        fn probe(&self, origin: [f32; 3], dir: [f32; 3], max_len: f32) -> Option<Hit> {
            let n = normalize([0.0, 1.0, -self.grade]);
            let d_n = dot(dir, n);
            if d_n > -1e-6 {
                return None;
            }
            let t = dot(origin, n) / -d_n;
            if t > max_len {
                return None;
            }
            Some(Hit {
                t: if t < 0.0 { 0.0 } else { t },
                normal: n,
                friction: self.friction,
            })
        }
    }

    /// (A brake held at a stop selects reverse after `REVERSE_DWELL_S`, so parking is the
    /// handbrake's job.) How far a car parked on `slope` with `input` held moves in 30 s after 3 s to settle, m.
    #[cfg(feature = "serde")]
    fn parked_drift(slope: &Slope, input: &VehicleInput) -> f32 {
        let spec = gannet();
        let mut s = VehicleState::spawn(&spec, [0.0; 3], 0.0);
        for _ in 0..360 {
            step(&spec, &mut s, input, &Assists::OFF, slope);
        }
        let start = s.pos;
        for _ in 0..3600 {
            step(&spec, &mut s, input, &Assists::OFF, slope);
        }
        length(sub(s.pos, start))
    }

    #[test]
    #[cfg(feature = "serde")]
    fn a_braked_car_holds_on_a_grade_until_it_beats_the_grip() {
        let handbrake = VehicleInput {
            handbrake: true,
            ..VehicleInput::default()
        };
        let dry = Slope {
            grade: 0.1,
            friction: 1.0,
        };
        let d = parked_drift(&dry, &handbrake);
        assert!(d < 0.01, "handbrake on 10%: crept {d} m");
        // Ice: the rears' grip is less than the grade asks for, so the car slides.
        let ice = Slope {
            grade: 0.1,
            friction: 0.1,
        };
        let d = parked_drift(&ice, &handbrake);
        assert!(d > 1.0, "handbrake on icy 10%: slid only {d} m");
        // Dry but steeper than the locked rears can hold.
        let steep = Slope {
            grade: 0.8,
            friction: 1.0,
        };
        let d = parked_drift(&steep, &handbrake);
        assert!(d > 1.0, "handbrake on 80%: slid only {d} m");
        // Nothing held: it rolls away.
        let d = parked_drift(&dry, &VehicleInput::default());
        assert!(d > 1.0, "free on 10%: rolled only {d} m");
    }

    #[test]
    fn a_knocked_parked_car_rocks_and_settles() {
        let mut r = Rock::default();
        assert!(r.settled());
        r.knock([1.0, 0.0], 8.0);
        let mut peak: f32 = 0.0;
        let mut back = false;
        for i in 0..240 {
            r.step();
            peak = peak.max(r.tilt[0]);
            back |= r.tilt[0] < 0.0;
            if i == 180 {
                assert!(abs(r.tilt[0]) < 0.1 * peak, "still rocking at 1.5 s: {r:?}");
            }
        }
        assert!(peak > 0.01 && peak < 0.07, "leans {peak} rad");
        assert!(back, "sways back past level");
        assert_eq!(r.tilt[1], 0.0);
        for _ in 0..240 {
            r.step();
        }
        assert!(r.settled(), "{r:?}");
    }
}
