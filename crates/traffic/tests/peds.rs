//! Pedestrians on the procedural places at seed 1: the sidewalk graph lies on the
//! sidewalks and crosswalks the world draws; the simulation keeps people on it and out of
//! the lanes, crosses on the walk phase, steps aside for a car pass without anyone ever
//! inside its footprint, keeps groups together, holds the density bands without spawning
//! in view, is deterministic (pinned hash) and allocates nothing after construction. Run
//! with `--nocapture` to see the numbers: `cargo test -p traffic --test peds -- --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock, RwLockReadGuard};

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

/// Every test but the budget runs under a read lock the budget test takes for writing, so
/// it times the step on a quiet machine.
static QUIET: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    QUIET.read().unwrap_or_else(|e| e.into_inner())
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
const COAST_SEED1_30S_HASH: u64 = 0xe973_bc20_db0a_d30a;

/// Size ceiling for a place's sidewalk graph (16×16 chunks), about 1.3 times the largest
/// region's at seed 1 (2.9 MiB; the coast's 2.2 MiB, 0.55 MiB of it the turn curves).
const MAX_WALK_BYTES: usize = 4 * 1024 * 1024;

/// Most walk samples `world` lays another road's asphalt over (where two roads squeeze
/// together it draws one's pavement across the other's sidewalk).
const MAX_WALK_ON_ASPHALT: f32 = 0.03;
/// People off the crossings on asphalt stay at least this far outside every lane (the
/// gutter is 0.45 m): on the road only where `world` lays pavement over a sidewalk.
const LANE_CLEAR_M: f32 = 0.5;
/// Half the width of the crossing links' band (`walks::ROOM_CROSSING_M`).
const CROSSWALK_HALF_M: f32 = 1.2;
/// How far off the curb into the gutter people may step from a car on their walk
/// (`peds::ROAD_DODGE_M`).
const ROAD_DODGE_M: f32 = 0.4;

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
                        let grammar = grammar();
                        let place = build_place(Seed(1), &t, &grammar);
                        let lanes = LaneGraph::new(&place);
                        let walks = WalkGraph::new(&place, &lanes);
                        let collider = WorldCollider::build(&place, &grammar);
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

/// The shipped coast grammar: `build_place` dresses buildings from it.
fn grammar() -> world::Grammar {
    ron::from_str(include_str!(
        "../../../assets/grammar/california_coast_80s.ron"
    ))
    .expect("parse grammar")
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
    let _quiet = shared();
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
        assert!(
            g.bytes() <= MAX_WALK_BYTES,
            "{}: {} bytes",
            b.name,
            g.bytes()
        );
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

/// Lane centerline segments with their lane's half width, in 8 m buckets. Turn curves
/// are left out: a few tight ones (squeezed roads, acute corners) cut across the curb
/// return, over the corner's sidewalk.
type Segment = ([f32; 2], [f32; 2], f32);

struct LaneIndex {
    cells: HashMap<(i32, i32), Vec<Segment>>,
}

impl LaneIndex {
    fn new(place: &Place, g: &LaneGraph) -> LaneIndex {
        let mut cells: HashMap<(i32, i32), Vec<Segment>> = HashMap::new();
        let mut add = |a: [f32; 3], b: [f32; 3], half: f32| {
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
                    cells.entry(c).or_default().push((p, q, half));
                }
            }
        };
        for (l, lane) in g.lanes.iter().enumerate() {
            // The lane widths the paint uses (`lanes::lane_offset`).
            let e = &place.roads.edges[lane.edge as usize];
            let n = f32::from(
                if lane.forward {
                    e.lanes_fwd
                } else {
                    e.lanes_back
                }
                .max(1),
            );
            let inner = e.width_m * 0.5 - 0.45;
            let width = if e.oneway {
                2.0 * inner / n
            } else if e.class == place::RoadClass::Highway {
                3.6
            } else {
                inner / n
            };
            for w in g.points(l as u32).windows(2) {
                add(w[0], w[1], 0.5 * width);
            }
        }
        LaneIndex { cells }
    }

    /// How far `p` is outside the nearest lane (negative inside it).
    fn outside(&self, p: [f32; 3]) -> f32 {
        let (cx, cz) = ((p[0] / 8.0).floor() as i32, (p[2] / 8.0).floor() as i32);
        let mut best = f32::MAX;
        for dz in -1..=1 {
            for dx in -1..=1 {
                for &(a, b, half) in self.cells.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                    let ab = [b[0] - a[0], b[1] - a[1]];
                    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                    let t = if l2 > 0.0 {
                        (((p[0] - a[0]) * ab[0] + (p[2] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let d = ((p[0] - a[0] - ab[0] * t).powi(2) + (p[2] - a[1] - ab[1] * t).powi(2))
                        .sqrt();
                    best = best.min(d - half);
                }
            }
        }
        best
    }
}

/// How far `p` is outside the band of every walk and spur link around it (each link's
/// room to either side of its line; 0 within one).
fn outside_walks(g: &WalkGraph, p: [f32; 3]) -> f32 {
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
                if link.kind == LinkKind::Crossing {
                    continue;
                }
                let a = g.nodes[link.a as usize].pos;
                let d = [p[0] - a[0], p[2] - a[2]];
                let s = d[0] * link.dir[0] + d[1] * link.dir[1];
                let lat = d[0] * link.dir[1] - d[1] * link.dir[0];
                let room = if lat < 0.0 {
                    link.room[0]
                } else {
                    link.room[1]
                };
                let ds = if s < 0.0 {
                    -s
                } else {
                    (s - link.length_m).max(0.0)
                };
                let dl = (lat.abs() - room).max(0.0);
                best = best.min((ds * ds + dl * dl).sqrt());
            }
        }
    }
    best
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
    let _quiet = shared();
    let b = coast();
    let index = LaneIndex::new(&b.place, &b.lanes);
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
            let lane = index.outside(p.pos);
            worst_lane = worst_lane.min(lane);
            let (_, surf) = b.collider.level_from([p.pos[0], p.pos[2]], p.pos[1] + 1.0);
            assert!(
                lane > LANE_CLEAR_M || surf != Surface::Asphalt,
                "person {} at {:?} is in a lane ({lane:.2} m outside it, on asphalt)",
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
        "{checked} person-samples, {crossing} on crossings; at least {worst_lane:.2} m outside every lane, \
         farthest off a walk line {worst_walk:.2} m"
    );
    assert!(checked > 10_000 && crossing > 50, "{checked} {crossing}");
}

#[test]
fn people_cross_only_on_the_walk_phase() {
    let _quiet = shared();
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
                    for l in c.lane..c.lane + c.lane_count {
                        let light = b.lanes.light(l, k);
                        assert!(
                            light.is_none_or(|v| v == Light::Red),
                            "stepped out against the light"
                        );
                    }
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

/// A signalized crossing over a road with two lanes arriving shows *walk* only while both
/// are red: with the second lane put on another phase of the junction, the walk follows
/// both lights, never the first alone.
#[test]
fn walk_waits_for_every_approach_lane() {
    let _quiet = shared();
    let b = coast();
    let (k, x) = b
        .walks
        .crossings
        .iter()
        .enumerate()
        .find(|(_, x)| {
            x.signal
                && x.lane_count >= 2
                && b.lanes.junctions[x.node as usize].phases >= 2
                && (x.lane..x.lane + x.lane_count).all(|l| b.lanes.light(l, 0).is_some())
        })
        .expect("a signalized crossing over two approach lanes");
    let mut lanes = b.lanes.clone();
    let phases = lanes.junctions[x.node as usize].phases;
    let second = &mut lanes.lanes[x.lane as usize + 1];
    second.phase = (second.phase + 1) % phases;
    let period = 120 * 600;
    let (mut walks, mut first_only) = (0, 0);
    for t in (0..period).step_by(30) {
        let red = |l: u32| lanes.light(l, t) == Some(Light::Red);
        let both = red(x.lane) && red(x.lane + 1);
        assert_eq!(b.walks.walk(&lanes, k as u32, t, 0), both, "tick {t}");
        walks += usize::from(both);
        first_only += usize::from(red(x.lane) && !red(x.lane + 1));
    }
    println!("{walks} walk samples, {first_only} with only the first lane red");
    assert!(walks > 0 && first_only > 0, "{walks} {first_only}");
}

/// A car driving straight from `start` along `dir` at `speed` for `secs`.
struct Drive {
    start: [f32; 3],
    dir: [f32; 2],
    speed: f32,
    secs: f32,
}

/// Drives `drive` through the people from signal tick `t0`: no one is ever inside the
/// car's footprint or within [`BODY_GAP_M`] of it. Excluded: a seated person (the bench is solid, the car stops at
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
                gap(&car, p.pos) >= BODY_GAP_M,
                "person {} inside the car or within {BODY_GAP_M} m of it at {along:.2}, \
                 {across:.2} (step {k})",
                p.id
            );
        }
    }
    dodged
}

#[test]
fn people_step_aside_for_a_car_and_are_never_inside_it() {
    let _quiet = shared();
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
    let _quiet = shared();
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
    let _quiet = shared();
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
    let _quiet = shared();
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
            }
            // Past the despawn range only while someone of the group is still in view.
            let all = peds.pedestrians();
            for (i, p) in all.iter().enumerate() {
                if !p.alive || dist(p.pos, eye) < DESPAWN_M + 2.0 {
                    continue;
                }
                let group_seen = peds
                    .group(i)
                    .iter()
                    .any(|&m| inp.view.sees(all[m as usize].pos, 1.5));
                assert!(
                    group_seen,
                    "person {} at {:.1} m out of view",
                    p.id,
                    dist(p.pos, eye)
                );
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
    let _quiet = shared();
    let b = coast();
    let h = thirty_seconds(b);
    println!("coast seed 1, 30 s of pedestrians: {h:#018x}");
    assert_eq!(thirty_seconds(b), h, "same inputs, same people");
    assert_eq!(h, COAST_SEED1_30S_HASH);
}

#[test]
fn stepping_allocates_nothing_after_construction() {
    let _quiet = shared();
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
    let _quiet = QUIET.write().unwrap_or_else(|e| e.into_inner());
    // The car drives slowly through the people the whole time, so reactions are timed.
    // Three 5 s windows, the best of each figure gated, as for the cars: the median is what
    // the sim costs when the machine is not busy and does not move when a shared runner
    // steals a few slices; the p99 only catches gross regressions.
    let dir = walk_dir_near(b, eye);
    let (mut median, mut p99) = (f64::MAX, f64::MAX);
    let mut k = 0u64;
    for window in 0..3 {
        let mut times = Vec::with_capacity(600);
        for _ in 0..600 {
            let car = slow_car(eye, dir, -30.0 + 2.0 * (k % 2400) as f32 / 120.0, 2.0);
            let t = std::time::Instant::now();
            peds.step(
                &b.walks,
                &b.lanes,
                &input(eye, 120 * 40 + k, Density::Busy, Some(car)),
            );
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            k += 1;
        }
        times.sort_by(f64::total_cmp);
        let (m, p) = (times[times.len() / 2], times[times.len() * 99 / 100]);
        println!(
            "window {window}: {} people, median {m:.3} ms, p99 {p:.3} ms per step",
            peds.count()
        );
        median = median.min(m);
        p99 = p99.min(p);
    }
    assert!(peds.count() >= 100, "{}", peds.count());
    // Budget for 128 people in a debug build: median under 0.5 ms; the p99 under 5 ms only
    // catches a tenfold regression (on a 2-core CI runner with the other tests running it
    // reaches 3 ms).
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(median < 0.5, "median {median:.3} ms per step");
        assert!(p99 < 5.0, "p99 {p99:.3} ms per step");
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

/// The walk link with the most people along 60 m of it (the thickest crowd's own walk can
/// be a short one at a corner): a point on its walk line and its direction.
fn crowd_walk(b: &Built, peds: &Pedestrians) -> ([f32; 3], [f32; 2]) {
    let g = &b.walks;
    let along = |at: [f32; 3], d: [f32; 2]| {
        peds.iter()
            .filter(|p| {
                let r = [p.pos[0] - at[0], p.pos[2] - at[2]];
                (r[0] * d[0] + r[1] * d[1]).abs() < 30.0 && (r[0] * d[1] - r[1] * d[0]).abs() < 3.0
            })
            .count()
    };
    g.links
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind == LinkKind::Walk && l.road_side != 0 && l.length_m > 4.0)
        .map(|(i, l)| (g.point(i as u32, 0.5 * l.length_m, 0.0), l.dir))
        .max_by_key(|&(at, d)| along(at, d))
        .expect("walks")
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

/// A person's centre must stay at least this far from the car's outline: the figure is
/// about 0.4 m wide (hands and feet are the app's crowd test's, at 5 cm).
const BODY_GAP_M: f32 = 0.15;

/// How far the car's outline is from `p`, m (negative inside it).
fn gap(car: &Player, p: [f32; 3]) -> f32 {
    let f = [-car.heading.sin(), -car.heading.cos()];
    let d = [p[0] - car.pos[0], p[2] - car.pos[2]];
    let a = (d[0] * f[0] + d[1] * f[1]).abs() - car.half_len;
    let c = (d[0] * -f[1] + d[1] * f[0]).abs() - car.half_w;
    if a <= 0.0 && c <= 0.0 {
        a.max(c)
    } else {
        (a.max(0.0).powi(2) + c.max(0.0).powi(2)).sqrt()
    }
}

/// Steps `secs` from signal tick `t0` with the player's car at `path(seconds)` and the
/// camera on it: returns the person-steps inside its footprint or with its outline within
/// [`BODY_GAP_M`] of them, and how many people came
/// within 8 m of it (people at the curb of a wide road are that far from its middle).
/// Anyone the car appears on at the first step (it comes out of nowhere) gets 1.5 s to
/// get out, then counts like everyone else.
fn obstacle(
    b: &Built,
    peds: &mut Pedestrians,
    eye: [f32; 3],
    t0: u64,
    secs: f32,
    path: impl Fn(f32) -> Player,
) -> (usize, usize, usize) {
    let first = path(0.0);
    let landed: HashSet<u32> = peds
        .iter()
        .filter(|p| inside(&first, p.pos))
        .map(|p| p.id)
        .collect();
    let (mut steps_inside, mut near, mut close) = (0usize, HashSet::new(), HashSet::new());
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
            // Inside, or the car's outline within a body's half width of their centre.
            if gap(&car, p.pos) < BODY_GAP_M && !grace {
                steps_inside += 1;
            }
            let f = [-car.heading.sin(), -car.heading.cos()];
            let d = [p.pos[0] - car.pos[0], p.pos[2] - car.pos[2]];
            let along = (d[0] * f[0] + d[1] * f[1]).abs() - car.half_len;
            let across = (d[0] * -f[1] + d[1] * f[0]).abs() - car.half_w;
            if along.max(across) < 8.0 {
                near.insert(p.id);
            }
            if along.max(across) < 5.0 {
                close.insert(p.id);
            }
        }
    }
    (steps_inside, near.len(), close.len())
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
    let _quiet = shared();
    let b = coast();
    let (mut peds, eye, mid, along) = warm_and_crossing(b);
    let (n, near, _) = obstacle(b, &mut peds, eye, 120 * 40, 60.0, |_| {
        slow_car(mid, along, 0.0, 0.0)
    });
    println!(
        "stopped on a crosswalk: {n} person-steps inside or within 0.15 m, {near} people came near"
    );
    assert_eq!(n, 0);
    assert!(near > 0);
}

#[test]
fn nobody_enters_a_car_creeping_over_a_crosswalk() {
    let _quiet = shared();
    let b = coast();
    let (mut peds, eye, mid, along) = warm_and_crossing(b);
    let (n, near, _) = obstacle(b, &mut peds, eye, 120 * 40, 60.0, |t| {
        slow_car(mid, along, -8.0 + 0.25 * t, 0.25)
    });
    println!(
        "creeping at 0.25 m/s: {n} person-steps inside or within 0.15 m, {near} people came near"
    );
    assert_eq!(n, 0);
    assert!(near > 0);
}

#[test]
fn nobody_enters_a_car_parked_on_the_sidewalk() {
    let _quiet = shared();
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let (at, dir) = walk_near(b, crowd(&peds));
    // 2.5 m/s along the walk for 8 s, then parked there.
    let (n, near, _) = obstacle(b, &mut peds, eye, 120 * 40, 50.0, |t| {
        let t = t.min(8.0);
        slow_car(at, dir, -20.0 + 2.5 * t, if t < 8.0 { 2.5 } else { 0.0 })
    });
    println!(
        "parked on the sidewalk: {n} person-steps inside or within 0.15 m, {near} people came near"
    );
    assert_eq!(n, 0);
    assert!(near > 0);
}

#[test]
fn nobody_enters_a_car_driving_slowly_through_a_crowd() {
    let _quiet = shared();
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut peds = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut peds, eye, 0, 120 * 40, Density::Busy);
    let (at, dir) = crowd_walk(b, &peds);
    // Between 1 and 3 m/s for 60 s along the walk through the busiest stretch.
    let (n, _, close) = obstacle(b, &mut peds, eye, 120 * 40, 60.0, |t| {
        let s = -60.0 + 2.0 * t - 3.0 * (t / 3.0).cos() + 3.0;
        slow_car(at, dir, s, 2.0 + (t / 3.0).sin())
    });
    println!("1-3 m/s through a crowd: {n} person-steps inside or within 0.15 m, {close} people within 5 m");
    assert_eq!(n, 0);
    assert!(close >= 5, "{close}");
}

#[test]
fn people_stepping_aside_stay_on_the_walk() {
    let _quiet = shared();
    let b = coast();
    let eye = busy_eye(&b.walks);
    let mut warm = new_peds(b, PedestrianBudget::LARGE);
    run(b, &mut warm, eye, 0, 120 * 40, Density::Busy);
    // The walks with the most people near them, busiest first: drive along each at 1 m/s
    // (camera on the car) until enough people have stepped aside.
    let g = &b.walks;
    let mut walks: Vec<(usize, [f32; 3], [f32; 2])> = g
        .links
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind == LinkKind::Walk && l.road_side != 0 && l.length_m > 4.0)
        .map(|(i, l)| {
            let mid = g.point(i as u32, 0.5 * l.length_m, 0.0);
            let n = warm.iter().filter(|p| dist(p.pos, mid) < 12.0).count();
            (n, mid, l.dir)
        })
        .filter(|w| w.0 > 0)
        .collect();
    walks.sort_by_key(|w| std::cmp::Reverse(w.0));
    let (mut samples, mut worst, mut tried) = (0usize, 0.0f32, 0);
    for &(_, mid, dir) in walks.iter().take(8) {
        if samples > 300 {
            break;
        }
        tried += 1;
        let mut peds = warm.clone();
        for k in 0..120 * 40u64 {
            let car = slow_car(mid, dir, -20.0 + k as f32 / 120.0, 1.0);
            let inp = PedInput {
                tick: 120 * 40 + k,
                density: Density::Busy,
                view: View {
                    pos: car.pos,
                    dir,
                    ..view_at(eye, 0)
                },
                player: car,
            };
            peds.step(&b.walks, &b.lanes, &inp);
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
    }
    println!(
        "{samples} dodging person-steps over {tried} walks, at most {worst:.2} m off a walk line"
    );
    assert!(samples > 100, "{samples}");
}

/// Coast seeds 1–4, each built fresh: 60 s busy with the player's car (and the camera)
/// driving 4–8 m/s through the busiest stretch. Nobody off a crossing (stepping aside or
/// not) stands in a lane on the road (within [`LANE_CLEAR_M`] of its band, on asphalt), nobody ever stands inside the car or within
/// [`BODY_GAP_M`] of it (but anyone it appears on at its first step, for 1.5 s), and everyone
/// stays finite. "In a lane": inside
/// a lane's band by the lane graph and on asphalt by the world's collider (near junctions
/// where a road bends into its node the lane graph's lanes and the world's straightened
/// road ends differ by up to a meter, so the band alone flags people on the curb).
#[test]
fn other_seeds_keep_people_on_the_walks_and_clear_of_the_car() {
    let _quiet = shared();
    let ron = REGIONS[0].1;
    let t: RegionTemplate = ron::from_str(ron).expect("parse");
    let grammar = grammar();
    type Sweep = (u64, usize, f32, usize, usize, usize, f32, f32);
    let results: Vec<Sweep> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (1..=4u64)
            .map(|seed| {
                let (t, grammar) = (&t, &grammar);
                scope.spawn(move || {
                    let place = build_place(Seed(seed), t, grammar);
                    let lanes = LaneGraph::new(&place);
                    let walks = WalkGraph::new(&place, &lanes);
                    let index = LaneIndex::new(&place, &lanes);
                    let eye = busy_eye(&walks);
                    let mut peds = Pedestrians::new(
                        &walks,
                        Seed(seed),
                        place.header.era,
                        PedestrianBudget::LARGE,
                    );
                    let none = |k: u64| PedInput {
                        tick: k,
                        density: Density::Busy,
                        view: view_at(eye, k),
                        player: Player::ABSENT,
                    };
                    for k in 0..120 * 30 {
                        peds.step(&walks, &lanes, &none(k));
                    }
                    let at = crowd(&peds);
                    let dir = walks
                        .links
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| l.kind == LinkKind::Walk && l.road_side != 0)
                        .min_by(|(i, _), (j, _)| {
                            let a = walks.point(*i as u32, 0.0, 0.0);
                            let c = walks.point(*j as u32, 0.0, 0.0);
                            dist(a, at).total_cmp(&dist(c, at))
                        })
                        .map(|(_, l)| l.dir)
                        .expect("walks");
                    let path = |k: u64| {
                        let t = k as f32 / 120.0;
                        slow_car(at, dir, -60.0 + 6.0 * t, 6.0 + 2.0 * (t * 0.5).sin())
                    };
                    // Anyone the car appears on ([`appears_on`]) gets 1.5 s to get out.
                    let landed: HashSet<u32> = peds
                        .iter()
                        .filter(|p| appears_on(&path(0), p.pos))
                        .map(|p| p.id)
                        .collect();
                    let collider = WorldCollider::build(&place, grammar);
                    let (mut inside_steps, mut nearest, mut checked, mut in_lane) =
                        (0, f32::MAX, 0, 0usize);
                    let (mut dodges, mut off_walk, mut nearest_gap) = (0usize, 0.0f32, f32::MAX);
                    // Per slot: who, and where their move out of the way began.
                    let mut began: Vec<Option<(u32, [f32; 3])>> =
                        vec![None; peds.pedestrians().len()];
                    // Per slot: who, and when they last moved out of the way.
                    let mut dodged: Vec<(u32, u64)> = vec![(u32::MAX, 0); began.len()];
                    for k in 0..120 * 60u64 {
                        let car = path(k);
                        let inp = PedInput {
                            tick: 120 * 30 + k,
                            density: Density::Busy,
                            view: View {
                                pos: car.pos,
                                dir,
                                ..view_at(eye, 0)
                            },
                            player: car,
                        };
                        peds.step(&walks, &lanes, &inp);
                        for (i, p) in peds.pedestrians().iter().enumerate() {
                            if !p.alive {
                                continue;
                            }
                            assert!(p.pos.iter().all(|v| v.is_finite()));
                            began[i] = match began[i] {
                                Some((id, a)) if id == p.id && peds.dodging(i) => Some((id, a)),
                                _ => peds.dodging(i).then_some((p.id, p.pos)),
                            };
                            if peds.dodging(i) {
                                dodged[i] = (p.id, k);
                            }
                            if !(landed.contains(&p.id) && k < 180) {
                                let g = gap(&car, p.pos);
                                nearest_gap = nearest_gap.min(g);
                                // Inside, or within a body's half width of the outline.
                                if g < BODY_GAP_M {
                                    inside_steps += 1;
                                }
                            }
                            if k % 6 == 0 && !peds.on_crossing(&walks, i) {
                                checked += 1;
                                if let Some((_, a)) = began[i] {
                                    // As far off as the move took them (someone at a shop
                                    // window starts off the walk).
                                    let from = (near_link(&walks, a, false) - 1.5).max(0.0);
                                    dodges += 1;
                                    off_walk = off_walk.max(near_link(&walks, p.pos, false) - from);
                                }
                                let o = index.outside(p.pos);
                                let (_, surf) =
                                    collider.level_from([p.pos[0], p.pos[2]], p.pos[1] + 1.0);
                                // A step into the gutter (outside the lanes) is a way out,
                                // and a little while to get back from it.
                                let recent = dodged[i].0 == p.id && k < dodged[i].1 + 600;
                                let clear = if recent { 0.0 } else { LANE_CLEAR_M };
                                if o < clear && surf == Surface::Asphalt {
                                    in_lane += 1;
                                }
                                nearest = nearest.min(o);
                            }
                        }
                    }
                    (
                        seed,
                        inside_steps,
                        nearest,
                        checked,
                        in_lane,
                        dodges,
                        off_walk,
                        nearest_gap,
                    )
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().expect("seed")).collect()
    });
    for (seed, inside_steps, nearest, checked, in_lane, dodges, off_walk, nearest_gap) in results {
        println!(
            "coast seed {seed}: {inside_steps} person-steps inside the car or within \
             {BODY_GAP_M} m of it (nearest {nearest_gap:.2} m), at least {nearest:.2} m \
             outside every lane over {checked} samples; {dodges} dodging samples, at most \
             {off_walk:.2} m off a walk line"
        );
        // The car drives along the busiest walk: the gutter beside it is a way out too.
        assert!(
            off_walk < 1.5 + ROAD_DODGE_M,
            "seed {seed}: a dodge {off_walk} m off the walk"
        );
        assert_eq!(inside_steps, 0, "seed {seed}");
        assert_eq!(in_lane, 0, "seed {seed}: in a lane on the road");
        assert!(checked > 1000, "seed {seed}: {checked}");
    }
}

/// One scripted car: where it is at time `t` (s).
type Script<'a> = Box<dyn Fn(f32) -> Player + 'a>;

/// What a scripted car run did to the people around it.
#[derive(Default)]
struct Reach {
    /// Person-steps inside the car's footprint.
    inside: usize,
    /// People who came within 6 m of the car.
    near: usize,
    /// Samples (every 6 steps) of people off the crossings in a lane band on asphalt.
    in_lane: usize,
    /// How far a person dodging on the sidewalk got out of the walks' bands (less how far
    /// out they stood when the move started, and with the car on a walk, less the gutter).
    off_walk: f32,
    /// The farthest a single move out of the way carried anyone from where it started.
    longest_move: f32,
    /// The fastest anyone moved in one step (m/s), sitting down and getting up aside.
    fastest: f32,
    /// Person-steps outside the car with its outline within [`BODY_GAP_M`] of them.
    short: usize,
    /// The nearest the car's outline came to anyone's centre (negative inside), m.
    nearest: f32,
    /// The farthest one dodge episode (moves back to back) carried anyone from where it
    /// began, less what it carried them past [`EPISODE_M`] while the car was still coming
    /// at them ([`coming`]).
    episode: f32,
    /// The longest anyone stood within [`STOPPED_NEAR_M`] of the car while it stood
    /// still, s.
    stood: f32,
}

/// A dodge episode carries nobody farther than this once the car is no longer coming.
const EPISODE_M: f32 = 3.0;
/// Nobody stays within this of a stopped car for longer than [`STOPPED_S`].
const STOPPED_NEAR_M: f32 = 0.5;
const STOPPED_S: f32 = 3.0;

/// Whether the car of `script` is still coming at `p` at time `t`: where it will be over
/// the next 3 s, or where it would be going straight on at its speed now, comes within
/// 2.5 m of it (its warning reach, the widening ahead of a fast car, and a margin).
fn coming(script: &Script, t: f32, p: [f32; 3]) -> bool {
    let now = script(t);
    let f = [-now.heading.sin(), -now.heading.cos()];
    (0..=12).any(|k| {
        let dt = k as f32 * 0.25;
        let s = now.speed * dt;
        let on = Player {
            pos: [now.pos[0] + f[0] * s, now.pos[1], now.pos[2] + f[1] * s],
            ..now
        };
        gap(&script(t + dt), p) < 2.5 || (now.speed.abs() > 0.05 && gap(&on, p) < 2.5)
    })
}

/// One seed's place with what the checks need.
struct Scene<'a> {
    walks: &'a WalkGraph,
    lanes: &'a LaneGraph,
    index: &'a LaneIndex,
    collider: &'a WorldCollider,
}

/// Whether a car appearing out of nowhere lands on someone at `p`: inside its footprint
/// grown by 0.5 m, or where its front will be within 0.5 s (nobody gets a body's width
/// aside in that time).
fn appears_on(car: &Player, p: [f32; 3]) -> bool {
    let f = [-car.heading.sin(), -car.heading.cos()];
    let d = [p[0] - car.pos[0], p[2] - car.pos[2]];
    let along = (d[0] * f[0] + d[1] * f[1]) * car.speed.signum();
    let across = d[0] * -f[1] + d[1] * f[0];
    let ahead = car.half_len + 0.5 + car.speed.abs() * 0.5;
    along >= -car.half_len - 0.5 && along <= ahead && across.abs() <= car.half_w + 0.5
}

/// Whether the car's footprint is over a walk: any of its corners, its middle or the
/// middles of its ends within 0.25 m of a walk or spur link's band.
fn on_walk(g: &WalkGraph, car: &Player) -> bool {
    let f = [-car.heading.sin(), -car.heading.cos()];
    let r = [-f[1], f[0]];
    [
        (0.0, 0.0),
        (1.0, 1.0),
        (1.0, -1.0),
        (-1.0, 1.0),
        (-1.0, -1.0),
        (1.0, 0.0),
        (-1.0, 0.0),
    ]
    .iter()
    .any(|&(a, l)| {
        let p = [
            car.pos[0] + f[0] * a * car.half_len + r[0] * l * car.half_w,
            car.pos[1],
            car.pos[2] + f[1] * a * car.half_len + r[1] * l * car.half_w,
        ];
        outside_walks(g, p) <= 0.25
    })
}

/// Runs `script` for `secs` from the warmed people `warm`, signal tick `t0`, with the
/// camera on the car (anyone it appears on, [`appears_on`], at its first step or where it
/// jumps to, gets 1.5 s to get out).
fn drive_script(s: &Scene, warm: &Pedestrians, t0: u64, secs: f32, script: &Script) -> Reach {
    let mut peds = warm.clone();
    // Who the car appeared on, and the step it did.
    let mut landed: HashMap<u32, u64> = HashMap::new();
    let mut was: Option<Player> = None;
    let mut r = Reach {
        nearest: f32::MAX,
        ..Reach::default()
    };
    let mut near = HashSet::new();
    // Per slot: id, last position, whether sitting, where the current move out of the
    // way started and how far through it is, where the dodge episode (moves back to back)
    // started, and steps stood near the car stopped.
    type Last = (
        u32,
        [f32; 3],
        bool,
        Option<[f32; 3]>,
        f32,
        Option<[f32; 3]>,
        u32,
    );
    let mut last: Vec<Last> = peds
        .pedestrians()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let start = peds.dodging(i).then_some(p.pos);
            (
                p.id,
                p.pos,
                p.pose.action == Action::Sit,
                start,
                p.pose.t,
                start,
                0,
            )
        })
        .collect();
    // Per slot: how far the current episode may carry them.
    let mut allow = vec![EPISODE_M; last.len()];
    // Per slot: whether the car was coming when the current move began.
    let mut pushed = vec![false; last.len()];
    for k in 0..(secs * 120.0) as u64 {
        let car = script(k as f32 / 120.0);
        if was.is_none_or(|w| dist(w.pos, car.pos) > 1.0) {
            for p in peds.iter().filter(|p| appears_on(&car, p.pos)) {
                landed.insert(p.id, k);
            }
        }
        was = Some(car);
        let inp = PedInput {
            tick: t0 + k,
            density: Density::Busy,
            view: View {
                pos: car.pos,
                dir: [-car.heading.sin(), -car.heading.cos()],
                ..view_at(car.pos, 0)
            },
            player: car,
        };
        peds.step(s.walks, s.lanes, &inp);
        // On a walk: its footprint (corners, middle, the middles of its ends) within a
        // quarter meter of a walk's band.
        let car_on_walk = on_walk(s.walks, &car);
        for (i, p) in peds.pedestrians().iter().enumerate() {
            let prev = last[i];
            let sitting = p.pose.action == Action::Sit;
            let dodging = p.alive && peds.dodging(i);
            // A move starts afresh (its time back near 0) where the person stood.
            let start = match prev.3 {
                _ if !dodging => None,
                Some(a) if prev.0 == p.id && p.pose.t >= prev.4 => Some(a),
                _ if prev.0 == p.id => Some(prev.1),
                _ => Some(p.pos),
            };
            let episode = match prev.5 {
                _ if !dodging => None,
                Some(a) if prev.0 == p.id => Some(a),
                _ if prev.0 == p.id => Some(prev.1),
                _ => Some(p.pos),
            };
            let g = gap(&car, p.pos);
            let stood = if prev.0 == p.id && car.speed.abs() < 0.3 && g < STOPPED_NEAR_M {
                prev.6 + 1
            } else {
                0
            };
            last[i] = (p.id, p.pos, sitting, start, p.pose.t, episode, stood);
            if !p.alive {
                continue;
            }
            r.stood = r.stood.max(stood as f32 / 120.0);
            if !landed.get(&p.id).is_some_and(|&at| k < at + 180) {
                r.nearest = r.nearest.min(g);
                if g <= 0.0 && inside(&car, p.pos) {
                    r.inside += 1;
                } else if g < BODY_GAP_M {
                    r.short += 1;
                }
            }
            // An episode carries people past EPISODE_M only while the car keeps coming
            // (or in a move begun while it was): how far it has, beyond what the car's way
            // already took them.
            if let Some(a) = episode {
                let d = dist(a, p.pos);
                if prev.0 != p.id || prev.5.is_none() {
                    allow[i] = EPISODE_M;
                }
                if start != prev.3 || prev.0 != p.id {
                    pushed[i] = coming(script, k as f32 / 120.0, p.pos);
                }
                if d > allow[i] {
                    if pushed[i] || coming(script, k as f32 / 120.0, p.pos) {
                        allow[i] = d;
                    } else {
                        r.episode = r.episode.max(d - allow[i] + EPISODE_M);
                    }
                }
                r.episode = r.episode.max(d.min(EPISODE_M));
            }
            if dist(p.pos, car.pos) < 6.0 {
                near.insert(p.id);
            }
            if prev.0 == p.id && !sitting && !prev.2 {
                r.fastest = r.fastest.max(dist(prev.1, p.pos) * 120.0);
            }
            if let Some(a) = start {
                r.longest_move = r.longest_move.max(dist(a, p.pos));
            }
            if k % 6 == 0 {
                // On a crosswalk by where the person stands, whatever link the group is on.
                let on_crosswalk = near_link(s.walks, p.pos, true) <= CROSSWALK_HALF_M + 0.1;
                let (_, surf) = s.collider.level_from([p.pos[0], p.pos[2]], p.pos[1] + 1.0);
                // Out of the walks' bands only as far as the move took them (someone
                // standing at a shop window starts out of them), and with the car on a
                // walk, less the gutter or corner edge they may step onto; on the asphalt
                // (the gutter) the lane check stands.
                if let Some(a) = start.filter(|_| !on_crosswalk && surf != Surface::Asphalt) {
                    let from = outside_walks(s.walks, a);
                    let gutter = if car_on_walk { ROAD_DODGE_M } else { 0.0 };
                    let off = outside_walks(s.walks, p.pos) - from - gutter;
                    r.off_walk = r.off_walk.max(off);
                }
                // In a lane's band on the asphalt off the crosswalks (the gutter beside a
                // walk the car is on is somewhere to step out of its way).
                if s.index.outside(p.pos) < 0.0 && surf == Surface::Asphalt && !on_crosswalk {
                    r.in_lane += 1;
                }
            }
        }
    }
    r.near = near.len();
    r
}

/// The player's car at `pos` facing ground direction `dir`, at `speed` along it.
fn car_at(pos: [f32; 3], dir: [f32; 2], speed: f32) -> Player {
    Player {
        pos,
        heading: (-dir[0]).atan2(-dir[1]),
        speed,
        half_len: 2.3,
        half_w: 0.9,
    }
}

/// Distance covered by time `t` going at `v` after standing still until `t0` (from rest,
/// at 2 m/s² up to `v`), and the speed then.
fn pull_away(t: f32, t0: f32, v: f32) -> (f32, f32) {
    let t = (t - t0).max(0.0);
    let ramp = v / 2.0;
    if t < ramp {
        (t * t, 2.0 * t)
    } else {
        (ramp * ramp + v * (t - ramp), v)
    }
}

/// The scripted cars at crossing `c`: along it at 1, 2 and 3 m/s, forward and reversing,
/// from 12 m before one curb to 12 m past the other; along it at 2 m/s stopping on it
/// for 6 s and driving on; turning across it both ways at 3 and 6 m/s on 6 and 9 m
/// radii (12 m straight in, then half a circle through its middle); appearing on its
/// curb queue and standing there; along it to its middle, 3 s there and backing out, or
/// 6 s there and away to 8 m/s; and down the road along its lanes (below).
fn crossing_scripts(
    walks: &WalkGraph,
    lanes: &LaneGraph,
    c: usize,
) -> Vec<(String, f32, Script<'static>)> {
    let x = &walks.crossings[c];
    let (a, z) = (
        walks.nodes[x.curbs[0] as usize].pos,
        walks.nodes[x.curbs[1] as usize].pos,
    );
    let l = dist(a, z);
    let d = [(z[0] - a[0]) / l, (z[2] - a[2]) / l];
    let mid = [(a[0] + z[0]) * 0.5, a[1], (a[2] + z[2]) * 0.5];
    let at = move |o: [f32; 3], s: f32| [o[0] + d[0] * s, o[1], o[2] + d[1] * s];
    let mut scripts: Vec<(String, f32, Script)> = Vec::new();
    for speed in [1.0f32, 2.0, 3.0] {
        for reverse in [false, true] {
            scripts.push((
                format!("crossing {c} along {speed} m/s reversing {reverse}"),
                (l + 24.0) / speed,
                Box::new(move |t| {
                    let pos = at(a, -12.0 + speed * t);
                    // Reversing: the car faces back the way it came.
                    if reverse {
                        car_at(pos, [-d[0], -d[1]], -speed)
                    } else {
                        car_at(pos, d, speed)
                    }
                }),
            ));
        }
    }
    scripts.push((
        format!("crossing {c} along, stop and go"),
        24.0,
        Box::new(move |t| {
            let (s, v) = if t < 6.0 {
                (2.0 * t, 2.0)
            } else {
                let (s, v) = pull_away(t, 12.0, 2.0);
                (12.0 + s, if t < 12.0 { 0.0 } else { v })
            };
            car_at(at(a, -12.0 + s), d, v)
        }),
    ));
    for speed in [3.0f32, 6.0] {
        for r in [6.0f32, 9.0] {
            for side in [1.0f32, -1.0] {
                let n = [d[1] * side, -d[0] * side];
                let center = [mid[0] + n[0] * r, mid[2] + n[1] * r];
                let lead = 12.0 / speed;
                scripts.push((
                    format!("crossing {c} turn {speed} m/s radius {r} side {side}"),
                    lead + (std::f32::consts::PI * r) / speed,
                    Box::new(move |t| {
                        if t < lead {
                            // On the circle's start tangent, toward it.
                            let back = speed * (lead - t);
                            let p0 = [center[0] - d[0] * r, center[1] - d[1] * r];
                            let pos = [p0[0] + n[0] * back, mid[1], p0[1] + n[1] * back];
                            return car_at(pos, [-n[0], -n[1]], speed);
                        }
                        let t = t - lead;
                        let ang = -std::f32::consts::FRAC_PI_2 + speed * t / r;
                        let (sn, cs) = ang.sin_cos();
                        // Position on the circle from the center, and the tangent the car
                        // drives along.
                        let rad = [-n[0] * cs + d[0] * sn, -n[1] * cs + d[1] * sn];
                        let pos = [center[0] + rad[0] * r, mid[1], center[1] + rad[1] * r];
                        let tan = [d[0] * cs + n[0] * sn, d[1] * cs + n[1] * sn];
                        car_at(pos, tan, speed)
                    }),
                ));
            }
        }
    }
    scripts.push((
        format!("crossing {c} on the curb queue"),
        8.0,
        Box::new(move |_| car_at(a, d, 0.0)),
    ));
    // Creeping along it onto the curb queue for 20 s, slower than the sim counts as moving
    // and slower than it counts as creeping.
    for (speed, back) in [(0.04f32, 3.5f32), (0.2, 6.0)] {
        scripts.push((
            format!("crossing {c} creep {speed} m/s"),
            20.0,
            Box::new(move |t| car_at(at(a, -back + speed * t), d, speed)),
        ));
    }
    // Along the crosswalk at 2 m/s to its middle, 3 s there, and back out in reverse
    // (2 m/s² up to 2 m/s).
    let half = 0.5 * l;
    scripts.push((
        format!("crossing {c} along, stop, reverse"),
        (12.0 + half) + 5.0,
        Box::new(move |t| {
            let to_mid = (12.0 + half) / 2.0;
            if t < to_mid {
                return car_at(at(a, -12.0 + 2.0 * t), d, 2.0);
            }
            let (back, v) = pull_away(t, to_mid + 3.0, 2.0);
            car_at(at(a, half - back), d, -v)
        }),
    ));
    // Down the road both ways, each along a lane through the crosswalk (as the lane graph
    // lays it, straight on past its ends): stopping on it and backing out; stopping 6 s
    // and pulling away to 8 m/s; 8 m/s along the curb lane, the car's side 0.3 m off the
    // curb face; two passes 3 s apart.
    let x = &walks.crossings[c];
    let road = walks
        .links
        .iter()
        .find(|k| {
            k.kind == LinkKind::Crossing
                && k.crossing == c as u32
                && !x.curbs.contains(&k.a)
                && !x.curbs.contains(&k.b)
        })
        .map_or(mid, |k| {
            let (p, q) = (walks.nodes[k.a as usize].pos, walks.nodes[k.b as usize].pos);
            [
                (p[0] + q[0]) * 0.5,
                (p[1] + q[1]) * 0.5,
                (p[2] + q[2]) * 0.5,
            ]
        });
    let hw = 0.5 * x.length_m - 0.35;
    let across = [d[1], -d[0]];
    for way in [1.0f32, -1.0] {
        let Some(path) = lane_through(lanes, x.node, road, hw, [across[0] * way, across[1] * way])
        else {
            continue;
        };
        let at_xing = path.nearest(road);
        // Only where the lane's line runs clear of the curbs (where the road bends into
        // the junction its line straight on would not).
        let clear = (-40..=40).all(|s| {
            let (p, _) = path.at(at_xing + s as f32);
            x.curbs
                .iter()
                .all(|&n| dist(p, walks.nodes[n as usize].pos) > 1.8)
        });
        if !clear {
            continue;
        }
        let path = std::sync::Arc::new(path);
        let (p0, f0) = path.at(at_xing);
        let right = [-f0[1], f0[0]];
        // The lane's middle off the road's, to the car's right, and the shift that puts
        // the car's side 0.3 m off the curb.
        let lat0 = (p0[0] - road[0]) * right[0] + (p0[2] - road[2]) * right[1];
        let curb = hw - 0.3 - 0.9 - lat0;
        let on = {
            let path = path.clone();
            move |s: f32, shift: f32, v: f32| {
                let (p, f) = path.at(at_xing + s);
                let r = [-f[1], f[0]];
                car_at([p[0] + r[0] * shift, p[1], p[2] + r[1] * shift], f, v)
            }
        };
        let o = on.clone();
        scripts.push((
            format!("crossing {c} road {way} stop, reverse"),
            5.0 + 3.0 + 1.0 + 7.0,
            Box::new(move |t| {
                if t < 5.0 {
                    return o(-15.0 + 3.0 * t, 0.0, 3.0);
                }
                let (back, v) = pull_away(t, 8.0, 2.0);
                o(-back, 0.0, -v)
            }),
        ));
        let o = on.clone();
        scripts.push((
            format!("crossing {c} road {way} stop 6 s, away to 8 m/s"),
            5.0 + 6.0 + 4.0 + 3.0,
            Box::new(move |t| {
                if t < 5.0 {
                    return o(-15.0 + 3.0 * t, 0.0, 3.0);
                }
                let (s, v) = pull_away(t, 11.0, 8.0);
                o(s, 0.0, v)
            }),
        ));
        let o = on.clone();
        scripts.push((
            format!("crossing {c} road {way} curb lane 8 m/s"),
            10.0,
            Box::new(move |t| o(-40.0 + 8.0 * t, curb, 8.0)),
        ));
        // At 6 m/s, the second car taking over 9 m back once the first is 9 m past.
        scripts.push((
            format!("crossing {c} road {way} two passes"),
            10.0,
            Box::new(move |t| {
                let s = -20.0 + 6.0 * t;
                on(if s > 9.0 { s - 18.0 } else { s }, 0.0, 6.0)
            }),
        ));
    }
    // Along the crosswalk to its middle, 6 s there, then away at 2 m/s² to 8 m/s.
    scripts.push((
        format!("crossing {c} along, stop 6 s, away to 8 m/s"),
        (12.0 + half) / 2.0 + 6.0 + 4.0 + 2.0,
        Box::new(move |t| {
            let to_mid = (12.0 + half) / 2.0;
            if t < to_mid {
                return car_at(at(a, -12.0 + 2.0 * t), d, 2.0);
            }
            let (s, v) = pull_away(t, to_mid + 6.0, 8.0);
            car_at(at(a, half + s), d, v)
        }),
    ));
    scripts
}

/// A lane's centerline, straight on past both its ends, by arc length.
struct LanePath {
    pts: Vec<[f32; 3]>,
    arc: Vec<f32>,
}

impl LanePath {
    fn new(mut pts: Vec<[f32; 3]>) -> LanePath {
        let ext = |a: [f32; 3], b: [f32; 3]| {
            let l = dist(a, b).max(1e-3);
            [
                b[0] + (b[0] - a[0]) / l * 80.0,
                b[1],
                b[2] + (b[2] - a[2]) / l * 80.0,
            ]
        };
        let n = pts.len();
        let (head, tail) = (ext(pts[1], pts[0]), ext(pts[n - 2], pts[n - 1]));
        pts.insert(0, head);
        pts.push(tail);
        let mut arc = vec![0.0];
        for w in pts.windows(2) {
            arc.push(arc.last().unwrap() + dist(w[0], w[1]));
        }
        LanePath { pts, arc }
    }

    /// Position and ground direction `s` along.
    fn at(&self, s: f32) -> ([f32; 3], [f32; 2]) {
        let k = self.arc[1..]
            .partition_point(|&a| a < s)
            .min(self.pts.len() - 2);
        let (a, b) = (self.pts[k], self.pts[k + 1]);
        let l = (self.arc[k + 1] - self.arc[k]).max(1e-3);
        let t = (s - self.arc[k]) / l;
        let p = [a[0] + (b[0] - a[0]) * t, a[1], a[2] + (b[2] - a[2]) * t];
        (p, [(b[0] - a[0]) / l, (b[2] - a[2]) / l])
    }

    /// Arc length of the point nearest `p`.
    fn nearest(&self, p: [f32; 3]) -> f32 {
        let mut best = (f32::MAX, 0.0);
        for (k, w) in self.pts.windows(2).enumerate() {
            let l = dist(w[0], w[1]).max(1e-3);
            let d = [(w[1][0] - w[0][0]) / l, (w[1][2] - w[0][2]) / l];
            let t = ((p[0] - w[0][0]) * d[0] + (p[2] - w[0][2]) * d[1]).clamp(0.0, l);
            let q = [w[0][0] + d[0] * t, p[1], w[0][2] + d[1] * t];
            if dist(p, q) < best.0 {
                best = (dist(p, q), self.arc[k] + t);
            }
        }
        best.1
    }
}

/// A lane into or out of junction `node` through the crosswalk with its road's middle at
/// `road` (half width `hw`), driving along `dir` there: one whose end there is within
/// a few meters of the crosswalk, at its level.
fn lane_through(
    lanes: &LaneGraph,
    node: u32,
    road: [f32; 3],
    hw: f32,
    dir: [f32; 2],
) -> Option<LanePath> {
    lanes
        .lanes
        .iter()
        .enumerate()
        .filter(|(_, l)| l.end_node == node || l.start_node == node)
        .filter_map(|(i, l)| {
            let pts = lanes.points(i as u32);
            let end = if l.end_node == node {
                pts[pts.len() - 1]
            } else {
                pts[0]
            };
            if dist(end, road) > hw + 8.0 || (end[1] - road[1]).abs() > 0.5 {
                return None;
            }
            let path = LanePath::new(pts.to_vec());
            let (p, f) = path.at(path.nearest(road));
            let off = dist(p, road);
            (off < hw && f[0] * dir[0] + f[1] * dir[1] > 0.9).then_some((off, path))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|b| b.1)
}

/// Turning across crossing `c` at 6 m/s on 6 and 9 m radii both ways, as in
/// [`crossing_scripts`]: run from a moment with five or more people on it.
fn crowded_scripts(
    walks: &WalkGraph,
    lanes: &LaneGraph,
    c: usize,
) -> Vec<(String, f32, Script<'static>)> {
    crossing_scripts(walks, lanes, c)
        .into_iter()
        .filter(|(name, _, _)| name.contains("turn 6 m/s"))
        .map(|(name, secs, script)| (format!("{name} crowded"), secs, script))
        .collect()
}

/// The ways a player's car reaches people, on coast seeds 1–4: the crossings scripts
/// ([`crossing_scripts`]) at the two crossings with the most people near them (every
/// crossing with anyone within 25 m with `SKYWAVE_ALL_CROSSINGS=1`), and on the busiest
/// walk, standing 2.4 m short of someone and pulling away toward them, and parking after
/// a 2.5 m/s approach; the 6 m/s turns again on each crossing once five or more people are
/// on it. Nobody is ever inside the car or within [`BODY_GAP_M`] of it, or in a lane off a
/// crossing; a dodge on the sidewalk stays within the walks' bands (the gutter beside a
/// walk the car is on, at most), carries nobody more than 2 m, an episode of them no more
/// than [`EPISODE_M`] once the car stops coming; nobody stays by a stopped car longer than
/// [`STOPPED_S`] and nobody moves faster than 4 m/s.
#[test]
fn nobody_is_reached_by_a_scripted_car() {
    let _quiet = shared();
    let only = std::env::var("SKYWAVE_CASE").ok();
    let all = only.is_some() || std::env::var("SKYWAVE_ALL_CROSSINGS").is_ok_and(|v| v == "1");
    let only = &only;
    let ron = REGIONS[0].1;
    let t: RegionTemplate = ron::from_str(ron).expect("parse");
    let grammar = grammar();
    let results: Vec<Vec<(String, Reach)>> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (1..=4u64)
            .map(|seed| {
                let (t, grammar) = (&t, &grammar);
                scope.spawn(move || {
                    let place = build_place(Seed(seed), t, grammar);
                    let lanes = LaneGraph::new(&place);
                    let walks = WalkGraph::new(&place, &lanes);
                    let index = LaneIndex::new(&place, &lanes);
                    let collider = WorldCollider::build(&place, grammar);
                    let scene = Scene {
                        walks: &walks,
                        lanes: &lanes,
                        index: &index,
                        collider: &collider,
                    };
                    let eye = busy_eye(&walks);
                    let mut warm = Pedestrians::new(
                        &walks,
                        Seed(seed),
                        place.header.era,
                        PedestrianBudget::LARGE,
                    );
                    for k in 0..120 * 30 {
                        let inp = PedInput {
                            tick: k,
                            density: Density::Busy,
                            view: view_at(eye, k),
                            player: Player::ABSENT,
                        };
                        warm.step(&walks, &lanes, &inp);
                    }
                    let t0 = 120 * 30;
                    let mut out = Vec::new();
                    let mut busy: Vec<(usize, usize)> = walks
                        .crossings
                        .iter()
                        .enumerate()
                        .map(|(c, x)| {
                            let at = walks.nodes[x.curbs[0] as usize].pos;
                            (warm.iter().filter(|p| dist(p.pos, at) < 25.0).count(), c)
                        })
                        .filter(|b| b.0 > 0)
                        .collect();
                    busy.sort_by_key(|b| std::cmp::Reverse(b.0));
                    let take = if all { busy.len() } else { 2 };
                    let chosen: Vec<usize> = busy.iter().take(take).map(|b| b.1).collect();
                    for &c in &chosen {
                        for (name, secs, script) in crossing_scripts(&walks, &lanes, c) {
                            if only
                                .as_ref()
                                .is_some_and(|o| format!("seed {seed} {name}") != *o)
                            {
                                continue;
                            }
                            let r = drive_script(&scene, &warm, t0, secs, &script);
                            out.push((format!("seed {seed} {name}"), r));
                        }
                    }
                    // On from the warm people with no car, the first moment each chosen
                    // crossing has five or more people on it (at its curbs or across):
                    // the turns at 6 m/s from there.
                    let mut crowded: Vec<Option<(u64, Pedestrians)>> = vec![None; chosen.len()];
                    let mut on = warm.clone();
                    let mut count = vec![0usize; walks.crossings.len()];
                    for k in 0..120 * 60u64 {
                        if k % 12 == 0 {
                            count.fill(0);
                            for i in 0..on.pedestrians().len() {
                                if let Some(x) = on.crossing_of(&walks, i) {
                                    count[x as usize] += 1;
                                }
                            }
                            for (j, &c) in chosen.iter().enumerate() {
                                if crowded[j].is_none() && count[c] >= 5 {
                                    crowded[j] = Some((t0 + k, on.clone()));
                                }
                            }
                            if crowded.iter().all(Option::is_some) {
                                break;
                            }
                        }
                        let inp = PedInput {
                            tick: t0 + k,
                            density: Density::Busy,
                            view: view_at(eye, t0 + k),
                            player: Player::ABSENT,
                        };
                        on.step(&walks, &lanes, &inp);
                    }
                    for (&c, at) in chosen.iter().zip(&crowded) {
                        let Some((tick, people)) = at else {
                            continue;
                        };
                        for (name, secs, script) in crowded_scripts(&walks, &lanes, c) {
                            if only
                                .as_ref()
                                .is_some_and(|o| format!("seed {seed} {name}") != *o)
                            {
                                continue;
                            }
                            let r = drive_script(&scene, people, *tick, secs, &script);
                            out.push((format!("seed {seed} {name}"), r));
                        }
                    }
                    // On the busiest walk: stopped 2.4 m short of the person there, then
                    // pulling away at them; and parking there after 2.5 m/s.
                    let p = warm
                        .iter()
                        .filter(|p| p.pose.action == Action::None)
                        .max_by_key(|p| warm.iter().filter(|q| dist(q.pos, p.pos) < 15.0).count())
                        .expect("people")
                        .pos;
                    let dir = walks
                        .links
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| l.kind == LinkKind::Walk && l.road_side != 0)
                        .min_by(|(i, _), (j, _)| {
                            let ai = walks.point(*i as u32, 0.0, 0.0);
                            let aj = walks.point(*j as u32, 0.0, 0.0);
                            dist(ai, p).total_cmp(&dist(aj, p))
                        })
                        .map(|(_, l)| l.dir)
                        .expect("walks");
                    let at = move |s: f32| [p[0] - dir[0] * s, p[1], p[2] - dir[1] * s];
                    let stop_go: Script = Box::new(move |t| {
                        let (s, v) = pull_away(t, 3.0, 3.0);
                        car_at(at(2.4 + 2.3 - s), dir, if t < 3.0 { 0.0 } else { v })
                    });
                    let park: Script = Box::new(move |t| {
                        let tt = t.min(8.0);
                        car_at(at(20.0 - 2.5 * tt), dir, if t < 8.0 { 2.5 } else { 0.0 })
                    });
                    for (name, secs, script) in [
                        ("walk stop then pull away", 12.0, &stop_go),
                        ("walk parked after 2.5 m/s", 30.0, &park),
                    ] {
                        if only
                            .as_ref()
                            .is_some_and(|o| format!("seed {seed} {name}") != *o)
                        {
                            continue;
                        }
                        let r = drive_script(&scene, &warm, t0, secs, script);
                        out.push((format!("seed {seed} {name}"), r));
                    }
                    out
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().expect("seed")).collect()
    });
    let mut sum = Reach {
        nearest: f32::MAX,
        ..Reach::default()
    };
    let (mut runs, mut crowded) = (0, 0);
    for (name, r) in results.iter().flatten() {
        let bad = r.inside > 0
            || r.short > 0
            || r.in_lane > 0
            || r.off_walk > 0.15
            || r.longest_move > 2.05
            || r.fastest > 4.05
            || r.episode > EPISODE_M + 0.05
            || r.stood > STOPPED_S;
        if bad {
            println!(
                "{name}: {} person-steps inside, {} within {BODY_GAP_M} m (nearest {:.3} m), \
                 {} lane samples, {:.2} m out of the walks, longest move {:.2} m, episode \
                 {:.2} m, fastest {:.2} m/s, stood {:.1} s by the stopped car",
                r.inside,
                r.short,
                r.nearest,
                r.in_lane,
                r.off_walk,
                r.longest_move,
                r.episode,
                r.fastest,
                r.stood
            );
        }
        crowded += usize::from(name.ends_with("crowded"));
        sum.inside += r.inside;
        sum.short += r.short;
        sum.nearest = sum.nearest.min(r.nearest);
        sum.episode = sum.episode.max(r.episode);
        sum.stood = sum.stood.max(r.stood);
        sum.near += r.near;
        sum.in_lane += r.in_lane;
        sum.off_walk = sum.off_walk.max(r.off_walk);
        sum.longest_move = sum.longest_move.max(r.longest_move);
        sum.fastest = sum.fastest.max(r.fastest);
        runs += 1;
    }
    println!(
        "{runs} scripted runs ({crowded} on crowded crossings), {} people within 6 m, {} \
         person-steps inside the car, {} within {BODY_GAP_M} m of it (nearest {:.3} m), {} \
         lane samples off a crossing, dodges at most {:.2} m out of the walks, longest move \
         {:.2} m, longest episode {:.2} m, fastest {:.2} m/s, at most {:.1} s by a stopped car",
        sum.near,
        sum.inside,
        sum.short,
        sum.nearest,
        sum.in_lane,
        sum.off_walk,
        sum.longest_move,
        sum.episode,
        sum.fastest,
        sum.stood
    );
    assert_eq!(sum.inside, 0);
    assert_eq!(sum.short, 0, "nearest {}", sum.nearest);
    assert!(sum.episode <= EPISODE_M + 0.05, "{}", sum.episode);
    assert!(sum.stood <= STOPPED_S, "{}", sum.stood);
    assert!(
        crowded > 0 || only.is_some(),
        "no crossing had five people on it"
    );
    assert_eq!(sum.in_lane, 0);
    // A few centimeters for how the walks' straight bands meet at their nodes (around a
    // corner people may step 0.3 m past a corner's room, still on the sidewalk).
    assert!(sum.off_walk <= 0.15, "{}", sum.off_walk);
    // A few centimeters over for the walker's own step during the move and rounding.
    assert!(sum.longest_move <= 2.05, "{}", sum.longest_move);
    assert!(sum.fastest <= 4.05, "{}", sum.fastest);
    assert!(sum.near > 100, "{}", sum.near);
}
