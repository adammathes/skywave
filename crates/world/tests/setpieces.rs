//! The basin's set pieces (`docs/WORLD_GENERATION.md`, "Set pieces") end to end: every one is
//! sited on seeds 1-4, clear of the roads and buildings around it, and its solids hold a
//! car as they should: the tunnel's walls keep a car on the freeway and let it drive
//! through, the garage's middle bay takes a car and its walls stop one.
//! `cargo test -p world --release --test setpieces -- --nocapture` prints where each went.

use place::{Place, PoiKind, PropKind};
use skywave_core::Seed;
use vehicle::{step, Assists, VehicleInput, VehicleSpec, VehicleState};
use world::landmarks::Kit;
use world::math::{point_in_polygon, segment_distance, V2};
use world::procgen::build_place;
use world::procgen::setpieces::{DRIVE_IN_VIEW_M, ROADSIDE, ROADSIDE_APART_M};
use world::template::RegionTemplate;
use world::{resolve, WorldCollider};

const GRAMMAR: &str = include_str!("../../../assets/grammar/socal_basin_2001.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon_2001.ron");
const BASIN: &str = include_str!("../../../assets/regions/socal_basin.ron");
const HALBERD: &str = include_str!("../../../assets/vehicles/halberd_coupe.ron");

fn grammar() -> world::Grammar {
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("parse grammar");
    g.lexicon = ron::from_str(LEXICON).expect("parse lexicon");
    g.validate().expect("validate grammar");
    g.landmarks = world::landmarks::BUILTIN
        .iter()
        .map(|(_, text)| ron::from_str(text).expect("parse landmark kit"))
        .collect();
    g
}

fn basin(seed: u64) -> Place {
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    t.validate().expect("validate");
    build_place(Seed(seed), &t, &grammar())
}

fn count(p: &Place, f: impl Fn(PropKind) -> bool) -> usize {
    p.props.items.iter().filter(|q| f(q.kind)).count()
}

fn pois(p: &Place, kind: PoiKind) -> usize {
    p.pois.iter().filter(|q| q.kind == kind).count()
}

fn dir_of_yaw(yaw: f32) -> V2 {
    world::props::dir_of_yaw(yaw)
}

fn at(p: &place::Prop) -> V2 {
    V2::new(p.pos[0], p.pos[2])
}

/// The tunnel's pieces in order along it, from the near portal.
fn tunnel(p: &Place) -> Vec<&place::Prop> {
    let mut pieces: Vec<&place::Prop> = p
        .props
        .items
        .iter()
        .filter(|q| matches!(q.kind, PropKind::TunnelPiece { .. }))
        .collect();
    let d = dir_of_yaw(pieces[0].yaw);
    pieces.sort_by(|a, b| at(a).dot(d).total_cmp(&at(b).dot(d)));
    pieces
}

/// The rectangle a garage or stand stands on: its front's middle, the way out of it, its
/// width and depth behind the front.
fn footprint(q: &place::Prop, w: f32, d: f32) -> Vec<V2> {
    let out = dir_of_yaw(q.yaw);
    let x = out.left();
    let c = at(q);
    vec![
        c + x * (0.5 * w),
        c - x * (0.5 * w),
        c - x * (0.5 * w) - out * d,
        c + x * (0.5 * w) - out * d,
    ]
}

#[test]
fn every_basin_has_its_set_pieces_clear_of_roads_and_buildings() {
    for seed in 1..=4u64 {
        let p = basin(seed);
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        for kind in [
            PoiKind::Tunnel,
            PoiKind::CrewGarage,
            PoiKind::TacoStand,
            PoiKind::DragStrip,
        ] {
            assert_eq!(pois(&p, kind), 1, "seed {seed}: {kind:?}");
        }
        assert!(pois(&p, PoiKind::Overlook) >= 1, "seed {seed}");
        assert!(pois(&p, PoiKind::ChannelRamp) >= 1, "seed {seed}");
        assert_eq!(count(&p, |k| matches!(k, PropKind::TunnelPiece { .. })), 8);
        assert_eq!(count(&p, |k| matches!(k, PropKind::HeroCar { .. })), 3);
        assert_eq!(count(&p, |k| k == PropKind::StartTree), 1);
        assert_eq!(count(&p, |k| k == PropKind::TimingBoard), 1);
        assert_eq!(count(&p, |k| matches!(k, PropKind::StripLine { .. })), 2);
        assert!(count(&p, |k| matches!(k, PropKind::GuardRail { .. })) >= 2);
        assert!(count(&p, |k| matches!(k, PropKind::ChainLink { .. })) > 100);

        // The garage and the stand: no road or building on their ground.
        let (gw, gd, _) = world::setpieces::GARAGE_M;
        let (sw, sd, _) = world::setpieces::STAND_M;
        for q in &p.props.items {
            let area = match q.kind {
                PropKind::CrewGarage { .. } => footprint(q, gw, gd),
                PropKind::TacoStand { .. } => footprint(q, sw, sd),
                _ => continue,
            };
            for e in &p.roads.edges {
                for s in e.profile.windows(2) {
                    let (a, b) = (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2]));
                    for c in &area {
                        let d = segment_distance(*c, a, b).0;
                        assert!(d > 0.5 * e.width_m, "seed {seed}: {:?} on a road", q.kind);
                    }
                }
            }
            for b in &p.buildings {
                for v in &b.footprint {
                    let v = V2::from_array(*v);
                    assert!(
                        !world::math::point_in_polygon(v, &area),
                        "seed {seed}: a building in the {:?}",
                        q.kind
                    );
                }
            }
        }

        // The tunnel: on the freeway, no other road through its walls, no building within
        // a few meters of them.
        let pieces = tunnel(&p);
        let (a, b) = (at(pieces[0]), at(pieces[pieces.len() - 1]));
        let half = match pieces[0].kind {
            PropKind::TunnelPiece { width_dm, .. } => 0.05 * f32::from(width_dm),
            _ => unreachable!(),
        };
        for e in &p.roads.edges {
            let freeway = e.class == place::RoadClass::Highway && !e.oneway && e.lanes_fwd >= 2;
            if freeway {
                continue;
            }
            for s in e.profile.windows(2) {
                for q in [s[0], s[1]] {
                    let d = segment_distance(V2::new(q[0], q[2]), a, b).0;
                    assert!(d > half, "seed {seed}: a road inside the tunnel");
                }
            }
        }
        for bd in &p.buildings {
            for v in &bd.footprint {
                let d = segment_distance(V2::from_array(*v), a, b).0;
                assert!(d > half + 1.0, "seed {seed}: a building at the tunnel");
            }
        }
        println!(
            "seed {seed}: tunnel {:.0},{:.0} .. {:.0},{:.0}",
            a.x, a.y, b.x, b.y
        );
    }
}

/// The roadside set's landmarks in `p`: each prop, its kit and the kit's lot rectangle.
fn roadside<'a>(p: &'a Place, g: &'a world::Grammar) -> Vec<(&'a place::Prop, &'a Kit, Vec<V2>)> {
    p.props
        .items
        .iter()
        .filter_map(|q| match q.kind {
            PropKind::Landmark { kit, .. } => {
                let k = &g.landmarks[usize::from(kit)];
                let ours = ROADSIDE.iter().any(|(id, _, _)| *id == k.id);
                ours.then(|| (q, k, footprint(q, k.lot[0], k.lot[1])))
            }
            _ => None,
        })
        .collect()
}

/// Whether the segments `a..b` and `c..d` cross.
fn crosses(a: V2, b: V2, c: V2, d: V2) -> bool {
    let side = |p: V2, q: V2, r: V2| (q - p).turn(r - p);
    side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0
}

/// The freeway's centerline segments.
fn freeway(p: &Place) -> Vec<(V2, V2)> {
    p.roads
        .edges
        .iter()
        .filter(|e| e.class == place::RoadClass::Highway && !e.oneway && e.lanes_fwd >= 2)
        .flat_map(|e| {
            e.profile
                .windows(2)
                .map(|s| (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2])))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn the_roadside_set_stands_once_each_on_its_own_clear_lot() {
    let g = grammar();
    let picks: Vec<(usize, u8)> = ROADSIDE
        .iter()
        .map(|(id, letter, _)| {
            let k = g.landmarks.iter().position(|k| k.id == *id).unwrap();
            (k, *letter as u8 - b'A')
        })
        .collect();
    for seed in 1..=4u64 {
        let p = basin(seed);
        let set = roadside(&p, &g);
        // Once each, in the owner's picks, each with its point of interest.
        assert_eq!(set.len(), ROADSIDE.len(), "seed {seed}");
        for &(kit, variant) in &picks {
            let n = count(&p, |k| {
                k == PropKind::Landmark {
                    kit: kit as u8,
                    variant,
                }
            });
            assert_eq!(n, 1, "seed {seed}: kit {kit}");
        }
        for (_, _, poi) in ROADSIDE {
            assert_eq!(pois(&p, poi), 1, "seed {seed}: {poi:?}");
        }
        let fronts: Vec<V2> = p
            .props
            .items
            .iter()
            .filter(|q| {
                matches!(
                    q.kind,
                    PropKind::CrewGarage { .. } | PropKind::TacoStand { .. }
                )
            })
            .map(at)
            .collect();
        for (i, (q, kit, area)) in set.iter().enumerate() {
            let name = &kit.id;
            // No road on its ground: every road stays clear of the lot's corners and no
            // road's centerline point is inside it.
            for e in &p.roads.edges {
                for s in e.profile.windows(2) {
                    let (a, b) = (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2]));
                    for c in area {
                        let d = segment_distance(*c, a, b).0;
                        assert!(d > 0.5 * e.width_m, "seed {seed}: a road on the {name}");
                    }
                    assert!(
                        !point_in_polygon(a, area),
                        "seed {seed}: a road through the {name}"
                    );
                }
            }
            // No building on it, and nothing the detailing scatters: only its own cars.
            for b in &p.buildings {
                for v in &b.footprint {
                    let inside = point_in_polygon(V2::from_array(*v), area);
                    assert!(!inside, "seed {seed}: a building on the {name}");
                }
            }
            let shrunk: Vec<V2> = {
                let c = area.iter().fold(V2::new(0.0, 0.0), |a, &v| a + v) * 0.25;
                area.iter().map(|&v| c + (v - c) * 0.97).collect()
            };
            for o in &p.props.items {
                let ours = std::ptr::eq(o, *q) || matches!(o.kind, PropKind::ParkedCar { .. });
                if !ours && point_in_polygon(at(o), &shrunk) {
                    panic!("seed {seed}: a {:?} on the {name}", o.kind);
                }
            }
            // Apart from each other and the garage and stand.
            for (j, (o, _, _)) in set.iter().enumerate() {
                if i != j {
                    let d = (at(o) - at(q)).length();
                    assert!(
                        d >= ROADSIDE_APART_M,
                        "seed {seed}: {name} {d:.0} m from another"
                    );
                }
            }
            for f in &fronts {
                assert!(
                    (*f - at(q)).length() >= ROADSIDE_APART_M,
                    "seed {seed}: {name}"
                );
            }
            // Its parked cars stand on its lot, clear of its solid parts.
            let variant = match q.kind {
                PropKind::Landmark { variant, .. } => usize::from(variant),
                _ => unreachable!(),
            };
            let mut slabs = Vec::new();
            let facing = dir_of_yaw(q.yaw);
            world::landmarks::slabs(kit, variant, at(q), q.pos[1], facing, &mut slabs);
            let mut cars = 0;
            for o in &p.props.items {
                if !matches!(o.kind, PropKind::ParkedCar { .. }) || !point_in_polygon(at(o), area) {
                    continue;
                }
                cars += 1;
                for sl in &slabs {
                    let d = at(o) - sl.c;
                    let (u, v) = (sl.u, sl.u.right());
                    let inside =
                        d.dot(u).abs() < sl.half[0] + 1.0 && d.dot(v).abs() < sl.half[1] + 1.0;
                    assert!(!inside, "seed {seed}: a car parked in the {name}'s solids");
                }
            }
            assert!(cars >= 4, "seed {seed}: {cars} cars at the {name}");
            // Its lamps light it.
            let lit = p
                .lights
                .items
                .iter()
                .filter(|l| point_in_polygon(V2::new(l.pos[0], l.pos[2]), area))
                .count();
            assert!(lit >= 1, "seed {seed}: the {name} is dark");
            println!(
                "seed {seed}: {name} at {:.0},{:.0} facing {:.2},{:.2}, {cars} cars, {lit} lamps",
                q.pos[0], q.pos[2], facing.x, facing.y
            );
        }
        // The drive-in's screen shows from the freeway: near it, and some freeway point
        // within reach sees the screen's middle over every building lower than its top.
        let (q, kit, _) = set.iter().find(|(_, k, _)| k.id == "drive_in").unwrap();
        let screen = at(q) - dir_of_yaw(q.yaw) * (kit.lot[1] - 6.0);
        let fwy = freeway(&p);
        let near = fwy
            .iter()
            .map(|&(a, b)| segment_distance(screen, a, b).0)
            .fold(f32::INFINITY, f32::min);
        assert!(
            near < DRIVE_IN_VIEW_M + 80.0,
            "seed {seed}: screen {near:.0} m off the freeway"
        );
        let seen = fwy.iter().flat_map(|&(a, b)| [a, a.lerp(b, 0.5)]).any(|f| {
            (f - screen).length() < 600.0
                && !p.buildings.iter().any(|b| {
                    let n = b.footprint.len();
                    b.height_m > 12.0
                        && (0..n).any(|i| {
                            let (u, v) = (b.footprint[i], b.footprint[(i + 1) % n]);
                            crosses(f, screen, V2::from_array(u), V2::from_array(v))
                        })
                })
        });
        assert!(
            seen,
            "seed {seed}: no freeway view of the drive-in's screen"
        );
    }
}

#[test]
fn a_car_drives_into_the_drive_in_past_its_ticket_booth() {
    let spec = halberd();
    let g = grammar();
    for seed in [1u64, 3] {
        let p = basin(seed);
        let c = WorldCollider::build(&p, &g);
        let (q, _, _) = roadside(&p, &g)
            .into_iter()
            .find(|(_, k, _)| k.id == "drive_in")
            .unwrap();
        let out = dir_of_yaw(q.yaw);
        let right = (-out).right();
        let ground = |v: V2| [v.x, c.level_at([v.x, v.y]).0, v.y];
        // The entry lane left of the booth, seen from the road.
        let start = at(q) + right * 23.0 + out * 2.0;
        let mut s = VehicleState::spawn(&spec, ground(start), heading_of(-out));
        drive(&spec, &mut s, &c, 10.0, |s| input(cruise(s, 5.0), 0.0, 0.0));
        let into = (at(q) - V2::new(s.pos[0], s.pos[2])).dot(out);
        assert!(into > 30.0, "seed {seed}: {into:.1} m into the drive-in");
    }
}

fn halberd() -> VehicleSpec {
    let s: VehicleSpec = ron::from_str(HALBERD).expect("spec");
    s.validate().expect("valid");
    s
}

fn input(throttle: f32, brake: f32, steer: f32) -> VehicleInput {
    VehicleInput {
        throttle,
        brake,
        steer,
        handbrake: false,
    }
}

fn heading_of(d: V2) -> f32 {
    (-d.x).atan2(-d.y)
}

/// Drives `s` with `inp` (from the state) for `secs`, returning the largest closing speed
/// any hit reported.
fn drive(
    spec: &VehicleSpec,
    s: &mut VehicleState,
    c: &WorldCollider,
    secs: f32,
    inp: impl Fn(&VehicleState) -> VehicleInput,
) {
    for _ in 0..(secs * 120.0) as usize {
        let i = inp(s);
        step(spec, s, &i, &Assists::default(), c);
        resolve(s, spec, c);
    }
}

fn cruise(s: &VehicleState, mps: f32) -> f32 {
    (0.3 + (mps - s.speed_mps) * 0.5).clamp(0.0, 1.0)
}

#[test]
fn the_tunnel_walls_its_road_and_the_garage_takes_a_car() {
    let spec = halberd();
    let p = basin(1);
    let c = WorldCollider::build(&p, &grammar());
    let pieces = tunnel(&p);
    let d = dir_of_yaw(pieces[0].yaw);
    let (a, b) = (at(pieces[0]), at(pieces[pieces.len() - 1]));
    let half = match pieces[0].kind {
        PropKind::TunnelPiece { width_dm, .. } => 0.05 * f32::from(width_dm),
        _ => unreachable!(),
    };
    let ground = |q: V2| [q.x, c.level_at([q.x, q.y]).0, q.y];
    // Through it, in the outer lane, at 25 m/s: out the far portal.
    let start = a - d * 40.0 + d.right() * 7.5;
    let mut s = VehicleState::spawn(&spec, ground(start), heading_of(d));
    drive(&spec, &mut s, &c, 9.0, |s| input(cruise(s, 25.0), 0.0, 0.0));
    let along = (V2::new(s.pos[0], s.pos[2]) - a).dot(d);
    assert!(
        along > (b - a).length() + 8.0,
        "through the tunnel: {along:.1} m"
    );
    // Steered hard at a wall at 20 m/s from its middle: the wall holds it inside.
    let mid = a.lerp(b, 0.5);
    for sigma in [-1.0f32, 1.0] {
        let aim = (d + d.right() * (0.6 * sigma)).normalize();
        let mut s = VehicleState::spawn(&spec, ground(mid - d * 20.0), heading_of(aim));
        drive(&spec, &mut s, &c, 3.0, |s| input(cruise(s, 20.0), 0.0, 0.0));
        let q = V2::new(s.pos[0], s.pos[2]);
        let across = (q - a).dot(d.right()).abs();
        assert!(
            across < half,
            "out through the wall: {across:.1} m of {half:.1}"
        );
        assert!(s.pos.iter().all(|v| v.is_finite()));
    }

    // The garage: into the middle bay from the apron at a walking pace.
    let g = p
        .props
        .items
        .iter()
        .find(|q| matches!(q.kind, PropKind::CrewGarage { .. }))
        .unwrap();
    let out = dir_of_yaw(g.yaw);
    let front = at(g);
    let mut s = VehicleState::spawn(&spec, ground(front + out * 12.0), heading_of(-out));
    drive(&spec, &mut s, &c, 6.0, |s| {
        let into = (front - V2::new(s.pos[0], s.pos[2])).dot(out);
        if into < 5.0 {
            input(cruise(s, 3.0), 0.0, 0.0)
        } else if s.speed_mps.abs() > 0.3 {
            input(0.0, 1.0, 0.0)
        } else {
            input(0.0, 0.0, 0.0)
        }
    });
    let into = (front - V2::new(s.pos[0], s.pos[2])).dot(out);
    assert!(into > 3.0, "into the bay: {into:.1} m");
    // Into its side wall from beside it: it stops outside.
    let x = out.left();
    let (gw, gd, _) = world::setpieces::GARAGE_M;
    let side = front + x * (0.5 * gw + 10.0) - out * (0.5 * gd);
    let mut s = VehicleState::spawn(&spec, ground(side), heading_of(-x));
    drive(&spec, &mut s, &c, 5.0, |s| input(cruise(s, 8.0), 0.0, 0.0));
    let off = (V2::new(s.pos[0], s.pos[2]) - front).dot(x);
    assert!(off > 0.5 * gw, "through the garage's wall: {off:.1} m");
}

/// The landmark built from kit `id` (the basin's one), and that kit's lot.
fn landmark<'a>(p: &'a Place, id: &str) -> (&'a place::Prop, u8, [f32; 2]) {
    let kits = grammar().landmarks;
    let k = kits.iter().position(|k| k.id == id).unwrap();
    let mut marks = p.props.items.iter().filter_map(|q| match q.kind {
        PropKind::Landmark { kit, variant } if usize::from(kit) == k => Some((q, variant)),
        _ => None,
    });
    let (q, v) = marks.next().unwrap_or_else(|| panic!("no {id}"));
    assert!(marks.next().is_none(), "two {id}s");
    (q, v, kits[k].lot)
}

#[test]
fn every_basin_has_its_landmarks_on_open_ground_and_lit() {
    use world::procgen::setpieces::{LETTERS, LETTERS_RISE_M, WATER_TOWER};
    for seed in 1..=4u64 {
        let p = basin(seed);
        let c = WorldCollider::build(&p, &grammar());
        for kind in [PoiKind::HillsideLetters, PoiKind::WaterTower] {
            assert_eq!(pois(&p, kind), 1, "seed {seed}: {kind:?}");
        }
        let mut fronts = Vec::new();
        for (id, variant) in [LETTERS, WATER_TOWER] {
            let (q, v, [w, d]) = landmark(&p, id);
            assert_eq!(v, variant, "seed {seed}: {id}");
            // No road and no building on its lot; the letters keep well clear of roads.
            let area = footprint(q, w, d);
            let clear = if id == LETTERS.0 { 12.0 } else { 0.0 };
            for e in &p.roads.edges {
                for s in e.profile.windows(2) {
                    let (a, b) = (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2]));
                    for k in 0..=20 {
                        let t = k as f32 / 20.0;
                        let edge = area[k % 4].lerp(area[(k + 1) % 4], t);
                        let d = segment_distance(edge, a, b).0;
                        assert!(d > 0.5 * e.width_m + clear, "seed {seed}: {id} on a road");
                    }
                }
            }
            for b in &p.buildings {
                for v in &b.footprint {
                    let v = V2::from_array(*v);
                    assert!(
                        !world::math::point_in_polygon(v, &area),
                        "seed {seed}: a building on the {id}'s lot"
                    );
                }
            }
            // Its lamps light it.
            let lit = p
                .lights
                .items
                .iter()
                .filter(|l| {
                    let at = V2::new(l.pos[0], l.pos[2]);
                    world::math::point_in_polygon(at, &area)
                })
                .count();
            assert!(lit >= 2, "seed {seed}: {id} has {lit} lights");
            // Something of it stops a car.
            let solid = c
                .boxes
                .iter()
                .filter(|b| matches!(b.kind, world::props::PropKind::Landmark { .. }))
                .filter(|b| world::math::point_in_polygon(V2::new(b.c[0], b.c[1]), &area))
                .count();
            assert!(solid >= 1, "seed {seed}: nothing of the {id} is solid");
            fronts.push(at(q));
            let letter = world::landmarks::letter(usize::from(v));
            println!(
                "seed {seed}: {id} {letter} at {:.0},{:.0}",
                q.pos[0], q.pos[2]
            );
        }
        // Apart from each other.
        for i in 0..fronts.len() {
            for j in i + 1..fronts.len() {
                assert!((fronts[i] - fronts[j]).length() > 150.0, "seed {seed}");
            }
        }

        // The letters: up the hills over the road at their foot, facing out over the
        // valley, their line on the ground (buried a little, never floating).
        let (q, _, [w, _]) = landmark(&p, LETTERS.0);
        let out = dir_of_yaw(q.yaw);
        let x = out.left();
        let mut lowest = f32::MAX;
        for k in 0..=24 {
            let lx = (k as f32 / 24.0 - 0.5) * (w - 6.0);
            for z in [8.0f32, 9.2] {
                let g = at(q) + x * lx - out * z;
                let ground = c.level_at([g.x, g.y]).0;
                lowest = lowest.min(ground - q.pos[1]);
                assert!(
                    ground - q.pos[1] < 3.5 && ground - q.pos[1] > -0.6,
                    "seed {seed}: the letters' foot is {:.1} m off the ground",
                    ground - q.pos[1]
                );
            }
        }
        let below = at(q) + out * 80.0;
        let rise = q.pos[1] - c.level_at([below.x, below.y]).0;
        assert!(
            rise > 0.4 * LETTERS_RISE_M,
            "seed {seed}: letters {rise:.0} m up"
        );
    }
}

#[test]
fn the_water_tower_stops_a_car() {
    use world::procgen::setpieces::WATER_TOWER;
    let spec = halberd();
    let p = basin(1);
    let c = WorldCollider::build(&p, &grammar());
    let ground = |q: V2| [q.x, c.level_at([q.x, q.y]).0, q.y];
    // From the road straight at the tower's column: it stops the car short.
    let (id, depth) = (WATER_TOWER.0, 12.0);
    let (q, _, _) = landmark(&p, id);
    let out = dir_of_yaw(q.yaw);
    let front = at(q);
    // On the lot's front edge, clear of the cars parked at the curb.
    let start = front - out * 0.5;
    let mut s = VehicleState::spawn(&spec, ground(start), heading_of(-out));
    drive(&spec, &mut s, &c, 10.0, |s| input(cruise(s, 8.0), 0.0, 0.0));
    let into = (front - V2::new(s.pos[0], s.pos[2])).dot(out);
    assert!(into < depth, "{id}: {into:.1} m into its lot of {depth}");
    assert!(into > depth - 6.0, "{id}: only {into:.1} m in");
    println!("{id}: stopped {into:.1} m in");
    assert!(s.pos.iter().all(|v| v.is_finite()));
}

#[test]
fn every_basin_has_its_downtown_anchors() {
    use world::procgen::setpieces::{HALL, PALACE};
    let g = grammar();
    let kit = |id: &str| g.landmarks.iter().position(|k| k.id == id).unwrap();
    let (hall, palace) = (kit(HALL.0), kit(PALACE.0));
    let lot = |k: usize| (g.landmarks[k].lot[0], g.landmarks[k].lot[1]);
    let (hall_lot, palace_lot) = (lot(hall), lot(palace));
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    for seed in 1..=4u64 {
        let p = build_place(Seed(seed), &t, &g);
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        assert_eq!(pois(&p, PoiKind::CityHall), 1, "seed {seed}");
        assert_eq!(pois(&p, PoiKind::MoviePalace), 1, "seed {seed}");
        let m = world::procgen::MacroMap::generate(Seed(seed), &t, world::procgen::PLACE_CHUNKS);
        let b = m.basin.as_ref().unwrap();
        for (k, (w, d)) in [(hall, hall_lot), (palace, palace_lot)] {
            let q = p
                .props
                .items
                .iter()
                .find(|q| matches!(q.kind, PropKind::Landmark { kit, .. } if usize::from(kit) == k))
                .unwrap_or_else(|| panic!("seed {seed}: no kit {k}"));
            let area = footprint(q, w, d);
            // No road, no building on the lot; every landmark light on it.
            for e in &p.roads.edges {
                for s in e.profile.windows(2) {
                    let (a, c) = (V2::new(s[0][0], s[0][2]), V2::new(s[1][0], s[1][2]));
                    for v in &area {
                        let dist = segment_distance(*v, a, c).0;
                        assert!(dist > 0.5 * e.width_m, "seed {seed}: kit {k} on a road");
                    }
                }
            }
            for bd in &p.buildings {
                for v in &bd.footprint {
                    let v = V2::from_array(*v);
                    assert!(
                        !world::math::point_in_polygon(v, &area),
                        "seed {seed}: a building on kit {k}"
                    );
                }
            }
            println!("seed {seed}: kit {k} at {:.0},{:.0}", q.pos[0], q.pos[2]);
        }
        // The hall faces back down a boulevard from its head, on its axis.
        let q = p
            .props
            .items
            .iter()
            .find(|q| matches!(q.kind, PropKind::Landmark { kit, .. } if usize::from(kit) == hall))
            .unwrap();
        let (front, out) = (at(q), dir_of_yaw(q.yaw));
        let heads = b.lines.iter().filter(|l| l.boulevard).any(|l| {
            let (a, c) = (V2::from_array(l.a), V2::from_array(l.b));
            [(a, c), (c, a)].iter().any(|&(end, from)| {
                let down = (from - end).normalize();
                let on_axis = segment_distance(front, end - down * 60.0, end).0 < 0.5;
                on_axis && out.dot(down) > 0.999 && (front - end).length() < 40.0
            })
        });
        assert!(
            heads,
            "seed {seed}: the city hall does not head a boulevard"
        );
    }
}

#[test]
fn the_city_hall_stops_a_car_driven_up_the_boulevard_into_it() {
    let spec = halberd();
    let g = grammar();
    let hall = g
        .landmarks
        .iter()
        .position(|k| k.id == world::procgen::setpieces::HALL.0)
        .unwrap();
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    let p = build_place(Seed(1), &t, &g);
    let c = WorldCollider::build(&p, &g);
    let q = p
        .props
        .items
        .iter()
        .find(|q| matches!(q.kind, PropKind::Landmark { kit, .. } if usize::from(kit) == hall))
        .unwrap();
    let (front, out) = (at(q), dir_of_yaw(q.yaw));
    let ground = |v: V2| [v.x, c.level_at([v.x, v.y]).0, v.y];
    // Up the boulevard's inner lane from 150 m out at 20 m/s, straight on across the T and
    // over the forecourt: the building stops it short of the back of its lot.
    let start = front + out * 150.0 + out.left() * 6.0;
    let mut s = VehicleState::spawn(&spec, ground(start), heading_of(-out));
    drive(&spec, &mut s, &c, 14.0, |s| {
        input(cruise(s, 20.0), 0.0, 0.0)
    });
    let into = (front - V2::new(s.pos[0], s.pos[2])).dot(out);
    println!("into the city hall's lot: {into:.1} m");
    assert!(s.pos.iter().all(|v| v.is_finite()));
    assert!(into > 2.0, "the car never reached the hall: {into:.1} m");
    assert!(into < 20.0, "through the city hall: {into:.1} m");
}
