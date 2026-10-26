//! Acceptance tests for the arterial network and highway (stage 3): determinism,
//! connectivity, the highway's ramps and overpasses, the beachfront road, grades, water,
//! arterial spacing, the time budget, and `place::validate` on a skeleton Place built from
//! the result. Run with `--nocapture` to see the numbers:
//! `cargo test -p world --test arterials -- --nocapture`.

use std::time::Instant;

use place::{
    Era, Facades, Lanes, Lights, Place, PlaceHeader, Props, RegionId, RoadClass, RoadGraph,
    Sidewalks, Spawn, SpawnKind, Terrain, ZoneId, Zones, CHUNK_SIZE_M, DEFAULT_RES_PER_CHUNK,
    PLACE_VERSION,
};
use skywave_core::Seed;
use world::procgen::{arterials, hash_graph, MacroMap, ZONE_CELL_M};
use world::template::RegionTemplate;

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");
const SIZE: [u16; 2] = [16, 16];

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
    t.validate().expect("validate");
    t
}

fn generate(t: &RegionTemplate, seed: u64) -> (MacroMap, RoadGraph) {
    let m = MacroMap::generate(Seed(seed), t, SIZE);
    let g = arterials(Seed(seed), t, &m);
    (m, g)
}

/// Pinned `hash_graph` of `california_coast` arterials at 16×16 chunks for seeds 1..=8.
///
/// Update rule: these change only when arterial generation (or the macro map, or the
/// template) changes on purpose. Then regenerate (`cargo test -p world --test arterials --
/// --nocapture` prints the new table), look at the `tools/mapdump --roads` PNGs for seeds
/// 1–4 before and after, and commit the new values in their own commit whose message says
/// why they moved.
const GOLDEN: [u64; 8] = [
    0x7A01_F1B6_8A86_AE4B,
    0xB19D_E015_B125_B639,
    0xC28C_1CDC_5D0C_CB59,
    0x670C_DA41_1EBE_DD11,
    0xB828_D04C_F6C7_936D,
    0x0611_FC42_75BC_19F7,
    0xACF5_7142_C7E5_65E2,
    0xE29F_46E0_A3FF_2CDC,
];

#[test]
fn hashes_are_deterministic_and_pinned() {
    let t = template();
    let got: Vec<u64> = (1..=8).map(|s| hash_graph(&generate(&t, s).1)).collect();
    let (_, again) = generate(&t, 1);
    assert_eq!(hash_graph(&again), got[0], "same seed, same graph");
    assert_eq!(again, generate(&t, 1).1);
    for w in got.windows(2) {
        assert_ne!(w[0], w[1], "different seeds should differ");
    }
    let table = got
        .iter()
        .map(|h| {
            let l = format!("    0x{h:016X},");
            format!("{}_{}_{}_{},", &l[..10], &l[10..14], &l[14..18], &l[18..22])
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("arterial hashes:\n{table}");
    assert_eq!(
        got, GOLDEN,
        "arterial hashes moved; see the update rule on GOLDEN"
    );
}

/// Edges in the largest connected component, and the component count.
fn components(g: &RoadGraph) -> (usize, usize) {
    let mut parent: Vec<usize> = (0..g.nodes.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for e in &g.edges {
        let (a, b) = (
            root(&mut parent, e.a as usize),
            root(&mut parent, e.b as usize),
        );
        if a != b {
            parent[a.max(b)] = a.min(b);
        }
    }
    let mut count = vec![0usize; g.nodes.len()];
    for e in &g.edges {
        count[root(&mut parent, e.a as usize)] += 1;
    }
    (
        count.iter().copied().max().unwrap_or(0),
        count.iter().filter(|&&c| c > 0).count(),
    )
}

#[test]
fn main_component_holds_95_percent() {
    let t = template();
    for seed in 1..=16 {
        let (_, g) = generate(&t, seed);
        let (main, comps) = components(&g);
        println!(
            "seed {seed}: {} nodes, {} edges, {comps} component(s), main {main}",
            g.nodes.len(),
            g.edges.len()
        );
        assert!(
            g.edges.len() > 30,
            "seed {seed}: only {} edges",
            g.edges.len()
        );
        assert!(
            main * 20 >= g.edges.len() * 19,
            "seed {seed}: main component {main} of {}",
            g.edges.len()
        );
    }
}

fn is_highway_node(g: &RoadGraph, n: u32) -> bool {
    g.edges
        .iter()
        .any(|e| e.class == RoadClass::Highway && (e.a == n || e.b == n))
}

fn ramps(g: &RoadGraph) -> Vec<usize> {
    (0..g.edges.len())
        .filter(|&i| {
            let e = &g.edges[i];
            e.class == RoadClass::Collector
                && e.oneway
                && (is_highway_node(g, e.a) || is_highway_node(g, e.b))
        })
        .collect()
}

/// Intersection parameters of two 2D segments.
fn cross(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> Option<(f32, f32)> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-9 {
        return None;
    }
    let w = [c[0] - a[0], c[1] - a[1]];
    let t = (w[0] * s[1] - w[1] * s[0]) / den;
    let u = (w[0] * r[1] - w[1] * r[0]) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

/// Every crossing of a highway edge with a non-highway, non-ramp edge:
/// `(road edge, clearance above the highway)`.
fn overpasses(g: &RoadGraph) -> Vec<(usize, f32)> {
    let ramp = ramps(g);
    let mut out = Vec::new();
    for h in g.edges.iter().filter(|e| e.class == RoadClass::Highway) {
        for (i, e) in g.edges.iter().enumerate() {
            if e.class == RoadClass::Highway || ramp.contains(&i) {
                continue;
            }
            for hs in h.profile.windows(2) {
                for es in e.profile.windows(2) {
                    let p = |q: [f32; 3]| [q[0], q[2]];
                    if let Some((t, u)) = cross(p(es[0]), p(es[1]), p(hs[0]), p(hs[1])) {
                        // Meeting the highway at a shared node (its terminal) is no crossing.
                        let x = [
                            es[0][0] + (es[1][0] - es[0][0]) * t,
                            es[0][2] + (es[1][2] - es[0][2]) * t,
                        ];
                        let at_node = [e.a, e.b].iter().any(|&n| {
                            let q = g.nodes[n as usize].pos;
                            is_highway_node(g, n) && (q[0] - x[0]).hypot(q[1] - x[1]) < 1.0
                        });
                        if at_node {
                            continue;
                        }
                        let yr = es[0][1] + (es[1][1] - es[0][1]) * t;
                        let yh = hs[0][1] + (hs[1][1] - hs[0][1]) * u;
                        out.push((i, yr - yh));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn seed_one_has_highway_ramps_overpasses_and_beachfront() {
    let t = template();
    let (m, g) = generate(&t, 1);
    let mut count = [0usize; 7];
    for e in &g.edges {
        count[e.class as usize] += 1;
    }
    println!("seed 1 edges by class (Local, Collector, Arterial, Highway, Alley, Beachfront, Rural): {count:?}");

    // One highway chain: its edges form a single path.
    let hw: Vec<&place::Edge> = g
        .edges
        .iter()
        .filter(|e| e.class == RoadClass::Highway)
        .collect();
    assert!(!hw.is_empty(), "no highway");
    let mut deg = vec![0u32; g.nodes.len()];
    for e in &hw {
        deg[e.a as usize] += 1;
        deg[e.b as usize] += 1;
    }
    assert!(deg.iter().all(|&d| d <= 2), "highway branches");
    assert_eq!(
        deg.iter().filter(|&&d| d == 1).count(),
        2,
        "highway is not one chain"
    );
    for e in &hw {
        assert_eq!((e.lanes_fwd, e.lanes_back, e.sidewalk), (2, 2, false));
        assert_eq!((e.speed_kph, e.width_m), (100, 22.0));
    }

    // Ramps: one-way collectors off the highway rising about 6 m above the ground over
    // about 150 m.
    let graded = &m.heights;
    let ground = |x: f32, z: f32| {
        let w = m.width as usize;
        let (fx, fz) = (x / 4.0, z / 4.0);
        let (ix, iz) = (fx as usize, fz as usize);
        let (tx, tz) = (fx - ix as f32, fz - iz as f32);
        let at = |i: usize, j: usize| graded[j * w + i];
        let a = at(ix, iz) + (at(ix + 1, iz) - at(ix, iz)) * tx;
        let b = at(ix, iz + 1) + (at(ix + 1, iz + 1) - at(ix, iz + 1)) * tx;
        a + (b - a) * tz
    };
    let ramp = ramps(&g);
    println!("seed 1: {} ramps", ramp.len());
    assert!(ramp.len() >= 2, "only {} ramps", ramp.len());
    for &i in &ramp {
        let e = &g.edges[i];
        let above = |p: [f32; 3]| p[1] - ground(p[0], p[2]);
        let (p0, p1) = (e.profile[0], e.profile[e.profile.len() - 1]);
        let rise = (above(p1) - above(p0)).abs();
        let len: f32 = e
            .profile
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt())
            .sum();
        println!("  ramp {i}: {len:.0} m, rises {rise:.1} m above the ground");
        assert!((120.0..=220.0).contains(&len), "ramp {i} is {len} m");
        assert!((4.5..=7.5).contains(&rise), "ramp {i} rises {rise} m");
    }

    // Every other highway crossing is an overpass, unconnected, and one clears by 5 m.
    let over = overpasses(&g);
    let best = over.iter().map(|o| o.1).fold(f32::NEG_INFINITY, f32::max);
    println!(
        "seed 1: {} overpass crossings, best clearance {best:.1} m",
        over.len()
    );
    assert!(!over.is_empty(), "no overpass");
    assert!(best >= 5.0, "best overpass clearance {best}");
    for &(i, _) in &over {
        let e = &g.edges[i];
        assert!(
            !is_highway_node(&g, e.a) && !is_highway_node(&g, e.b),
            "crossing edge {i} connects to the highway"
        );
    }

    // The beachfront road hugs the beach band.
    let zw = m.zone_width as usize;
    let beach: Vec<[f32; 2]> = (0..m.zones.len())
        .filter(|&i| m.zones[i] == ZoneId::Beach)
        .map(|i| {
            [
                (i % zw) as f32 * ZONE_CELL_M + 8.0,
                (i / zw) as f32 * ZONE_CELL_M + 8.0,
            ]
        })
        .collect();
    let (mut near, mut total) = (0.0f32, 0.0f32);
    for e in g.edges.iter().filter(|e| e.class == RoadClass::Beachfront) {
        for w in e.profile.windows(2) {
            let len = ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
            let mid = [(w[0][0] + w[1][0]) * 0.5, (w[0][2] + w[1][2]) * 0.5];
            let d = beach
                .iter()
                .map(|b| ((b[0] - mid[0]).powi(2) + (b[1] - mid[1]).powi(2)).sqrt())
                .fold(f32::INFINITY, f32::min);
            total += len;
            if d <= 60.0 {
                near += len;
            }
        }
    }
    println!(
        "seed 1: beachfront {total:.0} m, {:.0}% within 60 m of the beach",
        100.0 * near / total
    );
    assert!(total > 500.0, "beachfront only {total} m");
    assert!(
        near >= 0.7 * total,
        "beachfront near the beach {near} of {total} m"
    );
}

#[test]
fn grades_stay_under_twelve_percent() {
    let t = template();
    for seed in 1..=16 {
        let (_, g) = generate(&t, seed);
        let ramp = ramps(&g);
        let (mut worst, mut worst_hw) = (0.0f32, 0.0f32);
        for (i, e) in g.edges.iter().enumerate() {
            if ramp.contains(&i) {
                continue;
            }
            for w in e.profile.windows(2) {
                let run = ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
                if run < 1e-3 {
                    continue;
                }
                let grade = (w[1][1] - w[0][1]).abs() / run;
                if e.class == RoadClass::Highway {
                    worst_hw = worst_hw.max(grade);
                } else {
                    worst = worst.max(grade);
                }
                assert!(
                    grade <= 0.12 + 1e-4,
                    "seed {seed} edge {i} ({:?}) grade {grade:.3} at {:?}",
                    e.class,
                    w[0]
                );
            }
        }
        println!(
            "seed {seed}: steepest road {:.1}%, steepest highway {:.2}%",
            worst * 100.0,
            worst_hw * 100.0
        );
        assert!(
            worst_hw <= 0.05 + 1e-4,
            "seed {seed}: highway grade {worst_hw}"
        );
    }
}

#[test]
fn no_edge_crosses_water() {
    let t = template();
    for seed in 1..=16 {
        let (m, g) = generate(&t, seed);
        for (i, e) in g.edges.iter().enumerate() {
            for w in e.profile.windows(2) {
                let len = ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
                let steps = (len / 4.0) as usize + 1;
                for k in 0..=steps {
                    let f = k as f32 / steps as f32;
                    let (x, z) = (
                        w[0][0] + (w[1][0] - w[0][0]) * f,
                        w[0][2] + (w[1][2] - w[0][2]) * f,
                    );
                    assert!(
                        m.zone_at(x, z) != ZoneId::Water && m.height_at(x, z) > m.water_level,
                        "seed {seed} edge {i} ({:?}) in water at ({x:.0}, {z:.0})",
                        e.class
                    );
                }
            }
        }
    }
}

/// Samples every 10 m along arterial edges; a sample is "crowded" when another arterial
/// edge (not sharing a node) runs parallel (|cos| > 0.9) within 150 m, measured across
/// (the offset is mostly perpendicular, so roads continuing end to end do not count).
#[test]
fn parallel_arterials_keep_their_distance() {
    let t = template();
    for seed in 1..=16 {
        let (_, g) = generate(&t, seed);
        let art: Vec<usize> = (0..g.edges.len())
            .filter(|&i| g.edges[i].class == RoadClass::Arterial)
            .collect();
        let pts = |i: usize| -> Vec<([f32; 2], [f32; 2])> {
            let mut out = Vec::new();
            for w in g.edges[i].profile.windows(2) {
                let (a, b) = ([w[0][0], w[0][2]], [w[1][0], w[1][2]]);
                let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                if len < 1e-3 {
                    continue;
                }
                let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
                let n = (len / 10.0) as usize + 1;
                for k in 0..n {
                    let f = (k as f32 + 0.5) / n as f32;
                    out.push(([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f], d));
                }
            }
            out
        };
        let samples: Vec<Vec<([f32; 2], [f32; 2])>> = art.iter().map(|&i| pts(i)).collect();
        let mut worst = 0.0f32;
        for (x, &i) in art.iter().enumerate() {
            let mut crowded = 0.0f32;
            for (p, d) in &samples[x] {
                let hit = art.iter().enumerate().any(|(y, &j)| {
                    let (ei, ej) = (&g.edges[i], &g.edges[j]);
                    if i == j || ei.a == ej.a || ei.a == ej.b || ei.b == ej.a || ei.b == ej.b {
                        return false;
                    }
                    samples[y].iter().any(|(q, e)| {
                        let off = [q[0] - p[0], q[1] - p[1]];
                        let dist = (off[0] * off[0] + off[1] * off[1]).sqrt();
                        let parallel = (d[0] * e[0] + d[1] * e[1]).abs() > 0.9;
                        let across =
                            dist > 0.0 && ((off[0] * d[0] + off[1] * d[1]) / dist).abs() < 0.5;
                        dist < 150.0 && parallel && across
                    })
                });
                if hit {
                    crowded += 10.0;
                }
            }
            worst = worst.max(crowded);
            assert!(
                crowded <= 100.0,
                "seed {seed}: arterial edge {i} runs within 150 m of a parallel arterial for {crowded} m"
            );
        }
        println!("seed {seed}: most crowded arterial {worst} m");
    }
}

#[test]
fn seed_one_fits_the_budget() {
    let t = template();
    let m = MacroMap::generate(Seed(1), &t, SIZE);
    // Best of three, so a busy test runner does not fail the budget.
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let start = Instant::now();
        let g = arterials(Seed(1), &t, &m);
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
        assert!(!g.edges.is_empty());
    }
    println!("arterials seed 1: {best:.1} ms (target < 150 ms in debug)");
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(best < 150.0, "arterials took {best:.1} ms");
    }
}

/// A skeleton Place from the macro terrain, the zones, the graph and a player start at the
/// strip's center.
fn skeleton(m: &MacroMap, g: &RoadGraph) -> Place {
    let header = PlaceHeader {
        version: PLACE_VERSION,
        seed: m.seed.0,
        chunk_size_m: CHUNK_SIZE_M,
        size_chunks: m.size_chunks,
        chunk_origin: [0, 0],
        region: RegionId::CaliforniaCoast,
        era: Era::Eighties,
        origin_geo: None,
    };
    let res = DEFAULT_RES_PER_CHUNK;
    let [tw, th] = Terrain::samples(&header, res);
    let graded = &m.heights;
    let (w, h) = (m.width as usize, m.height as usize);
    let mut heights = Vec::with_capacity(tw * th);
    for z in 0..th {
        for x in 0..tw {
            heights.push(graded[z.min(h - 1) * w + x.min(w - 1)]);
        }
    }
    let [zw, zh] = Zones::cells_per_side(&header, res);
    let mzw = m.zone_width as usize;
    let cells: Vec<ZoneId> = (0..zw * zh)
        .map(|i| m.zones[(i / zw / 4) * mzw + (i % zw) / 4])
        .collect();
    let mid = m.strip.centerline[m.strip.centerline.len() / 2];
    let y = graded[(mid[1] / 4.0) as usize * w + (mid[0] / 4.0) as usize];
    let mut place = Place {
        header,
        terrain: Terrain {
            res_per_chunk: res,
            heights,
            water_level: m.water_level,
            water: Vec::new(),
        },
        zones: Zones { cells },
        roads: g.clone(),
        parcels: Vec::new(),
        buildings: Vec::new(),
        pois: Vec::new(),
        spawns: vec![Spawn {
            pos: [mid[0], y, mid[1]],
            heading: 0.0,
            kind: SpawnKind::PlayerStart,
        }],
        lanes: Lanes::default(),
        sidewalks: Sidewalks::default(),
        facades: Facades::default(),
        props: Props::default(),
        lights: Lights::default(),
    };
    place.sort_by_chunk();
    place
}

#[test]
fn skeleton_places_validate() {
    let t = template();
    for seed in 1..=16 {
        let (m, g) = generate(&t, seed);
        let place = skeleton(&m, &g);
        assert_eq!(place.validate(), Ok(()), "seed {seed}");
    }
}

/// The corridor the macro map carved holds the highway: the highway's profile stays on the
/// macro terrain (no separate grading pass), never steeper than 5%, and the carve cuts
/// and fills at most 30 m.
#[test]
fn highway_runs_in_the_carved_corridor() {
    let t = template();
    for seed in 1..=8 {
        let (m, g) = generate(&t, seed);
        let (mut grade, mut off) = (0.0f32, 0.0f32);
        for e in g.edges.iter().filter(|e| e.class == RoadClass::Highway) {
            for w in e.profile.windows(2) {
                let run = ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
                if run > 1e-3 {
                    grade = grade.max((w[1][1] - w[0][1]).abs() / run);
                }
            }
            for p in &e.profile {
                off = off.max((p[1] - m.height_at(p[0], p[2])).abs());
            }
        }
        let cut = m.corridor.iter().fold(0.0f32, |a, &c| a.min(c));
        let fill = m.corridor.iter().fold(0.0f32, |a, &c| a.max(c));
        let top = m.highway_bed.iter().fold(f32::MIN, |a, &b| a.max(b));
        println!(
            "seed {seed}: highway grade {:.2}%, off terrain {off:.2} m, corridor cut {:.1} m, fill {fill:.1} m, bed top {top:.0} m",
            grade * 100.0,
            -cut
        );
        assert!(grade <= 0.05 + 1e-4, "seed {seed}: highway grade {grade}");
        assert!(off <= 2.0, "seed {seed}: highway {off} m off the terrain");
        assert!(-cut <= 30.0, "seed {seed}: corridor cut {} m", -cut);
        assert!(fill <= 30.0, "seed {seed}: corridor fill {fill} m");
    }
}
