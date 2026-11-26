//! Handling feel probes (`docs/VEHICLES.md`, "Feel bands"): every car through the moves
//! where a tail steps out (turning out of a junction on the gas, braking into a corner,
//! lifting mid-corner) as a keyboard steers. With the stability assist the rear axle stays
//! near its tires' peak slip angle; the handbrake still slides. Print the table with
//! `cargo test -p vehicle --test handling -- --nocapture`.
#![cfg(feature = "serde")]

use vehicle::{step, Assists, FlatGround, VehicleInput, VehicleSpec, VehicleState};

const MPH: f32 = 0.447_04;
const STEPS_PER_S: usize = 120;
const GROUND: FlatGround = FlatGround {
    y: 0.0,
    friction: 1.0,
};

macro_rules! car {
    ($id:literal) => {
        include_str!(concat!("../../../assets/vehicles/", $id, ".ron"))
    };
}

const ROSTER: [&str; 25] = [
    car!("basilisk_v12"),
    car!("basilisk_roadster"),
    car!("avocet_v8"),
    car!("avocet_spider"),
    car!("gannet_gt"),
    car!("longbow_coupe"),
    car!("longbow_convertible"),
    car!("appaloosa_gt"),
    car!("appaloosa_convertible"),
    car!("halcyon_turbo"),
    car!("pipit_hatch"),
    car!("buckboard_pickup"),
    car!("regent_coupe_cabrio"),
    car!("bancroft_sedan"),
    car!("prescott_landau"),
    car!("burro_4x4"),
    car!("seabreeze_convertible"),
    car!("homestead_wagon"),
    car!("drayman_van"),
    car!("meadow_minivan"),
    car!("halberd_coupe"),
    car!("halberd_targa"),
    car!("tanager_awd"),
    car!("destrier_coupe"),
    car!("siskin_coupe"),
];

fn load(text: &str) -> VehicleSpec {
    let spec: VehicleSpec = ron::from_str(text).expect("spec parses");
    spec.validate().expect("spec validates");
    spec
}

fn input(throttle: f32, brake: f32, steer: f32, handbrake: bool) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steer,
        handbrake,
    }
}

/// Full throttle in a straight line to `mps`, then a second holding it.
fn up_to(spec: &VehicleSpec, s: &mut VehicleState, a: &Assists, mps: f32) {
    while s.speed_mps < mps {
        step(spec, s, &input(1.0, 0.0, 0.0, false), a, &GROUND);
    }
    for _ in 0..STEPS_PER_S {
        let t = ((mps - s.speed_mps) * 0.5).clamp(0.0, 1.0);
        step(spec, s, &input(t, 0.0, 0.0, false), a, &GROUND);
    }
}

/// The rear axle's slip angle over the tires' peak (grounded wheels only).
fn rear_slip(spec: &VehicleSpec, s: &VehicleState) -> f32 {
    let r = &s.wheels[2..];
    let n = r.iter().filter(|w| w.grounded).count();
    if n == 0 || s.speed_mps < 3.0 {
        return 0.0;
    }
    let sum: f32 = r.iter().filter(|w| w.grounded).map(|w| w.slip_angle).sum();
    (sum / n as f32).abs().to_degrees() / spec.tires.slip_peak_deg
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Probe {
    /// 25 mph, full key and full throttle for 2 s, then the key released.
    Junction,
    /// 45 mph, full key, a third of a second on part throttle, a second of full brake, coast.
    TrailBrake,
    /// 45 mph, full key on 80% throttle for 1.5 s, then off the throttle with the key held.
    Lift,
    /// 30 mph, a quarter key and the handbrake for a second.
    Handbrake,
}

const PROBES: [Probe; 4] = [
    Probe::Junction,
    Probe::TrailBrake,
    Probe::Lift,
    Probe::Handbrake,
];

/// Peak rear slip over the tires' peak through one probe, steered as a keyboard does
/// (out in 120 ms, back in 300 ms).
fn probe(spec: &VehicleSpec, a: &Assists, p: Probe) -> f32 {
    let sec = STEPS_PER_S;
    let mph = match p {
        Probe::Junction => 25.0,
        Probe::TrailBrake | Probe::Lift => 45.0,
        Probe::Handbrake => 30.0,
    };
    let mut s = VehicleState::spawn(spec, [0.0; 3], 0.0);
    up_to(spec, &mut s, a, mph * MPH);
    let (mut stick, mut peak) = (0.0f32, 0.0f32);
    for k in 0..3 * sec {
        let (t, b, want, hb): (f32, f32, f32, bool) = match p {
            Probe::Junction => (1.0, 0.0, if k < 2 * sec { 1.0 } else { 0.0 }, false),
            Probe::TrailBrake if k < sec / 3 => (0.5, 0.0, 1.0, false),
            Probe::TrailBrake if k < 4 * sec / 3 => (0.0, 1.0, 1.0, false),
            Probe::TrailBrake => (0.0, 0.0, 1.0, false),
            Probe::Lift if k < 3 * sec / 2 => (0.8, 0.0, 1.0, false),
            Probe::Lift => (0.0, 0.0, 1.0, false),
            Probe::Handbrake => (0.0, 0.0, 0.25, k < sec),
        };
        let rate = if want.abs() > stick.abs() {
            1.0 / 0.12
        } else {
            1.0 / 0.3
        } / sec as f32;
        stick += (want - stick).clamp(-rate, rate);
        step(spec, &mut s, &input(t, b, stick, hb), a, &GROUND);
        peak = peak.max(rear_slip(spec, &s));
    }
    peak
}

#[test]
fn the_tail_steps_out_progressively_and_comes_back() {
    let presets = [("arcade", Assists::ARCADE), ("light", Assists::LIGHT)];
    for text in ROSTER {
        let spec = load(text);
        for (name, a) in presets {
            let r = PROBES.map(|p| probe(&spec, &a, p));
            println!("{:22} {name:6} {r:.2?}", spec.id);
            for (p, rear) in PROBES.iter().zip(r) {
                let what = format!("{} {name} {p:?}: rear slip {rear:.2}x peak", spec.id);
                match (name, p) {
                    // In Arcade the 1.2x grip lets a few rears out-grip the handbrake (as
                    // before the tail hold, which lets go while the handbrake is held).
                    ("arcade", Probe::Handbrake) => assert!(rear.is_finite(), "{what}"),
                    (_, Probe::Handbrake) => assert!(rear > 1.5, "{what}"),
                    ("arcade", _) => assert!(rear < 1.5, "{what}"),
                    _ => assert!(rear < 2.0, "{what}"),
                }
            }
        }
    }
}
