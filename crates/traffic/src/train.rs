//! The basin's freight train (`docs/NPCS.md`, "The train"): a long, slow set piece on the
//! rail line, and the gates at its grade crossings. Nothing it does can hurt anyone, by
//! construction rather than by luck:
//!
//! - **It runs on a schedule, not at anyone.** It waits [`REST_S`] hidden in the wall at
//!   one end of the line, warns [`ANNOUNCE_S`] (the crossings near that end close), then
//!   runs the line at [`CRUISE_MPS`] and waits at the other end.
//! - **Crossings close well ahead of it.** A crossing is *closed* from the moment the
//!   train's head comes within [`WARN_M`] of it until its tail is [`CLEAR_M`] past: its
//!   bells ring and lamps flash at once, its arms start down [`BELL_LEAD_S`] later and are
//!   down [`GATE_S`] after that. Closed crossings are what the NPC cars stop at and what
//!   people never step onto ([`crate::cars::Traffic::set_barriers`],
//!   [`crate::peds::Pedestrians::set_rail`]).
//! - **It stops for anything in its way.** Each step it looks along the track ahead for
//!   [`Train::stopping_m`] (its braking distance plus a margin) for anything in
//!   [`Obstacle`]s inside its swept width, and brakes to stand [`STOP_GAP_M`] short of the
//!   nearest; a crossing whose arms are not yet down counts as in its way. Something that
//!   appears closer than it can brake for (a car pulled onto the track right in front of
//!   it) stops it dead on the spot: it is a wall that moves, and it never moves into
//!   anyone.
//!
//! Plain arrays and floats in a fixed order: the same inputs give the same train
//! everywhere. Nothing allocates after [`TrainLine::new`].

use alloc::vec::Vec;

use place::{Place, PoiKind, PropKind};

use crate::cars::{Barrier, Car, Player};
use crate::math::{sin_cos, sqrt};
use crate::peds::Pedestrians;

/// The train's running speed, m/s (about 25 mph), its acceleration and its service and
/// hardest braking, m/s².
pub const CRUISE_MPS: f32 = 11.0;
const ACCEL: f32 = 0.35;
const BRAKE: f32 = 1.2;
const HARD_BRAKE: f32 = 3.0;
/// It stands this far short of anything in its way, m.
pub const STOP_GAP_M: f32 = 8.0;
/// It looks this much farther ahead than it needs to stop, m.
const LOOK_MARGIN_M: f32 = 30.0;
/// It waits this long at an end of the line, then warns this long before it moves, s.
pub const REST_S: f32 = 120.0;
pub const ANNOUNCE_S: f32 = 15.0;
/// A crossing closes when the train's head comes within this of it...
pub const WARN_M: f32 = 260.0;
/// ...and opens when its tail is this far past.
pub const CLEAR_M: f32 = 20.0;
/// The bells ring this long before the arms start down, and the arms take this long to
/// come down (or go up), s.
pub const BELL_LEAD_S: f32 = 4.0;
pub const GATE_S: f32 = 8.0;
/// The locomotive's and each car's length, the gap between them, the cars behind the
/// locomotive, m.
pub const LOCO_M: f32 = 21.0;
pub const CAR_M: f32 = 16.5;
pub const COUPLER_M: f32 = 0.8;
pub const CARS: usize = 9;
/// Half the width of its bodies, m.
pub const HALF_W_M: f32 = 1.6;
/// Anything this close to its side (beyond the body) is in its way when ahead, m.
const SWEEP_MARGIN_M: f32 = 1.0;
/// Anything this close to a body touches it, m.
const CONTACT_M: f32 = 0.1;

/// The train's whole length, m.
pub const TRAIN_M: f32 = LOCO_M + CARS as f32 * (CAR_M + COUPLER_M);

/// Something the train must not move into: a box on the ground, its middle `c`, the way
/// it faces `u` (unit), its half length along `u` and half width across.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub c: [f32; 2],
    pub u: [f32; 2],
    pub half: [f32; 2],
}

impl Obstacle {
    /// A car: its middle, the way it faces, half its length and half its width.
    pub fn car(c: [f32; 2], u: [f32; 2], half_len: f32, half_w: f32) -> Obstacle {
        Obstacle {
            c,
            u,
            half: [half_len, half_w],
        }
    }

    /// A person standing at `c`.
    pub fn person(c: [f32; 2]) -> Obstacle {
        Obstacle {
            c,
            u: [1.0, 0.0],
            half: [PERSON_R_M, PERSON_R_M],
        }
    }

    /// Its four corners.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let (u, v) = (self.u, [-self.u[1], self.u[0]]);
        let mut out = [[0.0; 2]; 4];
        for (k, (a, b)) in [(1.0f32, 1.0f32), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)]
            .into_iter()
            .enumerate()
        {
            let (a, b) = (a * self.half[0], b * self.half[1]);
            out[k] = [
                self.c[0] + u[0] * a + v[0] * b,
                self.c[1] + u[1] * a + v[1] * b,
            ];
        }
        out
    }

    /// Whether it overlaps the box at `c` facing `u` with half sizes `half`, grown by
    /// `margin` (separating axes).
    pub fn overlaps(&self, c: [f32; 2], u: [f32; 2], half: [f32; 2], margin: f32) -> bool {
        let d = [self.c[0] - c[0], self.c[1] - c[1]];
        let axes = [u, [-u[1], u[0]], self.u, [-self.u[1], self.u[0]]];
        for ax in axes {
            let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
            let ra = half[0] * dot(u, ax).abs() + half[1] * dot([-u[1], u[0]], ax).abs();
            let rb = self.half[0] * dot(self.u, ax).abs()
                + self.half[1] * dot([-self.u[1], self.u[0]], ax).abs();
            if dot(d, ax).abs() > ra + rb + margin {
                return false;
            }
        }
        true
    }
}

/// The rail line the train runs: its centerline (top of rail), its length along it, and
/// where it crosses roads.
#[derive(Clone, Debug, PartialEq)]
pub struct TrainLine {
    pub pts: Vec<[f32; 3]>,
    arc: Vec<f32>,
    pub len: f32,
    /// Distance along the line of each grade crossing.
    pub crossings: Vec<f32>,
}

impl TrainLine {
    /// The line through `pts` (in order, at least two), its crossings at `crossings`
    /// (ground points; each is put where it lies along the line).
    pub fn new(pts: Vec<[f32; 3]>, crossings: &[[f32; 2]]) -> TrainLine {
        let mut arc = Vec::with_capacity(pts.len());
        let mut s = 0.0;
        for k in 0..pts.len() {
            if k > 0 {
                let (a, b) = (pts[k - 1], pts[k]);
                s += sqrt((b[0] - a[0]) * (b[0] - a[0]) + (b[2] - a[2]) * (b[2] - a[2]));
            }
            arc.push(s);
        }
        let mut line = TrainLine {
            pts,
            arc,
            len: s,
            crossings: Vec::new(),
        };
        let mut cs: Vec<f32> = crossings.iter().map(|&c| line.project(c).0).collect();
        cs.sort_by(f32::total_cmp);
        line.crossings = cs;
        line
    }

    /// Where `p` lies along the line (clamped to it) and how far from it.
    pub fn project(&self, p: [f32; 2]) -> (f32, f32) {
        let (s, off) = self.project_signed(p);
        (s, off.abs())
    }

    /// Where `p` lies along the line (clamped to it) and how far from it, left of the line
    /// positive.
    pub fn project_signed(&self, p: [f32; 2]) -> (f32, f32) {
        let mut best = (0.0, f32::INFINITY, 0.0);
        for k in 0..self.pts.len().saturating_sub(1) {
            let (a, b) = (self.pts[k], self.pts[k + 1]);
            let d = [b[0] - a[0], b[2] - a[2]];
            let l2 = d[0] * d[0] + d[1] * d[1];
            if l2 <= 0.0 {
                continue;
            }
            let t = (((p[0] - a[0]) * d[0] + (p[1] - a[2]) * d[1]) / l2).clamp(0.0, 1.0);
            let q = [a[0] + d[0] * t, a[2] + d[1] * t];
            let dist = sqrt((p[0] - q[0]) * (p[0] - q[0]) + (p[1] - q[1]) * (p[1] - q[1]));
            if dist < best.1 {
                let side = d[0] * (p[1] - q[1]) - d[1] * (p[0] - q[0]);
                let off = if side < 0.0 { -dist } else { dist };
                best = (self.arc[k] + t * (self.arc[k + 1] - self.arc[k]), dist, off);
            }
        }
        (best.0, best.2)
    }

    /// The point at `s` along the line and the unit direction along it there; past either
    /// end the end segment continues straight.
    pub fn at(&self, s: f32) -> ([f32; 3], [f32; 2]) {
        let n = self.pts.len();
        let k = if s <= 0.0 {
            0
        } else {
            self.arc[1..].partition_point(|&v| v < s).min(n - 2)
        };
        let (a, b) = (self.pts[k], self.pts[k + 1]);
        let seg = (self.arc[k + 1] - self.arc[k]).max(1e-6);
        let t = (s - self.arc[k]) / seg;
        let dir = [(b[0] - a[0]) / seg, (b[2] - a[2]) / seg];
        let y = a[1] + (b[1] - a[1]) * t.clamp(0.0, 1.0);
        ([a[0] + (b[0] - a[0]) * t, y, a[2] + (b[2] - a[2]) * t], dir)
    }
}

/// Height of the rail's top over a `place::PropKind::Track` piece's `pos` on its bed.
fn rail_top(bed: u8) -> f32 {
    match bed & !place::TRACK_SPUR {
        place::TRACK_CROSSING => 0.03,
        _ => 0.58,
    }
}

impl TrainLine {
    /// The place's main rail line, from its track pieces (not a spur's), and its grade
    /// crossings; `None` without one.
    pub fn of(place: &Place) -> Option<TrainLine> {
        let pieces: Vec<([f32; 3], [f32; 2], f32)> = place
            .props
            .items
            .iter()
            .filter_map(|p| match p.kind {
                PropKind::Track { len_dm, bed } if bed & place::TRACK_SPUR == 0 => {
                    let (sn, cs) = sin_cos(p.yaw);
                    let pos = [p.pos[0], p.pos[1] + rail_top(bed), p.pos[2]];
                    Some((pos, [-sn, -cs], f32::from(len_dm) * 0.1))
                }
                _ => None,
            })
            .collect();
        let first = pieces.first()?;
        // In order along the line: by distance along the first piece's direction.
        let d = first.1;
        let key = |p: &([f32; 3], [f32; 2], f32)| p.0[0] * d[0] + p.0[2] * d[1];
        let mut sorted = pieces.clone();
        sorted.sort_by(|a, b| key(a).total_cmp(&key(b)));
        let mut pts = Vec::with_capacity(sorted.len() + 2);
        let (p0, d0, l0) = sorted[0];
        let sign = |dir: [f32; 2]| {
            if dir[0] * d[0] + dir[1] * d[1] >= 0.0 {
                1.0
            } else {
                -1.0
            }
        };
        let s0 = sign(d0) * 0.5 * l0;
        pts.push([p0[0] - d0[0] * s0, p0[1], p0[2] - d0[1] * s0]);
        pts.extend(sorted.iter().map(|p| p.0));
        let (pn, dn, ln) = sorted[sorted.len() - 1];
        let sn = sign(dn) * 0.5 * ln;
        pts.push([pn[0] + dn[0] * sn, pn[1], pn[2] + dn[1] * sn]);
        let crossings: Vec<[f32; 2]> = place
            .pois
            .iter()
            .filter(|p| p.kind == PoiKind::GradeCrossing)
            .map(|p| p.pos)
            .collect();
        (pts.len() >= 2).then(|| TrainLine::new(pts, &crossings))
    }
}

/// A crossing gate of the place: where its mast stands and which way it faces (a
/// `place::Prop`'s `pos` and `yaw`), its arm's length, the crossing it closes (an index
/// into [`TrainLine::crossings`]) and the line its arm closes across the road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateSite {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub arm_m: f32,
    pub crossing: usize,
    pub barrier: Barrier,
}

/// The place's crossing gates on `line`, each with the crossing nearest it.
pub fn gates_of(place: &Place, line: &TrainLine) -> Vec<GateSite> {
    let mut out = Vec::new();
    for p in &place.props.items {
        let PropKind::CrossingGate { arm_dm } = p.kind else {
            continue;
        };
        let at = [p.pos[0], p.pos[2]];
        let (s, _) = line.project(at);
        let Some(crossing) = line
            .crossings
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - s)
                    .abs()
                    .total_cmp(&(b.1 - s).abs())
                    .then(a.0.cmp(&b.0))
            })
            .map(|c| c.0)
        else {
            continue;
        };
        let (sn, cs) = sin_cos(p.yaw);
        let facing = [-sn, -cs];
        // The arm swings across the road to the facing's right.
        let across = [-facing[1], facing[0]];
        let arm_m = f32::from(arm_dm) * 0.1;
        out.push(GateSite {
            pos: p.pos,
            yaw: p.yaw,
            arm_m,
            crossing,
            barrier: Barrier {
                a: at,
                b: [at[0] + across[0] * arm_m, at[1] + across[1] * arm_m],
                facing,
            },
        });
    }
    out
}

/// What the train is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Hidden at the end of the line it starts from, `t` seconds in.
    Resting { t: f32 },
    /// About to move: its first crossings close, `t` seconds in.
    Announcing { t: f32 },
    /// Under way (or stopped for something in its way).
    Running,
}

/// A grade crossing's warning: closed or not, how long, and its arms (1 up, 0 down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gate {
    pub closed: bool,
    pub closed_s: f32,
    pub arm: f32,
}

/// The train and its crossings.
#[derive(Clone, Debug, PartialEq)]
pub struct Train {
    pub line: TrainLine,
    pub phase: Phase,
    /// Which way it runs: along the line (`true`) or back.
    pub forward: bool,
    /// Its head's distance from the end it started at, and its speed, m/s.
    pub head: f32,
    pub speed: f32,
    /// Each crossing's warning, in the line's crossing order.
    pub gates: Vec<Gate>,
    /// Steps it was stopped dead by something it could not brake for.
    pub hard_stops: u32,
}

impl Train {
    /// A train resting at the start of `line`, `rest_s` seconds before it warns (pass less
    /// than [`REST_S`] to have it come sooner).
    pub fn new(line: TrainLine, rest_s: f32) -> Train {
        let gates = line
            .crossings
            .iter()
            .map(|_| Gate {
                closed: false,
                closed_s: 0.0,
                arm: 1.0,
            })
            .collect();
        Train {
            line,
            phase: Phase::Resting {
                t: (REST_S - rest_s).max(0.0),
            },
            forward: true,
            head: 0.0,
            speed: 0.0,
            gates,
            hard_stops: 0,
        }
    }

    /// The line distance of travel distance `u`.
    fn s_of(&self, u: f32) -> f32 {
        if self.forward {
            u
        } else {
            self.line.len - u
        }
    }

    /// Travel distance of line distance `s`.
    fn u_of(&self, s: f32) -> f32 {
        self.s_of(s)
    }

    /// Where its tail is, as travel distance.
    pub fn tail(&self) -> f32 {
        self.head - TRAIN_M
    }

    /// How far ahead it looks: its braking distance plus a margin, m.
    pub fn stopping_m(&self) -> f32 {
        self.speed * self.speed / (2.0 * BRAKE) + LOOK_MARGIN_M
    }

    /// Whether the train is on the line at all (not hidden at an end).
    pub fn moving_phase(&self) -> bool {
        self.phase == Phase::Running
    }

    /// Each body (the locomotive first): its middle on the ground (top of rail), the way
    /// it faces, its length.
    pub fn bodies(&self) -> impl Iterator<Item = ([f32; 3], [f32; 2], f32)> + '_ {
        (0..=CARS).map(move |k| {
            let (front, len) = if k == 0 {
                (0.0, LOCO_M)
            } else {
                (
                    LOCO_M + COUPLER_M + (k - 1) as f32 * (CAR_M + COUPLER_M),
                    CAR_M,
                )
            };
            let u = self.head - front - 0.5 * len;
            let (p, d) = self.line.at(self.s_of(u));
            let d = if self.forward { d } else { [-d[0], -d[1]] };
            (p, d, len)
        })
    }

    /// Where `p` lies in travel distance from the start, and how far off the track (signed).
    fn locate(&self, p: [f32; 2]) -> (f32, f32) {
        let (s, off) = self.line.project_signed(p);
        // Past the line's ends the projection clamps; such points are not on the track.
        (self.u_of(s), off)
    }

    /// Where `o` lies along and across the track: its least and greatest travel distance
    /// and signed offset over its corners.
    fn extent(&self, o: &Obstacle) -> ([f32; 2], [f32; 2]) {
        let (mut u, mut off) = (
            [f32::INFINITY, f32::NEG_INFINITY],
            [f32::INFINITY, f32::NEG_INFINITY],
        );
        for p in o.corners() {
            let (a, b) = self.locate(p);
            u = [u[0].min(a), u[1].max(a)];
            off = [off[0].min(b), off[1].max(b)];
        }
        (u, off)
    }

    /// Whether the disc at `p` of radius `r` reaches into the train (for tests and probes).
    pub fn touches(&self, p: [f32; 2], r: f32) -> bool {
        if self.phase != Phase::Running && self.head <= 0.0 {
            return false;
        }
        let (u, off) = self.locate(p);
        off.abs() < HALF_W_M + r && u <= self.head + r && u >= self.tail() - r
    }

    /// Whether `o`, grown by `margin`, overlaps any of the train's bodies.
    pub fn overlaps(&self, o: &Obstacle, margin: f32) -> bool {
        if self.phase != Phase::Running && self.head <= 0.0 {
            return false;
        }
        self.bodies()
            .any(|(p, d, len)| o.overlaps([p[0], p[2]], d, [0.5 * len, HALF_W_M], margin))
    }

    /// One step of `dt` seconds: the schedule, the crossings, and the train's speed
    /// against `obs` (everyone it must not move into).
    pub fn step(&mut self, obs: &[Obstacle], dt: f32) {
        match self.phase {
            Phase::Resting { t } => {
                self.phase = if t + dt >= REST_S {
                    Phase::Announcing { t: 0.0 }
                } else {
                    Phase::Resting { t: t + dt }
                };
            }
            Phase::Announcing { t } => {
                self.phase = if t + dt >= ANNOUNCE_S {
                    Phase::Running
                } else {
                    Phase::Announcing { t: t + dt }
                };
            }
            Phase::Running => self.run(obs, dt),
        }
        self.step_gates(dt);
    }

    fn run(&mut self, obs: &[Obstacle], dt: f32) {
        // Off the far end: rest there and come back the other way.
        if self.tail() > self.line.len + CLEAR_M + 40.0 {
            self.forward = !self.forward;
            self.head = 0.0;
            self.speed = 0.0;
            self.phase = Phase::Resting { t: 0.0 };
            return;
        }
        let look = self.stopping_m();
        // The nearest thing ahead, as the room left to it.
        let mut room = f32::INFINITY;
        let mut inside = false;
        for o in obs {
            // Its corners along and across the track: nowhere near the train or the track
            // ahead of it is no matter.
            let (u, off) = self.extent(o);
            let band = HALF_W_M + SWEEP_MARGIN_M;
            if off[0] > band || off[1] < -band || u[1] < self.tail() - 1.0 {
                continue;
            }
            let ahead = u[0] - self.head;
            if ahead > look {
                continue;
            }
            if ahead <= 0.5 {
                // At or under its nose, or against its side: any touch and it never moves
                // on into it.
                inside |= self.overlaps(o, CONTACT_M);
                continue;
            }
            room = room.min(ahead - STOP_GAP_M);
        }
        // A crossing whose arms are not down yet is in its way.
        for (k, &cs) in self.line.crossings.iter().enumerate() {
            let cu = self.u_of(cs);
            if cu > self.head && self.gates[k].arm > 0.0 {
                room = room.min(cu - self.head - 2.0 * STOP_GAP_M);
            }
        }
        if inside {
            self.speed = 0.0;
            self.hard_stops += 1;
            return;
        }
        let v = self.speed;
        let a = if room <= 0.0 {
            // At its stopping point, or too close to brake for: stop where it stands (a
            // hard stop if it was still rolling).
            self.speed = 0.0;
            self.hard_stops += u32::from(v > 1.0);
            return;
        } else if v * v / (2.0 * room) >= BRAKE * 0.8 {
            -(v * v / (2.0 * room)).min(HARD_BRAKE)
        } else if v < CRUISE_MPS {
            ACCEL
        } else {
            0.0
        };
        self.speed = (v + a * dt).clamp(0.0, CRUISE_MPS);
        let ds = (self.speed * dt).min(room.max(0.0));
        self.head += ds;
    }

    fn step_gates(&mut self, dt: f32) {
        let live = !matches!(self.phase, Phase::Resting { .. });
        for k in 0..self.gates.len() {
            let cu = self.u_of(self.line.crossings[k]);
            let near = cu <= self.head + WARN_M && cu >= self.tail() - CLEAR_M;
            let g = &mut self.gates[k];
            g.closed = live && near;
            if g.closed {
                g.closed_s += dt;
                if g.closed_s > BELL_LEAD_S {
                    g.arm = (g.arm - dt / GATE_S).max(0.0);
                }
            } else {
                g.closed_s = 0.0;
                g.arm = (g.arm + dt / GATE_S).min(1.0);
            }
        }
    }

    /// Whether crossing `k`'s bells ring and lamps flash: while it is closed, and while its
    /// arms are still on their way up.
    pub fn warning(&self, k: usize) -> bool {
        let g = &self.gates[k];
        g.closed || g.arm < 1.0
    }

    /// Fingerprint of the train and its gates. Allocates nothing.
    pub fn hash(&self) -> u64 {
        let phase = match self.phase {
            Phase::Resting { t } => u64::from(t.to_bits()),
            Phase::Announcing { t } => 1 << 40 | u64::from(t.to_bits()),
            Phase::Running => 2 << 40,
        };
        let mut h = skywave_core::hash64(
            0x0054_5241_494E,
            &[
                u64::from(self.head.to_bits()),
                u64::from(self.speed.to_bits()),
                u64::from(self.forward),
                phase,
            ],
        );
        for g in &self.gates {
            h = skywave_core::hash64(h, &[u64::from(g.arm.to_bits()) | u64::from(g.closed) << 32]);
        }
        h
    }
}

/// How far beyond [`WARN_M`] from the train people start keeping off a crossing: ten
/// seconds at its cruise, longer than anyone takes to walk off the track, m.
pub const PEOPLE_LEAD_M: f32 = 110.0;

/// Most obstacles the train weighs in a step.
const MAX_OBSTACLES: usize = 512;
/// A person as an obstacle, m.
const PERSON_R_M: f32 = 0.45;
/// How far from the track's middle the train stops for someone standing there, m.
pub const PERSON_SWEEP_M: f32 = HALF_W_M + SWEEP_MARGIN_M + PERSON_R_M;

/// A place's railway: its train, its crossing gates, and what they tell the traffic and
/// the people each step.
#[derive(Clone, Debug)]
pub struct Railway {
    pub train: Train,
    pub gates: Vec<GateSite>,
    barriers: Vec<Barrier>,
    closed: Vec<bool>,
    held: Vec<bool>,
    obs: Vec<Obstacle>,
}

impl Railway {
    /// The place's railway, its train resting `rest_s` before it next warns; `None` without
    /// a rail line with a crossing.
    pub fn of(place: &Place, rest_s: f32) -> Option<Railway> {
        let line = TrainLine::of(place)?;
        if line.crossings.is_empty() {
            return None;
        }
        let gates = gates_of(place, &line);
        let n = line.crossings.len();
        Some(Railway {
            train: Train::new(line, rest_s),
            barriers: Vec::with_capacity(gates.len()),
            gates,
            closed: alloc::vec![false; n],
            held: alloc::vec![false; n],
            obs: Vec::with_capacity(MAX_OBSTACLES),
        })
    }

    /// The gate lines the cars must not cross now: every gate of a closed crossing.
    pub fn barriers(&self) -> &[Barrier] {
        &self.barriers
    }

    /// Which crossings people keep off now: closed, or the train within
    /// [`PEOPLE_LEAD_M`] of closing them, in [`TrainLine::crossings`] order.
    pub fn held(&self) -> &[bool] {
        &self.held
    }

    /// Which crossings are closed now (warning, or their arms not yet back up), in
    /// [`TrainLine::crossings`] order.
    pub fn closed(&self) -> &[bool] {
        &self.closed
    }

    /// One step of `dt` seconds: the train against the player's car, the NPC `cars` and
    /// the people, then the crossings' state for the next step's traffic and people.
    pub fn step(&mut self, player: &Player, cars: &[Car], peds: &Pedestrians, dt: f32) {
        self.obs.clear();
        let t = &self.train;
        // Only what could be near the train or the track ahead of it.
        let (head, _) = t.line.at(t.s_of(t.head + t.stopping_m()));
        let (tail, _) = t.line.at(t.s_of(t.tail()));
        let near = |o: &Obstacle| {
            let r = o.half[0].max(o.half[1]);
            let p = o.c;
            let d = [head[0] - tail[0], head[2] - tail[2]];
            let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-6);
            let k = (((p[0] - tail[0]) * d[0] + (p[1] - tail[2]) * d[1]) / l2).clamp(0.0, 1.0);
            let q = [tail[0] + d[0] * k, tail[2] + d[1] * k];
            let e = [p[0] - q[0], p[1] - q[1]];
            e[0] * e[0] + e[1] * e[1] < (HALF_W_M + r + 4.0) * (HALF_W_M + r + 4.0)
        };
        let mut push = |o: Obstacle| {
            if self.obs.len() < MAX_OBSTACLES && near(&o) {
                self.obs.push(o);
            }
        };
        let (sn, cs) = sin_cos(player.heading);
        push(Obstacle::car(
            [player.pos[0], player.pos[2]],
            [-sn, -cs],
            player.half_len,
            player.half_w,
        ));
        for c in cars {
            push(Obstacle::car(
                [c.pos[0], c.pos[2]],
                c.dir,
                0.5 * c.len,
                c.half_w,
            ));
        }
        for p in peds.iter() {
            push(Obstacle::person([p.pos[0], p.pos[2]]));
        }
        self.train.step(&self.obs, dt);
        for (k, c) in self.closed.iter_mut().enumerate() {
            // Held until its arms are all the way up again, not just until the train is by.
            let g = &self.train.gates[k];
            *c = g.closed || g.arm < 1.0;
        }
        // People are held a little before the bells, so whoever is on the track then is
        // off it by the time the crossing closes.
        let t = &self.train;
        let live = !matches!(t.phase, Phase::Resting { .. });
        for (k, h) in self.held.iter_mut().enumerate() {
            let cu = t.u_of(t.line.crossings[k]);
            let near = live && cu <= t.head + WARN_M + PEOPLE_LEAD_M && cu >= t.tail() - CLEAR_M;
            *h = self.closed[k] || near;
        }
        self.barriers.clear();
        for g in &self.gates {
            if self.closed[g.crossing] {
                self.barriers.push(g.barrier);
            }
        }
    }

    pub fn hash(&self) -> u64 {
        self.train.hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight line west to east, 1,600 m, crossings at 300, 800 and 1,500 m.
    fn train(rest_s: f32) -> Train {
        let pts = (0..=10)
            .map(|k| [k as f32 * 160.0, 0.5, 0.0])
            .collect::<Vec<_>>();
        let line = TrainLine::new(pts, &[[300.0, 0.0], [800.0, 3.0], [1500.0, -2.0]]);
        Train::new(line, rest_s)
    }

    const DT: f32 = 1.0 / 120.0;

    #[test]
    fn it_rests_warns_runs_and_comes_back() {
        let mut t = train(0.0);
        assert_eq!(t.line.crossings, alloc::vec![300.0, 800.0, 1500.0]);
        let mut s = 0.0;
        let mut ran = false;
        while s < 600.0 {
            t.step(&[], DT);
            s += DT;
            ran |= t.phase == Phase::Running && t.speed > 10.0;
            // Every crossing's arms are down before the head reaches it.
            for (k, &c) in t.line.crossings.iter().enumerate() {
                let cu = t.u_of(c);
                if t.head + 0.5 >= cu && t.tail() <= cu {
                    assert_eq!(t.gates[k].arm, 0.0, "crossing {k} open under the train");
                }
            }
            if !t.forward {
                break;
            }
        }
        assert!(ran && !t.forward, "it ran the line and turned back");
        assert!(t.gates.iter().all(|g| !g.closed));
        assert_eq!(t.hard_stops, 0);
    }

    #[test]
    fn it_stops_short_of_anyone_on_the_track_and_goes_on_when_they_leave() {
        let mut t = train(0.0);
        // A car standing on the track at 1,000 m.
        let mut obs = alloc::vec![Obstacle::car([1000.0, 0.5], [0.0, 1.0], 2.3, 0.9)];
        let mut s = 0.0;
        while s < 300.0 {
            t.step(&obs, DT);
            s += DT;
            assert!(!t.overlaps(&obs[0], 0.0));
        }
        assert_eq!(t.speed, 0.0);
        let gap = 1000.0 - 0.9 - t.head;
        assert!(gap > 0.0 && gap <= STOP_GAP_M + 1.0, "{gap}");
        assert_eq!(t.hard_stops, 0);
        // Gone: it pulls away.
        obs.clear();
        for _ in 0..600 {
            t.step(&obs, DT);
        }
        assert!(t.speed > 0.5);
    }

    #[test]
    fn something_cutting_in_stops_it_dead_and_beside_it_does_not() {
        let mut t = train(0.0);
        while t.head < 600.0 {
            t.step(&[], DT);
        }
        // Beside the track, 5 m off: no reason to stop.
        let beside = [Obstacle::person([t.head + 20.0, 5.0])];
        let v = t.speed;
        t.step(&beside, DT);
        assert!(t.speed >= v - 0.01);
        // A car pulled on right in front of its nose: it stops on the spot.
        let head = t.head;
        let cut = [Obstacle::car([head + 3.0, 0.0], [0.0, 1.0], 2.3, 0.9)];
        t.step(&cut, DT);
        assert_eq!(t.speed, 0.0);
        assert!(t.head <= head + 0.1);
    }

    #[test]
    fn a_glancing_touch_on_its_side_stops_it() {
        let mut t = train(0.0);
        while t.head < 600.0 {
            t.step(&[], DT);
        }
        assert!(t.speed > 5.0);
        // A car at 40 degrees whose front corner just reaches the side of the third car
        // back: no disc along its middle would reach the train, its corner does.
        let a = 40.0f32.to_radians();
        let u = [a.sin(), -a.cos()];
        let (hl, hw) = (2.2, 0.9);
        let reach = hl * a.cos() + hw * a.sin();
        let mid = t.head - 60.0;
        let c = [mid, HALF_W_M + reach - 0.05];
        let o = Obstacle::car(c, u, hl, hw);
        assert!(!t.touches(c, hw));
        t.step(&[o], DT);
        assert_eq!(t.speed, 0.0, "it stopped");
        // And one just clear of it does not.
        let mut t2 = t.clone();
        t2.speed = 5.0;
        let o = Obstacle::car([mid, HALF_W_M + reach + 0.3], u, hl, hw);
        t2.step(&[o], DT);
        assert!(t2.speed > 0.0);
    }
}
