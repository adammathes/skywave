//! The lane graph of the procedural coast at seed 1: pinned hash, connectivity, continuous
//! turn curves and size budget. Run with `--nocapture` to see the numbers:
//! `cargo test -p traffic --test coast -- --nocapture`.

use std::sync::OnceLock;

use place::Place;
use skywave_core::Seed;
use traffic::lanes::CURVE_STEPS;
use traffic::{Control, LaneGraph, Light, TurnKind};
use world::procgen::build_place;
use world::template::RegionTemplate;

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");

/// Pinned [`LaneGraph::hash`] of the coast at seed 1. The graph is a pure function of the
/// place, so this moves when lane building or the place's roads change; update it in its
/// own commit with the reason.
const COAST_SEED1_HASH: u64 = 0xf095_ff61_cfc3_7eaa;

/// Size ceilings for the coast's graph (16×16 chunks); about twice what seed 1 needs
/// (1149 lanes, 2214 turns, 363 KiB).
const MAX_LANES: usize = 2_400;
const MAX_TURNS: usize = 4_500;
const MAX_BYTES: usize = 768 * 1024;

/// Turns tighter than this (other than U-turns) come from roads squeezed between two close
/// junctions and from acute corners; car following slows for them by `min_radius_m`, and
/// they must stay rare: under `MAX_SHARP_SHARE` of all turns (seed 1: 2.9%).
const SHARP_RADIUS_M: f32 = 3.0;
const MAX_SHARP_SHARE: f32 = 0.05;

fn coast() -> &'static (Place, LaneGraph) {
    static COAST: OnceLock<(Place, LaneGraph)> = OnceLock::new();
    COAST.get_or_init(|| {
        let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
        t.validate().expect("validate");
        let place = build_place(Seed(1), &t);
        let graph = LaneGraph::new(&place);
        (place, graph)
    })
}

#[test]
fn coast_graph_hash_is_pinned() {
    let (place, g) = coast();
    assert_eq!(LaneGraph::new(place), *g, "rebuilding gives the same graph");
    let h = g.hash();
    println!("coast seed 1 lane graph hash: {h:#018x}");
    assert_eq!(h, COAST_SEED1_HASH);
}

#[test]
fn every_lane_leads_somewhere_except_dead_ends() {
    let (_, g) = coast();
    let mut controls = [0usize; 4];
    let mut kinds = [0usize; 4];
    for (id, l) in g.lanes.iter().enumerate() {
        controls[l.control as usize] += 1;
        let degree = g.junctions[l.end_node as usize].degree;
        if l.control == Control::DeadEnd {
            assert_eq!(l.turn_count, 0);
            assert!(
                degree == 1,
                "lane {id} (edge {}) ends at a node of degree {degree} with no turn",
                l.edge
            );
        } else {
            assert!(l.turn_count > 0, "lane {id} has no turn");
        }
        assert!(l.length_m > 0.0, "lane {id} has no length");
        for t in g.next(id as u32) {
            kinds[t.kind as usize] += 1;
            assert_eq!(
                g.lanes[t.to as usize].start_node, l.end_node,
                "turn leaves from where the lane ends"
            );
        }
    }
    println!(
        "lanes {} (free {}, stop {}, signal {}, dead end {}); turns {} (straight {}, left {}, right {}, u {}); stop lines {}",
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
        g.stop_lines.len()
    );
    assert!(
        controls[Control::Signal as usize] > 0,
        "the coast has signals"
    );
    assert!(
        controls[Control::Stop as usize] > 0,
        "the coast has stop signs"
    );
}

#[test]
fn turn_curves_are_continuous() {
    let (_, g) = coast();
    let mut longest_step = 0.0f32;
    let mut sharp = 0usize;
    for (i, t) in g.turns.iter().enumerate() {
        let i = i as u32;
        let (a, b) = (g.end_pose(t.from), g.pose(t.to, 0.0));
        let (s, e) = (g.turn_pose(i, 0.0), g.turn_pose(i, t.length_m));
        // Positions meet the lanes at both ends; directions match there.
        for (p, q) in [(s.pos, a.pos), (e.pos, b.pos)] {
            let d = (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0, f32::max);
            assert!(d < 1e-3, "turn {i} misses its lane by {d}");
        }
        if t.length_m > 0.05 {
            for (p, q) in [(s.dir, a.dir), (e.dir, b.dir)] {
                let c = p[0] * q[0] + p[1] * q[1];
                assert!(c > 0.999, "turn {i} ({:?}) kinks at an end: {c}", t.kind);
            }
        }
        // No gaps or jumps along the curve.
        let n = 4 * CURVE_STEPS;
        let mut prev = s;
        for k in 1..=n {
            let p = g.turn_pose(i, t.length_m * k as f32 / n as f32);
            let step = ((p.pos[0] - prev.pos[0]).powi(2) + (p.pos[2] - prev.pos[2]).powi(2)).sqrt();
            longest_step = longest_step.max(step / (t.length_m / n as f32).max(1e-3));
            assert!(step <= 2.0 * t.length_m / n as f32 + 1e-3, "turn {i} jumps");
            prev = p;
        }
        if t.min_radius_m < SHARP_RADIUS_M && t.kind != TurnKind::UTurn {
            sharp += 1;
        }
    }
    let share = sharp as f32 / g.turns.len() as f32;
    println!(
        "sharp turns (r < {SHARP_RADIUS_M} m) {sharp} ({:.2}%)",
        share * 100.0
    );
    assert!(share < MAX_SHARP_SHARE, "too many sharp turns: {sharp}");
    println!("longest curve step / nominal: {longest_step:.3}");
}

#[test]
fn signals_alternate_on_the_coast() {
    let (_, g) = coast();
    let lane = g
        .lanes
        .iter()
        .position(|l| l.control == Control::Signal)
        .expect("a signal lane") as u32;
    let mut seen = [false; 3];
    for tick in (0..7200u64).step_by(60) {
        seen[g.light(lane, tick).expect("signal") as usize] = true;
    }
    assert_eq!(seen, [true; 3], "green, yellow and red in one cycle");
    assert_eq!(g.light(lane, 0), g.light(lane, 7200));
    assert!(matches!(
        g.light(lane, 0),
        Some(Light::Green | Light::Yellow | Light::Red)
    ));
}

#[test]
fn coast_graph_fits_its_budget() {
    let (_, g) = coast();
    println!(
        "lanes {}, points {}, turns {}, junctions {}, {} KiB",
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
