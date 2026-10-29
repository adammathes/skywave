//! Stage 7, detailing ([`world::procgen::detail`]): the props and lights of a procedural
//! place. Run with `--nocapture` to see the counts: `cargo test -p world --test detail --
//! --nocapture`.

use std::collections::BTreeMap;
use std::time::Instant;

use place::{LightKind, Place, PropKind, RoadClass, ZoneId};
use skywave_core::Seed;
use world::math::{point_in_polygon, segment_distance, V2};
use world::procgen::detail::{MAX_CARS_PER_CHUNK, MAX_LOT_CARS_PER_CHUNK};
use world::procgen::{build_place, hash_detail};
use world::props::{dir_of_yaw, PIER_HEAD_HALF_WIDTH_M, PIER_HEAD_M};
use world::roads::Network;
use world::template::RegionTemplate;
use world::WorldCollider;

const GRAMMAR: &str = include_str!("../../../assets/grammar/california_coast_80s.ron");

fn grammar() -> world::Grammar {
    ron::from_str(GRAMMAR).expect("parse grammar")
}

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
    t.validate().expect("validate");
    t
}

fn name(k: PropKind) -> &'static str {
    match k {
        PropKind::StreetLight => "street light",
        PropKind::PalmTree => "palm",
        PropKind::Oak => "oak",
        PropKind::Hedge { .. } => "hedge",
        PropKind::Bench => "bench",
        PropKind::Hydrant => "hydrant",
        PropKind::Mailbox => "mailbox",
        PropKind::Sign { .. } => "street sign",
        PropKind::NeonSign { .. } => "neon sign",
        PropKind::Billboard => "billboard",
        PropKind::PowerPole => "power pole",
        PropKind::Fence { .. } => "fence",
        PropKind::ParkedCar { .. } => "parked car",
        PropKind::Eucalyptus => "eucalyptus",
        PropKind::TrashCan => "trash can",
        PropKind::StopSign => "stop sign",
        PropKind::Signal { .. } => "signal",
        PropKind::Pier { .. } => "pier",
        PropKind::Track { .. } => "track",
        PropKind::Crossbuck => "crossbuck",
        PropKind::FaceBillboard { .. } => "face billboard",
        PropKind::BusShelter { .. } => "bus shelter",
        PropKind::TunnelPiece { .. } => "tunnel",
        PropKind::StartTree => "start tree",
        PropKind::TimingBoard => "timing board",
        PropKind::StripLine { .. } => "strip line",
        PropKind::GuardRail { .. } => "guard rail",
        PropKind::ChainLink { .. } => "chain-link",
        PropKind::CrewGarage { .. } => "crew garage",
        PropKind::TacoStand { .. } => "taco stand",
        PropKind::HeroCar { .. } => "hero car",
        PropKind::PicnicTable => "picnic table",
        PropKind::CrossingGate { .. } => "crossing gate",
        PropKind::Landmark { .. } => "landmark",
        PropKind::LotLabel { .. } => "lot label",
    }
}

fn is_tree(k: PropKind) -> bool {
    matches!(k, PropKind::PalmTree | PropKind::Oak | PropKind::Eucalyptus)
}

fn zone_at(p: &Place, q: [f32; 2]) -> ZoneId {
    let h = &p.header;
    let [w, d] = place::Zones::cells_per_side(h, p.terrain.res_per_chunk);
    let cell = h.chunk_size_m / f32::from(p.terrain.res_per_chunk);
    let o = h.origin_m();
    let x = (((q[0] - o[0]) / cell).max(0.0) as usize).min(w - 1);
    let z = (((q[1] - o[1]) / cell).max(0.0) as usize).min(d - 1);
    p.zones.cells[z * w + x]
}

fn ground_point(x: &place::Prop) -> V2 {
    V2::new(x.pos[0], x.pos[2])
}

/// Whether `q` lies on the pier (its deck and head).
fn on_pier(p: &Place, q: V2) -> bool {
    p.props.items.iter().any(|x| {
        let PropKind::Pier { len_m } = x.kind else {
            return false;
        };
        let at = ground_point(x);
        let end = at + dir_of_yaw(x.yaw) * (f32::from(len_m) + PIER_HEAD_M);
        segment_distance(q, at, end).0 < PIER_HEAD_HALF_WIDTH_M + 0.5
    })
}

/// Every road's centerline segments with its half width and the outer edge of its walk.
fn road_segments(p: &Place, net: &Network) -> Vec<(V2, V2, f32, f32, usize)> {
    let mut out = Vec::new();
    for (i, e) in p.roads.edges.iter().enumerate() {
        let hw = e.width_m * 0.5;
        let walk = hw
            + if e.sidewalk {
                place::SIDEWALK_WIDTH_M
            } else {
                0.2
            };
        for w in net.paths[i].pts.windows(2) {
            out.push((w[0], w[1], hw, walk, i));
        }
    }
    out
}

fn footprints(p: &Place) -> Vec<Vec<V2>> {
    p.buildings
        .iter()
        .map(|b| b.footprint.iter().map(|&q| V2::from_array(q)).collect())
        .collect()
}

fn polygon_distance(q: V2, poly: &[V2]) -> f32 {
    if point_in_polygon(q, poly) {
        return 0.0;
    }
    let n = poly.len();
    (0..n)
        .map(|i| segment_distance(q, poly[i], poly[(i + 1) % n]).0)
        .fold(f32::MAX, f32::min)
}

/// Seed 1's props, counted by kind and by zone: each zone gets its own furniture.
#[test]
fn seed_one_props_by_zone() {
    let t = template();
    let t0 = Instant::now();
    let p = build_place(Seed(1), &t, &grammar());
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_zone: BTreeMap<(String, &str), usize> = BTreeMap::new();
    for x in &p.props.items {
        *by_kind.entry(name(x.kind)).or_default() += 1;
        let z = zone_at(&p, [x.pos[0], x.pos[2]]);
        *by_zone.entry((format!("{z:?}"), name(x.kind))).or_default() += 1;
    }
    let mut lights: BTreeMap<String, usize> = BTreeMap::new();
    for l in &p.lights.items {
        *lights.entry(format!("{:?}", l.kind)).or_default() += 1;
    }
    println!(
        "seed 1: {} props, {} lights; the whole place built in {ms:.0} ms",
        p.props.items.len(),
        p.lights.items.len()
    );
    println!("by kind: {by_kind:?}");
    println!("lights: {lights:?}");
    let mut zone = String::new();
    for ((z, k), n) in &by_zone {
        if *z != zone {
            print!("\n{z}:");
            zone = z.clone();
        }
        print!(" {k} {n},");
    }
    println!();
    let kind = |k: &str| by_kind.get(k).copied().unwrap_or(0);
    let zoned = |z: &str, k: &str| by_zone.get(&(z.to_string(), k)).copied().unwrap_or(0);
    let check = |what: &str, n: usize, lo: usize, hi: usize| {
        assert!((lo..=hi).contains(&n), "{what}: {n} not in {lo}..={hi}");
    };
    check("street lights", kind("street light"), 800, 2500);
    check("parked cars", kind("parked car"), 800, 4000);
    check("billboards", kind("billboard"), 3, 40);
    check("piers", kind("pier"), 1, 1);
    check("strip lights", zoned("Strip", "street light"), 50, 400);
    check("strip palms", zoned("Strip", "palm"), 30, 400);
    check("strip neon", zoned("Strip", "neon sign"), 10, 200);
    check("strip benches", zoned("Strip", "bench"), 10, 200);
    check("downtown signals", zoned("Downtown", "signal"), 12, 300);
    check("residential oaks", zoned("Residential", "oak"), 200, 3000);
    check(
        "residential cars",
        zoned("Residential", "parked car"),
        300,
        3000,
    );
    check(
        "residential hydrants",
        zoned("Residential", "hydrant"),
        100,
        1000,
    );
    check("hill gums", zoned("Hills", "eucalyptus"), 400, 5000);
    check(
        "suburb lot lines",
        zoned("Suburb", "fence") + zoned("Suburb", "hedge"),
        200,
        3000,
    );
    check("suburb mailboxes", zoned("Suburb", "mailbox"), 40, 1000);
    check("farm poles", zoned("Farmland", "power pole"), 5, 500);
    check("beach palms", zoned("Beach", "palm"), 10, 500);
}

/// Props stand on the ground the collider stands on (a walk's top, the graded terrain,
/// the asphalt for a curbside car), never on a road's asphalt (but parked cars in their
/// lane), clear of buildings; trees keep 1 m off walls and walks unless planted at the
/// curb; neon signs hang on walls; the collider has a shape for every solid prop.
#[test]
fn props_stand_on_the_ground_clear_of_roads_and_buildings() {
    let t = template();
    for seed in 1..=2u64 {
        let p = build_place(Seed(seed), &t, &grammar());
        let net = Network::new(&p);
        let c = WorldCollider::build(&p, &grammar());
        let segs = road_segments(&p, &net);
        let fps = footprints(&p);
        let (mut grounded, mut worst) = (0usize, 0.0f32);
        let mut curbside = 0usize;
        for x in &p.props.items {
            let q = ground_point(x);
            // Per nearby road: its distance, half width, walk edge and index.
            let mut near: Vec<(f32, f32, f32, usize)> = Vec::new();
            for sg in segs.iter().filter(|s| (s.0 - q).length() < 200.0) {
                let d = segment_distance(q, sg.0, sg.1).0;
                match near.iter_mut().find(|n| n.3 == sg.4) {
                    Some(n) => n.0 = n.0.min(d),
                    None => near.push((d, sg.2, sg.3, sg.4)),
                }
            }
            let to_building = fps
                .iter()
                .map(|f| polygon_distance(q, f))
                .fold(f32::MAX, f32::min);
            let what = format!("seed {seed}: {} at {:?}", name(x.kind), x.pos);
            if matches!(x.kind, PropKind::Pier { .. }) || on_pier(&p, q) {
                continue;
            }
            if let PropKind::ParkedCar { .. } = x.kind {
                assert!(to_building > 0.3, "{what} in a building");
                // In a curb lane at the asphalt's height, or off every road.
                let lane = near.iter().find(|n| n.0 > n.1 - 2.0 && n.0 < n.1 - 0.5);
                if let Some(&(_, _, _, e)) = lane {
                    let path = &net.paths[e];
                    let y = world::roads::profile_height(&path.raw, &path.raw_y, q);
                    if (y - x.pos[1]).abs() < 0.1 {
                        curbside += 1;
                        continue;
                    }
                }
                assert!(near.iter().all(|n| n.0 >= n.1 + 0.5), "{what} on a road");
                continue;
            }
            // Off the asphalt.
            assert!(
                near.iter().all(|n| n.0 >= n.1 + 0.1),
                "{what} on a road: {:?}",
                near.iter().map(|n| n.0 - n.1).fold(f32::MAX, f32::min)
            );
            if let PropKind::NeonSign { .. } = x.kind {
                assert!(to_building < 0.1, "{what} {to_building} m off a wall");
                continue;
            }
            assert!(to_building >= 0.2, "{what} in a building");
            if is_tree(x.kind) {
                assert!(to_building >= 1.0, "{what} {to_building} m from a wall");
                // Planted at the curb (a parkway or a well), or a meter past every walk.
                let bad = near
                    .iter()
                    .find(|n| !(n.0 <= n.1 + 1.25 || n.0 >= n.2 + 1.0 || n.2 <= n.1 + 0.25));
                assert!(bad.is_none(), "{what} by a walk: {bad:?}");
            }
            // The surface under it (not a deck above it).
            let (g, _) = c.level_from([q.x, q.y], x.pos[1] + 1.5);
            worst = worst.max((g - x.pos[1]).abs());
            grounded += 1;
            assert!(
                (g - x.pos[1]).abs() <= 0.1,
                "{what} stands at {} over ground at {g}",
                x.pos[1]
            );
        }
        println!(
            "seed {seed}: {grounded} props on the ground (worst {worst:.3} m off), {curbside} \
             cars at the curb"
        );
        // The collider: a post at every lamp and tree, a box at every car.
        let posts = |k: fn(PropKind) -> bool| {
            p.props
                .items
                .iter()
                .filter(|x| k(x.kind))
                .filter(|x| {
                    let q = [x.pos[0], x.pos[2]];
                    !c.posts
                        .iter()
                        .any(|s| (s.c[0] - q[0]).abs() < 1e-3 && (s.c[1] - q[1]).abs() < 1e-3)
                })
                .count()
        };
        assert_eq!(posts(|k| k == PropKind::StreetLight || is_tree(k)), 0);
        let cars = p
            .props
            .items
            .iter()
            .filter(|x| matches!(x.kind, PropKind::ParkedCar { .. }))
            .count();
        assert!(
            c.boxes.len() >= cars,
            "{} boxes for {cars} cars",
            c.boxes.len()
        );
    }
}

/// At most [`MAX_CARS_PER_CHUNK`] parked cars at the curbs and in driveways in any chunk,
/// and [`MAX_LOT_CARS_PER_CHUNK`] in its lots at any hour.
#[test]
fn parked_cars_keep_to_the_chunk_cap() {
    let t = template();
    for seed in 1..=4u64 {
        let p = build_place(Seed(seed), &t, &grammar());
        let mut per: BTreeMap<[u16; 2], (usize, [usize; 24])> = BTreeMap::new();
        for x in &p.props.items {
            if let PropKind::ParkedCar { hours, .. } = x.kind {
                let c = p
                    .header
                    .chunk_at([x.pos[0], x.pos[2]])
                    .expect("in the place");
                let e = per.entry(c).or_default();
                if hours == place::ALL_HOURS {
                    e.0 += 1;
                } else {
                    for (h, n) in e.1.iter_mut().enumerate() {
                        *n += (hours >> h & 1) as usize;
                    }
                }
            }
        }
        let most = per.values().map(|e| e.0).max().unwrap_or(0);
        let most_lot = per
            .values()
            .map(|e| e.1.iter().copied().max().unwrap_or(0))
            .max()
            .unwrap_or(0);
        println!(
            "seed {seed}: {} chunks with cars, at most {most} at the curbs, {most_lot} in lots",
            per.len()
        );
        assert!(
            most <= MAX_CARS_PER_CHUNK && most > 0,
            "seed {seed}: {most}"
        );
        assert!(
            most_lot <= MAX_LOT_CARS_PER_CHUNK,
            "seed {seed}: {most_lot}"
        );
    }
}

/// One sodium light in the head of every street lamp; neon lights at signs, a fill in
/// front of lit shops; nothing else.
#[test]
fn lights_are_the_lamps_signs_and_shops() {
    let p = build_place(Seed(1), &template(), &grammar());
    let lamps: Vec<&place::Prop> = p
        .props
        .items
        .iter()
        .filter(|x| x.kind == PropKind::StreetLight)
        .collect();
    let street: Vec<&place::Light> = p
        .lights
        .items
        .iter()
        .filter(|l| l.kind == LightKind::Street)
        .collect();
    assert!(!lamps.is_empty());
    assert_eq!(street.len(), lamps.len(), "one light per lamp");
    for l in &street {
        // Over a lamp, 7.26 m up and 2.175 m out along its arm.
        let ok = lamps.iter().any(|x| {
            let head = ground_point(x) + dir_of_yaw(x.yaw) * 2.175;
            (head.x - l.pos[0]).abs() < 1e-3
                && (head.y - l.pos[2]).abs() < 1e-3
                && (x.pos[1] + 7.26 - l.pos[1]).abs() < 1e-3
        });
        assert!(ok, "street light at {:?} under no lamp head", l.pos);
        assert!((l.radius - 22.0).abs() < 1e-3);
    }
    let fps = footprints(&p);
    let signs: Vec<V2> = p
        .props
        .items
        .iter()
        .filter(|x| matches!(x.kind, PropKind::NeonSign { .. }))
        .map(ground_point)
        .collect();
    let (mut neon, mut window) = (0, 0);
    for l in &p.lights.items {
        let q = V2::new(l.pos[0], l.pos[2]);
        let wall = fps
            .iter()
            .map(|f| polygon_distance(q, f))
            .fold(f32::MAX, f32::min);
        match l.kind {
            LightKind::Neon => {
                neon += 1;
                let by_sign = signs.iter().any(|s| (*s - q).length() < 2.0);
                // 1.2 m out from its sign, which on a strip mall's mansard canopy stands up
                // to 3 m out from the wall.
                assert!(
                    wall < 4.5 || by_sign,
                    "neon light at {:?} by no sign",
                    l.pos
                );
            }
            LightKind::Window => {
                window += 1;
                assert!(wall < 3.5, "shop light at {:?} by no shop", l.pos);
            }
            LightKind::Street => {}
            LightKind::Headlight => panic!("headlights belong to cars"),
            LightKind::Flood => panic!("the coast has no set pieces"),
        }
    }
    println!("{} lamps, {neon} neon, {window} shop lights", street.len());
    assert!(neon > 50 && window > 20, "{neon} neon, {window} shops");
}

/// Pinned `hash_detail` of seeds 1..=4.
///
/// Update rule: these move whenever the detailing (or a stage before it) changes. Regenerate
/// with `cargo test -p world --test detail -- --nocapture` (it prints the table), check the
/// coast screenshots, and commit the new values in their own commit saying why they moved.
const GOLDEN: [u64; 4] = [
    0x6561_81E5_D1E3_0295,
    0xCE47_D6E4_EE0F_2B6E,
    0x6C71_F635_B201_A595,
    0x288F_019C_D8DF_4A9A,
];

/// The detail is deterministic, valid and pinned.
#[test]
fn detail_is_deterministic_valid_and_pinned() {
    let t = template();
    let mut got = Vec::new();
    for seed in 1..=4u64 {
        let p = build_place(Seed(seed), &t, &grammar());
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        assert!(!p.props.items.is_empty() && !p.lights.items.is_empty());
        got.push(hash_detail(&p));
    }
    assert_eq!(hash_detail(&build_place(Seed(1), &t, &grammar())), got[0]);
    let table = got
        .iter()
        .map(|h| {
            let l = format!("0x{h:016X}");
            format!("{}_{}_{}_{}", &l[..6], &l[6..10], &l[10..14], &l[14..18])
        })
        .collect::<Vec<_>>()
        .join(", ");
    println!("detail hashes: [{table}]");
    assert_eq!(
        got, GOLDEN,
        "detail hashes moved; see the update rule on GOLDEN"
    );
}

/// The roads the detailing uses are the ones the meshes use (the zone helper agrees with
/// the place's own lookup on a road's side).
#[test]
fn rural_and_hill_roads_are_dark_but_their_junctions() {
    let p = build_place(Seed(1), &template(), &grammar());
    let net = Network::new(&p);
    let segs = road_segments(&p, &net);
    let mut lone = 0;
    for x in p
        .props
        .items
        .iter()
        .filter(|x| x.kind == PropKind::StreetLight)
    {
        let q = ground_point(x);
        if on_pier(&p, q) {
            continue;
        }
        let nearest = segs
            .iter()
            .min_by(|a, b| {
                segment_distance(q, a.0, a.1)
                    .0
                    .total_cmp(&segment_distance(q, b.0, b.1).0)
            })
            .expect("roads");
        let class = p.roads.edges[nearest.4].class;
        if class == RoadClass::Rural || class == RoadClass::Highway {
            // Only at a junction: within 30 m of a node with three roads or more.
            let at_junction = net
                .junctions
                .iter()
                .any(|j| j.ends.len() >= 3 && (j.pos - q).length() < 30.0);
            assert!(at_junction, "a lamp along a {class:?} road at {:?}", x.pos);
            lone += 1;
        }
    }
    println!("{lone} lamps at rural junctions");
}
