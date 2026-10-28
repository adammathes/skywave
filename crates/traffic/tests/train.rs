//! The basin's train and its grade crossings with busy traffic and busy pedestrians
//! (`docs/NPCS.md`, "The train"): over a long seeded run the train passes every crossing,
//! nobody stands inside it for a single step, no NPC car is on a crossing whose arms are
//! down, nobody walks over one, and the train never has to stop dead. Run with
//! `--nocapture` for the counts: `cargo test -p traffic --release --test train -- --nocapture`.

use place::Place;
use skywave_core::Seed;
use traffic::peds::{PedInput, PedestrianBudget, Pedestrians};
use traffic::train::{Obstacle, Railway, HALF_W_M};
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
/// How far beyond the train's side people keep while a crossing is not fully open, m.
const PERSON_CLEAR_M: f32 = 0.8;

/// Counts over a run.
#[derive(Debug, Default)]
struct Counts {
    /// Person-steps inside a train body.
    people_in_train: u32,
    /// Car-steps of an NPC car overlapping a train body.
    cars_in_train: u32,
    /// Person-steps within [`PERSON_CLEAR_M`] of the train's swept width, within 60 m of a
    /// crossing that is closed or whose arms are not yet all the way up.
    people_on_closed: u32,
    /// Car-steps of an NPC car on the track at a crossing whose arms are down, the
    /// train within 200 m of it.
    cars_on_closed: u32,
    /// Person-steps and car-steps within 15 m of a train body: the probe saw people and
    /// cars close to it.
    people_near: u32,
    cars_near: u32,
    /// Person-steps within 40 m and car-steps within 80 m of the crossing the camera is at,
    /// while it is closed: the holds were tested there.
    people_at_closed: u32,
    cars_at_closed: u32,
    /// Whether a walk crosses the track at the crossing (else nobody can be there).
    walkable: bool,
    /// Person-steps inside an NPC car's body.
    people_in_cars: u32,
    /// Of those, within 60 m of the crossing.
    people_in_cars_at: u32,
    /// Crossings the train's head went past.
    passed: usize,
    hard_stops: u32,
}

/// `minutes` of busy traffic and people round crossing `k` of seed `seed`'s railway, the
/// train coming first after `rest_s`.
fn run(seed: u64, k_pick: usize, minutes: f32, rest_s: f32) -> Counts {
    run_with(seed, k_pick, minutes, rest_s, true, true)
}

/// [`run`], with the train or without it (the world as it was before the train, for
/// comparison), sending cars to the closed crossing or not.
fn run_with(
    seed: u64,
    k_pick: usize,
    minutes: f32,
    rest_s: f32,
    train: bool,
    send: bool,
) -> Counts {
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
    // The lanes into the crossing, and where on each a car starts 45 m short of it: while
    // the crossing is closed the probe sends cars there, so every crossing is tested with
    // cars waiting at it however quiet its road.
    let mut approaches = Vec::new();
    for (i, lane) in graph.lanes.iter().enumerate() {
        let k0 = lane.first_point as usize;
        let pts = &graph.points[k0..k0 + lane.point_count as usize];
        let arc = &graph.arc[k0..k0 + lane.point_count as usize];
        for j in 0..pts.len().saturating_sub(1) {
            let (a, b) = (pts[j], pts[j + 1]);
            let d = [b[0] - a[0], b[2] - a[2]];
            let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-6);
            let t = (((at[0] - a[0]) * d[0] + (at[2] - a[2]) * d[1]) / l2).clamp(0.0, 1.0);
            let q = [a[0] + d[0] * t - at[0], a[2] + d[1] * t - at[2]];
            let s = arc[j] + t * (arc[j + 1] - arc[j]);
            if q[0] * q[0] + q[1] * q[1] < 8.0 * 8.0 && s >= 45.0 {
                approaches.push((i as u32, s - 45.0));
                break;
            }
        }
    }
    // The walks over the track within 60 m of the crossing.
    let cs = line.crossings[k];
    let over: Vec<u32> = (0..walks.links.len() as u32)
        .filter(|&i| {
            let l = &walks.links[i as usize];
            let (a, b) = (walks.nodes[l.a as usize].pos, walks.nodes[l.b as usize].pos);
            let (_, oa) = line.project_signed([a[0], a[2]]);
            let (_, ob) = line.project_signed([b[0], b[2]]);
            // Over the track, or ending on its middle.
            if oa * ob > 0.0 || oa == ob {
                return false;
            }
            let t = oa / (oa - ob);
            let x = [a[0] + (b[0] - a[0]) * t, a[2] + (b[2] - a[2]) * t];
            let (sx, ox) = line.project(x);
            ox < 1.0 && (sx - cs).abs() < 60.0
        })
        .collect();
    let walkable = !over.is_empty();
    // While the crossing is closed the probe sends people at it too: every 5 s someone
    // starts 25 m short of a walk over the track, headed 15 m past it.
    let mut senders = Vec::new();
    for &l in &over {
        let lk = &walks.links[l as usize];
        for (from, to) in [(lk.a, lk.b), (lk.b, lk.a)] {
            let (p, q) = (walks.nodes[from as usize].pos, walks.nodes[to as usize].pos);
            let d = [q[0] - p[0], q[2] - p[2]];
            let n = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-3);
            let goal = [q[0] + d[0] / n * 15.0, q[2] + d[1] / n * 15.0];
            let Some(&l0) = walks.links_at(from).iter().find(|&&m| m != l) else {
                continue;
            };
            let m = &walks.links[l0 as usize];
            // Walking toward `from`: forward when that is its `b` end.
            let fwd = m.b == from;
            senders.push((l0, (m.length_m - 25.0).max(0.0), fwd, goal));
        }
    }
    let mut want = vec![false; approaches.len()];
    let mut c = Counts {
        walkable,
        ..Counts::default()
    };
    let mut passed = vec![false; line.crossings.len()];
    let steps = (minutes * 60.0 * 120.0) as u64;
    for tick in 0..steps {
        cars.set_barriers(rail.barriers());
        peds.set_rail_closed(rail.held());
        let inp = Inputs {
            tick,
            player,
            view,
            clock,
            density: Density::Busy,
        };
        // Every 5 s a car wanted at each approach; it takes the first free slot in the pool
        // (busy traffic keeps it full), ahead of the ordinary top-up.
        if send && rail.closed()[k] {
            for (j, &(lane, s)) in approaches.iter().enumerate() {
                want[j] |= tick % 600 == 0;
                let p = graph.pose(lane, s).pos;
                let free = cars
                    .cars()
                    .iter()
                    .all(|q| (q.pos[0] - p[0]).powi(2) + (q.pos[2] - p[2]).powi(2) > 12.0 * 12.0);
                if want[j] && free && cars.spawn_at(&graph, lane, s, 8.0).is_some() {
                    want[j] = false;
                }
            }
        }
        cars.populate(&graph, &inp);
        if send && rail.closed()[k] && tick % 600 == 300 {
            for &(l, s, fwd, goal) in &senders {
                peds.send(&walks, l, s, fwd, goal);
            }
        }
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
        if train {
            rail.step(&player, cars.cars(), &peds, DT);
        }
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
            let q = [car.pos[0], car.pos[2]];
            let body = Obstacle::car(q, car.dir, 0.5 * car.len, car.half_w);
            for p in peds.iter() {
                if body.overlaps([p.pos[0], p.pos[2]], [1.0, 0.0], [0.3, 0.3], 0.0) {
                    c.people_in_cars += 1;
                    let d = [p.pos[0] - at[0], p.pos[2] - at[2]];
                    c.people_in_cars_at += u32::from(d[0] * d[0] + d[1] * d[1] < 60.0 * 60.0);
                }
            }
            if t.overlaps(&Obstacle::car(q, car.dir, 0.5 * car.len, car.half_w), 0.0) {
                c.cars_in_train += 1;
            }
            if t.touches(q, 15.0) {
                c.cars_near += 1;
            }
        }
        if t.gates[k].closed {
            let within = |p: [f32; 3], r: f32| {
                let d = [p[0] - at[0], p[2] - at[2]];
                d[0] * d[0] + d[1] * d[1] < r * r
            };
            c.people_at_closed += peds.iter().filter(|p| within(p.pos, 40.0)).count() as u32;
            c.cars_at_closed += cars.cars().iter().filter(|q| within(q.pos, 80.0)).count() as u32;
        }
        // People on the track near a crossing from the moment it closes until its arms are
        // all the way up again, anywhere along the track within 60 m of it.
        for (j, &cs) in t.line.crossings.iter().enumerate() {
            if !rail.closed()[j] {
                continue;
            }
            let on = |q: [f32; 2]| {
                let (s, off) = t.line.project(q);
                (s - cs).abs() < 60.0 && off < HALF_W_M + PERSON_CLEAR_M
            };
            c.people_on_closed += peds.iter().filter(|p| on([p.pos[0], p.pos[2]])).count() as u32;
        }
        // Cars on the track at a crossing whose arms are down, the train near.
        for (j, &cs) in t.line.crossings.iter().enumerate() {
            if t.gates[j].arm > 0.0 {
                continue;
            }
            let on = |q: [f32; 2], r: f32| {
                let (s, off) = t.line.project(q);
                (s - cs).abs() < 20.0 && off < HALF_W_M + r + 0.5
            };
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
    // Long enough for the train to warn, run the whole line and clear its last crossing.
    for (seed, k) in [(1u64, 3usize), (2, 3)] {
        let c = run(seed, k, 3.1, 0.0);
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
        busy_while_closed(&c);
    }
}

/// The crossing was tested busy: cars waited at it while it was closed, and people too
/// where a walk crosses it (a person-step or car-step is 1/120 s).
fn busy_while_closed(c: &Counts) {
    assert!(c.cars_at_closed >= 120 * 20, "no cars at it: {c:?}");
    if c.walkable {
        assert!(c.people_at_closed >= 120 * 20, "no people at it: {c:?}");
    }
}

/// The long sweep: every crossing of seeds 1-4, each with the camera and the crowd at it.
/// People inside NPC cars (they do not see people yet, `docs/NPCS.md`) round a crossing,
/// with and without the train: never more with it.
#[test]
#[ignore]
fn people_in_cars_with_and_without_the_train() {
    for (seed, k) in [(2u64, 0usize), (1, 3), (2, 3), (3, 1)] {
        let without = run_with(seed, k, 3.1, 0.0, false, false);
        let with = run_with(seed, k, 3.1, 0.0, true, false);
        println!("seed {seed} crossing {k}: without {without:?}\n    with {with:?}");
        assert!(with.people_in_cars_at <= without.people_in_cars_at);
    }
}

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
            busy_while_closed(&c);
        }
    }
}
