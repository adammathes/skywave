//! Acceptance tests for the local streets (stage 4): chunk-border stability, determinism,
//! connectivity and validation of the merged graph, grades, water, downtown block sizes and
//! the per-chunk time. Run with `--nocapture` to see the numbers:
//! `cargo test -p world --test streets -- --nocapture`.

use std::time::Instant;

use place::{
    Edge, Era, Facades, Lanes, Lights, Place, PlaceHeader, Props, RegionId, RoadClass, RoadGraph,
    Sidewalks, Spawn, SpawnKind, Terrain, ZoneId, Zones, CHUNK_SIZE_M, DEFAULT_RES_PER_CHUNK,
    PLACE_VERSION,
};
use skywave_core::{Rng, Seed};
use world::procgen::{
    arterials, hash_chunk, hash_graph, streets, MacroMap, NodeRef, StreetChunk, StreetPlan,
};
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

fn chunks_of(plan: &StreetPlan<'_>) -> Vec<StreetChunk> {
    let mut out = Vec::new();
    for z in 0..i32::from(SIZE[1]) {
        for x in 0..i32::from(SIZE[0]) {
            out.push(plan.chunk([x, z]));
        }
    }
    out
}

/// Border nodes of `c` on the border it shares with the chunk at `(dx, dz)`:
/// `(key, x bits, z bits, class, width, unit direction into the chunk)`.
fn border_nodes(c: &StreetChunk, dx: i32, dz: i32) -> Vec<BorderNode> {
    let x0 = c.chunk[0] as f32 * CHUNK_SIZE_M;
    let z0 = c.chunk[1] as f32 * CHUNK_SIZE_M;
    let on = |p: [f32; 2]| match (dx, dz) {
        (1, 0) => p[0] == x0 + CHUNK_SIZE_M,
        (-1, 0) => p[0] == x0,
        (0, 1) => p[1] == z0 + CHUNK_SIZE_M,
        _ => p[1] == z0,
    };
    let mut out = Vec::new();
    for (i, (n, r)) in c.nodes.iter().zip(&c.refs).enumerate() {
        let NodeRef::Border { key } = *r else {
            continue;
        };
        if !on(n.pos) {
            continue;
        }
        let edges: Vec<&Edge> = c
            .edges
            .iter()
            .filter(|e| e.a as usize == i || e.b as usize == i)
            .collect();
        assert_eq!(
            edges.len(),
            1,
            "chunk {:?}: border node {i} at {:?} has {} edges",
            c.chunk,
            n.pos,
            edges.len()
        );
        let e = edges[0];
        let (p, q) = if e.a as usize == i {
            (e.profile[0], e.profile[1])
        } else {
            (
                e.profile[e.profile.len() - 1],
                e.profile[e.profile.len() - 2],
            )
        };
        let d = [q[0] - p[0], q[2] - p[2]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
        out.push((
            key,
            n.pos[0].to_bits(),
            n.pos[1].to_bits(),
            e.class,
            e.width_m.to_bits(),
            [d[0] / l, d[1] / l],
        ));
    }
    out.sort_by_key(|b| (b.0, b.1, b.2));
    out
}

type BorderNode = (u64, u32, u32, RoadClass, u32, [f32; 2]);

/// Asserts that chunk `a` and its neighbor `b` (at `(dx, dz)` from it) agree on their shared
/// border: the same border nodes (key, position, class, width), each with one edge on each
/// side, continuing in a straight line. Returns the number of crossings.
fn check_border(seed: u64, a: &StreetChunk, b: &StreetChunk, dx: i32, dz: i32) -> usize {
    let mine = border_nodes(a, dx, dz);
    let theirs = border_nodes(b, -dx, -dz);
    let ids = |v: &[BorderNode]| -> Vec<(u64, u32, u32, RoadClass, u32)> {
        v.iter().map(|b| (b.0, b.1, b.2, b.3, b.4)).collect()
    };
    assert_eq!(
        ids(&mine),
        ids(&theirs),
        "seed {seed}: border of {:?} toward ({dx}, {dz}) differs",
        a.chunk
    );
    for (p, q) in mine.iter().zip(&theirs) {
        let cos = p.5[0] * q.5[0] + p.5[1] * q.5[1];
        assert!(
            cos < -0.999,
            "seed {seed}: street bends at the border of {:?} at ({}, {}): cos {cos}",
            a.chunk,
            f32::from_bits(p.1),
            f32::from_bits(p.2)
        );
    }
    mine.len()
}

/// Generating a chunk alone or after its eight neighbors gives the same streets, and it
/// agrees with each neighbor on the shared border; twenty random chunks with border
/// crossings on each of seeds 1–4.
#[test]
fn chunk_borders_are_stable() {
    let t = template();
    for seed in 1..=4u64 {
        let (m, g) = generate(&t, seed);
        let plan = StreetPlan::new(Seed(seed), &t, &m, &g);
        let mut rng = Rng::new(Seed(seed), &[0x5354_4142]);
        let mut picked: Vec<[i32; 2]> = Vec::new();
        let mut tries = 0;
        while picked.len() < 20 && tries < 400 {
            tries += 1;
            let c = [
                rng.range(0, u32::from(SIZE[0])) as i32,
                rng.range(0, u32::from(SIZE[1])) as i32,
            ];
            if picked.contains(&c) {
                continue;
            }
            let out = plan.chunk(c);
            if out.refs.iter().any(|r| matches!(r, NodeRef::Border { .. })) {
                picked.push(c);
            }
        }
        assert!(
            picked.len() >= 15,
            "seed {seed}: only {} chunks with border streets",
            picked.len()
        );
        let mut shared = 0;
        for &c in &picked {
            // Alone (a fresh plan, nothing else generated) and after all eight neighbors.
            let alone = streets(Seed(seed), &t, &m, &g, c);
            let mut around = Vec::new();
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx != 0 || dz != 0 {
                        around.push(((dx, dz), plan.chunk([c[0] + dx, c[1] + dz])));
                    }
                }
            }
            let with = plan.chunk(c);
            assert_eq!(
                alone, with,
                "seed {seed} chunk {c:?}: depends on what was generated before"
            );
            for ((dx, dz), nb) in &around {
                if dx.abs() + dz.abs() == 1 {
                    shared += check_border(seed, &with, nb, *dx, *dz);
                }
            }
        }
        println!(
            "seed {seed}: {} chunks, {shared} border crossings checked",
            picked.len()
        );
        assert!(shared > 20, "seed {seed}: only {shared} border crossings");
    }
}

/// Every chunk border of seeds 1..=8.
#[test]
fn every_border_matches() {
    let t = template();
    for seed in 1..=8u64 {
        let (m, g) = generate(&t, seed);
        let plan = StreetPlan::new(Seed(seed), &t, &m, &g);
        let chunks = chunks_of(&plan);
        let (w, h) = (usize::from(SIZE[0]), usize::from(SIZE[1]));
        let mut shared = 0;
        for z in 0..h {
            for x in 0..w {
                let c = &chunks[z * w + x];
                if x + 1 < w {
                    shared += check_border(seed, c, &chunks[z * w + x + 1], 1, 0);
                }
                if z + 1 < h {
                    shared += check_border(seed, c, &chunks[(z + 1) * w + x], 0, 1);
                }
            }
        }
        println!("seed {seed}: {shared} border crossings");
    }
}

/// Pinned `hash_chunk` of a few seed-1 chunks and `hash_graph` of the merged graph for
/// seeds 1..=4 (`california_coast`, 16×16 chunks).
///
/// Update rule: these change only when street generation (or the arterials, the macro map
/// or the template) changes on purpose. Then regenerate (`cargo test -p world --test
/// streets -- --nocapture` prints the new tables), look at the `tools/mapdump --streets`
/// PNGs for seeds 1–2 before and after, and commit the new values in their own commit whose
/// message says why they moved.
const CHUNKS: [[i32; 2]; 5] = [[3, 7], [7, 7], [10, 6], [7, 13], [4, 14]];
const GOLDEN_CHUNKS: [u64; 5] = [
    0x6168_2D51_7717_A59C,
    0xEE47_ECE0_9460_0CFD,
    0xB032_8865_255B_3AFA,
    0x4EF9_E3CA_FCA4_53D5,
    0x86C4_38B6_34CC_EDFE,
];
const GOLDEN_MERGED: [u64; 4] = [
    0xACFC_4044_E638_0A91,
    0x5E17_4257_AC83_3C0A,
    0xBA57_D589_4020_4F83,
    0xAFD6_8C7C_F0DA_53DD,
];

fn table(hs: &[u64]) -> String {
    hs.iter()
        .map(|h| {
            let l = format!("    0x{h:016X},");
            format!("{}_{}_{}_{},", &l[..10], &l[10..14], &l[14..18], &l[18..22])
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn hashes_are_deterministic_and_pinned() {
    let t = template();
    let (m, g) = generate(&t, 1);
    let plan = StreetPlan::new(Seed(1), &t, &m, &g);
    for &c in &CHUNKS {
        assert!(
            !plan.chunk(c).edges.is_empty(),
            "pinned chunk {c:?} is empty"
        );
    }
    let chunks: Vec<u64> = CHUNKS.iter().map(|&c| hash_chunk(&plan.chunk(c))).collect();
    let again: Vec<u64> = CHUNKS
        .iter()
        .map(|&c| hash_chunk(&streets(Seed(1), &t, &m, &g, c)))
        .collect();
    assert_eq!(chunks, again, "same seed, same chunks");
    let merged: Vec<u64> = (1..=4)
        .map(|s| {
            let (m, g) = generate(&t, s);
            hash_graph(&StreetPlan::new(Seed(s), &t, &m, &g).merged())
        })
        .collect();
    for w in merged.windows(2) {
        assert_ne!(w[0], w[1], "different seeds should differ");
    }
    println!("chunk hashes:\n{}", table(&chunks));
    println!("merged hashes:\n{}", table(&merged));
    assert_eq!(
        chunks, GOLDEN_CHUNKS,
        "chunk hashes moved; see the update rule"
    );
    assert_eq!(
        merged, GOLDEN_MERGED,
        "merged hashes moved; see the update rule"
    );
}

/// Whether `e` is a ramp or a piece of one: a one-way collector that reaches a highway
/// node through one-way collectors only (the merged graph splits ramps at chunk borders and
/// where streets join them, so a piece need not touch the highway itself).
fn is_ramp(g: &RoadGraph, e: &Edge) -> bool {
    let ramp_like = |h: &Edge| h.class == RoadClass::Collector && h.oneway;
    if !ramp_like(e) {
        return false;
    }
    let mut seen = vec![false; g.nodes.len()];
    let mut todo = vec![e.a, e.b];
    while let Some(n) = todo.pop() {
        if std::mem::replace(&mut seen[n as usize], true) {
            continue;
        }
        let at = |h: &&Edge| h.a == n || h.b == n;
        if g.edges
            .iter()
            .filter(at)
            .any(|h| h.class == RoadClass::Highway)
        {
            return true;
        }
        // Only through the joints of a split ramp: nothing but ramp pieces meet there.
        if g.edges.iter().filter(at).all(ramp_like) {
            for h in g.edges.iter().filter(at) {
                todo.extend([h.a, h.b]);
            }
        }
    }
    false
}

/// A skeleton Place from the graded terrain, the zones, the graph and a player start at
/// the strip's center.
fn skeleton(m: &MacroMap, graded: &[f32], g: &RoadGraph) -> Place {
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

/// Edges in the largest connected component.
fn main_component(g: &RoadGraph) -> usize {
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
    count.into_iter().max().unwrap_or(0)
}

#[test]
fn merged_graphs_validate_climb_gently_and_stay_dry() {
    let t = template();
    for seed in 1..=8u64 {
        let (m, g) = generate(&t, seed);
        let plan = StreetPlan::new(Seed(seed), &t, &m, &g);
        let merged = plan.merged();
        let mut count = [0usize; 7];
        for e in &merged.edges {
            count[e.class as usize] += 1;
        }
        let main = main_component(&merged);
        println!(
            "seed {seed}: {} nodes, {} edges (local {}, alley {}, rural {}), main component {:.1}%",
            merged.nodes.len(),
            merged.edges.len(),
            count[RoadClass::Local as usize],
            count[RoadClass::Alley as usize],
            count[RoadClass::Rural as usize],
            100.0 * main as f32 / merged.edges.len() as f32
        );
        assert!(
            count[RoadClass::Local as usize] > 100,
            "seed {seed}: only {} local edges",
            count[RoadClass::Local as usize]
        );
        let place = skeleton(&m, plan.graded(), &merged);
        assert_eq!(place.validate(), Ok(()), "seed {seed}");
        assert!(
            main * 20 >= merged.edges.len() * 19,
            "seed {seed}: main component {main}"
        );

        let mut worst = 0.0f32;
        for (i, e) in merged.edges.iter().enumerate() {
            if is_ramp(&merged, e) {
                continue;
            }
            for w in e.profile.windows(2) {
                let run = ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
                if run < 1e-3 {
                    continue;
                }
                let grade = (w[1][1] - w[0][1]).abs() / run;
                if matches!(e.class, RoadClass::Local | RoadClass::Alley) {
                    worst = worst.max(grade);
                }
                assert!(
                    grade <= 0.12 + 1e-4,
                    "seed {seed} edge {i} ({:?}) grade {grade:.3} at {:?}",
                    e.class,
                    w[0]
                );
                let steps = (run / 4.0) as usize + 1;
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
        println!("  steepest street {:.1}%", worst * 100.0);
    }
}

/// Downtown lattices draw their blocks from the template, and the streets in the graph are
/// spaced by whole blocks.
#[test]
fn downtown_blocks_match_the_template() {
    let t = template();
    let ((l0, l1), (d0, d1)) = t.zones.downtown.block_m;
    let mut gaps_seen = 0;
    for seed in 1..=4u64 {
        let (m, g) = generate(&t, seed);
        let plan = StreetPlan::new(Seed(seed), &t, &m, &g);
        let merged = plan.merged();
        for cell in plan
            .grid_cells()
            .iter()
            .filter(|c| c.zone == ZoneId::Downtown)
        {
            let [len, dep] = cell.block_m;
            assert!(
                (l0..=l1).contains(&len) && (d0..=d1).contains(&dep),
                "seed {seed}: block {len}×{dep}"
            );
            let d = cell.dir;
            let n = [d[1], -d[0]];
            // Offsets of the local streets in this lattice, per family.
            let mut offs: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
            for e in merged.edges.iter().filter(|e| e.class == RoadClass::Local) {
                let (p, q) = (e.profile[0], e.profile[e.profile.len() - 1]);
                let mid = [(p[0] + q[0]) * 0.5, (p[2] + q[2]) * 0.5];
                if plan.comp_at(mid) != Some(cell.comp) {
                    continue;
                }
                let v = [q[0] - p[0], q[2] - p[2]];
                let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
                if l < 10.0 {
                    continue;
                }
                let along = (v[0] * d[0] + v[1] * d[1]) / l;
                let rel = [mid[0] - cell.origin[0], mid[1] - cell.origin[1]];
                if along.abs() > 0.999 {
                    offs[0].push(rel[0] * n[0] + rel[1] * n[1]);
                } else if along.abs() < 0.001 {
                    offs[1].push(rel[0] * d[0] + rel[1] * d[1]);
                }
            }
            for (k, sp) in [(0, dep), (1, len)] {
                let o = &mut offs[k];
                o.sort_by(|a, b| a.total_cmp(b));
                // Merged junctions move street ends a few meters; cluster within 6 m.
                o.dedup_by(|a, b| (*a - *b).abs() < 6.0);
                for w in o.windows(2) {
                    let gap = w[1] - w[0];
                    let blocks = (gap / sp).round();
                    assert!(
                        blocks >= 1.0 && (gap - blocks * sp).abs() < 6.0,
                        "seed {seed}: downtown streets {gap:.1} m apart, block {sp:.1} m"
                    );
                    gaps_seen += 1;
                }
            }
        }
    }
    println!("downtown: {gaps_seen} street spacings checked");
    assert!(gaps_seen >= 8, "only {gaps_seen} downtown spacings");
}

#[test]
fn seed_one_chunks_fit_the_budget() {
    let t = template();
    let (m, g) = generate(&t, 1);
    let start = Instant::now();
    let plan = StreetPlan::new(Seed(1), &t, &m, &g);
    let plan_ms = start.elapsed().as_secs_f64() * 1000.0;
    // Best of two per chunk, so a busy runner does not fail the budget.
    let mut times: Vec<f64> = Vec::new();
    for z in 0..i32::from(SIZE[1]) {
        for x in 0..i32::from(SIZE[0]) {
            let mut best = f64::INFINITY;
            for _ in 0..2 {
                let s = Instant::now();
                let c = plan.chunk([x, z]);
                best = best.min(s.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(c.chunk, [x, z]);
            }
            times.push(best);
        }
    }
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let max = sorted[sorted.len() - 1];
    println!(
        "streets seed 1: plan {plan_ms:.1} ms; per chunk median {:.3} ms, max {max:.3} ms (target <= 2 ms in debug)",
        sorted[sorted.len() / 2]
    );
    let chunks = chunks_of(&plan);
    let edges: usize = chunks.iter().map(|c| c.edges.len()).sum();
    assert!(edges > 0);
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(max <= 2.0, "slowest chunk took {max:.3} ms");
    }
}
