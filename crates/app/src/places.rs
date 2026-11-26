//! The places a scene can be in ([`PlaceKey`]): the hand-laid test town, a procedural place
//! built at load from its seed and its scenario's region template (and authored layout, if
//! it has one), or the landmark gallery. A scenario names its facade grammar
//! ([`grammar_of`]; the test town uses the first scenario's, [`grammar`]); every grammar
//! letters its shop signs from one lexicon and carries the landmark kits
//! ([`builtin::landmarks`]).
//!
//! A release build bakes each world's place for [`builtin::BAKED_SEED`] at build time
//! (`build.rs`), so the default start decodes it instead of generating it; any other seed,
//! and every debug build, generates at load. Generation is deterministic, so the two are
//! the same place. Native builds carry the baked places in the binary; the web build ships
//! them as files beside the page, one per world, and the platform fetches a world's before
//! loading it and hands it over with [`provide_baked`].

use std::sync::{Mutex, OnceLock};

use place::Place;
use skywave_core::Seed;
use world::template::RegionTemplate;
use world::{ChunkPlan, Grammar, Palette};

use crate::builtin::{self, GRAMMARS};
use crate::scenario::Scenario;
use crate::scene::PlaceKey;

/// `(scenario id, encoded Place)` for every world, baked into the binary by `build.rs`
/// (empty in a debug build and on the web).
const BAKED: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/baked.rs"));

/// Baked places the platform fetched, by scenario id ([`provide_baked`]).
static PROVIDED: Mutex<Vec<(String, &'static [u8])>> = Mutex::new(Vec::new());

/// Hands over scenario `id`'s baked place (the file `build.rs` wrote as `<id>.place`), for
/// the next load of that world at [`builtin::BAKED_SEED`]. It is kept for the session.
pub fn provide_baked(id: &str, bytes: Vec<u8>) {
    let mut provided = PROVIDED.lock().unwrap_or_else(|e| e.into_inner());
    if !provided.iter().any(|(p, _)| p == id) {
        provided.push((id.to_owned(), Vec::leak(bytes)));
    }
}

/// Scenario `s`'s place for `seed` as the build baked it, if it is at hand.
fn baked(seed: Seed, s: &Scenario) -> Option<&'static [u8]> {
    if seed != builtin::BAKED_SEED {
        return None;
    }
    let provided = PROVIDED.lock().unwrap_or_else(|e| e.into_inner());
    BAKED
        .iter()
        .copied()
        .chain(provided.iter().map(|(id, b)| (id.as_str(), *b)))
        .find(|(id, _)| *id == s.id)
        .map(|(_, bytes)| bytes)
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
/// one's is made afresh (the same plan: the test `fresh_plans_are_the_planned_ones`).
pub fn load(key: PlaceKey, seed: Seed, s: &Scenario) -> Result<(Place, ChunkPlan), String> {
    if key == PlaceKey::Coast && baked(seed, s).is_none() {
        return generate_planned(seed, s);
    }
    let place = build_in(key, seed, s)?;
    let plan = ChunkPlan::new(&place, &palette_of(s), grammar_of(s));
    Ok((place, plan))
}

/// Scenario `s`'s procedural place for `seed`, generated, with the plan its detail pass
/// read brought up to date for [`palette_of`]`(s)`.
fn generate_planned(seed: Seed, s: &Scenario) -> Result<(Place, ChunkPlan), String> {
    let t = template_of(s)?;
    let pal = palette_of(s);
    Ok(match &s.layout {
        Some(id) => world::procgen::authored::build_place_planned(
            seed,
            &builtin::layout(id)?,
            &t,
            grammar_of(s),
            &pal,
        ),
        None => world::procgen::build_place_planned(seed, &t, grammar_of(s), &pal),
    })
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
            None => builtin::generate(seed, &s.region, s.layout.as_deref(), grammar_of(s)),
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
    use crate::builtin::{
        grammar_text, landmarks, load_builtin_grammar, load_grammar, parse_kit, region_text,
        CALIFORNIA_COAST_80S, LANDMARKS, LEXICON, SOCAL_BASIN_2001,
    };

    /// A plan made afresh from the finished place (a baked load) builds as the plan its
    /// detail pass read (a generated load), in every world, and the place survives its
    /// encoding.
    #[test]
    fn fresh_plans_are_the_planned_ones() {
        for s in crate::scenario::all() {
            let (p, plan) = generate_planned(Seed(1), s).unwrap();
            let (pal, g) = (palette_of(s), grammar_of(s));
            let fresh = ChunkPlan::new(&p, &pal, g);
            assert!(
                plan.far_terrain(&p, &pal) == fresh.far_terrain(&p, &pal),
                "{}",
                s.id
            );
            assert!(
                world::WorldCollider::with_plan(&p, g, &plan)
                    == world::WorldCollider::with_plan(&p, g, &fresh),
                "{}",
                s.id
            );
            assert_eq!(place::decode(&place::encode(&p)).unwrap(), p, "{}", s.id);
        }
    }

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
            // A world still being built, or one from an authored layout that has no grammar of
            // its own yet, may dress in another's grammar.
            if !s.stub && s.layout.is_none() {
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

    /// Faults the map audit (`world::audit`) still finds in each world, seed 1. A world's
    /// count may only go down: fix the map, then lower its number here. New worlds start at 0.
    const MAP_FAULTS: [(&str, usize); 5] = [
        ("socal_beach_1984", 41),
        ("mirage_valley_2001", 92),
        ("bay_loop_2005", 152),
        ("mirage_bowl_2001", 0),
        ("lake_city_1986", 0),
    ];

    /// The map audit over every built-in world: no road you cannot reach, no loop back to
    /// its own junction, no stub, and a dead end only where its zone's streets have them
    /// (cul-de-sacs, switchbacks, the pier road) or the layout says `dead_end`. Prints
    /// each fault and where it is (`cargo test -p app map_audit -- --nocapture`).
    #[test]
    fn every_world_passes_the_map_audit() {
        use world::template::StreetPattern::{Beachfront, Curvy, Switchback};
        let mut wrong = Vec::new();
        for s in crate::scenario::all() {
            let place = build_in(PlaceKey::Coast, Seed(1), s).expect("the world builds");
            let t = crate::builtin::template(&s.region).unwrap();
            let layout = s
                .layout
                .as_deref()
                .map(|id| crate::builtin::layout(id).unwrap());
            // Where the layout says a road ends, and the far end of a runway.
            let runway = layout
                .iter()
                .filter_map(|l| l.airfield.as_ref())
                .flat_map(|a| [a.runway.0, a.runway.1].map(|p| world::math::V2::new(p.0, p.1)));
            let ends: Vec<world::math::V2> = layout
                .iter()
                .flat_map(|l| {
                    l.roads
                        .iter()
                        .filter(|r| r.dead_end)
                        .map(|r| l.road_line(r))
                })
                .filter_map(|line| line?.last().copied())
                .chain(runway)
                .collect();
            let ok = |p: [f32; 2]| {
                let p = world::math::V2::new(p[0], p[1]);
                let rule = t.zones.get(world::zone_at(&place, p));
                rule.is_some_and(|r| matches!(r.streets, Curvy | Switchback | Beachfront))
                    || ends.iter().any(|e| (*e - p).length() < 60.0)
            };
            let found = world::audit::roads(&place, ok);
            for f in &found {
                eprintln!(
                    "{}: {:?} at ({:.0}, {:.0}): {:?} road, {:.0} m (?scenario={}&pos={:.0},{:.0})",
                    s.id, f.fault, f.at[0], f.at[1], f.class, f.len_m, s.id, f.at[0], f.at[1]
                );
            }
            let known = MAP_FAULTS.iter().find(|k| k.0 == s.id).map_or(0, |k| k.1);
            eprintln!("{}: {} map faults (pinned {known})", s.id, found.len());
            if found.len() != known {
                wrong.push((s.id.clone(), found.len(), known));
            }
        }
        assert!(
            wrong.is_empty(),
            "map faults (world, found, pinned in MAP_FAULTS): {wrong:?}; more: fix the map, \
             fewer: lower the pin"
        );
    }
}
