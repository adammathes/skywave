//! The basin's set pieces that drive (`docs/NPCS.md`, "The racers and the convoy") among
//! busy traffic and busy pedestrians: over a long seeded run the racers run their course
//! again and again and the convoy comes in at dawn, and nobody stands inside a racer or a
//! truck for a single step. Run with `--nocapture` for the counts:
//! `cargo test -p traffic --release --test race -- --nocapture`.

use place::Place;
use skywave_core::Seed;
use traffic::peds::{PedInput, PedestrianBudget, Pedestrians};
use traffic::race::{Drivers, RacePhase, CONVOY_TAG, RACE_MPS, RACE_TAG};
use traffic::walks::WalkGraph;
use traffic::{
    Car, Clock, Density, Inputs, LaneGraph, Player, ShapeRule, Traffic, TrafficBudget, View,
};
use world::procgen::build_place;
use world::template::RegionTemplate;

const BASIN: &str = include_str!("../../../assets/regions/socal_basin.ron");
const GRAMMAR: &str = include_str!("../../../assets/grammar/socal_basin_2001.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon_2001.ron");

fn basin(seed: u64) -> Place {
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    t.validate().expect("validate");
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("grammar");
    g.lexicon = ron::from_str(LEXICON).expect("lexicon");
    build_place(Seed(seed), &t, &g)
}

const DT: f32 = 1.0 / 120.0;
/// Shapes: eight everyday cars, a semi (freeway lanes only) and a hero car (never picked by
/// the traffic, only put down as a racer).
const TRUCK: u8 = 8;
const HERO: u8 = 9;

fn traffic(graph: &LaneGraph, seed: u64) -> Traffic {
    let mut t = Traffic::new(graph, TrafficBudget::default(), Seed(seed));
    let mut zones = [0; traffic::cars::ZONES];
    zones[place::ZoneId::Highway as usize] = 3;
    let mut rules = [ShapeRule::default(); 10];
    rules[TRUCK as usize] = ShapeRule {
        zones,
        len_m: 17.5,
        half_w_m: 1.3,
    };
    t.set_shape_rules(&rules);
    t.set_shape_weights(&[1, 1, 1, 1, 1, 1, 1, 1, 1, 0]);
    t
}

/// Counts over a run.
#[derive(Debug, Default)]
struct Counts {
    /// Person-steps inside a racer's or a truck's body.
    people_in_racers: u32,
    people_in_trucks: u32,
    /// Person-steps within 10 m of a racer running its course, or of a truck: the probe saw
    /// people close to them.
    people_near_racers: u32,
    people_near_trucks: u32,
    /// Runs started, the racers' top speed, and convoys that came.
    runs: u32,
    top_mps: f32,
    convoys: u32,
    /// Trucks that came in.
    trucks: usize,
    /// Steps a racer was let go at the line with someone on the course.
    went_busy: u32,
}

/// Whether the person at `p` stands inside car `c`'s body (grown by their 0.3 m).
fn inside(c: &Car, p: [f32; 2]) -> bool {
    let rel = [p[0] - c.pos[0], p[1] - c.pos[2]];
    let along = rel[0] * c.dir[0] + rel[1] * c.dir[1];
    let across = rel[0] * -c.dir[1] + rel[1] * c.dir[0];
    along.abs() < 0.5 * c.len + 0.3 && across.abs() < c.half_w + 0.3
}

/// `minutes` of busy traffic and people with the player's car stopped on the race start's
/// line (`on_line`) or at the convoy's way in (else), from `hour`.
fn run(seed: u64, minutes: f32, on_line: bool, hour: f32) -> Counts {
    let place = basin(seed);
    let graph = LaneGraph::new(&place);
    let walks = WalkGraph::new(&place, &graph);
    let mut cars = traffic(&graph, seed);
    let (mut drivers, routes) = Drivers::of(&place, &graph, &[HERO], Some(TRUCK));
    cars.set_routes(routes);
    cars.set_crosswalks(walks.crosswalks());
    let (pos, look) = if on_line {
        let c = &drivers.race.as_ref().expect("a race start").course;
        let at = graph.pose(c.lanes[1], c.line_s[1] - 3.0).pos;
        (at, c.dir)
    } else {
        let c = drivers.convoy.as_ref().expect("a convoy");
        let p = graph.pose(c.lane, 0.0);
        // Beside the freeway 500 m in, looking back toward its way in.
        let q = graph.pose(c.lane, 500.0);
        let side = [-q.dir[1], q.dir[0]];
        (
            [
                q.pos[0] + side[0] * 30.0,
                q.pos[1],
                q.pos[2] + side[1] * 30.0,
            ],
            {
                let d = [p.pos[0] - q.pos[0], p.pos[2] - q.pos[2]];
                let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
                [d[0] / n, d[1] / n]
            },
        )
    };
    let player = Player {
        pos,
        heading: (-look[0]).atan2(-look[1]),
        speed: 0.0,
        half_len: 2.2,
        half_w: 0.9,
    };
    let view = View {
        pos,
        dir: look,
        tan_half: (55.0f32).to_radians().tan(),
    };
    let mut peds = Pedestrians::new(
        &walks,
        Seed(seed),
        place.header.era,
        PedestrianBudget::LARGE,
    );
    let mut people: Vec<[f32; 2]> = Vec::with_capacity(1024);
    let mut c = Counts::default();
    let mut trucks: Vec<u32> = Vec::new();
    let steps = (minutes * 60.0 * 120.0) as u64;
    for tick in 0..steps {
        let clock = Clock {
            seconds_of_day: hour * 3600.0 + tick as f32 * DT,
            dusk_s: 19.0 * 3600.0,
            dawn_s: 6.0 * 3600.0,
        };
        people.clear();
        people.extend(peds.iter().map(|p| [p.pos[0], p.pos[2]]));
        cars.set_people(people.iter().copied());
        let inp = Inputs {
            tick,
            player,
            view,
            clock,
            density: Density::Busy,
        };
        cars.populate(&graph, &inp);
        cars.step(&graph, &inp);
        let before = drivers.race.as_ref().map(|r| r.phase);
        drivers.step(&mut cars, &graph, &player, &view, &clock, &people, DT);
        if let Some(r) = &drivers.race {
            if before == Some(RacePhase::Countdown) && r.phase == RacePhase::Running {
                c.went_busy += u32::from(r.course_busy(&people));
            }
        }
        peds.step(
            &walks,
            &graph,
            &PedInput {
                tick,
                density: Density::Busy,
                view,
                player,
            },
        );
        let running = drivers
            .race
            .as_ref()
            .is_some_and(|r| r.phase == RacePhase::Running);
        for car in cars.cars() {
            let (racer, truck) = (car.script.tag == RACE_TAG, car.script.tag == CONVOY_TAG);
            if !racer && !truck {
                continue;
            }
            if truck && !trucks.contains(&car.id) {
                trucks.push(car.id);
            }
            if racer && running {
                c.top_mps = c.top_mps.max(car.speed);
            }
            for p in peds.iter() {
                let q = [p.pos[0], p.pos[2]];
                let near = (q[0] - car.pos[0]).powi(2) + (q[1] - car.pos[2]).powi(2) < 100.0;
                if racer && inside(car, q) && c.people_in_racers % 40 == 0 {
                    let pi = peds
                        .pedestrians()
                        .iter()
                        .position(|x| x.id == p.id)
                        .unwrap();
                    let nl = graph.lanes.len() as u32;
                    let lane_end = if car.seg < nl {
                        graph.lanes[car.seg as usize].length_m
                    } else {
                        -1.0
                    };
                    eprintln!("INSIDE t {:.1} car {} seg {} s {:.1}/{:.0} v {:.2} route {} stand {} | ped {} at {:.1},{:.1} oncross {} dodge {} carpos {:.1},{:.1} dir {:?}", tick as f32 * DT, car.id, car.seg, car.s, lane_end, car.speed, car.script.route, car.script.stand_seg, p.id, q[0], q[1], peds.on_crossing(&walks, pi), peds.dodging(pi), car.pos[0], car.pos[2], car.dir);
                }
                if racer {
                    c.people_in_racers += u32::from(inside(car, q));
                    c.people_near_racers += u32::from(near && running);
                } else {
                    c.people_in_trucks += u32::from(inside(car, q));
                    c.people_near_trucks += u32::from(near);
                }
            }
        }
    }
    c.runs = drivers.race.as_ref().map_or(0, |r| r.runs);
    c.convoys = drivers.convoy.as_ref().map_or(0, |v| v.runs);
    c.trucks = trucks.len();
    c
}

#[test]
fn racers_run_the_boulevard_and_nobody_is_ever_inside_one() {
    for seed in [1u64, 2] {
        let c = run(seed, 6.0, true, 21.0);
        println!("seed {seed}: {c:?}");
        assert_eq!(c.people_in_racers, 0, "seed {seed}: {c:?}");
        assert_eq!(c.went_busy, 0, "seed {seed}: {c:?}");
        assert!(c.runs >= 2, "seed {seed}: the racers ran: {c:?}");
        assert!(c.top_mps > 0.8 * RACE_MPS, "seed {seed}: at speed: {c:?}");
        assert!(
            c.people_near_racers > 0,
            "seed {seed}: nobody came near: {c:?}"
        );
    }
}

#[test]
fn the_convoy_comes_in_at_dawn_and_nobody_is_ever_inside_a_truck() {
    for seed in [1u64, 2] {
        let c = run(seed, 3.0, false, 5.9);
        println!("seed {seed}: {c:?}");
        assert_eq!(c.convoys, 1, "seed {seed}: it came: {c:?}");
        assert_eq!(
            c.trucks,
            traffic::race::TRUCKS,
            "seed {seed}: all of it: {c:?}"
        );
        assert_eq!(c.people_in_trucks, 0, "seed {seed}: {c:?}");
    }
}

/// The long sweep: seeds 1-4, the racers for a quarter hour and the convoy's dawn.
#[test]
#[ignore]
fn every_seed_for_a_quarter_hour() {
    for seed in 1..=4u64 {
        let c = run(seed, 15.0, true, 21.0);
        println!("seed {seed} racers: {c:?}");
        if std::env::var("ONLY1").is_ok() {
            return;
        }
        assert_eq!(c.people_in_racers + c.went_busy, 0, "{c:?}");
        assert!(c.runs >= 3, "{c:?}");
        let c = run(seed, 6.0, false, 5.8);
        println!("seed {seed} convoy: {c:?}");
        assert_eq!(c.people_in_trucks, 0, "{c:?}");
        assert_eq!(c.convoys, 1, "{c:?}");
    }
}
