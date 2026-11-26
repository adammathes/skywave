//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, a procedural place
//! built at load from its seed and its scenario's region template, or the landmark gallery.
//! A scenario names its facade grammar ([`grammar_of`]; the test town uses the first
//! scenario's, [`grammar`]); every grammar letters its shop signs from one lexicon and
//! carries the landmark kits ([`landmarks`]).
//!
//! A release build bakes each world's place for [`builtin::BAKED_SEED`] at build time (`build.rs`),
//! so the default start decodes it instead of generating it; any other seed, and every
//! debug build, generates at load. Generation is deterministic, so the two are the same
//! place.

use std::sync::OnceLock;

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;
use world::{ChunkPlan, Grammar, Palette};

use crate::builtin::{self, GRAMMARS};
#[cfg(test)]
use crate::builtin::{
    grammar_text, landmarks, load_builtin_grammar, load_grammar, parse_kit, region_text,
    CALIFORNIA_COAST_80S, LANDMARKS, LEXICON, SOCAL_BASIN_2001,
};
use crate::scenario::Scenario;
use crate::scene::PlaceKey;

/// `(region template id, grammar file, encoded Place)` for every world, baked by `build.rs`
/// (empty in a debug build).
const BAKED: &[(&str, &str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/baked.rs"));

/// Scenario `s`'s place for `seed` as the build baked it, if it did.
fn baked(seed: Seed, s: &Scenario) -> Option<&'static [u8]> {
    if seed != builtin::BAKED_SEED {
        return None;
    }
    BAKED
        .iter()
        .find(|(r, g, _)| *r == s.region && *g == s.grammar)
        .map(|(_, _, bytes)| *bytes)
}

/// The palette named `id`.
pub fn palette(id: &str) -> Option<Palette> {
    Palette::named(id)
}

/// The palette scenario `s` paints its place with.
pub fn palette_of(s: &Scenario) -> Palette {
    palette(&s.palette).unwrap_or_else(Palette::socal_1985)
}

/// The facade grammar the coast's buildings are planned by, parsed once. It is built in
/// and its tests parse it, so a failure here is a build that should not have shipped.
pub fn grammar() -> &'static Grammar {
    static GRAMMAR: OnceLock<Grammar> = OnceLock::new();
    GRAMMAR.get_or_init(|| {
        builtin::load_grammar(builtin::CALIFORNIA_COAST_80S, builtin::LEXICON)
            .expect("the built-in grammar")
    })
}

/// The facade grammar scenario `s` names, parsed once (the coast's, [`grammar`], for a
/// file that is not built in, which [`Scenario::validate`] refuses).
pub fn grammar_of(s: &Scenario) -> &'static Grammar {
    static GRAMMARS_PARSED: OnceLock<Vec<Grammar>> = OnceLock::new();
    let all = GRAMMARS_PARSED.get_or_init(|| {
        GRAMMARS
            .iter()
            .map(|(_, text)| builtin::load_builtin_grammar(text).expect("the built-in grammars"))
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

/// [`build_in`] and the place's [`ChunkPlan`] for meshes painted with [`palette_of`]`(s)`,
/// made once: a generated procedural place's is the one its detail pass read, a baked
/// one's is made afresh (the same plan: `world`'s `a_planned_place_builds_as_a_fresh_plan`).
pub fn load(key: PlaceKey, seed: Seed, s: &Scenario) -> Result<(Place, ChunkPlan), String> {
    if key == PlaceKey::Coast && baked(seed, s).is_none() {
        let t = template_of(s)?;
        let pal = palette_of(s);
        return Ok(world::procgen::build_place_planned(
            seed,
            &t,
            grammar_of(s),
            &pal,
        ));
    }
    let place = build_in(key, seed, s)?;
    let plan = ChunkPlan::new(&place, &palette_of(s), grammar_of(s));
    Ok((place, plan))
}

/// Scenario `s`'s region template, checked.
fn template_of(s: &Scenario) -> Result<RegionTemplate, String> {
    builtin::template(&s.region)
}

/// The place `key` names for `seed`: the test town, scenario `s`'s procedural place, or the
/// landmark gallery dressed as `s`'s world.
pub fn build_in(key: PlaceKey, seed: Seed, s: &Scenario) -> Result<Place, String> {
    match key {
        PlaceKey::TestTown => {
            // The test town's layout is fixed; the session seed drives its detailing.
            let mut place = Place::test_grid();
            place.header.seed = seed.0;
            Ok(place)
        }
        PlaceKey::Coast => match baked(seed, s) {
            Some(bytes) => place::decode(bytes).map_err(|e| format!("baked place: {e:?}")),
            None => builtin::generate(seed, &s.region, grammar_of(s)),
        },
        PlaceKey::Gallery => {
            let t = template_of(s)?;
            let g = grammar_of(s);
            let region = world::procgen::build::region_of(&t);
            let pal = palette_of(s);
            Ok(world::landmarks::gallery(
                seed.0,
                &g.landmarks,
                region,
                g.era,
                &pal,
            ))
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
    fn the_basin_grammar_loads_with_its_own_lexicon() {
        let g = load_builtin_grammar(SOCAL_BASIN_2001).unwrap();
        assert_eq!(g.template, "socal_basin");
        assert_eq!(g.era, place::Era::Aughts);
        assert_eq!(g.lexicon.id, "us_basin");
        assert!(g.street.bus_stop_m > 0.0 && !g.signs.fascias.is_empty());
        // The coast's lexicon is not the basin's: it lacks its trades.
        assert!(load_grammar(SOCAL_BASIN_2001, LEXICON).is_err());
        let text = ron::ser::to_string(&g).unwrap();
        assert_eq!(load_builtin_grammar(&text).unwrap(), g);
        assert_eq!(
            load_builtin_grammar(CALIFORNIA_COAST_80S).unwrap(),
            *grammar()
        );
    }

    #[test]
    fn scenarios_find_their_region_grammar_and_palette() {
        for s in crate::scenario::all() {
            assert!(region_text(&s.region).is_some());
            assert!(core::ptr::eq(grammar_of(s), grammar_of(s)), "parsed once");
            // A world still being built may dress in another's grammar.
            if !s.stub {
                assert_eq!(grammar_of(s).template, s.region);
            }
            assert!(palette(&s.palette).is_some());
        }
        // The first scenario's grammar is the coast's.
        assert_eq!(grammar_of(crate::scenario::get(0)), grammar());
        assert!(region_text("atlantis").is_none() && grammar_text("atlantis.ron").is_none());
    }

    #[test]
    fn the_landmark_kits_load_and_round_trip() {
        let kits = landmarks();
        assert_eq!(kits.len(), LANDMARKS.len());
        for ((id, text), k) in LANDMARKS.iter().zip(kits) {
            assert_eq!(&k.id, id, "a kit's id is its file stem");
            let back = ron::ser::to_string(k).unwrap();
            assert_eq!(&parse_kit(&back).unwrap(), k);
            assert_eq!(&parse_kit(text).unwrap(), k);
        }
        // File-name order, so a kit's index stays put as kits are added.
        assert!(LANDMARKS.windows(2).all(|w| w[0].0 < w[1].0));
        for s in crate::scenario::all() {
            assert_eq!(grammar_of(s).landmarks, kits);
        }
        assert!(parse_kit("Kit(").is_err());
        let mut k = kits[0].clone();
        k.lot = [1.0, 1.0];
        let err = parse_kit(&ron::ser::to_string(&k).unwrap()).unwrap_err();
        assert!(err.contains(kits[0].id.as_str()), "{err}");
    }

    #[test]
    fn the_gallery_builds_in_every_world() {
        for s in crate::scenario::all() {
            let p = build_in(PlaceKey::Gallery, Seed(1), s).unwrap();
            assert_eq!(p.validate(), Ok(()));
            let n: usize = landmarks().iter().map(|k| k.variants.len()).sum();
            let marks = p
                .props
                .items
                .iter()
                .filter(|q| matches!(q.kind, place::PropKind::Landmark { .. }))
                .count();
            assert_eq!(marks, n);
            assert_eq!(p.header.era, grammar_of(s).era);
        }
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
