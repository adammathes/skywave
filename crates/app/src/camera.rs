//! Cameras around the player's car (`docs/RENDERING.md`, "Cameras"): cockpit, hood, three
//! chase distances, and a slow orbit that takes over when the player has been idle.
//!
//! The rig's springs and filters advance in the fixed sim step after the car, so they are as
//! deterministic as the car. The final pose is put together at render time from the
//! interpolated rig state and the interpolated car pose: a camera bolted to the car never
//! jitters against it.
//!
//! Conventions: body space as in `vehicle` (`+x` right, `+y` up, `-z` forward, origin at the
//! center of mass); yaw is counter-clockwise seen from above, 0 facing `-z`, like the car's
//! heading; look pitch is positive up.

use core::f32::consts::{PI, TAU};
use glam::{Mat3, Mat4, Quat, Vec3};
use vehicle::Ground;

use crate::drive::{mix, mix3, Drive};
use crate::input::{Action, InputState};

/// Every camera number in one place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraTuning {
    /// Hood camera: height above the belt line, m, and how far ahead of the shape's eye, m.
    pub hood_above_belt_m: f32,
    pub hood_ahead_m: f32,
    /// Cockpit eye: raised and moved forward from the shape's driver's eye by this much, m,
    /// before the player's seat adjustment ([`CameraPrefs`]).
    pub cockpit_eye_up_m: f32,
    pub cockpit_eye_forward_m: f32,
    /// Vertical fields of view, degrees (at 16:9 the horizontal one is wider: 62° vertical
    /// is ≈ 94° horizontal). The cockpit's is the default of [`CameraPrefs::cockpit_fov_deg`].
    pub cockpit_fov_deg: f32,
    pub hood_fov_deg: f32,
    pub chase_fov_deg: f32,
    /// Near clip planes, m: close for the dash, farther out for depth precision elsewhere.
    pub cockpit_near_m: f32,
    pub near_m: f32,
    pub far_m: f32,

    /// Chase near, mid and far: distance behind and height above the ground point under
    /// the car, m.
    pub chase_distance_m: [f32; 3],
    pub chase_height_m: [f32; 3],
    /// What the chase camera aims at: this high above the ground under the car, m...
    pub chase_target_height_m: f32,
    /// ...and ahead along the car's heading by a base plus a share of the speed, capped, m
    /// and s. The camera sees where the car is going, not just the car.
    pub chase_lookahead_m: f32,
    pub chase_lookahead_s: f32,
    pub chase_lookahead_max_m: f32,
    /// Chase follow springs (critically damped), natural frequency rad/s: the camera's yaw
    /// chasing the car's heading (lag through corners)...
    pub chase_yaw_hz: f32,
    /// ...plus a lead into the turn: the yaw it chases (and the look-ahead) swings toward
    /// the way the wheel is turned by up to this much, degrees, faded in between these
    /// speeds, m/s. In a steady corner it about cancels the spring's lag, so the car stays
    /// framed from behind and the road ahead of the turn shows.
    pub chase_lead_deg: f32,
    pub chase_lead_speed_mps: [f32; 2],
    /// ...and its position chasing the target spot, damped relative to the car's velocity
    /// so speed adds no lag, only acceleration does (pull back under power, push in braking).
    pub chase_pos_hz: f32,
    /// The camera stays at least this far above the ground, m.
    pub min_clearance_m: f32,
    /// Chase elevation limits under right-stick orbiting, degrees above the horizontal.
    pub chase_min_elev_deg: f32,
    pub chase_max_elev_deg: f32,

    /// Cockpit head sway, body space, m: thrown sideways against lateral g (scaled by
    /// `0.5 + body_roll`), back under acceleration, down as the springs compress; each
    /// capped and lagged by a first-order filter.
    pub sway_lat_m_per_g: f32,
    pub sway_long_m_per_g: f32,
    pub sway_heave: f32,
    pub sway_max_m: f32,
    pub sway_lag_s: f32,

    /// Look: rate at full stick, rad/s; limits in the cockpit and hood, degrees; after this
    /// long without look input the view returns to center at `look_return_hz`.
    pub look_rate: f32,
    pub look_max_yaw_deg: f32,
    pub look_max_pitch_deg: f32,
    pub look_return_delay_s: f32,
    pub look_return_hz: f32,

    /// Orbit: after this long with no input and the car stopped (below `idle_speed_mps`),
    /// circle the car at this radius and height, m, and rate, rad/s.
    pub idle_orbit_s: f32,
    pub idle_speed_mps: f32,
    pub orbit_radius_m: f32,
    pub orbit_height_m: f32,
    pub orbit_rate: f32,
}

/// Tuning notes (M1 tuning pass): FOVs were 75° cockpit, 70° hood, 65° chase, which bent
/// the street at the edges of a 16:9 frame; the chase distances grew with the narrower
/// chase FOV so the car fills about the same share of the frame (≈ 20% of its width from
/// the mid camera). The yaw spring was 4 rad/s, which lagged ~11° behind the car through a
/// 0.8 g corner at 20 m/s; 6.5 halves that and the lead takes up the rest.
///
/// Feel pass 2 (after the first play, "hard to see out of the cockpits"): cockpit FOV
/// 58° → 62° (and a setting, 50–80°), and the eye 5 cm higher and 3 cm further forward
/// than the shape puts it. From the shape's eye the gannet's instrument hood and cowl hid
/// the road for more than 22 m ahead; 3 cm up and forward brought that to 9.5 m, 5 cm up to
/// 6.5 m (the homestead's long flat hood: 21 m → 11 m).
pub const TUNING: CameraTuning = CameraTuning {
    hood_above_belt_m: 0.30,
    hood_ahead_m: 1.0,
    cockpit_eye_up_m: 0.05,
    cockpit_eye_forward_m: 0.03,
    cockpit_fov_deg: 62.0,
    hood_fov_deg: 55.0,
    chase_fov_deg: 50.0,
    cockpit_near_m: 0.1,
    near_m: 0.25,
    far_m: 3000.0,

    chase_distance_m: [5.0, 7.0, 10.5],
    chase_height_m: [1.65, 2.3, 3.25],
    chase_target_height_m: 1.0,
    chase_lookahead_m: 1.0,
    chase_lookahead_s: 0.15,
    chase_lookahead_max_m: 5.0,
    chase_yaw_hz: 6.5,
    chase_lead_deg: 8.0,
    chase_lead_speed_mps: [4.0, 15.0],
    chase_pos_hz: 5.0,
    min_clearance_m: 0.4,
    chase_min_elev_deg: 3.0,
    chase_max_elev_deg: 60.0,

    sway_lat_m_per_g: 0.03,
    sway_long_m_per_g: 0.04,
    sway_heave: 0.5,
    sway_max_m: 0.06,
    sway_lag_s: 0.12,

    look_rate: 2.4,
    look_max_yaw_deg: 140.0,
    look_max_pitch_deg: 50.0,
    look_return_delay_s: 1.0,
    look_return_hz: 4.0,

    idle_orbit_s: 20.0,
    idle_speed_mps: 1.0,
    orbit_radius_m: 8.0,
    orbit_height_m: 2.2,
    orbit_rate: 0.12,
};

/// The player's camera settings (`crate::settings`), as the rig uses them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPrefs {
    /// Vertical field of view in the cockpit, degrees. The hood and chase cameras keep theirs.
    pub cockpit_fov_deg: f32,
    /// Seat adjustment of the cockpit eye, m: up and forward.
    pub seat_up_m: f32,
    pub seat_forward_m: f32,
    /// Cockpit head sway; off holds the eye still in the car.
    pub shake: bool,
}

impl Default for CameraPrefs {
    fn default() -> CameraPrefs {
        CameraPrefs {
            cockpit_fov_deg: TUNING.cockpit_fov_deg,
            seat_up_m: 0.0,
            seat_forward_m: 0.0,
            shake: true,
        }
    }
}

/// Which camera is in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraMode {
    Cockpit,
    Hood,
    ChaseNear,
    ChaseMid,
    ChaseFar,
    /// Taken over automatically while idle; never selected by the cycle.
    Orbit,
}

impl CameraMode {
    /// The order `CameraCycle` steps through.
    pub const CYCLE: [CameraMode; 5] = [
        CameraMode::Cockpit,
        CameraMode::Hood,
        CameraMode::ChaseNear,
        CameraMode::ChaseMid,
        CameraMode::ChaseFar,
    ];

    pub fn next(self) -> CameraMode {
        match self {
            CameraMode::Cockpit => CameraMode::Hood,
            CameraMode::Hood => CameraMode::ChaseNear,
            CameraMode::ChaseNear => CameraMode::ChaseMid,
            CameraMode::ChaseMid => CameraMode::ChaseFar,
            CameraMode::ChaseFar | CameraMode::Orbit => CameraMode::Cockpit,
        }
    }

    /// Stable name (URL parameters, HUD).
    pub fn name(self) -> &'static str {
        match self {
            CameraMode::Cockpit => "cockpit",
            CameraMode::Hood => "hood",
            CameraMode::ChaseNear => "chase-near",
            CameraMode::ChaseMid => "chase-mid",
            CameraMode::ChaseFar => "chase-far",
            CameraMode::Orbit => "orbit",
        }
    }

    /// Parses [`CameraMode::name`], plus `chase` for the mid chase camera.
    pub fn from_name(name: &str) -> Option<CameraMode> {
        if name == "chase" {
            return Some(CameraMode::ChaseMid);
        }
        CameraMode::CYCLE
            .into_iter()
            .chain([CameraMode::Orbit])
            .find(|m| m.name() == name)
    }

    /// Index into the chase tables, for the chase modes.
    fn chase_slot(self) -> Option<usize> {
        match self {
            CameraMode::ChaseNear => Some(0),
            CameraMode::ChaseMid => Some(1),
            CameraMode::ChaseFar => Some(2),
            _ => None,
        }
    }

    /// Cockpit and hood: the cameras that draw the car's cabin.
    pub fn first_person(self) -> bool {
        matches!(self, CameraMode::Cockpit | CameraMode::Hood)
    }
}

/// The cockpit eye in body space, before sway: the driver's eye of the car's shape, moved
/// by the tuning's offset and the player's seat adjustment (`-z` is forward).
pub fn cockpit_eye(drive: &Drive, t: &CameraTuning, prefs: &CameraPrefs) -> Vec3 {
    drive.layout.eye
        + Vec3::new(
            0.0,
            t.cockpit_eye_up_m + prefs.seat_up_m,
            -(t.cockpit_eye_forward_m + prefs.seat_forward_m),
        )
}

/// The hood camera in body space: on the centerline over the belt, ahead of the shape's
/// eye (the seat does not move it).
fn hood_eye(drive: &Drive, t: &CameraTuning) -> Vec3 {
    let eye = drive.layout.eye;
    Vec3::new(
        0.0,
        drive.layout.belt_y + t.hood_above_belt_m,
        eye.z - t.hood_ahead_m,
    )
}

/// A camera for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub eye: Vec3,
    /// Camera-to-world rotation; the camera looks down its `-z`.
    pub rot: Quat,
    pub fov_y_rad: f32,
    pub near_m: f32,
    pub far_m: f32,
}

impl CameraPose {
    pub fn view(&self) -> Mat4 {
        Mat4::from_quat(self.rot.conjugate()) * Mat4::from_translation(-self.eye)
    }

    pub fn forward(&self) -> Vec3 {
        self.rot * Vec3::NEG_Z
    }

    /// Yaw of the view direction, rad (0 faces `-z`, counter-clockwise positive).
    pub fn yaw(&self) -> f32 {
        let f = self.forward();
        (-f.x).atan2(-f.z)
    }
}

/// `x` wrapped into `(-π, π]`.
fn wrap_pi(x: f32) -> f32 {
    let y = (x + PI).rem_euclid(TAU) - PI;
    if y <= -PI {
        y + TAU
    } else {
        y
    }
}

/// Yaw of the car's heading, rad.
fn heading(drive: &Drive, alpha: f32) -> f32 {
    let f = drive.pose(alpha).forward();
    (-f.x).atan2(-f.z)
}

/// The chase camera's lead into the turn, rad (positive left, like yaw): toward the way the
/// wheel is turned, in proportion, faded in with speed.
fn chase_lead(drive: &Drive, t: &CameraTuning) -> f32 {
    let [lo, hi] = t.chase_lead_speed_mps;
    let s = ((drive.state.speed_mps.abs() - lo) / (hi - lo)).clamp(0.0, 1.0);
    // Steering right (positive) turns the heading clockwise: a negative yaw.
    -drive.state.steer.clamp(-1.0, 1.0) * t.chase_lead_deg.to_radians() * s * s * (3.0 - 2.0 * s)
}

/// Horizontal unit vector at yaw `y`.
fn yaw_dir(y: f32) -> Vec3 {
    Vec3::new(-y.sin(), 0.0, -y.cos())
}

/// Ground height under `p`, or `None` when nothing is there.
fn ground_y(ground: &impl Ground, p: Vec3) -> Option<f32> {
    const PROBE: f32 = 200.0;
    let top = p.y + 0.5 * PROBE;
    ground
        .probe([p.x, top, p.z], [0.0, -1.0, 0.0], PROBE)
        .map(|h| top - h.t)
}

/// Rotation whose `-z` points along `dir` with `+y` as close to up as it can be.
fn look_rot(dir: Vec3) -> Quat {
    let f = dir.normalize_or(Vec3::NEG_Z);
    let right = f.cross(Vec3::Y).normalize_or(Vec3::X);
    let up = right.cross(f);
    Quat::from_mat3(&Mat3::from_cols(right, up, -f)).normalize()
}

/// The springs and filters, stepped at the sim rate.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RigState {
    /// Chase camera position and velocity, world space.
    chase_pos: Vec3,
    chase_vel: Vec3,
    /// The chase camera's lagged copy of the car's heading, and its rate.
    yaw: f32,
    yaw_rate: f32,
    /// Look offsets: yaw (positive left) and pitch (positive up), rad.
    look_yaw: f32,
    look_pitch: f32,
    /// Cockpit head offset, body space, m.
    sway: Vec3,
    /// Orbit angle (a yaw around the car), rad.
    orbit: f32,
}

/// The camera rig.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraRig {
    pub tuning: CameraTuning,
    pub prefs: CameraPrefs,
    /// The selected camera; [`CameraRig::mode`] may show Orbit instead while idle.
    selected: CameraMode,
    look_back: bool,
    /// Seconds with no input and the car stopped.
    idle_s: f32,
    /// Seconds since the last look input.
    look_idle_s: f32,
    /// Reset the chase springs onto their targets at the next step.
    snap: bool,
    prev: RigState,
    curr: RigState,
}

impl CameraRig {
    pub fn new(mode: CameraMode, drive: &Drive) -> CameraRig {
        let yaw = heading(drive, 1.0);
        let s = RigState {
            chase_pos: drive.pose(1.0).pos,
            chase_vel: Vec3::ZERO,
            yaw,
            yaw_rate: 0.0,
            look_yaw: 0.0,
            look_pitch: 0.0,
            sway: Vec3::ZERO,
            orbit: yaw,
        };
        let mut rig = CameraRig {
            tuning: TUNING,
            prefs: CameraPrefs::default(),
            selected: CameraMode::ChaseMid,
            look_back: false,
            idle_s: 0.0,
            look_idle_s: 0.0,
            snap: true,
            prev: s,
            curr: s,
        };
        rig.set_mode(mode);
        rig.curr.chase_pos = rig.chase_target(drive, &s, 1.0);
        rig.prev = rig.curr;
        rig
    }

    /// The camera in use now.
    pub fn mode(&self) -> CameraMode {
        if self.idle_s > self.tuning.idle_orbit_s {
            CameraMode::Orbit
        } else {
            self.selected
        }
    }

    /// Selects a camera (Orbit selects the mid chase camera; it is only ever automatic).
    pub fn set_mode(&mut self, mode: CameraMode) {
        let mode = if mode == CameraMode::Orbit {
            CameraMode::ChaseMid
        } else {
            mode
        };
        if self.selected.first_person() && !mode.first_person() {
            self.snap = true;
        }
        self.selected = mode;
        self.curr.look_yaw = 0.0;
        self.curr.look_pitch = 0.0;
        self.prev.look_yaw = 0.0;
        self.prev.look_pitch = 0.0;
        self.idle_s = 0.0;
    }

    /// The camera the player selected (never Orbit), whatever [`CameraRig::mode`] shows.
    pub fn selected(&self) -> CameraMode {
        self.selected
    }

    /// `CameraCycle`: the next camera in [`CameraMode::CYCLE`].
    pub fn cycle(&mut self) {
        self.set_mode(self.selected.next());
    }

    /// The chase slot whose springs run: the selected chase camera, or the mid one while a
    /// first-person camera is up.
    fn chase_slot(&self) -> usize {
        self.selected.chase_slot().unwrap_or(1)
    }

    /// Where the chase camera wants to be for rig state `s` and the car at `alpha`.
    fn chase_target(&self, drive: &Drive, s: &RigState, alpha: f32) -> Vec3 {
        let t = &self.tuning;
        let slot = self.chase_slot();
        let pivot = self.pivot(drive, alpha);
        let (d, h) = (t.chase_distance_m[slot], t.chase_height_m[slot]);
        let rise = h - t.chase_target_height_m;
        let r = (d * d + rise * rise).sqrt();
        let elev = (rise.atan2(d) - s.look_pitch).clamp(
            t.chase_min_elev_deg.to_radians(),
            t.chase_max_elev_deg.to_radians(),
        );
        let back = if self.look_back { PI } else { 0.0 };
        let behind = -yaw_dir(s.yaw + s.look_yaw + back);
        pivot + behind * (r * elev.cos()) + Vec3::Y * (r * elev.sin())
    }

    /// The point the chase and orbit cameras turn around: above the car's center of mass by
    /// the target height over the ground point.
    fn pivot(&self, drive: &Drive, alpha: f32) -> Vec3 {
        let p = drive.pose(alpha).pos;
        Vec3::new(
            p.x,
            p.y - drive.spec.cg_height_m + self.tuning.chase_target_height_m,
            p.z,
        )
    }

    /// One fixed step of `dt` seconds, after the car's.
    pub fn step(&mut self, drive: &Drive, input: &InputState, dt: f32, ground: &impl Ground) {
        let t = self.tuning;
        self.prev = self.curr;
        let mut s = self.curr;

        // Idle: no input of any drive action and the car stopped.
        let active = [
            Action::Throttle,
            Action::Brake,
            Action::Steer,
            Action::Handbrake,
            Action::LookX,
            Action::LookY,
            Action::LookBack,
            Action::CameraCycle,
            Action::Headlights,
            Action::HighBeams,
            Action::Underglow,
            Action::Horn,
            Action::TimeScrub,
        ]
        .iter()
        .any(|&a| input.axis(a).abs() > 0.02);
        if active || drive.state.speed_mps.abs() > t.idle_speed_mps {
            self.idle_s = 0.0;
        } else {
            self.idle_s += dt;
        }

        // Look: rate input, held where it is, back to center a while after it stops.
        self.look_back = input.pressed(Action::LookBack);
        let (lx, ly) = (input.axis(Action::LookX), input.axis(Action::LookY));
        if lx != 0.0 || ly != 0.0 {
            self.look_idle_s = 0.0;
            s.look_yaw -= lx * t.look_rate * dt;
            s.look_pitch -= ly * t.look_rate * dt;
        } else {
            self.look_idle_s += dt;
            if self.look_idle_s > t.look_return_delay_s {
                let k = (t.look_return_hz * dt).min(1.0);
                s.look_yaw -= s.look_yaw * k;
                s.look_pitch -= s.look_pitch * k;
            }
        }
        if self.selected.first_person() {
            let (my, mp) = (
                t.look_max_yaw_deg.to_radians(),
                t.look_max_pitch_deg.to_radians(),
            );
            s.look_yaw = s.look_yaw.clamp(-my, my);
            s.look_pitch = s.look_pitch.clamp(-mp, mp);
        } else {
            s.look_yaw = wrap_pi(s.look_yaw);
            s.look_pitch = s.look_pitch.clamp(-1.2, 1.2);
        }

        // Chase yaw spring toward the car's heading, led into the turn.
        let target_yaw = heading(drive, 1.0) + chase_lead(drive, &t);
        let w = t.chase_yaw_hz;
        let err = wrap_pi(target_yaw - s.yaw);
        s.yaw_rate += (w * w * err - 2.0 * w * s.yaw_rate) * dt;
        s.yaw = wrap_pi(s.yaw + s.yaw_rate * dt);

        // Chase position spring, damped against the car's velocity.
        let car_vel = Vec3::from(drive.state.vel);
        if self.snap {
            s.yaw = target_yaw;
            s.yaw_rate = 0.0;
            s.chase_pos = self.chase_target(drive, &s, 1.0);
            s.chase_vel = car_vel;
            self.snap = false;
        } else {
            let w = t.chase_pos_hz;
            let target = self.chase_target(drive, &s, 1.0);
            let acc = (target - s.chase_pos) * (w * w) - (s.chase_vel - car_vel) * (2.0 * w);
            s.chase_vel += acc * dt;
            s.chase_pos += s.chase_vel * dt;
        }
        if let Some(gy) = ground_y(ground, s.chase_pos) {
            let floor = gy + t.min_clearance_m;
            if s.chase_pos.y < floor {
                s.chase_pos.y = floor;
                s.chase_vel.y = s.chase_vel.y.max(0.0);
            }
        }

        // Cockpit sway from the cabin's g-forces and the springs.
        let st = &drive.state;
        let roll_gain = 0.5 + drive.spec.feel.body_roll;
        let rest = drive.rest_compression();
        let mean_c = st.wheels.iter().map(|w| w.compression).sum::<f32>() * 0.25;
        let m = t.sway_max_m;
        let target = Vec3::new(
            (-t.sway_lat_m_per_g * roll_gain * st.lat_g).clamp(-m, m),
            (-t.sway_heave * (mean_c - rest)).clamp(-m, m),
            (t.sway_long_m_per_g * st.long_g).clamp(-m, m),
        );
        s.sway += (target - s.sway) * (dt / t.sway_lag_s).min(1.0);

        // Orbit: circle while idle, otherwise wait behind the chase camera.
        if self.mode() == CameraMode::Orbit {
            s.orbit = wrap_pi(s.orbit + t.orbit_rate * dt);
        } else {
            s.orbit = s.yaw;
        }
        self.curr = s;
    }

    /// The camera `alpha` of the way from the previous step to the current one.
    pub fn pose(&self, drive: &Drive, alpha: f32) -> CameraPose {
        let t = &self.tuning;
        let (a, b) = (&self.prev, &self.curr);
        let look_yaw = mix(a.look_yaw, b.look_yaw, alpha);
        let look_pitch = mix(a.look_pitch, b.look_pitch, alpha);
        let car = drive.pose(alpha);
        let back = if self.look_back { PI } else { 0.0 };
        let mode = self.mode();
        let first_person = |eye: Vec3, fov: f32, near: f32| CameraPose {
            eye: car.point(eye),
            rot: car.rot
                * Quat::from_rotation_y(look_yaw + back)
                * Quat::from_rotation_x(look_pitch),
            fov_y_rad: fov.to_radians(),
            near_m: near,
            far_m: t.far_m,
        };
        match mode {
            CameraMode::Cockpit => {
                let sway = if self.prefs.shake {
                    mix3(a.sway, b.sway, alpha)
                } else {
                    Vec3::ZERO
                };
                first_person(
                    cockpit_eye(drive, t, &self.prefs) + sway,
                    self.prefs.cockpit_fov_deg,
                    t.cockpit_near_m,
                )
            }
            CameraMode::Hood => first_person(hood_eye(drive, t), t.hood_fov_deg, t.near_m),
            CameraMode::ChaseNear | CameraMode::ChaseMid | CameraMode::ChaseFar => {
                let eye = mix3(a.chase_pos, b.chase_pos, alpha);
                let speed = drive.state.speed_mps.abs();
                let ahead = (t.chase_lookahead_m + t.chase_lookahead_s * speed)
                    .min(t.chase_lookahead_max_m);
                let dir = if self.look_back { -1.0 } else { 1.0 };
                let yaw = heading(drive, alpha) + chase_lead(drive, t);
                let target = self.pivot(drive, alpha) + yaw_dir(yaw) * (ahead * dir);
                CameraPose {
                    eye,
                    rot: look_rot(target - eye),
                    fov_y_rad: t.chase_fov_deg.to_radians(),
                    near_m: t.near_m,
                    far_m: t.far_m,
                }
            }
            CameraMode::Orbit => {
                // Orbit angles wrap; interpolate the short way.
                let o = a.orbit + wrap_pi(b.orbit - a.orbit) * alpha.clamp(0.0, 1.0);
                let pivot = self.pivot(drive, alpha);
                let eye = pivot - yaw_dir(o) * t.orbit_radius_m
                    + Vec3::Y * (t.orbit_height_m - t.chase_target_height_m);
                CameraPose {
                    eye,
                    rot: look_rot(pivot - eye),
                    fov_y_rad: t.chase_fov_deg.to_radians(),
                    near_m: t.near_m,
                    far_m: t.far_m,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::DEFAULT_VEHICLE;
    use crate::drive::{ground_for, DriveGround};
    use skywave_core::SIM_DT;
    use vehicle::VehicleInput;

    const DT: f32 = SIM_DT as f32;

    /// An open lot: the test town with its streets and buildings removed, so the lap
    /// test turns hard without hitting anything (the apron is flat).
    fn flat() -> DriveGround {
        let mut p = place::Place::test_grid();
        p.roads.nodes.clear();
        p.roads.edges.clear();
        p.parcels.clear();
        p.buildings.clear();
        ground_for(&p)
    }

    fn setup(mode: CameraMode) -> (Drive, CameraRig) {
        let d = Drive::new(DEFAULT_VEHICLE).unwrap();
        let rig = CameraRig::new(mode, &d);
        (d, rig)
    }

    #[test]
    fn cycle_order() {
        let (_, mut rig) = setup(CameraMode::Cockpit);
        let mut seen = vec![rig.mode()];
        for _ in 0..5 {
            rig.cycle();
            seen.push(rig.mode());
        }
        use CameraMode::*;
        assert_eq!(
            seen,
            [Cockpit, Hood, ChaseNear, ChaseMid, ChaseFar, Cockpit]
        );
        for m in CameraMode::CYCLE {
            assert_eq!(CameraMode::from_name(m.name()), Some(m));
        }
        assert_eq!(CameraMode::from_name("chase"), Some(ChaseMid));
        assert_eq!(CameraMode::from_name("drone"), None);
        assert_eq!(Orbit.next(), Cockpit);
    }

    #[test]
    fn fov_is_wide_in_the_cockpit_and_narrower_in_chase() {
        let (d, mut rig) = setup(CameraMode::Cockpit);
        let fov = |rig: &CameraRig| rig.pose(&d, 1.0).fov_y_rad.to_degrees();
        assert!((fov(&rig) - 62.0).abs() < 1e-3);
        rig.set_mode(CameraMode::Hood);
        assert!((fov(&rig) - 55.0).abs() < 1e-3);
        rig.set_mode(CameraMode::ChaseMid);
        assert!((fov(&rig) - 50.0).abs() < 1e-3);
        // The cockpit FOV setting changes the cockpit alone.
        rig.prefs.cockpit_fov_deg = 75.0;
        assert!((fov(&rig) - 50.0).abs() < 1e-3);
        rig.set_mode(CameraMode::Hood);
        assert!((fov(&rig) - 55.0).abs() < 1e-3);
        rig.set_mode(CameraMode::Cockpit);
        assert!((fov(&rig) - 75.0).abs() < 1e-3);
    }

    #[test]
    fn seat_moves_the_cockpit_eye_and_shake_can_be_turned_off() {
        let (mut d, mut rig) = setup(CameraMode::Cockpit);
        let local = |d: &Drive, rig: &CameraRig| {
            let car = d.pose(1.0);
            car.rot.inverse() * (rig.pose(d, 1.0).eye - car.pos)
        };
        let base = local(&d, &rig);
        let t = TUNING;
        let shaped = d.layout.eye + Vec3::new(0.0, t.cockpit_eye_up_m, -t.cockpit_eye_forward_m);
        assert!((base - shaped).length() < 1e-5, "{base}");
        rig.prefs.seat_up_m = 0.05;
        rig.prefs.seat_forward_m = -0.08;
        let moved = local(&d, &rig) - base;
        assert!(
            (moved - Vec3::new(0.0, 0.05, 0.08)).length() < 1e-5,
            "{moved}"
        );
        // Hard acceleration sways the head back; with shake off the eye stays put.
        let g = flat();
        let input = InputState::default();
        for _ in 0..120 {
            d.step(
                VehicleInput {
                    throttle: 1.0,
                    ..VehicleInput::default()
                },
                &g,
            );
            rig.step(&d, &input, DT, &g);
        }
        let swayed = local(&d, &rig);
        rig.prefs.shake = false;
        let still = local(&d, &rig);
        assert!((swayed - still).length() > 0.005, "{swayed} {still}");
        assert!((still - base - moved).length() < 1e-4, "{still}");
    }

    /// Whether the segment `a`–`b` passes through triangle `t` (Möller–Trumbore).
    fn segment_hits(a: Vec3, b: Vec3, t: [Vec3; 3]) -> bool {
        let d = b - a;
        let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return false;
        }
        let s = a - t[0];
        let u = s.dot(p) / det;
        let q = s.cross(e1);
        let v = d.dot(q) / det;
        let w = e2.dot(q) / det;
        u >= 0.0 && v >= 0.0 && u + v <= 1.0 && w > 1e-4 && w < 1.0
    }

    /// How far ahead of the cockpit eye (m, the car at rest) the road first shows past the
    /// car's cabin anywhere across a 4 m lane centered on the car: the shell drawn inside
    /// out (roof, pillars, cowl, hood) always, and with `dashboard` the interior and the
    /// steering wheel. Glass is not drawn from inside.
    fn road_visible_from(id: &str, eye: Vec3, dashboard: bool) -> f32 {
        let spec = crate::vehicles::builtin_spec(id).unwrap();
        let shape = crate::vehicles::builtin_shape(&spec).unwrap();
        let model = crate::car::CarModel::build(&spec, &shape).unwrap();
        let lod = &model.lods[0];
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        let mut add = |m: &render::Mesh, offset: Vec3| {
            for t in m.indices.chunks_exact(3) {
                tris.push(core::array::from_fn(|k| {
                    Vec3::from(m.vertices[t[k] as usize].pos) + offset
                }));
            }
        };
        add(&lod.shell_inside, Vec3::ZERO);
        if dashboard {
            add(&lod.interior, Vec3::ZERO);
            add(&lod.steering_wheel, model.layout.wheel_pivot);
        }
        let ground = -spec.cg_height_m;
        (0..400)
            .map(|k| 2.0 + 0.05 * k as f32)
            .find(|&ahead| {
                (-20..=20).any(|j| {
                    let road = Vec3::new(0.1 * j as f32, ground, eye.z - ahead);
                    !tris.iter().any(|&t| segment_hits(eye, road, t))
                })
            })
            .unwrap_or(f32::INFINITY)
    }

    /// "Hard to see out of the cockpits" (the first play): from the default eye the road
    /// shows from ~6 m ahead of the gannet's driver, and the eye offset is what gets it
    /// there. The homestead's long flat hood keeps it ~11 m out (from 21 m). Hood only (no
    /// dashboard) never shows less.
    #[test]
    fn the_road_shows_from_six_metres_ahead_in_the_cockpit() {
        for (id, limit) in [("gannet_gt", 6.5), ("homestead_wagon", 11.5)] {
            let d = Drive::new(id).unwrap();
            let eye = cockpit_eye(&d, &TUNING, &CameraPrefs::default());
            let seen = road_visible_from(id, eye, true);
            let shaped = road_visible_from(id, d.layout.eye, true);
            let hood = road_visible_from(id, eye, false);
            println!(
                "{id:15} road visible from {seen:.2} m ahead (shape's eye: {shaped:.2} m, \
                 hood only: {hood:.2} m)"
            );
            assert!(seen <= limit, "{id}: {seen} m");
            assert!(seen < shaped, "{id}: {seen} vs {shaped} m");
            assert!(hood <= seen, "{id}: {hood} vs {seen} m");
        }
    }

    #[test]
    fn cockpit_eye_is_inside_the_car() {
        for id in ["gannet_gt", "homestead_wagon"] {
            let d = Drive::new(id).unwrap();
            let rig = CameraRig::new(CameraMode::Cockpit, &d);
            let pose = rig.pose(&d, 1.0);
            let local = d.pose(1.0).rot.inverse() * (pose.eye - d.pose(1.0).pos);
            let eye = cockpit_eye(&d, &TUNING, &CameraPrefs::default());
            assert!((local - eye).length() < 1e-5, "{id} {local}");
            let [len, wid, h] = d.spec.dims_m;
            let ground = -d.spec.cg_height_m;
            assert!(local.x.abs() < 0.5 * wid, "{id} {local}");
            assert!(local.x < 0.0, "left-hand drive");
            assert!(local.y > ground && local.y < ground + h, "{id} {local}");
            // The body's length, centered over its own middle (not the center of mass).
            let mid = 0.5 * (d.layout.wheel_centers[0].z + d.layout.wheel_centers[2].z);
            assert!((local.z - mid).abs() < 0.5 * len, "{id} {local}");
            // Above the dash, below the roof, behind the steering wheel.
            assert!(local.y > d.layout.dash_y + 0.1, "{id} {local}");
            assert!(local.z > d.layout.wheel_pivot.z + 0.3, "{id} {local}");
            // Looking down the road.
            assert!((pose.forward() - d.pose(1.0).forward()).length() < 1e-5);
        }
    }

    #[test]
    fn look_back_flips_yaw() {
        for mode in [CameraMode::Cockpit, CameraMode::Hood] {
            let (d, mut rig) = setup(mode);
            let g = flat();
            let ahead = rig.pose(&d, 1.0).yaw();
            let mut input = InputState::default();
            input.set(Action::LookBack, 1.0);
            rig.step(&d, &input, DT, &g);
            let behind = rig.pose(&d, 1.0).yaw();
            assert!(
                (wrap_pi(behind - ahead).abs() - PI).abs() < 1e-4,
                "{ahead} {behind}"
            );
        }
        // The chase camera swings round to the front and looks back.
        let (d, mut rig) = setup(CameraMode::ChaseMid);
        let g = flat();
        let mut input = InputState::default();
        input.set(Action::LookBack, 1.0);
        for _ in 0..240 {
            rig.step(&d, &input, DT, &g);
        }
        let pose = rig.pose(&d, 1.0);
        let car = d.pose(1.0);
        assert!((pose.eye - car.pos).dot(car.forward()) > 4.0);
        assert!(pose.forward().dot(car.forward()) < -0.9);
    }

    #[test]
    fn look_returns_to_center_after_a_second() {
        let (d, mut rig) = setup(CameraMode::Cockpit);
        let g = flat();
        let mut input = InputState::default();
        input.set(Action::LookX, 1.0);
        for _ in 0..30 {
            rig.step(&d, &input, DT, &g);
        }
        let turned = rig.curr.look_yaw;
        assert!(turned < -0.4, "looks right");
        input.clear();
        for _ in 0..100 {
            rig.step(&d, &input, DT, &g);
        }
        assert_eq!(rig.curr.look_yaw, turned, "held for a second");
        for _ in 0..240 {
            rig.step(&d, &input, DT, &g);
        }
        assert!(rig.curr.look_yaw.abs() < 0.01 * turned.abs());
    }

    #[test]
    fn chase_stays_above_ground_and_in_range_on_a_lap() {
        for (slot, mode) in [
            CameraMode::ChaseNear,
            CameraMode::ChaseMid,
            CameraMode::ChaseFar,
        ]
        .into_iter()
        .enumerate()
        {
            let (mut d, mut rig) = setup(mode);
            let g = flat();
            let t = rig.tuning;
            let reach = {
                let (dd, h) = (t.chase_distance_m[slot], t.chase_height_m[slot]);
                (dd * dd + h * h).sqrt()
            };
            let mut input = InputState::default();
            // A lap: accelerate, turn hard, look around with the stick, brake.
            for i in 0..2400 {
                let s = i as f32 * DT;
                let car = VehicleInput {
                    throttle: if s < 12.0 { 0.7 } else { 0.0 },
                    brake: if s >= 16.0 { 0.8 } else { 0.0 },
                    steer: if (3.0..12.0).contains(&s) { 0.6 } else { 0.0 },
                    handbrake: false,
                };
                input.set(
                    Action::LookX,
                    if (6.0..7.0).contains(&s) { 1.0 } else { 0.0 },
                );
                input.set(
                    Action::LookY,
                    if (6.0..6.5).contains(&s) { 1.0 } else { 0.0 },
                );
                d.step(car, &g);
                rig.step(&d, &input, DT, &g);
                for alpha in [0.0, 0.5, 1.0] {
                    let pose = rig.pose(&d, alpha);
                    assert!(pose.eye.is_finite() && pose.rot.is_finite());
                    assert!(pose.eye.y >= t.min_clearance_m - 1e-4, "{s}: {}", pose.eye);
                    let dist = (pose.eye - d.pose(alpha).pos).length();
                    assert!(
                        dist > 0.6 * reach && dist < 1.5 * reach,
                        "{mode:?} at {s}s: {dist} vs {reach}"
                    );
                }
            }
            // The lot is grass (friction 0.6), so the lap covers less than it would on asphalt.
            assert!(d.odometer_m() > 60.0, "{}", d.odometer_m());
        }
    }

    #[test]
    fn chase_camera_sits_behind_and_above_looking_down_at_the_car() {
        let (d, rig) = setup(CameraMode::ChaseMid);
        let pose = rig.pose(&d, 1.0);
        let car = d.pose(1.0);
        let rel = pose.eye - car.pos;
        assert!(
            (rel.dot(-car.forward()) - TUNING.chase_distance_m[1]).abs() < 0.05,
            "{rel}"
        );
        assert!(
            (pose.eye.y - TUNING.chase_height_m[1]).abs() < 0.05,
            "{}",
            pose.eye
        );
        assert!(pose.forward().y < -0.1, "pitched down");
        assert!(pose.forward().dot(car.forward()) > 0.9);
    }

    /// Through a steady corner the lead takes up the spring's lag: the camera stays close
    /// behind the car instead of swinging out to the inside of the turn.
    #[test]
    fn chase_leads_into_the_turn_at_speed() {
        let run = |lead: f32| {
            let (mut d, mut rig) = setup(CameraMode::ChaseMid);
            rig.tuning.chase_lead_deg = lead;
            let g = flat();
            let input = InputState::default();
            let mut worst = 0.0f32;
            for i in 0..1200 {
                let s = i as f32 * DT;
                let car = VehicleInput {
                    throttle: ((14.0 - d.state.speed_mps) * 0.3).clamp(0.0, 1.0),
                    steer: if s > 6.0 { 0.5 } else { 0.0 },
                    ..VehicleInput::default()
                };
                d.step(car, &g);
                rig.step(&d, &input, DT, &g);
                if s > 8.0 {
                    let off = wrap_pi(rig.curr.yaw - heading(&d, 1.0));
                    worst = worst.max(off.abs());
                }
            }
            worst.to_degrees()
        };
        let (with, without) = (run(TUNING.chase_lead_deg), run(0.0));
        println!(
            "chase yaw offset in a steady corner: {with:.1}° with lead, {without:.1}° without"
        );
        assert!(with < 0.6 * without, "lead {with}° vs no lead {without}°");
        // Standing still the wheel can turn without swinging the camera.
        let (mut d, rig) = setup(CameraMode::ChaseMid);
        d.state.steer = 1.0;
        assert_eq!(chase_lead(&d, &rig.tuning), 0.0);
    }

    #[test]
    fn idle_orbits_and_input_returns() {
        let (d, mut rig) = setup(CameraMode::ChaseMid);
        let g = flat();
        let mut input = InputState::default();
        let steps = (20.5 / DT) as usize;
        for _ in 0..steps {
            rig.step(&d, &input, DT, &g);
        }
        assert_eq!(rig.mode(), CameraMode::Orbit);
        let a = rig.pose(&d, 1.0).eye;
        for _ in 0..120 {
            rig.step(&d, &input, DT, &g);
        }
        let b = rig.pose(&d, 1.0).eye;
        assert!((a - b).length() > 0.5, "circles");
        input.set(Action::Throttle, 0.5);
        rig.step(&d, &input, DT, &g);
        assert_eq!(rig.mode(), CameraMode::ChaseMid);
    }

    #[test]
    fn cockpit_sway_is_small_and_lagged() {
        let (mut d, mut rig) = setup(CameraMode::Cockpit);
        let g = flat();
        let input = InputState::default();
        let mut max = 0.0f32;
        for i in 0..600 {
            let car = VehicleInput {
                throttle: 1.0,
                steer: if i > 300 { 1.0 } else { 0.0 },
                ..VehicleInput::default()
            };
            d.step(car, &g);
            let before = rig.curr.sway;
            rig.step(&d, &input, DT, &g);
            let moved = (rig.curr.sway - before).length();
            assert!(moved < 0.01, "lagged, not snapped");
            max = max.max(rig.curr.sway.abs().max_element());
        }
        assert!(max > 0.005, "sways at all");
        assert!(max <= TUNING.sway_max_m + 1e-6);
    }
}
