//! Acceptance tests for parking lots (`world::lots`, B10) on the procedural coast: the
//! layout (stalls clear of each other, the buildings, the walks and the streets; every
//! stall reached from a curb cut), occupancy by zone and hour, and collision (wheel stops,
//! poles and islands stop or lift the car, stripes do not; a scripted car drives in,
//! parks and leaves). Run with `--nocapture` to read the numbers:
//! `cargo test -p world --test lots -- --nocapture`.

use std::collections::BTreeMap;

use place::{Place, PropKind, ZoneId};
use skywave_core::Seed;
use vehicle::{step, Assists, VehicleInput, VehicleSpec, VehicleState};
use world::collide::Surface;
use world::lots::{self, ParkingLot, ISLAND_H_M, STOP_H_M};
use world::math::{point_in_polygon, V2};
use world::procgen::build_place;
use world::props::parked_now;
use world::roads::Network;
use world::template::RegionTemplate;
use world::{resolve, WorldCollider};

const GRAMMAR: &str = include_str!("../../../assets/grammar/california_coast_80s.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon.ron");
const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");
const GANNET: &str = include_str!("../../../assets/vehicles/gannet_gt.ron");
const STEPS_PER_S: usize = 120;

fn grammar() -> world::Grammar {
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("parse grammar");
    g.lexicon = ron::from_str(LEXICON).expect("parse lexicon");
    g
}

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
    t.validate().expect("validate");
    t
}

/// The coast of `seed`, its network with the lots' curb cuts, and its lots.
fn coast(seed: u64) -> (Place, Network, Vec<ParkingLot>) {
    let g = grammar();
    let place = build_place(Seed(seed), &template(), &g);
    let mut net = Network::new(&place);
    let lots = lots::plan(&place, &net, &g.lots);
    net.cuts = lots::cuts(&lots);
    (place, net, lots)
}

/// `q` shrunk toward its middle by `by` (so quads sharing an edge do not count as
/// overlapping).
fn shrink(q: &[V2; 4], by: f32) -> [V2; 4] {
    let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
    q.map(|p| p + (c - p).normalize() * by)
}

fn cross(a: V2, b: V2, c: V2) -> f32 {
    (b - a).turn(c - a)
}

fn segments_cross(a: V2, b: V2, c: V2, d: V2) -> bool {
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

/// Whether polygons `a` and `b` (either may be concave) overlap.
fn overlap(a: &[V2], b: &[V2]) -> bool {
    let (n, m) = (a.len(), b.len());
    for i in 0..n {
        for j in 0..m {
            if segments_cross(a[i], a[(i + 1) % n], b[j], b[(j + 1) % m]) {
                return true;
            }
        }
    }
    point_in_polygon(a[0], b) || point_in_polygon(b[0], a)
}

fn bounds(q: &[V2]) -> (V2, V2) {
    let (mut lo, mut hi) = (V2::new(f32::MAX, f32::MAX), V2::new(f32::MIN, f32::MIN));
    for p in q {
        lo = V2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = V2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, hi)
}

fn boxes_meet(a: (V2, V2), b: (V2, V2), r: f32) -> bool {
    a.0.x <= b.1.x + r && a.1.x >= b.0.x - r && a.0.y <= b.1.y + r && a.1.y >= b.0.y - r
}

/// Distance from `p` to edge `e`'s centerline.
fn road_distance(net: &Network, e: usize, p: V2) -> f32 {
    let path = &net.paths[e];
    let (s, _) = path.project(p);
    (p - path.xz(s, 0.0)).length()
}

#[test]
fn lots_are_laid_clear_and_every_stall_reaches_the_street() {
    let mut totals: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let stall_w = grammar().lots.stall_w;
    for seed in 1..=8 {
        let (place, net, lots) = coast(seed);
        assert!(!lots.is_empty(), "seed {seed}: no lots");
        let parcels: Vec<(Vec<V2>, (V2, V2))> = place
            .parcels
            .iter()
            .filter(|p| p.lot_kind == place::LotKind::Lot)
            .map(|p| {
                let poly: Vec<V2> = p.polygon.iter().map(|&q| V2::from_array(q)).collect();
                let bb = bounds(&poly);
                (poly, bb)
            })
            .collect();
        // Every stall is a car wide (across the car, at both its ends), even where an
        // alley bends.
        for l in &lots {
            for st in &l.stalls {
                let across = st.nose.right();
                let w = |a: V2, b: V2| ((b - a).dot(across)).abs();
                let narrow = w(st.q[0], st.q[1]).min(w(st.q[3], st.q[2]));
                assert!(
                    narrow >= stall_w - 0.3,
                    "seed {seed}: stall at {:?} only {narrow:.2} m wide",
                    st.c
                );
            }
        }
        // Beach lots stay off every parcel; a parcel's lot stays within its own.
        for l in &lots {
            for st in &l.stalls {
                let q = shrink(&st.q, 0.05);
                let bb = bounds(&q);
                let hits = parcels
                    .iter()
                    .filter(|(poly, pb)| boxes_meet(bb, *pb, 0.0) && overlap(&q, poly))
                    .count();
                let most = if l.zone == ZoneId::Beach { 0 } else { 1 };
                assert!(
                    hits <= most,
                    "seed {seed}: {:?} stall at {:?} in {hits} parcels",
                    l.zone,
                    st.c
                );
            }
        }
        let fps: Vec<(Vec<V2>, (V2, V2))> = place
            .buildings
            .iter()
            .map(|b| {
                let fp: Vec<V2> = b.footprint.iter().map(|&p| V2::from_array(p)).collect();
                let bb = bounds(&fp);
                (fp, bb)
            })
            .collect();
        let mut junctions: Vec<Vec<V2>> = Vec::new();
        for j in &net.junctions {
            let ring: Vec<V2> = net.junction_ring(j).iter().map(|p| p.xz()).collect();
            if ring.len() >= 3 {
                junctions.push(ring);
            }
            for i in 0..j.corners.len() {
                let walk: Vec<V2> = net.corner_walk(j, i).iter().map(|p| p.xz()).collect();
                if walk.len() >= 3 {
                    junctions.push(walk);
                }
            }
        }
        let junctions: Vec<(Vec<V2>, (V2, V2))> = junctions
            .into_iter()
            .map(|p| {
                let bb = bounds(&p);
                (p, bb)
            })
            .collect();
        let all: Vec<(usize, [V2; 4])> = lots
            .iter()
            .enumerate()
            .flat_map(|(k, l)| l.stalls.iter().map(move |s| (k, shrink(&s.q, 0.02))))
            .collect();
        for (i, (k, q)) in all.iter().enumerate() {
            let bb = bounds(q);
            // Stalls never overlap each other...
            for (_, o) in &all[i + 1..] {
                if boxes_meet(bb, bounds(o), 0.0) {
                    assert!(!overlap(q, o), "seed {seed}: stalls overlap at {:?}", q[0]);
                }
            }
            // ...or a building...
            for (fp, fb) in &fps {
                if boxes_meet(bb, *fb, 0.0) {
                    assert!(
                        !overlap(q, fp),
                        "seed {seed}: stall in a building at {:?}",
                        q[0]
                    );
                }
            }
            // ...or a junction or its corner walks...
            for (j, jb) in &junctions {
                if boxes_meet(bb, *jb, 0.0) {
                    assert!(
                        !overlap(q, j),
                        "seed {seed}: stall in a junction at {:?}",
                        q[0]
                    );
                }
            }
            // ...or a street or its walk (a lot on an alley opens onto it past its curb).
            let lot = &lots[*k];
            let alley = lot.on_alley.then(|| lot.cuts[0].edge as usize);
            let c = (q[0] + q[1] + q[2] + q[3]) * 0.25;
            for (e, edge) in place.roads.edges.iter().enumerate() {
                let (end, _) = net.ends(e);
                let opens = lot.on_alley && lot.cuts.iter().any(|c| c.edge as usize == e);
                let reach = if Some(e) == alley || opens {
                    end.hw + lots::CURB_TOP_M - 0.05
                } else {
                    end.hw + end.sw.max(lots::CURB_TOP_M) - 0.05
                };
                let _ = edge;
                for &p in q.iter().chain([&c]) {
                    let d = road_distance(&net, e, p);
                    assert!(
                        d >= reach,
                        "seed {seed}: stall on edge {e} ({:?}) at {p:?}: {d:.2} m from its middle",
                        place.roads.edges[e].class
                    );
                }
            }
        }
        for lot in &lots {
            let e = totals.entry(format!("{:?}", lot.zone)).or_default();
            e.0 += 1;
            e.1 += lot.stalls.len();
            assert!(!lot.cuts.is_empty() && !lot.entries.is_empty());
            // Every stall opens onto an aisle, or onto the alley...
            for s in &lot.stalls {
                let in_aisle = lot.aisles.iter().any(|a| point_in_polygon(s.mouth, a));
                let on_alley = lot.on_alley
                    && lot.cuts.iter().any(|c| {
                        road_distance(&net, c.edge as usize, s.mouth)
                            < net.ends(c.edge as usize).0.hw
                    });
                assert!(
                    in_aisle || on_alley,
                    "seed {seed}: stall at {:?} opens onto nothing",
                    s.c
                );
            }
            // ...every aisle meets a drive...
            for a in &lot.aisles {
                assert!(
                    lot.entries.iter().any(|d| overlap(a, d)),
                    "seed {seed}: aisle at {:?} meets no drive",
                    a[0]
                );
            }
            // ...and every drive starts at its curb cut's road.
            for d in &lot.entries {
                let near = lot.cuts.iter().any(|c| {
                    let e = c.edge as usize;
                    let (end, _) = net.ends(e);
                    let line = if lot.on_alley {
                        end.hw + lots::CURB_TOP_M
                    } else {
                        end.hw + end.sw
                    };
                    [d[0], d[1]]
                        .iter()
                        .all(|&p| (road_distance(&net, e, p) - line).abs() < 0.8)
                });
                assert!(near, "seed {seed}: drive at {:?} starts off its road", d[0]);
            }
        }
    }
    println!("lots and stalls per zone, seeds 1-8: {totals:?}");
    // Strip-mall front lots and alley lots on the strip, beach lots on the beach.
    for z in ["Beach", "Strip", "Commercial", "Downtown"] {
        assert!(totals.contains_key(z), "no lots in {z}: {totals:?}");
    }
}

#[test]
fn lots_are_deterministic() {
    let (_, _, a) = coast(2);
    let (_, _, b) = coast(2);
    assert_eq!(a, b);
}

/// Parked lot cars of `place` in zone `z` there at hour `h`.
fn parked(place: &Place, z: ZoneId, h: f32) -> usize {
    place
        .props
        .items
        .iter()
        .filter(|p| {
            let PropKind::ParkedCar { hours, .. } = p.kind else {
                return false;
            };
            hours != place::ALL_HOURS
                && world::zone_at(place, V2::new(p.pos[0], p.pos[2])) == z
                && parked_now(hours, p.pos, h * 3600.0)
        })
        .count()
}

#[test]
fn lots_fill_and_empty_by_zone_and_hour() {
    let g = grammar();
    let mut by: BTreeMap<u8, [usize; 24]> = BTreeMap::new();
    for seed in 1..=4 {
        let (place, _, _) = coast(seed);
        for z in [
            ZoneId::Beach,
            ZoneId::Strip,
            ZoneId::Commercial,
            ZoneId::Downtown,
        ] {
            let row = by.entry(z as u8).or_insert([0; 24]);
            for (h, n) in row.iter_mut().enumerate() {
                *n += parked(&place, z, h as f32 + 0.5);
            }
        }
    }
    for (z, row) in &by {
        println!("zone {z} lot cars by hour: {row:?}");
    }
    let beach = by[&(ZoneId::Beach as u8)];
    // The beach lot fullest around noon, near empty before dawn.
    assert!(beach[12] > beach[9] && beach[12] > beach[20], "{beach:?}");
    assert!(beach[3] * 10 < beach[12], "{beach:?}");
    // The downtown offices' lots empty at night, full in working hours.
    let office = by[&(ZoneId::Downtown as u8)];
    assert!(office[10] > 0, "{office:?}");
    assert_eq!(office[2], 0, "{office:?}");
    assert_eq!(office[23], 0, "{office:?}");
    // The shops' lots busier in the afternoon than at night.
    for z in [ZoneId::Strip, ZoneId::Commercial] {
        let r = by[&(z as u8)];
        assert!(r[13] > r[4], "{z:?}: {r:?}");
    }
    // A stall's hours follow its zone's curve: none where it is 0 %, all where 100 %.
    let mut r = g.lots.clone();
    r.occupancy = vec![(ZoneId::Strip, [0; 24]), (ZoneId::Beach, [100; 24])];
    let at = V2::new(10.0, 20.0);
    assert_eq!(lots::stall_hours(&r, ZoneId::Strip, 1, at), 0);
    assert_eq!(
        lots::stall_hours(&r, ZoneId::Beach, 1, at),
        place::ALL_HOURS | lots::LOT_CAR
    );
    assert_eq!(lots::stall_hours(&r, ZoneId::Suburb, 1, at), 0);
}

fn spec() -> VehicleSpec {
    let s: VehicleSpec = ron::from_str(GANNET).expect("spec parses");
    s.validate().expect("spec validates");
    s
}

fn xz(p: [f32; 3]) -> V2 {
    V2::new(p[0], p[2])
}

/// Yaw angle with `VehicleState::spawn`'s convention (0 faces `-z`, positive turns left)
/// that faces ground direction `d`.
fn heading_of(d: V2) -> f32 {
    (-d.x).atan2(-d.y)
}

fn forward(s: &VehicleState) -> V2 {
    xz(s.forward()).normalize()
}

#[test]
fn lot_furniture_lifts_or_stops_the_car_and_paint_does_not() {
    let (place, _, lots) = coast(1);
    let c = WorldCollider::build(&place, &grammar());
    let ground = |p: V2| c.terrain.as_ref().unwrap().height_at(p.x, p.y);
    let lot = lots
        .iter()
        .find(|l| !l.stops.is_empty() && !l.islands.is_empty())
        .unwrap();
    let i = &lot.islands[0];
    let ic = (i.q[0] + i.q[1] + i.q[2] + i.q[3]) * 0.25;
    let (y, surface) = c.level_at([ic.x, ic.y]);
    assert!(
        (y - ground(ic) - ISLAND_H_M).abs() < 0.01,
        "island at {}",
        y - ground(ic)
    );
    assert_eq!(surface, Surface::Grass);
    let s = lot.stops[0];
    let sc = (s[0] + s[1]) * 0.5;
    let (y, _) = c.level_at([sc.x, sc.y]);
    assert!(
        (y - ground(sc) - STOP_H_M).abs() < 0.01,
        "wheel stop at {}",
        y - ground(sc)
    );
    // A stripe is paint: the lot's own height and asphalt.
    let st = lot.stripes[lot.stripes.len() / 2];
    let m = (st[0] + st[1]) * 0.5;
    let (y, surface) = c.level_at([m.x, m.y]);
    let lift = lots::LOT_LIFT_M;
    assert!(
        (y - ground(m) - lift).abs() < 0.01,
        "stripe at {}",
        y - ground(m)
    );
    assert_eq!(surface, Surface::Asphalt);
    // A light pole stops a car driving at it.
    let (pole, _) = lot.poles[0];
    let car = spec();
    let mut hit = false;
    for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let d = V2::new(dx, dz);
        let from = pole - d * 6.0;
        let y = c.level_at([from.x, from.y]).0;
        let mut st = VehicleState::spawn(&car, [from.x, y, from.y], heading_of(d));
        for _ in 0..3 * STEPS_PER_S {
            let inp = VehicleInput {
                throttle: if st.speed_mps < 4.0 { 0.5 } else { 0.0 },
                ..VehicleInput::default()
            };
            step(&car, &mut st, &inp, &Assists::default(), &c);
            let impact = resolve(&mut st, &car, &c);
            if impact.contact && impact.hit.speed > 1.0 {
                hit = true;
            }
        }
        if hit {
            break;
        }
        assert!((xz(st.pos) - pole).dot(d) < 0.0, "drove through the pole");
    }
    assert!(hit, "the pole at {pole:?} never stopped the car");
}

/// Drives `car` along `path` (forward, or backing up with `reverse`) at walking pace until
/// within `stop` m of its last point, steering at the point 3 m ahead of it. Returns
/// whether it got there; fails on any contact with a wall, post or box.
fn follow(
    car: &VehicleSpec,
    s: &mut VehicleState,
    c: &WorldCollider,
    path: &[V2],
    reverse: bool,
    stop: f32,
) -> bool {
    let last = path[path.len() - 1];
    let mut k = 0;
    for _ in 0..40 * STEPS_PER_S {
        let p = xz(s.pos);
        if (p - last).length() < stop {
            // Brake to a stop.
            for _ in 0..2 * STEPS_PER_S {
                let inp = if s.speed_mps.abs() > 0.2 {
                    VehicleInput {
                        brake: if reverse { 0.0 } else { 1.0 },
                        throttle: if reverse { 1.0 } else { 0.0 },
                        ..VehicleInput::default()
                    }
                } else {
                    VehicleInput::default()
                };
                let inp = if reverse && s.speed_mps < -0.2 {
                    VehicleInput {
                        throttle: 0.5,
                        ..VehicleInput::default()
                    }
                } else {
                    inp
                };
                step(car, s, &inp, &Assists::default(), c);
                let impact = resolve(s, car, c);
                assert!(
                    !impact.contact,
                    "touched something stopping at {:?}",
                    xz(s.pos)
                );
            }
            return true;
        }
        while k + 1 < path.len() && (path[k] - p).length() < 3.0 {
            k += 1;
        }
        let target = path[k];
        let f = forward(s);
        let to = (target - p).normalize();
        // Positive steers right; backing up, the tail leads.
        let lead = if reverse { -f } else { f };
        let side = lead.right().dot(to);
        let steer = (side * 2.5).clamp(-1.0, 1.0) * if reverse { -1.0 } else { 1.0 };
        let speed = s.speed_mps.abs();
        let want = 2.5;
        let inp = if reverse {
            // Held at a standstill the brake selects reverse and becomes the throttle.
            VehicleInput {
                brake: if speed < want { 0.4 } else { 0.0 },
                throttle: if speed > want + 0.5 { 0.3 } else { 0.0 },
                steer,
                handbrake: false,
            }
        } else {
            VehicleInput {
                throttle: if speed < want { 0.35 } else { 0.0 },
                brake: if speed > want + 0.5 { 0.3 } else { 0.0 },
                steer,
                handbrake: false,
            }
        };
        step(car, s, &inp, &Assists::default(), c);
        let impact = resolve(s, car, c);
        assert!(
            !impact.contact,
            "touched something at {:?} heading for {target:?}",
            xz(s.pos)
        );
    }
    false
}

#[test]
fn a_car_drives_into_a_lot_parks_and_leaves() {
    let (place, net, lots) = coast(1);
    let mut c = WorldCollider::build(&place, &grammar());
    let car = spec();
    // Before dawn: the shops' lots stand nearly empty.
    let far = VehicleState::spawn(&car, [0.0, 0.0, 0.0], 0.0);
    c.advance_lot_cars(4.0 * 3600.0, [0.0; 3], [0.0, 0.0, -1.0], &far, &car);
    let lot = lots
        .iter()
        .find(|l| !l.on_alley && !l.aisles.is_empty())
        .expect("a front lot on seed 1");
    let cut = lot.cuts[0];
    let entry = lot.entries[0];
    let aisle = lot.aisles[0];
    // In the near lane in front of the cut, facing in.
    let street = (entry[0] + entry[1]) * 0.5;
    let inward = ((entry[2] + entry[3]) * 0.5 - street).normalize();
    let (end, _) = net.ends(cut.edge as usize);
    let start = street - inward * (end.sw + end.hw * 0.5);
    // Into the aisle, along it to a stall whose neighbours are free now, into the stall.
    let in_aisle = |p: V2| point_in_polygon(p, &aisle);
    let aisle_mid = (aisle[0] + aisle[1] + aisle[2] + aisle[3]) * 0.25;
    let busy = |q: V2| {
        let lot_car = |l: &world::collide::LotCar| {
            l.present && (V2::new(l.at[0], l.at[2]) - q).length() < 4.0
        };
        let curb_car = |p: &place::Prop| {
            matches!(p.kind, PropKind::ParkedCar { hours, .. } if hours == place::ALL_HOURS)
                && (V2::new(p.pos[0], p.pos[2]) - q).length() < 4.0
        };
        c.lot_cars.iter().any(lot_car) || place.props.items.iter().any(curb_car)
    };
    let stall = lot
        .stalls
        .iter()
        .filter(|s| in_aisle(s.mouth) && !busy(s.c))
        .max_by(|a, b| {
            let (da, db) = ((a.c - street).length(), (b.c - street).length());
            da.total_cmp(&db)
        })
        .expect("a free stall");
    let drive_in = |from: V2| {
        let a = (entry[2] + entry[3]) * 0.5;
        let along = (stall.mouth - aisle_mid).normalize();
        let turn = stall.mouth - stall.nose * 2.0 - along * 3.0;
        vec![from, street, a, aisle_mid, turn, stall.mouth, stall.c]
    };
    let y = c.level_at([start.x, start.y]).0;
    let mut s = VehicleState::spawn(&car, [start.x, y, start.y], heading_of(inward));
    let path = drive_in(start);
    assert!(
        follow(&car, &mut s, &c, &path, false, 1.2),
        "never reached the stall"
    );
    let parked_at = xz(s.pos);
    assert!(
        (parked_at - stall.c).length() < 1.5,
        "parked at {parked_at:?}, stall {:?}",
        stall.c
    );
    assert!(forward(&s).dot(stall.nose) > 0.8, "not nosed in");
    // Back out into the aisle, then out the way it came.
    let back = vec![stall.mouth, stall.mouth - stall.nose * 2.5];
    assert!(
        follow(&car, &mut s, &c, &back, true, 1.0),
        "never backed out"
    );
    let out = vec![aisle_mid, (entry[2] + entry[3]) * 0.5, street, start];
    assert!(
        follow(&car, &mut s, &c, &out, false, 2.0),
        "never got back to the street"
    );
    println!(
        "parked at {parked_at:?} in the stall at {:?}; back on the street at {:?}",
        stall.c,
        xz(s.pos)
    );
}

/// The corners of box `b`, on the ground.
fn corners(b: &world::collide::BoxSolid) -> [V2; 4] {
    let u = V2::new(b.u[0], b.u[1]);
    let v = u.left();
    let c = V2::new(b.c[0], b.c[1]);
    let (a, w) = (u * b.half[0], v * b.half[1]);
    [c - a - w, c + a - w, c + a + w, c - a + w]
}

#[test]
fn lot_cars_never_overlap_cars_or_poles() {
    for seed in 1..=8 {
        let (place, _, _) = coast(seed);
        let c = WorldCollider::build(&place, &grammar());
        let cars: Vec<[V2; 4]> = c
            .boxes
            .iter()
            .filter(|b| matches!(b.kind, world::props::PropKind::ParkedCar { .. }))
            .map(|b| shrink(&corners(b), 0.05))
            .collect();
        let lot: Vec<[V2; 4]> = c
            .lot_cars
            .iter()
            .map(|l| shrink(&corners(&c.boxes[l.solid as usize]), 0.05))
            .collect();
        for q in &lot {
            let bb = bounds(q);
            let others = cars
                .iter()
                .filter(|o| *o != q && boxes_meet(bb, bounds(*o), 0.0) && overlap(q, *o))
                .count();
            assert_eq!(
                others, 0,
                "seed {seed}: lot car at {:?} overlaps a car",
                q[0]
            );
            for p in &c.posts {
                let at = V2::new(p.c[0], p.c[1]);
                assert!(
                    !point_in_polygon(at, q),
                    "seed {seed}: a post stands in the lot car at {:?}",
                    q[0]
                );
            }
        }
    }
}

/// Seconds since midnight of hour `h`.
fn at_hour(h: f32) -> f32 {
    h * 3600.0
}

#[test]
fn lot_cars_never_come_or_go_in_view() {
    let (place, _, lots) = coast(1);
    let mut c = WorldCollider::build(&place, &grammar());
    let car = spec();
    let far = VehicleState::spawn(&car, [0.0, 0.0, 0.0], 0.0);
    // The busiest lot, driven past for a game hour of the morning fill (8 to 9 am).
    let lot = lots.iter().max_by_key(|l| l.stalls.len()).unwrap();
    let a = lot.stalls[0].c;
    let b = lot.stalls[lot.stalls.len() - 1].c;
    let dir = (b - a).normalize();
    let (from, to) = (a - dir * 120.0, b + dir * 120.0);
    c.advance_lot_cars(
        at_hour(8.0),
        [from.x, 2.0, from.y],
        [dir.x, 0.0, dir.y],
        &far,
        &car,
    );
    let steps = 3600;
    let (mut seen, mut unseen) = (0, 0);
    for k in 0..=steps {
        let f = k as f32 / steps as f32;
        // Back and forth along the lot, looking ahead.
        let (p, d) = if f < 0.5 {
            (from + (to - from) * (f * 2.0), dir)
        } else {
            (to + (from - to) * ((f - 0.5) * 2.0), -dir)
        };
        let eye = [p.x, 2.0, p.y];
        let before: Vec<bool> = c.lot_cars.iter().map(|l| l.present).collect();
        c.advance_lot_cars(at_hour(8.0 + f), eye, [d.x, 0.0, d.y], &far, &car);
        for (l, &was) in c.lot_cars.iter().zip(&before) {
            if l.present == was {
                continue;
            }
            let to_car = V2::new(l.at[0], l.at[2]) - p;
            let dist = to_car.length();
            if dist < world::collide::HOLD_M && to_car.dot(d) > world::collide::HOLD_COS * dist {
                seen += 1;
            } else {
                unseen += 1;
            }
        }
    }
    println!("lot cars came or went {unseen} times out of sight, {seen} in sight");
    assert_eq!(seen, 0);
    assert!(unseen > 0, "the lot never changed");
}

#[test]
fn a_lot_car_never_arrives_on_the_player() {
    let (place, _, _) = coast(1);
    let mut c = WorldCollider::build(&place, &grammar());
    let car = spec();
    // A lot car that arrives during the morning: its hour after an hour it is away.
    let (k, h) = c
        .lot_cars
        .iter()
        .enumerate()
        .find_map(|(k, l)| {
            (6..11)
                .find(|&h| {
                    l.hours >> (h - 1) & 1 == 0
                        && l.hours >> h & 1 == 1
                        && l.hours >> (h + 1) & 1 == 1
                })
                .map(|h| (k, h))
        })
        .expect("a lot car arriving in the morning");
    let at = c.lot_cars[k].at;
    let far = VehicleState::spawn(&car, [0.0, 0.0, 0.0], 0.0);
    let away = [at[0] + 500.0, 2.0, at[2] + 500.0];
    let look = [1.0, 0.0, 0.0];
    c.advance_lot_cars(at_hour(h as f32 - 1.0), away, look, &far, &car);
    assert!(!c.lot_cars[k].present);
    // The player parks in its stall, then the hour comes (seen from far off).
    let mut s = VehicleState::spawn(&car, at, 0.0);
    for _ in 0..STEPS_PER_S {
        step(
            &car,
            &mut s,
            &VehicleInput::default(),
            &Assists::default(),
            &c,
        );
        resolve(&mut s, &car, &c);
    }
    let parked = xz(s.pos);
    c.advance_lot_cars(at_hour(h as f32 + 1.0), away, look, &s, &car);
    assert!(!c.lot_cars[k].present, "a car arrived on the player");
    for _ in 0..STEPS_PER_S {
        step(
            &car,
            &mut s,
            &VehicleInput::default(),
            &Assists::default(),
            &c,
        );
        let impact = resolve(&mut s, &car, &c);
        assert!(!impact.contact, "shoved at {:?}", xz(s.pos));
    }
    assert!((xz(s.pos) - parked).length() < 0.3, "moved off the stall");
    // Once the player has gone, it arrives.
    c.advance_lot_cars(at_hour(h as f32 + 1.0), away, look, &far, &car);
    assert!(c.lot_cars[k].present);
}
