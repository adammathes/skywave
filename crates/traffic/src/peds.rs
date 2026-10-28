//! Pedestrians: people walking the sidewalk graph ([`crate::walks`]) around the player.
//!
//! **Walkers and people.** A walker is one person or a group of two or three that keeps
//! together: it holds the place on the graph (a link, a distance along it, a direction),
//! a speed (the slowest member's own pace, 1.2–1.6 m/s) and what it is doing; its members
//! walk abreast across the walk, each easing toward their slot so link changes and stops
//! never jump. A walker heads for a spot (a shop window, a door, a bench, a stop, the
//! beach, a parking entrance, a landmark), choosing at every fork the way whose far end
//! lies nearest it, with a little randomness; it gives up and picks another after a while.
//! At the spot the group stays a while: looking into a window, standing in a ring, sitting
//! on the bench.
//!
//! **Crossings.** Walkers wait at the curb before a crossing: on a signal until the walk
//! phase ([`WalkGraph::walk`]: the road's own approach red long enough to get across),
//! elsewhere for a look both ways. Either way they wait while the player's car comes on
//! toward the crossing. Once on the road they never stop on it.
//!
//! **The car** (no harm, by construction: nobody is ever collidable), moving or standing
//! still. Its danger zone is its footprint plus a margin and, while it moves, its path
//! ahead for a distance that grows with its speed, around the curve it is turning on. A
//! person in it steps to the nearest spot on the walk geometry around them clear of the
//! car's footprint and whole path ahead, without passing into the car or across its path
//! just in front of it (off the curb only when the car is on the walk itself); with no
//! clear spot, to the least bad one, never standing still in its path. The move is the
//! figure's step aside or a hop, a startled jump when the car is close, and changes course
//! if the car turns into its spot. A walker on a crossing, or with nowhere to step,
//! hurries off along its path away from the car when that leads out of its way, so nobody
//! freezes in the road. Walkers stop short of the car and do not walk toward its path;
//! held by a stopped car, they turn back. Nobody walks into the zone; when the car has
//! gone, people return to their slots and carry on.
//!
//! **Spawning** keeps the density target (per zone, scaled by [`Density`]) within 120 m of
//! the eye: new walkers appear 40–120 m away and out of view, on a walk or already at a
//! spot (sitting, standing about); they leave beyond 150 m, or out of view when there are
//! too many. Everything is drawn from one seeded PCG stream in a fixed order, so the same
//! seed and inputs give bit-identical people.
//!
//! **Output** per person: position, heading (the vehicle sim's convention), figure seed and
//! [`PedPose`] (gait phase and speed, the action and its blend weight), which
//! [`PedPose::joints`] turns into a [`Joints`] for the figure's skeleton.
//!
//! Fixed pools from a [`PedestrianBudget`], allocated once; stepping allocates nothing.

use alloc::vec::Vec;

use skywave_core::{hash64, Rng, Seed, SIM_HZ};

use crate::cars::{Density, Player, View};
use crate::figure::{
    aside_offset, hop_offset, look_yaw, phase_rate, sit_reach, Era, FigureSpec, Joints, Skeleton,
    ASIDE_M, HOP_M,
};
use crate::lanes::{heading, LaneGraph, NONE};
use crate::math::{dot, len2, right, sin_cos, unit2};
use crate::walks::{LinkKind, SpotKind, WalkGraph, CELL_M};

const DT: f32 = 1.0 / SIM_HZ as f32;
const TAU: f32 = core::f32::consts::TAU;
const PI: f32 = core::f32::consts::PI;
/// Hash key of the pedestrian stream.
const KEY_PEDS: u64 = 0x5045_4453_494d_0001;

/// People spawn this far from the eye, despawn beyond `DESPAWN_M`, and count toward the
/// density target within `TARGET_M`.
pub const SPAWN_NEAR_M: f32 = 40.0;
pub const SPAWN_FAR_M: f32 = 120.0;
pub const DESPAWN_M: f32 = 150.0;
const TARGET_M: f32 = 120.0;
/// Radius around a person the view check covers, m; a spawn point's covers the slots
/// its group fans out to.
const VIEW_R_M: f32 = 1.5;
const SPAWN_VIEW_R_M: f32 = 3.0;
/// Ticks between spawn attempts and between density target updates.
const SPAWN_TICKS: u64 = 6;
const TARGET_TICKS: u64 = 60;
/// Share of spawns placed straight at a spot.
const SPAWN_AT_SPOT: f32 = 0.3;
/// Walking pace range, the hurry off a crossing, and acceleration.
const PACE_MIN: f32 = 1.2;
const PACE_MAX: f32 = 1.6;
const HURRY: f32 = 2.2;
const ACCEL: f32 = 2.0;
/// How fast a person closes on their slot beyond the walker's own speed.
const CATCH_UP: f32 = 0.7;
/// Turning rate, rad/s, and while getting out of the way.
const TURN_RATE: f32 = 3.5;
const TURN_RATE_DODGE: f32 = 10.0;
/// Spacing of people walking abreast and of a group standing about.
const ABREAST_M: f32 = 0.65;
/// Spacing of a group in single file on a spur and queued at the curb.
const QUEUE_M: f32 = 0.6;
/// Walking groups keep within this of the walk line.
const WALK_SPREAD_M: f32 = 0.75;
const RING_M: f32 = 0.55;
/// A walker gives up on a spot after this many seconds.
const GIVE_UP_S: f32 = 120.0;
/// Step aside and hop durations, s.
const ASIDE_S: f32 = 0.6;
const HOP_S: f32 = 0.5;
/// Hop rather than step aside when the car will be there sooner than this, s.
const HOP_TTI_S: f32 = 0.8;
/// Danger ahead of the car's front: a base plus seconds of travel; beside its path: half
/// its width plus a margin.
const DANGER_BASE_M: f32 = 4.0;
const DANGER_S: f32 = 2.0;
const DANGER_SIDE_M: f32 = 0.7;
/// The way of a car faster than walking widens ahead of its front by up to this much per
/// meter (all of it from twice this speed, m/s), up to this much (a turn it may start
/// before it gets there).
const CONE: f32 = 0.2;
const CONE_MAX_M: f32 = 1.0;
const CONE_FROM_MPS: f32 = 3.0;
/// Walkers stop short of this from the car's footprint, moving or not.
const CLEAR_M: f32 = 0.5;
/// A car going faster than this is moving: closing on anyone ahead of it, however slowly.
/// Up to [`CREEP_MPS`] it is creeping (walkers held by it turn back sooner).
const MOVING_MPS: f32 = 0.05;
const CREEP_MPS: f32 = 0.3;
/// Below this the car is standing dead still.
const STOPPED_MPS: f32 = 0.01;
/// One dodge episode (moves back to back) carries nobody farther than this from where it
/// began, unless the car is still coming at them.
const EPISODE_M: f32 = 3.0;
/// Below this curvature (1/m) the car's path is taken as straight.
const MIN_CURVATURE: f32 = 0.01;
/// A person steps aside to a clear spot no farther than this, sampled this finely, and
/// covers ground at least this fast on the way (a longer move takes longer).
const DODGE_REACH_M: f32 = 2.0;
const DODGE_STEP_M: f32 = 0.25;
const DODGE_MPS: f32 = 2.5;
/// Nobody moves faster than this, m/s; the steepest slopes of the step-aside curve and of
/// the hop curve (share of the move per share of its time).
const MAX_MPS: f32 = 4.0;
const ASIDE_SLOPE: f32 = 1.5;
const HOP_SLOPE: f32 = 3.75;
/// A move is checked against where a moving car will be (on the circle it is turning on)
/// every this many seconds over this many steps: the person keeps this much clear of its
/// footprint all the way (a body's half width and a margin; the body swings on a turn).
const FUTURE_DT_S: f32 = 0.05;
const FUTURE_N: usize = 50;
const FUTURE_CLEAR_M: f32 = 0.25;
/// A move under way changes course, if its spot is no longer clear, after this long, s.
const REPLAN_S: f32 = 0.25;
/// With the car closer than this (s), the hop is a startled jump: under way at once and
/// quicker.
const URGENT_TTI_S: f32 = 0.8;
const URGENT_HOP_S: f32 = 0.3;
/// On the road a walker waits for anyone this far from their place in the group.
const STRANDED_M: f32 = 1.0;
/// Stepping across a moving car's path needs this long to spare before it arrives, s.
const CROSS_SPARE_S: f32 = 0.8;
/// How far off the curb into the gutter a person may step when a car on the walk itself
/// covers all of it: within the 0.45 m the lane graph leaves outside the lanes.
const ROAD_DODGE_M: f32 = 0.4;
/// How far past its room either side of a corner a person may step when a car on the
/// corner covers all of it (still on the sidewalk).
const CORNER_DODGE_M: f32 = 0.3;
/// And past the back of the walk, with nowhere clear on it.
const BACK_DODGE_M: f32 = 0.3;
/// Moving off along a path escapes a car only if the way away from it is at least this
/// much along the path (cosine).
const ALONG_MIN: f32 = 0.3;
/// A person within this of the footprint (or in a moving car's way) gets out of it, to a
/// spot at least this far off; nobody walks into it.
const SPOT_CLEAR_M: f32 = 0.4;
/// Beside a car standing still or creeping, a spot is at least this far off: nobody waits
/// right up against it.
const STILL_CLEAR_M: f32 = 0.6;
/// Watched zone (walkers do not walk toward the car's path in it): farther ahead and wider.
const WATCH_BASE_M: f32 = 10.0;
const WATCH_S: f32 = 2.5;
const WATCH_SIDE_M: f32 = 4.0;
/// Back to walking once nothing has threatened for this long, s.
const CALM_S: f32 = 0.6;
/// After holding this long for a stopped car, a walker turns back, s.
const HOLD_TURN_S: f32 = 4.0;
/// With the car standing still this long on a crossing, the walkers by it there walk off
/// it (on across, or back to the curb), s.
const STILL_CLEAR_S: f32 = 0.25;
/// Sitting down and getting up, s.
const SIT_S: f32 = 1.2;
const STAND_S: f32 = 1.0;
/// Seats along a bench from its middle (to the sitter's left).
const BENCH_SEATS: [f32; 2] = [-0.45, 0.45];
/// Blend weight rates, per second.
const BLEND_IN: f32 = 8.0;
const BLEND_OUT: f32 = 4.0;

/// Pool sizes, fixed at construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PedestrianBudget {
    /// Most people alive at once (and most walkers).
    pub pedestrians: usize,
}

impl PedestrianBudget {
    /// Web and Deck.
    pub const SMALL: PedestrianBudget = PedestrianBudget { pedestrians: 128 };
    /// Desktop.
    pub const LARGE: PedestrianBudget = PedestrianBudget { pedestrians: 256 };
}

/// Inputs of one step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PedInput {
    /// The signal clock ([`crate::SignalClock`]): crossings follow the lights.
    pub tick: u64,
    pub density: Density,
    /// Nobody spawns or despawns inside it.
    pub view: View,
    /// The player's car ([`Player::ABSENT`] for none): an obstacle at any speed.
    pub player: Player,
}

/// Multiplier on the zones' people per 100 m of walk for a density setting.
pub fn density_scale(d: Density) -> f32 {
    match d {
        Density::Off => 0.0,
        Density::Low => 0.35,
        Density::Normal => 1.0,
        Density::Busy => 2.0,
    }
}

/// What a person's pose is blending toward from the gait.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None = 0,
    /// [`Skeleton::step_aside`].
    Aside = 1,
    /// [`Skeleton::hop`].
    Hop = 2,
    /// [`Skeleton::sit`]: the person's position is the seat point while it lasts.
    Sit = 3,
}

/// Everything needed to pose a person's figure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PedPose {
    /// Gait phase (cycles) and speed for [`Skeleton::walk`].
    pub phase: f32,
    pub speed: f32,
    pub action: Action,
    /// The action's progress, 0–1.
    pub t: f32,
    /// Step aside or hop toward the figure's right (1) or left (-1).
    pub side: f32,
    /// [`look_yaw`] toward the car while stepping aside.
    pub look: f32,
    /// Blend from the gait toward the action, 0–1.
    pub weight: f32,
}

impl PedPose {
    const REST: PedPose = PedPose {
        phase: 0.0,
        speed: 0.0,
        action: Action::None,
        t: 0.0,
        side: 1.0,
        look: 0.0,
        weight: 0.0,
    };

    /// Blend weights of the gait, step aside, hop and sit; they sum to 1.
    pub fn weights(&self) -> [f32; 4] {
        let mut w = [1.0 - self.weight, 0.0, 0.0, 0.0];
        if self.action != Action::None {
            w[self.action as usize] = self.weight;
        } else {
            w[0] = 1.0;
        }
        w
    }

    /// The joints of this pose on `sk` (the gait blended toward the action).
    pub fn joints(&self, sk: &Skeleton) -> Joints {
        let walk = sk.walk(self.phase, self.speed);
        let action = match self.action {
            Action::None => return walk,
            Action::Aside => sk.step_aside(self.t, self.side, self.look),
            Action::Hop => sk.hop(self.t, self.side),
            Action::Sit => sk.sit(self.t),
        };
        Joints::blend(&walk, &action, self.weight)
    }
}

/// One person.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pedestrian {
    pub alive: bool,
    /// Counts up with every spawn: a new person is a new id.
    pub id: u32,
    /// Figure seed ([`crate::figure::figure`] with [`Pedestrians::era`]).
    pub seed: u64,
    /// Figure origin on the ground (the seat point while sitting).
    pub pos: [f32; 3],
    /// Vehicle sim convention: 0 faces -z, positive turns left.
    pub heading: f32,
    pub pose: PedPose,
    walker: u16,
    slot: u8,
    height: f32,
    /// Where the person is, before any sidestep.
    base: [f32; 3],
    dodge: Dodge,
    sit: Sit,
}

impl Pedestrian {
    /// Standing height, m: the figure's ([`FigureSpec::height_m`] of [`Self::seed`], clamped
    /// to 1.4–2.1 m), which the gait is paced for; draw the figure scaled to it.
    pub fn height_m(&self) -> f32 {
        self.height
    }
}

/// Getting out of the car's way: one move (a step aside or a hop) of the person's ground
/// offset from where they were, `from` to `to`, at `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Dodge {
    action: Action,
    t: f32,
    /// Seconds the move takes: longer moves take longer.
    dur: f32,
    from: [f32; 2],
    to: [f32; 2],
    /// Seconds since anything last threatened.
    calm: f32,
    /// A startled move: under way at once (no crouch first).
    quick: bool,
    /// No safe spot across the walk: the walker should move off along it.
    stuck: bool,
    /// The offset where the episode's first move began.
    origin: [f32; 2],
}

impl Dodge {
    const NONE: Dodge = Dodge {
        action: Action::None,
        t: 0.0,
        dur: HOP_S,
        from: [0.0; 2],
        to: [0.0; 2],
        calm: 0.0,
        stuck: false,
        quick: false,
        origin: [0.0; 2],
    };

    /// The offset now: the figure's own sidestep or hop curve, scaled to the move.
    fn offset(&self) -> [f32; 2] {
        if self.action == Action::None {
            return self.to;
        }
        let k = move_frac(self.action, self.quick, self.t);
        [
            self.from[0] + (self.to[0] - self.from[0]) * k,
            self.from[1] + (self.to[1] - self.from[1]) * k,
        ]
    }
}

/// How a move of `d` meters goes with the car `tti` seconds off: a step aside or a hop,
/// whether it is a startled one (under way at once), and how long it takes (its curve's
/// steepest part at [`MAX_MPS`] at most).
fn move_kind(tti: f32, d: f32) -> (Action, bool, f32) {
    let aside = tti >= HOP_TTI_S && d <= ASIDE_M + 0.2;
    let quick = !aside && (tti < URGENT_TTI_S || d > HOP_M);
    let dur = if aside {
        ASIDE_S.max(ASIDE_SLOPE * d / MAX_MPS)
    } else if quick {
        URGENT_HOP_S.max(ASIDE_SLOPE * d / MAX_MPS)
    } else {
        HOP_S.max(HOP_SLOPE * d / MAX_MPS)
    };
    (if aside { Action::Aside } else { Action::Hop }, quick, dur)
}

/// Share of a move made at `t` (0–1 of its time).
fn move_frac(action: Action, quick: bool, t: f32) -> f32 {
    if action == Action::Hop && !quick {
        hop_offset(t) / HOP_M
    } else {
        aside_offset(t) / ASIDE_M
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sit {
    No,
    /// Sitting down (`up` false) or getting up, at `t`, on `seat`.
    On {
        seat: [f32; 3],
        t: f32,
        up: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Doing {
    Walk,
    /// At the curb of a crossing since `since` (our tick), looking `look` ticks at least.
    Wait {
        crossing: u32,
        since: u64,
        look: u64,
    },
    /// On a crossing's links.
    Cross,
    /// Hurrying along the path away from the car (`cross`: it was crossing), until calm
    /// for a while.
    Escape {
        cross: bool,
        calm: f32,
    },
    /// Waiting for the car to pass or move, since `since`.
    Hold {
        since: u64,
    },
    /// At its target spot until `until`.
    Stay {
        until: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Walker {
    alive: bool,
    members: [u16; 3],
    count: u8,
    link: u32,
    /// Travelling from the link's `a` to `b`.
    fwd: bool,
    /// Distance from the end it entered by (may run a little negative after a turn).
    s: f32,
    v: f32,
    pace: f32,
    /// Group center off the line, to the left of travel, and where it settles.
    lat: f32,
    lane: f32,
    doing: Doing,
    /// Target spot ([`NONE`] when wandering to `goal`), its slot there, the goal point.
    spot: u32,
    spot_slot: u8,
    goal: [f32; 2],
    give_up: u64,
    /// Walking off a crossing the car stands still on: everyone follows, along it.
    clearing: bool,
}

impl Walker {
    const DEAD: Walker = Walker {
        alive: false,
        members: [0; 3],
        count: 0,
        link: 0,
        fwd: true,
        s: 0.0,
        v: 0.0,
        pace: 1.4,
        lat: 0.0,
        lane: 0.0,
        doing: Doing::Walk,
        spot: NONE,
        spot_slot: 0,
        goal: [0.0; 2],
        give_up: 0,
        clearing: false,
    };
}

/// The car as seen by pedestrians.
#[derive(Clone, Copy)]
struct CarFrame {
    c: [f32; 2],
    /// Forward along its motion, and its right.
    f: [f32; 2],
    r: [f32; 2],
    hw: f32,
    hl: f32,
    v: f32,
    /// Path curvature, 1/m, positive turning left (0 going straight).
    k: f32,
}

impl CarFrame {
    /// The car `car`, turning at `yaw_rate` rad/s (positive left).
    fn new(car: &Player, yaw_rate: f32) -> CarFrame {
        let (sn, cs) = sin_cos(car.heading);
        let mut f = [-sn, -cs];
        if car.speed < 0.0 {
            f = [-f[0], -f[1]];
        }
        CarFrame {
            c: [car.pos[0], car.pos[2]],
            f,
            r: right(f),
            hw: car.half_w,
            hl: car.half_len,
            v: car.speed.abs(),
            k: if car.speed.abs() > 0.3 {
                (yaw_rate / car.speed.abs()).clamp(-0.5, 0.5)
            } else {
                0.0
            },
        }
    }

    /// `(ahead, lat)` of ground point `p` along the path the car is driving: around the
    /// circle it is turning on (arc length from the car's middle, offset outward or inward
    /// signed to its right at the car), or straight on.
    fn path(&self, p: [f32; 3]) -> (f32, f32) {
        if self.k.abs() < MIN_CURVATURE {
            return self.rel(p);
        }
        let r = 1.0 / self.k.abs();
        let toward = if self.k > 0.0 {
            [-self.r[0], -self.r[1]]
        } else {
            self.r
        };
        let o = [self.c[0] + toward[0] * r, self.c[1] + toward[1] * r];
        let u0 = [-toward[0], -toward[1]];
        let v = [p[0] - o[0], p[2] - o[1]];
        let d = len2(v);
        if d < 1e-3 {
            return (0.0, -r * dot(self.r, u0));
        }
        let w = [v[0] / d, v[1] / d];
        let turn = if u0[0] * self.f[1] - u0[1] * self.f[0] >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let ang = crate::math::atan2(u0[0] * w[1] - u0[1] * w[0], dot(u0, w)) * turn;
        (ang * r, (d - r) * dot(self.r, u0))
    }

    /// Half the width the body sweeps along its path: on a curve its corners swing out
    /// past its half width by about `hl² k / 2`.
    fn sweep_hw(&self) -> f32 {
        self.hw + 0.5 * self.hl * self.hl * self.k.abs()
    }

    /// Ground direction across the path at `p`, to its right.
    fn path_right(&self, p: [f32; 3]) -> [f32; 2] {
        if self.k.abs() < MIN_CURVATURE {
            return self.r;
        }
        let r = 1.0 / self.k.abs();
        let toward = if self.k > 0.0 {
            [-self.r[0], -self.r[1]]
        } else {
            self.r
        };
        let o = [self.c[0] + toward[0] * r, self.c[1] + toward[1] * r];
        let w = unit2([p[0] - o[0], p[2] - o[1]]);
        let s = dot(self.r, [-toward[0], -toward[1]]);
        [w[0] * s, w[1] * s]
    }

    /// `(ahead, lat)` of ground point `p`: along the motion from the center, and to the
    /// right of it.
    fn rel(&self, p: [f32; 3]) -> (f32, f32) {
        let d = [p[0] - self.c[0], p[2] - self.c[1]];
        (dot(d, self.f), dot(d, self.r))
    }

    fn inside(&self, p: [f32; 3], margin: f32) -> bool {
        let (a, l) = self.rel(p);
        a.abs() <= self.hl + margin && l.abs() <= self.hw + margin
    }

    /// Whether a spot at `p` is in the car's way for good: within `margin` of its
    /// footprint, or of its path ahead however far (a spot farther along the path is no
    /// way out of it).
    fn blocks(&self, p: [f32; 3], margin: f32) -> bool {
        let (a, l) = self.rel(p);
        let near = a.abs() <= self.hl + margin && l.abs() <= self.hw + margin;
        let (pa, pl) = self.path(p);
        near || (self.moving() && pa > -self.hl - 0.3 && pl.abs() < self.side(pa) + margin)
    }

    /// Half the width of the car's way `pa` along its path: what the body sweeps, and
    /// ahead of a fast car's front a cone for a turn it may yet start into someone.
    fn side(&self, pa: f32) -> f32 {
        let fast = ((self.v - CONE_FROM_MPS) / CONE_FROM_MPS).clamp(0.0, 1.0);
        self.sweep_hw() + (fast * CONE * (pa - self.hl)).clamp(0.0, CONE_MAX_M)
    }

    /// How far `p` is out of the car's way (negative inside): out of its path beside it
    /// while it moves toward `p`, else out of its footprint, roughly.
    fn gap(&self, p: [f32; 3]) -> f32 {
        let (pa, pl) = self.path(p);
        let reach = self.hl + DANGER_BASE_M + DANGER_S * self.v;
        if self.moving() && pa > -self.hl - 0.3 && pa < reach {
            pl.abs() - self.sweep_hw()
        } else {
            self.out_of(p)
        }
    }

    /// Whether a person may move straight from `p` to `q` out of the car's way: deeper
    /// into a moving car's way only where they are past with `spare` seconds left before
    /// it gets there (never just in front of it; `None`: any way out will do), and never
    /// into its footprint (from inside it, only ever farther out).
    fn reachable(&self, p: [f32; 3], q: [f32; 3], spare: Option<f32>) -> bool {
        // Wherever the way there goes deeper into the car's way than `p` (toward the
        // middle of its path or past it; on a curve even a spot on one's own side can lie
        // past the middle in a straight line), be past it before the car gets there.
        let (ap, lp) = self.path(p);
        let d = dist(p, q);
        if let Some(spare) = spare.filter(|_| self.moving()) {
            // From inside its way, out of it before the car gets here.
            let way = |a: f32, l: f32| a > -self.hl && l.abs() < self.side(a) + SPOT_CLEAR_M;
            if way(ap, lp) {
                let out = (1..=8).find(|&k| {
                    let (a, l) = self.path(crate::math::lerp3(p, q, k as f32 / 8.0));
                    !way(a, l)
                });
                let gone = out.map_or(d, |k| k as f32 / 8.0 * d) / DODGE_MPS;
                if (ap - self.hl) / self.v < gone {
                    return false;
                }
            }
            for k in 1..=8 {
                let f = k as f32 / 8.0;
                let (a, l) = self.path(crate::math::lerp3(p, q, f));
                let deeper = l * lp < 0.0 || l.abs() < lp.abs() - 0.05;
                let in_way = a > -self.hl && l.abs() < self.side(a) + SPOT_CLEAR_M;
                let arrive = (a - self.hl) / self.v;
                if deeper && in_way && arrive < f * d / DODGE_MPS + spare {
                    return false;
                }
            }
        }
        let mut prev = self.out_of(p);
        (1..=4).all(|k| {
            let g = self.out_of(crate::math::lerp3(p, q, k as f32 / 4.0));
            let ok = g > 0.1 || g >= prev - 1e-3;
            prev = g;
            ok
        })
    }

    /// Whether someone at `p` going on along `dir` at `v` comes within [`FUTURE_CLEAR_M`]
    /// of where a turning car's body will be over the next second (closer than now).
    fn swings_into(&self, p: [f32; 3], dir: [f32; 2], v: f32) -> bool {
        if !self.moving() || self.k.abs() < MIN_CURVATURE {
            return false;
        }
        let now = self.out_of(p);
        (1..=10).any(|j| {
            let tau = j as f32 * 0.1;
            let q = [p[0] + dir[0] * v * tau, p[1], p[2] + dir[1] * v * tau];
            let g = self.ahead(tau).out_of(q);
            g < FUTURE_CLEAR_M && g < now
        })
    }

    /// The car `tau` seconds on along the circle it is turning on (or straight on).
    fn ahead(&self, tau: f32) -> CarFrame {
        let s = self.v * tau;
        if self.k.abs() < MIN_CURVATURE {
            return CarFrame {
                c: [self.c[0] + self.f[0] * s, self.c[1] + self.f[1] * s],
                ..*self
            };
        }
        let r = 1.0 / self.k.abs();
        // Turning left (k > 0) about a center to the left of the motion.
        let sign = if self.k > 0.0 { 1.0 } else { -1.0 };
        let toward = [-self.r[0] * sign, -self.r[1] * sign];
        let o = [self.c[0] + toward[0] * r, self.c[1] + toward[1] * r];
        let (sn, cs) = sin_cos(s / r);
        let turn = |v: [f32; 2]| {
            let l = left(v);
            [v[0] * cs + l[0] * sn * sign, v[1] * cs + l[1] * sn * sign]
        };
        let rel = turn([self.c[0] - o[0], self.c[1] - o[1]]);
        let f = turn(self.f);
        CarFrame {
            c: [o[0] + rel[0], o[1] + rel[1]],
            f,
            r: right(f),
            ..*self
        }
    }

    /// Ground direction out of the footprint at `p`, across its nearest side.
    fn normal(&self, p: [f32; 3]) -> [f32; 2] {
        let (a, l) = self.rel(p);
        let (d, s) = if a.abs() - self.hl > l.abs() - self.hw {
            (self.f, a)
        } else {
            (self.r, l)
        };
        if s < 0.0 {
            [-d[0], -d[1]]
        } else {
            d
        }
    }

    /// How far `p` is outside the footprint (negative inside), as the larger of the two
    /// axes.
    fn out_of(&self, p: [f32; 3]) -> f32 {
        let (a, l) = self.rel(p);
        (a.abs() - self.hl).max(l.abs() - self.hw)
    }

    /// How clear of the car's way `p` is: past its footprint and, while it moves, past
    /// the side of its path ahead (negative in it).
    fn clearance(&self, p: [f32; 3]) -> f32 {
        let (pa, pl) = self.path(p);
        if self.moving() && pa > -self.hl - 0.3 {
            pl.abs() - self.side(pa)
        } else {
            self.out_of(p)
        }
    }

    /// Whether the footprint overlaps walk link `l`'s band (the car is on that walk).
    fn on_walk(&self, walks: &WalkGraph, l: u32) -> bool {
        let link = &walks.links[l as usize];
        let a = walks.nodes[link.a as usize].pos;
        let (f, r) = (self.f, self.r);
        let reach = link.room[0].max(link.room[1]) + 0.25;
        [
            (0.0, 0.0),
            (1.0, 1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (-1.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
        ]
        .iter()
        .any(|&(ka, kl)| {
            let x = self.c[0] + f[0] * ka * self.hl + r[0] * kl * self.hw;
            let z = self.c[1] + f[1] * ka * self.hl + r[1] * kl * self.hw;
            let rel = [x - a[0], z - a[2]];
            let s = dot(rel, link.dir);
            s > -0.5 && s < link.length_m + 0.5 && dot(rel, left(link.dir)).abs() < reach
        })
    }

    fn moving(&self) -> bool {
        self.v > MOVING_MPS
    }

    /// How far clear of the car's way a spot to step to is.
    fn spot_clear(&self) -> f32 {
        if self.v > CREEP_MPS {
            SPOT_CLEAR_M
        } else {
            STILL_CLEAR_M
        }
    }

    /// Whether the car is still coming at `p`: moving, with `p` in its way ahead or close
    /// beside it (where its body swings on a turn).
    fn coming(&self, p: [f32; 3]) -> bool {
        let (pa, pl) = self.path(p);
        let reach = self.hl + DANGER_BASE_M + DANGER_S * self.v;
        let ahead = pa > -self.hl && pa < reach && pl.abs() < self.side(pa) + SPOT_CLEAR_M;
        self.moving() && (ahead || self.out_of(p) < DANGER_SIDE_M)
    }

    /// Too close: within [`DANGER_SIDE_M`] of the car's footprint at any speed, or of its
    /// path ahead while it moves. Returns `(ahead, lat)`.
    fn danger(&self, p: [f32; 3]) -> Option<(f32, f32)> {
        self.danger_within(p, DANGER_SIDE_M)
    }

    /// [`Self::danger`] with `margin` around the footprint and beside its path.
    fn danger_within(&self, p: [f32; 3], margin: f32) -> Option<(f32, f32)> {
        let (a, l) = self.rel(p);
        let (pa, pl) = self.path(p);
        let reach = self.hl + DANGER_BASE_M + DANGER_S * self.v;
        let near = a.abs() <= self.hl + margin && l.abs() <= self.hw + margin;
        let ahead =
            self.moving() && pa > -self.hl - 0.3 && pa < reach && pl.abs() < self.side(pa) + margin;
        (near || ahead).then_some((pa.max(a), pl))
    }

    /// In the zone near the moving car's way where walkers do not walk toward it.
    fn watch(&self, p: [f32; 3]) -> bool {
        let (a, l) = self.path(p);
        let reach = self.hl + WATCH_BASE_M + WATCH_S * self.v;
        self.moving() && a > -self.hl - 1.0 && a < reach && l.abs() < self.hw + WATCH_SIDE_M
    }

    /// Ground direction away from the car's path at `p` (from its middle when it is not
    /// moving).
    fn away(&self, p: [f32; 3], fallback: [f32; 2]) -> [f32; 2] {
        if !self.moving() {
            let d = unit2([p[0] - self.c[0], p[2] - self.c[1]]);
            return if d == [0.0; 2] { fallback } else { d };
        }
        let (_, l) = self.path(p);
        let r = self.path_right(p);
        if l.abs() < 0.05 {
            let s = if dot(fallback, r) >= 0.0 { 1.0 } else { -1.0 };
            return [r[0] * s, r[1] * s];
        }
        let s = if l > 0.0 { 1.0 } else { -1.0 };
        [r[0] * s, r[1] * s]
    }
}

/// The pedestrians around the player.
#[derive(Clone, Debug, PartialEq)]
pub struct Pedestrians {
    peds: Vec<Pedestrian>,
    walkers: Vec<Walker>,
    /// Per spot: bench seats taken (bits) or walkers there.
    spot_use: Vec<u8>,
    /// Per grid cell: people at normal density from its walks.
    cell_weight: Vec<f32>,
    rng: Rng,
    era: Era,
    tick: u64,
    next_id: u32,
    target: usize,
    alive: usize,
    /// The car's position `x, z`, heading, and 1 once seen, as of the last step.
    car_prev: [f32; 4],
    /// Steps the car has stood still for.
    car_still: u32,
    /// Where walks cross a rail line ([`Pedestrians::set_rail`]), and which of its crossings
    /// are closed now.
    rail: Vec<RailSpan>,
    rail_closed: Vec<bool>,
}

/// A stretch of a walk link over a rail line: along the link from `a0` to `a1` (link
/// coordinates), at the line's grade crossing `crossing`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RailSpan {
    link: u32,
    a0: f32,
    a1: f32,
    crossing: u16,
}

/// A walk counts as over the track from this far either side of the rail line's middle...
const RAIL_SPAN_M: f32 = 3.5;
/// ...and people wait this far short of that while its crossing is closed, m.
const RAIL_HOLD_M: f32 = 1.0;
/// Walks this far (beyond half their length) from a crossing are not looked at, and walks
/// are sampled this often for their stretch over the track, m.
const RAIL_NEAR_M: f32 = 30.0;
const RAIL_SAMPLE_M: f32 = 0.25;
/// Nobody spawns within this of a walk over the track, m.
const RAIL_SPAWN_CLEAR_M: f32 = 12.0;

fn left(d: [f32; 2]) -> [f32; 2] {
    [d[1], -d[0]]
}

fn xz(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[2]]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    len2([b[0] - a[0], b[2] - a[2]])
}

/// Forward ground direction of a heading.
fn forward(h: f32) -> [f32; 2] {
    let (sn, cs) = sin_cos(h);
    [-sn, -cs]
}

/// `a` turned toward `b` by at most `max` radians.
fn turn_toward(a: f32, b: f32, max: f32) -> f32 {
    let mut d = b - a;
    while d > PI {
        d -= TAU;
    }
    while d < -PI {
        d += TAU;
    }
    let mut h = a + d.clamp(-max, max);
    if h > PI {
        h -= TAU;
    } else if h < -PI {
        h += TAU;
    }
    h
}

fn approach(x: f32, to: f32, step: f32) -> f32 {
    if x < to {
        (x + step).min(to)
    } else {
        (x - step).max(to)
    }
}

/// Offsets of `n` people abreast, centered.
fn abreast(n: u8, k: u8) -> f32 {
    (f32::from(k) - 0.5 * f32::from(n - 1)) * ABREAST_M
}

impl Pedestrians {
    /// Empty pools for `budget` over `walks`, seeded by the place (or session) `seed`, with
    /// figures of the place's `era`.
    pub fn new(
        walks: &WalkGraph,
        seed: Seed,
        era: place::Era,
        budget: PedestrianBudget,
    ) -> Pedestrians {
        let n = budget.pedestrians.clamp(1, u16::MAX as usize);
        let cells = (walks.grid_cells[0] * walks.grid_cells[1]) as usize;
        let mut cell_weight = alloc::vec![0.0f32; cells];
        for (c, w) in cell_weight.iter_mut().enumerate() {
            for &l in walks.cell_links(c) {
                let link = &walks.links[l as usize];
                if link.kind == LinkKind::Walk {
                    *w += link.length_m * link.density / 100.0;
                }
            }
        }
        let rest = Pedestrian {
            alive: false,
            id: 0,
            seed: 0,
            pos: [0.0; 3],
            heading: 0.0,
            pose: PedPose::REST,
            walker: 0,
            slot: 0,
            height: 1.75,
            base: [0.0; 3],
            dodge: Dodge::NONE,
            sit: Sit::No,
        };
        Pedestrians {
            peds: alloc::vec![rest; n],
            walkers: alloc::vec![Walker::DEAD; n],
            spot_use: alloc::vec![0u8; walks.spots.len()],
            cell_weight,
            rng: Rng::new(seed, &[KEY_PEDS]),
            era: if era == place::Era::Seventies {
                Era::Seventies
            } else {
                Era::Eighties
            },
            tick: 0,
            next_id: 0,
            target: 0,
            alive: 0,
            car_prev: [0.0; 4],
            car_still: 0,
            rail: Vec::new(),
            rail_closed: Vec::new(),
        }
    }

    /// Every slot of the pool; the live people have `alive` set.
    pub fn pedestrians(&self) -> &[Pedestrian] {
        &self.peds
    }

    /// The live people.
    pub fn iter(&self) -> impl Iterator<Item = &Pedestrian> {
        self.peds.iter().filter(|p| p.alive)
    }

    /// The figures' era.
    pub fn era(&self) -> Era {
        self.era
    }

    /// People alive.
    pub fn count(&self) -> usize {
        self.alive
    }

    /// The current density target around the eye.
    pub fn target(&self) -> usize {
        self.target
    }

    /// Whether person `i` (an index into [`Self::pedestrians`]) is on a crossing's road
    /// links right now.
    pub fn on_crossing(&self, walks: &WalkGraph, i: usize) -> bool {
        let p = &self.peds[i];
        let w = &self.walkers[usize::from(p.walker)];
        p.alive && walks.links[w.link as usize].kind == LinkKind::Crossing
    }

    /// The crossing person `i` is about to step onto from the curb, or on: `None` on the
    /// walks.
    pub fn crossing_of(&self, walks: &WalkGraph, i: usize) -> Option<u32> {
        let p = &self.peds[i];
        let w = &self.walkers[usize::from(p.walker)];
        let c = walks.links[w.link as usize].crossing;
        (p.alive && c != NONE).then_some(c)
    }

    /// Whether person `i` is stepping aside or hopping.
    pub fn dodging(&self, i: usize) -> bool {
        self.peds[i].dodge.action != Action::None
    }

    /// Person `i`'s group (itself included), as indices into [`Self::pedestrians`].
    pub fn group(&self, i: usize) -> &[u16] {
        let w = &self.walkers[usize::from(self.peds[i].walker)];
        &w.members[..usize::from(w.count)]
    }

    /// Hash of every live person's id, position, heading and pose.
    pub fn hash(&self) -> u64 {
        let mut h = hash64(self.tick, &[self.alive as u64]);
        let f = |v: f32| u64::from(v.to_bits());
        for p in self.iter() {
            h = hash64(
                h,
                &[
                    u64::from(p.id),
                    p.seed,
                    f(p.pos[0]),
                    f(p.pos[1]),
                    f(p.pos[2]),
                    f(p.heading),
                    f(p.pose.phase),
                    f(p.pose.t),
                    f(p.pose.weight),
                    p.pose.action as u64,
                ],
            );
        }
        h
    }

    /// One 120 Hz step.
    pub fn step(&mut self, walks: &WalkGraph, lanes: &LaneGraph, input: &PedInput) {
        self.tick += 1;
        // The car's yaw rate from its heading last step (none after a jump).
        let pl = &input.player;
        let [px, pz, ph, seen] = self.car_prev;
        let yaw_rate = if seen > 0.0 && len2([pl.pos[0] - px, pl.pos[2] - pz]) < 2.0 {
            let mut dh = pl.heading - ph;
            while dh > PI {
                dh -= TAU;
            }
            while dh < -PI {
                dh += TAU;
            }
            (dh / DT).clamp(-2.0, 2.0)
        } else {
            0.0
        };
        self.car_prev = [pl.pos[0], pl.pos[2], pl.heading, 1.0];
        let car = CarFrame::new(pl, yaw_rate);
        self.car_still = if !car.moving() {
            self.car_still.saturating_add(1)
        } else {
            0
        };
        if self.tick % TARGET_TICKS == 1 {
            self.update_target(walks, input);
        }
        self.despawn(input);
        if self.tick.is_multiple_of(SPAWN_TICKS) && self.alive < self.target {
            self.spawn(walks, input, &car);
        }
        for w in 0..self.walkers.len() {
            if self.walkers[w].alive {
                self.step_walker(walks, lanes, input, &car, w);
            }
        }
        for i in 0..self.peds.len() {
            if self.peds[i].alive {
                self.step_person(walks, &car, i);
            }
        }
    }

    fn update_target(&mut self, walks: &WalkGraph, input: &PedInput) {
        let scale = density_scale(input.density);
        let mut sum = 0.0f32;
        if scale > 0.0 {
            let eye = xz(input.view.pos);
            let r = (TARGET_M / CELL_M) as i32 + 1;
            if let Some(c) = walks.cell_of(eye) {
                let w = walks.grid_cells[0] as i32;
                let (cx, cz) = (c as i32 % w, c as i32 / w);
                for dz in -r..=r {
                    for dx in -r..=r {
                        let (x, z) = (cx + dx, cz + dz);
                        if x < 0 || z < 0 || x >= w || z >= walks.grid_cells[1] as i32 {
                            continue;
                        }
                        let center = [
                            walks.grid_origin[0] + (x as f32 + 0.5) * CELL_M,
                            walks.grid_origin[1] + (z as f32 + 0.5) * CELL_M,
                        ];
                        let d = len2([center[0] - eye[0], center[1] - eye[1]]);
                        if d < TARGET_M {
                            sum += self.cell_weight[(z * w + x) as usize];
                        }
                    }
                }
            }
        }
        self.target = ((sum * scale + 0.5) as usize).min(self.peds.len());
    }

    fn despawn(&mut self, input: &PedInput) {
        let eye = input.view.pos;
        let over = self.alive > self.target + 2;
        let mut culled = false;
        for w in 0..self.walkers.len() {
            let wk = self.walkers[w];
            if !wk.alive {
                continue;
            }
            // Nobody vanishes in view: only hidden walkers go, far off, or over target
            // beyond the spawn ring's inner edge, or all of them with density off.
            let members = &wk.members[..usize::from(wk.count)];
            let hidden = members.iter().all(|&m| {
                let p = self.peds[usize::from(m)].pos;
                !input.view.sees(p, VIEW_R_M) && dist(p, eye) > SPAWN_NEAR_M
            });
            if !hidden {
                continue;
            }
            let far = members
                .iter()
                .all(|&m| dist(self.peds[usize::from(m)].pos, eye) > DESPAWN_M);
            if far || (over && !culled) || input.density == Density::Off {
                culled |= !far;
                self.free(w);
            }
        }
    }

    fn free(&mut self, w: usize) {
        let wk = self.walkers[w];
        for &m in &wk.members[..usize::from(wk.count)] {
            self.peds[usize::from(m)].alive = false;
            self.alive -= 1;
        }
        self.leave_spot(w);
        self.walkers[w].alive = false;
    }

    fn leave_spot(&mut self, w: usize) {
        let wk = &mut self.walkers[w];
        if let Doing::Stay { .. } = wk.doing {
            if wk.spot != NONE {
                let u = &mut self.spot_use[wk.spot as usize];
                *u = u.saturating_sub(1);
            }
        }
    }

    fn spawn(&mut self, walks: &WalkGraph, input: &PedInput, car: &CarFrame) {
        let Some(c) = walks.cell_of(xz(input.view.pos)) else {
            return;
        };
        let r = (SPAWN_FAR_M / CELL_M) as i32 + 1;
        let w = walks.grid_cells[0] as i32;
        let (cx, cz) = (c as i32 % w + self.rand_i(r), c as i32 / w + self.rand_i(r));
        if cx < 0 || cz < 0 || cx >= w || cz >= walks.grid_cells[1] as i32 {
            return;
        }
        let cell = (cz * w + cx) as usize;
        let n = self.group_size();
        let Some(slot) = self.walkers.iter().position(|x| !x.alive) else {
            return;
        };
        if self.alive + usize::from(n) > self.peds.len() {
            return;
        }
        let ok = |p: [f32; 3]| {
            let d = dist(p, input.view.pos);
            (SPAWN_NEAR_M..=SPAWN_FAR_M).contains(&d)
                && !input.view.sees(p, SPAWN_VIEW_R_M)
                && car.danger(p).is_none()
                && !car.inside(p, 3.0)
        };
        if self.rng.next_f32() < SPAWN_AT_SPOT {
            let spots = walks.cell_spots(cell);
            if spots.is_empty() {
                return;
            }
            let sp = spots[self.rng.range(0, spots.len() as u32) as usize];
            let spot = &walks.spots[sp as usize];
            if !ok(spot.pos) || self.near_rail(walks, spot.pos) || !self.spot_free(walks, sp, n) {
                return;
            }
            let link = &walks.links[spot.link as usize];
            let fwd = self.rng.next_u32() & 1 == 0;
            let s = if fwd { spot.s } else { link.length_m - spot.s };
            self.make_walker(walks, slot, n, spot.link, fwd, s);
            self.arrive(walks, slot, sp);
            for k in 0..usize::from(n) {
                let m = usize::from(self.walkers[slot].members[k]);
                let (p, h) = self.stay_slot(walks, slot, k);
                let ped = &mut self.peds[m];
                ped.base = p;
                ped.pos = p;
                ped.heading = h;
                if let Sit::On { seat, .. } = ped.sit {
                    ped.sit = Sit::On {
                        seat,
                        t: 1.0,
                        up: false,
                    };
                    ped.pos = seat;
                    ped.pose.action = Action::Sit;
                    ped.pose.t = 1.0;
                    ped.pose.weight = 1.0;
                }
            }
            return;
        }
        let links = walks.cell_links(cell);
        if links.is_empty() {
            return;
        }
        let l = links[self.rng.range(0, links.len() as u32) as usize];
        let link = &walks.links[l as usize];
        if link.kind != LinkKind::Walk || self.rng.next_f32() * 3.0 >= link.density {
            return;
        }
        let s = self.rng.next_f32() * link.length_m;
        let p = walks.point(l, s, 0.0);
        if !ok(p) || self.near_rail(walks, p) {
            return;
        }
        let fwd = self.rng.next_u32() & 1 == 0;
        let s = if fwd { s } else { link.length_m - s };
        self.make_walker(walks, slot, n, l, fwd, s);
        self.pick_target(walks, slot);
        for k in 0..usize::from(n) {
            let m = usize::from(self.walkers[slot].members[k]);
            let p = self.walk_slot(walks, slot, k);
            let ped = &mut self.peds[m];
            ped.base = p;
            ped.pos = p;
            let wk = &self.walkers[slot];
            ped.heading = heading(travel(walks, wk));
        }
    }

    /// The walks over the rail line through `line` (ground points in order) and which of
    /// its `crossings` each is at; people keep off them while [`Pedestrians::set_rail_closed`]
    /// says that crossing is closed, and nobody ever spawns near them. Call once.
    pub fn set_rail(&mut self, walks: &WalkGraph, line: &[[f32; 2]], crossings: &[[f32; 2]]) {
        self.rail.clear();
        // Distance from `q` to the line.
        let off = |q: [f32; 2]| {
            line.windows(2)
                .map(|w| {
                    let d = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
                    let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-6);
                    let t =
                        (((q[0] - w[0][0]) * d[0] + (q[1] - w[0][1]) * d[1]) / l2).clamp(0.0, 1.0);
                    len2([q[0] - w[0][0] - d[0] * t, q[1] - w[0][1] - d[1] * t])
                })
                .fold(f32::INFINITY, f32::min)
        };
        for (li, link) in walks.links.iter().enumerate() {
            let (pa, pb) = (
                walks.nodes[link.a as usize].pos,
                walks.nodes[link.b as usize].pos,
            );
            let (pa, pb) = ([pa[0], pa[2]], [pb[0], pb[2]]);
            // Only walks at a crossing: the nearest crossing to the link's middle.
            let mid = [0.5 * (pa[0] + pb[0]), 0.5 * (pa[1] + pb[1])];
            let Some((crossing, d)) = crossings
                .iter()
                .enumerate()
                .map(|(k, c)| (k, len2([c[0] - mid[0], c[1] - mid[1]])))
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            else {
                continue;
            };
            if d > 0.5 * link.length_m + RAIL_NEAR_M {
                continue;
            }
            // The stretch of the link within reach of the track, sampled.
            let n = ((link.length_m / RAIL_SAMPLE_M) as usize).max(1);
            let mut span: Option<(f32, f32)> = None;
            for k in 0..=n {
                let t = k as f32 / n as f32;
                let q = [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t];
                if off(q) < RAIL_SPAN_M {
                    let s = t * link.length_m;
                    span = Some(span.map_or((s, s), |(a, b)| (a.min(s), b.max(s))));
                }
            }
            if let Some((a0, a1)) = span {
                self.rail.push(RailSpan {
                    link: li as u32,
                    a0: a0 - RAIL_SAMPLE_M,
                    a1: a1 + RAIL_SAMPLE_M,
                    crossing: crossing as u16,
                });
            }
        }
        self.rail_closed.clear();
        self.rail_closed.resize(crossings.len(), false);
    }

    /// One person of fresh seeded looks on `link`, `s` meters along it from the end it
    /// starts at (its `a` end when `fwd`), heading for the ground point `goal`: for
    /// scripted scenes and probes that want people somewhere. `false` when the pool is
    /// full. Does not check for room or for being seen, as [`Pedestrians::step`] does.
    pub fn send(
        &mut self,
        walks: &WalkGraph,
        link: u32,
        s: f32,
        fwd: bool,
        goal: [f32; 2],
    ) -> bool {
        let Some(slot) = self.walkers.iter().position(|x| !x.alive) else {
            return false;
        };
        if self.alive >= self.peds.len() {
            return false;
        }
        self.make_walker(walks, slot, 1, link, fwd, s);
        self.walkers[slot].goal = goal;
        self.walkers[slot].give_up = self.tick + (GIVE_UP_S * SIM_HZ as f32) as u64;
        let p = self.walk_slot(walks, slot, 0);
        let ped = &mut self.peds[usize::from(self.walkers[slot].members[0])];
        ped.base = p;
        ped.pos = p;
        true
    }

    /// Which of the rail line's crossings are closed now (in [`Pedestrians::set_rail`]'s
    /// order). Allocates nothing.
    pub fn set_rail_closed(&mut self, closed: &[bool]) {
        for (c, &x) in self.rail_closed.iter_mut().zip(closed) {
            *c = x;
        }
    }

    /// Where a walker on `link` travelling `fwd`, at `s` from its entry end, must stop
    /// short of the first closed rail crossing ahead it has not stepped onto yet.
    fn rail_stop(&self, walks: &WalkGraph, link: u32, fwd: bool, s: f32) -> Option<f32> {
        let len = walks.links[link as usize].length_m;
        let mut best: Option<f32> = None;
        for r in &self.rail {
            if r.link != link
                || !self
                    .rail_closed
                    .get(usize::from(r.crossing))
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }
            let (t0, t1) = if fwd {
                (r.a0, r.a1)
            } else {
                (len - r.a1, len - r.a0)
            };
            // Already on it (or past it): walk on off it.
            if s > t0 - RAIL_HOLD_M + 0.01 && s <= t1 || s > t1 {
                continue;
            }
            let stop = t0 - RAIL_HOLD_M;
            best = Some(best.map_or(stop, |b: f32| b.min(stop)));
        }
        best
    }

    /// Whether `p` is near a walk over the rail line.
    fn near_rail(&self, walks: &WalkGraph, p: [f32; 3]) -> bool {
        self.rail.iter().any(|r| {
            let q = walks.point(r.link, 0.5 * (r.a0 + r.a1), 0.0);
            dist(p, q) < RAIL_SPAWN_CLEAR_M
        })
    }

    fn rand_i(&mut self, r: i32) -> i32 {
        self.rng.range(0, (2 * r + 1) as u32) as i32 - r
    }

    fn group_size(&mut self) -> u8 {
        let x = self.rng.next_f32();
        if x < 0.55 {
            1
        } else if x < 0.88 {
            2
        } else {
            3
        }
    }

    fn make_walker(&mut self, walks: &WalkGraph, slot: usize, n: u8, link: u32, fwd: bool, s: f32) {
        let mut members = [0u16; 3];
        let mut pace = PACE_MAX;
        let mut k = 0;
        for (i, p) in self.peds.iter().enumerate() {
            if k == usize::from(n) {
                break;
            }
            if !p.alive {
                members[k] = i as u16;
                k += 1;
            }
        }
        for (k, &m) in members[..usize::from(n)].iter().enumerate() {
            let seed = u64::from(self.rng.next_u32()) << 32 | u64::from(self.rng.next_u32());
            let own = self.rng.range_f32(PACE_MIN, PACE_MAX);
            pace = pace.min(own);
            let height = FigureSpec::from_seed(seed, self.era)
                .height_m
                .clamp(1.4, 2.1);
            self.peds[usize::from(m)] = Pedestrian {
                alive: true,
                id: self.next_id,
                seed,
                pos: [0.0; 3],
                heading: 0.0,
                pose: PedPose {
                    phase: self.rng.next_f32(),
                    ..PedPose::REST
                },
                walker: slot as u16,
                slot: k as u8,
                height,
                base: [0.0; 3],
                dodge: Dodge::NONE,
                sit: Sit::No,
            };
            self.next_id = self.next_id.wrapping_add(1);
            self.alive += 1;
        }
        let lane = self.rng.range_f32(-0.35, 0.15);
        self.walkers[slot] = Walker {
            alive: true,
            members,
            count: n,
            link,
            fwd,
            s,
            v: pace,
            pace,
            lat: 0.0,
            lane,
            doing: Doing::Walk,
            spot: NONE,
            spot_slot: 0,
            goal: [0.0; 2],
            give_up: 0,
            clearing: false,
        };
        let lat = self.lat_target(walks, &self.walkers[slot]);
        self.walkers[slot].lat = lat;
    }

    /// Whether `n` more people fit at spot `sp`.
    fn spot_free(&self, walks: &WalkGraph, sp: u32, n: u8) -> bool {
        // Nobody stays by a rail line's track.
        if self.near_rail(walks, walks.spots[sp as usize].pos) {
            return false;
        }
        let used = self.spot_use[sp as usize];
        match walks.spots[sp as usize].kind {
            SpotKind::Bench | SpotKind::Window | SpotKind::Door => used == 0 && n <= 3,
            _ => used < 3,
        }
    }

    /// A new spot to head for: one of a few random spots up to ~100 m away that has room,
    /// or a random walk point to wander to.
    fn pick_target(&mut self, walks: &WalkGraph, w: usize) {
        let wk = self.walkers[w];
        let here = xz(walks.point(wk.link, link_s(walks, &wk), 0.0));
        self.walkers[w].give_up = self.tick + (GIVE_UP_S * SIM_HZ as f32) as u64;
        let Some(c) = walks.cell_of(here) else {
            return;
        };
        let gw = walks.grid_cells[0] as i32;
        let (cx, cz) = (c as i32 % gw, c as i32 / gw);
        for attempt in 0..8 {
            let (x, z) = (cx + self.rand_i(3), cz + self.rand_i(3));
            if x < 0 || z < 0 || x >= gw || z >= walks.grid_cells[1] as i32 {
                continue;
            }
            let cell = (z * gw + x) as usize;
            if attempt < 6 {
                let spots = walks.cell_spots(cell);
                if spots.is_empty() {
                    continue;
                }
                let sp = spots[self.rng.range(0, spots.len() as u32) as usize];
                let at = xz(walks.spots[sp as usize].pos);
                if len2([at[0] - here[0], at[1] - here[1]]) < 15.0
                    || !self.spot_free(walks, sp, wk.count)
                {
                    continue;
                }
                self.walkers[w].spot = sp;
                self.walkers[w].goal = at;
                return;
            }
            let links = walks.cell_links(cell);
            if links.is_empty() {
                continue;
            }
            let l = links[self.rng.range(0, links.len() as u32) as usize];
            let lk = &walks.links[l as usize];
            if lk.kind != LinkKind::Walk {
                continue;
            }
            self.walkers[w].spot = NONE;
            self.walkers[w].goal = xz(walks.point(l, 0.5 * lk.length_m, 0.0));
            return;
        }
        // Nothing found: wander on, any way.
        self.walkers[w].spot = NONE;
        self.walkers[w].goal = here;
    }

    /// Where member `k` of walker `w` walks.
    fn walk_slot(&self, walks: &WalkGraph, w: usize, k: usize) -> [f32; 3] {
        let wk = &self.walkers[w];
        if walks.links[wk.link as usize].kind == LinkKind::Spur {
            // Single file out to the curb and waiting there, the first at the curb point
            // and the rest behind on the walk side: nobody spreads along the curb edge.
            let back = QUEUE_M * k as f32;
            let s = link_s(walks, wk) + if wk.fwd { -back } else { back };
            // Never past the curb point toward the road (followers coming off a crossing
            // would stand in the gutter), and a little past the foot onto the walk.
            let link = &walks.links[wk.link as usize];
            let curb_at_b = walks.crossings[link.crossing as usize]
                .curbs
                .contains(&link.b);
            let (lo, hi) = if curb_at_b {
                (-0.5, link.length_m)
            } else {
                (0.0, link.length_m + 0.5)
            };
            return walks.point(wk.link, s.clamp(lo, hi), 0.0);
        }
        let lat = wk.lat + abreast(wk.count, k as u8);
        let lat = if wk.fwd { lat } else { -lat };
        walks.point(wk.link, link_s(walks, wk), lat)
    }

    /// Where member `k` of walker `w` stands at its spot, and the heading it faces.
    fn stay_slot(&self, walks: &WalkGraph, w: usize, k: usize) -> ([f32; 3], f32) {
        let wk = &self.walkers[w];
        let spot = &walks.spots[wk.spot as usize];
        let link = &walks.links[spot.link as usize];
        let along = link.dir;
        // Later walkers at the same spot stand along from the first, either side.
        let sign = if wk.spot_slot % 2 == 1 { 1.0 } else { -1.0 };
        let shift = sign * 1.4 * f32::from(wk.spot_slot.div_ceil(2));
        let c = [
            spot.pos[0] + along[0] * shift,
            spot.pos[1],
            spot.pos[2] + along[1] * shift,
        ];
        let n = wk.count;
        let ring = matches!(
            spot.kind,
            SpotKind::Landmark | SpotKind::Stop | SpotKind::Parking
        ) && n >= 2;
        if spot.kind == SpotKind::Bench {
            // In front of a seat either side of the middle; a third person stands by.
            let x = left(spot.facing);
            if k < 2 {
                let off = BENCH_SEATS[k];
                let reach = sit_reach(self.peds[usize::from(wk.members[k])].height);
                let stand = [
                    spot.pos[0] + x[0] * off + spot.facing[0] * reach,
                    spot.pos[1],
                    spot.pos[2] + x[1] * off + spot.facing[1] * reach,
                ];
                return (stand, heading(spot.facing));
            }
            let p = walks.point(spot.link, spot.s, spot.lat * 0.3);
            return (
                [p[0] + x[0] * 1.2, p[1], p[2] + x[1] * 1.2],
                heading(spot.facing),
            );
        }
        if ring {
            let a = TAU * k as f32 / f32::from(n) + 0.3;
            let (sn, cs) = sin_cos(a);
            let p = [c[0] + cs * RING_M, c[1], c[2] + sn * RING_M];
            let to_c = unit2([c[0] - p[0], c[2] - p[2]]);
            return (p, heading(to_c));
        }
        let off = abreast(n, k as u8);
        (
            [c[0] + along[0] * off, c[1], c[2] + along[1] * off],
            heading(spot.facing),
        )
    }

    /// Walker `w` has reached spot `sp`: stay a while (sit if it is a bench).
    fn arrive(&mut self, walks: &WalkGraph, w: usize, sp: u32) {
        let kind = walks.spots[sp as usize].kind;
        let (lo, hi) = match kind {
            SpotKind::Window => (3.0, 9.0),
            SpotKind::Door => (4.0, 12.0),
            SpotKind::Bench => (20.0, 80.0),
            SpotKind::Stop => (15.0, 50.0),
            SpotKind::Beach | SpotKind::Landmark => (20.0, 60.0),
            SpotKind::Parking => (5.0, 15.0),
        };
        let secs = self.rng.range_f32(lo, hi);
        let used = self.spot_use[sp as usize];
        self.spot_use[sp as usize] = used.saturating_add(1);
        let wk = &mut self.walkers[w];
        wk.spot = sp;
        wk.spot_slot = used;
        wk.doing = Doing::Stay {
            until: self.tick + (secs * SIM_HZ as f32) as u64,
        };
        if kind == SpotKind::Bench {
            let spot = &walks.spots[sp as usize];
            let x = left(spot.facing);
            for (&m, off) in wk
                .members
                .iter()
                .zip(BENCH_SEATS)
                .take(usize::from(wk.count))
            {
                let m = usize::from(m);
                let seat = [
                    spot.pos[0] + x[0] * off,
                    spot.pos[1],
                    spot.pos[2] + x[1] * off,
                ];
                self.peds[m].sit = Sit::On {
                    seat,
                    t: 0.0,
                    up: false,
                };
            }
        }
    }

    fn step_walker(
        &mut self,
        walks: &WalkGraph,
        lanes: &LaneGraph,
        input: &PedInput,
        car: &CarFrame,
        w: usize,
    ) {
        let wk = self.walkers[w];
        let link = walks.links[wk.link as usize];
        let t = travel(walks, &wk);
        let on_road = link.kind == LinkKind::Crossing;

        // The car, moving or not: on the road, anyone it threatens or a car in the way
        // ahead sends the walker off the crossing; off the road, a car in the way ahead or
        // anyone walking toward its path holds the walker, and anyone with no room across
        // the walk to step aside sends it off along the walk.
        let c = car;
        let mut escape: Option<[f32; 2]> = None;
        let mut hold = false;
        for &m in &wk.members[..usize::from(wk.count)] {
            let p = &self.peds[usize::from(m)];
            if p.sit != Sit::No {
                continue;
            }
            let at = |k: f32| [p.pos[0] + t[0] * k, p.pos[1], p.pos[2] + t[1] * k];
            let blocked = c.danger(at(1.0)).is_some() || c.danger(at(2.0)).is_some();
            // Moving off along the path only helps if it leads out of the car's way (not
            // when the car comes along the path itself: then people step aside instead).
            let e = c.away(p.pos, back_dir(walks, &wk));
            let along = dot(e, t).abs() >= ALONG_MIN;
            if along && ((on_road && (blocked || c.danger(p.pos).is_some())) || p.dodge.stuck) {
                escape = Some(e);
            }
            let ahead = at(1.0);
            if blocked || (c.watch(ahead) && c.path(ahead).1.abs() < c.path(p.pos).1.abs() - 0.2) {
                hold = true;
            }
        }
        // The car standing still on the crossing by someone: they walk on off it along
        // the crossing, past the car (back to the curb when it is in the way ahead), with
        // nobody holding or waiting for the rest.
        let still = self.car_still as f32 > STILL_CLEAR_S * SIM_HZ as f32;
        let by_car = wk.members[..usize::from(wk.count)].iter().any(|&m| {
            let p = &self.peds[usize::from(m)];
            c.danger_within(p.pos, STILL_CLEAR_M + 0.5).is_some()
        });
        let clearing = on_road && still && by_car;
        let dodging = wk.members[..usize::from(wk.count)]
            .iter()
            .any(|&m| self.peds[usize::from(m)].dodge.action != Action::None);
        // Nobody is left behind on the road: on a crossing and the spur off it, or while
        // they stand in a lane or the way back to their place crosses one, the walker waits
        // for anyone the car kept from following (rather than drawing them on diagonally
        // across a lane). Only then: on the walk, a car that blocks someone for good would
        // hold the group for good.
        let stranded = wk.members[..usize::from(wk.count)]
            .iter()
            .enumerate()
            .any(|(k, &m)| {
                let p = &self.peds[usize::from(m)];
                let slot = self.walk_slot(walks, w, k);
                // (Walking off past a car standing on the crossing, those on its band follow
                // along it.)
                p.sit == Sit::No
                    && dist(p.base, slot) > STRANDED_M
                    && !(clearing && on_crossing_band(walks, p.base))
                    && (on_road
                        || link.kind == LinkKind::Spur
                        || walks.in_lane(p.base)
                        || through_lane(walks, p.base, slot))
            });

        let mut wk = wk;
        match wk.doing {
            Doing::Stay { until } => {
                if self.tick >= until {
                    // Up from the bench first.
                    let mut seated = false;
                    for &m in &wk.members[..usize::from(wk.count)] {
                        let p = &mut self.peds[usize::from(m)];
                        if let Sit::On { seat, t, .. } = p.sit {
                            p.sit = Sit::On { seat, t, up: true };
                            seated = true;
                        }
                    }
                    if !seated {
                        self.walkers[w] = wk;
                        self.leave_spot(w);
                        self.walkers[w].doing = Doing::Walk;
                        self.pick_target(walks, w);
                        return;
                    }
                }
                self.walkers[w] = wk;
                return;
            }
            Doing::Escape { cross, calm } => {
                let calm = if escape.is_some() { 0.0 } else { calm + DT };
                if let Some(e) = escape {
                    if dot(e, t) < 0.0 {
                        reverse(walks, &mut wk);
                    }
                }
                wk.doing = if calm > CALM_S {
                    if cross || on_road {
                        Doing::Cross
                    } else {
                        Doing::Walk
                    }
                } else {
                    Doing::Escape { cross, calm }
                };
                wk.v = approach(wk.v, HURRY, ACCEL * 2.0 * DT);
            }
            Doing::Wait {
                crossing,
                since,
                look,
            } => {
                wk.v = approach(wk.v, 0.0, ACCEL * DT);
                if let Some(e) = escape {
                    // A car on the walk at the curb: off along the spur, away from it.
                    if dot(e, t) < 0.0 {
                        reverse(walks, &mut wk);
                    }
                    wk.doing = Doing::Escape {
                        cross: false,
                        calm: 0.0,
                    };
                }
                let x = &walks.crossings[crossing as usize];
                let clear = ((x.length_m / PACE_MIN + 2.0) * SIM_HZ as f32) as u64;
                // The car neither on the crossing nor coming at it.
                let (a, b) = (
                    walks.nodes[x.curbs[0] as usize].pos,
                    walks.nodes[x.curbs[1] as usize].pos,
                );
                let car_clear = (0..=8).all(|k| {
                    let q = crate::math::lerp3(a, b, k as f32 / 8.0);
                    !c.watch(q) && !c.inside(q, CLEAR_M + 1.0)
                });
                if self.tick >= since + look
                    && walks.walk(lanes, crossing, input.tick, clear)
                    && car_clear
                {
                    wk.doing = Doing::Cross;
                }
            }
            Doing::Hold { since } => {
                wk.v = approach(wk.v, 0.0, ACCEL * 2.0 * DT);
                if escape.is_some() {
                    wk.doing = Doing::Escape {
                        cross: false,
                        calm: 0.0,
                    };
                } else if (!hold && !dodging) || clearing {
                    wk.doing = if on_road { Doing::Cross } else { Doing::Walk };
                } else if hold
                    && self.tick > since + (HOLD_TURN_S * SIM_HZ as f32) as u64
                    && (c.v <= CREEP_MPS
                        || self.tick > since + 3 * (HOLD_TURN_S * SIM_HZ as f32) as u64)
                {
                    reverse(walks, &mut wk);
                    wk.doing = Doing::Walk;
                    self.walkers[w] = wk;
                    self.pick_target(walks, w);
                    wk = self.walkers[w];
                }
            }
            Doing::Walk | Doing::Cross => {
                if let Some(e) = escape {
                    if dot(e, t) < 0.0 {
                        reverse(walks, &mut wk);
                    }
                    wk.doing = Doing::Escape {
                        cross: wk.doing == Doing::Cross,
                        calm: 0.0,
                    };
                } else if !clearing && (dodging || (hold && !on_road)) {
                    // On a crossing only while someone steps aside from a car coming along it.
                    wk.doing = Doing::Hold { since: self.tick };
                }
                // Off a crossing the car stands on in a hurry.
                let (v, accel) = if clearing {
                    (HURRY, ACCEL * 2.0)
                } else if on_road {
                    (wk.pace.max(1.3), ACCEL)
                } else {
                    (wk.pace, ACCEL)
                };
                wk.v = approach(wk.v, v, accel * DT);
            }
        }

        if stranded {
            wk.v = approach(wk.v, 0.0, ACCEL * 2.0 * DT);
        }

        // Move along, link to link.
        let moving = matches!(
            wk.doing,
            Doing::Walk | Doing::Cross | Doing::Escape { .. } | Doing::Hold { .. }
        );
        if moving {
            let before = wk.s;
            wk.s += wk.v * DT;
            // Never onto a closed railroad crossing: wait short of the track.
            if let Some(stop) = self.rail_stop(walks, wk.link, wk.fwd, before) {
                if wk.s > stop {
                    wk.s = stop.max(before.min(stop));
                    wk.v = 0.0;
                }
            }
            // The target spot is on this link and we are passing it.
            if wk.spot != NONE && matches!(wk.doing, Doing::Walk) {
                let spot = &walks.spots[wk.spot as usize];
                if spot.link == wk.link {
                    let at = if wk.fwd {
                        spot.s
                    } else {
                        link.length_m - spot.s
                    };
                    if wk.s >= at && wk.s - wk.v * DT <= at + 0.5 {
                        self.walkers[w] = wk;
                        if self.spot_free(walks, wk.spot, wk.count) {
                            self.arrive(walks, w, wk.spot);
                        } else {
                            self.pick_target(walks, w);
                        }
                        return;
                    }
                }
            }
            let mut guard = 0;
            while wk.s >= walks.links[wk.link as usize].length_m && guard < 4 {
                guard += 1;
                let len = walks.links[wk.link as usize].length_m;
                let came = wk.link;
                let at = if wk.fwd {
                    walks.links[came as usize].b
                } else {
                    walks.links[came as usize].a
                };
                let next = self.choose(walks, &wk, at, came);
                let nl = &walks.links[next as usize];
                // Onto a link that crosses a closed rail line right away: wait here, unless
                // already over the track (then walk on off it).
                let next_fwd = nl.a == at;
                let end = if wk.fwd { len } else { 0.0 };
                let over_track = self.rail.iter().any(|r| {
                    r.link == came && end >= r.a0 - RAIL_HOLD_M && end <= r.a1 + RAIL_HOLD_M
                });
                if !over_track
                    && self
                        .rail_stop(walks, next, next_fwd, f32::NEG_INFINITY)
                        .is_some_and(|stop| stop < 0.5)
                {
                    wk.s = len;
                    wk.v = 0.0;
                    break;
                }
                // Stepping off the curb onto a crossing: wait there first.
                let from_kind = walks.links[came as usize].kind;
                if nl.kind == LinkKind::Crossing
                    && from_kind == LinkKind::Spur
                    && wk.doing != Doing::Cross
                {
                    wk.s = len;
                    wk.v = 0.0;
                    let look = u64::from(self.rng.range(60, 200));
                    wk.doing = Doing::Wait {
                        crossing: nl.crossing,
                        since: self.tick,
                        look,
                    };
                    break;
                }
                if nl.kind != LinkKind::Crossing
                    && nl.kind != LinkKind::Spur
                    && matches!(wk.doing, Doing::Cross)
                {
                    wk.doing = Doing::Walk;
                }
                // Carry on from the shared node, keeping the position continuous.
                let p = walks.point(came, if wk.fwd { wk.s } else { len - wk.s }, 0.0);
                wk.link = next;
                wk.fwd = nl.a == at;
                let start = walks.nodes[at as usize].pos;
                let d = travel(walks, &wk);
                wk.s = dot([p[0] - start[0], p[2] - start[2]], d).max(-1.0);
            }
            if let Doing::Walk = wk.doing {
                let here = walks.point(wk.link, link_s(walks, &wk), 0.0);
                let reached =
                    wk.spot == NONE && len2([wk.goal[0] - here[0], wk.goal[1] - here[2]]) < 3.0;
                if self.tick >= wk.give_up || reached {
                    self.walkers[w] = wk;
                    self.pick_target(walks, w);
                    wk = self.walkers[w];
                }
            }
        }
        let target = self.lat_target(walks, &wk);
        wk.lat = approach(wk.lat, target, 0.5 * DT);
        wk.clearing = clearing;
        self.walkers[w] = wk;
    }

    /// Group center target off the line (left of travel), inside the link's room.
    fn lat_target(&self, walks: &WalkGraph, wk: &Walker) -> f32 {
        let link = &walks.links[wk.link as usize];
        // People walk near the walk line; the rest of the width is for stepping aside.
        let room = [
            link.room[0].min(WALK_SPREAD_M),
            link.room[1].min(WALK_SPREAD_M),
        ];
        let (r_right, r_left) = if wk.fwd {
            (room[0], room[1])
        } else {
            (room[1], room[0])
        };
        let half = abreast(wk.count, wk.count - 1).abs();
        let lo = -r_right + half + 0.15;
        let hi = r_left - half - 0.15;
        if lo > hi {
            0.5 * (lo + hi)
        } else {
            wk.lane.clamp(lo, hi)
        }
    }

    /// The link to take from node `at`, having come along `came`: the way whose far end
    /// (past plain two-link nodes) lies nearest the goal, with some randomness; back only
    /// from a dead end.
    fn choose(&mut self, walks: &WalkGraph, wk: &Walker, at: u32, came: u32) -> u32 {
        let links = walks.links_at(at);
        let mut best = (f32::MAX, came);
        for &l in links {
            if l == came {
                continue;
            }
            // Follow plain nodes a few links on.
            let (mut node, mut via) = (other(walks, l, at), l);
            for _ in 0..4 {
                let next = walks.links_at(node);
                if next.len() != 2 {
                    break;
                }
                let n = if next[0] == via { next[1] } else { next[0] };
                node = other(walks, n, node);
                via = n;
            }
            let p = walks.nodes[node as usize].pos;
            let mut score = len2([p[0] - wk.goal[0], p[2] - wk.goal[1]]);
            score += self.rng.next_f32() * 4.0;
            if walks.links[l as usize].kind == LinkKind::Spur {
                score += 4.0;
            }
            if score < best.0 {
                best = (score, l);
            }
        }
        best.1
    }

    fn step_person(&mut self, walks: &WalkGraph, car: &CarFrame, i: usize) {
        let mut p = self.peds[i];
        let wk = self.walkers[usize::from(p.walker)];
        let k = usize::from(p.slot);
        let staying = matches!(wk.doing, Doing::Stay { .. });

        // Sitting down, seated, getting up (at once if the car comes at the bench).
        if let Sit::On { seat, t, up } = p.sit {
            let up = up || car.danger(seat).is_some();
            if t > 0.0 || p.pose.action == Action::Sit {
                let t = if up {
                    t - DT / STAND_S
                } else {
                    (t + DT / SIT_S).min(1.0)
                };
                if t <= 0.0 && up {
                    let reach = sit_reach(p.height);
                    let f = forward(p.heading);
                    p.base = [seat[0] + f[0] * reach, seat[1], seat[2] + f[1] * reach];
                    p.pos = p.base;
                    p.sit = Sit::No;
                    p.pose.action = Action::None;
                    p.pose.weight = 0.0;
                    p.pose.t = 0.0;
                } else {
                    p.sit = Sit::On { seat, t, up };
                    p.pos = seat;
                    p.pose.action = Action::Sit;
                    p.pose.t = t;
                    p.pose.weight = 1.0;
                    p.pose.speed = 0.0;
                    p.pose.phase += phase_rate(p.height, 0.0) * DT;
                }
                self.peds[i] = p;
                return;
            }
            if up {
                // Never sat down: just go.
                p.sit = Sit::No;
            }
        }

        // Where to be and which way to face.
        let (want, face) = if staying {
            self.stay_slot(walks, usize::from(p.walker), k)
        } else {
            let p0 = self.walk_slot(walks, usize::from(p.walker), k);
            let face = match wk.doing {
                Doing::Wait { crossing, .. } => {
                    let x = &walks.crossings[crossing as usize];
                    let (a, b) = (
                        walks.nodes[x.curbs[0] as usize].pos,
                        walks.nodes[x.curbs[1] as usize].pos,
                    );
                    let near_a = dist(p.base, a) < dist(p.base, b);
                    let d = if near_a {
                        [b[0] - a[0], b[2] - a[2]]
                    } else {
                        [a[0] - b[0], a[2] - b[2]]
                    };
                    heading(unit2(d))
                }
                _ => heading(travel(walks, &wk)),
            };
            (p0, face)
        };
        let on_road = walks.links[wk.link as usize].kind == LinkKind::Crossing;
        // Off a crossing's band while the group is on it (back from stepping aside), or in
        // a lane off the crossings: into the band of the group's link first, straight
        // across, not on toward the slot through a lane.
        let in_road = walks.in_lane(p.base) && !on_crossing_band(walks, p.base);
        let want = if in_road && !on_road {
            out_of_lanes(walks, wk.link, p.base)
                .unwrap_or_else(|| into_band(walks, wk.link, p.base, want))
        } else if on_road || in_road {
            into_band(walks, wk.link, p.base, want)
        } else {
            want
        };
        // Ease toward the slot, but never into the car's way off the road (on it the
        // walker hurries everyone off), and not while dodging.
        let mut dg = p.dodge;
        let max = (wk.v + CATCH_UP).max(if staying { 1.2 } else { 0.0 }) * DT;
        let d = [want[0] - p.base[0], want[2] - p.base[2]];
        let l = len2(d);
        let before = p.base;
        let next = if l > max {
            [
                p.base[0] + d[0] / l * max,
                want[1],
                p.base[2] + d[1] / l * max,
            ]
        } else {
            want
        };
        let clear = car.spot_clear();
        let into = |q: [f32; 3]| {
            car.danger_within(q, clear).is_some()
                && (car.danger_within(p.base, clear).is_none() || car.gap(q) < car.gap(p.base))
        };
        let into_car = into(next);
        // Beside a car standing still, along its side instead (around it to the group;
        // never into a lane off the crossings, never closer to it). Not outward: beside a
        // car on a crosswalk there is no room for it, and that pins people at the edge.
        let lane = |q| walks.in_lane(q) && !on_crossing_band(walks, q);
        let (next, into_car) = if into_car && !car.moving() && car.out_of(p.base) > 0.0 {
            let n = car.normal(p.base);
            let step = [next[0] - p.base[0], next[2] - p.base[2]];
            let inward = dot(step, n).min(0.0);
            let q = [
                p.base[0] + step[0] - n[0] * inward,
                next[1],
                p.base[2] + step[1] - n[1] * inward,
            ];
            let ok = !into(q) && car.out_of(q) >= car.out_of(p.base) && !lane(q);
            (if ok { q } else { next }, !ok)
        } else {
            (next, into_car)
        };
        if dg.action == Action::None && !into_car {
            p.base = next;
        }
        let moved = dist(before, p.base) / DT;

        // The car: off the road, step to the nearest spot across the walk out of its way
        // (on the road the walker hurries off the crossing instead, unless that way is
        // into the car: then step out of its way across the crossing or back to a curb).
        let off = dg.offset();
        let here = [p.base[0] + off[0], p.base[1], p.base[2] + off[1]];
        // Kept from following the group by the car and left behind (in a lane off the
        // crossing, say): out of the way as if it threatened.
        let left_behind = into_car && l > STRANDED_M && dg.action == Action::None;
        let threat = car
            .danger_within(here, clear)
            .or_else(|| left_behind.then(|| car.path(here)));
        match threat {
            Some((a, _))
                // (Walking off past a car standing dead still; one edging in, however
                // slowly, they keep off as ever.)
                if !(wk.clearing && car.v < STOPPED_MPS && car.out_of(here) > 0.0)
                    && (!on_road
                        || into_car
                        || !along_helps(car, here, &wk, walks)
                        || car.swings_into(here, travel(walks, &wk), wk.v)) =>
            {
                dg.calm = 0.0;
                // A move under way keeps going while its spot is clear; if the car turns
                // so the spot is in its way, the next one.
                let to = [p.base[0] + dg.to[0], p.base[1], p.base[2] + dg.to[1]];
                // A move under way may change course only once it has run a little, so
                // a person never keeps restarting in place.
                let busy = dg.action != Action::None && dg.t < 1.0 && dg.t * dg.dur < REPLAN_S;
                let spot_ok = dg.action != Action::None && !car.blocks(to, car.spot_clear());
                if !busy && !spot_ok {
                    // Moves back to back stay within reach of where the first began,
                    // unless the car keeps coming.
                    let origin = if dg.action == Action::None {
                        off
                    } else {
                        dg.origin
                    };
                    let start = [p.base[0] + origin[0], p.base[1], p.base[2] + origin[1]];
                    let tti = if car.moving() {
                        (a - car.hl) / car.v
                    } else {
                        0.0
                    };
                    let seek = Seek {
                        car,
                        p: here,
                        // (Someone left out on a crosswalk steps along it too.)
                        on_road: on_road
                            || (on_crossing_band(walks, here) && !on_walk_band(walks, here, 0.0)),
                        limit: Some(start),
                        coming: car.coming(here),
                        tti,
                    };
                    let target = dodge_target(walks, &seek);
                    if let Some(target) = target {
                        let delta = [target[0] - here[0], target[2] - here[2]];
                        // A startled or long hop runs the whole way at once.
                        let (action, quick, dur) = move_kind(tti, len2(delta));
                        dg = Dodge {
                            action,
                            t: 0.0,
                            dur,
                            quick,
                            from: off,
                            to: [off[0] + delta[0], off[1] + delta[1]],
                            calm: 0.0,
                            stuck: false,
                            origin,
                        };
                    } else {
                        // No room across the walk: keep up with the walker as it moves off
                        // along it.
                        p.base = [p.base[0] + off[0], p.base[1], p.base[2] + off[1]];
                        dg = Dodge {
                            stuck: true,
                            ..Dodge::NONE
                        };
                    }
                }
            }
            // Walking off past a car standing on the crossing: done stepping aside.
            Some(_) if wk.clearing && car.v < STOPPED_MPS => {
                dg.calm = CALM_S + DT;
                dg.stuck = false;
            }
            _ => {
                dg.calm += DT;
                dg.stuck = false;
            }
        }
        if dg.action != Action::None {
            dg.t = (dg.t + DT / dg.dur).min(1.0);
            if dg.t >= 1.0 && dg.calm > CALM_S {
                // Done: walk back from where the move left us.
                p.base = [p.base[0] + dg.to[0], p.base[1], p.base[2] + dg.to[1]];
                dg = Dodge::NONE;
            }
        }
        p.dodge = dg;
        let off = dg.offset();
        p.pos = [p.base[0] + off[0], p.base[1], p.base[2] + off[1]];

        // Heading: along the way moved, else the way to face; face the dodge's frame.
        let target = if dg.action != Action::None {
            // Keep the sidestep across the body.
            let f = forward(p.heading);
            let m = unit2([dg.to[0] - dg.from[0], dg.to[1] - dg.from[1]]);
            let across = [-m[1], m[0]];
            if dot(f, across) >= 0.0 {
                heading(across)
            } else {
                heading([-across[0], -across[1]])
            }
        } else if moved > 0.25 {
            heading(unit2(d))
        } else {
            face
        };
        let rate = if dg.action != Action::None {
            TURN_RATE_DODGE
        } else {
            TURN_RATE
        };
        p.heading = turn_toward(p.heading, target, rate * DT);

        // Pose.
        if dg.action != Action::None {
            let f = forward(p.heading);
            let fr = right(f);
            p.pose.action = dg.action;
            p.pose.t = dg.t;
            let m = [dg.to[0] - dg.from[0], dg.to[1] - dg.from[1]];
            p.pose.side = if dot(m, fr) >= 0.0 { 1.0 } else { -1.0 };
            let rel = [car.c[0] - p.pos[0], car.c[1] - p.pos[2]];
            p.pose.look = look_yaw(dot(rel, fr), -dot(rel, f));
            p.pose.weight = (p.pose.weight + BLEND_IN * DT).min(1.0);
            p.pose.speed = approach(p.pose.speed, 0.0, 8.0 * DT);
        } else {
            p.pose.weight = (p.pose.weight - BLEND_OUT * DT).max(0.0);
            if p.pose.weight == 0.0 {
                p.pose.action = Action::None;
            }
            let v = if moved > 0.05 { moved.min(2.0) } else { 0.0 };
            p.pose.speed = approach(p.pose.speed, v, 6.0 * DT);
        }
        p.pose.phase += phase_rate(p.height, p.pose.speed) * DT;
        if p.pose.phase > 1024.0 {
            p.pose.phase -= 1024.0;
        }

        // At a bench seat, facing out: sit down.
        if staying && matches!(p.sit, Sit::On { up: false, .. }) && dist(p.base, want) < 0.05 {
            let mut dh = face - p.heading;
            while dh > PI {
                dh -= TAU;
            }
            while dh < -PI {
                dh += TAU;
            }
            if dh.abs() < 0.1 && dg.action == Action::None {
                p.heading = face;
                if let Sit::On { seat, .. } = p.sit {
                    p.sit = Sit::On {
                        seat,
                        t: DT / SIT_S,
                        up: false,
                    };
                    p.pos = seat;
                    p.pose.action = Action::Sit;
                    p.pose.t = DT / SIT_S;
                    p.pose.weight = 1.0;
                }
            }
        }
        self.peds[i] = p;
    }
}

/// Where to get out of the car's way from `p`: the nearest point within [`DODGE_REACH_M`]
/// on the walk geometry around (walks, corners and spurs, within each link's width; on
/// a crossing, its links too) at least [`SPOT_CLEAR_M`] clear of the car's footprint and
/// path (see [`Seek`]). `None` if there is none: the walker must move off along its path.
fn dodge_target(walks: &WalkGraph, seek: &Seek) -> Option<[f32; 3]> {
    let c = walks.cell_of(xz(seek.p))? as i32;
    let w = walks.grid_cells[0] as i32;
    let (cx, cz) = (c % w, c / w);
    // Across the walk geometry first; only for a car on a walk itself, the gutter beside
    // that walk too; failing both, the least bad spot (clearest of the car's way, still
    // never into it), so nobody stands still in its path.
    let future: [CarFrame; FUTURE_N] =
        core::array::from_fn(|j| seek.car.ahead(j as f32 * FUTURE_DT_S));
    // Within reach of where the episode began; with the car still coming, as far as it
    // takes before settling for the least bad spot.
    let free = Seek {
        limit: None,
        ..*seek
    };
    for (pass, seek) in [
        (Pass::Walk, seek),
        (Pass::Gutter, seek),
        (Pass::Walk, &free),
        (Pass::Gutter, &free),
        (Pass::LeastBad, if seek.coming { &free } else { seek }),
    ] {
        if pass != Pass::LeastBad && seek.limit.is_none() && !seek.coming {
            continue;
        }
        if let Some(q) = dodge_search(walks, seek, &future, (cx, cz, w), pass) {
            return Some(q);
        }
    }
    None
}

/// Who is looking for a spot out of the car's way: from `p` (on a crossing's road links
/// when `on_road`), within [`EPISODE_M`] of `limit` when given unless the car is still
/// `coming` at them, with the car `tti` seconds off.
#[derive(Clone, Copy)]
struct Seek<'a> {
    car: &'a CarFrame,
    p: [f32; 3],
    on_road: bool,
    limit: Option<[f32; 3]>,
    coming: bool,
    tti: f32,
}

/// How clear of a turning car's footprint a move from `p` to `q` keeps the person, all
/// the way and on over [`FUTURE_N`] steps, where the car will be then (`future`): its
/// body swings across ground beside it (going straight, its way covers that).
fn move_clear(seek: &Seek, future: &[CarFrame; FUTURE_N], q: [f32; 3]) -> f32 {
    if !seek.car.moving() || seek.car.k.abs() < MIN_CURVATURE {
        return f32::MAX;
    }
    let (action, quick, dur) = move_kind(seek.tti, dist(seek.p, q));
    let mut clear = f32::MAX;
    for (j, then) in future.iter().enumerate() {
        let t = j as f32 * FUTURE_DT_S / dur;
        let f = if t >= 1.0 {
            1.0
        } else {
            move_frac(action, quick, t)
        };
        clear = clear.min(then.out_of(crate::math::lerp3(seek.p, q, f)));
    }
    clear
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Walk,
    Gutter,
    LeastBad,
}

fn dodge_search(
    walks: &WalkGraph,
    seek: &Seek,
    future: &[CarFrame; FUTURE_N],
    (cx, cz, w): (i32, i32, i32),
    pass: Pass,
) -> Option<[f32; 3]> {
    let &Seek {
        car,
        p,
        on_road,
        limit,
        ..
    } = seek;
    // Every move keeps a body's half width clear of where the car will be (or, already
    // closer, gets no closer).
    let keep = FUTURE_CLEAR_M.min(car.out_of(p) - 0.05);
    // Out of the lanes and the turn curves; with nowhere clear, the turn curves will do
    // (a tight one cuts over a curb return, sidewalk all the same).
    let in_lane = |q| {
        if pass == Pass::LeastBad {
            walks.in_lane(q)
        } else {
            walks.in_lane_or_turn(q)
        }
    };
    // Lower is better: the distance to a clear spot, or (least bad) how deep in the car's
    // way it still is, then the distance.
    let mut best: Option<(f32, [f32; 3])> = None;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (x, z) = (cx + dx, cz + dz);
            if x < 0 || z < 0 || x >= w || z >= walks.grid_cells[1] as i32 {
                continue;
            }
            for &l in walks.cell_links((z * w + x) as usize) {
                let link = &walks.links[l as usize];
                if link.kind == LinkKind::Crossing && !on_road {
                    continue;
                }
                let a = walks.nodes[link.a as usize].pos;
                let rel = [p[0] - a[0], p[2] - a[2]];
                let along = dot(rel, link.dir);
                let s0 = (along - DODGE_REACH_M).max(0.0);
                let s1 = (along + DODGE_REACH_M).min(link.length_m);
                if s1 < s0 || dot(rel, left(link.dir)).abs() > DODGE_REACH_M + 1.5 {
                    continue;
                }
                let (mut lo, mut hi) = (-link.room[0] + 0.05, link.room[1] - 0.05);
                // Around a corner the walk line keeps to the middle of the sidewalk: more
                // of it on either side than its room.
                let corner = link.kind == LinkKind::Walk && link.road_side == 0;
                let edge = link.road_side != 0 || corner;
                let gutter = pass == Pass::Gutter
                    || (pass == Pass::LeastBad && edge && car.on_walk(walks, l));
                if pass == Pass::Gutter && (!edge || !car.on_walk(walks, l)) {
                    continue;
                }
                if gutter && corner {
                    (lo, hi) = (lo - CORNER_DODGE_M, hi + CORNER_DODGE_M);
                } else if gutter {
                    // Only off the curb of a walk the car is on: the gutter alone, or
                    // (least bad) the walk, its gutter and a little past its back edge
                    // together (a doorway, a step).
                    let keep = pass == Pass::LeastBad;
                    let back = if keep { BACK_DODGE_M } else { 0.0 };
                    if link.road_side > 0 {
                        (lo, hi) = (if keep { lo - back } else { hi }, hi + ROAD_DODGE_M);
                    } else {
                        (lo, hi) = (lo - ROAD_DODGE_M, if keep { hi + back } else { lo });
                    }
                }
                let ns = ((s1 - s0) / DODGE_STEP_M) as i32 + 1;
                let nc = ((hi - lo) / DODGE_STEP_M) as i32 + 1;
                for i in 0..=ns {
                    let s = s0 + (s1 - s0) * i as f32 / ns as f32;
                    for k in 0..=nc {
                        let lat = lo + (hi - lo) * k as f32 / nc as f32;
                        let q = walks.point(l, s, lat);
                        let d = dist(q, p);
                        if d > DODGE_REACH_M
                            || limit.is_some_and(|o| dist(q, o) > EPISODE_M)
                            || (link.kind != LinkKind::Crossing && in_lane(q))
                        {
                            continue;
                        }
                        let cost = if pass == Pass::LeastBad {
                            if !car.reachable(p, q, None) {
                                continue;
                            }
                            // A way that keeps the body clear of where the car will be
                            // first, then one past the car's way in time, then one that
                            // keeps out of the lanes.
                            let swept = (keep - move_clear(seek, future, q)).max(0.0);
                            let late = if car.reachable(p, q, Some(0.0)) {
                                0.0
                            } else {
                                100.0
                            };
                            let lane = on_road
                                && link.kind != LinkKind::Crossing
                                && through_lane(walks, p, q);
                            // (And one that keeps to the walks, not across a corner.)
                            let cut = link.kind != LinkKind::Crossing
                                && on_walk_band(walks, q, 0.1)
                                && !along_walks(walks, p, q);
                            swept * 1000.0
                                + late
                                + if lane { 50.0 } else { 0.0 }
                                + if cut { 30.0 } else { 0.0 }
                                - car.clearance(q) * 10.0
                                + d
                        } else {
                            if car.blocks(q, car.spot_clear())
                                || !car.reachable(p, q, Some(CROSS_SPARE_S))
                                || (on_road
                                    && link.kind != LinkKind::Crossing
                                    && through_lane(walks, p, q))
                                || move_clear(seek, future, q) < keep
                                || (pass == Pass::Walk
                                    && link.kind != LinkKind::Crossing
                                    && !along_walks(walks, p, q))
                            {
                                continue;
                            }
                            d
                        };
                        if best.is_none_or(|b| cost < b.0) {
                            best = Some((cost, q));
                        }
                    }
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// Whether the straight way from `p` to `q` keeps within the walks' bands (each walk or
/// spur link's room either side of its line, give or take a little where they meet; or a
/// crossing's), sampled every quarter meter: no cutting across the corner between two walks.
fn along_walks(walks: &WalkGraph, p: [f32; 3], q: [f32; 3]) -> bool {
    let n = (dist(p, q) / 0.25) as i32 + 1;
    (1..n).all(|k| {
        let x = crate::math::lerp3(p, q, k as f32 / n as f32);
        on_crossing_band(walks, x) || on_walk_band(walks, x, 0.1)
    })
}

/// Whether `p` lies within `slack` of a walk or spur link's band.
fn on_walk_band(walks: &WalkGraph, p: [f32; 3], slack: f32) -> bool {
    let Some(c) = walks.cell_of(xz(p)) else {
        return false;
    };
    let w = walks.grid_cells[0] as i32;
    let (cx, cz) = (c as i32 % w, c as i32 / w);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (x, z) = (cx + dx, cz + dz);
            if x < 0 || z < 0 || x >= w || z >= walks.grid_cells[1] as i32 {
                continue;
            }
            for &l in walks.cell_links((z * w + x) as usize) {
                let link = &walks.links[l as usize];
                if link.kind == LinkKind::Crossing {
                    continue;
                }
                let a = walks.nodes[link.a as usize].pos;
                let rel = [p[0] - a[0], p[2] - a[2]];
                let s = dot(rel, link.dir);
                let lat = dot(rel, left(link.dir));
                if s >= -slack
                    && s <= link.length_m + slack
                    && lat >= -link.room[0] - slack
                    && lat <= link.room[1] + slack
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether the straight way from `p` to `q` crosses a lane's band off the crossings.
fn through_lane(walks: &WalkGraph, p: [f32; 3], q: [f32; 3]) -> bool {
    (1..=8).any(|k| {
        let x = crate::math::lerp3(p, q, k as f32 / 8.0);
        walks.in_lane(x) && !on_crossing_band(walks, x)
    })
}

/// Whether `p` lies on a crossing's road links, within their width.
fn on_crossing_band(walks: &WalkGraph, p: [f32; 3]) -> bool {
    let Some(c) = walks.cell_of(xz(p)) else {
        return false;
    };
    let w = walks.grid_cells[0] as i32;
    let (cx, cz) = (c as i32 % w, c as i32 / w);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (x, z) = (cx + dx, cz + dz);
            if x < 0 || z < 0 || x >= w || z >= walks.grid_cells[1] as i32 {
                continue;
            }
            for &l in walks.cell_links((z * w + x) as usize) {
                let link = &walks.links[l as usize];
                if link.kind != LinkKind::Crossing {
                    continue;
                }
                let a = walks.nodes[link.a as usize].pos;
                let rel = [p[0] - a[0], p[2] - a[2]];
                let s = dot(rel, link.dir);
                let lat = dot(rel, left(link.dir));
                if s >= -0.3
                    && s <= link.length_m + 0.3
                    && lat >= -link.room[0]
                    && lat <= link.room[1]
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Where someone at `p` heading for `want` on link `l` goes: `want` from within the link's
/// width, else straight across into it.
fn into_band(walks: &WalkGraph, l: u32, p: [f32; 3], want: [f32; 3]) -> [f32; 3] {
    let link = &walks.links[l as usize];
    let a = walks.nodes[link.a as usize].pos;
    let rel = [p[0] - a[0], p[2] - a[2]];
    let lat = dot(rel, left(link.dir));
    let (lo, hi) = (-link.room[0] + 0.1, link.room[1] - 0.1);
    if lat >= lo - 0.1 && lat <= hi + 0.1 {
        return want;
    }
    let s = dot(rel, link.dir).clamp(0.0, link.length_m);
    walks.point(l, s, lat.clamp(lo, hi))
}

/// Where someone standing in a lane off the crossings at `p` goes first: the nearest point
/// of their group's link `l` within its width (else on its line) that is out of every lane.
fn out_of_lanes(walks: &WalkGraph, l: u32, p: [f32; 3]) -> Option<[f32; 3]> {
    let link = &walks.links[l as usize];
    let a = walks.nodes[link.a as usize].pos;
    let rel = [p[0] - a[0], p[2] - a[2]];
    let s = dot(rel, link.dir).clamp(0.0, link.length_m);
    let lat = dot(rel, left(link.dir)).clamp(-link.room[0] + 0.1, link.room[1] - 0.1);
    [lat, 0.0]
        .iter()
        .map(|&lat| walks.point(l, s, lat))
        .find(|&q| !walks.in_lane(q))
}

/// Whether moving along walker `wk`'s path takes someone at `p` out of the car's way.
fn along_helps(car: &CarFrame, p: [f32; 3], wk: &Walker, walks: &WalkGraph) -> bool {
    dot(car.away(p, back_dir(walks, wk)), travel(walks, wk)).abs() >= ALONG_MIN
}

/// Unit ground direction walker `wk` travels.
fn travel(walks: &WalkGraph, wk: &Walker) -> [f32; 2] {
    let d = walks.links[wk.link as usize].dir;
    if wk.fwd {
        d
    } else {
        [-d[0], -d[1]]
    }
}

/// Distance along the link from its `a` end.
fn link_s(walks: &WalkGraph, wk: &Walker) -> f32 {
    if wk.fwd {
        wk.s
    } else {
        walks.links[wk.link as usize].length_m - wk.s
    }
}

/// Toward the back of the walk (away from the road), or the walker's left.
fn back_dir(walks: &WalkGraph, wk: &Walker) -> [f32; 2] {
    let link = &walks.links[wk.link as usize];
    let l = left(link.dir);
    let k = if link.road_side == 0 {
        1.0
    } else {
        -f32::from(link.road_side)
    };
    [l[0] * k, l[1] * k]
}

fn reverse(walks: &WalkGraph, wk: &mut Walker) {
    let len = walks.links[wk.link as usize].length_m;
    wk.fwd = !wk.fwd;
    wk.s = (len - wk.s).max(-1.0);
}

/// The node of link `l` that is not `n`.
fn other(walks: &WalkGraph, l: u32, n: u32) -> u32 {
    let link = &walks.links[l as usize];
    if link.a == n {
        link.b
    } else {
        link.a
    }
}
