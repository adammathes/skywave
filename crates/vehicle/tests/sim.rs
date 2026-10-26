//! Acceptance tests for `vehicle::sim` (`docs/VEHICLES.md`, "Acceptance tests"), driven with
//! the real specs from `assets/vehicles/`. Run with `--nocapture` to read the measured
//! numbers: `cargo test -p vehicle --test sim -- --nocapture`. The specs are parsed through
//! serde, so these tests need the (default) `serde` feature.
#![cfg(feature = "serde")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use vehicle::sim::{steer_target, DT, REVERSE_MAX_MPS};
use vehicle::{step, Assists, FlatGround, VehicleInput, VehicleSpec, VehicleState};

const CORSAIR: &str = include_str!("../../../assets/vehicles/corsair_gtx.ron");
const PALISADE: &str = include_str!("../../../assets/vehicles/palisade_wagon.ron");
const MPH: f32 = 0.447_04;
const STEPS_PER_S: usize = 120;
const GROUND: FlatGround = FlatGround {
    y: 0.0,
    friction: 1.0,
};

/// Counts heap allocations made on the current thread, so `step` can be shown to make none
/// while other tests allocate in parallel.
struct CountingAlloc;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn allocations() -> usize {
    ALLOCS.with(|n| n.get())
}

fn load(text: &str) -> VehicleSpec {
    let spec: VehicleSpec = ron::from_str(text).expect("spec parses");
    spec.validate().expect("spec validates");
    spec
}

fn cars() -> [VehicleSpec; 2] {
    [load(CORSAIR), load(PALISADE)]
}

fn spawn(spec: &VehicleSpec) -> VehicleState {
    VehicleState::spawn(spec, [0.0, 0.0, 0.0], 0.0)
}

fn input(throttle: f32, brake: f32, steer: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steer,
        handbrake: false,
    }
}

fn len(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn horizontal_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dz) = (a[0] - b[0], a[2] - b[2]);
    (dx * dx + dz * dz).sqrt()
}

fn assert_finite(s: &VehicleState) {
    let all = s.pos.iter().chain(&s.rot).chain(&s.vel).chain(&s.ang_vel);
    assert!(all.clone().all(|v| v.is_finite()), "non-finite state {s:?}");
}

#[test]
fn hands_off_cars_settle_and_stay_put() {
    for spec in cars() {
        let mut s = spawn(&spec);
        let start = s.pos;
        let idle = VehicleInput::default();
        for _ in 0..2 * STEPS_PER_S {
            step(&spec, &mut s, &idle, &Assists::default(), &GROUND);
        }
        let settled = s.pos;
        let mut worst_v: f32 = 0.0;
        let mut worst_w: f32 = 0.0;
        for _ in 0..10 * STEPS_PER_S {
            step(&spec, &mut s, &idle, &Assists::default(), &GROUND);
            worst_v = worst_v.max(len(s.vel));
            worst_w = worst_w.max(len(s.ang_vel));
        }
        let up = s.up();
        println!(
            "{:15} settle: sag {:+.4} m, drift {:.2e} m/s, spin {:.2e} rad/s, moved {:.2e} m",
            spec.id,
            settled[1] - start[1],
            worst_v,
            worst_w,
            horizontal_distance(s.pos, start)
        );
        assert!(worst_v < 0.01, "{} drifts at {worst_v} m/s", spec.id);
        assert!(worst_w < 0.01, "{} rotates at {worst_w} rad/s", spec.id);
        assert!(
            horizontal_distance(s.pos, settled) < 0.01,
            "{} wandered",
            spec.id
        );
        assert!(up[1] > 0.9999, "{} is not level: up {up:?}", spec.id);
        for w in &s.wheels {
            assert!(w.grounded && w.compression > 0.0 && w.compression < spec.suspension.travel_m);
        }
    }
}

/// Full throttle from rest on flat ground: time to 60 mph and the speed after `secs`.
fn full_throttle(spec: &VehicleSpec, assists: &Assists, secs: usize) -> (f32, Vec<f32>) {
    let mut s = spawn(spec);
    let go = input(1.0, 0.0, 0.0);
    let mut zero_to_sixty = f32::NAN;
    let mut speeds = Vec::new();
    for i in 0..secs * STEPS_PER_S {
        step(spec, &mut s, &go, assists, &GROUND);
        if zero_to_sixty.is_nan() && s.speed_mps >= 60.0 * MPH {
            zero_to_sixty = (i + 1) as f32 * DT;
        }
        if (i + 1) % STEPS_PER_S == 0 {
            speeds.push(s.speed_mps);
        }
    }
    assert_finite(&s);
    (zero_to_sixty, speeds)
}

#[test]
fn straight_line_acceleration_and_top_speed() {
    let [corsair, palisade] = cars();
    let mut tops = [0.0; 2];
    for (k, (spec, band)) in [(&corsair, (5.0, 8.0)), (&palisade, (10.0, 16.0))]
        .into_iter()
        .enumerate()
    {
        let (t60, speeds) = full_throttle(spec, &Assists::default(), 150);
        let top = speeds[speeds.len() - 1];
        let earlier = speeds[speeds.len() - 31];
        tops[k] = top;
        println!(
            "{:15} 0-60 mph {:.2} s, top speed {:.1} m/s ({:.0} mph), +{:.3} m/s over the last 30 s",
            spec.id,
            t60,
            top,
            top / MPH,
            top - earlier
        );
        assert!(
            t60 >= band.0 && t60 <= band.1,
            "{} 0-60 mph {t60} s outside {band:?}",
            spec.id
        );
        assert!(top < 95.0, "{} top speed {top} m/s", spec.id);
        assert!(
            top - earlier < 0.25,
            "{} has not plateaued: {earlier} -> {top} m/s",
            spec.id
        );
    }
    assert!(tops[0] > tops[1], "top speed order {tops:?}");
}

/// Accelerates to 60 mph, then stands on the brake: metres to a stop.
fn stopping_distance(spec: &VehicleSpec, assists: &Assists) -> f32 {
    let mut s = spawn(spec);
    let go = input(1.0, 0.0, 0.0);
    while s.speed_mps < 60.0 * MPH {
        step(spec, &mut s, &go, assists, &GROUND);
    }
    let start = s.pos;
    let stop = input(0.0, 1.0, 0.0);
    for _ in 0..20 * STEPS_PER_S {
        step(spec, &mut s, &stop, assists, &GROUND);
        if s.speed_mps < 0.05 {
            return horizontal_distance(s.pos, start);
        }
    }
    panic!("{} did not stop", spec.id);
}

#[test]
fn braking_from_sixty_stops_within_sixty_metres() {
    for spec in cars() {
        let with_abs = stopping_distance(
            &spec,
            &Assists {
                abs: true,
                ..Assists::OFF
            },
        );
        let without = stopping_distance(&spec, &Assists::OFF);
        println!(
            "{:15} 60-0 mph: {with_abs:.1} m with ABS, {without:.1} m without",
            spec.id
        );
        assert!(with_abs < 60.0, "{} ABS stop {with_abs} m", spec.id);
        assert!(without < 60.0, "{} non-ABS stop {without} m", spec.id);
    }
}

/// Skidpad: steers on yaw rate to hold a 40 m circle while the target speed creeps up, until
/// an axle's mean slip passes the tire peak for half a second. Returns the highest
/// one-second-average lateral g reached while the car was holding the circle.
fn skidpad(spec: &VehicleSpec) -> f32 {
    const RADIUS: f32 = 40.0;
    let assists = Assists::OFF;
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
            &assists,
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
        // Launch wheelspin does not count: only once the car has held the circle.
        past_peak_s = if best > 0.0 && front.max(rear) > 1.0 {
            past_peak_s + DT
        } else {
            0.0
        };
        if past_peak_s > 0.5 {
            break;
        }
    }
    assert_finite(&s);
    best
}

#[test]
fn skidpad_lateral_grip_is_ordered() {
    let [corsair, palisade] = cars();
    let gc = skidpad(&corsair);
    let gp = skidpad(&palisade);
    println!("{:15} skidpad {gc:.3} g", corsair.id);
    println!("{:15} skidpad {gp:.3} g", palisade.id);
    assert!(gc > gp, "skidpad order: corsair {gc} g, palisade {gp} g");
    assert!(
        gp > 0.5 && gc < 1.6,
        "implausible skidpad numbers {gc} {gp}"
    );
}

#[test]
fn full_lock_at_thirty_does_not_flip() {
    for spec in cars() {
        for (name, assists) in [("light", Assists::LIGHT), ("off", Assists::OFF)] {
            let mut s = spawn(&spec);
            while s.speed_mps < 30.0 * MPH {
                step(&spec, &mut s, &input(1.0, 0.0, 0.0), &assists, &GROUND);
            }
            let mut min_up: f32 = 1.0;
            let mut peak_g: f32 = 0.0;
            for _ in 0..5 * STEPS_PER_S {
                let throttle = ((30.0 * MPH - s.speed_mps) * 0.3).clamp(0.0, 1.0);
                step(&spec, &mut s, &input(throttle, 0.0, 1.0), &assists, &GROUND);
                min_up = min_up.min(s.up()[1]);
                peak_g = peak_g.max(s.lat_g.abs());
            }
            println!(
                "{:15} full lock at 30 mph, assists {name}: min up {min_up:.3}, peak {peak_g:.2} g",
                spec.id
            );
            assert!(min_up > 0.5, "{} rolled ({name}): up.y {min_up}", spec.id);
        }
    }
}

/// Runs `steps` steps of constant input.
fn hold(
    spec: &VehicleSpec,
    s: &mut VehicleState,
    inp: VehicleInput,
    assists: &Assists,
    steps: usize,
) {
    for _ in 0..steps {
        step(spec, s, &inp, assists, &GROUND);
    }
}

fn up_to_speed(spec: &VehicleSpec, s: &mut VehicleState, mps: f32) {
    while s.speed_mps < mps {
        step(spec, s, &input(1.0, 0.0, 0.0), &Assists::default(), &GROUND);
    }
}

#[test]
fn steering_right_turns_right() {
    for spec in cars() {
        let mut s = spawn(&spec);
        up_to_speed(&spec, &mut s, 10.0);
        let x0 = s.pos[0];
        hold(
            &spec,
            &mut s,
            input(0.3, 0.0, 0.5),
            &Assists::default(),
            2 * STEPS_PER_S,
        );
        assert!(s.ang_vel[1] < -0.1, "{} yaw rate {:?}", spec.id, s.ang_vel);
        assert!(s.pos[0] > x0 + 1.0, "{} did not move right", spec.id);
        assert!(s.lat_g > 0.1, "{} lateral g {}", spec.id, s.lat_g);
        let roll = s.right()[1];
        // Body rolls toward the outside (left) of a right turn: the right side rises.
        assert!(
            roll > 0.0,
            "{} rolls the wrong way: right.y {roll}",
            spec.id
        );
    }
}

#[test]
fn soft_car_leans_more() {
    let [corsair, palisade] = cars();
    let mut lean = [0.0; 2];
    for (k, spec) in [&corsair, &palisade].into_iter().enumerate() {
        let mut s = spawn(spec);
        up_to_speed(spec, &mut s, 12.0);
        hold(
            spec,
            &mut s,
            input(0.2, 0.0, 0.35),
            &Assists::default(),
            3 * STEPS_PER_S,
        );
        lean[k] = s.right()[1].asin().to_degrees() / s.lat_g.max(0.05);
    }
    println!(
        "roll gradient: corsair {:.2} deg/g, palisade {:.2} deg/g",
        lean[0], lean[1]
    );
    assert!(lean[1] > 1.5 * lean[0], "roll gradients {lean:?}");
}

#[test]
fn handbrake_locks_the_rear() {
    for spec in cars() {
        let mut s = spawn(&spec);
        up_to_speed(&spec, &mut s, 15.0);
        let pull = VehicleInput {
            handbrake: true,
            ..VehicleInput::default()
        };
        hold(&spec, &mut s, pull, &Assists::default(), STEPS_PER_S / 2);
        assert!(s.speed_mps > 5.0);
        for w in &s.wheels[2..] {
            assert_eq!(w.spin, 0.0, "{} rear wheel turns under handbrake", spec.id);
        }
        assert!(
            s.wheels[0].spin > 10.0,
            "{} front wheels locked too",
            spec.id
        );
    }
}

#[test]
fn braking_at_a_standstill_selects_reverse() {
    for spec in cars() {
        let mut s = spawn(&spec);
        hold(
            &spec,
            &mut s,
            input(0.0, 1.0, 0.0),
            &Assists::default(),
            3 * STEPS_PER_S,
        );
        assert_eq!(s.gear, -1, "{}", spec.id);
        assert!(
            s.speed_mps < -1.0,
            "{} reverse speed {}",
            spec.id,
            s.speed_mps
        );
        // Throttle while reversing brakes, then selects drive once stopped.
        hold(
            &spec,
            &mut s,
            input(1.0, 0.0, 0.0),
            &Assists::default(),
            4 * STEPS_PER_S,
        );
        assert_eq!(s.gear, 1, "{}", spec.id);
        assert!(
            s.speed_mps > 1.0,
            "{} forward speed {}",
            spec.id,
            s.speed_mps
        );
    }
}

#[test]
fn abs_keeps_the_wheels_turning_on_a_slippery_road() {
    let wet = FlatGround {
        y: 0.0,
        friction: 0.45,
    };
    for spec in cars() {
        for abs in [false, true] {
            let assists = Assists {
                abs,
                ..Assists::OFF
            };
            let mut s = spawn(&spec);
            up_to_speed(&spec, &mut s, 25.0);
            let start = s.pos;
            let mut locked_steps = 0;
            let mut stopped = false;
            for _ in 0..20 * STEPS_PER_S {
                step(&spec, &mut s, &input(0.0, 1.0, 0.0), &assists, &wet);
                if s.speed_mps < 0.05 {
                    stopped = true;
                    break;
                }
                if s.speed_mps > 3.0 && s.wheels[0].spin == 0.0 {
                    locked_steps += 1;
                }
            }
            let d = horizontal_distance(s.pos, start);
            println!(
                "{:15} 25-0 m/s at 0.45 grip, ABS {abs}: {d:.1} m, front locked {:.2} s",
                spec.id,
                locked_steps as f32 * DT
            );
            assert!(stopped, "{} did not stop", spec.id);
            if abs {
                assert!(
                    locked_steps < STEPS_PER_S / 10,
                    "{} ABS let the fronts lock",
                    spec.id
                );
            } else {
                assert!(
                    locked_steps > STEPS_PER_S,
                    "{} fronts never locked without ABS",
                    spec.id
                );
            }
        }
    }
}

#[test]
fn traction_control_caps_wheelspin() {
    for spec in cars() {
        let mut worst = [0.0f32; 2];
        for (k, assists) in [Assists::OFF, Assists::default()].iter().enumerate() {
            let mut s = spawn(&spec);
            for _ in 0..2 * STEPS_PER_S {
                step(&spec, &mut s, &input(1.0, 0.0, 0.0), assists, &GROUND);
                if s.speed_mps > 3.0 {
                    worst[k] = worst[k].max(s.wheels[2].slip_ratio);
                }
            }
        }
        println!(
            "{:15} launch rear slip ratio: {:.3} off, {:.3} light",
            spec.id, worst[0], worst[1]
        );
        assert!(worst[1] <= worst[0], "{} {worst:?}", spec.id);
    }
}

#[test]
fn dropped_car_lands_and_settles() {
    for spec in cars() {
        let mut s = VehicleState::spawn(&spec, [0.0, 1.5, 0.0], 0.3);
        for w in &mut s.wheels {
            w.grounded = false;
        }
        hold(
            &spec,
            &mut s,
            VehicleInput::default(),
            &Assists::default(),
            4 * STEPS_PER_S,
        );
        assert_finite(&s);
        assert!(
            len(s.vel) < 0.01 && len(s.ang_vel) < 0.01,
            "{} still moving",
            spec.id
        );
        assert!(
            (s.pos[1] - spec.cg_height_m).abs() < 0.01,
            "{} rests at {}",
            spec.id,
            s.pos[1]
        );
        assert!(s.wheels.iter().all(|w| w.grounded));
    }
}

#[test]
fn upside_down_car_rights_itself() {
    for spec in cars() {
        let mut s = VehicleState::spawn(&spec, [0.0, 1.2, 0.0], 0.0);
        // Roll it onto its roof: a half turn about the forward axis.
        s.rot = [0.0, 0.0, 1.0, 0.0];
        let mut righted_at = None;
        for i in 0..15 * STEPS_PER_S {
            step(
                &spec,
                &mut s,
                &VehicleInput::default(),
                &Assists::default(),
                &GROUND,
            );
            assert!(s.pos[1] > 0.0, "{} sank through the ground", spec.id);
            let t = i as f32 * DT;
            if t < 2.9 {
                assert!(s.up()[1] < 0.0, "{} righted before the delay", spec.id);
            }
            if righted_at.is_none() && s.up()[1] > 0.95 && t > 3.0 {
                righted_at = Some(t);
            }
        }
        assert_finite(&s);
        println!(
            "{:15} auto-right: back on its wheels at {righted_at:?} s",
            spec.id
        );
        assert!(righted_at.is_some(), "{} never righted", spec.id);
        assert!(
            s.up()[1] > 0.99 && len(s.vel) < 0.05,
            "{} did not settle upright",
            spec.id
        );
    }
}

/// Full stick from `mps` for 4 s on a light throttle (more would spin the rear-drive cars
/// and measure a drift, not the steering): the road-wheel angle at the end, the mean
/// lateral g and the front tires' mean slip relative to their peak over the last 2 s.
fn full_stick_at(spec: &VehicleSpec, assists: &Assists, mps: f32) -> (f32, f32, f32) {
    let mut s = spawn(spec);
    up_to_speed(spec, &mut s, mps);
    let (mut g, mut slip, mut n) = (0.0, 0.0, 0.0);
    for i in 0..4 * STEPS_PER_S {
        step(spec, &mut s, &input(0.15, 0.0, 1.0), assists, &GROUND);
        if i >= 2 * STEPS_PER_S {
            g += s.lat_g;
            slip += 0.5 * (s.wheels[0].slip + s.wheels[1].slip);
            n += 1.0;
        }
    }
    assert_finite(&s);
    (s.steer_angle_rad.to_degrees(), g / n, slip / n)
}

/// Grip-limited steering (on in the default assists). Before it, full stick asked for the
/// spec's speed-sensitive lock: corsair 22.4° and palisade 22.5° at 30 m/s, far past what
/// the tires use, so the fronts scrubbed at 2–4× their peak slip. Now full stick aims the
/// fronts their peak slip angle off the front axle's travel (7° and 9.5° on a straight at
/// speed, plus the turn's kinematic angle once turning), so they sit near their peak with
/// more lateral g, a slide gets counter-steer, and full lock is still there below 8 m/s.
/// The test prints both.
#[test]
fn steering_at_speed_is_grip_limited() {
    for spec in cars() {
        let (on, off) = (Assists::LIGHT, Assists::OFF);
        // Rolling straight ahead (heading 0 faces -z) at `v`: the full-stick target.
        let target = |assists: &Assists, v: f32| {
            let mut s = spawn(&spec);
            s.vel = [0.0, 0.0, -v];
            steer_target(&spec, assists, &s, 1.0).to_degrees()
        };
        for v in [0.0, 3.0, 6.0, 7.9, -6.0] {
            assert_eq!(
                target(&on, v),
                target(&off, v),
                "{} lock at {v} m/s",
                spec.id
            );
        }
        let (old30, new30) = (target(&off, 30.0), target(&on, 30.0));
        println!(
            "{:15} full stick at 30 m/s: {old30:.1}° speed-sensitive, {new30:.1}° grip-limited",
            spec.id
        );
        assert!(
            (new30 - spec.tires.slip_peak_deg).abs() < 1e-3,
            "{} {new30}°",
            spec.id
        );
        assert!(new30 < 0.5 * old30);
        // Sliding sideways to the left, hands off: the wheels follow the travel (counter-
        // steer) instead of pointing straight.
        let mut s = spawn(&spec);
        s.vel = [-4.0, 0.0, -20.0];
        assert!(steer_target(&spec, &on, &s, 0.0) < -0.15);
        assert_eq!(steer_target(&spec, &off, &s, 0.0), 0.0);
        for mps in [15.0, 25.0] {
            let before = Assists {
                steering: 0.0,
                ..Assists::LIGHT
            };
            let (a0, g0, slip0) = full_stick_at(&spec, &before, mps);
            let (angle, g, slip) = full_stick_at(&spec, &on, mps);
            println!(
                "{:15} full stick from {mps} m/s: {angle:.1}°, {g:.2} g, front slip {slip:.2} of \
                 peak (speed-sensitive lock: {a0:.1}°, {g0:.2} g, {slip0:.2})",
                spec.id
            );
            assert!(g > 0.95 * g0, "{} lost grip: {g} g vs {g0} g", spec.id);
            assert!(g > 0.6, "{} turns at only {g} g at {mps} m/s", spec.id);
            assert!(
                slip < 2.0,
                "{} front tires far past their peak ({slip}) at {mps} m/s",
                spec.id
            );
        }
    }
}

/// Reverse is capped at 8 m/s (before: it ran up to ~20 m/s, first gear's top speed).
#[test]
fn reverse_is_capped() {
    for spec in cars() {
        let mut s = spawn(&spec);
        let mut fastest: f32 = 0.0;
        for _ in 0..12 * STEPS_PER_S {
            step(
                &spec,
                &mut s,
                &input(0.0, 1.0, 0.0),
                &Assists::default(),
                &GROUND,
            );
            fastest = fastest.max(-s.speed_mps);
        }
        println!("{:15} reverse top speed {fastest:.2} m/s", spec.id);
        assert_eq!(s.gear, -1);
        assert!(fastest <= REVERSE_MAX_MPS + 0.3, "{} {fastest}", spec.id);
        assert!(fastest > REVERSE_MAX_MPS - 1.5, "{} {fastest}", spec.id);
    }
}

/// The scripted 10 s drive: full throttle, a steering sweep, a stop, a handbrake flick.
fn replay_input(i: usize) -> VehicleInput {
    let t = i as f32 * DT;
    // A triangle wave keeps the script free of platform `sin`.
    let tri = |t: f32, period: f32| {
        let p = (t / period).fract();
        if p < 0.5 {
            4.0 * p - 1.0
        } else {
            3.0 - 4.0 * p
        }
    };
    if t < 3.0 {
        input(1.0, 0.0, 0.0)
    } else if t < 6.5 {
        input(0.6, 0.0, 0.8 * tri(t - 3.0, 1.75))
    } else if t < 8.5 {
        input(0.0, 1.0, 0.0)
    } else {
        VehicleInput {
            throttle: 0.4,
            brake: 0.0,
            steer: -0.5,
            handbrake: t < 9.0,
        }
    }
}

fn replay(spec: &VehicleSpec) -> u64 {
    let mut s = spawn(spec);
    for i in 0..10 * STEPS_PER_S {
        step(spec, &mut s, &replay_input(i), &Assists::default(), &GROUND);
    }
    assert_finite(&s);
    s.hash()
}

#[test]
fn replay_is_deterministic_and_pinned() {
    // Pinned hashes of the 10 s scripted drive. A deliberate change to the sim, a spec or the
    // script moves them: run `cargo test -p vehicle --test sim replay -- --nocapture`, copy the
    // printed hashes here, and commit that on its own with the reason.
    const PINNED: [u64; 2] = [0x2699_71ff_d7ab_3649, 0x2e3c_ee38_7736_a728];
    let specs = cars();
    let mut hashes = [0u64; 2];
    for (k, spec) in specs.iter().enumerate() {
        hashes[k] = replay(spec);
        println!("{:15} replay {:#018x}", spec.id, hashes[k]);
        assert_eq!(
            hashes[k],
            replay(spec),
            "{} replay is not deterministic",
            spec.id
        );
    }
    assert_eq!(hashes, PINNED, "replay hashes moved (corsair, palisade)");
}

#[test]
fn step_does_not_allocate() {
    for spec in cars() {
        let mut s = spawn(&spec);
        let before = allocations();
        for i in 0..10 * STEPS_PER_S {
            step(
                &spec,
                &mut s,
                &replay_input(i),
                &Assists::default(),
                &GROUND,
            );
        }
        assert_eq!(allocations() - before, 0, "{} step allocated", spec.id);
    }
}
