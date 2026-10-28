//! NPC cars and pedestrians together on the coast at seed 1.

use place::Place;
use skywave_core::Seed;
use traffic::cars::{CAR_HALF_W_M, CAR_LEN_M};
use traffic::peds::{OtherCar, PedInput, PedestrianBudget, Pedestrians};
use traffic::walks::{LinkKind, WalkGraph, CELL_M};
use traffic::{Car, Clock, Density, Inputs, LaneGraph, Player, Traffic, TrafficBudget, View};
use world::procgen::build_place;
use world::template::RegionTemplate;

const HZ: u64 = 120;
const BODY_GAP_M: f32 = 0.15;

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

fn view_at(eye: [f32; 3], tick: u64) -> View {
    let a = tick as f32 * 0.001;
    View {
        pos: eye,
        dir: [a.cos(), a.sin()],
        tan_half: (55.0f32).to_radians().tan(),
    }
}

/// How far car `c`'s body is from `p`, m (negative inside it).
fn gap(c: &Car, p: [f32; 3]) -> f32 {
    let d = [p[0] - c.pos[0], p[2] - c.pos[2]];
    let a = (d[0] * c.dir[0] + d[1] * c.dir[1]).abs() - CAR_LEN_M * 0.5;
    let l = (d[0] * -c.dir[1] + d[1] * c.dir[0]).abs() - CAR_HALF_W_M;
    if a > 0.0 && l > 0.0 {
        (a * a + l * l).sqrt()
    } else {
        a.max(l)
    }
}

#[test]
fn npc_cars_never_reach_anyone() {
    let b = build(1);
    let eye = if std::env::var("EYE_START").is_ok() {
        let st = b.place.player_start().expect("start").pos;
        b.walks
            .nodes
            .iter()
            .map(|n| n.pos)
            .min_by(|a, c| dist(*a, st).total_cmp(&dist(*c, st)))
            .unwrap()
    } else {
        busy_eye(&b.walks)
    };
    // The traffic's player: a point at the eye, so cars come and go round it.
    let me = Player {
        pos: eye,
        heading: 0.0,
        speed: 0.0,
        half_len: 0.01,
        half_w: 0.01,
    };
    let mut traffic = Traffic::new(&b.lanes, TrafficBudget::default(), Seed(1));
    if std::env::var("NO_SEE").is_err() {
        traffic.see_walks(&b.lanes, &b.walks);
    }
    let mut others: Vec<OtherCar> = Vec::with_capacity(128);
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
    let (mut inside, mut near, mut steps, mut close, mut within50) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for tick in 0..60 * HZ {
        let view = view_at(eye, tick);
        let inp = Inputs {
            tick,
            player: me,
            view,
            clock,
            density: Density::Busy,
        };
        traffic.see_people(peds.seen());
        traffic.step(&b.lanes, &inp);
        traffic.populate(&b.lanes, &inp);
        let pin = PedInput {
            tick,
            density: Density::Busy,
            view,
            player: Player::ABSENT,
        };
        others.clear();
        if std::env::var("NO_SEE").is_err() {
            others.extend(
                traffic
                    .cars()
                    .iter()
                    .map(|c| OtherCar::npc(c, 0.0, [CAR_LEN_M * 0.5, CAR_HALF_W_M])),
            );
        }
        peds.see_cars(&others, traffic.crossings_taken());
        peds.step(&b.walks, &b.lanes, &pin);
        for (i, p) in peds.pedestrians().iter().enumerate() {
            if !p.alive {
                continue;
            }
            for c in traffic.cars() {
                if dist(p.pos, c.pos) < 50.0 {
                    within50 += 1;
                }
                if dist(p.pos, c.pos) > 6.0 {
                    continue;
                }
                close += 1;
                let g = gap(c, p.pos);
                if g < 0.0 {
                    inside += 1;
                    if inside % 5 == 1 {
                        println!("t {tick} person {} on_crossing {} in_lane {} turn-band {} dodging {} | car {} seg {} turn {} speed {:.2} gap {g:.2}", p.id, peds.on_crossing(&b.walks, i), b.walks.in_lane(p.pos), b.walks.in_lane_or_turn(p.pos), peds.dodging(i), c.id, c.seg, c.seg >= b.lanes.lanes.len() as u32, c.speed);
                    }
                } else if g < BODY_GAP_M {
                    near += 1;
                }
            }
        }
        steps += 1;
    }
    println!("close {close} within50 {within50}");
    println!("{steps} steps: {inside} person-steps inside an NPC car, {near} within {BODY_GAP_M} m; {} people, {} cars", peds.count(), traffic.cars().len());
}
