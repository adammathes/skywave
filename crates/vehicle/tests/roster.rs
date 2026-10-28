//! Performance bands for the whole roster (`docs/VEHICLES.md`, "Launch roster", the 2001
//! hero cars and traffic cars), driven with the real specs from `assets/vehicles/`: every
//! car's 0–60 mph time in its band, all of them distinct in 0–60, top speed and skidpad
//! grip, the class orderings, the mid-engined exotics ahead of everything and the pony car
//! behind the other sports cars, the 2001 heroes' launches and the rotary's redline, and
//! nothing rolls at full lock or needs more than 60 m to stop. Run with `--nocapture` for
//! the table: `cargo test -p vehicle --test roster -- --nocapture`.
#![cfg(feature = "serde")]

use vehicle::sim::DT;
use vehicle::{step, Assists, FlatGround, VehicleClass, VehicleInput, VehicleSpec, VehicleState};

const MPH: f32 = 0.447_04;
const STEPS_PER_S: usize = 120;
const GROUND: FlatGround = FlatGround {
    y: 0.0,
    friction: 1.0,
};

macro_rules! car {
    ($id:literal) => {
        (
            $id,
            include_str!(concat!("../../../assets/vehicles/", $id, ".ron")),
        )
    };
}

/// A car as `(id, spec text)` and its 0–60 mph band in seconds.
type Entry = ((&'static str, &'static str), (f32, f32));

/// Every built-in car.
const ROSTER: [Entry; 35] = [
    (car!("gannet_gt"), (5.0, 8.0)),
    (car!("halcyon_turbo"), (6.0, 8.0)),
    (car!("pipit_hatch"), (8.0, 10.0)),
    (car!("buckboard_pickup"), (8.0, 10.0)),
    (car!("regent_coupe_cabrio"), (9.0, 12.0)),
    (car!("bancroft_sedan"), (9.5, 11.5)),
    (car!("prescott_landau"), (10.5, 12.5)),
    (car!("burro_4x4"), (11.0, 13.5)),
    (car!("seabreeze_convertible"), (11.0, 14.0)),
    (car!("homestead_wagon"), (10.0, 16.0)),
    (car!("drayman_van"), (12.5, 15.0)),
    (car!("meadow_minivan"), (14.0, 16.5)),
    (car!("basilisk_v12"), (4.0, 5.0)),
    (car!("basilisk_roadster"), (4.2, 5.2)),
    (car!("avocet_v8"), (4.6, 5.6)),
    (car!("avocet_spider"), (4.8, 5.8)),
    (car!("longbow_coupe"), (5.4, 6.6)),
    (car!("longbow_convertible"), (5.6, 6.8)),
    (car!("appaloosa_gt"), (5.8, 7.2)),
    (car!("appaloosa_convertible"), (6.0, 7.4)),
    // The 2001 heroes: the era's band is 4.5-6.5 s.
    (car!("halberd_coupe"), (5.5, 6.1)),
    (car!("halberd_targa"), (5.7, 6.3)),
    (car!("tanager_awd"), (5.0, 5.8)),
    (car!("destrier_coupe"), (5.9, 6.5)),
    (car!("siskin_coupe"), (6.1, 6.5)),
    (car!("glaive_turbo"), (5.1, 5.7)),
    (car!("shrike_coupe"), (5.4, 6.0)),
    (car!("falchion_sedan"), (5.9, 6.5)),
    (car!("courser_v8"), (6.0, 6.5)),
    // The 2001 traffic cars (the semi, which only traffic drives, is not driven here).
    (car!("plover_sedan"), (9.3, 10.3)),
    (car!("sorrel_utility"), (9.9, 10.9)),
    (car!("wren_minivan"), (10.5, 11.6)),
    (car!("percheron_pickup"), (7.9, 8.9)),
    (car!("linnet_hatch"), (9.1, 10.1)),
    (car!("sledge_box_truck"), (16.0, 19.0)),
];

/// The mid-engined exotics, which out-accelerate and outrun every other car, and the pony
/// cars, which every other sports car of the 1980s roster out-accelerates and outruns.
const EXOTICS: [&str; 4] = [
    "basilisk_v12",
    "basilisk_roadster",
    "avocet_v8",
    "avocet_spider",
];
const PONY: [&str; 2] = ["appaloosa_gt", "appaloosa_convertible"];
const SPORTS: [&str; 7] = [
    "basilisk_v12",
    "basilisk_roadster",
    "avocet_v8",
    "avocet_spider",
    "longbow_coupe",
    "longbow_convertible",
    "gannet_gt",
];

/// Smallest gaps between any two cars for them to count as distinct: six sim steps (the
/// Homestead and the Seabreeze, tuned before the roster, are 0.09 s apart), ~1.5 mph and
/// a hundredth of a g.
const MIN_GAP_S: f32 = 0.05;
const MIN_GAP_MPS: f32 = 0.7;
const MIN_GAP_G: f32 = 0.01;

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

/// Full throttle from rest for 150 s: seconds to 60 mph and the final (plateaued) speed.
fn straight_line(spec: &VehicleSpec) -> (f32, f32) {
    let mut s = spawn(spec);
    let mut t60 = f32::NAN;
    let mut at_120 = 0.0;
    for i in 0..150 * STEPS_PER_S {
        step(
            spec,
            &mut s,
            &input(1.0, 0.0, 0.0),
            &Assists::default(),
            &GROUND,
        );
        if t60.is_nan() && s.speed_mps >= 60.0 * MPH {
            t60 = (i + 1) as f32 * DT;
        }
        if i + 1 == 120 * STEPS_PER_S {
            at_120 = s.speed_mps;
        }
    }
    assert!(
        s.speed_mps - at_120 < 0.25,
        "{} has not plateaued: {at_120} -> {} m/s",
        spec.id,
        s.speed_mps
    );
    (t60, s.speed_mps)
}

/// Skidpad: steers on yaw rate to hold a 40 m circle while the target speed creeps up, until
/// an axle's mean slip passes the tire peak for half a second. Returns the highest
/// one-second-average lateral g reached while the car was holding the circle (the same
/// procedure as `tests/sim.rs`).
fn skidpad(spec: &VehicleSpec) -> f32 {
    const RADIUS: f32 = 40.0;
    let mut s = spawn(spec);
    let mut steer_i = 0.0;
    let mut lat_avg = 0.0;
    let mut best: f32 = 0.0;
    let mut holding_s = 0.0;
    let mut past_peak_s = 0.0;
    let mut target = 6.0;
    for _ in 0..120 * STEPS_PER_S {
        target += 0.2 * DT;
        let v = s.speed_mps;
        let throttle = ((target - v) * 0.5).clamp(0.0, 1.0);
        let want_yaw = -v / RADIUS;
        let up = s.up();
        let yaw = s.ang_vel[0] * up[0] + s.ang_vel[1] * up[1] + s.ang_vel[2] * up[2];
        let err = want_yaw - yaw;
        steer_i = (steer_i - 1.5 * err * DT).clamp(-1.0, 1.0);
        let steer = (steer_i - 0.6 * err).clamp(-1.0, 1.0);
        step(
            spec,
            &mut s,
            &input(throttle, 0.0, steer),
            &Assists::OFF,
            &GROUND,
        );
        lat_avg += (s.lat_g - lat_avg) * DT;
        holding_s = if err.abs() < 0.05 * want_yaw.abs() {
            holding_s + DT
        } else {
            0.0
        };
        if holding_s >= 1.0 {
            best = best.max(lat_avg);
        }
        let w = &s.wheels;
        let front = 0.5 * (w[0].slip + w[1].slip);
        let rear = 0.5 * (w[2].slip + w[3].slip);
        past_peak_s = if best > 0.0 && front.max(rear) > 1.0 {
            past_peak_s + DT
        } else {
            0.0
        };
        if past_peak_s > 0.5 {
            break;
        }
    }
    best
}

struct Measured {
    id: &'static str,
    class: VehicleClass,
    t60: f32,
    top: f32,
    grip: f32,
}

fn measure() -> Vec<Measured> {
    ROSTER
        .iter()
        .map(|&((id, text), band)| {
            let spec = load(text);
            assert_eq!(spec.id, id);
            let (t60, top) = straight_line(&spec);
            let grip = skidpad(&spec);
            println!(
                "{id:22} {:12} 0-60 mph {t60:5.2} s   top {top:4.1} m/s ({:3.0} mph)   skidpad {grip:.3} g",
                format!("{:?}", spec.class),
                top / MPH
            );
            assert!(
                t60 >= band.0 && t60 <= band.1,
                "{id} 0-60 mph {t60} s outside {band:?}"
            );
            assert!(top < 95.0, "{id} top speed {top} m/s");
            assert!(grip > 0.5 && grip < 1.6, "{id} skidpad {grip} g");
            Measured {
                id,
                class: spec.class,
                t60,
                top,
                grip,
            }
        })
        .collect()
}

/// Asserts every pair of `value`s is at least `gap` apart.
fn assert_distinct(m: &[Measured], what: &str, gap: f32, value: impl Fn(&Measured) -> f32) {
    for (i, a) in m.iter().enumerate() {
        for b in &m[i + 1..] {
            let (va, vb) = (value(a), value(b));
            assert!(
                (va - vb).abs() >= gap,
                "{what}: {} ({va}) and {} ({vb}) are too close",
                a.id,
                b.id
            );
        }
    }
}

/// Asserts every car of each class in `order` beats every car of the classes after it.
fn assert_ordered(
    m: &[Measured],
    what: &str,
    order: &[VehicleClass],
    value: impl Fn(&Measured) -> f32,
) {
    for (k, &hi) in order.iter().enumerate() {
        for &lo in &order[k + 1..] {
            for a in m.iter().filter(|c| c.class == hi) {
                for b in m.iter().filter(|c| c.class == lo) {
                    assert!(
                        value(a) > value(b),
                        "{what}: {} ({:?}, {}) should beat {} ({:?}, {})",
                        a.id,
                        hi,
                        value(a),
                        b.id,
                        lo,
                        value(b)
                    );
                }
            }
        }
    }
}

#[test]
fn every_car_is_distinct_and_the_classes_are_ordered() {
    use VehicleClass::*;
    let m = measure();
    assert_distinct(&m, "0-60 mph", MIN_GAP_S, |c| c.t60);
    assert_distinct(&m, "top speed", MIN_GAP_MPS, |c| c.top);
    assert_distinct(&m, "skidpad", MIN_GAP_G, |c| c.grip);
    assert_ordered(&m, "top speed", &[Sports, Coupe, Sedan, Wagon, Van], |c| {
        c.top
    });
    assert_ordered(&m, "skidpad", &[Sports, Hatch, Sedan, Wagon, Van], |c| {
        c.grip
    });
    // (fast, slow) pairs: every exotic beats every other car, every other sports car beats
    // the pony cars.
    let pairs = m.iter().flat_map(|a| m.iter().map(move |b| (a, b)));
    for (a, b) in pairs {
        let ahead = (EXOTICS.contains(&a.id) && !EXOTICS.contains(&b.id))
            || (SPORTS.contains(&a.id) && PONY.contains(&b.id));
        if ahead {
            assert!(a.t60 < b.t60, "0-60 mph: {} should beat {}", a.id, b.id);
            assert!(a.top > b.top, "top speed: {} should beat {}", a.id, b.id);
        }
    }
}

#[test]
fn nothing_rolls_at_full_lock_and_everything_stops() {
    for &((id, text), _) in &ROSTER {
        let spec = load(text);
        for assists in [Assists::ARCADE, Assists::OFF] {
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
            assert!(min_up > 0.5, "{id} rolled: up.y {min_up}");
        }

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
        println!("{id:22} 60-0 mph without assists {d:.1} m");
        assert!(d < 60.0, "{id} stops in {d} m");
    }
}

/// Full throttle from rest for `secs`: the peak slip ratio of any driven wheel once it is
/// rolling (past 1 m/s, so a stopped car's first creep does not count).
fn launch_slip(spec: &VehicleSpec, assists: &Assists, secs: f32) -> f32 {
    let driven: &[usize] = match spec.drive {
        vehicle::Drive::Front => &[0, 1],
        vehicle::Drive::Rear => &[2, 3],
        vehicle::Drive::All => &[0, 1, 2, 3],
    };
    let mut s = spawn(spec);
    let mut peak: f32 = 0.0;
    for _ in 0..(secs * STEPS_PER_S as f32) as usize {
        step(spec, &mut s, &input(1.0, 0.0, 0.0), assists, &GROUND);
        if s.speed_mps > 1.0 {
            for &k in driven {
                peak = peak.max(s.wheels[k].slip_ratio);
            }
        }
    }
    peak
}

/// The 2001 heroes' launches: the blown muscle coupe lights up its rear tires off the line
/// whatever the assists (more than any other car), and so does the V8 sports car, a little
/// less; the all-wheel-drive coupe and rally sedan leave without a chirp even with every
/// assist off.
#[test]
fn the_muscle_cars_wheelspin_and_the_awd_cars_launch() {
    let presets = [Assists::OFF, Assists::LIGHT, Assists::ARCADE];
    let slips: Vec<(&str, [f32; 3])> = ROSTER
        .iter()
        .map(|&((id, text), _)| {
            let spec = load(text);
            let s = presets.map(|a| launch_slip(&spec, &a, 1.5));
            println!(
                "{id:22} launch slip: off {:.3}  light {:.3}  arcade {:.3}",
                s[0], s[1], s[2]
            );
            (id, s)
        })
        .collect();
    let of = |id: &str| slips.iter().find(|c| c.0 == id).unwrap().1;
    let (muscle, v8) = (of("destrier_coupe"), of("courser_v8"));
    let (awd, rally) = (of("tanager_awd"), of("glaive_turbo"));
    for k in 0..presets.len() {
        assert!(muscle[k] > 0.5, "the muscle coupe hooks up: {muscle:?}");
        assert!(v8[k] > 0.5, "the V8 sports car hooks up: {v8:?}");
        assert!(awd[k] < 0.1, "the AWD coupe spins its tires: {awd:?}");
        assert!(rally[k] < 0.1, "the rally sedan spins its tires: {rally:?}");
        for (id, s) in &slips {
            if *id != "destrier_coupe" {
                assert!(muscle[k] > s[k], "{id} spins more than the muscle coupe");
            }
        }
    }
}

/// The rotary revs past every other engine on the roster.
#[test]
fn the_rotary_revs_highest() {
    let rotary = load(car!("shrike_coupe").1);
    for &((id, text), _) in &ROSTER {
        if id != "shrike_coupe" {
            assert!(
                load(text).engine.redline_rpm < rotary.engine.redline_rpm,
                "{id}"
            );
        }
    }
}
