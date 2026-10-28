//! Skywave traffic over a place's roads (`docs/NPCS.md`).
//!
//! The lane graph ([`LaneGraph`]): built once from a `place::Place`'s road graph, it
//! holds a centerline per road lane in its driving direction, a turn table per junction
//! (which incoming lane may continue onto which outgoing lane, each with its curve across
//! the junction), stop lines where traffic stops, and a fixed-cycle signal timer per
//! signalized junction ([`signal`]). The queries are what car following needs: a pose at a
//! distance along a lane or turn, the turns leaving a lane, the distance to its stop line and
//! the light it faces.
//!
//! Plain structs and flat arrays: lanes index into one point pool, turns are stored grouped
//! by the lane they leave. Building allocates; queries never do. Everything is a pure
//! function of the place, so the same place gives a bit-identical graph everywhere.
//!
//! The pedestrian figure ([`figure`]): seeded low-poly people, their walk cycle and the
//! poses for stepping out of a car's way and sitting down, posed as one rigid transform
//! per part.
//!
//! Pedestrians: the sidewalk graph ([`walks`], walks along both sides of every road with
//! sidewalks, corners, crossings gated by the lane graph's signals, and the spots people
//! go to) and the people on it ([`peds`]: walking in ones, twos and threes, waiting at the
//! curb for the walk phase, staying at shop windows, benches and landmarks, stepping
//! aside for the player's car, spawned and despawned out of view by density).
//!
//! NPC cars drive on the lane graph ([`Traffic`], [`cars`]): a fixed pool stepped at 120 Hz with car
//! following, signals and stop signs, right of way, lane changes, and spawning by density
//! out of the player's view.
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod cars;
pub mod figure;
pub mod lanes;
pub mod math;
pub mod peds;
pub mod race;
pub mod signal;
pub mod train;
pub mod walks;

pub use cars::{
    Barrier, Car, Clock, Density, Inputs, Player, Script, ShapeRule, Traffic, TrafficBudget, View,
};
pub use lanes::{Control, Junction, Lane, LaneGraph, Pose, Turn, TurnKind, NONE};
pub use signal::{Light, SignalClock};
