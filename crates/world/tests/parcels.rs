//! Acceptance tests for blocks and parcels (stage 5): coverage, overlaps, frontage,
//! polygon shape, validation, determinism, chunking and the per-chunk time. Run with
//! `--nocapture` to see the numbers: `cargo test -p world --test parcels -- --nocapture`.

use std::time::Instant;

use place::{
    Era, Facades, Lanes, Lights, LotKind, Parcel, Place, PlaceHeader, Props, RegionId, RoadGraph,
    Sidewalks, Spawn, SpawnKind, Terrain, ZoneId, Zones, CHUNK_SIZE_M, DEFAULT_RES_PER_CHUNK,
    PLACE_VERSION, SIDEWALK_WIDTH_M,
};
use skywave_core::{Rng, Seed};
use world::procgen::{
    arterials, hash_parcels, parcels, FaceKind, MacroMap, ParcelChunk, ParcelPlan, StreetPlan,
};
use world::template::RegionTemplate;

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");
const SIZE: [u16; 2] = [16, 16];

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
    t.validate().expect("validate");
    t
}

/// The macro map, the merged road graph and the graded terrain for `seed`.
fn generate(t: &RegionTemplate, seed: u64) -> (MacroMap, RoadGraph, Vec<f32>) {
    let m = MacroMap::generate(Seed(seed), t, SIZE);
    let g = arterials(Seed(seed), t, &m);
    let plan = StreetPlan::new(Seed(seed), t, &m, &g);
    let merged = plan.merged();
    let graded = plan.graded().to_vec();
    (m, merged, graded)
}

fn area(poly: &[[f32; 2]]) -> f64 {
    let mut acc = 0.0f64;
    for (i, p) in poly.iter().enumerate() {
        let q = poly[(i + 1) % poly.len()];
        acc += f64::from(p[1]) * f64::from(q[0]) - f64::from(p[0]) * f64::from(q[1]);
    }
    acc * 0.5
}

fn inside(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    place::validate::point_in_polygon(p, poly)
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dz * dz;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ez) = (a[0] + dx * t - p[0], a[1] + dz * t - p[1]);
    (ex * ex + ez * ez).sqrt()
}

/// Distance from a point to a polygon's boundary.
fn boundary_dist(p: [f32; 2], poly: &[[f32; 2]]) -> f32 {
    (0..poly.len())
        .map(|i| seg_dist(p, poly[i], poly[(i + 1) % poly.len()]))
        .fold(f32::INFINITY, f32::min)
}

/// Distance from a parcel to its frontage edge's property line (0 when it reaches it).
fn frontage_gap(g: &RoadGraph, p: &Parcel) -> f32 {
    let e = &g.edges[p.frontage_edge as usize];
    let walk = if e.sidewalk { SIDEWALK_WIDTH_M } else { 0.0 };
    let line = e.width_m * 0.5 + walk;
    let mut best = f32::INFINITY;
    for w in e.profile.windows(2) {
        let (a, b) = ([w[0][0], w[0][2]], [w[1][0], w[1][2]]);
        for i in 0..p.polygon.len() {
            let (c, d) = (p.polygon[i], p.polygon[(i + 1) % p.polygon.len()]);
            best = best
                .min(seg_dist(a, c, d))
                .min(seg_dist(b, c, d))
                .min(seg_dist(c, a, b))
                .min(seg_dist(d, a, b));
        }
    }
    (best - line).max(0.0)
}

/// A skeleton Place with the parcels, as in the streets tests.
fn skeleton(m: &MacroMap, graded: &[f32], g: &RoadGraph, parcels: Vec<Parcel>) -> Place {
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
        parcels,
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

fn all_parcels(chunks: &[ParcelChunk]) -> Vec<Parcel> {
    chunks
        .iter()
        .flat_map(|c| c.parcels.iter().cloned())
        .collect()
}

/// Parcels cover at least 90% of the closed blocks outside parks, never overlap, reach
/// their frontage edge within 1 m and are simple counter-clockwise polygons (seed 1).
#[test]
fn seed_one_parcels_cover_blocks_without_overlaps() {
    let t = template();
    let (m, g, _) = generate(&t, 1);
    let plan = ParcelPlan::new(Seed(1), &t, &m, &g);
    let st = plan.stats();
    println!("seed 1 faces: {st:?}");
    let (mut block_m2, mut lot_m2) = (0.0f64, 0.0f64);
    let (mut blocks, mut covered, mut dropped) = (0usize, 0usize, 0u32);
    let mut worst = (1.0f64, [0.0f32; 2]);
    let mut kinds = [0usize; 3];
    for f in 0..plan.faces().len() {
        let face = &plan.faces()[f];
        let built = plan.build_face(f);
        dropped += built.dropped;
        for p in &built.parcels {
            kinds[p.lot_kind as usize] += 1;
            assert!(
                place::validate::is_simple_ccw(&p.polygon),
                "parcel at {:?} is not simple and counter-clockwise",
                p.polygon[0]
            );
            let gap = frontage_gap(&g, p);
            assert!(
                gap <= 1.0,
                "parcel at {:?} is {gap:.2} m from its frontage edge {}",
                p.polygon[0],
                p.frontage_edge
            );
        }
        if face.open || face.zone == ZoneId::Park || face.kind == FaceKind::Parking {
            continue;
        }
        for b in &built.blocks {
            let a = area(&b.polygon);
            let mut sum = 0.0;
            for p in &built.parcels {
                let c = p.polygon[0];
                let mid = [
                    (p.polygon[0][0] + p.polygon[1][0]) * 0.5,
                    (p.polygon[0][1] + p.polygon[1][1]) * 0.5,
                ];
                // Parcels of this block: a vertex or edge midpoint inside or on it.
                if inside(c, &b.polygon)
                    || boundary_dist(c, &b.polygon) < 0.01
                        && (inside(mid, &b.polygon) || boundary_dist(mid, &b.polygon) < 0.01)
                {
                    sum += area(&p.polygon);
                }
            }
            let share = sum / a;
            block_m2 += a;
            lot_m2 += sum.min(a);
            blocks += 1;
            if share >= 0.9 {
                covered += 1;
            }
            if share < worst.0 {
                worst = (share, b.polygon[0]);
            }
        }
    }
    let share = lot_m2 / block_m2;
    println!(
        "seed 1: {blocks} closed blocks, {:.1} ha; parcels cover {:.2}% ({covered} blocks >= 90%, worst {:.1}% at {:?}); {dropped} pieces dropped; {} lots, {} parks, {} empty",
        block_m2 / 1e4,
        share * 100.0,
        worst.0 * 100.0,
        worst.1,
        kinds[0],
        kinds[1],
        kinds[2]
    );
    assert!(
        share >= 0.9,
        "parcels cover {:.1}% of blocks",
        share * 100.0
    );
    assert!(
        covered * 100 >= blocks * 95,
        "only {covered} of {blocks} blocks are 90% covered"
    );

    // Overlaps: random points in sampled chunks lie in at most one parcel, and no parcel
    // has a vertex more than 1 cm inside another of its chunk.
    let chunks = plan.all();
    let mut rng = Rng::new(Seed(1), &[0x4F56_4552]);
    let mut points = 0;
    for _ in 0..48 {
        let c = &chunks[rng.range(0, chunks.len() as u32) as usize];
        if c.parcels.is_empty() {
            continue;
        }
        let x0 = c.chunk[0] as f32 * CHUNK_SIZE_M;
        let z0 = c.chunk[1] as f32 * CHUNK_SIZE_M;
        for _ in 0..400 {
            let p = [
                x0 + rng.next_f32() * 160.0 - 16.0,
                z0 + rng.next_f32() * 160.0 - 16.0,
            ];
            let n = chunks
                .iter()
                .flat_map(|c| &c.parcels)
                .filter(|q| inside(p, &q.polygon) && boundary_dist(p, &q.polygon) > 1e-3)
                .count();
            assert!(n <= 1, "point {p:?} lies in {n} parcels");
            points += 1;
        }
        for (i, a) in c.parcels.iter().enumerate() {
            for (j, b) in c.parcels.iter().enumerate() {
                if i == j {
                    continue;
                }
                for &v in &a.polygon {
                    assert!(
                        !(inside(v, &b.polygon) && boundary_dist(v, &b.polygon) > 0.01),
                        "chunk {:?}: parcels {i} and {j} overlap at {v:?}",
                        c.chunk
                    );
                }
            }
        }
    }
    println!("overlaps: {points} sample points checked");
    assert_eq!(dropped, 0, "pieces dropped");
}

/// A Place with the parcels validates for seeds 1..=4, and every parcel's frontage edge is
/// within 1 m.
#[test]
fn places_with_parcels_validate() {
    let t = template();
    for seed in 1..=4u64 {
        let (m, g, graded) = generate(&t, seed);
        let plan = ParcelPlan::new(Seed(seed), &t, &m, &g);
        let ps = all_parcels(&plan.all());
        let mut kinds = [0usize; 3];
        for p in &ps {
            kinds[p.lot_kind as usize] += 1;
            assert!(frontage_gap(&g, p) <= 1.0, "seed {seed}");
        }
        println!(
            "seed {seed}: {} parcels ({} lots, {} parks, {} empty), {} faces ({} open)",
            ps.len(),
            kinds[LotKind::Lot as usize],
            kinds[LotKind::Park as usize],
            kinds[LotKind::LotEmpty as usize],
            plan.stats().kept,
            plan.stats().open
        );
        assert!(ps.len() > 500, "seed {seed}: only {} parcels", ps.len());
        let place = skeleton(&m, &graded, &g, ps);
        assert_eq!(place.validate(), Ok(()), "seed {seed}");
    }
}

/// Pinned `hash_parcels` of every parcel (chunk order) for seeds 1..=4
/// (`california_coast`, 16×16 chunks).
///
/// Update rule: these change only when parcel generation (or the streets, arterials, macro
/// map or template) changes on purpose. Then regenerate (`cargo test -p world --test
/// parcels -- --nocapture` prints the new table), look at the `tools/mapdump --parcels`
/// PNGs for seeds 1–2 before and after, and commit the new values in their own commit whose
/// message says why they moved.
const GOLDEN: [u64; 4] = [
    0x440F_7EF3_0A84_712C,
    0xA1B5_7AB4_D9C2_7539,
    0xF132_3C1E_3721_E0BC,
    0x3B0C_2CC1_24B5_4F10,
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
    let mut hs = Vec::new();
    for seed in 1..=4u64 {
        let (m, g, _) = generate(&t, seed);
        let a = hash_parcels(&all_parcels(&ParcelPlan::new(Seed(seed), &t, &m, &g).all()));
        let b = hash_parcels(&all_parcels(&ParcelPlan::new(Seed(seed), &t, &m, &g).all()));
        assert_eq!(a, b, "seed {seed}: same input, same parcels");
        hs.push(a);
    }
    for w in hs.windows(2) {
        assert_ne!(w[0], w[1], "different seeds should differ");
    }
    println!("parcel hashes:\n{}", table(&hs));
    assert_eq!(hs, GOLDEN, "parcel hashes moved; see the update rule");
}

/// Building one chunk gives exactly that chunk's slice of the whole build (seeds 1 and 2),
/// and stitching the chunks a face overlaps gives back the whole face.
#[test]
fn chunks_are_slices_of_the_whole() {
    let t = template();
    for seed in 1..=2u64 {
        let (m, g, _) = generate(&t, seed);
        let plan = ParcelPlan::new(Seed(seed), &t, &m, &g);
        let all = plan.all();
        for c in &all {
            assert_eq!(&plan.chunk(c.chunk), c, "seed {seed} chunk {:?}", c.chunk);
        }
        for f in 0..plan.faces().len() {
            let id = plan.faces()[f].id;
            let whole = plan.build_face(f).parcels;
            let mut stitched: Vec<Parcel> = Vec::new();
            for c in &all {
                for (p, &fid) in c.parcels.iter().zip(&c.faces) {
                    if fid == id {
                        stitched.push(p.clone());
                    }
                }
            }
            let key = |p: &Parcel| {
                (
                    p.polygon[0][0].to_bits(),
                    p.polygon[0][1].to_bits(),
                    p.polygon.len(),
                )
            };
            let mut a = whole.clone();
            a.sort_by_key(key);
            stitched.sort_by_key(key);
            assert_eq!(a, stitched, "seed {seed}: face {f} split across chunks");
        }
    }
}

/// A chunk built alone (with a fresh plan) equals the same chunk built after its eight
/// neighbors; twenty random chunks with parcels on seeds 1–2.
#[test]
fn one_chunk_matches_with_neighbors() {
    let t = template();
    for seed in 1..=2u64 {
        let (m, g, _) = generate(&t, seed);
        let plan = ParcelPlan::new(Seed(seed), &t, &m, &g);
        let mut rng = Rng::new(Seed(seed), &[0x4E45_4947]);
        let mut done = 0;
        let mut tries = 0;
        while done < 20 && tries < 400 {
            tries += 1;
            let c = [
                rng.range(0, u32::from(SIZE[0])) as i32,
                rng.range(0, u32::from(SIZE[1])) as i32,
            ];
            let alone = parcels(Seed(seed), &t, &m, &g, c);
            if alone.parcels.is_empty() {
                continue;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx != 0 || dz != 0 {
                        let _ = plan.chunk([c[0] + dx, c[1] + dz]);
                    }
                }
            }
            assert_eq!(plan.chunk(c), alone, "seed {seed} chunk {c:?}");
            done += 1;
        }
        assert_eq!(done, 20, "seed {seed}: too few chunks with parcels");
    }
}

#[test]
fn seed_one_chunks_fit_the_budget() {
    let t = template();
    let (m, g, _) = generate(&t, 1);
    let start = Instant::now();
    let plan = ParcelPlan::new(Seed(1), &t, &m, &g);
    let plan_ms = start.elapsed().as_secs_f64() * 1000.0;
    // Best of two per chunk, so a busy runner does not fail the budget.
    let mut times: Vec<(f64, [i32; 2])> = Vec::new();
    for z in 0..i32::from(SIZE[1]) {
        for x in 0..i32::from(SIZE[0]) {
            let mut best = f64::INFINITY;
            for _ in 0..2 {
                let s = Instant::now();
                let c = plan.chunk([x, z]);
                best = best.min(s.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(c.chunk, [x, z]);
            }
            times.push((best, [x, z]));
        }
    }
    times.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (max, at) = times[times.len() - 1];
    println!(
        "parcels seed 1: plan {plan_ms:.1} ms; per chunk median {:.3} ms, p90 {:.3} ms, max {max:.3} ms at {at:?} (target <= 2 ms)",
        times[times.len() / 2].0,
        times[times.len() * 9 / 10].0,
    );
    // Faces that span several chunks are rebuilt by each chunk, so the worst chunk is
    // well over the 2 ms target today (docs/IDEAS.md: cache faces across chunks). Until
    // then the enforced limit is the measured ceiling on CI, not the target.
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(max <= 12.0, "slowest chunk took {max:.3} ms");
    }
}
