//! Skywave vehicles: data and the simulation that drives it.
//!
//! A car is a [`VehicleSpec`] (a RON file in `assets/vehicles/`, layout in
//! `docs/VEHICLES.md`) plus a shape file. This crate holds the spec, its validator, the
//! derived quantities the sim reads from it, the raycast-car sim itself ([`sim`]) and
//! [`shape`], which lofts a shape file into meshes. It parses nothing itself: with the
//! default `serde` feature the spec types derive
//! `Serialize`/`Deserialize` and the `app` crate loads the RON text.
//!
//! Body space is right-handed and Y-up with the origin at the center of mass: `+x` is the
//! driver's right, `+y` up, `-z` forward (the same convention as the camera).
#![no_std]

extern crate alloc;

mod math;
pub mod shape;
pub mod sim;
mod spec;

pub use sim::{
    step, Assists, FlatGround, Ground, Hit, Rock, VehicleInput, VehicleState, WheelState,
};
pub use spec::{
    Aero, Brakes, Drive, Engine, Era, Feel, Gauge, Interior, SpecError, SpeedUnit, Steer, Susp,
    Tires, VehicleClass, VehicleSpec,
};
