//! Lake City's hero landmark (`docs/WORLD_GENERATION.md`, "Hero landmarks"): on seeds 1-4 the
//! lake city (its authored layout) sites its drive-in hot dog stand on an arterial lot ahead of the start, close
//! enough to see from there, clear of every building, with its stall cars parked on its lot.
//! `cargo test -p world --release --test lake_city -- --nocapture` prints where it went.

use place::{Place, PoiKind, PropKind, SpawnKind};
use skywave_core::Seed;
use world::math::{point_in_polygon, V2};
use world::layout::Layout;
use world::procgen::authored::build_place;
use world::procgen::setpieces::HERO_AHEAD_M;
use world::template::RegionTemplate;

const GRAMMAR: &str = include_str!("../../../assets/grammar/great_lakes_80s.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon.ron");
const LAKE_CITY: &str = include_str!("../../../assets/regions/great_lakes_city.ron");
const LAYOUT: &str = include_str!("../../../assets/layouts/lake_city.ron");

/// The farthest the stand stands from the start, m: in sight down the road.
const IN_SIGHT_M: f32 = 700.0;

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

fn lake_city(seed: u64) -> Place {
    let t: RegionTemplate = ron::from_str(LAKE_CITY).expect("parse");
    t.validate().expect("validate");
    let l: Layout = ron::from_str(LAYOUT).expect("parse layout");
    l.validate().expect("validate layout");
    build_place(Seed(seed), &l, &t, &grammar())
}

#[test]
fn the_hot_dog_stand_stands_ahead_of_the_start() {
    let kit = world::landmarks::BUILTIN
        .iter()
        .position(|(id, _)| *id == "hot_dog_stand")
        .expect("the kit is built in") as u8;
    for seed in 1..=4 {
        let p = lake_city(seed);
        p.validate().expect("valid");
        let start = p
            .spawns
            .iter()
            .find(|s| s.kind == SpawnKind::PlayerStart)
            .expect("a start");
        let at = V2::new(start.pos[0], start.pos[2]);
        let (sin, cos) = world::math::sin_cos(start.heading);
        let ahead = V2::new(-sin, -cos);
        let stands: Vec<_> = p
            .pois
            .iter()
            .filter(|q| q.kind == PoiKind::HotDogStand)
            .collect();
        assert_eq!(stands.len(), 1, "seed {seed}: one stand");
        let mid = V2::new(stands[0].pos[0], stands[0].pos[1]);
        let d = mid - at;
        println!(
            "seed {seed}: stand {:.0} m from the start, {:.0} m ahead",
            d.length(),
            d.dot(ahead)
        );
        assert!(d.dot(ahead) >= HERO_AHEAD_M, "seed {seed}: ahead");
        assert!(d.length() <= IN_SIGHT_M, "seed {seed}: in sight");
        let marks: Vec<_> = p
            .props
            .items
            .iter()
            .filter(|q| q.kind == PropKind::Landmark { kit, variant: 0 })
            .collect();
        assert_eq!(marks.len(), 1, "seed {seed}: its landmark");
        // Its lot (40 x 46 m behind the front) holds no building.
        let front = V2::new(marks[0].pos[0], marks[0].pos[2]);
        let inward = (mid - front).normalize();
        let right = inward.right();
        let lot = [
            front - right * 20.0,
            front + right * 20.0,
            front + right * 20.0 + inward * 46.0,
            front - right * 20.0 + inward * 46.0,
        ];
        for b in &p.buildings {
            assert!(
                !b.footprint
                    .iter()
                    .any(|&q| point_in_polygon(V2::from_array(q), &lot)),
                "seed {seed}: a building on the stand's lot"
            );
        }
        let cars = p
            .props
            .items
            .iter()
            .filter(|q| matches!(q.kind, PropKind::ParkedCar { .. }))
            .filter(|q| point_in_polygon(V2::new(q.pos[0], q.pos[2]), &lot))
            .count();
        assert!(cars >= 14, "seed {seed}: its stalls hold {cars} cars");
    }
}
