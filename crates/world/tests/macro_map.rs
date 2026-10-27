//! Acceptance tests for the macro map (stages 1–2): determinism, the zones seed 1 must
//! show, water below the water level, the strip next to the beach, height ranges and the
//! generation budget. Run with `--nocapture` to see timings and hashes:
//! `cargo test -p world --test macro_map -- --nocapture`.

use std::time::Instant;

use place::ZoneId;
use skywave_core::Seed;
use world::math::{signed_area, V2};
use world::procgen::{MacroMap, MacroPoiKind, WaterKind, ZONE_CELL_M};
use world::template::RegionTemplate;

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");
const GREAT_LAKES: &str = include_str!("../../../assets/regions/great_lakes_city.ron");
const NORTH_SHORE: &str = include_str!("../../../assets/regions/north_shore_suburb.ron");
const SIZE: [u16; 2] = [16, 16];

fn template(text: &str) -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(text).expect("parse");
    t.validate().expect("validate");
    t
}

fn california(seed: u64) -> MacroMap {
    MacroMap::generate(Seed(seed), &template(CALIFORNIA), SIZE)
}

/// Pinned `MacroMap::hash` of `california_coast` at 16×16 chunks for seeds 1..=8.
///
/// Update rule: these change only when macro generation (or the template) changes on
/// purpose. Then regenerate (`cargo test -p world --test macro_map -- --nocapture` prints
/// the new table), check the `tools/mapdump` PNGs for seeds 1–4 before and after, and
/// commit the new values in their own commit whose message says why they moved.
const GOLDEN: [u64; 8] = [
    0x807D_D128_42C5_A1A6,
    0x3071_05C7_9D6E_0A9D,
    0xC350_F560_6FEB_2F33,
    0x0D3B_2DCE_449F_43F4,
    0xC5BD_C624_6514_E21F,
    0x3CFF_32CF_BC5C_290D,
    0xD266_9F4F_AF61_9CB5,
    0xA918_031A_7A25_A794,
];

#[test]
fn hashes_are_deterministic_and_pinned() {
    let t = template(CALIFORNIA);
    let got: Vec<u64> = (1..=8)
        .map(|s| MacroMap::generate(Seed(s), &t, SIZE).hash())
        .collect();
    let again = MacroMap::generate(Seed(1), &t, SIZE);
    assert_eq!(again.hash(), got[0], "same seed, same map");
    assert_eq!(again, MacroMap::generate(Seed(1), &t, SIZE));
    for w in got.windows(2) {
        assert_ne!(w[0], w[1], "different seeds should differ");
    }
    let table: Vec<String> = got.iter().map(|h| format!("    0x{:016X},", h)).collect();
    let table = table
        .iter()
        .map(|l| format!("{}_{}_{}_{},", &l[..10], &l[10..14], &l[14..18], &l[18..22]))
        .collect::<Vec<_>>()
        .join("\n");
    println!("macro map hashes:\n{table}");
    assert_eq!(
        got, GOLDEN,
        "macro map hashes moved; see the update rule on GOLDEN"
    );
}

#[test]
fn seed_one_has_every_zone() {
    let m = california(1);
    let mut count = [0usize; 12];
    for &z in &m.zones {
        count[z as usize] += 1;
    }
    let required = [
        ZoneId::Water,
        ZoneId::Beach,
        ZoneId::Strip,
        ZoneId::Downtown,
        ZoneId::Commercial,
        ZoneId::Residential,
        ZoneId::Suburb,
        ZoneId::Hills,
        ZoneId::Farmland,
        ZoneId::Industrial,
        ZoneId::Park,
        ZoneId::Highway,
    ];
    for z in required {
        assert!(count[z as usize] > 0, "{z:?} missing: {count:?}");
    }
    // Ocean on the west: the west column is water between the headlands the walls of the
    // north and south edges raise out of it, the whole east column is not.
    let (zw, zh) = (m.zone_width as usize, m.zone_height as usize);
    let play = m.playable();
    let between = |cz: usize| (play[1]..=play[3]).contains(&(cz as f32 * ZONE_CELL_M + 8.0));
    assert!((0..zh)
        .filter(|&cz| between(cz))
        .all(|cz| m.zones[cz * zw] == ZoneId::Water));
    assert!((0..zh).all(|cz| m.zones[cz * zw + zw - 1] != ZoneId::Water));
    assert!(m.pass.is_some());
    assert!(m.pois.iter().any(|p| p.kind == MacroPoiKind::Pier));
    assert!(m.pois.iter().any(|p| p.kind == MacroPoiKind::Overlook));
}

#[test]
fn water_lies_below_the_water_level() {
    for seed in 1..=4 {
        let m = california(seed);
        let (w, zw) = (m.width as usize, m.zone_width as usize);
        for (i, &z) in m.zones.iter().enumerate() {
            if z != ZoneId::Water {
                continue;
            }
            let (cx, cz) = (i % zw, i / zw);
            let mut sum = 0.0;
            for j in 0..4 {
                for k in 0..4 {
                    sum += m.heights[(cz * 4 + j) * w + cx * 4 + k];
                }
            }
            assert!(sum / 16.0 < m.water_level, "seed {seed} water cell {i}");
        }
        // Every sample clearly out at sea is under water, the headlands at the ends of the
        // coast aside.
        let play = m.playable();
        for iz in 0..m.height as usize {
            for ix in 0..w {
                let (x, z) = (ix as f32 * 4.0, iz as f32 * 4.0);
                if m.coast_distance(x, z) < -4.0 && (play[1]..=play[3]).contains(&z) {
                    assert!(
                        m.heights[iz * w + ix] < m.water_level,
                        "seed {seed} ({x}, {z})"
                    );
                }
            }
        }
        for body in &m.water {
            let poly: Vec<V2> = body.polygon.iter().map(|&p| V2::from_array(p)).collect();
            assert!(
                signed_area(&poly) > 0.0,
                "seed {seed} {:?} not CCW",
                body.kind
            );
        }
        assert_eq!(m.water[0].kind, WaterKind::Ocean);
    }
}

#[test]
fn strip_runs_beside_the_beach() {
    let t = template(CALIFORNIA);
    // Strip cells start at most inland_m + width_m behind the beach band; allow one cell's
    // diagonal for the grid and the smoothed shoreline the strip is laid off.
    let reach = t.layout.strip.inland_m.1 + t.layout.strip.width_m.1 + 2.0 * ZONE_CELL_M;
    for seed in 1..=4 {
        let m = MacroMap::generate(Seed(seed), &t, SIZE);
        let zw = m.zone_width as usize;
        let center = |i: usize| {
            (
                (i % zw) as f32 * ZONE_CELL_M + 8.0,
                (i / zw) as f32 * ZONE_CELL_M + 8.0,
            )
        };
        let beach: Vec<(f32, f32)> = (0..m.zones.len())
            .filter(|&i| m.zones[i] == ZoneId::Beach)
            .map(center)
            .collect();
        let mut strip = 0;
        for i in (0..m.zones.len()).filter(|&i| m.zones[i] == ZoneId::Strip) {
            strip += 1;
            let (x, z) = center(i);
            let near = beach
                .iter()
                .map(|&(bx, bz)| ((bx - x) * (bx - x) + (bz - z) * (bz - z)).sqrt())
                .fold(f32::INFINITY, f32::min);
            assert!(
                near <= reach,
                "seed {seed}: strip cell ({x}, {z}) is {near} m from the beach"
            );
        }
        assert!(strip > 100, "seed {seed}: only {strip} strip cells");
        let len = m.strip.length_m;
        assert!(
            (800.0..=1500.0).contains(&len),
            "seed {seed}: strip {len} m"
        );
    }
}

#[test]
fn heights_are_finite_and_within_the_template() {
    for (text, hilly) in [
        (CALIFORNIA, true),
        (GREAT_LAKES, false),
        (NORTH_SHORE, false),
    ] {
        let t = template(text);
        let (lo, hi) = t.height_bounds();
        for seed in 1..=3 {
            let m = MacroMap::generate(Seed(seed), &t, SIZE);
            assert_eq!(m.heights.len(), 512 * 512);
            let mut max = f32::NEG_INFINITY;
            for &h in &m.heights {
                assert!(h.is_finite() && (lo..=hi).contains(&h), "{} {h}", t.name);
                max = max.max(h);
            }
            let top = t.elevation.water_level_m + t.elevation.ridge_height_m.0;
            if hilly {
                assert!(max >= top, "{} seed {seed}: hills only reach {max}", t.name);
                assert!(m.pass.is_some());
            } else {
                assert!(m.pass.is_none());
                // No hills or farms, but in the bands of the edge walls.
                let zw = m.zone_width as usize;
                assert!(m.zones.iter().enumerate().all(|(i, &z)| {
                    let (x, z_m) = ((i % zw) as f32 * 16.0 + 8.0, (i / zw) as f32 * 16.0 + 8.0);
                    !m.in_playable(x, z_m, 0.0) || (z != ZoneId::Hills && z != ZoneId::Farmland)
                }));
                // The lake is on the east edge.
                let mid = m.zone_height as usize / 2 * zw;
                assert_eq!(m.zones[mid + zw - 1], ZoneId::Water, "{}", t.name);
            }
        }
    }
}

#[test]
fn odd_sizes_do_not_panic() {
    let t = template(CALIFORNIA);
    for size in [[1, 1], [4, 4], [8, 24], [24, 8], [0, 3]] {
        let m = MacroMap::generate(Seed(5), &t, size);
        let (sx, sz) = (size[0].max(1) as usize, size[1].max(1) as usize);
        assert_eq!(m.heights.len(), sx * sz * 32 * 32);
        assert_eq!(m.zones.len(), sx * sz * 8 * 8);
        assert!(m.heights.iter().all(|h| h.is_finite()));
    }
}

#[test]
fn generation_fits_the_budget() {
    let t = template(CALIFORNIA);
    // Best of three, so a busy test runner does not fail the budget.
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let start = Instant::now();
        let m = MacroMap::generate(Seed(1), &t, SIZE);
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
        assert!(!m.heights.is_empty());
    }
    println!("macro map 16x16 chunks: {best:.1} ms (target < 50 ms in debug)");
    if std::env::var_os("SKYWAVE_ENFORCE_BUDGETS").is_some() {
        assert!(best < 150.0, "macro map took {best:.1} ms");
    }
}
