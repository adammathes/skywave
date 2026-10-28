//! The basin's train and its grade crossings with busy traffic and busy pedestrians
//! (`docs/NPCS.md`, "The train"): over a long seeded run the train passes every crossing,
//! nobody stands inside it for a single step, no NPC car is on a crossing whose arms are
//! down, nobody walks over one, and the train never has to stop dead. Run with
//! `--nocapture` for the counts: `cargo test -p traffic --release --test train -- --nocapture`.

use place::Place;
use skywave_core::Seed;
use traffic::peds::{PedInput, PedestrianBudget, Pedestrians};
use traffic::train::{Railway, HALF_W_M};
use traffic::walks::WalkGraph;
use traffic::{Clock, Density, Inputs, LaneGraph, Player, Traffic, TrafficBudget, View};
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

/// Counts over a run.
#[derive(Debug, Default)]
struct Counts {
    /// Person-steps inside a train body.
    people_in_train: u32,
    /// Car-steps of an NPC car overlapping a train body.
    cars_in_train: u32,
    /// Person-steps on the track at a crossing whose arms are down.
    people_on_closed: u32,
    /// Car-steps of an NPC car on the track at a crossing whose arms are down, the
    /// train within 200 m of it.
    cars_on_closed: u32,
    /// Person-steps and car-steps within 15 m of a train body: the probe saw people and
    /// cars close to it.
    people_near: u32,
    cars_near: u32,
    /// Crossings the train's head went past.
    passed: usize,
    hard_stops: u32,
}

/// `minutes` of busy traffic and people round crossing `k` of seed `seed`'s railway, the
/// train coming first after `rest_s`.
fn run(seed: u64, k_pick: usize, minutes: f32, rest_s: f32) -> Counts {
    let place = basin(seed);
    let graph = LaneGraph::new(&place);
    let walks = WalkGraph::new(&place, &graph);
    let mut rail = Railway::of(&place, rest_s).expect("a railway");
    let line = &rail.train.line;
    let k = k_pick.min(line.crossings.len() - 1);
    let (at, dir) = line.at(line.crossings[k]);
    // The camera and the player's car (standing still, off the road) 35 m along the
    // track from the crossing and 25 m off it, looking across the crossing.
    let side = [-dir[1], dir[0]];
    let pos = [
        at[0] + dir[0] * 35.0 + side[0] * 25.0,
        at[1],
        at[2] + dir[1] * 35.0 + side[1] * 25.0,
    ];
    let to = [at[0] - pos[0], at[2] - pos[2]];
    let n = (to[0] * to[0] + to[1] * to[1]).sqrt();
    let look = [to[0] / n, to[1] / n];
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
    let line_xz: Vec<[f32; 2]> = line.pts.iter().map(|p| [p[0], p[2]]).collect();
    let crossings_xz: Vec<[f32; 2]> = line
        .crossings
        .iter()
        .map(|&s| {
            let (p, _) = line.at(s);
            [p[0], p[2]]
        })
        .collect();
    let mut peds = Pedestrians::new(
        &walks,
        Seed(seed),
        place.header.era,
        PedestrianBudget::LARGE,
    );
    peds.set_rail(&walks, &line_xz, &crossings_xz);
    let mut cars = Traffic::new(&graph, TrafficBudget::default(), Seed(seed));
    let clock = Clock {
        seconds_of_day: 12.0 * 3600.0,
        dusk_s: 19.0 * 3600.0,
        dawn_s: 6.0 * 3600.0,
    };
    let mut c = Counts::default();
    let mut passed = vec![false; line.crossings.len()];
    let steps = (minutes * 60.0 * 120.0) as u64;
    for tick in 0..steps {
        cars.set_barriers(rail.barriers());
        peds.set_rail_closed(rail.closed());
        let inp = Inputs {
            tick,
            player,
            view,
            clock,
            density: Density::Busy,
        };
        cars.populate(&graph, &inp);
        cars.step(&graph, &inp);
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
        rail.step(&player, cars.cars(), &peds, DT);
        let t = &rail.train;
        for p in peds.iter() {
            if t.touches([p.pos[0], p.pos[2]], 0.3) {
                c.people_in_train += 1;
            }
            if t.touches([p.pos[0], p.pos[2]], 15.0) {
                c.people_near += 1;
            }
        }
        for car in cars.cars() {
            let k = (0.5 * car.len - car.half_w).max(0.0);
            for f in [-1.0f32, 0.0, 1.0] {
                let q = [
                    car.pos[0] + car.dir[0] * k * f,
                    car.pos[2] + car.dir[1] * k * f,
                ];
                if t.touches(q, car.half_w) {
                    c.cars_in_train += 1;
                    break;
                }
                if f == 0.0 && t.touches(q, 15.0) {
                    c.cars_near += 1;
                }
            }
        }
        // On the track at a crossing whose arms are down.
        for (j, &cs) in t.line.crossings.iter().enumerate() {
            if t.gates[j].arm > 0.0 {
                continue;
            }
            let on = |q: [f32; 2], r: f32| {
                let (s, off) = t.line.project(q);
                (s - cs).abs() < 20.0 && off < HALF_W_M + r + 0.5
            };
            c.people_on_closed += peds
                .iter()
                .filter(|p| on([p.pos[0], p.pos[2]], 0.3))
                .count() as u32;
            let (h, _) = t.line.at(if t.forward {
                t.head
            } else {
                t.line.len - t.head
            });
            let (x, _) = t.line.at(cs);
            let train_near = ((h[0] - x[0]).powi(2) + (h[2] - x[2]).powi(2)).sqrt() < 200.0;
            if train_near {
                c.cars_on_closed += cars
                    .cars()
                    .iter()
                    .filter(|car| on([car.pos[0], car.pos[2]], car.half_w))
                    .count() as u32;
            }
        }
        for (j, &cs) in t.line.crossings.iter().enumerate() {
            let cu = if t.forward { cs } else { t.line.len - cs };
            passed[j] |= t.head > cu + 5.0 && t.head > 0.0;
        }
    }
    c.passed = passed.iter().filter(|&&p| p).count();
    c.hard_stops = rail.train.hard_stops;
    c
}

#[test]
fn nobody_is_ever_inside_the_train_and_nobody_crosses_a_closed_crossing() {
    for (seed, k) in [(1u64, 3usize), (2, 3)] {
        let c = run(seed, k, 4.0, 5.0);
        println!("seed {seed} crossing {k}: {c:?}");
        assert_eq!(c.people_in_train, 0, "seed {seed}: {c:?}");
        assert_eq!(c.cars_in_train, 0, "seed {seed}: {c:?}");
        assert_eq!(c.people_on_closed, 0, "seed {seed}: {c:?}");
        assert_eq!(c.cars_on_closed, 0, "seed {seed}: {c:?}");
        assert_eq!(c.hard_stops, 0, "seed {seed}: {c:?}");
        assert!(c.passed >= 3, "seed {seed}: the train ran: {c:?}");
        assert!(
            c.people_near > 0 && c.cars_near > 0,
            "seed {seed}: nobody came near: {c:?}"
        );
    }
}

/// The long sweep: every crossing of seeds 1-4, each with the camera and the crowd at it.
#[test]
#[ignore]
fn every_crossing_of_four_seeds() {
    for seed in 1..=4u64 {
        let n = Railway::of(&basin(seed), 5.0)
            .expect("a railway")
            .train
            .line
            .crossings
            .len();
        for k in 0..n {
            let c = run(seed, k, 4.0, 5.0);
            println!("seed {seed} crossing {k}: {c:?}");
            assert_eq!(c.people_in_train + c.cars_in_train, 0, "{c:?}");
            assert_eq!(
                c.people_on_closed + c.cars_on_closed + c.hard_stops,
                0,
                "{c:?}"
            );
        }
    }
}
