//! Performance bands and handling checks for the Convertible class (`docs/VEHICLES.md`),
//! driven with the real specs from `assets/vehicles/`. Run with `--nocapture` to read the
//! measured numbers: `cargo test -p vehicle --test convertibles -- --nocapture`.
#![cfg(feature = "serde")]

use vehicle::{
    step, Assists, Drive, FlatGround, VehicleClass, VehicleInput, VehicleSpec, VehicleState,
};

const SEABREEZE: &str = include_str!("../../../assets/vehicles/seabreeze_convertible.ron");
const REGENT: &str = include_str!("../../../assets/vehicles/regent_coupe_cabrio.ron");
const MPH: f32 = 0.447_04;
const STEPS_PER_S: usize = 120;
const GROUND: FlatGround = FlatGround {
    y: 0.0,
    friction: 1.0,
};

fn load(text: &str) -> VehicleSpec {
    let spec: VehicleSpec = ron::from_str(text).expect("spec parses");
    spec.validate().expect("spec validates");
    spec
}

fn input(throttle: f32, brake: f32, steer: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steer,
        handbrake: false,
    }
}

fn spawn(spec: &VehicleSpec) -> VehicleState {
    VehicleState::spawn(spec, [0.0, 0.0, 0.0], 0.0)
}

/// Seconds from rest to 60 mph at full throttle, and the speed after 120 s.
fn zero_to_sixty(spec: &VehicleSpec) -> (f32, f32) {
    let mut s = spawn(spec);
    let mut t60 = f32::NAN;
    for i in 0..120 * STEPS_PER_S {
        step(
            spec,
            &mut s,
            &input(1.0, 0.0, 0.0),
            &Assists::default(),
            &GROUND,
        );
        if t60.is_nan() && s.speed_mps >= 60.0 * MPH {
            t60 = (i + 1) as f32 / STEPS_PER_S as f32;
        }
    }
    (t60, s.speed_mps)
}

#[test]
fn specs_are_front_drive_convertibles() {
    for (text, mass, gears) in [(SEABREEZE, 1250.0, 3), (REGENT, 1650.0, 4)] {
        let spec = load(text);
        assert_eq!(spec.class, VehicleClass::Convertible, "{}", spec.id);
        assert_eq!(spec.drive, Drive::Front, "{}", spec.id);
        assert!(spec.auto, "{}", spec.id);
        assert_eq!(spec.gear_count(), gears, "{}", spec.id);
        assert!((spec.mass_kg - mass).abs() < 1.0, "{}", spec.id);
        assert!(spec.feel.oversteer_bias < 0.0, "{} understeers", spec.id);
    }
}

#[test]
fn zero_to_sixty_bands() {
    let mut tops = [0.0; 2];
    for (k, (text, band)) in [(SEABREEZE, (11.0, 14.0)), (REGENT, (9.0, 12.0))]
        .into_iter()
        .enumerate()
    {
        let spec = load(text);
        let (t60, top) = zero_to_sixty(&spec);
        tops[k] = top;
        println!(
            "{:22} 0-60 mph {t60:.2} s, top speed {top:.1} m/s ({:.0} mph)",
            spec.id,
            top / MPH
        );
        assert!(
            t60 >= band.0 && t60 <= band.1,
            "{} 0-60 mph {t60} s outside {band:?}",
            spec.id
        );
    }
    assert!(tops[1] > tops[0], "the big V8 is faster flat out: {tops:?}");
}

#[test]
fn full_lock_at_thirty_does_not_flip() {
    for text in [SEABREEZE, REGENT] {
        let spec = load(text);
        for assists in [Assists::LIGHT, Assists::OFF] {
            let mut s = spawn(&spec);
            while s.speed_mps < 30.0 * MPH {
                step(&spec, &mut s, &input(1.0, 0.0, 0.0), &assists, &GROUND);
            }
            let mut min_up: f32 = 1.0;
            for _ in 0..5 * STEPS_PER_S {
                let throttle = ((30.0 * MPH - s.speed_mps) * 0.3).clamp(0.0, 1.0);
                step(&spec, &mut s, &input(throttle, 0.0, 1.0), &assists, &GROUND);
                min_up = min_up.min(s.up()[1]);
            }
            assert!(min_up > 0.5, "{} rolled: up.y {min_up}", spec.id);
        }
    }
}

#[test]
fn the_big_coupe_leans_more_and_stops_within_sixty_metres() {
    let mut lean = [0.0; 2];
    for (k, text) in [SEABREEZE, REGENT].into_iter().enumerate() {
        let spec = load(text);
        let mut s = spawn(&spec);
        while s.speed_mps < 12.0 {
            step(
                &spec,
                &mut s,
                &input(1.0, 0.0, 0.0),
                &Assists::default(),
                &GROUND,
            );
        }
        for _ in 0..3 * STEPS_PER_S {
            step(
                &spec,
                &mut s,
                &input(0.2, 0.0, 0.35),
                &Assists::default(),
                &GROUND,
            );
        }
        lean[k] = s.right()[1].asin().to_degrees() / s.lat_g.max(0.05);

        let mut s = spawn(&spec);
        while s.speed_mps < 60.0 * MPH {
            step(&spec, &mut s, &input(1.0, 0.0, 0.0), &Assists::OFF, &GROUND);
        }
        let start = s.pos;
        let mut stopped = None;
        for _ in 0..20 * STEPS_PER_S {
            step(&spec, &mut s, &input(0.0, 1.0, 0.0), &Assists::OFF, &GROUND);
            if s.speed_mps < 0.05 {
                let (dx, dz) = (s.pos[0] - start[0], s.pos[2] - start[2]);
                stopped = Some((dx * dx + dz * dz).sqrt());
                break;
            }
        }
        let d = stopped.expect("stops");
        println!(
            "{:22} roll {:.2} deg/g, 60-0 mph {d:.1} m",
            spec.id, lean[k]
        );
        assert!(d < 60.0, "{} stops in {d} m", spec.id);
    }
    assert!(lean[1] > lean[0], "roll gradients {lean:?}");
}
