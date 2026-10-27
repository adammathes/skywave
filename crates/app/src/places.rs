//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, or a procedural
//! place built at load from its seed and its scenario's region template. A scenario names
//! its facade grammar ([`grammar_of`]; the test town uses the first scenario's,
//! [`grammar`]) and every grammar letters its shop signs from one lexicon.

use std::sync::OnceLock;

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;
use world::{Grammar, Palette};

use crate::scenario::Scenario;
use crate::scene::PlaceKey;

const CALIFORNIA_COAST: &str = include_str!("../../../assets/regions/california_coast.ron");
const CALIFORNIA_COAST_80S: &str = include_str!("../../../assets/grammar/california_coast_80s.ron");
const LEXICON: &str = include_str!("../../../assets/grammar/lexicon.ron");

/// The region templates compiled into the game, as `(id, RON text)`.
const REGIONS: [(&str, &str); 1] = [("california_coast", CALIFORNIA_COAST)];
/// The facade grammars compiled into the game, as `(file name, RON text)`.
const GRAMMARS: [(&str, &str); 1] = [("california_coast_80s.ron", CALIFORNIA_COAST_80S)];

/// The text of the built-in region template `id`.
pub fn region_text(id: &str) -> Option<&'static str> {
    REGIONS.iter().find(|(r, _)| *r == id).map(|(_, t)| *t)
}

/// The text of the built-in grammar file `file`.
pub fn grammar_text(file: &str) -> Option<&'static str> {
    GRAMMARS.iter().find(|(g, _)| *g == file).map(|(_, t)| *t)
}

/// The palette named `id`.
pub fn palette(id: &str) -> Option<Palette> {
    match id {
        "socal_1985" => Some(Palette::socal_1985()),
        _ => None,
    }
}

/// The palette scenario `s` paints its place with.
pub fn palette_of(s: &Scenario) -> Palette {
    palette(&s.palette).unwrap_or_else(Palette::socal_1985)
}

/// Parses a facade grammar (`assets/grammar/*.ron`) and the sign lexicon it names
/// (`assets/grammar/lexicon.ron`), attaches the lexicon and validates both.
pub fn load_grammar(text: &str, lexicon: &str) -> Result<Grammar, String> {
    let mut g: Grammar = ron::from_str(text).map_err(|e| format!("grammar: {e}"))?;
    g.lexicon = ron::from_str(lexicon).map_err(|e| format!("lexicon: {e}"))?;
    g.validate().map_err(|e| format!("grammar: {e}"))?;
    Ok(g)
}

/// The facade grammar the coast's buildings are planned by, parsed once. It is built in
/// and its tests parse it, so a failure here is a build that should not have shipped.
pub fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR
        .get_or_init(|| load_grammar(CALIFORNIA_COAST_80S, LEXICON).expect("the built-in grammar"))
}

/// The facade grammar scenario `s` names, parsed once (the coast's, [`grammar`], for a
/// file that is not built in, which [`Scenario::validate`] refuses).
pub fn grammar_of(s: &Scenario) -> &'static Grammar {
    static GRAMMARS_PARSED: OnceLock<Vec<Grammar>> = OnceLock::new();
    let all = GRAMMARS_PARSED.get_or_init(|| {
        GRAMMARS
            .iter()
            .map(|(_, text)| load_grammar(text, LEXICON).expect("the built-in grammars"))
            .collect()
    });
    GRAMMARS
        .iter()
        .position(|(g, _)| *g == s.grammar)
        .map_or_else(grammar, |i| &all[i])
}

/// The place `key` names for `seed`, in the first scenario.
#[cfg(test)]
pub fn build(key: PlaceKey, seed: Seed) -> Result<Place, String> {
    build_in(key, seed, crate::scenario::get(0))
}

/// The place `key` names for `seed`: the test town, or scenario `s`'s procedural place.
pub fn build_in(key: PlaceKey, seed: Seed, s: &Scenario) -> Result<Place, String> {
    match key {
        PlaceKey::TestTown => {
            // The test town's layout is fixed; the session seed drives its detailing.
            let mut place = Place::test_grid();
            place.header.seed = seed.0;
            Ok(place)
        }
        PlaceKey::Coast => {
            let text = region_text(&s.region).ok_or("no such region template")?;
            let t: RegionTemplate =
                ron::from_str(text).map_err(|e| format!("region template: {e}"))?;
            t.validate()
                .map_err(|e| format!("region template: {e:?}"))?;
            Ok(world::procgen::build_place(seed, &t, grammar_of(s)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_grammar_loads_and_round_trips() {
        let g = load_grammar(CALIFORNIA_COAST_80S, LEXICON).unwrap();
        assert_eq!(g.template, "california_coast");
        assert_eq!(g.era, place::Era::Eighties);
        assert_eq!(&g, grammar());
        let text = ron::ser::to_string(&g).unwrap();
        assert_eq!(load_grammar(&text, LEXICON).unwrap(), g);
    }

    #[test]
    fn scenarios_find_their_region_grammar_and_palette() {
        for s in crate::scenario::all() {
            assert!(region_text(&s.region).is_some());
            assert!(core::ptr::eq(grammar_of(s), grammar_of(s)), "parsed once");
            assert_eq!(grammar_of(s).template, s.region);
            assert!(palette(&s.palette).is_some());
        }
        // The first scenario's grammar is the coast's.
        assert_eq!(grammar_of(crate::scenario::get(0)), grammar());
        assert!(region_text("atlantis").is_none() && grammar_text("atlantis.ron").is_none());
    }

    #[test]
    fn broken_grammars_are_refused() {
        assert!(load_grammar("Grammar(", LEXICON).is_err());
        assert!(load_grammar(CALIFORNIA_COAST_80S, "Lexicon(").is_err());
        let mut g = grammar().clone();
        g.styles.clear();
        let text = ron::ser::to_string(&g).unwrap();
        let err = load_grammar(&text, LEXICON).unwrap_err();
        assert!(err.contains("StuccoBox"), "{err}");
        let mut lex = grammar().lexicon.clone();
        lex.id = "elsewhere".into();
        let lex = ron::ser::to_string(&lex).unwrap();
        let err = load_grammar(CALIFORNIA_COAST_80S, &lex).unwrap_err();
        assert!(err.contains("us_roadside"), "{err}");
    }
}
