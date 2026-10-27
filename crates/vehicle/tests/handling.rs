//! Handling feel probes for the whole roster (`docs/VEHICLES.md`, "Feel bands"): every car
//! in every assist preset through four scripted maneuvers, measuring the peak rear slip
//! angle, the peak yaw rate past the rate the steering asks for, and whether the car spins.
//! The rear should step out only when provoked, progressively, and be catchable; in Arcade
//! it should be hard to spin at all; the handbrake must still slide. Run with `--nocapture`
//! for the table: `cargo test -p vehicle --test handling -- --nocapture`.
#![cfg(feature = "serde")]

use vehicle::{step, Assists, FlatGround, VehicleInput, VehicleSpec, VehicleState};

const MPH: f32 = 0.447_04;
const STEPS_PER_S: usize = 120;
const GROUND: FlatGround = FlatGround {
    y: 0.0,
    friction: 1.0,
};
/// Sideslip (heading against the direction of travel) past which a car has spun, degrees.
const SPIN_DEG: f32 = 60.0;

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

const PRESETS: [(&str, Assists); 3] = [
    ("arcade", Assists::ARCADE),
    ("light", Assists::LIGHT),
    ("off", Assists::OFF),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Probe {
    /// 60 mph, one sine of full stick over 1.5 s (out of the lane and back, as a keyboard
    /// steers), throttle held.
    LaneChange,
    /// 40 mph, half stick, then the throttle to the floor through the apex.
    Corner,
    /// 25 mph, full stick (a junction turn on the keyboard), then the throttle to the floor.
    Junction,
    /// From rest, a quarter stick and full throttle to 30 mph.
    Launch,
    /// 30 mph, then a quarter stick and the handbrake for a second, off the throttle.
    Handbrake,
}

const PROBES: [Probe; 5] = [
    Probe::LaneChange,
    Probe::Corner,
    Probe::Junction,
    Probe::Launch,
    Probe::Handbrake,
];

/// What a probe measured: peak rear slip angle (degrees), peak yaw rate past the
/// steering's intended rate (degrees per second) and the peak sideslip (degrees).
#[derive(Clone, Copy, Debug, Default)]
struct Feel {
    rear_slip_deg: f32,
    overshoot_dps: f32,
    sideslip_deg: f32,
}

impl Feel {
    fn spun(&self) -> bool {
        self.sideslip_deg > SPIN_DEG
    }
}

fn load(text: &str) -> VehicleSpec {
    let spec: VehicleSpec = ron::from_str(text).expect("spec parses");
    spec.validate().expect("spec validates");
    spec
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Folds this step's state into `feel`.
fn observe(spec: &VehicleSpec, assists: &Assists, s: &VehicleState, feel: &mut Feel) {
    // The rear axle's slip angle: the mean of its grounded wheels, so a lifted inside wheel
    // spinning in the air does not count as the tail stepping out.
    let rear = &s.wheels[2..];
    let n = rear.iter().filter(|w| w.grounded).count();
    if n > 0 {
        let sum: f32 = rear
            .iter()
            .filter(|w| w.grounded)
            .map(|w| w.slip_angle)
            .sum();
        feel.rear_slip_deg = feel.rear_slip_deg.max((sum / n as f32).abs().to_degrees());
    }
    let (fwd, right, up) = (s.forward(), s.right(), s.up());
    let v_f = dot(s.vel, fwd);
    let v_r = dot(s.vel, right);
    let speed = (v_f * v_f + v_r * v_r).sqrt();
    if speed < 3.0 {
        return;
    }
    feel.sideslip_deg = feel.sideslip_deg.max(v_r.atan2(v_f).abs().to_degrees());
    // The yaw rate the road wheels ask for (bicycle model, within what the tires can hold),
    // as the stability assist sees it. Rotating against the wheels counts in full.
    let yaw = dot(s.ang_vel, up);
    let d = s.steer_angle_rad;
    let limit = spec.tires.grip_lat * assists.grip_boost * 9.81 / speed;
    let wanted = (-v_f * d.sin() / (d.cos() * spec.wheelbase_m)).clamp(-limit, limit);
    let over = if yaw * wanted < 0.0 {
        yaw.abs()
    } else {
        yaw.abs() - wanted.abs()
    };
    feel.overshoot_dps = feel.overshoot_dps.max(over.to_degrees());
}

fn input(throttle: f32, steer: f32, handbrake: bool) -> VehicleInput {
    VehicleInput {
        throttle,
        brake: 0.0,
        steer,
        handbrake,
    }
}

/// Full throttle in a straight line to `mps`, then holds it for a second to settle.
fn up_to(spec: &VehicleSpec, s: &mut VehicleState, assists: &Assists, mps: f32) {
    while s.speed_mps < mps {
        step(spec, s, &input(1.0, 0.0, false), assists, &GROUND);
    }
    for _ in 0..STEPS_PER_S {
        let throttle = ((mps - s.speed_mps) * 0.5).clamp(0.0, 1.0);
        step(spec, s, &input(throttle, 0.0, false), assists, &GROUND);
    }
}

fn probe(spec: &VehicleSpec, assists: &Assists, p: Probe) -> Feel {
    let mut s = VehicleState::spawn(spec, [0.0; 3], 0.0);
    let mut feel = Feel::default();
    let run = |s: &mut VehicleState, i: VehicleInput, feel: &mut Feel| {
        step(spec, s, &i, assists, &GROUND);
        observe(spec, assists, s, feel);
    };
    match p {
        Probe::LaneChange => {
            up_to(spec, &mut s, assists, 60.0 * MPH);
            let n = 3 * STEPS_PER_S / 2;
            for k in 0..n {
                let phase = std::f32::consts::TAU * k as f32 / n as f32;
                run(&mut s, input(1.0, phase.sin(), false), &mut feel);
            }
            for _ in 0..3 * STEPS_PER_S {
                run(&mut s, input(1.0, 0.0, false), &mut feel);
            }
        }
        Probe::Corner | Probe::Junction => {
            let (mph, stick) = if p == Probe::Corner {
                (40.0, 0.5)
            } else {
                (25.0, 1.0)
            };
            up_to(spec, &mut s, assists, mph * MPH);
            for k in 0..STEPS_PER_S {
                let steer = stick * (k as f32 / (0.3 * STEPS_PER_S as f32)).min(1.0);
                let throttle = ((mph * MPH - s.speed_mps) * 0.5).clamp(0.0, 1.0);
                run(&mut s, input(throttle, steer, false), &mut feel);
            }
            for _ in 0..3 * STEPS_PER_S {
                run(&mut s, input(1.0, stick, false), &mut feel);
            }
        }
        Probe::Launch => {
            // A launch, not a skidpad: at most 6 s, so a car that cannot reach 30 mph on a
            // quarter lock is not held at its limit until it gives up.
            let mut t = 0;
            while s.speed_mps < 30.0 * MPH && t < 6 * STEPS_PER_S {
                run(&mut s, input(1.0, 0.25, false), &mut feel);
                t += 1;
            }
        }
        Probe::Handbrake => {
            up_to(spec, &mut s, assists, 30.0 * MPH);
            for _ in 0..STEPS_PER_S {
                run(&mut s, input(0.0, 0.25, true), &mut feel);
            }
        }
    }
    feel
}

/// Every car × preset × probe, printed as a table.
fn measure() -> Vec<(String, &'static str, Probe, Feel, f32)> {
    let mut out = Vec::new();
    println!(
        "{:22} {:6} | {:>20} | {:>20} | {:>20} | {:>20} | {:>20}",
        "car",
        "assist",
        "lane change 60",
        "corner 40 + gas",
        "junction 25 + gas",
        "launch 1/4 lock",
        "handbrake 30"
    );
    for text in ROSTER {
        let spec = load(text);
        let peak = spec.tires.slip_peak_deg;
        for (name, assists) in PRESETS {
            let mut line = format!("{:22} {:6}", spec.id, name);
            for p in PROBES {
                let f = probe(&spec, &assists, p);
                line += &format!(
                    " | {:5.1}° {:5.1}°/s {:5.1}°{}",
                    f.rear_slip_deg,
                    f.overshoot_dps,
                    f.sideslip_deg,
                    if f.spun() { "!" } else { " " }
                );
                out.push((spec.id.clone(), name, p, f, peak));
            }
            println!("{line}");
        }
    }
    out
}

/// The sports and muscle cars, which may stay lively with no assists (a choice: the
/// "driver" preset is for drivers).
const LIVELY: [&str; 14] = [
    "basilisk_v12",
    "basilisk_roadster",
    "avocet_v8",
    "avocet_spider",
    "gannet_gt",
    "longbow_coupe",
    "longbow_convertible",
    "appaloosa_gt",
    "appaloosa_convertible",
    "halcyon_turbo",
    "halberd_coupe",
    "halberd_targa",
    "tanager_awd",
    "destrier_coupe",
];

#[test]
fn the_rear_steps_out_only_when_provoked() {
    for (id, preset, p, f, peak) in measure() {
        let what = format!("{id} {preset} {p:?}: {f:?} (tire peak {peak}°)");
        let rear = f.rear_slip_deg / peak;
        match (preset, p) {
            // Arcade: the rear stays at its tires' peak through a lane change and a corner
            // under power, little yaw past what the wheels ask for, and nothing spins.
            ("arcade", Probe::LaneChange) => {
                assert!(
                    rear < 1.1 && f.overshoot_dps < 25.0 && f.sideslip_deg < 8.0,
                    "{what}"
                );
            }
            ("arcade", Probe::Corner) => assert!(rear < 1.1 && f.overshoot_dps < 5.0, "{what}"),
            // A keyboard junction turn under full throttle steps the tail out a little.
            ("arcade", Probe::Junction) => assert!(rear < 1.6 && !f.spun(), "{what}"),
            // Light: a little more, still caught.
            ("light", Probe::LaneChange) => {
                assert!(
                    rear < 1.25 && f.overshoot_dps < 32.0 && f.sideslip_deg < 8.0,
                    "{what}"
                );
            }
            ("light", Probe::Corner) => assert!(rear < 1.25 && f.overshoot_dps < 10.0, "{what}"),
            // The Destrier is built to wag its tail out of slow corners (its spec's intent):
            // in Light it may slide further, but it still never spins.
            ("light", Probe::Junction) => {
                let limit = if id == "destrier_coupe" { 4.0 } else { 2.9 };
                assert!(rear < limit && !f.spun(), "{what}");
            }
            // With traction control a launch at a quarter lock is a little wheelspin; without
            // it only the sports and muscle cars may swing round.
            ("arcade" | "light", Probe::Launch) => assert!(rear < 1.0 && !f.spun(), "{what}"),
            ("off", Probe::Launch) => assert!(LIVELY.contains(&id.as_str()) || !f.spun(), "{what}"),
            // The handbrake slides the rear past its peak in every preset, well past it with
            // the pivot of Arcade.
            ("arcade", Probe::Handbrake) => assert!(rear > 2.0, "{what}"),
            (_, Probe::Handbrake) => assert!(rear > 1.5, "{what}"),
            _ => assert!(f.rear_slip_deg.is_finite(), "{what}"),
        }
    }
}
