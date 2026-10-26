//! Acceptance tests for `world::collide`: the real specs from `assets/vehicles/` driven
//! through the test town. Scenarios (a wall with open ground in front of it, a clear stretch
//! of curb, a lamp post) are found by searching the collider's own shapes, so they follow the
//! town layout. Run with `--nocapture` to read the measured numbers:
//! `cargo test -p world --test collide -- --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use place::Place;
use vehicle::sim::DT;
use vehicle::{step, Assists, FlatGround, VehicleInput, VehicleSpec, VehicleState};
use world::collide::{Impact, Wall};
use world::props::PropKind;
use world::{resolve, WorldCollider};

const CORSAIR: &str = include_str!("../../../assets/vehicles/corsair_gtx.ron");
const PALISADE: &str = include_str!("../../../assets/vehicles/palisade_wagon.ron");
const MPH: f32 = 0.447_04;
const STEPS_PER_S: usize = 120;
const SIDEWALK_Y: f32 = world::CURB_M;

/// Counts heap allocations made on the current thread.
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

fn town() -> (Place, WorldCollider) {
    let place = Place::test_grid();
    let collider = WorldCollider::build(&place);
    (place, collider)
}

fn input(throttle: f32, brake: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steer: 0.0,
        handbrake: false,
    }
}

/// Throttle that holds `mps`.
fn cruise(s: &VehicleState, mps: f32) -> VehicleInput {
    input((0.3 + (mps - s.speed_mps) * 0.5).clamp(0.0, 1.0), 0.0)
}

/// Brakes to a stop, then lets go (held at a standstill the brake selects reverse).
fn stop(s: &VehicleState) -> VehicleInput {
    if s.speed_mps.abs() > 0.3 {
        input(0.0, 1.0)
    } else {
        VehicleInput::default()
    }
}

fn tick(spec: &VehicleSpec, s: &mut VehicleState, inp: VehicleInput, c: &WorldCollider) -> Impact {
    step(spec, s, &inp, &Assists::default(), c);
    resolve(s, spec, c)
}

fn assert_finite(s: &VehicleState) {
    let all = s.pos.iter().chain(&s.rot).chain(&s.vel).chain(&s.ang_vel);
    assert!(all.clone().all(|v| v.is_finite()), "non-finite state {s:?}");
}

fn hspeed(s: &VehicleState) -> f32 {
    (s.vel[0] * s.vel[0] + s.vel[2] * s.vel[2]).sqrt()
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: [f32; 2], b: [f32; 2], s: f32) -> [f32; 2] {
    [a[0] + b[0] * s, a[1] + b[1] * s]
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn norm(a: [f32; 2]) -> [f32; 2] {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l]
}

/// Yaw angle with `VehicleState::spawn`'s convention (0 faces `-z`, positive turns left)
/// that faces the ground-plane direction `d`.
fn heading_of(d: [f32; 2]) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    // Exact for the grid's axis-aligned directions, so pinned runs need no platform `atan2`.
    match (d[0], d[1]) {
        (x, z) if x == 0.0 && z < 0.0 => 0.0,
        (x, z) if x == 0.0 && z > 0.0 => PI,
        (x, z) if z == 0.0 && x < 0.0 => FRAC_PI_2,
        (x, z) if z == 0.0 && x > 0.0 => -FRAC_PI_2,
        _ => (-d[0]).atan2(-d[1]),
    }
}

fn xz(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[2]]
}

fn forward2(s: &VehicleState) -> [f32; 2] {
    norm(xz(s.forward()))
}

/// The eight body-box corners, world space (the box `world::collide` collides).
fn body_corners(s: &VehicleState, spec: &VehicleSpec) -> [[f32; 3]; 8] {
    let [len, wid, hgt] = spec.dims_m;
    let low = -spec.cg_height_m + 0.25;
    let (r, u, f) = (s.right(), s.up(), s.forward());
    let mut out = [[0.0; 3]; 8];
    for (k, o) in out.iter_mut().enumerate() {
        let x = if k & 1 == 0 { -0.5 * wid } else { 0.5 * wid };
        let y = if k & 2 == 0 { low } else { low + hgt };
        let z = if k & 4 == 0 { -0.5 * len } else { 0.5 * len };
        for a in 0..3 {
            o[a] = s.pos[a] + r[a] * x + u[a] * y - f[a] * z;
        }
    }
    out
}

/// A car at `ground` facing `heading`, already rolling at `mps` with the gearbox, engine and
/// wheels where a run-up on flat ground left them.
fn launched(spec: &VehicleSpec, ground: [f32; 3], heading: f32, mps: f32) -> VehicleState {
    let flat = FlatGround {
        y: 0.0,
        friction: 1.0,
    };
    let mut run = VehicleState::spawn(spec, [0.0; 3], 0.0);
    while run.speed_mps < mps {
        step(spec, &mut run, &input(1.0, 0.0), &Assists::default(), &flat);
    }
    for _ in 0..STEPS_PER_S / 2 {
        let inp = cruise(&run, mps);
        step(spec, &mut run, &inp, &Assists::default(), &flat);
    }
    let mut s = VehicleState::spawn(spec, ground, heading);
    let f = s.forward();
    s.vel = [f[0] * run.speed_mps, 0.0, f[2] * run.speed_mps];
    s.pos[1] = ground[1] + run.pos[1];
    for (w, r) in s.wheels.iter_mut().zip(&run.wheels) {
        w.spin = r.spin;
        w.compression = r.compression;
    }
    s.engine_rpm = run.engine_rpm;
    s.gear = run.gear;
    s.shift_timer = run.shift_timer;
    s.throttle = run.throttle;
    s.traction_cut = run.traction_cut;
    s.speed_mps = run.speed_mps;
    s
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = sub(b, a);
    let t = (dot(sub(p, a), ab) / dot(ab, ab)).clamp(0.0, 1.0);
    let q = add(a, ab, t);
    dot(sub(p, q), sub(p, q)).sqrt()
}

fn segments_cross(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let side = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (d1, d2) = (side(a, b, c), side(a, b, d));
    let (d3, d4) = (side(c, d, a), side(c, d, b));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

fn seg_seg_dist(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> f32 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    seg_dist(a, c, d)
        .min(seg_dist(b, c, d))
        .min(seg_dist(c, a, b))
        .min(seg_dist(d, a, b))
}

/// No wall (except `skip`), post or box within `half` of the path `a..b`.
fn clear(c: &WorldCollider, a: [f32; 2], b: [f32; 2], half: f32, skip: Option<usize>) -> bool {
    let walls = c
        .walls
        .iter()
        .enumerate()
        .all(|(i, w)| Some(i) == skip || seg_seg_dist(a, b, w.a, w.b) > half);
    let posts = c.posts.iter().all(|p| seg_dist(p.c, a, b) > half + p.r);
    let boxes = c.boxes.iter().all(|x| {
        let r = (x.half[0] * x.half[0] + x.half[1] * x.half[1]).sqrt();
        seg_dist(x.c, a, b) > half + r
    });
    walls && posts && boxes
}

/// Ground levels along the path `a..b` and two lines `half` to either side, every 0.25 m.
fn levels(c: &WorldCollider, a: [f32; 2], b: [f32; 2], half: f32) -> [Vec<f32>; 3] {
    let d = norm(sub(b, a));
    let side = [-d[1], d[0]];
    let n = (dot(sub(b, a), sub(b, a)).sqrt() / 0.25) as usize + 1;
    [-half, 0.0, half].map(|o| {
        (0..=n)
            .map(|k| {
                let p = add(add(a, sub(b, a), k as f32 / n as f32), side, o);
                c.level_at(p).0
            })
            .collect()
    })
}

/// A wall at least `min_len` long whose outside is level and open for `run` m across a
/// 3.6 m wide lane centered on its midpoint.
fn open_wall(c: &WorldCollider, min_len: f32, run: f32) -> (usize, Wall) {
    for (i, w) in c.walls.iter().enumerate() {
        let len = dot(sub(w.b, w.a), sub(w.b, w.a)).sqrt();
        if len < min_len {
            continue;
        }
        let mid = add(w.a, sub(w.b, w.a), 0.5);
        let (a, b) = (add(mid, w.n, 0.1), add(mid, w.n, run));
        let lv = levels(c, a, b, 1.8);
        let y = lv[1][0];
        if lv.iter().all(|l| l.iter().all(|&v| v == y)) && clear(c, a, b, 1.8, Some(i)) {
            return (i, *w);
        }
    }
    panic!("no open wall in the test town");
}

/// Penetration of the body box behind the plane of `w` (positive when behind it), and the
/// gap between the box and the plane.
fn behind_and_gap(s: &VehicleState, spec: &VehicleSpec, w: &Wall) -> (f32, f32) {
    let mut gap = f32::MAX;
    for p in body_corners(s, spec) {
        gap = gap.min(dot(sub(xz(p), w.a), w.n));
    }
    (-gap, gap)
}

#[test]
fn collider_builds_fast_and_matches_the_meshes() {
    let place = Place::test_grid();
    let t0 = std::time::Instant::now();
    let c = WorldCollider::build(&place);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let (_, stats) = world::build_with_stats(&place, &world::Palette::socal_1985());
    println!(
        "collider: {} patches, {} steps, {} walls, {} posts, {} boxes on a {:?} grid in {ms:.1} ms",
        c.patches.len(),
        c.steps.len(),
        c.walls.len(),
        c.posts.len(),
        c.boxes.len(),
        c.cells
    );
    // Budget 50 ms (debug, world at opt-level 1); assert 3x so a loaded CI machine does
    // not flake.
    assert!(ms < 150.0, "{ms} ms");
    assert_eq!(
        c.posts.len() + c.boxes.len(),
        stats.props,
        "one solid per prop"
    );
    let walls: usize = place.buildings.iter().map(|b| b.footprint.len()).sum();
    assert_eq!(c.walls.len(), walls);
    assert_eq!(c, WorldCollider::build(&place), "deterministic");
    // Heights where the meshes put them: arterial asphalt, its curb, sidewalk, the apron.
    assert_eq!(c.level_at([150.0, 3.0]).0, 0.0);
    assert_eq!(c.level_at([150.0, 8.0]).0, SIDEWALK_Y);
    assert_eq!(c.level_at([150.0, -8.0]).0, SIDEWALK_Y);
    assert_eq!(c.level_at([1000.0, 1000.0]).0, world::TERRAIN_Y);
    // A wheel next to the curb reads the height at which it touches the curb's edge.
    let r = vehicle::sim::WHEEL_RADIUS_M;
    let near = c.ground_at([150.0, 6.9]);
    assert!(near.y > 0.0 && near.y < SIDEWALK_Y, "{near:?}");
    assert!(near.normal[2] < 0.0, "tilted away from the curb: {near:?}");
    assert_eq!(c.ground_at([150.0, 7.0 - r - 0.01]).y, 0.0);
}

#[test]
fn walls_stop_the_car() {
    let (_, c) = town();
    let (_, wall) = open_wall(&c, 6.0, 14.0);
    let mid = add(wall.a, sub(wall.b, wall.a), 0.5);
    for spec in cars() {
        for mph in [20.0, 40.0, 60.0] {
            let v = mph * MPH;
            let start = add(mid, wall.n, 4.0 + 0.5 * spec.dims_m[0]);
            let y = c.level_at(start).0;
            let mut s = launched(
                &spec,
                [start[0], y, start[1]],
                heading_of([-wall.n[0], -wall.n[1]]),
                v,
            );
            let mut hit_at = None;
            let mut worst_behind: f32 = f32::MIN;
            let mut min_gap = f32::MAX;
            let mut rebound: f32 = 0.0;
            let mut min_up: f32 = 1.0;
            for i in 0..4 * STEPS_PER_S {
                let inp = if hit_at.is_none() {
                    cruise(&s, v)
                } else {
                    VehicleInput::default()
                };
                let impact = tick(&spec, &mut s, inp, &c);
                assert_finite(&s);
                if impact.contact && hit_at.is_none() {
                    hit_at = Some(i);
                }
                let (behind, gap) = behind_and_gap(&s, &spec, &wall);
                worst_behind = worst_behind.max(behind);
                min_gap = min_gap.min(gap);
                if hit_at.is_some() {
                    rebound = rebound.max(gap - min_gap);
                }
                min_up = min_up.min(s.up()[1]);
            }
            let (_, gap) = behind_and_gap(&s, &spec, &wall);
            println!(
                "{:15} wall at {mph} mph: rest gap {gap:.3} m, deepest {worst_behind:+.4} m, rebound {rebound:.3} m, min up.y {min_up:.3}, final speed {:.3} m/s",
                spec.id,
                hspeed(&s)
            );
            assert!(hit_at.is_some(), "{} never reached the wall", spec.id);
            assert!(worst_behind < 0.02, "{} passed the wall plane", spec.id);
            assert!(gap < 1.5, "{} stopped {gap} m from the wall", spec.id);
            assert!(rebound <= 0.5, "{} rebounded {rebound} m", spec.id);
            assert!(min_up > 0.8, "{} tipped: up.y {min_up}", spec.id);
            assert!(hspeed(&s) < 0.2, "{} still moving", spec.id);
        }
    }
}

#[test]
fn glancing_hit_yaws_away_and_keeps_speed() {
    let (_, c) = town();
    let angle = 20.0f32.to_radians();
    let v = 40.0 * MPH;
    // The car's nose corner meets the wall a quarter of the way along it.
    let (i, wall) = (0..c.walls.len())
        .find_map(|i| {
            let w = c.walls[i];
            let len = dot(sub(w.b, w.a), sub(w.b, w.a)).sqrt();
            if len < 16.0 {
                return None;
            }
            let t = norm(sub(w.b, w.a));
            let d = add(
                [-w.n[0] * angle.sin(), -w.n[1] * angle.sin()],
                t,
                angle.cos(),
            );
            let hit = add(add(w.a, t, 0.3 * len), w.n, 1.2);
            let start = add(hit, d, -14.0);
            let end = add(add(w.a, t, len + 4.0), w.n, 1.5);
            let lv = levels(&c, start, hit, 1.2);
            let y = lv[1][0];
            let level = lv.iter().all(|l| l.iter().all(|&v| v == y));
            let along = levels(&c, hit, end, 0.8);
            let level = level && along.iter().all(|l| l.iter().all(|&v| v == y));
            (level && clear(&c, start, hit, 1.3, Some(i)) && clear(&c, hit, end, 1.3, Some(i)))
                .then_some((i, w))
        })
        .expect("no long open wall in the test town");
    let t = norm(sub(wall.b, wall.a));
    let len = dot(sub(wall.b, wall.a), sub(wall.b, wall.a)).sqrt();
    let d = add(
        [-wall.n[0] * angle.sin(), -wall.n[1] * angle.sin()],
        t,
        angle.cos(),
    );
    let hit = add(add(wall.a, t, 0.3 * len), wall.n, 1.2);
    let start = add(hit, d, -14.0);
    println!("glancing wall {i}: {wall:?}");
    for spec in cars() {
        let y = c.level_at(start).0;
        let mut s = launched(&spec, [start[0], y, start[1]], heading_of(d), v);
        let mut hit_at = None;
        let mut before = 0.0;
        for k in 0..3 * STEPS_PER_S {
            let inp = if hit_at.is_none() {
                cruise(&s, v)
            } else {
                VehicleInput::default()
            };
            let pre = hspeed(&s);
            let impact = tick(&spec, &mut s, inp, &c);
            assert_finite(&s);
            if impact.contact && hit_at.is_none() {
                hit_at = Some(k);
                before = pre;
            }
            if let Some(h) = hit_at {
                if k == h + STEPS_PER_S / 2 {
                    break;
                }
            }
        }
        assert!(hit_at.is_some(), "{} missed the wall", spec.id);
        let kept = hspeed(&s) / before;
        let into = -dot(forward2(&s), wall.n);
        println!(
            "{:15} glancing 20 deg at 40 mph: kept {:.0}% of {before:.1} m/s, heading into the wall {:.1} deg (was 20), yaw rate {:+.2} rad/s",
            spec.id,
            kept * 100.0,
            into.asin().to_degrees(),
            s.ang_vel[1]
        );
        assert!(kept >= 0.3, "{} kept {kept}", spec.id);
        assert!(
            into < angle.sin() * 0.5,
            "{} did not yaw away: {into}",
            spec.id
        );
        assert!(s.up()[1] > 0.8);
    }
}

/// A straight stretch of curb with open asphalt before it and open sidewalk-level ground
/// (or the apron, if `apron`) for `post` m after it, crossed at `angle` from the road.
/// Returns the start point, the travel direction and the crossing point.
fn curb_run(
    place: &Place,
    c: &WorldCollider,
    angle: f32,
    pre: f32,
    post: f32,
    apron: bool,
) -> ([f32; 2], [f32; 2], [f32; 2]) {
    let nodes = &place.roads.nodes;
    for e in &place.roads.edges {
        let a = nodes[e.a as usize].pos;
        let b = nodes[e.b as usize].pos;
        let len = dot(sub(b, a), sub(b, a)).sqrt();
        let along = norm(sub(b, a));
        for sigma in [1.0f32, -1.0] {
            let out = [along[1] * sigma, -along[0] * sigma];
            let d = add(
                [along[0] * angle.cos(), along[1] * angle.cos()],
                out,
                angle.sin(),
            );
            let mut s = 20.0;
            while s < len - 20.0 {
                let k = add(add(a, along, s), out, e.width_m * 0.5);
                s += 2.0;
                let (start, end) = (add(k, d, -pre), add(k, d, post));
                if !clear(c, start, end, 1.4, None) {
                    continue;
                }
                let ok = levels(c, start, end, 1.0).iter().all(|l| {
                    let first_up = l.iter().position(|&v| v != 0.0);
                    match first_up {
                        Some(j) if j > 0 => l[j..]
                            .iter()
                            .all(|&v| v == SIDEWALK_Y || (apron && v == world::TERRAIN_Y)),
                        _ => false,
                    }
                });
                if ok {
                    return (start, d, k);
                }
            }
        }
    }
    panic!("no open curb in the test town");
}

#[test]
fn slow_cars_mount_the_curb() {
    let (place, c) = town();
    let (start, d, _) = curb_run(&place, &c, 35.0f32.to_radians(), 7.0, 14.0, false);
    let v = 20.0 * MPH;
    for spec in cars() {
        let mut s = launched(&spec, [start[0], 0.0, start[1]], heading_of(d), v);
        let mut peak_vy: f32 = 0.0;
        let mut braking = false;
        for _ in 0..5 * STEPS_PER_S {
            let on_top = s.wheels.iter().all(|w| w.grounded && w.contact[1] > 0.1);
            braking |= on_top;
            let inp = if braking { stop(&s) } else { cruise(&s, v) };
            let impact = tick(&spec, &mut s, inp, &c);
            assert_finite(&s);
            assert!(!impact.contact, "{} hit something", spec.id);
            assert!(!impact.curb_kick, "{} kicked at 20 mph", spec.id);
            peak_vy = peak_vy.max(s.vel[1]);
        }
        let ys: Vec<f32> = s.wheels.iter().map(|w| w.contact[1]).collect();
        println!(
            "{:15} curb at 20 mph: peak vy {peak_vy:.2} m/s, wheels rest at {ys:.3?}, speed {:.3}",
            spec.id,
            hspeed(&s)
        );
        assert!(braking, "{} never got up", spec.id);
        for w in &s.wheels {
            assert!(
                w.grounded && (w.contact[1] - SIDEWALK_Y).abs() < 0.01,
                "{ys:?}"
            );
        }
        assert!(hspeed(&s) < 0.05);
        assert!(s.up()[1] > 0.999);
    }
}

#[test]
fn fast_cars_bump_over_the_curb() {
    let (place, c) = town();
    let (start, d, _) = curb_run(&place, &c, 35.0f32.to_radians(), 7.0, 45.0, true);
    let v = 45.0 * MPH;
    for spec in cars() {
        let mut s = launched(&spec, [start[0], 0.0, start[1]], heading_of(d), v);
        let mut kicked_at = None;
        let mut peak_vy: f32 = 0.0;
        for i in 0..4 * STEPS_PER_S {
            let inp = if kicked_at.is_some() {
                stop(&s)
            } else {
                cruise(&s, v)
            };
            let impact = tick(&spec, &mut s, inp, &c);
            assert_finite(&s);
            assert!(!impact.contact, "{} hit something", spec.id);
            if impact.curb_kick && kicked_at.is_none() {
                kicked_at = Some(i);
            }
            if let Some(k) = kicked_at {
                if i < k + STEPS_PER_S / 2 {
                    peak_vy = peak_vy.max(s.vel[1]);
                }
            }
        }
        println!(
            "{:15} curb at 45 mph: peak vy {peak_vy:.2} m/s, final vy {:+.3} m/s, up.y {:.4}, speed {:.2}",
            spec.id,
            s.vel[1],
            s.up()[1],
            hspeed(&s)
        );
        assert!(kicked_at.is_some(), "{} was not kicked", spec.id);
        assert!(peak_vy > 0.5, "{} peak vy {peak_vy}", spec.id);
        assert!(s.vel[1].abs() < 0.05, "{} still bouncing", spec.id);
        assert!(s.wheels.iter().all(|w| w.grounded));
        assert!(s.up()[1] > 0.99);
    }
}

#[test]
fn rolled_car_on_the_curb_rests_and_rights() {
    // The sim's body-corner ground contacts carry a rolled car; on the collider they read
    // the curb's two levels.
    let (place, c) = town();
    let (_, d, k) = curb_run(&place, &c, 35.0f32.to_radians(), 7.0, 14.0, false);
    for spec in cars() {
        let mut s = VehicleState::spawn(&spec, [k[0], 1.2, k[1]], heading_of(d));
        s.rot = vehicle_roof_down(s.rot);
        let mut righted = false;
        for _ in 0..15 * STEPS_PER_S {
            tick(&spec, &mut s, VehicleInput::default(), &c);
            assert_finite(&s);
            for p in body_corners(&s, &spec) {
                let floor = c.level_at(xz(p)).0;
                assert!(
                    p[1] > floor - 0.1,
                    "{} sank into the ground at {p:?}",
                    spec.id
                );
            }
            righted |= s.up()[1] > 0.95;
        }
        let speed = (s.vel[0].powi(2) + s.vel[1].powi(2) + s.vel[2].powi(2)).sqrt();
        println!(
            "{:15} rolled on the curb: righted {righted}, rests at up.y {:.4}, {speed:.3} m/s",
            spec.id,
            s.up()[1]
        );
        assert!(
            righted && s.up()[1] > 0.98 && speed < 0.05,
            "{} did not settle",
            spec.id
        );
    }
}

/// `rot` followed by a half turn about the body's forward axis: on its roof.
fn vehicle_roof_down(rot: [f32; 4]) -> [f32; 4] {
    let (a, b) = (rot, [0.0f32, 0.0, 1.0, 0.0]);
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

#[test]
fn lamp_posts_stop_the_car() {
    let (place, c) = town();
    let v = 30.0 * MPH;
    // An arterial lamp, approached square across the road with nothing else in the way.
    let nodes = &place.roads.nodes;
    let (lamp, d) = c
        .posts
        .iter()
        .filter(|p| p.kind == PropKind::StreetLight)
        .find_map(|p| {
            let e = place.roads.edges.iter().find(|e| {
                let (a, b) = (nodes[e.a as usize].pos, nodes[e.b as usize].pos);
                e.width_m > 12.0 && (seg_dist(p.c, a, b) - (e.width_m * 0.5 + 0.45)).abs() < 0.01
            })?;
            let (a, b) = (nodes[e.a as usize].pos, nodes[e.b as usize].pos);
            let along = norm(sub(b, a));
            let side = [-along[1], along[0]];
            let d = if dot(sub(p.c, a), side) > 0.0 {
                side
            } else {
                [-side[0], -side[1]]
            };
            let (from, to) = (add(p.c, d, -9.0), add(p.c, d, -0.3));
            let others = c
                .posts
                .iter()
                .filter(|q| q.c != p.c)
                .all(|q| seg_dist(q.c, from, to) > 1.4 + q.r);
            let boxes = c.boxes.iter().all(|x| seg_dist(x.c, from, to) > 3.8);
            (others && boxes).then_some((*p, d))
        })
        .expect("no open lamp post");
    for spec in cars() {
        let start = add(lamp.c, d, -(0.5 * spec.dims_m[0] + 5.0));
        let mut s = launched(&spec, [start[0], 0.0, start[1]], heading_of(d), v);
        let mut hit = false;
        let mut closest = f32::MAX;
        for _ in 0..3 * STEPS_PER_S {
            let inp = if hit {
                VehicleInput::default()
            } else {
                cruise(&s, v)
            };
            hit |= tick(&spec, &mut s, inp, &c).contact;
            assert_finite(&s);
            // The lamp stays in front of the nose: never inside the body.
            let ahead = dot(sub(lamp.c, xz(s.pos)), forward2(&s));
            closest = closest.min(ahead - 0.5 * spec.dims_m[0]);
        }
        println!(
            "{:15} lamp at 30 mph: closest nose-to-pole-center {closest:.3} m, final speed {:.3} m/s",
            spec.id,
            hspeed(&s)
        );
        assert!(hit, "{} missed the lamp", spec.id);
        assert!(closest > 0.0, "{} tunneled into the lamp", spec.id);
        assert!(hspeed(&s) < 0.2, "{} did not stop", spec.id);
    }
}

#[test]
fn walls_hold_at_seventy_meters_per_second() {
    let (_, c) = town();
    let (_, wall) = open_wall(&c, 6.0, 14.0);
    let mid = add(wall.a, sub(wall.b, wall.a), 0.5);
    let spec = load(CORSAIR);
    for v in [40.0, 70.0] {
        let start = add(mid, wall.n, 1.0 + 0.5 * spec.dims_m[0]);
        let y = c.level_at(start).0;
        let mut s = VehicleState::spawn(
            &spec,
            [start[0], y, start[1]],
            heading_of([-wall.n[0], -wall.n[1]]),
        );
        let f = s.forward();
        s.vel = [f[0] * v, 0.0, f[2] * v];
        let mut worst: f32 = f32::MIN;
        for _ in 0..STEPS_PER_S {
            tick(&spec, &mut s, VehicleInput::default(), &c);
            assert_finite(&s);
            worst = worst.max(behind_and_gap(&s, &spec, &wall).0);
        }
        println!("corsair_gtx     wall at {v} m/s: deepest {worst:+.4} m");
        assert!(worst < 0.02, "passed the wall at {v} m/s: {worst}");
    }
}

#[test]
fn parked_on_the_strip_stays_put() {
    let (_, c) = town();
    for spec in cars() {
        // The east-west arterial through the commercial strip, eastbound lane.
        let mut s = VehicleState::spawn(&spec, [150.0, 0.0, 3.5], heading_of([1.0, 0.0]));
        let idle = VehicleInput::default();
        for _ in 0..2 * STEPS_PER_S {
            tick(&spec, &mut s, idle, &c);
        }
        let settled = s.pos;
        let mut worst_v: f32 = 0.0;
        for _ in 0..10 * STEPS_PER_S {
            assert!(!tick(&spec, &mut s, idle, &c).contact);
            worst_v = worst_v.max((s.vel[0].powi(2) + s.vel[1].powi(2) + s.vel[2].powi(2)).sqrt());
        }
        let moved = ((s.pos[0] - settled[0]).powi(2) + (s.pos[2] - settled[2]).powi(2)).sqrt();
        println!(
            "{:15} hands-off on the strip: moved {moved:.2e} m, worst {worst_v:.2e} m/s",
            spec.id
        );
        assert!(moved < 1e-3 && worst_v < 0.01, "{} moved {moved}", spec.id);
        assert!(s.wheels.iter().all(|w| w.grounded));
    }
}

/// From the middle of a street, square toward a house front: over the curb, across the
/// yard, into the wall. Returns the start and the direction.
fn yard_run(c: &WorldCollider) -> ([f32; 2], [f32; 2]) {
    // Walls nearest the first block's east street first, so the pick does not depend on
    // the order of the Place's tables.
    const NEAR: [f32; 2] = [-300.0, -380.0];
    let mut order: Vec<usize> = (0..c.walls.len()).collect();
    let d2 = |i: usize| {
        let w = &c.walls[i];
        let m = add(w.a, sub(w.b, w.a), 0.5);
        let v = sub(m, NEAR);
        dot(v, v)
    };
    order.sort_by(|&a, &b| d2(a).total_cmp(&d2(b)));
    for i in order {
        let w = &c.walls[i];
        let len = dot(sub(w.b, w.a), sub(w.b, w.a)).sqrt();
        if len < 6.0 {
            continue;
        }
        let mid = add(w.a, sub(w.b, w.a), 0.5);
        // Walk out from the wall to the first drop to the asphalt.
        let Some(k) = (1..120)
            .map(|k| k as f32 * 0.25)
            .find(|&t| c.level_at(add(mid, w.n, t)).0 == 0.0)
        else {
            continue;
        };
        let start = add(mid, w.n, k + 5.5);
        let lv = levels(c, add(mid, w.n, 0.1), add(mid, w.n, k + 8.0), 1.0);
        let ok = lv.iter().all(|l| {
            let j = l.iter().position(|&v| v == 0.0);
            matches!(j, Some(j) if j > 0 && l[..j].iter().all(|&v| v == SIDEWALK_Y) && l[j..].iter().all(|&v| v == 0.0))
        });
        if ok && clear(c, add(mid, w.n, 0.1), add(mid, w.n, k + 8.0), 1.4, Some(i)) {
            return (start, [-w.n[0], -w.n[1]]);
        }
    }
    panic!("no open front yard");
}

fn scripted(
    spec: &VehicleSpec,
    c: &WorldCollider,
    start: [f32; 2],
    d: [f32; 2],
) -> (u64, f32, f32) {
    let mut s = VehicleState::spawn(spec, [start[0], 0.0, start[1]], heading_of(d));
    // Speed when all four wheels first stood on the sidewalk level, and the hardest hit.
    let (mut mounted_at, mut hit) = (0.0f32, 0.0f32);
    for i in 0..10 * STEPS_PER_S {
        let t = i as f32 * DT;
        let inp = if t < 6.0 {
            input(0.7, 0.0)
        } else {
            input(0.0, 0.6)
        };
        hit = hit.max(tick(spec, &mut s, inp, c).speed);
        if mounted_at == 0.0 && s.wheels.iter().all(|w| w.grounded && w.contact[1] > 0.1) {
            mounted_at = hspeed(&s);
        }
        assert_finite(&s);
    }
    (s.hash(), mounted_at, hit)
}

#[test]
fn scripted_drive_is_deterministic_and_pinned() {
    // Pinned hashes of the 10 s drive over a curb into a house. A deliberate change to the
    // sim, the collider, a spec or the town moves them: run
    // `cargo test -p world --test collide scripted -- --nocapture`, copy the printed hashes
    // here, and commit that on its own with the reason.
    const PINNED: [u64; 2] = [0x579a_c325_de0a_ddc1, 0x8f60_30ca_ad8d_42b2];
    let (_, c) = town();
    let (start, d) = yard_run(&c);
    let mut hashes = [0u64; 2];
    for (k, spec) in cars().iter().enumerate() {
        let (h, mounted, hit) = scripted(spec, &c, start, d);
        println!(
            "{:15} scripted {h:#018x}: up the curb at {mounted:.1} m/s, into the wall at {hit:.1} m/s",
            spec.id
        );
        assert!(
            mounted > 0.0 && hit > 1.0,
            "{}: curb {mounted}, wall {hit}",
            spec.id
        );
        assert_eq!(
            h,
            scripted(spec, &c, start, d).0,
            "{} not deterministic",
            spec.id
        );
        hashes[k] = h;
    }
    assert_eq!(hashes, PINNED, "scripted hashes moved (corsair, palisade)");
}

#[test]
fn probe_and_resolve_do_not_allocate() {
    let (_, c) = town();
    let (start, d) = yard_run(&c);
    for spec in cars() {
        let mut s = VehicleState::spawn(&spec, [start[0], 0.0, start[1]], heading_of(d));
        let before = allocations();
        for _ in 0..5 * STEPS_PER_S {
            tick(&spec, &mut s, input(0.7, 0.0), &c);
        }
        assert_eq!(allocations() - before, 0, "{} allocated", spec.id);
    }
}
