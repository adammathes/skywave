//! The inland basin (`socal_basin`, `docs/WORLD_GENERATION.md`, "Inland basins") end to
//! end: its template loads, its places validate and are deterministic for seeds 1-4, and
//! they hold what the template promises: the grid and its boulevards, the freeway with its
//! overpasses and interchanges (a loop on-ramp among them) turning back along the desert,
//! the channel bridged by every road over it, the rail line at grade with the roads it
//! crosses, the airfield, the hills, every road inside its edges. Run with `--nocapture`
//! for the numbers: `cargo test -p world --release --test basin -- --nocapture`.

use std::time::Instant;

use place::{EdgeKind, RegionId, RoadClass, SpawnKind, WaterKind, ZoneId};
use skywave_core::Seed;
use world::math::{segment_distance, V2};

use world::procgen::{arterials, build_place, hash_graph, MacroMap, PLACE_CHUNKS};
use world::template::RegionTemplate;

const GRAMMAR: &str = include_str!("../../../assets/grammar/socal_basin_2001.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon_2001.ron");
const BASIN: &str = include_str!("../../../assets/regions/socal_basin.ron");

fn grammar() -> world::Grammar {
    let mut g: world::Grammar = ron::from_str(GRAMMAR).expect("parse grammar");
    g.lexicon = ron::from_str(LEXICON).expect("parse lexicon");
    g.validate().expect("validate grammar");
    g
}

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(BASIN).expect("parse");
    t.validate().expect("validate");
    t
}

/// Pinned hashes of seeds 1..=4: `MacroMap::hash`, the arterials' `hash_graph` and the
/// Place's `content_hash`.
///
/// Update rule: as for the coast's (`tests/place.rs`): these move whenever a stage the
/// basin runs through changes. Regenerate with `cargo test -p world --release --test basin
/// -- --nocapture` (it prints the table), look at the basin's screenshots, and commit the
/// new values in their own commit whose message says why they moved.
const GOLDEN: [(u64, u64, u64); 4] = [
    (
        0x39A1_C053_F7A8_0B31,
        0xA2D7_B9A0_3D09_A4FE,
        0xE55D_4EEC_42A0_4F70,
    ),
    (
        0xA3F4_A281_380E_596B,
        0x7AD4_5617_94BE_609B,
        0xB984_1F40_4C01_1645,
    ),
    (
        0x82B6_07FD_D1FD_CF4F,
        0xB348_0BC3_E55C_8454,
        0x9595_79BC_FF11_CD4C,
    ),
    (
        0x6DBB_5CB8_4DBB_0036,
        0x68EB_95E2_5702_79E7,
        0xAD5F_6466_8F46_B5D3,
    ),
];

#[test]
fn the_template_loads_and_is_a_basin() {
    let t = template();
    assert_eq!(t.name, "socal_basin");
    assert!(t.coast.is_none() && t.elevation.is_none() && t.layout.is_none());
    let b = t.basin.as_ref().unwrap();
    assert_eq!(b.grid.spacing_m, 400.0);
    assert_eq!(b.grid.boulevard_lanes, 3);
    assert!(b.channel.depth_m < place::validate::PROFILE_TERRAIN_TOLERANCE_M);
    assert_eq!(
        t.edges.sides(),
        [
            EdgeKind::Hills,
            EdgeKind::Desert,
            EdgeKind::RailYards,
            EdgeKind::Mountains
        ]
    );
    // A basin with a coast, or without its hills or desert edge, is refused.
    let mut bad = t.clone();
    bad.edges.north = EdgeKind::Mountains;
    assert!(bad.validate().is_err());
    let mut bad = t.clone();
    bad.edges.east = EdgeKind::Hills;
    assert!(bad.validate().is_err());
    let mut bad = t.clone();
    bad.basin.as_mut().unwrap().channel.depth_m = 9.0;
    assert!(bad.validate().is_err());
    let coast: RegionTemplate =
        ron::from_str(include_str!("../../../assets/regions/california_coast.ron")).unwrap();
    let mut both = t.clone();
    both.coast = coast.coast;
    assert!(both.validate().is_err());
}

#[test]
fn basins_validate_and_are_deterministic() {
    let t = template();
    let g = grammar();
    let mut got = Vec::new();
    for seed in 1..=4u64 {
        let start = Instant::now();
        let m = MacroMap::generate(Seed(seed), &t, PLACE_CHUNKS);
        let roads = arterials(Seed(seed), &t, &m);
        let p = build_place(Seed(seed), &t, &g);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        assert_eq!(p.header.region, RegionId::SocalBasin);
        assert!(!p.buildings.is_empty() && !p.parcels.is_empty());
        let s = p.player_start().expect("a PlayerStart");
        assert_eq!(s.kind, SpawnKind::PlayerStart);
        println!(
            "seed {seed}: {} nodes, {} edges, {} buildings, {} props, start ({:.0}, {:.0}), {ms:.0} ms",
            p.roads.nodes.len(),
            p.roads.edges.len(),
            p.buildings.len(),
            p.props.items.len(),
            s.pos[0],
            s.pos[2],
        );
        got.push((m.hash(), hash_graph(&roads), p.content_hash()));
    }
    let again = build_place(Seed(1), &t, &g).content_hash();
    assert_eq!(again, got[0].2);
    let table = got
        .iter()
        .map(|(a, b, c)| format!("    (0x{a:016X}, 0x{b:016X}, 0x{c:016X}),"))
        .collect::<Vec<_>>()
        .join("\n");
    println!("basin hashes:\n{table}");
    assert_eq!(
        got.as_slice(),
        GOLDEN.as_slice(),
        "basin hashes moved; see the update rule on GOLDEN"
    );
}

/// Whether the polyline `a` crosses the segment `c`–`d`, and where.
fn crossing(a: &[[f32; 3]], c: V2, d: V2) -> Option<(usize, f32)> {
    use world::math::intersect_lines;
    for (k, w) in a.windows(2).enumerate() {
        let (p, q) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
        if let Some(x) = intersect_lines(p, q - p, c, d - c) {
            let t = (x - p).dot(q - p) / (q - p).dot(q - p);
            let u = (x - c).dot(d - c) / (d - c).dot(d - c);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                return Some((k, t));
            }
        }
    }
    None
}

/// What the template promises, on every seed: see the module docs.
#[test]
fn basins_hold_their_grid_freeway_channel_rail_airfield_and_hills() {
    use place::{PropKind, TRACK_BRIDGE, TRACK_CROSSING};
    let t = template();
    let g = grammar();
    for seed in 1..=4u64 {
        let m = MacroMap::generate(Seed(seed), &t, PLACE_CHUNKS);
        let b = m.basin.as_ref().unwrap();
        let p = build_place(Seed(seed), &t, &g);
        let edges = &p.roads.edges;
        let play = m.playable();
        // Every road inside the edges' bands.
        for e in edges {
            for q in &e.profile {
                assert!(
                    q[0] >= play[0] && q[2] >= play[1] && q[0] <= play[2] && q[2] <= play[3],
                    "seed {seed}: {:?} road at ({:.0}, {:.0}) in an edge band",
                    e.class,
                    q[0],
                    q[2]
                );
            }
        }
        // The freeway: one chain, both ends at grade on the grid.
        let mut hdeg = vec![0u32; p.roads.nodes.len()];
        let mut deg = vec![0u32; p.roads.nodes.len()];
        for e in edges {
            deg[e.a as usize] += 1;
            deg[e.b as usize] += 1;
            if e.class == RoadClass::Highway {
                hdeg[e.a as usize] += 1;
                hdeg[e.b as usize] += 1;
            }
        }
        let ends: Vec<usize> = (0..hdeg.len()).filter(|&i| hdeg[i] == 1).collect();
        assert_eq!(ends.len(), 2, "seed {seed}: the freeway is not one chain");
        for &n in &ends {
            assert!(deg[n] >= 2, "seed {seed}: the freeway dead-ends");
        }
        // It turns back along the desert edge.
        let east = p.header.extent_m()[0] - m.bands[place::EAST];
        assert!(m.highway.iter().any(|q| east - q[0] < 120.0), "seed {seed}");
        // Overpasses: arterials over it, unjoined, a deck well above it.
        let hw: Vec<V2> = m.highway.iter().map(|&q| V2::from_array(q)).collect();
        let mut overpasses = 0;
        for e in edges.iter().filter(|e| e.class == RoadClass::Arterial) {
            for w in hw.windows(2) {
                if let Some((k, t)) = crossing(&e.profile, w[0], w[1]) {
                    let y = e.profile[k][1] + t * (e.profile[k + 1][1] - e.profile[k][1]);
                    let ground = m.height_at(w[0].x, w[0].y);
                    if y > ground + 5.0 {
                        overpasses += 1;
                    }
                }
            }
        }
        assert!(overpasses >= 3, "seed {seed}: {overpasses} overpasses");
        // Interchanges, one with a loop: a ramp turning through more than 180 degrees.
        let ramps: Vec<&place::Edge> = edges
            .iter()
            .filter(|e| e.oneway && e.class == RoadClass::Collector)
            .collect();
        assert!(ramps.len() >= 6, "seed {seed}: {} ramps", ramps.len());
        let turn = |e: &place::Edge| {
            let pts: Vec<V2> = e.profile.iter().map(|q| V2::new(q[0], q[2])).collect();
            pts.windows(3)
                .map(|w| {
                    let (u, v) = ((w[1] - w[0]).normalize(), (w[2] - w[1]).normalize());
                    world::math::atan2(u.turn(v), u.dot(v)).abs()
                })
                .sum::<f32>()
        };
        assert!(
            ramps.iter().any(|e| turn(e) > std::f32::consts::PI),
            "seed {seed}: no loop on-ramp"
        );
        // Boulevards: one-way carriageways of three lanes, both ways, each way across.
        let blvd = edges
            .iter()
            .filter(|e| e.oneway && e.class == RoadClass::Arterial && e.lanes_fwd == 3)
            .count();
        assert!(blvd >= 8, "seed {seed}: {blvd} boulevard pieces");
        // The channel: the bed its depth under the ground beside it, a dry channel body,
        // every road over it on a bridge.
        let mid = V2::from_array(b.channel[b.channel.len() / 2]);
        let bed = p.terrain.height_at(&p.header, [mid.x, mid.y]).unwrap();
        let lip = m.height_at(mid.x + 40.0, mid.y);
        assert!(
            lip - bed > 6.5,
            "seed {seed}: channel only {} deep",
            lip - bed
        );
        assert!(p.terrain.water.iter().any(|w| w.kind == WaterKind::Channel));
        let (c0, c1) = (
            V2::from_array(b.channel[0]),
            V2::from_array(b.channel[b.channel.len() - 1]),
        );
        let mut bridges = 0;
        for e in edges {
            if let Some((k, t)) = crossing(&e.profile, c0, c1) {
                let q = V2::new(e.profile[k][0], e.profile[k][2])
                    .lerp(V2::new(e.profile[k + 1][0], e.profile[k + 1][2]), t);
                let y = e.profile[k][1] + t * (e.profile[k + 1][1] - e.profile[k][1]);
                let g = p.terrain.height_at(&p.header, [q.x, q.y]).unwrap();
                assert!(
                    y > g + 5.0,
                    "seed {seed}: {:?} road dips into the channel",
                    e.class
                );
                bridges += 1;
            }
        }
        assert!(bridges >= 5, "seed {seed}: {bridges} bridges");
        assert!(!b.ramps.is_empty(), "seed {seed}: no channel ramps");
        // The rail line: track at grade across every out line it meets, with a sign each
        // way, and a bridge over the channel.
        let tracks: Vec<(u8, V2)> = p
            .props
            .items
            .iter()
            .filter_map(|x| match x.kind {
                PropKind::Track { bed, .. } => Some((bed, V2::new(x.pos[0], x.pos[2]))),
                _ => None,
            })
            .collect();
        let crossings = tracks.iter().filter(|t| t.0 == TRACK_CROSSING).count();
        let signs = p
            .props
            .items
            .iter()
            .filter(|x| x.kind == PropKind::Crossbuck)
            .count();
        let out_lines = b.lines.len() - b.cross;
        assert!(crossings >= out_lines, "seed {seed}: {crossings} crossings");
        assert!(
            signs >= 2 * out_lines,
            "seed {seed}: {signs} crossing signs"
        );
        assert!(tracks.iter().any(|t| t.0 == TRACK_BRIDGE), "seed {seed}");
        assert!(!b.spurs.is_empty(), "seed {seed}: no spurs");
        // The airfield: its runway, and no tree on it.
        assert!(
            edges
                .iter()
                .any(|e| (e.width_m - b.runway_m).abs() < 0.01 && !e.sidewalk),
            "seed {seed}: no runway"
        );
        let field: Vec<V2> = b.airfield.iter().map(|&q| V2::from_array(q)).collect();
        for x in &p.props.items {
            let tree = matches!(
                x.kind,
                PropKind::PalmTree | PropKind::Oak | PropKind::Eucalyptus
            );
            let q = V2::new(x.pos[0], x.pos[2]);
            assert!(
                !(tree && world::math::point_in_polygon(q, &field)),
                "seed {seed}: a tree on the airfield at {q:?}"
            );
        }
        // The grammar's street dressing: bus stops, and billboards blank or pictured, never
        // the coast's lettered ads.
        let count =
            |f: &dyn Fn(&PropKind) -> bool| p.props.items.iter().filter(|x| f(&x.kind)).count();
        let shelters = count(&|k| matches!(k, PropKind::BusShelter { .. }));
        let faces = count(&|k| matches!(k, PropKind::FaceBillboard { .. }));
        assert!(shelters >= 10, "seed {seed}: {shelters} bus shelters");
        assert!(faces >= 3, "seed {seed}: {faces} billboards");
        assert_eq!(count(&|k| *k == PropKind::Billboard), 0, "seed {seed}");
        // The hills: an overlook with a road to it.
        let overlooks: Vec<V2> = m
            .pois
            .iter()
            .filter(|q| q.kind == world::procgen::MacroPoiKind::Overlook)
            .map(|q| V2::from_array(q.pos))
            .collect();
        let reached = overlooks.iter().any(|&o| {
            edges.iter().any(|e| {
                e.profile.windows(2).any(|w| {
                    let (a, c) = (V2::new(w[0][0], w[0][2]), V2::new(w[1][0], w[1][2]));
                    segment_distance(o, a, c).0 < 60.0
                })
            })
        });
        assert!(reached, "seed {seed}: no road to an overlook");
        let hills = p
            .zones
            .cells
            .iter()
            .filter(|&&z| z == ZoneId::Hills)
            .count();
        assert!(hills * 10 > p.zones.cells.len(), "seed {seed}");
        println!(
            "seed {seed}: {overpasses} overpasses, {} ramps, {blvd} boulevard pieces, {bridges} channel bridges, {} ramps into it, {crossings} grade crossings, {} spurs, {} overlooks, {shelters} bus shelters, {faces} billboards",
            ramps.len(),
            b.ramps.len(),
            b.spurs.len(),
            overlooks.len()
        );
    }
}
