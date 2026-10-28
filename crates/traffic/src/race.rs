//! The basin's set pieces that drive (`docs/NPCS.md`, "The racers and the convoy"): three
//! street racers at the race start and a truck convoy that rolls in off the desert at
//! dawn. Both are scripted traffic cars ([`Script`]): among the other cars they keep every
//! rule an NPC car keeps (signals, claims at junctions, following, never moving into the
//! player), and they see people themselves, which ordinary NPC cars do not yet. Places and
//! people only: nothing is timed against the player, scored or chased.
//!
//! - **The racers** are called when the player stops on the line, or every
//!   [`RACE_EVERY_S`] while the player is near it. They come up from behind, out of view
//!   where they can, stand at the line one to a lane (behind the player's car in its lane),
//!   and go once nobody at all is on the course; they run it at [`RACE_MPS`] straight on
//!   through the junctions, stopping at red like anyone, and stop short of anyone who steps
//!   into their lane. After [`COURSE_M`] they are ordinary cars again.
//! - **The convoy**: [`TRUCKS`] semis come in on the freeway at its desert end once each
//!   dawn while the player is within [`CONVOY_NEAR_M`], nose to tail at [`CONVOY_MPS`], and
//!   drive the desert leg into the basin; after [`CONVOY_M`] they are ordinary traffic.
//!
//! Plain arrays and floats in a fixed order: the same inputs give the same run
//! everywhere. Nothing allocates after [`Drivers::of`].

use alloc::vec::Vec;

use place::{Place, PoiKind, PropKind};
use skywave_core::hash64;

use crate::cars::{Clock, Player, Script, Traffic, View};
use crate::lanes::{LaneGraph, TurnKind, NONE};
use crate::math::{dot, len2, sin_cos};

/// [`Script::tag`]s: a racer, a convoy truck.
pub const RACE_TAG: u8 = 1;
pub const CONVOY_TAG: u8 = 2;

/// Racers at the line, one to a lane.
pub const RACERS: usize = 3;
/// [`Race::ids`] for a racer sitting a run out: the player's car holds its lane.
pub const SAT_OUT: u32 = NONE - 1;
/// A run is called this often while the player is within [`RACE_NEAR_M`] of the line, s...
pub const RACE_EVERY_S: f32 = 240.0;
const RACE_NEAR_M: f32 = 500.0;
/// ...or when the player stops on the line, at least this long after the last, s.
const AGAIN_S: f32 = 30.0;
/// The player is on the line within this of it along the road, and stopped below this
/// speed, m and m/s.
const LINE_BOX_M: f32 = 5.0;
const STOPPED_MPS: f32 = 0.5;
/// The course's length from the line, and the racers' speed on it, m and m/s.
pub const COURSE_M: f32 = 800.0;
pub const RACE_MPS: f32 = 24.0;
/// Racers come up from this far behind the line, and wait this long for their place to
/// be out of view before coming anyway, m and s.
const STAGE_BACK_M: f32 = 140.0;
const CALL_WAIT_S: f32 = 20.0;
/// A course ends short of a junction whose lane straight on is shorter than this, m.
const SHORT_LANE_M: f32 = 10.0;
/// No car is put down within this of another, m.
const SPAWN_ROOM_M: f32 = 14.0;
/// Racers standing count down this long (the course clear all the while) before they go;
/// a run that does not get going, or does not finish, within these is called off, s.
const COUNT_S: f32 = 3.0;
const STAGE_TIMEOUT_S: f32 = 120.0;
const RUN_TIMEOUT_S: f32 = 180.0;
/// A racer stands at the line when it is this near it and nearly still, m and m/s.
const STAND_NEAR_M: f32 = 40.0;
const STAND_MPS: f32 = 0.2;
/// Anyone this near the middle of a course lane is on the course, m: about a lane's half
/// width (the sidewalk beside it is not the course).
pub const COURSE_REACH_M: f32 = 1.8;

/// Trucks in the convoy, their spacing front to front and their speed, m and m/s.
pub const TRUCKS: usize = 5;
const TRUCK_GAP_M: f32 = 34.0;
/// Trucks come in this far along the freeway's first lane, never within this of another
/// car, m.
const TRUCK_IN_M: f32 = 20.0;
const TRUCK_ROOM_M: f32 = 25.0;
pub const CONVOY_MPS: f32 = 22.0;
/// How far the convoy drives scripted, and how near its way in the player must be for it
/// to come, m.
pub const CONVOY_M: f32 = 3000.0;
pub const CONVOY_NEAR_M: f32 = 2500.0;
/// It waits this long for its way in to be out of view, and never comes in this near the
/// player, s and m.
const CONVOY_WAIT_S: f32 = 60.0;
/// A convoy that has not started coming in this long into its hour does not come.
const CONVOY_GIVE_UP_S: f32 = 1200.0;
const CONVOY_CLOSE_M: f32 = 150.0;
/// It comes in between this long before dawn and this long after, s.
const DAWN_BEFORE_S: f32 = 900.0;
const DAWN_AFTER_S: f32 = 2400.0;

/// Where a race runs: its line and lanes, where each racer comes up from and the route it
/// drives, and the course's lanes for seeing it clear.
#[derive(Clone, Debug, PartialEq)]
pub struct RaceCourse {
    /// The line's middle, the way down the course and the carriageway's width.
    pub line: [f32; 2],
    pub dir: [f32; 2],
    pub width: f32,
    /// Per racer: the lane at the line and how far along it the line is.
    pub lanes: [u32; RACERS],
    pub line_s: [f32; RACERS],
    /// Per racer: where it comes up from (a lane and how far along it) and its route
    /// ([`Traffic::set_routes`]).
    starts: [(u32, f32); RACERS],
    routes: [u32; RACERS],
    /// The course's lanes from the line on, as segments.
    corridor: Vec<[[f32; 2]; 2]>,
}

/// Where a race stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RacePhase {
    /// Between runs.
    Idle,
    /// Putting the racers down behind the line.
    Calling,
    /// The racers coming up to the line.
    Staging,
    /// All standing at the line, the course clear, counting down.
    Countdown,
    /// Running the course.
    Running,
}

/// The race start's racers.
#[derive(Clone, Debug, PartialEq)]
pub struct Race {
    pub course: RaceCourse,
    pub phase: RacePhase,
    /// Seconds in the phase, and since the last run (in [`RacePhase::Idle`]).
    t: f32,
    idle_s: f32,
    /// Each racer's car id ([`NONE`] before it is put down) and shape.
    pub ids: [u32; RACERS],
    shapes: [u8; RACERS],
    /// Runs started.
    pub runs: u32,
}

/// The dawn convoy.
#[derive(Clone, Debug, PartialEq)]
pub struct Convoy {
    /// Its way in: the freeway's desert end, the lane the trucks come in on and their
    /// route.
    pub at: [f32; 2],
    pub lane: u32,
    route: u32,
    shape: u8,
    /// The trucks' car ids, the last one's put down ([`NONE`] before).
    pub ids: [u32; TRUCKS],
    /// Whether it has come this dawn; seconds waited for its way in to be out of view.
    came: bool,
    waited: f32,
    /// Dawns it came.
    pub runs: u32,
}

/// The set pieces that drive, for one place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drivers {
    pub race: Option<Race>,
    pub convoy: Option<Convoy>,
}

/// Lane `lane`'s point nearest `p`: its distance along the lane, how far off, and the way
/// along there.
fn locate(g: &LaneGraph, lane: u32, p: [f32; 2]) -> (f32, f32, [f32; 2]) {
    let l = &g.lanes[lane as usize];
    let r = l.first_point as usize..(l.first_point + l.point_count) as usize;
    let (pts, arc) = (&g.points[r.clone()], &g.arc[r]);
    let mut best = (0.0, f32::INFINITY, [1.0, 0.0]);
    for k in 0..pts.len().saturating_sub(1) {
        let (a, b) = ([pts[k][0], pts[k][2]], [pts[k + 1][0], pts[k + 1][2]]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        if l2 <= 0.0 {
            continue;
        }
        let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
        let off = len2([a[0] + d[0] * t - p[0], a[1] + d[1] * t - p[1]]);
        if off < best.1 {
            let n = crate::math::sqrt(l2);
            best = (
                arc[k] + t * (arc[k + 1] - arc[k]),
                off,
                [d[0] / n, d[1] / n],
            );
        }
    }
    best
}

/// The turn straight on from `lane` (into the lane of its own slot when there is one), or
/// onto a freeway lane when `freeway`.
fn straight_on(g: &LaneGraph, lane: u32, freeway: bool) -> Option<u32> {
    let l = &g.lanes[lane as usize];
    let first = l.first_turn;
    let ok = |t: &crate::Turn| {
        t.kind == TurnKind::Straight && (!freeway || g.lanes[t.to as usize].freeway)
    };
    let mut best: Option<(u32, u32)> = None;
    for (k, t) in g.next(lane).iter().enumerate() {
        if !ok(t) {
            continue;
        }
        let slot_off = u32::from(g.lanes[t.to as usize].slot.abs_diff(l.slot));
        if best.is_none_or(|b| slot_off < b.0) {
            best = Some((slot_off, first + k as u32));
        }
    }
    best.map(|b| b.1)
}

/// The turns straight on from `lane` (from `s` along it) for `run` meters, and the course
/// it covers as segments into `corridor`.
fn route_on(
    g: &LaneGraph,
    lane: u32,
    s: f32,
    run: f32,
    freeway: bool,
    corridor: &mut Vec<[[f32; 2]; 2]>,
) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = lane;
    let mut from = s;
    let mut done = 0.0;
    loop {
        let l = &g.lanes[cur as usize];
        let r = l.first_point as usize..(l.first_point + l.point_count) as usize;
        let (pts, arc) = (&g.points[r.clone()], &g.arc[r]);
        for k in 0..pts.len().saturating_sub(1) {
            if arc[k + 1] >= from {
                corridor.push([[pts[k][0], pts[k][2]], [pts[k + 1][0], pts[k + 1][2]]]);
            }
        }
        done += l.length_m - from;
        if done >= run {
            break;
        }
        let Some(t) = straight_on(g, cur, freeway) else {
            break;
        };
        let turn = &g.turns[t as usize];
        // Not on into a crossing of two divided roads (a few meters of lane between two
        // signals): a course ends short of one.
        if g.lanes[turn.to as usize].length_m < SHORT_LANE_M {
            break;
        }
        let (a, b) = (turn.curve[0], turn.curve[3]);
        corridor.push([[a[0], a[2]], [b[0], b[2]]]);
        done += turn.length_m;
        out.push(t);
        cur = turn.to;
        from = 0.0;
    }
    out
}

/// Whether anyone in `people` is within [`COURSE_REACH_M`] of the course.
fn on_course(corridor: &[[[f32; 2]; 2]], people: &[[f32; 2]]) -> bool {
    people.iter().any(|&p| {
        corridor.iter().any(|seg| {
            let (a, b) = (seg[0], seg[1]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-6);
            let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
            len2([a[0] + d[0] * t - p[0], a[1] + d[1] * t - p[1]]) < COURSE_REACH_M
        })
    })
}

/// Whether car `c` is still on the script it was given.
fn scripted(traffic: &Traffic, id: u32) -> bool {
    traffic
        .cars()
        .iter()
        .any(|c| c.id == id && c.script.active())
}

/// Lets go of car `id`: an ordinary car from here.
fn let_go(traffic: &mut Traffic, id: u32) {
    if let Some(s) = traffic.script_mut(id) {
        s.route = NONE;
        s.stand_seg = NONE;
        s.v0_mps = 0.0;
    }
}

impl Drivers {
    /// The place's racers (its [`PoiKind::RaceStart`]'s line, racing in `racer_shapes`)
    /// and convoy (its [`PoiKind::Convoy`], trucks of `truck_shape`), each when the place
    /// has it, and the routes they drive (hand them to [`Traffic::set_routes`]).
    pub fn of(
        place: &Place,
        g: &LaneGraph,
        racer_shapes: &[u8],
        truck_shape: Option<u8>,
    ) -> (Drivers, Vec<Vec<u32>>) {
        let mut routes = Vec::new();
        let mut d = Drivers::default();
        if !racer_shapes.is_empty() {
            d.race = Race::of(place, g, racer_shapes, &mut routes);
        }
        if let Some(shape) = truck_shape {
            d.convoy = Convoy::of(place, g, shape, &mut routes);
        }
        (d, routes)
    }

    /// One step of `dt` seconds after the traffic's: calls, stages, starts and lets go of
    /// the racers, and brings the convoy in at dawn. `people` are where everyone is.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        traffic: &mut Traffic,
        g: &LaneGraph,
        player: &Player,
        view: &View,
        clock: &Clock,
        people: &[[f32; 2]],
        dt: f32,
    ) {
        if let Some(r) = self.race.as_mut() {
            r.step(traffic, g, player, view, people, dt);
        }
        if let Some(c) = self.convoy.as_mut() {
            c.step(traffic, g, player, view, clock, dt);
        }
    }

    pub fn hash(&self) -> u64 {
        let mut h = 0x5241_4345u64;
        if let Some(r) = &self.race {
            h = hash64(
                h,
                &[
                    r.phase as u64 | u64::from(r.runs) << 8,
                    u64::from(r.t.to_bits()) | u64::from(r.idle_s.to_bits()) << 32,
                    u64::from(r.ids[0]) | u64::from(r.ids[1]) << 32,
                    u64::from(r.ids[2]),
                ],
            );
        }
        if let Some(c) = &self.convoy {
            h = hash64(
                h,
                &[
                    u64::from(c.came) | u64::from(c.runs) << 8,
                    u64::from(c.waited.to_bits()) | u64::from(c.ids[0]) << 32,
                ],
            );
        }
        h
    }
}

impl Race {
    fn of(place: &Place, g: &LaneGraph, shapes: &[u8], routes: &mut Vec<Vec<u32>>) -> Option<Race> {
        place.pois.iter().find(|p| p.kind == PoiKind::RaceStart)?;
        let p = place
            .props
            .items
            .iter()
            .find(|p| matches!(p.kind, PropKind::StartLine { .. }))?;
        let PropKind::StartLine { width_dm } = p.kind else {
            return None;
        };
        let width = f32::from(width_dm) * 0.1;
        let line = [p.pos[0], p.pos[2]];
        let (sn, cs) = sin_cos(p.yaw);
        let dir = [-sn, -cs];
        // The lanes across the line, running down the course, by slot.
        let mut lanes: Vec<(u8, u32, f32)> = (0..g.lanes.len() as u32)
            .filter_map(|l| {
                let (s, off, d) = locate(g, l, line);
                (off < 0.5 * width
                    && dot(d, dir) > 0.9
                    && s > 0.0
                    && s < g.lanes[l as usize].length_m)
                    .then_some((g.lanes[l as usize].slot, l, s))
            })
            .collect();
        lanes.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        if lanes.len() < RACERS {
            return None;
        }
        let mut course = RaceCourse {
            line,
            dir,
            width,
            lanes: [NONE; RACERS],
            line_s: [0.0; RACERS],
            starts: [(NONE, 0.0); RACERS],
            routes: [NONE; RACERS],
            corridor: Vec::new(),
        };
        for (k, &(_, lane, s)) in lanes.iter().take(RACERS).enumerate() {
            course.lanes[k] = lane;
            course.line_s[k] = s;
            let mut corridor = Vec::new();
            let on = route_on(g, lane, s, COURSE_M, false, &mut corridor);
            if k == 0 {
                course.corridor = corridor;
            } else {
                course.corridor.extend(corridor);
            }
            // Where it comes up from: on its own lane well behind the line, else on the
            // lane leading straight into it.
            let into = (s < STAGE_BACK_M + 5.0)
                .then(|| {
                    (0..g.turns.len() as u32).find(|&t| {
                        let turn = &g.turns[t as usize];
                        turn.to == lane
                            && turn.kind == TurnKind::Straight
                            && g.lanes[turn.from as usize].slot == g.lanes[lane as usize].slot
                    })
                })
                .flatten();
            let (start, route) = match into {
                Some(t) => {
                    let from = g.turns[t as usize].from;
                    let len = g.lanes[from as usize].length_m;
                    let back = (len - (STAGE_BACK_M - s)).max(5.0).min(len - 1.0);
                    let mut r = alloc::vec![t];
                    r.extend(on);
                    ((from, back), r)
                }
                None => ((lane, (s - STAGE_BACK_M).max(0.0)), on),
            };
            course.starts[k] = start;
            course.routes[k] = routes.len() as u32;
            routes.push(route);
        }
        let mut sh = [0u8; RACERS];
        for (k, s) in sh.iter_mut().enumerate() {
            *s = shapes[k % shapes.len()];
        }
        Some(Race {
            course,
            phase: RacePhase::Idle,
            t: 0.0,
            idle_s: 0.0,
            ids: [NONE; RACERS],
            shapes: sh,
            runs: 0,
        })
    }

    /// Calls a run now if none is under way (a scene that opens on one).
    pub fn call(&mut self) {
        if self.phase == RacePhase::Idle {
            self.phase = RacePhase::Calling;
            self.t = 0.0;
        }
    }

    /// Whether the player's car stands on the line.
    pub fn player_on_line(&self, player: &Player) -> bool {
        let c = &self.course;
        let rel = [player.pos[0] - c.line[0], player.pos[2] - c.line[1]];
        let along = dot(rel, c.dir);
        let across = dot(rel, [-c.dir[1], c.dir[0]]);
        along.abs() < LINE_BOX_M && across.abs() < 0.5 * c.width && player.speed.abs() < STOPPED_MPS
    }

    /// Whether anyone in `people` is on the course.
    pub fn course_busy(&self, people: &[[f32; 2]]) -> bool {
        on_course(&self.course.corridor, people)
    }

    fn step(
        &mut self,
        traffic: &mut Traffic,
        g: &LaneGraph,
        player: &Player,
        view: &View,
        people: &[[f32; 2]],
        dt: f32,
    ) {
        self.t += dt;
        match self.phase {
            RacePhase::Idle => {
                self.idle_s += dt;
                let rel = [
                    player.pos[0] - self.course.line[0],
                    player.pos[2] - self.course.line[1],
                ];
                let near = len2(rel) < RACE_NEAR_M;
                let called = (self.player_on_line(player) && self.idle_s >= AGAIN_S)
                    || (near && self.idle_s >= RACE_EVERY_S);
                if called {
                    self.phase = RacePhase::Calling;
                    self.t = 0.0;
                }
            }
            RacePhase::Calling => {
                // The player's car on the line holds its lane: that racer sits it out.
                if self.t <= dt && self.player_on_line(player) {
                    let k = (0..RACERS).min_by(|&a, &b| {
                        let off = |k: usize| {
                            let p = g.pose(self.course.lanes[k], self.course.line_s[k]).pos;
                            len2([p[0] - player.pos[0], p[2] - player.pos[2]])
                        };
                        off(a).total_cmp(&off(b))
                    });
                    if let Some(k) = k {
                        self.ids[k] = SAT_OUT;
                    }
                }
                for k in 0..RACERS {
                    if self.ids[k] != NONE {
                        continue;
                    }
                    let (lane, s) = self.course.starts[k];
                    let p = g.pose(lane, s).pos;
                    let room = traffic
                        .cars()
                        .iter()
                        .all(|c| len2([c.pos[0] - p[0], c.pos[2] - p[2]]) >= SPAWN_ROOM_M);
                    let hidden = !view.sees(p, 6.0) || self.t > CALL_WAIT_S;
                    if !room || !hidden {
                        continue;
                    }
                    let script = Script {
                        tag: RACE_TAG,
                        route: self.course.routes[k],
                        leg: 0,
                        v0_mps: 0.0,
                        stand_seg: self.course.lanes[k],
                        stand_s: self.course.line_s[k] - 0.5,
                    };
                    self.ids[k] = traffic
                        .spawn_scripted(g, lane, s, 8.0, self.shapes[k], script)
                        .unwrap_or(NONE);
                }
                if self.ids.iter().all(|&id| id != NONE) {
                    self.phase = RacePhase::Staging;
                    self.t = 0.0;
                } else if self.t > STAGE_TIMEOUT_S {
                    self.call_off(traffic);
                }
            }
            RacePhase::Staging | RacePhase::Countdown => {
                let standing = (0..RACERS).all(|k| {
                    self.ids[k] == SAT_OUT
                        || traffic.cars().iter().any(|c| {
                            c.id == self.ids[k]
                                && c.speed < STAND_MPS
                                && len2([
                                    c.pos[0] - self.course.line[0],
                                    c.pos[2] - self.course.line[1],
                                ]) < STAND_NEAR_M
                        })
                });
                let gone = self
                    .ids
                    .iter()
                    .any(|&id| id != SAT_OUT && !scripted(traffic, id));
                let clear = !on_course(&self.course.corridor, people);
                if gone || (self.phase == RacePhase::Staging && self.t > STAGE_TIMEOUT_S) {
                    self.call_off(traffic);
                } else if self.phase == RacePhase::Staging {
                    if standing && clear {
                        self.phase = RacePhase::Countdown;
                        self.t = 0.0;
                    }
                } else if !(standing && clear) {
                    // Someone stepped onto the course: wait for them again.
                    self.phase = RacePhase::Staging;
                    self.t = 0.0;
                } else if self.t >= COUNT_S {
                    for &id in self.ids.iter().filter(|&&id| id != SAT_OUT) {
                        if let Some(s) = traffic.script_mut(id) {
                            s.stand_seg = NONE;
                            s.v0_mps = RACE_MPS;
                        }
                    }
                    self.phase = RacePhase::Running;
                    self.t = 0.0;
                    self.runs += 1;
                }
            }
            RacePhase::Running => {
                let done = self
                    .ids
                    .iter()
                    .all(|&id| id == SAT_OUT || !scripted(traffic, id));
                if done || self.t > RUN_TIMEOUT_S {
                    self.call_off(traffic);
                }
            }
        }
    }

    /// Lets the racers go as ordinary cars and waits for the next run.
    fn call_off(&mut self, traffic: &mut Traffic) {
        for id in &mut self.ids {
            if *id != NONE && *id != SAT_OUT {
                let_go(traffic, *id);
            }
            *id = NONE;
        }
        self.phase = RacePhase::Idle;
        self.t = 0.0;
        self.idle_s = 0.0;
    }
}

impl Convoy {
    fn of(place: &Place, g: &LaneGraph, shape: u8, routes: &mut Vec<Vec<u32>>) -> Option<Convoy> {
        let at = place.pois.iter().find(|p| p.kind == PoiKind::Convoy)?.pos;
        // The freeway lane leaving its end, the slow lane: the highest slot.
        let lane = (0..g.lanes.len() as u32)
            .filter(|&l| {
                let ln = &g.lanes[l as usize];
                let p = g.pose(l, 0.0).pos;
                ln.freeway && len2([p[0] - at[0], p[2] - at[1]]) < 80.0
            })
            .max_by(|&a, &b| {
                let (la, lb) = (&g.lanes[a as usize], &g.lanes[b as usize]);
                la.slot.cmp(&lb.slot).then(b.cmp(&a))
            })?;
        if g.lanes[lane as usize].length_m < TRUCK_IN_M + TRUCK_GAP_M {
            return None;
        }
        let mut corridor = Vec::new();
        let route = route_on(g, lane, 0.0, CONVOY_M, true, &mut corridor);
        let r = routes.len() as u32;
        routes.push(route);
        Some(Convoy {
            at,
            lane,
            route: r,
            shape,
            ids: [NONE; TRUCKS],
            came: false,
            waited: 0.0,
            runs: 0,
        })
    }

    /// Whether `clock` is in the convoy's hour about dawn.
    fn at_dawn(clock: &Clock) -> bool {
        let day = 86_400.0;
        let d = (clock.seconds_of_day - clock.dawn_s + day) % day;
        d < DAWN_AFTER_S || d > day - DAWN_BEFORE_S
    }

    fn step(
        &mut self,
        traffic: &mut Traffic,
        g: &LaneGraph,
        player: &Player,
        view: &View,
        clock: &Clock,
        dt: f32,
    ) {
        if !Self::at_dawn(clock) {
            self.came = false;
            self.waited = 0.0;
            self.ids = [NONE; TRUCKS];
            return;
        }
        let near = len2([player.pos[0] - self.at[0], player.pos[2] - self.at[1]]) < CONVOY_NEAR_M;
        if self.came || !near {
            return;
        }
        // One truck at a time where the freeway begins, each once the one before is a gap
        // ahead and nobody is in the way: the column rolls in.
        let spot = g.pose(self.lane, TRUCK_IN_M).pos;
        self.waited += dt;
        let seen = view.sees(spot, 10.0) && self.waited < CONVOY_WAIT_S;
        let close = len2([spot[0] - player.pos[0], spot[2] - player.pos[2]]) < CONVOY_CLOSE_M;
        // Nobody on its lane near where it comes in, or turning onto it.
        let nl = g.lanes.len() as u32;
        let room = traffic.cars().iter().all(|c| {
            if c.seg == self.lane {
                c.s - c.len > TRUCK_IN_M + 4.0 || c.s > TRUCK_IN_M + TRUCK_ROOM_M
            } else {
                c.seg < nl || g.turns[(c.seg - nl) as usize].to != self.lane
            }
        });
        let next = self.ids.iter().position(|&id| id == NONE).unwrap_or(TRUCKS);
        let gap = next == 0
            || traffic.cars().iter().any(|c| {
                c.id == self.ids[next - 1]
                    && (c.seg != self.lane || c.s >= TRUCK_IN_M + TRUCK_GAP_M)
            })
            || !traffic.cars().iter().any(|c| c.id == self.ids[next - 1]);
        if self.waited > CONVOY_GIVE_UP_S && next == 0 {
            // Not this dawn.
            self.came = true;
            return;
        }
        if seen || close || !room || !gap {
            return;
        }
        let script = Script {
            tag: CONVOY_TAG,
            route: self.route,
            leg: 0,
            v0_mps: CONVOY_MPS,
            stand_seg: NONE,
            stand_s: 0.0,
        };
        self.ids[next] = traffic
            .spawn_scripted(
                g,
                self.lane,
                TRUCK_IN_M,
                CONVOY_MPS * 0.6,
                self.shape,
                script,
            )
            .unwrap_or(NONE);
        if next == 0 && self.ids[0] != NONE {
            self.runs += 1;
        }
        if next + 1 == TRUCKS {
            self.came = true;
        }
    }
}
