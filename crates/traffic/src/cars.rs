//! NPC cars on the lane graph (`docs/NPCS.md`, "As built: cars").
//!
//! A fixed pool of kinematic cars, each on a lane or a turn of a [`LaneGraph`] at a
//! distance `s` of its front bumper along it. Every 120 Hz [`Traffic::step`] plans every
//! car from the same snapshot, then moves them all:
//!
//! - **Following.** The intelligent-driver model against the nearest obstacle along the
//!   car's path (the lane it is on, the turn it chose, the lane after it): the car ahead,
//!   a closed stop line, or the player's car. Desired speed is the lane's posted speed
//!   (its road class) times a per-driver factor, capped through turns by their radius.
//!   On top of the model a hard limit: no step moves a car's front past the obstacle less
//!   a small margin, so no car ever runs into another, a closed stop line or the player.
//!   Under both a backstop: a move that would bring a car's body into contact with any
//!   other car's or the player's is not made (roads can cross without a junction).
//! - **Junctions.** A car must *claim* its turn before it may leave its lane. It claims it
//!   within its comfortable stopping distance of the stop line, when the gate is open: the
//!   light is green (or yellow and too close to stop), a stop sign has been stopped at and
//!   waited out, there is room past the junction, no conflicting turn (one that crosses,
//!   or comes within a car's width of, or merges into the same lane as this one) is
//!   claimed, nobody with the right of way is about to arrive, and the player is not in or
//!   rushing at the junction. A claim is held through the turn until the car's rear has
//!   left it, so crossing streams never share a junction.
//! - **Right of way** among conflicting cars about to arrive: green and uncontrolled
//!   approaches before stop signs; straight and right before left; then whoever arrives
//!   first (stop signs: whoever stopped first); then the lower id. A strict order, so two
//!   cars never wait on each other.
//! - **Lane changes** where the graph has parallel lanes (same road, same direction): to
//!   reach the lane the chosen turn leaves from, or to pass a slower car when going
//!   straight. The switch is instant in the graph and eased sideways in the pose.
//! - **Hit by the player** ([`Traffic::shove`]): the push along the car's way goes into its
//!   speed, the rest slides and turns the drawn car off its path; a real knock has it brake
//!   to a stop and stand a moment, then it drives on and steers back to its lane.
//!
//! [`Traffic::populate`] spawns and despawns cars by density in a ring around the player,
//! always out of the camera's view. Everything is deterministic (one seeded PCG, IEEE
//! basic arithmetic, a fixed car order) and nothing allocates after [`Traffic::new`].

use alloc::vec::Vec;

use skywave_core::{hash64, Rng, Seed, SIM_HZ};

use crate::lanes::{bezier, heading, Control, LaneGraph, Pose, TurnKind, CURVE_STEPS, NONE};
use crate::math::{dist2, dot, len2, right, sin_cos, sqrt, unit2};
use crate::signal::Light;

/// Bumper-to-bumper length every NPC car is simulated with, meters.
pub const CAR_LEN_M: f32 = 4.6;
/// Half the width every NPC car is simulated with, meters.
pub const CAR_HALF_W_M: f32 = 0.9;

const DT: f32 = 1.0 / SIM_HZ as f32;
/// Brake lamps: slowing faster than this lights them (m/s²)...
pub const BRAKE_LAMP_DECEL: f32 = 0.6;
/// ...and they stay lit while the car keeps slowing faster than this (m/s²), or creeps
/// under this speed (m/s)...
const BRAKE_LAMP_HOLD: f32 = 0.2;
const CREEP_MPS: f32 = 1.0;
/// ...and does not speed up faster than this (m/s²): pulling away. A car standing (at or
/// under `STANDING_MPS`) and not pulling away shows them however gently it stopped.
const PULL_AWAY: f32 = 0.3;
const STANDING_MPS: f32 = 0.05;
/// Intelligent-driver model: maximum acceleration, comfortable and emergency braking
/// (m/s²), jam distance behind a car, a stop line and the player (m).
const A_MAX: f32 = 1.4;
const B_COMF: f32 = 2.5;
const B_MAX: f32 = 8.0;
const S0: f32 = 2.0;
const LINE_S0: f32 = 0.6;
const PLAYER_S0: f32 = 3.0;
/// The hard limit: closest a car's front may come to the car ahead, the player (m).
const MIN_GAP: f32 = 0.3;
const PLAYER_GAP: f32 = 0.6;
/// The contact backstop pads a moving car's body by this (m).
const BODY_MARGIN_M: f32 = 0.05;
/// A car the player knocks (changing its speed by at least this, m/s; a lighter touch only
/// nudges it) brakes hard to a stop and stands 1 to 2 s (the driver's share from its id)...
const HOLD_DV: f32 = 0.5;
const HOLD_S: (u32, u32) = (SIM_HZ, SIM_HZ * 2);
/// ...while its shove (sideways and back from where it drives) slides out at this (m/s²)
/// and its yaw rate at this (rad/s²)...
const SLIDE_DECEL: f32 = 8.0;
const SPIN_DECEL: f32 = 3.0;
/// ...but never further off its path than this sideways (m) nor turned more than this
/// (rad), and less where the caller says the road has less room ([`Traffic::shove`]): a
/// shove past it stops there. Pushed this far a car's body stays 0.6 m clear of a
/// neighbouring lane's cars (lanes 3.3 m apart and more)...
pub const SHOVE_SIDE_M: f32 = 0.5;
pub const SHOVE_YAW: f32 = 0.12;
/// The room [`Traffic::shove`] takes where the road has room for every shove: right, left
/// (m) and the turn (rad).
pub const FREE_ROOM: [f32; 3] = [SHOVE_SIDE_M, SHOVE_SIDE_M, SHOVE_YAW];
/// ...then it steers back to its lane: the shove eases out by this per meter driven, the
/// yaw with it in proportion (so the car never crabs), or by this (rad per meter) once the
/// shove is gone.
const SHOVE_RETURN: f32 = 0.15;
const YAW_RETURN: f32 = 0.05;
/// Lateral acceleration through turns (m/s²) and the slowest turn speed (m/s).
const A_LAT: f32 = 2.2;
const MIN_TURN_MPS: f32 = 3.0;
/// How far ahead a car looks: speed × seconds plus meters, clamped (m).
const LOOK_S: f32 = 5.0;
const LOOK_M: f32 = 25.0;
const LOOK_MIN: f32 = 40.0;
const LOOK_MAX: f32 = 90.0;
/// Conflicting cars further than this from their stop line, in seconds, do not count.
const YIELD_ETA_S: f32 = 5.0;
/// A car claims a signal's turn only if it reaches the line this long before red (s).
const RED_MARGIN_S: f32 = 0.5;
/// Turns whose curves come closer than this conflict (m): a car's width plus a margin.
const CONFLICT_M: f32 = 2.2;
/// Cars on turns from the same lane follow each other while the leader's rear is within
/// this of the turns' start (m), where the curves have not yet parted.
const MOUTH_M: f32 = 8.0;
/// Within this of its lane's end a car gives up on changing lanes for its turn and picks
/// a turn from its own lane (m).
const REPICK_M: f32 = 60.0;
/// A car passes only a slower car within this gap (m).
const PASS_GAP_M: f32 = 40.0;
/// Each car considers a lane change every this many ticks.
const CHANGE_EVERY: u64 = 30;
/// Speed a lane change's leftover offset eases out at (m/s).
const SHIFT_MPS: f32 = 1.4;
/// Stopped, for a stop sign: slower than this (m/s) within this of the line (m).
const STOPPED_MPS: f32 = 0.3;
const STOP_NEAR_M: f32 = 2.5;
/// Wait at a stop sign once stopped, ticks (0.5 to 1.5 s).
const STOP_WAIT: (u32, u32) = (SIM_HZ / 2, SIM_HZ * 3 / 2);
/// A turn the player stands on, or will drive across within these seconds, cannot be
/// claimed; turns further than this (m) from the player's reach are not checked.
const PLAYER_ETA_S: f32 = 3.0;
const PLAYER_NODE_M: f32 = 60.0;
/// A spawn keeps this far from every car (m).
const SPAWN_CLEAR_M: f32 = 12.0;
/// Lanes checked for a spawn per step.
const SCAN_LANES: usize = 48;
/// Despawn a hidden car that has not moved for this long (s): longer than any red.
const STUCK_S: u32 = 90;
/// Head lights come on this long after dusk at the latest (s): drivers differ.
const LIGHT_DELAY_S: f32 = 900.0;
/// Hash key of the traffic generator.
const KEY_TRAFFIC: u64 = 0x5452_4146_4649_4300;

/// How many cars the pool holds and where they come and go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrafficBudget {
    /// Pool capacity: the most cars ever live (the busy density).
    pub cars: usize,
    /// Spawn ring around the player, meters.
    pub spawn_min_m: f32,
    pub spawn_max_m: f32,
    /// Hidden cars beyond this from the player despawn, meters.
    pub despawn_m: f32,
    /// Cars hidden this long despawn once out of the spawn ring's inner half, seconds.
    pub hidden_s: f32,
    /// Vehicle shapes to choose from (`Car::shape` is below this).
    pub shapes: u8,
}

impl Default for TrafficBudget {
    fn default() -> TrafficBudget {
        TrafficBudget {
            cars: 96,
            spawn_min_m: 150.0,
            spawn_max_m: 300.0,
            despawn_m: 360.0,
            hidden_s: 15.0,
            shapes: 10,
        }
    }
}

/// The traffic density setting.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    Off = 0,
    Low = 1,
    #[default]
    Normal = 2,
    Busy = 3,
}

impl Density {
    /// Cars to keep live out of a pool of `cap`.
    pub fn target(self, cap: usize) -> usize {
        match self {
            Density::Off => 0,
            Density::Low => cap / 4,
            Density::Normal => cap / 2,
            Density::Busy => cap,
        }
    }
}

/// Where a car stands with the stop sign at the end of its lane.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopState {
    Approach = 0,
    Waiting = 1,
    Done = 2,
}

/// One NPC car. The first block is what the app draws and the sound system hears; the
/// rest is simulation state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Car {
    /// Unique for the life of the [`Traffic`]: follow a car across steps by it.
    pub id: u32,
    /// Ground point under the middle of the car.
    pub pos: [f32; 3],
    /// Yaw in the vehicle sim's convention (0 faces -z), and the same as a unit ground
    /// direction `[x, z]`.
    pub heading: f32,
    pub dir: [f32; 2],
    /// m/s, along the path.
    pub speed: f32,
    /// Vehicle shape index, below [`TrafficBudget::shapes`].
    pub shape: u8,
    /// Seed for the paint color.
    pub paint: u32,
    /// Head lights on.
    pub lights: bool,
    /// Brake lamps lit: slowing harder than [`BRAKE_LAMP_DECEL`] in the last step, and
    /// after that while still slowing or creeping in a queue or at a line, until it pulls
    /// away; and whenever it stands still and is not pulling away.
    pub braking: bool,
    /// Lane id, or the lane count plus a turn id while on a turn.
    pub seg: u32,
    /// The segment before `seg` (the rear may still be on it), or [`NONE`].
    pub prev: u32,
    /// Front bumper along `seg`, meters.
    pub s: f32,
    /// The turn to take at the end of the lane the car is on or heading to, or [`NONE`].
    pub next_turn: u32,
    /// The turn this car has claimed (ahead or under it), or [`NONE`].
    pub held: u32,
    /// The turn the car's rear is still leaving, or [`NONE`]; `clear_left` to go.
    pub clearing: u32,
    pub clear_left: f32,
    /// Desired speed as a share of the posted speed.
    pub v0: f32,
    /// Desired time gap to the car ahead, seconds.
    pub headway: f32,
    /// Ground offset `[x, z]` left over from a lane change, eased out, and the lane
    /// changed from while it lasts ([`NONE`] after): the car still blocks it.
    pub offset: [f32; 2],
    pub ghost: u32,
    /// A shove from the player's car: the ground offset `[x, z]` of the drawn car from its
    /// place on the graph and the yaw off its path (rad, positive turns left), both eased
    /// out as it drives on; the slide and the yaw rate still moving them (m/s, rad/s); and
    /// ticks left standing on the brakes. All zero for a car never hit.
    pub shove: [f32; 2],
    pub yaw: f32,
    pub slide: [f32; 2],
    pub spin: f32,
    pub hold: u32,
    /// How far it may be shoved off its path to its right and left (m) and turned (rad),
    /// from the last hit.
    pub room: [f32; 3],
    pub stop: StopState,
    /// Ticks left to wait at a stop sign; the tick the wait ended.
    pub wait: u32,
    pub stop_tick: u64,
    /// Ticks since the camera last saw the car; ticks it has stood still.
    pub hidden: u32,
    pub stuck: u32,
    /// Seconds after dusk this driver switches the lights on (off as long after dawn).
    pub light_delay_s: f32,
}

/// The player's car, an obstacle every NPC car follows and yields to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Player {
    pub pos: [f32; 3],
    pub heading: f32,
    pub speed: f32,
    pub half_len: f32,
    pub half_w: f32,
}

impl Player {
    /// A player too far away to matter, for scripted scenarios.
    pub const ABSENT: Player = Player {
        pos: [1.0e7, 0.0, 1.0e7],
        heading: 0.0,
        speed: 0.0,
        half_len: 2.3,
        half_w: 0.9,
    };
}

/// The camera, as a horizontal view cone: cars never appear or vanish inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub pos: [f32; 3],
    /// Unit ground direction the camera faces.
    pub dir: [f32; 2],
    /// Tangent of half the horizontal field of view.
    pub tan_half: f32,
}

impl View {
    /// Whether any of a circle of `r` around `p` is in the cone (or close to the camera).
    pub fn sees(&self, p: [f32; 3], r: f32) -> bool {
        let rel = [p[0] - self.pos[0], p[2] - self.pos[2]];
        let along = dot(rel, self.dir);
        if along < -r {
            return false;
        }
        if len2(rel) < r + 2.0 {
            return true;
        }
        let lat = dot(rel, right(self.dir)).abs();
        let cos = 1.0 / sqrt(1.0 + self.tan_half * self.tan_half);
        lat * cos - along * self.tan_half * cos <= r
    }
}

/// Time of day for head lights: dusk and dawn are when the app's streetlights switch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    pub seconds_of_day: f32,
    pub dusk_s: f32,
    pub dawn_s: f32,
}

impl Clock {
    /// Whether a driver who switches `delay_s` after dusk (and as long after dawn) has
    /// the head lights on.
    pub fn dark(&self, delay_s: f32) -> bool {
        let day = 86_400.0;
        let mut t = self.seconds_of_day - delay_s;
        if t < 0.0 {
            t += day;
        }
        if self.dusk_s > self.dawn_s {
            t >= self.dusk_s || t < self.dawn_s
        } else {
            t >= self.dusk_s && t < self.dawn_s
        }
    }
}

/// What the caller feeds every step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    /// The signal tick ([`crate::SignalClock`]).
    pub tick: u64,
    pub player: Player,
    pub view: View,
    pub clock: Clock,
    pub density: Density,
}

/// The player as three circles along its length, for path checks.
struct PlayerShape {
    pts: [[f32; 2]; 3],
    r: f32,
    half_len: f32,
    dir: [f32; 2],
    speed: f32,
}

impl PlayerShape {
    fn new(p: &Player) -> PlayerShape {
        let (sn, cs) = sin_cos(p.heading);
        let d = [-sn, -cs];
        let k = (p.half_len - p.half_w).max(0.0);
        let c = [p.pos[0], p.pos[2]];
        PlayerShape {
            pts: [
                [c[0] + d[0] * k, c[1] + d[1] * k],
                c,
                [c[0] - d[0] * k, c[1] - d[1] * k],
            ],
            r: p.half_w,
            half_len: p.half_len,
            dir: d,
            speed: p.speed,
        }
    }
}

/// Up to four segments along a car's path: segment and distance from the car's front to
/// its start (negative for the one it is on).
type Path = [(u32, f32); 4];

/// NPC traffic over one lane graph: build it with the graph it will step on.
#[derive(Debug)]
pub struct Traffic {
    pub budget: TrafficBudget,
    cars: Vec<Car>,
    rng: Rng,
    next_id: u32,
    lanes: u32,
    /// Per segment, the frontmost and rearmost car on it while planning.
    head: Vec<u32>,
    tail: Vec<u32>,
    /// Per car slot, the car in front and behind on the same segment while planning.
    ahead: Vec<u32>,
    behind: Vec<u32>,
    /// Per car slot, the planned acceleration, the most it may move this step, and the gap
    /// to the player when the player is on its path.
    plan: Vec<(f32, f32, f32)>,
    /// Claims held per turn.
    claims: Vec<u16>,
    /// Per turn, the turns it conflicts with (sorted), as offsets into `conflicts`.
    conflict_first: Vec<u32>,
    conflicts: Vec<u32>,
    /// Per node, the lanes ending there.
    incoming_first: Vec<u32>,
    incoming: Vec<u32>,
    /// Per node, the middle of its turns.
    node_pos: Vec<[f32; 2]>,
    /// Per lane, a bounding circle `[x, z, r]`.
    lane_ball: Vec<[f32; 3]>,
    cursor: u32,
    /// Cars with a [`Car::ghost`].
    ghosts: u32,
    /// Pool slots sorted by `x_key`, each car's x at the start of the step.
    by_x: Vec<u32>,
    x_key: Vec<f32>,
}

impl Clone for Traffic {
    /// A copy with the same reserved pool, so stepping it never allocates either.
    fn clone(&self) -> Traffic {
        let cap = self.budget.cars;
        let mut cars = Vec::with_capacity(cap);
        cars.extend_from_slice(&self.cars);
        let mut by_x = Vec::with_capacity(cap);
        by_x.extend_from_slice(&self.by_x);
        Traffic {
            budget: self.budget,
            cars,
            rng: self.rng.clone(),
            next_id: self.next_id,
            lanes: self.lanes,
            head: self.head.clone(),
            tail: self.tail.clone(),
            ahead: self.ahead.clone(),
            behind: self.behind.clone(),
            plan: self.plan.clone(),
            claims: self.claims.clone(),
            conflict_first: self.conflict_first.clone(),
            conflicts: self.conflicts.clone(),
            incoming_first: self.incoming_first.clone(),
            incoming: self.incoming.clone(),
            node_pos: self.node_pos.clone(),
            lane_ball: self.lane_ball.clone(),
            cursor: self.cursor,
            ghosts: self.ghosts,
            by_x,
            x_key: self.x_key.clone(),
        }
    }
}

fn seg_dist(a0: [f32; 2], a1: [f32; 2], b0: [f32; 2], b1: [f32; 2]) -> f32 {
    let cross = |o: [f32; 2], p: [f32; 2], q: [f32; 2]| {
        (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0])
    };
    let (d1, d2) = (cross(a0, a1, b0), cross(a0, a1, b1));
    let (d3, d4) = (cross(b0, b1, a0), cross(b0, b1, a1));
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
        return 0.0;
    }
    point_seg(a0, b0, b1)
        .0
        .min(point_seg(a1, b0, b1).0)
        .min(point_seg(b0, a0, a1).0)
        .min(point_seg(b1, a0, a1).0)
}

/// Distance from `p` to segment `a`-`b`, and where along it `p` projects (0..1).
fn point_seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, f32) {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let l2 = dot(ab, ab);
    let t = if l2 > 0.0 {
        (dot(ap, ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (len2([ap[0] - ab[0] * t, ap[1] - ab[1] * t]), t)
}

/// Arc length along `lane` of its point nearest `p`.
fn project(g: &LaneGraph, lane: u32, p: [f32; 2]) -> f32 {
    let l = &g.lanes[lane as usize];
    let r = l.first_point as usize..(l.first_point + l.point_count) as usize;
    let (pts, arc) = (&g.points[r.clone()], &g.arc[r]);
    let mut best = (f32::INFINITY, 0.0);
    for k in 0..pts.len() - 1 {
        let (d, t) = point_seg(p, xz(pts[k]), xz(pts[k + 1]));
        if d < best.0 {
            best = (d, arc[k] + (arc[k + 1] - arc[k]) * t);
        }
    }
    best.1
}

/// Corners of a box `half_len` by `half_w` around `pos` facing ground direction `f`.
fn body(pos: [f32; 3], f: [f32; 2], half_len: f32, half_w: f32) -> [[f32; 2]; 4] {
    let r = right(f);
    let at = |a: f32, b: f32| [pos[0] + f[0] * a + r[0] * b, pos[2] + f[1] * a + r[1] * b];
    [
        at(half_len, half_w),
        at(half_len, -half_w),
        at(-half_len, -half_w),
        at(-half_len, half_w),
    ]
}

/// Whether two boxes from [`body`] overlap (separating axes).
fn touch(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    for poly in [a, b] {
        for k in 0..2 {
            let (p, q) = (poly[k], poly[k + 1]);
            let n = [q[1] - p[1], p[0] - q[0]];
            let span = |s: &[[f32; 2]; 4]| {
                let mut lo_hi = (f32::INFINITY, f32::NEG_INFINITY);
                for v in s {
                    let d = dot(*v, n);
                    lo_hi = (lo_hi.0.min(d), lo_hi.1.max(d));
                }
                lo_hi
            };
            let (x, y) = (span(a), span(b));
            if x.1 <= y.0 || y.1 <= x.0 {
                return false;
            }
        }
    }
    true
}

fn xz(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[2]]
}

/// Intelligent-driver acceleration at speed `v` toward desired `v0` behind an obstacle
/// `gap` meters ahead closing at `dv`.
fn idm(v: f32, v0: f32, headway: f32, gap: f32, dv: f32, s0: f32) -> f32 {
    if gap <= 0.1 {
        return -B_MAX;
    }
    let dyn_gap = (v * headway + v * dv / (2.0 * sqrt(A_MAX * B_COMF))).max(0.0);
    let k = (s0 + dyn_gap) / gap;
    free_accel(v, v0) - A_MAX * k * k
}

fn free_accel(v: f32, v0: f32) -> f32 {
    if v > v0 {
        -B_COMF * ((v - v0) * 0.5).min(1.0)
    } else {
        let r = v / v0.max(0.1);
        A_MAX * (1.0 - r * r * r * r)
    }
}

/// Distance in which a car at `v` stops comfortably, plus a margin: where it commits.
fn commit_m(v: f32) -> f32 {
    v * v / (2.0 * B_COMF) + 3.0
}

/// Within this of its lane's end a car stops trying to reach another lane for its turn:
/// before it would commit to the turn.
fn repick_m(v: f32) -> f32 {
    REPICK_M.max(commit_m(v) + 10.0)
}

/// Seconds to cover `d` meters from `v`, gathering speed at half the maximum.
fn time_to(d: f32, v: f32) -> f32 {
    let a = 0.5 * A_MAX;
    (sqrt(v * v + 2.0 * a * d.max(0.0)) - v) / a
}

fn turn_speed(g: &LaneGraph, turn: u32) -> f32 {
    let r = g.turns[turn as usize].min_radius_m.min(1000.0);
    sqrt(A_LAT * r).max(MIN_TURN_MPS)
}

fn turn_weight(kind: TurnKind) -> f32 {
    match kind {
        TurnKind::Straight => 6.0,
        TurnKind::Left | TurnKind::Right => 2.0,
        TurnKind::UTurn => 0.25,
    }
}

impl Traffic {
    /// A pool for `graph` with `budget`, seeded by `seed`. Precomputes which turns
    /// conflict at every junction; this is the only place that allocates.
    pub fn new(graph: &LaneGraph, budget: TrafficBudget, seed: Seed) -> Traffic {
        let g = graph;
        let (nl, nt, nn) = (g.lanes.len(), g.turns.len(), g.junctions.len());

        let mut incoming_first = alloc::vec![0u32; nn + 1];
        for l in &g.lanes {
            incoming_first[l.end_node as usize + 1] += 1;
        }
        for k in 0..nn {
            incoming_first[k + 1] += incoming_first[k];
        }
        let mut incoming = alloc::vec![0u32; nl];
        let mut fill = incoming_first.clone();
        for (id, l) in g.lanes.iter().enumerate() {
            let f = &mut fill[l.end_node as usize];
            incoming[*f as usize] = id as u32;
            *f += 1;
        }

        // What a car on each turn sweeps until its claim ends: the curve at even
        // parameter steps, then the start of the lane after it, where its rear clears.
        const N: usize = 8;
        const LAND: usize = 3;
        let poly: Vec<[[f32; 2]; N + LAND + 1]> = g
            .turns
            .iter()
            .map(|t| {
                let mut p = [[0.0; 2]; N + LAND + 1];
                for (k, q) in p.iter_mut().enumerate() {
                    *q = if k <= N {
                        xz(bezier(&t.curve, k as f32 / N as f32))
                    } else {
                        let s = (CAR_LEN_M + 1.0) * (k - N) as f32 / LAND as f32;
                        xz(g.pose(t.to, s).pos)
                    };
                }
                p
            })
            .collect();
        let bbox = |p: &[[f32; 2]; N + LAND + 1]| {
            let mut b = [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ];
            for q in p {
                b = [
                    b[0].min(q[0]),
                    b[1].min(q[1]),
                    b[2].max(q[0]),
                    b[3].max(q[1]),
                ];
            }
            b
        };
        let boxes: Vec<[f32; 4]> = poly.iter().map(bbox).collect();
        let mut node_pos = alloc::vec![[0.0f32; 2]; nn];
        for (node, pos) in node_pos.iter_mut().enumerate() {
            let range = incoming_first[node] as usize..incoming_first[node + 1] as usize;
            let (mut mid, mut n) = ([0.0f32; 2], 0.0f32);
            for &lane in &incoming[range] {
                for t in g.next(lane) {
                    mid[0] += (t.curve[0][0] + t.curve[3][0]) * 0.5;
                    mid[1] += (t.curve[0][2] + t.curve[3][2]) * 0.5;
                    n += 1.0;
                }
            }
            if n > 0.0 {
                *pos = [mid[0] / n, mid[1] / n];
            }
        }
        // Conflicts by sweeping the turns' boxes along x: turns of two close junctions
        // can cross too.
        let mouth: Vec<usize> = poly
            .iter()
            .map(|p| {
                let mut acc = 0.0;
                let mut k = 0;
                while k < N + LAND && acc < MOUTH_M + CAR_LEN_M {
                    acc += len2([p[k + 1][0] - p[k][0], p[k + 1][1] - p[k][1]]);
                    k += 1;
                }
                k
            })
            .collect();
        let mut order: Vec<u32> = (0..nt as u32).collect();
        order.sort_by(|&a, &b| {
            boxes[a as usize][0]
                .total_cmp(&boxes[b as usize][0])
                .then(a.cmp(&b))
        });
        let mut per: Vec<Vec<u32>> = alloc::vec![Vec::new(); nt];
        for (x, &a) in order.iter().enumerate() {
            let (a, ba) = (a as usize, boxes[a as usize]);
            for &b in &order[x + 1..] {
                let (b, bb) = (b as usize, boxes[b as usize]);
                if bb[0] > ba[2] + CONFLICT_M {
                    break;
                }
                let (ta, tb) = (&g.turns[a], &g.turns[b]);
                if bb[1] > ba[3] + CONFLICT_M || ba[1] > bb[3] + CONFLICT_M {
                    continue;
                }
                // Turns from one lane share their mouth, where cars follow each other;
                // past it they conflict like any other where they run close.
                let (ia, ib) = if ta.from == tb.from {
                    (mouth[a], mouth[b])
                } else {
                    (0, 0)
                };
                let hit = (ta.to == tb.to && ta.from != tb.from)
                    || (ia..N + LAND).any(|i| {
                        (ib..N + LAND).any(|j| {
                            seg_dist(poly[a][i], poly[a][i + 1], poly[b][j], poly[b][j + 1])
                                < CONFLICT_M
                        })
                    });
                if hit {
                    per[a].push(b as u32);
                    per[b].push(a as u32);
                }
            }
        }
        let mut conflict_first = Vec::with_capacity(nt + 1);
        let mut conflicts = Vec::new();
        for list in &mut per {
            list.sort_unstable();
            conflict_first.push(conflicts.len() as u32);
            conflicts.extend_from_slice(list);
        }
        conflict_first.push(conflicts.len() as u32);

        let lane_ball = (0..nl as u32)
            .map(|id| {
                let mut b = [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ];
                for p in g.points(id) {
                    b = [
                        b[0].min(p[0]),
                        b[1].min(p[2]),
                        b[2].max(p[0]),
                        b[3].max(p[2]),
                    ];
                }
                let c = [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];
                [c[0], c[1], len2([b[2] - c[0], b[3] - c[1]])]
            })
            .collect();

        let cap = budget.cars;
        Traffic {
            budget,
            cars: Vec::with_capacity(cap),
            rng: Rng::new(seed, &[KEY_TRAFFIC]),
            next_id: 0,
            lanes: nl as u32,
            head: alloc::vec![NONE; nl + nt],
            tail: alloc::vec![NONE; nl + nt],
            ahead: alloc::vec![NONE; cap],
            behind: alloc::vec![NONE; cap],
            plan: alloc::vec![(0.0, 0.0, f32::INFINITY); cap],
            claims: alloc::vec![0; nt],
            conflict_first,
            conflicts,
            incoming_first,
            incoming,
            node_pos,
            lane_ball,
            cursor: 0,
            ghosts: 0,
            by_x: Vec::with_capacity(cap),
            x_key: alloc::vec![0.0; cap],
        }
    }

    /// The live cars, in no particular order (follow one by [`Car::id`]).
    pub fn cars(&self) -> &[Car] {
        &self.cars
    }

    /// Whether `a` and `b` conflict at their junction.
    pub fn conflict(&self, a: u32, b: u32) -> bool {
        self.conflicts_of(a).binary_search(&b).is_ok()
    }

    fn conflicts_of(&self, t: u32) -> &[u32] {
        let (a, b) = (
            self.conflict_first[t as usize],
            self.conflict_first[t as usize + 1],
        );
        &self.conflicts[a as usize..b as usize]
    }

    fn seg_len(&self, g: &LaneGraph, seg: u32) -> f32 {
        if seg < self.lanes {
            g.lanes[seg as usize].length_m
        } else {
            g.turns[(seg - self.lanes) as usize].length_m
        }
    }

    fn seg_pose(&self, g: &LaneGraph, seg: u32, s: f32) -> Pose {
        if seg < self.lanes {
            g.pose(seg, s)
        } else {
            g.turn_pose(seg - self.lanes, s)
        }
    }

    /// A car of fresh seeded looks and temperament with its front `s` meters along `lane`
    /// at `speed`, choosing its turn. Its id, or `None` when the pool is full. Does not
    /// check for room: [`Traffic::populate`] does, scripted scenarios place cars apart.
    pub fn spawn_at(&mut self, g: &LaneGraph, lane: u32, s: f32, speed: f32) -> Option<u32> {
        if self.cars.len() >= self.budget.cars {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let shape = self.rng.range(0, u32::from(self.budget.shapes.max(1))) as u8;
        let paint = self.rng.next_u32();
        let v0 = self.rng.range_f32(0.9, 1.1);
        let headway = self.rng.range_f32(1.1, 1.6);
        let light_delay_s = self.rng.range_f32(0.0, LIGHT_DELAY_S);
        let next_turn = self.pick(g, lane, false);
        let mut car = Car {
            id,
            pos: [0.0; 3],
            heading: 0.0,
            dir: [0.0, 0.0],
            speed,
            shape,
            paint,
            lights: false,
            braking: false,
            seg: lane,
            prev: NONE,
            s,
            next_turn,
            held: NONE,
            clearing: NONE,
            clear_left: 0.0,
            v0,
            headway,
            offset: [0.0; 2],
            ghost: NONE,
            shove: [0.0; 2],
            yaw: 0.0,
            slide: [0.0; 2],
            spin: 0.0,
            hold: 0,
            room: [0.0; 3],
            stop: StopState::Approach,
            wait: 0,
            stop_tick: 0,
            hidden: 0,
            stuck: 0,
            light_delay_s,
        };
        self.place(g, &mut car);
        self.cars.push(car);
        Some(id)
    }

    /// A turn to take at the end of `lane`: from any lane of its road and direction, or
    /// only its own, weighted toward going straight. [`NONE`] when there is none.
    fn pick(&mut self, g: &LaneGraph, lane: u32, own: bool) -> u32 {
        let (lo, hi) = if own {
            (lane, lane + 1)
        } else {
            self.group(g, lane)
        };
        let weight = |l: u32, k: TurnKind| turn_weight(k) * if l == lane { 2.0 } else { 1.0 };
        let mut total = 0.0;
        for l in lo..hi {
            for t in g.next(l) {
                total += weight(l, t.kind);
            }
        }
        if total <= 0.0 {
            return NONE;
        }
        let mut r = self.rng.next_f32() * total;
        let mut last = NONE;
        for l in lo..hi {
            let first = g.lanes[l as usize].first_turn;
            for (k, t) in g.next(l).iter().enumerate() {
                last = first + k as u32;
                r -= weight(l, t.kind);
                if r < 0.0 {
                    return last;
                }
            }
        }
        last
    }

    /// The lanes of `lane`'s road and direction, slot 0 (leftmost) first.
    fn group(&self, g: &LaneGraph, lane: u32) -> (u32, u32) {
        let l = &g.lanes[lane as usize];
        let same = |o: u32| {
            let m = &g.lanes[o as usize];
            m.edge == l.edge && m.forward == l.forward
        };
        let (mut lo, mut hi) = (lane, lane + 1);
        while lo > 0 && same(lo - 1) {
            lo -= 1;
        }
        while hi < self.lanes && same(hi) {
            hi += 1;
        }
        (lo, hi)
    }

    fn path(&self, g: &LaneGraph, c: &Car) -> Path {
        let mut p = [(NONE, 0.0); 4];
        p[0] = (c.seg, -c.s);
        let mut start = self.seg_len(g, c.seg) - c.s;
        let mut seg = c.seg;
        // Whether `next_turn` is still ahead on the path.
        let mut turn_pending = true;
        for slot in p.iter_mut().skip(1) {
            let next = if seg < self.lanes {
                let t = c.next_turn;
                if !turn_pending || t == NONE || g.turns[t as usize].from != seg {
                    break;
                }
                turn_pending = false;
                self.lanes + t
            } else {
                g.turns[(seg - self.lanes) as usize].to
            };
            *slot = (next, start);
            start += self.seg_len(g, next);
            seg = next;
        }
        p
    }

    fn link(&mut self) {
        for i in 0..self.cars.len() {
            let (seg, s, id) = (self.cars[i].seg as usize, self.cars[i].s, self.cars[i].id);
            let (mut prev, mut cur) = (NONE, self.head[seg]);
            while cur != NONE {
                let o = &self.cars[cur as usize];
                if o.s < s || (o.s == s && o.id > id) {
                    break;
                }
                prev = cur;
                cur = self.behind[cur as usize];
            }
            self.ahead[i] = prev;
            self.behind[i] = cur;
            if prev == NONE {
                self.head[seg] = i as u32;
            } else {
                self.behind[prev as usize] = i as u32;
            }
            if cur == NONE {
                self.tail[seg] = i as u32;
            } else {
                self.ahead[cur as usize] = i as u32;
            }
        }
    }

    fn unlink(&mut self) {
        for c in &self.cars {
            self.head[c.seg as usize] = NONE;
            self.tail[c.seg as usize] = NONE;
        }
    }

    /// Ranks a car about to cross its junction for the right of way, lower first, or
    /// `None` when it does not contend (red light, stop not yet made, still far off).
    fn rank(&self, g: &LaneGraph, c: &Car, d: f32, tick: u64) -> Option<(u8, u8, f32, u32)> {
        let lane = c.seg;
        let t = &g.turns[c.next_turn as usize];
        let left = u8::from(matches!(t.kind, TurnKind::Left | TurnKind::UTurn));
        let eta = d.max(0.0) / c.speed.max(1.0);
        match g.lanes[lane as usize].control {
            Control::Stop => {
                (c.stop == StopState::Done).then(|| (1, 0, -((tick - c.stop_tick) as f32), c.id))
            }
            Control::Signal if g.light(lane, tick) == Some(Light::Red) => None,
            _ => Some((0, left, eta, c.id)),
        }
    }

    /// Whether the player stands on `turn` (at a junction around `node`), or is driving
    /// across it within the next few seconds.
    fn player_on_turn(g: &LaneGraph, turn: u32, ps: &PlayerShape, node: [f32; 2]) -> bool {
        let p = ps.pts[1];
        let reach = ps.speed * PLAYER_ETA_S;
        if len2([node[0] - p[0], node[1] - p[1]]) > PLAYER_NODE_M + reach + ps.half_len {
            return false;
        }
        let d = ps.dir;
        let back = [p[0] - d[0] * ps.half_len, p[1] - d[1] * ps.half_len];
        let ahead = ps.half_len + if ps.speed > 1.0 { reach } else { 0.0 };
        let front = [p[0] + d[0] * ahead, p[1] + d[1] * ahead];
        let clear = ps.r + CAR_HALF_W_M + 0.5;
        let curve = &g.turns[turn as usize].curve;
        let mut prev = xz(curve[0]);
        for k in 1..=CURVE_STEPS {
            let q = xz(bezier(curve, k as f32 / CURVE_STEPS as f32));
            if seg_dist(prev, q, back, front) < clear {
                return true;
            }
            prev = q;
        }
        false
    }

    /// Whether car `i`, `d` meters from its stop line, may claim `turn` now.
    fn gate(
        &self,
        g: &LaneGraph,
        inp: &Inputs,
        ps: &PlayerShape,
        i: usize,
        turn: u32,
        d: f32,
    ) -> bool {
        let c = &self.cars[i];
        let lane = c.seg;
        let ok = match g.lanes[lane as usize].control {
            Control::Signal => {
                // Commit only to what clears the line before red.
                let left = g.until_red(lane, inp.tick).unwrap_or(0) as f32 * DT;
                let makes_it = time_to(d, c.speed) + RED_MARGIN_S < left;
                match g.light(lane, inp.tick) {
                    Some(Light::Green) => makes_it || d > commit_m(c.speed),
                    Some(Light::Yellow) => makes_it && d <= commit_m(c.speed),
                    _ => false,
                }
            }
            Control::Stop => c.stop == StopState::Done,
            Control::Free | Control::DeadEnd => true,
        };
        if !ok {
            return false;
        }
        let t = &g.turns[turn as usize];
        // Room past the junction for the car, and for those already through this turn.
        let to = t.to;
        let to_len = g.lanes[to as usize].length_m;
        let last = self.tail[to as usize];
        let mut room = if last == NONE {
            to_len
        } else {
            self.cars[last as usize].s - CAR_LEN_M
        };
        room -= f32::from(self.claims[turn as usize]) * (CAR_LEN_M + S0);
        if room < (CAR_LEN_M + 1.0).min(to_len) {
            return false;
        }
        if self
            .conflicts_of(turn)
            .iter()
            .any(|&o| self.claims[o as usize] > 0)
        {
            return false;
        }
        if Self::player_on_turn(g, turn, ps, self.node_pos[t.node as usize]) {
            return false;
        }
        // Cars with the right of way about to arrive.
        let Some(me) = self.rank(g, c, d, inp.tick) else {
            return false;
        };
        let node = t.node as usize;
        let range = self.incoming_first[node] as usize..self.incoming_first[node + 1] as usize;
        for &k in &self.incoming[range] {
            let h = self.head[k as usize];
            if k == lane || h == NONE {
                continue;
            }
            let hc = &self.cars[h as usize];
            if hc.held != NONE || hc.next_turn == NONE || !self.conflict(turn, hc.next_turn) {
                continue;
            }
            let dh = g.lanes[k as usize].length_m - hc.s;
            let Some(them) = self.rank(g, hc, dh, inp.tick) else {
                continue;
            };
            if them.0 == 0 && them.2 > YIELD_ETA_S {
                continue;
            }
            if them < me {
                return false;
            }
        }
        true
    }

    /// Whether the end of car `i`'s current lane, `d` meters ahead, lets it through:
    /// claims the turn when the gate is open and the car is within its commit distance,
    /// gives a claim back on red (claims clear the line before red, so only a car held up
    /// on its way loses one).
    fn current_gate(
        &mut self,
        g: &LaneGraph,
        inp: &Inputs,
        ps: &PlayerShape,
        i: usize,
        d: f32,
    ) -> bool {
        let c = self.cars[i];
        let turn = c.next_turn;
        if turn == NONE || g.turns[turn as usize].from != c.seg {
            return false;
        }
        if c.held == turn {
            if g.light(c.seg, inp.tick) == Some(Light::Red) {
                self.claims[turn as usize] -= 1;
                self.cars[i].held = NONE;
                return false;
            }
            return true;
        }
        let open = self.gate(g, inp, ps, i, turn, d);
        if open && d <= commit_m(c.speed) {
            self.claims[turn as usize] += 1;
            self.cars[i].held = turn;
        }
        open
    }

    /// Whether a lane the car has not reached yet lets it through, as far as it can tell.
    fn future_gate(g: &LaneGraph, lane: u32, turn: u32, tick: u64) -> bool {
        match g.lanes[lane as usize].control {
            Control::Stop => false,
            Control::Signal => g.light(lane, tick) == Some(Light::Green),
            _ => turn != NONE,
        }
    }

    /// Gap from car `c`'s front to the player along its path, when the player is on it.
    fn player_gap(
        &self,
        g: &LaneGraph,
        c: &Car,
        path: &Path,
        ps: &PlayerShape,
        look: f32,
    ) -> Option<f32> {
        let center = ps.pts[1];
        if len2([center[0] - c.pos[0], center[1] - c.pos[2]]) > look + 10.0 {
            return None;
        }
        let reach = ps.r + CAR_HALF_W_M + 0.4;
        let mut best: Option<f32> = None;
        let mut test = |a: [f32; 2], b: [f32; 2], start: f32, len: f32| {
            for q in ps.pts {
                let (d, t) = point_seg(q, a, b);
                let along = start + len * t;
                if d < reach && along > -CAR_LEN_M * 0.5 && along < look {
                    let gap = along - ps.r;
                    best = Some(best.map_or(gap, |b: f32| b.min(gap)));
                }
            }
        };
        for &(seg, start) in path {
            if seg == NONE || start > look {
                break;
            }
            if seg < self.lanes {
                let l = &g.lanes[seg as usize];
                let r = l.first_point as usize..(l.first_point + l.point_count) as usize;
                let (pts, arc) = (&g.points[r.clone()], &g.arc[r]);
                let from = (-start).max(0.0);
                let k0 = arc[1..].partition_point(|&v| v < from).min(pts.len() - 2);
                for k in k0..pts.len() - 1 {
                    let s = start + arc[k];
                    if s > look {
                        break;
                    }
                    test(xz(pts[k]), xz(pts[k + 1]), s, arc[k + 1] - arc[k]);
                }
            } else {
                let curve = &g.turns[(seg - self.lanes) as usize].curve;
                let mut prev = curve[0];
                let mut acc = start;
                for k in 1..=CURVE_STEPS {
                    let p = bezier(curve, k as f32 / CURVE_STEPS as f32);
                    let len = dist2(prev, p);
                    test(xz(prev), xz(p), acc, len);
                    acc += len;
                    prev = p;
                }
            }
        }
        best
    }

    /// The rearmost car ahead of `after` on another turn from the same lane as turn
    /// segment `seg`, while it is still in the mouth the turns share, or [`NONE`].
    fn mouth_leader(&self, g: &LaneGraph, seg: u32, after: f32) -> u32 {
        let t = seg - self.lanes;
        let from = &g.lanes[g.turns[t as usize].from as usize];
        let mut best = NONE;
        for o in from.first_turn..from.first_turn + from.turn_count {
            if o == t {
                continue;
            }
            let mut j = self.tail[(self.lanes + o) as usize];
            while j != NONE && self.cars[j as usize].s <= after {
                j = self.ahead[j as usize];
            }
            if j == NONE {
                continue;
            }
            let s = self.cars[j as usize].s;
            if s - CAR_LEN_M < MOUTH_M && (best == NONE || s < self.cars[best as usize].s) {
                best = j;
            }
        }
        best
    }

    /// Plans car `i` from the snapshot: its acceleration and the most it may move.
    fn plan_car(
        &mut self,
        g: &LaneGraph,
        inp: &Inputs,
        ps: &PlayerShape,
        i: usize,
    ) -> (f32, f32, f32) {
        let nl = self.lanes;
        {
            let c = self.cars[i];
            if c.seg < nl {
                let l = &g.lanes[c.seg as usize];
                let d = l.length_m - c.s;
                let stranded = c.next_turn != NONE && g.turns[c.next_turn as usize].from != c.seg;
                if c.held == NONE && stranded && d < repick_m(c.speed) {
                    self.cars[i].next_turn = self.pick(g, c.seg, true);
                }
                // A claim stands only while the car is committed: a car held up (a car
                // ahead, a slowdown) gives it back, and nobody keeps one on red.
                let red = g.light(c.seg, inp.tick) == Some(Light::Red);
                if c.held != NONE && (red || d > commit_m(c.speed) + 2.0) {
                    self.claims[c.held as usize] -= 1;
                    self.cars[i].held = NONE;
                }
                if l.control == Control::Stop {
                    match c.stop {
                        StopState::Approach if d < STOP_NEAR_M && c.speed < STOPPED_MPS => {
                            self.cars[i].stop = StopState::Waiting;
                            self.cars[i].wait = self.rng.range(STOP_WAIT.0, STOP_WAIT.1);
                        }
                        StopState::Waiting if c.wait > 0 => self.cars[i].wait -= 1,
                        StopState::Waiting => {
                            self.cars[i].stop = StopState::Done;
                            self.cars[i].stop_tick = inp.tick;
                        }
                        _ => {}
                    }
                }
            }
        }
        let c = self.cars[i];
        let v = c.speed;
        let mut v0 = if c.seg < nl {
            c.v0 * g.lanes[c.seg as usize].speed_mps
        } else {
            let t = c.seg - nl;
            let to = g.turns[t as usize].to;
            (c.v0 * g.lanes[to as usize].speed_mps).min(turn_speed(g, t))
        };
        v0 = v0.max(1.0);
        let look = (v * LOOK_S + LOOK_M).clamp(LOOK_MIN, LOOK_MAX);
        let mut a = free_accel(v, v0);
        let mut max_ds = f32::INFINITY;
        let path = self.path(g, &c);
        for k in 0..path.len() {
            let (seg, start) = path[k];
            if seg == NONE || start > look {
                break;
            }
            let mut leader = if k == 0 {
                self.ahead[i]
            } else {
                self.tail[seg as usize]
            };
            if k == 0 && self.ghosts > 0 {
                for (j, o) in self.cars.iter().enumerate() {
                    let nearer = leader == NONE || o.s < self.cars[leader as usize].s;
                    if o.ghost == seg && o.s > c.s && nearer {
                        leader = j as u32;
                    }
                }
            }
            if seg >= nl {
                let side = self.mouth_leader(g, seg, if k == 0 { c.s } else { f32::NEG_INFINITY });
                if side != NONE
                    && (leader == NONE || self.cars[side as usize].s < self.cars[leader as usize].s)
                {
                    leader = side;
                }
            }
            if leader != NONE {
                let l = &self.cars[leader as usize];
                let gap = start + l.s - CAR_LEN_M;
                a = a.min(idm(v, v0, c.headway, gap, v - l.speed, S0));
                max_ds = max_ds.min(gap - MIN_GAP);
                break;
            }
            if seg >= nl {
                continue;
            }
            let d = start + g.lanes[seg as usize].length_m;
            if d > look {
                break;
            }
            let open = if k == 0 {
                self.current_gate(g, inp, ps, i, d)
            } else {
                Self::future_gate(g, seg, c.next_turn, inp.tick)
            };
            if !open {
                a = a.min(idm(v, v0, c.headway, d, v, LINE_S0));
                max_ds = max_ds.min(d);
                break;
            }
            let (next, _) = path.get(k + 1).copied().unwrap_or((NONE, 0.0));
            if next != NONE {
                let vt = turn_speed(g, next - nl);
                if v > vt {
                    a = a.min((vt * vt - v * v) / (2.0 * d.max(1.0)));
                }
            }
        }
        let player = self.player_gap(g, &c, &path, ps, look);
        if let Some(gap) = player {
            let vp = (dot(ps.dir, c.dir) * ps.speed).max(0.0);
            a = a.min(idm(v, v0, c.headway, gap, v - vp, PLAYER_S0));
            max_ds = max_ds.min(gap - PLAYER_GAP);
        }
        let player = player.unwrap_or(f32::INFINITY);
        if c.hold > 0 {
            a = -B_MAX;
        }
        (a.clamp(-B_MAX, A_MAX), max_ds, player)
    }

    /// Moves car `i` by its plan along its path, unless the move would bring its body
    /// into contact with another car's or the player's: a backstop under the plan's own
    /// limits for geometry they cannot see (roads that cross without a junction).
    fn move_car(&mut self, g: &LaneGraph, i: usize, ps: &PlayerShape) {
        let (a, max_ds, _) = self.plan[i];
        let nl = self.lanes;
        let old = self.cars[i];
        let mut c = old;
        let mut v = (c.speed + a * DT).max(0.0);
        let mut ds = v * DT;
        let limit = max_ds.max(0.0);
        if ds > limit {
            ds = limit;
            v = ds / DT;
        }
        // Claims given back once the move stands.
        let mut release = [NONE; 2];
        c.s += ds;
        loop {
            let len = self.seg_len(g, c.seg);
            if c.s <= len {
                break;
            }
            if c.seg < nl {
                if c.next_turn == NONE || c.held != c.next_turn {
                    // Never leave a lane without a claim.
                    c.s = len;
                    v = 0.0;
                    break;
                }
                c.s -= len;
                c.prev = c.seg;
                c.seg = nl + c.next_turn;
                let to = g.turns[c.next_turn as usize].to;
                c.next_turn = self.pick(g, to, false);
            } else {
                let t = c.seg - nl;
                c.s -= len;
                c.prev = c.seg;
                c.seg = g.turns[t as usize].to;
                release[0] = c.clearing;
                c.clearing = t;
                c.clear_left = CAR_LEN_M + 0.5 - c.s;
                c.held = NONE;
                c.stop = StopState::Approach;
            }
        }
        if c.clearing != NONE {
            c.clear_left -= ds;
            if c.clear_left <= 0.0 {
                release[1] = c.clearing;
                c.clearing = NONE;
            }
        }
        c.speed = v;
        let step = SHIFT_MPS * DT;
        let off = len2(c.offset);
        c.offset = if off <= step {
            [0.0; 2]
        } else {
            let k = (off - step) / off;
            [c.offset[0] * k, c.offset[1] * k]
        };
        let shoved = c.hold > 0
            || c.shove != [0.0; 2]
            || c.yaw != 0.0
            || c.slide != [0.0; 2]
            || c.spin != 0.0;
        if shoved {
            unshove(&mut c, ds);
        }
        if ds > 0.0 || c.offset != old.offset || shoved {
            self.place(g, &mut c);
            if self.bumps(i, &old, &c, ps) {
                self.cars[i] = Car {
                    speed: 0.0,
                    slide: [0.0; 2],
                    spin: 0.0,
                    hold: c.hold,
                    braking: brake_lamps(&old, 0.0),
                    ..old
                };
                return;
            }
        }
        for t in release {
            if t != NONE {
                self.claims[t as usize] -= 1;
            }
        }
        if c.offset == [0.0; 2] && c.ghost != NONE {
            c.ghost = NONE;
            self.ghosts -= 1;
        }
        c.braking = brake_lamps(&old, c.speed);
        self.cars[i] = c;
    }

    /// Whether car `i` moving from `old` to `new` would touch another car or the player:
    /// come into contact, or closer while already in contact.
    fn bumps(&self, i: usize, old: &Car, new: &Car, ps: &PlayerShape) -> bool {
        let (hl, hw) = (
            CAR_LEN_M * 0.5 + BODY_MARGIN_M,
            CAR_HALF_W_M + BODY_MARGIN_M,
        );
        let nb = body(new.pos, new.dir, hl, hw);
        let ob = body(old.pos, old.dir, hl, hw);
        let worse = |p: [f32; 3], b: &[[f32; 2]; 4]| {
            let d2 = |q: [f32; 3]| (q[0] - p[0]) * (q[0] - p[0]) + (q[2] - p[2]) * (q[2] - p[2]);
            touch(&nb, b) && (!touch(&ob, b) || d2(new.pos) < d2(old.pos))
        };
        let near =
            |p: [f32; 3], r: f32| (p[0] - new.pos[0]).abs() < r && (p[2] - new.pos[2]).abs() < r;
        let reach = CAR_LEN_M + 1.0;
        // Cars sorted by x at the start of the step have moved less than a meter since.
        let x = new.pos[0];
        let lo = self
            .by_x
            .partition_point(|&j| self.x_key[j as usize] < x - reach - 1.0);
        for &j in &self.by_x[lo..] {
            if self.x_key[j as usize] > x + reach + 1.0 {
                break;
            }
            let o = &self.cars[j as usize];
            if j as usize != i && near(o.pos, reach) {
                let b = body(o.pos, o.dir, CAR_LEN_M * 0.5, CAR_HALF_W_M);
                if worse(o.pos, &b) {
                    return true;
                }
            }
        }
        let pp = [ps.pts[1][0], 0.0, ps.pts[1][1]];
        if near(pp, reach * 0.5 + ps.half_len) {
            let b = body(pp, ps.dir, ps.half_len, ps.r);
            return worse(pp, &b);
        }
        false
    }

    /// Where car `i` could move to on lane `m`: its new `s`, when there is room.
    fn change_room(&self, g: &LaneGraph, inp: &Inputs, i: usize, m: u32) -> Option<f32> {
        let c = &self.cars[i];
        let lm = g.lanes[m as usize].length_m;
        let s2 = project(g, m, xz(g.pose(c.seg, c.s).pos));
        if s2 < CAR_LEN_M + 8.0 || lm - s2 < REPICK_M * 0.5 {
            return None;
        }
        let v = c.speed;
        for (j, o) in self.cars.iter().enumerate() {
            let gap_need = if j == i {
                continue;
            } else if o.seg == m || o.ghost == m {
                if o.s >= s2 {
                    (
                        o.s - CAR_LEN_M - s2,
                        3.0 + 0.6 * v + (v - o.speed).max(0.0) * 1.5,
                    )
                } else {
                    (
                        s2 - CAR_LEN_M - o.s,
                        3.0 + 0.6 * o.speed + (o.speed - v).max(0.0) * 2.0,
                    )
                }
            } else if o.seg >= self.lanes && g.turns[(o.seg - self.lanes) as usize].to == m {
                let rest = self.seg_len(g, o.seg) - o.s;
                (
                    s2 + rest - CAR_LEN_M,
                    3.0 + 0.6 * o.speed + (o.speed - v).max(0.0) * 2.0,
                )
            } else {
                continue;
            };
            if gap_need.0 < gap_need.1 {
                return None;
            }
        }
        // Not into the player: on the target lane from just behind to well ahead.
        let p = &inp.player;
        let q = [p.pos[0], p.pos[2]];
        if len2([q[0] - c.pos[0], q[1] - c.pos[2]]) < PASS_GAP_M + 20.0 {
            let sp = project(g, m, q);
            let lat = len2([q[0] - g.pose(m, sp).pos[0], q[1] - g.pose(m, sp).pos[2]]);
            let on_lane = lat < p.half_w + CAR_HALF_W_M + 0.8;
            if on_lane && sp > s2 - CAR_LEN_M - p.half_len - 3.0 && sp < s2 + 20.0 + 2.0 * v {
                return None;
            }
        }
        Some(s2)
    }

    /// Lane changes: each car considers one every [`CHANGE_EVERY`] ticks, at most one
    /// car changes per step.
    fn change_lanes(&mut self, g: &LaneGraph, inp: &Inputs) {
        let nl = self.lanes;
        for i in 0..self.cars.len() {
            let c = self.cars[i];
            if !(inp.tick + u64::from(c.id)).is_multiple_of(CHANGE_EVERY)
                || c.seg >= nl
                || c.held != NONE
                || c.clearing != NONE
            {
                continue;
            }
            let l = &g.lanes[c.seg as usize];
            if l.length_m - c.s < repick_m(c.speed) {
                continue;
            }
            let (lo, hi) = self.group(g, c.seg);
            if hi - lo < 2 {
                continue;
            }
            let straight = |m: u32| {
                let first = g.lanes[m as usize].first_turn;
                g.next(m)
                    .iter()
                    .position(|t| t.kind == TurnKind::Straight)
                    .map(|k| first + k as u32)
            };
            let want = if c.next_turn == NONE {
                c.seg
            } else {
                g.turns[c.next_turn as usize].from
            };
            let mut target = None;
            if want != c.seg {
                let m = if want < c.seg { c.seg - 1 } else { c.seg + 1 };
                target = self
                    .change_room(g, inp, i, m)
                    .map(|s2| (m, s2, c.next_turn));
            } else if c.next_turn != NONE
                && g.turns[c.next_turn as usize].kind == TurnKind::Straight
            {
                // Pass a slower car ahead on this lane.
                let v0 = c.v0 * l.speed_mps;
                let slower = |speed: f32| speed < 0.85 * v0 && speed + 2.0 < v0;
                let slow = self.cars.iter().any(|o| {
                    o.seg == c.seg
                        && o.s > c.s
                        && o.s - CAR_LEN_M - c.s < PASS_GAP_M
                        && slower(o.speed)
                }) || (self.plan[i].2 < PASS_GAP_M && slower(inp.player.speed));
                if slow {
                    for m in [c.seg.wrapping_sub(1), c.seg + 1] {
                        if m < lo || m >= hi {
                            continue;
                        }
                        if let (Some(t), Some(s2)) = (straight(m), self.change_room(g, inp, i, m)) {
                            target = Some((m, s2, t));
                            break;
                        }
                    }
                }
            }
            if let Some((m, s2, turn)) = target {
                let mut car = c;
                car.seg = m;
                car.s = s2;
                car.prev = NONE;
                car.next_turn = turn;
                car.stop = StopState::Approach;
                car.offset = [0.0; 2];
                self.place(g, &mut car);
                // The drawn car stays where it was and eases over.
                car.offset = [c.pos[0] - car.pos[0], c.pos[2] - car.pos[2]];
                car.pos = c.pos;
                if car.ghost == NONE {
                    self.ghosts += 1;
                }
                car.ghost = c.seg;
                self.cars[i] = car;
                return;
            }
        }
    }

    /// Updates car `c`'s drawn pose from its place on the graph.
    fn place(&self, g: &LaneGraph, c: &mut Car) {
        let front = self.seg_pose(g, c.seg, c.s);
        let rs = c.s - CAR_LEN_M;
        let rear = if rs >= 0.0 {
            self.seg_pose(g, c.seg, rs).pos
        } else if c.prev != NONE {
            let rp = self.seg_len(g, c.prev) + rs;
            let p = self.seg_pose(g, c.prev, rp.max(0.0));
            // Behind a segment shorter than the car, straight back from its start.
            let back = (-rp).max(0.0);
            [
                p.pos[0] - p.dir[0] * back,
                p.pos[1],
                p.pos[2] - p.dir[1] * back,
            ]
        } else {
            let f = front.pos;
            [
                f[0] - front.dir[0] * CAR_LEN_M,
                f[1],
                f[2] - front.dir[1] * CAR_LEN_M,
            ]
        };
        let f = front.pos;
        let mut d = unit2([f[0] - rear[0], f[2] - rear[2]]);
        if d == [0.0, 0.0] {
            d = front.dir;
        }
        c.pos = [
            (f[0] + rear[0]) * 0.5 + c.offset[0] + c.shove[0],
            (f[1] + rear[1]) * 0.5,
            (f[2] + rear[2]) * 0.5 + c.offset[1] + c.shove[1],
        ];
        if c.yaw != 0.0 {
            let (sn, cs) = sin_cos(c.yaw);
            c.heading = heading(d) + c.yaw;
            c.dir = [d[0] * cs + d[1] * sn, d[1] * cs - d[0] * sn];
        } else if d != c.dir {
            c.heading = heading(d);
            c.dir = d;
        }
    }

    /// One 120 Hz step: plans every car from the same snapshot, moves them, makes at most
    /// one lane change, and updates poses and lights. `graph` must be the one the traffic
    /// was built with.
    pub fn step(&mut self, graph: &LaneGraph, inp: &Inputs) {
        let g = graph;
        let ps = PlayerShape::new(&inp.player);
        self.link();
        for i in 0..self.cars.len() {
            self.plan[i] = self.plan_car(g, inp, &ps, i);
        }
        self.unlink();
        self.sort_x();
        for i in 0..self.cars.len() {
            self.move_car(g, i, &ps);
        }
        self.change_lanes(g, inp);
        for c in &mut self.cars {
            c.lights = inp.clock.dark(c.light_delay_s);
        }
    }

    /// Sorts the pool slots by x (insertion sort: the order barely changes per step).
    fn sort_x(&mut self) {
        let n = self.cars.len();
        for (k, c) in self.cars.iter().enumerate() {
            self.x_key[k] = c.pos[0];
        }
        if self.by_x.len() != n {
            self.by_x.clear();
            self.by_x.extend(0..n as u32);
        }
        for k in 1..n {
            let j = self.by_x[k];
            let x = self.x_key[j as usize];
            let mut m = k;
            while m > 0 && self.x_key[self.by_x[m - 1] as usize] > x {
                self.by_x[m] = self.by_x[m - 1];
                m -= 1;
            }
            self.by_x[m] = j;
        }
    }

    fn remove(&mut self, i: usize) {
        let c = self.cars.swap_remove(i);
        if c.ghost != NONE {
            self.ghosts -= 1;
        }
        for t in [c.held, c.clearing] {
            if t != NONE {
                self.claims[t as usize] -= 1;
            }
        }
    }

    /// Spawns and despawns by density, after [`Traffic::step`]. Despawns hidden cars
    /// beyond the ring, hidden for a while, or stuck; spawns at most one car per step on
    /// a lane in the ring, out of view, flowing toward the player. Neither happens inside
    /// the view.
    pub fn populate(&mut self, graph: &LaneGraph, inp: &Inputs) {
        let g = graph;
        let b = self.budget;
        let target = inp.density.target(b.cars);
        let pp = inp.player.pos;
        let mut excess = self.cars.len() > target;
        let hidden_ticks = (b.hidden_s * SIM_HZ as f32) as u32;
        let mut i = self.cars.len();
        while i > 0 {
            i -= 1;
            let c = &mut self.cars[i];
            let seen = inp.view.sees(c.pos, CAR_LEN_M);
            c.hidden = if seen { 0 } else { c.hidden.saturating_add(1) };
            c.stuck = if c.speed < STOPPED_MPS {
                c.stuck.saturating_add(1)
            } else {
                0
            };
            if seen {
                continue;
            }
            let d = len2([c.pos[0] - pp[0], c.pos[2] - pp[2]]);
            let gone = d > b.despawn_m
                || (c.hidden > hidden_ticks && d > b.spawn_min_m * 0.5)
                || c.stuck > STUCK_S * SIM_HZ
                || (excess && d > b.spawn_min_m);
            if gone {
                excess = excess && self.cars.len() - 1 > target;
                self.remove(i);
            }
        }
        if self.cars.len() >= target {
            return;
        }
        for _ in 0..SCAN_LANES.min(self.lanes as usize) {
            let lane = self.cursor;
            self.cursor = (self.cursor + 1) % self.lanes;
            let ball = self.lane_ball[lane as usize];
            let dc = len2([ball[0] - pp[0], ball[1] - pp[2]]);
            let l = &g.lanes[lane as usize];
            if dc + ball[2] < b.spawn_min_m
                || dc - ball[2] > b.spawn_max_m
                || l.length_m < 25.0
                || l.turn_count == 0
            {
                continue;
            }
            let s = self.rng.range_f32(12.0, l.length_m - 8.0);
            let p = g.pose(lane, s);
            let rel = [pp[0] - p.pos[0], pp[2] - p.pos[2]];
            let d = len2(rel);
            if d < b.spawn_min_m
                || d > b.spawn_max_m
                || dot(rel, p.dir) <= 0.0
                || inp.view.sees(p.pos, CAR_LEN_M + 2.0)
            {
                continue;
            }
            // Room on the lane, and a speed the car can stop from.
            let mut room = l.length_m - s;
            let mut clear = true;
            for o in &self.cars {
                // Nobody close on any lane (roads may cross without a junction).
                clear &= len2([o.pos[0] - p.pos[0], o.pos[2] - p.pos[2]]) >= SPAWN_CLEAR_M;
                if o.seg == lane || o.ghost == lane {
                    clear &= (o.s - s).abs() >= 25.0;
                    if o.s > s {
                        room = room.min(o.s - CAR_LEN_M - s);
                    }
                }
            }
            if !clear {
                continue;
            }
            let v0 = l.speed_mps * 0.8;
            let speed = v0.min(sqrt(2.0 * B_COMF * (room - 6.0).max(0.0)));
            self.spawn_at(g, lane, s, speed);
            return;
        }
    }

    /// The player's car hit car `i` (an index into [`Traffic::cars`]): its ground velocity
    /// changes by `dv` (`[x, z]`, m/s) and its yaw rate by `dspin` (rad/s). What is along
    /// its way (and keeps it at or above standstill) goes into its speed on its path, which
    /// the car ahead and the stop line still limit; the rest slides it off its path. Either
    /// way, knocked by [`HOLD_DV`] or more, it stands on the brakes for 1 to 2 s; then it
    /// drives on and steers back to its lane. It is never shoved further off its path than
    /// `room` (to its right and left, m, each at most [`SHOVE_SIDE_M`]) nor turned more
    /// than `room[2]` (rad, at most [`SHOVE_YAW`]): the caller measures what keeps its body
    /// on the roadway ([`FREE_ROOM`] where there is room for all of it).
    pub fn shove(&mut self, i: usize, dv: [f32; 2], dspin: f32, room: [f32; 3]) {
        let c = &mut self.cars[i];
        c.room = [
            room[0].clamp(0.0, SHOVE_SIDE_M),
            room[1].clamp(0.0, SHOVE_SIDE_M),
            room[2].clamp(0.0, SHOVE_YAW),
        ];
        let along = dot(dv, c.dir);
        let speed = (c.speed + along).max(0.0);
        let taken = speed - c.speed;
        c.speed = speed;
        c.slide = [
            c.slide[0] + dv[0] - c.dir[0] * taken,
            c.slide[1] + dv[1] - c.dir[1] * taken,
        ];
        c.spin += dspin;
        if len2(dv) >= HOLD_DV {
            c.hold = HOLD_S.0 + (c.id % 8) * (HOLD_S.1 - HOLD_S.0) / 8;
        }
    }

    /// A hash of every car, bit for bit, in pool order.
    pub fn hash(&self) -> u64 {
        let f = |x: f32| u64::from(x.to_bits());
        let mut h = 0x7A4F_F1C0u64;
        for c in &self.cars {
            let w = [
                u64::from(c.id) | u64::from(c.seg) << 32,
                f(c.s) | f(c.speed) << 32,
                u64::from(c.next_turn) | u64::from(c.held) << 32,
                f(c.pos[0]) | f(c.pos[1]) << 32,
                f(c.pos[2]) | f(c.heading) << 32,
                u64::from(c.shape) | u64::from(c.lights) << 8 | u64::from(c.paint) << 32,
                f(c.offset[0]) | f(c.offset[1]) << 32,
                c.stop as u64,
                u64::from(c.ghost),
            ];
            h = hash64(h, &w);
            // Only a car the player has hit has these, so traffic never hit hashes as
            // before they existed.
            if c.hold > 0 || c.shove != [0.0; 2] || c.yaw != 0.0 || c.slide != [0.0; 2] {
                let w = [
                    f(c.shove[0]) | f(c.shove[1]) << 32,
                    f(c.slide[0]) | f(c.slide[1]) << 32,
                    f(c.yaw) | f(c.spin) << 32,
                    u64::from(c.hold),
                    f(c.room[0]) | f(c.room[1]) << 32,
                    f(c.room[2]),
                ];
                h = hash64(h, &w);
            }
        }
        h
    }
}

/// One step of a shoved car `c` that drives `ds` on its path: the slide and the yaw rate
/// carry it on while they last, then, off the brakes and moving, it steers back toward
/// its lane in proportion to the distance driven.
fn unshove(c: &mut Car, ds: f32) {
    c.shove = [c.shove[0] + c.slide[0] * DT, c.shove[1] + c.slide[1] * DT];
    c.yaw += c.spin * DT;
    // Kept on the roadway: the path's direction is the drawn one less the yaw.
    let (sn, cs) = sin_cos(c.yaw);
    let path = [c.dir[0] * cs - c.dir[1] * sn, c.dir[1] * cs + c.dir[0] * sn];
    let side = right(path);
    let lat = dot(c.shove, side);
    let most = if lat > 0.0 { c.room[0] } else { c.room[1] };
    if lat.abs() > most {
        let over = lat - most * lat.signum();
        c.shove = [c.shove[0] - side[0] * over, c.shove[1] - side[1] * over];
        let out = dot(c.slide, side);
        if out * lat > 0.0 {
            c.slide = [c.slide[0] - side[0] * out, c.slide[1] - side[1] * out];
        }
    }
    if c.yaw.abs() > c.room[2] {
        c.yaw = c.room[2] * c.yaw.signum();
        if c.spin * c.yaw > 0.0 {
            c.spin = 0.0;
        }
    }
    let v = len2(c.slide);
    c.slide = if v <= SLIDE_DECEL * DT {
        [0.0; 2]
    } else {
        let k = (v - SLIDE_DECEL * DT) / v;
        [c.slide[0] * k, c.slide[1] * k]
    };
    c.spin = if c.spin.abs() <= SPIN_DECEL * DT {
        0.0
    } else {
        c.spin - SPIN_DECEL * DT * c.spin.signum()
    };
    if c.hold > 0 {
        // The hold runs once the car stands.
        if c.speed == 0.0 {
            c.hold -= 1;
        }
        return;
    }
    let off = len2(c.shove);
    let back = SHOVE_RETURN * ds;
    if off > back {
        let k = (off - back) / off;
        c.shove = [c.shove[0] * k, c.shove[1] * k];
        c.yaw *= k;
        return;
    }
    c.shove = [0.0; 2];
    let turn = YAW_RETURN * ds;
    c.yaw = if c.yaw.abs() <= turn {
        0.0
    } else {
        c.yaw - turn * c.yaw.signum()
    };
}

/// Whether a car that was `old` and now goes `speed` shows its brake lamps.
fn brake_lamps(old: &Car, speed: f32) -> bool {
    let dv = speed - old.speed;
    let held = dv < -BRAKE_LAMP_HOLD * DT || (speed < CREEP_MPS && dv <= PULL_AWAY * DT);
    let standing = speed <= STANDING_MPS && dv <= PULL_AWAY * DT;
    dv < -BRAKE_LAMP_DECEL * DT || standing || (old.braking && held)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idm_brakes_for_a_close_obstacle_and_cruises_on_a_free_road() {
        assert!((free_accel(0.0, 15.0) - A_MAX).abs() < 1e-6);
        assert!(free_accel(15.0, 15.0).abs() < 1e-6);
        assert!(free_accel(20.0, 10.0) < 0.0);
        assert!(idm(15.0, 15.0, 1.4, 5.0, 0.0, S0) < -B_COMF);
        assert!(idm(15.0, 15.0, 1.4, 200.0, 0.0, S0) > -0.1);
        assert_eq!(idm(5.0, 15.0, 1.4, 0.0, 0.0, S0), -B_MAX);
    }

    #[test]
    fn view_cone_sees_ahead_not_behind() {
        let v = View {
            pos: [0.0; 3],
            dir: [0.0, -1.0],
            tan_half: 1.0,
        };
        assert!(v.sees([0.0, 0.0, -100.0], 0.0));
        assert!(v.sees([90.0, 0.0, -100.0], 0.0));
        assert!(!v.sees([120.0, 0.0, -100.0], 0.0));
        assert!(v.sees([120.0, 0.0, -100.0], 20.0));
        assert!(!v.sees([0.0, 0.0, 100.0], 3.0));
        assert!(v.sees([0.0, 0.0, 1.0], 3.0), "next to the camera");
    }

    #[test]
    fn lights_follow_dusk_and_dawn_across_midnight() {
        let at = |t: f32| Clock {
            seconds_of_day: t,
            dusk_s: 19.0 * 3600.0,
            dawn_s: 6.0 * 3600.0,
        };
        assert!(!at(12.0 * 3600.0).dark(0.0));
        assert!(at(19.5 * 3600.0).dark(0.0));
        assert!(!at(19.1 * 3600.0).dark(900.0), "this driver is slower");
        assert!(at(2.0 * 3600.0).dark(900.0));
        assert!(at(6.1 * 3600.0).dark(900.0), "and keeps them on past dawn");
        assert!(!at(7.0 * 3600.0).dark(900.0));
    }

    #[test]
    fn crossing_segments_have_no_distance() {
        let d = seg_dist([0.0, -1.0], [0.0, 1.0], [-1.0, 0.0], [1.0, 0.0]);
        assert_eq!(d, 0.0);
        let d = seg_dist([0.0, 0.0], [1.0, 0.0], [0.0, 2.0], [1.0, 2.0]);
        assert!((d - 2.0).abs() < 1e-6);
    }
}
