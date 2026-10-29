//! The basin's set pieces (`docs/WORLD_GENERATION.md`, "Set pieces") end to end: every one is
//! sited on seeds 1-4, clear of the roads and buildings around it, and its solids hold a
//! car as they should: the tunnel's walls keep a car on the freeway and let it drive
//! through, the garage's middle bay takes a car and its walls stop one.
//! `cargo test -p world --release --test setpieces -- --nocapture` prints where each went.

use place::{Place, PoiKind, PropKind};
use skywave_core::Seed;
use vehicle::{step, Assists, VehicleInput, VehicleSpec, VehicleState};
use world::math::{segment_distance, V2};
use world::procgen::build_place;
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

/// The grammar with the landmark kits the game attaches.
fn grammar_with_kits() -> world::Grammar {
    let mut g = grammar();
    g.landmarks = [CITY_HALL, MOVIE_PALACE, WATER_TOWER]
        .iter()
        .map(|t| {
            let k: world::landmarks::Kit = ron::from_str(t).expect("parse kit");
            k.validate().expect("valid kit");
            k
        })
        .collect();
    g
}

const CITY_HALL: &str = include_str!("../../../assets/landmarks/city_hall.ron");
const MOVIE_PALACE: &str = include_str!("../../../assets/landmarks/movie_palace.ron");
const WATER_TOWER: &str = include_str!("../../../assets/landmarks/water_tower.ron");

#[test]
fn every_basin_has_its_downtown_anchors() {
    use world::procgen::setpieces::{HALL_KIT, HALL_LOT_M, PALACE_KIT, PALACE_LOT_M};
    let g = grammar_with_kits();
    let kit = |id: &str| g.landmarks.iter().position(|k| k.id == id).unwrap();
    let (hall, palace) = (kit(HALL_KIT), kit(PALACE_KIT));
    // The sites are sized to the kits' own lots.
    assert_eq!(g.landmarks[hall].lot, [HALL_LOT_M.0, HALL_LOT_M.1]);
    assert_eq!(g.landmarks[palace].lot, [PALACE_LOT_M.0, PALACE_LOT_M.1]);
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    for seed in 1..=4u64 {
        let p = build_place(Seed(seed), &t, &g);
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        assert_eq!(pois(&p, PoiKind::CityHall), 1, "seed {seed}");
        assert_eq!(pois(&p, PoiKind::MoviePalace), 1, "seed {seed}");
        let m = world::procgen::MacroMap::generate(Seed(seed), &t, world::procgen::PLACE_CHUNKS);
        let b = m.basin.as_ref().unwrap();
        for (k, (w, d)) in [(hall, HALL_LOT_M), (palace, PALACE_LOT_M)] {
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
    let g = grammar_with_kits();
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    let p = build_place(Seed(1), &t, &g);
    let c = WorldCollider::build(&p, &g);
    let q = p
        .props
        .items
        .iter()
        .find(|q| matches!(q.kind, PropKind::Landmark { .. }) && at(q).y < 500.0)
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
