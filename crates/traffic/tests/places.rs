//! Lane graphs of the procedural places: every region at seeds 1..=3 for connectivity,
//! curves, signal phases and control against what the world draws; the coast at seed 1
//! for the pinned hash and the size budget. Run with `--nocapture` to see the numbers:
//! `cargo test -p traffic --test places -- --nocapture`.

use std::sync::OnceLock;

use place::{Place, PropKind, RoadClass};
use skywave_core::Seed;
use traffic::lanes::{CURVE_STEPS, WAVE_DELAY_TICKS};
use traffic::signal::cycle_ticks;
use traffic::{Control, LaneGraph, Light, TurnKind, NONE};
use world::procgen::build_place;
use world::roads::Network;
use world::template::RegionTemplate;

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
const SEEDS: [u64; 3] = [1, 2, 3];
const GRAMMAR: &str = include_str!("../../../assets/grammar/california_coast_80s.ron");

/// Pinned [`LaneGraph::hash`] of the coast at seed 1. The graph is a pure function of the
/// place, so this moves when lane building or the place's roads or props change; update it
/// in its own commit with the reason.
const COAST_SEED1_HASH: u64 = 0x8a7d_05e8_0195_3c0d;

/// Size ceilings for the coast's graph (16×16 chunks); about twice what seed 1 needs.
const MAX_LANES: usize = 2_400;
const MAX_TURNS: usize = 4_500;
const MAX_BYTES: usize = 768 * 1024;

/// Turns tighter than this (other than U-turns) come from roads squeezed between two close
/// junctions and from acute corners; car following slows for them by `min_radius_m`, and
/// they must stay rare.
const SHARP_RADIUS_M: f32 = 3.0;
const MAX_SHARP_SHARE: f32 = 0.05;

/// Share of lanes that must lie in the largest strongly connected component: from any of
/// them a car can reach any other. The rest are on road islands (validation allows 5% of
/// edges off the main component) and one-way dead ends.
const MIN_CONNECTED_SHARE: f32 = 0.95;

struct Built {
    name: String,
    place: Place,
    graph: LaneGraph,
}

/// Every region at every seed, built once, in parallel.
fn places() -> &'static [Built] {
    static PLACES: OnceLock<Vec<Built>> = OnceLock::new();
    PLACES.get_or_init(|| {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = REGIONS
                .iter()
                .flat_map(|&(region, ron)| SEEDS.iter().map(move |&seed| (region, ron, seed)))
                .map(|(region, ron, seed)| {
                    scope.spawn(move || {
                        let t: RegionTemplate = ron::from_str(ron).expect("parse");
                        t.validate().expect("validate");
                        let g: world::Grammar = ron::from_str(GRAMMAR).expect("grammar");
                        let place = build_place(Seed(seed), &t, &g);
                        let graph = LaneGraph::new(&place);
                        Built {
                            name: format!("{region} seed {seed}"),
                            place,
                            graph,
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

#[test]
fn coast_graph_hash_is_pinned() {
    let b = coast();
    assert_eq!(b.name, "california_coast seed 1");
    assert_eq!(
        LaneGraph::new(&b.place),
        b.graph,
        "rebuilding gives the same graph"
    );
    let h = b.graph.hash();
    println!("coast seed 1 lane graph hash: {h:#018x}");
    assert_eq!(h, COAST_SEED1_HASH);
}

/// Largest strongly connected component of the lane graph (lanes linked by turns), as a
/// count of lanes. Iterative Tarjan.
fn largest_component(g: &LaneGraph) -> usize {
    let n = g.lanes.len();
    let (mut index, mut low) = (vec![u32::MAX; n], vec![0u32; n]);
    let mut on_stack = vec![false; n];
    let (mut stack, mut next, mut best) = (Vec::new(), 0u32, 0usize);
    for root in 0..n {
        if index[root] != u32::MAX {
            continue;
        }
        // (lane, next turn to look at)
        let mut work = vec![(root, 0usize)];
        while let Some(&mut (v, ref mut k)) = work.last_mut() {
            if *k == 0 && index[v] == u32::MAX {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            let turns = g.next(v as u32);
            if *k < turns.len() {
                let w = turns[*k].to as usize;
                *k += 1;
                if index[w] == u32::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut size = 0;
                loop {
                    let w = stack.pop().expect("stack");
                    on_stack[w] = false;
                    size += 1;
                    if w == v {
                        break;
                    }
                }
                best = best.max(size);
            }
        }
    }
    best
}

#[test]
fn every_lane_leads_somewhere_and_the_graph_is_connected() {
    for b in places() {
        let (place, g) = (&b.place, &b.graph);
        let mut controls = [0usize; 4];
        let mut kinds = [0usize; 4];
        for (id, l) in g.lanes.iter().enumerate() {
            controls[l.control as usize] += 1;
            assert!(l.length_m > 0.0, "{}: lane {id} has no length", b.name);
            let degree = g.junctions[l.end_node as usize].degree;
            if l.control == Control::DeadEnd {
                assert_eq!(degree, 1, "{}: lane {id} is stuck at a junction", b.name);
                // Back out the other side, unless the road is one-way.
                let oneway = place.roads.edges[l.edge as usize].oneway;
                assert_eq!(l.turn_count, u32::from(!oneway), "{}: lane {id}", b.name);
            } else {
                assert!(l.turn_count > 0, "{}: lane {id} has no turn", b.name);
            }
            for t in g.next(id as u32) {
                kinds[t.kind as usize] += 1;
                assert_eq!(
                    g.lanes[t.to as usize].start_node, l.end_node,
                    "{}: turn leaves from where the lane ends",
                    b.name
                );
            }
        }
        let big = largest_component(g);
        let share = big as f32 / g.lanes.len() as f32;
        println!(
            "{}: lanes {} (free {}, stop {}, signal {}, dead end {}); turns {} (straight {}, \
             left {}, right {}, u {}); stop lines {}; connected {:.1}%",
            b.name,
            g.lanes.len(),
            controls[0],
            controls[1],
            controls[2],
            controls[3],
            g.turns.len(),
            kinds[0],
            kinds[1],
            kinds[2],
            kinds[3],
            g.stop_lines.len(),
            share * 100.0
        );
        assert!(
            share >= MIN_CONNECTED_SHARE,
            "{}: {big} lanes connected",
            b.name
        );
        assert!(
            controls[Control::Stop as usize] > 0,
            "{}: no stop signs",
            b.name
        );
    }
}

#[test]
fn turn_curves_are_continuous() {
    for b in places() {
        let g = &b.graph;
        let mut sharp = 0usize;
        for (i, t) in g.turns.iter().enumerate() {
            let i = i as u32;
            let (a, e) = (g.end_pose(t.from), g.pose(t.to, 0.0));
            let (s, f) = (g.turn_pose(i, 0.0), g.turn_pose(i, t.length_m));
            // Positions meet the lanes at both ends; directions match there.
            for (p, q) in [(s.pos, a.pos), (f.pos, e.pos)] {
                let d = (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0, f32::max);
                assert!(d < 1e-3, "{}: turn {i} misses its lane by {d}", b.name);
            }
            if t.length_m > 0.05 {
                for (p, q) in [(s.dir, a.dir), (f.dir, e.dir)] {
                    let c = p[0] * q[0] + p[1] * q[1];
                    assert!(c > 0.999, "{}: turn {i} kinks at an end: {c}", b.name);
                }
            }
            // No gaps or jumps along the curve.
            let n = 4 * CURVE_STEPS;
            let mut prev = s;
            for k in 1..=n {
                let p = g.turn_pose(i, t.length_m * k as f32 / n as f32);
                let step = dist(p.pos, prev.pos);
                assert!(
                    step <= 2.0 * t.length_m / n as f32 + 1e-3,
                    "{}: turn {i} jumps",
                    b.name
                );
                prev = p;
            }
            if t.min_radius_m < SHARP_RADIUS_M && t.kind != TurnKind::UTurn {
                sharp += 1;
            }
        }
        let share = sharp as f32 / g.turns.len() as f32;
        println!("{}: sharp turns {sharp} ({:.2}%)", b.name, share * 100.0);
        assert!(share < MAX_SHARP_SHARE, "{}: too many sharp turns", b.name);
    }
}

#[test]
fn crossing_approaches_never_go_together() {
    let mut junctions = 0;
    let mut most_phases = 0;
    for b in places() {
        let g = &b.graph;
        for (node, j) in g.junctions.iter().enumerate() {
            if j.phases == 0 {
                continue;
            }
            junctions += 1;
            most_phases = most_phases.max(j.phases);
            // One lane per approach: its travel direction coming in.
            let mut approaches: Vec<(u32, [f32; 2])> = Vec::new();
            for (id, l) in g.lanes.iter().enumerate() {
                if l.end_node as usize == node && l.slot == 0 {
                    assert_eq!(l.control, Control::Signal, "{}: node {node}", b.name);
                    assert!(l.phase < j.phases);
                    approaches.push((id as u32, g.end_pose(id as u32).dir));
                }
            }
            for tick in (0..u64::from(cycle_ticks(j.phases))).step_by(30) {
                let going: Vec<[f32; 2]> = approaches
                    .iter()
                    .filter(|a| g.light(a.0, tick) != Some(Light::Red))
                    .map(|a| a.1)
                    .collect();
                for (x, p) in going.iter().enumerate() {
                    for q in &going[x + 1..] {
                        let c = p[0] * q[0] + p[1] * q[1];
                        // Only nearly opposite streams share a green (slack for roads that
                        // curve between their node and their stop line).
                        assert!(c < -0.6, "{}: node {node} greens crossing streams", b.name);
                    }
                }
            }
        }
    }
    println!("signal junctions {junctions}, most phases {most_phases}");
    assert!(junctions > 0);
}

/// Which lanes are on the main roads (arterials and the beachfront), the ones the lane
/// graph times as green waves.
fn main_lanes(b: &Built) -> Vec<bool> {
    b.graph
        .lanes
        .iter()
        .map(|l| {
            matches!(
                b.place.roads.edges[l.edge as usize].class,
                RoadClass::Arterial | RoadClass::Beachfront
            )
        })
        .collect()
}

/// A car holding the posted speed down the main roads: past the stop line of `from` (a
/// signal approach) at `tick`, straight on through `first`, and on through every signal
/// after for as long as the main road goes, at most `most` signals. Returns how many in a
/// row it meets green, with each junction's offset from `offset`.
fn green_run(
    g: &LaneGraph,
    main: &[bool],
    from: u32,
    first: u32,
    tick: u64,
    most: u32,
    offset: &dyn Fn(usize) -> u32,
) -> u32 {
    let light = |lane: u32, t: f32| {
        let l = &g.lanes[lane as usize];
        let j = &g.junctions[l.end_node as usize];
        let off = offset(l.end_node as usize);
        traffic::signal::light(off, l.phase, j.phases, tick + (t * 120.0) as u64)
    };
    let turn = |a: u32, b: u32| g.next(a).iter().find(|x| x.to == b).unwrap().length_m;
    let mut t = turn(from, first) / g.lanes[first as usize].speed_mps;
    let mut lane = first;
    let mut greens = 0;
    while greens < most {
        let Some((arr, dt)) = g.wave_link(lane, main) else {
            break;
        };
        t += dt;
        if light(arr, t) != Light::Green {
            break;
        }
        greens += 1;
        let Some(x) = g
            .next(arr)
            .iter()
            .find(|x| x.kind == TurnKind::Straight && main[x.to as usize])
        else {
            break;
        };
        t += x.length_m / g.lanes[arr as usize].speed_mps;
        lane = x.to;
    }
    greens
}

#[test]
fn a_car_at_the_limit_rides_a_green_wave_down_the_main_roads() {
    let mut coast_best = 0;
    for b in places() {
        let g = &b.graph;
        let main = main_lanes(b);
        let seed = b.place.header.seed;
        let timed = |n: usize| g.junctions[n].offset;
        let hashed = |n: usize| traffic::signal::offset(seed, n as u32, g.junctions[n].phases);
        // Next signal on green, after one passed on green, for every link and five
        // moments through the green; then the longest run anywhere.
        let (mut links, mut wave, mut random, mut best, mut best_hashed) = (0, 0, 0, 0, 0);
        // The same for the links timed down the wave (the next signal timed from this one).
        let (mut with, mut with_green) = (0, 0);
        for t in &g.turns {
            let (from, first) = (t.from, t.to);
            let l = &g.lanes[from as usize];
            if t.kind != TurnKind::Straight
                || l.control != Control::Signal
                || !main[first as usize]
                || g.lanes[first as usize].slot != 0
            {
                continue;
            }
            let j = &g.junctions[l.end_node as usize];
            let cycle = u64::from(cycle_ticks(j.phases));
            // When `from`'s green starts.
            let start = (0..cycle)
                .find(|&k| {
                    g.light(from, k) == Some(Light::Green)
                        && g.light(from, k + cycle - 1) != Some(Light::Green)
                })
                .unwrap();
            let Some((arr, dt)) = g.wave_link(first, &main) else {
                continue;
            };
            let next = g.lanes[arr as usize].end_node as usize;
            let v = g.lanes[first as usize].speed_mps;
            let ticks = ((dt + t.length_m / v) * 120.0 + 0.5) as u32 + WAVE_DELAY_TICKS;
            let down = g.junctions[next].phases == j.phases
                && g.junctions[next].offset
                    == traffic::signal::follow(
                        j.offset,
                        l.phase,
                        g.lanes[arr as usize].phase,
                        j.phases,
                        ticks,
                    );
            for s in [1u64, 6, 12, 18, 24] {
                let tick = start + s * 120;
                links += 1;
                let green = green_run(g, &main, from, first, tick, 1, &timed);
                wave += green;
                if down {
                    with += 1;
                    with_green += green;
                }
                random += green_run(g, &main, from, first, tick, 1, &hashed);
                best = best.max(green_run(g, &main, from, first, tick, 12, &timed));
                best_hashed = best_hashed.max(green_run(g, &main, from, first, tick, 12, &hashed));
            }
        }
        let share = |n: u32| n as f32 / links.max(1) as f32;
        println!(
            "{}: {links} main-road link drives, next light green {:.1}% timed ({:.1}% with hashed offsets), {:.1}% of {with} down the wave; longest run {best} ({best_hashed})",
            b.name,
            100.0 * share(wave),
            100.0 * share(random),
            100.0 * with_green as f32 / with.max(1) as f32,
        );
        assert!(with_green as f32 >= 0.95 * with as f32, "{}", b.name);
        if links > 0 {
            assert!(share(wave) > share(random), "{}", b.name);
        }
        if b.name == "california_coast seed 1" {
            coast_best = best;
            assert!(share(wave) >= 0.7, "{}", b.name);
        }
    }
    assert!(
        coast_best >= 4,
        "longest green run on the coast {coast_best}"
    );
}

/// A drive at the posted speed down the coast's longest green wave (the best run from any
/// main-road approach 6 s into its green): the lane and turn pieces from 40 m before the
/// first stop line on through the last green, the tick it passes that line, the greens
/// expected (that one too).
fn longest_wave(b: &Built) -> (Vec<(bool, u32, f32)>, u64, u32) {
    let g = &b.graph;
    let main = main_lanes(b);
    let timed = |n: usize| g.junctions[n].offset;
    let mut best = (0, 0, 0, 0u64);
    for t in &g.turns {
        let l = &g.lanes[t.from as usize];
        if t.kind != TurnKind::Straight || l.control != Control::Signal || !main[t.to as usize] {
            continue;
        }
        let cycle = u64::from(cycle_ticks(g.junctions[l.end_node as usize].phases));
        let start = (0..cycle)
            .find(|&k| {
                g.light(t.from, k) == Some(Light::Green)
                    && g.light(t.from, k + cycle - 1) != Some(Light::Green)
            })
            .unwrap();
        let tick = start + 6 * 120;
        let n = green_run(g, &main, t.from, t.to, tick, 12, &timed);
        if n > best.0 {
            best = (n, t.from, t.to, tick);
        }
    }
    let (n, from, first, tick) = best;
    let turn_id = |a: u32, b: u32| {
        let l = &g.lanes[a as usize];
        (l.first_turn..l.first_turn + l.turn_count)
            .find(|&k| g.turns[k as usize].to == b)
            .unwrap()
    };
    let mut path = vec![(false, from, 40.0f32.min(g.lanes[from as usize].length_m))];
    let (mut lane, mut prev) = (first, from);
    let mut signals = 0;
    while signals < n {
        let t = turn_id(prev, lane);
        path.push((true, t, g.turns[t as usize].length_m));
        let l = &g.lanes[lane as usize];
        path.push((false, lane, l.length_m));
        signals += u32::from(l.control == Control::Signal);
        let next = g
            .next(lane)
            .iter()
            .find(|x| x.kind == TurnKind::Straight && main[x.to as usize]);
        if signals == n {
            // On across the last junction.
            if let Some(x) = next {
                path.push((true, turn_id(lane, x.to), x.length_m));
            }
            break;
        }
        (prev, lane) = (lane, next.unwrap().to);
    }
    (path, tick, n + 1)
}

/// Drives `path` (from [`longest_wave`]) at `speed(seconds, posted)` m/s through a
/// [`traffic::Streak`], passing the first stop line at about `tick`; returns the streak's
/// run after every step.
fn drive_streak(
    g: &LaneGraph,
    path: &[(bool, u32, f32)],
    tick: u64,
    speed: &dyn Fn(f32, f32) -> f32,
) -> (Vec<u8>, u32) {
    let mut streak = traffic::Streak::new(g);
    let posted = g.lanes[path[0].1 as usize].speed_mps;
    let mut t0 = tick - (path[0].2 / posted * 120.0) as u64;
    let mut runs = Vec::new();
    let (mut piece, mut s) = (0, 0.0f32);
    let lane0 = &g.lanes[path[0].1 as usize];
    s += lane0.length_m - path[0].2;
    let mut k = 0u64;
    while piece < path.len() {
        let (turn, id, _) = path[piece];
        let (pose, len) = if turn {
            (g.turn_pose(id, s), g.turns[id as usize].length_m)
        } else {
            (g.pose(id, s), g.lanes[id as usize].length_m)
        };
        let v = speed(k as f32 / 120.0, posted);
        let car = traffic::Player {
            pos: pose.pos,
            heading: pose.heading,
            speed: v,
            half_len: 2.3,
            half_w: 0.9,
        };
        streak.step(g, t0, &car, 1.0 / 120.0);
        runs.push(streak.run);
        s += v / 120.0;
        if s > len {
            s -= len;
            piece += 1;
        }
        t0 += 1;
        k += 1;
    }
    (runs, streak.greens)
}

#[test]
fn the_streak_counts_a_green_wave_and_ends_quietly() {
    let b = coast();
    let g = &b.graph;
    let (path, tick, greens) = longest_wave(b);
    println!("longest wave on the coast: {greens} greens in a row at the posted speed");
    assert!(greens >= 5, "{greens}");
    // Holding the limit: every light adds one.
    let (runs, counted) = drive_streak(g, &path, tick, &|_, v| v);
    assert_eq!(counted, greens);
    assert_eq!(*runs.last().unwrap(), greens as u8);
    assert!(runs.windows(2).all(|w| w[1] == w[0] || w[1] == w[0] + 1));
    // A hard brake for a second after the second light ends the run there; the next light
    // starts again at 1.
    let after = |runs: &[u8], n: u8| runs.iter().position(|&r| r == n).unwrap();
    let second = after(&runs, 2) as f32 / 120.0;
    let brake = move |t: f32, v: f32| {
        let into = t - (second + 0.5);
        if (0.0..1.0).contains(&into) {
            v - 6.0 * into
        } else if (1.0..2.0).contains(&into) {
            v - 6.0 * (2.0 - into)
        } else {
            v
        }
    };
    let (braked, _) = drive_streak(g, &path, tick, &brake);
    let k = ((second + 0.9) * 120.0) as usize;
    assert_eq!(braked[k], 0, "the hard brake ends the run");
    assert!(braked[k..].contains(&1));
    // Speeding a fifth over the limit never gets a run going.
    let (fast, n) = drive_streak(g, &path, tick, &|_, v| v * 1.25);
    assert!(fast.iter().all(|&r| r == 0), "{n}");
    // Through the first light on red (a cycle's half later): the run starts after it.
    let (late, _) = drive_streak(g, &path, tick + 30 * 120, &|_, v| v);
    assert!(
        late[..((path[0].2 / g.lanes[path[0].1 as usize].speed_mps + 0.5) * 120.0) as usize]
            .iter()
            .all(|&r| r == 0)
    );
}

/// Whether a drawn stop sign (`signal` false) or signal pole stands within `reach` of `at`.
fn sign_near(place: &Place, at: [f32; 3], signal: bool, reach: f32) -> bool {
    place.props.items.iter().any(|p| {
        let kind = match p.kind {
            PropKind::StopSign => false,
            PropKind::Signal { .. } => true,
            _ => return false,
        };
        kind == signal && dist(p.pos, at) < reach
    })
}

#[test]
fn control_matches_the_drawn_signs() {
    for b in places() {
        let (place, g) = (&b.place, &b.graph);
        let net = Network::new(place);
        // Every stop sign the world draws stops a lane, beside that lane's stop line,
        // unless it faces a road with no lanes toward its junction.
        for p in &place.props.items {
            if p.kind != PropKind::StopSign {
                continue;
            }
            let (sn, cs) = (p.yaw.sin(), p.yaw.cos());
            let facing = [-sn, -cs];
            let nobody_comes = net.junctions.iter().any(|j| {
                let rel = [p.pos[0] - j.pos.x, p.pos[2] - j.pos.y];
                rel[0].hypot(rel[1]) < 40.0
                    && j.ends.iter().any(|e| {
                        let edge = &place.roads.edges[e.edge];
                        let lanes_in = if edge.b as usize == j.node {
                            edge.lanes_fwd
                        } else {
                            edge.lanes_back
                        };
                        lanes_in == 0
                            && facing[0] * e.dir.x + facing[1] * e.dir.y > 0.9
                            && rel[0] * e.dir.x + rel[1] * e.dir.y > 0.0
                    })
            });
            if nobody_comes {
                continue;
            }
            let stops = g.stop_lines.iter().enumerate().any(|(k, s)| {
                dist(s.b, p.pos) < 8.0
                    && g.lanes
                        .iter()
                        .any(|l| l.stop_line == k as u32 && l.control == Control::Stop)
            });
            assert!(stops, "{}: stop sign at {:?} stops nobody", b.name, p.pos);
        }
        // Every stop-controlled lane has a stop sign by its stop line; every signalized
        // node has signal poles.
        for (id, l) in g.lanes.iter().enumerate() {
            match l.control {
                Control::Stop => {
                    let s = &g.stop_lines[l.stop_line as usize];
                    assert!(
                        sign_near(place, s.b, false, 8.0),
                        "{}: lane {id} stops with no sign",
                        b.name
                    );
                }
                Control::Signal => {
                    let n = place.roads.nodes[l.end_node as usize].pos;
                    assert!(
                        sign_near(place, [n[0], 0.0, n[1]], true, 50.0),
                        "{}: lane {id} has a light nobody drew",
                        b.name
                    );
                }
                _ => assert_eq!(l.stop_line, NONE),
            }
        }
    }
}

/// Every signed approach stops at the bar `world` paints: within 0.5 m but on a few bends,
/// never more than 1.25 m off.
#[test]
fn stop_lines_sit_on_the_painted_stop_bar() {
    let (mut checked, mut off, mut worst) = (0, 0, 0.0f32);
    for b in places() {
        let (place, g) = (&b.place, &b.graph);
        let net = Network::new(place);
        for s in &g.stop_lines {
            let e = &place.roads.edges[s.edge as usize];
            let which = usize::from(e.a != s.node);
            let (ji, slot) = net.edge_ends[s.edge as usize][which];
            let jn = &net.junctions[ji];
            if e.a == e.b || slot >= jn.ends.len() {
                continue;
            }
            let end = &jn.ends[slot];
            // `world` paints the stop bar 4.4..4.85 m past the trim, and stands the sign or
            // pole 1.0 or 1.2 m past it. Approaches with no drawn sign (alleys at a
            // signal) keep traffic's own setback.
            if !sign_near(place, s.b, false, 8.0) && !sign_near(place, s.b, true, 8.0) {
                continue;
            }
            // Nor is there a bar on a road too short for its markings.
            let (ea, eb) = net.ends(s.edge as usize);
            if net.paths[s.edge as usize].len() - ea.trim - eb.trim < 12.0 {
                continue;
            }
            // Arc length of the stop line's middle along the drawn road from the node.
            let path = &net.paths[s.edge as usize];
            let mid = [(s.a[0] + s.b[0]) * 0.5, (s.a[2] + s.b[2]) * 0.5];
            let (mut acc, mut best) = (0.0f32, (0.0f32, f32::INFINITY));
            for w in path.pts.windows(2) {
                let ab = [w[1].x - w[0].x, w[1].y - w[0].y];
                let ap = [mid[0] - w[0].x, mid[1] - w[0].y];
                let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / l2).clamp(0.0, 1.0);
                let d = ((ap[0] - ab[0] * t).powi(2) + (ap[1] - ab[1] * t).powi(2)).sqrt();
                if d < best.1 {
                    best = (acc + l2.sqrt() * t, d);
                }
                acc += l2.sqrt();
            }
            let along = if which == 0 { best.0 } else { acc - best.0 };
            let d = (along - (end.trim + 4.85)).abs();
            worst = worst.max(d);
            checked += 1;
            off += usize::from(d >= 0.5);
            assert!(d < 1.25, "{}: stop line {d} m off the painted bar", b.name);
        }
    }
    println!("stop lines on the paint: {checked}, {off} over 0.5 m off, worst {worst:.3} m");
    assert!(checked > 1000);
    // Where a road curves right past a long trim, the sign and the bar sit on the bend.
    assert!(off * 200 <= checked, "{off} stop lines more than 0.5 m off");
}

#[test]
fn poses_are_arc_length_along_the_lane() {
    let g = &coast().graph;
    for id in 0..g.lanes.len() as u32 {
        let pts = g.points(id);
        let l = &g.lanes[id as usize];
        let arc = &g.arc[l.first_point as usize..(l.first_point + l.point_count) as usize];
        for k in 0..pts.len() - 1 {
            for f in [0.0, 0.25, 0.5, 0.9] {
                let s = arc[k] + (arc[k + 1] - arc[k]) * f;
                let p = g.pose(id, s).pos;
                assert!(
                    (dist(p, pts[k]) - (s - arc[k])).abs() < 0.01,
                    "lane {id} at {s}"
                );
            }
        }
        assert!(dist(g.end_pose(id).pos, pts[pts.len() - 1]) < 1e-4);
        assert!((arc[arc.len() - 1] - l.length_m).abs() < 1e-4);
    }
}

#[test]
fn stop_distance_reaches_the_stop_line() {
    let g = &coast().graph;
    let id = g
        .lanes
        .iter()
        .position(|l| l.control == Control::Stop)
        .expect("a stop lane") as u32;
    let l = &g.lanes[id as usize];
    assert_eq!(g.stop_distance(id, 0.0), Some(l.length_m));
    let d = g.stop_distance(id, l.length_m - 2.0).expect("stop");
    assert!((d - 2.0).abs() < 1e-4);
    assert!(g.stop_distance(id, l.length_m + 1.0).expect("stop") < 0.0);
    // The lane's end lies on its stop line.
    let s = &g.stop_lines[l.stop_line as usize];
    let e = g.end_pose(id).pos;
    let (ab, ae) = (
        [s.b[0] - s.a[0], s.b[2] - s.a[2]],
        [e[0] - s.a[0], e[2] - s.a[2]],
    );
    let t = (ab[0] * ae[0] + ab[1] * ae[1]) / (ab[0] * ab[0] + ab[1] * ab[1]);
    let off = ((ae[0] - ab[0] * t).powi(2) + (ae[1] - ab[1] * t).powi(2)).sqrt();
    assert!((0.0..=1.0).contains(&t) && off < 0.01, "t {t}, off {off}");
    // A free lane has none.
    let free = g
        .lanes
        .iter()
        .position(|l| l.control == Control::Free)
        .expect("free") as u32;
    assert_eq!(g.stop_distance(free, 0.0), None);
}

#[test]
fn coast_graph_fits_its_budget() {
    let g = &coast().graph;
    println!(
        "coast seed 1: lanes {}, points {}, turns {}, junctions {}, {} KiB",
        g.lanes.len(),
        g.points.len(),
        g.turns.len(),
        g.junctions.len(),
        g.bytes() / 1024
    );
    assert!(g.lanes.len() <= MAX_LANES);
    assert!(g.turns.len() <= MAX_TURNS);
    assert!(g.bytes() <= MAX_BYTES);
}
