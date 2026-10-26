//! The procedural pipeline end to end ([`build_place`]): every reference seed gives a
//! valid, deterministic skeleton Place with its start on a road. Run with `--nocapture`
//! to see the numbers: `cargo test -p world --test place -- --nocapture`.

use std::time::Instant;

use place::{RegionId, SpawnKind};
use skywave_core::Seed;
use world::procgen::build_place;
use world::template::RegionTemplate;

const CALIFORNIA: &str = include_str!("../../../assets/regions/california_coast.ron");

fn template() -> RegionTemplate {
    let t: RegionTemplate = ron::from_str(CALIFORNIA).expect("parse");
    t.validate().expect("validate");
    t
}

/// Pinned `Place::content_hash` of `build_place(seed, california_coast)` for seeds 1..=8.
///
/// Update rule: these move whenever any procedural stage (macro map, arterials, and later
/// streets, parcels and buildings) or the skeleton assembly changes. Regenerate with
/// `cargo test -p world --test place -- --nocapture` (it prints the table), check the
/// `?place=coast` screenshots, and commit the new values in their own commit whose
/// message says why they moved.
const GOLDEN: [u64; 8] = [
    0xA0F0_BBEB_2D7A_A686,
    0x66CE_8A10_8A94_E6E9,
    0x9E12_34CF_3A08_5D04,
    0x5B8F_D268_4165_4779,
    0x5CFE_5034_66FC_0643,
    0x6E81_583E_7862_CEC0,
    0x1982_A601_BC31_3A89,
    0x942B_77E7_007D_49D4,
];

#[test]
fn places_validate_and_are_deterministic() {
    let t = template();
    let mut got = Vec::new();
    for seed in 1..=8u64 {
        let start = Instant::now();
        let p = build_place(Seed(seed), &t);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(p.validate(), Ok(()), "seed {seed}");
        assert_eq!(p.header.region, RegionId::CaliforniaCoast);
        assert!(p.buildings.is_empty() && p.parcels.is_empty());
        let s = p.player_start().expect("a PlayerStart");
        assert_eq!(s.kind, SpawnKind::PlayerStart);
        let ground = p
            .terrain
            .height_at(&p.header, [s.pos[0], s.pos[2]])
            .unwrap();
        println!(
            "seed {seed}: {} nodes, {} edges, start ({:.0}, {:.1}, {:.0}) heading {:.2} over ground {ground:.1} m, {ms:.0} ms",
            p.roads.nodes.len(),
            p.roads.edges.len(),
            s.pos[0],
            s.pos[1],
            s.pos[2],
            s.heading
        );
        assert!((s.pos[1] - ground).abs() < 8.0, "seed {seed}");
        got.push(p.content_hash());
    }
    assert_eq!(build_place(Seed(1), &t).content_hash(), got[0]);
    let table = got
        .iter()
        .map(|h| {
            let l = format!("    0x{h:016X},");
            format!("{}_{}_{}_{},", &l[..10], &l[10..14], &l[14..18], &l[18..22])
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("place hashes:\n{table}");
    assert_eq!(
        got, GOLDEN,
        "place hashes moved; see the update rule on GOLDEN"
    );
}
