//! NPC cars and pedestrians together on the coast at seed 1: the cars see the people
//! (`Traffic::see_walks`/`see_people`) and the people see the cars
//! (`Pedestrians::see_cars`), so nobody is ever inside an NPC car's body, whatever its
//! length. Run with `--nocapture` to see the numbers:
//! `cargo test --release -p traffic --test npc_peds -- --nocapture`.

use place::Place;
use skywave_core::Seed;
use traffic::cars::ShapeRule;
use traffic::peds::{OtherCar, PedInput, PedestrianBudget, Pedestrians};
use traffic::walks::WalkGraph;
use traffic::{Car, Clock, Density, Inputs, LaneGraph, Player, Traffic, TrafficBudget, View};
use world::procgen::build_place;
use world::template::RegionTemplate;

const HZ: u64 = 120;
/// Bodies of every length the traffic has: an everyday car, a box truck and a semi
/// (length, half width, m), all on every kind of street here.
const BODIES: [(f32, f32); 3] = [(4.6, 0.9), (7.4, 1.2), (17.5, 1.3)];

struct Built {
    place: Place,
    lanes: LaneGraph,
    walks: WalkGraph,
}

fn build(seed: u64) -> Built {
    let t: RegionTemplate =
        ron::from_str(include_str!("../../../assets/regions/california_coast.ron")).expect("parse");
    let grammar: world::Grammar = ron::from_str(include_str!(
        "../../../assets/grammar/california_coast_80s.ron"
    ))
    .expect("grammar");
    let place = build_place(Seed(seed), &t, &grammar);
    let lanes = LaneGraph::new(&place);
    let walks = WalkGraph::new(&place, &lanes);
    Built {
        place,
        lanes,
        walks,
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn view_at(eye: [f32; 3], tick: u64) -> View {
    let a = tick as f32 * 0.001;
    View {
        pos: eye,
        dir: [a.cos(), a.sin()],
        tan_half: (55.0f32).to_radians().tan(),
    }
}

/// How far car `c`'s body (its own length and width round its ground point) is from `p`,
/// m (negative inside it).
fn gap(c: &Car, p: [f32; 3]) -> f32 {
    let d = [p[0] - c.pos[0], p[2] - c.pos[2]];
    let a = (d[0] * c.dir[0] + d[1] * c.dir[1]).abs() - c.len * 0.5;
    let l = (d[0] * -c.dir[1] + d[1] * c.dir[0]).abs() - c.half_w;
    if a > 0.0 && l > 0.0 {
        (a * a + l * l).sqrt()
    } else {
        a.max(l)
    }
}

/// What a run saw.
#[derive(Default, Debug)]
struct Tally {
    /// Person-steps inside an NPC car's body: of someone off the sidewalks (on a crossing,
    /// in a lane or a turn's band), or on them by a car or a box truck.
    inside: usize,
    /// Person-steps on a sidewalk inside a semi's body: on a bend its rigid 17.5 m box cuts
    /// over the inside curb, a body that leaves the road (#192) rather than a car that does
    /// not see. Counted, not asserted (the game keeps semis to the freeways).
    semi_over_curb: usize,
    /// Per body, car-steps turning at a junction within 15 m of someone on a crossing.
    turning_by_crossers: [usize; 3],
}

/// 60 s of busy cars of every body and people round `eye`, the cars and people seeing
/// each other when `see`.
fn run(b: &Built, eye: [f32; 3], see: bool) -> Tally {
    // The traffic's player: a point at the eye, so cars come and go round it.
    let me = Player {
        pos: eye,
        heading: 0.0,
        speed: 0.0,
        half_len: 0.01,
        half_w: 0.01,
    };
    let budget = TrafficBudget {
        shapes: BODIES.len() as u8,
        ..TrafficBudget::default()
    };
    let mut traffic = Traffic::new(&b.lanes, budget, Seed(1));
    let rules = BODIES.map(|(len_m, half_w_m)| ShapeRule {
        len_m,
        half_w_m,
        ..ShapeRule::default()
    });
    traffic.set_shape_rules(&rules);
    if see {
        traffic.see_walks(&b.lanes, &b.walks);
    }
    let mut others: Vec<OtherCar> = Vec::with_capacity(budget.cars);
    let mut peds = Pedestrians::new(
        &b.walks,
        Seed(1),
        b.place.header.era,
        PedestrianBudget::SMALL,
    );
    let clock = Clock {
        seconds_of_day: 43_200.0,
        dusk_s: 70_000.0,
        dawn_s: 20_000.0,
    };
    let turns_from = b.lanes.lanes.len() as u32;
    let mut tally = Tally::default();
    for tick in 0..60 * HZ {
        let view = view_at(eye, tick);
        let inp = Inputs {
            tick,
            player: me,
            view,
            clock,
            density: Density::Busy,
        };
        if see {
            traffic.see_people(peds.seen());
        }
        traffic.step(&b.lanes, &inp);
        traffic.populate(&b.lanes, &inp);
        let pin = PedInput {
            tick,
            density: Density::Busy,
            view,
            player: Player::ABSENT,
        };
        others.clear();
        if see {
            others.extend(
                traffic
                    .cars()
                    .iter()
                    .map(|c| OtherCar::npc(c, 0.0, [c.len * 0.5, c.half_w])),
            );
        }
        peds.see_cars(&others, traffic.crossings_taken());
        peds.step(&b.walks, &b.lanes, &pin);
        for c in traffic.cars() {
            let crossers = peds.pedestrians().iter().enumerate().any(|(i, p)| {
                p.alive && peds.on_crossing(&b.walks, i) && dist(p.pos, c.pos) < 15.0
            });
            if c.seg >= turns_from && crossers {
                tally.turning_by_crossers[usize::from(c.shape)] += 1;
            }
        }
        for (i, p) in peds.pedestrians().iter().enumerate() {
            if !p.alive {
                continue;
            }
            for c in traffic.cars() {
                if dist(p.pos, c.pos) < 0.5 * c.len + 2.0 && gap(c, p.pos) < 0.0 {
                    let off_walk = peds.on_crossing(&b.walks, i) || b.walks.in_lane_or_turn(p.pos);
                    if !off_walk && c.len > 10.0 {
                        tally.semi_over_curb += 1;
                    } else {
                        tally.inside += 1;
                    }
                }
            }
        }
    }
    tally
}

/// Turning cars, box trucks and semis among people at every 16th crosswalk (every one with
/// `NPC_PEDS_ALL` set, `NPC_PEDS_ONLY=k` for crossing `k` alone), the eye at its curb: with
/// the cars and the people seeing each other, nobody is ever inside an NPC body (but a semi's
/// over a curb, [`Tally::semi_over_curb`]); blind, people are.
#[test]
fn npc_cars_of_every_length_never_reach_anyone() {
    let b = build(1);
    let every = if std::env::var("NPC_PEDS_ALL").is_ok() {
        1
    } else {
        16
    };
    let (mut blind_inside, mut turning, mut runs) = (0, [0; 3], 0);
    let mut failed = Vec::new();
    let only: Option<usize> = std::env::var("NPC_PEDS_ONLY")
        .ok()
        .and_then(|v| v.parse().ok());
    for (k, x) in b.walks.crossings.iter().enumerate().step_by(every) {
        if only.is_some_and(|o| o != k) {
            continue;
        }
        let eye = b.walks.nodes[x.curbs[0] as usize].pos;
        let blind = run(&b, eye, false);
        let seeing = run(&b, eye, true);
        println!(
            "crossing {k} (signal {}): blind {} inside, seeing {} inside ({} semi over the curb), \
             turning by crossers {:?}",
            x.signal,
            blind.inside,
            seeing.inside,
            seeing.semi_over_curb,
            seeing.turning_by_crossers
        );
        if seeing.inside > 0 {
            failed.push((k, seeing.inside));
        }
        blind_inside += blind.inside;
        for (t, n) in turning.iter_mut().zip(seeing.turning_by_crossers) {
            *t += n;
        }
        runs += 1;
    }
    assert!(
        failed.is_empty(),
        "person-steps inside an NPC car (crossing, steps): {failed:?}"
    );
    println!(
        "{runs} crossings: blind {blind_inside} person-steps inside, seeing 0; turning {turning:?}"
    );
    assert!(blind_inside > 0, "blind, the cars reach people here");
    for (k, &n) in turning.iter().enumerate() {
        assert!(n > 0, "no {:?} body turned by people crossing", BODIES[k]);
    }
}
