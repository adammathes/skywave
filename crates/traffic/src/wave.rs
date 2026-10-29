//! The player's green wave: signals passed on green in a run, for the feedback that grows
//! with it (`docs/NPCS.md`, "Green waves").
//!
//! [`Streak::step`] runs once per sim step with the signal tick and the player's car. The
//! car passes a signal where it crosses the stop line of a signalized approach driving its
//! way. On green, without a stop, a hard brake or speeding since the last light, the run
//! grows by one; on red, or after any of those, it is over (quietly: the run is 0, the
//! next green starts again at 1). Yellow neither adds nor breaks. Nothing here feeds back
//! into the sim.

use alloc::vec::Vec;

use crate::cars::Player;
use crate::lanes::{Control, LaneGraph};
use crate::math::{dot, sin_cos, sqrt};
use crate::signal::Light;

/// Slower than this, m/s, the car has stopped.
pub const STOP_MPS: f32 = 1.0;
/// Slowing harder than this, m/s², is a hard brake (smoothed over [`BRAKE_TAU_S`]).
pub const HARD_BRAKE_MPS2: f32 = 4.0;
const BRAKE_TAU_S: f32 = 0.2;
/// Faster than the posted speed times this plus [`SPEEDING_MPS`] is speeding.
pub const SPEEDING: f32 = 1.1;
pub const SPEEDING_MPS: f32 = 1.0;
/// Stop lines are this much wider each side for the crossing test, m (a car straddling
/// the lane line still passes).
const LINE_SLACK_M: f32 = 1.0;
/// The car must be driving within about 45 degrees of the approach's way.
const ALONG_COS: f32 = 0.7;
/// A step longer than this, m, is the car put somewhere else (a reset), not driving.
const JUMP_M: f32 = 5.0;

/// A signalized approach: its stop line's ends `(x, z)`, the way through it, one of its
/// lanes (for the light and the posted speed).
#[derive(Clone, Copy, Debug)]
struct Approach {
    a: [f32; 2],
    b: [f32; 2],
    dir: [f32; 2],
    lane: u32,
}

/// The run of greens (see the module docs).
#[derive(Clone, Debug)]
pub struct Streak {
    approaches: Vec<Approach>,
    /// Lights passed on green in the current run.
    pub run: u8,
    /// Every light that added to a run so far (a counter: the chime rings on each new one).
    pub greens: u32,
    /// The posted speed of the last light's road, m/s (0 before one).
    limit: f32,
    prev: Option<[f32; 2]>,
    prev_speed: f32,
    decel: f32,
}

impl Streak {
    /// For the signals of `graph`: one approach per signalized stop line.
    pub fn new(graph: &LaneGraph) -> Streak {
        let mut approaches: Vec<Approach> = Vec::new();
        let mut seen = alloc::vec![false; graph.stop_lines.len()];
        for (id, l) in graph.lanes.iter().enumerate() {
            if l.control != Control::Signal || seen[l.stop_line as usize] {
                continue;
            }
            seen[l.stop_line as usize] = true;
            let s = &graph.stop_lines[l.stop_line as usize];
            approaches.push(Approach {
                a: [s.a[0], s.a[2]],
                b: [s.b[0], s.b[2]],
                dir: graph.end_pose(id as u32).dir,
                lane: id as u32,
            });
        }
        Streak {
            approaches,
            run: 0,
            greens: 0,
            limit: 0.0,
            prev: None,
            prev_speed: 0.0,
            decel: 0.0,
        }
    }

    /// One sim step of `dt` s at signal tick `tick` with the player's car `car` (its
    /// speed signed, negative reversing). Returns the light it passed, if it passed one.
    pub fn step(&mut self, graph: &LaneGraph, tick: u64, car: &Player, dt: f32) -> Option<Light> {
        let pos = [car.pos[0], car.pos[2]];
        let speed = car.speed;
        let k = dt / (BRAKE_TAU_S + dt);
        self.decel += ((self.prev_speed - speed) / dt - self.decel) * k;
        self.prev_speed = speed;
        if speed < STOP_MPS
            || self.decel > HARD_BRAKE_MPS2
            || (self.limit > 0.0 && speed > self.limit * SPEEDING + SPEEDING_MPS)
        {
            self.run = 0;
        }
        let prev = self.prev.replace(pos);
        let prev = prev?;
        let (s, c) = sin_cos(car.heading);
        let fwd = [-s, -c];
        let step = [pos[0] - prev[0], pos[1] - prev[1]];
        if dot(step, fwd) <= 0.0 || dot(step, step) > JUMP_M * JUMP_M {
            return None;
        }
        let ap = *self
            .approaches
            .iter()
            .find(|ap| dot(ap.dir, fwd) > ALONG_COS && crosses(prev, pos, ap))?;
        let l = &graph.lanes[ap.lane as usize];
        let light = graph.light(ap.lane, tick)?;
        let speeding = speed > l.speed_mps * SPEEDING + SPEEDING_MPS;
        match light {
            Light::Green if !speeding => {
                self.run = self.run.saturating_add(1);
                self.greens = self.greens.wrapping_add(1);
                self.limit = l.speed_mps;
            }
            Light::Yellow if !speeding => {}
            _ => {
                self.run = 0;
                self.limit = 0.0;
            }
        }
        Some(light)
    }
}

/// Whether the step from `p` to `q` crosses `ap`'s stop line (widened by
/// [`LINE_SLACK_M`] each side) from before it to past it.
fn crosses(p: [f32; 2], q: [f32; 2], ap: &Approach) -> bool {
    let rel = |x: [f32; 2]| [x[0] - ap.a[0], x[1] - ap.a[1]];
    let (dp, dq) = (dot(rel(p), ap.dir), dot(rel(q), ap.dir));
    if !(dp < 0.0 && dq >= 0.0) {
        return false;
    }
    let t = dp / (dp - dq);
    let x = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
    let ab = [ap.b[0] - ap.a[0], ap.b[1] - ap.a[1]];
    let len = sqrt(dot(ab, ab));
    if len <= 0.0 {
        return false;
    }
    let along = dot(rel(x), ab) / len;
    along > -LINE_SLACK_M && along < len + LINE_SLACK_M
}
