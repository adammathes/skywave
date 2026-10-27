//! Pedestrians on the procedural places at seed 1: the sidewalk graph lies on the
//! sidewalks and crosswalks the world draws; the simulation keeps people on it and out of
//! the lanes, crosses on the walk phase, steps aside for a car pass without anyone ever
//! inside its footprint, keeps groups together, holds the density bands without spawning
//! in view, is deterministic (pinned hash) and allocates nothing after construction. Run
//! with `--nocapture` to see the numbers: `cargo test -p traffic --test peds -- --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use place::Place;
use skywave_core::Seed;
use traffic::peds::{
    Action, PedInput, PedestrianBudget, Pedestrians, DESPAWN_M, SPAWN_FAR_M, SPAWN_NEAR_M,
};
use traffic::walks::{LinkKind, WalkGraph, CELL_M};
use traffic::{Density, LaneGraph, Light, Player, View, NONE};
use world::collide::{Surface, WorldCollider};
use world::procgen::build_place;
use world::template::RegionTemplate;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

/// Counts heap allocations made on the current thread.
struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|n| n.set(n.get() + 1));
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

const REGIONS: [(&str, &str); 3] = [
    (
        "california_coast",
        include_str!("../../../assets/regions/california_coast.ron"),
    ),
    (
        "great_lakes_city",
        include_str!("../../../assets/regions/great_lakes_city.ron"),
    ),
    (
        "north_shore_suburb",
        include_str!("../../../assets/regions/north_shore_suburb.ron"),
    ),
];

/// Pinned [`Pedestrians::hash`] after 30 s on the coast at seed 1 ([`thirty_seconds`]).
/// The sim is a pure function of the place and its inputs, so this moves when pedestrian
/// behaviour, the sidewalk graph, the lane graph or the place change; update it in its own
/// commit with the reason.
const COAST_SEED1_30S_HASH: u64 = 0xa36b_2bc7_215a_2e43;

/// Most walk samples `world` lays another road's asphalt over (where two roads squeeze
/// together it draws one's pavement across the other's sidewalk).
const MAX_WALK_ON_ASPHALT: f32 = 0.03;
/// People off the crossings stay this far from every lane and turn centerline: wider
/// than any lane's half width (a 10 m local street's single lane is 4.55 m wide).
const LANE_CLEAR_M: f32 = 2.3;

struct Built {
    name: String,
    place: Place,
    lanes: LaneGraph,
    walks: WalkGraph,
    collider: WorldCollider,
}

/// Every region at seed 1, built once, in parallel.
fn places() -> &'static [Built] {
    static PLACES: OnceLock<Vec<Built>> = OnceLock::new();
    PLACES.get_or_init(|| {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = REGIONS
                .iter()
                .map(|&(region, ron)| {
                    scope.spawn(move || {
                        let t: RegionTemplate = ron::from_str(ron).expect("parse");
                        let place = build_place(Seed(1), &t);
                        let lanes = LaneGraph::new(&place);
                        let walks = WalkGraph::new(&place, &lanes);
                        let collider = WorldCollider::build(&place);
                        Built {
                            name: format!("{region} seed 1"),
                            place,
                            lanes,
                            walks,
                            collider,
                        }
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().expect("build")).collect()
        })
    })
}

fn coast() -> &'static Built {
    &places()[0]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The walk node nearest the middle of the grid cell with the most walk-borne people:
/// the busiest stretch of the place.
fn busy_eye(walks: &WalkGraph) -> [f32; 3] {
    let cells = (walks.grid_cells[0] * walks.grid_cells[1]) as usize;
    let mut weight = vec![0.0f32; cells];
    for (i, l) in walks.links.iter().enumerate() {
        if l.kind == LinkKind::Walk {
            let p = walks.point(i as u32, 0.5 * l.length_m, 0.0);
            if let Some(c) = walks.cell_of([p[0], p[2]]) {
                weight[c] += l.length_m * l.density;
            }
        }
    }
    // Sum over 3x3 neighborhoods so the eye sits in a busy area, not a busy cell.
    let w = walks.grid_cells[0] as i32;
    let h = walks.grid_cells[1] as i32;
    let mut best = (0.0f32, 0usize);
    for c in 0..cells {
        let (x, z) = (c as i32 % w, c as i32 / w);
        let mut sum = 0.0;
        for dz in -2..=2 {
            for dx in -2..=2 {
                let (a, b) = (x + dx, z + dz);
                if a >= 0 && b >= 0 && a < w && b < h {
                    sum += weight[(b * w + a) as usize];
                }
            }
        }
        if sum > best.0 {
            best = (sum, c);
        }
    }
    let (x, z) = (best.1 as i32 % w, best.1 as i32 / w);
    let center = [
        walks.grid_origin[0] + (x as f32 + 0.5) * CELL_M,
        0.0,
        walks.grid_origin[1] + (z as f32 + 0.5) * CELL_M,
    ];
    walks
        .nodes
        .iter()
        .map(|n| n.pos)
        .min_by(|a, b| dist(*a, center).total_cmp(&dist(*b, center)))
        .expect("nodes")
}

/// A camera at `eye` turning slowly (a full turn in about 52 s), so people spawn all
/// around over a run.
fn view_at(eye: [f32; 3], tick: u64) -> View {
    let a = tick as f32 * 0.001;
    View {
        pos: eye,
        dir: [a.cos(), a.sin()],
        tan_half: (55.0f32).to_radians().tan(),
    }
}

fn input(eye: [f32; 3], tick: u64, density: Density, car: Option<Player>) -> PedInput {
    PedInput {
        tick,
        density,
        view: view_at(eye, tick),
        player: car.unwrap_or(Player::ABSENT),
    }
}

/// Runs `steps` steps from signal tick `t0` with no car.
fn run(b: &Built, peds: &mut Pedestrians, eye: [f32; 3], t0: u64, steps: u64, d: Density) {
    for k in 0..steps {
        peds.step(&b.walks, &b.lanes, &input(eye, t0 + k, d, None));
    }
}

fn new_peds(b: &Built, budget: PedestrianBudget) -> Pedestrians {
    Pedestrians::new(&b.walks, Seed(1), b.place.header.era, budget)
}

#[test]
fn walks_lie_on_sidewalks_and_crossings_on_the_road() {
    for b in places() {
        let g = &b.walks;
        let (mut walk_n, mut walk_bad, mut cross_n, mut cross_road) = (0, 0, 0, 0);
        for (i, l) in g.links.iter().enumerate() {
            let steps = (l.length_m / 0.5) as u32 + 1;
            for k in 0..=steps {
                let s = l.length_m * k as f32 / steps as f32;
                let p = g.point(i as u32, s, 0.0);
                // From just above: a walk under a deck is on its own ground.
                let (_, surf) = b.collider.level_from([p[0], p[2]], p[1] + 1.0);
                match l.kind {
                    LinkKind::Walk | LinkKind::Spur => {
                        walk_n += 1;
                        walk_bad += usize::from(surf == Surface::Asphalt);
                    }
                    LinkKind::Crossing => {
                        cross_n += 1;
                        cross_road += usize::from(surf == Surface::Asphalt);
                    }
                }
            }
        }
        let bad = walk_bad as f32 / walk_n as f32;
        let road = cross_road as f32 / cross_n as f32;
        println!(
            "{}: {} nodes, {} links, {:.1} km of walk, {} crossings ({} signalized), {} spots, \
             {} KiB; walk samples on asphalt {:.2}%, crossing samples on asphalt {:.1}%",
            b.name,
            g.nodes.len(),
            g.links.len(),
            g.walk_length() / 1000.0,
            g.crossings.len(),
            g.crossings.iter().filter(|c| c.signal).count(),
            g.spots.len(),
            g.bytes() / 1024,
            bad * 100.0,
            road * 100.0,
        );
        assert!(bad < MAX_WALK_ON_ASPHALT, "{}: {bad}", b.name);
        assert!(road > 0.85, "{}: {road}", b.name);
        assert!(g.crossings.iter().any(|c| c.signal), "{}", b.name);
        for c in &g.crossings {
            assert!(c.lane == NONE || b.lanes.lanes[c.lane as usize].end_node == c.node);
            for &v in &c.curbs {
                assert_eq!(g.links_at(v).len(), 2, "a curb joins its spur and the road");
            }
        }
        // Links meet their nodes; everything is finite.
        for l in &g.links {
            let (a, c) = (g.nodes[l.a as usize].pos, g.nodes[l.b as usize].pos);
            assert!((dist(a, c) - l.length_m).abs() < 1e-3);
            assert!(a.iter().chain(&c).all(|v| v.is_finite()));
        }
        assert_eq!(WalkGraph::new(&b.place, &b.lanes), *g, "rebuilds the same");
    }
}

/// Lane centerline segments in 8 m buckets. Turn curves are left out: a few tight ones
/// (squeezed roads, acute corners) cut across the curb return, over the corner's sidewalk.
type Segment = ([f32; 2], [f32; 2]);

struct LaneIndex {
    cells: HashMap<(i32, i32), Vec<Segment>>,
}

impl LaneIndex {
    fn new(g: &LaneGraph) -> LaneIndex {
        let mut cells: HashMap<(i32, i32), Vec<Segment>> = HashMap::new();
        let mut add = |a: [f32; 3], b: [f32; 3]| {
            let (p, q) = ([a[0], a[2]], [b[0], b[2]]);
            let steps = (dist(a, b) / 4.0) as i32 + 1;
            let mut seen = HashSet::new();
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let c = (
                    ((p[0] + (q[0] - p[0]) * t) / 8.0).floor() as i32,
                    ((p[1] + (q[1] - p[1]) * t) / 8.0).floor() as i32,
                );
                if seen.insert(c) {
                    cells.entry(c).or_default().push((p, q));
                }
            }
        };
        for l in 0..g.lanes.len() as u32 {
            for w in g.points(l).windows(2) {
                add(w[0], w[1]);
            }
        }
        LaneIndex { cells }
    }

    fn nearest(&self, p: [f32; 3]) -> f32 {
        let (cx, cz) = ((p[0] / 8.0).floor() as i32, (p[2] / 8.0).floor() as i32);
        let mut best = f32::MAX;
        for dz in -1..=1 {
            for dx in -1..=1 {
                for &(a, b) in self.cells.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                    let ab = [b[0] - a[0], b[1] - a[1]];
                    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                    let t = if l2 > 0.0 {
                        (((p[0] - a[0]) * ab[0] + (p[2] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let d = ((p[0] - a[0] - ab[0] * t).powi(2) + (p[2] - a[1] - ab[1] * t).powi(2))
                        .sqrt();
                    best = best.min(d);
                }
            }
        }
        best
    }
}

/// Distance from `p` to the nearest link of `kinds` (within the grid cells around it).
fn near_link(g: &WalkGraph, p: [f32; 3], crossing: bool) -> f32 {
    let mut best = f32::MAX;
    let Some(c) = g.cell_of([p[0], p[2]]) else {
        return best;
    };
    let w = g.grid_cells[0] as i32;
    let (cx, cz) = (c as i32 % w, c as i32 / w);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (x, z) = (cx + dx, cz + dz);
            if x < 0 || z < 0 || x >= w || z >= g.grid_cells[1] as i32 {
                continue;
            }
            for &l in g.cell_links((z * w + x) as usize) {
                let link = &g.links[l as usize];
                if (link.kind == LinkKind::Crossing) != crossing {
                    continue;
                }
                let a = g.nodes[link.a as usize].pos;
                let d = [p[0] - a[0], p[2] - a[2]];
                let s = (d[0] * link.dir[0] + d[1] * link.dir[1]).clamp(0.0, link.length_m);
                let q = g.point(l, s, 0.0);
                best = best.min(dist(p, q));
            }
        }
    }
    best
}

#[test]
fn people_stay_on_the_walks_and_out_of_the_lanes() {
    let b = coast();
    let index = LaneIndex::new(&b.lanes);
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    let (mut checked, mut crossing, mut worst_lane, mut worst_walk) = (0, 0, f32::MAX, 0.0f32);
    for k in 0..120 * 90 {
        peds.step(&b.walks, &b.lanes, &input(eye, k, Density::Busy, None));
        if k % 6 != 0 {
            continue;
        }
        for (i, p) in peds.pedestrians().iter().enumerate() {
            if !p.alive {
                continue;
            }
            assert!(p.pos.iter().all(|v| v.is_finite()) && p.heading.is_finite());
            checked += 1;
            if peds.on_crossing(&b.walks, i) {
                crossing += 1;
                let d = near_link(&b.walks, p.pos, true);
                assert!(d < 1.6, "person {} {d:.2} m off its crossing", p.id);
                continue;
            }
            let lane = index.nearest(p.pos);
            worst_lane = worst_lane.min(lane);
            assert!(
                lane > LANE_CLEAR_M,
                "person {} at {:?} is {lane:.2} m from a lane",
                p.id,
                p.pos
            );
            // Benches stand at the back of the walk, within its width.
            let walk = near_link(&b.walks, p.pos, false);
            worst_walk = worst_walk.max(walk);
            assert!(walk < 2.0, "person {} {walk:.2} m off the walks", p.id);
        }
    }
    println!(
        "{checked} person-samples, {crossing} on crossings; nearest lane {worst_lane:.2} m, \
         farthest off a walk line {worst_walk:.2} m"
    );
    assert!(checked > 10_000 && crossing > 50, "{checked} {crossing}");
}

#[test]
fn people_cross_only_on_the_walk_phase() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    let mut on = vec![false; peds.pedestrians().len()];
    let (mut signal_starts, mut other_starts) = (0, 0);
    for k in 0..120 * 150 {
        peds.step(&b.walks, &b.lanes, &input(eye, k, Density::Busy, None));
        for (i, was) in on.iter_mut().enumerate() {
            let now = peds.on_crossing(&b.walks, i);
            if now && !*was {
                let c = &b.walks.crossings[peds.crossing_of(&b.walks, i).unwrap() as usize];
                if c.signal {
                    let light = b.lanes.light(c.lane, k);
                    assert_eq!(light, Some(Light::Red), "stepped out against the light");
                    signal_starts += 1;
                } else {
                    other_starts += 1;
                }
            }
            *was = now;
        }
    }
    println!("{signal_starts} crossings started on walk, {other_starts} at unsignalized ones");
    assert!(signal_starts >= 5 && other_starts >= 5);
}

/// A car driving straight from `start` along `dir` at `speed` for `secs`.
struct Drive {
    start: [f32; 3],
    dir: [f32; 2],
    speed: f32,
    secs: f32,
}

/// Drives `drive` through the people from signal tick `t0`: no one is ever inside the
/// car's footprint. Excluded: a seated person (the bench is solid, the car stops at
/// it), and anyone already in its way within its warning reach when it appears out of
/// nowhere at the first step (a real car comes from farther off). Returns the people who
/// stepped aside or hopped.
fn car_pass(
    b: &Built,
    peds: &mut Pedestrians,
    eye: [f32; 3],
    t0: u64,
    drive: Drive,
) -> HashSet<u32> {
    let Drive {
        start,
        dir,
        speed,
        secs,
    } = drive;
    let heading = (-dir[0]).atan2(-dir[1]);
    let half = [0.9, 2.3];
    let mut dodged = HashSet::new();
    let right = [-dir[1], dir[0]];
    let rel = |p: [f32; 3], c: [f32; 3]| {
        let d = [p[0] - c[0], p[2] - c[2]];
        (
            d[0] * dir[0] + d[1] * dir[1],
            d[0] * right[0] + d[1] * right[1],
        )
    };
    let reach = half[1] + 4.0 + 2.0 * speed;
    let surprised: HashSet<u32> = peds
        .iter()
        .filter(|p| {
            let (along, across) = rel(p.pos, start);
            along > -half[1] && along < reach && across.abs() < half[0] + 0.7
        })
        .map(|p| p.id)
        .collect();
    let steps = (secs * 120.0) as u64;
    for k in 0..steps {
        let s = speed * k as f32 / 120.0;
        let car = Player {
            pos: [start[0] + dir[0] * s, start[1], start[2] + dir[1] * s],
            heading,
            speed,
            half_w: half[0],
            half_len: half[1],
        };
        peds.step(
            &b.walks,
            &b.lanes,
            &input(eye, t0 + k, Density::Busy, Some(car)),
        );
        for (i, p) in peds.pedestrians().iter().enumerate() {
            if !p.alive {
                continue;
            }
            if peds.dodging(i) {
                dodged.insert(p.id);
            }
            if p.pose.action == Action::Sit || surprised.contains(&p.id) {
                continue;
            }
            let (along, across) = rel(p.pos, car.pos);
            assert!(
                along.abs() > half[1] || across.abs() > half[0],
                "person {} inside the car at {along:.2}, {across:.2} (step {k})",
                p.id
            );
        }
    }
    dodged
}

#[test]
fn people_step_aside_for_a_car_and_are_never_inside_it() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut warm = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut warm, eye, 0, 120 * 40, Density::Busy);
    let t0 = 120 * 40;
    // Drive at people walking or standing, from behind them along the way they face, at
    // a range of speeds.
    let targets: Vec<(u32, [f32; 3], f32)> = warm
        .iter()
        .filter(|p| p.pose.action == Action::None && dist(p.pos, eye) < 90.0)
        .map(|p| (p.id, p.pos, p.heading))
        .take(24)
        .collect();
    assert!(targets.len() >= 12, "{} targets", targets.len());
    let (mut hit, mut total) = (0, 0);
    for (n, &(id, pos, heading)) in targets.iter().enumerate() {
        let mut peds = warm.clone();
        let f = [-heading.sin(), -heading.cos()];
        let speed = [6.0, 10.0, 14.0, 18.0][n % 4];
        let back = 30.0 + speed;
        let start = [pos[0] - f[0] * back, pos[1], pos[2] - f[1] * back];
        let drive = Drive {
            start,
            dir: f,
            speed,
            secs: 2.0 * back / speed + 2.0,
        };
        let dodged = car_pass(b, &mut peds, eye, t0, drive);
        total += 1;
        hit += usize::from(dodged.contains(&id));
        // After the car is gone, everyone is back to walking or standing.
        run(b, &mut peds, eye, t0 + 2000, 600, Density::Busy);
        assert!(peds
            .iter()
            .all(|p| !matches!(p.pose.action, Action::Aside | Action::Hop)));
    }
    println!("{hit} of {total} targeted people stepped aside or hopped");
    assert!(hit * 10 >= total * 8, "{hit} of {total}");
}

#[test]
fn people_clear_a_crossing_for_a_car() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    let mut passes = 0;
    let mut k = 0u64;
    while passes < 8 && k < 120 * 400 {
        peds.step(&b.walks, &b.lanes, &input(eye, k, Density::Busy, None));
        k += 1;
        // Someone mid-crossing: drive down the road through the crosswalk at them.
        let Some(i) = (0..peds.pedestrians().len()).find(|&i| peds.on_crossing(&b.walks, i)) else {
            continue;
        };
        let p = peds.pedestrians()[i];
        let c = &b.walks.crossings[peds.crossing_of(&b.walks, i).unwrap() as usize];
        let (a, z) = (
            b.walks.nodes[c.curbs[0] as usize].pos,
            b.walks.nodes[c.curbs[1] as usize].pos,
        );
        let across = [z[0] - a[0], z[2] - a[2]];
        let l = (across[0].powi(2) + across[1].powi(2)).sqrt();
        let along = [across[1] / l, -across[0] / l];
        let start = [
            p.pos[0] - along[0] * 40.0,
            p.pos[1],
            p.pos[2] - along[1] * 40.0,
        ];
        let mut trial = peds.clone();
        let drive = Drive {
            start,
            dir: along,
            speed: 12.0,
            secs: 6.0,
        };
        car_pass(b, &mut trial, eye, k, drive);
        // Nobody stayed on the crossing in the car's way.
        passes += 1;
        run(b, &mut peds, eye, k, 120 * 20, Density::Busy);
        k += 120 * 20;
    }
    println!("{passes} crossing passes");
    assert!(passes >= 4);
}

#[test]
fn groups_keep_together() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    let (mut groups, mut worst) = (0usize, 0.0f32);
    for k in 0..120 * 90 {
        peds.step(&b.walks, &b.lanes, &input(eye, k, Density::Busy, None));
        if k % 12 != 0 {
            continue;
        }
        let all = peds.pedestrians();
        for i in 0..all.len() {
            let g = peds.group(i);
            if !all[i].alive || g.len() < 2 || g[0] as usize != i {
                continue;
            }
            groups += 1;
            for &a in g {
                for &c in g {
                    let d = dist(all[a as usize].pos, all[c as usize].pos);
                    worst = worst.max(d);
                    assert!(d < 3.0, "group of {} spread {d:.2} m", g.len());
                }
            }
        }
    }
    println!("{groups} group-samples, widest {worst:.2} m");
    assert!(groups > 500);
}

#[test]
fn density_bands_hold_and_nothing_spawns_in_view() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut counts = Vec::new();
    for d in [Density::Off, Density::Low, Density::Normal, Density::Busy] {
        let mut peds = new_peds(b, PedestrianBudget::LARGE);
        let mut seen = HashSet::new();
        let mut last: HashMap<u32, [f32; 3]> = HashMap::new();
        let (mut sum, mut samples, mut gone) = (0usize, 0usize, 0usize);
        for k in 0..120 * 60 {
            let inp = input(eye, k, d, None);
            peds.step(&b.walks, &b.lanes, &inp);
            let now: HashMap<u32, [f32; 3]> = peds.iter().map(|p| (p.id, p.pos)).collect();
            for (id, pos) in &last {
                if !now.contains_key(id) {
                    gone += 1;
                    assert!(!inp.view.sees(*pos, 1.5), "person {id} vanished in view");
                }
            }
            last = now;
            for p in peds.iter() {
                if seen.insert(p.id) {
                    let r = dist(p.pos, eye);
                    assert!(
                        !inp.view.sees(p.pos, 1.5),
                        "person {} spawned in view",
                        p.id
                    );
                    assert!(
                        (SPAWN_NEAR_M - 1.0..=SPAWN_FAR_M + 1.0).contains(&r),
                        "spawned {r:.1} m away"
                    );
                }
                // Past the despawn range only while still in view.
                assert!(dist(p.pos, eye) < DESPAWN_M + 2.0 || inp.view.sees(p.pos, 1.5));
            }
            if k >= 120 * 30 {
                sum += peds.count();
                samples += 1;
            }
        }
        let mean = sum as f32 / samples as f32;
        let target = peds.target();
        println!(
            "{d:?}: target {target}, mean {mean:.1} over the last 30 s, {} spawned, {gone} gone",
            seen.len()
        );
        if d == Density::Off {
            assert_eq!(seen.len(), 0);
        } else {
            let t = target as f32;
            assert!(
                (mean - t).abs() <= (0.2 * t).max(3.0),
                "{d:?}: mean {mean} vs target {t}"
            );
        }
        counts.push(mean);
    }
    assert!(counts[1] < counts[2] && counts[2] < counts[3], "{counts:?}");
}

/// 30 s at seed 1 on the coast with a car driving through the busy area.
fn thirty_seconds(b: &Built) -> u64 {
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::SMALL);
    for k in 0..120 * 30u64 {
        let s = k as f32 / 120.0;
        let car = (k > 120 * 15).then(|| Player {
            pos: [eye[0] - 60.0 + 8.0 * (s - 15.0), eye[1], eye[2]],
            heading: -std::f32::consts::FRAC_PI_2,
            speed: 8.0,
            half_w: 0.9,
            half_len: 2.3,
        });
        peds.step(&b.walks, &b.lanes, &input(eye, k, Density::Normal, car));
    }
    peds.hash()
}

#[test]
fn thirty_seconds_at_seed_1_hash_is_pinned() {
    let b = coast();
    let h = thirty_seconds(b);
    println!("coast seed 1, 30 s of pedestrians: {h:#018x}");
    assert_eq!(thirty_seconds(b), h, "same inputs, same people");
    assert_eq!(h, COAST_SEED1_30S_HASH);
}

#[test]
fn stepping_allocates_nothing_after_construction() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::SMALL);
    let before = allocations();
    for k in 0..120 * 40u64 {
        let car = (k > 120 * 20).then(|| Player {
            pos: [
                eye[0] + 50.0 - 10.0 * (k as f32 / 120.0 - 20.0),
                eye[1],
                eye[2] + 1.0,
            ],
            heading: std::f32::consts::FRAC_PI_2,
            speed: 10.0,
            half_w: 0.9,
            half_len: 2.3,
        });
        peds.step(&b.walks, &b.lanes, &input(eye, k, Density::Busy, car));
    }
    assert_eq!(allocations(), before, "stepping allocated");
    assert!(peds.count() > 50, "{}", peds.count());
}

#[test]
fn step_budget_for_128_people() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::SMALL);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let mut times = Vec::with_capacity(1200);
    // The car drives slowly through the people the whole time, so reactions are timed.
    let dir = walk_dir_near(b, eye);
    for k in 0..1200u64 {
        let car = slow_car(eye, dir, -30.0 + 2.0 * k as f32 / 120.0, 2.0);
        let t = std::time::Instant::now();
        peds.step(
            &b.walks,
            &b.lanes,
            &input(eye, 120 * 40 + k, Density::Busy, Some(car)),
        );
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let p99 = times[times.len() * 99 / 100];
    println!(
        "{} people: median {:.3} ms, p99 {p99:.3} ms per step",
        peds.count(),
        times[times.len() / 2]
    );
    assert!(peds.count() >= 100, "{}", peds.count());
    // Budget 0.5 ms p99 for 128 people in a debug build.
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(p99 < 0.5, "{p99} ms");
    }
}

/// Where the people are thickest: the walking or standing person with the most others
/// within 20 m.
fn crowd(peds: &Pedestrians) -> [f32; 3] {
    let all: Vec<[f32; 3]> = peds
        .iter()
        .filter(|p| p.pose.action != Action::Sit)
        .map(|p| p.pos)
        .collect();
    *all.iter()
        .max_by_key(|a| all.iter().filter(|b| dist(**a, **b) < 20.0).count())
        .expect("people")
}

/// The direction of the walk link nearest `p`.
fn walk_dir_near(b: &Built, p: [f32; 3]) -> [f32; 2] {
    walk_near(b, p).1
}

/// The walk link (along a road) nearest `p`: a point on its walk line and its direction.
fn walk_near(b: &Built, p: [f32; 3]) -> ([f32; 3], [f32; 2]) {
    let g = &b.walks;
    g.links
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind == LinkKind::Walk && l.road_side != 0 && l.length_m > 4.0)
        .map(|(i, l)| (g.point(i as u32, 0.5 * l.length_m, 0.0), l.dir))
        .min_by(|a, c| dist(a.0, p).total_cmp(&dist(c.0, p)))
        .expect("walks")
}

/// The player's car `s` meters along `dir` from `at`, at `speed`.
fn slow_car(at: [f32; 3], dir: [f32; 2], s: f32, speed: f32) -> Player {
    Player {
        pos: [at[0] + dir[0] * s, at[1], at[2] + dir[1] * s],
        heading: (-dir[0]).atan2(-dir[1]),
        speed,
        half_len: 2.3,
        half_w: 0.9,
    }
}

fn inside(car: &Player, p: [f32; 3]) -> bool {
    let f = [-car.heading.sin(), -car.heading.cos()];
    let d = [p[0] - car.pos[0], p[2] - car.pos[2]];
    let along = d[0] * f[0] + d[1] * f[1];
    let across = d[0] * -f[1] + d[1] * f[0];
    along.abs() <= car.half_len && across.abs() <= car.half_w
}

/// Steps `secs` from signal tick `t0` with the player's car at `path(seconds)` and the
/// camera on it: returns the person-steps inside its footprint and how many people came
/// within 5 m of it.
/// Anyone the car appears on at the first step (it comes out of nowhere) gets 1.5 s to
/// get out, then counts like everyone else.
fn obstacle(
    b: &Built,
    peds: &mut Pedestrians,
    eye: [f32; 3],
    t0: u64,
    secs: f32,
    path: impl Fn(f32) -> Player,
) -> (usize, usize) {
    let first = path(0.0);
    let landed: HashSet<u32> = peds
        .iter()
        .filter(|p| inside(&first, p.pos))
        .map(|p| p.id)
        .collect();
    let (mut steps_inside, mut near) = (0usize, HashSet::new());
    for k in 0..(secs * 120.0) as u64 {
        let car = path(k as f32 / 120.0);
        let view = View {
            pos: car.pos,
            dir: [-car.heading.sin(), -car.heading.cos()],
            ..view_at(eye, 0)
        };
        let inp = PedInput {
            tick: t0 + k,
            density: Density::Busy,
            view,
            player: car,
        };
        peds.step(&b.walks, &b.lanes, &inp);
        for p in peds.iter() {
            let grace = landed.contains(&p.id) && k < 180;
            if inside(&car, p.pos) && !grace {
                steps_inside += 1;
            }
            let f = [-car.heading.sin(), -car.heading.cos()];
            let d = [p.pos[0] - car.pos[0], p.pos[2] - car.pos[2]];
            let along = (d[0] * f[0] + d[1] * f[1]).abs() - car.half_len;
            let across = (d[0] * -f[1] + d[1] * f[0]).abs() - car.half_w;
            if along.max(across) < 5.0 {
                near.insert(p.id);
            }
        }
    }
    (steps_inside, near.len())
}

/// The busy coast after 40 s, and a crossing near the eye: its middle and the road's
/// direction there.
fn warm_and_crossing(b: &Built) -> (Pedestrians, [f32; 3], [f32; 3], [f32; 2]) {
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let g = &b.walks;
    // The crossing with the most people within 25 m of it.
    let c = g
        .crossings
        .iter()
        .max_by_key(|x| {
            let at = g.nodes[x.curbs[0] as usize].pos;
            peds.iter().filter(|p| dist(p.pos, at) < 25.0).count()
        })
        .expect("crossings");
    let (a, z) = (
        g.nodes[c.curbs[0] as usize].pos,
        g.nodes[c.curbs[1] as usize].pos,
    );
    let mid = [
        (a[0] + z[0]) * 0.5,
        (a[1] + z[1]) * 0.5,
        (a[2] + z[2]) * 0.5,
    ];
    let across = [z[0] - a[0], z[2] - a[2]];
    let l = (across[0].powi(2) + across[1].powi(2)).sqrt();
    (peds, eye, mid, [across[1] / l, -across[0] / l])
}

#[test]
fn nobody_enters_a_car_stopped_on_a_crosswalk() {
    let b = coast();
    let (mut peds, eye, mid, along) = warm_and_crossing(b);
    let (n, near) = obstacle(b, &mut peds, eye, 120 * 40, 60.0, |_| {
        slow_car(mid, along, 0.0, 0.0)
    });
    println!("stopped on a crosswalk: {n} person-steps inside, {near} people came near");
    assert_eq!(n, 0);
    assert!(near > 0);
}

#[test]
fn nobody_enters_a_car_creeping_over_a_crosswalk() {
    let b = coast();
    let (mut peds, eye, mid, along) = warm_and_crossing(b);
    let (n, near) = obstacle(b, &mut peds, eye, 120 * 40, 60.0, |t| {
        slow_car(mid, along, -8.0 + 0.25 * t, 0.25)
    });
    println!("creeping at 0.25 m/s: {n} person-steps inside, {near} people came near");
    assert_eq!(n, 0);
    assert!(near > 0);
}

#[test]
fn nobody_enters_a_car_parked_on_the_sidewalk() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let (at, dir) = walk_near(b, crowd(&peds));
    // 2.5 m/s along the walk for 8 s, then parked there.
    let (n, near) = obstacle(b, &mut peds, eye, 120 * 40, 50.0, |t| {
        let t = t.min(8.0);
        slow_car(at, dir, -20.0 + 2.5 * t, if t < 8.0 { 2.5 } else { 0.0 })
    });
    println!("parked on the sidewalk: {n} person-steps inside, {near} people came near");
    assert_eq!(n, 0);
    assert!(near > 0);
}

#[test]
fn nobody_enters_a_car_driving_slowly_through_a_crowd() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let (at, dir) = walk_near(b, crowd(&peds));
    // Between 1 and 3 m/s for 60 s along the walk through the busiest stretch.
    let (n, near) = obstacle(b, &mut peds, eye, 120 * 40, 60.0, |t| {
        let s = -60.0 + 2.0 * t - 3.0 * (t / 3.0).cos() + 3.0;
        slow_car(at, dir, s, 2.0 + (t / 3.0).sin())
    });
    println!("1-3 m/s through a crowd: {n} person-steps inside, {near} people came near");
    assert_eq!(n, 0);
    assert!(near > 5, "{near}");
}

#[test]
fn people_stepping_aside_stay_on_the_walk() {
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let (at, dir) = walk_near(b, crowd(&peds));
    let (mut samples, mut worst) = (0usize, 0.0f32);
    for k in 0..120 * 50u64 {
        let car = slow_car(at, dir, -25.0 + k as f32 / 120.0, 1.0);
        peds.step(
            &b.walks,
            &b.lanes,
            &input(eye, 120 * 40 + k, Density::Busy, Some(car)),
        );
        for (i, p) in peds.pedestrians().iter().enumerate() {
            if !p.alive || !peds.dodging(i) {
                continue;
            }
            samples += 1;
            let off = near_link(&b.walks, p.pos, false);
            worst = worst.max(off);
            assert!(off < 1.5, "person {} stepped {off:.2} m off the walk", p.id);
        }
    }
    println!("{samples} dodging person-steps, at most {worst:.2} m off a walk line");
    assert!(samples > 100, "{samples}");
}
