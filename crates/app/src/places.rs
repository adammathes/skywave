//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, or a procedural
//! place built at load from its seed and region template.

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;

use crate::scene::PlaceKey;

const CALIFORNIA_COAST: &str = include_str!("../../../assets/regions/california_coast.ron");

/// The place `key` names for `seed`.
pub fn build(key: PlaceKey, seed: Seed) -> Result<Place, String> {
    match key {
        PlaceKey::TestTown => {
            // The test town's layout is fixed; the session seed drives its detailing.
            let mut place = Place::test_grid();
            place.header.seed = seed.0;
            Ok(place)
        }
        PlaceKey::Coast => {
            let t: RegionTemplate =
                ron::from_str(CALIFORNIA_COAST).map_err(|e| format!("region template: {e}"))?;
            t.validate()
                .map_err(|e| format!("region template: {e:?}"))?;
            Ok(world::procgen::build_place(seed, &t))
        }
    }
}
