//! Hill starts (`docs/CONTROLS_INPUT.md`, Brake/Reverse): every car, braked to a stop facing up a steep grade, stays in drive, pulls
//! away and climbs on full throttle; let off the brake it rolls back no more than a couple
//! of meters before it bites. San Francisco's steepest streets are about 31%.
#![cfg(feature = "serde")]

use vehicle::{step, Assists, Ground, Hit, VehicleInput, VehicleSpec, VehicleState};

const STEPS_PER_S: usize = 120;

/// A plane rising `grade` m per m toward +z: a car at heading pi faces uphill.
struct Slope {
    grade: f32,
}

impl Ground for Slope {
    fn probe(&self, origin: [f32; 3], dir: [f32; 3], max_len: f32) -> Option<Hit> {
        let l = (1.0 + self.grade * self.grade).sqrt();
        let n = [0.0, 1.0 / l, -self.grade / l];
        let d_n = dir[0] * n[0] + dir[1] * n[1] + dir[2] * n[2];
        if d_n > -1e-6 {
            return None;
        }
        let t = (origin[0] * n[0] + origin[1] * n[1] + origin[2] * n[2]) / -d_n;
        if t > max_len {
            return None;
        }
        Some(Hit {
            t: t.max(0.0),
            normal: n,
            friction: 1.0,
        })
    }
}

macro_rules! car {
    ($id:literal) => {
        include_str!(concat!("../../../assets/vehicles/", $id, ".ron"))
    };
}

const ROSTER: [&str; 27] = [
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
    car!("dunlin_roadster"),
    car!("palfrey_roadster"),
];

fn load(text: &str) -> VehicleSpec {
    let spec: VehicleSpec = ron::from_str(text).expect("spec parses");
    spec.validate().expect("spec validates");
    spec
}

fn input(throttle: f32, brake: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steer: 0.0,
        handbrake: false,
    }
}

/// Up `grade` from a stop on the brake: rolling uphill at 5 m/s, brake hard and hold 3 s
/// (the car must stop and stay in drive), then release it and floor the throttle (after
/// `gap` s of neither, as a foot moves across). Returns the furthest it rolled back, m, and
/// its uphill speed after 6 s, m/s.
fn hill_start(spec: &VehicleSpec, a: &Assists, grade: f32, gap: f32) -> (f32, f32) {
    let g = Slope { grade };
    // Rolling uphill at 5 m/s, on the ground.
    let mut s = VehicleState::spawn(spec, [0.0, 0.3, 0.0], std::f32::consts::PI);
    let l = (1.0 + grade * grade).sqrt();
    s.vel = [0.0, 5.0 * grade / l, 5.0 / l];
    for _ in 0..3 * STEPS_PER_S {
        step(spec, &mut s, &input(0.0, 1.0), a, &g);
    }
    assert!(
        s.gear > 0 && s.speed_mps.abs() < 0.1,
        "{} {grade} held at gear {} {}",
        spec.id,
        s.gear,
        s.speed_mps
    );
    let z0 = s.pos[2];
    let mut back = 0.0f32;
    let n_gap = (gap * STEPS_PER_S as f32) as usize;
    for k in 0..6 * STEPS_PER_S {
        let i = if k < n_gap {
            input(0.0, 0.0)
        } else {
            input(1.0, 0.0)
        };
        step(spec, &mut s, &i, a, &g);
        back = back.max(z0 - s.pos[2]);
    }
    let up = s.vel[2];
    (back, up)
}

/// The default (Arcade): every car, braked to a stop on a 30% street (San Francisco's
/// steepest), stays in drive and pulls firmly away; with half a second off both pedals it
/// rolls back about a meter before it climbs. Light, with half the hill help, climbs 25%.
#[test]
fn every_car_restarts_on_a_steep_hill() {
    for text in ROSTER {
        let spec = load(text);
        for grade in [0.2f32, 0.3] {
            let (b, u) = hill_start(&spec, &Assists::ARCADE, grade, 0.0);
            assert!(b < 0.2 && u > 3.5, "{} {grade}: back {b} up {u}", spec.id);
            let (b, u) = hill_start(&spec, &Assists::ARCADE, grade, 0.5);
            assert!(
                b < 1.5 && u > 1.8,
                "{} {grade} after a gap: back {b} up {u}",
                spec.id
            );
        }
        let (b, u) = hill_start(&spec, &Assists::LIGHT, 0.25, 0.5);
        assert!(b < 1.2 && u > 1.5, "{} light: back {b} up {u}", spec.id);
    }
}

/// Prints the table behind those bands.
#[test]
#[ignore]
fn hill_table() {
    for text in ROSTER {
        let spec = load(text);
        let mut line = format!("{:22}", spec.id);
        for grade in [0.2f32, 0.25, 0.3] {
            for gap in [0.0f32, 0.5] {
                let (b, u) = hill_start(&spec, &Assists::ARCADE, grade, gap);
                line += &format!(
                    " | {:>2.0}% gap {gap:.1}: back {b:4.1} up {u:5.1}",
                    grade * 100.0
                );
            }
        }
        println!("{line}");
    }
}
