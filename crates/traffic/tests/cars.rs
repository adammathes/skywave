//! NPC cars (`traffic::cars`): car following on a straight lane, a signalized crossing,
//! the player as an obstacle, and busy traffic on the coast at seed 1 (signals, lane
//! intervals, spawning out of view, the pinned hash, allocation and the step budget).
//! Run with `--nocapture` to see the numbers: `cargo test -p traffic --test cars --
//! --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock, RwLockReadGuard};

use place::StopLine;
use skywave_core::Seed;
use traffic::cars::{CAR_HALF_W_M, CAR_LEN_M};
use traffic::{
    Car, Clock, Control, Density, Inputs, Junction, Lane, LaneGraph, Light, Player, Traffic,
    TrafficBudget, Turn, TurnKind, View, NONE,
};
use world::procgen::build_place;
use world::template::RegionTemplate;

/// Counts heap allocations made on the current thread, so stepping can be shown to make
/// none while other tests allocate in parallel.
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

/// Pinned [`Traffic::hash`] after 30 s of busy traffic on the coast at seed 1 around a
/// player standing at the start. Moves when the lane graph, the place or the traffic sim
/// changes; update it in its own commit with the reason.
const COAST_SEED1_TRAFFIC_HASH: u64 = 0x9eb9_45c8_a080_070b;

const HZ: usize = 120;
const EAST: f32 = -std::f32::consts::FRAC_PI_2;

/// Every test but the budget runs under a read lock the budget test takes for writing, so
/// it times the step on a quiet machine.
static QUIET: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    QUIET.read().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------
// Hand-built graphs.

/// A lane graph built lane by lane: straight lanes, straight turns.
struct Builder {
    g: LaneGraph,
    turns: Vec<Turn>,
}

impl Builder {
    fn new(nodes: usize) -> Builder {
        let g = LaneGraph {
            junctions: vec![
                Junction {
                    degree: 1,
                    phases: 0,
                    offset: 0,
                };
                nodes
            ],
            ..LaneGraph::default()
        };
        Builder {
            g,
            turns: Vec::new(),
        }
    }

    /// A straight lane from `a` to `b` (ground points), `[start, end]` nodes.
    fn lane(
        &mut self,
        a: [f32; 2],
        b: [f32; 2],
        nodes: [u32; 2],
        control: Control,
        phase: u8,
    ) -> u32 {
        let g = &mut self.g;
        let id = g.lanes.len() as u32;
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let first = g.points.len() as u32;
        g.points.push([a[0], 0.0, a[1]]);
        g.points.push([b[0], 0.0, b[1]]);
        g.arc.extend([0.0, len]);
        let stop_line = if matches!(control, Control::Stop | Control::Signal) {
            g.stop_lines.push(StopLine {
                edge: id,
                node: nodes[1],
                a: [b[0], 0.0, b[1]],
                b: [b[0], 0.0, b[1]],
            });
            g.stop_lines.len() as u32 - 1
        } else {
            NONE
        };
        g.lanes.push(Lane {
            edge: id,
            forward: true,
            slot: 0,
            phase,
            control,
            start_node: nodes[0],
            end_node: nodes[1],
            first_point: first,
            point_count: 2,
            length_m: len,
            speed_mps: 13.4,
            stop_line,
            first_turn: 0,
            turn_count: 0,
        });
        id
    }

    fn turn(&mut self, from: u32, to: u32) {
        let (a, b) = (self.g.end_pose(from).pos, self.g.pose(to, 0.0).pos);
        let at = |t: f32| [a[0] + (b[0] - a[0]) * t, 0.0, a[2] + (b[2] - a[2]) * t];
        self.turns.push(Turn {
            node: self.g.lanes[from as usize].end_node,
            from,
            to,
            kind: TurnKind::Straight,
            curve: [a, at(1.0 / 3.0), at(2.0 / 3.0), b],
            length_m: ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt(),
            min_radius_m: f32::INFINITY,
        });
    }

    fn build(mut self) -> LaneGraph {
        self.turns.sort_by_key(|t| t.from);
        for (k, t) in self.turns.iter().enumerate() {
            let l = &mut self.g.lanes[t.from as usize];
            if l.turn_count == 0 {
                l.first_turn = k as u32;
            }
            l.turn_count += 1;
        }
        self.g.turns = self.turns;
        self.g
    }
}

/// One lane, 5 km due east along z = 0, ending nowhere.
fn straight() -> LaneGraph {
    let mut b = Builder::new(2);
    b.lane([0.0, 0.0], [5000.0, 0.0], [0, 1], Control::Free, 0);
    b.build()
}

/// A signalized crossing at the origin: lane 0 eastbound (phase 0) and lane 2 northbound
/// (phase 1) stop 12 m short of it and carry on as lanes 1 and 3.
fn crossing() -> LaneGraph {
    let mut b = Builder::new(5);
    b.lane([-400.0, 1.8], [-12.0, 1.8], [1, 0], Control::Signal, 0);
    b.lane([12.0, 1.8], [2000.0, 1.8], [0, 2], Control::Free, 0);
    b.lane([1.8, 400.0], [1.8, 12.0], [3, 0], Control::Signal, 1);
    b.lane([1.8, -12.0], [1.8, -2000.0], [0, 4], Control::Free, 0);
    b.turn(0, 1);
    b.turn(2, 3);
    let mut g = b.build();
    g.junctions[0] = Junction {
        degree: 4,
        phases: 2,
        offset: 0,
    };
    g
}

fn inputs(tick: u64, player: Player, view: View, density: Density) -> Inputs {
    Inputs {
        tick,
        player,
        view,
        clock: Clock {
            seconds_of_day: 12.0 * 3600.0,
            dusk_s: 19.0 * 3600.0,
            dawn_s: 6.0 * 3600.0,
        },
        density,
    }
}

const NO_VIEW: View = View {
    pos: [1.0e7, 0.0, 1.0e7],
    dir: [0.0, -1.0],
    tan_half: 1.0,
};

/// The shipped coast grammar: `build_place` dresses buildings from it.
fn grammar() -> world::Grammar {
    ron::from_str(include_str!(
        "../../../assets/grammar/california_coast_80s.ron"
    ))
    .expect("parse grammar")
}

fn scripted(tick: u64, player: Player) -> Inputs {
    inputs(tick, player, NO_VIEW, Density::Off)
}

/// Corners of a car-sized box around `pos` facing `heading`.
fn corners(pos: [f32; 3], heading: f32, half_len: f32, half_w: f32) -> [[f32; 2]; 4] {
    let (sn, cs) = heading.sin_cos();
    let (f, r) = ([-sn, -cs], [cs, -sn]);
    let c = [pos[0], pos[2]];
    let at = |a: f32, b: f32| [c[0] + f[0] * a + r[0] * b, c[1] + f[1] * a + r[1] * b];
    [
        at(half_len, half_w),
        at(half_len, -half_w),
        at(-half_len, -half_w),
        at(-half_len, half_w),
    ]
}

/// Whether two convex quads overlap (separating axes).
fn overlap(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    for poly in [a, b] {
        for k in 0..4 {
            let (p, q) = (poly[k], poly[(k + 1) % 4]);
            let n = [q[1] - p[1], p[0] - q[0]];
            let proj = |s: &[[f32; 2]; 4]| {
                s.iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                        let d = v[0] * n[0] + v[1] * n[1];
                        (lo.min(d), hi.max(d))
                    })
            };
            let (x, y) = (proj(a), proj(b));
            if x.1 <= y.0 || y.1 <= x.0 {
                return false;
            }
        }
    }
    true
}

fn car_box(c: &Car) -> [[f32; 2]; 4] {
    corners(c.pos, c.heading, CAR_LEN_M * 0.5, CAR_HALF_W_M)
}

fn player_box(p: &Player) -> [[f32; 2]; 4] {
    corners(p.pos, p.heading, p.half_len, p.half_w)
}

/// Asserts no two cars on one lane or turn are closer than a car length.
fn assert_lane_intervals(t: &Traffic, when: &str) {
    let mut by_seg: BTreeMap<u32, Vec<f32>> = BTreeMap::new();
    for c in t.cars() {
        by_seg.entry(c.seg).or_default().push(c.s);
    }
    for (seg, mut s) in by_seg {
        s.sort_by(f32::total_cmp);
        for w in s.windows(2) {
            assert!(
                w[1] - w[0] >= CAR_LEN_M,
                "{when}: two cars {:.2} m apart on segment {seg}",
                w[1] - w[0]
            );
        }
    }
}

/// Asserts every car that was there a step ago moved no further than its speed allows
/// and turned by a bounded angle: no jumps across lanes, turns or lane changes.
fn assert_continuous(before: &[Car], cars: &[Car], when: &str) {
    for c in cars {
        let Some(o) = before.iter().find(|o| o.id == c.id) else {
            continue;
        };
        let moved = ((c.pos[0] - o.pos[0]).powi(2) + (c.pos[2] - o.pos[2]).powi(2)).sqrt();
        let most = c.speed.max(o.speed) / HZ as f32 * 1.05 + 0.02;
        assert!(
            moved <= most,
            "{when}: car {} jumped {moved:.3} m (at most {most:.3}) on segment {}",
            c.id,
            c.seg
        );
        let mut turn = (c.heading - o.heading).abs();
        if turn > std::f32::consts::PI {
            turn = std::f32::consts::TAU - turn;
        }
        assert!(
            turn < 0.1,
            "{when}: car {} turned {turn:.3} rad in a step",
            c.id
        );
    }
}

/// Asserts no two cars' bodies overlap.
fn assert_no_contact(cars: &[Car], when: &str) {
    for (k, a) in cars.iter().enumerate() {
        for b in &cars[k + 1..] {
            let close = (a.pos[0] - b.pos[0]).abs() < 6.0 && (a.pos[2] - b.pos[2]).abs() < 6.0;
            assert!(
                !close || !overlap(&car_box(a), &car_box(b)),
                "{when}: cars {} and {} touch",
                a.id,
                b.id
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Scripted scenarios.

#[test]
fn a_platoon_settles_to_safe_gaps() {
    let _quiet = shared();
    let g = straight();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(7));
    for k in 0..12 {
        let speed = [6.0, 14.0, 9.0][k % 3];
        t.spawn_at(&g, 0, 300.0 - 15.0 * k as f32, speed)
            .expect("room");
    }
    let mut closest = f32::INFINITY;
    for tick in 0..60 * HZ as u64 {
        t.step(&g, &scripted(tick, Player::ABSENT));
        assert_lane_intervals(&t, "platoon");
        let mut cars: Vec<&Car> = t.cars().iter().collect();
        cars.sort_by(|a, b| b.s.total_cmp(&a.s));
        for w in cars.windows(2) {
            closest = closest.min(w[0].s - CAR_LEN_M - w[1].s);
        }
    }
    let mut cars: Vec<&Car> = t.cars().iter().collect();
    cars.sort_by(|a, b| b.s.total_cmp(&a.s));
    println!(
        "platoon after 60 s: leader at {:.0} m, {:.1} m/s; closest gap ever {closest:.2} m",
        cars[0].s, cars[0].speed
    );
    for w in cars.windows(2) {
        let gap = w[0].s - CAR_LEN_M - w[1].s;
        println!("  {:5.1} m/s, gap {gap:6.1} m", w[1].speed);
        assert!(
            gap >= 2.0 + 0.9 * w[1].speed,
            "gap {gap:.1} m at {:.1} m/s",
            w[1].speed
        );
        assert!(w[1].speed > 5.0, "the platoon flows");
    }
    assert!(closest > 0.0);
}

#[test]
fn cars_stop_at_red_and_go_on_green() {
    let _quiet = shared();
    let g = crossing();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(3));
    assert!(t.conflict(0, 1), "the crossing streams conflict");
    // Per car id: stopped at the line on red, crossed (tick), spawned (tick).
    let mut stopped: BTreeMap<u32, bool> = BTreeMap::new();
    let mut crossed: BTreeMap<u32, u64> = BTreeMap::new();
    let mut spawned: BTreeMap<u32, u64> = BTreeMap::new();
    let total = 240 * HZ as u64;
    for tick in 0..total {
        // A car every 7 s on each approach while there is room.
        if tick % (7 * HZ as u64) == 0 && tick < total - 100 * HZ as u64 {
            for lane in [0, 2] {
                let room = t.cars().iter().all(|c| c.seg != lane || c.s > 40.0);
                if room {
                    let id = t.spawn_at(&g, lane, 20.0, 10.0).expect("pool");
                    spawned.insert(id, tick);
                }
            }
        }
        let before: Vec<(u32, u32)> = t.cars().iter().map(|c| (c.id, c.seg)).collect();
        t.step(&g, &scripted(tick, Player::ABSENT));
        assert_lane_intervals(&t, "crossing");
        let cars = t.cars();
        assert_no_contact(cars, "crossing");
        for c in cars {
            let was = before.iter().find(|b| b.0 == c.id).map(|b| b.1);
            if let Some(lane @ (0 | 2)) = was {
                let light = g.light(lane, tick).expect("signal");
                if c.seg != lane {
                    assert_ne!(light, Light::Red, "car {} ran the red", c.id);
                    crossed.insert(c.id, tick);
                } else if light == Light::Red && g.lanes[lane as usize].length_m - c.s < 3.0 {
                    let e = stopped.entry(c.id).or_insert(false);
                    *e |= c.speed < 0.05;
                }
            }
        }
    }
    let mut waited = 0;
    for (&id, &at) in &spawned {
        let stopped_on_red = stopped.get(&id).copied();
        if let Some(s) = stopped_on_red {
            assert!(s, "car {id} reached the line on red and did not stop");
            waited += 1;
        }
        assert!(
            crossed.contains_key(&id),
            "car {id} (spawned at {at}) never went through"
        );
    }
    println!(
        "crossing: {} cars, {} crossed, {waited} waited at a red",
        spawned.len(),
        crossed.len()
    );
    assert!(waited >= 4, "some cars met a red");
}

/// Brake lamps: a car queued at a red shows them for as long as it stands, the car behind
/// it too, and both go dark pulling away on green; a car closing on a slower one shows them
/// while it slows; a car cruising a free road never shows them.
#[test]
fn queued_cars_show_brake_lamps_and_cruising_ones_do_not() {
    let _quiet = shared();
    let g = straight();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(5));
    t.spawn_at(&g, 0, 20.0, 10.0).expect("room");
    for tick in 0..40 * HZ as u64 {
        t.step(&g, &scripted(tick, Player::ABSENT));
        let c = t.cars()[0];
        // Past the first 20 s it is at its cruising speed.
        if tick > 20 * HZ as u64 {
            assert!(
                !c.braking,
                "cruising at {:.1} m/s shows brake lamps",
                c.speed
            );
        }
    }

    // Closing on a slower car: the follower slows to its pace with its brake lamps lit,
    // and the leader, cruising slowly, never shows them.
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(5));
    let lead = t.spawn_at(&g, 0, 150.0, 4.0).expect("room");
    let follow = t.spawn_at(&g, 0, 20.0, 15.0).expect("room");
    let mut lit = 0;
    for tick in 0..30 * HZ as u64 {
        let before: Vec<Car> = t.cars().to_vec();
        t.step(&g, &scripted(tick, Player::ABSENT));
        for c in t.cars() {
            let b = before.iter().find(|b| b.id == c.id).expect("same cars");
            if c.id == follow && b.speed - c.speed > 1.0 / HZ as f32 {
                assert!(
                    c.braking,
                    "slowing at {:.1} m/s²",
                    (b.speed - c.speed) * HZ as f32
                );
                lit += 1;
            }
            if c.id == lead && tick > 10 * HZ as u64 {
                assert!(!c.braking, "the slow leader shows brake lamps");
            }
        }
    }
    assert!(lit > HZ / 4, "the follower braked for {lit} steps");

    let g = crossing();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(3));
    let (mut queued, mut lit_queued, mut went) = (0, 0, 0);
    for tick in 0..120 * HZ as u64 {
        if tick % (5 * HZ as u64) == 0 && t.cars().iter().all(|c| c.seg != 0 || c.s > 40.0) {
            t.spawn_at(&g, 0, 20.0, 10.0).expect("pool");
        }
        let before: Vec<Car> = t.cars().to_vec();
        t.step(&g, &scripted(tick, Player::ABSENT));
        for c in t.cars() {
            let Some(b) = before.iter().find(|b| b.id == c.id) else {
                continue;
            };
            // Standing: not moving, and not pulling away either.
            if c.seg == 0 && c.speed < 0.05 && c.speed <= b.speed {
                queued += 1;
                lit_queued += usize::from(c.braking);
            }
            if c.speed > 1.0 && c.speed > b.speed {
                went += 1;
                assert!(
                    !c.braking,
                    "car {} pulls away with its brake lamps lit",
                    c.id
                );
            }
        }
    }
    println!("brake lamps: {lit_queued} of {queued} queued car-steps lit, {went} pull-aways");
    assert!(queued > 10 * HZ, "cars queued at the red");
    assert_eq!(
        lit_queued, queued,
        "every car standing in the queue shows its brake lamps"
    );
    assert!(went > 0);
}

/// Two lanes due east (lanes 0 and 1, slot 0 at z = 0, slot 1 at z = 3.5) for 3 km,
/// going straight on into lanes 2 and 3.
fn two_lanes() -> LaneGraph {
    let mut b = Builder::new(3);
    b.lane([0.0, 0.0], [3000.0, 0.0], [0, 1], Control::Free, 0);
    b.lane([0.0, 3.5], [3000.0, 3.5], [0, 1], Control::Free, 0);
    b.lane([3020.0, 0.0], [6000.0, 0.0], [1, 2], Control::Free, 0);
    b.lane([3020.0, 3.5], [6000.0, 3.5], [1, 2], Control::Free, 0);
    b.turn(0, 2);
    b.turn(1, 3);
    let mut g = b.build();
    for (lane, edge, slot) in [(1, 0, 1), (3, 2, 1)] {
        g.lanes[lane].edge = edge;
        g.lanes[lane].slot = slot;
    }
    g
}

#[test]
fn the_player_stopping_dead_is_never_hit_and_is_passed() {
    let _quiet = shared();
    let g = two_lanes();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(11));
    let mut px = 200.0f32;
    for k in 0..6 {
        t.spawn_at(&g, 0, px - 25.0 - 30.0 * k as f32, 12.0)
            .expect("room");
    }
    let mut min_gap = f32::INFINITY;
    let mut changes = 0;
    for tick in 0..120 * HZ as u64 {
        // The player drives lane 0 at 12 m/s, then stops dead at 10 s.
        let speed = if tick < 10 * HZ as u64 { 12.0 } else { 0.0 };
        px += speed / HZ as f32;
        let player = Player {
            pos: [px - 2.3, 0.0, 0.0],
            heading: EAST,
            speed,
            half_len: 2.3,
            half_w: 0.9,
        };
        let before: Vec<u32> = t.cars().iter().map(|c| c.seg).collect();
        t.step(&g, &scripted(tick, player));
        assert_lane_intervals(&t, "player");
        assert_no_contact(t.cars(), "player");
        for (c, was) in t.cars().iter().zip(before) {
            changes += usize::from(was == 0 && c.seg == 1);
            assert!(
                !overlap(&car_box(c), &player_box(&player)),
                "car {} hit the player at tick {tick}",
                c.id
            );
            if c.seg == 0 {
                assert!(c.s <= px - 4.6, "car {} passed through the player", c.id);
                min_gap = min_gap.min(px - 4.6 - c.s);
            }
        }
    }
    let passed = t.cars().iter().filter(|c| c.pos[0] > px + 5.0).count();
    println!(
        "player stopped dead: closest approach {min_gap:.2} m, {changes} lane changes, \
         {passed} of {} cars past",
        t.cars().len()
    );
    assert!(min_gap > 0.5);
    assert_eq!(
        passed,
        t.cars().len(),
        "every car passes the stopped player"
    );
}

/// The player shoves a car running at 10 m/s with a car behind it: forward, sideways and
/// a little yaw. It stands on its brakes (stopped within 2 s, then standing 1-2 s), then
/// drives on and steers back onto its lane line,
/// moving continuously the whole time; the car behind queues behind it and never touches.
#[test]
fn a_shoved_car_brakes_stands_and_steers_back() {
    let _quiet = shared();
    let g = straight();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(5));
    let a = t.spawn_at(&g, 0, 120.0, 10.0).expect("room");
    let b = t.spawn_at(&g, 0, 80.0, 10.0).expect("room");
    let find = |t: &Traffic, id: u32| *t.cars().iter().find(|c| c.id == id).unwrap();
    let hit = 4 * HZ as u64;
    let (mut stood, mut moving_again, mut back) = (None, None, None);
    let (mut widest, mut most_yaw, mut closest) = (0.0f32, 0.0f32, f32::INFINITY);
    for tick in 0..30 * HZ as u64 {
        if tick == hit {
            let i = t.cars().iter().position(|c| c.id == a).unwrap();
            t.shove(i, [2.0, 1.5], 0.4, traffic::cars::FREE_ROOM);
        }
        let before = find(&t, a);
        t.step(&g, &scripted(tick, Player::ABSENT));
        assert_no_contact(t.cars(), "shoved");
        let (ca, cb) = (find(&t, a), find(&t, b));
        let moved =
            ((ca.pos[0] - before.pos[0]).powi(2) + (ca.pos[2] - before.pos[2]).powi(2)).sqrt();
        let slide = (before.slide[0].powi(2) + before.slide[1].powi(2)).sqrt();
        let most = (ca.speed.max(before.speed) + slide) / HZ as f32 * 1.05 + 0.01;
        assert!(moved <= most, "jumped {moved:.3} m at tick {tick}");
        let turn = (ca.heading - before.heading).abs();
        assert!(turn < 0.01, "turned {turn:.4} rad in a step at tick {tick}");
        closest = closest.min(ca.s - cb.s - CAR_LEN_M);
        if tick <= hit {
            continue;
        }
        let since = (tick - hit) as f32 / HZ as f32;
        widest = widest.max(ca.pos[2].abs());
        most_yaw = most_yaw.max((ca.heading - EAST).abs());
        if stood.is_none() && ca.speed == 0.0 {
            stood = Some(since);
        }
        if stood.is_some() && moving_again.is_none() && ca.speed > 0.0 {
            moving_again = Some(since);
        }
        if moving_again.is_some() && back.is_none() && ca.shove == [0.0; 2] && ca.yaw == 0.0 {
            back = Some(since);
            assert!(ca.pos[2].abs() < 1e-4 && (ca.heading - EAST).abs() < 1e-4);
        }
    }
    println!(
        "shoved: stood at {stood:?} s, moving again at {moving_again:?} s, back on its lane \
         at {back:?} s; widest {widest:.2} m off it, yaw at most {most_yaw:.3} rad; the car \
         behind came within {closest:.2} m"
    );
    let stood = stood.expect("the car stopped");
    let moving_again = moving_again.expect("the car drove on");
    // From 14.7 m/s (its own speed and the shove's) at 8 m/s².
    assert!(stood < 2.0, "stopped {stood} s after");
    let stand = moving_again - stood;
    assert!((1.0..=2.05).contains(&stand), "stood {stand} s");
    assert!(widest < 1.0 && most_yaw < 0.3, "{widest} m, {most_yaw} rad");
    assert!(back.expect("back on its lane") < 20.0);
    assert!(closest > 0.3, "{closest}");
}

/// The player's car (the real sim, a hatch-sized coupe) rear-ends the last of two cars
/// standing at a red at 20 km/h and brakes, met with the game's own collision
/// (`world::collide::hit_car`, then `Traffic::shove`). The car hit is shoved less than a
/// meter without touching the car ahead, brakes and is off its brakes within 3 s; on the
/// green it drives off and steers back onto its lane. The bodies never overlap.
#[test]
fn rear_ending_a_queue_at_twenty_shoves_the_last_car() {
    use vehicle::{Assists, FlatGround, VehicleInput, VehicleSpec, VehicleState};
    use world::collide::{hit_car, BoxSolid, OtherCar};
    let _quiet = shared();
    let spec: VehicleSpec =
        ron::from_str(include_str!("../../../assets/vehicles/gannet_gt.ron")).expect("spec");
    let flat = FlatGround {
        y: 0.0,
        friction: 1.0,
    };
    let g = crossing();
    let len = g.lanes[0].length_m;
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(3));
    // Lane 0 is red from 30 s to 60 s of the cycle.
    let red_from = 30 * HZ as u64;
    let ahead = t.spawn_at(&g, 0, len - 1.0, 0.0).expect("room");
    let last = t
        .spawn_at(&g, 0, len - 1.0 - CAR_LEN_M - 2.5, 0.0)
        .expect("room");
    let start = g.pose(0, len - 45.0);
    let mut car = VehicleState::spawn(&spec, start.pos, EAST);
    let v = 20.0 / 3.6;
    let find = |t: &Traffic, id: u32| *t.cars().iter().find(|c| c.id == id).unwrap();
    let (mut hit, mut shoved, mut held, mut off_brakes, mut overlaps) =
        (None, 0.0f32, false, None, 0);
    let mut from = [0.0; 3];
    let mut gone = None;
    for tick in red_from..red_from + 45 * HZ as u64 {
        let input = if hit.is_none() {
            let gap = v - car.speed_mps;
            VehicleInput {
                throttle: (0.3 + gap * 0.5).clamp(0.0, 1.0),
                brake: if gap < -0.5 { 0.5 } else { 0.0 },
                ..VehicleInput::default()
            }
        } else {
            VehicleInput {
                brake: if car.speed_mps > 0.2 { 1.0 } else { 0.0 },
                handbrake: car.speed_mps <= 0.2,
                ..VehicleInput::default()
            }
        };
        vehicle::step(&spec, &mut car, &input, &Assists::default(), &flat);
        for i in 0..t.cars().len() {
            let c = t.cars()[i];
            let other = OtherCar {
                body: BoxSolid {
                    c: [c.pos[0], c.pos[2]],
                    u: c.dir,
                    half: [CAR_LEN_M * 0.5, CAR_HALF_W_M],
                    y0: 0.25,
                    y1: 1.35,
                    kind: world::props::PropKind::ParkedCar { color: 0 },
                    lot: world::collide::NO_LOT,
                },
                vel: [
                    c.dir[0] * c.speed + c.slide[0],
                    c.dir[1] * c.speed + c.slide[1],
                ],
                spin: c.spin,
                mass_kg: 1300.0,
            };
            if let Some(h) = hit_car(&mut car, &spec, &other) {
                if h.bump.speed > 0.0 {
                    t.shove(i, h.dv, h.dspin, traffic::cars::FREE_ROOM);
                    if hit.is_none() {
                        assert_eq!(c.id, last, "hit the car ahead");
                        hit = Some((tick, h.bump.speed));
                        from = c.pos;
                    }
                }
            }
        }
        let f = car.forward();
        let player = Player {
            pos: car.pos,
            heading: traffic::math::atan2(-f[0], -f[2]),
            speed: car.speed_mps.abs(),
            half_len: 0.5 * spec.dims_m[0],
            half_w: 0.5 * spec.dims_m[1],
        };
        let me = player_box(&player);
        overlaps += t
            .cars()
            .iter()
            .filter(|c| overlap(&car_box(c), &me))
            .count();
        t.step(&g, &scripted(tick, player));
        assert_no_contact(t.cars(), "rear-ended");
        let c = find(&t, last);
        if let Some((at, _)) = hit {
            held |= c.hold > 0;
            if tick < red_from + 30 * HZ as u64 {
                let d = ((c.pos[0] - from[0]).powi(2) + (c.pos[2] - from[2]).powi(2)).sqrt();
                shoved = shoved.max(d);
            }
            if held && c.hold == 0 && off_brakes.is_none() {
                off_brakes = Some((tick - at) as f32 / HZ as f32);
            }
            if gone.is_none() && c.seg != 0 {
                gone = Some((c.shove, c.yaw, find(&t, ahead).seg));
            }
        }
    }
    println!(
        "rear-ended at 20 km/h: hit {hit:?}, shoved {shoved:.2} m, off the brakes after \
         {off_brakes:?} s, {overlaps} steps overlapping, on through the junction with \
         {gone:?}, the player at {:.2} m/s",
        car.speed_mps
    );
    let (_, closing) = hit.expect("reached the queue");
    assert!((v - 1.0..v + 0.5).contains(&closing), "{closing} m/s");
    assert!(held && shoved < 1.0, "{held} {shoved}");
    assert!(off_brakes.expect("off the brakes") <= 3.0);
    let (shove, yaw, _) = gone.expect("drove off on the green");
    assert!(shove == [0.0; 2] && yaw == 0.0, "{shove:?} {yaw}");
    assert_eq!(overlaps, 0);
    assert!(car.speed_mps.abs() < 0.3);
}

/// Ten broadside rams in a row (4 m/s across and the yaw cap each, half a second apart),
/// then a three-second push at 2 m/s, on a car standing in its lane: the car never goes
/// more than `SHOVE_SIDE_M` off its lane's line nor turns more than `SHOVE_YAW`, and once
/// left alone it drives back onto its line.
#[test]
fn rams_never_push_a_car_off_its_lane() {
    use traffic::cars::{SHOVE_SIDE_M, SHOVE_YAW};
    let _quiet = shared();
    let g = straight();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(5));
    let a = t.spawn_at(&g, 0, 300.0, 0.0).expect("room");
    let (mut widest, mut most_yaw) = (0.0f32, 0.0f32);
    let mut back = None;
    let rams = 10 * HZ as u64 / 2;
    let push = rams + 3 * HZ as u64;
    for tick in 0..40 * HZ as u64 {
        let i = t.cars().iter().position(|c| c.id == a).unwrap();
        if tick < rams && tick % (HZ as u64 / 2) == 0 {
            t.shove(i, [0.0, 4.0], 0.6, traffic::cars::FREE_ROOM);
        } else if (rams..push).contains(&tick) {
            // 2 m/s held against it: what it loses to friction each step, given back.
            t.shove(i, [0.0, 8.0 / HZ as f32], 0.0, traffic::cars::FREE_ROOM);
        }
        t.step(&g, &scripted(tick, Player::ABSENT));
        let c = t.cars()[i];
        widest = widest.max(c.pos[2].abs());
        most_yaw = most_yaw.max((c.heading - EAST).abs());
        if tick > push && back.is_none() && c.shove == [0.0; 2] && c.yaw == 0.0 {
            back = Some((tick - push) as f32 / HZ as f32);
        }
    }
    println!(
        "ten rams and a 3 s push: at most {widest:.3} m off the line, {most_yaw:.3} rad \
         turned; back on its line {back:?} s after"
    );
    assert!(widest <= SHOVE_SIDE_M + 1e-4, "{widest} m");
    assert!(most_yaw <= SHOVE_YAW + 1e-4, "{most_yaw} rad");
    assert!(back.expect("back on its line") < 15.0);
}

/// The player's car stands across a one-lane road: the cars coming queue behind it and
/// none passes through or touches it.
#[test]
fn the_player_parked_across_a_lane_holds_the_queue() {
    let _quiet = shared();
    let g = straight();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(9));
    for k in 0..5 {
        t.spawn_at(&g, 0, 200.0 - 30.0 * k as f32, 12.0)
            .expect("room");
    }
    let player = Player {
        pos: [400.0, 0.0, 0.3],
        heading: 0.0,
        speed: 0.0,
        half_len: 2.3,
        half_w: 0.9,
    };
    for tick in 0..60 * HZ as u64 {
        t.step(&g, &scripted(tick, player));
        assert_no_contact(t.cars(), "across");
        for c in t.cars() {
            assert!(
                !overlap(&car_box(c), &player_box(&player)),
                "car {} touched the player at tick {tick}",
                c.id
            );
            assert!(c.pos[0] < 400.0, "car {} passed through", c.id);
        }
    }
    let mut s: Vec<f32> = t.cars().iter().map(|c| c.s).collect();
    s.sort_by(f32::total_cmp);
    let head = 400.0 - 0.9 - s[s.len() - 1];
    println!("player across the lane: the head of the queue {head:.2} m short (front)");
    assert!(t.cars().iter().all(|c| c.speed < 0.01), "the queue stands");
    assert!(head > 0.5 && head < 5.0, "{head}");
}

#[test]
fn a_player_waiting_at_a_red_does_not_stop_the_green() {
    let _quiet = shared();
    let g = crossing();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(5));
    // Stopped at lane 0's line, all the time.
    let player = Player {
        pos: [-12.0 - 0.6 - 2.3, 0.0, 1.8],
        heading: EAST,
        speed: 0.0,
        half_len: 2.3,
        half_w: 0.9,
    };
    let mut crossed = 0;
    for tick in 0..150 * HZ as u64 {
        if tick % (7 * HZ as u64) == 0 && t.cars().iter().all(|c| c.seg != 2 || c.s > 40.0) {
            t.spawn_at(&g, 2, 20.0, 10.0).expect("pool");
        }
        let before: Vec<(u32, u32)> = t.cars().iter().map(|c| (c.id, c.seg)).collect();
        t.step(&g, &scripted(tick, player));
        assert_no_contact(t.cars(), "player at red");
        for c in t.cars() {
            assert!(!overlap(&car_box(c), &player_box(&player)));
            if before.contains(&(c.id, 2)) && c.seg != 2 {
                crossed += 1;
                assert_ne!(g.light(2, tick), Some(Light::Red));
            }
        }
    }
    println!("player waiting at the red: {crossed} cross-traffic cars went through");
    assert!(crossed >= 10);
}

#[test]
fn a_claim_held_up_behind_a_car_is_given_back() {
    let _quiet = shared();
    let g = crossing();
    let mut t = Traffic::new(&g, TrafficBudget::default(), Seed(9));
    let len = g.lanes[0].length_m;
    // Lane 0 is green until tick 3000, yellow to 3420, red after; lane 2 green from 3600.
    let a = t.spawn_at(&g, 0, len - 30.0, 12.0).expect("pool");
    let cross = t.spawn_at(&g, 2, 300.0, 10.0).expect("pool");
    let held = |t: &Traffic, id: u32| t.cars().iter().find(|c| c.id == id).map(|c| c.held);
    let mut cross_went = None;
    for tick in 2950..6600u64 {
        if tick == 3005 {
            // A car appears stopped between A and the line: A must give its claim back.
            assert_ne!(held(&t, a), Some(NONE), "A claimed its turn on green");
            t.spawn_at(&g, 0, len - 8.0, 0.0).expect("pool");
        }
        t.step(&g, &scripted(tick, Player::ABSENT));
        assert_no_contact(t.cars(), "claim");
        if tick > 3420 {
            assert_eq!(held(&t, a), Some(NONE), "A kept its claim into the red");
        }
        let on_lane2 = t.cars().iter().any(|c| c.id == cross && c.seg == 2);
        if cross_went.is_none() && !on_lane2 {
            cross_went = Some(tick);
        }
    }
    println!("held-up claim: cross traffic went at tick {cross_went:?}");
    let went = cross_went.expect("the cross traffic went");
    assert!((3600..6600).contains(&went));
}

#[test]
fn off_spawns_nothing() {
    let _quiet = shared();
    let b = coast();
    let mut t = b.traffic.clone();
    for tick in 0..10 * HZ as u64 {
        let inp = inputs(tick, b.player, b.view_at(0.0), Density::Off);
        t.step(&b.graph, &inp);
        t.populate(&b.graph, &inp);
        assert!(t.cars().is_empty());
    }
}

// ---------------------------------------------------------------------------------------
// The coast at seed 1.

struct Coast {
    graph: LaneGraph,
    traffic: Traffic,
    /// The player standing still on a lane near the start, facing along it.
    player: Player,
}

impl Coast {
    /// A camera behind the player turned `yaw` radians left of the player's heading.
    fn view_at(&self, yaw: f32) -> View {
        let (sn, cs) = (self.player.heading + yaw).sin_cos();
        let dir = [-sn, -cs];
        View {
            pos: [
                self.player.pos[0] - dir[0] * 6.0,
                0.0,
                self.player.pos[2] - dir[1] * 6.0,
            ],
            dir,
            tan_half: 1.0,
        }
    }
}

impl Coast {
    fn new(region: &str, seed: u64) -> Coast {
        let t: RegionTemplate = ron::from_str(region).expect("parse");
        let place = build_place(Seed(seed), &t, &grammar());
        let graph = LaneGraph::new(&place);
        let start = place.player_start().expect("start").pos;
        // The nearest point of a lane long enough to queue on.
        let (mut best, mut at) = (f32::INFINITY, (0u32, 0.0f32));
        for (id, l) in graph.lanes.iter().enumerate() {
            if l.length_m < 80.0 {
                continue;
            }
            let mut s = 30.0;
            while s < l.length_m - 30.0 {
                let p = graph.pose(id as u32, s).pos;
                let d = (p[0] - start[0]).powi(2) + (p[2] - start[2]).powi(2);
                if d < best {
                    (best, at) = (d, (id as u32, s));
                }
                s += 5.0;
            }
        }
        let pose = graph.pose(at.0, at.1);
        let player = Player {
            pos: pose.pos,
            heading: pose.heading,
            speed: 0.0,
            half_len: 2.3,
            half_w: 0.9,
        };
        let traffic = Traffic::new(&graph, TrafficBudget::default(), Seed(seed));
        Coast {
            graph,
            traffic,
            player,
        }
    }
}

const COAST: &str = include_str!("../../../assets/regions/california_coast.ron");

fn coast() -> &'static Coast {
    static COAST_SEED1: OnceLock<Coast> = OnceLock::new();
    COAST_SEED1.get_or_init(|| Coast::new(COAST, 1))
}

/// Steps `t` with step and populate for `seconds` at `density` with the camera turning a
/// full circle every 40 s, calling `check` after every step with the inputs and the cars
/// before it.
fn drive(
    b: &Coast,
    t: &mut Traffic,
    seconds: usize,
    density: Density,
    mut check: impl FnMut(&Traffic, &Inputs, &[Car]),
) {
    let mut before: Vec<Car> = Vec::with_capacity(t.budget.cars);
    for tick in 0..(seconds * HZ) as u64 {
        let yaw = tick as f32 / (40.0 * HZ as f32) * std::f32::consts::TAU;
        let inp = inputs(tick, b.player, b.view_at(yaw), density);
        before.clear();
        before.extend_from_slice(t.cars());
        t.step(&b.graph, &inp);
        t.populate(&b.graph, &inp);
        check(t, &inp, &before);
    }
}

#[test]
fn coast_traffic_obeys_signals_keeps_apart_and_hides_its_comings_and_goings() {
    let _quiet = shared();
    let b = coast();
    let g = &b.graph;
    let mut t = b.traffic.clone();
    let target = Density::Busy.target(t.budget.cars);
    let (mut entered, mut spawns, mut despawns, mut least, mut most) = (0, 0, 0, usize::MAX, 0);
    let (mut changes, mut stops, mut through) = (0, 0, 0);
    let pbox = player_box(&b.player);
    let mut tick = 0u64;
    drive(b, &mut t, 120, Density::Busy, |t, inp, before| {
        let cars = t.cars();
        assert!(cars.len() <= target);
        if tick >= 60 * HZ as u64 {
            least = least.min(cars.len());
        }
        most = most.max(cars.len());
        assert_lane_intervals(t, "coast");
        assert_no_contact(cars, "coast");
        assert_continuous(before, cars, "coast");
        for c in cars {
            assert!(!overlap(&car_box(c), &pbox), "car {} hit the player", c.id);
            match before.iter().find(|o| o.id == c.id) {
                None => {
                    spawns += 1;
                    assert!(!inp.view.sees(c.pos, 0.0), "car {} spawned in view", c.id);
                }
                Some(o) if o.seg < t_lanes(g) && o.seg != c.seg => {
                    if c.seg < t_lanes(g) {
                        changes += 1;
                        continue;
                    }
                    through += 1;
                    let l = &g.lanes[o.seg as usize];
                    if l.control == Control::Signal {
                        entered += 1;
                        let light = g.light(o.seg, inp.tick).expect("signal");
                        assert_ne!(light, Light::Red, "car {} entered on red", c.id);
                    }
                }
                Some(o) => {
                    let l = o.seg < t_lanes(g) && g.lanes[o.seg as usize].control == Control::Stop;
                    if l && o.stop != c.stop && c.stop == traffic::cars::StopState::Done {
                        stops += 1;
                    }
                }
            }
        }
        for o in before {
            if !cars.iter().any(|c| c.id == o.id) {
                despawns += 1;
                assert!(!inp.view.sees(o.pos, 0.0), "car {} vanished in view", o.id);
            }
        }
        tick += 1;
    });
    println!(
        "coast busy 120 s: {least}..{most} cars (target {target}); {spawns} spawned, \
         {despawns} despawned; {through} turns taken, {entered} at signals, {stops} stop \
         signs obeyed, {changes} lane changes"
    );
    assert!(least * 2 >= target, "only {least} cars of {target}");
    assert!(entered > 0 && stops > 0 && through > 50);
    assert!(despawns > 0);
}

fn t_lanes(g: &LaneGraph) -> u32 {
    g.lanes.len() as u32
}

#[test]
fn lower_densities_keep_their_count() {
    let _quiet = shared();
    for (density, seconds) in [(Density::Low, 40), (Density::Normal, 40)] {
        let b = coast();
        let mut t = b.traffic.clone();
        let target = density.target(t.budget.cars);
        let mut least = usize::MAX;
        let mut tick = 0;
        drive(b, &mut t, seconds, density, |t, _, _| {
            assert!(t.cars().len() <= target);
            if tick >= 30 * HZ {
                least = least.min(t.cars().len());
            }
            tick += 1;
        });
        println!("{density:?}: {least}..{target} cars after 30 s");
        assert!(
            least * 10 >= target * 9,
            "{density:?}: only {least} of {target}"
        );
    }
}

/// Every region at seeds 1 to 3, a minute of busy traffic each: no contact, no red run.
#[test]
fn every_region_drives_without_contact() {
    let _quiet = shared();
    let regions = [
        COAST,
        include_str!("../../../assets/regions/great_lakes_city.ron"),
        include_str!("../../../assets/regions/north_shore_suburb.ron"),
    ];
    std::thread::scope(|scope| {
        for (r, region) in regions.into_iter().enumerate() {
            for seed in 1..=3 {
                scope.spawn(move || {
                    let b = Coast::new(region, seed);
                    let g = &b.graph;
                    let mut t = b.traffic.clone();
                    let (mut through, mut most) = (0, 0);
                    drive(&b, &mut t, 60, Density::Busy, |t, inp, before| {
                        let when = format!("region {r} seed {seed} tick {}", inp.tick);
                        assert_no_contact(t.cars(), &when);
                        assert_continuous(before, t.cars(), &when);
                        assert_lane_intervals(t, &when);
                        most = most.max(t.cars().len());
                        for c in t.cars() {
                            let Some(o) = before.iter().find(|o| o.id == c.id) else {
                                continue;
                            };
                            if o.seg < t_lanes(g) && c.seg >= t_lanes(g) {
                                through += 1;
                                assert_ne!(g.light(o.seg, inp.tick), Some(Light::Red), "{when}");
                            }
                        }
                    });
                    println!("region {r} seed {seed}: up to {most} cars, {through} turns taken");
                    assert!(through > 20);
                });
            }
        }
    });
}

fn coast_hash(seconds: usize) -> u64 {
    let b = coast();
    let mut t = b.traffic.clone();
    for tick in 0..(seconds * HZ) as u64 {
        let inp = inputs(tick, b.player, b.view_at(0.0), Density::Busy);
        t.step(&b.graph, &inp);
        t.populate(&b.graph, &inp);
    }
    t.hash()
}

#[test]
fn coast_traffic_hash_is_pinned() {
    let _quiet = shared();
    let h = coast_hash(30);
    println!("coast seed 1 traffic hash after 30 s: {h:#018x}");
    assert_eq!(h, coast_hash(30), "the same inputs give the same traffic");
    assert_eq!(h, COAST_SEED1_TRAFFIC_HASH);
}

#[test]
fn stepping_does_not_allocate() {
    let _quiet = shared();
    let b = coast();
    let mut t = b.traffic.clone();
    let before = allocations();
    for tick in 0..20 * HZ as u64 {
        let inp = inputs(
            tick,
            b.player,
            b.view_at(tick as f32 * 0.001),
            Density::Busy,
        );
        t.step(&b.graph, &inp);
        t.populate(&b.graph, &inp);
    }
    let n = t.cars().len();
    assert_eq!(allocations() - before, 0, "stepping {n} cars allocated");
    assert!(n > 48);
}

/// Per-step cost of 96 cars (step and populate) in this build, as the best p99 of three
/// 5 s windows (a busy machine slows some); asserted only under `SKYWAVE_ENFORCE_BUDGETS`.
#[test]
fn ninety_six_cars_step_within_budget() {
    let _quiet = QUIET.write().unwrap_or_else(|e| e.into_inner());
    let b = coast();
    let mut t = b.traffic.clone();
    let window = 5 * HZ;
    let mut times = Vec::with_capacity(window);
    // Per 5 s window: (median, p99). The median is the gate: it is what the sim costs
    // when the machine is not busy, and it does not move when a shared CI runner or a
    // container running three builds steals a few slices. The p99 only catches gross
    // regressions, so it has fourfold slack.
    let mut windows = Vec::new();
    let mut tick = 0u64;
    while tick < 120 * HZ as u64 && windows.len() < 3 {
        let inp = inputs(tick, b.player, b.view_at(0.0), Density::Busy);
        let start = std::time::Instant::now();
        t.step(&b.graph, &inp);
        t.populate(&b.graph, &inp);
        let dt = start.elapsed().as_secs_f64() * 1e3;
        if t.cars().len() == t.budget.cars {
            times.push(dt);
            if times.len() == window {
                times.sort_by(f64::total_cmp);
                let median = times[times.len() / 2];
                let p99 = times[times.len() * 99 / 100];
                println!(
                    "96 cars: median {median:.3} ms, p99 {p99:.3} ms, worst {:.3} ms per step",
                    times[times.len() - 1]
                );
                windows.push((median, p99));
                times.clear();
            }
        }
        tick += 1;
    }
    assert_eq!(windows.len(), 3, "the pool was not full long enough");
    let median = windows.iter().map(|w| w.0).fold(f64::INFINITY, f64::min);
    let p99 = windows.iter().map(|w| w.1).fold(f64::INFINITY, f64::min);
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(median < 0.5, "median {median:.3} ms per step");
        assert!(p99 < 2.0, "p99 {p99:.3} ms per step");
    }
}

#[test]
fn shape_weights_pick_shapes_and_all_ones_change_nothing() {
    let g = straight();
    let budget = TrafficBudget {
        shapes: 12,
        ..TrafficBudget::default()
    };
    let spawn = |weights: Option<&[u32]>| {
        let mut t = Traffic::new(&g, budget, Seed(5));
        if let Some(w) = weights {
            t.set_shape_weights(w);
        }
        for k in 0..90 {
            t.spawn_at(&g, 0, 4500.0 - 40.0 * k as f32, 10.0)
                .expect("room");
        }
        t.cars().iter().map(|c| c.shape).collect::<Vec<u8>>()
    };
    let plain = spawn(None);
    assert_eq!(spawn(Some(&[1; 12])), plain);
    assert_eq!(spawn(Some(&[0; 12])), plain, "no weight at all: unweighted");
    // Shape 4 weighs nothing: it never comes, and the others still do.
    let mut w = [1; 12];
    w[4] = 0;
    let shapes = spawn(Some(&w));
    assert!(!shapes.contains(&4));
    assert!((0..12u8).filter(|&k| k != 4).all(|k| shapes.contains(&k)));
    // Heavier shapes come more often.
    let mut w = [1; 12];
    w[7] = 40;
    let shapes = spawn(Some(&w));
    assert!(shapes.iter().filter(|&&k| k == 7).count() > 45);
}
