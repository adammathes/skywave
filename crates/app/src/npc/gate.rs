//! The M4 gate (`docs/MILESTONES.md`): the zero-violence scenarios and the density bands,
//! played through the app on the coast at seed 1. The player's car is driven by input
//! through [`App::update`], so the traffic sees it exactly as in the game; pedestrians are
//! simulation-only in the app today, so the scenarios that need them step a
//! [`traffic::Pedestrians`] beside the app, fed the app's own player and camera.

use std::sync::OnceLock;

use place::Place;
use skywave_core::{Seed, SIM_DT};
use traffic::peds::{Action as PedAction, PedInput, PedestrianBudget, Pedestrians};
use traffic::walks::WalkGraph;
use traffic::{Control, Density, LaneGraph, Player};

use crate::{places, Action, App, InputState, PlaceKey, SceneSetup};

/// 60 mph, m/s.
const MPH_60: f32 = 26.82;

/// The coast at seed 1 with its lane and sidewalk graphs, built once for every test here.
struct Coast {
    place: Place,
    lanes: LaneGraph,
    walks: WalkGraph,
}

fn coast() -> &'static Coast {
    static COAST: OnceLock<Coast> = OnceLock::new();
    COAST.get_or_init(|| {
        let place = places::build(PlaceKey::Coast, Seed(1)).expect("coast");
        let lanes = LaneGraph::new(&place);
        let walks = WalkGraph::new(&place, &lanes);
        Coast {
            place,
            lanes,
            walks,
        }
    })
}

fn app(query: &str) -> App {
    App::from_setup(&SceneSetup::from_query(query).unwrap()).unwrap()
}

/// The coast scene with the car at ground point `pos` facing `heading` (the vehicle sim's
/// convention), plus `extra` keys.
fn app_at(pos: [f32; 3], heading: f32, extra: &str) -> App {
    let yaw = -heading.to_degrees();
    app(&format!(
        "place=coast&car=gannet_gt&cam=chase-mid&pos={},{}&yaw={yaw}&{extra}",
        pos[0], pos[2]
    ))
}

/// A footprint on the ground: center, unit forward, half length and half width.
#[derive(Clone, Copy, Debug)]
struct Rect {
    c: [f32; 2],
    f: [f32; 2],
    hl: f32,
    hw: f32,
}

impl Rect {
    fn player(p: &Player) -> Rect {
        Rect {
            c: [p.pos[0], p.pos[2]],
            f: forward(p.heading),
            hl: p.half_len,
            hw: p.half_w,
        }
    }

    /// NPC car `c`'s body as the player's car meets it: its shape's box.
    fn car(app: &App, c: &traffic::Car) -> Rect {
        let b = app.npcs.body(c);
        Rect {
            c: [c.pos[0] - c.dir[0] * b.back, c.pos[2] - c.dir[1] * b.back],
            f: c.dir,
            hl: b.half[0],
            hw: b.half[1],
        }
    }

    /// Whether ground point `p` is inside.
    fn contains(&self, p: [f32; 3]) -> bool {
        let d = [p[0] - self.c[0], p[2] - self.c[1]];
        dot(d, self.f).abs() <= self.hl && dot(d, right(self.f)).abs() <= self.hw
    }

    /// Whether the two footprints overlap (separating axes).
    fn overlaps(&self, o: &Rect) -> bool {
        let d = [o.c[0] - self.c[0], o.c[1] - self.c[1]];
        [self.f, right(self.f), o.f, right(o.f)].iter().all(|&n| {
            let ra = self.hl * dot(self.f, n).abs() + self.hw * dot(right(self.f), n).abs();
            let rb = o.hl * dot(o.f, n).abs() + o.hw * dot(right(o.f), n).abs();
            dot(d, n).abs() < ra + rb
        })
    }

    /// Distance between the two footprints' centers less both half lengths: a cheap
    /// lower bound on the gap (negative when they may touch).
    fn rough_gap(&self, o: &Rect) -> f32 {
        let d = [o.c[0] - self.c[0], o.c[1] - self.c[1]];
        dot(d, d).sqrt() - self.hl - o.hl
    }
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn right(d: [f32; 2]) -> [f32; 2] {
    [-d[1], d[0]]
}

/// Unit ground direction of a heading (0 faces -z, positive turns left).
fn forward(h: f32) -> [f32; 2] {
    [-h.sin(), -h.cos()]
}

fn wrap(a: f32) -> f32 {
    (a + core::f32::consts::PI).rem_euclid(core::f32::consts::TAU) - core::f32::consts::PI
}

/// The NPC cars whose footprint overlaps the player's now.
fn overlapping(app: &App) -> usize {
    let me = Rect::player(&app.traffic_player());
    app.npcs
        .cars()
        .iter()
        .filter(|c| me.overlaps(&Rect::car(app, c)))
        .count()
}

/// The pedal and steering input that holds `v` m/s (negative: reversing) along the
/// straight line through `at` in direction `dir`.
fn follow(app: &App, at: [f32; 3], dir: [f32; 2], v: f32) -> InputState {
    let p = app.traffic_player();
    let speed = app.drive.state.speed_mps;
    let mut input = InputState::default();
    if v == 0.0 {
        input.set(Action::Brake, 1.0);
        input.set(Action::Handbrake, 1.0);
        return input;
    }
    let ahead = if v > 0.0 { 1.0 } else { -1.0 };
    let err = (v - speed) * ahead;
    if err > 0.0 {
        let pedal = if v > 0.0 {
            Action::Throttle
        } else {
            Action::Brake
        };
        input.set(pedal, (err * 0.5 + 0.1).min(1.0));
    } else if err < -0.5 {
        let pedal = if v > 0.0 {
            Action::Brake
        } else {
            Action::Throttle
        };
        input.set(pedal, (-err * 0.3).min(1.0));
    }
    let lat = dot([p.pos[0] - at[0], p.pos[2] - at[2]], right(dir));
    let herr = wrap(p.heading - (-dir[0]).atan2(-dir[1]));
    // Reversing, the car points the same way and the steering acts mirrored.
    let steer = (1.5 * herr - 0.15 * lat) * ahead;
    input.set(Action::Steer, steer.clamp(-1.0, 1.0));
    input
}

/// One sim step of `app` with `input`.
fn step(app: &mut App, input: &InputState) {
    let steps = app.update(input, SIM_DT);
    assert_eq!(steps.count, 1);
}

/// The straight line along `lane`'s last 10 m (lanes may bend gently further back).
fn end_line(g: &LaneGraph, lane: u32) -> traffic::Pose {
    g.pose(lane, g.lanes[lane as usize].length_m - 10.0)
}

/// Along `lane`, how far the ground point `p` near its end is from its start: by
/// [`end_line`], extended straight past the end.
fn along(g: &LaneGraph, lane: u32, p: [f32; 3]) -> f32 {
    let a = end_line(g, lane);
    g.lanes[lane as usize].length_m - 10.0 + dot([p[0] - a.pos[0], p[2] - a.pos[2]], a.dir)
}

/// The end of the approach the junction scenario runs: a straight two-lane arterial
/// (14 m/s) into a signalized four-way junction on the coast's north side.
const APPROACH_END: [f32; 2] = [807.95, 1760.15];

/// The signalized lane whose end lies at `end`.
fn lane_ending_at(g: &LaneGraph, end: [f32; 2]) -> u32 {
    (0..g.lanes.len() as u32)
        .find(|&l| {
            let e = g.end_pose(l).pos;
            g.lanes[l as usize].control == Control::Signal
                && (e[0] - end[0]).hypot(e[2] - end[1]) < 2.0
        })
        .expect("the approach lane")
}

/// Drives `app` from a standstill along `lane` at full throttle up to 60 mph and on,
/// `ticks` steps, calling `each` after every step.
fn run_up(app: &mut App, lane: u32, ticks: u32, mut each: impl FnMut(&App)) {
    let g = &coast().lanes;
    let a = g.pose(lane, 0.0);
    for _ in 0..ticks {
        let input = follow(app, a.pos, a.dir, MPH_60);
        step(app, &input);
        each(app);
    }
}

/// The player takes a busy signalized junction at 60 mph against the light, some 4–9 s
/// into its red, while the cross traffic has its green. Every NPC car yields or is
/// missed: none ever overlaps the player's footprint, and a cross-traffic car at the head
/// of its queue holds on its green until the player has cleared the junction, then goes.
/// There is no damage to check: nothing in the game has a damage state.
#[test]
fn sixty_through_a_red_light_touches_no_car() {
    let g = &coast().lanes;
    let lane = lane_ending_at(g, APPROACH_END);
    let len = g.lanes[lane as usize].length_m;
    let start = g.pose(lane, 6.0);
    // How long the run-up takes to the stop line, alone on the road.
    let mut alone = app_at(start.pos, start.heading, "t=12:00&traffic=off");
    let half_len = alone.traffic_player().half_len;
    let mut arrive = None;
    let mut speed_at_line = 0.0;
    let mut tick = 0u32;
    run_up(&mut alone, lane, 12 * 120, |app| {
        tick += 1;
        let front = along(g, lane, app.traffic_player().pos) + half_len;
        if arrive.is_none() && front >= len {
            arrive = Some(tick);
            speed_at_line = app.drive.state.speed_mps;
        }
    });
    let arrive = arrive.expect("the car reaches the stop line");
    assert!(
        speed_at_line > MPH_60 - 1.0,
        "{speed_at_line} m/s at the line"
    );

    // Busy traffic, the player waiting at the lane's start for a moment to go: its own
    // lane clear ahead, and the line reached 4–9 s into the red.
    let mut app = app_at(start.pos, start.heading, "t=12:00&traffic=busy");
    let red = |t: u64| g.light(lane, t) == Some(traffic::Light::Red);
    let mut waited = 0;
    loop {
        let t = app.npcs.signals.tick + u64::from(arrive);
        let clear = app
            .npcs
            .cars()
            .iter()
            .all(|c| c.seg != lane && c.prev != lane);
        if clear && red(t) && red(t - 4 * 120) && !red(t - 9 * 120) {
            break;
        }
        let hold = follow(&app, start.pos, start.dir, 0.0);
        step(&mut app, &hold);
        waited += 1;
        assert!(waited < 240 * 120, "no moment to go in four minutes");
    }
    let mut overlaps = 0;
    let mut tick = 0u32;
    let mut crossed_on = None;
    // NPC cars that came within 15 m of the player while it was in the junction (with
    // their least and greatest speed then), and the least rough gap to any of them.
    let mut near = std::collections::BTreeMap::new();
    let mut least = f32::INFINITY;
    let mut first_overlap = None;
    let (node, phase) = (
        g.lanes[lane as usize].end_node,
        g.lanes[lane as usize].phase,
    );
    // Per cross-traffic car at its stop line on a green: steps it stood there while the
    // player came through, and whether it moved off once the player had cleared.
    let mut held = std::collections::BTreeMap::new();
    run_up(&mut app, lane, arrive + 10 * 120, |app| {
        tick += 1;
        let p = app.traffic_player();
        let me = Rect::player(&p);
        let s = along(g, lane, p.pos);
        if crossed_on.is_none() && s + half_len >= len {
            crossed_on = Some((app.npcs.signals.tick, app.drive.state.speed_mps));
        }
        // Cross traffic held at its line on a green while the player comes through,
        // until the player has cleared the junction.
        let clear = s - half_len > len + 15.0;
        for c in app.npcs.cars() {
            let Some(cl) = g.lanes.get(c.seg as usize) else {
                continue;
            };
            let green = g.light(c.seg, app.npcs.signals.tick) == Some(traffic::Light::Green);
            if cl.end_node == node && cl.phase != phase && green && cl.length_m - c.s < 3.0 {
                let w = held.entry(c.id).or_insert((0u32, false));
                if !clear && c.speed < 0.5 {
                    w.0 += 1;
                } else if clear && c.speed > 1.0 {
                    w.1 = true;
                }
            }
        }
        for c in app.npcs.cars() {
            let r = Rect::car(app, c);
            if me.overlaps(&r) {
                overlaps += 1;
                first_overlap.get_or_insert((tick, c.id, c.speed, c.pos));
            }
            if s + half_len > len && s - half_len < len + 30.0 {
                let gap = me.rough_gap(&r);
                least = least.min(gap);
                if gap < 15.0 {
                    let v = near.entry(c.id).or_insert((c.speed, c.speed));
                    *v = (v.0.min(c.speed), v.1.max(c.speed));
                }
            }
        }
    });
    let (tick, speed) = crossed_on.expect("through the junction");
    // Braked hard or stood: slowed by over 2 m/s, or under 0.5 m/s.
    let braked = near
        .values()
        .filter(|&&(lo, hi)| lo < hi - 2.0 || lo < 0.5)
        .count();
    // Yielded: stood a second or more on its green for the player, then went.
    let yielded = held
        .values()
        .filter(|&&(stood, went)| stood >= 120 && went)
        .count();
    eprintln!(
        "60 mph through a red: waited {:.1} s, line at {speed:.1} m/s, {} cars near in the \
         junction ({braked} braked or stood), {yielded} held on a green at the line until the \
         player had passed ({held:?}), least rough gap {least:.1} m, {overlaps} \
         overlapping car-steps (first {first_overlap:?})",
        waited as f32 / 120.0,
        near.len()
    );
    assert!(red(tick), "the player ran the red");
    assert!(speed > MPH_60 - 1.0, "{speed} m/s");
    assert!(!near.is_empty(), "the junction had traffic");
    assert!(
        yielded > 0,
        "no cross traffic yielded to the player: {held:?}"
    );
    assert_eq!(overlaps, 0, "first: {first_overlap:?}");
    assert!(app.drive.state.upset_s == 0.0);
}

/// The end of a signalized approach downtown whose crosswalk people use the most (ten
/// people in two minutes at busy density with the player waiting at its stop line).
const DOWNTOWN_END: [f32; 2] = [695.43, 981.20];

/// A pedestrian sim beside the app (seeded like the place), fed the app's player and
/// camera at busy density.
fn people() -> Pedestrians {
    let c = coast();
    Pedestrians::new(
        &c.walks,
        Seed(1),
        c.place.header.era,
        PedestrianBudget::LARGE,
    )
}

fn ped_input(app: &App) -> PedInput {
    let inp = app.traffic_inputs();
    PedInput {
        tick: app.npcs.signals.tick,
        density: Density::Busy,
        view: inp.view,
        player: inp.player,
    }
}

/// What the people and cars did round the player over a run.
#[derive(Debug, Default)]
struct Tally {
    /// Person-steps inside the player's footprint, and the first (step, person id).
    inside: usize,
    first_inside: Option<(u32, u32)>,
    /// People who came within 5 m of the footprint, and who stepped aside or hopped.
    near: std::collections::BTreeSet<u32>,
    aside: std::collections::BTreeSet<u32>,
    hop: std::collections::BTreeSet<u32>,
    /// NPC car-steps overlapping the player's footprint, and the first (step, car id).
    cars: usize,
    first_car: Option<(u32, u32)>,
}

/// Runs `app` and `peds` together for `ticks` steps, the player driven by `control`.
fn run_with_people(
    app: &mut App,
    peds: &mut Pedestrians,
    ticks: u32,
    mut control: impl FnMut(&App) -> InputState,
) -> Tally {
    let c = coast();
    let mut t = Tally::default();
    for k in 0..ticks {
        let input = control(app);
        step(app, &input);
        peds.step(&c.walks, &c.lanes, &ped_input(app));
        let me = Rect::player(&app.traffic_player());
        for p in peds.iter() {
            assert!(p.pos.iter().all(|v| v.is_finite()), "person {}", p.id);
            if me.contains(p.pos) {
                t.inside += 1;
                t.first_inside.get_or_insert((k, p.id));
            }
            let d = [p.pos[0] - me.c[0], p.pos[2] - me.c[1]];
            let out = (dot(d, me.f).abs() - me.hl).max(dot(d, right(me.f)).abs() - me.hw);
            if out < 5.0 {
                t.near.insert(p.id);
            }
            if p.pose.weight > 0.0 {
                match p.pose.action {
                    PedAction::Aside => t.aside.insert(p.id),
                    PedAction::Hop => t.hop.insert(p.id),
                    _ => false,
                };
            }
        }
        for car in app.npcs.cars() {
            if me.overlaps(&Rect::car(app, car)) {
                t.cars += 1;
                t.first_car.get_or_insert((k, car.id));
            }
        }
    }
    t
}

/// Holds the player still for `secs` with people about, so the walks round it fill.
fn warm_people(app: &mut App, peds: &mut Pedestrians, secs: u32) {
    let p = app.traffic_player();
    let (at, dir) = (p.pos, forward(p.heading));
    let t = run_with_people(app, peds, secs * 120, |a| follow(a, at, dir, 0.0));
    assert_eq!(t.inside + t.cars, 0, "while warming up: {t:?}");
}

/// The player creeps from the stop line onto a downtown junction's crosswalk and stands
/// on it for a minute through the light's cycle: the people crossing go round the car, nobody
/// is ever inside it, and the NPC cars never overlap it.
#[test]
fn a_minute_stopped_on_a_crosswalk_with_people_about() {
    let c = coast();
    let lane = lane_ending_at(&c.lanes, DOWNTOWN_END);
    let l = &c.lanes.lanes[lane as usize];
    let x = c
        .walks
        .crossings
        .iter()
        .find(|x| x.node == l.end_node && x.edge == l.edge)
        .expect("the crosswalk past the stop line");
    let (a, b) = (
        c.walks.nodes[x.curbs[0] as usize].pos,
        c.walks.nodes[x.curbs[1] as usize].pos,
    );
    let mid = [(a[0] + b[0]) * 0.5, a[1], (a[2] + b[2]) * 0.5];
    let on = along(&c.lanes, lane, mid);
    let line = end_line(&c.lanes, lane);
    let wait = c.lanes.pose(lane, l.length_m - 4.0);
    let mut app = app_at(wait.pos, wait.heading, "t=12:00&traffic=busy");
    let mut peds = people();
    warm_people(&mut app, &mut peds, 40);
    let mut stopped = false;
    let mut on_it = 0u32;
    let t = run_with_people(&mut app, &mut peds, 90 * 120, |a| {
        let s = along(&c.lanes, lane, a.traffic_player().pos);
        stopped |= s >= on;
        if stopped && a.drive.state.speed_mps.abs() < 0.1 {
            on_it += 1;
        }
        follow(a, line.pos, line.dir, if stopped { 0.0 } else { 1.5 })
    });
    let s = along(&c.lanes, lane, app.traffic_player().pos);
    eprintln!(
        "a minute on a crosswalk: {:.1} s standing {:.1} m past its middle; {} person-steps \
         inside (first {:?}), {} people near, {} stepped aside, {} hopped; {} car-steps \
         overlapping (first {:?})",
        on_it as f32 / 120.0,
        s - on,
        t.inside,
        t.first_inside,
        t.near.len(),
        t.aside.len(),
        t.hop.len(),
        t.cars,
        t.first_car
    );
    assert!(on_it >= 60 * 120, "stood {on_it} steps");
    assert!(
        (s - on).abs() < 1.5,
        "{:.2} m off the crosswalk's middle",
        s - on
    );
    assert!(!t.near.is_empty(), "nobody came near");
    assert_eq!(t.inside, 0, "first {:?}", t.first_inside);
    assert_eq!(t.cars, 0, "first {:?}", t.first_car);
}

/// The player waits at the stop line of the busiest crosswalk downtown until people are
/// crossing on their walk phase and waiting at the corner, then noses the car at 1.5 m/s
/// into the corner they are crossing from, until the curb and the signal pole stop it:
/// people step aside or hop out of its way; nobody is ever inside the car. (The walks
/// themselves are too narrow for the car: walls, posts and parked cars keep it off.)
#[test]
fn driving_slowly_into_the_crowd_on_a_corner() {
    let c = coast();
    let w = &c.walks;
    let lane = lane_ending_at(&c.lanes, DOWNTOWN_END);
    let l = &c.lanes.lanes[lane as usize];
    let line = end_line(&c.lanes, lane);
    let wait = c.lanes.pose(lane, l.length_m - 4.0);
    let x = w
        .crossings
        .iter()
        .position(|x| x.node == l.end_node && x.edge == l.edge)
        .expect("the crosswalk past the stop line") as u32;
    let r = right(line.dir);
    let corner = w.crossings[x as usize]
        .curbs
        .iter()
        .map(|&n| w.nodes[n as usize].pos)
        .max_by(|a, b| {
            let side = |q: [f32; 3]| dot([q[0] - line.pos[0], q[2] - line.pos[2]], r);
            side(*a).total_cmp(&side(*b))
        })
        .unwrap();
    let mut app = app_at(wait.pos, wait.heading, "t=12:00&traffic=busy");
    let mut peds = people();
    warm_people(&mut app, &mut peds, 20);
    // People on the crosswalk or about to step onto it.
    let crowd = |peds: &Pedestrians| {
        (0..peds.pedestrians().len())
            .filter(|&i| peds.pedestrians()[i].alive && peds.crossing_of(w, i) == Some(x))
            .count()
    };
    let mut waited = 0;
    while crowd(&peds) < 4 {
        warm_people(&mut app, &mut peds, 1);
        waited += 1;
        assert!(waited < 180, "nobody crossing in three minutes");
    }
    let at_start = crowd(&peds);
    let p = app.traffic_player().pos;
    let to = [corner[0] - p[0], corner[2] - p[2]];
    let dir = [to[0] / dot(to, to).sqrt(), to[1] / dot(to, to).sqrt()];
    let t = run_with_people(&mut app, &mut peds, 15 * 120, |a| follow(a, p, dir, 1.5));
    let q = app.traffic_player().pos;
    let left = (corner[0] - q[0]).hypot(corner[2] - q[2]);
    eprintln!(
        "1.5 m/s into the crowd on a corner: waited {waited} s, {at_start} people crossing, \
         stopped {left:.1} m from the corner; {} person-steps inside (first {:?}), {} people \
         near, {} stepped aside, {} hopped; {} car-steps overlapping (first {:?})",
        t.inside,
        t.first_inside,
        t.near.len(),
        t.aside.len(),
        t.hop.len(),
        t.cars,
        t.first_car
    );
    assert!(left < 4.0, "the car stopped {left:.1} m from the corner");
    assert!(t.near.len() >= 3, "{} people came near", t.near.len());
    assert!(
        !t.aside.is_empty() || !t.hop.is_empty(),
        "nobody stepped aside or hopped"
    );
    assert_eq!(t.inside, 0, "first {:?}", t.first_inside);
}

/// The player waits first at the arterial junction's red with a queue formed behind it,
/// then backs up into the queue at a walking pace: it bumps the first car of the queue
/// and stops against it. No NPC car's body ever overlaps the player's footprint, the car
/// it hit is shoved less than a meter and stands on its brakes, its brake lamps lit.
#[test]
fn reversing_into_a_queue_stops_against_it() {
    let c = coast();
    let lane = lane_ending_at(&c.lanes, APPROACH_END);
    let line = end_line(&c.lanes, lane);
    let wait = c
        .lanes
        .pose(lane, c.lanes.lanes[lane as usize].length_m - 4.0);
    let mut app = app_at(wait.pos, wait.heading, "t=12:00&traffic=busy");
    let me = app.traffic_player();
    let mine = along(&c.lanes, lane, me.pos);
    // Wait for a queue: a car stopped on the lane within 10 m behind, the light red with
    // 10 s of it left.
    let mut waited = 0;
    let behind = loop {
        let tick = app.npcs.signals.tick;
        let red = |t: u64| c.lanes.light(lane, t) == Some(traffic::Light::Red);
        let queued = app.npcs.cars().iter().find(|car| {
            let s = along(&c.lanes, lane, car.pos);
            car.seg == lane && car.speed < 0.1 && s < mine && mine - s < 10.0
        });
        if let (Some(car), true) = (queued, red(tick) && red(tick + 10 * 120)) {
            break (car.id, car.pos, mine - along(&c.lanes, lane, car.pos));
        }
        let hold = follow(&app, line.pos, line.dir, 0.0);
        step(&mut app, &hold);
        waited += 1;
        assert!(waited < 180 * 120, "no queue in three minutes");
    };
    let (id, at, behind) = behind;
    let half = app.traffic_player().half_len;
    let mut cars = 0;
    let mut first = None;
    let mut backed = 0.0f32;
    let (mut hits, mut held, mut lit, mut shoved) = (0, false, false, 0.0f32);
    for k in 0..8 * 120 {
        let input = follow(
            &app,
            line.pos,
            line.dir,
            if k < 5 * 120 { -1.5 } else { 0.0 },
        );
        step(&mut app, &input);
        backed = backed.max(mine - along(&c.lanes, lane, app.traffic_player().pos));
        hits += usize::from(app.drive.impact.hit.speed > 0.0);
        if let Some(car) = app.npcs.cars().iter().find(|car| car.id == id) {
            held |= car.hold > 0;
            lit |= car.hold > 0 && car.braking;
            shoved = shoved.max((car.pos[0] - at[0]).hypot(car.pos[2] - at[2]));
        }
        let n = overlapping(&app);
        cars += n;
        if n > 0 {
            first.get_or_insert(k);
        }
    }
    let speed = app.drive.state.speed_mps;
    eprintln!(
        "reversing into a queue: waited {:.1} s, the car behind {behind:.1} m back (centers), \
         backed {backed:.2} m, {hits} steps hitting, the car shoved {shoved:.2} m, \
         {cars} car-steps overlapping (first at step {first:?}), {speed:.2} m/s after",
        waited as f32 / 120.0
    );
    // It reached the queue and stopped there: short of where the cars' centers would meet
    // less both cars' half lengths, never through it.
    assert!(hits > 0, "never touched the queue");
    assert!(
        backed > behind - 2.0 * half - 1.5 && backed < behind - 2.0 * half + 0.5,
        "backed {backed:.2} m, the queue {behind:.1} m back"
    );
    assert!(
        held && lit,
        "the car hit did not brake ({held}, lamps {lit})"
    );
    assert!(shoved < 1.0, "the car hit moved {shoved:.2} m");
    assert!(speed.abs() < 0.3, "{speed} m/s");
    assert_eq!(cars, 0, "first at step {first:?}");
}

/// Ten broadside rams at 8 m/s, half a second apart, into the NPC car nearest the busiest
/// block of the strip, pushing it toward the curb, with the people of that block about:
/// the car's body never leaves the roadway (every corner on asphalt, level with the road
/// at its lane's line), and nobody standing on a sidewalk is ever inside it.
#[test]
fn rams_never_push_an_npc_onto_the_sidewalk() {
    // The busiest block of the strip at seed 1 (`peds::tests::STRIP`).
    let mut app =
        app("place=coast&car=gannet_gt&cam=chase-mid&pos=872.0,942.5&yaw=1&t=12:00&traffic=busy");
    let me = app.traffic_player();
    let hold = InputState::default();
    // A car standing in the outermost lane (curb side) nearest a point 15 m up the road.
    let ahead = forward(me.heading);
    let target = [me.pos[0] + ahead[0] * 15.0, me.pos[2] + ahead[1] * 15.0];
    let g = &app.npcs.graph;
    let mut best = (f32::INFINITY, 0u32, 0.0f32);
    for (l, lane) in g.lanes.iter().enumerate() {
        let mut s = 8.0;
        while s < lane.length_m - 8.0 {
            let p = g.pose(l as u32, s).pos;
            let d = (p[0] - target[0]).hypot(p[2] - target[1]);
            if d < best.0 {
                best = (d, l as u32, s);
            }
            s += 1.0;
        }
    }
    let near = &g.lanes[best.1 as usize];
    let lane = (0..g.lanes.len() as u32)
        .filter(|&l| {
            let o = &g.lanes[l as usize];
            o.edge == near.edge && o.forward == near.forward
        })
        .max_by_key(|&l| g.lanes[l as usize].slot)
        .unwrap();
    let graph = app.npcs.graph.clone();
    let id = app
        .npcs
        .traffic
        .spawn_at(&graph, lane, best.2, 0.0)
        .expect("room in the pool");
    let spec = app.drive.spec.clone();
    let (mut rams, mut off_road, mut inside, mut widest) = (0, 0, 0, 0.0f32);
    let mut room = [0.0f32; 3];
    let mut seen = 0;
    for k in 0..15 * 120 {
        let Some(i) = app.npcs.cars().iter().position(|c| c.id == id) else {
            // Gone once it drove on, out of view far away.
            assert!(rams == 10, "the car left");
            break;
        };
        seen = k;
        let c = app.npcs.cars()[i];
        let b = app.npcs.body(&c);
        if k < 10 * 60 && k % 60 == 0 {
            // The player's car square on to the car's left side, nose a hair into it,
            // closing at 8 m/s toward its right.
            let r = [-c.dir[1], c.dir[0]];
            let mid = [c.pos[0] - c.dir[0] * b.back, c.pos[2] - c.dir[1] * b.back];
            let back = b.half[1] + 0.5 * spec.dims_m[0] - 0.03;
            let at = [mid[0] - r[0] * back, c.pos[1], mid[1] - r[1] * back];
            let mut ram = vehicle::VehicleState::spawn(&spec, at, (-r[0]).atan2(-r[1]));
            ram.vel = [r[0] * 8.0, 0.0, r[1] * 8.0];
            if app.npcs.collide(&mut ram, &spec, &app.ground).is_some() {
                rams += 1;
            }
            room = app.npcs.cars()[i].room;
        }
        step(&mut app, &hold);
        let Some(c) = app.npcs.cars().iter().find(|c| c.id == id).copied() else {
            continue;
        };
        widest = widest.max((c.shove[0]).hypot(c.shove[1]));
        let b = app.npcs.body(&c);
        off_road += usize::from(!body_on_road(&app, &c, &b));
        let rect = Rect::car(&app, &c);
        for p in app.npcs.crowd.people() {
            let (_, surface) = app.ground.level_from([p.pos[0], p.pos[2]], p.pos[1] + 0.5);
            if surface != world::collide::Surface::Asphalt && rect.contains(p.pos) {
                inside += 1;
            }
        }
    }
    eprintln!(
        "ten rams toward the curb: {rams} hit, room {room:?}, shoved at most {widest:.2} m, \
         {off_road} steps off the roadway, {inside} person-steps on a sidewalk inside it, \
         watched {:.1} s",
        seen as f32 / 120.0
    );
    assert_eq!(rams, 10);
    assert_eq!(off_road, 0);
    assert_eq!(inside, 0);
}

/// A car waiting at a stop sign on a lane that can only turn is nosed into from the front
/// ten times at 8 m/s, then let go: it is pushed back at most 1.5 m along its path and
/// 0.5 m to the side, and as it turns (what was behind it swinging out) its body is never
/// over a curb anywhere its unshoved body is not.
#[test]
fn a_car_pushed_back_from_a_stop_line_turns_without_mounting_the_curb() {
    let c = coast();
    let g = &c.lanes;
    // Stop lanes whose every way out is a turn (none straight on) onto a lane with room for
    // the car, longest first.
    let mut lanes: Vec<u32> = (0..g.lanes.len() as u32)
        .filter(|&l| {
            let lane = &g.lanes[l as usize];
            lane.control == Control::Stop
                && lane.length_m > 50.0
                && !g.next(l).is_empty()
                && g.next(l).iter().all(|t| {
                    t.kind != traffic::TurnKind::Straight && g.lanes[t.to as usize].length_m > 30.0
                })
        })
        .collect();
    lanes.sort_by(|a, b| {
        g.lanes[*b as usize]
            .length_m
            .total_cmp(&g.lanes[*a as usize].length_m)
    });
    let lane = *lanes.first().expect("a stop lane that must turn");
    let len = g.lanes[lane as usize].length_m;
    let wait = g.pose(lane, len - 30.0);
    let mut app = app_at(wait.pos, wait.heading, "t=12:00&traffic=busy");
    let graph = app.npcs.graph.clone();
    let id = app
        .npcs
        .traffic
        .spawn_at(&graph, lane, len - 1.0, 0.0)
        .expect("room in the pool");
    let spec = app.drive.spec.clone();
    let hold = follow(&app, wait.pos, wait.dir, 0.0);
    let (mut rams, mut curb, mut widest, mut back) = (0, 0, 0.0f32, 0.0f32);
    let mut turned = false;
    for k in 0..25 * 120 {
        let Some(i) = app.npcs.cars().iter().position(|c| c.id == id) else {
            break;
        };
        let car = app.npcs.cars()[i];
        if k < 10 * 60 && k % 60 == 0 {
            // Nose to nose, closing at 8 m/s.
            let b = app.npcs.body(&car);
            let mid = [
                car.pos[0] - car.dir[0] * b.back,
                car.pos[2] - car.dir[1] * b.back,
            ];
            let ahead = b.half[0] + 0.5 * spec.dims_m[0] - 0.03;
            let at = [
                mid[0] + car.dir[0] * ahead,
                car.pos[1],
                mid[1] + car.dir[1] * ahead,
            ];
            let mut ram =
                vehicle::VehicleState::spawn(&spec, at, car.heading + core::f32::consts::PI);
            ram.vel = [-car.dir[0] * 8.0, 0.0, -car.dir[1] * 8.0];
            if app.npcs.collide(&mut ram, &spec, &app.ground).is_some() {
                rams += 1;
            }
        }
        step(&mut app, &hold);
        let Some(car) = app.npcs.cars().iter().find(|c| c.id == id).copied() else {
            break;
        };
        turned |= car.seg >= g.lanes.len() as u32;
        if car.shove != [0.0; 2] || car.yaw != 0.0 {
            let (sn, cs) = car.yaw.sin_cos();
            let d = [
                car.dir[0] * cs - car.dir[1] * sn,
                car.dir[1] * cs + car.dir[0] * sn,
            ];
            back = back.max(-dot(car.shove, d));
            widest = widest.max(dot(car.shove, right(d)).abs());
            let b = app.npcs.body(&car);
            let unshoved = traffic::Car {
                pos: [
                    car.pos[0] - car.shove[0],
                    car.pos[1],
                    car.pos[2] - car.shove[1],
                ],
                dir: d,
                shove: [0.0; 2],
                yaw: 0.0,
                ..car
            };
            if !body_on_road(&app, &car, &b) && body_on_road(&app, &unshoved, &b) {
                curb += 1;
            }
        }
    }
    eprintln!(
        "pushed back from a stop line: lane {lane}, {rams} rams, back at most {back:.2} m, \
         {widest:.2} m to the side, turned {turned}, {curb} steps over a curb"
    );
    assert_eq!(rams, 10);
    assert!(turned, "never turned");
    assert!(back <= traffic::cars::SHOVE_ALONG_M + 1e-3, "{back} m back");
    assert!(
        widest <= traffic::cars::SHOVE_SIDE_M + 1e-3,
        "{widest} m aside"
    );
    assert_eq!(curb, 0);
}

/// On the coast at seed 1, every 5 m of every lane 15 m clear of its ends, the largest
/// traffic body standing on its lane: wherever it stands on the roadway, it still does
/// shoved as far as the measured room allows to either side and turned either way as far
/// as allowed (`npc::road_room`, checked here by an independent test of its corners).
#[test]
fn the_measured_room_keeps_every_shove_on_the_roadway() {
    let app = app("place=coast&car=gannet_gt&cam=chase-mid&t=12:00&traffic=busy");
    let g = &app.npcs.graph;
    let template = app.npcs.cars()[0];
    let shape = (0..app.npcs.budget_shapes())
        .max_by(|&a, &b| {
            let len = |k: u8| {
                app.npcs
                    .body(&traffic::Car {
                        shape: k,
                        ..template
                    })
                    .half[0]
            };
            len(a).total_cmp(&len(b))
        })
        .unwrap();
    let (mut samples, mut road, mut full, mut none, mut left) = (0, 0, 0, 0, Vec::new());
    for id in 0..g.lanes.len() as u32 {
        let len = g.lanes[id as usize].length_m;
        let mut s = 15.0;
        while s < len - 15.0 {
            let p = g.pose(id, s);
            s += 5.0;
            samples += 1;
            let car = traffic::Car {
                pos: p.pos,
                heading: p.heading,
                dir: p.dir,
                shape,
                shove: [0.0; 2],
                yaw: 0.0,
                ..template
            };
            let b = app.npcs.body(&car);
            if !body_on_road(&app, &car, &b) {
                continue;
            }
            road += 1;
            let room = super::road_room(&app.ground, &car, &b).unwrap_or([0.0; 3]);
            full += usize::from(room == traffic::cars::FREE_ROOM);
            none += usize::from(room[0] == 0.0 && room[1] == 0.0);
            let side = right(p.dir);
            for (k, sign) in [(0, 1.0), (1, -1.0)] {
                for turn in [-room[2], room[2]] {
                    let (sn, cs) = turn.sin_cos();
                    let off = sign * room[k];
                    let shoved = traffic::Car {
                        pos: [p.pos[0] + side[0] * off, p.pos[1], p.pos[2] + side[1] * off],
                        heading: p.heading + turn,
                        dir: [p.dir[0] * cs + p.dir[1] * sn, p.dir[1] * cs - p.dir[0] * sn],
                        shove: [side[0] * off, side[1] * off],
                        yaw: turn,
                        ..car
                    };
                    if !body_on_road(&app, &shoved, &b) {
                        left.push((id, s - 5.0, room));
                    }
                }
            }
        }
    }
    eprintln!(
        "measured room on the coast: {road} of {samples} lane points on the roadway, {full} \
         with the full room, {none} with none to either side; {} shoved poses off it: {:?}",
        left.len(),
        &left[..left.len().min(6)]
    );
    assert!(road * 10 > samples * 9, "{road} of {samples}");
    assert!(left.is_empty());
}

/// Whether NPC car `c`'s body (`b`) stands on the roadway: every corner on asphalt and
/// level (within 8 cm) with the road at the same point along its path.
fn body_on_road(app: &App, c: &traffic::Car, b: &super::CarBody) -> bool {
    let (sn, cs) = c.yaw.sin_cos();
    let d = [c.dir[0] * cs - c.dir[1] * sn, c.dir[1] * cs + c.dir[0] * sn];
    let line = [c.pos[0] - c.shove[0], c.pos[2] - c.shove[1]];
    let rect = Rect::car(app, c);
    let y = c.pos[1] + 0.5;
    [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
        .iter()
        .all(|&(u, v)| {
            let r = right(rect.f);
            let q = [
                rect.c[0] + rect.f[0] * b.half[0] * u + r[0] * b.half[1] * v,
                rect.c[1] + rect.f[1] * b.half[0] * u + r[1] * b.half[1] * v,
            ];
            let (yq, surface) = app.ground.level_from(q, y);
            let along = dot([q[0] - line[0], q[1] - line[1]], d);
            let (yl, _) = app
                .ground
                .level_from([line[0] + d[0] * along, line[1] + d[1] * along], y);
            surface == world::collide::Surface::Asphalt && (yq - yl).abs() < 0.08
        })
}

/// A tiny seeded generator for the bumping drive's choices.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A minute of driving at busy traffic round a coast junction, every half second steering
/// at the nearest NPC car with a random pedal (forward or reverse): many bumps, never a
/// non-finite number, never a launch, and no hit leaves the player's car faster than it
/// was going the step before by more than the rebound.
#[test]
fn a_minute_of_random_bumping_stays_finite_and_grounded() {
    let mut app = app(&format!("{}&t=12:00&traffic=busy", super::tests::JUNCTION));
    let mut rng = Lcg(7);
    let (mut throttle, mut reverse) = (0.5f32, false);
    let y0 = app.drive.state.pos[1];
    let (mut hits, mut new_hits, mut in_contact) = (0, 0, false);
    let (mut max_up, mut max_rise, mut worst) = (0.0f32, 0.0f32, f32::NEG_INFINITY);
    for k in 0..60 * 120 {
        if k % 60 == 0 {
            throttle = 0.2 + 0.6 * rng.next();
            reverse = rng.next() < 0.25;
        }
        let p = app.traffic_player();
        let target = app
            .npcs
            .cars()
            .iter()
            .map(|car| ([car.pos[0] - p.pos[0], car.pos[2] - p.pos[2]], car))
            .filter(|(d, _)| dot(*d, *d) < 40.0 * 40.0)
            .min_by(|a, b| dot(a.0, a.0).total_cmp(&dot(b.0, b.0)))
            .map(|(d, _)| d);
        let mut input = InputState::default();
        if let Some(d) = target {
            let want = (-d[0]).atan2(-d[1]);
            let err = wrap(want - p.heading);
            let steer = if reverse { -err } else { err };
            input.set(Action::Steer, (1.5 * steer).clamp(-1.0, 1.0));
        }
        let pedal = if reverse {
            Action::Brake
        } else {
            Action::Throttle
        };
        input.set(pedal, throttle);
        let before = app.drive.state.vel;
        let hspeed = |v: [f32; 3]| v[0].hypot(v[2]);
        step(&mut app, &input);
        let s = &app.drive.state;
        let all = s.pos.iter().chain(&s.rot).chain(&s.vel).chain(&s.ang_vel);
        assert!(all.clone().all(|v| v.is_finite()), "non-finite at step {k}");
        for car in app.npcs.cars() {
            assert!(car.pos.iter().all(|v| v.is_finite()) && car.heading.is_finite());
        }
        max_up = max_up.max(s.vel[1]);
        max_rise = max_rise.max(s.pos[1] - y0);
        let impact = app.drive.impact;
        if impact.hit.speed > 0.0 {
            hits += 1;
            new_hits += usize::from(!in_contact);
            worst = worst.max(hspeed(s.vel) - hspeed(before));
        }
        in_contact = impact.hit.speed > 0.0;
    }
    eprintln!(
        "random bumping: {hits} steps hitting ({new_hits} hits), worst speed gained in a hit \
         {worst:.2} m/s, up at most {max_up:.2} m/s, rose at most {max_rise:.2} m"
    );
    assert!(new_hits >= 5, "{new_hits} hits");
    assert!(worst <= 1.0, "{worst} m/s gained in a hit");
    // Upward only: a downhill run is not a bounce. Well over what is measured (0.49 m/s,
    // 0.18 m).
    assert!(max_up < 1.6 && max_rise < 0.4, "{max_up} m/s, {max_rise} m");
}

/// The car-count band each of Low, Normal and Busy holds at every step of 120 s, of the
/// 96-car pool's targets 24, 48 and 96: never above the target, and never below 3/8, 5/8
/// and 3/4 of it, so no two settings' bands overlap. (Measured, seed 1: Low dips to 10 at
/// the start, 14 downtown; Normal to 33; Busy to 78. Cars despawn hidden and far, and
/// respawn only 150-300 m off, out of view and on lanes flowing toward the player.)
const BANDS: [(usize, usize); 3] = [(9, 24), (30, 48), (72, 96)];

/// Live NPC cars sampled every step over `secs` with the player held still: least, most.
fn hold_and_count(app: &mut App, secs: u32) -> (usize, usize) {
    let p = app.traffic_player();
    let (at, dir) = (p.pos, forward(p.heading));
    let (mut lo, mut hi) = (usize::MAX, 0);
    for _ in 0..secs * 120 {
        let input = follow(app, at, dir, 0.0);
        step(app, &input);
        let n = app.traffic_count();
        lo = lo.min(n);
        hi = hi.max(n);
    }
    (lo, hi)
}

/// Every density holds its band of live cars over 120 s (after the warm-up), and Off
/// drops every car and idles the sim: the traffic is never stepped (the game times every
/// traffic step it runs; Off times none) and its state does not change.
fn densities_hold_their_bands(name: &str, query: &str, bands: [(usize, usize); 3]) {
    let mut app = app(&format!("{query}&t=12:00&traffic=off"));
    for (d, (lo, hi)) in [Density::Low, Density::Normal, Density::Busy]
        .into_iter()
        .zip(bands)
    {
        app.apply_traffic(Density::Off);
        app.apply_traffic(d);
        app.finish_warm_up();
        let (least, most) = hold_and_count(&mut app, 120);
        eprintln!("{name}: {d:?} {least}..={most} cars over 120 s (band {lo}..={hi})");
        assert!(least >= lo && most <= hi, "{name} {d:?}: {least}..={most}");
    }
    app.apply_traffic(Density::Off);
    assert_eq!(app.traffic_count(), 0);
    assert!(!app.npcs.active());
    let hash = app.npcs.traffic.hash();
    let timed = std::cell::Cell::new(0u32);
    let input = follow(&app, app.traffic_player().pos, [0.0, -1.0], 0.0);
    for _ in 0..120 * 120 {
        app.advance(&input, SIM_DT, &|| {
            timed.set(timed.get() + 1);
            0.0
        });
        assert_eq!(app.traffic_count(), 0);
    }
    assert_eq!(timed.get(), 0, "Off stepped the traffic");
    assert_eq!(app.npcs.traffic.hash(), hash);
    assert_eq!(app.npcs.step_ms(), 0.0);
}

/// The start on the strip, where the game begins.
#[test]
fn densities_hold_their_bands_at_the_start() {
    densities_hold_their_bands(
        "the start",
        "place=coast&car=gannet_gt&cam=chase-mid",
        BANDS,
    );
}

/// The busy downtown junction of the crosswalk scenarios.
#[test]
fn densities_hold_their_bands_downtown() {
    let g = &coast().lanes;
    let wait = end_line(g, lane_ending_at(g, DOWNTOWN_END));
    let yaw = -wait.heading.to_degrees();
    densities_hold_their_bands(
        "downtown",
        &format!(
            "place=coast&car=gannet_gt&cam=chase-mid&pos={},{}&yaw={yaw}",
            wait.pos[0], wait.pos[2]
        ),
        BANDS,
    );
}

/// The arterial approach of the junction scenario, on the coast's north side.
#[test]
fn densities_hold_their_bands_on_the_arterial() {
    let g = &coast().lanes;
    let wait = end_line(g, lane_ending_at(g, APPROACH_END));
    let yaw = -wait.heading.to_degrees();
    densities_hold_their_bands(
        "the arterial",
        &format!(
            "place=coast&car=gannet_gt&cam=chase-mid&pos={},{}&yaw={yaw}",
            wait.pos[0], wait.pos[2]
        ),
        BANDS,
    );
}
