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
//! **The car** (no harm, by construction: nobody is ever collidable). A person in the
//! car's path ahead, within a distance that grows with its speed, steps aside toward the
//! curb or the building line (the figure's step aside, [`crate::figure::ASIDE_M`]), or
//! hops if the car is very close, and hops again while still in the way; a walker on a
//! crossing, or meeting the car head-on across its way, hurries off along its path to
//! the nearer side away from the car instead, so nobody freezes in the road. Walkers do
//! not walk into the car's path or a stopped car; they wait, then go back the other way.
//! When the car has passed, people walk back to their slots and carry on.
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

use crate::figure::{
    aside_offset, hop_offset, look_yaw, phase_rate, sit_reach, Era, FigureSpec, Joints, Skeleton,
    ASIDE_M, HOP_M,
};
use crate::lanes::{LaneGraph, NONE};
use crate::math::{atan2, dot, len2, right, sin_cos, unit2};
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
/// Out of view also means beyond this angle past the view cone's edge (cosine margin), or
/// farther than `VIEW_M`.
const VIEW_MARGIN: f32 = 0.1;
const VIEW_M: f32 = 170.0;
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
const RING_M: f32 = 0.55;
/// A walker gives up on a spot after this many seconds.
const GIVE_UP_S: f32 = 120.0;
/// Step aside and hop durations, s.
const ASIDE_S: f32 = 0.6;
const HOP_S: f32 = 0.5;
/// Hop rather than step aside when the car will be there sooner than this, s.
const HOP_TTI_S: f32 = 0.8;
/// Danger ahead of the car's front: a base plus seconds of travel; beside its path: half
/// its width plus a margin. People out of the way by `CLEAR_M` past the side are safe.
const DANGER_BASE_M: f32 = 4.0;
const DANGER_S: f32 = 2.0;
const DANGER_SIDE_M: f32 = 0.7;
/// Watched zone (walkers do not walk toward the car's path in it): farther ahead and wider.
const WATCH_BASE_M: f32 = 10.0;
const WATCH_S: f32 = 2.5;
const WATCH_SIDE_M: f32 = 4.0;
/// Back to walking once nothing has threatened for this long, s.
const CALM_S: f32 = 0.6;
/// After holding this long for a stopped car, a walker turns back, s.
const HOLD_TURN_S: f32 = 4.0;
/// Sitting down and getting up, s.
const SIT_S: f32 = 1.2;
const STAND_S: f32 = 1.0;
/// Seats along a bench from its middle (to the sitter's left).
const BENCH_SEATS: [f32; 2] = [-0.45, 0.45];
/// Blend weight rates, per second.
const BLEND_IN: f32 = 8.0;
const BLEND_OUT: f32 = 4.0;

/// Pedestrian density setting.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Off = 0,
    Low = 1,
    Normal = 2,
    Busy = 3,
}

impl Density {
    /// Multiplier on the zones' people per 100 m of walk.
    pub fn scale(self) -> f32 {
        match self {
            Density::Off => 0.0,
            Density::Low => 0.35,
            Density::Normal => 1.0,
            Density::Busy => 2.0,
        }
    }
}

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

/// The player's car this step: its footprint and motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Car {
    /// Center of the footprint on the ground.
    pub pos: [f32; 3],
    /// Vehicle sim convention: 0 faces -z, positive turns left.
    pub heading: f32,
    /// Along the heading, m/s (negative reversing).
    pub speed: f32,
    /// Half width and half length of the body.
    pub half_extents: [f32; 2],
}

/// What the camera sees: a horizontal cone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub eye: [f32; 3],
    /// Unit ground direction the camera looks along.
    pub forward: [f32; 2],
    /// Cosine of half the horizontal field of view.
    pub cos_half_fov: f32,
}

impl View {
    /// Whether ground point `p` is (nearly) in view.
    pub fn sees(&self, p: [f32; 3]) -> bool {
        let d = [p[0] - self.eye[0], p[2] - self.eye[2]];
        let l = len2(d);
        if l > VIEW_M {
            return false;
        }
        if l < 1.0 {
            return true;
        }
        dot(d, self.forward) / l > self.cos_half_fov - VIEW_MARGIN
    }
}

/// Inputs of one step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PedInput {
    /// The signal clock ([`crate::SignalClock`]): crossings follow the lights.
    pub tick: u64,
    pub density: Density,
    pub view: View,
    pub car: Option<Car>,
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

/// Getting out of the car's way: moves of `dir` (ground, unit) done so far and under way.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Dodge {
    action: Action,
    t: f32,
    dir: [f32; 2],
    done: f32,
    calm: f32,
}

impl Dodge {
    const NONE: Dodge = Dodge {
        action: Action::None,
        t: 0.0,
        dir: [0.0; 2],
        done: 0.0,
        calm: 0.0,
    };

    fn offset(&self) -> f32 {
        self.done
            + match self.action {
                Action::Aside => aside_offset(self.t),
                Action::Hop => hop_offset(self.t),
                _ => 0.0,
            }
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
}

impl CarFrame {
    fn new(car: &Car) -> CarFrame {
        let (sn, cs) = sin_cos(car.heading);
        let mut f = [-sn, -cs];
        if car.speed < 0.0 {
            f = [-f[0], -f[1]];
        }
        CarFrame {
            c: [car.pos[0], car.pos[2]],
            f,
            r: right(f),
            hw: car.half_extents[0],
            hl: car.half_extents[1],
            v: car.speed.abs(),
        }
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

    fn moving(&self) -> bool {
        self.v > 0.3
    }

    /// In the moving car's way soon: `(ahead, lat)`.
    fn danger(&self, p: [f32; 3]) -> Option<(f32, f32)> {
        let (a, l) = self.rel(p);
        let reach = self.hl + DANGER_BASE_M + DANGER_S * self.v;
        (self.moving() && a > -self.hl - 0.3 && a < reach && l.abs() < self.hw + DANGER_SIDE_M)
            .then_some((a, l))
    }

    /// In the zone near the moving car's way where walkers do not walk toward it.
    fn watch(&self, p: [f32; 3]) -> bool {
        let (a, l) = self.rel(p);
        let reach = self.hl + WATCH_BASE_M + WATCH_S * self.v;
        self.moving() && a > -self.hl - 1.0 && a < reach && l.abs() < self.hw + WATCH_SIDE_M
    }

    /// Ground direction away from the car's path at `p`.
    fn away(&self, p: [f32; 3], fallback: [f32; 2]) -> [f32; 2] {
        let (_, l) = self.rel(p);
        if l.abs() < 0.05 {
            let s = if dot(fallback, self.r) >= 0.0 {
                1.0
            } else {
                -1.0
            };
            return [self.r[0] * s, self.r[1] * s];
        }
        let s = if l > 0.0 { 1.0 } else { -1.0 };
        [self.r[0] * s, self.r[1] * s]
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
}

fn left(d: [f32; 2]) -> [f32; 2] {
    [d[1], -d[0]]
}

fn xz(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[2]]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    len2([b[0] - a[0], b[2] - a[2]])
}

/// Heading (vehicle convention) of ground direction `d`.
fn heading_of(d: [f32; 2]) -> f32 {
    atan2(-d[0], -d[1])
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
        let car = input.car.as_ref().map(CarFrame::new);
        if self.tick % TARGET_TICKS == 1 {
            self.update_target(walks, input);
        }
        self.despawn(input);
        if self.tick.is_multiple_of(SPAWN_TICKS) && self.alive < self.target {
            self.spawn(walks, input);
        }
        for w in 0..self.walkers.len() {
            if self.walkers[w].alive {
                self.step_walker(walks, lanes, input, car.as_ref(), w);
            }
        }
        for i in 0..self.peds.len() {
            if self.peds[i].alive {
                self.step_person(walks, car.as_ref(), i);
            }
        }
    }

    fn update_target(&mut self, walks: &WalkGraph, input: &PedInput) {
        let scale = input.density.scale();
        let mut sum = 0.0f32;
        if scale > 0.0 {
            let eye = xz(input.view.eye);
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
        let eye = input.view.eye;
        let over = self.alive > self.target + 2;
        let mut culled = false;
        for w in 0..self.walkers.len() {
            let wk = self.walkers[w];
            if !wk.alive {
                continue;
            }
            let members = &wk.members[..usize::from(wk.count)];
            let far = members
                .iter()
                .all(|&m| dist(self.peds[usize::from(m)].pos, eye) > DESPAWN_M);
            let hidden = || {
                members.iter().all(|&m| {
                    let p = self.peds[usize::from(m)].pos;
                    !input.view.sees(p) && dist(p, eye) > SPAWN_NEAR_M
                })
            };
            if far || (over && !culled && hidden()) || input.density == Density::Off && hidden() {
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

    fn spawn(&mut self, walks: &WalkGraph, input: &PedInput) {
        let Some(c) = walks.cell_of(xz(input.view.eye)) else {
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
            let d = dist(p, input.view.eye);
            (SPAWN_NEAR_M..=SPAWN_FAR_M).contains(&d) && !input.view.sees(p)
        };
        if self.rng.next_f32() < SPAWN_AT_SPOT {
            let spots = walks.cell_spots(cell);
            if spots.is_empty() {
                return;
            }
            let sp = spots[self.rng.range(0, spots.len() as u32) as usize];
            let spot = &walks.spots[sp as usize];
            if !ok(spot.pos) || !self.spot_free(walks, sp, n) {
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
        if !ok(p) {
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
            ped.heading = heading_of(travel(walks, wk));
        }
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
        };
        let lat = self.lat_target(walks, &self.walkers[slot]);
        self.walkers[slot].lat = lat;
    }

    /// Whether `n` more people fit at spot `sp`.
    fn spot_free(&self, walks: &WalkGraph, sp: u32, n: u8) -> bool {
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
                return (stand, heading_of(spot.facing));
            }
            let p = walks.point(spot.link, spot.s, spot.lat * 0.3);
            return (
                [p[0] + x[0] * 1.2, p[1], p[2] + x[1] * 1.2],
                heading_of(spot.facing),
            );
        }
        if ring {
            let a = TAU * k as f32 / f32::from(n) + 0.3;
            let (sn, cs) = sin_cos(a);
            let p = [c[0] + cs * RING_M, c[1], c[2] + sn * RING_M];
            let to_c = unit2([c[0] - p[0], c[2] - p[2]]);
            return (p, heading_of(to_c));
        }
        let off = abreast(n, k as u8);
        (
            [c[0] + along[0] * off, c[1], c[2] + along[1] * off],
            heading_of(spot.facing),
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
        car: Option<&CarFrame>,
        w: usize,
    ) {
        let wk = self.walkers[w];
        let link = walks.links[wk.link as usize];
        let t = travel(walks, &wk);
        let on_road = link.kind == LinkKind::Crossing;

        // The car: anyone in its way on the road makes the walker escape along the
        // crossing; anyone walking toward its path makes the walker hold.
        let mut escape: Option<[f32; 2]> = None;
        let mut hold = false;
        if let Some(c) = car {
            for &m in &wk.members[..usize::from(wk.count)] {
                let p = &self.peds[usize::from(m)];
                if p.sit != Sit::No {
                    continue;
                }
                if on_road && c.danger(p.pos).is_some() {
                    escape = Some(c.away(p.pos, back_dir(walks, &wk)));
                }
                let ahead = [p.pos[0] + t[0], p.pos[1], p.pos[2] + t[1]];
                if c.inside(ahead, 0.8)
                    || (c.watch(ahead) && c.rel(ahead).1.abs() < c.rel(p.pos).1.abs() - 0.2)
                {
                    hold = true;
                }
            }
        }
        let dodging = wk.members[..usize::from(wk.count)]
            .iter()
            .any(|&m| self.peds[usize::from(m)].dodge.action != Action::None);

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
                let x = &walks.crossings[crossing as usize];
                let clear = ((x.length_m / PACE_MIN + 2.0) * SIM_HZ as f32) as u64;
                let car_clear = car.is_none_or(|c| {
                    let mid = walks.nodes[x.curbs[0] as usize].pos;
                    let far = walks.nodes[x.curbs[1] as usize].pos;
                    !c.watch(mid) && !c.watch(far) && !c.inside(mid, 2.0) && !c.inside(far, 2.0)
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
                } else if !hold && !dodging {
                    wk.doing = Doing::Walk;
                } else if hold
                    && self.tick > since + (HOLD_TURN_S * SIM_HZ as f32) as u64
                    && car.is_some_and(|c| !c.moving())
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
                } else if (hold || dodging) && !on_road {
                    wk.doing = Doing::Hold { since: self.tick };
                }
                let v = if on_road { wk.pace.max(1.3) } else { wk.pace };
                wk.v = approach(wk.v, v, ACCEL * DT);
            }
        }

        // Move along, link to link.
        let moving = matches!(
            wk.doing,
            Doing::Walk | Doing::Cross | Doing::Escape { .. } | Doing::Hold { .. }
        );
        if moving {
            wk.s += wk.v * DT;
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
        self.walkers[w] = wk;
    }

    /// Group center target off the line (left of travel), inside the link's room.
    fn lat_target(&self, walks: &WalkGraph, wk: &Walker) -> f32 {
        let link = &walks.links[wk.link as usize];
        let (r_right, r_left) = if wk.fwd {
            (link.room[0], link.room[1])
        } else {
            (link.room[1], link.room[0])
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

    fn step_person(&mut self, walks: &WalkGraph, car: Option<&CarFrame>, i: usize) {
        let mut p = self.peds[i];
        let wk = self.walkers[usize::from(p.walker)];
        let k = usize::from(p.slot);
        let staying = matches!(wk.doing, Doing::Stay { .. });

        // Sitting down, seated, getting up.
        if let Sit::On { seat, t, up } = p.sit {
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
                    heading_of(unit2(d))
                }
                _ => heading_of(travel(walks, &wk)),
            };
            (p0, face)
        };
        let max = (wk.v + CATCH_UP).max(if staying { 1.2 } else { 0.0 }) * DT;
        let d = [want[0] - p.base[0], want[2] - p.base[2]];
        let l = len2(d);
        let before = p.base;
        if l > max {
            p.base = [
                p.base[0] + d[0] / l * max,
                want[1],
                p.base[2] + d[1] / l * max,
            ];
        } else {
            p.base = want;
        }
        let moved = dist(before, p.base) / DT;

        // The car.
        let mut threat = None;
        if let Some(c) = car {
            if let Some((a, _)) = c.danger(p.pos) {
                // Off the road, step straight away from the car's path: toward the curb or
                // the building line when it comes along the walk. On the road the walker
                // hurries off the crossing instead.
                if walks.links[wk.link as usize].kind != LinkKind::Crossing
                    && !matches!(wk.doing, Doing::Escape { .. })
                {
                    let e = c.away(p.pos, back_dir(walks, &wk));
                    let tti = (a - c.hl) / c.v.max(0.1);
                    threat = Some((e, tti));
                }
            }
        }
        let mut dg = p.dodge;
        match threat {
            Some((dir, tti)) => {
                dg.calm = 0.0;
                if dg.action == Action::None {
                    dg = Dodge {
                        action: if tti < HOP_TTI_S {
                            Action::Hop
                        } else {
                            Action::Aside
                        },
                        t: 0.0,
                        dir,
                        done: 0.0,
                        calm: 0.0,
                    };
                } else if dg.t >= 1.0 {
                    dg.done += if dg.action == Action::Hop {
                        HOP_M
                    } else {
                        ASIDE_M
                    };
                    dg.action = Action::Hop;
                    dg.t = 0.0;
                }
            }
            None => dg.calm += DT,
        }
        if dg.action != Action::None {
            let dur = if dg.action == Action::Hop {
                HOP_S
            } else {
                ASIDE_S
            };
            dg.t = (dg.t + DT / dur).min(1.0);
            if dg.t >= 1.0 && dg.calm > CALM_S {
                // Done: walk back from where the step left us.
                let off = dg.offset();
                p.base = [
                    p.base[0] + dg.dir[0] * off,
                    p.base[1],
                    p.base[2] + dg.dir[1] * off,
                ];
                dg = Dodge::NONE;
            }
        }
        p.dodge = dg;
        let off = dg.offset();
        p.pos = [
            p.base[0] + dg.dir[0] * off,
            p.base[1],
            p.base[2] + dg.dir[1] * off,
        ];

        // Heading: along the way moved, else the way to face; face the dodge's frame.
        let target = if dg.action != Action::None {
            // Keep the sidestep across the body: face along the walk.
            let f = forward(p.heading);
            let across = [-dg.dir[1], dg.dir[0]];
            if dot(f, across) >= 0.0 {
                heading_of(across)
            } else {
                heading_of([-across[0], -across[1]])
            }
        } else if moved > 0.25 {
            heading_of(unit2(d))
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
            p.pose.side = if dot(dg.dir, fr) >= 0.0 { 1.0 } else { -1.0 };
            if let Some(c) = car {
                let rel = [c.c[0] - p.pos[0], c.c[1] - p.pos[2]];
                p.pose.look = look_yaw(dot(rel, fr), -dot(rel, f));
            }
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
