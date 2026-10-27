//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, or a procedural
//! place built at load from its seed and region template. Both are dressed by one facade
//! grammar ([`grammar`]).

use std::sync::OnceLock;

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;
use world::Grammar;

use crate::scene::PlaceKey;

const CALIFORNIA_COAST: &str = include_str!("../../../assets/regions/california_coast.ron");
const CALIFORNIA_COAST_80S: &str = include_str!("../../../assets/grammar/california_coast_80s.ron");

/// Parses and validates a facade grammar (`assets/grammar/*.ron`).
pub fn load_grammar(text: &str) -> Result<Grammar, String> {
    let g: Grammar = ron::from_str(text).map_err(|e| format!("grammar: {e}"))?;
    g.validate().map_err(|e| format!("grammar: {e}"))?;
    Ok(g)
}

/// The facade grammar the buildings are planned by, parsed once. It is built in and its
/// tests parse it, so a failure here is a build that should not have shipped.
pub fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| load_grammar(CALIFORNIA_COAST_80S).expect("the built-in grammar"))
}

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
            Ok(world::procgen::build_place(seed, &t, grammar()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_grammar_loads_and_round_trips() {
        let g = load_grammar(CALIFORNIA_COAST_80S).unwrap();
        assert_eq!(g.template, "california_coast");
        assert_eq!(g.era, place::Era::Eighties);
        assert_eq!(&g, grammar());
        let text = ron::ser::to_string(&g).unwrap();
        assert_eq!(load_grammar(&text).unwrap(), g);
    }

    #[test]
    fn broken_grammars_are_refused() {
        assert!(load_grammar("Grammar(").is_err());
        let mut g = grammar().clone();
        g.styles.clear();
        let text = ron::ser::to_string(&g).unwrap();
        let err = load_grammar(&text).unwrap_err();
        assert!(err.contains("StuccoBox"), "{err}");
    }
}
