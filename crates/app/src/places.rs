//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, or a procedural
//! place built at load from its seed and region template.

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;

use crate::scene::PlaceKey;

const CALIFORNIA_COAST: &str = include_str!("../../../assets/regions/california_coast.ron");

/// Chunks merged per side into one render mesh on a procedural place, to keep draw calls
/// down (a 16×16-chunk place draws as 8×8 meshes plus the glow and lamp meshes).
pub const MERGE_CHUNKS: i32 = 2;

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

/// How many chunks per side go into one render mesh.
pub fn merge(key: PlaceKey) -> i32 {
    match key {
        PlaceKey::TestTown => 1,
        PlaceKey::Coast => MERGE_CHUNKS,
    }
}
